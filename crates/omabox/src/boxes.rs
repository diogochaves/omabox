//! The boxes under $XDG_RUNTIME_DIR/omabox: which there are, whether each is up, what its runtime
//! dir says, and need_box, every box command's check that its box is up.

use crate::exit::{Ctx, Res};
use crate::jail::{in_jail, jail_id};
use crate::json;
use crate::procfs;
use crate::select;
use crate::shell::{self, strip_nl};
use crate::{cat, jobs};
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// `$D/REL` for box dir D.
pub fn at(d: &[u8], rel: &str) -> PathBuf {
    shell::path(&cat!(d, "/", rel))
}

/// A value of the box's box.json (written by `up`), and whether jq read it.
pub fn meta(d: &[u8], key: &str) -> (Vec<u8>, bool) {
    json::field(&at(d, "box.json"), key)
}

/// box_pid's answer.
pub enum BoxPid {
    None,
    Pid(Vec<u8>),
    /// Its status 3: a box whose state this user namespace cannot read.
    Unknown,
}

/// The box's PID 1 on the host. Top-level boxes run inside pasta's pid namespace (Arch's pasta has
/// no --no-pidns yet: UPSTREAM.md), so bwrap's child-pid is pasta's numbering: pasta_pid records the
/// host one (and is asked again if `up` never got to it). A recorded pid is only the box's while it
/// is still in the box's own pid namespace (box.json `pidns`, written by `up`): after the box died
/// and the pid was reused, it is some other process of the user's, which `down` must not SIGKILL and
/// `run` must not enter.
pub fn box_pid(d: &[u8]) -> BoxPid {
    let pid = if at(d, "pasta.pid").is_file() {
        if pasta_pid(d) {
            std::fs::read(at(d, "pid"))
                .map(|v| strip_nl(&v))
                .unwrap_or_default()
        } else {
            Vec::new()
        }
    } else {
        json::field(&at(d, "info.json"), "child-pid").0
    };
    if pid.is_empty() {
        return BoxPid::None;
    }
    // From another user namespace (unshare -Ur, a sandbox that makes its own) a process of ours has
    // an unreadable ns: that is not a dead box (`up` would clear the live box's dir and orphan it),
    // so it is status 3, which box_alive stops on. A process that is not ours (its pid reused) is
    // unreadable too, and is not the box.
    let Some(ns) = procfs::pidns(&pid) else {
        return if procfs::owner(&pid) == Some(procfs::euid()) {
            BoxPid::Unknown
        } else {
            BoxPid::None
        };
    };
    if Some(&ns) == procfs::pidns(b"self").as_ref() {
        return BoxPid::None;
    }
    let want = meta(d, "pidns").0;
    if !want.is_empty() && ns != want {
        return BoxPid::None;
    }
    BoxPid::Pid(pid)
}

/// Records the box's host pid in $D/pid, unless it is there already (box_pid then checks it is
/// still the box's). Only through this box's own pasta: pasta never removes its pid file, and every
/// box's PID 1 is pid 2 in its pasta's namespace, so a pasta.pid whose pid now belongs to another
/// box's pasta would lead to that box's PID 1, which `down` would then SIGKILL.
fn pasta_pid(d: &[u8]) -> bool {
    if at(d, "pid").metadata().is_ok_and(|m| m.len() > 0) {
        return true;
    }
    let (inner, ok) = json::field(&at(d, "info.json"), "child-pid");
    if !ok || inner.is_empty() {
        return false;
    }
    let Some(pasta) = box_pasta(d) else {
        return false;
    };
    let Some(bw) = procfs::children(&pasta).into_iter().next() else {
        return false;
    };
    // pasta -> bwrap -> the box's PID 1, whose pid in pasta's ns is the child-pid.
    for p in procfs::children(&bw) {
        if procfs::status(&p, "NSpid", 3).is_some_and(|v| v == inner) {
            let _ = std::fs::write(at(d, "pid"), cat!(p, "\n"));
            return true;
        }
    }
    false
}

/// The pid in pasta.pid while it is still this box's pasta: a pasta (its comm: pasta re-execs itself
/// as passt.avx2 on most CPUs, and is non-dumpable, but comm stays readable) whose command line names
/// this pid file. Neither alone: a copy of sleep can be called pasta, and any argv can hold a path.
pub fn box_pasta(d: &[u8]) -> Option<Vec<u8>> {
    let pasta = std::fs::read(at(d, "pasta.pid"))
        .ok()
        .map(|v| strip_nl(&v))
        .filter(|v| !v.is_empty())?;
    let comm = procfs::read(&pasta, "comm")?;
    let named = |n: &[u8]| comm == n || comm.starts_with(&cat!(n, "."));
    if !named(b"pasta") && !named(b"passt") {
        return None;
    }
    let cmdline: Vec<u8> = std::fs::read(shell::path(&cat!("/proc/", pasta, "/cmdline")))
        .unwrap_or_default()
        .into_iter()
        .map(|c| if c == 0 { b' ' } else { c })
        .collect();
    let needle = cat!(" -P ", d, "/pasta.pid ");
    cmdline
        .split(|&c| c == b'\n')
        .any(|l| crate::exit::contains(l, &needle))
        .then_some(pasta)
}

pub fn box_alive(ctx: &Ctx) -> Res<bool> {
    match box_pid(&ctx.d) {
        BoxPid::Unknown => Err(ctx.die(cat!(
            "cannot tell whether box '",
            ctx.name,
            "' is up from this user namespace (a sandbox's?): run omabox outside it"
        ))),
        // Not a zombie (#166): a PID 1 that has exited but is not reaped yet answers kill -0,
        // while its namespaces are gone.
        BoxPid::Pid(p) => Ok(procfs::running(&p)),
        BoxPid::None => Ok(false),
    }
}

/// What down, the reapers and up's waits ask of the system about a box: the faults the suite made
/// by redefining bash's functions (`box_pid() {...}; kill_box`, `rm_box() {...}; cmd_down`) are a
/// #[test]'s fake of this (#324 wave 6-suite, D5). Each method is the real one unless a fake says
/// otherwise; RealBox is the real one throughout.
pub trait BoxSys {
    fn box_pid(&self, d: &[u8]) -> BoxPid {
        box_pid(d)
    }
    fn box_pasta(&self, d: &[u8]) -> Option<Vec<u8>> {
        box_pasta(d)
    }
    fn peek_pids(&self, ctx: &Ctx) -> Vec<Vec<u8>> {
        crate::marks::peek_pids(ctx)
    }
    fn proc_start(&self, pid: &[u8]) -> Option<Vec<u8>> {
        procfs::proc_start(pid)
    }
    fn box_state(&self, d: &[u8]) -> &'static str {
        box_state(d)
    }
    fn box_alive(&self, ctx: &Ctx) -> Res<bool> {
        box_alive(ctx)
    }
    /// `on_box ARGS 2>/dev/null`, its output captured.
    fn on_box(&self, ctx: &Ctx, args: &[Vec<u8>]) -> crate::exec::Ran {
        self.on_box_io(
            ctx,
            args,
            crate::exec::Out::Capture(None),
            crate::exec::ErrTo::Null,
        )
    }
    /// `on_box ARGS`, its stdout and stderr where OUT and ERR say (stdin the caller's).
    fn on_box_io(
        &self,
        ctx: &Ctx,
        args: &[Vec<u8>],
        out: crate::exec::Out,
        err: crate::exec::ErrTo,
    ) -> crate::exec::Ran {
        crate::exec::on_box(ctx, args, false, out, err)
    }
    /// The box's processes, as the host numbers them.
    fn box_pids(&self, ctx: &Ctx) -> Vec<Vec<u8>> {
        crate::jobs::box_pids(ctx)
    }
    /// A process's state as /proc/PID/status says it.
    fn pid_state(&self, pid: &[u8]) -> Vec<u8> {
        crate::hypr::pid_state(pid)
    }
    /// Whether a process's exit is under way (procfs::exiting).
    fn pid_exiting(&self, pid: &[u8]) -> bool {
        procfs::exiting(pid)
    }
    /// The box's desktop now, as one snapshot (desk_snap), in the foreground.
    fn desk_snap(&self, ctx: &Ctx) -> Vec<u8> {
        crate::desk::desk_snap(ctx, false)
    }
    /// The mapped layers that hold the keyboard against any window (kb_layers).
    fn kb_layers(&self, ctx: &Ctx) -> Vec<u8> {
        crate::desk::kb_layers(ctx)
    }
    fn rm_box(&self, ctx: &Ctx) {
        crate::ready::rm_box(ctx);
    }
    fn kill_box(&self, ctx: &Ctx) -> bool {
        crate::ready::kill_box_with(self, ctx)
    }
}

/// The system itself.
pub struct RealBox;

impl BoxSys for RealBox {}

/// up, dead, or unknown: a box whose state this user namespace cannot read (box_pid's 3), for a
/// list, which shows it as that instead of refusing them all (#87).
pub fn box_state(d: &[u8]) -> &'static str {
    match box_pid(d) {
        BoxPid::Unknown => "unknown",
        BoxPid::Pid(p) if procfs::running(&p) => "up",
        _ => "dead",
    }
}

/// A dir with box.json (written by `up` before the launch) or info.json (bwrap's): one that `up`
/// left behind is listed (dead), so `down --all` clears it (finding 163). In a jail, the jail's own
/// boxes; ALL: every box.
pub fn list_names(ctx: &Ctx, all: bool) -> Vec<Vec<u8>> {
    box_dirs(ctx, all)
        .into_iter()
        .filter(|n| box_name_ok(n))
        .collect()
}

/// The folders under the runtime dir that look like boxes' (a box.json or info.json in them) but no
/// box can be named so (#371: one made by hand ended `ls` with select_box's refusal): `ls` names them.
pub fn stray_names(ctx: &Ctx) -> Vec<Vec<u8>> {
    box_dirs(ctx, false)
        .into_iter()
        .filter(|n| !box_name_ok(n))
        .collect()
}

/// Whether N is a name select_box takes as it is.
fn box_name_ok(n: &[u8]) -> bool {
    n.first().is_some_and(u8::is_ascii_alphanumeric) && crate::select::tame(n) == n
}

/// The folders under the runtime dir with a box.json or info.json (in a jail, the jail's, unless ALL).
fn box_dirs(ctx: &Ctx, all: bool) -> Vec<Vec<u8>> {
    let id = if in_jail() && !all {
        jail_id()
    } else {
        Vec::new()
    };
    let boxes = shell::path(&ctx.boxes);
    shell::glob(&boxes, true)
        .into_iter()
        .filter(|n| {
            let d = cat!(ctx.boxes, "/", n, "/");
            (at(&d, "box.json").is_file() || at(&d, "info.json").is_file())
                && (id.is_empty() || meta(&d, "jail").0 == id)
        })
        .collect()
}

/// File NAME of the box's runtime dir (<box dir>/run/NAME), read for the host: its content when it
/// is a regular file and not a link, None otherwise. The box writes that dir: a FIFO there would
/// hang the host's read, a link would read a host file (finding 258).
pub fn run_read(d: &[u8], file: &str) -> Option<Vec<u8>> {
    let f = at(d, &format!("run/{file}"));
    if f.is_symlink() || !f.is_file() {
        return None;
    }
    let mut v = Vec::new();
    File::open(&f).ok()?.take(65536).read_to_end(&mut v).ok()?;
    Some(v)
}

/// One value from the environment Hyprland's children get (share/hyprland.lua writes it at start).
/// The file is in the box's runtime dir, which the box can write: only a tame name is taken
/// (finding 63).
pub fn box_env(d: &[u8], key: &str) -> Option<Vec<u8>> {
    let env = run_read(d, "omabox.env").unwrap_or_default();
    let lead = cat!(key, "=");
    let v = env
        .split(|&c| c == 0 || c == b'\n')
        .find_map(|l| l.strip_prefix(lead.as_slice()))
        .map(<[u8]>::to_vec)
        .unwrap_or_default();
    shell::re_match("^[A-Za-z0-9_.-]+$", &v).then_some(v)
}

/// File PATH of the box's HOME, for the host to read: resolved when it is a regular file, not a
/// link, and inside the HOME once its dirs are resolved. The box writes its HOME freely: a FIFO
/// there hangs the host's read, a link reads a host file (finding 159).
pub fn box_home_file(d: &[u8], file: &str) -> Option<PathBuf> {
    let f = at(d, &format!("home/{file}"));
    if !f.is_file() || f.is_symlink() {
        return None;
    }
    let home = std::fs::canonicalize(at(d, "home")).ok()?;
    let p = std::fs::canonicalize(&f).ok()?;
    p.strip_prefix(&home)
        .is_ok_and(|r| !r.as_os_str().is_empty())
        .then_some(p)
}

/// The last N bytes of a file.
pub fn tail(p: &Path, n: u64) -> Vec<u8> {
    use std::io::{Seek, SeekFrom};
    let Ok(mut f) = File::open(p) else {
        return Vec::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(n)));
    let mut v = Vec::new();
    let _ = f.take(n).read_to_end(&mut v);
    v
}

fn digits(s: &[u8], i: usize, max: usize) -> usize {
    s[i.min(s.len())..]
        .iter()
        .take(max)
        .take_while(|c| c.is_ascii_digit())
        .count()
}

/// Where `(terminated with signal [0-9]{1,2}|exited with -?[0-9]{1,3})` matching at I ends.
fn end_words(s: &[u8], i: usize) -> Option<usize> {
    let r = &s[i..];
    if let Some(t) = r.strip_prefix(b"terminated with signal ") {
        let n = digits(t, 0, 2);
        return (n > 0).then(|| i + 23 + n);
    }
    let t = r.strip_prefix(b"exited with ")?;
    let minus = usize::from(t.first() == Some(&b'-') && digits(t, 1, 1) == 1);
    let n = digits(t, minus, 3);
    (n > 0).then(|| i + 12 + minus + n)
}

/// grep -o's last match of `spawned child [0-9]+ (…)` in TEXT (each line, each match).
fn last_spawned(text: &[u8]) -> Option<Vec<u8>> {
    let mut last = None;
    for line in text.split(|&c| c == b'\n') {
        let mut i = 0;
        while i < line.len() {
            let m = line[i..].strip_prefix(b"spawned child ").and_then(|t| {
                let n = digits(t, 0, usize::MAX);
                (n > 0 && t.get(n) == Some(&b' ')).then(|| end_words(line, i + 14 + n + 1))?
            });
            match m {
                Some(e) => {
                    last = Some(line[i..e].to_vec());
                    i = e;
                }
                None => i += 1,
            }
        }
    }
    last
}

/// The last line that is `omabox: Hyprland (…)` whole.
fn last_session_line(text: &[u8]) -> Option<Vec<u8>> {
    text.split(|&c| c == b'\n')
        .rev()
        .find(|l| {
            l.strip_prefix(b"omabox: Hyprland ")
                .is_some_and(|_| end_words(l, 17) == Some(l.len()))
        })
        .map(<[u8]>::to_vec)
}

/// How the box's Hyprland ended, from labwc's line (headless boxes: labwc's -S child is Hyprland):
/// crashed (SIGSEGV), exited (status N), or None. The box writes its log: only that line read.
pub fn hypr_end(d: &[u8]) -> Option<String> {
    let mut line = box_home_file(d, "labwc.log").and_then(|f| last_spawned(&tail(&f, 65536)));
    // An interactive box has no labwc: session.sh writes its own line at the end of hyprland.log
    // (finding 350).
    if line.is_none() {
        line = box_home_file(d, "hyprland.log").and_then(|f| last_session_line(&tail(&f, 4096)));
    }
    let line = line?;
    if let Some(n) = shell::re_group(r"signal ([0-9]+)$", &line, 1) {
        let n = String::from_utf8_lossy(&n).into_owned();
        return Some(match procfs::signame(n.as_bytes()) {
            Some(s) if !s.is_empty() => format!("crashed (SIG{s})"),
            _ => format!("crashed (signal {n})"),
        });
    }
    let n = shell::re_group(r"with (-?[0-9]+)$", &line, 1)?;
    Some(format!("exited (status {})", String::from_utf8_lossy(&n)))
}

/// A box whose covers (#210) jail_verify has not passed: its PID 1 is held, and nothing may enter it
/// (a command entering it would see its mounts before they are checked). The file is made before the
/// box is launched and only ever goes once the check has passed.
pub fn covers_held(ctx: &Ctx) -> Res<()> {
    if !ctx.d.is_empty() && ctx.at("covers-pending").exists() {
        return Err(ctx.die(cat!(
            "box '",
            ctx.name,
            "' is still starting: what its jail hides is not checked yet (omabox down ",
            ctx.name,
            " ends it)"
        )));
    }
    Ok(())
}

/// `touch FILE`, its failure ignored.
pub fn touch(p: &Path) {
    let f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(p);
    if let Ok(f) = f {
        let now = std::time::SystemTime::now();
        let _ = f.set_times(
            std::fs::FileTimes::new()
                .set_accessed(now)
                .set_modified(now),
        );
    }
}

pub fn need_box(ctx: &Ctx) -> Res<()> {
    if box_alive(ctx)? {
        covers_held(ctx)?;
        touch(&ctx.at("used"));
        if take_over(ctx, false)? {
            jobs::job_note(ctx);
            return Ok(());
        }
    }
    let expired = ctx.in_boxes(cat!(".expired-", ctx.name));
    if expired.is_file() {
        let idle = std::fs::read(&expired)
            .map(|v| strip_nl(&v))
            .unwrap_or_default();
        return Err(ctx.die(cat!(
            "box '",
            ctx.name,
            "' went down after ",
            idle,
            " idle (start it again with: omabox up ",
            ctx.name,
            "; --idle 0 keeps it)"
        )));
    }
    // How its Hyprland ended, when labwc said (#232).
    let end = hypr_end(&ctx.d)
        .map(|e| format!(": its Hyprland {e}"))
        .unwrap_or_default();
    if ctx.at("box.json").is_file() {
        return Err(ctx.die(cat!(
            "box '",
            ctx.name,
            "' is dead",
            end,
            " (why: omabox log -b ",
            ctx.name,
            " all; clear it with: omabox down ",
            ctx.name,
            ")"
        )));
    }
    // A name derived in another directory than the session's box was started in (issue #136): that
    // box first, by name.
    let mine = if ctx.name_derived {
        select::session_boxes(ctx)
    } else {
        Vec::new()
    };
    match mine.as_slice() {
        [] => {}
        [one] => {
            return Err(ctx.die(cat!(
                "no box '",
                ctx.name,
                "' is up; this session's box is '",
                one,
                "' (started from another directory): -b ",
                one
            )));
        }
        _ => {
            return Err(ctx.die(cat!(
                "no box '",
                ctx.name,
                "' is up; this session's boxes are ",
                mine.join(&b' '),
                " (started from other directories): -b NAME"
            )));
        }
    }
    let have: Vec<u8> = list_names(ctx, false)
        .iter()
        .flat_map(|n| cat!(n, " "))
        .collect();
    let up = if have.is_empty() {
        Vec::new()
    } else {
        cat!("up: ", have, "; ")
    };
    Err(ctx.die(cat!(
        "no box '",
        ctx.name,
        "' is up (",
        up,
        "start one with: omabox up ",
        ctx.name,
        ")"
    )))
}

/// Whether the agent `up` recorded for this box still runs: its pid, started when it was then, and
/// not a zombie. None recorded, or no start time, is none alive.
pub fn agent_alive(d: &[u8]) -> bool {
    let (pid, ok1) = meta(d, "agent");
    let (start, ok2) = meta(d, "agent_start");
    if !ok1 || !ok2 || pid.is_empty() || start.is_empty() {
        return false;
    }
    procfs::proc_stat(&pid, 0).unwrap_or_default() != b"Z"
        && procfs::proc_start(&pid).unwrap_or_default() == start
}

/// FILE opened for writing and flocked, waiting up to WAIT. A `down` removes a name's lock file once
/// the box is gone (#158), so a file opened before that may be the removed one, which nobody else
/// sees: then the path is opened again.
pub fn lock_file(p: &Path, wait: Duration) -> Option<File> {
    let end = Instant::now() + wait;
    loop {
        let f = File::create(p).ok()?;
        loop {
            // SAFETY: flock on a descriptor we own, which stays open for the call.
            let rc = unsafe {
                libc::flock(
                    std::os::fd::AsRawFd::as_raw_fd(&f),
                    libc::LOCK_EX | libc::LOCK_NB,
                )
            };
            if rc == 0 {
                break;
            }
            if Instant::now() >= end {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let held = f.metadata().ok().map(|m| (m.dev(), m.ino()));
        let there = std::fs::symlink_metadata(p)
            .ok()
            .map(|m| (m.dev(), m.ino()));
        if held == there {
            return Some(f);
        }
    }
}

/// A session resumed in a new process (`claude --continue` and `--resume` keep its id) finds its box
/// still up and recording the agent that exited, and the reaper would take the box down under the
/// new one at its next check (finding 93). So the session's box goes over to this command's agent,
/// but only then: not a box that records no agent, nor one whose agent still runs. Under the box's
/// lock, which the reaper's `down` takes too before it checks the agent again; `up` holds it already
/// (HELD). False when the box is no longer up once it has the lock (that down came first): the
/// caller then says so.
pub fn take_over(ctx: &Ctx, held: bool) -> Res<bool> {
    let d = &ctx.d;
    if meta(d, "agent").0.is_empty() || agent_alive(d) || !select::session_box(ctx) {
        return Ok(true);
    }
    let Some((pid, start)) = select::agent_proc() else {
        return Ok(true);
    };
    let lock = if held {
        None
    } else {
        match lock_file(
            &ctx.in_boxes(cat!(".lock-", ctx.name)),
            Duration::from_secs(60),
        ) {
            Some(f) => Some(f),
            None => return Ok(true),
        }
    };
    // Both checked again under the lock: the reaper's down, then a new `up` of the name that found
    // no agent, may have come first.
    let mut up = true;
    if !box_alive(ctx)? {
        up = false;
    } else if !meta(d, "agent").0.is_empty() && !agent_alive(d) {
        // (said when it cannot be: bash left box.json.tmp, unsaid, finding 443)
        let tmp = shell::Rm(shell::bytes(&at(d, "box.json.tmp")).to_vec());
        if !(record_agent(d, &pid, &start)
            && std::fs::rename(shell::path(&tmp.0), at(d, "box.json")).is_ok())
        {
            crate::exit::say(cat!(
                "box '",
                ctx.name,
                "' could not be given to this agent (box.json not written): the reaper may take it down"
            ));
        }
    }
    drop(lock);
    Ok(up)
}

/// `jq '.agent = PID | .agent_start = START' box.json > box.json.tmp`: whether it was written.
fn record_agent(d: &[u8], pid: &[u8], start: &[u8]) -> bool {
    use std::io::Write;
    let Ok(mut tmp) = File::create(at(d, "box.json.tmp")) else {
        return false;
    };
    let Ok(text) = std::fs::read(at(d, "box.json")) else {
        return false;
    };
    let Ok(pid) = serde_json::from_slice::<serde_json::Value>(pid) else {
        return false;
    };
    let start = serde_json::Value::String(String::from_utf8_lossy(start).into_owned());
    let (vals, mut ok) = json::values(&text);
    let mut out = String::new();
    for mut v in vals {
        if !json::set(
            &mut v,
            vec![("agent", pid.clone()), ("agent_start", start.clone())],
        ) {
            ok = false;
            break;
        }
        out.push_str(&json::pretty(&v));
        out.push('\n');
    }
    tmp.write_all(out.as_bytes()).is_ok() && ok
}

/// box_file PATH VALUE: VALUE written to a file in the box's runtime dir without opening what is
/// there (a link, a FIFO: finding 74): a new file in the box dir, renamed over it.
pub fn box_file(ctx: &Ctx, p: &std::path::Path, value: &[u8]) -> bool {
    let Ok((mut f, tmp)) = shell::mktemp_in(&cat!(ctx.d, "/.write.XXXXXX")) else {
        return false;
    };
    let ok = std::io::Write::write_all(&mut f, &cat!(value, "\n")).is_ok()
        && std::fs::rename(shell::path(&tmp), p).is_ok();
    if !ok {
        let _ = std::fs::remove_file(shell::path(&tmp));
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    // box_file (finding 74; t_unit_config's): a flag over a link to a FIFO is written, not blocked
    // on; over a link to a host file, that file is untouched and the flag a plain file.
    #[test]
    fn box_file_replaces_links() {
        let d = std::env::temp_dir().join(format!("omabox-boxfile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("run")).unwrap();
        let fifo = d.join("fifo");
        let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a path of ours, NUL-terminated.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        std::fs::write(d.join("host-file"), "keep\n").unwrap();
        std::os::unix::fs::symlink(&fifo, d.join("run/omabox.confirm-close")).unwrap();
        std::os::unix::fs::symlink(d.join("host-file"), d.join("run/omabox.mode")).unwrap();
        let mut ctx = Ctx::new(b"config", b"/x".to_vec());
        ctx.d = shell::bytes(&d).to_vec();
        // (a write that blocked on the FIFO would hang here: the test's own timeout)
        assert!(box_file(&ctx, &d.join("run/omabox.confirm-close"), b"on"));
        assert!(box_file(&ctx, &d.join("run/omabox.mode"), b"1280x720@60"));
        assert_eq!(
            std::fs::read_to_string(d.join("host-file")).unwrap(),
            "keep\n",
            "...over a link to a host file: that file untouched"
        );
        let m = std::fs::symlink_metadata(d.join("run/omabox.mode")).unwrap();
        assert!(m.is_file(), "...the flag a plain file now");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// t_unit_lock_markers: a holder removes the lock file while a waiter waits on it, as down
    /// does: the waiter ends up on the path's file, so a newcomer's try fails while it holds it.
    #[test]
    fn a_lock_removed_under_a_waiter() {
        let d = std::env::temp_dir().join(format!("omabox-lockx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join(".lock-x");
        std::thread::scope(|s| {
            s.spawn(|| {
                let _h = lock_file(&f, Duration::from_secs(5)).unwrap();
                std::thread::sleep(Duration::from_secs(1));
                std::fs::remove_file(&f).unwrap();
                std::thread::sleep(Duration::from_millis(500));
            });
            std::thread::sleep(Duration::from_millis(300));
            let w = lock_file(&f, Duration::from_secs(10)).unwrap();
            assert_eq!(
                w.metadata().unwrap().ino(),
                std::fs::metadata(&f).unwrap().ino(),
                "a lock file removed under a waiter: the waiter locks the new one"
            );
            assert!(
                lock_file(&f, Duration::ZERO).is_none(),
                "...and holds it alone"
            );
        });
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn labwc_line() {
        let t = b"x spawned child 12 exited with 1 y spawned child 13 terminated with signal 115\nspawned child 9 exited with -";
        assert_eq!(
            last_spawned(t),
            Some(b"spawned child 13 terminated with signal 11".to_vec())
        );
        assert_eq!(
            last_spawned(b"spawned child 4 exited with -22"),
            Some(b"spawned child 4 exited with -22".to_vec())
        );
        assert_eq!(last_spawned(b"spawned child x exited with 1"), None);
    }

    #[test]
    fn session_line() {
        let t = b"omabox: Hyprland exited with 3\nomabox: Hyprland terminated with signal 6 \nz";
        assert_eq!(
            last_session_line(t),
            Some(b"omabox: Hyprland exited with 3".to_vec())
        );
        assert_eq!(
            last_session_line(b"omabox: Hyprland exited with 1234"),
            None
        );
    }
}
