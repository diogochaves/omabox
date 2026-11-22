//! `omabox look app [--keys "K..."] [--type TEXT] [--after DURATION] [--timeout 10s] [-o FILE]
//! [--keep-box] [UP OPTIONS] -- CMD [ARG...]` and `omabox look plugin DIR [--open] [-o FILE]
//! [--keep-box] [UP OPTIONS]` (#461): "I built this, does it look right and did it log errors?" in one
//! call. A throwaway box of its own (as a throwaway `run`'s), then the commands an agent would chain:
//! for an app `run -d`, its window waited for, keys sent, `wait still`, `shot --window`, the program's
//! output read; for a plugin `up --plugin`, its state, the bar shot, with --open its panel summoned and
//! shot, the shell's warnings. The box is taken down however this ends, unless --keep-box. With a box
//! up already (-b NAME's, else the session's, as `run` picks one) the look is into that box, left as
//! it is (`pick`); `omabox look` with no kind is such a box now, with what it logged since the last
//! look (`plain`; finding 735). Nothing new
//! is isolated here: every step is one of omabox's own commands, run as a subprocess, so S1-S10 hold
//! as they are. One short report on stdout; exit 0 when it found no problem, 1 when it did, and an
//! error of omabox's own as its other commands do.
//!
//! Each kind is a file here (`app.rs`: app and page; `plugin.rs`, `theme.rs`, `plain.rs`); this one
//! holds what they share: the options, the box picked and started, the report (`Look`, `report`)
//! and the marks between looks.

use crate::args::optarg;
use crate::boxes::{box_alive, need_box};
use crate::cat;
use crate::exit::say;
use crate::exit::{Ctx, Exit, Res, out};
use crate::num::ms_duration;
use crate::select::{env_names_box, select_box, session_boxes};
use crate::shell::{self, Re};
use crate::sig::Teardown;
use serde_json::Value;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::time::{Duration, Instant};

mod app;
mod plain;
mod plugin;
mod theme;
use app::*;
use plain::*;
use plugin::*;
use theme::*;

/// How much of the program's output the report shows, and how wide a line.
const OUTPUT_LINES: usize = 40;
const LINE_WIDTH: usize = 200;
/// Problems shown at most (the rest counted).
const PROBLEMS_MAX: usize = 20;

/// A line of a program's output that names a problem: an error or a crash in any wording a toolkit
/// uses, a QML file and line. Matched on the line in lower case.
const PROBLEM_RE: &str = r"(^|[^a-z])(error|errors|critical|fatal|traceback|panicked|exception|segmentation fault|core dumped|aborted)([^a-z]|$)|\.qml:[0-9]+";

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    // `look -b NAME page FILE`, and `omabox -b NAME look page FILE`, which front.rs makes that
    // (#471): the kind after -b NAME is the kind, -b NAME its option.
    let moved = kind_first(args);
    let args = moved.as_deref().unwrap_or(args);
    match args.first().map(Vec::as_slice) {
        Some(b"app") => app(ctx, &args[1..]),
        Some(b"plugin") => plugin(ctx, &args[1..]),
        Some(b"theme") => theme(ctx, &args[1..]),
        Some(b"page") => page(ctx, &args[1..]),
        Some(b"-h" | b"--help") => crate::help::usage("look"),
        None => plain(ctx, args),
        Some(o) if o.starts_with(b"-") => plain(ctx, args),
        Some(k) => Err(ctx.die(cat!(
            "look: app, plugin, theme or page, got ",
            k,
            ": ",
            kind_hint(args)
        ))),
    }
}

/// ARGS that start `-b NAME KIND`, as `KIND -b NAME ...`; None for any others.
fn kind_first(args: &[Vec<u8>]) -> Option<Vec<Vec<u8>>> {
    match args {
        [b, n, k, rest @ ..]
            if matches!(b.as_slice(), b"-b" | b"--box")
                && matches!(k.as_slice(), b"app" | b"plugin" | b"theme" | b"page") =>
        {
            Some([&[k.clone(), b.clone(), n.clone()], rest].concat())
        }
        _ => None,
    }
}

/// The look ARGS were meant as, when their first word names no kind (#471: `look hello -- python3
/// hello.py`): `look app -- CMD` for a command after `--`, `look page` for a page or URL, `look
/// plugin`/`look theme` for a folder with a plugin's manifest or a theme's colours, else the four.
fn kind_hint(args: &[Vec<u8>]) -> Vec<u8> {
    let w = &args[0];
    let ws = String::from_utf8_lossy(w);
    if let Some(i) = args.iter().position(|a| a == b"--") {
        let mut c = b"omabox look app --".to_vec();
        for a in &args[i + 1..] {
            c.push(b' ');
            c.extend(shell::quote_q(a));
        }
        if i + 1 < args.len() {
            return c;
        }
    }
    let q = shell::quote_q(w);
    let lw = ws.to_ascii_lowercase();
    // (#478: `look shot -o shot.png`, meaning the box now: look with no kind, its -b and -o kept)
    if matches!(
        lw.as_str(),
        "shot" | "screenshot" | "screen" | "snapshot" | "now"
    ) {
        let mut c = b"omabox look".to_vec();
        for o in args.windows(2) {
            if matches!(o[0].as_slice(), b"-b" | b"--box" | b"-o") {
                c.extend(cat!(" ", o[0], " ", shell::quote_q(&o[1])));
            }
        }
        if !args.iter().any(|a| a == b"-o") {
            c.extend(b" -o FILE");
        }
        return cat!(
            c,
            " (no kind) is a shot of the box as it is now; omabox shot -o FILE the image alone"
        );
    }
    if lw.starts_with("http://")
        || lw.starts_with("https://")
        || lw.ends_with(".html")
        || lw.ends_with(".htm")
    {
        return cat!("omabox look page ", q);
    }
    let p = std::path::Path::new(OsStr::from_bytes(w));
    if p.join("manifest.json").is_file() {
        return cat!("omabox look plugin ", q);
    }
    if p.join("colors.toml").is_file() {
        return cat!("omabox look theme ", q);
    }
    b"omabox look app -- CMD, omabox look plugin DIR, omabox look theme DIR or omabox look page FILE|URL"
        .to_vec()
}

/// The subfolders of DIR (by name, sorted) that HAS takes for the kind's folder (a plugin's manifest
/// with an id, a theme's colors.toml).
fn subfolders_with(dir: &[u8], has: &dyn Fn(&[u8]) -> bool) -> Vec<Vec<u8>> {
    let base = dir
        .strip_suffix(b"/")
        .filter(|b| !b.is_empty())
        .unwrap_or(dir);
    shell::glob(&shell::path(base), true)
        .into_iter()
        .filter(|n| has(&cat!(base, "/", n)))
        .collect()
}

/// After `look KIND DIR`'s refusal of a folder that is not the kind's (#478: `look plugin .` where
/// the plugin is `./cups`): the subfolders that are, and the look (ARGS, the words after KIND) again
/// with the first of them in DIR's place. Nothing when none is.
fn subfolder_hint(kind: &str, args: &[Vec<u8>], dir: &[u8], subs: &[Vec<u8>]) -> Vec<u8> {
    let Some(first) = subs.first() else {
        return Vec::new();
    };
    let base = dir
        .strip_suffix(b"/")
        .filter(|b| !b.is_empty())
        .unwrap_or(dir);
    let path = cat!(base, "/", first);
    let mut cmd = cat!("omabox look ", kind);
    let mut put = false;
    for a in args {
        cmd.push(b' ');
        if !put && a.as_slice() == dir {
            cmd.extend(shell::quote_q(&path));
            put = true;
        } else {
            cmd.extend(shell::quote_q(a));
        }
    }
    let shown: Vec<u8> = subs
        .iter()
        .take(4)
        .map(Vec::as_slice)
        .collect::<Vec<_>>()
        .join(b", ".as_slice());
    let more = if subs.len() > 4 {
        format!(" and {} more", subs.len() - 4)
    } else {
        String::new()
    };
    let (word, verb) = if subs.len() == 1 {
        ("subfolder", "is one")
    } else {
        ("subfolders", "are")
    };
    cat!("; its ", word, " ", shown, more, " ", verb, ": ", cmd)
}

/// An option look (of KIND, or the plain look) does not take: unknown_opt, but for an option meant
/// as "take a shot" (#478: `look --shot`, whose nearest name was page's --shots) what every look does.
fn look_unknown_opt(ctx: &Ctx, kind: Option<&str>, o: &[u8]) -> Exit {
    match shot_opt_hint(kind, o) {
        Some(m) => ctx.usage_die(m),
        None => crate::args::unknown_opt(ctx, "look", o),
    }
}

/// look_unknown_opt's line for O when it asks for a shot (`--shot`, `--screenshot`, `--png`): every
/// look takes one, -o FILE says where.
fn shot_opt_hint(kind: Option<&str>, o: &[u8]) -> Option<Vec<u8>> {
    let w = o.split(|&c| c == b'=').next().unwrap_or_default();
    if !matches!(
        w,
        b"--shot" | b"--screenshot" | b"--snapshot" | b"--png" | b"--image"
    ) {
        return None;
    }
    let (label, cmd) = match kind {
        Some(k) => (cat!("look ", k), cat!("omabox look ", k, " ... -o FILE")),
        None => (b"look".to_vec(), b"omabox look -o FILE".to_vec()),
    };
    let more: &[u8] = if kind == Some("page") {
        b"; --shots N takes N over time"
    } else {
        b""
    };
    Some(cat!(
        label,
        ": unknown option ",
        o,
        ": every look takes a shot (its report's screenshot: line); -o FILE says where: ",
        cmd,
        more,
        " (omabox help look)"
    ))
}

/// What `look app` was asked for.
#[derive(Default)]
struct App {
    /// keys' words in the order given: --keys' combos, and `-t TEXT` for each --type.
    keys: Vec<Vec<u8>>,
    after: Option<Vec<u8>>,
    timeout: Duration,
    out: Option<Vec<u8>>,
    keep: bool,
    up: Vec<Vec<u8>>,
    cmd: Vec<Vec<u8>>,
    /// plugin's DIR, and --open.
    dir: Option<Vec<u8>>,
    open: bool,
    /// page: its shots over --after (a contact sheet).
    shots: u32,
    /// -b NAME; --fresh (a throwaway box even when one is up); and what pick found: a box that was
    /// up already, the caller's own, where nothing is started or taken down.
    boxname: Option<Vec<u8>>,
    fresh: bool,
    own: bool,
}

/// The box a look works in, as `run` picks one: a box that is up (-b NAME's, else this session's)
/// is looked into as it is, and left as it is; with none up, or with --fresh, a throwaway `look-PID`,
/// taken down by the guard however the look ends (unless --keep-box). A box asked for by name, or one
/// that was up and is gone, is never replaced by a throwaway (#295's rule for run): that is said.
fn pick(ctx: &mut Ctx, a: &mut App, me: u32) -> Res<(Vec<u8>, Option<Teardown>)> {
    if a.fresh {
        if a.boxname.is_some() {
            return Err(ctx.die("look: --fresh starts a box of its own: not with -b NAME"));
        }
    } else {
        let given = a.boxname.clone().unwrap_or_default();
        select_box(ctx, &given)?;
        if box_alive(ctx)? {
            need_box(ctx)?;
            if !a.up.is_empty() {
                say(cat!(
                    "box '",
                    ctx.name,
                    "' is already up: ",
                    a.up.join(&b' '),
                    " ignored (look --fresh for a box of its own)"
                ));
            }
            a.own = true;
            a.keep = false;
            return Ok((ctx.name.clone(), None));
        }
        if !given.is_empty()
            || (ctx.name_derived && !session_boxes(ctx).is_empty())
            || env_names_box()
            || shell::path(&ctx.d).is_dir()
            || ctx.in_boxes(cat!(".expired-", ctx.name)).is_file()
            || ctx.in_boxes(cat!(".taken-", ctx.name)).is_file()
        {
            need_box(ctx)?;
        }
    }
    let name = format!("look-{me}").into_bytes();
    select_box(ctx, &name)?;
    let guard = (!a.keep).then(|| Teardown::new(name.clone()));
    // (for the caller's next command, which may look for it: need_box names it, #471)
    if guard.is_some() {
        crate::whose::write_look(ctx, &name, &crate::whose::me());
    }
    Ok((name, guard))
}

/// KIND's options (app, plugin): an app's program after --, a plugin's DIR as its one word.
fn parse(ctx: &Ctx, args: &[Vec<u8>], kind: &str) -> Res<App> {
    let mut a = App {
        timeout: Duration::from_secs(10),
        ..App::default()
    };
    let dur = |ctx: &Ctx, opt: &[u8], v: &[u8]| -> Res<Duration> {
        ms_duration(v)
            .and_then(|ms| u64::try_from(ms).ok())
            .map(Duration::from_millis)
            .ok_or_else(|| {
                ctx.die(cat!(
                    "look: ",
                    opt,
                    " takes a duration (500ms, 2s), got ",
                    v
                ))
            })
    };
    let mut i = 0;
    while i < args.len() {
        let o = &args[i];
        let val = |i: usize| -> Res<&Vec<u8>> {
            optarg(ctx, o, args.get(i + 1))?;
            Ok(&args[i + 1])
        };
        match o.as_slice() {
            b"--" if kind == "app" => {
                a.cmd = args[i + 1..].to_vec();
                break;
            }
            b"--open" if kind == "plugin" => a.open = true,
            b"--hold" if kind == "page" => {
                // KEY DURATION: down, held that long (real frames go by), up.
                let k = val(i)?.clone();
                optarg(ctx, &args[i + 1], args.get(i + 2))?;
                let d = args[i + 2].clone();
                dur(ctx, o, &d)?;
                a.keys
                    .extend([cat!("down:", k), b"-s".to_vec(), d, cat!("up:", k)]);
                i += 2;
            }
            b"--shots" if kind == "page" => {
                let v = val(i)?;
                a.shots = std::str::from_utf8(v)
                    .ok()
                    .and_then(|n| n.parse().ok())
                    .filter(|n| (1..=24).contains(n))
                    .ok_or_else(|| ctx.die(cat!("look page: --shots takes 1 to 24, got ", v)))?;
                i += 1;
            }
            b"--keys" if matches!(kind, "app" | "page") => {
                a.keys.extend(
                    val(i)?
                        .split(|c| c.is_ascii_whitespace())
                        .filter(|w| !w.is_empty())
                        .map(<[u8]>::to_vec),
                );
                i += 1;
            }
            b"--type" if matches!(kind, "app" | "page") => {
                a.keys.extend([b"-t".to_vec(), val(i)?.clone()]);
                i += 1;
            }
            b"--after" if matches!(kind, "app" | "page") => {
                let v = val(i)?;
                dur(ctx, o, v)?;
                a.after = Some(v.clone());
                i += 1;
            }
            b"--timeout" => {
                a.timeout = dur(ctx, o, val(i)?)?;
                i += 1;
            }
            b"-o" => {
                a.out = Some(val(i)?.clone());
                i += 1;
            }
            b"--keep-box" => a.keep = true,
            b"--fresh" => a.fresh = true,
            b"-b" | b"--box" => {
                a.boxname = Some(val(i)?.clone());
                i += 1;
            }
            b"-h" | b"--help" => {
                crate::help::usage("look")?;
                return Err(Exit(0));
            }
            _ if crate::cmd::run::UP_VALUED
                .iter()
                .any(|u| u.as_bytes() == o.as_slice()) =>
            {
                a.up.extend([o.clone(), val(i)?.clone()]);
                i += 1;
            }
            _ if crate::cmd::run::UP_FLAGS
                .iter()
                .any(|u| u.as_bytes() == o.as_slice())
                || o == b"--env" =>
            {
                if o == b"--env" {
                    a.up.extend([o.clone(), val(i)?.clone()]);
                    i += 1;
                } else {
                    a.up.push(o.clone());
                }
            }
            _ if o.starts_with(b"-") => return Err(look_unknown_opt(ctx, Some(kind), o)),
            _ if kind != "app" && a.dir.is_none() => a.dir = Some(o.clone()),
            _ if kind != "app" => {
                return Err(ctx.die(cat!("look ", kind, ": one DIR (its folder), got ", o)));
            }
            _ => {
                return Err(ctx.die(cat!(
                    "look app: the program goes after --: omabox look app [OPTIONS] -- CMD [ARG...], got ",
                    o
                )));
            }
        }
        i += 1;
    }
    if kind == "page" && a.dir.is_none() {
        return Err(ctx.die("look page: its file or URL: omabox look page FILE|URL"));
    }
    if kind != "app" && a.dir.is_none() {
        return Err(ctx.die(cat!(
            "look ",
            kind,
            ": its folder: omabox look ",
            kind,
            " DIR"
        )));
    }
    if kind == "app" && a.cmd.is_empty() {
        return Err(ctx.die(
            "look app: the program goes after --: omabox look app [OPTIONS] -- CMD [ARG...]",
        ));
    }
    Ok(a)
}

/// One of omabox's own commands on the look's box: its status, stdout and stderr.
struct Ran {
    ok: bool,
    code: i32,
    out: Vec<u8>,
    err: Vec<u8>,
}

/// `omabox ARGS`, as main's foreground child (sig::run_child: a signal's stop waits for it, and a
/// Teardown forwards its signal to look's up, finding 754), its stdout and stderr read whole.
fn ob(args: &[&[u8]]) -> Ran {
    let mut c = crate::sig::omabox_cmd();
    c.args(args.iter().map(|a| OsStr::from_bytes(a)))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let r = crate::sig::run_child(c, false, |ch| {
        let err = ch.stderr.take().map(|mut e| {
            std::thread::spawn(move || {
                let mut v = Vec::new();
                let _ = std::io::Read::read_to_end(&mut e, &mut v);
                v
            })
        });
        let mut out = Vec::new();
        if let Some(mut o) = ch.stdout.take() {
            let _ = std::io::Read::read_to_end(&mut o, &mut out);
        }
        (out, err.and_then(|t| t.join().ok()).unwrap_or_default())
    });
    match r {
        Ok(((out, err), (rc, sig))) => Ran {
            ok: rc == 0 && sig.is_none(),
            // (Output's code(): none for a signal, 1 then)
            code: if sig.is_some() { 1 } else { rc },
            out,
            err,
        },
        Err(_) => Ran {
            ok: false,
            code: 125,
            out: Vec::new(),
            err: b"omabox: could not start omabox\n".to_vec(),
        },
    }
}

/// omabox's own warnings in a command's stderr (what an agent would have seen), each a problem.
fn warnings(err: &[u8], into: &mut Vec<Vec<u8>>) {
    for l in err.split(|&c| c == b'\n') {
        if let Some(w) = l.strip_prefix(b"omabox: warning: ") {
            into.push(w.to_vec());
        }
    }
}

/// A `run -d` whose command ended at once (its stderr ERR): how ("exited with status 3", "was killed
/// by signal 11"), and its log.
pub fn ended_at_once(err: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let re = Re::new(r"the command (.*) at once in box '[^']*'.*\(log: (/[^)]*)\)")?;
    err.split(|&c| c == b'\n')
        .find_map(|l| Some((re.group(l, 1)?, re.group(l, 2)?)))
}

/// The lines of OUTPUT that name a problem (PROBLEM_RE).
pub fn problem_lines(output: &[u8]) -> Vec<Vec<u8>> {
    let Some(re) = Re::new(PROBLEM_RE) else {
        return Vec::new();
    };
    output
        .split(|&c| c == b'\n')
        .filter(|l| re.is_match(&l.to_ascii_lowercase()))
        .map(<[u8]>::to_vec)
        .collect()
}

/// A box's mark of its last look, in its dir (host side: nothing in the box reads or writes it).
const MARK_FILE: &str = "look.mark";

/// Where each log of the box ends now: its run -d logs by size, the shell's by its first line (a
/// restarted shell starts its log again) and its length.
fn mark_now(ctx: &Ctx) -> Value {
    let mut logs = serde_json::Map::new();
    if let Ok(rd) = std::fs::read_dir(ctx.at("home")) {
        for e in rd.flatten() {
            let f = e.file_name();
            let f = f.to_string_lossy();
            if f.starts_with("run-") && f.ends_with(".log") {
                let len = e.metadata().map(|m| m.len()).unwrap_or(0);
                logs.insert(f.into_owned(), Value::from(len));
            }
        }
    }
    let (first, len) = shell_log_mark(&ctx.d);
    serde_json::json!({
        "logs": logs,
        "shell_first": String::from_utf8_lossy(&first),
        "shell_len": len,
    })
}

/// The first line of the box's shell log (4 KiB of it at most) and its length, read from the
/// file through the box's folder, never the whole log (S7: a box can make its log a sparse
/// terabyte; finding 755).
fn shell_log_mark(d: &[u8]) -> (Vec<u8>, u64) {
    let Some(f) = crate::boxes::box_home_file(d, "shell.log") else {
        return (Vec::new(), 0);
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut head = Vec::new();
    let _ = std::io::Read::read_to_end(&mut std::io::Read::take(f, 4096), &mut head);
    let first = head.split(|&c| c == b'\n').next().unwrap_or(b"").to_vec();
    (first, len)
}

/// The box's shell log from byte FROM on (all of it with none), READ_CAP bytes at most: the last
/// READ_CAP when there is more.
fn shell_log_since(d: &[u8], from: Option<u64>) -> Vec<u8> {
    use std::io::{Read, Seek, SeekFrom};
    let Some(mut f) = crate::boxes::box_home_file(d, "shell.log") else {
        return Vec::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let cap = crate::boxes::READ_CAP;
    let from = from.filter(|&w| w <= len).unwrap_or(0);
    let at = from.max(len.saturating_sub(cap));
    let mut v = Vec::new();
    if f.seek(SeekFrom::Start(at)).is_ok() {
        let _ = f.take(cap).read_to_end(&mut v);
    }
    v
}

fn mark_read(ctx: &Ctx) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(ctx.at(MARK_FILE)).ok()?).ok()
}

fn mark_write(ctx: &Ctx, m: &Value) {
    let _ = std::fs::write(ctx.at(MARK_FILE), m.to_string());
}

/// The part of a log written since OLD's mark of it: all of it with no mark, or when it is shorter
/// than it was (another file under the name).
pub fn since(all: &[u8], was: Option<u64>) -> &[u8] {
    match was.and_then(|w| usize::try_from(w).ok()) {
        Some(w) if w <= all.len() => &all[w..],
        _ => all,
    }
}

/// up NAME for a look: its own process, its errors the caller's; --owner and no idle limit unless
/// the box is kept; EXTRA (a plugin's --plugin DIR) before the caller's UP OPTIONS. Its warnings are
/// problems.
fn start(ctx: &Ctx, a: &App, name: &[u8], me: u32, extra: &[&[u8]], l: &mut Look) -> Res<Vec<u8>> {
    if a.own {
        return Ok(Vec::new());
    }
    let owner = me.to_string();
    let mut up: Vec<&[u8]> = vec![b"up", name];
    if !a.keep {
        up.extend([b"--idle".as_slice(), b"0", b"--owner", owner.as_bytes()]);
    }
    up.extend_from_slice(extra);
    up.extend(a.up.iter().map(Vec::as_slice));
    let r = crate::sig::teardown_forwarding(|| ob(&up));
    if !r.ok {
        let _ = std::io::Write::write_all(&mut std::io::stderr(), &r.err);
        return Err(ctx.die("look: the box did not start"));
    }
    warnings(&r.err, &mut l.problems);
    Ok(r.err)
}

/// The last non-empty line of a command's stdout (shot's path).
fn last_line(o: &[u8]) -> Option<Vec<u8>> {
    o.split(|&c| c == b'\n')
        .rfind(|l| !l.is_empty())
        .map(<[u8]>::to_vec)
}

fn suffixed(path: &[u8], suffix: &str) -> Vec<u8> {
    let slash = path.iter().rposition(|&c| c == b'/').map_or(0, |i| i + 1);
    match path[slash..].iter().rposition(|&c| c == b'.') {
        Some(d) => cat!(&path[..slash + d], "-", suffix, &path[slash + d..]),
        None => cat!(path, "-", suffix),
    }
}

/// A problem's LINE cut to WIDTH characters as cut_line cuts it, except the commands it ends with: a
/// last `(...)` that names an omabox command is kept whole after the cut (#471: `its Hyprland did not
/// answer (h… (+62 chars)` lost `omabox log -b NAME; omabox down NAME`).
pub fn cut_keep_command(l: &[u8], width: usize) -> Vec<u8> {
    let plain = crate::cmd::log::cut_line(l, width);
    if plain.len() == l.len() || !l.ends_with(b")") {
        return plain;
    }
    let Some(open) = l.iter().rposition(|&c| c == b'(') else {
        return plain;
    };
    let tail = &l[open..];
    let n = |v: &[u8]| v.iter().filter(|&&b| (b & 0xc0) != 0x80).count();
    if !crate::exit::contains(tail, b"omabox ") || n(tail) * 2 > width {
        return plain;
    }
    let head = l[..open].strip_suffix(b" ").unwrap_or(&l[..open]);
    let mut v = crate::cmd::log::cut_line(head, width - n(tail) - 1);
    v.push(b' ');
    v.extend_from_slice(tail);
    v
}

/// The report, as the issue drew it: HEAD's lines (`screenshot: PATH`, `window: ...`), the problems,
/// notes, OUTPUT's first lines (none: no such part), the box kept.
/// It reaches the terminal plain (finding 747).
pub fn report(
    head: &[(&str, &[u8])],
    problems: &[Vec<u8>],
    notes: &[Vec<u8>],
    output: Option<&[u8]>,
    kept: Option<&[u8]>,
) -> Vec<u8> {
    let cut = |l: &[u8]| crate::cmd::log::cut_line(l, LINE_WIDTH);
    let mut r = Vec::new();
    for (k, v) in head {
        r.extend(cat!(*k, ": ", *v, "\n"));
    }
    if problems.is_empty() {
        r.extend(b"problems: none\n");
    } else {
        r.extend(b"problems:\n");
        // (one of several lines, as a command's stderr can be: each line cut on its own)
        for p in problems.iter().take(PROBLEMS_MAX) {
            let p = p.strip_suffix(b"\n").unwrap_or(p);
            for l in p.split(|&c| c == b'\n').filter(|l| !l.is_empty()) {
                r.extend(cat!("  ", cut_keep_command(l, LINE_WIDTH), "\n"));
            }
        }
        if problems.len() > PROBLEMS_MAX {
            r.extend(format!("  ... and {} more\n", problems.len() - PROBLEMS_MAX).into_bytes());
        }
    }
    for n in notes {
        r.extend(cat!("note: ", n, "\n"));
    }
    if let Some(output) = output {
        let lines: Vec<&[u8]> = output
            .strip_suffix(b"\n")
            .unwrap_or(output)
            .split(|&c| c == b'\n')
            .filter(|_| !output.is_empty())
            .collect();
        if lines.is_empty() {
            r.extend(b"output: none\n");
        } else {
            let head = if lines.len() > OUTPUT_LINES {
                format!("output (first {OUTPUT_LINES} of {} lines):\n", lines.len())
            } else {
                "output:\n".to_string()
            };
            r.extend(head.into_bytes());
            for l in lines.iter().take(OUTPUT_LINES) {
                r.extend(cat!("  ", cut(l), "\n"));
            }
        }
    }
    if let Some(n) = kept {
        r.extend(cat!(
            "box: ",
            n,
            " kept (omabox -b ",
            n,
            " ...; omabox down ",
            n,
            ")\n"
        ));
    }
    // What the box said is in it (a window's class and title, a page's console, a program's output,
    // the shell's warnings, a step's error): every control character but newline and tab, and every
    // byte that is no UTF-8, as `?` (S7). The report itself has none of its own.
    crate::term::plain_cap(&r, usize::MAX)
}

/// One look's report as each kind gathers it: its HEAD lines in order, the problems, the notes, the
/// program's output (none: no such part) and the box it keeps; `finish` prints it through `report`
/// and ends with 1 when it found a problem.
struct Look {
    head: Vec<(&'static str, Vec<u8>)>,
    problems: Vec<Vec<u8>>,
    notes: Vec<Vec<u8>>,
    output: Option<Vec<u8>>,
    kept: Option<Vec<u8>>,
    /// (plain's, after a look before: its problems said as new; one seen then is not gone, only not
    /// repeated)
    new: bool,
}

impl Look {
    /// A look whose box is KEPT (--keep-box's name), or none.
    fn new(kept: Option<&[u8]>) -> Self {
        Look {
            head: Vec::new(),
            problems: Vec::new(),
            notes: Vec::new(),
            output: None,
            kept: kept.map(<[u8]>::to_vec),
            new: false,
        }
    }

    fn head(&mut self, k: &'static str, v: impl AsRef<[u8]>) {
        self.head.push((k, v.as_ref().to_vec()));
    }

    fn problem(&mut self, p: Vec<u8>) {
        self.problems.push(p);
    }

    fn note(&mut self, n: Vec<u8>) {
        self.notes.push(n);
    }

    /// In a box that was up (OWN): its name, and the mark that what this look reported is not new
    /// to the next one.
    fn own_box(&mut self, ctx: &Ctx, own: bool, name: &[u8]) {
        if own {
            self.head("box", name);
            mark_write(ctx, &mark_now(ctx));
        }
    }

    /// The report's bytes, as `report` makes them (plain on the terminal, S7).
    fn text(&self) -> Vec<u8> {
        let head: Vec<(&str, &[u8])> = self.head.iter().map(|(k, v)| (*k, v.as_slice())).collect();
        let mut rep = report(
            &head,
            &self.problems,
            &self.notes,
            self.output.as_deref(),
            self.kept.as_deref(),
        );
        if self.new {
            if let Some(at) = rep.windows(10).position(|w| w == b"\nproblems:") {
                rep.splice(at + 1..at + 1, b"new ".iter().copied());
            }
        }
        rep
    }

    /// The report on stdout; exit 0 with no problem, 1 with any.
    fn finish(self) -> Res<()> {
        out(self.text())?;
        if self.problems.is_empty() {
            Ok(())
        } else {
            Err(Exit(1))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #478: a folder whose subfolder is the plugin or theme: the look again with it.
    #[test]
    fn a_subfolder_that_is_the_kind_named() {
        let t = std::env::temp_dir().join(format!("omabox-sub478-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        for d in ["cups", "wifi", "notes", ".hidden"] {
            std::fs::create_dir_all(t.join(d)).unwrap();
        }
        std::fs::write(t.join("cups/manifest.json"), r#"{"id": "example.cups"}"#).unwrap();
        std::fs::write(t.join("wifi/manifest.json"), r#"{"id": "example.wifi"}"#).unwrap();
        std::fs::write(t.join(".hidden/manifest.json"), r#"{"id": "x.y"}"#).unwrap();
        std::fs::write(t.join("notes/manifest.json"), "{}").unwrap();
        std::fs::write(t.join("ember.toml"), "").unwrap();
        let dir = t.to_str().unwrap().as_bytes().to_vec();
        let subs = subfolders_with(&dir, &|d| plugin_id(d).is_some());
        assert_eq!(
            subs,
            w(&["cups", "wifi"]),
            "...an id's manifest, no dot folder"
        );
        let h = |args: &[&str], dir: &str, subs: &[&str]| {
            String::from_utf8(subfolder_hint("plugin", &w(args), dir.as_bytes(), &w(subs))).unwrap()
        };
        assert_eq!(
            h(&[".", "--open"], ".", &["cups"]),
            "; its subfolder cups is one: omabox look plugin ./cups --open"
        );
        assert_eq!(
            h(&["-o", "a.png", "x/"], "x/", &["cups", "wifi"]),
            "; its subfolders cups, wifi are: omabox look plugin -o a.png x/cups"
        );
        assert_eq!(
            h(&["p"], "p", &["a", "b", "c", "d", "e", "f"]),
            "; its subfolders a, b, c, d and 2 more are: omabox look plugin p/a"
        );
        assert_eq!(h(&["."], ".", &[]), "", "...none: nothing");
        let theme =
            String::from_utf8(subfolder_hint("theme", &w(&["."]), b".", &w(&["ember"]))).unwrap();
        assert_eq!(
            theme,
            "; its subfolder ember is one: omabox look theme ./ember"
        );
        std::fs::remove_dir_all(&t).unwrap();
    }

    /// #478: `look shot` and `look --shot` meant the box now, or a look's shot.
    #[test]
    fn a_shot_asked_for_names_look_dash_o() {
        let h = |s: &[&str]| String::from_utf8(kind_hint(&w(s))).unwrap();
        assert_eq!(
            h(&["shot", "-o", "shot.png"]),
            "omabox look -o shot.png (no kind) is a shot of the box as it is now; omabox shot -o FILE the image alone"
        );
        assert!(
            h(&["screenshot", "--help"]).starts_with("omabox look -o FILE (no kind)"),
            "{}",
            h(&["screenshot", "--help"])
        );
        assert!(h(&["shot", "-b", "x"]).starts_with("omabox look -b x -o FILE (no kind)"));
        let o = |k: Option<&str>, o: &str| {
            shot_opt_hint(k, o.as_bytes()).map(|v| String::from_utf8(v).unwrap())
        };
        assert_eq!(
            o(None, "--shot").unwrap(),
            "look: unknown option --shot: every look takes a shot (its report's screenshot: line); -o FILE says where: omabox look -o FILE (omabox help look)"
        );
        assert!(
            o(Some("theme"), "--screenshot=x.png")
                .unwrap()
                .contains("look theme: unknown option --screenshot=x.png: every look takes a shot (its report's screenshot: line); -o FILE says where: omabox look theme ... -o FILE (omabox")
        );
        assert!(
            o(Some("page"), "--shot")
                .unwrap()
                .contains("; --shots N takes N over time")
        );
        assert_eq!(
            o(None, "--nope"),
            None,
            "...another option: unknown_opt's line"
        );
    }

    pub(super) fn w(v: &[&str]) -> Vec<Vec<u8>> {
        v.iter().map(|s| s.as_bytes().to_vec()).collect()
    }

    /// #471: `-b NAME` before the kind (as `omabox -b NAME look page FILE` arrives) is the kind's.
    #[test]
    fn b_before_the_kind() {
        assert_eq!(
            kind_first(&w(&["-b", "x", "page", "f.html"])),
            Some(w(&["page", "-b", "x", "f.html"]))
        );
        assert_eq!(
            kind_first(&w(&["--box", "x", "app", "--", "foot"])),
            Some(w(&["app", "--box", "x", "--", "foot"]))
        );
        assert_eq!(kind_first(&w(&["-b", "x", "-o", "f.png"])), None);
        assert_eq!(kind_first(&w(&["page", "f.html", "-b", "x"])), None);
    }

    /// #471: a problem shortened keeps the commands it ends with; each of its lines is cut on its
    /// own.
    #[test]
    fn a_cut_problem_keeps_its_commands() {
        let hung = format!(
            "omabox: box 'look-1': {} its Hyprland did not answer (hung? omabox log -b look-1; omabox down look-1)",
            "x".repeat(190)
        );
        let c = String::from_utf8(cut_keep_command(hung.as_bytes(), 200)).unwrap();
        assert!(
            c.ends_with(" (hung? omabox log -b look-1; omabox down look-1)"),
            "{c}"
        );
        assert!(c.contains("… (+"), "{c}");
        // Short enough: as it is; no omabox command at its end: cut_line's cut.
        assert_eq!(
            cut_keep_command(b"short (omabox x)", 200),
            b"short (omabox x)"
        );
        let long = "y".repeat(250) + " (not a command)";
        assert_eq!(
            cut_keep_command(long.as_bytes(), 200),
            crate::cmd::log::cut_line(long.as_bytes(), 200)
        );
        let two = format!("shot: omabox: warning: the shell is gone\n{hung}\n");
        let r = String::from_utf8(report(&[], &[two.into_bytes()], &[], None, None)).unwrap();
        assert!(
            r.starts_with(
                "problems:\n  shot: omabox: warning: the shell is gone\n  omabox: box 'look-1': x"
            ),
            "{r}"
        );
        assert!(
            r.ends_with("(hung? omabox log -b look-1; omabox down look-1)\n"),
            "{r}"
        );
    }

    /// What is new in a log since its mark: from there on; all of it unmarked, or shorter than it was.
    #[test]
    fn a_log_since_its_mark() {
        assert_eq!(since(b"one\ntwo\n", Some(4)), b"two\n");
        assert_eq!(since(b"one\ntwo\n", None), b"one\ntwo\n");
        assert_eq!(since(b"new\n", Some(40)), b"new\n");
        assert_eq!(since(b"one\n", Some(4)), b"");
    }

    // #471: the command a kind-less look was meant as.
    #[test]
    fn kind_hint_names_the_command() {
        let v = |s: &[&str]| s.iter().map(|w| w.as_bytes().to_vec()).collect::<Vec<_>>();
        let h = |s: &[&str]| String::from_utf8(kind_hint(&v(s))).unwrap();
        assert_eq!(
            h(&["hello", "--", "python3", "hello.py"]),
            "omabox look app -- python3 hello.py"
        );
        assert_eq!(h(&["x", "--", "a b"]), "omabox look app -- a\\ b");
        assert_eq!(h(&["game/index.html"]), "omabox look page game/index.html");
        assert_eq!(h(&["https://x.test"]), "omabox look page https://x.test");
        assert!(h(&["hello"]).starts_with("omabox look app -- CMD, "));
        assert!(h(&["hello", "--"]).starts_with("omabox look app -- CMD, "));
    }

    #[test]
    fn problems_in_output() {
        let o = b"hi\nqrc:/Main.qml:42: TypeError: x\nwarn: wayland.c:1888: compositor does not implement\nERROR: boom\nterror is not one\nthread 'main' panicked at src/main.rs\n";
        let p = problem_lines(o);
        assert_eq!(
            p,
            [
                b"qrc:/Main.qml:42: TypeError: x".to_vec(),
                b"ERROR: boom".to_vec(),
                b"thread 'main' panicked at src/main.rs".to_vec()
            ]
        );
    }

    #[test]
    fn a_command_that_ended_at_once() {
        let e = b"omabox: the command exited with status 3 at once in box 'look-1', and left nothing running (log: /run/user/1000/omabox/look-1/home/run-1.log)\nomabox: its log ends:\n";
        assert_eq!(
            ended_at_once(e),
            Some((
                b"exited with status 3".to_vec(),
                b"/run/user/1000/omabox/look-1/home/run-1.log".to_vec()
            ))
        );
        assert_eq!(ended_at_once(b"omabox: no such box\n"), None);
    }

    /// Finding 747: what the box said reaches the terminal plain (a window's title, a page's
    /// console, a program's output, a note), newline and tab kept.
    #[test]
    fn the_report_is_plain() {
        let title = report_window_line(false, "foot", "t\x1b]52;c;aGk=\x07", "1x1", "tiled", "0x1");
        let r = report(
            &[("window", &title)],
            &[b"app: error \x1b[2J\x9b".to_vec()],
            &[b"note\x1b]8;;x\x07".to_vec()],
            Some(b"out\tput \x1b[31mred\r\n\xff"),
            None,
        );
        assert_eq!(
            String::from_utf8(r).unwrap(),
            "window: foot \"t?]52;c;aGk=?\" 1x1 (tiled)\nproblems:\n  app: error ?[2J?\nnote: note?]8;;x?\noutput:\n  out\tput ?[31mred?\n  ?\n"
        );
    }

    /// Finding 755: the shell log's mark and what is read since it come from the file through the
    /// box's folder, bounded: a sparse terabyte of a log is never read whole, a link reads nothing.
    #[test]
    fn the_shell_log_is_read_bounded() {
        let d = std::env::temp_dir().join(format!("omabox-shelllog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("home")).unwrap();
        let db = shell::bytes(&d).to_vec();
        let f = std::fs::File::create(d.join("home/shell.log")).unwrap();
        std::io::Write::write_all(&mut &f, b"first line\nWARN x\n").unwrap();
        f.set_len(1 << 40).unwrap();
        drop(f);
        let (first, len) = shell_log_mark(&db);
        assert_eq!((first.as_slice(), len), (&b"first line"[..], 1 << 40));
        assert_eq!(
            shell_log_since(&db, None).len() as u64,
            crate::boxes::READ_CAP
        );
        assert_eq!(shell_log_since(&db, Some((1 << 40) - 3)).len(), 3);
        std::fs::remove_file(d.join("home/shell.log")).unwrap();
        std::fs::write(d.join("secret"), "host\n").unwrap();
        std::os::unix::fs::symlink(d.join("secret"), d.join("home/shell.log")).unwrap();
        assert_eq!(shell_log_mark(&db), (Vec::new(), 0));
        assert!(shell_log_since(&db, None).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_report() {
        let r = report(
            &[
                ("screenshot", b"/tmp/x.png"),
                ("window", b"foot \"t\" 800x600 (tiled)"),
            ],
            &[],
            &[],
            Some(b""),
            None,
        );
        assert_eq!(
            r,
            b"screenshot: /tmp/x.png\nwindow: foot \"t\" 800x600 (tiled)\nproblems: none\noutput: none\n"
        );
        let out: Vec<u8> = (1..=45)
            .flat_map(|i| format!("l{i}\n").into_bytes())
            .collect();
        let r = String::from_utf8(report(
            &[("screenshot", b"none")],
            &[b"no window".to_vec()],
            &[],
            Some(&out),
            Some(b"b"),
        ))
        .unwrap();
        assert!(
            r.starts_with(
                "screenshot: none\nproblems:\n  no window\noutput (first 40 of 45 lines):\n  l1\n"
            ),
            "{r}"
        );
        assert!(
            r.ends_with("  l40\nbox: b kept (omabox -b b ...; omabox down b)\n"),
            "{r}"
        );
    }

    /// A Look's report is `report`'s, its parts in the order they were given; a plain look after
    /// one before says its problems as new.
    #[test]
    fn a_look_is_its_report() {
        let mut l = Look::new(Some(b"b"));
        l.head("screenshot", b"none");
        l.head("window", b"t\x1b]52;c;x\x07");
        l.problem(b"one".to_vec());
        l.note(b"two".to_vec());
        l.output = Some(b"out\n".to_vec());
        assert_eq!(
            l.text(),
            report(
                &[("screenshot", b"none"), ("window", b"t\x1b]52;c;x\x07")],
                &[b"one".to_vec()],
                &[b"two".to_vec()],
                Some(b"out\n"),
                Some(b"b"),
            )
        );
        l.new = true;
        assert!(
            String::from_utf8(l.text())
                .unwrap()
                .contains("?]52;c;x?\nnew problems:\n  one\n")
        );
        let mut l = Look::new(None);
        l.new = true;
        l.head("box", b"b");
        assert_eq!(l.text(), b"box: b\nnew problems: none\n");
    }
}
