use std::io::{Read, Seek, SeekFrom, Write};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub const COMMAND_BUDGET: Duration = Duration::from_secs(15);

/// Run an integration-test child without letting a broken binary hang CI.
pub fn output_with_deadline(command: &mut Command, input: &[u8], budget: Duration) -> Output {
    let label = format!("{command:?}");
    let mut stdout = tempfile::tempfile().expect("create stdout capture");
    let mut stderr = tempfile::tempfile().expect("create stderr capture");
    command
        .stdin(Stdio::piped())
        .stdout(stdout.try_clone().expect("clone stdout capture"))
        .stderr(stderr.try_clone().expect("clone stderr capture"));
    herdr_agent_quota::platform::detach_process_group(command);

    let mut child = command
        .spawn()
        .unwrap_or_else(|error| panic!("spawn {label}: {error}"));
    child
        .stdin
        .take()
        .expect("open child stdin")
        .write_all(input)
        .unwrap_or_else(|error| panic!("write stdin for {label}: {error}"));

    let deadline = Instant::now() + budget;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .unwrap_or_else(|error| panic!("wait for {label}: {error}"))
        {
            break status;
        }
        if Instant::now() >= deadline {
            herdr_agent_quota::platform::kill_process_group(child.id());
            let _ = child.kill();
            let _ = child.wait();
            panic!("{label} exceeded its {budget:?} test budget");
        }
        thread::sleep(Duration::from_millis(10));
    };

    let mut captured_stdout = Vec::new();
    let mut captured_stderr = Vec::new();
    stdout.seek(SeekFrom::Start(0)).unwrap();
    stderr.seek(SeekFrom::Start(0)).unwrap();
    stdout.read_to_end(&mut captured_stdout).unwrap();
    stderr.read_to_end(&mut captured_stderr).unwrap();
    Output {
        status,
        stdout: captured_stdout,
        stderr: captured_stderr,
    }
}
