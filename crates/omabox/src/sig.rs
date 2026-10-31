//! What bash's traps did for the commands ported here: a throwaway box taken down however `run` ends
//! (its EXIT trap: on the way out, and on INT, TERM or HUP), and a follower's processes ended with
//! it (follow_start's: exit 130, 143 or 129). Also how this half starts bin/omabox for what it has
//! not taken over (up, down).

use crate::cat;
use crate::exit::say;
use std::ffi::OsStr;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

/// bin/omabox, for a command this half leaves to it, with this process's environment.
pub fn omabox_cmd() -> Command {
    command(crate::root().join("bin/omabox"))
}

/// SIGPIPE as this process came, before Rust's runtime set it ignored (in main's preamble): read by
/// a constructor, which runs before that.
static PIPE_IGNORED: AtomicBool = AtomicBool::new(false);

extern "C" fn pipe_as_it_came() {
    PIPE_IGNORED.store(ignored(libc::SIGPIPE), Ordering::Relaxed);
}

#[used]
#[unsafe(link_section = ".init_array")]
static PIPE_AS_IT_CAME: extern "C" fn() = pipe_as_it_came;

/// Whether SIGPIPE came ignored.
pub fn pipe_ignored() -> bool {
    PIPE_IGNORED.load(Ordering::Relaxed)
}

/// `Command::new` for every child of this half: std's spawn sets SIGPIPE back to the default in it,
/// where bash's children keep an ignored one (a write to a closed pipe is EPIPE there, not death).
pub fn command(prog: impl AsRef<OsStr>) -> Command {
    // (every child of main's is made here, just before it starts: none once a trap is on its way)
    park_while_stopping();
    let mut c = Command::new(prog);
    // Every sort omabox runs orders bytes (and reads numbers) as C does, the same in every locale
    // (finding 446: bash's ran it in the caller's, whose collation it followed).
    if c.get_program() == "sort" {
        c.env("LC_ALL", "C");
    }
    if pipe_ignored() {
        // SAFETY: signal is async-signal-safe; run between fork and exec, after std's own reset.
        unsafe {
            std::os::unix::process::CommandExt::pre_exec(&mut c, || {
                libc::signal(libc::SIGPIPE, libc::SIG_IGN);
                Ok(())
            });
        }
    }
    c
}

/// The command's foreground child running (held_run, jq on the main thread): a signal's trap waits
/// for it, as bash runs a trap once the command in the foreground is over.
pub static FOREGROUND: Mutex<()> = Mutex::new(());

static MAIN: std::sync::OnceLock<std::thread::ThreadId> = std::sync::OnceLock::new();

/// This thread is main's (set at its start).
pub fn set_main() {
    let _ = MAIN.set(std::thread::current().id());
}

/// Whether this thread is main's, and not running a trap's undo (UNDO_MAIN): what parks and
/// takes FOREGROUND. An undo main runs (at the command's end, its traps dropped) runs its children
/// plainly, as a trap's thread does: the thread, on its way to the same undos, waits for that one
/// (trap::run_undos' lock) and must find main neither parked nor waiting for FOREGROUND, which the
/// thread may hold.
fn main_step() -> bool {
    MAIN.get() == Some(&std::thread::current().id()) && !UNDO_MAIN.load(Ordering::SeqCst)
}

/// Main runs a trap's undo (trap::run_undos).
pub static UNDO_MAIN: AtomicBool = AtomicBool::new(false);

/// This thread is main's.
pub fn on_main() -> bool {
    MAIN.get() == Some(&std::thread::current().id())
}

/// A terminating signal's trap is on its way (shot --no-pointer's release); GATED: main holds the
/// pointer's gate (ptr.rs), where it must not stop.
pub static STOPPING: AtomicBool = AtomicBool::new(false);
pub static GATED: AtomicBool = AtomicBool::new(false);

/// C run as bash runs a foreground command (every child of main's waited for goes through here):
/// C made before (sig::command parks, never under FOREGROUND), then with FOREGROUND held: spawned
/// (nothing once a trap is on its way), FG_ACTIVE raised once it exists when it takes a Ctrl-C
/// (takes_ctrl_c; never one started with SIGINT ignored, INT_IGNORED: a Ctrl-C reaches no child
/// then, finding 112's `<(...)`), a pidfd of it in FG_CHILD, WITH run on it (feeding or reading its pipes),
/// its end read and recorded (exec::status: FG_INT) before the lock drops. C is dropped right after
/// the spawn, so the pipe ends it held close. Its status and signal, or the spawn's error.
pub fn run_child<T>(
    c: Command,
    int_ignored: bool,
    with: impl FnOnce(&mut std::process::Child) -> T,
) -> std::io::Result<(T, (i32, Option<i32>))> {
    let mut c = c;
    let end = |ch: &mut std::process::Child| match ch.wait() {
        Ok(s) => crate::exec::status(s),
        Err(_) => (1, None),
    };
    if !main_step() {
        let spawned = c.spawn();
        drop(c);
        let mut ch = spawned?;
        let t = with(&mut ch);
        let st = end(&mut ch);
        return Ok((t, st));
    }
    // Nothing new starts once a trap is on its way, as bash starts nothing after its trap: a TERM
    // between two steps of monitor add must not see the box's output made after the trap's remove
    // (a monitor no one listed).
    let r = loop {
        park_while_stopping();
        let g = FOREGROUND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // (a signal between the park and the lock: its trap first, nothing started)
        if STOPPING.load(Ordering::SeqCst) && !GATED.load(Ordering::SeqCst) {
            drop(g);
            continue;
        }
        FG_INT.store(false, Ordering::SeqCst);
        let spawned = c.spawn();
        drop(c);
        let mut ch = match spawned {
            Ok(ch) => ch,
            Err(e) => {
                drop(g);
                break Err(e);
            }
        };
        FG_ACTIVE.store(
            !int_ignored && TAKES_INT.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        let pid = i32::try_from(ch.id()).unwrap_or(0);
        set_fg_child(pidfd_open(pid).map(|fd| (fd, pid)));
        let t = with(&mut ch);
        // (its end seen without reaping it, so its pid is not free for reuse while FG_CHILD names
        // it; then FG_CHILD cleared, then it is reaped)
        wait_unreaped(pid);
        set_fg_child(None);
        let st = end(&mut ch);
        FG_ACTIVE.store(false, Ordering::SeqCst);
        drop(g);
        break Ok((t, st));
    };
    park_while_stopping();
    r
}

/// run_child for a status: bash's `$?` (128+N for signal N).
pub fn run_status(c: Command) -> std::io::Result<i32> {
    run_child(c, false, |_| ()).map(|(_, (rc, _))| rc)
}

/// run_child for `$(...)`-like output: its stdout read whole (a pipe of its own), and its status.
pub fn run_output(mut c: Command) -> std::io::Result<(i32, Vec<u8>)> {
    c.stdout(std::process::Stdio::piped());
    run_child(c, false, |ch| {
        let mut out = Vec::new();
        if let Some(mut o) = ch.stdout.take() {
            let _ = std::io::Read::read_to_end(&mut o, &mut out);
        }
        out
    })
    .map(|(out, (rc, _))| (rc, out))
}

/// Main waits here while a trap is on its way (it ends this process, or lets main go on); another
/// thread (a trap's undo) never does.
fn park_while_stopping() {
    if !main_step() {
        return;
    }
    while STOPPING.load(Ordering::SeqCst) && !GATED.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// A foreground child that takes a Ctrl-C is running (FG_ACTIVE, takes_ctrl_c), and one of the
/// children it waited for died of SIGINT (FG_INT, exec::status): bash's test for a Ctrl-C the child
/// handled, which lets the command go on when that child did not die of it.
pub static FG_ACTIVE: AtomicBool = AtomicBool::new(false);
pub static FG_INT: AtomicBool = AtomicBool::new(false);
/// Main's foreground child while it runs, unreaped: a pidfd of it and its pid, for a signal's stop
/// that waited for it long enough (trap.rs's foreground_over, signal_foreground).
static FG_CHILD: Mutex<Option<(std::os::fd::OwnedFd, i32)>> = Mutex::new(None);

fn set_fg_child(v: Option<(std::os::fd::OwnedFd, i32)>) {
    if let Ok(mut g) = FG_CHILD.lock() {
        *g = v;
    }
}

/// A pidfd of PID (pidfd_open), or None (a kernel without it, or PID gone).
pub fn pidfd_open(pid: i32) -> Option<std::os::fd::OwnedFd> {
    use std::os::fd::FromRawFd;
    // SAFETY: pidfd_open takes a pid and flags; its result is a new fd of ours, or -1.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    let fd = i32::try_from(fd).ok().filter(|&f| f >= 0)?;
    // SAFETY: fd was just opened for us, and nothing else owns it.
    Some(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) })
}

/// Waits for PID's end without reaping it (waitid's WNOWAIT): until it is reaped its pid, and its
/// process group's id when it leads one, stay its own.
fn wait_unreaped(pid: i32) {
    let Ok(id) = libc::id_t::try_from(pid) else {
        return;
    };
    loop {
        // SAFETY: a zeroed siginfo_t of ours for waitid to fill.
        let r = unsafe {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            libc::waitid(libc::P_PID, id, &mut info, libc::WEXITED | libc::WNOWAIT)
        };
        if r == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return;
        }
    }
}

/// SIG to main's foreground child, through its pidfd (never another process that took its pid), and
/// to its process group when it leads one of its own (timeout's: what it runs is in it). Whether
/// there was such a child.
pub fn signal_foreground(sig: libc::c_int) -> bool {
    let Ok(g) = FG_CHILD.lock() else {
        return false;
    };
    let Some((fd, pid)) = g.as_ref() else {
        return false;
    };
    // SAFETY: a signal through a pidfd we hold; the child is unreaped while FG_CHILD names it, so
    // its group's id is still its own (getpgid, kill of the group).
    unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            fd.as_raw_fd(),
            sig,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        );
        if libc::getpgid(*pid) == *pid && *pid != libc::getpgrp() {
            libc::kill(-*pid, sig);
        }
    }
    true
}

/// While main runs F, its foreground child is one that takes a Ctrl-C itself (check run's
/// scenario): a terminal's INT that the child outlives lets the command go on (trap.rs), as bash goes
/// on when the child it waited for did not die of it. Any other child is just waited for, then the
/// command stops (finding 454).
pub fn takes_ctrl_c<T>(f: impl FnOnce() -> T) -> T {
    let was = TAKES_INT.swap(true, Ordering::SeqCst);
    let r = f();
    TAKES_INT.store(was, Ordering::SeqCst);
    r
}

static TAKES_INT: AtomicBool = AtomicBool::new(false);

/// D asleep on main, in-process: a trap on its way parks main here at once (STOPPING), so it has no
/// child to wait for (bash's `sleep` was one, a process per poll).
pub fn sleep(d: std::time::Duration) {
    let end = std::time::Instant::now() + d;
    loop {
        park_while_stopping();
        let now = std::time::Instant::now();
        if now >= end {
            return;
        }
        std::thread::sleep((end - now).min(std::time::Duration::from_millis(20)));
    }
}

/// A handler's errno given back as it found it.
pub struct Errno(libc::c_int);

impl Errno {
    pub fn save() -> Errno {
        // SAFETY: errno is this thread's; reading it is async-signal-safe.
        Errno(unsafe { *libc::__errno_location() })
    }
}

impl Drop for Errno {
    fn drop(&mut self) {
        // SAFETY: as in save: this thread's errno set back.
        unsafe { *libc::__errno_location() = self.0 }
    }
}

/// The write end of the pipe a signal handler tells a watcher thread through.
static SIG_PIPE: AtomicI32 = AtomicI32::new(-1);

extern "C" fn to_pipe(sig: libc::c_int) {
    let _e = Errno::save();
    let fd = SIG_PIPE.load(Ordering::Relaxed);
    if fd >= 0 {
        let b = [u8::try_from(sig).unwrap_or(0)];
        // SAFETY: write is async-signal-safe; b outlives the call.
        unsafe {
            libc::write(fd, b.as_ptr().cast(), 1);
        }
    }
}

const TERMINATING: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

/// Whether SIG is ignored now (as it came: this process ignores none itself).
pub fn ignored(sig: libc::c_int) -> bool {
    // SAFETY: a query of the disposition into a zeroed sigaction of ours.
    unsafe {
        let mut old: libc::sigaction = std::mem::zeroed();
        libc::sigaction(sig, std::ptr::null(), &mut old) == 0 && old.sa_sigaction == libc::SIG_IGN
    }
}

/// `trap ... SIG` as a non-interactive bash sets it: HANDLER, unless SIG was ignored when this
/// omabox started (nohup, a `&` of a script's), which bash cannot trap: then it stays ignored.
pub fn catch(sig: libc::c_int, handler: extern "C" fn(libc::c_int)) {
    if ignored(sig) {
        return;
    }
    // SAFETY: the handlers given here only make async-signal-safe calls.
    unsafe {
        libc::signal(sig, handler as *const () as libc::sighandler_t);
    }
}

/// catch for a handler that reads its siginfo_t (SA_SIGINFO): whether the kernel sent it (a
/// terminal's Ctrl-C) or a process (kill).
pub fn catch_info(
    sig: libc::c_int,
    handler: extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void),
) {
    if ignored(sig) {
        return;
    }
    // SAFETY: a zeroed sigaction of ours, its handler async-signal-safe; SA_RESTART as signal()'s.
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handler as *const () as libc::sighandler_t;
        sa.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(sig, &sa, std::ptr::null_mut());
    }
}

/// The box `run` started for its command, taken down once (`out=$(cmd_down NAME 2>&1) || say "$out"`).
pub struct Teardown {
    name: Vec<u8>,
    done: Arc<Mutex<bool>>,
}

impl Teardown {
    /// From now until finish: a terminating signal takes the box down, then ends this process as the
    /// signal would have.
    pub fn new(name: Vec<u8>) -> Teardown {
        let done = Arc::new(Mutex::new(false));
        if let Ok((r, w)) = crate::exec::pipe() {
            SIG_PIPE.store(w.as_raw_fd(), Ordering::Relaxed);
            std::mem::forget(w);
            for s in TERMINATING {
                catch(s, to_pipe);
            }
            let (n, d) = (name.clone(), done.clone());
            std::thread::spawn(move || {
                let mut f = std::fs::File::from(r);
                let mut b = [0u8; 1];
                if f.read_exact(&mut b).is_ok() {
                    down(&n, &d);
                    let sig = libc::c_int::from(b[0]);
                    // SAFETY: back to the default action, then the signal again: this process ends
                    // as it would have.
                    unsafe {
                        libc::signal(sig, libc::SIG_DFL);
                        libc::raise(sig);
                    }
                }
            });
        }
        Teardown { name, done }
    }

    pub fn finish(self) {
        down(&self.name, &self.done);
    }
}

fn down(name: &[u8], done: &Mutex<bool>) {
    let Ok(mut d) = done.lock() else { return };
    if *d {
        return;
    }
    *d = true;
    let Ok((r, w)) = crate::exec::pipe() else {
        return;
    };
    let mut c = omabox_cmd();
    c.arg("down").arg(OsStr::from_bytes(name));
    let Ok(w2) = w.try_clone() else { return };
    c.stdin(Stdio::null())
        .stdout(Stdio::from(w))
        .stderr(Stdio::from(w2));
    let child = c.spawn();
    drop(c);
    let out = crate::exec::read_all(r, None);
    let ok = child
        .and_then(|mut ch| ch.wait())
        .is_ok_and(|s| s.success());
    if !ok {
        say(cat!(crate::shell::subst(&out)));
    }
}

/// A follower's processes (follow_start), ended with this one: SIGTERM to each, then exit 130 (INT),
/// 143 (TERM) or 129 (HUP). Each held by a pidfd (the fd's number + 1; 0 an empty slot), opened as
/// it was started, kept for this process's life: one that ended and was reaped before the signal
/// gets nothing, nor whatever took its pid (#377).
static FOLLOWERS: [AtomicI32; 8] = [const { AtomicI32::new(0) }; 8];
static FOLLOWING: AtomicBool = AtomicBool::new(false);

extern "C" fn end_followers(sig: libc::c_int) {
    for p in &FOLLOWERS {
        let fd = p.load(Ordering::Relaxed) - 1;
        if fd >= 0 {
            // SAFETY: a raw syscall, async-signal-safe: a signal through a pidfd of ours.
            unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    fd,
                    libc::SIGTERM,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                );
            }
        }
    }
    let code = match sig {
        libc::SIGINT => 130,
        libc::SIGTERM => 143,
        _ => 129,
    };
    // SAFETY: _exit is async-signal-safe; nothing is left to flush (each line was).
    unsafe { libc::_exit(code) }
}

/// From now, a terminating signal ends the follower's processes, then this one (follow_start's traps).
pub fn follow_on() {
    if !FOLLOWING.swap(true, Ordering::Relaxed) {
        for s in TERMINATING {
            catch(s, end_followers);
        }
    }
}

/// A child just started (held_run's, entering a box): one of the follower's while it follows.
pub fn spawned(pid: u32) {
    if FOLLOWING.load(Ordering::Relaxed) {
        follower(pid);
    }
}

/// Registers PID, a child just started (not reaped: its pid is its own), as one of the follower's
/// processes.
pub fn follower(pid: u32) {
    use std::os::fd::IntoRawFd;
    let Some(fd) = i32::try_from(pid).ok().and_then(pidfd_open) else {
        return;
    };
    let fd = fd.into_raw_fd();
    for p in &FOLLOWERS {
        if p.compare_exchange(0, fd + 1, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return;
        }
    }
    // (no slot left: not followed, as before)
    // SAFETY: fd is ours alone, just opened.
    unsafe { libc::close(fd) };
}

/// Signal cases run in a process of their own (they end it): the test binary again, this one test
/// alone, OMABOX_SIG_CASE naming it; its status and stderr, or None when it hung (KILLed at 15 s).
#[cfg(test)]
mod signal_cases {
    use std::os::unix::process::ExitStatusExt;

    const CASE: &str = "OMABOX_SIG_CASE";

    /// Some((code or 128+signal, stderr, "[killed by signal N]" after it)) from the parent; None in the child alone, which runs the
    /// case (a child that cannot be started fails the test, never runs the case in the parent).
    fn in_child(name: &str) -> Option<Option<(i32, String)>> {
        in_child_tty(name, None)
    }

    /// in_child, the child with a terminal of its own (a pty, the child its session's leader) when
    /// CTRL_C_AT is given: a Ctrl-C typed there that many ms from its start, which the kernel sends
    /// to the child's whole process group (SI_KERNEL), as a user's would.
    fn in_child_tty(name: &str, ctrl_c_at: Option<u64>) -> Option<Option<(i32, String)>> {
        if std::env::var(CASE).as_deref() == Ok(name) {
            return None;
        }
        let exe = std::env::current_exe().expect("the test binary's path");
        let mut cmd = std::process::Command::new(exe);
        cmd.args([
            &format!("sig::signal_cases::{name}"),
            "--exact",
            "--nocapture",
        ])
        .args(["--test-threads=1"])
        .env(CASE, name)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
        let mut master = None;
        if ctrl_c_at.is_some() {
            let (mut m, mut sl) = (-1, -1);
            // SAFETY: openpty fills two fds of ours; no name, termios or size asked for.
            let r = unsafe {
                libc::openpty(
                    &mut m,
                    &mut sl,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            };
            assert_eq!(r, 0, "openpty");
            // SAFETY: setsid and ioctl are async-signal-safe; sl is the pty's slave, open here.
            unsafe {
                std::os::unix::process::CommandExt::pre_exec(&mut cmd, move || {
                    libc::setsid();
                    libc::ioctl(sl, libc::TIOCSCTTY, 0);
                    Ok(())
                });
            }
            master = Some((m, sl));
        }
        let mut ch = cmd.spawn().expect("the case's child");
        if let (Some((m, sl)), Some(at)) = (master, ctrl_c_at) {
            // SAFETY: the slave is the child's now; ours closed.
            unsafe { libc::close(sl) };
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(at));
                // SAFETY: one byte, ^C, to the pty's master, which stays open (ours till we exit).
                unsafe { libc::write(m, b"\x03".as_ptr().cast(), 1) };
            });
        }
        let err = ch.stderr.take().expect("the case's stderr");
        let reader = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = std::io::Read::read_to_string(&mut { err }, &mut s);
            s
        });
        let end = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let st = loop {
            if let Ok(Some(st)) = ch.try_wait() {
                break Some(st);
            }
            if std::time::Instant::now() > end {
                let _ = ch.kill();
                let _ = ch.wait();
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        let err = reader.join().unwrap_or_default();
        // (a death by a signal said in the words too: a code trap exits 128+N, it does not die)
        Some(st.map(|s| match s.code() {
            Some(c) => (c, err),
            None => {
                let n = s.signal().unwrap_or(0);
                (128 + n, format!("{err}[killed by signal {n}]"))
            }
        }))
    }

    /// A file of the case's, the same path in the test and in its child (named by the test's pid).
    fn case_file(tag: &str) -> std::path::PathBuf {
        let pid = if std::env::var(CASE).is_ok() {
            std::os::unix::process::parent_id()
        } else {
            std::process::id()
        };
        std::env::temp_dir().join(format!("omabox-sig-{tag}.{pid}"))
    }

    fn sleep_child(s: &str) -> std::io::Result<i32> {
        let mut c = super::command("sleep");
        c.arg(s);
        super::run_status(c)
    }

    /// SIG to this process alone at each AT (ms from now).
    fn kill_at(at: &[(u64, i32)]) {
        let pid = i32::try_from(std::process::id()).unwrap_or(0);
        let v = at.to_vec();
        std::thread::spawn(move || {
            let mut t = 0;
            for (at, sig) in v {
                std::thread::sleep(std::time::Duration::from_millis(at - t));
                t = at;
                // SAFETY: a signal to this test's own process.
                unsafe { libc::kill(pid, sig) };
            }
        });
    }

    /// A code trap (record stop's) and a TERM to omabox alone during a foreground child, an INT
    /// 200 ms on, still in it: the child's end, the undo, then the first signal's 143, nothing more
    /// of the command (bash ran the lowest-numbered pending trap's code, 130: finding 452).
    #[test]
    fn code_trap_first_signal_wins() {
        let Some(r) = in_child("code_trap_first_signal_wins") else {
            super::set_main();
            let _t = crate::trap::on_exit_code(|| eprintln!("CASE undo"));
            kill_at(&[(200, libc::SIGTERM), (400, libc::SIGINT)]);
            let _ = sleep_child("0.6");
            eprintln!("CASE after the child");
            return;
        };
        let (rc, err) = r.expect("it hung");
        assert_eq!(rc, 143, "{err}");
        assert!(!err.contains("[killed by signal"), "{err}");
        assert_eq!(err.matches("CASE undo").count(), 1, "{err}");
        assert!(!err.contains("CASE after"), "{err}");
    }

    /// A second signal while the undo runs (its own child, 0.6 s): the undo finishes, then this
    /// process ends of the first (bash's trap died of the second, its undo cut short: finding 452).
    /// For a trap with an exit code and one without.
    fn second_during_undo(name: &str, code: bool) -> Option<(i32, String)> {
        let Some(r) = in_child(name) else {
            super::set_main();
            let undo = || {
                eprintln!("CASE undo begun");
                let _ = sleep_child("0.6");
                eprintln!("CASE undo over");
            };
            let _t = if code {
                crate::trap::on_exit_code(undo)
            } else {
                crate::trap::on_exit(undo)
            };
            kill_at(&[
                (200, libc::SIGTERM),
                (500, libc::SIGINT),
                (600, libc::SIGHUP),
            ]);
            std::thread::sleep(std::time::Duration::from_secs(3));
            eprintln!("CASE after the wait");
            return None;
        };
        Some(r.expect("it hung"))
    }

    #[test]
    fn second_signal_during_the_undo() {
        let Some((rc, err)) = second_during_undo("second_signal_during_the_undo", false) else {
            return;
        };
        assert_eq!(rc, 143, "{err}");
        assert!(err.contains("[killed by signal 15]"), "{err}");
        assert!(err.contains("CASE undo over"), "{err}");
        assert!(!err.contains("CASE after"), "{err}");
    }

    #[test]
    fn second_signal_during_a_code_undo() {
        let Some((rc, err)) = second_during_undo("second_signal_during_a_code_undo", true) else {
            return;
        };
        assert_eq!(rc, 143, "{err}");
        assert!(!err.contains("[killed by signal"), "{err}");
        assert!(err.contains("CASE undo over"), "{err}");
        assert!(!err.contains("CASE after"), "{err}");
    }

    /// A TERM to omabox alone during a foreground child (a check run's `up`): the trap waits for it,
    /// so the undo sees what it did (bash ran its EXIT trap at once, the child left running: finding
    /// 453); then 143.
    #[test]
    fn term_waits_for_the_foreground_child() {
        let f = case_file("fg");
        let Some(r) = in_child("term_waits_for_the_foreground_child") else {
            super::set_main();
            let g = f.clone();
            let _t = crate::trap::on_exit(move || {
                let seen = if g.exists() {
                    "the child's file"
                } else {
                    "nothing"
                };
                eprintln!("CASE undo saw {seen}");
            });
            kill_at(&[(150, libc::SIGTERM)]);
            let mut c = super::command("sh");
            c.args(["-c", "sleep 0.5; : > \"$0\"", &f.to_string_lossy()]);
            let _ = super::run_status(c);
            eprintln!("CASE after the child");
            return;
        };
        let (rc, err) = r.expect("it hung");
        let _ = std::fs::remove_file(&f);
        assert_eq!(rc, 143, "{err}");
        assert!(err.contains("CASE undo saw the child's file"), "{err}");
        assert!(!err.contains("CASE after"), "{err}");
    }

    /// A foreground child that does not end: waited for FG_GRACE[0], sent a TERM it ignores, then a
    /// KILL; the undo runs and this ends of its signal, never hanging on it (finding 453).
    #[test]
    fn a_child_that_will_not_end_is_stopped() {
        let Some(r) = in_child("a_child_that_will_not_end_is_stopped") else {
            super::set_main();
            let _t = crate::trap::on_exit_code(|| eprintln!("CASE undo"));
            kill_at(&[(150, libc::SIGHUP)]);
            let mut c = super::command("sh");
            c.args(["-c", "trap '' TERM; exec sleep 30"]);
            let rc = super::run_status(c);
            eprintln!("CASE after the child {rc:?}");
            return;
        };
        let t0 = std::time::Instant::now();
        let (rc, err) = r.expect("it hung");
        assert_eq!(rc, 129, "{err}");
        assert!(err.contains(crate::trap::STOPPING_LINE), "{err}");
        assert!(!err.contains("[killed by signal"), "{err}");
        assert!(err.contains("CASE undo"), "{err}");
        assert!(!err.contains("CASE after"), "{err}");
        assert!(t0.elapsed() < std::time::Duration::from_secs(8), "{err}");
    }

    /// A foreground child that leads a process group of its own (as `timeout` does), whose own
    /// child would outlive it: the stop's TERM reaches the whole group, through a pidfd (finding 453).
    #[test]
    fn the_stop_reaches_the_childs_group() {
        let f = case_file("grp");
        let Some(r) = in_child("the_stop_reaches_the_childs_group") else {
            super::set_main();
            let _t = crate::trap::on_exit(|| eprintln!("CASE undo"));
            kill_at(&[(300, libc::SIGTERM)]);
            let mut c = super::command("sh");
            c.args([
                "-c",
                "sleep 30 & echo $! > \"$0\"; wait",
                &f.to_string_lossy(),
            ]);
            std::os::unix::process::CommandExt::process_group(&mut c, 0);
            let _ = super::run_status(c);
            eprintln!("CASE after the child");
            return;
        };
        let (rc, err) = r.expect("it hung");
        let gc = std::fs::read_to_string(&f).unwrap_or_default();
        let _ = std::fs::remove_file(&f);
        assert_eq!(rc, 143, "{err}");
        assert!(err.contains("CASE undo"), "{err}");
        let gc: i32 = gc.trim().parse().expect("the grandchild's pid");
        std::thread::sleep(std::time::Duration::from_millis(200));
        // SAFETY: a probe of a pid, signal 0.
        let alive = unsafe { libc::kill(gc, 0) } == 0
            && std::fs::read_to_string(format!("/proc/{gc}/cmdline"))
                .is_ok_and(|c| c.starts_with("sleep"));
        assert!(!alive, "the group's sleep {gc} outlived the stop");
    }

    /// A TERM while main runs a trap's undo itself (a trap dropped on a refusal), its first of two
    /// steps: the signal's thread waits for that undo, so its second step runs too (it found TRAPS
    /// empty and ended the process halfway through).
    #[test]
    fn a_signal_waits_for_an_undo_main_runs() {
        let Some(r) = in_child("a_signal_waits_for_an_undo_main_runs") else {
            super::set_main();
            crate::trap::stop_on_signals();
            let t = crate::trap::on_exit(|| {
                let _ = sleep_child("0.5");
                eprintln!("CASE step 1 over");
                let _ = sleep_child("0.2");
                eprintln!("CASE step 2 over");
            });
            kill_at(&[(200, libc::SIGTERM)]);
            drop(t);
            eprintln!("CASE after the undo");
            std::thread::sleep(std::time::Duration::from_secs(3));
            eprintln!("CASE after the wait");
            return;
        };
        let (rc, err) = r.expect("it hung");
        assert_eq!(rc, 143, "{err}");
        assert!(err.contains("CASE step 2 over"), "{err}");
        assert!(!err.contains("CASE after the wait"), "{err}");
    }

    /// An INT sent by a process (`kill -INT PID`) while a foreground child runs that would take a
    /// Ctrl-C and outlives this one: the command stops like TERM, once that child is over, 130 (bash
    /// went on: finding 454).
    #[test]
    fn int_from_a_process_stops_the_command() {
        let Some(r) = in_child("int_from_a_process_stops_the_command") else {
            super::set_main();
            crate::trap::stop_on_signals();
            kill_at(&[(150, libc::SIGINT)]);
            let mut c = super::command("sh");
            c.args(["-c", "trap '' INT; sleep 0.5"]);
            let _ = super::takes_ctrl_c(|| super::run_status(c));
            eprintln!("CASE past the step");
            return;
        };
        let (rc, err) = r.expect("it hung");
        assert_eq!(rc, 130, "{err}");
        assert!(!err.contains("CASE past"), "{err}");
    }

    /// An INT sent by a process while main sleeps in-process (trap::sleep): it stops at once, no
    /// sleep child to wait for.
    #[test]
    fn int_during_an_in_process_sleep() {
        let Some(r) = in_child("int_during_an_in_process_sleep") else {
            super::set_main();
            let _t = crate::trap::on_exit(|| eprintln!("CASE undo"));
            kill_at(&[(150, libc::SIGINT)]);
            crate::trap::sleep(b"5");
            eprintln!("CASE past the sleep");
            return;
        };
        let t0 = std::time::Instant::now();
        let (rc, err) = r.expect("it hung");
        assert_eq!(rc, 130, "{err}");
        assert!(err.contains("CASE undo"), "{err}");
        assert!(!err.contains("CASE past"), "{err}");
        assert!(t0.elapsed() < std::time::Duration::from_secs(3), "{err}");
    }

    /// A Ctrl-C from the terminal (SI_KERNEL, to the whole group) during a 1 s foreground child that
    /// ignores it, after 400 ms: MARKED (sig::takes_ctrl_c, a scenario's), the command goes
    /// on, as before and as bash's; otherwise it stops once the child is over, 130 (finding 454).
    fn ctrl_c(name: &str, marked: bool, child: &str) -> Option<(i32, String)> {
        let Some(r) = in_child_tty(name, Some(400)) else {
            super::set_main();
            let _t = crate::trap::on_exit(|| eprintln!("CASE undo"));
            let mut c = super::command("sh");
            c.args(["-c", child]);
            let rc = if marked {
                super::takes_ctrl_c(|| super::run_status(c))
            } else {
                super::run_status(c)
            };
            eprintln!("CASE past the step {rc:?}");
            return None;
        };
        Some(r.expect("it hung"))
    }

    /// A child that handles a Ctrl-C (says so) and ends 0.
    const HANDLES: &str = "trap 'echo CASE child got INT >&2' INT; sleep 1; exit 0";

    #[test]
    fn ctrl_c_a_marked_child_outlives_goes_on() {
        let name = "ctrl_c_a_marked_child_outlives_goes_on";
        let Some((rc, err)) = ctrl_c(name, true, HANDLES) else {
            return;
        };
        assert_eq!(rc, 0, "{err}");
        assert!(err.contains("CASE child got INT"), "{err}");
        assert!(err.contains("CASE past the step Ok(0)"), "{err}");
        assert!(err.contains("CASE undo"), "{err}");
    }

    /// A Ctrl-C a marked child ignores (it runs 30 s), then a TERM from a process: the TERM stops the
    /// command, the child waited for (bounded) and stopped, 143, in about two seconds; the Ctrl-C's
    /// wait read no more signals and held it for the child's 30 s.
    #[test]
    fn ctrl_c_then_a_term_stops() {
        let name = "ctrl_c_then_a_term_stops";
        let Some(r) = in_child_tty(name, Some(400)) else {
            super::set_main();
            let _t = crate::trap::on_exit(|| eprintln!("CASE undo"));
            kill_at(&[(800, libc::SIGTERM)]);
            let mut c = super::command("sh");
            c.args(["-c", "trap '' INT; exec sleep 30"]);
            let rc = super::takes_ctrl_c(|| super::run_status(c));
            eprintln!("CASE past the step {rc:?}");
            return;
        };
        let t0 = std::time::Instant::now();
        let (rc, err) = r.expect("it hung");
        assert_eq!(rc, 143, "{err}");
        assert!(err.contains("CASE undo"), "{err}");
        assert!(!err.contains("CASE past"), "{err}");
        assert!(t0.elapsed() < std::time::Duration::from_secs(6), "{err}");
    }

    #[test]
    fn ctrl_c_a_marked_child_dies_of_stops() {
        let name = "ctrl_c_a_marked_child_dies_of_stops";
        let Some((rc, err)) = ctrl_c(name, true, "exec sleep 1") else {
            return;
        };
        assert_eq!(rc, 130, "{err}");
        assert!(err.contains("CASE undo"), "{err}");
        assert!(!err.contains("CASE past"), "{err}");
    }

    #[test]
    fn ctrl_c_another_child_outlives_stops() {
        let name = "ctrl_c_another_child_outlives_stops";
        let Some((rc, err)) = ctrl_c(name, false, HANDLES) else {
            return;
        };
        assert_eq!(rc, 130, "{err}");
        assert!(err.contains("CASE child got INT"), "{err}");
        assert!(err.contains("CASE undo"), "{err}");
        assert!(!err.contains("CASE past"), "{err}");
    }
}
