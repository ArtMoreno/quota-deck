mod support;

use herdr_agent_quota::configure::{agy, claude};
use serde_json::Value;
use std::fs;
use std::process::Command;
use support::{output_with_deadline, COMMAND_BUDGET};
use tempfile::tempdir;

#[test]
fn agy_setup_is_idempotent_and_restores_the_previous_statusline() {
    let directory = tempdir().unwrap();
    let settings = directory.path().join("settings.json");
    let state = directory.path().join("state");
    let executable = directory.path().join("herdr-agent-quota");
    fs::write(
        &settings,
        r#"{"theme":"dark","statusLine":{"type":"command","command":"echo old","refreshInterval":5}}"#,
    )
    .unwrap();

    agy::apply_at(&settings, &state, &executable).unwrap();
    let once = fs::read(&settings).unwrap();
    agy::apply_at(&settings, &state, &executable).unwrap();
    assert_eq!(fs::read(&settings).unwrap(), once);

    let installed: Value = serde_json::from_slice(&once).unwrap();
    assert_eq!(installed["theme"], "dark");
    assert_eq!(installed["statusLine"]["refreshInterval"], 5);
    let command = installed["statusLine"]["command"].as_str().unwrap();
    assert!(command.contains("agy-statusline"));
    assert!(command.contains(state.to_str().unwrap()));

    agy::uninstall_at(&settings, &state).unwrap();
    let restored: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert_eq!(restored["statusLine"]["command"], "echo old");
    assert_eq!(restored["statusLine"]["refreshInterval"], 5);
}

#[test]
fn uninstall_restores_the_exact_original_settings_bytes() {
    let directory = tempdir().unwrap();
    let settings = directory.path().join("settings.json");
    let state = directory.path().join("state");
    let executable = directory.path().join("herdr-agent-quota");
    let original =
        b"{\"theme\":\"dark\",\"statusLine\":{\"type\":\"command\",\"command\":\"echo old\"}}\r\n";
    fs::write(&settings, original).unwrap();

    claude::apply_at(&settings, &state, &executable).unwrap();
    claude::uninstall_at(&settings, &state).unwrap();

    assert_eq!(fs::read(&settings).unwrap(), original);
}

#[test]
fn full_settings_backup_stays_beside_the_source_and_state_keeps_only_a_digest() {
    let directory = tempdir().unwrap();
    let settings = directory.path().join("settings.json");
    let state = directory.path().join("state");
    let executable = directory.path().join("herdr-agent-quota");
    let original = br#"{"privateCommand":"example only","statusLine":null}"#;
    fs::write(&settings, original).unwrap();

    claude::apply_at(&settings, &state, &executable).unwrap();
    let adjacent = directory
        .path()
        .join("settings.json.herdr-agent-quota.backup");
    assert_eq!(fs::read(&adjacent).unwrap(), original);
    let state_files = fs::read_dir(&state)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert!(state_files
        .iter()
        .all(|path| fs::read(path).unwrap() != original));
    let installed = state_files
        .iter()
        .find(|path| path.to_string_lossy().ends_with(".installed"))
        .expect("installed digest");
    assert_eq!(fs::read(installed).unwrap().len(), 32);

    claude::uninstall_at(&settings, &state).unwrap();
    assert!(!adjacent.exists());
    assert_eq!(fs::read(settings).unwrap(), original);
}

#[test]
fn uninstall_preserves_settings_edited_while_installed() {
    let directory = tempdir().unwrap();
    let settings = directory.path().join("settings.json");
    let state = directory.path().join("state");
    let executable = directory.path().join("herdr-agent-quota");
    fs::write(
        &settings,
        r#"{"statusLine":{"type":"command","command":"echo old"}}"#,
    )
    .unwrap();

    claude::apply_at(&settings, &state, &executable).unwrap();
    let mut edited: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    edited["userEdit"] = Value::String("keep me".to_string());
    fs::write(&settings, serde_json::to_vec_pretty(&edited).unwrap()).unwrap();
    claude::uninstall_at(&settings, &state).unwrap();

    let restored: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert_eq!(restored["userEdit"], "keep me");
    assert_eq!(restored["statusLine"]["command"], "echo old");
}

#[test]
fn uninstall_restores_an_absent_settings_file() {
    let directory = tempdir().unwrap();
    let settings = directory.path().join("settings.json");
    let state = directory.path().join("state");
    let executable = directory.path().join("herdr-agent-quota");

    claude::apply_at(&settings, &state, &executable).unwrap();
    assert!(settings.exists());
    claude::uninstall_at(&settings, &state).unwrap();

    assert!(!settings.exists());
}

#[test]
fn old_plugin_wrappers_are_repaired_without_becoming_the_backup() {
    let directory = tempdir().unwrap();
    let settings = directory.path().join("settings.json");
    let state = directory.path().join("state");
    let executable = directory.path().join("herdr-agent-quota");
    fs::write(
        &settings,
        r#"{"statusLine":{"type":"command","command":"HERDR_PLUGIN_STATE_DIR='/wrong' '/old/herdr-agent-quota' claude-statusline"}}"#,
    )
    .unwrap();

    claude::apply_at(&settings, &state, &executable).unwrap();
    let installed: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    let command = installed["statusLine"]["command"].as_str().unwrap();
    assert!(command.contains(state.to_str().unwrap()));
    assert!(!command.contains("/wrong"));

    claude::uninstall_at(&settings, &state).unwrap();
    let restored: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert!(restored.get("statusLine").is_none());
}

#[test]
fn claude_statusline_refreshes_rate_limits_with_the_configured_interval() {
    let directory = tempdir().unwrap();
    let settings = directory.path().join("settings.json");
    let state = directory.path().join("state");
    let executable = directory.path().join("herdr-agent-quota");
    fs::write(
        &settings,
        r#"{"statusLine":{"type":"command","command":"echo old"}}"#,
    )
    .unwrap();

    claude::apply_at(&settings, &state, &executable).unwrap();
    let installed: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert_eq!(installed["statusLine"]["refreshInterval"], 60);

    claude::apply_at_with_refresh_interval(&settings, &state, &executable, 300).unwrap();
    let customized: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert_eq!(customized["statusLine"]["refreshInterval"], 300);

    claude::uninstall_at(&settings, &state).unwrap();
    let restored: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert_eq!(restored["statusLine"]["command"], "echo old");
    assert!(restored["statusLine"].get("refreshInterval").is_none());
}

/// `configure` rewrites the whole settings file, so it must hand the user's
/// own keys back in the order they wrote them. Re-sorting a config file the
/// plugin does not own is an unasked-for edit, and it shows up in their diffs.
#[test]
fn claude_install_keeps_the_users_own_settings_key_order() {
    let home = tempfile::tempdir().unwrap();
    let settings = home.path().join("settings.json");
    let state = home.path().join("state");
    std::fs::write(
        &settings,
        r#"{"zzzLast":1,"model":"opus","alwaysThinkingEnabled":true,"aaaFirst":2}"#,
    )
    .unwrap();

    herdr_agent_quota::configure::claude::apply_at(
        &settings,
        &state,
        std::path::Path::new("/usr/local/bin/herdr-agent-quota"),
    )
    .unwrap();

    let written = std::fs::read_to_string(&settings).unwrap();
    // Order the keys by where they actually landed in the rewritten file.
    let mut order: Vec<(usize, &str)> = ["zzzLast", "model", "alwaysThinkingEnabled", "aaaFirst"]
        .into_iter()
        .map(|key| {
            let at = written
                .find(&format!("\"{key}\""))
                .unwrap_or_else(|| panic!("configure dropped {key}:\n{written}"));
            (at, key)
        })
        .collect();
    order.sort_unstable();
    let order: Vec<&str> = order.into_iter().map(|(_, key)| key).collect();
    assert_eq!(
        order,
        vec!["zzzLast", "model", "alwaysThinkingEnabled", "aaaFirst"],
        "configure re-sorted the user's settings file:\n{written}"
    );
    assert!(written.contains("claude-statusline"));
}

#[test]
fn claude_preserves_a_user_owned_refresh_interval_on_first_install() {
    let directory = tempdir().unwrap();
    let settings = directory.path().join("settings.json");
    let state = directory.path().join("state");
    let executable = directory.path().join("herdr-agent-quota");
    fs::write(
        &settings,
        r#"{"statusLine":{"type":"command","command":"echo old","refreshInterval":15}}"#,
    )
    .unwrap();

    claude::apply_at(&settings, &state, &executable).unwrap();
    let installed: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert_eq!(installed["statusLine"]["refreshInterval"], 15);

    claude::uninstall_at(&settings, &state).unwrap();
    let restored: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert_eq!(restored["statusLine"]["refreshInterval"], 15);
}

#[test]
fn repair_migrates_a_previous_backup_from_the_old_state_directory() {
    let directory = tempdir().unwrap();
    let settings = directory.path().join("settings.json");
    let old_state = directory.path().join("old-state");
    let state = directory.path().join("state");
    let executable = directory.path().join("herdr-agent-quota");
    fs::create_dir_all(&old_state).unwrap();
    fs::write(
        old_state.join("claude-statusline.original.json"),
        r#"{"type":"command","command":"echo user-owned"}"#,
    )
    .unwrap();
    // Built through serde rather than by interpolation: a Windows state path
    // is full of backslashes, and pasting one into a JSON string literal
    // produces invalid escapes.
    fs::write(
        &settings,
        serde_json::json!({
            "statusLine": {
                "type": "command",
                "command": format!(
                    "HERDR_PLUGIN_STATE_DIR='{}' '/old/herdr-agent-quota' claude-statusline",
                    old_state.display()
                ),
            }
        })
        .to_string(),
    )
    .unwrap();

    claude::apply_at(&settings, &state, &executable).unwrap();
    claude::uninstall_at(&settings, &state).unwrap();
    let restored: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert_eq!(restored["statusLine"]["command"], "echo user-owned");
}

#[test]
fn direct_configuration_write_refuses_an_ambiguous_cache_directory() {
    let output = output_with_deadline(
        Command::new(env!("CARGO_BIN_EXE_herdr-agent-quota"))
            .args(["configure", "--apply"])
            .env_remove("HERDR_PLUGIN_STATE_DIR"),
        &[],
        COMMAND_BUDGET,
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("must run through Herdr"));
}
