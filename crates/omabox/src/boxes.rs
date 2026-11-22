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

/// The box's HOME inside it, for every box but a desk one (#321), and any box from before box.json
/// had a `home`.
pub const SBX_HOME: &str = "/home/sbx";

/// P is absolute, not `/`, and lexically clean: no empty, `.` or `..` part, no trailing slash, no
/// control character.
pub fn clean_abs(p: &[u8]) -> bool {
    p.len() > 1
        && p[0] == b'/'
        && !p.iter().any(|c| c.is_ascii_control())
        && p[1..]
            .split(|c| *c == b'/')
            .all(|c| !c.is_empty() && c != b"." && c != b"..")
}

/// box_home D (#321): the box's HOME inside it, as box.json's `home` says (up writes it, outside
/// what the box can write), when that is a clean absolute path; else SBX_HOME (a box from before
/// it, or a value that is not one).
pub fn box_home(d: &[u8]) -> Vec<u8> {
    let (h, _) = meta(d, "home");
    if clean_abs(&h) {
        h
    } else {
        SBX_HOME.as_bytes().to_vec()
    }
}

/// The box at D is plain-like (#247, #321): a plain box, or a desk box on a plain base (box.json's
/// desktop and desk_base; Desktop::plain_like).
pub fn plain_like(d: &[u8]) -> bool {
    use crate::upspec::{DeskBase, Desktop};
    Desktop::of_box(&meta(d, "desktop").0).plain_like(DeskBase::of_box(&meta(d, "desk_base").0))
}

/// The box at D runs its shell in share/shell.sh's slot (#438): a plain box's `up --shell CMD`
/// (box.json shell_cmd, #247), or a desk box with a shell (#321 step 4, finding 703), whose slot runs
/// what its user's config starts (shellnote::slot_cmd). Omarchy's shell has no slot, a desk box's on
/// Omarchy's base included (its shell is Omarchy's, finding 702).
pub fn slot_shell(d: &[u8]) -> bool {
    !meta(d, "shell_cmd").0.is_empty()
        || (crate::upspec::Desktop::of_box(&meta(d, "desktop").0) == crate::upspec::Desktop::Desk
            && plain_like(d)
            && meta(d, "shell").0 == b"true")
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

/// What a call says of box NAME whose PID 1 this user namespace cannot read (box_pid's Unknown).
pub fn unknown_text(name: &[u8]) -> Vec<u8> {
    cat!(
        "cannot tell whether box '",
        name,
        "' is up from this user namespace (a sandbox's?): run omabox outside it"
    )
}

/// What a call says of box NAME (dir D) that went down after need_box let it through (box_pid's
/// None), ending with the command that fixes it (findings 740, 744, 754), as gone_hint words it from
/// what D's box.json still holds.
pub fn gone_text(d: &[u8], name: &[u8]) -> Vec<u8> {
    let throwaway = !meta(d, "owner").0.is_empty();
    gone_hint(name, throwaway, &meta(d, "mode").0, &meta(d, "desktop").0)
}

/// gone_text's words. A throwaway (`run`'s `NAME-run<pid>`, box.json's `owner`, or a look's own
/// `look-<pid>`) went with its command: no `up` of that name brings it back (round-2 review). Any
/// other: `omabox up NAME`, with --interactive and --desktop as box.json recorded them (MODE,
/// DESKTOP; empty when it is gone), and a word that the rest of its options are not known.
pub fn gone_hint(name: &[u8], throwaway: bool, mode: &[u8], desktop: &[u8]) -> Vec<u8> {
    if throwaway || is_look_name(name) || is_run_name(name) {
        return cat!(
            "box '",
            name,
            "' is not up: it was a throwaway, taken down when the command that made it ended (start a box first, omabox up, and work in it)"
        );
    }
    let mut up = cat!("omabox up ", name);
    if mode == b"interactive" {
        up.extend_from_slice(b" --interactive");
    }
    if desktop == b"plain" || desktop == b"desk" {
        up.extend_from_slice(&cat!(" --desktop ", desktop));
    }
    let rest: &[u8] = if mode.is_empty() && desktop.is_empty() {
        b""
    } else {
        b", and its other options as you gave them"
    };
    cat!("box '", name, "' is not up (start it with: ", up, rest, ")")
}

/// Whether NAME is a `run` throwaway's (`BASE-run<pid>`).
fn is_run_name(name: &[u8]) -> bool {
    let Some(i) = name.windows(4).rposition(|w| w == b"-run") else {
        return false;
    };
    let p = &name[i + 4..];
    i > 0 && !p.is_empty() && p[0] != b'0' && p.iter().all(u8::is_ascii_digit)
}

/// The box's PID 1, or the line that says why there is none (unknown_text or gone_text): one text
/// for every caller that needs the pid (finding 754).
pub fn pid_or_text(ctx: &Ctx) -> Result<Vec<u8>, Vec<u8>> {
    match box_pid(&ctx.d) {
        BoxPid::Pid(p) => Ok(p),
        BoxPid::Unknown => Err(unknown_text(&ctx.name)),
        BoxPid::None => Err(gone_text(&ctx.d, &ctx.name)),
    }
}

/// pid_or_text, its line said (die).
pub fn need_pid(ctx: &Ctx) -> Res<Vec<u8>> {
    pid_or_text(ctx).map_err(|m| ctx.die(m))
}

pub fn box_alive(ctx: &Ctx) -> Res<bool> {
    match box_pid(&ctx.d) {
        BoxPid::Unknown => Err(ctx.die(unknown_text(&ctx.name))),
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
    let mut v = Vec::new();
    open_box_file(&at(d, &format!("run/{file}")))?
        .take(65536)
        .read_to_end(&mut v)
        .ok()?;
    Some(v)
}

/// P, a file the box can write (its runtime dir, its HOME), opened for the host to read: a link at
/// P is refused, not followed (O_NOFOLLOW), a FIFO does not hang the open (O_NONBLOCK), and the fd's
/// own fstat must be a regular file. The check is the open itself, so a box that swaps the file
/// between a look and the read gains nothing (finding 710; it was a look by path, then an open).
pub fn open_box_file(p: &Path) -> Option<File> {
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_CLOEXEC)
        .open(p)
        .ok()?;
    f.metadata().ok()?.is_file().then_some(f)
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

/// File FILE of the box's HOME (a relative path under it), opened for the host to read: walked from
/// the HOME's own fd with no link followed on the way or at the end (openat2's
/// RESOLVE_BENEATH|RESOLVE_NO_SYMLINKS, else an O_NOFOLLOW walk a folder at a time), never waiting on
/// a FIFO (O_NONBLOCK), and only a regular file by the fd's own fstat. The box writes its HOME
/// freely: a FIFO there would hang the host's read, a link (at the file or at a folder on its way)
/// would read a host file, and a look at the path then an open by name lets the box swap it between
/// the two (findings 159, 747). The HOME is the box folder's (`<box dir>/home`), a desk box's too,
/// never the path the box sees, which for a desk box is your real HOME.
pub fn box_home_file(d: &[u8], file: &str) -> Option<File> {
    open_beneath(&at(d, "home"), file)
}

/// box_home_file's content, its first CAP bytes.
pub fn box_home_read(d: &[u8], file: &str, cap: u64) -> Option<Vec<u8>> {
    let mut v = Vec::new();
    box_home_file(d, file)?.take(cap).read_to_end(&mut v).ok()?;
    Some(v)
}

/// What a host read of one box log or config takes at most (S7: bounded on the host's side).
pub const READ_CAP: u64 = 8 << 20;

/// REL, a relative path of plain names, opened under DIR as box_home_file opens one: None for an
/// absolute path, an empty one, a `.` or `..` part, a link anywhere on the way, a FIFO, a folder or
/// anything else but a regular file.
pub fn open_beneath(dir: &Path, rel: &str) -> Option<File> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let parts: Vec<&str> = rel.split('/').collect();
    if rel.is_empty()
        || rel.contains('\0')
        || parts
            .iter()
            .any(|p| p.is_empty() || *p == "." || *p == "..")
    {
        return None;
    }
    // (DIR itself is the host's: the box dir's `home` is a link of up's to the HOME folder, and no
    // box writes the box dir)
    let base = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(dir)
        .ok()?;
    let flags =
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_CLOEXEC;
    let c = std::ffi::CString::new(rel).ok()?;
    // SAFETY: open_how is plain data; zeroed, then the fields set.
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.flags = flags as u64;
    how.resolve = libc::RESOLVE_BENEATH | libc::RESOLVE_NO_SYMLINKS | libc::RESOLVE_NO_MAGICLINKS;
    // SAFETY: openat2 reads the path and how (its size given) and returns a new fd of ours, or -1.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            base.as_raw_fd(),
            c.as_ptr(),
            &how,
            std::mem::size_of::<libc::open_how>(),
        )
    };
    let f = if fd >= 0 {
        // SAFETY: just opened, ours alone.
        File::from(unsafe { OwnedFd::from_raw_fd(fd as i32) })
    } else {
        let e = std::io::Error::last_os_error().raw_os_error();
        // (a kernel or a seccomp filter without openat2: a folder at a time)
        if !matches!(e, Some(libc::ENOSYS | libc::EPERM)) {
            return None;
        }
        walk_beneath(base, &parts, flags)?
    };
    f.metadata().ok()?.is_file().then_some(f)
}

/// open_beneath's way without openat2: each folder opened from its parent's fd, O_NOFOLLOW (a link
/// is ELOOP, never followed), checked a folder by its fstat, then the file with FLAGS.
fn walk_beneath(base: File, parts: &[&str], flags: libc::c_int) -> Option<File> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let mut cur = base;
    let (last, dirs) = parts.split_last()?;
    let open = |at: &File, name: &str, fl: libc::c_int| -> Option<File> {
        let c = std::ffi::CString::new(name).ok()?;
        // SAFETY: openat on an fd of ours and a NUL-terminated name; a new fd of ours, or -1.
        let fd = unsafe { libc::openat(at.as_raw_fd(), c.as_ptr(), fl) };
        // SAFETY: just opened, ours alone.
        (fd >= 0).then(|| File::from(unsafe { OwnedFd::from_raw_fd(fd) }))
    };
    for p in dirs {
        let next = open(
            &cur,
            p,
            libc::O_RDONLY
                | libc::O_DIRECTORY
                | libc::O_NOFOLLOW
                | libc::O_NONBLOCK
                | libc::O_CLOEXEC,
        )?;
        if !next.metadata().ok()?.is_dir() {
            return None;
        }
        cur = next;
    }
    open(&cur, last, flags)
}

/// The last N bytes of F, a file open_box_file or box_home_file opened.
pub fn tail_file(mut f: File, n: u64) -> Vec<u8> {
    use std::io::{Seek, SeekFrom};
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
    let mut line = box_home_file(d, "labwc.log").and_then(|f| last_spawned(&tail_file(f, 65536)));
    // An interactive box has no labwc: session.sh writes its own line at the end of hyprland.log
    // (finding 350).
    if line.is_none() {
        line =
            box_home_file(d, "hyprland.log").and_then(|f| last_session_line(&tail_file(f, 4096)));
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
        crate::whose::mark_used(ctx);
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
    // Taken down by a `down` (#295): when, and by whom.
    if let Some(n) = crate::whose::taken_note(ctx, &crate::whose::me()) {
        return Err(ctx.die(cat!(n, "; start it again with: omabox up ", ctx.name)));
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
    // A look's own box, gone with its look (#471), named for what it is: by its name, or as the
    // last look of this caller's when the name was not given.
    let me = crate::whose::me();
    let gone = if is_look_name(&ctx.name) {
        Some(ctx.name.clone())
    } else if ctx.name_derived {
        crate::whose::look_gone(ctx, &me)
    } else {
        None
    };
    if let Some(l) = gone {
        return Err(ctx.die(look_gone_text(&ctx.name, &l, &up)));
    }
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

/// Whether NAME is a look's own box's (`look-PID`).
pub fn is_look_name(name: &[u8]) -> bool {
    name.strip_prefix(b"look-")
        .is_some_and(|p| !p.is_empty() && p.iter().all(u8::is_ascii_digit))
}

/// need_box's words when box NAME is not up and LOOK, a look's own box, was meant or was the
/// caller's last (UP: need_box's `up: A B; `, or nothing).
fn look_gone_text(name: &[u8], look: &[u8], up: &[u8]) -> Vec<u8> {
    let head = if name == look {
        cat!(
            "no box '",
            name,
            "' is up: it was a look's own box, taken down when the look ended"
        )
    } else {
        cat!(
            "no box '",
            name,
            "' is up; your last look ran in a box of its own, ",
            look,
            ", taken down when it ended"
        )
    };
    cat!(
        head,
        " (",
        up,
        "--keep-box keeps a look's box: omabox look ... --keep-box, then -b BOX; or start a box first, omabox up, and a look works in it)"
    )
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

/// A box or name lock held (`lock_file`): the open, flocked file, and its entry in this thread's
/// list of the locks it holds, which `edit_box_json` reads so that a holder's write does not wait on
/// its own lock (flock locks one open file, so a second open in the same process would).
pub struct Lock {
    f: File,
    key: (u64, u64),
    // The list is the thread's: a Lock stays on the thread that took it (not Send).
    _thread: std::marker::PhantomData<*const ()>,
}

thread_local! {
    static HELD: std::cell::RefCell<Vec<(u64, u64)>> = const { std::cell::RefCell::new(Vec::new()) };
}

impl std::ops::Deref for Lock {
    type Target = File;
    fn deref(&self) -> &File {
        &self.f
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        HELD.with(|h| {
            let mut h = h.borrow_mut();
            if let Some(i) = h.iter().position(|k| *k == self.key) {
                h.swap_remove(i);
            }
        });
    }
}

/// Whether this thread holds the lock that P names now (a file removed and made again under its
/// name is another lock).
pub fn holds(p: &Path) -> bool {
    let Ok(m) = std::fs::symlink_metadata(p) else {
        return false;
    };
    HELD.with(|h| h.borrow().contains(&(m.dev(), m.ino())))
}

/// FILE opened for writing and flocked, waiting up to WAIT. A `down` removes a name's lock file once
/// the box is gone (#158), so a file opened before that may be the removed one, which nobody else
/// sees: then the path is opened again.
pub fn lock_file(p: &Path, wait: Duration) -> Option<Lock> {
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
        if let Some(key) = held
            && held == there
        {
            HELD.with(|h| h.borrow_mut().push(key));
            return Some(Lock {
                f,
                key,
                _thread: std::marker::PhantomData,
            });
        }
    }
}

/// A unique name for a temporary file in a box dir: this process's pid and a count.
fn tmp_name() -> String {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        ".box.json.{}.{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// The one way box.json is changed: under the box's lock (taken unless this thread holds it), EDIT
/// given box.json's path returns the new text (None: not written, EDIT having said why or not), which
/// goes to a new file of a name of its own in the box dir (O_EXCL), synced, and renamed over
/// box.json. Two writers no longer share one temporary file (a short write after a long one left the
/// long one's tail: invalid JSON), and a write cannot put back what another wrote after it read
/// (finding 753). Whether box.json was written; a box gone (its dir or box.json) is false, unsaid,
/// and leaves no lock file behind (down removes it, #158); a write or rename that fails is said.
pub fn edit_box_json(ctx: &Ctx, edit: impl FnOnce(&Path) -> Option<Vec<u8>>) -> bool {
    edit_box_json_in(
        &ctx.d,
        &ctx.in_boxes(cat!(".lock-", ctx.name)),
        &ctx.label(),
        edit,
    )
}

/// What edit_box_json_wait did: box.json written; the box gone (its dir or box.json), before or
/// while it waited; the lock not had in time (nothing said); or not written, EDIT or the write
/// having said why (or not).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edited {
    Written,
    Gone,
    Busy,
    NotWritten,
}

/// edit_box_json waiting up to WAIT for the box's lock, telling a lock not had (Busy, unsaid) and a
/// box gone apart from a write that failed: for a caller whose edit is only a record, which says its
/// own line for each (restart-shell's version, round-2 review).
pub fn edit_box_json_wait(
    ctx: &Ctx,
    wait: Duration,
    edit: impl FnOnce(&Path) -> Option<Vec<u8>>,
) -> Edited {
    edit_box_json_try(
        &ctx.d,
        &ctx.in_boxes(cat!(".lock-", ctx.name)),
        &ctx.label(),
        wait,
        edit,
    )
}

/// How long a record that follows the work (restart-shell's version, plugin_check's states) waits
/// for the box's lock when its thread does not hold it: a save holds it for its whole copy, and the
/// work itself is done (round-2 review: restart-shell waited 60 s, then exited 1).
pub const RECORD_WAIT: Duration = Duration::from_secs(3);

/// The line for a record edit_box_json_wait did not write because the lock was busy or the box is
/// gone (None for the rest: written, or said already). LABEL before it, NAME the box, WHAT the record.
pub fn record_note(label: &[u8], name: &[u8], what: &str, e: Edited) -> Option<Vec<u8>> {
    match e {
        Edited::Busy => Some(cat!(
            label,
            "note: ",
            what,
            " not recorded in box '",
            name,
            "''s box.json: its lock was busy for ",
            RECORD_WAIT.as_secs().to_string(),
            " s (a save, plugin add or mode at work); ls --json shows the one before"
        )),
        Edited::Gone => Some(cat!(
            label,
            "note: box '",
            name,
            "' went down: ",
            what,
            " not recorded"
        )),
        Edited::Written | Edited::NotWritten => None,
    }
}

/// edit_box_json on box dir D, its lock LK, LABEL before what is said.
pub fn edit_box_json_in(
    d: &[u8],
    lk: &Path,
    label: &[u8],
    edit: impl FnOnce(&Path) -> Option<Vec<u8>>,
) -> bool {
    match edit_box_json_try(d, lk, label, Duration::from_secs(60), edit) {
        Edited::Written => true,
        Edited::Busy => {
            crate::exit::say(cat!(
                label,
                shell::bytes(&at(d, "box.json")),
                " not written: its box's lock (",
                shell::bytes(lk),
                ") was not had within 60 s (an up or a down in progress?)"
            ));
            false
        }
        Edited::Gone | Edited::NotWritten => false,
    }
}

/// edit_box_json_in waiting up to WAIT for the lock, saying nothing for Busy or Gone.
fn edit_box_json_try(
    d: &[u8],
    lk: &Path,
    label: &[u8],
    wait: Duration,
    edit: impl FnOnce(&Path) -> Option<Vec<u8>>,
) -> Edited {
    use std::io::Write;
    let bj = at(d, "box.json");
    if !bj.is_file() {
        return Edited::Gone;
    }
    let _lock = if holds(lk) {
        None
    } else {
        match lock_file(lk, wait) {
            Some(l) => Some(l),
            None => return Edited::Busy,
        }
    };
    // A down that came first while we waited: the box is gone, and so goes the lock file we may have
    // made again, removed while held as down does.
    if !bj.is_file() {
        if _lock.is_some() && !shell::path(d).is_dir() {
            let _ = std::fs::remove_file(lk);
        }
        return Edited::Gone;
    }
    let Some(out) = edit(&bj) else {
        return Edited::NotWritten;
    };
    let tmp = at(d, &tmp_name());
    let mut f = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
    {
        Ok(f) => f,
        Err(e) => {
            crate::exit::say(cat!(
                label,
                crate::shots::cant_write(shell::bytes(&tmp), &e)
            ));
            return Edited::NotWritten;
        }
    };
    let _rm = shell::Rm(shell::bytes(&tmp).to_vec());
    if let Err(e) = f.write_all(&out).and_then(|()| f.sync_all()) {
        crate::exit::say(cat!(
            label,
            crate::shots::cant_write(shell::bytes(&tmp), &e)
        ));
        return Edited::NotWritten;
    }
    drop(f);
    if let Err(e) = std::fs::rename(&tmp, &bj) {
        crate::exit::say(cat!(label, crate::shots::cant_write(shell::bytes(&bj), &e)));
        return Edited::NotWritten;
    }
    Edited::Written
}

/// edit_box_json setting KEYS (`jq '.K1 = V1 | ...'` on each value box.json holds): false, unsaid,
/// when box.json is no JSON or holds a value that is no object.
pub fn set_box_json(ctx: &Ctx, keys: Vec<(&str, serde_json::Value)>) -> bool {
    edit_box_json(ctx, |bj| set_keys(bj, keys))
}

/// box.json at BJ with KEYS set in each of its values, as jq writes it.
fn set_keys(bj: &Path, keys: Vec<(&str, serde_json::Value)>) -> Option<Vec<u8>> {
    let text = std::fs::read(bj).ok()?;
    let (vals, ok) = json::values(&text);
    if !ok {
        return None;
    }
    let mut out = String::new();
    for mut v in vals {
        if !json::set(&mut v, keys.clone()) {
            return None;
        }
        out.push_str(&json::pretty(&v));
        out.push('\n');
    }
    Some(out.into_bytes())
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
        let start = serde_json::Value::String(String::from_utf8_lossy(&start).into_owned());
        let set = serde_json::from_slice::<serde_json::Value>(&pid)
            .is_ok_and(|pid| set_box_json(ctx, vec![("agent", pid), ("agent_start", start)]));
        if !set {
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

    /// Round-2 review: the gone line names no `up` for a throwaway (run's, look's), and keeps an
    /// interactive or desk box's options box.json recorded.
    #[test]
    fn gone_hint_by_kind() {
        let t = |n: &str, o: bool, m: &str, dk: &str| {
            String::from_utf8(gone_hint(n.as_bytes(), o, m.as_bytes(), dk.as_bytes())).unwrap()
        };
        let thrown = "' is not up: it was a throwaway, taken down when the command that made it ended (start a box first, omabox up, and work in it)";
        assert_eq!(
            t("proj-run12345", false, "", ""),
            format!("box 'proj-run12345{thrown}")
        );
        assert_eq!(
            t("look-777", false, "", ""),
            format!("box 'look-777{thrown}")
        );
        assert_eq!(
            t("x", true, "headless", "omarchy"),
            format!("box 'x{thrown}")
        );
        assert_eq!(
            t("bx", false, "", ""),
            "box 'bx' is not up (start it with: omabox up bx)"
        );
        assert_eq!(
            t("bx", false, "headless", "omarchy"),
            "box 'bx' is not up (start it with: omabox up bx, and its other options as you gave them)"
        );
        assert_eq!(
            t("d", false, "interactive", "desk"),
            "box 'd' is not up (start it with: omabox up d --interactive --desktop desk, and its other options as you gave them)"
        );
        // (not a run's: no pid after -run, or the word only)
        for n in ["-run12", "my-run", "a-run0", "a-runx1", "rerun5"] {
            assert!(
                t(n, false, "", "").contains("start it with: omabox up"),
                "{n}"
            );
        }
    }

    /// Finding 754: a box gone after need_box, or one this namespace cannot read, said in one text
    /// for every caller, each ending with what fixes it (740, 744).
    #[test]
    fn need_pid_says_its_fix() {
        let mut ctx = Ctx::new(b"run", b"/nonexistent/omabox-754".to_vec());
        ctx.parsing = false;
        ctx.name = b"bx".to_vec();
        ctx.d = cat!(ctx.boxes, "/bx");
        assert_eq!(
            pid_or_text(&ctx),
            Err(b"box 'bx' is not up (start it with: omabox up bx)".to_vec())
        );
        let (r, said) = crate::exit::heard(|| need_pid(&ctx));
        assert_eq!(r, Err(crate::exit::Exit(1)));
        assert!(
            said.contains("omabox: box 'bx' is not up (start it with: omabox up bx)"),
            "{said}"
        );
        assert_eq!(
            unknown_text(b"bx"),
            b"cannot tell whether box 'bx' is up from this user namespace (a sandbox's?): run omabox outside it".to_vec()
        );
    }

    /// #471: no box up where a look's own box was meant says so, and how to keep or start one.
    #[test]
    fn a_looks_box_gone_said() {
        assert!(is_look_name(b"look-123"));
        assert!(!is_look_name(b"look-"));
        assert!(!is_look_name(b"look-a1"));
        assert!(!is_look_name(b"mylook-1"));
        let s = |n: &str, l: &str, up: &str| {
            String::from_utf8(look_gone_text(n.as_bytes(), l.as_bytes(), up.as_bytes())).unwrap()
        };
        assert_eq!(
            s("look-7", "look-7", ""),
            "no box 'look-7' is up: it was a look's own box, taken down when the look ended (--keep-box keeps a look's box: omabox look ... --keep-box, then -b BOX; or start a box first, omabox up, and a look works in it)"
        );
        assert_eq!(
            s("proj", "look-7", "up: a b ; "),
            "no box 'proj' is up; your last look ran in a box of its own, look-7, taken down when it ended (up: a b ; --keep-box keeps a look's box: omabox look ... --keep-box, then -b BOX; or start a box first, omabox up, and a look works in it)"
        );
    }

    // box_home (#321): box.json's `home` when it is a clean absolute path, else /home/sbx.
    #[test]
    fn box_home_defaults() {
        let d = std::env::temp_dir().join(format!("omabox-boxhome-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let db = shell::bytes(&d).to_vec();
        // No box.json, or one from before `home`: /home/sbx.
        assert_eq!(box_home(&db), SBX_HOME.as_bytes());
        std::fs::write(d.join("box.json"), r#"{"desktop":"plain"}"#).unwrap();
        assert_eq!(box_home(&db), SBX_HOME.as_bytes());
        for (home, want) in [
            ("/home/sbx", "/home/sbx"),
            ("/home/u", "/home/u"),
            ("/var/home/u", "/var/home/u"),
            ("/root", "/root"),
            ("", SBX_HOME),
            ("/", SBX_HOME),
            ("home/u", SBX_HOME),
            ("/home/u/", SBX_HOME),
            ("/home//u", SBX_HOME),
            ("/home/./u", SBX_HOME),
            ("/home/../etc", SBX_HOME),
            ("/home/u/..", SBX_HOME),
            ("/home/u\n/x", SBX_HOME),
        ] {
            let j = serde_json::json!({ "desktop": "desk", "home": home });
            std::fs::write(d.join("box.json"), j.to_string()).unwrap();
            assert_eq!(box_home(&db), want.as_bytes(), "{home:?}");
        }
        // Not a string: the default.
        std::fs::write(d.join("box.json"), r#"{"home":["/home/u"]}"#).unwrap();
        assert_eq!(box_home(&db), SBX_HOME.as_bytes());
        let _ = std::fs::remove_dir_all(&d);
    }

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

    /// box_home_file (finding 747): a file of the box HOME is opened from the HOME's fd, a folder at a
    /// time with no link followed (at the file or on its way), a FIFO never waited on, only a regular
    /// file kept; `..`, `.`, an absolute path and an empty one are refused. Both ways: openat2's, and
    /// the O_NOFOLLOW walk a kernel without it takes.
    #[test]
    fn box_home_file_no_link_no_fifo_no_escape() {
        let d = std::env::temp_dir().join(format!("omabox-homefile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let real = d.join("homes/b1");
        std::fs::create_dir_all(real.join(".config/omarchy")).unwrap();
        std::fs::create_dir_all(d.join("box")).unwrap();
        // (the box dir's `home` is a link of up's to the HOME folder: followed, it is the host's)
        std::os::unix::fs::symlink(&real, d.join("box/home")).unwrap();
        std::fs::create_dir_all(d.join("host/.config/omarchy")).unwrap();
        std::fs::write(d.join("host/secret"), "host secret\n").unwrap();
        std::fs::write(d.join("host/.config/omarchy/shell.json"), "host\n").unwrap();
        std::fs::write(real.join(".config/omarchy/shell.json"), "{}\n").unwrap();
        std::fs::write(real.join("run-1.log"), "box's\n").unwrap();
        std::os::unix::fs::symlink(d.join("host/secret"), real.join("run-2.log")).unwrap();
        std::os::unix::fs::symlink("run-1.log", real.join("run-3.log")).unwrap();
        std::os::unix::fs::symlink(d.join("host/.config"), real.join(".cfg")).unwrap();
        std::fs::create_dir_all(real.join(".local/state/x")).unwrap();
        std::os::unix::fs::symlink(d.join("host"), real.join(".local/state/x/up")).unwrap();
        let c =
            std::ffi::CString::new(real.join("shell.log").as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a path of ours, NUL-terminated.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let db = shell::bytes(&d.join("box")).to_vec();
        let home = d.join("box/home");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let read = |f: Option<File>| {
                f.map(|mut f| {
                    let mut v = Vec::new();
                    f.read_to_end(&mut v).unwrap();
                    String::from_utf8(v).unwrap()
                })
            };
            let names = [
                "run-1.log",
                ".config/omarchy/shell.json",
                "run-2.log",
                "run-3.log",
                "shell.log",
                ".cfg/omarchy/shell.json",
                ".local/state/x/up/secret",
                "../../host/secret",
                ".config/../run-1.log",
                "./run-1.log",
                ".config",
                "",
                "/etc/passwd",
                ".config//omarchy/shell.json",
            ];
            let mut r = Vec::new();
            for n in names {
                r.push((n, read(box_home_file(&db, n))));
            }
            // The walk without openat2, on the same names.
            for n in names {
                let parts: Vec<&str> = n.split('/').collect();
                let w = if parts
                    .iter()
                    .any(|p| p.is_empty() || *p == "." || *p == "..")
                {
                    None
                } else {
                    let base = File::open(&home).unwrap();
                    walk_beneath(
                        base,
                        &parts,
                        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK,
                    )
                    .filter(|f| f.metadata().is_ok_and(|m| m.is_file()))
                };
                r.push((n, read(w)));
            }
            let _ = tx.send(r);
        });
        let r = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("an open waited on a FIFO");
        for half in r.chunks(14) {
            let got: Vec<(&str, Option<&str>)> =
                half.iter().map(|(n, v)| (*n, v.as_deref())).collect();
            assert_eq!(got[0], ("run-1.log", Some("box's\n")));
            assert_eq!(got[1], (".config/omarchy/shell.json", Some("{}\n")));
            for (n, v) in &got[2..] {
                assert_eq!(
                    *v, None,
                    "{n}: refused (a link, a FIFO, a way out, a folder)"
                );
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// open_box_file (finding 710): what run_read and tail read of a box's files. A FIFO is not
    /// waited on, a link to a host file not read (at run/, in the HOME), a regular file read.
    #[test]
    fn box_files_open_no_fifo_no_link() {
        let d = std::env::temp_dir().join(format!("omabox-boxopen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("run")).unwrap();
        std::fs::create_dir_all(d.join("home")).unwrap();
        let c = std::ffi::CString::new(d.join("run/fifo").as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a path of ours, NUL-terminated.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        std::fs::write(d.join("host-file"), "host secret\n").unwrap();
        std::os::unix::fs::symlink(d.join("host-file"), d.join("run/link")).unwrap();
        std::os::unix::fs::symlink(d.join("host-file"), d.join("home/hyprland.log")).unwrap();
        std::os::unix::fs::symlink(d.join("run/fifo"), d.join("home/gdb.log")).unwrap();
        std::fs::write(d.join("run/ok"), "fine\n").unwrap();
        let db = shell::bytes(&d).to_vec();
        let tail = |p: &Path, n| {
            open_box_file(p)
                .map(|f| tail_file(f, n))
                .unwrap_or_default()
        };
        // (each in a thread: an open that waited on the FIFO would hang; 5 s says so)
        let (tx, rx) = std::sync::mpsc::channel();
        let d2 = d.clone();
        std::thread::spawn(move || {
            let r = (
                run_read(&db, "fifo"),
                run_read(&db, "link"),
                run_read(&db, "ok"),
                tail(&d2.join("run/fifo"), 512),
                tail(&d2.join("home/hyprland.log"), 512),
                tail(&d2.join("home/gdb.log"), 512),
                tail(&d2.join("run/ok"), 3),
            );
            let _ = tx.send(r);
        });
        let r = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("a read waited on a FIFO");
        assert_eq!(r.0, None, "run_read: a FIFO");
        assert_eq!(r.1, None, "run_read: a link to a host file");
        assert_eq!(r.2.as_deref(), Some(&b"fine\n"[..]));
        assert_eq!(r.3, b"", "tail: a FIFO");
        assert_eq!(r.4, b"", "tail: a link to a host file");
        assert_eq!(r.5, b"", "tail: a link to a FIFO");
        assert_eq!(r.6, b"ne\n");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// One box.json edit: key K one more, read and written back, as every writer did (finding 753).
    fn bump(d: &[u8], lk: &Path, k: &str) -> bool {
        edit_box_json_in(d, lk, b"", |bj| {
            let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(bj).ok()?).ok()?;
            let n = v.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
            // Widen the window between the read and the write.
            std::thread::sleep(Duration::from_micros(200));
            json::set(&mut v, vec![(k, serde_json::Value::from(n + 1))]);
            Some(json::pretty(&v).into_bytes())
        })
    }

    /// Finding 753: writers editing different keys of one box.json at once lose none of their
    /// edits, box.json stays one JSON value, and no temporary file is left. (The writers' old way,
    /// one fixed box.json.tmp and no lock, lost most edits and broke the JSON.)
    #[test]
    fn box_json_edits_at_once_all_kept() {
        let d = std::env::temp_dir().join(format!("omabox-bj-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("box.json"), "{\"created\": \"1\"}\n").unwrap();
        let db = shell::bytes(&d).to_vec();
        let lk = d.join(".lock-x");
        let (threads, each) = (8, 25);
        std::thread::scope(|s| {
            for t in 0..threads {
                let (db, lk) = (&db, &lk);
                s.spawn(move || {
                    let k = format!("k{t}");
                    for _ in 0..each {
                        assert!(bump(db, lk, &k), "an edit not written");
                    }
                });
            }
        });
        let text = std::fs::read(d.join("box.json")).unwrap();
        let (vals, ok) = json::values(&text);
        assert!(
            ok && vals.len() == 1,
            "box.json is one JSON value: {text:?}"
        );
        for t in 0..threads {
            assert_eq!(
                vals[0]
                    .get(format!("k{t}"))
                    .and_then(serde_json::Value::as_u64),
                Some(each),
                "every edit of k{t} kept: {text:?}"
            );
        }
        assert_eq!(vals[0]["created"], "1", "the key nobody edits kept");
        let left: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.starts_with(".box.json") || n.ends_with(".tmp"))
            .collect();
        assert!(left.is_empty(), "no temporary file left: {left:?}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// Finding 753: a holder of the box's lock edits box.json at once (no wait on its own lock); a
    /// box gone is no edit and leaves no lock file; an edit that gives nothing writes nothing.
    #[test]
    fn box_json_edit_held_and_gone() {
        let d = std::env::temp_dir().join(format!("omabox-bjh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("b")).unwrap();
        std::fs::write(d.join("b/box.json"), "{}\n").unwrap();
        let db = shell::bytes(&d.join("b")).to_vec();
        let lk = d.join(".lock-b");
        let held = lock_file(&lk, Duration::ZERO).unwrap();
        assert!(holds(&lk));
        let t0 = Instant::now();
        assert!(bump(&db, &lk, "a"), "the holder's edit");
        assert!(
            t0.elapsed() < Duration::from_secs(5),
            "...not waiting on its own lock"
        );
        // Another thread waits for it.
        std::thread::scope(|s| {
            let h = s.spawn(|| {
                assert!(!holds(&lk), "the lock is the thread's");
                lock_file(&lk, Duration::from_millis(100)).is_none()
            });
            assert!(
                h.join().unwrap(),
                "...which another thread cannot have meanwhile"
            );
        });
        drop(held);
        assert!(!holds(&lk));
        assert!(!edit_box_json_in(&db, &lk, b"", |_| None), "no edit");
        assert_eq!(
            std::fs::read(d.join("b/box.json")).unwrap(),
            b"{\n  \"a\": 1\n}"
        );
        std::fs::remove_dir_all(d.join("b")).unwrap();
        std::fs::remove_file(&lk).unwrap();
        assert!(!bump(&db, &lk, "a"), "a box gone: not written");
        assert!(!lk.exists(), "...and no lock file made");
        assert!(!d.join("b").exists(), "...nor its dir");
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// Round-2 review: a record that follows the work waits RECORD_WAIT-short for a busy lock and
    /// tells it (Busy) and a box gone (Gone) apart from a failed write, each with its own line.
    #[test]
    fn box_json_record_busy_and_gone() {
        let d = std::env::temp_dir().join(format!("omabox-bjr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("b")).unwrap();
        std::fs::write(d.join("b/box.json"), "{}\n").unwrap();
        let db = shell::bytes(&d.join("b")).to_vec();
        let lk = d.join(".lock-b");
        let edit = |_: &Path| Some(b"{\"v\": 1}\n".to_vec());
        // Another thread holds the lock (a save): not waited out.
        std::thread::scope(|s| {
            let (tx, rx) = std::sync::mpsc::channel();
            let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
            let lk2 = lk.clone();
            s.spawn(move || {
                let _l = lock_file(&lk2, Duration::ZERO).unwrap();
                tx.send(()).unwrap();
                let _ = go_rx.recv();
            });
            rx.recv().unwrap();
            let t0 = Instant::now();
            assert_eq!(
                edit_box_json_try(&db, &lk, b"", Duration::from_millis(200), edit),
                Edited::Busy
            );
            assert!(t0.elapsed() < Duration::from_secs(5), "...not a 60 s wait");
            go_tx.send(()).unwrap();
        });
        assert_eq!(
            std::fs::read(d.join("b/box.json")).unwrap(),
            b"{}\n",
            "...nothing written"
        );
        assert_eq!(
            edit_box_json_try(&db, &lk, b"", Duration::ZERO, edit),
            Edited::Written
        );
        assert_eq!(
            edit_box_json_try(&db, &lk, b"", Duration::ZERO, |_| None),
            Edited::NotWritten
        );
        std::fs::remove_dir_all(d.join("b")).unwrap();
        let _ = std::fs::remove_file(&lk);
        assert_eq!(
            edit_box_json_try(&db, &lk, b"", Duration::ZERO, edit),
            Edited::Gone
        );
        let busy = record_note(b"restart-shell: ", b"b", "the X", Edited::Busy).unwrap();
        assert!(
            busy.starts_with(b"restart-shell: note: the X not recorded in box 'b''s box.json: its lock was busy for 3 s"),
            "{}",
            String::from_utf8_lossy(&busy)
        );
        assert_eq!(
            record_note(b"", b"b", "the X", Edited::Gone).unwrap(),
            b"note: box 'b' went down: the X not recorded"
        );
        assert_eq!(record_note(b"", b"b", "x", Edited::Written), None);
        assert_eq!(record_note(b"", b"b", "x", Edited::NotWritten), None);
        std::fs::remove_dir_all(&d).unwrap();
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

    // #385: how a box's Hyprland ended, from labwc's line or session.sh's: a signal by its name, one
    // with no name (32, 33: glibc's own; past RTMAX) by its number, an exit by its status.
    #[test]
    fn hypr_end_words() {
        let base = std::env::temp_dir().join(format!("omabox-hypr-end-{}", std::process::id()));
        let d = base.join("box");
        std::fs::create_dir_all(d.join("home")).unwrap();
        let db = d.as_os_str().as_encoded_bytes();
        let end = |file: &str, line: &str| {
            let _ = std::fs::remove_file(d.join("home/labwc.log"));
            let _ = std::fs::remove_file(d.join("home/hyprland.log"));
            std::fs::write(d.join("home").join(file), line).unwrap();
            hypr_end(db)
        };
        for (file, line, want) in [
            (
                "labwc.log",
                "x spawned child 12 terminated with signal 11",
                "crashed (SIGSEGV)",
            ),
            (
                "labwc.log",
                "spawned child 12 terminated with signal 32",
                "crashed (signal 32)",
            ),
            (
                "labwc.log",
                "spawned child 12 terminated with signal 99",
                "crashed (signal 99)",
            ),
            (
                "labwc.log",
                "spawned child 9 exited with 3",
                "exited (status 3)",
            ),
            (
                "hyprland.log",
                "a\nomabox: Hyprland terminated with signal 33\n",
                "crashed (signal 33)",
            ),
            (
                "hyprland.log",
                "omabox: Hyprland terminated with signal 6\n",
                "crashed (SIGABRT)",
            ),
        ] {
            assert_eq!(end(file, line).as_deref(), Some(want), "{file}: {line:?}");
        }
        assert_eq!(end("hyprland.log", "nothing said\n"), None);
        std::fs::remove_dir_all(&base).unwrap();
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
