#[test]
fn pane_focus_uses_the_quota_only_focus_path() {
    let manifest = include_str!("../herdr-plugin.toml");
    let hook = manifest
        .split("[[events]]")
        .find(|event| event.contains("on = \"pane.focused\""))
        .unwrap();
    // Commands are argv arrays, not shell strings: the subcommand is its own
    // element, so the last element is the whole word.
    assert!(hook.contains("\"focus\"]"), "{hook}");
    assert!(!hook.contains("\"event\"]"), "{hook}");
}

#[test]
fn plugin_exposes_one_click_configure_and_uninstall_actions() {
    let manifest = include_str!("../herdr-plugin.toml");
    assert!(manifest.contains("id = \"configure\""));
    assert!(manifest.contains("\"configure\", \"--apply\""));
    assert!(manifest.contains("id = \"uninstall\""));
    assert!(manifest.contains("\"configure\", \"--uninstall\""));
}

#[test]
fn windows_build_creates_the_extensionless_runtime_command() {
    let manifest = include_str!("../herdr-plugin.toml");
    let build = manifest
        .split("[[build]]")
        .find(|build| build.contains("platforms = [\"windows\"]"))
        .expect("Windows post-build step");
    assert!(
        build.contains("target/release/herdr-agent-quota.exe"),
        "{build}"
    );
    assert!(
        build.contains("-Destination 'target/release/herdr-agent-quota'"),
        "{build}"
    );
    assert!(include_str!("../install.ps1").contains("herdr-agent-quota.exe"));
    assert!(include_str!("../install.sh").contains("herdr-agent-quota.exe"));
}

#[test]
fn grok_runtime_refresh_does_not_go_through_a_plugin_action() {
    let manifest = include_str!("../herdr-plugin.toml");
    assert!(!manifest.contains("id = \"refresh-grok\""));
}

#[test]
fn exited_panes_do_not_trigger_a_quota_refresh() {
    let manifest = include_str!("../herdr-plugin.toml");
    assert!(!manifest.contains("on = \"pane.exited\""));
}

/// Settings remains a pane so it receives the plugin environment, while a
/// small action lets a keybinding open that pane.
#[test]
fn settings_are_an_action_backed_by_a_plugin_pane() {
    let manifest = include_str!("../herdr-plugin.toml");
    let pane = manifest
        .split("[[panes]]")
        .find(|pane| pane.contains("id = \"settings\""))
        .unwrap();
    assert!(pane.contains("\"settings\"]"), "{pane}");
    assert!(pane.contains("placement = \"popup\""), "{pane}");
    let action = manifest
        .split("[[actions]]")
        .find(|action| action.contains("id = \"open-settings\""))
        .expect("settings action");
    assert!(action.contains("\"open-settings\"]"), "{action}");
    // The shell chain the action used to carry now lives in the subcommand it
    // invokes, so that is where the pane-open call has to be checked.
    let source: String = include_str!("../src/herdr.rs")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(source.contains(r#""plugin", "pane", "open","#), "{source}");
    assert!(source.contains(r#""settings-windows""#), "{source}");
    assert!(source.contains(r#""settings""#), "{source}");
    assert!(source.contains(r#""--cwd""#), "{source}");
}

#[test]
fn the_dashboard_popup_matches_the_designed_card_size() {
    let manifest = include_str!("../herdr-plugin.toml");
    let pane = manifest
        .split("[[panes]]")
        .find(|pane| pane.contains("id = \"dashboard\""))
        .unwrap();
    assert!(pane.contains("width = 78"), "{pane}");
    assert!(pane.contains("height = 20"), "{pane}");
    let action = manifest
        .split("[[actions]]")
        .find(|action| action.contains("id = \"open-dashboard\""))
        .expect("popup dashboard action");
    assert!(action.contains("\"open-dashboard\"]"), "{action}");
}

#[test]
fn the_dashboard_has_a_one_click_split_entrypoint() {
    let manifest = include_str!("../herdr-plugin.toml");
    let pane = manifest
        .split("[[panes]]")
        .find(|pane| pane.contains("id = \"dashboard-split\""))
        .expect("split dashboard pane");
    assert!(pane.contains("placement = \"split\""), "{pane}");
    let action = manifest
        .split("[[actions]]")
        .find(|action| action.contains("id = \"open-dashboard-split\""))
        .expect("split dashboard action");
    assert!(action.contains("\"open-dashboard-split\"]"), "{action}");
}

#[test]
fn windows_plugin_panes_resolve_the_exe_through_cmd() {
    let manifest = include_str!("../herdr-plugin.toml");
    for (id, subcommand) in [
        ("dashboard-windows", "dashboard"),
        ("settings-windows", "settings"),
        ("dashboard-split-windows", "dashboard"),
    ] {
        let pane = manifest
            .split("[[panes]]")
            .find(|pane| pane.contains(&format!("id = \"{id}\"")))
            .unwrap_or_else(|| panic!("missing {id}"));
        assert!(pane.contains("platforms = [\"windows\"]"), "{pane}");
        assert!(
            pane.contains(&format!(
                "command = [\"cmd.exe\", \"/D\", \"/C\", \"target\\\\release\\\\herdr-agent-quota.exe\", \"{subcommand}\"]"
            )),
            "{pane}"
        );
    }
}

/// The pane draws one row per option and cannot fold them, so the popup has to
/// be tall enough for the whole list. A default popup is 24 rows; the list is
/// longer than that, and an option below the fold is an option nobody finds.
#[test]
fn the_settings_popup_is_tall_enough_for_every_option() {
    let manifest = include_str!("../herdr-plugin.toml");
    let pane = manifest
        .split("[[panes]]")
        .find(|pane| pane.contains("id = \"settings\""))
        .unwrap();
    let height: usize = pane
        .lines()
        .find_map(|line| line.strip_prefix("height = "))
        .expect("the settings popup declares a height")
        .trim()
        .parse()
        .unwrap();
    // Three section headers, eight choices, seven fields, eight agents, four
    // lines of TUI chrome, and the two rows consumed by Herdr's pane border.
    assert!(height >= 3 + 8 + 7 + 8 + 4 + 2, "height = {height}");
}

/// Herdr accepts a plugin-owned agent view only from `plugin:<manifest id>`
/// and answers `plugin_not_found` for anything else, so the source the plugin
/// sends and the id it is installed under have to be the same string.
#[test]
fn the_agent_view_source_matches_the_manifest_id() {
    let manifest = include_str!("../herdr-plugin.toml");
    let id = manifest
        .lines()
        .find_map(|line| line.strip_prefix("id = "))
        .expect("the manifest declares an id")
        .trim()
        .trim_matches('"');
    // The fork's id differs from upstream's on purpose: two plugins cannot
    // share one in a Herdr install. Pinned here so a rename has to be
    // deliberate, since Herdr answers `plugin_not_found` for any agent-view
    // source that is not `plugin:<manifest id>`.
    assert_eq!(id, "herdr-agent-quota-win");
    let source = include_str!("../src/herdr.rs")
        .lines()
        .find_map(|line| line.trim().strip_prefix("const AGENT_VIEW_SOURCE: &str = "))
        .expect("the plugin declares an agent view source")
        .trim()
        .trim_end_matches(';')
        .trim_matches('"');
    assert_eq!(source, format!("plugin:{id}"));
}
