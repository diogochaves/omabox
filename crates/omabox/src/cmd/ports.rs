//! `omabox ports [-b NAME] [--json]` (#88, findings 156, 165): each box's TCP servers, and what holds
//! their port on this side's 127.0.0.1. A connected box's pasta takes a box server's port here about
//! a second after it listens, but a port has one holder: of two boxes serving on one, one gets it
//! and the other's pasta says nothing. pasta is non-dumpable, so which pasta holds a port cannot be
//! read: a port held by a socket of ours that no readable process has is put down to the boxes
//! serving on it, all of them named when there are several. Nothing connects to a port to tell them
//! apart: it could be one of the user's own services. A `--net host` box (#313) has no pasta: its
//! servers are on the host's own ports, and said so (`host-net`).

use crate::args::{optarg, unexpected, unknown_opt};
use crate::boxes::{BoxPid, box_pid, box_state, list_names, meta, need_box};
use crate::cat;
use crate::exit::{Ctx, Res, out};
use crate::jail::in_jail;
use crate::jobs::box_pids;
use crate::jq::{self, Args};
use crate::mon::{pad, records};
use crate::select::select_box;
use crate::shell::{self, strip_nl};
use std::collections::HashMap;
use std::os::unix::ffi::OsStrExt;

/// tcp_listeners DIR: the listening TCP sockets in a /proc net dir, "PORT ADDR UID INODE" each
/// (awk's, as bash runs it).
fn tcp_listeners(dir: &[u8]) -> Vec<[Vec<u8>; 4]> {
    let r = jq::tool(
        "awk",
        &[
            crate::jqprog::TCP_LISTENERS_AWK.as_bytes().to_vec(),
            cat!(dir, "/tcp"),
            cat!(dir, "/tcp6"),
        ],
        None,
        jq::Err::Null,
    );
    records(&r.out)
        .into_iter()
        .map(|l| {
            let f = shell::read_fields(l, 4);
            [f[0].clone(), f[1].clone(), f[2].clone(), f[3].clone()]
        })
        .collect()
}

/// sock_owners PID...: each socket inode the processes hold, with the last of them (in that order)
/// holding it (fds this user cannot read are skipped), into OWN.
fn sock_owners(pids: &[Vec<u8>], own: &mut HashMap<Vec<u8>, Vec<u8>>) {
    for p in pids {
        let dir = cat!("/proc/", p, "/fd");
        let Ok(rd) = std::fs::read_dir(shell::path(&dir)) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(t) = std::fs::read_link(e.path()) else {
                continue;
            };
            let t = t.as_os_str().as_bytes();
            let ino = t
                .strip_prefix(b"socket:[")
                .and_then(|r| r.strip_suffix(b"]"))
                .filter(|i| !i.is_empty() && i.iter().all(u8::is_ascii_digit));
            if let Some(ino) = ino {
                if p.iter().all(u8::is_ascii_digit) {
                    own.insert(ino.to_vec(), p.clone());
                }
            }
        }
    }
}

/// `$(cat "/proc/PID/comm" 2>/dev/null)`.
fn comm(p: &[u8]) -> Option<Vec<u8>> {
    std::fs::read(shell::path(&cat!("/proc/", p, "/comm")))
        .ok()
        .map(|c| strip_nl(&c))
}

/// Unquoted `${V//,/ }` in a for list: its words.
fn comma_words(v: &[u8]) -> Vec<Vec<u8>> {
    let v: Vec<u8> = v
        .iter()
        .map(|&c| if c == b',' { b' ' } else { c })
        .collect();
    shell::words(&v)
}

/// local_only ADDRS: every address a server listens on is ::1 or a 127.x but 127.0.0.1, which is not
/// reachable from outside its box (NOTES 89), though its pasta still takes its port (finding 165).
fn local_only(addrs: &[u8]) -> bool {
    comma_words(addrs)
        .iter()
        .all(|a| a == b"::1" || (a.starts_with(b"127.") && a != b"127.0.0.1"))
}

/// A box up, its PID 1: `[ "$(box_state)" = up ] && pid=$(box_pid) && [ -n "$pid" ]`.
fn up_pid(d: &[u8]) -> Option<Vec<u8>> {
    if box_state(d) != "up" {
        return None;
    }
    match box_pid(d) {
        BoxPid::Pid(p) if !p.is_empty() => Some(p),
        _ => None,
    }
}

/// What ports reads of the system: each box's PID 1 and processes, a net dir's listeners, the sockets
/// processes hold.
trait PortsSys {
    fn up_pid(&self, d: &[u8]) -> Option<Vec<u8>> {
        up_pid(d)
    }
    fn box_pids(&self, ctx: &Ctx) -> Vec<Vec<u8>> {
        box_pids(ctx)
    }
    fn tcp_listeners(&self, dir: &[u8]) -> Vec<[Vec<u8>; 4]> {
        tcp_listeners(dir)
    }
    fn sock_owners(&self, pids: &[Vec<u8>], own: &mut HashMap<Vec<u8>, Vec<u8>>) {
        sock_owners(pids, own);
    }
}

/// The system itself.
struct RealPorts;

impl PortsSys for RealPorts {}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    run_with(&RealPorts, ctx, args)
}

fn run_with<S: PortsSys>(sys: &S, ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut json) = (Vec::new(), false);
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, args.get(i + 1))?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"--json" => json = true,
            b"-h" | b"--help" => return crate::args::usage("ports"),
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "ports", a)),
            _ => return Err(unexpected(ctx, "ports", a, "")),
        }
        i += 1;
    }
    ctx.parsed();
    if !name.is_empty() {
        select_box(ctx, &name)?;
        need_box(ctx)?;
        name = ctx.name.clone();
    }
    // Every box up is read, -b NAME or not: whether its port reaches here depends on the others.
    let me = crate::procfs::euid().to_string().into_bytes();
    let mut pkeys: Vec<Vec<u8>> = Vec::new();
    let mut addrs: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();
    let mut comms: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();
    let mut pnet: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();
    let mut serving: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();
    for n in list_names(ctx, false) {
        select_box(ctx, &n)?;
        let Some(pid) = sys.up_pid(&ctx.d) else {
            continue;
        };
        // Read once, with a fallback (#92): a box going down meanwhile has its dir removed. A box
        // whose box.json is gone already is left out.
        let (net, _) = meta(&ctx.d, "net");
        if net.is_empty() && !ctx.at("box.json").is_file() {
            continue;
        }
        let mut own = HashMap::new();
        sys.sock_owners(&sys.box_pids(ctx), &mut own);
        // Sockets no box process holds are pasta's, in the box: its mirrors of the host's servers.
        for [port, addr, _uid, ino] in sys.tcp_listeners(&cat!("/proc/", pid, "/net")) {
            let Some(p) = own.get(&ino).filter(|p| !p.is_empty()) else {
                continue;
            };
            let key = cat!(ctx.name, " ", port);
            if addrs.get(&key).is_none_or(Vec::is_empty) {
                pkeys.push(key.clone());
                comms.insert(key.clone(), comm(p).unwrap_or(b"?".to_vec()));
                pnet.insert(key.clone(), net.clone());
            }
            let cur = addrs.get(&key).cloned().unwrap_or_default();
            if !crate::exit::contains(&cat!(",", cur, ","), &cat!(",", addr, ",")) {
                let v = if cur.is_empty() {
                    addr.clone()
                } else {
                    cat!(cur, ",", addr)
                };
                addrs.insert(key, v);
            }
        }
        // An isolated box's --allow ports (#286): pasta listens on them in the box, leading to the
        // host's, so a box server on one fails at once. Listed, so that is not a mystery.
        if net == b"isolated" {
            let (allow, _) = meta(&ctx.d, "allow");
            for port in comma_words(&allow) {
                let key = cat!(ctx.name, " ", port);
                if !shell::is_num(&port) || addrs.get(&key).is_some_and(|a| !a.is_empty()) {
                    continue;
                }
                pkeys.push(key.clone());
                addrs.insert(key.clone(), b"127.0.0.1,::1".to_vec());
                comms.insert(key.clone(), b"pasta".to_vec());
                pnet.insert(key, b"allowed".to_vec());
            }
        }
    }
    for key in &pkeys {
        if pnet.get(key).map(Vec::as_slice) == Some(b"connected") {
            let (b, port) = crate::cmd::mode::screen_parts(key);
            serving
                .entry(port.to_vec())
                .or_default()
                .extend_from_slice(&cat!(" ", b));
        }
    }
    // In a jail list_names is the jail's boxes only, so a port another jail's box or one of the
    // user's holds was put down to the jail's box serving on it too (finding 237): the ports the
    // connected boxes outside the jail serve on, so such a port reads as taken.
    let mut outside: HashMap<Vec<u8>, bool> = HashMap::new();
    if in_jail() {
        let mine: Vec<Vec<u8>> = list_names(ctx, false);
        for n in list_names(ctx, true) {
            if mine.contains(&n) {
                continue;
            }
            // (not select_box: it refuses a box not the jail's)
            ctx.name = n.clone();
            ctx.d = cat!(ctx.boxes, "/", n);
            let Some(pid) = sys.up_pid(&ctx.d) else {
                continue;
            };
            if meta(&ctx.d, "net").0 != b"connected" {
                continue;
            }
            let mut own = HashMap::new();
            sys.sock_owners(&sys.box_pids(ctx), &mut own);
            for [port, _, _, ino] in sys.tcp_listeners(&cat!("/proc/", pid, "/net")) {
                if own.get(&ino).is_some_and(|p| !p.is_empty()) {
                    outside.insert(port, true);
                }
            }
        }
    }
    // What holds each port on 127.0.0.1 here: a listener on 127.0.0.1, 0.0.0.0 or :: (or their
    // mapped forms) keeps pasta from binding it.
    let mut held: HashMap<Vec<u8>, (Vec<u8>, Vec<u8>)> = HashMap::new();
    for [port, addr, uid, ino] in sys.tcp_listeners(b"/proc/self/net") {
        if matches!(
            addr.as_slice(),
            b"127.0.0.1" | b"0.0.0.0" | b"::" | b"::ffff:127.0.0.1" | b"::ffff:0.0.0.0"
        ) && !held.contains_key(&port)
        {
            held.insert(port, (uid, ino));
        }
    }
    let mut hostpid = HashMap::new();
    if held.values().any(|(u, _)| *u == me) {
        let pids: Vec<Vec<u8>> = shell::glob(std::path::Path::new("/proc"), false)
            .into_iter()
            .filter(|p| p.first().is_some_and(u8::is_ascii_digit))
            .collect();
        sys.sock_owners(&pids, &mut hostpid);
    }
    let jailed = in_jail();
    let mut rows: Vec<Vec<u8>> = Vec::new();
    for key in &pkeys {
        let (n, port) = crate::cmd::mode::screen_parts(key);
        let (mut who, mut boxes, mut resets) = (Vec::new(), Vec::new(), Vec::new());
        let pn = pnet.get(key).cloned().unwrap_or_default();
        let state: Vec<u8> = if pn == b"isolated" || pn == b"allowed" {
            pn.clone()
        } else if pn == b"host" {
            // (#313: no forward, no mirror; the box's server is on the host's own port)
            b"host-net".to_vec()
        } else if pn != b"connected" {
            b"no-network".to_vec()
        } else if let Some((uid, ino)) = held.get(port) {
            if *uid != me {
                who = uid.clone();
                b"other-user".to_vec()
            } else if let Some(hp) = hostpid.get(ino).filter(|p| !p.is_empty()) {
                who = hp.clone();
                b"host".to_vec()
            } else if outside.contains_key(port) {
                boxes = b"outside".to_vec();
                b"host".to_vec()
            } else {
                let s = serving.get(port).cloned().unwrap_or_default();
                boxes = s.strip_prefix(b" ").unwrap_or(&s).to_vec();
                for b in shell::words(&boxes) {
                    let a = addrs.get(&cat!(b, " ", port)).cloned().unwrap_or_default();
                    if local_only(&a) {
                        resets.extend_from_slice(&cat!(" ", b));
                    }
                }
                if resets.first() == Some(&b' ') {
                    resets.remove(0);
                }
                if boxes != n {
                    b"shared".to_vec()
                } else if !resets.is_empty() {
                    b"this-resets".to_vec()
                } else {
                    b"this".to_vec()
                }
            }
        } else {
            b"nothing".to_vec()
        };
        // A jailed agent (#93) is told the state, not which of the user's processes or users.
        if jailed {
            who.clear();
        }
        rows.push(cat!(
            n,
            "|",
            port,
            "|",
            addrs.get(key).cloned().unwrap_or_default(),
            "|",
            comms.get(key).cloned().unwrap_or_default(),
            "|",
            state,
            "|",
            who,
            "|",
            boxes,
            "|",
            resets
        ));
    }
    if !name.is_empty() {
        rows.retain(|o| o.split(|&c| c == b'|').next() == Some(name.as_slice()));
    }
    if !rows.is_empty() {
        let all = cat!(rows.join(&b'\n'), "\n");
        let s = jq::tool(
            "sort",
            &["-t|", "-k1,1", "-k2,2n"],
            Some(&all),
            jq::Err::Inherit,
        );
        rows = records(&s.out).into_iter().map(<[u8]>::to_vec).collect();
    }
    // (`IFS='|' read -r -a f <<<"$o|"`: its first line's eight fields)
    let fields = |o: &[u8]| -> Vec<Vec<u8>> {
        let line = o.split(|&c| c == b'\n').next().unwrap_or_default();
        let mut f: Vec<Vec<u8>> = line.split(|&c| c == b'|').map(<[u8]>::to_vec).collect();
        f.resize(8, Vec::new());
        f
    };
    if json {
        let mut each = Vec::new();
        let mut rc = 0;
        for o in &rows {
            let f = fields(o);
            let hcomm = if f[4] == b"host" && !f[5].is_empty() {
                comm(&f[5]).unwrap_or_default()
            } else {
                Vec::new()
            };
            let r = Args::new(&["-nc"])
                .kv("--arg", "box", &f[0])
                .kv("--argjson", "port", &f[1])
                .kv("--arg", "addr", &f[2])
                .kv("--arg", "proc", &f[3])
                .kv("--arg", "state", &f[4])
                .kv("--arg", "who", &f[5])
                .kv("--arg", "boxes", &f[6])
                .kv("--arg", "resets", &f[7])
                .kv("--arg", "hcomm", &hcomm)
                .arg(crate::jqprog::PORTS_JSON_JQ)
                .run(None, jq::Err::Inherit);
            rc = r.rc;
            each.extend_from_slice(&r.out);
        }
        let r = jq::tool("jq", &["-sc", "."], Some(&each), jq::Err::Inherit);
        out(&r.out)?;
        // (the loop's status is its last jq's; pipefail takes it when the last one is fine)
        return crate::cmd::hyprctl::code(if r.rc != 0 { r.rc } else { rc });
    }
    let line = |f: [&[u8]; 5]| -> Vec<u8> {
        cat!(
            pad(f[0], 24),
            " ",
            pad(f[1], 6),
            " ",
            pad(f[2], 17),
            " ",
            pad(f[3], 12),
            " ",
            f[4],
            "\n"
        )
    };
    out(line([
        b"BOX",
        b"PORT",
        b"ADDRESS",
        b"PROCESS",
        b"ON 127.0.0.1:PORT HERE",
    ]))?;
    for o in &rows {
        let f = fields(o);
        let says: Vec<u8> = match f[4].as_slice() {
            b"this" => b"this box".to_vec(),
            b"this-resets" => cat!(
                "this box's pasta holds it, but connections are reset: the server listens on ",
                f[2],
                " only (127.0.0.1, 0.0.0.0 or :: would answer)"
            ),
            b"shared" => {
                let mut why = Vec::new();
                for r in shell::words(&f[7]) {
                    if !why.is_empty() {
                        why.extend_from_slice(b"; ");
                    }
                    let a = addrs.get(&cat!(r, " ", f[1])).cloned().unwrap_or_default();
                    why.extend_from_slice(&cat!(r, "'s server is on ", a, " only and would reset"));
                }
                let list: Vec<u8> = f[6]
                    .iter()
                    .flat_map(|&c| if c == b' ' { b", ".to_vec() } else { vec![c] })
                    .collect();
                let why = if why.is_empty() {
                    Vec::new()
                } else {
                    cat!(" (", why, ")")
                };
                cat!(
                    "one of ",
                    list,
                    ": only one of them gets it, omabox cannot tell which",
                    why
                )
            }
            b"host" => {
                if f[6] == b"outside" {
                    b"taken: a box outside this jail serves on it too, and only one of them gets it (which, omabox cannot tell from here): use another port".to_vec()
                } else if f[5].is_empty() {
                    b"not this box: a process of the user's holds it".to_vec()
                } else {
                    cat!(
                        "not this box: ",
                        comm(&f[5]).unwrap_or(b"a process".to_vec()),
                        " (pid ",
                        f[5],
                        ") holds it"
                    )
                }
            }
            b"other-user" => {
                if f[5].is_empty() {
                    b"not this box: another user's process holds it".to_vec()
                } else {
                    cat!("not this box: a process of uid ", f[5], " holds it")
                }
            }
            b"nothing" => {
                b"nothing yet (a port comes about a second after its server listens)".to_vec()
            }
            b"isolated" => b"not forwarded: the box is isolated".to_vec(),
            b"allowed" => b"the host's: --allow leads the box's port there (pasta's, in the box: a box server cannot have it)".to_vec(),
            b"no-network" => b"not forwarded: the box has no network".to_vec(),
            b"host-net" => b"the host's own port: the box shares your network (--net host), nothing is forwarded".to_vec(),
            _ => Vec::new(),
        };
        out(line([&f[0], &f[1], &f[2], &f[3], &says]))?;
    }
    if rows.is_empty() {
        out("(no box has a TCP server)\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// t_unit_cli's stand-ins: every box up (PID 1 this process), one server on 0.0.0.0:40001 held by
    /// this process in each box, none on this side; reading box PA's processes takes its box.json
    /// away, as a down meanwhile does.
    struct Going {
        pa: Vec<u8>,
    }

    impl PortsSys for Going {
        fn up_pid(&self, _: &[u8]) -> Option<Vec<u8>> {
            Some(std::process::id().to_string().into_bytes())
        }
        fn box_pids(&self, ctx: &Ctx) -> Vec<Vec<u8>> {
            if ctx.name == self.pa {
                let _ = std::fs::remove_file(ctx.at("box.json"));
            }
            vec![std::process::id().to_string().into_bytes()]
        }
        fn tcp_listeners(&self, dir: &[u8]) -> Vec<[Vec<u8>; 4]> {
            if dir == b"/proc/self/net" {
                return Vec::new();
            }
            vec![[
                b"40001".to_vec(),
                b"0.0.0.0".to_vec(),
                b"0".to_vec(),
                b"424242".to_vec(),
            ]]
        }
        fn sock_owners(&self, _: &[Vec<u8>], own: &mut HashMap<Vec<u8>, Vec<u8>>) {
            own.insert(
                b"424242".to_vec(),
                std::process::id().to_string().into_bytes(),
            );
        }
    }

    // t_unit_cli (#92, finding 164): ports while a box goes down, its box.json gone after the up
    // check (pb) or during its scan (pa): listed as it was or left out, exit 0.
    #[test]
    fn ports_while_a_box_goes_down() {
        let rt = std::env::temp_dir().join(format!("omabox-ports-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&rt);
        for b in ["pa", "pb"] {
            std::fs::create_dir_all(rt.join(b)).unwrap();
        }
        std::fs::write(rt.join("pa/box.json"), r#"{"net":"connected"}"#).unwrap();
        // (pb listed by its info.json: its box.json gone already)
        std::fs::write(rt.join("pb/info.json"), "{}").unwrap();
        let mut ctx = Ctx::new(b"ports", shell::bytes(&rt).to_vec());
        let sys = Going { pa: b"pa".to_vec() };
        let (r, said) = crate::exit::heard_all(|| {
            shell::with_env(&[("OMABOX_JAIL", None)], || {
                run_with(&sys, &mut ctx, &[b"--json".to_vec()])
            })
        });
        assert!(r.is_ok(), "exit 0: {said}");
        let rows = jq::jq(
            &["-r", r#".[] | "\(.box) \(.port) \(.host.state)""#],
            Some(said.as_bytes()),
            jq::Err::Inherit,
        );
        assert_eq!(
            String::from_utf8_lossy(&rows.text()),
            "pa 40001 nothing",
            "ports: a box going down meanwhile is listed as it was or left out: {said}"
        );
        let _ = std::fs::remove_dir_all(&rt);
    }

    /// A host box's stand-ins (#313): box ph up (PID 1 this process), its server on 127.0.0.1:40002
    /// (socket 525252), which is also what holds that port on this side (the same network).
    struct HostNet;

    impl PortsSys for HostNet {
        fn up_pid(&self, _: &[u8]) -> Option<Vec<u8>> {
            Some(me())
        }
        fn box_pids(&self, _: &Ctx) -> Vec<Vec<u8>> {
            vec![me()]
        }
        fn tcp_listeners(&self, _: &[u8]) -> Vec<[Vec<u8>; 4]> {
            vec![[
                b"40002".to_vec(),
                b"127.0.0.1".to_vec(),
                crate::procfs::euid().to_string().into_bytes(),
                b"525252".to_vec(),
            ]]
        }
        fn sock_owners(&self, _: &[Vec<u8>], own: &mut HashMap<Vec<u8>, Vec<u8>>) {
            own.insert(b"525252".to_vec(), me());
        }
    }

    // #313 (finding 706): a host box's server is on the host's own port, which ports says as such
    // (state host-net, nothing forwarded), not as a process of the user's holding it.
    #[test]
    fn ports_says_a_host_box_is_on_the_hosts_port() {
        let rt = std::env::temp_dir().join(format!("omabox-hports-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&rt);
        std::fs::create_dir_all(rt.join("ph")).unwrap();
        std::fs::write(rt.join("ph/box.json"), r#"{"net":"host"}"#).unwrap();
        let ports = |json: bool| {
            let mut ctx = Ctx::new(b"ports", shell::bytes(&rt).to_vec());
            let args: Vec<Vec<u8>> = if json {
                vec![b"--json".to_vec()]
            } else {
                Vec::new()
            };
            crate::exit::heard_all(|| {
                shell::with_env(&[("OMABOX_JAIL", None)], || {
                    run_with(&HostNet, &mut ctx, &args)
                })
            })
            .1
        };
        let said = ports(true);
        let st = jq::jq(
            &["-c", ".[] | [.box, .port, .host]"],
            Some(said.as_bytes()),
            jq::Err::Inherit,
        );
        assert_eq!(
            String::from_utf8_lossy(&st.text()),
            r#"["ph",40002,{"state":"host-net"}]"#,
            "{said}"
        );
        let text = ports(false);
        assert!(
            text.contains("the host's own port: the box shares your network (--net host)"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(&rt);
    }

    /// t_unit_jail_policy's stand-ins: box jp (the jail's, pid "1") serves on 40001 (socket
    /// 424242), held on this side by socket 515151 of uid UID. With OUTSIDE, box jo (another
    /// jail's, pid "2") serves on it too when it is Some(true) (socket 434343), and the host's
    /// holder is one no readable process has (a pasta's).
    struct JPorts {
        uid: String,
        outside: Option<bool>,
    }

    fn me() -> Vec<u8> {
        std::process::id().to_string().into_bytes()
    }

    impl PortsSys for JPorts {
        fn up_pid(&self, d: &[u8]) -> Option<Vec<u8>> {
            Some(if d.ends_with(b"/jo") { b"2" } else { b"1" }.to_vec())
        }
        fn box_pids(&self, ctx: &Ctx) -> Vec<Vec<u8>> {
            vec![if ctx.name == b"jo" { b"2" } else { b"1" }.to_vec()]
        }
        fn tcp_listeners(&self, dir: &[u8]) -> Vec<[Vec<u8>; 4]> {
            let l = |u: &str, ino: &str| {
                [
                    b"40001".to_vec(),
                    b"0.0.0.0".to_vec(),
                    u.as_bytes().to_vec(),
                    ino.as_bytes().to_vec(),
                ]
            };
            match dir {
                b"/proc/self/net" => vec![l(&self.uid, "515151")],
                b"/proc/2/net" if self.outside == Some(true) => vec![l("0", "434343")],
                b"/proc/2/net" => Vec::new(),
                _ => vec![l("0", "424242")],
            }
        }
        fn sock_owners(&self, pids: &[Vec<u8>], own: &mut HashMap<Vec<u8>, Vec<u8>>) {
            if self.outside.is_none() {
                own.insert(b"424242".to_vec(), me());
                own.insert(b"515151".to_vec(), me());
            } else if pids == [b"2".to_vec()] {
                own.insert(b"434343".to_vec(), me());
            } else {
                own.insert(b"424242".to_vec(), me());
            }
        }
    }

    // t_unit_jail_policy's (#93, finding 237): ports for a jailed caller, the host side by state only;
    // a port a box outside the jail serves on too reads as taken, without naming that box.
    #[test]
    fn ports_for_a_jailed_caller() {
        let rt = std::env::temp_dir().join(format!("omabox-jports-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&rt);
        std::fs::create_dir_all(rt.join("jp")).unwrap();
        std::fs::write(rt.join("jp/box.json"), r#"{"net":"connected","jail":"j1"}"#).unwrap();
        let uid = crate::procfs::euid().to_string();
        let ports = |sys: &JPorts, jailed: bool, json: bool| {
            let mut ctx = Ctx::new(b"ports", shell::bytes(&rt).to_vec());
            let args: Vec<Vec<u8>> = if json {
                vec![b"--json".to_vec()]
            } else {
                Vec::new()
            };
            let jail = jailed.then_some(r#"{"id":"j1"}"#);
            let (_, said) = crate::exit::heard_all(|| {
                shell::with_env(&[("OMABOX_JAIL", jail)], || run_with(sys, &mut ctx, &args))
            });
            said
        };
        let host = |said: &str| {
            String::from_utf8_lossy(
                &jq::jq(
                    &["-c", ".[0].host"],
                    Some(said.as_bytes()),
                    jq::Err::Inherit,
                )
                .text(),
            )
            .into_owned()
        };
        let mine = JPorts {
            uid: uid.clone(),
            outside: None,
        };
        let comm = std::fs::read_to_string("/proc/self/comm").unwrap();
        let out = ports(&mine, false, false);
        assert!(
            out.contains(&format!(
                "not this box: {} (pid {}) holds it",
                comm.trim(),
                std::process::id()
            )),
            "ports, not jailed: the host process named: {out}"
        );
        assert_eq!(
            host(&ports(&mine, true, true)),
            r#"{"state":"host"}"#,
            "ports, jailed: no pid or process in --json"
        );
        assert!(
            ports(&mine, true, false).contains("not this box: a process of the user's holds it"),
            "...nor in the text"
        );
        let other = JPorts {
            uid: "4242".into(),
            outside: None,
        };
        assert_eq!(
            host(&ports(&other, true, true)),
            r#"{"state":"other-user"}"#,
            "...another user's: no uid"
        );
        assert!(
            ports(&other, true, false).contains("not this box: another user's process holds it"),
            "...said so"
        );
        // Finding 237: the port held by a socket no readable process has (a pasta's), with a box
        // outside the jail (jo, socket 434343) serving on it too: taken, not `this`; with none,
        // `this` as before.
        std::fs::create_dir_all(rt.join("jo")).unwrap();
        std::fs::write(rt.join("jo/box.json"), r#"{"net":"connected","jail":"j2"}"#).unwrap();
        let out_on = JPorts {
            uid: uid.clone(),
            outside: Some(true),
        };
        assert_eq!(
            host(&ports(&out_on, true, true)),
            r#"{"state":"host"}"#,
            "ports, jailed: a port a box outside the jail serves on too reads as host, not this (finding 237)"
        );
        let text = ports(&out_on, true, false);
        assert!(
            text.contains("taken: a box outside this jail serves on it too"),
            "...said, without naming that box: {text}"
        );
        assert!(
            !text.contains("jo") && !ports(&out_on, true, true).contains("jo"),
            "...whose name is not in it"
        );
        let out_off = JPorts {
            uid,
            outside: Some(false),
        };
        let st = jq::jq(
            &["-r", ".[0].host.state"],
            Some(ports(&out_off, true, true).as_bytes()),
            jq::Err::Inherit,
        );
        assert_eq!(
            st.text(),
            b"this",
            "...with no box outside on it: this, as before"
        );
        let _ = std::fs::remove_dir_all(&rt);
    }
}
