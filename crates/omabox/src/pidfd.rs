//! Processes held by a pidfd (pidfd_open, pidfd_send_signal), not by their pid's number: a signal
//! through one reaches that process or none, never another that took its pid (#377).

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

/// A process, pinned.
pub struct Pidfd {
    fd: OwnedFd,
    pid: Vec<u8>,
}

impl Pidfd {
    /// PID pinned, when STILL holds of it once its pidfd is open: STILL reads /proc/PID, and what
    /// it read was the pinned process when that process is still there after (an unreaped process
    /// keeps its pid, so it cannot have been reused in between). None: no such process (a pid
    /// only), it was gone by the end, or STILL did not hold.
    pub fn pin(pid: &[u8], still: impl FnOnce() -> bool) -> Option<Pidfd> {
        let n: libc::pid_t = std::str::from_utf8(pid)
            .ok()
            .filter(|p| crate::shell::is_num(p.as_bytes()))
            .and_then(|p| p.parse().ok())
            .filter(|&n| n > 0)?;
        // SAFETY: pidfd_open takes a pid and flags; its result is a new fd of ours (close-on-exec),
        // or -1.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, n, 0) };
        let fd = i32::try_from(fd).ok().filter(|&f| f >= 0)?;
        // SAFETY: just opened, ours alone.
        let p = Pidfd {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
            pid: pid.to_vec(),
        };
        (still() && p.alive()).then_some(p)
    }

    /// Its pid, as it was given.
    pub fn pid(&self) -> &[u8] {
        &self.pid
    }

    /// SIG to it: whether it was sent (false once it is gone and reaped). Signal 0: whether it is
    /// there, as `kill -0` says (a zombie is).
    pub fn signal(&self, sig: libc::c_int) -> bool {
        // SAFETY: a signal through a pidfd we hold; no siginfo, no flags.
        unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.fd.as_raw_fd(),
                sig,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            ) == 0
        }
    }

    pub fn alive(&self) -> bool {
        self.signal(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pid_pinned_or_not() {
        let me = std::process::id().to_string().into_bytes();
        let p = Pidfd::pin(&me, || true).expect("this process pins");
        assert!(p.alive() && p.pid() == me.as_slice());
        assert!(
            Pidfd::pin(&me, || false).is_none(),
            "...not when the check fails"
        );
        for bad in [&b""[..], b"0", b"-1", b"1x", b" 1"] {
            assert!(Pidfd::pin(bad, || true).is_none(), "{bad:?}: no process");
        }
    }

    /// A reaped process signals nothing through its pidfd, whatever has its pid now.
    #[test]
    #[ignore = "starts processes: run by test/run.sh (cargo test -- --ignored)"]
    fn a_reaped_process_takes_no_signal() {
        let mut c = std::process::Command::new("sleep")
            .arg("300")
            .spawn()
            .unwrap();
        let p = Pidfd::pin(c.id().to_string().as_bytes(), || true).unwrap();
        c.kill().unwrap();
        assert!(p.alive(), "a zombie is still there, as kill -0 says");
        c.wait().unwrap();
        assert!(!p.alive() && !p.signal(libc::SIGCONT), "...reaped: gone");
    }
}
