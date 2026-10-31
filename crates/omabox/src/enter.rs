//! Entering a box natively (#324, finding 490): setns(2) through a pidfd of the box's PID 1, in the
//! child between fork and exec, where bash ran nsenter. The pidfd pins the box's PID 1 (box_pid
//! asked again once it is open), so a pid reused meanwhile is never entered. With the box's pid
//! namespace the child forks once more, as nsenter did: the setns applies to children made after it.
//! The process left outside (the waiter) waits for the command, passes signals on and dies of the
//! same death. With a session of its own (box_exec, gdb, restart-shell: finding 491) the child in
//! the box forks once more again: it stays as the session's leader (the keeper) and the command
//! leads a process group of its own in that session, so every signal passed on reaches the
//! command's whole group, as the caller's group signals reached everything in it under nsenter, and
//! the keeper kills that group when the waiter dies (finding 494).
//!
//! Everything run in the child is async-signal-safe (the parent may have threads): plain syscalls on
//! values copied into it before the fork, words for a failure built beforehand.

use crate::boxes::{BoxPid, box_pid};
use crate::cat;
use std::ffi::OsString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

/// on_box's namespaces: the box's user and mount ones only (finding 307).
pub const TOOL: libc::c_int = libc::CLONE_NEWUSER | libc::CLONE_NEWNS;
/// restart-shell's kill: TOOL and the box's pid namespace, where its pids are its own.
pub const PIDS: libc::c_int = TOOL | libc::CLONE_NEWPID;

/// Every namespace of the box (box_exec, gdb), its network too when it has one of its own (since
/// finding 89; one started before shares the host's, which is not ours to join).
pub fn all(pid: &[u8]) -> libc::c_int {
    let ns = PIDS | libc::CLONE_NEWUTS | libc::CLONE_NEWIPC;
    let net = |p: &[u8]| std::fs::read_link(crate::shell::path(&cat!("/proc/", p, "/ns/net"))).ok();
    if net(pid) != net(b"self") {
        ns | libc::CLONE_NEWNET
    } else {
        ns
    }
}

/// The words before a failure's reason: what ns_said says of it (exec.rs's own_line keeps them).
pub const FAIL: &str = "omabox: cannot enter the box: ";

/// A box to enter: a pidfd of its PID 1 and which of its namespaces.
pub struct Entry {
    pidfd: OwnedFd,
    ns: libc::c_int,
}

impl Entry {
    /// The box in D whose PID 1 box_pid gave as PID, pinned. Err: the words a failed entry holds
    /// (the box went as it was entered), for ns_said.
    pub fn open(d: &[u8], pid: &[u8], ns: libc::c_int) -> Result<Entry, Vec<u8>> {
        let gone = |e: std::io::Error| cat!(FAIL, errno_text(e.raw_os_error().unwrap_or(0)), "\n");
        let n: libc::pid_t = std::str::from_utf8(pid)
            .ok()
            .and_then(|p| p.parse().ok())
            .ok_or_else(|| gone(std::io::Error::from_raw_os_error(libc::ESRCH)))?;
        // SAFETY: pidfd_open takes a pid and flags; the result is a new fd of ours (close-on-exec),
        // or -1.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, n, 0) };
        let fd = RawFd::try_from(fd).unwrap_or(-1);
        if fd < 0 {
            return Err(gone(std::io::Error::last_os_error()));
        }
        // SAFETY: just opened, ours alone.
        let low = unsafe { OwnedFd::from_raw_fd(fd) };
        // (at 10 or above, as every fd a child is told about: fds_only's dup2 to 3 never lands on it)
        let pidfd = crate::exec::dup_hi(low.as_raw_fd()).map_err(gone)?;
        drop(low);
        // Still the box's PID 1 now that it is pinned: had it died and its pid been reused before the
        // pidfd was opened, the pid would not be in the box's pid namespace now (box_pid).
        if !matches!(box_pid(d), BoxPid::Pid(p) if p == pid) {
            return Err(gone(std::io::Error::from_raw_os_error(libc::ESRCH)));
        }
        Ok(Entry { pidfd, ns })
    }

    /// The namespaces pasta made for the box in D (its user and network ones: root there, the
    /// network the box's), through pasta's child, pinned: the protected ports' rule is changed there
    /// (#399), where pasta's shell set it and the box's processes have no capability. Err when the
    /// box has no pasta of its own, or that child is not in a network namespace other than ours.
    pub fn pasta(d: &[u8]) -> Result<Entry, Vec<u8>> {
        let esrch = || cat!(FAIL, errno_text(libc::ESRCH), "\n");
        let pasta = crate::boxes::box_pasta(d).ok_or_else(esrch)?;
        let kid = crate::procfs::children(&pasta)
            .into_iter()
            .next()
            .ok_or_else(esrch)?;
        let n: libc::pid_t = std::str::from_utf8(&kid)
            .ok()
            .and_then(|p| p.parse().ok())
            .ok_or_else(esrch)?;
        // SAFETY: as in open.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, n, 0) };
        let fd = RawFd::try_from(fd).unwrap_or(-1);
        if fd < 0 {
            return Err(esrch());
        }
        // SAFETY: just opened, ours alone.
        let low = unsafe { OwnedFd::from_raw_fd(fd) };
        let pidfd = crate::exec::dup_hi(low.as_raw_fd()).map_err(|_| esrch())?;
        drop(low);
        // Pinned, still that pasta's child (a pid reused before the pidfd was opened is not), in a
        // network namespace that is not ours.
        let net =
            |p: &[u8]| std::fs::read_link(crate::shell::path(&cat!("/proc/", p, "/ns/net"))).ok();
        if crate::boxes::box_pasta(d).as_ref() != Some(&pasta)
            || !crate::procfs::children(&pasta).contains(&kid)
            || net(&kid).is_none()
            || net(&kid) == net(b"self")
        {
            return Err(esrch());
        }
        Ok(Entry {
            pidfd,
            ns: libc::CLONE_NEWUSER | libc::CLONE_NEWNET,
        })
    }

    /// The pidfd, for a command line another program runs (`_enter`, box_pty's script).
    pub fn pidfd(&self) -> RawFd {
        self.pidfd.as_raw_fd()
    }

    pub fn ns(&self) -> libc::c_int {
        self.ns
    }

    /// What the child does: enter, then (with a session) leave the caller's terminal, (keep_caps)
    /// keep its capabilities in the box for the next exec, and (EFD) make EFD its stderr.
    pub fn plan(&self, session: bool, keep_caps: bool, efd: Option<RawFd>) -> Plan {
        Plan {
            pidfd: self.pidfd.as_raw_fd(),
            ns: self.ns,
            session,
            keep_caps,
            efd: efd.unwrap_or(-1),
            shield: false,
            // (read here: SIGRTMIN is glibc's, kept apart from what the command may use)
            pdeath: libc::SIGRTMIN(),
            argv: if session { argv_span() } else { (0, 0) },
        }
    }
}

/// Where this process's command line lies in its memory: /proc/self/stat's arg_start and arg_end
/// (fields 48 and 49), (0, 0) when it cannot tell. The keeper, a fork of this process, blanks it
/// there (finding 511).
fn argv_span() -> (usize, usize) {
    let s = std::fs::read("/proc/self/stat").unwrap_or_default();
    let w = crate::shell::words(crate::procfs::after_comm(&s));
    let n = |i: usize| -> usize {
        w.get(i)
            .and_then(|v| std::str::from_utf8(v).ok()?.parse().ok())
            .unwrap_or(0)
    };
    let (start, end) = (n(45), n(46));
    if start > 0 && end > start {
        (start, end)
    } else {
        (0, 0)
    }
}

/// The entry as the child runs it (copied into it: raw numbers only).
#[derive(Clone, Copy)]
pub struct Plan {
    pidfd: RawFd,
    ns: libc::c_int,
    session: bool,
    keep_caps: bool,
    efd: RawFd,
    shield: bool,
    /// The keeper's PR_SET_PDEATHSIG: a signal nothing else sends it, caught even where the others
    /// came ignored.
    pdeath: libc::c_int,
    /// The caller's command line in memory (argv_span), for the keeper to blank.
    argv: (usize, usize),
}

impl Plan {
    /// For a command whose first word is ARGV0: one that is `timeout` put itself in a process group
    /// of its own in bash's (nsenter's) day, so neither the terminal's INT and QUIT nor a group's
    /// reached it; they are not passed on to it here either (gdb's runs, record's ffmpeg: a stop's TERM
    /// ends them, finding 453).
    pub fn shield_timeout(mut self, argv0: Option<&[u8]>) -> Plan {
        self.shield = argv0.is_some_and(|a| a == b"timeout" || a == b"/usr/bin/timeout");
        self
    }

    /// Whether it enters the box's pid namespace, where any box process could open the child's fds
    /// again through /proc/PID/fd: exec::command then closes every fd it is not given (finding 345).
    pub fn pid_ns(&self) -> bool {
        self.ns & libc::CLONE_NEWPID != 0
    }

    /// On C, as the first of its own pre_exec steps (before fds_only closes the pidfd).
    pub fn apply(self, c: &mut Command) {
        // SAFETY: enter makes only async-signal-safe calls on values copied into the closure.
        unsafe {
            c.pre_exec(move || {
                enter(&self);
                Ok(())
            });
        }
    }
}

/// The reason for errno E, from a fixed table (strerror is not async-signal-safe); "an error" past it.
pub fn errno_text(e: i32) -> &'static str {
    match e {
        libc::EPERM => "Operation not permitted",
        libc::ENOENT => "No such file or directory",
        libc::ESRCH => "No such process",
        libc::EBADF => "Bad file descriptor",
        libc::EAGAIN => "Resource temporarily unavailable",
        libc::ENOMEM => "Cannot allocate memory",
        libc::EACCES => "Permission denied",
        libc::EINVAL => "Invalid argument",
        libc::ENOSPC => "No space left on device",
        libc::EUSERS => "Too many users",
        libc::ENOSYS => "Function not implemented",
        _ => "an error",
    }
}

/// FAIL and E's reason on fd 2 (the held pipe, ns_held), then exit 1, as nsenter's failures did.
fn die(e: i32) -> ! {
    let t = errno_text(e);
    // SAFETY: write and _exit are async-signal-safe; the bytes are static.
    unsafe {
        libc::write(2, FAIL.as_ptr().cast(), FAIL.len());
        libc::write(2, t.as_ptr().cast(), t.len());
        libc::write(2, b"\n".as_ptr().cast(), 1);
        libc::_exit(1)
    }
}

fn errno() -> i32 {
    // SAFETY: this thread's errno.
    unsafe { *libc::__errno_location() }
}

/// The entry itself, between fork and exec. Returns in the process that goes on to exec.
fn enter(p: &Plan) {
    // SAFETY: setns, close, fork, setsid, setpgid, prctl, getpid, getppid, dup2 are
    // async-signal-safe syscalls on numbers of ours; a pidfd's setns joins every namespace in NS at
    // once.
    unsafe {
        if libc::setns(p.pidfd, p.ns) != 0 {
            die(errno());
        }
        libc::close(p.pidfd);
        if p.ns & libc::CLONE_NEWPID != 0 {
            // (a SIGCHLD the caller came with ignored would reap the command unseen: nsenter's reset)
            libc::signal(libc::SIGCHLD, libc::SIG_DFL);
            // (this process becomes the waiter: its parent is the caller's thread)
            let caller = libc::getppid();
            match libc::fork() {
                -1 => die(errno()),
                0 => {
                    // In the box, its parent (the waiter) outside its pid namespace, so getppid
                    // says 0; anything else means the waiter went before the prctl.
                    let pdeath = if p.session { p.pdeath } else { libc::SIGKILL };
                    libc::prctl(libc::PR_SET_PDEATHSIG, pdeath);
                    if libc::getppid() != 0 {
                        libc::_exit(1);
                    }
                    if p.session {
                        // No controlling terminal: the caller's is not the box's to read, write or
                        // signal (finding 491). This process keeps the session; the command gets a
                        // process group of its own in it (finding 494).
                        libc::setsid();
                        let keeper = libc::getpid();
                        match libc::fork() {
                            -1 => die(errno()),
                            0 => {
                                libc::setpgid(0, 0);
                                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                                if libc::getppid() != keeper {
                                    libc::_exit(1);
                                }
                            }
                            cmd => {
                                libc::setpgid(cmd, cmd);
                                keep(cmd, p);
                            }
                        }
                    }
                }
                child => wait_as(child, caller, p),
            }
        } else if p.session {
            libc::setsid();
        }
        if p.keep_caps {
            keep_caps();
        }
        // The caller's stderr from here on: the held pipe (nsenter's stderr was) is never the box's.
        if p.efd >= 0 {
            libc::dup2(p.efd, 2);
            libc::close(p.efd);
        }
    }
}

/// nsenter --keep-caps: the capabilities the box's user namespace gave (all of them) kept across the
/// next exec, as inheritable and ambient ones (gdb's setpriv then narrows them, finding 229).
unsafe fn keep_caps() {
    #[repr(C)]
    struct Head {
        version: u32,
        pid: i32,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Data {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }
    let mut h = Head {
        version: 0x2008_0522, // _LINUX_CAPABILITY_VERSION_3
        pid: 0,
    };
    let mut d = [Data::default(); 2];
    // SAFETY: capget and capset read and fill these two structs of ours; prctl takes numbers.
    unsafe {
        if libc::syscall(libc::SYS_capget, &mut h, d.as_mut_ptr()) != 0 {
            die(errno());
        }
        let eff = (u64::from(d[1].effective) << 32) | u64::from(d[0].effective);
        for x in &mut d {
            x.inheritable = x.permitted;
        }
        if libc::syscall(libc::SYS_capset, &mut h, d.as_ptr()) != 0 {
            die(errno());
        }
        for cap in 0..64u64 {
            if eff & (1 << cap) != 0 {
                // (one past the kernel's last says EINVAL: nothing to keep there)
                libc::prctl(libc::PR_CAP_AMBIENT, libc::PR_CAP_AMBIENT_RAISE, cap, 0, 0);
            }
        }
    }
}

/// Who this process (the waiter, or the keeper in the box) passes signals on to: its child, and for
/// the keeper that child's process group (GROUP). CHILD is 0 once the child is reaped, so nothing
/// goes to a pid that may be another's by then. OWN_SESSION: the command does not share the
/// caller's terminal, so the terminal's own signals go through here too. ASKED: a stop was asked for
/// (a Ctrl-Z), so the child's stop is this process's too; any other stop of the command's (its own,
/// a box process's, a tracer's) is not.
static CHILD: AtomicI32 = AtomicI32::new(0);
static GROUP: AtomicBool = AtomicBool::new(false);
static OWN_SESSION: AtomicBool = AtomicBool::new(false);
static ASKED: AtomicBool = AtomicBool::new(false);
/// This process is the keeper.
static KEEPER: AtomicBool = AtomicBool::new(false);

/// What is passed on to the command.
const PASSED: [libc::c_int; 6] = [
    libc::SIGHUP,
    libc::SIGINT,
    libc::SIGQUIT,
    libc::SIGTERM,
    libc::SIGUSR1,
    libc::SIGUSR2,
];

/// SIG to the child, or the keeper's to the command's group.
fn send(sig: libc::c_int) {
    let pid = CHILD.load(Ordering::Relaxed);
    if pid > 0 {
        let to = if GROUP.load(Ordering::Relaxed) {
            -pid
        } else {
            pid
        };
        // SAFETY: kill is async-signal-safe.
        unsafe { libc::kill(to, sig) };
    }
}

extern "C" fn pass_on(sig: libc::c_int, info: *mut libc::siginfo_t, _: *mut libc::c_void) {
    let _e = crate::sig::Errno::save();
    // A terminal's (SI_KERNEL) reached the command itself when it shares the terminal; a process's
    // (kill, the stop's pidfd: si_code <= 0) only this one.
    // SAFETY: the kernel's siginfo for this handler (SA_SIGINFO).
    let sent = info.is_null() || unsafe { (*info).si_code } <= 0;
    if sent || OWN_SESSION.load(Ordering::Relaxed) {
        send(sig);
    }
}

/// The keeper's PR_SET_PDEATHSIG: its waiter is gone (a KILL to it, finding 453's last step, or the
/// caller's whole group killed), so the command's group is killed, nothing it started left in the
/// box. (Its getppid cannot tell: the waiter's death hands it to a reaper outside the box's pid
/// namespace too.)
extern "C" fn parent_gone(_: libc::c_int) {
    let _e = crate::sig::Errno::save();
    send(libc::SIGKILL);
}

/// A terminal's stop (Ctrl-Z) for a command in a session of its own: the waiter asks the keeper,
/// the keeper stops the command's group with SIGSTOP (the kernel will not stop an orphaned process
/// group for SIGTSTP); each then stops with its child.
extern "C" fn stop_it(_: libc::c_int) {
    let _e = crate::sig::Errno::save();
    ASKED.store(true, Ordering::Relaxed);
    send(if KEEPER.load(Ordering::Relaxed) {
        libc::SIGSTOP
    } else {
        libc::SIGTSTP
    });
}

/// SIG's disposition as this process has it: ignored.
fn ignored(sig: libc::c_int) -> bool {
    // SAFETY: a query into a zeroed sigaction of ours.
    unsafe {
        let mut old: libc::sigaction = std::mem::zeroed();
        libc::sigaction(sig, std::ptr::null(), &mut old) == 0 && old.sa_sigaction == libc::SIG_IGN
    }
}

/// SIG caught by H (one that came ignored stays so, for this process as for the command, unless
/// ALWAYS), with every signal blocked while H runs.
fn catch(sig: libc::c_int, h: libc::sighandler_t, info: bool, always: bool) {
    if !always && ignored(sig) {
        return;
    }
    // SAFETY: a sigaction struct of ours, its handler async-signal-safe.
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = h;
        sa.sa_flags = libc::SA_RESTART | if info { libc::SA_SIGINFO } else { 0 };
        libc::sigfillset(&mut sa.sa_mask);
        libc::sigaction(sig, &sa, std::ptr::null_mut());
    }
}

/// The signal handling of a waiter or a keeper: PASSED passed on (INT and QUIT ignored when SHIELD),
/// a terminal's stops asked for when OWN_SESSION.
fn handle(shield: bool, own_session: bool) {
    for s in PASSED {
        if shield && (s == libc::SIGINT || s == libc::SIGQUIT) {
            // SAFETY: signal is async-signal-safe.
            unsafe { libc::signal(s, libc::SIG_IGN) };
            continue;
        }
        catch(s, pass_on as *const () as libc::sighandler_t, true, false);
    }
    if own_session {
        for s in [libc::SIGTSTP, libc::SIGTTIN, libc::SIGTTOU] {
            catch(s, stop_it as *const () as libc::sighandler_t, false, false);
        }
    }
}

/// The waiter, outside the box's pid namespace (nsenter's continue_as_child): no fd of the
/// command's, signals passed on, stopped with it when a stop was asked; ends with the caller
/// (PR_SET_PDEATHSIG TERM, passed on: finding 494), then as its child ended.
fn wait_as(child: libc::pid_t, caller: libc::pid_t, p: &Plan) -> ! {
    CHILD.store(child, Ordering::Relaxed);
    OWN_SESSION.store(p.session, Ordering::Relaxed);
    // SAFETY: async-signal-safe syscalls only, between fork and exec.
    unsafe {
        libc::syscall(libc::SYS_close_range, 0u32, u32::MAX, 0u32);
        // (its name in ps and pgrep: its command line is still omabox's, a fork's)
        libc::prctl(libc::PR_SET_NAME, c"omabox-enter".as_ptr());
    }
    handle(p.shield, p.session);
    // The caller's end is the command's: its thread that started this one waits for it, so this
    // fires when the caller is killed (a TERM or KILL to omabox alone).
    // SAFETY: prctl and getppid take numbers.
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
        if libc::getppid() != caller {
            libc::kill(libc::getpid(), libc::SIGTERM);
        }
    }
    reap_and_end(child)
}

/// The keeper, in the box: the session's leader, the command (CMD) leading its group; signals
/// passed on to that group, the group killed when the waiter goes (P's pdeath); then as the command
/// ended.
fn keep(cmd: libc::pid_t, p: &Plan) -> ! {
    CHILD.store(cmd, Ordering::Relaxed);
    GROUP.store(true, Ordering::Relaxed);
    OWN_SESSION.store(true, Ordering::Relaxed);
    KEEPER.store(true, Ordering::Relaxed);
    // SAFETY: async-signal-safe syscalls only, between fork and exec.
    unsafe {
        libc::syscall(libc::SYS_close_range, 0u32, u32::MAX, 0u32);
        libc::prctl(libc::PR_SET_NAME, c"omabox-session".as_ptr());
        blank_argv(p.argv);
    }
    handle(p.shield, true);
    // (a waiter gone before this, its signal's default action ended this process, and the command
    // with it, before the command could start anything)
    catch(
        p.pdeath,
        parent_gone as *const () as libc::sighandler_t,
        false,
        true,
    );
    reap_and_end(cmd)
}

/// The keeper's command line made its name alone: as a fork of omabox's it was the caller's whole
/// call, the command's words in it, so in the box `run -- pgrep -f WORD` found the keeper by them, and
/// `run -- pkill -f WORD` killed it, and the command with it (finding 511). The kernel reads
/// /proc/PID/cmdline from this memory: the name, then NULs to the end of the span.
///
/// # Safety
/// SPAN is this process's argv strings (argv_span, read before the fork), nothing of them read
/// after this; the keeper only waits and ends.
unsafe fn blank_argv(span: (usize, usize)) {
    let (start, end) = span;
    if end <= start {
        return;
    }
    let name = b"omabox-session";
    for (i, at) in (start..end).enumerate() {
        // (the last byte stays a NUL: the kernel reads the span as it is, no setproctitle guess)
        let b = if at + 1 < end {
            name.get(i).copied().unwrap_or(0)
        } else {
            0
        };
        // SAFETY: inside this process's argv strings (the caller's word).
        unsafe { std::ptr::write_volatile(at as *mut u8, b) };
    }
}

/// Waits for CHILD's end, a stop it was asked for stopping this one too (and continuing it after);
/// then this process ends as it did: its exit status, or its death (not dumpable first: never a
/// core of this process's), so the caller sees 128+N. Signals are blocked from the child's end on,
/// and CHILD cleared before it is reaped.
fn reap_and_end(child: libc::pid_t) -> ! {
    // SAFETY: async-signal-safe syscalls on structs of ours.
    unsafe {
        let mut all: libc::sigset_t = std::mem::zeroed();
        libc::sigfillset(&mut all);
        let mut none: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut none);
        let id = libc::id_t::try_from(child).unwrap_or(0);
        loop {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            if libc::waitid(
                libc::P_PID,
                id,
                &mut info,
                libc::WEXITED | libc::WSTOPPED | libc::WNOWAIT,
            ) != 0
            {
                if errno() == libc::EINTR {
                    continue;
                }
                libc::_exit(1);
            }
            libc::sigprocmask(libc::SIG_BLOCK, &all, std::ptr::null_mut());
            if info.si_code == libc::CLD_STOPPED || info.si_code == libc::CLD_TRAPPED {
                // (the stop taken, so the next wait sees what comes after it)
                let mut seen: libc::siginfo_t = std::mem::zeroed();
                libc::waitid(libc::P_PID, id, &mut seen, libc::WSTOPPED | libc::WNOHANG);
                if ASKED.swap(false, Ordering::Relaxed) {
                    libc::kill(libc::getpid(), libc::SIGSTOP);
                    send(libc::SIGCONT);
                }
                libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
                continue;
            }
            CHILD.store(0, Ordering::Relaxed);
            break;
        }
        let mut st = 0;
        while libc::waitpid(child, &mut st, 0) < 0 && errno() == libc::EINTR {}
        if libc::WIFEXITED(st) {
            libc::_exit(libc::WEXITSTATUS(st));
        }
        if libc::WIFSIGNALED(st) {
            let s = libc::WTERMSIG(st);
            libc::prctl(libc::PR_SET_DUMPABLE, 0);
            libc::signal(s, libc::SIG_DFL);
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, s);
            libc::sigprocmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
            libc::kill(libc::getpid(), s);
            libc::_exit(128 + s);
        }
        libc::_exit(1)
    }
}

/// `_enter PIDFD NS EFD -- ARGV...`: the entry for a command line another program runs (box_pty's
/// script holds the caller's terminal on the host and runs this on its pseudo-terminal). PIDFD is
/// an Entry's, passed on open; EFD the caller's stderr's fd, or `-` to keep this one's.
pub fn main(args: &[OsString]) -> ExitCode {
    let num = |a: Option<&OsString>| -> Option<i32> { a?.to_str()?.parse().ok() };
    let (Some(pidfd), Some(ns)) = (num(args.first()), num(args.get(1))) else {
        return usage();
    };
    let efd = match args.get(2).and_then(|a| a.to_str()) {
        Some("-") => -1,
        _ => match num(args.get(2)) {
            Some(n) => n,
            None => return usage(),
        },
    };
    if args.get(3).is_none_or(|a| a != "--") || args.len() < 5 {
        return usage();
    }
    let argv = &args[4..];
    let mut c = Command::new(&argv[0]);
    c.args(&argv[1..]);
    Plan {
        pidfd,
        ns,
        session: false,
        keep_caps: false,
        efd,
        shield: false,
        pdeath: libc::SIGRTMIN(),
        argv: (0, 0),
    }
    .apply(&mut c);
    let e = c.exec();
    let (why, rc) = crate::exec::exec_failed(argv[0].as_bytes(), &e);
    eprintln!(
        "omabox: cannot run {} in the box: {why}",
        argv[0].to_string_lossy()
    );
    ExitCode::from(rc)
}

fn usage() -> ExitCode {
    eprintln!("omabox: _enter PIDFD NS EFD|- -- ARGV... (omabox's own, for box_pty)");
    ExitCode::from(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons() {
        assert_eq!(errno_text(libc::EPERM), "Operation not permitted");
        assert_eq!(errno_text(libc::ESRCH), "No such process");
        assert_eq!(errno_text(9999), "an error");
    }

    /// Finding 511: the span the keeper blanks is this process's command line, as the kernel reads it.
    #[test]
    fn the_span_is_the_command_line() {
        let (start, end) = argv_span();
        assert!(end > start, "no span read");
        // SAFETY: the kernel's own span of this process's argv strings, only read.
        let mem = unsafe { std::slice::from_raw_parts(start as *const u8, end - start) };
        assert_eq!(mem, std::fs::read("/proc/self/cmdline").unwrap().as_slice());
    }

    #[test]
    fn namespaces() {
        // This process's own network: never joined.
        assert_eq!(all(b"self") & libc::CLONE_NEWNET, 0);
        assert_eq!(all(b"self") & PIDS, PIDS);
        assert_eq!(TOOL & libc::CLONE_NEWPID, 0);
    }

    #[test]
    fn a_pid_that_is_no_box_is_not_entered() {
        let d = std::env::temp_dir().join(format!("omabox-enter-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        let r = Entry::open(crate::shell::bytes(&d), b"1", TOOL);
        let _ = std::fs::remove_dir_all(&d);
        let Err(w) = r else {
            panic!("entered pid 1 for a dir that is no box")
        };
        assert!(
            w.starts_with(FAIL.as_bytes()),
            "{}",
            String::from_utf8_lossy(&w)
        );
    }
}
