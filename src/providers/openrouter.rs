//! OpenRouter credit balance.
//!
//! OpenRouter sells dollars, not a renewing subscription window, so this
//! collector is shaped differently from Codex or Claude: there is no reset time
//! and no rolling period. What it reports is how much of the money on the
//! account is still there, expressed as a percentage so it renders in the same
//! sidebar row as everything else, with the dollar figures kept in the window
//! label where they are actually readable.
//!
//! Two endpoints are consulted because they answer different questions:
//! `/credits` is the account balance, and `/auth/key` is the spend limit on the
//! specific key in use, which may be lower. When the key carries a limit that
//! is the tighter and more meaningful number, so it wins.

use crate::cache::CacheStore;
use crate::model::{Provider, ProviderSnapshot, UsageWindow, WindowKind};
use crate::providers::ProviderError;
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

const CREDITS_URL: &str = "https://openrouter.ai/api/v1/credits";
const KEY_URL: &str = "https://openrouter.ai/api/v1/auth/key";

/// Where an OpenRouter key is looked for, in order.
///
/// The environment comes first because that is where OpenRouter's own docs and
/// every CLI that talks to it expect the key, and it is what Hermes records as
/// the source for its pooled OpenRouter credential. The file fallbacks exist so
/// the key does not have to be exported into the Herdr server's environment,
/// which is not something a user can do from inside Herdr.
pub fn api_key() -> std::result::Result<String, ProviderError> {
    for name in ["OPENROUTER_API_KEY", "OPENROUTER_KEY"] {
        if let Some(key) = std::env::var(name)
            .ok()
            .filter(|key| !key.trim().is_empty())
        {
            return Ok(key.trim().to_string());
        }
    }
    for path in key_files() {
        if let Ok(contents) = std::fs::read_to_string(&path) {
            let key = contents.trim();
            if !key.is_empty() {
                return Ok(key.to_string());
            }
        }
    }
    if let Ok(home) = crate::providers::hermes::hermes_home() {
        if let Some(key) = dotenv_value(&home.join(".env"), "OPENROUTER_API_KEY") {
            return Ok(key);
        }
    }
    Err(ProviderError::MissingCredentials)
}

/// Read one credential from Hermes' active `.env` without importing or copying
/// the rest of the profile. Hermes accepts `export`, plain, and quoted values.
fn dotenv_value(path: &Path, name: &str) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    for raw in contents.trim_start_matches('\u{feff}').lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let assignment = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = assignment.split_once('=') else {
            continue;
        };
        if key.trim() != name {
            continue;
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| {
                value
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
            })
            .unwrap_or(value)
            .trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

/// Candidate key files, most specific first.
fn key_files() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(explicit) = std::env::var_os("HERDR_AGENT_QUOTA_OPENROUTER_KEY_FILE") {
        paths.push(PathBuf::from(explicit));
    }
    // The plugin's own config directory: the one channel Herdr gives a user for
    // handing a plugin a value, and the same one install.ps1 writes.
    if let Some(config) = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR") {
        paths.push(PathBuf::from(config).join(crate::prefs::OPENROUTER_KEY));
    }
    if let Ok(home) = crate::platform::home_dir() {
        paths.push(home.join(".config").join("openrouter").join("key"));
        paths.push(home.join(".openrouter").join("key"));
    }
    paths
}

pub fn fetch() -> Result<ProviderSnapshot> {
    let key = api_key().map_err(anyhow::Error::from)?;
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(10))
        .timeout_write(Duration::from_secs(10))
        .build();

    let credits: Value = agent
        .get(CREDITS_URL)
        .set("Authorization", &format!("Bearer {key}"))
        .set("Accept", "application/json")
        .call()
        .map_err(|error| ProviderError::Request(http_error_status(&error)))
        .map_err(anyhow::Error::from)?
        .into_json()
        .context("decode OpenRouter credits response")?;

    // The key endpoint is advisory: an account with no per-key limit still has
    // a balance worth showing, so a failure here must not lose the balance.
    let key_info: Option<Value> = agent
        .get(KEY_URL)
        .set("Authorization", &format!("Bearer {key}"))
        .set("Accept", "application/json")
        .call()
        .ok()
        .and_then(|response| response.into_json().ok());

    let snapshot = parse(&credits, key_info.as_ref(), CacheStore::now_unix())?.with_account_id(
        Some(crate::model::private_identifier("credential", key.trim())),
    );
    Ok(snapshot)
}

/// Build the snapshot from the two documented payloads.
///
/// Split out from the request so the shapes are testable without a network.
pub fn parse(
    credits: &Value,
    key_info: Option<&Value>,
    fetched_at_unix: u64,
) -> Result<ProviderSnapshot> {
    let data = credits.get("data").unwrap_or(credits);
    let total = number(data, "total_credits");
    let used = number(data, "total_usage");

    // A key-scoped spend limit binds before the account balance does, so it is
    // preferred whenever the account actually set one. `limit` is null for an
    // unlimited key, which is the common case.
    let key_data = key_info.map(|value| value.get("data").unwrap_or(value));
    let key_limit = key_data.and_then(|data| number(data, "limit"));
    let key_used = key_data.and_then(|data| number(data, "usage"));

    let (limit, spent, label) = match (key_limit, key_used) {
        (Some(limit), Some(spent)) if limit > 0.0 => (limit, spent, "key"),
        _ => match (total, used) {
            (Some(total), Some(used)) if total > 0.0 => (total, used, "credits"),
            _ => {
                return Err(anyhow::Error::from(ProviderError::UnsupportedResponse(
                    "OpenRouter reported no credit total".to_string(),
                )))
            }
        },
    };

    let remaining = (limit - spent).max(0.0);
    let used_percent = ((spent / limit) * 100.0).clamp(0.0, 100.0);
    // No reset: OpenRouter dollars are bought, not granted per period. Passing
    // `None` is what keeps a reset ETA off the row rather than inventing one.
    let window = UsageWindow::new(WindowKind::Monthly, used_percent, None)
        .map_err(anyhow::Error::from)?
        .with_source_window(format!("{label} {}", money(remaining)), None);

    let mut snapshot = ProviderSnapshot::new(Provider::OpenRouter, vec![window], fetched_at_unix);
    // The free-tier flag is the one piece of account shape worth surfacing:
    // a free-tier key's balance behaves nothing like a funded one.
    if key_data
        .and_then(|data| data.get("is_free_tier").and_then(Value::as_bool))
        .unwrap_or(false)
    {
        snapshot.model = Some("free tier".to_string());
    }
    Ok(snapshot)
}

/// Render a dollar amount at the precision a sidebar can afford.
///
/// Cents matter near zero and are noise at three figures, so the scale decides.
fn money(amount: f64) -> String {
    if amount >= 100.0 {
        format!("${amount:.0}")
    } else if amount >= 10.0 {
        format!("${amount:.1}")
    } else {
        format!("${amount:.2}")
    }
}

fn number(value: &Value, key: &str) -> Option<f64> {
    value
        .get(key)
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite())
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

    #[test]
    fn reports_the_account_balance_when_the_key_is_unlimited() {
        let credits = json!({"data": {"total_credits": 50.0, "total_usage": 12.5}});
        let key = json!({"data": {"limit": null, "usage": 12.5, "is_free_tier": false}});
        let snapshot = parse(&credits, Some(&key), 0).unwrap();
        let window = &snapshot.windows[0];
        assert_eq!(window.used_percent, 25.0);
        assert_eq!(window.remaining_percent, 75.0);
        // $37.50 is in the one-decimal band; see `dollar_precision_follows_the_scale`.
        assert_eq!(window.source_label.as_deref(), Some("credits $37.5"));
        // Bought dollars do not renew, so a reset would be a fabrication.
        assert!(window.resets_at.is_none());
    }

    #[test]
    fn a_key_limit_binds_before_the_account_balance() {
        // $500 on the account but a $10 key: the key is what runs out first.
        let credits = json!({"data": {"total_credits": 500.0, "total_usage": 4.0}});
        let key = json!({"data": {"limit": 10.0, "usage": 4.0}});
        let snapshot = parse(&credits, Some(&key), 0).unwrap();
        assert_eq!(snapshot.windows[0].used_percent, 40.0);
        assert_eq!(
            snapshot.windows[0].source_label.as_deref(),
            Some("key $6.00")
        );
    }

    #[test]
    fn a_missing_key_endpoint_still_yields_the_balance() {
        let credits = json!({"data": {"total_credits": 20.0, "total_usage": 5.0}});
        let snapshot = parse(&credits, None, 0).unwrap();
        assert_eq!(snapshot.windows[0].used_percent, 25.0);
    }

    #[test]
    fn an_account_with_no_total_is_unsupported_rather_than_zero() {
        // Reporting 0% used for an unknown total would read as "plenty left".
        let credits = json!({"data": {"total_usage": 5.0}});
        assert!(parse(&credits, None, 0).is_err());
        let zero_total = json!({"data": {"total_credits": 0.0, "total_usage": 0.0}});
        assert!(parse(&zero_total, None, 0).is_err());
    }

    #[test]
    fn free_tier_is_recorded() {
        let credits = json!({"data": {"total_credits": 1.0, "total_usage": 0.0}});
        let key = json!({"data": {"is_free_tier": true}});
        assert_eq!(
            parse(&credits, Some(&key), 0).unwrap().model.as_deref(),
            Some("free tier")
        );
    }

    #[test]
    fn dollar_precision_follows_the_scale() {
        assert_eq!(money(4.567), "$4.57");
        assert_eq!(money(45.67), "$45.7");
        assert_eq!(money(456.7), "$457");
    }

    #[test]
    fn reads_the_key_from_hermes_dotenv_syntax() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(".env");
        std::fs::write(
            &path,
            "# profile credentials\nexport OPENROUTER_API_KEY=\"from-hermes\"\n",
        )
        .unwrap();
        assert_eq!(
            dotenv_value(&path, "OPENROUTER_API_KEY").as_deref(),
            Some("from-hermes")
        );
    }
}
