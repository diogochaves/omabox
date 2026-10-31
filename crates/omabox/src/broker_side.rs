//! The broker's side of a jailed agent's call (#324 wave 6-front, A3; issue #16, finding 99):
//! omabox-relay's `listen` ran this omabox for a caller inside ai-jail. broker_init finds the
//! caller's jail and reads its policy from the jail's own bwrap (OMABOX_JAIL, which everything that
//! matters reads and every omabox this one starts inherits); broker_check says what a jailed agent
//! may ask, before any command runs (internal `_` ones too).

use crate::cat;
use crate::exit::{Ctx, Res};
use crate::json;
use crate::procfs;
use crate::shell;
use serde_json::{Map, Value};

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

fn starts_dir(p: &[u8], r: &[u8]) -> bool {
    p.starts_with(&cat!(r, "/"))
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
            let d = trim1(d);
            // at or above it: the folder is covered; inside it: masked
            hidden |= cat!(r, "/").starts_with(&cat!(d, "/"));
            masked |= starts_dir(d, r);
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
        let d = trim1(d);
        if let Some((l, r)) = kept.iter().find(|(l, _)| starts_dir(d, l)) {
            let m = cat!(r, &d[l.len()..]);
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
            if !starts_dir(&m, r) {
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
            let hit = ms
                .iter()
                .any(|m| *m == p || m.starts_with(&format!("{p}/")));
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
    let (mut p, mut jail, mut parent) = (peer.clone(), None, Vec::new());
    while shell::dec(&p).is_some_and(|n| n > 1) {
        let Some(pp) = procfs::proc_ppid(&p).filter(|pp| !pp.is_empty()) else {
            break;
        };
        let exe = std::fs::read_link(shell::path(&cat!("/proc/", pp, "/exe")))
            .ok()
            .and_then(|e| {
                e.file_name()
                    .map(|n| shell::bytes(std::path::Path::new(n)).to_vec())
            });
        if procfs::read(&p, "comm").is_some_and(|c| c == b"bwrap")
            && exe.as_deref() == Some(b"ai-jail")
        {
            jail = Some(p.clone());
            parent = pp;
            break;
        }
        p = pp;
    }
    // Not in a jail: a caller's omabox hands its command here only when it cannot make a user
    // namespace (relay_wanted, #114): said so, or the refusal reads like the broker's fault.
    let Some(jail) = jail else {
        return Err(ctx.die("the omabox broker serves agents inside ai-jail; this caller is not in one. Its omabox came here because it cannot make a user namespace: unprivileged user namespaces look disabled on this machine (sysctl kernel.unprivileged_userns_clone, kernel.apparmor_restrict_unprivileged_userns); omabox needs them (./install.sh --check starts a box to show it works)"));
    };
    let ns = |pid: &[u8]| std::fs::read_link(shell::path(&cat!("/proc/", pid, "/ns/pid"))).ok();
    if ns(&peer) == ns(&jail) {
        return Err(ctx.die("broker: the caller is not inside its jail"));
    }
    let Some(start) = procfs::proc_start(&jail).filter(|s| !s.is_empty()) else {
        return Err(ctx.die("broker: the jail is gone"));
    };
    let cmdline = std::fs::read(shell::path(&cat!("/proc/", jail, "/cmdline"))).unwrap_or_default();
    let fds = cat!("/proc/", parent, "/fd");
    let mut pol = policy_of(
        &cmdline,
        Some(&fds),
        &read_args_fd,
        &crate::repo::readlink_e,
    )
    .map_err(|why| ctx.die(cat!("broker: cannot read the jail's policy: ", why)))?;
    // The jail's mounts as its own PID 1 sees them: bwrap's init in the jail. Not the caller's: its
    // view of them is the caller's to narrow (#210 review). jail_verify reads them again through it.
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

/// broker_check: what a jailed agent may ask of the broker. Not the user's desktop (host, peek,
/// interactive boxes), nor omabox's own settings, the guard, the broker, the user's saves, or
/// omabox's own `_` commands; the boxes' own commands, and `up`'s mounts and network, are checked
/// where they are used.
pub fn check(ctx: &Ctx, cmd: &[u8], args: &[Vec<u8>]) -> Res<()> {
    match refusal(cmd, args) {
        Some(m) => Err(ctx.die(m)),
        None => Ok(()),
    }
}

/// broker_check's answer: why the command is refused, or None when a jailed agent may ask it.
pub fn refusal(cmd: &[u8], args: &[Vec<u8>]) -> Option<Vec<u8>> {
    match cmd {
        b"up" | b"run" | b"shot" | b"screenshot" | b"pixel" | b"windows" | b"tree" | b"keys"
        | b"click" | b"scroll" | b"pointer" | b"drag" | b"wait" | b"hyprctl" | b"lua" | b"log"
        | b"logs" | b"gdb" | b"events" | b"restart-shell" | b"reload" | b"mode" | b"output"
        | b"monitor" | b"gpu" | b"cpu" | b"audio" | b"env" | b"path" | b"down" | b"ls"
        | b"list" | b"brief" => None,
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

/// broker_init then broker_check, as bash's main ran them: as failures (1, run's 125), not usage
/// errors.
pub fn serve(ctx: &mut Ctx, cmd: &[u8], args: &[Vec<u8>]) -> Res<()> {
    let was = ctx.parsing;
    ctx.parsing = false;
    let r = init(ctx).and_then(|()| check(ctx, cmd, args));
    ctx.parsing = was;
    r
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
        ] {
            assert_eq!(r(c), None, "{c}");
        }
        assert!(r("clip").is_some_and(|m| m.contains("clip is not for agents")));
        for c in [
            "config workspace 5",
            "config workspace default",
            "config nokey",
            "config --json workspace",
        ] {
            assert!(r(c).is_some_and(|m| m.contains("does not change")), "{c}");
        }
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
