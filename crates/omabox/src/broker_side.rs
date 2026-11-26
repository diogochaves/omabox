//! The broker's side of a jailed agent's call (#324 wave 6-front, A3; issue #16, finding 99):
//! omabox-relay's `listen` ran this omabox for a caller inside ai-jail. broker_init finds the
//! caller's jail and reads its policy from the jail's own bwrap (OMABOX_JAIL, which everything that
//! matters reads and every omabox this one starts inherits); broker_check says what a jailed agent
//! may ask, before any command runs (internal `_` ones too).

use crate::cat;
use crate::exit::{Ctx, Res};
use crate::json;
use crate::paths::{at_or_under, below, under};
use crate::procfs;
use crate::shell;
use serde_json::{Map, Value};
use std::os::unix::fs::OpenOptionsExt;

/// bwrap's options and how many words each takes after it (BWRAP_ARITY).
fn arity(op: &[u8]) -> Option<usize> {
    let o = op.strip_prefix(b"--")?;
    Some(match o {
        b"unshare-user"
        | b"unshare-user-try"
        | b"unshare-ipc"
        | b"unshare-pid"
        | b"unshare-net"
        | b"unshare-uts"
        | b"unshare-cgroup"
        | b"unshare-cgroup-try"
        | b"unshare-all"
        | b"share-net"
        | b"die-with-parent"
        | b"as-pid-1"
        | b"new-session"
        | b"clearenv"
        | b"disable-userns"
        | b"assert-userns-disabled"
        | b"level-prefix" => 0,
        b"tmpfs" | b"dev" | b"proc" | b"mqueue" | b"dir" | b"remount-ro" | b"chdir"
        | b"hostname" | b"unsetenv" | b"perms" | b"size" | b"uid" | b"gid" | b"userns"
        | b"userns2" | b"pidns" | b"seccomp" | b"add-seccomp-fd" | b"exec-label"
        | b"file-label" | b"block-fd" | b"userns-block-fd" | b"info-fd" | b"json-status-fd"
        | b"lock-file" | b"sync-fd" | b"cap-add" | b"cap-drop" | b"argv0" | b"overlay-src"
        | b"tmp-overlay" | b"ro-overlay" => 1,
        b"bind" | b"bind-try" | b"dev-bind" | b"dev-bind-try" | b"ro-bind" | b"ro-bind-try"
        | b"setenv" | b"symlink" | b"file" | b"bind-data" | b"ro-bind-data" | b"chmod"
        | b"bind-fd" | b"ro-bind-fd" => 2,
        b"overlay" => 3,
        _ => return None,
    })
}

/// `mapfile -d ''`: each NUL-ended word, and a last one without its NUL.
fn words0(v: &[u8]) -> Vec<Vec<u8>> {
    let mut w: Vec<Vec<u8>> = v.split(|&b| b == 0).map(<[u8]>::to_vec).collect();
    if w.last().is_some_and(Vec::is_empty) {
        w.pop();
    }
    w
}

/// `${p%/}`: one trailing slash off.
fn trim1(p: &[u8]) -> &[u8] {
    p.strip_suffix(b"/").unwrap_or(p)
}

fn text(b: &[u8]) -> Value {
    Value::String(String::from_utf8_lossy(b).into_owned())
}

/// How policy_of reads `--args N`'s memfd: (ai-jail's fd dir, N) to its words, or why not.
pub type ReadFd<'a> = &'a dyn Fn(&[u8], &[u8]) -> Result<Vec<u8>, String>;

/// A bwrap command line's policy, as jail_policy reads it: whether it has the host's network, the
/// host folders it sees at their own path (bind mounts with source and destination the same that no
/// later mount covers), whether something of the jail's is mounted inside each, and those mounts
/// (`masks`). FDS: ai-jail's /proc/PID/fd, where `--args N` (ai-jail 2.6.2's #147) names its
/// options memfd. READ_FD reads that fd's words, and RESOLVE resolves a path (readlink -e): the two
/// things outside the words, as arguments for the unit tests. Err: why it cannot be read.
pub fn policy_of(
    cmdline: &[u8],
    fds: Option<&[u8]>,
    read_fd: ReadFd,
    resolve: &dyn Fn(&[u8]) -> Option<Vec<u8>>,
) -> Result<Value, String> {
    let mut a = words0(cmdline);
    let (mut i, mut net, mut expanded) = (1usize, true, false);
    let (mut binds, mut dests): (Vec<usize>, Vec<Vec<u8>>) = (Vec::new(), Vec::new());
    let show = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    while i < a.len() {
        let op = a[i].clone();
        if op == b"--" {
            break;
        }
        if op == b"--args" {
            if expanded {
                return Err("bwrap --args more than once".into());
            }
            let n = a
                .get(i + 1)
                .filter(|n| !n.is_empty() && n.iter().all(u8::is_ascii_digit))
                .cloned()
                .ok_or("bwrap option --args is cut short")?;
            let fds = fds.ok_or("bwrap --args with no ai-jail to read it from")?;
            let b = read_fd(fds, &n)?;
            a.splice(i..i + 2, words0(&b));
            expanded = true;
            continue;
        }
        let n = arity(&op).ok_or_else(|| format!("unknown bwrap option {}", show(&op)))?;
        if i + n >= a.len() {
            return Err(format!("bwrap option {} is cut short", show(&op)));
        }
        match op.as_slice() {
            b"--unshare-net" | b"--unshare-all" => net = false,
            b"--share-net" => net = true,
            b"--bind" | b"--bind-try" | b"--dev-bind" | b"--dev-bind-try" | b"--ro-bind"
            | b"--ro-bind-try" => {
                if a[i + 1] == a[i + 2] {
                    binds.push(dests.len());
                }
                dests.push(a[i + 2].clone());
            }
            b"--tmpfs" | b"--dev" | b"--proc" | b"--mqueue" | b"--dir" | b"--tmp-overlay"
            | b"--ro-overlay" => dests.push(a[i + 1].clone()),
            b"--symlink" | b"--file" | b"--bind-data" | b"--ro-bind-data" | b"--bind-fd"
            | b"--ro-bind-fd" => dests.push(a[i + 2].clone()),
            b"--overlay" => dests.push(a[i + 3].clone()),
            _ => {}
        }
        i += n + 1;
    }
    if i >= a.len() {
        return Err("no -- in the jail's bwrap command line".into());
    }
    let (mut roots, mut kept) = (Vec::new(), Vec::new());
    for &k in &binds {
        let r = trim1(&dests[k]);
        let (mut masked, mut hidden) = (false, false);
        for d in &dests[k + 1..] {
            // at or above it: the folder is covered; inside it: masked
            hidden |= at_or_under(r, d);
            masked |= below(d, r);
        }
        if hidden {
            continue;
        }
        let Some(rr) = resolve(r) else { continue };
        let mut m = Map::new();
        m.insert("path".into(), text(&rr));
        m.insert("masked".into(), Value::Bool(masked));
        roots.push(Value::Object(m));
        kept.push((r.to_vec(), rr));
    }
    // The paths ai-jail hides inside a kept folder: a mount there that is not one of its folders
    // (--mask/--deny-path's empty file, a tmpfs, ...); a --seed at or under one would copy the host's
    // real file (#198 security).
    let mut masks = Vec::new();
    for (j, d) in dests.iter().enumerate() {
        if binds.contains(&j) {
            continue;
        }
        let inside = kept.iter().find_map(|(l, r)| {
            under(d, l)
                .filter(|rest| !rest.is_empty())
                .map(|rest| (r, rest))
        });
        if let Some((r, rest)) = inside {
            // (a kept folder of `/` gives `/rest`, not `//rest`, which would match nothing)
            let r = r.strip_suffix(b"/").unwrap_or(r);
            let m = cat!(r, "/", rest);
            // (bash's printf | jq -R: one per line, none empty)
            for line in m.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
                masks.push(text(line));
            }
        }
    }
    let mut p = Map::new();
    p.insert("net".into(), Value::Bool(net));
    p.insert("roots".into(), Value::Array(roots));
    p.insert("masks".into(), Value::Array(masks));
    Ok(Value::Object(p))
}

/// mountinfo's escapes in a mount point: `\040` a space, `\011` a tab, `\012` a newline, `\134` a
/// backslash.
pub fn unescape(mp: &[u8]) -> Vec<u8> {
    let mut o = Vec::new();
    let mut i = 0;
    while i < mp.len() {
        let e = match mp.get(i..i + 4) {
            Some(b"\\040") => Some(b' '),
            Some(b"\\011") => Some(b'\t'),
            Some(b"\\012") => Some(b'\n'),
            Some(b"\\134") => Some(b'\\'),
            _ => None,
        };
        match e {
            Some(c) => {
                o.push(c);
                i += 4;
            }
            None => {
                o.push(mp[i]);
                i += 1;
            }
        }
    }
    o
}

/// jail_mounts (#210): the jail's mounts as its init sees them (MOUNTINFO's text) added to POL's
/// masks: a mount point inside one of its folders that is not one of them is hidden there now,
/// wherever the jail renamed it to (`mv config config2` moves the mount, on the host too); one with
/// a newline hides its whole folder. Folders holding one are marked masked.
pub fn with_mounts(pol: &mut Value, mountinfo: &[u8]) {
    let roots: Vec<Vec<u8>> = pol
        .get("roots")
        .and_then(Value::as_array)
        .map(|rs| {
            rs.iter()
                .filter_map(|r| r.get("path").and_then(Value::as_str))
                // (bash read them a line each)
                .flat_map(|s| s.split('\n').map(|l| l.as_bytes().to_vec()))
                .collect()
        })
        .unwrap_or_default();
    let mut out: Vec<Vec<u8>> = Vec::new();
    'line: for line in mountinfo.split(|&b| b == b'\n') {
        let Some(mp) = shell::words(line).into_iter().nth(4) else {
            continue;
        };
        let m = unescape(&mp);
        for r in &roots {
            if !below(&m, r) {
                continue;
            }
            if m.contains(&b'\n') {
                out.push(r.clone());
                continue 'line;
            }
            if roots.contains(&m) {
                continue 'line;
            }
            out.push(m);
            continue 'line;
        }
    }
    let Some(obj) = pol.as_object_mut() else {
        return;
    };
    let mut ms: Vec<String> = obj
        .get("masks")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    ms.extend(out.iter().map(|m| String::from_utf8_lossy(m).into_owned()));
    ms.sort();
    ms.dedup();
    if let Some(Value::Array(rs)) = obj.get_mut("roots") {
        for r in rs.iter_mut() {
            let p = r
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let hit = ms.iter().any(|m| at_or_under(m.as_bytes(), p.as_bytes()));
            if let Some(o) = r.as_object_mut() {
                let was = o.get("masked").and_then(Value::as_bool).unwrap_or(false);
                o.insert("masked".into(), Value::Bool(was || hit));
            }
        }
    }
    obj.insert(
        "masks".into(),
        Value::Array(ms.into_iter().map(Value::String).collect()),
    );
}

/// The words of `bwrap --args N`'s memfd: only ai-jail's options memfd, read from ai-jail's fds.
fn read_args_fd(fds: &[u8], n: &[u8]) -> Result<Vec<u8>, String> {
    let f = shell::path(&cat!(fds, "/", n));
    let n = String::from_utf8_lossy(n);
    let l = std::fs::read_link(&f)
        .map(|l| shell::bytes(&l).to_vec())
        .unwrap_or_default();
    if l != b"/memfd:ai-jail-bwrap-args" && l != b"/memfd:ai-jail-bwrap-args (deleted)" {
        return Err(format!("bwrap --args {n} is not ai-jail's options memfd"));
    }
    std::fs::read(f).map_err(|_| format!("cannot read the jail's bwrap options (--args {n})"))
}

/// `pgrep -P PID -x NAME | head -1`.
fn child_named(pid: &[u8], name: &[u8]) -> Option<Vec<u8>> {
    procfs::children(pid)
        .into_iter()
        .find(|c| procfs::read(c, "comm").is_some_and(|n| n == name))
}

/// What find_jail reads of a process (PID, or `self` for this one), so the unit tests can hand it an
/// ancestry of their own: its parent, its name, its exe's file name, and a namespace link of it.
pub trait ProcView {
    fn ppid(&self, pid: &[u8]) -> Option<Vec<u8>>;
    fn comm(&self, pid: &[u8]) -> Option<Vec<u8>>;
    fn exe_name(&self, pid: &[u8]) -> Option<Vec<u8>>;
    fn ns(&self, pid: &[u8], kind: &str) -> Option<Vec<u8>>;
}

/// The host's /proc, as the broker reads it.
pub struct HostProc;

impl ProcView for HostProc {
    fn ppid(&self, pid: &[u8]) -> Option<Vec<u8>> {
        procfs::proc_ppid(pid).filter(|pp| !pp.is_empty())
    }
    fn comm(&self, pid: &[u8]) -> Option<Vec<u8>> {
        procfs::read(pid, "comm")
    }
    fn exe_name(&self, pid: &[u8]) -> Option<Vec<u8>> {
        let e = std::fs::read_link(shell::path(&cat!("/proc/", pid, "/exe"))).ok()?;
        e.file_name()
            .map(|n| shell::bytes(std::path::Path::new(n)).to_vec())
    }
    fn ns(&self, pid: &[u8], kind: &str) -> Option<Vec<u8>> {
        let l = std::fs::read_link(shell::path(&cat!("/proc/", pid, "/ns/", kind))).ok()?;
        Some(shell::bytes(&l).to_vec())
    }
}

/// The caller's jail: walking up from PEER, the first `bwrap` whose parent is ai-jail itself, as
/// (that bwrap, its parent). ai-jail itself: an exe named `ai-jail` in this broker's own user and pid
/// namespaces. A process inside a jail never is, so a jailed agent that can make a user namespace
/// (ai-jail with seccomp off) and runs a binary it named ai-jail with a bwrap of its own under it
/// does not pass that bwrap off as its jail: the walk goes on up to the real one (finding 751; it
/// took the first parent so named).
pub fn find_jail(peer: &[u8], pv: &dyn ProcView) -> Option<(Vec<u8>, Vec<u8>)> {
    let here = |pid: &[u8]| {
        ["user", "pid"].iter().all(|k| {
            let mine = pv.ns(b"self", k);
            mine.is_some() && pv.ns(pid, k) == mine
        })
    };
    let mut p = peer.to_vec();
    while shell::dec(&p).is_some_and(|n| n > 1) {
        let pp = pv.ppid(&p)?;
        if pv.comm(&p).is_some_and(|c| c == b"bwrap")
            && pv.exe_name(&pp).as_deref() == Some(b"ai-jail")
            && here(&pp)
        {
            return Some((p, pp));
        }
        p = pp;
    }
    None
}

/// Whether the jail (ROOT, an O_PATH fd of its init's root) has the host's folder RR (P resolved on
/// the host) at P, the path its command line mounts it at: the same file, by device and inode, P
/// walked inside the jail (its links followed as the jail sees
/// them, never out of it). A jail's command line is its word only: a folder it names that the jail
/// does not have is not one of its folders (finding 751). A kernel without openat2: true (find_jail's
/// namespace check stands alone then).
pub fn jail_has(root: &std::fs::File, p: &[u8], rr: &[u8]) -> bool {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::fs::MetadataExt;
    let Ok(host) = std::fs::metadata(shell::path(rr)) else {
        return false;
    };
    let Ok(c) = std::ffi::CString::new(if p.is_empty() {
        b"/".to_vec()
    } else {
        p.to_vec()
    }) else {
        return false;
    };
    // SAFETY: open_how is plain data; zeroed, then the fields set.
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.flags = (libc::O_PATH | libc::O_CLOEXEC) as u64;
    how.resolve = libc::RESOLVE_IN_ROOT | libc::RESOLVE_NO_MAGICLINKS;
    // SAFETY: openat2 reads the path and how (its size given) and returns a new fd of ours, or -1.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            root.as_raw_fd(),
            c.as_ptr(),
            &how,
            std::mem::size_of::<libc::open_how>(),
        )
    };
    if fd < 0 {
        let e = std::io::Error::last_os_error().raw_os_error();
        return matches!(e, Some(libc::ENOSYS | libc::EPERM));
    }
    // SAFETY: just opened, ours alone.
    let f = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd as i32) });
    f.metadata()
        .is_ok_and(|m| m.dev() == host.dev() && m.ino() == host.ino())
}

/// The caller this broker's omabox runs for: its pid, in this process's pid namespace, and its start
/// in clock ticks. The relay's `listen` sets OMABOX_BROKER_PEER from the socket's SO_PEERCRED (the
/// kernel's, not the caller's word: what the caller sends is renamed OMABOX_RELAY_*), and init makes
/// sure of it by its pidfd. An up is that caller's, made before this process (#404, finding 579).
static CALLER: std::sync::OnceLock<(Vec<u8>, Vec<u8>)> = std::sync::OnceLock::new();

/// The jailed caller (pid, start), once init has made sure of it.
pub fn caller() -> Option<&'static (Vec<u8>, Vec<u8>)> {
    CALLER.get()
}

/// broker_init: the caller (OMABOX_BROKER_PEER, made sure of by its pidfd OMABOX_BROKER_PIDFD) is
/// ai-jail's, a process in the pid namespace of a bwrap whose parent is ai-jail itself; that bwrap's
/// command line is the jail's whole policy, which nothing in the jail can change. Sets OMABOX_JAIL
/// (the policy, its mounts as the jail's init sees them, its id and the caller's working directory
/// when the jail sees it at the same path) in this process's environment. Runs before any thread.
pub fn init(ctx: &Ctx) -> Res<()> {
    let peer = shell::env_or_empty("OMABOX_BROKER_PEER");
    let pfd = shell::env_or_empty("OMABOX_BROKER_PIDFD");
    let cwd_given = shell::env("OMABOX_BROKER_CWD").filter(|c| !c.is_empty());
    // SAFETY: the front runs before any other thread is started.
    unsafe {
        std::env::remove_var("OMABOX_BROKER_PEER");
        std::env::remove_var("OMABOX_BROKER_PIDFD");
        std::env::remove_var("OMABOX_BROKER_CWD");
    }
    let num = |v: &[u8]| !v.is_empty() && v.iter().all(u8::is_ascii_digit);
    if !num(&peer) || !num(&pfd) {
        return Err(ctx.die("broker: no caller"));
    }
    let there = || {
        let info = std::fs::read(shell::path(&cat!("/proc/self/fdinfo/", pfd))).unwrap_or_default();
        procfs::status_last(&info, "Pid") == peer
    };
    if !there() {
        return Err(ctx.die("broker: the caller is gone"));
    }
    // (its start read between two looks through its pidfd: the start of that very process)
    if let Some(start) = procfs::proc_start(&peer).filter(|s| !s.is_empty())
        && there()
    {
        let _ = CALLER.set((peer.clone(), start));
    }
    let found = find_jail(&peer, &HostProc);
    // Not in a jail: a caller's omabox hands its command here only when it cannot make a user
    // namespace (relay_wanted, #114): said so, or the refusal reads like the broker's fault.
    let Some((jail, parent)) = found else {
        return Err(ctx.die("the omabox broker serves agents inside ai-jail; this caller is not in one. Its omabox came here because it cannot make a user namespace: unprivileged user namespaces look disabled on this machine (sysctl kernel.unprivileged_userns_clone, kernel.apparmor_restrict_unprivileged_userns); omabox needs them (./install.sh --check starts a box to show it works)"));
    };
    let ns = |pid: &[u8]| std::fs::read_link(shell::path(&cat!("/proc/", pid, "/ns/pid"))).ok();
    if ns(&peer) == ns(&jail) {
        return Err(ctx.die("broker: the caller is not inside its jail"));
    }
    let Some(start) = procfs::proc_start(&jail).filter(|s| !s.is_empty()) else {
        return Err(ctx.die("broker: the jail is gone"));
    };
    // The jail's own PID 1: bwrap's init in the jail. Its mounts are the jail's as the jail sees them
    // (not the caller's: its view of them is the caller's to narrow, #210 review), and its root is
    // where each folder the jail's command line names is confirmed (finding 751).
    let init = child_named(&jail, b"bwrap");
    let mntns = init
        .as_ref()
        .and_then(|i| std::fs::read_link(shell::path(&cat!("/proc/", i, "/ns/mnt"))).ok());
    let (Some(init), Some(mntns)) = (init, mntns) else {
        return Err(ctx.die("broker: cannot read the jail's mounts (no bwrap init in it)"));
    };
    let Ok(mi) = std::fs::read(shell::path(&cat!("/proc/", init, "/mountinfo"))) else {
        return Err(ctx.die("broker: cannot read the jail's mounts"));
    };
    let jroot = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(shell::path(&cat!("/proc/", init, "/root")))
        .map_err(|_| ctx.die("broker: cannot read the jail's root"))?;
    let cmdline = std::fs::read(shell::path(&cat!("/proc/", jail, "/cmdline"))).unwrap_or_default();
    let fds = cat!("/proc/", parent, "/fd");
    // (each folder resolved on the host, and kept only when the jail has that same folder there)
    let resolve = |r: &[u8]| {
        let rr = crate::repo::readlink_e(r)?;
        jail_has(&jroot, r, &rr).then_some(rr)
    };
    let mut pol = policy_of(&cmdline, Some(&fds), &read_args_fd, &resolve)
        .map_err(|why| ctx.die(cat!("broker: cannot read the jail's policy: ", why)))?;
    with_mounts(&mut pol, &mi);
    let id = cat!(jail, " ", start);
    let Some(obj) = pol.as_object_mut() else {
        return Err(ctx.die("broker: cannot read the jail's policy: not an object"));
    };
    obj.insert(
        "mntpid".into(),
        Value::Number(shell::dec(&init).unwrap_or(0).into()),
    );
    obj.insert("mntns".into(), text(shell::bytes(&mntns)));
    if !there() {
        return Err(ctx.die("broker: the caller is gone"));
    }
    if let Some(fd) = shell::dec(&pfd).and_then(|f| i32::try_from(f).ok()) {
        // SAFETY: the pidfd the relay placed for this process; nothing else holds this number.
        unsafe {
            libc::close(fd);
        }
    }
    obj.insert("id".into(), text(&id));
    set_jail(&pol);
    // The caller's working directory, when the jail sees it at the same path here (box_exec starts
    // `run` there); the broker itself stays in /.
    let cwd = crate::jailpath::jail_sees(cwd_given.as_deref().unwrap_or(b"/nonexistent"))
        .unwrap_or_default();
    if let Some(o) = pol.as_object_mut() {
        o.insert("cwd".into(), text(&cwd));
    }
    set_jail(&pol);
    Ok(())
}

fn set_jail(pol: &Value) {
    // SAFETY: the front runs before any other thread is started.
    unsafe {
        std::env::set_var("OMABOX_JAIL", json::compact(pol));
    }
}

/// broker_check's answer: why the command is refused, or None when a jailed agent may ask it. What a
/// jailed agent may ask of the broker: not the user's desktop (host, peek, interactive boxes), nor
/// omabox's own settings, the guard, the broker, the user's saves, or omabox's own `_` commands; the
/// boxes' own commands, and `up`'s mounts and network, are checked where they are used.
pub fn refusal(cmd: &[u8], args: &[Vec<u8>]) -> Option<Vec<u8>> {
    match cmd {
        b"up" | b"run" | b"shot" | b"screenshot" | b"pixel" | b"find" | b"windows" | b"window"
        | b"tree" | b"tray" | b"keys" | b"click" | b"scroll" | b"pointer" | b"drag" | b"wait"
        | b"hyprctl" | b"lua" | b"log" | b"logs" | b"gdb" | b"pid" | b"events"
        | b"restart-shell" | b"reload" | b"mode" | b"output" | b"monitor" | b"gpu" | b"cpu"
        | b"frames" | b"audio" | b"polkit" | b"env" | b"path" | b"down" | b"ls" | b"list"
        | b"brief" | b"jobs" | b"stop" | b"dnd" | b"dock" => None,
        // scene (#489): its file read on stdin only (scene::load), its windows the jail's box's.
        b"scene" => None,
        // snapshot, diff (#446): the jail's own box HOME, recorded in its own folder.
        b"snapshot" | b"diff" => None,
        // record (#224): its files only through fds the caller's omabox opened (record checks
        // them). ports (#93): the jail's own boxes, the host side named by state only.
        b"record" | b"ports" => None,
        b"help" | b"-h" | b"--help" | b"version" | b"-V" | b"--version" => None,
        // Reading is not changing (#114): bare, --json, or one known KEY; never KEY VALUE nor KEY
        // default. Not protect-ports: which host ports hold the user's services is not the jail's
        // to know (#374), nor in its listing (config).
        b"config" => {
            let read = match args {
                [] => true,
                [a] => a == b"--json" || crate::conf::default(a).is_some(),
                _ => false,
            };
            if args.first().is_some_and(|a| a == b"protect-ports") {
                return Some(b"config: protect-ports is not for a jailed agent (which host ports are protected is the user's to know)".to_vec());
            }
            (!read).then(|| b"config: a jailed agent does not change omabox's settings".to_vec())
        }
        // Never on the allowed list: it hands the user's clipboard over (finding 119).
        b"clip" => Some(
            b"omabox clip is not for agents: the user hands their clipboard over themselves"
                .to_vec(),
        ),
        _ => Some(cat!("omabox ", cmd, " is not for an agent inside ai-jail")),
    }
}

/// broker_init then broker_check (`refusal`), as bash's main ran them: as failures (1, run's 125),
/// not usage errors. A refusal gets its line in the jail's own box's command log (cmdlog::refused).
pub fn serve(ctx: &mut Ctx, cmd: &[u8], args: &[Vec<u8>]) -> Res<()> {
    let was = ctx.parsing;
    ctx.parsing = false;
    let r = init(ctx).and_then(|()| check(ctx, cmd, args));
    ctx.parsing = was;
    r
}

/// broker_check: `refusal`'s answer as a failure, its line in the jail's own box's command log
/// (#447: main never begins a refused command).
pub fn check(ctx: &Ctx, cmd: &[u8], args: &[Vec<u8>]) -> Res<()> {
    match refusal(cmd, args) {
        Some(m) => {
            let e = ctx.die(&m);
            crate::cmdlog::refused(&ctx.boxes, cmd, args, e.0, &m);
            Err(e)
        }
        None => Ok(()),
    }
}

/// `_rust-jail jail_policy [FDS]` (the command line on stdin) and `_rust-jail jail_mounts POLICY
/// MOUNTINFO`: the suite's check that these read a jail as bash's functions of those names do, on
/// its fixtures (t_unit_jail_policy, t_unit_jail_covers). Their output and status.
pub fn probe(args: &[Vec<u8>]) -> u8 {
    let say = |t: &str| crate::exit::out(format!("{t}\n")).map_or(1, |()| 0);
    match args {
        [f, rest @ ..] if f == b"jail_policy" && rest.len() <= 1 => {
            let mut cmdline = Vec::new();
            let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut cmdline);
            let fds = rest.first().map(Vec::as_slice).filter(|f| !f.is_empty());
            match policy_of(&cmdline, fds, &read_args_fd, &crate::repo::readlink_e) {
                Ok(p) => say(&json::compact(&p)),
                Err(e) => {
                    say(&e);
                    1
                }
            }
        }
        [f, pol, mi] if f == b"jail_mounts" => {
            let Ok(text) = std::fs::read(shell::path(mi)) else {
                return 1;
            };
            let Some(mut p) = json::values(pol).0.into_iter().next() else {
                return 1;
            };
            with_mounts(&mut p, &text);
            say(&json::compact(&p))
        }
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// broker_check (the cases of t_unit_jail_policy, t_unit_clip, t_unit_pixel, t_unit_cpu,
    /// t_unit_pointer, t_unit_inspect, t_config_kept, t_gdb and t_output, #324 wave 6-suite): what a
    /// jailed agent may ask, and the words of a refusal.
    #[test]
    fn broker_check_allows_and_refuses() {
        let r = |w: &str| {
            let w: Vec<Vec<u8>> = w.split(' ').map(|x| x.as_bytes().to_vec()).collect();
            refusal(&w[0], &w[1..]).map(|m| String::from_utf8(m).unwrap())
        };
        for c in [
            "host",
            "peek",
            "keys-to-box",
            "guard",
            "broker",
            "_reap",
            "config workspace 3",
            "save s",
            "saves",
            "saves rm s",
        ] {
            let m = r(c).unwrap_or_else(|| panic!("{c} allowed"));
            assert!(
                m.contains("not for an agent inside ai-jail") || m.contains("does not change"),
                "{c}: {m}"
            );
        }
        for c in [
            "shot",
            "keys",
            "up",
            "down",
            "config --json",
            "config workspace",
            "ports --json",
            "pixel",
            "find",
            "tree",
            "reload",
            "restart-shell",
            "gdb",
            "output",
            "cpu",
            "gpu",
            "scroll",
            "lua",
            "hyprctl",
            "log",
            "events",
            "jobs",
            "stop 5",
        ] {
            assert_eq!(r(c), None, "{c}");
        }
        assert!(r("clip").is_some_and(|m| m.contains("clip is not for agents")));
        // (#345: a mount made into a box that is up, which the jail's covers are not checked
        // against; a jailed agent's plugins go in with up --plugin)
        assert!(r("plugin add /x").is_some_and(|m| m.contains("not for an agent inside ai-jail")));
        for c in [
            "config workspace 5",
            "config workspace default",
            "config nokey",
            "config --json workspace",
        ] {
            assert!(r(c).is_some_and(|m| m.contains("does not change")), "{c}");
        }
    }

    /// A stand-in /proc: each pid's (parent, comm, exe name, user ns, pid ns); `self` is the broker.
    struct Fake(
        Vec<(
            &'static str,
            &'static str,
            &'static str,
            &'static str,
            &'static str,
            &'static str,
        )>,
    );

    impl Fake {
        fn row(&self, pid: &[u8]) -> Option<&(&str, &str, &str, &str, &str, &str)> {
            self.0.iter().find(|r| r.0.as_bytes() == pid)
        }
    }

    impl ProcView for Fake {
        fn ppid(&self, pid: &[u8]) -> Option<Vec<u8>> {
            self.row(pid).map(|r| r.1.as_bytes().to_vec())
        }
        fn comm(&self, pid: &[u8]) -> Option<Vec<u8>> {
            self.row(pid).map(|r| r.2.as_bytes().to_vec())
        }
        fn exe_name(&self, pid: &[u8]) -> Option<Vec<u8>> {
            self.row(pid).map(|r| r.3.as_bytes().to_vec())
        }
        fn ns(&self, pid: &[u8], kind: &str) -> Option<Vec<u8>> {
            let r = self.row(pid)?;
            Some(if kind == "user" { r.4 } else { r.5 }.as_bytes().to_vec())
        }
    }

    /// find_jail (finding 751): ai-jail is a parent so named in the broker's own user and pid
    /// namespaces; one inside a jail (another user or pid namespace) is passed over, the walk going
    /// on to the real jail above it, or to none.
    #[test]
    fn find_jail_wants_ai_jail_in_the_brokers_namespaces() {
        let host = ("self", "1", "omabox", "omabox", "u0", "p0");
        let real = [
            host,
            ("60", "1", "ai-jail", "ai-jail", "u0", "p0"),
            ("70", "60", "bwrap", "bwrap", "u0", "p0"),
        ];
        // the caller under the real jail
        let mut v = real.to_vec();
        v.push(("100", "70", "claude", "node", "u1", "p1"));
        assert_eq!(
            find_jail(b"100", &Fake(v)),
            Some((b"70".to_vec(), b"60".to_vec()))
        );
        // a fake ai-jail inside it, in the jail's user namespace, with a bwrap of its own: the real one
        let mut v = real.to_vec();
        v.extend([
            ("80", "70", "ai-jail", "ai-jail", "u1", "p1"),
            ("90", "80", "bwrap", "bwrap", "u1", "p1"),
            ("100", "90", "sh", "sh", "u2", "p2"),
        ]);
        assert_eq!(
            find_jail(b"100", &Fake(v)),
            Some((b"70".to_vec(), b"60".to_vec()))
        );
        // only a fake one: another user namespace, or the same user namespace and another pid one
        for (u, pn) in [("u1", "p0"), ("u0", "p1")] {
            let v = vec![
                host,
                ("80", "1", "ai-jail", "ai-jail", u, pn),
                ("90", "80", "bwrap", "bwrap", u, pn),
                ("100", "90", "sh", "sh", "u2", "p2"),
            ];
            assert_eq!(find_jail(b"100", &Fake(v)), None, "{u} {pn}");
        }
        // no namespace to read: not ai-jail
        let v = vec![
            ("80", "1", "ai-jail", "ai-jail", "u0", "p0"),
            ("90", "80", "bwrap", "bwrap", "u0", "p0"),
            ("100", "90", "sh", "sh", "u2", "p2"),
        ];
        assert_eq!(find_jail(b"100", &Fake(v)), None);
    }

    /// jail_has (finding 751): a folder the jail's command line names counts only when the jail has
    /// that same folder (device and inode) at its path, walked inside the jail: not there, another
    /// folder there, or a link there to the host's folder (followed inside the jail) is not.
    #[test]
    fn jail_has_wants_the_same_folder_inside_the_jail() {
        let d = std::env::temp_dir().join(format!("omabox-jailhas-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let host = d.join("host/proj");
        let jail = d.join("jail");
        std::fs::create_dir_all(&host).unwrap();
        std::fs::create_dir_all(jail.join("other")).unwrap();
        std::os::unix::fs::symlink(&host, jail.join("esc")).unwrap();
        let open = |p: &std::path::Path| {
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC)
                .open(p)
                .unwrap()
        };
        let h = shell::bytes(&host).to_vec();
        assert!(jail_has(&open(std::path::Path::new("/")), &h, &h));
        let j = open(&jail);
        assert!(!jail_has(&j, &h, &h), "not in the jail");
        assert!(!jail_has(&j, b"/other", &h), "another folder there");
        assert!(!jail_has(&j, b"/esc", &h), "a link out of the jail");
        assert!(
            !jail_has(&j, b"/other", b"/nonexistent-omabox"),
            "gone on the host"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    fn line(w: &[&str]) -> Vec<u8> {
        let mut v = Vec::new();
        for x in w {
            v.extend_from_slice(x.as_bytes());
            v.push(0);
        }
        v
    }

    fn same(p: &[u8]) -> Option<Vec<u8>> {
        (!p.starts_with(b"/gone")).then(|| {
            if p.is_empty() {
                b"/".to_vec()
            } else {
                p.to_vec()
            }
        })
    }

    fn no_fd(_: &[u8], n: &[u8]) -> Result<Vec<u8>, String> {
        Err(format!(
            "bwrap --args {} is not ai-jail's options memfd",
            String::from_utf8_lossy(n)
        ))
    }

    fn pol(w: &[&str]) -> Result<String, String> {
        policy_of(&line(w), Some(b"/fds"), &no_fd, &same).map(|v| json::compact(&v))
    }

    #[test]
    fn policies() {
        assert_eq!(
            pol(&[
                "bwrap",
                "--unshare-net",
                "--ro-bind",
                "/a",
                "/a",
                "--tmpfs",
                "/a/x",
                "--",
                "sh"
            ]),
            Ok(r#"{"net":false,"roots":[{"path":"/a","masked":true}],"masks":["/a/x"]}"#.into())
        );
        // a later mount at or above a folder covers it; a source elsewhere is no folder of the jail's
        assert_eq!(
            pol(&[
                "bwrap", "--bind", "/a/b/", "/a/b", "--bind", "/x", "/y", "--tmpfs", "/a", "--",
                "sh"
            ]),
            Ok(r#"{"net":true,"roots":[],"masks":[]}"#.into())
        );
        assert_eq!(
            pol(&[
                "bwrap",
                "--bind",
                "/gone",
                "/gone",
                "--share-net",
                "--",
                "sh"
            ]),
            Ok(r#"{"net":true,"roots":[],"masks":[]}"#.into())
        );
        assert_eq!(
            pol(&["bwrap", "--nope", "--", "sh"]),
            Err("unknown bwrap option --nope".into())
        );
        assert_eq!(
            pol(&["bwrap", "--bind", "/a"]),
            Err("bwrap option --bind is cut short".into())
        );
        assert_eq!(
            pol(&["bwrap", "--unshare-all"]),
            Err("no -- in the jail's bwrap command line".into())
        );
        assert_eq!(
            pol(&["bwrap", "--args"]),
            Err("bwrap option --args is cut short".into())
        );
        assert!(
            pol(&["bwrap", "--args", "3", "--"])
                .is_err_and(|e| e.contains("not ai-jail's options memfd"))
        );
        assert_eq!(
            policy_of(&line(&["bwrap", "--args", "3", "--"]), None, &no_fd, &same),
            Err("bwrap --args with no ai-jail to read it from".into())
        );
        let fd = |_: &[u8], _: &[u8]| -> Result<Vec<u8>, String> {
            Ok(line(&["--ro-bind", "/h", "/h", "--args", "4"]))
        };
        assert_eq!(
            policy_of(
                &line(&["bwrap", "--args", "3", "--", "sh"]),
                Some(b"/f"),
                &fd,
                &same
            ),
            Err("bwrap --args more than once".into())
        );
        let fd1 = |_: &[u8], _: &[u8]| -> Result<Vec<u8>, String> {
            Ok(line(&["--ro-bind", "/h", "/h"]))
        };
        assert_eq!(
            policy_of(
                &line(&["bwrap", "--args", "3", "--", "sh"]),
                Some(b"/f"),
                &fd1,
                &same
            )
            .map(|v| json::compact(&v)),
            Ok(r#"{"net":true,"roots":[{"path":"/h","masked":false}],"masks":[]}"#.into())
        );
    }

    #[test]
    fn mounts() {
        let mut p: Value = serde_json::from_str(
            r#"{"net":true,"roots":[{"path":"/h","masked":false},{"path":"/h/in","masked":false},{"path":"/o","masked":false}],"masks":["/z"]}"#,
        )
        .unwrap_or_default();
        let mi = b"1 2 0:1 / /h rw - x x\n3 4 0:1 / /h/a\\040b rw - x x\n5 6 0:1 / /h/in rw - x x\n7 8 0:1 / /o/n\\012l rw - x x\n";
        with_mounts(&mut p, mi);
        assert_eq!(
            json::compact(&p),
            r#"{"net":true,"roots":[{"path":"/h","masked":true},{"path":"/h/in","masked":false},{"path":"/o","masked":true}],"masks":["/h/a b","/o","/z"]}"#
        );
    }

    #[test]
    fn checks() {
        let c = Ctx::new(b"x", b"/b".to_vec());
        let w = |s: &[&str]| s.iter().map(|a| a.as_bytes().to_vec()).collect::<Vec<_>>();
        assert!(check(&c, b"shot", &[]).is_ok());
        assert!(check(&c, b"config", &w(&["--json"])).is_ok());
        assert!(check(&c, b"config", &w(&["gpu"])).is_ok());
        assert!(check(&c, b"config", &w(&["gpu", "auto"])).is_err());
        assert!(check(&c, b"config", &w(&["protect-ports"])).is_err());
        for x in [
            "_reap",
            "_rust",
            "_rust-jq",
            "_rust-opts",
            "_ptr-release",
            "_save-watch",
            "clip",
            "host",
            "peek",
            "guard",
            "broker",
            "setup",
            "save",
            "saves",
            "which",
            "calibrate",
        ] {
            assert!(check(&c, x.as_bytes(), &[]).is_err(), "{x}");
        }
    }
}
