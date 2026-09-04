use crate::process::{run_shell_with_deadline, CommandOutput, STATUSLINE_COMMAND_BUDGET};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) struct Adapter {
    pub label: &'static str,
    pub subcommand: &'static str,
    pub backup_file: &'static str,
}

impl Adapter {
    pub fn check(&self, path: &Path) -> Result<()> {
        let settings = read_settings(path, self.label)?;
        if self.is_installed(settings.get("statusLine")) {
            println!(
                "{} statusLine collector is installed: {}",
                self.label,
                path.display()
            );
        } else {
            println!(
                "{} statusLine preview for {}: install a reversible, silent quota collector",
                self.label,
                path.display()
            );
        }
        Ok(())
    }

    pub fn apply(&self, path: &Path, state: &Path, executable: &Path) -> Result<()> {
        self.apply_with_refresh_interval(path, state, executable, None)
    }

    pub fn apply_with_refresh_interval(
        &self,
        path: &Path,
        state: &Path,
        executable: &Path,
        refresh_interval_seconds: Option<u64>,
    ) -> Result<()> {
        let original_bytes = path
            .exists()
            .then(|| fs::read(path).with_context(|| format!("read {} settings", self.label)))
            .transpose()?;
        let mut settings = read_settings(path, self.label)?;
        let installed = self.is_installed(settings.get("statusLine"));
        if !installed && !can_chain_statusline(settings.get("statusLine")) {
            anyhow::bail!(
                "existing {} statusLine has no safely chainable command; refusing to replace it",
                self.label
            );
        }
        fs::create_dir_all(state).context("create plugin state directory")?;
        let backup = state.join(self.backup_file);
        let raw_backup = raw_backup_path(path)?;
        let legacy_raw_backup = sidecar(state, self.backup_file, "settings");
        let missing_backup = sidecar(state, self.backup_file, "missing");
        let installed_snapshot = sidecar(state, self.backup_file, "installed");
        if installed_snapshot.exists()
            && !installed_snapshot_matches(
                &installed_snapshot,
                original_bytes.as_deref().unwrap_or_default(),
            )?
        {
            remove_if_exists(&raw_backup)?;
            remove_if_exists(&legacy_raw_backup)?;
            remove_if_exists(&missing_backup)?;
        }
        if !installed {
            remove_if_exists(&raw_backup)?;
            remove_if_exists(&legacy_raw_backup)?;
            remove_if_exists(&missing_backup)?;
            match &original_bytes {
                Some(bytes) => write_source_protected_backup(path, &raw_backup, bytes, self.label)?,
                None => write_private(&missing_backup, [], "missing-settings marker")?,
            }
        } else if !raw_backup.exists() && legacy_raw_backup.exists() {
            let bytes = fs::read(&legacy_raw_backup)
                .with_context(|| format!("read legacy {} settings backup", self.label))?;
            write_source_protected_backup(path, &raw_backup, &bytes, self.label)?;
            remove_if_exists(&legacy_raw_backup)?;
        }
        if !backup.exists() || !installed {
            let original = if installed {
                self.previous_backup_from_wrapper(settings.get("statusLine"))?
                    .unwrap_or(Value::Null)
            } else {
                settings.get("statusLine").cloned().unwrap_or(Value::Null)
            };
            write_private(
                &backup,
                serde_json::to_vec_pretty(&original)?,
                "statusLine backup",
            )?;
        }
        let wrapper_command = wrapper_command(state, executable, self.subcommand);
        let status_line = settings
            .get_mut("statusLine")
            .and_then(Value::as_object_mut)
            .map(|object| {
                object.insert("type".to_string(), Value::String("command".to_string()));
                object.insert(
                    "command".to_string(),
                    Value::String(wrapper_command.clone()),
                );
                if let Some(seconds) = refresh_interval_seconds {
                    if installed || !object.contains_key("refreshInterval") {
                        object.insert("refreshInterval".to_string(), Value::from(seconds));
                    }
                }
                Value::Object(object.clone())
            })
            .unwrap_or_else(|| {
                let mut value = json!({"type": "command", "command": wrapper_command});
                if let Some(seconds) = refresh_interval_seconds {
                    value["refreshInterval"] = Value::from(seconds);
                }
                value
            });
        settings["statusLine"] = status_line;
        write_settings(path, &settings, self.label)?;
        write_private(
            &installed_snapshot,
            settings_digest(&fs::read(path)?),
            "installed-settings digest",
        )
    }

    pub fn uninstall(&self, path: &Path, state: &Path) -> Result<()> {
        if !path.exists() {
            return Ok(());
        }
        let mut settings = read_settings(path, self.label)?;
        if !self.is_installed(settings.get("statusLine")) {
            return Ok(());
        }
        let backup = state.join(self.backup_file);
        let raw_backup = raw_backup_path(path)?;
        let legacy_raw_backup = sidecar(state, self.backup_file, "settings");
        let missing_backup = sidecar(state, self.backup_file, "missing");
        let installed_snapshot = sidecar(state, self.backup_file, "installed");
        let current = fs::read(path)?;
        let unchanged = installed_snapshot.exists()
            && installed_snapshot_matches(&installed_snapshot, &current)?;
        if unchanged && raw_backup.exists() {
            write_raw_settings(path, &fs::read(&raw_backup)?, self.label)?;
        } else if unchanged && legacy_raw_backup.exists() {
            write_raw_settings(path, &fs::read(&legacy_raw_backup)?, self.label)?;
        } else if unchanged && missing_backup.exists() {
            fs::remove_file(path).with_context(|| format!("remove {} settings", self.label))?;
        } else {
            let original: Value = if backup.exists() {
                serde_json::from_slice(&fs::read(&backup)?)?
            } else {
                Value::Null
            };
            if original.is_null() {
                settings
                    .as_object_mut()
                    .with_context(|| format!("{} settings must be an object", self.label))?
                    .remove("statusLine");
            } else {
                settings["statusLine"] = original;
            }
            write_settings(path, &settings, self.label)?;
        }
        remove_if_exists(&backup)?;
        remove_if_exists(&raw_backup)?;
        remove_if_exists(&legacy_raw_backup)?;
        remove_if_exists(&missing_backup)?;
        remove_if_exists(&installed_snapshot)?;
        Ok(())
    }

    pub fn previous_command(&self, state: &Path) -> Result<Option<String>> {
        let backup = state.join(self.backup_file);
        if !backup.exists() {
            return Ok(None);
        }
        let value: Value = serde_json::from_slice(&fs::read(backup)?)?;
        Ok(match value {
            Value::String(command) => Some(command),
            Value::Object(map) => map
                .get("command")
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => None,
        })
    }

    pub(crate) fn run_previous(&self, state: &Path, input: &[u8]) -> Result<Option<CommandOutput>> {
        let Some(command) = self.previous_command(state)? else {
            return Ok(None);
        };
        run_shell_with_deadline(&command, input, STATUSLINE_COMMAND_BUDGET).map(Some)
    }

    fn is_installed(&self, status_line: Option<&Value>) -> bool {
        status_line
            .and_then(|value| value.get("command"))
            .and_then(Value::as_str)
            .is_some_and(|command| {
                command.contains(self.subcommand)
                    && (command.contains("herdr-agent-quota")
                        || command.contains("agy-statusline.sh"))
            })
    }

    fn previous_backup_from_wrapper(&self, status_line: Option<&Value>) -> Result<Option<Value>> {
        let Some(command) = status_line
            .and_then(|value| value.get("command"))
            .and_then(Value::as_str)
        else {
            return Ok(None);
        };
        let Some(old_state) = state_dir_from_wrapper(command) else {
            return Ok(None);
        };
        let backup = Path::new(&old_state).join(self.backup_file);
        if !backup.exists() {
            return Ok(None);
        }
        let value = serde_json::from_slice(&fs::read(backup)?)?;
        Ok(Some(value))
    }
}

fn can_chain_statusline(status_line: Option<&Value>) -> bool {
    match status_line {
        None | Some(Value::Null) | Some(Value::String(_)) => true,
        Some(Value::Object(map)) => map
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|command| !command.trim().is_empty()),
        Some(_) => false,
    }
}

pub(crate) fn settings_path(environment: &str, relative: &str) -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(environment) {
        return Ok(PathBuf::from(path));
    }
    Ok(crate::platform::home_dir()?.join(relative))
}

fn read_settings(path: &Path, label: &str) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let value: Value =
        serde_json::from_slice(&fs::read(path).with_context(|| format!("read {label} settings"))?)
            .with_context(|| format!("parse {label} settings"))?;
    if !value.is_object() {
        anyhow::bail!("{label} settings must be a JSON object")
    }
    Ok(value)
}

fn write_settings(path: &Path, settings: &Value, label: &str) -> Result<()> {
    write_raw_settings(path, &serde_json::to_vec_pretty(settings)?, label)
}

fn write_raw_settings(path: &Path, contents: &[u8], label: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {label} settings directory"))?;
    }
    let permissions = fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    let temporary = path.with_extension("json.herdr-agent-quota.tmp");
    fs::write(&temporary, contents)?;
    if let Some(permissions) = permissions {
        fs::set_permissions(&temporary, permissions)
            .with_context(|| format!("preserve {label} settings permissions"))?;
    } else {
        restrict_private(&temporary)?;
    }
    fs::rename(temporary, path).with_context(|| format!("replace {label} settings"))
}

fn raw_backup_path(path: &Path) -> Result<PathBuf> {
    let mut name = path
        .file_name()
        .context("settings path has no filename")?
        .to_os_string();
    name.push(".herdr-agent-quota.backup");
    Ok(path.with_file_name(name))
}

fn settings_digest(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

fn installed_snapshot_matches(path: &Path, current: &[u8]) -> Result<bool> {
    let marker = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    // Pre-1.4 installs stored the complete installed file here. Accept it once
    // so an upgrade remains reversible, then apply replaces it with the digest.
    Ok(marker == current || marker == settings_digest(current))
}

fn write_private(path: &Path, bytes: impl AsRef<[u8]>, label: &str) -> Result<()> {
    fs::write(path, bytes).with_context(|| format!("write {label}"))?;
    restrict_private(path)
}

fn write_source_protected_backup(
    _source: &Path,
    backup: &Path,
    bytes: &[u8],
    label: &str,
) -> Result<()> {
    fs::write(backup, bytes).with_context(|| format!("write {label} settings backup"))?;
    // This file sits beside the source so Windows applies the same per-user
    // directory ACL. Unix also preserves the source file's exact mode bits.
    #[cfg(unix)]
    fs::set_permissions(backup, fs::metadata(_source)?.permissions())
        .with_context(|| format!("protect {label} settings backup"))?;
    Ok(())
}

fn restrict_private(_path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(_path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("protect {}", _path.display()))?;
    }
    Ok(())
}

fn sidecar(state: &Path, backup_file: &str, suffix: &str) -> PathBuf {
    state.join(format!("{backup_file}.{suffix}"))
}

fn remove_if_exists(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(())
}

/// The statusLine command line this plugin installs.
///
/// The agent CLI runs this string through the platform's shell, so it has to be
/// written in that shell's syntax. The two differ in more than quoting: a
/// leading `VAR=value` assignment is how POSIX passes the state directory and
/// is simply invalid in `cmd.exe`, so Windows passes `--state-dir` instead.
fn wrapper_command(state: &Path, executable: &Path, subcommand: &str) -> String {
    if cfg!(windows) {
        format!(
            "{} {} --state-dir {}",
            cmd_quote(executable),
            subcommand,
            cmd_quote(state)
        )
    } else {
        format!(
            "HERDR_PLUGIN_STATE_DIR={} {} {}",
            shell_quote(state),
            shell_quote(executable),
            subcommand
        )
    }
}

/// Recover the state directory recorded in a wrapper this plugin wrote.
///
/// Used to follow a wrapper an earlier install left behind, so its backup of
/// the user's original statusLine is not orphaned.
fn state_dir_from_wrapper(command: &str) -> Option<String> {
    if let Some(rest) = command.strip_prefix("HERDR_PLUGIN_STATE_DIR='") {
        return rest.split_once("' ").map(|(state, _)| state.to_string());
    }
    let rest = command.split_once("--state-dir ")?.1.trim_start();
    Some(match rest.strip_prefix('"') {
        Some(quoted) => quoted.split_once('"')?.0.to_string(),
        None => rest.split_whitespace().next()?.to_string(),
    })
}

/// Quote a path for `cmd.exe`.
///
/// `cmd` has no escape character inside a quoted string, but Windows forbids a
/// double quote in a path, so wrapping is both sufficient and safe. Any
/// embedded quote is dropped rather than producing a command line that quietly
/// means something else.
fn cmd_quote(path: &Path) -> String {
    format!("\"{}\"", path.display().to_string().replace('"', ""))
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}
