use herdr_agent_quota::configure::grok::{apply_at, uninstall_at};
use std::fs;
use tempfile::tempdir;

#[test]
fn unified_watcher_removes_legacy_grok_hook() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("herdr-agent-quota.json");
    let state = directory.path().join("state");
    let executable = directory.path().join("herdr-agent-quota");

    fs::write(
        &path,
        r#"{"hooks":{"PostToolUse":[]},"managedBy":"herdr-agent-quota","command":"refresh --provider grok"}"#,
    )
    .unwrap();
    apply_at(&path, &state, &executable).unwrap();
    assert!(!path.exists());

    apply_at(&path, &state, &executable).unwrap();
    assert!(!path.exists());

    uninstall_at(&path).unwrap();
    assert!(!path.exists());
}

#[test]
fn grok_hook_uninstall_preserves_a_user_owned_file() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("herdr-agent-quota.json");
    fs::write(&path, "{\"hooks\":{\"Stop\":[]}}").unwrap();

    uninstall_at(&path).unwrap();
    assert!(path.exists());
}
