use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::process::{Child, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

/// A statusLine command must never consume the refresh interval itself.
pub const STATUSLINE_COMMAND_BUDGET: Duration = Duration::from_secs(2);

#[derive(Debug, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: Vec<u8>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

/// Run a user-owned shell command with a hard wall-clock budget.
///
/// The child owns a process group so a timeout removes the shell and any
/// helper it started. stdout is drained concurrently, while the caller keeps
/// ownership of the child and can therefore kill and reap it without a pid
/// reuse race.
pub fn run_shell_with_deadline(
    command: &str,
    input: &[u8],
    budget: Duration,
) -> Result<CommandOutput> {
    let mut child = crate::platform::shell_command(command);
    child
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::platform::detach_process_group(&mut child);

    let mut child = child.spawn().context("run previous statusLine")?;
    let stdin = child
        .stdin
        .take()
        .context("open previous statusLine stdin")?;
    let stdout = child
        .stdout
        .take()
        .context("open previous statusLine stdout")?;
    let input = input.to_vec();
    let writer = thread::spawn(move || {
        let mut stdin = stdin;
        let _ = stdin.write_all(&input);
    });

    let child = Arc::new(Mutex::new(Some(child)));
    let timed_out = Arc::new(AtomicBool::new(false));
    let (cancel, cancelled) = mpsc::channel();
    let watchdog_child = Arc::clone(&child);
    let watchdog_timed_out = Arc::clone(&timed_out);
    let watchdog = thread::spawn(move || {
        if cancelled.recv_timeout(budget).is_err() {
            watchdog_timed_out.store(true, Ordering::Release);
            let _ = terminate_and_reap(&watchdog_child);
        }
    });

    let mut stdout = stdout;
    let mut output = Vec::new();
    let read_result = stdout.read_to_end(&mut output);
    let _ = cancel.send(());
    let status = terminate_and_reap(&child)?;
    let _ = watchdog.join();

    let _ = writer.join();
    read_result.context("read previous statusLine output")?;

    Ok(CommandOutput {
        stdout: output,
        exit_code: status.and_then(|status| status.code()),
        timed_out: timed_out.load(Ordering::Acquire),
    })
}

fn terminate_and_reap(child: &Mutex<Option<Child>>) -> Result<Option<ExitStatus>> {
    let mut slot = child
        .lock()
        .map_err(|_| anyhow::anyhow!("lock previous statusLine child"))?;
    let Some(mut child) = slot.take() else {
        return Ok(None);
    };
    // The group created in the child setup makes this include shell
    // descendants, not just the shell itself.
    crate::platform::kill_process_group(child.id());
    let _ = child.kill();
    child.wait().map(Some).context("reap previous statusLine")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn captures_a_completed_command() {
        let result =
            run_shell_with_deadline("echo done", b"ignored", Duration::from_secs(5)).unwrap();
        assert_eq!(String::from_utf8_lossy(&result.stdout).trim(), "done");
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.timed_out);
    }

    #[test]
    fn kills_a_command_that_exceeds_its_budget() {
        let started = Instant::now();
        let result = run_shell_with_deadline(
            crate::platform::TEST_LONG_RUNNING_SHELL_COMMAND,
            b"",
            Duration::from_millis(200),
        )
        .unwrap();
        assert!(result.timed_out);
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
