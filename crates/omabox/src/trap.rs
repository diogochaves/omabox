//! bash's EXIT trap for what a command made on its way (gpu's and cpu's temporary dir, the monitor
//! an interactive `monitor add` was making): undone when the command ends, however it ends, and on
//! INT, TERM or HUP, the signal then raised again, as bash dies of it after its EXIT trap. A code
//! trap's (on_exit_code) ends with the signal's 128+N instead. Before the undo, main's foreground
//! child is waited for, bounded (foreground_over: bash ran its EXIT trap at once on TERM and HUP,
//! the child left running, finding 453), then, in a busy section of main's (sig::busy: up's),
//! main's in-process step, bounded too (main_at_rest: #367). An INT stops the command as TERM does (finding 454), but
//! for a Ctrl-C from the terminal while a child that takes one runs (sig::takes_ctrl_c: check run's
//! scenario): when that child outlives it, the command goes on, as bash's does; another signal
//! meanwhile stops it (ctrl_c_wait). The first
//! signal decides: once its undo begins, another INT, TERM or HUP changes nothing, and the undo
//! finishes (bash's second signal cut its trap short, or ran the lowest-numbered pending trap's
//! code: finding 452). Nothing new starts meanwhile (sig::run_child). A signal ignored when omabox
//! started stays ignored (sig::catch).

use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

type Undo = Box<dyn FnOnce() + Send>;

static TRAPS: Mutex<Vec<(u64, Undo)>> = Mutex::new(Vec::new());
static NEXT: AtomicU64 = AtomicU64::new(1);
static SIG_W: AtomicI32 = AtomicI32::new(-1);

/// Traps that end with an exit code (`trap 'exit 143' TERM`), not the signal: on_exit_code's.
static CODE_TRAPS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// One thing to undo; undone when dropped (the command's end, a refusal on the way).
pub struct Trap(u64);

/// Undo F when the command ends, or on INT, TERM or HUP.
pub fn on_exit(f: impl FnOnce() + Send + 'static) -> Trap {
    arm();
    let id = NEXT.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut t) = TRAPS.lock() {
        t.push((id, Box::new(f)));
    }
    Trap(id)
}

/// on_exit with bash's `trap 'exit 130' INT; trap 'exit 143' TERM; trap 'exit 129' HUP` beside it
/// (rec_traps, finding 320's): on any of them the undo runs once main's foreground child is over,
/// whatever it died of, then this process exits with that code (not of the signal).
pub fn on_exit_code(f: impl FnOnce() + Send + 'static) -> Trap {
    CODE_TRAPS.fetch_add(1, Ordering::SeqCst);
    let c = CodeCount;
    on_exit(move || {
        drop(c);
        f();
    })
}

/// F, which makes something and arms its undo, as one step: a signal's undo waits for F to finish
/// (UNDOING), so a signal between the making and the arming never leaves what F made (#360). F's
/// children run plainly, as an undo's do: main neither parks nor waits for FOREGROUND inside it.
pub fn made<R>(f: impl FnOnce() -> R) -> R {
    undoing(f)
}

/// The signal (INT, TERM or HUP) whose undo is running or ran; 0 when none came. An undo reads it
/// to tell a command stopped on purpose from one that failed (#388).
static SIGNALLED: AtomicI32 = AtomicI32::new(0);

/// Whether the undos run because a signal stopped the command, not because it failed or ended.
pub fn signalled() -> bool {
    SIGNALLED.load(Ordering::SeqCst) != 0
}

/// Every undo still pending, the last made first (bash's EXIT trap on the way out).
pub fn run_undos() {
    undoing(|| {
        let all: Vec<(u64, Undo)> = TRAPS
            .lock()
            .map(|mut t| std::mem::take(&mut *t))
            .unwrap_or_default();
        for (_, u) in all.into_iter().rev() {
            u();
        }
    });
}

static UNDOING: Mutex<()> = Mutex::new(());

thread_local! {
    /// This thread runs undos (UNDOING is its): an undo that drops a trap of its own runs it there.
    static IN_UNDO: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// F, which takes undos off TRAPS and runs them, one such at a time: a signal's thread and main (a
/// trap dropped at its end or on a refusal) never both, so a signal during main's undo waits for it
/// to finish rather than finding TRAPS empty and ending the process halfway through it. Main's undos
/// run their children plainly, never parked (sig::UNDO_MAIN).
fn undoing<R>(f: impl FnOnce() -> R) -> R {
    if IN_UNDO.with(std::cell::Cell::get) {
        return f();
    }
    let _u = UNDOING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    IN_UNDO.with(|c| c.set(true));
    let main = crate::sig::on_main();
    if main {
        crate::sig::UNDO_MAIN.store(true, Ordering::SeqCst);
    }
    let r = f();
    if main {
        crate::sig::UNDO_MAIN.store(false, Ordering::SeqCst);
    }
    IN_UNDO.with(|c| c.set(false));
    r
}

/// Whether an on_exit_code trap is on (a `trap 'exit 143' TERM` of bash's is set).
pub fn code_active() -> bool {
    CODE_TRAPS.load(Ordering::SeqCst) > 0
}

/// CODE_TRAPS' share of one on_exit_code trap.
struct CodeCount;

impl Drop for CodeCount {
    fn drop(&mut self) {
        CODE_TRAPS.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Trap {
    /// Done with it: nothing left to undo.
    pub fn forget(self) {
        // (under UNDOING: a signal's undo of it already under way finishes first, and ends this)
        let id = self.0;
        undoing(|| {
            take(id);
        });
        std::mem::forget(self);
    }
}

impl Drop for Trap {
    fn drop(&mut self) {
        let id = self.0;
        // (taken under UNDOING: a signal's thread then either runs it, or waits for it)
        undoing(|| {
            if let Some(f) = take(id) {
                f();
            }
        });
    }
}

fn take(id: u64) -> Option<Undo> {
    let mut t = TRAPS.lock().ok()?;
    let i = t.iter().position(|(k, _)| *k == id)?;
    Some(t.remove(i).1)
}

/// Whether a foreground child ran when the signal came, read in the handler: the thread wakes later,
/// when that step may be over and an in-process one begun (where bash, had it been there, would end
/// at once: the safe side).
static SIG_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The pipe's byte for a signal the kernel sent (SI_KERNEL: a terminal's Ctrl-C to its foreground
/// process group), beside the signal's number; one a process sent (kill) has it clear.
const FROM_TTY: u8 = 0x80;

extern "C" fn on_signal(sig: libc::c_int, info: *mut libc::siginfo_t, _: *mut libc::c_void) {
    let _e = crate::sig::Errno::save();
    SIG_BUSY.store(
        crate::sig::FG_ACTIVE.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );
    // Nothing new starts from here (the thread lets main go on after a Ctrl-C its child outlived;
    // it clears this then).
    crate::sig::STOPPING.store(true, Ordering::SeqCst);
    // SAFETY: the kernel's siginfo for this delivery (SA_SIGINFO), read only.
    let tty = !info.is_null() && unsafe { (*info).si_code } == libc::SI_KERNEL;
    let fd = SIG_W.load(Ordering::Relaxed);
    if fd >= 0 {
        let b = [u8::try_from(sig).unwrap_or(0) | if tty { FROM_TTY } else { 0 }];
        // SAFETY: write is async-signal-safe; b outlives the call.
        unsafe {
            libc::write(fd, b.as_ptr().cast(), 1);
        }
    }
}

/// How long a signal's stop waits for main's foreground child, then once more after a TERM to it,
/// then after a KILL (each step's end shortened in the unit tests).
#[cfg(not(test))]
const FG_GRACE: [std::time::Duration; 3] = [
    std::time::Duration::from_secs(10),
    std::time::Duration::from_secs(5),
    std::time::Duration::from_secs(5),
];
#[cfg(test)]
const FG_GRACE: [std::time::Duration; 3] = [
    std::time::Duration::from_millis(1000),
    std::time::Duration::from_millis(500),
    std::time::Duration::from_millis(500),
];

/// Main's foreground child over (sig::FOREGROUND held), so the undo comes after what that step did
/// (check run's down after its box's up; gpu's and cpu's dir removed once nothing writes to it): at
/// most FG_GRACE[0], then the child is sent a TERM (not at once: a child that is itself omabox would
/// get the signal in its own cleanup), FG_GRACE[1] more, then a KILL. None when even then main has not
/// let go (what the child left running holds its pipes): the undo goes on regardless (finding 453).
pub(crate) fn foreground_over() -> Option<std::sync::MutexGuard<'static, ()>> {
    let t0 = std::time::Instant::now();
    let mut said = false;
    for (i, grace) in FG_GRACE.iter().enumerate() {
        let end = std::time::Instant::now() + *grace;
        loop {
            match crate::sig::FOREGROUND.try_lock() {
                Ok(g) => return Some(g),
                Err(std::sync::TryLockError::Poisoned(p)) => return Some(p.into_inner()),
                Err(std::sync::TryLockError::WouldBlock) => {}
            }
            // (said once the wait is felt: the stop is on its way)
            if !said && t0.elapsed() >= SAY_AFTER {
                said = true;
                say_stopping();
            }
            if std::time::Instant::now() >= end {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if let Some(s) = [libc::SIGTERM, libc::SIGKILL].get(i) {
            crate::sig::signal_foreground(*s);
        }
    }
    None
}

/// What a stop says once it has waited SAY_AFTER for the step in progress (finding 453).
pub(crate) const STOPPING_LINE: &str =
    "omabox: stopping once the step in progress ends (at most 20 s)\n";
const SAY_AFTER: std::time::Duration = std::time::Duration::from_secs(1);

/// STOPPING_LINE said, once in this process.
fn say_stopping() {
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !SAID.swap(true, Ordering::SeqCst) {
        let _ = std::io::Write::write_all(&mut std::io::stderr(), STOPPING_LINE.as_bytes());
    }
}

/// Main at rest (sig::main_rests: parked, or waiting for FOREGROUND, which the caller holds) when it
/// is in a busy section, so the undo never runs alongside main's in-process work there (up's box dir
/// made, its HOME seeded: a dir made again after the undo removed it, #367, finding 576): at most
/// FG_GRACE[0], then the undo goes on regardless (in-process work that blocks, as a child that will
/// not end).
fn main_at_rest() {
    let t0 = std::time::Instant::now();
    while !crate::sig::main_rests() {
        if t0.elapsed() >= SAY_AFTER {
            say_stopping();
        }
        if t0.elapsed() >= FG_GRACE[0] {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// What a Ctrl-C a marked child may outlive comes to (ctrl_c_wait).
enum CtrlC {
    /// The child is over and did not die of it: the command goes on.
    GoesOn,
    /// The child died of it: the command stops (FOREGROUND held).
    Stops(std::sync::MutexGuard<'static, ()>),
    /// Another INT, TERM or HUP came first: that one stops the command.
    Then(libc::c_int),
}

/// The marked child's end, waited for as long as it runs, while F, the handler's pipe, is still read:
/// a signal meanwhile is not left unread behind the wait.
fn ctrl_c_wait(f: &std::fs::File) -> CtrlC {
    use std::os::fd::AsRawFd;
    loop {
        let got = match crate::sig::FOREGROUND.try_lock() {
            Ok(g) => Some(g),
            Err(std::sync::TryLockError::Poisoned(p)) => Some(p.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => None,
        };
        if let Some(g) = got {
            return if crate::sig::FG_INT.load(Ordering::SeqCst) {
                CtrlC::Stops(g)
            } else {
                CtrlC::GoesOn
            };
        }
        let mut p = libc::pollfd {
            fd: f.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one pollfd of ours, 20 ms at most.
        if unsafe { libc::poll(&mut p, 1, 20) } == 1 && p.revents & libc::POLLIN != 0 {
            let mut b = [0u8; 1];
            // SAFETY: a one-byte read into b from the pipe poll said is readable.
            if unsafe { libc::read(f.as_raw_fd(), b.as_mut_ptr().cast(), 1) } == 1 {
                return CtrlC::Then(libc::c_int::from(b[0] & !FROM_TTY));
            }
        }
    }
}

/// The handlers and the thread that undoes on a signal, once.
fn arm() {
    if SIG_W.load(Ordering::SeqCst) >= 0 {
        return;
    }
    let Ok((r, w)) = crate::exec::pipe() else {
        return;
    };
    use std::os::fd::IntoRawFd;
    SIG_W.store(w.into_raw_fd(), Ordering::SeqCst);
    for s in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        crate::sig::catch_info(s, on_signal);
    }
    std::thread::spawn(move || {
        let mut f = std::fs::File::from(r);
        loop {
            let mut b = [0u8; 1];
            if std::io::Read::read_exact(&mut f, &mut b).is_err() {
                return;
            }
            let mut sig = libc::c_int::from(b[0] & !FROM_TTY);
            // A Ctrl-C from the terminal while a child that takes one ran (sig::takes_ctrl_c):
            // waited for as long as that child runs; when it did not die of it, the command goes on,
            // as bash's does. With a code trap on (bash's `trap 'exit 130' INT`) never.
            let ctrl_c = sig == libc::SIGINT
                && b[0] & FROM_TTY != 0
                && SIG_BUSY.swap(false, Ordering::SeqCst)
                && CODE_TRAPS.load(Ordering::SeqCst) == 0;
            crate::sig::STOPPING.store(true, Ordering::SeqCst);
            let fg = if ctrl_c {
                match ctrl_c_wait(&f) {
                    CtrlC::GoesOn => {
                        crate::sig::STOPPING.store(false, Ordering::SeqCst);
                        continue;
                    }
                    CtrlC::Stops(g) => Some(g),
                    // (another signal meanwhile, a second Ctrl-C too: it stops the command)
                    CtrlC::Then(s) => {
                        sig = s;
                        foreground_over()
                    }
                }
            } else {
                // Every other INT, TERM or HUP stops the command: its foreground child waited for,
                // bounded (an INT sent by a process, `kill -INT`, among them: finding 454).
                foreground_over()
            };
            // (main's in-process step over too, once its foreground child is: not when that child
            // would not end, main stuck in it)
            if fg.is_some() {
                main_at_rest();
            }
            // From here a second INT, TERM or HUP changes nothing: the thread reads no more of
            // them, and the undo finishes (finding 452).
            // (FOREGROUND held, when it was had, till this process ends: main starts nothing more)
            let _fg = fg;
            // (read before the undo, which drops the code traps' count with them)
            let code = CODE_TRAPS.load(Ordering::SeqCst) > 0;
            SIGNALLED.store(sig, Ordering::SeqCst);
            run_undos();
            if code {
                // (bash's `exit 130|143|129`, its EXIT trap run on the way: the first signal's)
                let _ = std::io::Write::flush(&mut std::io::stdout());
                // SAFETY: _exit ends this process at once, nothing left to run.
                unsafe { libc::_exit(128 + sig) }
            }
            // SAFETY: the default disposition back, then the signal to this process: it ends of it,
            // as bash does after its EXIT trap.
            unsafe {
                libc::signal(sig, libc::SIG_DFL);
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, sig);
                libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
                libc::kill(libc::getpid(), sig);
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
            std::process::exit(128 + sig);
        }
    });
}

/// No trap, but this module's way with a signal: INT, TERM and HUP stop the command once its
/// foreground step is over (bounded), and end it of the signal; a Ctrl-C a child that takes one
/// outlives lets it go on.
pub fn stop_on_signals() {
    arm();
    // (once more, over another module's handlers: `trap - INT TERM HUP` after record start)
    for s in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        crate::sig::catch_info(s, on_signal);
    }
}

/// `sleep SECS` (decimal seconds, as omabox's durations are written), in-process: a signal's stop
/// parks main at once (sig::sleep), where bash's `sleep` was a child it waited for.
pub fn sleep(secs: &[u8]) {
    let s = std::str::from_utf8(secs)
        .ok()
        .and_then(|t| t.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(0.0);
    crate::sig::sleep(std::time::Duration::from_secs_f64(s.min(1e9)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // #360: a signal's undo that starts while made's F runs waits for F, so it finds what F made
    // already armed, never made and not yet armed.
    #[test]
    fn a_signals_undo_waits_for_made() {
        let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(None));
        let (a, s) = (armed.clone(), seen.clone());
        made(|| {
            let t = std::thread::spawn(move || {
                undoing(|| *s.lock().unwrap() = Some(a.load(Ordering::SeqCst)));
            });
            // (the other thread asks for the undo meanwhile)
            std::thread::sleep(std::time::Duration::from_millis(100));
            armed.store(true, Ordering::SeqCst);
            t
        })
        .join()
        .unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            Some(true),
            "the undo ran after F armed"
        );
    }

    // t_unit_monitors' (an interactive monitor that fails in `up`): the add's own undo, made inside
    // up's, runs when the add ends, and leaves up's in place: up's runs when up ends (bash's
    // monitor_add_window replaced up's EXIT trap with its own, and put it back).
    #[test]
    fn a_nested_undo_leaves_the_outer() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (l1, l2) = (log.clone(), log.clone());
        let up = on_exit_code(move || l1.lock().unwrap().push("up's"));
        {
            let _add = on_exit(move || l2.lock().unwrap().push("the add's"));
        }
        assert_eq!(
            *log.lock().unwrap(),
            ["the add's"],
            "the add's undo, as it ends"
        );
        drop(up);
        assert_eq!(
            *log.lock().unwrap(),
            ["the add's", "up's"],
            "...and up's own after it, as up ends"
        );
    }
}
