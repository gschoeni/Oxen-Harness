//! Crash detection across runs: a fatal-signal handler that drops a marker
//! file, and a startup check that reads (and consumes) the marker the
//! *previous* run left behind.
//!
//! A process that dies on SIGSEGV/SIGBUS gets no chance to log anything
//! through normal channels — the user just sees their terminal back, and the
//! next session starts as if nothing happened. The marker closes that gap:
//! call [`check_previous_crash`] first (so a host can say "the last run
//! crashed" and log it), then [`install`] to arm the handler for this run.
//!
//! The handler does the absolute minimum allowed in async-signal context —
//! `open`/`write`/`close` of a pre-computed path — then re-raises the signal
//! so the OS still produces its normal crash report/core dump. A clean exit
//! never creates the marker.

use std::path::Path;

// The crate exists to install a signal handler; the unsafety is the point,
// and it is confined to `imp` with async-signal-safety documented per fn.
#[allow(unsafe_code)]
#[cfg(unix)]
mod imp {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;
    use std::sync::OnceLock;

    /// The marker path as a NUL-terminated string, fixed at install time so
    /// the signal handler allocates nothing.
    static MARKER: OnceLock<CString> = OnceLock::new();

    /// Async-signal-safe by construction: an atomic load ([`OnceLock::get`])
    /// and raw `open`/`write`/`close`. No allocation, no locks, no formatting.
    unsafe extern "C" fn on_fatal_signal(sig: libc::c_int) {
        if let Some(path) = MARKER.get() {
            let fd = libc::open(
                path.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC,
                0o600 as libc::c_uint,
            );
            if fd >= 0 {
                let name: &[u8] = match sig {
                    libc::SIGSEGV => b"SIGSEGV",
                    libc::SIGBUS => b"SIGBUS",
                    libc::SIGILL => b"SIGILL",
                    libc::SIGFPE => b"SIGFPE",
                    _ => b"unknown signal",
                };
                libc::write(fd, name.as_ptr().cast(), name.len());
                libc::close(fd);
            }
        }
        // SA_RESETHAND restored the default disposition on handler entry, so
        // re-raising hands the signal back to the OS for the normal crash
        // termination (and report/core dump where configured).
        libc::raise(sig);
    }

    pub(super) fn install(marker: &Path) {
        let Ok(cpath) = CString::new(marker.as_os_str().as_bytes()) else {
            return;
        };
        // First install wins; a second call (e.g. in tests) is a no-op.
        if MARKER.set(cpath).is_err() {
            return;
        }
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_fatal_signal as *const () as usize;
            action.sa_flags = libc::SA_RESETHAND | libc::SA_NODEFER;
            // Hardware faults only. SIGABRT stays out: `panic = "abort"`
            // builds, std::process::abort, and allocator OOM raise it for
            // failures that already report themselves — a marker there would
            // turn every such exit into a false "crashed last run" banner.
            for sig in [libc::SIGSEGV, libc::SIGBUS, libc::SIGILL, libc::SIGFPE] {
                libc::sigaction(sig, &action, std::ptr::null_mut());
            }
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use std::path::Path;

    pub(super) fn install(_marker: &Path) {}
}

/// The whole dance in one call — for hosts: report-then-arm. Returns the
/// signal name when the previous run crashed (already consumed), with the
/// handler armed for this run either way. Order matters: the old marker is
/// read before anything could clobber it.
pub fn arm(marker: &Path) -> Option<String> {
    let previous = check_previous_crash(marker);
    install(marker);
    previous
}

/// Arm the fatal-signal handler for this run. Best-effort and idempotent —
/// a path that can't be represented, or a second call, quietly does nothing.
/// Call after [`check_previous_crash`] so the previous run's marker isn't
/// clobbered before it's read.
pub fn install(marker: &Path) {
    imp::install(marker);
}

/// Read and consume the marker a previous run's crash left behind, returning
/// the signal name (e.g. `"SIGSEGV"`). `None` means the previous run exited
/// cleanly — the common case. The marker is removed, so a crash is reported
/// once, not on every subsequent launch.
pub fn check_previous_crash(marker: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(marker).ok()?;
    let _ = std::fs::remove_file(marker);
    let signal = contents.trim();
    Some(if signal.is_empty() {
        // The handler died before the write landed; the marker's existence is
        // still evidence enough.
        "unknown signal".to_string()
    } else {
        signal.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_marker_means_no_crash() {
        let dir = tempfile::tempdir().unwrap();
        assert!(check_previous_crash(&dir.path().join("last-crash")).is_none());
    }

    #[test]
    fn a_marker_is_reported_once_then_consumed() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("last-crash");
        std::fs::write(&marker, "SIGBUS").unwrap();
        assert_eq!(check_previous_crash(&marker).as_deref(), Some("SIGBUS"));
        assert!(check_previous_crash(&marker).is_none(), "consumed on read");
    }

    /// The real thing: a child process installs the handler and segfaults;
    /// the parent finds the marker. The child re-enters this same test binary
    /// with `CRASH_HELPER` set, so the crash happens in an isolated process.
    #[cfg(unix)]
    #[test]
    fn a_segfault_writes_the_marker() {
        if std::env::var_os("CRASH_HELPER").is_some() {
            let marker = std::path::PathBuf::from(std::env::var("CRASH_MARKER").unwrap());
            install(&marker);
            // SAFETY: not safe — the whole point. Faults with SIGSEGV so the
            // handler under test runs; confined to the helper child process.
            #[allow(unsafe_code)]
            unsafe {
                std::ptr::write_volatile(std::ptr::null_mut::<u8>(), 1)
            };
            unreachable!("the write above must fault");
        }

        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("last-crash");
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "tests::a_segfault_writes_the_marker",
                "--exact",
                "--nocapture",
            ])
            .env("CRASH_HELPER", "1")
            .env("CRASH_MARKER", &marker)
            .output()
            .unwrap();
        assert!(
            !out.status.success(),
            "the helper child should die on the fault"
        );
        assert_eq!(
            check_previous_crash(&marker).as_deref(),
            Some("SIGSEGV"),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
