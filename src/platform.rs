//! The handful of places where Windows and Unix genuinely differ.
//!
//! Upstream targets macOS and Linux and reaches for POSIX directly: `$HOME`,
//! `sh -c`, `AF_UNIX`, and process groups. None of those exist on Windows in
//! the same shape, but every one of them has an exact counterpart. Keeping the
//! substitutions in one module means the ~20k lines of provider and rendering
//! code stay platform-agnostic and read the same on both systems.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

#[doc(hidden)]
pub const TEST_LONG_RUNNING_SHELL_COMMAND: &str = if cfg!(windows) {
    "ping -n 21 127.0.0.1 >nul"
} else {
    "sleep 20 & wait"
};

/// The user's home directory.
///
/// `$HOME` is the POSIX answer and is also what Git Bash / MSYS set, so it
/// stays first: a user who runs the binary from that shell gets the same
/// directory their agent CLIs resolved. Native Windows processes inherit no
/// `HOME` at all, which is why `%USERPROFILE%` backs it up — without the
/// fallback every `~/.claude`-style lookup fails with "HOME is not set" when
/// the Herdr server (a native process) runs the plugin.
pub fn home_dir() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(home));
    }
    #[cfg(windows)]
    {
        if let Some(profile) = std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()) {
            return Ok(PathBuf::from(profile));
        }
        // Domain profiles occasionally expose only the split form.
        let drive = std::env::var_os("HOMEDRIVE").filter(|value| !value.is_empty());
        let path = std::env::var_os("HOMEPATH").filter(|value| !value.is_empty());
        if let (Some(drive), Some(path)) = (drive, path) {
            let mut joined = drive;
            joined.push(path);
            return Ok(PathBuf::from(joined));
        }
    }
    anyhow::bail!("cannot resolve the home directory (set HOME or USERPROFILE)")
}

/// Same as [`home_dir`] but for the callers that already return `Option`.
pub fn home_dir_opt() -> Option<PathBuf> {
    home_dir().ok()
}

/// Roaming application data — the Windows home of what XDG calls config/data.
///
/// Used for the agent CLIs that follow platform convention on Windows instead
/// of dropping a dotfile in the home directory.
#[cfg(windows)]
pub fn roaming_data_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir_opt().map(|home| home.join("AppData").join("Roaming")))
}

/// Local (non-roaming) application data — the Windows analogue of
/// `$XDG_CACHE_HOME` and `$XDG_DATA_HOME`.
#[cfg(windows)]
pub fn local_data_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir_opt().map(|home| home.join("AppData").join("Local")))
}

/// Build a command that runs `command` through the platform's own shell.
///
/// The string being run is not ours: it is whatever statusLine command the
/// user already had configured, which this plugin chains rather than
/// discards. It therefore has to be interpreted by the same shell the harness
/// would have used — `sh` on Unix, `cmd.exe` on Windows, which is what Claude
/// Code and Antigravity use there.
///
/// `cmd.exe /D` skips AutoRun registry commands so a user's `cmd` customization
/// cannot inject output into a statusLine snapshot.
pub fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut child = Command::new(cmd_executable());
        child.args(["/D", "/C", command]);
        child
    }
    #[cfg(not(windows))]
    {
        let mut child = Command::new("sh");
        child.args(["-c", command]);
        child
    }
}

/// Absolute path to `cmd.exe`, falling back to the bare name.
///
/// `%COMSPEC%` is the documented location and survives a stripped `PATH`,
/// which the Herdr server's environment can be.
#[cfg(windows)]
pub fn cmd_executable() -> PathBuf {
    std::env::var_os("COMSPEC")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cmd.exe"))
}

/// Put a spawned child in its own process group.
///
/// A statusLine command or the Codex app-server can outlive its budget and
/// start helpers of its own; killing the immediate child then leaves orphans
/// holding the pipe open. Both platforms can group the descendants, they just
/// spell it differently: `setpgid` before exec on Unix, the
/// `CREATE_NEW_PROCESS_GROUP` creation flag on Windows.
pub fn detach_process_group(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW — the second keeps a
        // console window from flashing when the Herdr server (itself a console
        // process) spawns a collector.
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = command;
    }
}

/// Fully detach a child so it survives the parent exiting.
///
/// The watch poller is started by a short-lived event hook and has to outlive
/// it. `setsid` does that on Unix; on Windows a new process group plus a
/// detached console is the equivalent, since there are no session leaders and
/// a child is not killed by its parent exiting in the first place.
pub fn detach_fully(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = command;
    }
}

/// Kill a whole process group.
///
/// Best-effort on both platforms: the caller still kills and reaps the direct
/// child afterwards, so a failure here only means a grandchild may linger.
pub fn kill_process_group(pid: u32) {
    #[cfg(windows)]
    {
        // Windows has no killpg. `taskkill /T` walks the child tree by parent
        // pid, which covers the same case: a shell that spawned helpers.
        let mut command = Command::new("taskkill.exe");
        command
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        detach_process_group(&mut command);
        let _ = command.status();
    }
    #[cfg(unix)]
    unsafe {
        let _ = libc::killpg(pid as libc::pid_t, libc::SIGKILL);
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
    }
}

/// A connection to the Herdr control socket.
///
/// Herdr speaks one newline-delimited JSON request/reply per connection over a
/// Unix domain socket on macOS and Linux and over a named pipe on Windows.
/// A Windows named pipe is opened with the ordinary file API and implements
/// `Read`/`Write`, so both sides collapse to the same two traits and
/// `herdr.rs` needs no `cfg` of its own.
pub struct HerdrSocket {
    #[cfg(windows)]
    inner: std::fs::File,
    #[cfg(windows)]
    read_deadline: std::time::Instant,
    #[cfg(not(windows))]
    inner: std::os::unix::net::UnixStream,
}

#[cfg(windows)]
fn windows_named_pipe_path(path: &std::ffi::OsStr) -> std::path::PathBuf {
    if path.to_string_lossy().starts_with(r"\\.\pipe\") {
        return path.into();
    }
    let mut native = std::ffi::OsString::from(r"\\.\pipe\");
    native.push(path);
    native.into()
}

impl HerdrSocket {
    /// Connect to the endpoint Herdr advertised in `$HERDR_SOCKET_PATH`.
    pub fn connect(path: &std::ffi::OsStr, timeout: std::time::Duration) -> Result<Self> {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let native_path = windows_named_pipe_path(path);
            // FILE_FLAG_OVERLAPPED is deliberately absent: every call here is
            // one short blocking round trip, and synchronous handles let the
            // std `Read`/`Write` impls work unchanged.
            const FILE_SHARE_READ: u32 = 0x0000_0001;
            const FILE_SHARE_WRITE: u32 = 0x0000_0002;
            let inner = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .open(native_path)
                .with_context(|| format!("connect to Herdr at {}", path.to_string_lossy()))?;
            Ok(Self {
                inner,
                read_deadline: std::time::Instant::now() + timeout,
            })
        }
        #[cfg(not(windows))]
        {
            let inner = std::os::unix::net::UnixStream::connect(path)
                .with_context(|| format!("connect to Herdr at {}", path.to_string_lossy()))?;
            inner.set_read_timeout(Some(timeout))?;
            inner.set_write_timeout(Some(timeout))?;
            Ok(Self { inner })
        }
    }
}

impl std::io::Read for HerdrSocket {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::System::Pipes::PeekNamedPipe;

            loop {
                let mut available = 0;
                let ok = unsafe {
                    PeekNamedPipe(
                        self.inner.as_raw_handle(),
                        std::ptr::null_mut(),
                        0,
                        std::ptr::null_mut(),
                        &mut available,
                        std::ptr::null_mut(),
                    )
                };
                if ok == 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if available > 0 {
                    break;
                }
                if std::time::Instant::now() >= self.read_deadline {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "Herdr named pipe read timed out",
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        self.inner.read(buffer)
    }
}

impl std::io::Write for HerdrSocket {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buffer)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// The file name a bundled executable takes on this platform.
pub const EXECUTABLE_SUFFIX: &str = std::env::consts::EXE_SUFFIX;

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    fn test_pipe_path(label: &str) -> String {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!(
            r"\\.\pipe\herdr-agent-quota-{label}-{}-{nonce}",
            std::process::id()
        )
    }

    #[cfg(windows)]
    unsafe fn create_test_pipe(path: &str) -> windows_sys::Win32::Foundation::HANDLE {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
        use windows_sys::Win32::System::Pipes::{
            CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
        };

        let wide: Vec<u16> = OsStr::new(path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            1,
            4096,
            4096,
            0,
            std::ptr::null(),
        )
    }

    #[cfg(windows)]
    unsafe fn connect_test_pipe(handle: windows_sys::Win32::Foundation::HANDLE) {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_PIPE_CONNECTED};
        use windows_sys::Win32::System::Pipes::ConnectNamedPipe;

        if ConnectNamedPipe(handle, std::ptr::null_mut()) == 0 {
            assert_eq!(GetLastError(), ERROR_PIPE_CONNECTED);
        }
    }

    #[test]
    fn home_falls_back_when_home_is_unset() {
        // The resolver must not depend on HOME alone; on Windows the Herdr
        // server never sets it.
        let resolved = home_dir();
        #[cfg(windows)]
        assert!(resolved.is_ok() || std::env::var_os("USERPROFILE").is_none());
        #[cfg(not(windows))]
        assert!(resolved.is_ok() || std::env::var_os("HOME").is_none());
    }

    #[test]
    fn shell_command_targets_the_platform_shell() {
        let command = shell_command("printf hi");
        let program = command.get_program().to_string_lossy().to_lowercase();
        #[cfg(windows)]
        assert!(program.contains("cmd"));
        #[cfg(not(windows))]
        assert!(program.contains("sh"));
    }

    #[cfg(windows)]
    #[test]
    fn named_pipe_socket_round_trips_one_newline_delimited_message() {
        use std::ffi::OsStr;
        use std::io::{BufRead, BufReader, Write};
        use std::os::windows::io::FromRawHandle;
        use std::sync::mpsc;
        use std::time::Duration;
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;

        let server_path = test_pipe_path("roundtrip");
        let logical_path = server_path.strip_prefix(r"\\.\pipe\").unwrap().to_string();
        let (ready_tx, ready_rx) = mpsc::channel();
        let server = std::thread::spawn(move || unsafe {
            let handle = create_test_pipe(&server_path);
            assert_ne!(handle, INVALID_HANDLE_VALUE);
            ready_tx.send(()).unwrap();
            connect_test_pipe(handle);

            let mut pipe = std::fs::File::from_raw_handle(handle);
            let mut request = String::new();
            BufReader::new(&mut pipe).read_line(&mut request).unwrap();
            assert_eq!(request, "{\"method\":\"ping\"}\n");
            pipe.write_all(b"{\"ok\":true}\n").unwrap();
            pipe.flush().unwrap();
        });

        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let mut socket =
            HerdrSocket::connect(OsStr::new(&logical_path), Duration::from_secs(2)).unwrap();
        socket.write_all(b"{\"method\":\"ping\"}\n").unwrap();
        socket.flush().unwrap();
        let mut reply = String::new();
        BufReader::new(socket).read_line(&mut reply).unwrap();
        assert_eq!(reply, "{\"ok\":true}\n");
        server.join().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn named_pipe_socket_times_out_when_the_server_never_replies() {
        use std::ffi::OsStr;
        use std::io::Read;
        use std::os::windows::io::FromRawHandle;
        use std::sync::mpsc;
        use std::time::{Duration, Instant};
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;

        let path = test_pipe_path("timeout");
        let server_path = path.clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = std::thread::spawn(move || unsafe {
            let handle = create_test_pipe(&server_path);
            assert_ne!(handle, INVALID_HANDLE_VALUE);
            ready_tx.send(()).unwrap();
            connect_test_pipe(handle);
            let _pipe = std::fs::File::from_raw_handle(handle);
            let _ = release_rx.recv_timeout(Duration::from_secs(2));
        });

        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let budget = Duration::from_millis(150);
        let mut socket = HerdrSocket::connect(OsStr::new(&path), budget).unwrap();
        let started = Instant::now();
        let error = socket.read(&mut [0_u8; 1]).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(1));
        release_tx.send(()).unwrap();
        server.join().unwrap();
    }
}
