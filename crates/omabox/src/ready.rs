//! up, from its launch to a box ready (#324 wave 5a): the waits for Hyprland, the shell and its bar, a
//! start that died, the owner gone. As bin/omabox's ready_timeout, wait_hyprland, wait_ready,
//! bar_settle, shell_crash and shell_watch, died_starting, starting_ok and up_owner_check (with
//! kill_box, kill_reapers, rm_box), sweep_homes, box_held, more_screens, client_render_check and
//! box_json.

use crate::boxes::{BoxPid, BoxSys, RealBox, box_home_file, box_state, meta, run_read};
use crate::cat;
use crate::exec::{ErrTo, ExecOpts, Out, Redir, box_exec, on_box, pipe, read_all};
use crate::exit::{Ctx, Exit, Res, muted, say};
use crate::jq::{self, Args};
use crate::mon::w;
use crate::settle::{after, mono_ms, past};
use crate::shell::{self, strip_nl};
use crate::shellnote::{shell_crash_dirs, shell_crashes, shell_pid, shell_running_with};
use std::ffi::{OsStr, OsString};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::time::{Duration, Instant};

/// wait_ready's tray check: whether the box's shell.json has the tray in its bar.
const TRAY_JQ: &str = r#"[.bar.layout[][]? | if type == "object" then .id else . end] | index("omarchy.tray") != null"#;
/// intro_cover: Omarchy 4.5's start-up cover on the Overlay level (#291, finding 415).
const INTRO_COVER_JQ: &str = r#"first(.[].levels["3"][]? | select(.namespace == "omarchy-background") | "\(.x),\(.y) \(.w)x\(.h)")"#;
/// shell_adopt: the pid of the shell another start brought up (finding 248).
const ADOPT_JQ: &str = "first(.[].pid) // empty";
/// shell_adopt's write of that pid where shell.sh keeps it, in the box.
const ADOPT_SH: &str =
    r#"f=$XDG_RUNTIME_DIR/omabox-shell.pid; echo "$1" > "$f.new" && mv -f "$f.new" "$f""#;
/// more_screens (#308).
const MORE_SCREENS_JQ: &str = r#"[.[] | select((.disabled | not) and .name != $main)
    | " + \(.width)x\(.height)@\(.refreshRate * 100 | round / 100)" + ((.scale * 100 | round / 100) as $s
      | if $s == 1 then "" else ",scale=\($s)" end)] | join("")"#;
/// egl_render's awk: the OpenGL ES profile's renderer, else the first one named (#204, finding 290).
const EGL_AWK: &str = r#"/^OpenGL ES profile renderer: / { sub(/^[^:]*: /, ""); es = $0 }
    /renderer: / && first == "" { first = $0; sub(/^[^:]*: /, "", first) }
    END { print (es != "" ? es : first) }"#;
/// egl_render's record.
const EGL_JQ: &str = r#"{renderer: $r, software: ($r | test("llvmpipe|softpipe|swrast|software rasterizer|lavapipe"; "i"))}"#;
const CLIENT_RENDER_JQ: &str = ".client_render = $r";
const RENDER_DRIVER_JQ: &str = ".render.driver // empty";
/// soft_note: the renderer, when it is software.
const SOFT_JQ: &str = "select(.software == true) | .renderer";
/// box_orphan (#220).
const ORPHAN_JQ: &str = r#"select(.mode == "headless" and (.idle // 0) == 0 and .owner == null and .agent == null
    and .starter.pid != null and .starter.start != null and (.starter.terminal | not)) | "\(.starter.pid) \(.starter.start)""#;
/// box_json: one box as `ls --json` lists it.
const BOX_JSON_JQ: &str = r#"{name: $name, mode, size, scale: (.scale // 1), created, plugins, net: (.net // "host"),
      allowed: (.allow // "" | split(",") | map(select(. != "") | tonumber)), addrs: (.addrs // []), idle: (.idle // 0),
      dir: (.dir // null),
      plugin_status: ((.plugin_status // {}) as $s | (.plugin_via // {}) as $v
        | reduce (.plugins // [])[] as $i ($s; .[$i] = ((.[$i] // {}) + {via: ($v[$i] // "mounted")}))), bar: (.bar // "user"), systemd: (.systemd // false), audio: (.audio // false), autoreload: (.autoreload // false), hyprland: (.hyprland // null), omarchy: (.omarchy // null),
      hyprland_version: (.hyprland_version // null), quickshell: (.quickshell // null), quickshell_version: (.quickshell_version // null), aquamarine: (.aquamarine // null), render: (.render // null),
      client_render: (.client_render // null),
      monitors: (.monitors // null),
      omarchy_version: (.omarchy_version // null), theme: (if $theme != "" then $theme else null end),
      starter: (.starter // null)} + (if .desk_shell then {desk_shell} else {} end) + (if .no_seed then {no_seed} else {} end) + {desk_fonts: (.desk_fonts // [])} + {clean: (.clean // null)} + {state: $state, hung: $hung, peeking: $peeking, keys_to_box: $keys, shell_state: $sh, shell_crashes: $crashes, orphan: $orphan}"#;
/// box_json's answer for a box.json jq cannot read.
const BOX_JSON_BARE_JQ: &str = "{name: $name, state: $state, hung: false, plugins: [], peeking: false, keys_to_box: false, orphan: false}";

/// UP_OWNER: a throwaway box's `omabox run` (`up --owner`), its pid and start time as box.json has
/// them (an empty start: a box.json from before owner_start, the pid alone to go by).
pub struct Owner {
    pub pid: Vec<u8>,
    pub start: Vec<u8>,
}

/// What up's waits share from the launch on: the owner it goes with, whether a jailed caller's covers
/// were made (#210: died_starting's hint), the crash folders before the shell started (SHELL_BEFORE:
/// up sets it once the HOME is seeded, from shell_crash_dirs) and when the bar came (BAR_AT, mono_ms,
/// set by wait_ready: finding 450, where bash read the wall clock). BEFORE_END drops up's own undo
/// (bash's `trap - EXIT`): up_owner_check calls it before it ends the box itself.
pub struct Ready<'a> {
    pub owner: Option<Owner>,
    pub covers: bool,
    pub shell_before: Vec<u8>,
    pub bar_at: i64,
    pub before_end: &'a dyn Fn(),
}

/// ready_timeout (#273): $OMABOX_READY_TIMEOUT, a duration (30s, 2m; a bare number is seconds), as
/// whole seconds, rounded up. up asks it before any box starts, so a bad value is said then.
pub fn ready_timeout(ctx: &Ctx) -> Res<u64> {
    let raw = crate::paths::ready_timeout_raw();
    let Some(ms) = crate::num::ms_duration(&raw) else {
        return Err(ctx.die(cat!(
            "OMABOX_READY_TIMEOUT takes a duration, like 30s or 2m (a bare number is seconds), got ",
            raw
        )));
    };
    Ok(u64::try_from(ms.saturating_add(999) / 1000).unwrap_or(0))
}

fn poll() {
    crate::sig::sleep(Duration::from_millis(100));
}

fn os(v: &[u8]) -> &OsStr {
    OsStr::from_bytes(v)
}

/// starting_ok: where up waits on its box, one that died, or a throwaway's whose `omabox run` is
/// gone, ends it now.
pub fn starting_ok(ctx: &Ctx, r: &Ready) -> Res<()> {
    starting_ok_with(&RealBox, ctx, r)
}

fn starting_ok_with<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx, r: &Ready) -> Res<()> {
    if !sys.box_alive(ctx)? {
        return Err(died_starting(ctx, r.covers));
    }
    up_owner_check_with(sys, ctx, r.owner.as_ref(), r.before_end)
}

/// up_owner_check (#222): a throwaway box's up whose `omabox run` is gone (killed outright while up
/// waited on the box) ends the box itself, as the reaper's down would; one whose PID 1 will not go
/// stays, dead, for the reaper. BEFORE_END first (up's undo dropped: bash's `trap - EXIT`), and
/// SIGPIPE ignored (`trap '' PIPE`): the run read this up's stderr, and a write to it could end this
/// process. Said last. Err(1) when it ended the up.
pub fn up_owner_check(ctx: &Ctx, owner: Option<&Owner>, before_end: &dyn Fn()) -> Res<()> {
    up_owner_check_with(&RealBox, ctx, owner, before_end)
}

fn up_owner_check_with<S: BoxSys + ?Sized>(
    sys: &S,
    ctx: &Ctx,
    owner: Option<&Owner>,
    before_end: &dyn Fn(),
) -> Res<()> {
    let Some(o) = owner else { return Ok(()) };
    if owner_alive(&o.pid, &o.start) {
        return Ok(());
    }
    before_end();
    // SAFETY: SIGPIPE's disposition set to ignored; no handler of ours is replaced.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let mut left = "";
    if muted(|| sys.kill_box(ctx)) {
        kill_reapers(ctx);
        sys.rm_box(ctx);
        let _ = std::fs::remove_file(ctx.in_boxes(cat!(".lock-", ctx.name)));
    } else {
        left = " (left, dead, to its reaper)";
    }
    say(cat!(
        "box '",
        ctx.name,
        "': its omabox run (",
        o.pid,
        ") is gone: not starting it",
        left
    ));
    Err(Exit(1))
}

/// owner_alive PID START: the `omabox run` that owns a throwaway box still runs: its pid, started
/// when it was then, and not a zombie (a pid reused after a SIGKILLed run is not it). No START: the
/// pid alone.
pub fn owner_alive(pid: &[u8], start: &[u8]) -> bool {
    if !signal_ok(pid, 0) {
        return false;
    }
    if start.is_empty() {
        return true;
    }
    let (Some(state), Some(st)) = (
        crate::procfs::proc_stat(pid, 0),
        crate::procfs::proc_start(pid),
    ) else {
        return false;
    };
    state != b"Z" && st == start
}

/// `kill -SIG PID`: whether it was sent (a pid only; anything else is no process).
fn signal_ok(pid: &[u8], sig: libc::c_int) -> bool {
    let Some(p) = shell::dec(pid)
        .and_then(|n| i32::try_from(n).ok())
        .filter(|&n| n > 0)
    else {
        return false;
    };
    // SAFETY: kill with a positive pid and a signal number; nothing of ours is touched.
    unsafe { libc::kill(p, sig) == 0 }
}

/// kill_box: SIGKILL the box's PID 1 (the kernel then kills everything in its pid namespace;
/// SIGTERM is ignored, finding 6) and pasta's; wait for them, at most 10 s. The dir stays. Whether
/// PID 1 is gone (bash's status).
pub fn kill_box(ctx: &Ctx) -> bool {
    kill_box_with(&RealBox, ctx)
}

pub fn kill_box_with<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx) -> bool {
    let pid = match sys.box_pid(&ctx.d) {
        BoxPid::Pid(p) => p,
        BoxPid::None | BoxPid::Unknown => Vec::new(),
    };
    if !pid.is_empty() {
        // A launcher stopped along with its command can't reap it, and PID 1 can't exit until it does:
        // each process that entered the box from outside (enter.rs's, bash's nsenter) is continued.
        // (found while PID 1 is there to compare namespaces with)
        let entered = crate::procfs::entered(&pid);
        signal_ok(&pid, libc::SIGKILL);
        for p in entered {
            signal_ok(&p, libc::SIGCONT);
        }
    }
    // A box whose pid was never found: pasta's own pid namespace holds it, end that instead.
    if let Some(pasta) = sys.box_pasta(&ctx.d) {
        signal_ok(&pasta, libc::SIGKILL);
        if let Some(c) = crate::procfs::children(&pasta).first() {
            signal_ok(c, libc::SIGKILL);
        }
    }
    let end = Instant::now() + Duration::from_secs(10);
    while !pid.is_empty() && signal_ok(&pid, 0) {
        if Instant::now() >= end {
            let wchan = std::fs::read(shell::path(&cat!("/proc/", pid, "/wchan")))
                .map(|v| strip_nl(&v))
                .unwrap_or_default();
            say(cat!(
                "box '",
                ctx.name,
                "': PID 1 (",
                pid,
                ") still exiting after 10 s (",
                wchan,
                ")"
            ));
            return false;
        }
        crate::sig::sleep(Duration::from_millis(50));
    }
    true
}

/// kill_reapers: the selected box's reaper (`$ROOT/bin/omabox _reap NAME ...`, the binary's since the
/// switch; bash's `bash $ROOT/bin/omabox _reap NAME ...` before it, or the port's
/// `$ROOT/build/omabox _reap NAME ...` that bash's hand-off exec'd), unless that is us: nothing
/// may be left running for a box that is gone (#115, finding 178). pgrep -f's match of its command
/// line (its words joined by spaces), looked for in /proc here.
pub fn kill_reapers(ctx: &Ctx) {
    let me = std::process::id().to_string().into_bytes();
    for p in reaper_pids(&crate::paths::root(), &ctx.name) {
        if p != me {
            signal_ok(&p, libc::SIGTERM);
        }
    }
}

/// The pids of box NAME's reapers (reaper_line), lowest first.
pub fn reaper_pids(root: &[u8], name: &[u8]) -> Vec<Vec<u8>> {
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let n = e.file_name().into_encoded_bytes();
        if !shell::is_num(&n) {
            continue;
        }
        let Ok(cl) = std::fs::read(e.path().join("cmdline")) else {
            continue;
        };
        if reaper_line(&cl, root, name) {
            out.push(n);
        }
    }
    out.sort_by_key(|p| shell::dec(p).unwrap_or(0));
    out
}

/// Whether CMDLINE (NUL-ended words) is box NAME's reaper: `((/usr/bin/)?bash )?ROOT/bin/omabox _reap
/// NAME ` at its start (the binary's since the switch, #324 wave 6-flip; bash's before it), or the
/// port's, `ROOT/build(/bin)?/omabox _reap NAME ` (bash's hand-off exec'd it; in build's bin/ before
/// wave 6-front).
fn reaper_line(cmdline: &[u8], root: &[u8], name: &[u8]) -> bool {
    let words = cmdline.strip_suffix(b"\0").unwrap_or(cmdline);
    let line: Vec<u8> = words
        .iter()
        .map(|&c| if c == 0 { b' ' } else { c })
        .collect();
    let tail = cat!(root, "/bin/omabox _reap ", name, " ");
    [b"".as_slice(), b"bash ", b"/usr/bin/bash "]
        .iter()
        .any(|pre| line.starts_with(&cat!(pre, tail)))
        || line
            .strip_prefix(cat!(root, "/build").as_slice())
            .map(|l| l.strip_prefix(b"/bin").unwrap_or(l))
            .is_some_and(|l| l.starts_with(&cat!("/omabox _reap ", name, " ")))
}

/// rm_box: the box dir and its HOME; only a dir under BOXES, and the HOME only when it is this box's
/// or no box's (home_taken: never a box's of another runtime dir, finding 584).
pub fn rm_box(ctx: &Ctx) {
    let pre = cat!(ctx.boxes, "/");
    if !(ctx.d.starts_with(&pre) && ctx.d.len() > pre.len()) {
        return;
    }
    let home = cat!(crate::paths::homes(), "/", ctx.name);
    let mut paths = vec![ctx.d.clone()];
    if home_taken(ctx).is_none() {
        paths.push(home);
    }
    rm_tree(&cat!("box '", ctx.name, "'"), &paths);
}

/// home_taken: why this box's name may not have its HOME (HOMES/NAME), when it is another box's: its
/// record names another box dir, which is there (that box is up, or being made, in another runtime
/// dir), or it is in use (a box of an older omabox, which wrote no record, runs on it). None when it
/// is this box's (its record names this box dir; an older omabox's, this box dir's `home` leads to
/// it), or no box's.
pub fn home_taken(ctx: &Ctx) -> Option<Vec<u8>> {
    let h = cat!(crate::paths::homes(), "/", ctx.name);
    let p = shell::path(&h);
    if !p.is_dir() || p.is_symlink() {
        return None;
    }
    match crate::homeuse::record(&h) {
        Some(r) if r == ctx.d => return None,
        Some(r) if crate::homeuse::there(&r) => {
            let at = r
                .strip_suffix(cat!("/", ctx.name).as_slice())
                .unwrap_or(&r)
                .to_vec();
            return Some(cat!("is a box's in ", at));
        }
        Some(_) => {}
        None => {
            let link = std::fs::read_link(ctx.at("home")).ok();
            if link.is_some_and(|l| shell::bytes(&l) == cat!(h, "/home").as_slice()) {
                return None;
            }
        }
    }
    crate::homeuse::Users::scan().uses(&h).then(|| {
        b"is in use (a box this runtime dir does not have runs on it: an older omabox's, or one of another runtime dir)".to_vec()
    })
}

/// rm_tree WHAT PATH...: rm -rf, after making each dir writable again (a user manager's mode-000
/// runtime dirs; an app's read-only folder in HOME, #264). A link is never followed out of the tree:
/// chmod -R follows none it meets inside one, and a PATH that is a link is left out of the chmod
/// (rm removes the link). Not chmod's -P, which coreutils has only since 9.5: an older chmod refused
/// it and made nothing writable (finding 524). Says what is left, rm's first line. Whether all went.
pub fn rm_tree(what: &[u8], paths: &[Vec<u8>]) -> bool {
    rm_tree_with("chmod", what, paths)
}

/// rm_tree with CHMOD as its chmod (a test's stand-in for an older one).
fn rm_tree_with(chmod: &str, what: &[u8], paths: &[Vec<u8>]) -> bool {
    let args = |p: &Vec<u8>| -> OsString { os(p).to_os_string() };
    let trees: Vec<&Vec<u8>> = paths
        .iter()
        .filter(|p| !shell::path(p).is_symlink())
        .collect();
    if !trees.is_empty() {
        let mut c = crate::sig::command(chmod);
        c.args(["-R", "u+rwX", "--"])
            .args(trees.into_iter().map(args))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let _ = crate::sig::run_status(c);
    }
    let mut c = crate::sig::command("rm");
    c.args(["-rf", "--"])
        .args(paths.iter().map(args))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let r = crate::sig::run_child(c, false, |ch| {
        let mut e = Vec::new();
        if let Some(mut s) = ch.stderr.take() {
            let _ = std::io::Read::read_to_end(&mut s, &mut e);
        }
        e
    });
    let (rc, err) = match r {
        Ok((err, (rc, _))) => (rc, err),
        Err(e) => (127, cat!("rm: ", e.to_string())),
    };
    if rc == 0 {
        return true;
    }
    let first = err.split(|&c| c == b'\n').next().unwrap_or_default();
    say(cat!("could not remove all of ", what, ": ", first));
    false
}

/// sweep_homes: HOMEs whose box is gone (the runtime dir is cleared at logout or reboot, ~/.cache is
/// not). Gone where it was made: the box dir its record names is not there (an `up` with another
/// runtime dir sees none of this one's boxes, finding 584), and no process uses the HOME (no box has
/// it mounted, nothing has it open). A HOME with no record (an older omabox's) goes only when its box
/// is not in this runtime dir and nothing uses it either. The box dir and the record are made before
/// the HOME, so a box being set up is never swept. In byte order (bash's glob: the locale's
/// collation, finding 446).
pub fn sweep_homes(ctx: &Ctx) {
    let homes = crate::paths::homes();
    let mut users: Option<crate::homeuse::Users> = None;
    for n in shell::glob(&shell::path(&homes), true) {
        let h = cat!(homes, "/", n);
        let p = shell::path(&h);
        if !p.is_dir() || p.is_symlink() {
            continue;
        }
        let made_in =
            crate::homeuse::record(&h).unwrap_or_else(|| shell::bytes(&ctx.in_boxes(&n)).to_vec());
        if crate::homeuse::there(&made_in) {
            continue;
        }
        if users
            .get_or_insert_with(crate::homeuse::Users::scan)
            .uses(&h)
        {
            continue;
        }
        rm_tree(
            &cat!("the HOME of box '", n, "', which is gone"),
            std::slice::from_ref(&h),
        );
    }
}

/// box_held PID (#210): the box's PID 1 is blocked reading fd 4, the box's go pipe: bwrap's
/// --block-fd, which it reaches only once every mount is made.
pub fn box_held(ctx: &Ctx, pid: &[u8]) -> bool {
    let fd4 = std::fs::read_link(shell::path(&cat!("/proc/", pid, "/fd/4")));
    if !fd4.is_ok_and(|l| shell::bytes(&l) == cat!(ctx.d, "/go").as_slice()) {
        return false;
    }
    let (Ok(wc), Ok(sc)) = (
        std::fs::read(shell::path(&cat!("/proc/", pid, "/wchan"))),
        std::fs::read(shell::path(&cat!("/proc/", pid, "/syscall"))),
    ) else {
        return false;
    };
    crate::exit::contains(&strip_nl(&wc), b"pipe_read")
        && crate::exit::contains(&strip_nl(&sc), b" 0x4 ")
}

/// What a VM's render node needs for a box (install.sh says the same, finding 538).
const VIRGL_HINT: &[u8] = b"in a VM, virtio-gpu with virgl (QEMU's virtio-vga-gl, libvirt's accel3d with an OpenGL display)";

/// render_why (#396, finding 568): why a headless box's labwc could not render on its GPU, from the
/// head of its log, else empty. wlroots refuses a render node whose EGL device is software (a VM's
/// virtio-gpu without virgl: "Software rendering detected"), or finds no EGL on it ("Could not
/// initialize EGL", from its GLES2 renderer); either way labwc goes on with pixman, which offers no
/// dmabuf, and the box's Hyprland aborts (CBackend::create() failed!), its line only in labwc.log.
fn render_why(ctx: &Ctx) -> Vec<u8> {
    let Some(f) = box_home_file(&ctx.d, "labwc.log") else {
        return Vec::new();
    };
    let mut log = Vec::new();
    if let Ok(fh) = std::fs::File::open(f) {
        use std::io::Read;
        let _ = fh.take(256 * 1024).read_to_end(&mut log);
    }
    let bj: serde_json::Value = std::fs::read(ctx.at("box.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let field = |k: &str| {
        bj.pointer(&format!("/render/{k}"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .as_bytes()
            .to_vec()
    };
    let (node, driver) = (field("node"), field("driver"));
    if !crate::exit::contains(&log, b"Software rendering detected")
        && !crate::exit::contains(&log, b"Could not initialize EGL")
    {
        return Vec::new();
    }
    let other = if driver.is_empty() {
        Vec::new()
    } else {
        crate::shots::other_gpu(&driver)
    };
    render_why_from(&log, &node, &driver, &other)
}

/// render_why on labwc's LOG, the box's render NODE and its DRIVER, and OTHER as other_gpu gives it
/// (`renderD129 nvidia`, or empty): what to say, or empty when LOG says neither failure.
fn render_why_from(log: &[u8], node: &[u8], driver: &[u8], other: &[u8]) -> Vec<u8> {
    let node: &[u8] = if node.is_empty() {
        b"its render node"
    } else {
        node
    };
    let lines = || log.split(|&c| c == b'\n');
    let mut m = if lines().any(|l| crate::exit::contains(l, b"Software rendering detected")) {
        cat!(
            "a box needs GPU rendering: ",
            node,
            " renders in software only (no 3D acceleration)"
        )
    } else if lines().any(|l| {
        crate::exit::contains(l, b"[render/gles2/")
            && crate::exit::contains(l, b"Could not initialize EGL")
    }) {
        // EGL's own reason: the first `message: "..."` of an [EGL] line.
        let egl = lines()
            .filter(|l| crate::exit::contains(l, b"[EGL]"))
            .find_map(|l| {
                let i = l.windows(10).position(|w| w == b"message: \"")? + 10;
                let t = &l[i..];
                let j = t.iter().rposition(|&c| c == b'"')?;
                Some(shell::cut_line(&t[..j], 200).to_vec())
            })
            .filter(|s| !s.is_empty());
        let mut m = cat!("a box needs GPU rendering: labwc found no EGL on ", node);
        if let Some(e) = egl {
            m.extend(cat!(" (EGL: ", e, ")"));
        }
        m
    } else {
        return Vec::new();
    };
    if driver == b"virtio_gpu" {
        m.extend(cat!("; ", VIRGL_HINT));
    }
    if !other.is_empty() {
        let n = other.split(|&c| c == b' ').next().unwrap_or_default();
        m.extend(cat!(
            "; a box on another GPU: OMABOX_RENDER_NODE=/dev/dri/",
            n,
            " omabox up --new (",
            n,
            " is ",
            after_space(other),
            ")"
        ));
    }
    m
}

/// died_starting COVERS: a box that ended while up waited for it, said with why its labwc could not
/// render when it logged that (#396), else Hyprland's last line when it logged one (finding 95),
/// else why share/session.sh ended it (finding 245), else bwrap's own refusal (a jailed caller's
/// cover, COVERS: #210).
pub fn died_starting(ctx: &Ctx, covers: bool) -> Exit {
    let mut why = Vec::new();
    let render = render_why(ctx);
    let last = box_home_file(&ctx.d, "hyprland.log")
        .and_then(|f| std::fs::read(f).ok())
        .map(|t| last_line(&t))
        .unwrap_or_default();
    let boxlog = std::fs::read(ctx.at("box.log")).unwrap_or_default();
    let sess = boxlog
        .split(|&c| c == b'\n')
        .find(|l| l.starts_with(b"session.sh: ") && !l.starts_with(b"session.sh: Hyprland "))
        .map(|l| shell::cut_line(l, 200))
        .filter(|s| !s.is_empty());
    let bw = boxlog
        .split(|&c| c == b'\n')
        .find(|l| l.starts_with(b"bwrap: "))
        .map(|l| shell::cut_line(l, 300))
        .filter(|s| !s.is_empty());
    if !render.is_empty() {
        why = cat!(" (", render, ")");
    } else if !last.is_empty() {
        why = cat!(" (Hyprland: ", last, ")");
    } else if let Some(s) = sess {
        why = cat!(" (", s.strip_prefix(b"session.sh: ").unwrap_or(s), ")");
    } else if let Some(s) = bw {
        why = cat!(" (", s, ")");
        if covers {
            why.extend_from_slice(
                b"; if the jail renamed a folder holding a path it hides meanwhile, try again",
            );
        }
    }
    ctx.die(cat!(
        "box '",
        ctx.name,
        "' died while starting",
        why,
        "; logs: ",
        ctx.d,
        "/home/*.log ",
        ctx.d,
        "/box.log"
    ))
}

/// `grep -v '^[[:space:]]*$' | tail -n 1 | sed 's/^[[:space:]]*//'`: the last line that is not blank,
/// its leading blanks off.
fn last_line(t: &[u8]) -> Vec<u8> {
    let sp = |c: &u8| matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r');
    t.split(|&c| c == b'\n')
        .rfind(|l| !l.iter().all(sp))
        .map(|l| {
            let i = l.iter().position(|c| !sp(c)).unwrap_or(l.len());
            l[i..].to_vec()
        })
        .unwrap_or_default()
}

/// wait_hyprland: until the box's Hyprland is up (share/hyprland.lua writes omabox.env once it is).
pub fn wait_hyprland(ctx: &Ctx, r: &Ready) -> Res<()> {
    wait_hyprland_with(&RealBox, ctx, r)
}

fn wait_hyprland_with<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx, r: &Ready) -> Res<()> {
    let t = ready_timeout(ctx)?;
    let deadline = after(t);
    while !ctx
        .at("run/omabox.env")
        .metadata()
        .is_ok_and(|m| m.len() > 0)
    {
        starting_ok_with(sys, ctx, r)?;
        if past(deadline) {
            let render = render_why(ctx);
            let why = if render.is_empty() {
                Vec::new()
            } else {
                cat!(" (", render, ")")
            };
            return Err(ctx.die(cat!(
                "Hyprland not up after ",
                t.to_string(),
                "s",
                why,
                "; logs: ",
                ctx.d,
                "/home/*.log"
            )));
        }
        poll();
    }
    Ok(())
}

/// `on_box hyprctl -j layers [2>/dev/null]`: the box's layers, as hyprctl answered (or nothing).
fn layers(ctx: &Ctx, err: ErrTo) -> crate::exec::Ran {
    on_box(
        ctx,
        &w(&[b"hyprctl", b"-j", b"layers"]),
        false,
        Out::Capture(None),
        err,
    )
}

/// `... | layer_at NS`: where layer NS is drawn in LAYERS, or empty; and jq's status.
fn layer_at(lays: &[u8], ns: &str, err: jq::Err) -> jq::Ran {
    Args::new(&["-r", "--arg", "n", ns])
        .arg(crate::jqprog::LAYER_AT_JQ)
        .run(Some(lays), err)
}

/// `box_sys ARGV... >/dev/null 2>&1` (or captured): its status and output (None: the box was not
/// there to run it in).
fn box_sys_rc(ctx: &mut Ctx, argv: &[&[u8]]) -> Option<(i32, Vec<u8>)> {
    crate::layer::box_sys_out(ctx, &w(argv))
}

/// `$(box_exec ARGV... 2>/dev/null [</dev/null] | head -c LIMIT)`: its status and its output, raw
/// (None: refused, the box gone).
fn box_exec_out(
    ctx: &mut Ctx,
    argv: &[Vec<u8>],
    stdin_null: bool,
    limit: Option<usize>,
) -> Option<(i32, Vec<u8>)> {
    let (r, wr) = pipe().ok()?;
    let reader = std::thread::spawn(move || read_all(r, limit));
    let rc = {
        let _e = Redir::null(2);
        box_exec(
            ctx,
            argv,
            ExecOpts {
                stdin_null,
                stdout: Some(wr.as_raw_fd()),
                ..ExecOpts::default()
            },
        )
    };
    drop(wr);
    let out = reader.join().unwrap_or_default();
    rc.ok().map(|rc| (rc, out))
}

/// meta_scale: box.json's scale, 1 when none.
fn meta_scale(d: &[u8]) -> Vec<u8> {
    let s = meta(d, "scale").0;
    if s.is_empty() { b"1".to_vec() } else { s }
}

/// wait_ready WANT_SHELL: Hyprland up, an NVIDIA box's screen at its size, then (WANT_SHELL) the bar
/// on screen with the tray watcher when its layout has a tray, the start-up cover gone, the
/// notification server, and a headless box's bar holding still. Sets R.bar_at.
pub fn wait_ready(ctx: &mut Ctx, r: &mut Ready, want_shell: bool) -> Res<()> {
    let t = ready_timeout(ctx)?;
    let deadline = after(t);
    wait_hyprland(ctx, r)?;
    if meta(&ctx.d, "wayland_screen").0 == b"true" {
        // labwc's initial xdg configure can replace the requested mode with 1280x720. Reapply it
        // after Hyprland starts, when the parent headless output already has the requested size.
        let size = meta(&ctx.d, "size").0;
        let lua = cat!(
            "hl.monitor({ output = 'WAYLAND-1', mode = '",
            size,
            "', position = '0x0', scale = ",
            meta_scale(&ctx.d),
            " })"
        );
        let e = on_box(
            ctx,
            &w(&[b"hyprctl", b"eval", &lua]),
            false,
            Out::Null,
            ErrTo::Inherit,
        )
        .or_exit()?;
        if e.rc != 0 {
            return Err(ctx.die(cat!("could not set the NVIDIA box screen to ", size)));
        }
        loop {
            let screen = muted(|| crate::screen::box_screen(ctx)).unwrap_or_default();
            if after_space(&screen) == size.as_slice() {
                break;
            }
            starting_ok(ctx, r)?;
            if past(deadline) {
                return Err(ctx.die(cat!(
                    "NVIDIA box screen stayed at ",
                    after_space(&screen),
                    ", asked for ",
                    size
                )));
            }
            poll();
        }
    }
    if !want_shell {
        return Ok(());
    }
    // The bar on screen; only wait for the tray watcher when the user's layout has a tray.
    let tray_expected = box_home_file(&ctx.d, ".config/omarchy/shell.json").is_some_and(|f| {
        Args::new(&["-e", TRAY_JQ])
            .arg(shell::bytes(&f))
            .run_file(jq::Err::Null)
            .ok()
    });
    loop {
        let bar = layer_at(&layers(ctx, ErrTo::Null).out, "omarchy-bar", jq::Err::Null).text();
        if !bar.is_empty()
            && (!tray_expected
                || box_sys_rc(
                    ctx,
                    &[
                        b"/usr/bin/busctl",
                        b"--user",
                        b"status",
                        b"org.kde.StatusNotifierWatcher",
                    ],
                )
                .is_some_and(|(rc, _)| rc == 0))
        {
            break;
        }
        starting_ok(ctx, r)?;
        // A shell that crashed (or quit) on its way up: said now, with its report, not after
        // READY_TIMEOUT.
        if let Some(how) = shell_crash(ctx, r, shell_pid(ctx).as_deref()) {
            let m = shell_crash_text(ctx, "while", &how);
            return Err(ctx.die(m.strip_prefix(b"warning: ").unwrap_or(&m)));
        }
        if past(deadline) {
            return Err(ctx.die(cat!(
                "shell not ready after ",
                t.to_string(),
                "s; see ",
                ctx.d,
                "/home/shell.log"
            )));
        }
        poll();
    }
    r.bar_at = mono_ms();
    // Omarchy 4.5's boot-intro cover (#291, finding 415): black over every screen, the bar too, until
    // the shell has its wallpaper. share/shell.sh marks the box's start so the shell never maps it;
    // one up all the same is waited out, 12 s at most (the shell lifts it by 10 s), then said.
    let cd = after(12);
    loop {
        let l = layers(ctx, ErrTo::Null);
        if !Args::new(&["-er", INTRO_COVER_JQ])
            .run(Some(&l.out), jq::Err::Inherit)
            .ok()
        {
            break;
        }
        starting_ok(ctx, r)?;
        if past(cd) || past(deadline) {
            say(cat!(
                "the shell's start-up cover (black, omarchy-background) is still up; see ",
                ctx.d,
                "/home/shell.log"
            ));
            break;
        }
        poll();
    }
    // The notification server (finding 42), which registers up to ~3 s after the bar: a
    // `notify-send` right after `up` failed with ServiceUnknown (finding 62). A warning, not a
    // failed box. (NameHasOwner: `busctl status` exits 0 for a name nobody owns yet.)
    let nd = after(5);
    loop {
        let o = box_sys_rc(
            ctx,
            &[
                b"/usr/bin/busctl",
                b"--user",
                b"call",
                b"org.freedesktop.DBus",
                b"/org/freedesktop/DBus",
                b"org.freedesktop.DBus",
                b"NameHasOwner",
                b"s",
                b"org.freedesktop.Notifications",
            ],
        )
        .map(|(_, o)| o)
        .unwrap_or_default();
        if strip_nl(&o[..o.len().min(64)]) == b"b true" {
            break;
        }
        starting_ok(ctx, r)?;
        if past(nd) {
            say(cat!(
                "no notification server on the box's bus yet; see ",
                ctx.d,
                "/home/shell.log"
            ));
            break;
        }
        poll();
    }
    // The bar's pixels holding still (3 identical captures), so the first shot shows a finished bar.
    // Not for interactive boxes: they are for the user's eyes, and a hidden one draws only at the
    // host's render_unfocused_fps.
    if meta(&ctx.d, "mode").0 == b"interactive" {
        return Ok(());
    }
    // (bash's `local geom; geom=$(on_box ... | layer_at ...)` under set -e and pipefail: a failure
    // of either ends up with its status)
    let l = layers(ctx, ErrTo::Inherit).or_exit()?;
    let g = layer_at(&l.out, "omarchy-bar", jq::Err::Inherit);
    let rc = if g.rc != 0 { g.rc } else { l.rc };
    if rc != 0 {
        return Err(Exit(u8::try_from(rc).unwrap_or(1)));
    }
    bar_settle(ctx, r, &g.text(), deadline)
}

/// `${S#* }`: what follows S's first space (S itself when it has none).
fn after_space(s: &[u8]) -> &[u8] {
    match s.iter().position(|&c| c == b' ') {
        Some(i) => &s[i + 1..],
        None => s,
    }
}

/// bar_settle GEOM DEADLINE: until the bar's pixels at GEOM hold still (3 identical captures), or
/// DEADLINE. A box gone meanwhile is said at once (#157, finding 216). A missed frame starts the
/// count again. (The frames themselves are compared, where bash compared their md5sums.)
pub fn bar_settle(ctx: &Ctx, r: &Ready, geom: &[u8], deadline: Instant) -> Res<()> {
    bar_settle_with(&RealBox, ctx, r, geom, deadline)
}

fn bar_settle_with<S: BoxSys + ?Sized>(
    sys: &S,
    ctx: &Ctx,
    r: &Ready,
    geom: &[u8],
    deadline: Instant,
) -> Res<()> {
    let (mut last, mut same): (Option<Vec<u8>>, u32) = (None, 0);
    while same < 3 && !past(deadline) {
        starting_ok_with(sys, ctx, r)?;
        let g = sys.on_box(ctx, &w(&[b"timeout", b"2", b"grim", b"-g", geom, b"-"]));
        if g.rc != 0 {
            same = 0;
            last = None;
            poll();
            continue;
        }
        if last.as_ref() == Some(&g.out) {
            same += 1;
        } else {
            same = 0;
            last = Some(g.out);
        }
        poll();
    }
    Ok(())
}

/// shell_crash [PID]: the shell crashed or quit as it starts (#126, finding 183): "crashed REPORT",
/// "crashed -" or "exited"; None while it has not. A duplicate of a shell another start brought up
/// meanwhile (finding 248) is adopted, said, and is no failed start.
pub fn shell_crash(ctx: &Ctx, r: &Ready, pid: Option<&[u8]>) -> Option<Vec<u8>> {
    shell_crash_with(&RealBox, ctx, r, pid)
}

fn shell_crash_with<S: BoxSys + ?Sized>(
    sys: &S,
    ctx: &Ctx,
    r: &Ready,
    pid: Option<&[u8]>,
) -> Option<Vec<u8>> {
    if let Some(c) = shell_crash_report(ctx, &r.shell_before) {
        return Some(c);
    }
    let gone = || pid.is_some_and(|p| !p.is_empty() && !shell_running_with(sys, ctx, p));
    if !shell_crash_logged(ctx) && !gone() {
        return None;
    }
    if !shell_crash_logged(ctx)
        && let Some(p) = shell_adopt_with(sys, ctx)
    {
        say(cat!(
            "box '",
            ctx.name,
            "': another start of the shell came first; that one (pid ",
            p,
            ") is up, started after the old one was stopped: the box's shell now"
        ));
        return None;
    }
    // The crash handler, another process, writes the report just after the shell's pid goes and its
    // log line. A second for it.
    for _ in 0..10 {
        poll();
        if let Some(c) = shell_crash_report(ctx, &r.shell_before) {
            return Some(c);
        }
    }
    Some(if shell_crash_logged(ctx) {
        b"crashed -".to_vec()
    } else {
        b"exited".to_vec()
    })
}

/// shell_crash_report BEFORE: a new report folder since BEFORE (SHELL_BEFORE), the last one in byte
/// order (bash's sort and comm: the locale's collation, finding 446), as "crashed REPORT" ("crashed
/// -" for a name not plain). Not one of a process older than this start (its pid at most
/// omabox-shell.mark, finding 255); a box whose shell.sh writes no mark counts every report.
pub fn shell_crash_report(ctx: &Ctx, before: &[u8]) -> Option<Vec<u8>> {
    let mark = run_read(&ctx.d, "omabox-shell.mark")
        .map(|m| strip_nl(&m))
        .filter(|m| shell::is_num(m))
        .and_then(|m| shell::dec(&m))
        .unwrap_or(0);
    let mut kept = None;
    for rep in new_lines(before, &shell_crash_dirs(ctx)) {
        if mark > 0
            && plain(&rep)
            && let Some(f) = box_home_file(
                &ctx.d,
                &String::from_utf8_lossy(&cat!(".cache/quickshell/crashes/", rep, "/report.txt")),
            )
        {
            let c = std::fs::read(f)
                .ok()
                .and_then(|t| crashed_pid(&t))
                .unwrap_or_default();
            if !c.is_empty() && shell::dec(&c).is_some_and(|c| c <= mark) {
                continue;
            }
        }
        kept = Some(rep);
    }
    let new = kept?;
    Some(if plain(&new) {
        cat!(
            "crashed ",
            ctx.d,
            "/home/.cache/quickshell/crashes/",
            new,
            "/report.txt"
        )
    } else {
        b"crashed -".to_vec()
    })
}

/// `^[A-Za-z0-9_.-]+$`, ASCII (finding 445).
fn plain(v: &[u8]) -> bool {
    !v.is_empty()
        && v.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
}

/// `comm -13 <(sort <<<"$OLD") <(sort <<<"$NOW")`: NOW's lines that OLD has not (once for each one
/// more than OLD has), in byte order.
fn new_lines(old: &[u8], now: &[u8]) -> Vec<Vec<u8>> {
    let lines = |v: &[u8]| -> Vec<Vec<u8>> {
        let mut l: Vec<Vec<u8>> = strip_nl(v)
            .split(|&c| c == b'\n')
            .map(<[u8]>::to_vec)
            .filter(|l| !l.is_empty())
            .collect();
        l.sort();
        l
    };
    let (mut o, n) = (lines(old), lines(now));
    let mut out = Vec::new();
    for x in n {
        if let Some(i) = o.iter().position(|y| *y == x) {
            o.remove(i);
        } else {
            out.push(x);
        }
    }
    out
}

/// A report's first `Crashed process ID: N` line's N (all digits, maybe none).
fn crashed_pid(t: &[u8]) -> Option<Vec<u8>> {
    t.split(|&c| c == b'\n').find_map(|l| {
        let n = l.strip_prefix(b"Crashed process ID: ")?;
        n.iter().all(u8::is_ascii_digit).then(|| n.to_vec())
    })
}

/// shell_crash_logged: shell.log says "Quickshell has crashed".
pub fn shell_crash_logged(ctx: &Ctx) -> bool {
    box_home_file(&ctx.d, "shell.log")
        .and_then(|f| std::fs::read(f).ok())
        .is_some_and(|t| crate::exit::contains(&t, b"Quickshell has crashed"))
}

/// shell_adopt (finding 248): this start's shell exited at once as a duplicate of one that something
/// else started after shell.sh stopped the old one, and that one is up: its pid goes where shell.sh
/// keeps the shell's, written in the box, and is returned.
fn shell_adopt_with<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx) -> Option<Vec<u8>> {
    let t = std::fs::read(box_home_file(&ctx.d, "shell.log")?).ok()?;
    if !crate::exit::contains(&t, b"An instance of this configuration is already running") {
        return None;
    }
    let tree = meta(&ctx.d, "omarchy").0;
    let cfg = cat!(
        if tree.is_empty() {
            b"/usr/share/omarchy".as_slice()
        } else {
            &tree
        },
        "/shell"
    );
    let qs = crate::shellnote::box_qs(ctx);
    let l = sys.on_box(
        ctx,
        &w(&[&qs, b"list", b"-j", b"-p", &cfg, b"--any-display"]),
    );
    let p = if l.rc == 0 {
        let j = jq::jq(&["-r", ADOPT_JQ], Some(&l.out), jq::Err::Null);
        if j.ok() { j.text() } else { Vec::new() }
    } else {
        Vec::new()
    };
    if !shell::is_num(&p) || !shell_running_with(sys, ctx, &p) {
        return None;
    }
    let wr = sys.on_box_io(
        ctx,
        &w(&[b"sh", b"-c", ADOPT_SH.as_bytes(), b"sh", &p]),
        Out::Inherit,
        ErrTo::Inherit,
    );
    (wr.rc == 0).then_some(p)
}

/// shell_crash_text WHEN HOW: shell_crash's answer in words. A box on a Quickshell build (finding
/// 358) says which.
pub fn shell_crash_text(ctx: &Ctx, when: &str, how: &[u8]) -> Vec<u8> {
    crash_text(&ctx.name, &meta(&ctx.d, "quickshell").0, when, how)
}

fn crash_text(name: &[u8], qs: &[u8], when: &str, how: &[u8]) -> Vec<u8> {
    let qs = if qs.is_empty() {
        Vec::new()
    } else {
        cat!(" on the Quickshell build ", qs)
    };
    let log = cat!("omabox log -b ", name, " shell");
    if how == b"crashed -" {
        cat!(
            "warning: the shell crashed",
            qs,
            " ",
            when,
            " starting (",
            log,
            ")"
        )
    } else if let Some(rep) = how.strip_prefix(b"crashed ") {
        cat!(
            "warning: the shell crashed",
            qs,
            " ",
            when,
            " starting (report: ",
            rep,
            "; ",
            log,
            ")"
        )
    } else {
        cat!(
            "warning: the shell exited",
            qs,
            " ",
            when,
            " starting (",
            log,
            ")"
        )
    }
}

/// shell_watch: once its bar is on screen, the shell watched until 2 s after the bar came
/// (R.bar_at), as a crash there went unsaid (#126). The warning, when it went.
pub fn shell_watch(ctx: &Ctx, r: &Ready) -> Option<Vec<u8>> {
    loop {
        if let Some(how) = shell_crash(ctx, r, shell_pid(ctx).as_deref()) {
            return Some(shell_crash_text(ctx, "after", &how));
        }
        if mono_ms() >= r.bar_at.saturating_add(2000) {
            return None;
        }
        poll();
    }
}

/// more_screens MAIN (#308): the box's other monitors as up's line has the main one, each after
/// " + "; empty when it has none.
pub fn more_screens(ctx: &Ctx, main: &[u8]) -> Vec<u8> {
    let Ok(m) = crate::hypr::hypr_json(ctx, &["monitors"]) else {
        return Vec::new();
    };
    Args::new(&["-r", "--arg", "main"])
        .arg(main)
        .arg(MORE_SCREENS_JQ)
        .run(Some(&m), jq::Err::Null)
        .text()
}

/// client_render_check (#204, finding 290): eglinfo in the box's session as an app gets it, its
/// verdict kept in box.json's client_render for `ls` and `gpu`, and said once when it is software.
/// No eglinfo or no renderer named: nothing kept, nothing said.
pub fn client_render_check(ctx: &mut Ctx) -> Res<()> {
    let argv = w(&[
        b"/usr/bin/timeout",
        b"-k",
        b"1",
        b"5",
        b"/usr/bin/eglinfo",
        b"-B",
        b"-p",
        b"wayland",
    ]);
    let out = box_exec_out(ctx, &argv, true, Some(65_536))
        .map(|(_, o)| o)
        .unwrap_or_default();
    client_render_from(ctx, &out, None)
}

/// client_render_check on eglinfo's output OUT (empty: none ran), OTHER as soft_note takes it: a
/// #[test] gives both, where the suite stubbed box_exec and other_gpu.
fn client_render_from(ctx: &Ctx, out: &[u8], other: Option<&[u8]>) -> Res<()> {
    let rec = egl_render(&shell::subst(out));
    if rec.is_empty() {
        return Ok(());
    }
    let bj = ctx.at("box.json");
    let tmp = ctx.at("box.json.tmp");
    let j = Args::new(&["--argjson", "r"])
        .arg(&rec)
        .arg(CLIENT_RENDER_JQ)
        .arg(shell::bytes(&bj))
        .run_file(jq::Err::Inherit);
    if j.ok() && std::fs::write(&tmp, &j.out).is_ok() {
        let _ = std::fs::rename(&tmp, &bj);
    } else {
        // (bash left its box.json.tmp there: none left, finding 443's rule)
        let _ = std::fs::remove_file(&tmp);
    }
    let driver = Args::new(&["-r", RENDER_DRIVER_JQ])
        .arg(shell::bytes(&bj))
        .run_file(jq::Err::Inherit)
        .text();
    let note = soft_note(&rec, &driver, other);
    if !note.is_empty() {
        say(cat!("box '", ctx.name, "': ", note));
    }
    Ok(())
}

/// egl_render: eglinfo's output as client_render's record, or empty when it names no renderer.
pub fn egl_render(out: &[u8]) -> Vec<u8> {
    let r = jq::awk(&[EGL_AWK], Some(out), jq::Err::Null).text();
    if r.is_empty() {
        return Vec::new();
    }
    Args::new(&["-nc", "--arg", "r"])
        .arg(&r)
        .arg(EGL_JQ)
        .run(None, jq::Err::Null)
        .text()
}

/// soft_note RECORD DRIVER [OTHER]: what up says once when RECORD is software, empty else. DRIVER:
/// the box's GPU's ("" for an interactive box); OTHER as other_gpu gives it, looked up when None.
pub fn soft_note(rec: &[u8], driver: &[u8], other: Option<&[u8]>) -> Vec<u8> {
    let r = jq::jq(&["-r", SOFT_JQ], Some(rec), jq::Err::Null);
    let r = if r.ok() { r.text() } else { Vec::new() };
    if r.is_empty() {
        return Vec::new();
    }
    let why: &[u8] = if driver == b"nvidia" {
        b" (NVIDIA's EGL did not initialise in it, so Mesa fell back to software)"
    } else {
        b""
    };
    let cut = match r.windows(2).position(|x| x == b" (") {
        Some(i) => &r[..i],
        None => &r[..],
    };
    let mut m = cat!(
        "its apps render on ",
        cut,
        ", in software",
        why,
        ": CPU and frame cost measured in it are software rendering's, and omabox gpu counts none of their drawing; "
    );
    if driver.is_empty() {
        m.extend_from_slice(b"compare against a baseline measured in this same box");
        return m;
    }
    let other = match other {
        Some(o) => o.to_vec(),
        None => crate::shots::other_gpu(driver),
    };
    if other.is_empty() {
        m.extend_from_slice(
            b"this machine has no other GPU: compare against a baseline measured in this same box",
        );
    } else {
        let n = other.split(|&c| c == b' ').next().unwrap_or_default();
        m.extend(cat!(
            "for GPU rendering, a box on another GPU: OMABOX_RENDER_NODE=/dev/dri/",
            n,
            " omabox up --new (",
            n,
            " is ",
            after_space(&other),
            ")"
        ));
    }
    m
}

/// box_theme: the box's theme name (its theme.name's first line), empty when the file is not one to
/// read or the name not a plain one (finding 159).
pub fn box_theme(ctx: &Ctx) -> Vec<u8> {
    box_home_file(&ctx.d, ".local/state/omarchy/current/theme.name")
        .and_then(|f| std::fs::read(f).ok())
        .map(|t| theme_line(&t))
        .unwrap_or_default()
}

/// `IFS= read -r -n 65` then `^[A-Za-z0-9 ._-]{1,64}$` (ASCII, finding 445): the first line's name.
fn theme_line(t: &[u8]) -> Vec<u8> {
    // (read drops NUL bytes; -n 65 stops after 65 characters, past what a name may be)
    let l: Vec<u8> = t
        .split(|&c| c == b'\n')
        .next()
        .unwrap_or_default()
        .iter()
        .copied()
        .filter(|&c| c != 0)
        .take(65)
        .collect();
    let ok = (1..=64).contains(&l.len())
        && l.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b' ' | b'.' | b'_' | b'-'));
    if ok { l } else { Vec::new() }
}

/// box_orphan (#220): the selected box, up, is headless with no idle limit and no owner or agent,
/// started by a process up recorded (not a person at a terminal) that is gone.
pub fn box_orphan(ctx: &Ctx) -> bool {
    let s = Args::new(&["-r", ORPHAN_JQ])
        .arg(cat!(ctx.d, "/box.json"))
        .run_file(jq::Err::Null);
    let s = s.text();
    if s.is_empty() {
        return false;
    }
    let (pid, start) = match s.iter().position(|&c| c == b' ') {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (&s[..], &s[..]),
    };
    !owner_alive(pid, start)
}

/// box_json [HUNG [SHELL [GONE]]]: the selected box as one line of JSON, as `ls --json` lists it and
/// `up --json` prints it (its newline too). HUNG: hung_boxes named it; SHELL: shell_state's hung or
/// ok when asked already ("" asks); GONE: a Hyprland gone (crashed, exited, going: finding 350), its
/// state.
pub fn box_json(ctx: &Ctx, hung: bool, shell_given: &str, gone: &str) -> Vec<u8> {
    let mut state = box_state(&ctx.d).to_string();
    let mut hung = hung;
    if !gone.is_empty() && state == "up" {
        state = gone.to_string();
        hung = false;
    }
    let peeking = crate::marks::peek_open(ctx);
    let keys = ctx.at("keys-to-box").is_file();
    let theme = box_theme(ctx);
    let (sh, crashes) = if state == "up" {
        (
            crate::shellnote::shell_state_given(ctx, shell_given),
            shell_crashes(ctx),
        )
    } else {
        ("none", 0)
    };
    let orphan = state == "up" && box_orphan(ctx);
    let tf = |b: bool| if b { "true" } else { "false" };
    let j = Args::new(&["-c"])
        .kv("--arg", "name", &ctx.name)
        .kv("--arg", "state", &state)
        .kv("--argjson", "peeking", tf(peeking))
        .kv("--argjson", "keys", tf(keys))
        .kv("--arg", "theme", &theme)
        .kv("--argjson", "hung", tf(hung))
        .kv("--arg", "sh", sh)
        .kv("--argjson", "crashes", crashes.to_string())
        .kv("--argjson", "orphan", tf(orphan))
        .arg(BOX_JSON_JQ)
        .arg(cat!(ctx.d, "/box.json"))
        .run_file(jq::Err::Null);
    if j.ok() {
        return j.out;
    }
    Args::new(&["-nc"])
        .kv("--arg", "name", &ctx.name)
        .kv("--arg", "state", &state)
        .arg(BOX_JSON_BARE_JQ)
        .run(None, jq::Err::Inherit)
        .out
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    // wait layer (finding 146; t_unit_wait's): a hidden menu mapped at 1x1 is not the menu.
    #[test]
    fn layer_at_as_bash() {
        let lj = r#"{"HEADLESS-1":{"levels":{"2":[{"namespace":"omarchy-bar","x":0,"y":0,"w":1920,"h":34}],
    "3":[{"namespace":"omarchy-menu","x":0,"y":0,"w":1,"h":1},{"namespace":"omarchy-osd","x":0,"y":0,"w":0,"h":0}]}}}"#;
        let at = |l: &str, ns: &str| {
            String::from_utf8(layer_at(l.as_bytes(), ns, jq::Err::Inherit).text()).unwrap()
        };
        assert_eq!(
            at(lj, "omarchy-bar"),
            "0,0 1920x34",
            "wait layer: a drawn layer is there"
        );
        assert_eq!(at(lj, "omarchy-menu"), "", "...a hidden 1x1 one is not");
        assert_eq!(at(lj, "omarchy-osd"), "", "...nor an empty one");
        let both = lj.replace(
            r#""w":1,"h":1}"#,
            r#""w":1,"h":1},{"namespace":"omarchy-menu","x":660,"y":300,"w":600,"h":480}"#,
        );
        assert_eq!(
            at(&both, "omarchy-menu"),
            "660,300 600x480",
            "...the drawn one when both are mapped"
        );
    }

    /// bash's stubs for up's waits: box_alive's answer, on_box failing (no frame), kill_box's
    /// answer, rm_box saying it ran; box_pid and box_pasta given (kill_box's own test).
    #[derive(Default)]
    struct Fake {
        alive: bool,
        killed: bool,
        pids: Option<(Vec<u8>, Vec<u8>)>,
        log: Mutex<Vec<String>>,
    }

    impl BoxSys for Fake {
        fn box_alive(&self, _: &Ctx) -> Res<bool> {
            Ok(self.alive)
        }
        fn on_box(&self, _: &Ctx, _: &[Vec<u8>]) -> crate::exec::Ran {
            crate::exec::Ran {
                rc: 1,
                out: Vec::new(),
                refused: false,
                gone_said: false,
            }
        }
        fn kill_box(&self, ctx: &Ctx) -> bool {
            if self.pids.is_some() {
                return kill_box_with(self, ctx);
            }
            self.killed
        }
        fn rm_box(&self, _: &Ctx) {
            self.log.lock().unwrap().push("removed".into());
        }
        fn box_pid(&self, d: &[u8]) -> BoxPid {
            match &self.pids {
                Some((one, _)) => BoxPid::Pid(one.clone()),
                None => crate::boxes::box_pid(d),
            }
        }
        fn box_pasta(&self, d: &[u8]) -> Option<Vec<u8>> {
            match &self.pids {
                Some((_, pasta)) => Some(pasta.clone()),
                None => crate::boxes::box_pasta(d),
            }
        }
    }

    fn uctx(tag: &str) -> Ctx {
        let b = std::env::temp_dir().join(format!("omabox-ready-{tag}-{}", std::process::id()));
        let b = shell::bytes(&b).to_vec();
        let mut c = Ctx::new(b"up", b.clone());
        c.parsed();
        c.name = b"u".to_vec();
        c.d = cat!(b, "/uo");
        c
    }

    fn ready(owner: Option<Owner>) -> Ready<'static> {
        Ready {
            owner,
            covers: false,
            shell_before: Vec::new(),
            bar_at: 0,
            before_end: &|| {},
        }
    }

    /// t_unit_up_owner_gone (#222): an up whose `omabox run` is gone stops at once, says so and
    /// removes the box; one whose PID 1 will not go leaves it, dead, to the reaper; one whose run is
    /// still there goes on.
    #[test]
    fn up_stops_when_its_run_is_gone() {
        let me = std::process::id().to_string().into_bytes();
        let gone = || {
            Some(Owner {
                pid: me.clone(),
                start: b"not-its-start".to_vec(),
            })
        };
        let line = format!(
            "omabox: box 'u': its omabox run ({}) is gone: not starting it",
            std::process::id()
        );
        let ctx = uctx("og");
        let f = Fake {
            alive: true,
            killed: true,
            ..Fake::default()
        };
        let t0 = Instant::now();
        let (r, said) = crate::exit::heard(|| {
            bar_settle_with(
                &f,
                &ctx,
                &ready(gone()),
                b"0,0 10x10",
                Instant::now() + Duration::from_secs(15),
            )
        });
        assert_eq!(r, Err(Exit(1)), "an up whose run is gone stops: exit 1");
        assert_eq!(said, format!("{line}\n"), "...said");
        assert_eq!(*f.log.lock().unwrap(), ["removed"], "...the box removed");
        assert!(
            t0.elapsed() <= Duration::from_secs(3),
            "...at once, not at the deadline"
        );
        let f = Fake {
            alive: true,
            killed: false,
            ..Fake::default()
        };
        let (r, said) = crate::exit::heard(|| wait_hyprland_with(&f, &ctx, &ready(gone())));
        assert_eq!(
            (r, said),
            (
                Err(Exit(1)),
                format!("{line} (left, dead, to its reaper)\n")
            ),
            "...a PID 1 that will not go: the dead box left to the reaper"
        );
        assert!(f.log.lock().unwrap().is_empty());
        let mine = Owner {
            pid: me.clone(),
            start: crate::procfs::proc_start(&me).unwrap(),
        };
        let f = Fake {
            alive: true,
            ..Fake::default()
        };
        let r = bar_settle_with(
            &f,
            &ctx,
            &ready(Some(mine)),
            b"0,0 10x10",
            Instant::now() + Duration::from_secs(1),
        );
        assert_eq!(r, Ok(()), "...its run still there: up goes on");
    }

    /// t_unit_up_dies_late (#157, finding 216): bar_settle on a box that died says so at once.
    #[test]
    fn bar_settle_on_a_dead_box() {
        let ctx = uctx("dl");
        let t0 = Instant::now();
        let (r, said) = crate::exit::heard(|| {
            bar_settle_with(
                &Fake::default(),
                &ctx,
                &ready(None),
                b"0,0 10x10",
                Instant::now() + Duration::from_secs(15),
            )
        });
        assert_eq!(r, Err(Exit(1)), "bar_settle on a dead box: exit 1");
        assert!(
            said.contains("box 'u' died while starting"),
            "...said: died while starting: {said}"
        );
        assert!(
            t0.elapsed() <= Duration::from_secs(3),
            "...at once, not at the deadline"
        );
    }

    /// labwc's log on a render node that renders in software only (#396: a VM's virtio-gpu without
    /// virgl; these lines from a box whose Mesa GBM was forced to software): wlroots refuses the
    /// software EGL device, labwc goes on with pixman, and the box's Hyprland aborts.
    const LABWC_SOFT: &[u8] = b"00:00:00.167 [ERROR] [render/egl.c:334] Software rendering detected, please use the WLR_RENDERER_ALLOW_SOFTWARE environment variable to proceed\n00:00:00.167 [ERROR] [render/egl.c:611] Failed to initialize EGL context\n00:00:00.167 [ERROR] [render/gles2/renderer.c:499] Could not initialize EGL\nWelcome to Hyprland!\nterminate called after throwing an instance of 'std::runtime_error'\n  what():  CBackend::create() failed!\n00:00:00.237 [ERROR] [../labwc/src/server.c:207] spawned child 60 terminated with signal 6 (Aborted)\n";
    /// ...and as an EGL that does not initialise on the node does (Mesa's EGL alone on an NVIDIA
    /// GPU, whose GBM is NVIDIA's).
    const LABWC_NO_EGL: &[u8] = b"00:00:00.150 [ERROR] [EGL] command: eglInitialize, error: EGL_NOT_INITIALIZED (0x3001), message: \"DRI2: gbm device using incorrect/incompatible backend\"\n00:00:00.150 [ERROR] [EGL] command: eglInitialize, error: EGL_NOT_INITIALIZED (0x3001), message: \"eglInitialize\"\n00:00:00.150 [ERROR] [render/egl.c:269] Failed to initialize EGL\n00:00:00.150 [ERROR] [render/gles2/renderer.c:499] Could not initialize EGL\n00:00:00.189 [ERROR] [render/vulkan/renderer.c:2608] Could not match drm and vulkan device\n  what():  CBackend::create() failed!\n";

    /// t_unit_up_software (#396, finding 568): a box that died because its labwc could not render
    /// says so, with the node, not only "died while starting".
    #[test]
    fn died_starting_says_a_software_render_node() {
        let ctx = uctx("sw");
        let home = cat!(ctx.d, "/home");
        std::fs::create_dir_all(shell::path(&home)).unwrap();
        std::fs::write(shell::path(&cat!(home, "/labwc.log")), LABWC_SOFT).unwrap();
        std::fs::write(
            ctx.at("box.json"),
            r#"{"render":{"node":"/dev/dri/renderD128","driver":"virtio_gpu"}}"#,
        )
        .unwrap();
        let (e, said) = crate::exit::heard(|| died_starting(&ctx, false));
        assert_eq!(e, Exit(1));
        let want = "omabox: box 'u' died while starting (a box needs GPU rendering: /dev/dri/renderD128 renders in software only (no 3D acceleration); in a VM, virtio-gpu with virgl (QEMU's virtio-vga-gl, libvirt's accel3d with an OpenGL display)";
        assert!(said.starts_with(want), "the reason and the node: {said}");
        assert!(
            said.ends_with(&format!(
                "); logs: {d}/home/*.log {d}/box.log\n",
                d = String::from_utf8_lossy(&ctx.d)
            )),
            "...and the logs, as before: {said}"
        );
        // No labwc.log (an interactive box) or none of the two lines: said as before.
        std::fs::write(shell::path(&cat!(home, "/labwc.log")), b"hl\n").unwrap();
        let (_, said) = crate::exit::heard(|| died_starting(&ctx, false));
        assert!(
            said.starts_with("omabox: box 'u' died while starting; logs: "),
            "{said}"
        );
        let _ = std::fs::remove_dir_all(shell::path(&ctx.d));
    }

    /// t_unit_up_software: render_why's words for each log, node and other GPU.
    #[test]
    fn render_why_cases() {
        let s = |v: Vec<u8>| String::from_utf8(v).unwrap();
        assert_eq!(
            s(render_why_from(
                LABWC_SOFT,
                b"/dev/dri/renderD128",
                b"virtio_gpu",
                b""
            )),
            format!(
                "a box needs GPU rendering: /dev/dri/renderD128 renders in software only (no 3D acceleration); {}",
                String::from_utf8_lossy(VIRGL_HINT)
            ),
            "virtio-gpu without virgl, no other GPU"
        );
        assert_eq!(
            s(render_why_from(
                LABWC_SOFT,
                b"/dev/dri/renderD128",
                b"virtio_gpu",
                b"renderD129 nvidia"
            )),
            format!(
                "a box needs GPU rendering: /dev/dri/renderD128 renders in software only (no 3D acceleration); {}; a box on another GPU: OMABOX_RENDER_NODE=/dev/dri/renderD129 omabox up --new (renderD129 is nvidia)",
                String::from_utf8_lossy(VIRGL_HINT)
            ),
            "...beside a real GPU: that one named"
        );
        assert_eq!(
            s(render_why_from(
                LABWC_NO_EGL,
                b"/dev/dri/renderD128",
                b"amdgpu",
                b""
            )),
            "a box needs GPU rendering: labwc found no EGL on /dev/dri/renderD128 (EGL: DRI2: gbm device using incorrect/incompatible backend)",
            "no EGL on the node: EGL's first reason, no VM hint on a real GPU"
        );
        assert_eq!(
            s(render_why_from(
                b"00:00:00.141 [ERROR] [render/egl.c:208] EGL_EXT_platform_base not supported\n00:00:00.141 [ERROR] [render/gles2/renderer.c:499] Could not initialize EGL\n",
                b"",
                b"",
                b""
            )),
            "a box needs GPU rendering: labwc found no EGL on its render node",
            "...no EGL at all (t_up_software's): no EGL message; no node in box.json"
        );
        assert_eq!(
            s(render_why_from(
                b"[ERROR] [types/wlr_linux_dmabuf_v1.c:1130] Failed to get backend DRM FD\n",
                b"/dev/dri/renderD128",
                b"amdgpu",
                b""
            )),
            "",
            "a box that renders: nothing"
        );
    }

    /// t_unit_kill_box: kill_box runs every step, PID 1 and pasta killed, then waited for.
    /// (#[ignore]: it starts and kills processes; test/run.sh's unit tier runs it.)
    #[test]
    #[ignore = "starts processes: run by test/run.sh (cargo test -- --ignored)"]
    fn kill_box_ends_pid1_and_pasta() {
        // (children of a shell that exits: init reaps them, as it reaps a box's)
        let o = std::process::Command::new("sh")
            .args([
                "-c",
                "sleep 300 >/dev/null 2>&1 & a=$!; sleep 300 >/dev/null 2>&1 & echo \"$a $!\"",
            ])
            .output()
            .unwrap();
        let o = String::from_utf8(o.stdout).unwrap();
        let (one, pasta) = o.trim().split_once(' ').unwrap();
        let f = Fake {
            pids: Some((one.as_bytes().to_vec(), pasta.as_bytes().to_vec())),
            ..Fake::default()
        };
        let ctx = uctx("kb");
        let alive = |p: &str| signal_ok(p.as_bytes(), 0);
        assert!(alive(one) && alive(pasta));
        assert!(f.kill_box(&ctx), "kill_box runs to its end");
        assert!(!alive(one), "...PID 1 gone");
        let end = Instant::now() + Duration::from_secs(5);
        while alive(pasta) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(pasta), "...pasta gone");
    }

    /// t_unit_rm_tree (#264): rm_box takes the box dir and its HOME, read-only dirs and all;
    /// sweep_homes a gone box's HOME too, never through a link; a removal that still fails is
    /// said, with rm's first complaint.
    #[test]
    fn rm_tree_as_bash() {
        let r = std::env::temp_dir().join(format!("omabox-rmt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&r);
        let r = r.to_str().unwrap().to_string();
        let mode = |p: &str, m: u32| {
            std::fs::set_permissions(p, std::os::unix::fs::PermissionsExt::from_mode(m)).unwrap();
        };
        let mk = |b: &str| {
            let (run, home) = (
                format!("{r}/run/omabox/{b}/run/x"),
                format!("{r}/cache/omabox/{b}/home/locked/dest"),
            );
            std::fs::create_dir_all(&run).unwrap();
            std::fs::create_dir_all(&home).unwrap();
            std::fs::write(format!("{home}/f"), "").unwrap();
            mode(&home, 0o555);
            mode(&format!("{r}/cache/omabox/{b}/home/locked"), 0o555);
            mode(&run, 0o000);
        };
        mk("b1");
        mk("b2");
        mode(&format!("{r}/run/omabox/b2/run/x"), 0o755);
        std::fs::remove_dir_all(format!("{r}/run/omabox/b2")).unwrap(); // b2: a box that is gone, its HOME left
        std::fs::create_dir_all(format!("{r}/outside/keep")).unwrap();
        mode(&format!("{r}/outside/keep"), 0o555);
        std::os::unix::fs::symlink(format!("{r}/outside"), format!("{r}/cache/omabox/b3")).unwrap();
        // (b5: a box whose HOME is a link, given to rm_tree itself)
        std::fs::create_dir_all(format!("{r}/run/omabox/b5")).unwrap();
        std::os::unix::fs::symlink(format!("{r}/outside"), format!("{r}/cache/omabox/b5")).unwrap();
        let mut ctx = Ctx::new(b"down", format!("{r}/run/omabox").into_bytes());
        ctx.name = b"b1".to_vec();
        ctx.d = format!("{r}/run/omabox/b1").into_bytes();
        let mut ctx5 = Ctx::new(b"down", format!("{r}/run/omabox").into_bytes());
        ctx5.name = b"b5".to_vec();
        ctx5.d = format!("{r}/run/omabox/b5").into_bytes();
        let env = [("XDG_CACHE_HOME", Some(format!("{r}/cache")))];
        let env: Vec<(&str, Option<&str>)> = env.iter().map(|(k, v)| (*k, v.as_deref())).collect();
        let ((), said) = crate::exit::heard(|| {
            shell::with_env(&env, || {
                rm_box(&ctx);
                rm_box(&ctx5);
                sweep_homes(&ctx);
            })
        });
        let left: Vec<bool> = [
            "run/omabox/b1",
            "cache/omabox/b1",
            "cache/omabox/b2",
            "run/omabox/b5",
        ]
        .iter()
        .map(|p| std::path::Path::new(&format!("{r}/{p}")).exists())
        .collect();
        assert_eq!(
            (said.as_str(), left),
            ("", vec![false, false, false, false]),
            "rm_box: the box dir and its HOME go, read-only dirs and all; sweep_homes: a gone box's too"
        );
        assert!(
            std::fs::symlink_metadata(format!("{r}/cache/omabox/b5")).is_err(),
            "...a HOME that is a link: the link goes"
        );
        let keep = std::fs::metadata(format!("{r}/outside/keep")).unwrap();
        assert!(
            keep.permissions().readonly(),
            "...a link in the cache is not followed (what it points to keeps its modes)"
        );
        // Finding 584: a HOME whose record names a box dir of another runtime dir that is there (b7) is
        // that box's: kept by sweep_homes and by this runtime dir's rm_box of the same name, which up
        // refuses; one whose record names a gone box dir (b8) goes, though this runtime dir has a box
        // dir of its name; the box's own (b9: its record names it) goes with it.
        for (b, made_in) in [("b7", "run2"), ("b8", "gone"), ("b9", "run")] {
            std::fs::create_dir_all(format!("{r}/cache/omabox/{b}/home")).unwrap();
            let d = format!("{r}/{made_in}/omabox/{b}").into_bytes();
            assert!(crate::homeuse::write_record(
                format!("{r}/cache/omabox/{b}").as_bytes(),
                &d
            ));
        }
        std::fs::create_dir_all(format!("{r}/run2/omabox/b7")).unwrap();
        std::fs::create_dir_all(format!("{r}/run/omabox/b8")).unwrap();
        std::fs::create_dir_all(format!("{r}/run/omabox/b9")).unwrap();
        let at = |b: &str| {
            let mut c = Ctx::new(b"up", format!("{r}/run/omabox").into_bytes());
            c.name = b.as_bytes().to_vec();
            c.d = format!("{r}/run/omabox/{b}").into_bytes();
            c
        };
        let (c7, c9) = (at("b7"), at("b9"));
        let taken = shell::with_env(&env, || {
            sweep_homes(&ctx);
            let t = home_taken(&c7);
            rm_box(&c7);
            rm_box(&c9);
            t
        });
        let there = |p: &str| std::path::Path::new(&format!("{r}/{p}")).exists();
        assert_eq!(
            (
                there("cache/omabox/b7/home"),
                there("cache/omabox/b8"),
                there("cache/omabox/b9")
            ),
            (true, false, false),
            "a HOME of a box in another runtime dir is kept; one whose box is gone where it was made goes"
        );
        assert_eq!(
            taken,
            Some(format!("is a box's in {r}/run2/omabox").into_bytes()),
            "...and up's name there is refused, saying where that box is"
        );
        // (a chmod as coreutils' before 9.5, Ubuntu 24.04's, which refuses -P: finding 524)
        std::fs::create_dir_all(format!("{r}/old")).unwrap();
        std::fs::write(
            format!("{r}/old/chmod"),
            "#!/bin/sh\nfor a; do [ \"$a\" != -P ] || { echo \"chmod: invalid option -- 'P'\" >&2; exit 1; }; done\nexec chmod \"$@\"\n",
        )
        .unwrap();
        mode(&format!("{r}/old/chmod"), 0o755);
        mk("b6");
        let gone = crate::exit::heard(|| {
            rm_tree_with(
                &format!("{r}/old/chmod"),
                b"box 'b6'",
                &[
                    format!("{r}/run/omabox/b6").into_bytes(),
                    format!("{r}/cache/omabox/b6").into_bytes(),
                ],
            )
        });
        assert_eq!(
            (
                gone,
                std::path::Path::new(&format!("{r}/cache/omabox/b6")).exists()
            ),
            ((true, String::new()), false),
            "...with a chmod that has no -P too"
        );
        // (rm's own complaint: HOMEs' folder not writable, so the HOME itself cannot go; not as root)
        if crate::procfs::euid() != 0 {
            mk("b4");
            mode(&format!("{r}/run/omabox/b4/run/x"), 0o755);
            std::fs::remove_dir_all(format!("{r}/run/omabox/b4")).unwrap();
            mode(&format!("{r}/cache/omabox"), 0o555);
            let ((), said) = crate::exit::heard(|| shell::with_env(&env, || sweep_homes(&ctx)));
            // (omabox's words, then rm's first complaint, which names the HOME: rm's own words are
            // in the machine's language, so only its name and the path are checked)
            let head = "omabox: could not remove all of the HOME of box 'b4', which is gone: rm: ";
            assert!(
                said.starts_with(head)
                    && said.contains(&format!("'{r}/cache/omabox/b4'"))
                    && said.ends_with('\n')
                    && said.lines().count() == 1,
                "...a removal that still fails: said, with rm's first complaint: {said}"
            );
            mode(&format!("{r}/cache/omabox"), 0o755);
        }
        let _ = std::process::Command::new("chmod")
            .args(["-R", "u+rwX", &r])
            .status();
        std::fs::remove_dir_all(&r).unwrap();
    }

    /// t_unit_omarchy45's intro_cover cases: an Overlay omarchy-background layer's geometry; the
    /// wallpaper alone (Background level) is no cover. jq is the recipe's dependency.
    #[test]
    fn intro_cover_as_bash() {
        let cover = |layers: &str| {
            let r = crate::jq::jq(
                &["-er", INTRO_COVER_JQ],
                Some(layers.as_bytes()),
                crate::jq::Err::Null,
            );
            (r.rc, String::from_utf8(r.text()).unwrap())
        };
        assert_eq!(
            cover(
                r#"{"M":{"levels":{"0":[{"namespace":"omarchy-background","x":0,"y":0,"w":800,"h":600}],"3":[{"namespace":"omarchy-background","x":0,"y":0,"w":800,"h":600}]}}}"#
            ),
            (0, "0,0 800x600".to_string())
        );
        assert_ne!(
            cover(r#"{"M":{"levels":{"0":[{"namespace":"omarchy-background","x":0,"y":0,"w":800,"h":600}],"3":[{"namespace":"omarchy-notifications"}]}}}"#).0,
            0
        );
    }

    #[test]
    fn crash_words() {
        assert_eq!(
            crash_text(b"b", b"", "while", b"crashed -"),
            b"warning: the shell crashed while starting (omabox log -b b shell)"
        );
        assert_eq!(
            crash_text(b"b", b"/q/qs", "after", b"crashed /d/report.txt"),
            b"warning: the shell crashed on the Quickshell build /q/qs after starting (report: /d/report.txt; omabox log -b b shell)"
        );
        assert_eq!(
            crash_text(b"b", b"", "while", b"exited"),
            b"warning: the shell exited while starting (omabox log -b b shell)"
        );
        // (t_unit_shell_crash's three)
        assert_eq!(
            crash_text(b"u", b"", "after", b"crashed /r/report.txt"),
            b"warning: the shell crashed after starting (report: /r/report.txt; omabox log -b u shell)"
        );
        assert_eq!(
            crash_text(b"u", b"", "while", b"crashed -"),
            b"warning: the shell crashed while starting (omabox log -b u shell)"
        );
        assert_eq!(
            crash_text(b"u", b"", "after", b"exited"),
            b"warning: the shell exited after starting (omabox log -b u shell)"
        );
    }

    /// t_unit_shell_crash's stand-in box: what /proc/PID/stat says in it (STAT, "" for no such
    /// process); with LIST, `quickshell list` answers it, pid 77 is up, and the adopt's write puts
    /// its pid where shell.sh keeps it (finding 248).
    struct ShellBox {
        stat: &'static str,
        list: Option<&'static str>,
    }

    impl BoxSys for ShellBox {
        fn on_box_io(
            &self,
            ctx: &Ctx,
            args: &[Vec<u8>],
            _: crate::exec::Out,
            _: crate::exec::ErrTo,
        ) -> crate::exec::Ran {
            let ran = |rc: i32, out: &[u8]| crate::exec::Ran {
                rc,
                out: out.to_vec(),
                refused: false,
                gone_said: false,
            };
            let a = |i: usize| args.get(i).map_or(&b""[..], |v| v.as_slice());
            let Some(list) = self.list else {
                return if self.stat.is_empty() {
                    ran(1, b"")
                } else {
                    ran(0, &cat!(self.stat, "\n"))
                };
            };
            match (a(0), a(1)) {
                (b"/usr/bin/quickshell", b"list") => ran(0, &cat!(list, "\n")),
                (b"cat", b"/proc/77/stat") => ran(0, b"77 (quickshell) S 1\n"),
                (b"sh", b"-c") => {
                    std::fs::write(
                        shell::path(&cat!(ctx.d, "/run/omabox-shell.pid")),
                        cat!(a(4), "\n"),
                    )
                    .unwrap();
                    ran(0, b"")
                }
                _ => ran(1, b""),
            }
        }
    }

    // t_unit_shell_crash (#126, findings 183, 248, 255, 258; #303): shell_pid's file, and how up
    // tells a shell that crashed as it started.
    #[test]
    #[ignore = "runs awk: run by test/run.sh (cargo test -- --ignored)"]
    fn shell_crash_as_bash() {
        let mut ctx = uctx("shcrash");
        let d = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(&ctx.d)).to_path_buf();
        let _ = std::fs::remove_dir_all(&d);
        let cr = d.join("home/.cache/quickshell/crashes");
        std::fs::create_dir_all(cr.join("old")).unwrap();
        std::fs::create_dir_all(d.join("run")).unwrap();
        std::fs::write(d.join("home/shell.log"), "").unwrap();
        std::fs::write(d.join("run/omabox-shell.pid"), "42\n").unwrap();
        ctx.name = b"u".to_vec();
        let pidf = d.join("run/omabox-shell.pid");
        assert_eq!(
            shell_pid(&ctx),
            Some(b"42".to_vec()),
            "shell_pid: the file's number"
        );
        std::fs::rename(&pidf, d.join("run/pid.real")).unwrap();
        std::os::unix::fs::symlink(d.join("run/pid.real"), &pidf).unwrap();
        assert_eq!(shell_pid(&ctx), None, "...a link to it: nothing");
        std::fs::remove_file(&pidf).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&pidf)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            shell_pid(&ctx),
            None,
            "...a FIFO: nothing, at once (not hung)"
        );
        std::fs::remove_file(&pidf).unwrap();
        std::fs::rename(d.join("run/pid.real"), &pidf).unwrap();
        let r = Ready {
            owner: None,
            covers: false,
            shell_before: b"old".to_vec(),
            bar_at: 0,
            before_end: &|| {},
        };
        let sc = |stat: &'static str| {
            let sys = ShellBox { stat, list: None };
            let (c, said) =
                crate::exit::heard(|| shell_crash_with(&sys, &ctx, &r, shell_pid(&ctx).as_deref()));
            (c.map(|c| String::from_utf8_lossy(&c).into_owned()), said)
        };
        let s1 = "42 (quickshell) S 1";
        assert_eq!(
            sc(s1).0,
            None,
            "no new report, no crash line, the shell running: no crash"
        );
        // (#303: a folder counts once its report names the box shell's config; another Quickshell's
        // does not.)
        let cp = "Config Path: /usr/share/omarchy/shell/shell.qml\n";
        let q = cr.join("q1k4dmt");
        std::fs::create_dir(&q).unwrap();
        assert_eq!(
            sc(s1).0,
            None,
            "a report folder not written yet: no crash yet"
        );
        std::fs::write(q.join("report.txt"), "Config Path: /tmp/qt/shell.qml\n").unwrap();
        assert_eq!(sc(s1).0, None, "...another Quickshell's: no crash (#303)");
        std::fs::write(q.join("report.txt"), cp).unwrap();
        assert_eq!(
            sc(s1).0,
            Some(format!("crashed {}/report.txt", q.display())),
            "a new report folder: crashed, its report named"
        );
        std::fs::remove_dir_all(&q).unwrap();
        let ab = cr.join("a b");
        std::fs::create_dir(&ab).unwrap();
        std::fs::write(ab.join("report.txt"), cp).unwrap();
        assert_eq!(
            sc(s1).0.as_deref(),
            Some("crashed -"),
            "a folder named oddly (the box names it): crashed, no path"
        );
        std::fs::remove_dir_all(&ab).unwrap();
        std::fs::write(
            d.join("home/shell.log"),
            " ERROR: Quickshell has crashed under pid 43\n",
        )
        .unwrap();
        assert_eq!(
            sc(s1).0.as_deref(),
            Some("crashed -"),
            "only the log's crash line: crashed"
        );
        std::fs::write(d.join("home/shell.log"), "").unwrap();
        assert_eq!(sc("").0.as_deref(), Some("exited"), "its pid gone: exited");
        assert_eq!(
            sc("42 (quickshell) Z 1").0.as_deref(),
            Some("exited"),
            "...or a zombie"
        );
        assert_eq!(
            sc(s1).0,
            None,
            "the report folders before a restart are not a crash"
        );
        // finding 255: a report of a process older than this start (its pid below shell.sh's mark)
        // is an old shell's, stopped as it loaded, not this start's crash.
        let m = cr.join("m1");
        std::fs::create_dir(&m).unwrap();
        std::fs::write(
            m.join("report.txt"),
            format!("Signal: Segmentation fault (11)\nCrashed process ID: 30\n{cp}"),
        )
        .unwrap();
        std::fs::write(d.join("run/omabox-shell.mark"), "40\n").unwrap();
        assert_eq!(
            sc(s1).0,
            None,
            "a report of a process older than this start (below shell.sh's mark): no crash (finding 255)"
        );
        std::fs::write(
            m.join("report.txt"),
            format!("Crashed process ID: 41\n{cp}"),
        )
        .unwrap();
        let mrep = Some(format!("crashed {}/report.txt", m.display()));
        assert_eq!(sc(s1).0, mrep, "...one of this start's (above it): crashed");
        std::fs::remove_file(d.join("run/omabox-shell.mark")).unwrap();
        std::fs::write(
            m.join("report.txt"),
            format!("Crashed process ID: 30\n{cp}"),
        )
        .unwrap();
        assert_eq!(
            sc(s1).0,
            mrep,
            "...no mark (a box an older omabox started): every report counts, as before"
        );
        std::fs::remove_dir_all(&m).unwrap();
        // finding 248: its pid gone as a duplicate of a shell another start brought up meanwhile.
        let dup = |list: &'static str| {
            let sys = ShellBox {
                stat: "",
                list: Some(list),
            };
            let (c, said) =
                crate::exit::heard(|| shell_crash_with(&sys, &ctx, &r, shell_pid(&ctx).as_deref()));
            (c.map(|c| String::from_utf8_lossy(&c).into_owned()), said)
        };
        std::fs::write(
            d.join("home/shell.log"),
            "An instance of this configuration is already running.\n",
        )
        .unwrap();
        assert_eq!(
            dup("[]").0.as_deref(),
            Some("exited"),
            "a duplicate, no shell of its config up: exited"
        );
        assert_eq!(
            dup(r#"[{"pid": 77}]"#),
            (None, "omabox: box 'u': another start of the shell came first; that one (pid 77) is up, started after the old one was stopped: the box's shell now\n".to_string()),
            "...one up (#192): no failure, it is the box's shell now"
        );
        assert_eq!(
            std::fs::read(&pidf).unwrap(),
            b"77\n",
            "...its pid recorded where shell.sh keeps it"
        );
        std::fs::write(&pidf, "42\n").unwrap();
        assert_eq!(
            dup(r#"[{"pid": 78}]"#).0.as_deref(),
            Some("exited"),
            "...not one whose pid is gone too"
        );
        std::fs::write(d.join("home/shell.log"), "").unwrap();
        assert_eq!(
            dup(r#"[{"pid": 77}]"#).0.as_deref(),
            Some("exited"),
            "no duplicate line: exited, whatever runs"
        );
        let _ = std::fs::remove_dir_all(shell::path(&ctx.boxes));
    }

    #[test]
    fn new_reports_in_byte_order() {
        assert_eq!(
            new_lines(b"a\nb", b"b\nc\nB\na"),
            vec![b"B".to_vec(), b"c".to_vec()]
        );
        assert_eq!(new_lines(b"", b""), Vec::<Vec<u8>>::new());
        assert_eq!(new_lines(b"", b"x\n"), vec![b"x".to_vec()]);
        assert_eq!(
            crashed_pid(b"a\nCrashed process ID: 42\n"),
            Some(b"42".to_vec())
        );
        assert_eq!(crashed_pid(b"Crashed process ID: 4x\n"), None);
        assert_eq!(crashed_pid(b"Crashed process ID: \n"), Some(Vec::new()));
    }

    #[test]
    fn lines_and_names() {
        assert_eq!(last_line(b"a\n  b \n \t\n"), b"b ");
        assert_eq!(last_line(b"\n\n"), b"");
        assert_eq!(theme_line(b"Tokyo Night\nx"), b"Tokyo Night");
        assert_eq!(theme_line(b"caf\xc3\xa9"), b"");
        assert_eq!(theme_line(&[b'a'; 65]), b"");
        assert_eq!(theme_line(&[b'a'; 64]), vec![b'a'; 64]);
        assert_eq!(theme_line(b"a\0b"), b"ab");
        assert!(plain(b"x_1.y-z"));
        assert!(!plain(b"x y"));
        assert!(!plain(b""));
        assert_eq!(after_space(b"HEADLESS-2 1920x1080@60"), b"1920x1080@60");
        assert_eq!(after_space(b"x"), b"x");
    }

    #[test]
    fn reapers_by_their_command_line() {
        let (r, n): (&[u8], &[u8]) = (b"/r", b"b1");
        assert!(reaper_line(
            b"bash\0/r/bin/omabox\0_reap\0b1\x00123\x00",
            r,
            n
        ));
        assert!(reaper_line(
            b"/usr/bin/bash\0/r/bin/omabox\0_reap\0b1\x00123\x00",
            r,
            n
        ));
        // (the binary's own, since the switch: no bash before it)
        assert!(reaper_line(b"/r/bin/omabox\0_reap\0b1\x00123\x00", r, n));
        assert!(!reaper_line(b"/r/bin/omabox\0_reap\0b10\x00123\x00", r, n));
        assert!(!reaper_line(b"/x/r/bin/omabox\0_reap\0b1\x00123\x00", r, n));
        assert!(!reaper_line(
            b"bash\0/r/bin/omabox\0_reap\0b10\x00123\x00",
            r,
            n
        ));
        assert!(!reaper_line(
            b"sh\0/r/bin/omabox\0_reap\0b1\x00123\x00",
            r,
            n
        ));
        // (the name the last word: no space after it in pgrep's line)
        assert!(!reaper_line(b"bash\0/r/bin/omabox\0_reap\0b1\0", r, n));
        // (the Rust half's, which bash's hand-off exec'd)
        for sub in [&b""[..], b"/bin"] {
            let b = cat!("/r/build", sub);
            assert!(reaper_line(&cat!(b, "/omabox\0_reap\0b1\x00123\x00"), r, n));
            assert!(!reaper_line(
                &cat!(b, "/omabox\0_reap\0b10\x00123\x00"),
                r,
                n
            ));
        }
        assert!(!reaper_line(b"/x/build/omabox\0_reap\0b1\x00123\x00", r, n));
        assert!(!reaper_line(
            b"/r/build/lib/omabox\0_reap\0b1\x00123\x00",
            r,
            n
        ));
    }

    #[test]
    fn software_said() {
        let rec = br#"{"renderer":"llvmpipe (LLVM 19, 256 bits)","software":true}"#;
        assert_eq!(
            soft_note(rec, b"", None),
            b"its apps render on llvmpipe, in software: CPU and frame cost measured in it are software rendering's, and omabox gpu counts none of their drawing; compare against a baseline measured in this same box".to_vec()
        );
        let n = soft_note(rec, b"nvidia", Some(b"renderD128 amdgpu"));
        assert!(n.ends_with(
            b"OMABOX_RENDER_NODE=/dev/dri/renderD128 omabox up --new (renderD128 is amdgpu)"
        ));
        assert!(crate::exit::contains(&n, b"so Mesa fell back to software)"));
        let n = soft_note(rec, b"amdgpu", Some(b""));
        assert!(n.ends_with(
            b"this machine has no other GPU: compare against a baseline measured in this same box"
        ));
        assert!(soft_note(br#"{"renderer":"AMD","software":false}"#, b"", None).is_empty());
        if !crate::exec::tools_or_skip("software_said's egl_render", &["awk"]) {
            return;
        }
        assert_eq!(
            egl_render(
                b"x\nOpenGL core profile renderer: AMD R\nOpenGL ES profile renderer: AMD ES\n"
            ),
            br#"{"renderer":"AMD ES","software":false}"#
        );
        assert_eq!(egl_render(b"nothing\n"), b"");
    }

    #[test]
    fn bare_box_json() {
        let ctx = Ctx {
            name: b"b".to_vec(),
            d: b"/nonexistent/omabox/b".to_vec(),
            ..Ctx::new(b"up", b"/nonexistent/omabox".to_vec())
        };
        assert_eq!(
            box_json(&ctx, false, "", ""),
            b"{\"name\":\"b\",\"state\":\"dead\",\"hung\":false,\"plugins\":[],\"peeking\":false,\"keys_to_box\":false,\"orphan\":false}\n"
        );
    }

    /// t_unit_gpu's client_render_check (#204, finding 290), box_exec's output and other_gpu given:
    /// llvmpipe in an NVIDIA box said once, kept in box.json; no eglinfo (no output) neither.
    /// (#[ignore]: its eglinfo verdict is awk's, which differs on CI; test/run.sh's unit tier runs it.)
    #[test]
    #[ignore = "runs awk: run by test/run.sh (cargo test -- --ignored)"]
    fn client_render_as_bash() {
        let ctx = uctx("crc");
        let _ = std::fs::remove_dir_all(shell::path(&ctx.boxes));
        std::fs::create_dir_all(shell::path(&ctx.d)).unwrap();
        let bj = cat!(ctx.d, "/box.json");
        std::fs::write(shell::path(&bj), r#"{"render":{"driver":"nvidia"}}"#).unwrap();
        let eg_nv = b"MESA-EGL: warning: egl: failed to create dri2 screen\nWayland platform:\nEGL vendor string: Mesa Project\nOpenGL core profile renderer: llvmpipe (LLVM 22.1.8, 256 bits)\nOpenGL ES profile vendor: Mesa\nOpenGL ES profile renderer: llvmpipe (LLVM 22.1.8, 256 bits)\nOpenGL ES profile version: OpenGL ES 3.2 Mesa 26.2.2\n";
        let mut c = ctx.clone();
        c.name = b"pb".to_vec();
        let (r, said) =
            crate::exit::heard(|| client_render_from(&c, eg_nv, Some(b"renderD128 amdgpu")));
        assert!(r.is_ok());
        assert!(
            said.starts_with("omabox: box 'pb': its apps render on llvmpipe, in software"),
            "{said}"
        );
        assert_eq!(
            said.matches("its apps render").count(),
            1,
            "said once: {said}"
        );
        let kept = jq::jq(
            &["-c", ".client_render"],
            Some(&std::fs::read(shell::path(&bj)).unwrap()),
            jq::Err::Null,
        )
        .text();
        assert_eq!(
            kept,
            br#"{"renderer":"llvmpipe (LLVM 22.1.8, 256 bits)","software":true}"#
        );
        // no eglinfo (127: no output): nothing said, nothing kept
        std::fs::write(shell::path(&bj), r#"{"render":{"driver":"nvidia"}}"#).unwrap();
        let (r, said) =
            crate::exit::heard(|| client_render_from(&c, b"", Some(b"renderD128 amdgpu")));
        assert!(r.is_ok());
        assert_eq!(said, "");
        let kept = jq::jq(
            &["-c", ".client_render"],
            Some(&std::fs::read(shell::path(&bj)).unwrap()),
            jq::Err::Null,
        )
        .text();
        assert_eq!(kept, b"null");
        let _ = std::fs::remove_dir_all(shell::path(&ctx.boxes));
    }
}
