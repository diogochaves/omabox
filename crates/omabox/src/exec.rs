//! Entering a box: on_box (a tool against the box's compositor, in its user and mount namespaces,
//! finding 63) and box_exec (a command in all of its namespaces, with the session's environment).
//! Both natively, through enter.rs (setns where bash ran nsenter: #324, finding 490). What the entry
//! says when it fails is held apart from the command's own stderr and said after it (ns_held,
//! ns_said: #166); the command's stderr is the caller's from the exec on, so no box process ever
//! holds that pipe. A command in the box's pid namespace gets no fd of the host's a box process
//! could open again (finding 325: the relays, a terminal of its own), none but the ones it is given
//! (finding 345: fds_only), and no controlling terminal of the caller's (finding 491).

use crate::boxes::{self, BoxPid, box_env, box_home, box_pid, covers_held, run_read};
use crate::cat;
use crate::exit::{Ctx, Exit, Res};
use crate::jail;
use crate::jobs::job_how;
use crate::json;
use crate::procfs;
use crate::shell;
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

/// What ns_held keeps of the entry's stderr, and how long it waits for the rest once the command is
/// over (a box process that keeps it open does not hold the call).
const NS_HELD_MAX: usize = 4096;
const NS_HELD_WAIT: Duration = Duration::from_secs(1);

// --- fds ------------------------------------------------------------------------------------------

/// A pipe, both ends close-on-exec.
pub fn pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as RawFd; 2];
    // SAFETY: fds has room for the two descriptors pipe2 writes.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: both were just opened and are ours alone.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// FD again, at 10 or above (as bash's {var} redirections), close-on-exec: a number a child is told
/// about, and that a dup2 to 3 never lands on.
pub fn dup_hi(fd: RawFd) -> std::io::Result<OwnedFd> {
    // SAFETY: F_DUPFD_CLOEXEC takes an int; it only reads the descriptor table.
    let n = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 10) };
    if n < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a new descriptor of ours.
    Ok(unsafe { OwnedFd::from_raw_fd(n) })
}

pub fn dev_null(write: bool) -> std::io::Result<OwnedFd> {
    let f = std::fs::OpenOptions::new()
        .read(!write)
        .write(write)
        .open("/dev/null")?;
    Ok(f.into())
}

fn fstat(fd: RawFd) -> Option<libc::stat> {
    // SAFETY: stat is plain data fstat fills in.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: st is ours; a closed fd is EBADF, nothing else.
    (unsafe { libc::fstat(fd, &mut st) } == 0).then_some(st)
}

fn is_fifo_or_socket(st: &libc::stat) -> bool {
    matches!(st.st_mode & libc::S_IFMT, libc::S_IFIFO | libc::S_IFSOCK)
}

/// fd_exposed: fd N is one a box process could open again through /proc/PID/fd of a command given it
/// (finding 325): a file, a directory, a terminal, any device but /dev/null. Not a pipe or a socket,
/// nor /dev/null, nor an fd that is not open.
pub fn fd_exposed(fd: RawFd) -> bool {
    let Some(st) = fstat(fd) else { return false };
    if is_fifo_or_socket(&st) {
        return false;
    }
    if st.st_mode & libc::S_IFMT == libc::S_IFCHR {
        let null = std::fs::metadata("/dev/null");
        return !null.is_ok_and(|m| m.dev() == st.st_dev && m.ino() == st.st_ino);
    }
    true
}

pub fn isatty(fd: RawFd) -> bool {
    // SAFETY: isatty only looks at the descriptor.
    unsafe { libc::isatty(fd) == 1 }
}

/// `[ /dev/fd/A -ef /dev/fd/B ]`.
fn same_file(a: RawFd, b: RawFd) -> bool {
    match (fstat(a), fstat(b)) {
        (Some(x), Some(y)) => x.st_dev == y.st_dev && x.st_ino == y.st_ino,
        _ => false,
    }
}

/// `[ -x PATH ]`.
pub fn executable(p: &[u8]) -> bool {
    std::ffi::CString::new(p.to_vec()).is_ok_and(|c| {
        // SAFETY: c is a valid C string for the call.
        unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 }
    })
}

/// The fds open in this process now, past stdio: what fds_only closes in a child.
fn open_fds() -> Vec<RawFd> {
    std::fs::read_dir("/proc/self/fd")
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.file_name().to_str()?.parse::<RawFd>().ok())
                .filter(|&n| n > 2)
                .collect()
        })
        .unwrap_or_default()
}

/// The hard core limit is 0: no cores to skip (nocore).
fn core_hard_zero() -> bool {
    // SAFETY: rlimit is plain data getrlimit fills in.
    let mut r: libc::rlimit = unsafe { std::mem::zeroed() };
    // SAFETY: r is ours.
    unsafe { libc::getrlimit(libc::RLIMIT_CORE, &mut r) == 0 && r.rlim_max == 0 }
}

/// `F >/dev/null` or `F 2>/dev/null` for a stretch of this process: fd FD is /dev/null until the guard
/// drops, as bash's redirection of a function's call is for every word said in it and every child it
/// starts. Only where no other thread writes to FD meanwhile.
pub struct Redir {
    fd: RawFd,
    saved: Option<OwnedFd>,
}

impl Redir {
    pub fn null(fd: RawFd) -> Redir {
        match dev_null(true) {
            Ok(n) => Redir::to(fd, n.as_raw_fd()),
            Err(_) => Redir { fd, saved: None },
        }
    }

    /// `F 2>"$file"`: fd FD the same as TO (a file of the caller's) until the guard drops.
    pub fn to(fd: RawFd, to: RawFd) -> Redir {
        let _ = std::io::stdout().flush();
        let saved = dup_hi(fd).ok();
        // SAFETY: both fds are open; dup2 only changes this process's descriptor table.
        unsafe {
            libc::dup2(to, fd);
        }
        Redir { fd, saved }
    }
}

impl Drop for Redir {
    fn drop(&mut self) {
        let _ = std::io::stdout().flush();
        if let Some(s) = &self.saved {
            // SAFETY: as in null: the fd saved there back in its place.
            unsafe {
                libc::dup2(s.as_raw_fd(), self.fd);
            }
        }
    }
}

// --- children -------------------------------------------------------------------------------------

/// How a child is started past its stdio: the fds it keeps by number (KEEP), one given it as fd 3
/// (run -d's), fds_only (every other fd closed), nocore (a core limit of 1 byte, finding 67:
/// `prlimit --core=1:`, done here before the exec).
#[derive(Default)]
pub struct Fds {
    pub keep: Vec<RawFd>,
    pub fd3: Option<RawFd>,
    pub only: bool,
    pub nocore: bool,
    /// Where held_run says the child's pid once started, and 0 once it is reaped (`$!`, for a
    /// caller that may have to kill it: settle_end's kill_tree).
    pub pid: Option<Arc<AtomicI32>>,
    /// Started as bash starts what it backgrounds (`&`, coproc, a process substitution): SIGINT and
    /// SIGQUIT ignored, so a Ctrl-C or Ctrl-\ at the terminal leaves it running; a tool holding a
    /// key or a button lets go once its stdin closes (finding 112).
    pub bg: bool,
    /// Started inside a box (enter.rs), first of all, before the fds are closed.
    pub enter: Option<crate::enter::Plan>,
}

/// What bash's `&` does to a child's signals (no job control): SIGINT and SIGQUIT ignored.
pub fn background(c: &mut Command) {
    // SAFETY: signal is async-signal-safe; between fork and exec it changes only the child.
    unsafe {
        c.pre_exec(|| {
            libc::signal(libc::SIGINT, libc::SIG_IGN);
            libc::signal(libc::SIGQUIT, libc::SIG_IGN);
            Ok(())
        });
    }
}

pub fn command(argv: &[Vec<u8>], fds: Fds) -> Command {
    let mut c = crate::sig::command(OsStr::from_bytes(argv.first().map_or(&b""[..], |a| a)));
    c.args(argv.iter().skip(1).map(|a| OsStr::from_bytes(a)));
    // Into the box's pid namespace, only the fds it is given, whatever the caller asked (finding
    // 345: a box process could open the others again through /proc/PID/fd; t_unit_ns_fds).
    let only = fds.only || fds.enter.is_some_and(|p| p.pid_ns());
    if let Some(p) = fds.enter {
        p.apply(&mut c);
    }
    let open = if only { open_fds() } else { Vec::new() };
    let nocore = fds.nocore && !core_hard_zero();
    // SAFETY: the closure only makes async-signal-safe calls (dup2, close, fcntl, signal,
    // getrlimit, setrlimit) on memory it owns, between fork and exec.
    unsafe {
        c.pre_exec(move || {
            if let Some(src) = fds.fd3 {
                if libc::dup2(src, 3) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            for &fd in &open {
                if !fds.keep.contains(&fd) && !(fds.fd3.is_some() && fd == 3) {
                    libc::close(fd);
                }
            }
            for &fd in &fds.keep {
                libc::fcntl(fd, libc::F_SETFD, 0);
            }
            if fds.bg {
                libc::signal(libc::SIGINT, libc::SIG_IGN);
                libc::signal(libc::SIGQUIT, libc::SIG_IGN);
            }
            if nocore {
                let mut r: libc::rlimit = std::mem::zeroed();
                if libc::getrlimit(libc::RLIMIT_CORE, &mut r) == 0 {
                    r.rlim_cur = 1;
                    libc::setrlimit(libc::RLIMIT_CORE, &r);
                }
            }
            Ok(())
        });
    }
    c
}

/// A child's end as bash's `$?`: its status, or 128+N for signal N; and the signal.
pub fn status(s: std::process::ExitStatus) -> (i32, Option<i32>) {
    if s.signal() == Some(libc::SIGINT) {
        crate::sig::FG_INT.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    match (s.code(), s.signal()) {
        (Some(c), _) => (c, None),
        (None, Some(sig)) => (128 + sig, Some(sig)),
        _ => (1, None),
    }
}

fn wait(child: &mut Child) -> (i32, Option<i32>) {
    match child.wait() {
        Ok(s) => status(s),
        Err(_) => (1, None),
    }
}

/// A pipe written from a thread of its own (a process substitution's `<(printf ...)`): the bytes,
/// then its end. A reader that goes first gets no SIGPIPE here: blocked in that thread, a write
/// fails instead.
pub fn feed(w: OwnedFd, data: Vec<u8>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        // SAFETY: a signal set of our own, for this thread's mask only.
        unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, libc::SIGPIPE);
            libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        }
        let mut f = std::fs::File::from(w);
        let _ = f.write_all(&data);
    })
}

/// What a reader of R gets, up to LIMIT bytes (None: all) and its EOF; past the limit R is closed,
/// as `| head -c N` does (a writer then gets SIGPIPE).
pub fn read_all(r: OwnedFd, limit: Option<usize>) -> Vec<u8> {
    let mut f = std::fs::File::from(r);
    let mut out = Vec::new();
    match limit {
        Some(n) => {
            let _ = (&mut f).take(n as u64).read_to_end(&mut out);
        }
        None => {
            let _ = f.read_to_end(&mut out);
        }
    }
    out
}

// --- ns_held / ns_said -------------------------------------------------------------------------------

/// The entry's own words, kept apart from the command's (#166): its stderr goes to a pipe read here
/// by a thread, the first NS_HELD_MAX bytes kept, the rest read into nothing, until its writers are
/// gone, or NS_HELD_WAIT after the command ended (then whatever came).
struct Held {
    buf: Arc<Mutex<Vec<u8>>>,
    done: mpsc::Receiver<()>,
    stop: Arc<AtomicBool>,
}

fn hold(r: OwnedFd) -> Held {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, done) = mpsc::channel();
    let (b, s) = (buf.clone(), stop.clone());
    std::thread::spawn(move || {
        let mut tmp = [0u8; 8192];
        let mut pfd = libc::pollfd {
            fd: r.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        while !s.load(Ordering::Relaxed) {
            // SAFETY: one pollfd of ours.
            let n = unsafe { libc::poll(&mut pfd, 1, 50) };
            if n <= 0 {
                continue;
            }
            // SAFETY: tmp is ours, its length given.
            let got = unsafe { libc::read(r.as_raw_fd(), tmp.as_mut_ptr().cast(), tmp.len()) };
            if got == 0
                || (got < 0
                    && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted)
            {
                break;
            }
            let got = usize::try_from(got).unwrap_or(0);
            if let Ok(mut v) = b.lock() {
                let room = NS_HELD_MAX.saturating_sub(v.len());
                v.extend_from_slice(&tmp[..got.min(room)]);
            }
        }
        let _ = tx.send(());
    });
    Held { buf, done, stop }
}

impl Held {
    /// NS_HELD: what came, up to its first NUL (`read -d ''`).
    fn take(self) -> Vec<u8> {
        let _ = self.done.recv_timeout(NS_HELD_WAIT);
        self.stop.store(true, Ordering::Relaxed);
        let v = self.buf.lock().map(|v| v.clone()).unwrap_or_default();
        match v.iter().position(|&c| c == 0) {
            Some(i) => v[..i].to_vec(),
            None => v,
        }
    }
}

/// Where a helper's own words go: the command's stderr as the caller set it (`2>&1` into a capture,
/// `2>/dev/null`, or this process's).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ErrTo {
    Inherit,
    Null,
    Out,
    /// An fd of the caller's (`2>&FD`: settle_begin's tool, its words to the pipe read with its
    /// answer).
    Fd(RawFd),
}

/// The words ns_said says, given what the entry's stderr held and the command's end RC: its own lines
/// whole (at most 3, printable ASCII, 400 at most), then, with KILLED (run's own command, #303), how
/// the command was killed, then how many bytes of anything else were dropped. Only what the entry's
/// stderr held is counted (bash's own job line for a killed command was counted too, and its
/// "killed by" said only when that line was its own: finding 440).
pub fn said_text(held: &[u8], rc: i32, killed: bool) -> Vec<u8> {
    let mut lines: Vec<&[u8]> = Vec::new();
    if let Some(last) = held.iter().rposition(|&c| c == b'\n') {
        lines = held[..last].split(|&c| c == b'\n').collect();
    }
    let (mut out, mut n, mut kept) = (Vec::new(), 0, 0usize);
    for l in lines {
        let l: Vec<u8> = l
            .iter()
            .copied()
            .filter(|c| (0x20..0x7f).contains(c))
            .collect();
        if n < 3 && l.len() <= 400 && own_line(&l) {
            out.extend_from_slice(&l);
            out.push(b'\n');
            n += 1;
            kept += l.len() + 1;
        }
    }
    if killed {
        out.extend_from_slice(&cat!(
            "omabox: the command ",
            job_how(rc.to_string().as_bytes()),
            "\n"
        ));
    }
    let dropped = held.len().saturating_sub(kept);
    if dropped > 0 {
        out.extend_from_slice(
            format!("omabox: dropped {dropped} bytes of other output on the box entry's stderr\n")
                .as_bytes(),
        );
    }
    out
}

/// ns_own_line: the entry's own (enter.rs's FAIL). Nothing else writes there now (bash's held
/// nsenter's, prlimit's and env's, and a box process could reach it: finding 490).
fn own_line(l: &[u8]) -> bool {
    let p = crate::enter::FAIL.as_bytes();
    l.starts_with(p) && l.len() > p.len()
}

/// What a sink gets: this process's stderr, nothing, or the capture (BUF).
fn say_to(err: ErrTo, buf: &mut Vec<u8>, m: &[u8]) {
    match err {
        ErrTo::Inherit => {
            let _ = std::io::stderr().write_all(m);
        }
        ErrTo::Null => {}
        ErrTo::Out => buf.extend_from_slice(m),
        ErrTo::Fd(fd) => {
            // SAFETY: the caller's fd, open while the call runs; m is valid for its length.
            let _ = unsafe { libc::write(fd, m.as_ptr().cast(), m.len()) };
        }
    }
}

/// ns_said RC [WORDS]: on a box that went down meanwhile (its PID 1 can outlive the command by up to
/// 2 s), WORDS instead (BOX_GONE_SAID), or nothing; else said_text. Asked only when the command
/// failed with words held, or was killed by SIGKILL (what the box's end does to what runs in it). A
/// command killed by SIG (but INT and PIPE, which a shell says nothing of either) is said under
/// KILL_SAY. A box this namespace cannot read (box_alive's refusal) ends it, said where the rest
/// would go.
#[allow(clippy::too_many_arguments)]
fn ns_said(
    ctx: &Ctx,
    held: &[u8],
    sig: Option<i32>,
    rc: i32,
    words: Option<&[u8]>,
    kill_say: bool,
    err: ErrTo,
    buf: &mut Vec<u8>,
) -> Result<bool, u8> {
    ns_said_with(
        &|| alive_quiet(ctx),
        held,
        sig,
        rc,
        words,
        kill_say,
        err,
        buf,
    )
}

/// ns_said, whether the box is up asked of ALIVE (alive_quiet).
#[allow(clippy::too_many_arguments)]
fn ns_said_with(
    alive: &dyn Fn() -> Result<bool, (Vec<u8>, u8)>,
    held: &[u8],
    sig: Option<i32>,
    rc: i32,
    words: Option<&[u8]>,
    kill_say: bool,
    err: ErrTo,
    buf: &mut Vec<u8>,
) -> Result<bool, u8> {
    let killed = kill_say && sig.is_some_and(|s| s != libc::SIGINT && s != libc::SIGPIPE);
    let box_end = sig == Some(libc::SIGKILL);
    if held.is_empty() && !killed && !box_end {
        return Ok(false);
    }
    if rc != 0 && (!held.is_empty() || box_end) {
        for _ in 0..10 {
            match alive() {
                Ok(true) => {}
                Ok(false) => {
                    if let Some(w) = words {
                        say_to(err, buf, &cat!("omabox: ", w, "\n"));
                        return Ok(true);
                    }
                    return Ok(false);
                }
                Err((m, code)) => {
                    say_to(err, buf, &m);
                    return Err(code);
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    say_to(err, buf, &said_text(held, rc, killed));
    Ok(false)
}

/// box_alive, its refusal's message returned rather than said (it goes where the caller's stderr is).
fn alive_quiet(ctx: &Ctx) -> Result<bool, (Vec<u8>, u8)> {
    match box_pid(&ctx.d) {
        BoxPid::Unknown => {
            let m = cat!(
                "omabox: cannot tell whether box '",
                ctx.name,
                "' is up from this user namespace (a sandbox's?): run omabox outside it\n"
            );
            Err((m, ctx.die_rc.unwrap_or(1)))
        }
        BoxPid::Pid(p) => Ok(procfs::running(&p)),
        BoxPid::None => Ok(false),
    }
}

// --- on_box ---------------------------------------------------------------------------------------

/// Where a command's stdout goes.
pub enum Out {
    Inherit,
    Null,
    /// To this fd (a file of the caller's, a pipe it reads).
    Fd(OwnedFd),
    /// Read here, as `$(...)`, up to so many bytes (`| head -c N`).
    Capture(Option<usize>),
}

/// How a call through on_box ended: its status (DIE_RC's for a refusal, said where its stderr goes),
/// what was captured, and whether ns_said said the box went down.
pub struct Ran {
    pub rc: i32,
    pub out: Vec<u8>,
    pub refused: bool,
    pub gone_said: bool,
}

impl Ran {
    /// Outside `$(...)`, a refusal ends the command (on_box's die).
    pub fn or_exit(self) -> Res<Ran> {
        if self.refused {
            return Err(Exit(u8::try_from(self.rc).unwrap_or(1)));
        }
        Ok(self)
    }
}

/// on_box ARGS: a tool against the box's compositor (hyprctl, cat, tail, stat), inside the box's
/// user and mount namespaces with a clean environment, where every path resolves as the box sees it
/// (finding 63). Not its pid namespace (finding 307). stdin as the caller's (or /dev/null with
/// STDIN_NULL); stdout and stderr where OUT and ERR say.
pub fn on_box(ctx: &Ctx, args: &[Vec<u8>], stdin_null: bool, out: Out, err: ErrTo) -> Ran {
    let stdin = if stdin_null { In::Null } else { In::Inherit };
    on_box_io(ctx, args, stdin, out, err, None)
}

/// on_box with its stdin any fd (settle_begin's pipe), and its tool's pid said in PID (`$!` of a
/// background on_box).
pub fn on_box_io(
    ctx: &Ctx,
    args: &[Vec<u8>],
    stdin: In,
    out: Out,
    err: ErrTo,
    pid_to: Option<Arc<AtomicI32>>,
) -> Ran {
    on_box_with(ctx, args, stdin, out, err, pid_to, false, None)
}

/// on_box while up still waits for the box's Hyprland, before share/hyprland.lua wrote its
/// omabox.env (#406): a tool against the Hyprland of instance SIG (a tame name the caller checked),
/// with no WAYLAND_DISPLAY; stdin /dev/null.
pub fn on_box_starting(ctx: &Ctx, sig: &[u8], args: &[Vec<u8>], out: Out, err: ErrTo) -> Ran {
    on_box_with(ctx, args, In::Null, out, err, None, false, Some(sig))
}

/// on_box_io for what bash starts in the background (`&`, coproc, `<(...)`): SIGINT and SIGQUIT
/// ignored in it (Fds::bg).
pub fn on_box_bg(
    ctx: &Ctx,
    args: &[Vec<u8>],
    stdin: In,
    out: Out,
    err: ErrTo,
    pid_to: Option<Arc<AtomicI32>>,
) -> Ran {
    on_box_with(ctx, args, stdin, out, err, pid_to, true, None)
}

/// What on_box says of a box that went down as it entered.
fn gone_words(name: &[u8]) -> Vec<u8> {
    cat!(
        "box '",
        name,
        "' went down (why: omabox log -b ",
        name,
        " all; clear it with: omabox down ",
        name,
        ")"
    )
}

#[allow(clippy::too_many_arguments)]
fn on_box_with(
    ctx: &Ctx,
    args: &[Vec<u8>],
    stdin: In,
    out: Out,
    err: ErrTo,
    pid_to: Option<Arc<AtomicI32>>,
    bg: bool,
    starting: Option<&[u8]>,
) -> Ran {
    let mut buf = Vec::new();
    let refuse = |buf: &mut Vec<u8>, m: Vec<u8>| {
        say_to(err, buf, &cat!("omabox: ", m, "\n"));
        Ran {
            rc: i32::from(ctx.die_rc.unwrap_or(1)),
            out: std::mem::take(buf),
            refused: true,
            gone_said: false,
        }
    };
    let pid = match box_pid(&ctx.d) {
        BoxPid::Pid(p) => p,
        // (box_pid's 3 leaves it empty: callers run it in an || list, where set -e does not hold.)
        BoxPid::None | BoxPid::Unknown => {
            return refuse(&mut buf, cat!("box '", ctx.name, "' is not up"));
        }
    };
    if covers_held(ctx).is_err() {
        // (covers_held's die, its words to wherever this call's stderr is)
        return refuse(
            &mut buf,
            cat!(
                "box '",
                ctx.name,
                "' is still starting: what its jail hides is not checked yet (omabox down ",
                ctx.name,
                " ends it)"
            ),
        );
    }
    let (wd, sig) = if let Some(s) = starting {
        (None, s.to_vec())
    } else {
        let Some(wd) = box_env(&ctx.d, "WAYLAND_DISPLAY") else {
            return refuse(
                &mut buf,
                cat!("box '", ctx.name, "': no WAYLAND_DISPLAY in its omabox.env"),
            );
        };
        let Some(sig) = box_env(&ctx.d, "HYPRLAND_INSTANCE_SIGNATURE") else {
            return refuse(
                &mut buf,
                cat!(
                    "box '",
                    ctx.name,
                    "': no HYPRLAND_INSTANCE_SIGNATURE in its omabox.env"
                ),
            );
        };
        (Some(wd), sig)
    };
    // One of the helpers up bound: whether the box still has the host's (#421).
    if crate::helpers::runs_helper(args) {
        crate::helpers::warn_stale(ctx);
    }
    let lang = shell::env("LANG")
        .filter(|l| !l.is_empty())
        .unwrap_or(b"C.UTF-8".to_vec());
    // The tool itself, with a clean environment (no env -i, no sh between: the entry does the rest).
    let mut env: Vec<(Vec<u8>, Vec<u8>)> = vec![(b"PATH".to_vec(), b"/usr/bin".to_vec())];
    env.push((
        b"XDG_RUNTIME_DIR".to_vec(),
        format!("/run/user/{}", procfs::uid()).into_bytes(),
    ));
    if let Some(wd) = wd {
        env.push((b"WAYLAND_DISPLAY".to_vec(), wd));
    }
    env.push((b"HYPRLAND_INSTANCE_SIGNATURE".to_vec(), sig));
    env.push((b"LANG".to_vec(), lang));
    // (a crash of one of omabox's tools there never notifies the desktop either: finding 492)
    let fds = Fds {
        pid: pid_to,
        bg,
        nocore: true,
        ..Fds::default()
    };
    let r = match crate::enter::Entry::open(&ctx.d, &pid, crate::enter::TOOL) {
        Err(words) => Ok(HeldRun {
            rc: 1,
            sig: None,
            out: Vec::new(),
            held: words,
        }),
        Ok(entry) => held_run(&entry, args, Some(env), stdin, out, err, fds),
    };
    let r = match r {
        Ok(r) => r,
        Err(e) => {
            let (why, rc) = exec_failed(args.first().map_or(&b""[..], |a| a), &e);
            say_to(
                err,
                &mut buf,
                &cat!(
                    "omabox: cannot run ",
                    args.first().cloned().unwrap_or_default(),
                    " in box '",
                    ctx.name,
                    "': ",
                    why,
                    "\n"
                ),
            );
            return Ran {
                rc: i32::from(rc),
                out: buf,
                refused: false,
                gone_said: false,
            };
        }
    };
    buf = r.out;
    let words = gone_words(&ctx.name);
    match ns_said(
        ctx,
        &r.held,
        r.sig,
        r.rc,
        Some(&words),
        false,
        err,
        &mut buf,
    ) {
        Ok(gone_said) => Ran {
            rc: r.rc,
            out: buf,
            refused: false,
            gone_said,
        },
        Err(code) => Ran {
            rc: i32::from(code),
            out: buf,
            refused: true,
            gone_said: false,
        },
    }
}

/// A command's stdin.
pub enum In {
    Inherit,
    Null,
    Fd(OwnedFd),
}

/// What held_run gives back: the status, the signal that ended it, what was captured (and said
/// to ErrTo::Out), what the entry's stderr held.
struct HeldRun {
    rc: i32,
    sig: Option<i32>,
    out: Vec<u8>,
    held: Vec<u8>,
}

/// ns_held ARGV: ARGV run in the box ENTRY names (with ENV alone, when given), its stderr held (hold)
/// until it is in, then the caller's (ERR).
fn held_run(
    entry: &crate::enter::Entry,
    argv: &[Vec<u8>],
    env: Option<Vec<(Vec<u8>, Vec<u8>)>>,
    stdin: In,
    out: Out,
    err: ErrTo,
    mut fds: Fds,
) -> std::io::Result<HeldRun> {
    let (hr, hw) = pipe()?;
    let (cap, limit, stdout) = match out {
        Out::Inherit => (None, None, Stdio::inherit()),
        Out::Null => (None, None, Stdio::null()),
        Out::Fd(f) => (None, None, Stdio::from(f)),
        Out::Capture(n) => {
            let (r, w) = pipe()?;
            let efd_src = dup_hi(w.as_raw_fd())?;
            (Some((r, efd_src)), n, Stdio::from(w))
        }
    };
    let efd = match (err, &cap) {
        (ErrTo::Out, Some((_, w))) => dup_hi(w.as_raw_fd())?,
        (ErrTo::Null, _) => dup_hi(dev_null(true)?.as_raw_fd())?,
        (ErrTo::Fd(fd), _) => dup_hi(fd)?,
        _ => dup_hi(2)?,
    };
    // (on_box's tools stay on the caller's terminal: not in the box's pid namespace, finding 307)
    fds.enter = Some(entry.plan(false, false, Some(efd.as_raw_fd())));
    let pid_to = fds.pid.take();
    let bg = fds.bg;
    let mut c = command(argv, fds);
    if let Some(env) = env {
        c.env_clear();
        c.envs(
            env.iter()
                .map(|(k, v)| (OsStr::from_bytes(k), OsStr::from_bytes(v))),
        );
    }
    c.stdin(match stdin {
        In::Inherit => Stdio::inherit(),
        In::Null => Stdio::null(),
        In::Fd(f) => Stdio::from(f),
    });
    c.stdout(stdout);
    c.stderr(Stdio::from(hw));
    // (a foreground child, sig::run_child; one bash backgrounds, INT ignored, is not busy)
    let ((held, out), (rc, sig)) = crate::sig::run_child(c, bg, |child| {
        drop(efd);
        crate::sig::spawned(child.id());
        if let Some(p) = &pid_to {
            p.store(i32::try_from(child.id()).unwrap_or(0), Ordering::SeqCst);
        }
        let held = hold(hr);
        let out = match cap {
            Some((r, w)) => {
                drop(w);
                read_all(r, limit)
            }
            None => Vec::new(),
        };
        (held, out)
    })?;
    if let Some(p) = &pid_to {
        p.store(0, Ordering::SeqCst);
    }
    Ok(HeldRun {
        rc,
        sig,
        out,
        held: held.take(),
    })
}

// --- box_exec ---------------------------------------------------------------------------------------

/// The inner bash of box_exec: --pass's values from their fd, the caller's dir, then the PATH the
/// session has (findings 47, 57, 135, 358). Bash's, but for its first line, where the caller's
/// stderr was swapped in: it is the command's before the exec now (enter.rs, finding 490). That line
/// is a comment, so bash's own words for the command keep their line numbers.
const BOX_EXEC_SH: &str = r#"
      # (the caller's stderr is the command's already: omabox's entry made it so)
      if [ -n "${OMABOX_PASS_FD:-}" ]; then
        while IFS= read -r -d "" kv <&"$OMABOX_PASS_FD"; do export "$kv"; done
        eval "exec $OMABOX_PASS_FD<&-"; unset OMABOX_PASS_FD
      fi
      cd "$1" && shift
      if [ -n "${OMABOX_SYS:-}" ]; then unset OMABOX_SYS OMABOX_CALLER_PATH; PATH=/usr/bin exec -- "$@"; fi
      p=/opt/omabox/share/bin:$HOME/.local/bin
      # `up --quickshell`: the build as quickshell and qs, before the caller's /usr/bin (finding 358).
      [ ! -x "${XDG_RUNTIME_DIR:-/nonexistent}/omabox-quickshell/quickshell" ] || p=$p:$XDG_RUNTIME_DIR/omabox-quickshell
      # `up --omarchy`: its bin before the caller's dirs, as in the session (finding 135).
      [ "${OMARCHY_PATH:-/usr/share/omarchy}" = /usr/share/omarchy ] || p=$p:${OMARCHY_PATH%/}/bin
      set -f; IFS=:
      for d in $OMABOX_CALLER_PATH $PATH; do
        case $d in /*) ;; *) continue ;; esac
        case ":$p:" in *":$d:"*) ;; *) [ -d "$d" ] && p=$p:$d ;; esac
      done
      unset IFS OMABOX_CALLER_PATH; set +f; PATH=$p exec -- "$@""#;

/// box_exec's options: its stdin /dev/null, its stdout the caller's stderr (run -d's
/// `</dev/null >&2`), an fd given it as 3 (BOX_KEEP=3, `3>&1`), and whether a command killed by a signal is said
/// in words (NS_KILL_SAY: run's own command, #303).
#[derive(Default)]
pub struct ExecOpts {
    pub stdin_null: bool,
    pub stdout_to_stderr: bool,
    pub fd3: Option<OwnedFd>,
    pub kill_say: bool,
    /// Its stdout an fd of the caller's (`$(box_sys ...)`: a pipe read there) rather than this
    /// process's.
    pub stdout: Option<RawFd>,
    /// box_sys (BOX_SYS): one of omabox's own helpers, by its full path, with only the box's
    /// display, Hyprland and session bus of omabox.env, in the box HOME, PATH=/usr/bin, its pipes not
    /// relayed and no terminal of its own.
    pub sys: bool,
    /// Started as bash starts what it backgrounds (a `<(...)`): SIGINT and SIGQUIT ignored in it.
    pub bg: bool,
    /// Its stdin this fd (`< FILE`, a pipe fed here) rather than this process's.
    pub stdin: Option<RawFd>,
}

/// box_sys's environment: HOME, LANG, the runtime dir, then the first WAYLAND_DISPLAY,
/// HYPRLAND_INSTANCE_SIGNATURE and DBUS_SESSION_BUS_ADDRESS of omabox.env, each only when tame.
fn sys_env(d: &[u8]) -> Vec<Vec<u8>> {
    let lang = shell::env("LANG")
        .filter(|l| !l.is_empty())
        .unwrap_or(b"C.UTF-8".to_vec());
    let mut env = vec![
        cat!("HOME=", box_home(d)),
        cat!("LANG=", lang),
        format!("XDG_RUNTIME_DIR=/run/user/{}", procfs::uid()).into_bytes(),
        b"OMABOX_SYS=1".to_vec(),
    ];
    let mut seen: Vec<Vec<u8>> = Vec::new();
    // (`while IFS= read -r -d '' v`: a last record without its NUL is not read)
    let text = run_read(d, "omabox.env").unwrap_or_default();
    let mut recs: Vec<&[u8]> = text.split(|&c| c == 0).collect();
    recs.pop();
    for v in recs {
        let i = v.iter().position(|&c| c == b'=').unwrap_or(v.len());
        let (k, val) = (&v[..i], v.get(i + 1..).unwrap_or_default());
        let tame = match k {
            b"WAYLAND_DISPLAY" | b"HYPRLAND_INSTANCE_SIGNATURE" => {
                shell::re_match("^[A-Za-z0-9_.-]+$", val)
            }
            b"DBUS_SESSION_BUS_ADDRESS" => shell::re_match("^unix:[A-Za-z0-9_.=/,-]{1,200}$", val),
            _ => continue,
        };
        if tame && !seen.iter().any(|s| s == k) {
            seen.push(k.to_vec());
            env.push(v.to_vec());
        }
    }
    env
}

/// box_exec ARGS: a command inside the box's namespaces (unprivileged: we own its user namespace)
/// with the session's environment, in box_cwd's dir, with box_exec's PATH. Its stdin, stdout and
/// stderr are never a file, a terminal or a device of the host's (finding 325), and it gets no other
/// fd but those it is given (#224).
pub fn box_exec(ctx: &mut Ctx, args: &[Vec<u8>], o: ExecOpts) -> Res<i32> {
    let pid = match box_pid(&ctx.d) {
        BoxPid::Pid(p) => p,
        BoxPid::None | BoxPid::Unknown => {
            return Err(ctx.die(cat!("box '", ctx.name, "' is not up")));
        }
    };
    covers_held(ctx)?;
    let mut wd = shell::pwd();
    let env: Vec<Vec<u8>> = if o.sys {
        if !args.first().is_some_and(|a| a.starts_with(b"/")) {
            let a = args.first().cloned().unwrap_or(b"nothing".to_vec());
            return Err(ctx.die(cat!(
                "box_exec: omabox's own helper goes by its full path, got ",
                a
            )));
        }
        wd.clear();
        sys_env(&ctx.d)
    } else {
        // Not the session's own marker: a command run here is not the box's session (finding 424).
        mapfile0(&run_read(&ctx.d, "omabox.env").unwrap_or_default())
            .into_iter()
            .filter(|v| !v.starts_with(b"OMABOX_OWN_SESSION="))
            .collect()
    };
    if jail::in_jail() && !o.sys {
        // A jailed caller's, when its jail and the host agree on it.
        wd = jail::get_raw("cwd");
    }
    let cwd = if wd.is_empty() {
        box_home(&ctx.d)
    } else {
        box_cwd(&ctx.d, &pid, &wd)
    };
    // run --pass (finding 66): the caller's values through a pipe, never on a command line;
    // --env-file's entries first, so a --pass of the same name wins.
    let mut keep: Vec<RawFd> = Vec::new();
    let mut pass_word = Vec::new();
    let mut pfd: Option<OwnedFd> = None;
    if !ctx.pass.is_empty() || !ctx.envkv.is_empty() {
        let mut data = Vec::new();
        for kv in &ctx.envkv {
            data.extend_from_slice(kv);
            data.push(0);
        }
        for v in &ctx.pass {
            data.extend_from_slice(&cat!(v, "=", caller_env(v).unwrap_or_default()));
            data.push(0);
        }
        let (r, w) = pipe().map_err(|_| Exit(1))?;
        let r = dup_hi(r.as_raw_fd()).map_err(|_| Exit(1))?;
        feed(w, data);
        keep.push(r.as_raw_fd());
        pass_word = format!("OMABOX_PASS_FD={}", r.as_raw_fd()).into_bytes();
        pfd = Some(r);
    }
    let mut nsline: Vec<Vec<u8>> = vec![b"/usr/bin/env".to_vec(), b"-i".to_vec()];
    nsline.extend(env);
    nsline.extend([
        b"OMABOX=1".to_vec(),
        cat!("OMABOX_NAME=", ctx.name),
        cat!(
            "OMABOX_CALLER_PATH=",
            if o.sys { Vec::new() } else { caller_path(true) }
        ),
    ]);
    if !pass_word.is_empty() {
        nsline.push(pass_word);
    }
    nsline.extend([
        b"/usr/bin/bash".to_vec(),
        b"-c".to_vec(),
        BOX_EXEC_SH.as_bytes().to_vec(),
        b"bash".to_vec(),
        cwd,
    ]);
    nsline.extend(args.iter().cloned());
    // The caller's stdio as this command gets them.
    let null_in = if o.stdin_null {
        dev_null(false).ok()
    } else {
        None
    };
    let src = [
        o.stdin
            .unwrap_or_else(|| null_in.as_ref().map_or(0, AsRawFd::as_raw_fd)),
        match (o.stdout, o.stdout_to_stderr) {
            (Some(fd), _) => fd,
            (None, true) => 2,
            (None, false) => 1,
        },
        2,
    ];
    if o.fd3.is_some() {
        keep.push(3);
    }
    let fd3 = o.fd3.as_ref().map(AsRawFd::as_raw_fd);
    // (all of the box's namespaces, its network when it has its own: enter::all)
    let (rc, held, sig) = match crate::enter::Entry::open(&ctx.d, &pid, crate::enter::all(&pid)) {
        Err(words) => (1, words, None),
        Ok(entry) => {
            let pty = !o.sys
                && isatty(src[0])
                && isatty(src[1])
                && fg_tty()
                && executable(b"/usr/bin/script");
            let r = if pty {
                box_pty(&entry, &nsline, keep, fd3).map(|rc| HeldRun {
                    rc,
                    sig: None,
                    out: Vec::new(),
                    held: Vec::new(),
                })
            } else {
                let relays = Relays::new(src, !o.sys).map_err(|_| Exit(1))?;
                let fds = Fds {
                    keep,
                    fd3,
                    only: true,
                    nocore: true,
                    pid: None,
                    bg: o.bg,
                    enter: None,
                };
                let shield = args.first().map(Vec::as_slice);
                relays.run(&entry, shield, &nsline, fds)
            };
            match r {
                Ok(r) => (r.rc, r.held, r.sig),
                Err(e) => {
                    // (bash's nsenter or env said why; std's spawn says it here)
                    let (why, code) = exec_failed(b"/usr/bin/env", &e);
                    crate::exit::say(cat!("cannot enter box '", ctx.name, "': ", why));
                    return Err(Exit(code));
                }
            }
        }
    };
    drop(pfd);
    drop(o.fd3);
    let mut sink = Vec::new();
    match ns_said(
        ctx,
        &held,
        sig,
        rc,
        None,
        o.kill_say,
        ErrTo::Inherit,
        &mut sink,
    ) {
        Ok(_) => Ok(rc),
        Err(code) => Err(Exit(code)),
    }
}

/// `mapfile -d ''`: each NUL-ended record, and a last one without its NUL.
fn mapfile0(v: &[u8]) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = v.split(|&c| c == 0).map(<[u8]>::to_vec).collect();
    if out.last().is_some_and(Vec::is_empty) {
        out.pop();
    }
    out
}

/// A variable of the caller's as it was when omabox started (run --pass): what came, as
/// /proc/self/environ holds it (bash's caller_env read /proc/$$/environ, before its globals overwrote
/// the names they share with it: finding 74; finding 514). A jailed caller's comes as
/// OMABOX_RELAY_PASS_NAME (relay_call). None when it is not there.
pub fn caller_env(name: &[u8]) -> Option<Vec<u8>> {
    let k = if jail::in_jail() {
        cat!("OMABOX_RELAY_PASS_", name)
    } else {
        name.to_vec()
    };
    let lead = cat!(k, "=");
    let find = |env: &[u8]| {
        env.split(|&c| c == 0)
            .find_map(|e| e.strip_prefix(lead.as_slice()))
            .map(<[u8]>::to_vec)
    };
    match std::fs::read("/proc/self/environ") {
        Ok(env) => find(&env),
        Err(_) => std::env::var_os(OsStr::from_bytes(&k)).map(|v| v.as_bytes().to_vec()),
    }
}

/// caller_path [box]: the caller's PATH without the guard's xdg-open (finding 92); for a box (BOX),
/// nor an Omarchy checkout's bin (finding 149). A jailed caller's PATH is OMABOX_RELAY_PATH.
pub fn caller_path(for_box: bool) -> Vec<u8> {
    let p = if jail::in_jail() {
        shell::env_or_empty("OMABOX_RELAY_PATH")
    } else {
        shell::env_or_empty("PATH")
    };
    // (`IFS=: read -ra` of `<<<"$p"`: the first line, split at each :, a last empty field dropped.)
    let line = p.split(|&c| c == b'\n').next().unwrap_or_default();
    let mut dirs: Vec<&[u8]> = if line.is_empty() {
        Vec::new()
    } else {
        line.split(|&c| c == b':').collect()
    };
    if dirs.last().is_some_and(|d| d.is_empty()) {
        dirs.pop();
    }
    let mut out: Vec<&[u8]> = Vec::new();
    for d in dirs {
        if d.ends_with(b"/share/guard") || d.ends_with(b"/share/guard/") {
            continue;
        }
        let t = d.strip_suffix(b"/").unwrap_or(d);
        if for_box
            && t != b"/usr/share/omarchy/bin"
            && shell::path(&cat!(t, "/../default/hypr/envs.lua")).is_file()
        {
            continue;
        }
        out.push(d);
    }
    out.join(&b':')
}

/// box_cwd PID WD: WD when it is the same directory in the box (a mount of it: same device and
/// inode, or an overlay of it), else the box HOME (finding 63: /home/USER and /tmp exist in a box
/// too, empty or the box's own).
pub fn box_cwd(d: &[u8], pid: &[u8], wd: &[u8]) -> Vec<u8> {
    if same_in_box(d, pid, wd) {
        return wd.to_vec();
    }
    box_home(d)
}

/// same_in_box D PID P: the box has host path P at the same path as the host's own (a mount of it:
/// the same device and inode, or an overlay of it), looked up in the box's root as stat_in_box
/// does. False when either has none; never true for what only shares the path, as a desk box's HOME
/// does with yours (#321).
pub fn same_in_box(d: &[u8], pid: &[u8], p: &[u8]) -> bool {
    let here = std::fs::metadata(shell::path(p))
        .map(|m| format!("{}:{}", m.dev(), m.ino()).into_bytes())
        .unwrap_or_default();
    let there = stat_in_box(d, pid, p)
        .map(|st| format!("{}:{}", st.st_dev, st.st_ino).into_bytes())
        .unwrap_or_default();
    (!here.is_empty() && here == there) || (!there.is_empty() && under_overlay(d, p))
}

/// has_in_box D PID P: the box has something at P (its links followed inside its root, as it
/// resolves them: never through the host's).
pub fn has_in_box(d: &[u8], pid: &[u8], p: &[u8]) -> bool {
    stat_in_box(d, pid, p).is_some()
}

/// `stat -L WD` as the box sees it, with no process started (bash ran nsenter and stat): looked up
/// from the box's root (/proc/PID/root), where its mounts are, links resolved inside that root
/// (openat2's RESOLVE_IN_ROOT, as they resolve in the box). PID is the box's PID 1 as box_pid gave
/// it, asked again once the root is open, so a pid reused meanwhile is never read.
fn stat_in_box(d: &[u8], pid: &[u8], wd: &[u8]) -> Option<libc::stat> {
    let root = std::fs::File::open(shell::path(&cat!("/proc/", pid, "/root"))).ok()?;
    if !matches!(box_pid(d), BoxPid::Pid(p) if p == pid) {
        return None;
    }
    let path = std::ffi::CString::new(wd.to_vec()).ok()?;
    // SAFETY: open_how is plain data; zeroed, then the fields set.
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.flags = (libc::O_PATH | libc::O_CLOEXEC) as u64;
    how.resolve = libc::RESOLVE_IN_ROOT;
    // SAFETY: openat2 reads the path and how (its size given) and returns a new fd of ours, or -1.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            root.as_raw_fd(),
            path.as_ptr(),
            &how,
            std::mem::size_of::<libc::open_how>(),
        )
    };
    let fd = RawFd::try_from(fd).ok().filter(|&f| f >= 0)?;
    // SAFETY: just opened, ours alone.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    fstat(fd.as_raw_fd())
}

/// `jq -e --arg p WD/ 'any(.overlays[]?; . as $o | $p | startswith($o + "/"))' box.json`.
fn under_overlay(d: &[u8], wd: &[u8]) -> bool {
    let Ok(text) = std::fs::read(boxes::at(d, "box.json")) else {
        return false;
    };
    let (vals, _) = json::values(&text);
    let p = cat!(wd, "/");
    let mut last = false;
    for v in &vals {
        let Some(serde_json::Value::Array(a)) = v.get("overlays") else {
            last = false;
            continue;
        };
        last = false;
        for o in a {
            let serde_json::Value::String(o) = o else {
                return false;
            };
            if p.starts_with(&cat!(o, "/")) {
                last = true;
                break;
            }
        }
    }
    last
}

/// Whether this process is in its terminal's foreground process group: one in the background must
/// neither read the terminal (SIGTTIN) nor put it in raw mode (SIGTTOU).
fn fg_tty() -> bool {
    let Some(s) = procfs::read(b"self", "stat") else {
        return false;
    };
    let f = shell::words(procfs::after_comm(&s));
    f.len() > 5 && f[2] == f[5]
}

/// box_pty: the command on a pseudo-terminal of its own, for a caller at a terminal (finding 325):
/// util-linux `script` holds the caller's terminal and the pty's master; the box gets the slave, its
/// controlling terminal (finding 491: the box reaches only that one; what it writes there the
/// caller's terminal shows, as any program's). Script runs this binary's `_enter` there, with the
/// entry's pidfd (enter::main). A stderr that is not the terminal stays where it was (relayed if a
/// box could reach it), made the command's by the entry.
fn box_pty(
    entry: &crate::enter::Entry,
    inner: &[Vec<u8>],
    mut keep: Vec<RawFd>,
    fd3: Option<RawFd>,
) -> std::io::Result<i32> {
    let mut efd: Option<OwnedFd> = None;
    let mut relay: Option<Child> = None;
    if !isatty(2) {
        if relay_out(2, true) {
            let (r, w) = pipe()?;
            relay = Some(relay_copy_cmd(r, dup_hi(2)?).spawn()?);
            efd = Some(dup_hi(w.as_raw_fd())?);
        } else {
            efd = Some(dup_hi(2)?);
        }
    }
    if let Some(e) = &efd {
        keep.push(e.as_raw_fd());
    }
    keep.push(entry.pidfd());
    // (this process's own binary while it runs, whatever has become of the file since)
    let mut words: Vec<Vec<u8>> = vec![
        format!("/proc/{}/exe", std::process::id()).into_bytes(),
        b"_enter".to_vec(),
        entry.pidfd().to_string().into_bytes(),
        entry.ns().to_string().into_bytes(),
        efd.as_ref()
            .map_or("-".to_string(), |e| e.as_raw_fd().to_string())
            .into_bytes(),
        b"--".to_vec(),
    ];
    words.extend(inner.iter().cloned());
    let words: Vec<Vec<u8>> = words.iter().map(|w| shell::quote_at(w)).collect();
    let script = cat!("exec ", words.join(&b' '));
    let argv = vec![
        b"env".to_vec(),
        b"SHELL=/usr/bin/bash".to_vec(),
        b"/usr/bin/script".to_vec(),
        b"-q".to_vec(),
        b"-e".to_vec(),
        b"-c".to_vec(),
        script,
        b"/dev/null".to_vec(),
    ];
    let mut c = command(
        &argv,
        Fds {
            keep,
            fd3,
            only: true,
            nocore: true,
            pid: None,
            bg: false,
            enter: None,
        },
    );
    let mut child = c.spawn()?;
    drop(c);
    let (rc, _) = wait(&mut child);
    drop(efd);
    if let Some(r) = relay {
        drain(vec![r]);
    }
    Ok(rc)
}

/// How a stdio_relay call ended: its status and what `$(...)` read.
pub struct Relayed {
    pub rc: i32,
    pub out: Vec<u8>,
}

/// `stdio_relay fds_only "" ARGV...` (gdb's calls, finding 229): ARGV with no fd above 2 open, each
/// of its stdin, stdout and stderr a box could reach relayed through a pipe held here. CAPTURE: its
/// stdout read here (`$(...)`: a pipe, not relayed); ERR_NULL: `2>/dev/null`. A foreground command,
/// started in a box when ENTER says so (where bash's ARGV began with nsenter).
pub fn stdio_relay(
    argv: &[Vec<u8>],
    capture: bool,
    err_null: bool,
    enter: Option<crate::enter::Plan>,
) -> std::io::Result<Relayed> {
    stdio_relay_with([0, 1, 2], false, argv, capture, err_null, enter)
}

/// stdio_relay with SRC the caller's stdin, stdout and stderr, and PIPES as run's relays have it (a
/// pipe or socket relayed too, finding 337).
fn stdio_relay_with(
    src: [RawFd; 3],
    pipes: bool,
    argv: &[Vec<u8>],
    capture: bool,
    err_null: bool,
    enter: Option<crate::enter::Plan>,
) -> std::io::Result<Relayed> {
    let null_err = if err_null {
        Some(dev_null(true)?)
    } else {
        None
    };
    let (cap_r, cap_w) = if capture {
        let (r, w) = pipe()?;
        (Some(r), Some(w))
    } else {
        (None, None)
    };
    let src = [
        src[0],
        cap_w.as_ref().map_or(src[1], AsRawFd::as_raw_fd),
        null_err.as_ref().map_or(src[2], AsRawFd::as_raw_fd),
    ];
    let relays = Relays::new(src, pipes)?;
    drop(cap_w);
    drop(null_err);
    let reader = cap_r.map(|r| std::thread::spawn(move || read_all(r, None)));
    let [i, o, e] = relays.fds;
    let mut c = command(
        argv,
        Fds {
            only: true,
            // (gdb's steps, its --watch's gdb, restart-shell's kill: a crash there makes no
            // notification either, finding 492)
            nocore: true,
            enter,
            ..Fds::default()
        },
    );
    c.stdin(Stdio::from(i))
        .stdout(Stdio::from(o))
        .stderr(Stdio::from(e));
    // (a foreground child, sig::run_child)
    let spawned = crate::sig::run_status(c);
    let rc = match spawned {
        Ok(rc) => rc,
        Err(e) => {
            // (omabox's words: bash's were its own, "gdb: command not found", finding 441)
            let _ = writeln!(
                std::io::stderr(),
                "omabox: cannot run {}: {}",
                String::from_utf8_lossy(argv.first().map_or(&b""[..], |a| a)),
                if e.kind() == std::io::ErrorKind::NotFound {
                    "not found in PATH".to_string()
                } else {
                    e.to_string()
                }
            );
            127
        }
    };
    if let Some(mut c) = relays.input {
        let _ = c.kill();
        let _ = c.wait();
    }
    drain(relays.outs);
    let out = reader.and_then(|t| t.join().ok()).unwrap_or_default();
    Ok(Relayed { rc, out })
}

/// Why PROG could not be exec'd, in omabox's words (finding 441: not bash's `line N: exec:` line), and
/// the status bash's exec gave: 127 for one not there, 126 for one that cannot run.
pub fn exec_failed(prog: &[u8], e: &std::io::Error) -> (String, u8) {
    match e.kind() {
        std::io::ErrorKind::NotFound if prog.contains(&b'/') => ("no such file".into(), 127),
        std::io::ErrorKind::NotFound => ("not found in PATH".into(), 127),
        std::io::ErrorKind::PermissionDenied => ("permission denied".into(), 126),
        _ => (crate::shots::why(e), 126),
    }
}

/// `command -v NAME >/dev/null` for a program: an executable file of that name in a dir of PATH.
pub fn executable_on_path(name: &[u8]) -> bool {
    executable_in(&shell::env_or_empty("PATH"), name)
}

/// NAME is an executable file in one of PATH's folders (`a:b`, an empty one the current folder): what
/// a shell run with PATH finds (a box's start runs with up_box's LAUNCH_PATH, not the caller's, #364).
pub fn executable_in(path: &[u8], name: &[u8]) -> bool {
    path.split(|&c| c == b':')
        .map(|d| cat!(if d.is_empty() { b".".as_slice() } else { d }, "/", name))
        .any(|p| executable(&p) && shell::path(&p).is_file())
}

/// Where omabox looks for nft (S10, #439), in this order: the system's own folders, never the
/// caller's PATH. nft sets the protected ports' rule on the host, as the user: a PATH lookup would let
/// anything earlier on PATH stand in for it. Debian and openSUSE put it in /usr/sbin.
pub const NFT_DIRS: [&str; 2] = ["/usr/bin", "/usr/sbin"];

/// "/usr/bin or /usr/sbin": what a message says omabox looked in.
pub fn nft_dirs_said() -> String {
    NFT_DIRS.join(" or ")
}

/// nft_at ROOT: the nft omabox runs, under ROOT ("" for this machine; doctor's tests give a tree of
/// their own): the first executable file nft in NFT_DIRS. Err(None) when there is none; Err(why) when
/// the first is one another user could change: writable by its group or by anyone, or owned by
/// someone other than root and the caller (an owner a user namespace leaves unmapped, as nobody,
/// counts as root: the host's own nft, seen from one).
pub fn nft_at(root: &[u8]) -> Result<Vec<u8>, Option<String>> {
    use std::os::unix::fs::MetadataExt;
    for d in NFT_DIRS {
        let p = crate::gpu::at(root, format!("{d}/nft").as_bytes());
        let Ok(m) = std::fs::metadata(shell::path(&p)) else {
            continue;
        };
        if !m.is_file() || !executable(&p) {
            continue;
        }
        let shown = String::from_utf8_lossy(&p).into_owned();
        if m.mode() & 0o022 != 0 {
            return Err(Some(format!(
                "{shown} is writable by others than its owner: omabox will not run it"
            )));
        }
        if m.uid() != 0 && m.uid() != procfs::euid() && !unmapped(m.uid()) {
            return Err(Some(format!(
                "{shown} is owned by uid {}, neither root nor you: omabox will not run it",
                m.uid()
            )));
        }
        return Ok(p);
    }
    Err(None)
}

/// nft_at for this machine.
pub fn nft() -> Result<Vec<u8>, Option<String>> {
    nft_at(b"")
}

/// What a message that found no nft to run (nft's Err) tells the user to do.
pub fn nft_fix(why: &Option<String>) -> String {
    why.clone()
        .unwrap_or_else(|| "sudo pacman -S nftables".into())
}

/// UID is what an owner this user namespace does not map shows as (the overflow uid, 65534 by
/// default), and this is not the host's own user namespace, which maps every uid: there that uid is a
/// real owner (nobody's).
fn unmapped(uid: u32) -> bool {
    let overflow = std::fs::read_to_string("/proc/sys/kernel/overflowuid")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(65534);
    let map = std::fs::read_to_string("/proc/self/uid_map").unwrap_or_default();
    let init = map.split_whitespace().collect::<Vec<_>>() == ["0", "0", "4294967295"];
    uid == overflow && !init
}

/// tools_or_skip TEST TOOL... (tests only): every TOOL is on PATH, or TEST says it skips and which
/// are missing. A test that runs a host tool omabox runs (tar, grep, awk) checks it first: a package's
/// clean chroot has only the recipe's depends on PATH (#324).
#[cfg(test)]
pub fn tools_or_skip(test: &str, tools: &[&str]) -> bool {
    let miss: Vec<&str> = tools
        .iter()
        .copied()
        .filter(|t| !executable_on_path(t.as_bytes()))
        .collect();
    if !miss.is_empty() {
        eprintln!("skip {test}: no {} on PATH", miss.join(", "));
    }
    miss.is_empty()
}

// --- the relays ---------------------------------------------------------------------------------------

/// relay_out N: fd N goes through a relay: a box could reach it (fd_exposed), or it is a pipe or a
/// socket a process the command leaves running would hold (finding 337; with omabox-relay only).
fn relay_out(fd: RawFd, pipes: bool) -> bool {
    if fd_exposed(fd) {
        return true;
    }
    pipes && executable(&relay_bin()) && fstat(fd).is_some_and(|st| is_fifo_or_socket(&st))
}

/// The checkout's omabox-relay (a #[test]'s binary has no checkout: the source tree's).
fn relay_bin() -> Vec<u8> {
    #[cfg(test)]
    return concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tools/relay/omabox-relay"
    )
    .as_bytes()
    .to_vec();
    #[cfg(not(test))]
    cat!(shell::bytes(&crate::root()), "/tools/relay/omabox-relay")
}

/// stdio_relay: each of the command's stdin, stdout and stderr a box could reach goes through a pipe
/// held here. A terminal on stdin is not read: /dev/null. One stdout and stderr that are the same file
/// share one relay, so their lines land in order.
struct Relays {
    fds: [OwnedFd; 3],
    input: Option<Child>,
    outs: Vec<Child>,
}

impl Relays {
    fn new(src: [RawFd; 3], pipes: bool) -> std::io::Result<Relays> {
        let mut input = None;
        let stdin = if isatty(src[0]) {
            dev_null(false)?
        } else if fd_exposed(src[0]) {
            let (r, w) = pipe()?;
            // (`<(cat)`: a process substitution, SIGINT and SIGQUIT ignored in it)
            let mut c = crate::sig::command("cat");
            c.stdin(Stdio::from(dup_hi(src[0])?)).stdout(Stdio::from(w));
            background(&mut c);
            input = Some(c.spawn()?);
            r
        } else {
            dup_hi(src[0])?
        };
        let mut outs = Vec::new();
        let mut out_w: Option<OwnedFd> = None;
        let stdout = if relay_out(src[1], pipes) {
            let (r, w) = pipe()?;
            let mut c = relay_copy_cmd(r, dup_hi(src[1])?);
            c.stderr(Stdio::from(dup_hi(src[2])?));
            outs.push(c.spawn()?);
            out_w = Some(dup_hi(w.as_raw_fd())?);
            w
        } else {
            dup_hi(src[1])?
        };
        let stderr = if relay_out(src[2], pipes) {
            match &out_w {
                Some(w) if same_file(src[1], src[2]) => dup_hi(w.as_raw_fd())?,
                _ => {
                    let (r, w) = pipe()?;
                    let mut c = relay_copy_cmd(r, dup_hi(src[2])?);
                    c.stderr(Stdio::from(dup_hi(src[2])?));
                    outs.push(c.spawn()?);
                    w
                }
            }
        } else {
            dup_hi(src[2])?
        };
        drop(out_w);
        Ok(Relays {
            fds: [stdin, stdout, stderr],
            input,
            outs,
        })
    }

    /// The command through ns_held in ENTRY's box with these as its stdio; then this side's ends
    /// closed, the input relay ended, the output drained (relay_drain).
    fn run(
        self,
        entry: &crate::enter::Entry,
        cmd0: Option<&[u8]>,
        argv: &[Vec<u8>],
        fds: Fds,
    ) -> std::io::Result<HeldRun> {
        let [i, o, e] = self.fds;
        // (ns_held's EFD is the command's stderr: `{e}>&2` with stdio_relay's 2>&efd)
        let r = held_run_err(entry, cmd0, argv, In::Fd(i), Out::Fd(o), e, fds);
        if let Some(mut c) = self.input {
            let _ = c.kill();
            let _ = c.wait();
        }
        drain(self.outs);
        r
    }
}

/// relay_copy: R copied to OUT by omabox-relay copy (which relay_drain can end), or `cat` where this
/// omabox has none built.
fn relay_copy_cmd(r: OwnedFd, out: OwnedFd) -> Command {
    let bin = relay_bin();
    let mut c = if executable(&bin) {
        let mut c = crate::sig::command(OsStr::from_bytes(&bin));
        c.arg("copy");
        c
    } else {
        crate::sig::command("cat")
    };
    c.stdin(Stdio::from(r)).stdout(Stdio::from(out));
    // (`>(relay_copy)`: a process substitution, SIGINT and SIGQUIT ignored in it)
    background(&mut c);
    c
}

/// held_run with the command's stderr (EFD) a given fd, in a session of its own (box_exec's command
/// never needs the caller's terminal here: a caller at one gets box_pty, finding 491).
fn held_run_err(
    entry: &crate::enter::Entry,
    cmd0: Option<&[u8]>,
    argv: &[Vec<u8>],
    stdin: In,
    out: Out,
    err: OwnedFd,
    mut fds: Fds,
) -> std::io::Result<HeldRun> {
    let (hr, hw) = pipe()?;
    let efd = dup_hi(err.as_raw_fd())?;
    drop(err);
    fds.enter = Some(
        entry
            .plan(true, false, Some(efd.as_raw_fd()))
            .shield_timeout(cmd0),
    );
    let bg = fds.bg;
    let mut c = command(argv, fds);
    c.stdin(match stdin {
        In::Inherit => Stdio::inherit(),
        In::Null => Stdio::null(),
        In::Fd(f) => Stdio::from(f),
    });
    c.stdout(match out {
        Out::Fd(f) => Stdio::from(f),
        Out::Null => Stdio::null(),
        _ => Stdio::inherit(),
    });
    c.stderr(Stdio::from(hw));
    // (a foreground child, sig::run_child)
    let (held, (rc, sig)) = crate::sig::run_child(c, bg, |_| {
        drop(efd);
        hold(hr)
    })?;
    Ok(HeldRun {
        rc,
        sig,
        out: Vec::new(),
        held: held.take(),
    })
}

/// relay_drain: each output relay, once the command is over, given a second to end. One still running
/// after 100 ms has a process the command left running holding its pipe (finding 337, #239): told the
/// command has ended (SIGURG, every 50 ms), it copies what the pipe holds and ends. One still copying
/// after the second, or a `cat`, is left running.
fn drain(outs: Vec<Child>) {
    for mut c in outs {
        for i in 1..=100 {
            if matches!(c.try_wait(), Ok(Some(_)) | Err(_)) {
                break;
            }
            if i >= 10 && i % 5 == 0 {
                // SAFETY: a child of ours, not yet reaped: its pid is still its own.
                unsafe {
                    libc::kill(c.id() as libc::pid_t, libc::SIGURG);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // #364: a program is looked for in the PATH given, not the caller's: an nft only on the caller's
    // PATH is not one a box's start (PATH=/usr/bin) finds; a file that cannot run, or a folder, is none.
    #[test]
    fn executable_in_the_path_given() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("omabox-exin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for sub in ["a", "b", "c/prog"] {
            std::fs::create_dir_all(d.join(sub)).unwrap();
        }
        std::fs::write(d.join("a/prog"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(d.join("a/prog"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(d.join("b/prog"), "").unwrap();
        let dir = |s: &str| d.join(s).into_os_string().into_encoded_bytes();
        assert!(executable_in(&dir("a"), b"prog"));
        assert!(executable_in(&cat!(dir("b"), ":", dir("a")), b"prog"));
        assert!(!executable_in(&dir("b"), b"prog"), "not executable");
        assert!(!executable_in(&dir("c"), b"prog"), "a folder");
        assert!(!executable_in(&dir("b"), b"other"));
        assert!(!executable_in(b"", b"no-such-prog-here"));
        let _ = std::fs::remove_dir_all(&d);
    }

    // #439: nft from /usr/bin, else /usr/sbin (Debian, openSUSE), under a root of the test's own; never
    // from PATH, and never one another user could change.
    #[test]
    fn nft_from_the_system_folders_only() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("omabox-nft-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for sub in ["usr/bin", "usr/sbin", "path"] {
            std::fs::create_dir_all(d.join(sub)).unwrap();
        }
        let put = |rel: &str, mode: u32| {
            std::fs::write(d.join(rel), "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(d.join(rel), std::fs::Permissions::from_mode(mode)).unwrap();
        };
        let r = d.clone().into_os_string().into_encoded_bytes();
        let at = |rel: &str| d.join(rel).into_os_string().into_encoded_bytes();
        assert_eq!(nft_at(&r), Err(None), "none");
        put("path/nft", 0o755);
        assert_eq!(nft_at(&r), Err(None), "one on no system folder is none");
        put("usr/sbin/nft", 0o755);
        assert_eq!(nft_at(&r), Ok(at("usr/sbin/nft")), "Debian's, openSUSE's");
        assert_eq!(nft_at(&cat!(r.clone(), "/")), Ok(at("usr/sbin/nft")));
        put("usr/bin/nft", 0o644);
        assert_eq!(
            nft_at(&r),
            Ok(at("usr/sbin/nft")),
            "/usr/bin's not executable"
        );
        put("usr/bin/nft", 0o755);
        assert_eq!(nft_at(&r), Ok(at("usr/bin/nft")), "/usr/bin's first");
        for mode in [0o775, 0o757] {
            put("usr/bin/nft", mode);
            let e = nft_at(&r).unwrap_err().expect("a reason");
            assert!(
                e.contains("writable by others than its owner"),
                "{mode:o}: {e}"
            );
        }
        std::fs::remove_file(d.join("usr/bin/nft")).unwrap();
        std::fs::create_dir(d.join("usr/bin/nft")).unwrap();
        assert_eq!(nft_at(&r), Ok(at("usr/sbin/nft")), "a folder is no nft");
        let _ = std::fs::remove_dir_all(&d);
    }

    // As t_unit_box_gone checks bash's ns_said, but for what bash added (finding 440), and with the
    // entry's own words (finding 490).
    #[test]
    fn said_text_counts_what_was_held() {
        let held = b"omabox: note: forged\n\x1b[31momabox: cannot enter the box: red\x1b[0m\nomabox: cannot enter the box: real \x1b]0;t\x07\nhalf";
        let mut h = held.to_vec();
        h.resize(NS_HELD_MAX, b'A');
        assert_eq!(
            String::from_utf8_lossy(&said_text(&h, 1, false)),
            "omabox: cannot enter the box: real ]0;t\nomabox: dropped 4056 bytes of other output on the box entry's stderr\n"
        );
        assert_eq!(
            String::from_utf8_lossy(&said_text(b"omabox: cannot enter the box: x\n", 137, true)),
            "omabox: cannot enter the box: x\nomabox: the command was killed by SIGKILL (137)\n"
        );
        assert_eq!(
            String::from_utf8_lossy(&said_text(b"", 143, true)),
            "omabox: the command was killed by SIGTERM (143)\n"
        );
        assert_eq!(said_text(b"", 137, false), b"");
        // (nsenter's, prlimit's and env's are no longer its own: nothing of theirs comes there)
        assert_eq!(
            said_text(b"nsenter: x\n", 1, false),
            b"omabox: dropped 11 bytes of other output on the box entry's stderr\n"
        );
        assert_eq!(
            said_text(b"omabox: cannot enter the box: ", 1, false),
            b"omabox: dropped 30 bytes of other output on the box entry's stderr\n"
        );
    }

    // t_unit_box_gone (#166): an entry that failed on a box gone as it entered is said in words,
    // without the entry's line; on a box that is up, the line as it came (after 2 s of asking); a
    // command the box's end killed (box_exec's, no words) is left to run's words.
    #[test]
    fn box_gone_as_bash() {
        let said = |alive: bool, held: &[u8], sig, rc, words: Option<&[u8]>| {
            let mut buf = Vec::new();
            let r = ns_said_with(
                &|| Ok(alive),
                held,
                sig,
                rc,
                words,
                false,
                ErrTo::Out,
                &mut buf,
            );
            (r, String::from_utf8_lossy(&buf).into_owned())
        };
        let w = gone_words(b"bg");
        assert_eq!(
            said(false, b"omabox: cannot enter the box: No such process\n", None, 1, Some(&w)),
            (
                Ok(true),
                "omabox: box 'bg' went down (why: omabox log -b bg all; clear it with: omabox down bg)\n".to_string()
            ),
            "on_box on a box gone as it entered: said in words, without the entry's line"
        );
        let t0 = std::time::Instant::now();
        assert_eq!(
            said(
                true,
                b"omabox: cannot enter the box: Operation not permitted\n",
                None,
                1,
                Some(&w)
            ),
            (
                Ok(false),
                "omabox: cannot enter the box: Operation not permitted\n".to_string()
            ),
            "...on a box that is up, the entry's line as it came"
        );
        assert!(
            t0.elapsed() >= Duration::from_millis(1800),
            "...after asking for 2 s"
        );
        assert_eq!(
            said(false, b"", Some(libc::SIGKILL), 137, None),
            (Ok(false), String::new()),
            "box_exec on a box gone: nothing said, left to run's words"
        );
        let mut buf = Vec::new();
        assert_eq!(
            ns_said_with(
                &|| Err((b"omabox: cannot tell\n".to_vec(), 3)),
                b"omabox: cannot enter the box: x\n",
                None,
                1,
                Some(&w),
                false,
                ErrTo::Out,
                &mut buf
            ),
            Err(3),
            "...a box this namespace cannot read: its refusal, its code"
        );
        assert_eq!(buf, b"omabox: cannot tell\n");
    }

    // t_unit_box_gone: what the entry's stderr holds is read 4 KiB at most, and one a process keeps
    // open does not hold the call (NS_HELD_WAIT after the command).
    #[test]
    fn held_reads_4k_and_returns_while_held() {
        let (r, w) = pipe().unwrap();
        let keep = w.try_clone().unwrap();
        std::thread::spawn(move || {
            let mut f = std::fs::File::from(w);
            let _ = std::io::Write::write_all(
                &mut f,
                b"omabox: note: forged\n\x1b[31momabox: cannot enter the box: red\x1b[0m\nomabox: cannot enter the box: real \x1b]0;t\x07\nhalf",
            );
            let _ = std::io::Write::write_all(&mut f, &vec![b'A'; 3_000_000]);
        });
        let t0 = std::time::Instant::now();
        let held = hold(r).take();
        let ms = t0.elapsed().as_millis();
        drop(keep);
        assert_eq!(held.len(), NS_HELD_MAX, "4 KiB read at most");
        assert_eq!(
            String::from_utf8_lossy(&said_text(&held, 1, false)),
            "omabox: cannot enter the box: real ]0;t\nomabox: dropped 4056 bytes of other output on the box entry's stderr\n",
            "...anything else on it dropped and counted, escapes stripped"
        );
        assert!(
            ms < 2500,
            "...one a process keeps open: the call returns ({ms} ms)"
        );
    }

    // t_unit_guard_exec_host's: the caller's PATH without the guard's dir (a trailing slash too,
    // finding 92), and for a box without an Omarchy checkout's bin (a dev link's, finding 149).
    #[test]
    fn caller_path_as_bash() {
        let dl = std::env::temp_dir().join(format!("omabox-devlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dl);
        std::fs::create_dir_all(dl.join("bin")).unwrap();
        std::fs::create_dir_all(dl.join("default/hypr")).unwrap();
        std::fs::write(dl.join("default/hypr/envs.lua"), "").unwrap();
        let dl = dl.display().to_string();
        let cp = |path: &str, for_box: bool| {
            let v = shell::with_env(&[("PATH", Some(path)), ("OMABOX_JAIL", None)], || {
                caller_path(for_box)
            });
            String::from_utf8_lossy(&v).into_owned()
        };
        assert_eq!(
            cp("/x/share/guard/:/usr/bin:/y/share/guard:/bin", false),
            "/usr/bin:/bin",
            "...nor one written with a trailing slash (up and run leave it out too)"
        );
        assert_eq!(
            cp(&format!("{dl}/bin:/usr/share/omarchy/bin:/usr/bin"), true),
            "/usr/share/omarchy/bin:/usr/bin",
            "a box's PATH leaves out an Omarchy checkout's bin (a dev link's)"
        );
        assert_eq!(
            cp(&format!("{dl}/bin/:/usr/bin"), false),
            format!("{dl}/bin/:/usr/bin"),
            "...host keeps it"
        );
        let _ = std::fs::remove_dir_all(&dl);
    }

    // t_unit_fd_exposed (finding 325): which of a caller's fds a box could reach again through
    // /proc, and so are relayed.
    #[test]
    fn fd_exposed_as_bash() {
        let d = std::env::temp_dir().join(format!("omabox-fdx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("f"), "x\n").unwrap();
        let f = std::fs::File::open(d.join("f")).unwrap();
        assert!(fd_exposed(f.as_raw_fd()), "fd_exposed: a file");
        std::fs::write(d.join("f.d"), "x\n").unwrap();
        let del = std::fs::File::open(d.join("f.d")).unwrap();
        std::fs::remove_file(d.join("f.d")).unwrap();
        assert!(fd_exposed(del.as_raw_fd()), "...a deleted file still");
        let dir = std::fs::File::open(&d).unwrap();
        assert!(fd_exposed(dir.as_raw_fd()), "...a directory");
        let zero = std::fs::File::open("/dev/zero").unwrap();
        assert!(fd_exposed(zero.as_raw_fd()), "...a device (/dev/zero)");
        let null = std::fs::File::open("/dev/null").unwrap();
        assert!(!fd_exposed(null.as_raw_fd()), "...not /dev/null");
        let (r, w) = pipe().unwrap();
        assert!(!fd_exposed(r.as_raw_fd()), "...not a pipe");
        assert!(!fd_exposed(w.as_raw_fd()), "...stdout: a pipe");
        // (an fd past any limit: never open)
        assert!(!fd_exposed(999_999), "...not a closed fd");
        let out = std::fs::File::create(d.join("f.out")).unwrap();
        assert!(fd_exposed(out.as_raw_fd()), "...stdout: a file");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A terminal of our own (its other end): posix_openpt's.
    fn tty() -> (OwnedFd, OwnedFd) {
        // SAFETY: the calls take and return plain fds and a static name buffer, read at once.
        unsafe {
            let m = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            assert!(m >= 0);
            assert_eq!(libc::grantpt(m), 0);
            assert_eq!(libc::unlockpt(m), 0);
            let name = std::ffi::CStr::from_ptr(libc::ptsname(m)).to_owned();
            let s = libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
            assert!(s >= 0);
            (OwnedFd::from_raw_fd(m), OwnedFd::from_raw_fd(s))
        }
    }

    fn argv(v: &[&str]) -> Vec<Vec<u8>> {
        v.iter().map(|w| w.as_bytes().to_vec()).collect()
    }

    // t_unit_fd_exposed's stdio_relay: the bytes both ways, the status, the drained output, a writer
    // of stdin the command never reads not waited for; and with run's relays (pipes too, finding
    // 337), a process the command leaves running holds neither the caller's pipe nor the call.
    #[test]
    #[ignore = "starts processes: run by test/run.sh (cargo test -- --ignored)"]
    fn stdio_relay_as_bash() {
        let d = std::env::temp_dir().join(format!("omabox-relay-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let rnd: Vec<u8> = (0..200_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        std::fs::write(d.join("rnd"), &rnd).unwrap();
        std::fs::write(d.join("f"), "x\n").unwrap();
        let open = |n: &str| std::fs::File::open(d.join(n)).unwrap();
        let make = |n: &str| std::fs::File::create(d.join(n)).unwrap();
        let read = |n: &str| std::fs::read(d.join(n)).unwrap();
        let relay = |src: [RawFd; 3], pipes, a: &[&str], cap| {
            stdio_relay_with(src, pipes, &argv(a), cap, false, None).unwrap()
        };
        let (i, o) = (open("rnd"), make("rnd.out"));
        relay([i.as_raw_fd(), o.as_raw_fd(), 2], false, &["cat"], false);
        assert!(
            read("rnd.out") == rnd,
            "stdio_relay: 200 kB through a relayed stdin and stdout, byte for byte"
        );
        let (i, o, e) = (open("f"), make("o"), make("e"));
        let r = relay(
            [i.as_raw_fd(), o.as_raw_fd(), e.as_raw_fd()],
            false,
            &["sh", "-c", "echo out; echo err >&2; exit 7"],
            false,
        );
        assert_eq!(
            (read("o"), read("e"), r.rc),
            (b"out\n".to_vec(), b"err\n".to_vec(), 7),
            "...stdout, stderr and the exit status each its own"
        );
        let t0 = std::time::Instant::now();
        let (i, o) = (open("rnd"), make("o"));
        relay([i.as_raw_fd(), o.as_raw_fd(), 2], false, &["true"], false);
        assert!(
            t0.elapsed() < Duration::from_secs(10),
            "...a command that takes none of a 200 kB stdin returns at once"
        );
        let (i, o) = (open("f"), make("o"));
        relay(
            [i.as_raw_fd(), o.as_raw_fd(), 2],
            false,
            &["sh", "-c", "readlink /proc/self/fd/1"],
            false,
        );
        assert!(
            read("o").starts_with(b"pipe:"),
            "...readlink of a relayed fd: a pipe"
        );
        let both = make("both");
        relay(
            [i.as_raw_fd(), both.as_raw_fd(), both.as_raw_fd()],
            false,
            &[
                "sh",
                "-c",
                "for i in $(seq 300); do echo o$i; echo e$i >&2; done",
            ],
            false,
        );
        let b = String::from_utf8(read("both")).unwrap();
        let l: Vec<&str> = b.lines().collect();
        assert_eq!(
            (l.len(), l[..4].join(" "), l[598..].join(" ")),
            (600, "o1 e1 o2 e2".to_string(), "o300 e300".to_string()),
            "...stdout and stderr to one file (> f 2>&1): every line, in order (one relay for both)"
        );
        let (_m, s) = tty();
        let r = relay(
            [s.as_raw_fd(), 1, 2],
            false,
            &["sh", "-c", "readlink /proc/self/fd/0"],
            true,
        );
        assert_eq!(
            r.out, b"/dev/null\n",
            "...a terminal on stdin alone: the command gets /dev/null, not a relay"
        );
        // finding 337 (#239), run's relays: the caller's stdout a pipe it reads.
        assert!(
            executable(&relay_bin()),
            "omabox-relay is built (make -C tools/relay)"
        );
        let piped = |stdin: RawFd, a: &[&str]| {
            let (r, w) = pipe().unwrap();
            let reader = std::thread::spawn(move || read_all(r, None));
            let t0 = std::time::Instant::now();
            let rc = relay([stdin, w.as_raw_fd(), w.as_raw_fd()], true, a, false).rc;
            let ms = t0.elapsed().as_millis();
            drop(w);
            (reader.join().unwrap(), rc, ms)
        };
        let (out, rc, ms) = piped(i.as_raw_fd(), &["sh", "-c", "sleep 10 & echo hi; exit 3"]);
        assert_eq!(
            (out, rc),
            (b"hi\n".to_vec(), 3),
            "stdio_relay: a pipe held by a leftover (sleep &) is let go: its output, its status"
        );
        assert!(ms < 5000, "...at once ({ms} ms)");
        let alive = d.join("alive");
        let (out, _, _) = piped(
            i.as_raw_fd(),
            &[
                "sh",
                "-c",
                &format!(
                    "(sleep 0.5; echo later; echo late >&2; touch {}) & echo hi",
                    alive.display()
                ),
            ],
        );
        assert_eq!(out, b"hi\n", "...what the leftover writes later is dropped");
        let end = std::time::Instant::now() + Duration::from_secs(5);
        while !alive.exists() && std::time::Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(alive.exists(), "...and it lives on, its writes not failing");
        let ri = open("rnd");
        let (out, _, _) = piped(ri.as_raw_fd(), &["sh", "-c", "sleep 10 & cat"]);
        assert!(out == rnd, "...200 kB through a held pipe, byte for byte");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn env_records() {
        assert_eq!(
            mapfile0(b"A=1\0B=2\0"),
            vec![b"A=1".to_vec(), b"B=2".to_vec()]
        );
        assert_eq!(
            mapfile0(b"A=1\0\0B"),
            vec![b"A=1".to_vec(), Vec::new(), b"B".to_vec()]
        );
        assert!(mapfile0(b"").is_empty());
    }

    // #321: omabox's own helpers (shot's) get the box's HOME, a desk box's at your HOME's path.
    #[test]
    fn sys_env_home_is_the_box_homes() {
        let d = std::env::temp_dir().join(format!("omabox-seh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let ds = d.display().to_string();
        assert_eq!(sys_env(ds.as_bytes())[0], b"HOME=/home/sbx");
        std::fs::write(
            d.join("box.json"),
            r#"{"desktop":"desk","home":"/home/someone"}"#,
        )
        .unwrap();
        assert_eq!(sys_env(ds.as_bytes())[0], b"HOME=/home/someone");
        assert!(BOX_EXEC_SH.contains("p=/opt/omabox/share/bin:$HOME/.local/bin"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
