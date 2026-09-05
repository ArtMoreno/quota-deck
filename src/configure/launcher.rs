//! The typed Windows command runs in the current terminal, never opens a split.
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

const MARKER: &str = "rem QuotaDeck managed in-pane launcher";

fn quote(path: &Path) -> String {
    // cmd.exe cannot launch Windows' verbatim paths returned by current_exe.
    let path = path.to_string_lossy();
    let path = if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(&path).to_string()
    };
    path.replace('%', "%%")
}

pub fn apply() -> Result<()> {
    let Some(config) = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR") else {
        return Ok(());
    };
    let home = crate::platform::home_dir()?;
    let cache = crate::cache::CacheStore::from_env()?;
    install(
        &home.join(".local/bin/quotadeck.cmd"),
        &std::env::current_exe()?.with_extension("exe"),
        cache.root(),
        Path::new(&config),
    )
}

fn install(path: &Path, executable: &Path, state: &Path, config: &Path) -> Result<()> {
    let previous = match fs::read_to_string(path) {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).context("read existing QuotaDeck command"),
    };
    if !(previous.is_empty()
        || previous.contains(MARKER)
        || previous.contains("herdr-agent-quota-win") && previous.contains("open-dashboard-split"))
    {
        anyhow::bail!("{} is a custom command; preserve it and rename it before installing QuotaDeck's command", path.display());
    }
    fs::create_dir_all(path.parent().context("launcher directory")?)?;
    let body = format!("@echo off\r\n{MARKER}\r\nsetlocal DisableDelayedExpansion\r\nset \"HERDR_PLUGIN_STATE_DIR={}\"\r\nset \"HERDR_PLUGIN_CONFIG_DIR={}\"\r\n\"{}\" dashboard\r\nexit /b %errorlevel%\r\n", quote(state), quote(config), quote(executable));
    fs::write(path, body).context("write QuotaDeck command")?;
    println!(
        "QuotaDeck command: {} (runs in the current pane; q returns to the shell).",
        path.display()
    );
    Ok(())
}

pub fn uninstall() -> Result<()> {
    if std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").is_none() {
        return Ok(());
    }
    let path = crate::platform::home_dir()?.join(".local/bin/quotadeck.cmd");
    if fs::read_to_string(&path).is_ok_and(|body| body.contains(MARKER)) {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_command_runs_dashboard_directly_preserves_errors_and_custom_commands() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("space & 100% ! directory");
        fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("collector.cmd");
        fs::write(&exe, "@echo off\r\nif not \"%1\"==\"dashboard\" exit /b 9\r\nif not defined HERDR_PLUGIN_STATE_DIR exit /b 8\r\nif not defined HERDR_PLUGIN_CONFIG_DIR exit /b 7\r\nexit /b 23\r\n").unwrap();
        let launcher = bin.join("quotadeck.cmd");
        fs::write(
            &launcher,
            "herdr plugin action invoke open-dashboard-split --plugin herdr-agent-quota-win",
        )
        .unwrap();
        install(&launcher, &fs::canonicalize(&exe).unwrap(), &bin, &bin).unwrap();
        let output = std::process::Command::new(&launcher).output().unwrap();
        assert_eq!(output.status.code(), Some(23), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        fs::write(&launcher, "user custom command").unwrap();
        assert!(install(&launcher, &exe, &bin, &bin).is_err());
        assert_eq!(fs::read_to_string(launcher).unwrap(), "user custom command");
    }
}
