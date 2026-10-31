//! A jailed agent's omabox (#324 wave 6-front, A4; issue #16, finding 99): it cannot enter a box
//! (ai-jail's seccomp refuses it new namespaces), so it hands its command to the broker through
//! omabox-relay, with what only it has: the box it would pick by default (the repo and session it
//! is in, as seen here), its PATH (for `run`'s commands in the box) and the variables it names with
//! --pass. Paths the command takes become absolute (the broker is not in this directory); files it
//! writes or reads (shot, record, --in) are opened here and passed open. A --pass or --env-file
//! value goes to the relay on an fd, never on its command line, which every process can read in
//! /proc (finding 503).

use crate::cat;
use crate::exit::{Ctx, Exit, Res, say};
use crate::shell::{self, path};
use std::ffi::OsStr;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::process::Command;

/// relay_bin: the checkout's relay, or the one the jail was given (on PATH).
pub fn relay_bin() -> Option<Vec<u8>> {
    let mine = crate::paths::tool("relay");
    if crate::exec::executable(&mine) && path(&mine).is_file() {
        return Some(mine);
    }
    shell::env_or_empty("PATH")
        .split(|&c| c == b':')
        .map(|d| {
            cat!(
                if d.is_empty() { b".".as_slice() } else { d },
                "/omabox-relay"
            )
        })
        .find(|p| crate::exec::executable(p) && path(p).is_file())
}

/// A variable name a jailed caller may send (as the relay's NAME=VALUE entries take it).
fn pass_name(n: &[u8]) -> bool {
    shell::re_match("^[A-Za-z_][A-Za-z0-9_]{0,58}$", n)
}

/// What goes to the broker beside the words: `--env NAME=VALUE` entries on the relay's command line
/// (the default box, PATH, ...), and the caller's own values (PASS_NAME=VALUE) on an fd.
struct Env {
    open: Vec<Vec<u8>>,
    secret: Vec<Vec<u8>>,
}

impl Env {
    fn add(&mut self, kv: Vec<u8>) {
        self.open.push(b"--env".to_vec());
        self.open.push(kv);
    }
}

/// An fd this process keeps across exec (std opens every file close-on-exec).
fn inherited(fd: RawFd) -> bool {
    // SAFETY: fcntl on an fd this process owns; F_SETFD with 0 only clears FD_CLOEXEC.
    unsafe { libc::fcntl(fd, libc::F_SETFD, 0) == 0 }
}

/// The secret entries, NUL-ended, in a memfd the relay reads (--env-fd) and closes: a file only
/// this process and the relay hold, never on a command line.
fn secret_fd(ctx: &Ctx, entries: &[Vec<u8>]) -> Res<Option<OwnedFd>> {
    if entries.is_empty() {
        return Ok(None);
    }
    // SAFETY: memfd_create with a NUL-ended name and no flags (the fd is meant for the relay).
    let fd = unsafe { libc::memfd_create(c"omabox-relay-env".as_ptr(), 0) };
    if fd < 0 {
        return Err(ctx.die("cannot hand the caller's values to omabox-relay (memfd_create)"));
    }
    // SAFETY: memfd_create just returned this fd, owned by nothing else.
    let owned = unsafe { <OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd) };
    let mut f = std::fs::File::from(owned);
    let mut body = Vec::new();
    for e in entries {
        body.extend_from_slice(e);
        body.push(0);
    }
    use std::io::{Seek, Write};
    if f.write_all(&body).and_then(|()| f.rewind()).is_err() {
        return Err(ctx.die("cannot hand the caller's values to omabox-relay (memfd)"));
    }
    Ok(Some(OwnedFd::from(f)))
}

/// A socket path no socket can have (108 bytes and more): a call to it fails once its options are
/// read, before anything is sent.
const NO_SOCKET: &str = "/omabox-relay-probe/no-socket-has-a-path-this-long/................................................................................";

/// Whether the relay takes --env-fd. 0.x's, still mapped into a jail started before the upgrade,
/// answer it with their usage error, 2, which `run` would hand on as its command's (#380, finding
/// 536). Asked with a call to NO_SOCKET, its --env-fd /dev/null: 1 from a relay that read its
/// options, 2 from one that did not; one that cannot be asked is left to the call to say so.
fn takes_env_fd(relay: &[u8]) -> bool {
    let Ok(null) = std::fs::File::open("/dev/null") else {
        return true;
    };
    if !inherited(null.as_raw_fd()) {
        return true;
    }
    let mut c = crate::sig::command(OsStr::from_bytes(relay));
    c.arg("call")
        .arg(NO_SOCKET)
        .arg("--env-fd")
        .arg(null.as_raw_fd().to_string())
        .args(["--", "true"])
        .env_remove("OMABOX_RELAY_FAIL")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    crate::sig::run_status(c).map_or(true, |rc| rc != 2)
}

/// `"$relay" call "$BROKER_SOCK" [--fd N]... ENV... -- WORDS...`: the command, and the files it
/// must keep open until the relay is done with them.
fn call_cmd(
    ctx: &Ctx,
    relay: &[u8],
    fds: &[RawFd],
    env: &Env,
    words: &[Vec<u8>],
) -> Res<(Command, Option<OwnedFd>)> {
    let mut c = crate::sig::command(OsStr::from_bytes(relay));
    c.arg("call")
        .arg(OsStr::from_bytes(&crate::front::broker_sock(&ctx.boxes)));
    for fd in fds {
        c.arg("--fd").arg(fd.to_string());
    }
    c.args(env.open.iter().map(|a| OsStr::from_bytes(a)));
    // (asked before the values' fd is made: the question's relay never holds it)
    if !env.secret.is_empty() && !takes_env_fd(relay) {
        say(cat!(
            "the omabox-relay at ",
            relay,
            " is older than this omabox (it takes no --env-fd, which --pass and --env-file need): restart the jail, which maps the new one"
        ));
        return Err(Exit(ctx.die_rc.unwrap_or(1)));
    }
    let sf = secret_fd(ctx, &env.secret)?;
    if let Some(f) = &sf {
        if !inherited(f.as_raw_fd()) {
            return Err(ctx.die("cannot hand the caller's values to omabox-relay"));
        }
        c.arg("--env-fd").arg(f.as_raw_fd().to_string());
    }
    c.arg("--");
    c.args(words.iter().map(|a| OsStr::from_bytes(a)));
    Ok((c, sf))
}

/// `exec "$relay" call ...`: this process becomes the relay; only its failure to start comes back.
fn exec_call(ctx: &Ctx, relay: &[u8], fds: &[RawFd], env: &Env, words: &[Vec<u8>]) -> Exit {
    match call_cmd(ctx, relay, fds, env, words) {
        Ok((mut c, _keep)) => {
            let e = c.exec();
            say(cat!("cannot run ", relay, ": ", e.to_string()));
            Exit(126)
        }
        Err(e) => e,
    }
}

/// `"$relay" call ...` as a child, its status (bash's `|| rc=$?`).
fn run_call(ctx: &Ctx, relay: &[u8], fds: &[RawFd], env: &Env, words: &[Vec<u8>]) -> Res<i32> {
    let (c, _keep) = call_cmd(ctx, relay, fds, env, words)?;
    Ok(crate::sig::run_status(c).unwrap_or(126))
}

/// `"$relay" call ...` for its stdout (`$(...)`), its status.
fn output_call(ctx: &Ctx, relay: &[u8], env: &Env, words: &[Vec<u8>]) -> Res<(i32, Vec<u8>)> {
    let (mut c, _keep) = call_cmd(ctx, relay, &[], env, words)?;
    c.stdin(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    Ok(crate::sig::run_output(c).unwrap_or((126, Vec::new())))
}

/// A file opened here for the broker to read or write (`exec {fd}<>FILE`), kept open across exec.
fn open_for(ctx: &Ctx, f: &[u8], write: bool) -> Res<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(write)
        .open(path(f))
        .map_err(|e| ctx.die(cat!(f, ": ", crate::shots::why(&e))))?;
    if !inherited(file.as_raw_fd()) {
        return Err(ctx.die(cat!("cannot pass ", f, " to omabox-relay")));
    }
    Ok(file)
}

/// `readlink -f -- P`, P itself when it cannot be.
fn abs(p: &[u8]) -> Vec<u8> {
    crate::repo::readlink_f(p).unwrap_or_else(|| p.to_vec())
}

/// relay_call: the command to the broker, never back (the relay's status is this omabox's).
pub fn call(ctx: &mut Ctx, cmd: &[u8], args: &[Vec<u8>]) -> Res<()> {
    let Some(relay) = relay_bin() else {
        ctx.parsing = false;
        return Err(ctx.die(
            "omabox-relay is not in reach: map it into the jail (omabox broker on prints how)",
        ));
    };
    let mut env = Env {
        open: Vec::new(),
        secret: Vec::new(),
    };
    env.add(cat!("DEFAULT=", crate::select::default_name()));
    env.add(cat!("PATH=", crate::exec::caller_path(false)));
    let (mut words, mut prev): (Vec<Vec<u8>>, Vec<u8>) = (Vec::new(), Vec::new());
    for a in args {
        // run --env-file: read here, in the jail, and sent as --pass entries (the broker reads no
        // path of the caller's), their values as --pass values are.
        if prev == b"--env-file" && cmd == b"run" {
            for kv in crate::cmd::run::envfile_read(ctx, a)? {
                let k = kv.split(|&b| b == b'=').next().unwrap_or_default().to_vec();
                if !pass_name(&k) {
                    return Err(ctx.die(cat!(
                        "--env-file: ",
                        k,
                        ": a name up to 59 characters, from inside ai-jail"
                    )));
                }
                env.secret.push(cat!("PASS_", kv));
                words.push(b"--pass".to_vec());
                words.push(k);
            }
            prev.clear();
            continue;
        }
        if a == b"--env-file" && cmd == b"run" {
            prev = a.clone();
            continue;
        }
        if prev == b"--pass" && pass_name(a) {
            if let Some(v) = crate::exec::caller_env(a) {
                env.secret.push(cat!("PASS_", a, "=", v));
            }
        }
        let mut a = a.clone();
        match prev.as_slice() {
            b"--plugin" | b"--overlay" | b"--theme-dir" | b"--omarchy"
                if !a.starts_with(b"/") && path(&a).is_dir() =>
            {
                a = abs(&a);
            }
            b"--hyprland" | b"--quickshell"
                if !a.starts_with(b"/") && std::fs::metadata(path(&a)).is_ok() =>
            {
                a = abs(&a);
            }
            b"--ro-bind" if !a.starts_with(b"/") => {
                // (`${a%%:/*}`: the source, up to a `:/` that starts the destination)
                let cut = a.windows(2).position(|w| w == b":/").unwrap_or(a.len());
                a = cat!(abs(&a[..cut]), &a[cut..]);
            }
            // SRC:DEST: only SRC is the caller's path (DEST is in the box HOME). One that is not
            // there is sent as given, for the broker to say so.
            b"--seed" => {
                if let Some(i) = a.iter().position(|&b| b == b':').filter(|&i| i > 0) {
                    let src = &a[..i];
                    if !a.starts_with(b"/")
                        && !a.starts_with(b"~")
                        && std::fs::metadata(path(src)).is_ok()
                    {
                        a = cat!(abs(src), &a[i..]);
                    }
                }
            }
            _ => {}
        }
        prev = a.clone();
        words.push(a);
    }
    match cmd {
        b"shot" | b"screenshot" => shot(ctx, &relay, env, &words),
        b"record" => record(ctx, &relay, env, &words),
        b"click" | b"scroll" | b"pointer" | b"drag" | b"pixel" | b"find" => {
            relay_in(ctx, &relay, cmd, env, &words)
        }
        b"wait" if wait_image_file(&words).is_some() => relay_in(ctx, &relay, cmd, env, &words),
        _ => {
            // (run's: the relay's own failure, the broker not reached, is 125 as run's own are)
            if cmd == b"run" {
                // SAFETY: the front runs before any other thread is started.
                unsafe { std::env::set_var("OMABOX_RELAY_FAIL", "125") };
            }
            let mut all = vec![cmd.to_vec()];
            all.extend(words);
            Err(exec_call(ctx, &relay, &[], &env, &all))
        }
    }
}

/// The caller's default --fit (#188): the broker gets nothing of its environment but what is sent.
fn shot_fit(env: &mut Env) {
    let fit = shell::env_or_empty("OMABOX_SHOT_FIT");
    if !fit.is_empty() {
        env.add(cat!("SHOT_FIT=", fit));
    }
}

/// `${TMPDIR:-/tmp}`.
fn tmpdir() -> Vec<u8> {
    shell::env("TMPDIR")
        .filter(|t| !t.is_empty())
        .unwrap_or(b"/tmp".to_vec())
}

/// `mkdir -p -- "$(dirname -- OUT)" || die "LABEL: cannot make the directory D"`, then OUT resolved.
fn place_dir(ctx: &Ctx, label: &str, out: &[u8]) -> Res<Vec<u8>> {
    let dir = crate::shots::dirname(out);
    if !crate::shots::mkdir_p(&dir) {
        return Err(ctx.die(cat!(label, ": cannot make the directory ", dir)));
    }
    Ok(abs(out))
}

/// A temp file beside where it goes (`mktemp DIR/.omabox-KIND.XXXXXX`), mktemp's words kept out.
fn temp_beside(dir: &[u8], kind: &str) -> Option<(std::fs::File, Vec<u8>)> {
    crate::exit::muted(|| {
        let mut c = crate::sig::command("mktemp");
        c.arg(OsStr::from_bytes(&cat!(dir, "/.omabox-", kind, ".XXXXXX")))
            .stderr(std::process::Stdio::null());
        let (rc, o) = crate::sig::run_output(c).ok()?;
        let p = shell::strip_nl(&o);
        if rc != 0 || p.is_empty() {
            return None;
        }
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path(&p))
            .ok()?;
        Some((f, p))
    })
}

/// jail_place TMP OUT: a file the broker wrote through an fd (mktemp's, 0600) put at OUT with the
/// mode a file made here would have (as the umask says).
fn place(tmp: &[u8], out: &[u8]) {
    // SAFETY: umask is process-wide; read and set back at once, on the front's one thread.
    let mask = unsafe {
        let m = libc::umask(0o022);
        libc::umask(m);
        m
    };
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path(tmp), std::fs::Permissions::from_mode(0o666 & !mask));
    let _ = std::fs::rename(path(tmp), path(out));
}

fn size_of(f: &[u8]) -> u64 {
    std::fs::metadata(path(f)).map_or(0, |m| m.len())
}

fn code(rc: i32) -> Exit {
    Exit(u8::try_from(rc & 0xff).unwrap_or(1))
}

/// relay_shot: the file is made here (as `shot` names it) and filled by the broker through the fd;
/// renamed into place once it is whole.
fn shot(ctx: &mut Ctx, relay: &[u8], mut env: Env, args: &[Vec<u8>]) -> Res<()> {
    shot_fit(&mut env);
    let (mut name, mut out, mut fwd) = (Vec::new(), Vec::new(), Vec::new());
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_slice() {
            b"-b" | b"--box" => {
                crate::args::optarg(ctx, a, args.get(i + 1))?;
                name = args[i + 1].clone();
                fwd.extend_from_slice(&args[i..i + 2]);
                i += 1;
            }
            // (--monitor NAME was taken for the output file, #284)
            b"-g" | b"-w" | b"--window" | b"--monitor" | b"--fit" | b"--zoom" | b"--burst"
            | b"--every" | b"--ignore" => {
                crate::args::optarg(ctx, a, args.get(i + 1))?;
                fwd.extend_from_slice(&args[i..i + 2]);
                i += 1;
            }
            b"-o" | b"--output" => {
                crate::args::optarg(ctx, a, args.get(i + 1))?;
                crate::shots::two_files(ctx, "shot", &out, &args[i + 1])?;
                out = args[i + 1].clone();
                i += 1;
            }
            // The action's words are its own, not files (the broker says --burst is not a jail's).
            b"--after" => {
                fwd.extend_from_slice(&args[i..]);
                break;
            }
            // A path of the jail's, which the broker never opens (finding 243).
            b"--since" => {
                ctx.parsing = false;
                return Err(ctx.die(
                    "shot --since: not for a jailed agent (--changed compares with the last shot of it)",
                ));
            }
            _ if a.starts_with(b"-") => fwd.push(a.clone()),
            _ => {
                crate::shots::two_files(ctx, "shot", &out, a)?;
                out = a.clone();
            }
        }
        i += 1;
    }
    crate::select::select_box(ctx, &name)?;
    if out.is_empty() {
        out = cat!(
            tmpdir(),
            "/omabox-",
            ctx.name,
            "-",
            crate::cmd::shot::hms_ms(),
            ".png"
        );
    }
    let out = place_dir(ctx, "shot", &out)?;
    let dir = crate::shots::dirname(&out);
    let Some((file, tmp)) = temp_beside(&dir, "shot") else {
        return Err(ctx.die(cat!("shot: cannot write in ", dir)));
    };
    let _gone = crate::shell::Rm(tmp.clone());
    if !inherited(file.as_raw_fd()) {
        return Err(ctx.die(cat!("shot: cannot write in ", dir)));
    }
    let mut words = vec![b"shot".to_vec()];
    words.extend(fwd);
    words.extend([
        b"--to-fd".to_vec(),
        b"3".to_vec(),
        b"--as".to_vec(),
        out.clone(),
    ]);
    let rc = run_call(ctx, relay, &[file.as_raw_fd()], &env, &words)?;
    drop(file);
    // --changed with nothing changed writes no image: nothing at OUT either.
    if rc == 0 && size_of(&tmp) > 0 {
        place(&tmp, &out);
    }
    if rc == 0 { Ok(()) } else { Err(code(rc)) }
}

/// relay_in: click, scroll, pointer, drag and pixel --in SHOT, find and click --image FILE and wait
/// image FILE (#267), from a jail: the file opened here and passed as fd 3, with its name.
fn relay_in(ctx: &mut Ctx, relay: &[u8], cmd: &[u8], mut env: Env, args: &[Vec<u8>]) -> Res<()> {
    let (mut words, mut input) = (vec![cmd.to_vec()], None);
    let mut label: &[u8] = b"--in";
    let wfile = if cmd == b"wait" {
        wait_image_file(args)
    } else {
        None
    };
    let mut i = 0;
    while i < args.len() {
        if args[i] == b"--" {
            words.extend_from_slice(&args[i..]);
            break;
        }
        if (args[i] == b"--in" || args[i] == b"--image") && i + 1 < args.len() && input.is_none() {
            input = Some(args[i + 1].clone());
            label = if args[i] == b"--in" {
                b"--in"
            } else {
                b"--image"
            };
            words.extend([args[i].clone(), b"/dev/fd/3".to_vec()]);
            i += 2;
            continue;
        }
        if wfile == Some(i) {
            input = Some(args[i].clone());
            label = b"wait image";
            words.push(b"/dev/fd/3".to_vec());
            i += 1;
            continue;
        }
        words.push(args[i].clone());
        i += 1;
    }
    let Some(input) = input else {
        return Err(exec_call(ctx, relay, &[], &env, &words));
    };
    if !path(&input).is_file() {
        return Err(ctx.die(cat!(label, ": no such file: ", input)));
    }
    let f = open_for(ctx, &input, false)?;
    env.add(cat!("IN_AS=", abs(&input)));
    Err(exec_call(ctx, relay, &[f.as_raw_fd()], &env, &words))
}

/// wait's words: where the FILE of `wait image FILE` is (the first word after `image` that is
/// neither an option nor an option's value), None when it is not wait image.
fn wait_image_file(args: &[Vec<u8>]) -> Option<usize> {
    let takes = |w: &[u8]| {
        matches!(
            w,
            b"-b"
                | b"--box"
                | b"--ignore"
                | b"--timeout"
                | b"--quiet"
                | b"-g"
                | b"-w"
                | b"--window"
                | b"--monitor"
                | b"--similar"
        )
    };
    let (mut cond, mut i) = (false, 0);
    while i < args.len() {
        let a = &args[i];
        if a == b"--" {
            return None;
        }
        if takes(a) {
            i += 2;
            continue;
        }
        if !a.starts_with(b"-") {
            if cond {
                return Some(i);
            }
            if a != b"image" {
                return None;
            }
            cond = true;
        }
        i += 1;
    }
    None
}

/// Where `"$0" ARGS` sends its stdout: here, to stderr (`>&2`), or nowhere (`>/dev/null`).
#[derive(Clone, Copy)]
enum Out {
    Here,
    Err,
    Null,
}

/// `"$0" ARGS...` (this omabox again, through bin/omabox), its status.
fn again(args: &[Vec<u8>], to: Out) -> i32 {
    let mut c = crate::sig::omabox_cmd();
    c.args(args.iter().map(|a| OsStr::from_bytes(a)));
    match to {
        Out::Here => {}
        Out::Err => {
            if let Ok(e) = std::io::stderr().as_fd().try_clone_to_owned() {
                c.stdout(e);
            }
        }
        Out::Null => {
            c.stdout(std::process::Stdio::null());
        }
    }
    crate::sig::run_status(c).unwrap_or(126)
}

use std::os::fd::AsFd;

/// relay_record (#224): every file is the jail's, made here as a temp beside where it goes, filled by
/// the broker through its fd and renamed into place once whole. start names the video (-o, resolved
/// here, or a new one in $TMPDIR here) and checks it can be written; stop opens the video, its marks
/// and its captions for the name the broker says the recording goes to; `sheet VIDEO` passes the
/// video and its marks to read, the sheet to write. `record -- ACTION` is start, the action, stop,
/// run from here; `stop --sheet` is stop, then `sheet` of the video it wrote. status, mark and what
/// the broker refuses anyway go as they are.
#[allow(clippy::too_many_lines)]
fn record(ctx: &mut Ctx, relay: &[u8], mut env: Env, args: &[Vec<u8>]) -> Res<()> {
    shot_fit(&mut env);
    let all: Vec<Vec<u8>> = std::iter::once(b"record".to_vec())
        .chain(args.iter().cloned())
        .collect();
    let (mut pre, mut rest, mut sargs, mut ract, mut pol, mut unpol) = (
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let (mut name, mut sub, mut out, mut video, mut forv) = (
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None::<Vec<u8>>,
    );
    let (mut sheet, mut hold, mut holdset, mut dash) = (false, 1000i64, false, false);
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let next = args.get(i + 1);
        match a.as_slice() {
            b"-b" | b"--box" => {
                crate::args::optarg(ctx, a, next)?;
                name = args[i + 1].clone();
                pre = vec![a.clone(), name.clone()];
                i += 1;
            }
            b"-o" | b"--output" => {
                crate::args::optarg(ctx, a, next)?;
                out = args[i + 1].clone();
                i += 1;
            }
            b"--hold" => {
                crate::args::optarg(ctx, a, next)?;
                hold = crate::num::ms_duration(&args[i + 1])
                    .filter(|&h| h <= 60000)
                    .ok_or_else(|| {
                        ctx.die(cat!(
                            "record: --hold takes a duration up to 60s, like 500ms or 2s, got ",
                            args[i + 1]
                        ))
                    })?;
                holdset = true;
                i += 1;
            }
            b"--for" => {
                crate::args::optarg(ctx, a, next)?;
                forv = Some(args[i + 1].clone());
                i += 1;
            }
            // The polish (stop's): on a one-shot, given to its stop, not its start.
            b"--show-input" | b"--captions" => {
                pol.push(a.clone());
                rest.push(a.clone());
            }
            b"--cut-idle" => {
                pol.push(a.clone());
                rest.push(a.clone());
                if let Some(v) =
                    next.filter(|v| shell::re_match("^[0-9]{1,6}(\\.[0-9]{1,3})?(ms|s|m)?$", v))
                {
                    pol.push(v.clone());
                    rest.push(v.clone());
                    i += 1;
                }
            }
            b"--sheet" => sheet = true,
            b"--frames" | b"--every" | b"--fit" => {
                crate::args::optarg(ctx, a, next)?;
                sheet = true;
                sargs.extend_from_slice(&args[i..i + 2]);
                i += 1;
            }
            b"--monitor" | b"--fps" | b"--crf" => {
                crate::args::optarg(ctx, a, next)?;
                rest.extend_from_slice(&args[i..i + 2]);
                unpol.extend_from_slice(&args[i..i + 2]);
                i += 1;
            }
            b"--to-fd" | b"--marks-fd" | b"--vtt-fd" | b"--from-fd" | b"--marks-from-fd"
            | b"--as" | b"--file" => {
                ctx.parsing = false;
                return Err(ctx.die(cat!(
                    "record: ",
                    a,
                    " is omabox's own, between a jailed agent's omabox and the broker"
                )));
            }
            b"--" => {
                dash = true;
                ract = args[i + 1..].to_vec();
                break;
            }
            _ if a.starts_with(b"-") => {
                rest.push(a.clone());
                unpol.push(a.clone());
            }
            _ => {
                if sub.is_empty() {
                    sub = a.clone();
                    rest.push(a.clone());
                    unpol.push(a.clone());
                } else if sub == b"sheet" && video.is_empty() {
                    video = a.clone();
                } else {
                    rest.push(a.clone());
                    unpol.push(a.clone());
                }
            }
        }
        i += 1;
    }
    let pass = |ctx: &Ctx| exec_call(ctx, relay, &[], &env, &all);
    // --for (#259) runs here as start, a hold, stop; with anything it does not go with, the broker
    // says why.
    if let Some(f) = &forv {
        if !sub.is_empty() || dash || holdset {
            return Err(pass(ctx));
        }
        if crate::num::ms_duration(f).is_none_or(|ms| ms > 3_600_000) {
            say(cat!(
                "record: --for takes a duration up to 60m, like 5s or 1.5m (a bare number is seconds), got ",
                f
            ));
            return Err(Exit(2));
        }
    }
    match sub.as_slice() {
        b"start" | b"stop" | b"sheet" if dash => return Err(pass(ctx)),
        b"start" | b"stop" | b"sheet" => {}
        b"" if !dash && forv.is_none() => return Err(pass(ctx)),
        b"" => {}
        _ => return Err(pass(ctx)),
    }
    crate::select::select_box(ctx, &name)?;
    let rec = |w: &[&[Vec<u8>]]| -> Vec<Vec<u8>> {
        let mut v = vec![b"record".to_vec()];
        for p in w {
            v.extend_from_slice(p);
        }
        v
    };
    match sub.as_slice() {
        b"start" => {
            if sheet {
                return Err(pass(ctx));
            }
            if out.is_empty() {
                out = cat!(
                    tmpdir(),
                    "/omabox-",
                    ctx.name,
                    "-",
                    shell::date("%H%M%S"),
                    ".mp4"
                );
            }
            let out = place_dir(ctx, "record", &out)?;
            let dir = crate::shots::dirname(&out);
            // Written here at stop: said now if it cannot be.
            let Some((_, t)) = temp_beside(&dir, "record") else {
                return Err(ctx.die(cat!("record: cannot write in ", dir)));
            };
            let _ = std::fs::remove_file(path(&t));
            let w = rec(&[&pre, &rest, &[b"--as".to_vec(), out]]);
            Err(exec_call(ctx, relay, &[], &env, &w))
        }
        b"" => {
            // One action (or --for's hold), checked as record does before anything starts; each step
            // through this omabox.
            if forv.is_none() {
                crate::cmd::record::action_check(ctx, &ract)?;
            }
            let st: Vec<Vec<u8>> = if out.is_empty() {
                Vec::new()
            } else {
                vec![b"-o".to_vec(), out.clone()]
            };
            let mut so: Vec<Vec<u8>> = Vec::new();
            if sheet {
                so.push(b"--sheet".to_vec());
                so.extend(sargs.iter().cloned());
            }
            so.extend(pol.iter().cloned());
            let start = rec(&[&pre, &[b"start".to_vec()], &unpol, &st]);
            let rc = again(&start, Out::Here);
            if rc != 0 {
                return Err(code(rc));
            }
            let stop = rec(&[&pre, &[b"stop".to_vec()], &so]);
            if let Some(f) = forv {
                let h = again(&rec(&[&pre, &[b"hold".to_vec(), f]]), Out::Null);
                let s = again(&stop, Out::Here);
                if s != 0 {
                    return Err(code(s));
                }
                return if h == 0 { Ok(()) } else { Err(code(h)) };
            }
            let mut act = pre.clone();
            act.extend(ract);
            let arc = again(&act, Out::Err);
            crate::sig::sleep(std::time::Duration::from_millis(
                u64::try_from(hold).unwrap_or(0),
            ));
            let s = again(&stop, Out::Here);
            if s != 0 {
                return Err(code(s));
            }
            if arc != 0 {
                say(cat!(
                    "record: the action ended ",
                    arc.to_string(),
                    " (the recording is kept)"
                ));
                return Err(code(arc));
            }
            Ok(())
        }
        b"stop" => {
            if !out.is_empty() {
                return Err(pass(ctx));
            }
            let (rc, named) = output_call(
                ctx,
                relay,
                &env,
                &rec(&[&pre, &[b"status".to_vec(), b"--file".to_vec()]]),
            )?;
            if rc != 0 {
                return Err(code(rc));
            }
            let out = shell::strip_nl(&named);
            if !out.starts_with(b"/") {
                return Err(ctx.die(cat!(
                    "record stop: the broker named no file to write (",
                    out,
                    ")"
                )));
            }
            let dir = crate::shots::dirname(&out);
            let base = out.strip_suffix(b".mp4").unwrap_or(&out).to_vec();
            if !crate::shots::mkdir_p(&dir) {
                return Err(ctx.die(cat!(
                    "record stop: cannot make the directory ",
                    dir,
                    " (the video goes to ",
                    out,
                    ")"
                )));
            }
            let mut tmps: Vec<(std::fs::File, Vec<u8>)> = Vec::new();
            for _ in 0..3 {
                let Some(t) = temp_beside(&dir, "record") else {
                    for (_, p) in &tmps {
                        let _ = std::fs::remove_file(path(p));
                    }
                    return Err(ctx.die(cat!(
                        "record stop: cannot write in ",
                        dir,
                        " (the video goes to ",
                        out,
                        ")"
                    )));
                };
                tmps.push(t);
            }
            let gone: Vec<crate::shell::Rm> = tmps
                .iter()
                .map(|(_, p)| crate::shell::Rm(p.clone()))
                .collect();
            let fds: Vec<RawFd> = tmps.iter().map(|(f, _)| f.as_raw_fd()).collect();
            if !fds.iter().all(|&f| inherited(f)) {
                return Err(ctx.die(cat!("record stop: cannot write in ", dir)));
            }
            let w = rec(&[
                &pre,
                &rest,
                &[
                    b"--to-fd".to_vec(),
                    b"3".to_vec(),
                    b"--marks-fd".to_vec(),
                    b"4".to_vec(),
                    b"--vtt-fd".to_vec(),
                    b"5".to_vec(),
                    b"--as".to_vec(),
                    out.clone(),
                ],
            ]);
            let rc = run_call(ctx, relay, &fds, &env, &w)?;
            let paths: Vec<Vec<u8>> = tmps.into_iter().map(|(_, p)| p).collect();
            if rc != 0 || size_of(&paths[0]) == 0 {
                drop(gone);
                return Err(code(if rc != 0 { rc } else { 1 }));
            }
            place(&paths[0], &out);
            // No marks: none beside it, and an earlier take's go (as record does outside a jail).
            if size_of(&paths[1]) > 0 {
                place(&paths[1], &cat!(base, ".marks.tsv"));
                place(&paths[2], &cat!(base, ".vtt"));
            } else {
                let _ = std::fs::remove_file(path(&cat!(base, ".marks.tsv")));
                let _ = std::fs::remove_file(path(&cat!(base, ".vtt")));
            }
            drop(gone);
            if sheet {
                let mut w = vec![b"record".to_vec()];
                w.extend(pre.iter().cloned());
                w.extend([b"sheet".to_vec(), out]);
                w.extend(sargs);
                let mut c = crate::sig::omabox_cmd();
                c.args(w.iter().map(|a| OsStr::from_bytes(a)));
                let e = c.exec();
                say(cat!("cannot run omabox again: ", e.to_string()));
                return Err(Exit(126));
            }
            Ok(())
        }
        _ => {
            // sheet
            if video.is_empty() {
                return Err(pass(ctx));
            }
            if !path(&video).is_file() {
                return Err(ctx.die(cat!("record sheet: no video ", video)));
            }
            let video = abs(&video);
            let base = video.strip_suffix(b".mp4").unwrap_or(&video).to_vec();
            if out.is_empty() {
                out = cat!(base, ".sheet.png");
            }
            let dir0 = crate::shots::dirname(&out);
            if !crate::shots::mkdir_p(&dir0) {
                return Err(ctx.die(cat!("record sheet: cannot make the directory ", dir0)));
            }
            let out = abs(&out);
            let vf = open_for(ctx, &video, false)?;
            let mut fds = vec![vf.as_raw_fd()];
            let mut a: Vec<Vec<u8>> = vec![b"--from-fd".to_vec(), b"3".to_vec()];
            let marks = cat!(base, ".marks.tsv");
            let mf = if path(&marks).is_file() {
                let f = open_for(ctx, &marks, false)?;
                fds.push(f.as_raw_fd());
                a.extend([b"--marks-from-fd".to_vec(), b"4".to_vec()]);
                Some(f)
            } else {
                None
            };
            let dir = crate::shots::dirname(&out);
            let Some((tf, tmp)) = temp_beside(&dir, "sheet") else {
                return Err(ctx.die(cat!("record sheet: cannot write in ", dir)));
            };
            let _gone = crate::shell::Rm(tmp.clone());
            if !inherited(tf.as_raw_fd()) {
                return Err(ctx.die(cat!("record sheet: cannot write in ", dir)));
            }
            fds.push(tf.as_raw_fd());
            a.extend([
                b"--to-fd".to_vec(),
                (fds.len() + 2).to_string().into_bytes(),
                b"--as".to_vec(),
                out.clone(),
            ]);
            let w = rec(&[&pre, &rest, &sargs, &a]);
            let rc = run_call(ctx, relay, &fds, &env, &w)?;
            drop((vf, mf, tf));
            if rc == 0 && size_of(&tmp) > 0 {
                place(&tmp, &out);
                return Ok(());
            }
            Err(code(if rc != 0 { rc } else { 1 }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #267: the FILE of wait image FILE is found past options and their values; other conditions
    /// and a value named image are not it.
    #[test]
    fn wait_image_file_is_the_word_after_image() {
        let w = |s: &str| -> Vec<Vec<u8>> { s.split(' ').map(|x| x.as_bytes().to_vec()).collect() };
        assert_eq!(wait_image_file(&w("image a.png")), Some(1));
        assert_eq!(
            wait_image_file(&w("--timeout 2s image --gone -g 0,0,9,9 a.png")),
            Some(6)
        );
        assert_eq!(wait_image_file(&w("-b image still")), None);
        assert_eq!(wait_image_file(&w("window image")), None);
        assert_eq!(wait_image_file(&w("image --gone")), None);
        assert_eq!(wait_image_file(&w("until -- image x")), None);
    }

    /// A stand-in relay, written by a shell of its own: no fd of this process ever holds it open for
    /// writing, which a fork in another test's thread would carry into an exec of it (ETXTBSY).
    fn stand_in(name: &str, body: &str) -> String {
        let d = std::env::temp_dir().join(format!("omabox-relay-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("omabox-relay").to_str().unwrap().to_string();
        let ok = Command::new("sh")
            .args([
                "-c",
                "printf '%s\\n' \"$1\" > \"$2\" && chmod +x \"$2\"",
                "sh",
            ])
            .args([body, &f])
            .status()
            .unwrap()
            .success();
        assert!(ok, "the stand-in {f} written");
        f
    }

    // #380, finding 536: an older relay is told by its answer to --env-fd.
    #[test]
    fn takes_env_fd_tells_an_older_relay() {
        // (0.x's call: --fd and --env before --, anything else its usage error, 2)
        let old = stand_in(
            "old",
            "#!/bin/sh\nshift 2\nwhile [ \"$1\" != -- ]; do case $1 in --fd|--env) shift 2 ;; \
             *) echo \"omabox-relay: unknown option $1\" >&2; exit 2 ;; esac; done\nexit 0",
        );
        assert!(!takes_env_fd(old.as_bytes()), "0.x's relay: no --env-fd");
        // (1.0's: the question as asked, its options read, then a socket path too long: its own 1,
        // OMABOX_RELAY_FAIL not set for the question; asked anything else, a usage error)
        let new = stand_in(
            "new",
            &format!(
                "#!/bin/sh\n[ \"$1 $2 $3 $5 $6 $#\" = 'call {NO_SOCKET} --env-fd -- true 6' ] && \
                 [ -r \"/proc/$$/fd/$4\" ] && [ -z \"${{OMABOX_RELAY_FAIL+x}}\" ] || exit 2\n\
                 echo 'omabox-relay: socket path too long' >&2\nexit 1"
            ),
        );
        assert!(takes_env_fd(new.as_bytes()), "1.0's relay takes --env-fd");
        assert!(
            NO_SOCKET.len() >= 108,
            "NO_SOCKET: longer than any socket's path"
        );
        // (one that cannot be run: the call says so, as it did)
        assert!(takes_env_fd(b"/nonexistent/omabox-relay"));
        for f in [old, new] {
            let _ = std::fs::remove_dir_all(std::path::Path::new(&f).parent().unwrap());
        }
    }
}
