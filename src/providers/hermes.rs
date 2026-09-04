//! Hermes account dollars, by way of Nous Portal.
//!
//! Hermes is a harness rather than a subscription: it routes to whichever
//! provider it is configured for. What it does own is a Nous Portal account,
//! and that account is what Hermes' own `/usage` surface reports, so it is what
//! this collector reads too — from the same endpoint, with the same token, so
//! the sidebar and Hermes itself can never disagree.
//!
//! Two magnitudes matter and they behave differently, which is why they get
//! separate windows rather than one blended number: plan dollars renew at the
//! end of the billing period, and purchased top-up dollars roll over and never
//! reset. Collapsing them would make a full top-up balance look like a fresh
//! month.

use crate::cache::CacheStore;
use crate::model::{Provider, ProviderSnapshot, ResetAt, UsageWindow, WindowKind};
use crate::providers::ProviderError;
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_PORTAL_URL: &str = "https://portal.nousresearch.com";
const ACCOUNT_PATH: &str = "/api/oauth/account";

#[derive(Debug, Clone)]
pub struct HermesCredentials {
    pub access_token: String,
    pub portal_base_url: String,
    /// Portal account identity, used to drop another account's cached dollars
    /// after a re-login. Absent when the token carries no readable subject.
    pub account_id: Option<String>,
}

/// Hermes' home directory.
///
/// `HERMES_HOME` is what the Hermes launcher exports and is authoritative when
/// present; the dotfile is the default install.
pub fn hermes_home() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("HERMES_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(home));
    }
    Ok(crate::platform::home_dir()?.join(".hermes"))
}

pub fn auth_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("HERMES_AUTH_FILE") {
        return Ok(PathBuf::from(path));
    }
    Ok(hermes_home()?.join("auth.json"))
}

/// Read the Nous credentials Hermes stored for itself.
///
/// Only the fields needed to ask the Portal about the account are taken; the
/// refresh token and the pooled provider secrets are deliberately not read.
pub fn read_credentials(
    path: &std::path::Path,
) -> std::result::Result<HermesCredentials, ProviderError> {
    let bytes = std::fs::read(path).map_err(|_| ProviderError::MissingCredentials)?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| ProviderError::MissingCredentials)?;
    let nous = value
        .pointer("/providers/nous")
        .ok_or(ProviderError::MissingCredentials)?;
    let access_token = nous
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|token| !token.trim().is_empty())
        .ok_or(ProviderError::MissingCredentials)?
        .to_string();
    let portal_base_url = nous
        .get("portal_base_url")
        .and_then(Value::as_str)
        .filter(|url| !url.trim().is_empty())
        .unwrap_or(DEFAULT_PORTAL_URL)
        .trim_end_matches('/')
        .to_string();
    let account_id = account_id_from_token(&access_token);
    Ok(HermesCredentials {
        access_token,
        portal_base_url,
        account_id,
    })
}

/// Organisation id from the access token's payload.
///
/// The token is read only to label which account the cached dollars belong to.
/// It is never logged and never leaves the process; the claim is decoded rather
/// than the token stored.
fn account_id_from_token(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let decoded = base64url_decode(payload)?;
    let value: Value = serde_json::from_slice(&decoded).ok()?;
    value
        .get("org_id")
        .or_else(|| value.get("sub"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Minimal base64url decoder.
///
/// A JWT payload is base64url without padding. Pulling in a codec crate for
/// one 200-byte claim would be the larger change.
fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut accumulator: u32 = 0;
    let mut bits = 0u32;
    for byte in input.bytes() {
        if byte == b'=' {
            break;
        }
        let value = TABLE.iter().position(|candidate| *candidate == byte)? as u32;
        accumulator = (accumulator << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((accumulator >> bits) as u8);
        }
    }
    Some(output)
}

pub fn fetch() -> Result<ProviderSnapshot> {
    let path = auth_path().context("resolve Hermes auth path")?;
    let credentials = read_credentials(&path).map_err(anyhow::Error::from)?;
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(10))
        .timeout_write(Duration::from_secs(10))
        .build();
    let value: Value = agent
        .get(&format!("{}{ACCOUNT_PATH}", credentials.portal_base_url))
        .set(
            "Authorization",
            &format!("Bearer {}", credentials.access_token),
        )
        .set("Accept", "application/json")
        .call()
        .map_err(|error| ProviderError::Request(http_error_status(&error)))
        .map_err(anyhow::Error::from)?
        .into_json()
        .context("decode Nous Portal account response")?;
    let snapshot = parse(&value, CacheStore::now_unix())?;
    Ok(snapshot.with_account_id(credentials.account_id))
}

/// Build the snapshot from a Portal account payload.
///
/// The fields are located by name anywhere in the document rather than at a
/// fixed path: the Portal nests them under `paid_service_access_info` today,
/// and Hermes' own reader is equally tolerant because that nesting has moved
/// before. A field that is absent simply produces one fewer window.
pub fn parse(account: &Value, fetched_at_unix: u64) -> Result<ProviderSnapshot> {
    let plan_remaining = find_number(account, "subscription_credits_remaining");
    let plan_allowance = find_number(account, "monthly_credits");
    let topup_remaining = find_number(account, "purchased_credits_remaining");
    let total_usable = find_number(account, "total_usable_credits");
    let renews_at = find_string(account, "current_period_end").and_then(parse_reset);

    let mut windows = Vec::new();

    // Plan dollars: spent against this month's allowance, and they renew.
    if let (Some(remaining), Some(allowance)) = (plan_remaining, plan_allowance) {
        if allowance > 0.0 {
            let spent = (allowance - remaining).clamp(0.0, allowance);
            let used_percent = ((spent / allowance) * 100.0).clamp(0.0, 100.0);
            windows.push(
                UsageWindow::new(WindowKind::Monthly, used_percent, renews_at)
                    .map_err(anyhow::Error::from)?
                    .with_source_window(format!("plan {}", money(remaining)), None),
            );
        }
    }

    // Top-up dollars: bought, roll over, never reset. Only publish a percentage
    // when Portal supplies a positive total that can truthfully contain this
    // balance; otherwise a valid plan window stands on its own.
    if let (Some(remaining), Some(denominator)) = (
        topup_remaining.filter(|amount| *amount > 0.0),
        total_usable.filter(|total| *total > 0.0),
    ) {
        if remaining <= denominator {
            let used_percent = (100.0 - (remaining / denominator) * 100.0).clamp(0.0, 100.0);
            windows.push(
                UsageWindow::new(WindowKind::Weekly, used_percent, None)
                    .map_err(anyhow::Error::from)?
                    .with_source_window(format!("top-up {}", money(remaining)), None),
            );
        }
    }

    if windows.is_empty() {
        return Err(anyhow::Error::from(ProviderError::UnsupportedResponse(
            "Nous Portal reported no spendable balance".to_string(),
        )));
    }

    let mut snapshot = ProviderSnapshot::new(Provider::Hermes, windows, fetched_at_unix);
    snapshot.model = find_string(account, "plan_name")
        .or_else(|| find_string(account, "product_name"))
        .map(str::to_string);
    Ok(snapshot)
}

fn parse_reset(value: &str) -> Option<ResetAt> {
    ResetAt::parse_rfc3339(value)
        .or_else(|| value.parse::<u64>().ok().map(ResetAt::from_unix_seconds))
}

/// Find a numeric field by name anywhere in the document.
fn find_number(value: &Value, name: &str) -> Option<f64> {
    match value {
        Value::Object(map) => map
            .get(name)
            .and_then(Value::as_f64)
            .filter(|number| number.is_finite())
            .or_else(|| map.values().find_map(|child| find_number(child, name))),
        Value::Array(values) => values.iter().find_map(|child| find_number(child, name)),
        _ => None,
    }
}

fn find_string<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    match value {
        Value::Object(map) => map
            .get(name)
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .or_else(|| map.values().find_map(|child| find_string(child, name))),
        Value::Array(values) => values.iter().find_map(|child| find_string(child, name)),
        _ => None,
    }
}

fn money(amount: f64) -> String {
    if amount >= 100.0 {
        format!("${amount:.0}")
    } else if amount >= 10.0 {
        format!("${amount:.1}")
    } else {
        format!("${amount:.2}")
    }
}

fn http_error_status(error: &ureq::Error) -> String {
    match error {
        ureq::Error::Status(code, _) => format!("HTTP {code}"),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn account() -> Value {
        json!({
            "subscription": {"monthly_credits": 20.0, "plan_name": "Pro"},
            "paid_service_access_info": {
                "subscription_credits_remaining": 15.0,
                "purchased_credits_remaining": 8.0,
                "total_usable_credits": 23.0
            },
            "current_period_end": "2026-10-01T00:00:00Z"
        })
    }

    #[test]
    fn plan_and_topup_are_separate_windows() {
        let snapshot = parse(&account(), 0).unwrap();
        assert_eq!(snapshot.windows.len(), 2);
        let plan = &snapshot.windows[0];
        assert_eq!(plan.used_percent, 25.0);
        assert_eq!(plan.source_label.as_deref(), Some("plan $15.0"));
        // Plan dollars renew; the ETA is real.
        assert!(plan.resets_at.is_some());
    }

    #[test]
    fn topup_never_carries_a_reset() {
        // Purchased dollars roll over. A reset ETA on them would be a lie.
        let snapshot = parse(&account(), 0).unwrap();
        let topup = &snapshot.windows[1];
        assert!(topup.resets_at.is_none());
        assert_eq!(topup.source_label.as_deref(), Some("top-up $8.00"));
        assert!((topup.used_percent - 65.217_391_304_347_83).abs() < 1e-9);
    }

    #[test]
    fn topup_without_a_denominator_is_omitted_when_the_plan_is_valid() {
        let account = json!({
            "monthly_credits": 20.0,
            "subscription_credits_remaining": 15.0,
            "purchased_credits_remaining": 8.0
        });
        let snapshot = parse(&account, 0).unwrap();
        assert_eq!(snapshot.windows.len(), 1);
        assert_eq!(snapshot.windows[0].kind, WindowKind::Monthly);
    }

    #[test]
    fn an_unverifiable_topup_is_an_error_when_no_plan_remains() {
        for account in [
            json!({"purchased_credits_remaining": 8.0}),
            json!({
                "purchased_credits_remaining": 8.0,
                "total_usable_credits": 4.0
            }),
        ] {
            assert!(parse(&account, 0).is_err());
        }
    }

    #[test]
    fn fields_are_found_regardless_of_nesting() {
        // The Portal has moved these under different parents before.
        let flat = json!({
            "monthly_credits": 10.0,
            "subscription_credits_remaining": 2.5
        });
        let snapshot = parse(&flat, 0).unwrap();
        assert_eq!(snapshot.windows[0].used_percent, 75.0);
    }

    #[test]
    fn an_account_with_no_balance_is_an_error_not_a_full_bar() {
        assert!(parse(&json!({"subscription": {}}), 0).is_err());
    }

    #[test]
    fn the_org_claim_identifies_the_account() {
        // {"org_id":"nas_organisation:abc"} base64url, unpadded.
        let payload = "eyJvcmdfaWQiOiJuYXNfb3JnYW5pc2F0aW9uOmFiYyJ9";
        let token = format!("header.{payload}.signature");
        assert_eq!(
            account_id_from_token(&token).as_deref(),
            Some("nas_organisation:abc")
        );
    }

    #[test]
    fn a_malformed_token_yields_no_account_rather_than_panicking() {
        assert!(account_id_from_token("not-a-jwt").is_none());
        assert!(account_id_from_token("a.!!!.c").is_none());
    }
}
