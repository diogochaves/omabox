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

/// After a look app's program ended before its window, as E (`exited with status 126`), its log
/// TAIL: jobs::exit_hint's line for 126 and 127, as the look that runs it.
fn look_exit_hint(e: &[u8], cmd: &[Vec<u8>], tail: &[u8]) -> Vec<Vec<u8>> {
    look_exit_hint_as(e, b"omabox look app --", cmd, tail)
}

/// look_exit_hint with RUN the look that runs the command again (`omabox look app --overlay DIR --`).
fn look_exit_hint_as(e: &[u8], run: &[u8], cmd: &[Vec<u8>], tail: &[u8]) -> Vec<Vec<u8>> {
    // (wait job's detail: `exited with status N`, maybe more after it)
    let st: &[u8] = if crate::exit::contains(e, b"exited with status 126") {
        b"126"
    } else if crate::exit::contains(e, b"exited with status 127") {
        b"127"
    } else {
        return Vec::new();
    };
    let file = |f: &str| std::path::Path::new(f).is_file();
    let has = |t: &str| {
        ["/usr/bin/", "/usr/local/bin/"]
            .iter()
            .any(|d| std::path::Path::new(&format!("{d}{t}")).is_file())
    };
    crate::jobs::exit_hint(b"", run, st, cmd, tail, &has, &file)
}

/// run's note that the program's path is relative to a folder the box does not have (ERR, its
/// stderr; finding 743): the note up to its fix, and the folder. None without that note.
fn run_cwd_said(err: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let l = err.split(|&c| c == b'\n').find(|l| {
        crate::exit::contains(l, b" is relative to ")
            && crate::exit::contains(l, b"runs in the box HOME (")
    })?;
    let l = &l[pos(l, b"note: ")? + 6..];
    let i = pos(l, b" is relative to ")? + 16;
    let dir = l[i..i + pos(&l[i..], b", which ")?].to_vec();
    Some((l[..pos(l, b"); ")? + 1].to_vec(), dir))
}

/// The notes on a program that ended (E, how; TAIL its output), in a box that lacked the folder its
/// path is relative to (CWD, run_cwd_said's; finding 743): the folder mounted, and the 126/127 hint
/// on the look that mounts it, which would fail again in the box HOME otherwise. A look's own box
/// takes `--overlay DIR` on the look; a box that was up (OWN, NAME) is started again with it.
fn look_exit_notes(
    e: &[u8],
    cwd: Option<&(Vec<u8>, Vec<u8>)>,
    own: bool,
    name: &[u8],
    cmd: &[Vec<u8>],
    tail: &[u8],
) -> Vec<Vec<u8>> {
    let Some((head, dir)) = cwd else {
        return look_exit_hint(e, cmd, tail);
    };
    let run = if own {
        cat!("omabox look app -b ", name, " --")
    } else {
        cat!("omabox look app --overlay ", dir, " --")
    };
    let hint = look_exit_hint_as(e, &run, cmd, tail);
    let fix = match (own, hint.is_empty()) {
        (true, _) => cat!(
            "down, then up -b ",
            name,
            " --overlay ",
            dir,
            " (or --ro-bind, read-only) mounts it"
        ),
        (false, true) => cat!(run, " ", cmd.join(&b' '), " mounts it"),
        (false, false) => cat!("--overlay ", dir, " on the look mounts it"),
    };
    let mut v = vec![cat!(head, "; ", fix)];
    v.extend(hint);
    v
}

/// Where NEEDLE first is in HAY.
fn pos(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
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

/// The report's `window:` line, `CLASS "TITLE" WxH (LAYOUT)`. A page's window is called chromium,
/// and Chromium gives an --app window a class of its own (`chrome-<the page's path>-Default`) that
/// `--window chromium` does not select (#471): its address is said as the selector.
fn report_window_line(
    page: bool,
    class: &str,
    title: &str,
    size: &str,
    layout: &str,
    addr: &str,
) -> Vec<u8> {
    if page {
        format!("chromium \"{title}\" {size} ({layout}; select it with --window {addr})")
            .into_bytes()
    } else {
        format!("{class} \"{title}\" {size} ({layout})").into_bytes()
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
    /// page: its shots over --after (a contact sheet), and whether this is a page.
    shots: u32,
    page: bool,
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
            _ if o.starts_with(b"-") => return Err(crate::args::unknown_opt(ctx, "look", o)),
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

fn ob(args: &[&[u8]]) -> Ran {
    let mut c = crate::sig::omabox_cmd();
    c.args(args.iter().map(|a| OsStr::from_bytes(a)))
        .stdin(std::process::Stdio::null());
    match c.output() {
        Ok(o) => Ran {
            ok: o.status.success(),
            code: o.status.code().unwrap_or(1),
            out: o.stdout,
            err: o.stderr,
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

/// `omabox look [-b NAME] [-o FILE]`: the box as it is now, nothing started or changed: the screen
/// shot, its windows, the active one's text, its plugins' states, and the problems its programs and
/// its shell logged since the last look in it (any kind's: each leaves a mark, MARK_FILE).
fn plain(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut given, mut o_file) = (Vec::new(), None);
    let mut i = 0;
    while i < args.len() {
        let o = &args[i];
        match o.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, o, args.get(i + 1))?;
                given = args[i + 1].clone();
                i += 1;
            }
            b"-o" => {
                optarg(ctx, o, args.get(i + 1))?;
                o_file = Some(args[i + 1].clone());
                i += 1;
            }
            _ => return Err(crate::args::unknown_opt(ctx, "look", o)),
        }
        i += 1;
    }
    select_box(ctx, &given)?;
    if !box_alive(ctx)? && given.is_empty() && !shell::path(&ctx.d).is_dir() {
        return Err(ctx.die(
            "look: no box is up to look into: omabox look app|plugin|theme|page looks at one thing in a box of its own; omabox up starts yours",
        ));
    }
    need_box(ctx)?;
    let name = ctx.name.clone();
    let name = name.as_slice();
    let mut problems: Vec<Vec<u8>> = Vec::new();
    let mut notes: Vec<Vec<u8>> = Vec::new();
    let mut sh: Vec<&[u8]> = vec![b"shot", b"-b", name];
    if let Some(o) = &o_file {
        sh.extend([b"-o".as_slice(), o]);
    }
    let r = ob(&sh);
    let shot = if r.ok {
        last_line(&r.out)
    } else {
        problems.push(cat!("shot: ", shell::subst(&r.err)));
        None
    };
    // Its windows, the active one first; that one's text, where its toolkit gives any.
    let wins: Value = serde_json::from_slice(&ob(&[b"windows", b"-b", name, b"--json"]).out)
        .unwrap_or(Value::Null);
    let mut wins: Vec<Value> = wins.as_array().cloned().unwrap_or_default();
    let active = |w: &Value| w.get("active").and_then(Value::as_bool) == Some(true);
    wins.sort_by_key(|w| !active(w));
    let mut lines: Vec<Vec<u8>> = wins.iter().take(WINDOWS_MAX).map(window_line).collect();
    if wins.len() > WINDOWS_MAX {
        lines.push(
            format!("... and {} more (omabox windows)", wins.len() - WINDOWS_MAX).into_bytes(),
        );
    }
    let mut text = None;
    if let Some(a) = wins
        .first()
        .and_then(|w| w.get("address"))
        .and_then(Value::as_str)
    {
        let r = ob(&[b"tree", b"-b", name, b"--window", a.as_bytes(), b"--json"]);
        if r.ok {
            let items: Vec<String> = tree_texts(&r.out).0.into_iter().map(|t| t.0).collect();
            if !items.is_empty() {
                text = Some(text_line(&items, PANEL_TEXT_MAX));
            }
        }
    }
    // Its plugins: each one's state, and one the shell did not load is a problem.
    let me: Value = serde_json::from_slice(&ob(&[b"ls", b"--json"]).out).unwrap_or(Value::Null);
    let me = me
        .as_array()
        .and_then(|v| {
            v.iter()
                .find(|b| b.get("name").and_then(Value::as_str).map(str::as_bytes) == Some(name))
        })
        .cloned()
        .unwrap_or(Value::Null);
    let mut ids: Vec<String> = Vec::new();
    let mut plugins: Vec<String> = Vec::new();
    if let Some(m) = me.get("plugin_status").and_then(Value::as_object) {
        for (id, st) in m {
            let state = st.get("state").and_then(Value::as_str).unwrap_or("unknown");
            if state != "loaded" && state != "hosted" {
                problems.push(format!("plugin {id}: {state}").into_bytes());
            }
            plugins.push(format!("{id} {state}"));
            ids.push(id.clone());
        }
    }
    if let Some(st) = me.get("shell_state").and_then(Value::as_str) {
        if !matches!(st, "running" | "none" | "") {
            problems.push(format!("the box's shell is {st} (omabox restart-shell)").into_bytes());
        }
    }
    // What was logged since the last look.
    let old = mark_read(ctx);
    let now = mark_now(ctx, name);
    problems.extend(new_problems(ctx, name, &old, &now, &ids));
    mark_write(ctx, &now);
    let n = wins.len().to_string();
    let mut head: Vec<(&str, &[u8])> = vec![("screenshot", shot.as_deref().unwrap_or(b"none"))];
    let wl = lines
        .iter()
        .fold(n.into_bytes(), |acc, l| cat!(acc, "\n  ", l));
    head.push(("windows", &wl));
    if let Some(t) = &text {
        head.push(("text", t));
    }
    let pl = plugins.join(", ").into_bytes();
    if !plugins.is_empty() {
        head.push(("plugins", &pl));
    }
    head.push(("box", name));
    if wins.is_empty() && plugins.is_empty() {
        notes.push(b"no window is open in the box".to_vec());
    }
    let mut rep = report(&head, &problems, &notes, None, None);
    // (said as new once there was a look before: one seen then is not gone, only not repeated)
    if old.is_some() {
        if let Some(at) = rep.windows(10).position(|w| w == b"\nproblems:") {
            rep.splice(at + 1..at + 1, b"new ".iter().copied());
        }
    }
    out(rep)?;
    if problems.is_empty() {
        Ok(())
    } else {
        Err(Exit(1))
    }
}

/// How many windows a plain look lists.
const WINDOWS_MAX: usize = 12;
/// A box's mark of its last look, in its dir (host side: nothing in the box reads or writes it).
const MARK_FILE: &str = "look.mark";

/// `class "title" WxH (tiled|floating[, active])`.
fn window_line(w: &Value) -> Vec<u8> {
    let s = |k: &str| w.get(k).and_then(Value::as_str).unwrap_or("");
    let n = |k: &str| w.get(k).and_then(Value::as_i64).unwrap_or(0);
    let mut how = if w.get("floating").and_then(Value::as_bool) == Some(true) {
        "floating".to_string()
    } else {
        "tiled".to_string()
    };
    if w.get("active").and_then(Value::as_bool) == Some(true) {
        how.push_str(", active");
    }
    format!(
        "{} \"{}\" {}x{} ({how})",
        s("class"),
        s("title"),
        n("w"),
        n("h")
    )
    .into_bytes()
}

/// Where each log of the box ends now: its run -d logs by size, the shell's by its first line (a
/// restarted shell starts its log again) and its length.
fn mark_now(ctx: &Ctx, name: &[u8]) -> Value {
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
    let shell_log = ob(&[b"log", b"-b", name, b"shell", b"-n", b"all", b"--full"]).out;
    let first = shell_log.split(|&c| c == b'\n').next().unwrap_or(b"");
    serde_json::json!({
        "logs": logs,
        "shell_first": String::from_utf8_lossy(first),
        "shell_len": shell_log.len(),
    })
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

/// The problems logged between two marks: each run -d log's new lines that name one (a page's
/// console as look page reads it), under the job's program, and the shell's new warnings that name
/// one of the box's plugins (IDS).
fn new_problems(
    ctx: &Ctx,
    name: &[u8],
    old: &Option<Value>,
    now: &Value,
    ids: &[String],
) -> Vec<Vec<u8>> {
    let mut found = Vec::new();
    let was = |k: &str| old.as_ref()?.get("logs")?.get(k)?.as_u64();
    // Whose log: the job's program, while `jobs` lists it.
    let jobs: Value =
        serde_json::from_slice(&ob(&[b"jobs", b"-b", name, b"--json"]).out).unwrap_or(Value::Null);
    let whose = |file: &str| -> String {
        jobs.as_array()
            .and_then(|v| {
                v.iter().find(|j| {
                    j.get("log")
                        .and_then(Value::as_str)
                        .is_some_and(|l| l.ends_with(&format!("/{file}")))
                })
            })
            .and_then(|j| {
                j.get("argv")?
                    .as_array()?
                    .first()?
                    .as_str()
                    .map(String::from)
            })
            .map_or_else(
                || file.to_string(),
                |p| {
                    shell::basename(p.as_bytes())
                        .iter()
                        .map(|&c| c as char)
                        .collect()
                },
            )
    };
    if let Some(logs) = now.get("logs").and_then(Value::as_object) {
        for file in logs.keys() {
            let all = std::fs::read(ctx.at("home").join(file)).unwrap_or_default();
            let new = since(&all, was(file));
            let page = crate::exit::contains(&all, b"omabox-look-chromium")
                || !console_lines(&all).is_empty();
            let lines = if page {
                console_problems(&console_lines(new))
            } else {
                problem_lines(new)
            };
            let who = whose(file);
            found.extend(lines.into_iter().map(|l| cat!(who.as_str(), ": ", l)));
        }
    }
    let shell_log = ob(&[b"log", b"-b", name, b"shell", b"-n", b"all", b"--full"]).out;
    let first = shell_log.split(|&c| c == b'\n').next().unwrap_or(b"");
    let same = old
        .as_ref()
        .and_then(|o| o.get("shell_first")?.as_str())
        .is_some_and(|f| f.as_bytes() == String::from_utf8_lossy(first).as_bytes());
    let was_len = old
        .as_ref()
        .and_then(|o| o.get("shell_len")?.as_u64())
        .filter(|_| same);
    let warn = crate::plugins::shell_warnings_of(since(&shell_log, was_len));
    for id in ids {
        found.extend(plugin_warnings(&warn, id.as_bytes()));
    }
    found
}

/// The program's window: the job's (its pid), else the first that is not one of BEFORE (the
/// addresses there when it was started: none in a throwaway box, so any); None before one is up.
pub fn window_of(windows: &[u8], pid: Option<i64>, before: &[String]) -> Option<Value> {
    let v: Value = serde_json::from_slice(windows).ok()?;
    let all = v.as_array()?;
    let addr = |w: &Value| w.get("address").and_then(Value::as_str).map(String::from);
    all.iter()
        .find(|w| pid.is_some() && w.get("pid").and_then(Value::as_i64) == pid)
        .or_else(|| {
            all.iter()
                .find(|w| addr(w).is_none_or(|a| !before.contains(&a)))
        })
        .cloned()
}

/// The addresses of `windows --json`'s windows.
pub fn addresses(windows: &[u8]) -> Vec<String> {
    serde_json::from_slice::<Value>(windows)
        .ok()
        .and_then(|v| {
            Some(
                v.as_array()?
                    .iter()
                    .filter_map(|w| w.get("address").and_then(Value::as_str).map(String::from))
                    .collect(),
            )
        })
        .unwrap_or_default()
}

/// The pages earlier looks left open in a box that stays up: each `look page` Chromium job is
/// stopped, so the next one starts a Chromium of its own on the profile (a second one beside it
/// would hand its page to the first and end, its console never read).
fn stop_pages(name: &[u8]) {
    let r = ob(&[b"jobs", b"-b", name, b"--json"]);
    let Ok(Value::Array(jobs)) = serde_json::from_slice::<Value>(&r.out) else {
        return;
    };
    for j in jobs {
        let ours = j.get("argv").and_then(Value::as_array).is_some_and(|v| {
            v.iter().any(|w| {
                w.as_str()
                    .is_some_and(|w| w.contains("omabox-look-chromium"))
            })
        });
        if let (true, Some(pid)) = (ours, j.get("pid").and_then(Value::as_i64)) {
            ob(&[b"stop", b"-b", name, pid.to_string().as_bytes()]);
        }
    }
}

/// The job whose log is LOG: its pid, while `jobs` lists it.
fn job_pid(name: &[u8], log: &[u8]) -> Option<i64> {
    let r = ob(&[b"jobs", b"-b", name, b"--json"]);
    let v: Value = serde_json::from_slice(&r.out).ok()?;
    v.as_array()?
        .iter()
        .find(|j| j.get("log").and_then(Value::as_str).map(str::as_bytes) == Some(log))
        .and_then(|j| j.get("pid").and_then(Value::as_i64))
}

/// How the job PID ended ("exited with status 3"), once it has.
fn job_end(name: &[u8], pid: i64) -> Option<Vec<u8>> {
    let p = pid.to_string();
    let r = ob(&[
        b"wait",
        b"-b",
        name,
        b"--json",
        b"--timeout",
        b"1s",
        b"job",
        p.as_bytes(),
    ]);
    let v: Value = serde_json::from_slice(&r.out).ok()?;
    (v.get("result").and_then(Value::as_str) == Some("satisfied")).then(|| {
        v.get("detail")
            .and_then(Value::as_str)
            .unwrap_or("ended")
            .as_bytes()
            .to_vec()
    })
}

fn app(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let mut a = parse(ctx, args, "app")?;
    let me = std::process::id();
    // (a throwaway is taken down however this ends, as a throwaway run's: the box's reaper retries
    // once we are gone)
    let (name, guard) = pick(ctx, &mut a, me)?;
    let r = app_in(ctx, &a, &name, me);
    if let Some(g) = guard {
        g.finish();
    }
    r
}

/// look page FILE|URL: Chromium in the box on it, as an app window (its own profile in the box HOME,
/// its sandbox kept), the page's console read from Chromium's log (--enable-logging=stderr): its
/// uncaught exceptions and messages naming an error are the problems, the rest of Chromium's log
/// (the box's missing system bus and the like) is not shown. A file's folder is bound read-only.
fn page(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let mut a = parse(ctx, args, "page")?;
    a.page = true;
    let what = a.dir.take().unwrap_or_default();
    let mut bind = None;
    let url = if Re::new("^(https?|file)://").is_some_and(|r| r.is_match(&what)) {
        what
    } else {
        let f = crate::repo::readlink_f(&what).unwrap_or_default();
        if f.is_empty() || !shell::path(&f).is_file() {
            return Err(ctx.die(cat!(
                "look page: no such file: ",
                what,
                " (a FILE, or an http:// or file:// URL)"
            )));
        }
        let dirn = f[..f.iter().rposition(|&c| c == b'/').unwrap_or(0)].to_vec();
        bind = Some(dirn);
        cat!("file://", f)
    };
    a.cmd = vec![
        b"sh".to_vec(),
        b"-c".to_vec(),
        CHROMIUM_SH.as_bytes().to_vec(),
        b"sh".to_vec(),
        url,
    ];
    let me = std::process::id();
    let (name, guard) = pick(ctx, &mut a, me)?;
    // A box that was up has only the folders it was started with: a file outside them is said.
    if let Some(d) = bind.filter(|_| !a.own) {
        a.up.extend([b"--ro-bind".to_vec(), d]);
    }
    if a.own {
        if let Some(f) = a.cmd.last().and_then(|u| u.strip_prefix(b"file://")) {
            if !ob(&[b"run", b"-b", &name, b"--", b"test", b"-r", f]).ok {
                return Err(ctx.die(cat!(
                    "look page: box '",
                    name,
                    "' does not have ",
                    f,
                    " (not in a folder it was started with): omabox look page ",
                    f,
                    " --fresh opens it in a box of its own"
                )));
            }
        }
    }
    let r = app_in(ctx, &a, &name, me);
    if let Some(g) = guard {
        g.finish();
    }
    r
}

/// Chromium on $1 as an app window: the first of chromium, google-chrome-stable, chrome on the box's
/// PATH, its profile in the box HOME, its log (the page's console) on stderr.
const CHROMIUM_SH: &str = r#"b=$(command -v chromium || command -v google-chrome-stable || command -v chrome) || { echo "fatal: no Chromium in the box (chromium, google-chrome-stable)" >&2; exit 127; }; exec "$b" --app="$1" --user-data-dir="$HOME/.cache/omabox-look-chromium" --enable-logging=stderr --no-first-run --no-default-browser-check --password-store=basic"#;

/// A page's console in Chromium's log: each `CONSOLE` line as `message (source:line)`.
pub fn console_lines(log: &[u8]) -> Vec<Vec<u8>> {
    let Some(re) = Re::new(r":CONSOLE[:(]([0-9]+)\)?\] (.*), source: ([^ ]*)") else {
        return Vec::new();
    };
    log.split(|&c| c == b'\n')
        .filter_map(|l| {
            let line = re.group(l, 1)?;
            let mut msg = re.group(l, 2)?;
            if msg.len() >= 2 && msg.starts_with(b"\"") && msg.ends_with(b"\"") {
                msg = msg[1..msg.len() - 1].to_vec();
            }
            let src = re.group(l, 3)?;
            let file = src.rsplit(|&c| c == b'/').next().unwrap_or(&src).to_vec();
            Some(cat!(msg, " (", file, ":", line, ")"))
        })
        .collect()
}

/// The console lines that are problems: an uncaught exception, or a message naming an error.
pub fn console_problems(lines: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let Some(re) = Re::new(r"^uncaught |error|failed to load|net::err_") else {
        return Vec::new();
    };
    lines
        .iter()
        .filter(|l| re.is_match(&l.to_ascii_lowercase()))
        .cloned()
        .collect()
}

fn app_in(ctx: &Ctx, a: &App, name: &[u8], me: u32) -> Res<()> {
    let mut problems: Vec<Vec<u8>> = Vec::new();
    let mut notes: Vec<Vec<u8>> = Vec::new();
    let mut sheet = None;
    start(ctx, a, name, me, &[], &mut problems)?;
    // The program, a job of the box's: its log is its output.
    // In a box that was up: the same program still running from the last look is stopped first, and
    // its window is the job's, else one that was not there before it (never another program's).
    let before: Vec<String> = if a.own {
        addresses(&ob(&[b"windows", b"-b", name, b"--json"]).out)
    } else {
        Vec::new()
    };
    if a.own && a.page {
        stop_pages(name);
    }
    let mut run: Vec<&[u8]> = vec![b"run", b"-b", name, b"-d", b"-q", b"--print-log"];
    if a.own {
        run.push(b"--replace");
    }
    run.push(b"--");
    run.extend(a.cmd.iter().map(Vec::as_slice));
    let r = ob(&run);
    // A program named relative to a folder the box lacks: run's note, as the look's (finding 743),
    // in place of a 126/127 hint that would fail again in the box HOME.
    let cwd = run_cwd_said(&r.err);
    if !r.ok {
        // One that ended at once ran: its end and its output are the report.
        if let Some((end, log)) = ended_at_once(&r.err) {
            problems.push(cat!("it ", end, ", before opening a window"));
            let output = std::fs::read(shell::path(&log)).unwrap_or_default();
            problems.extend(problem_lines(&output));
            notes.extend(look_exit_notes(
                &end,
                cwd.as_ref(),
                a.own,
                name,
                &a.cmd,
                &output,
            ));
            out(report(
                &[("screenshot", b"none")],
                &problems,
                &notes,
                Some(&output),
                a.keep.then_some(name),
            ))?;
            return Err(Exit(1));
        }
        let _ = std::io::Write::write_all(&mut std::io::stderr(), &r.err);
        return Err(ctx.die("look: the program could not be started in the box"));
    }
    let log = shell::subst(&r.out);
    let pid = job_pid(name, &log);
    // Its window, or its end, or the timeout.
    let start = Instant::now();
    let mut win = None;
    let mut ended = None;
    while start.elapsed() < a.timeout {
        win = window_of(&ob(&[b"windows", b"-b", name, b"--json"]).out, pid, &before);
        if win.is_some() {
            break;
        }
        if pid.is_some() && job_pid(name, &log).is_none() {
            ended = pid.and_then(|p| job_end(name, p));
            break;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    let mut shot = None;
    let mut window_line = Vec::new();
    match &win {
        None => {
            let why = match &ended {
                Some(e) => cat!("it ", e, " before opening a window"),
                None => cat!(
                    "no window within ",
                    format!("{:.0}s", a.timeout.as_secs_f64()),
                    " (--timeout for a slower one)"
                ),
            };
            problems.push(why);
            // A script not executable (126) or named without ./ (127): the look that runs it (#471).
            if let Some(e) = &ended {
                let tail = std::fs::read(shell::path(&log)).unwrap_or_default();
                notes.extend(look_exit_notes(e, cwd.as_ref(), a.own, name, &a.cmd, &tail));
            }
        }
        Some(w) => {
            let addr = w.get("address").and_then(Value::as_str).unwrap_or("active");
            let sel = addr.as_bytes();
            // (its first frames drawn before a key: a shell still starting drops what it is sent)
            let wait_for: &[u8] = if a.page { b"2s" } else { b"5s" };
            let still = [
                b"wait".as_slice(),
                b"-b",
                name,
                b"--timeout",
                wait_for,
                b"still",
                b"--window",
                sel,
            ];
            if !a.keys.is_empty() {
                ob(&still);
            }
            let mut keys: Vec<&[u8]> = vec![b"keys", b"-b", name, b"--window", sel, b"--wait"];
            keys.extend(a.keys.iter().map(Vec::as_slice));
            let keys_done = |r: Ran, problems: &mut Vec<Vec<u8>>| {
                if !r.ok && r.code != 124 {
                    problems.push(cat!("keys: ", shell::subst(&r.err)));
                }
                warnings(&r.err, problems);
            };
            if a.shots > 1 {
                // A contact sheet: the keys (a held one too) sent while N shots are taken over
                // --after (2 s by default), on real time and real frames.
                let span = a
                    .after
                    .as_deref()
                    .and_then(ms_duration)
                    .and_then(|v| u64::try_from(v).ok())
                    .unwrap_or(2000);
                let step = Duration::from_millis(span / u64::from(a.shots));
                let mut paths: Vec<Vec<u8>> = Vec::new();
                std::thread::scope(|sc| {
                    let k = (!a.keys.is_empty()).then(|| sc.spawn(|| ob(&keys)));
                    for n in 0..a.shots {
                        std::thread::sleep(step);
                        let o = a.out.as_ref().map(|o| suffixed(o, &format!("{}", n + 1)));
                        let mut sh: Vec<&[u8]> = vec![b"shot", b"-b", name, b"--window", sel];
                        if let Some(o) = &o {
                            sh.extend([b"-o".as_slice(), o]);
                        }
                        if let Some(p) = last_line(&ob(&sh).out) {
                            paths.push(p);
                        }
                    }
                    if let Some(k) = k {
                        if let Ok(r) = k.join() {
                            keys_done(r, &mut problems);
                        }
                    }
                });
                shot = paths.last().cloned();
                let out_s = a.out.as_ref().map_or_else(
                    || cat!(shot.clone().unwrap_or_default(), "-sheet.png"),
                    |o| suffixed(o, "sheet"),
                );
                let mut m = std::process::Command::new("magick");
                m.arg("montage")
                    .args(paths.iter().map(|p| OsStr::from_bytes(p)))
                    .args([
                        "-tile",
                        &format!("{}x", a.shots.min(4)),
                        "-geometry",
                        "480x+4+4",
                        "-label",
                        "%t",
                    ])
                    .arg(OsStr::from_bytes(&out_s))
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());
                if m.status().is_ok_and(|st| st.success()) {
                    sheet = Some(out_s);
                } else {
                    notes.push(b"no contact sheet (ImageMagick's magick montage): the shots are each named in order".to_vec());
                    for (n, p) in paths.iter().enumerate() {
                        notes.push(cat!(format!("shot {}: ", n + 1), p));
                    }
                }
            } else {
                if !a.keys.is_empty() {
                    let r = ob(&keys);
                    keys_done(r, &mut problems);
                }
                if let Some(d) = &a.after {
                    ob(&[b"wait", b"--json", b"for", d]);
                }
                let r = ob(&still);
                if r.code == 124 && !a.page {
                    notes.push(b"still changing after 5s: the shot is of a moving window".to_vec());
                }
                let mut sh: Vec<&[u8]> = vec![b"shot", b"-b", name, b"--window", sel];
                if let Some(o) = &a.out {
                    sh.extend([b"-o".as_slice(), o]);
                }
                let r = ob(&sh);
                if r.ok {
                    shot = last_line(&r.out);
                } else {
                    problems.push(cat!("shot: ", shell::subst(&r.err)));
                }
            }
            let s = |k: &str| w.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let size = w
                .get("size")
                .and_then(Value::as_array)
                .map(|v| format!("{}x{}", v[0], v.get(1).unwrap_or(&Value::Null)))
                .unwrap_or_default();
            let layout = if w.get("floating").and_then(Value::as_bool) == Some(true) {
                "floating"
            } else {
                "tiled"
            };
            window_line = report_window_line(a.page, &s("class"), &s("title"), &size, layout, addr);
            // Ended meanwhile (a crash after its first frame).
            if pid.is_some() && job_pid(name, &log).is_none() {
                if let Some(e) = pid.and_then(|p| job_end(name, p)) {
                    problems.push(cat!("it ", e, " after opening its window"));
                }
            }
        }
    }
    // In a box that stays up the program does too: said, with what stops it.
    if a.own {
        if let Some(p) = pid.filter(|_| job_pid(name, &log).is_some()) {
            notes.push(cat!(
                "left running in the box: omabox stop -b ",
                name,
                " ",
                p.to_string()
            ));
        }
    }
    let mut output = std::fs::read(shell::path(&log)).unwrap_or_default();
    if a.page {
        let c = console_lines(&output);
        problems.extend(console_problems(&c));
        output = c.iter().flat_map(|l| cat!(l, "\n")).collect();
    } else {
        problems.extend(problem_lines(&output));
    }
    let mut head: Vec<(&str, &[u8])> = vec![("screenshot", shot.as_deref().unwrap_or(b"none"))];
    if let Some(sh) = &sheet {
        head.push(("sheet", sh));
    }
    if !window_line.is_empty() {
        head.push(("window", &window_line));
    }
    if a.own {
        head.push(("box", name));
        // (what this look reported is not new to the next one)
        mark_write(ctx, &mark_now(ctx, name));
    }
    out(report(
        &head,
        &problems,
        &notes,
        Some(&output),
        a.keep.then_some(name),
    ))?;
    if problems.is_empty() {
        Ok(())
    } else {
        Err(Exit(1))
    }
}

/// up NAME for a look: its own process, its errors the caller's; --owner and no idle limit unless
/// the box is kept; EXTRA (a plugin's --plugin DIR) before the caller's UP OPTIONS. Its warnings are
/// problems.
fn start(
    ctx: &Ctx,
    a: &App,
    name: &[u8],
    me: u32,
    extra: &[&[u8]],
    problems: &mut Vec<Vec<u8>>,
) -> Res<Vec<u8>> {
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
    let r = ob(&up);
    if !r.ok {
        let _ = std::io::Write::write_all(&mut std::io::stderr(), &r.err);
        return Err(ctx.die("look: the box did not start"));
    }
    warnings(&r.err, problems);
    Ok(r.err)
}

/// up's notes on a theme (`omabox: note: theme NAME: LINE`, #462): each LINE.
pub fn theme_notes(err: &[u8]) -> Vec<Vec<u8>> {
    err.split(|&c| c == b'\n')
        .filter_map(|l| l.strip_prefix(b"omabox: note: theme "))
        .filter_map(|l| {
            let i = l.windows(2).position(|w| w == b": ")?;
            Some(l[i + 2..].to_vec())
        })
        .collect()
}

/// The last non-empty line of a command's stdout (shot's path).
fn last_line(o: &[u8]) -> Option<Vec<u8>> {
    o.split(|&c| c == b'\n')
        .rfind(|l| !l.is_empty())
        .map(<[u8]>::to_vec)
}

/// The bar's strip of the first monitor, from what Hyprland reserves for it (`monitors`' reserved:
/// left, top, right, bottom): the largest edge, as `X,Y WxH`; None when nothing is reserved.
pub fn bar_strip(monitors: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(monitors).ok()?;
    let m = v.as_array()?.first()?;
    let n = |k: &str| m.get(k).and_then(Value::as_i64).unwrap_or(0);
    let (w, h) = (n("width"), n("height"));
    let r: Vec<i64> = m
        .get("reserved")?
        .as_array()?
        .iter()
        .map(|x| x.as_i64().unwrap_or(0))
        .collect();
    let (l, t, rt, b) = (*r.first()?, *r.get(1)?, *r.get(2)?, *r.get(3)?);
    let max = l.max(t).max(rt).max(b);
    if max <= 0 {
        return None;
    }
    Some(if max == t {
        format!("0,0 {w}x{t}")
    } else if max == b {
        format!("0,{} {w}x{b}", h - b)
    } else if max == l {
        format!("0,0 {l}x{h}")
    } else {
        format!("{},0 {rt}x{h}", w - rt)
    })
}

/// The lines of the shell's WARNINGS (shell_warnings_of's) that name plugin ID.
pub fn plugin_warnings(warnings: &[u8], id: &[u8]) -> Vec<Vec<u8>> {
    warnings
        .split(|&c| c == b'\n')
        .filter(|l| !id.is_empty() && l.windows(id.len()).any(|w| w == id))
        .map(<[u8]>::to_vec)
        .collect()
}

/// The bar's layer in an Omarchy box (4.5 and 4.0 alike).
const BAR_NS: &[u8] = b"omarchy-bar";
/// A panel's text items shown at most (the rest counted).
const PANEL_TEXT_MAX: usize = 60;

/// `tree --layer NS --json`'s text (#467): each node under the frame that has a name, as
/// (name, x, y, w, h, cut) in the screen's coordinates, in the tree's order (cut: its `cut` state,
/// only part of it shows); and the frame's x, y.
pub fn tree_texts(json: &[u8]) -> (Vec<TextItem>, (i64, i64)) {
    let v: Value = serde_json::from_slice(json).unwrap_or(Value::Null);
    let n = |e: &Value, k: &str| e.get(k).and_then(Value::as_i64).unwrap_or(0);
    let mut origin = (0, 0);
    let mut items = Vec::new();
    for e in v.as_array().map(Vec::as_slice).unwrap_or(&[]) {
        if n(e, "depth") == 0 {
            origin = (n(e, "x"), n(e, "y"));
            continue;
        }
        let name = e.get("name").and_then(Value::as_str).unwrap_or("");
        let text = e.get("text").and_then(Value::as_str).unwrap_or("");
        let said = if name.is_empty() { text } else { name };
        if !said.is_empty() {
            let cut = e
                .get("states")
                .and_then(Value::as_array)
                .is_some_and(|a| a.iter().any(|s| s == "cut"));
            items.push((
                said.to_string(),
                n(e, "x"),
                n(e, "y"),
                n(e, "w"),
                n(e, "h"),
                cut,
            ));
        }
    }
    (items, origin)
}

/// The text a bar widget shows (#467): the items of the bar's tree (TREE, `tree --json`) whose
/// middle is in widget ID's slot, from the shell's `debugBarGeometry` (GEOM: window coordinates, the
/// bar's frame giving the layer's place).
pub fn widget_text(tree: &[u8], geom: &[u8], id: &str) -> WidgetText {
    let g: Value = serde_json::from_slice(geom).unwrap_or(Value::Null);
    let Some(slot) = g.as_array().and_then(|a| {
        a.iter()
            .find(|s| s.get("id").and_then(Value::as_str) == Some(id))
    }) else {
        return WidgetText::Absent;
    };
    if slot.get("visible").and_then(Value::as_bool) != Some(true) {
        return WidgetText::Hidden;
    }
    let n = |k: &str| slot.get(k).and_then(Value::as_i64).unwrap_or(0);
    let (items, (ox, oy)) = tree_texts(tree);
    let (x0, y0) = (n("x") + ox, n("y") + oy);
    let (x1, y1) = (x0 + n("width"), y0 + n("height"));
    let mine: Vec<TextItem> = items
        .into_iter()
        .filter(|(_, x, y, w, h, _)| {
            let (mx, my) = (x + w / 2, y + h / 2);
            mx >= x0 && mx < x1 && my >= y0 && my < y1
        })
        .collect();
    let cut = mine.iter().any(|t| t.5);
    WidgetText::Shown(mine.into_iter().map(|(s, ..)| s).collect(), cut)
}

/// widget_text's answer.
#[derive(Debug, PartialEq)]
pub enum WidgetText {
    /// Its slot is shown: the text in it (none: icons or pictures only), and whether any of it is
    /// cut (only part of it shows: a label wider than a vertical bar).
    Shown(Vec<String>, bool),
    /// Its slot is in the bar's layout with nothing shown (the widget hides itself, or is empty).
    Hidden,
    /// No slot of it in the bar's layout (off the bar, or placed by another plugin).
    Absent,
}

/// The namespaces of LAYERS (`hyprctl -j layers`), each once, in order.
pub fn layer_names(layers: &[u8]) -> Vec<String> {
    fn walk(v: &Value, into: &mut Vec<String>) {
        match v {
            Value::Object(m) => {
                if let Some(ns) = m.get("namespace").and_then(Value::as_str) {
                    if !into.iter().any(|n| n == ns) {
                        into.push(ns.to_string());
                    }
                }
                m.values().for_each(|c| walk(c, into));
            }
            Value::Array(a) => a.iter().for_each(|c| walk(c, into)),
            _ => {}
        }
    }
    let mut into = Vec::new();
    walk(
        &serde_json::from_slice(layers).unwrap_or(Value::Null),
        &mut into,
    );
    into
}

/// Text items as one line: `a | b | c`, MAX at most and the rest counted; "none" for none.
pub fn text_line(items: &[String], max: usize) -> Vec<u8> {
    if items.is_empty() {
        return b"none".to_vec();
    }
    let mut s = items
        .iter()
        .take(max)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" | ");
    if items.len() > max {
        s.push_str(&format!(" | ... and {} more", items.len() - max));
    }
    s.into_bytes()
}

/// One text item of a layer: what it says and where (tree_texts').
type TextItem = (String, i64, i64, i64, i64, bool);

/// `tree --layer NS --json` in box NAME, as it printed it, or why it read nothing.
fn layer_tree(name: &[u8], ns: &[u8]) -> Result<Vec<u8>, Vec<u8>> {
    let r = ob(&[b"tree", b"-b", name, b"--layer", ns, b"--json"]);
    if !r.ok {
        let why = shell::subst(&r.err);
        let why = why.strip_prefix(b"omabox: ").unwrap_or(&why).to_vec();
        return Err(why);
    }
    Ok(r.out)
}

/// layer_tree's text items.
fn layer_texts(name: &[u8], ns: &[u8]) -> Result<Vec<TextItem>, Vec<u8>> {
    layer_tree(name, ns).map(|j| tree_texts(&j).0)
}

/// A plugin's id, from DIR/manifest.json.
fn plugin_id(dir: &[u8]) -> Option<Vec<u8>> {
    let m: Value =
        serde_json::from_slice(&std::fs::read(shell::path(&cat!(dir, "/manifest.json"))).ok()?)
            .ok()?;
    m.get("id")
        .and_then(Value::as_str)
        .map(|s| s.as_bytes().to_vec())
}

/// PATH with -SUFFIX before its extension (`/x/a.png`, `panel`: `/x/a-panel.png`).
fn suffixed(path: &[u8], suffix: &str) -> Vec<u8> {
    let slash = path.iter().rposition(|&c| c == b'/').map_or(0, |i| i + 1);
    match path[slash..].iter().rposition(|&c| c == b'.') {
        Some(d) => cat!(&path[..slash + d], "-", suffix, &path[slash + d..]),
        None => cat!(path, "-", suffix),
    }
}

fn plugin(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let mut a = parse(ctx, args, "plugin")?;
    let dir = a.dir.clone().unwrap_or_default();
    let Some(id) = plugin_id(&dir) else {
        return Err(ctx.die(cat!(
            "look plugin: ",
            dir,
            " has no manifest.json with an id: not a plugin's folder"
        )));
    };
    let me = std::process::id();
    let (name, guard) = pick(ctx, &mut a, me)?;
    let r = plugin_in(ctx, &a, &dir, &id, &name, me);
    if let Some(g) = guard {
        g.finish();
    }
    r
}

fn plugin_in(ctx: &Ctx, a: &App, dir: &[u8], id: &[u8], name: &[u8], me: u32) -> Res<()> {
    let mut problems: Vec<Vec<u8>> = Vec::new();
    let mut notes: Vec<Vec<u8>> = Vec::new();
    start(ctx, a, name, me, &[b"--plugin", dir], &mut problems)?;
    // Its state, as ls --json has it.
    let status = || -> Value {
        let ls: Value = serde_json::from_slice(&ob(&[b"ls", b"--json"]).out).unwrap_or(Value::Null);
        ls.as_array()
            .and_then(|v| {
                v.iter().find(|b| {
                    b.get("name").and_then(Value::as_str).map(str::as_bytes) == Some(name)
                })
            })
            .and_then(|b| {
                b.get("plugin_status")?
                    .get(std::str::from_utf8(id).ok()?)
                    .cloned()
            })
            .unwrap_or(Value::Null)
    };
    // In a box that was up: the shell again, so it shows the plugin as its files are now (and its
    // log holds this shell's lines only: no warning of an edit ago); a box without the plugin gets
    // it first, as `plugin add` puts one in.
    if a.own {
        let r = if status().is_null() {
            ob(&[b"plugin", b"-b", name, b"add", dir])
        } else {
            ob(&[b"restart-shell", b"-b", name])
        };
        if !r.ok {
            let _ = std::io::Write::write_all(&mut std::io::stderr(), &r.err);
            return Err(ctx.die(cat!(
                "look plugin: the shell of box '",
                name,
                "' could not be given the plugin (look --fresh for a box of its own)"
            )));
        }
        warnings(&r.err, &mut problems);
    }
    let st = status();
    let f = |k: &str| st.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let state = if f("state").is_empty() {
        "unknown".to_string()
    } else {
        f("state")
    };
    let mut line = cat!(id, " ", state.as_str());
    if !f("via").is_empty() {
        line = cat!(line, " (", f("via"), ")");
    }
    if !f("host").is_empty() {
        line = cat!(line, ", in ", f("host"), "'s layout");
    }
    if state != "loaded" && state != "hosted" {
        problems.push(cat!("plugin ", id, ": ", state.as_str()));
    }
    let still: [&[u8]; 6] = [b"wait", b"-b", name, b"--timeout", b"5s", b"still"];
    if ob(&still).code == 124 {
        notes.push(b"still changing after 5s: the shot is of a moving screen".to_vec());
    }
    // The bar's strip, from what Hyprland reserves for it; the whole screen when nothing is.
    let g = bar_strip(&ob(&[b"hyprctl", b"-b", name, b"-j", b"monitors"]).out);
    let mut sh: Vec<&[u8]> = vec![b"shot", b"-b", name];
    if let Some(g) = &g {
        sh.extend([b"-g".as_slice(), g.as_bytes()]);
    } else {
        notes.push(b"no bar reserves an edge: the shot is the whole screen".to_vec());
    }
    if let Some(o) = &a.out {
        sh.extend([b"-o".as_slice(), o]);
    }
    let r = ob(&sh);
    let shot = if r.ok {
        last_line(&r.out)
    } else {
        problems.push(cat!("shot: ", shell::subst(&r.err)));
        None
    };
    // What the widget shows (#467): the bar's text items in its slot, from the shell's own geometry
    // of the bar and `tree`, which reads the shell's text (tools/qtext).
    let id_s = String::from_utf8_lossy(id).into_owned();
    let geom = ob(&[
        b"run",
        b"-b",
        name,
        b"--",
        b"omarchy-shell",
        b"shell",
        b"debugBarGeometry",
    ]);
    let bar_json = layer_tree(name, BAR_NS);
    let bar = bar_json.as_ref().map(|j| tree_texts(j).0);
    let text = match bar_json.as_ref().map(|j| widget_text(j, &geom.out, &id_s)) {
        Err(why) => {
            notes.push(cat!("the bar's text could not be read: ", why));
            b"unknown".to_vec()
        }
        Ok(WidgetText::Shown(t, cut)) => {
            if cut {
                notes.push(
                    b"part of the widget's text is cut off (outside the bar, or clipped): the shot shows only some of it"
                        .to_vec(),
                );
            }
            text_line(&t, PANEL_TEXT_MAX)
        }
        Ok(WidgetText::Hidden) => {
            notes.push(cat!(
                id,
                " is in the bar's layout but shows nothing now (hidden, or no width)"
            ));
            b"none".to_vec()
        }
        Ok(WidgetText::Absent) => {
            notes.push(cat!(
                id,
                " has no slot in the bar's layout (off the bar, or placed by another plugin): its text is not read"
            ));
            b"unknown".to_vec()
        }
    };
    // --open: its panel summoned, then the whole screen.
    let mut panel = None;
    let mut panel_text = None;
    if a.open {
        let layers = |n: &[u8]| layer_names(&ob(&[b"hyprctl", b"-b", n, b"-j", b"layers"]).out);
        let before = layers(name);
        let r = ob(&[
            b"run",
            b"-b",
            name,
            b"--",
            b"omarchy-shell",
            b"shell",
            b"summon",
            id,
        ]);
        if !r.ok {
            problems.push(cat!(
                "summon ",
                id,
                ": ",
                shell::subst(&r.err),
                shell::subst(&r.out)
            ));
        }
        ob(&still);
        // Its text: the layers the summon mapped (Omarchy's panels are layers of their own), else
        // what is new in the bar's (a popup drawn in the bar's layer, #315).
        let mut items: Vec<String> = Vec::new();
        let new: Vec<String> = layers(name)
            .into_iter()
            .filter(|n| !before.contains(n))
            .collect();
        let mut unread = Vec::new();
        for ns in &new {
            match layer_texts(name, ns.as_bytes()) {
                Ok(t) => items.extend(t.into_iter().map(|(s, ..)| s)),
                Err(why) => unread.push(why),
            }
        }
        if new.is_empty() {
            if let (Ok(old), Ok(now)) = (&bar, layer_texts(name, BAR_NS)) {
                items.extend(
                    now.into_iter()
                        .filter(|t| !old.contains(t))
                        .map(|(s, ..)| s),
                );
            }
        }
        if items.is_empty() {
            for why in unread {
                notes.push(cat!("the panel's text could not be read: ", why));
            }
        }
        panel_text = Some(text_line(&items, PANEL_TEXT_MAX));
        if a.own {
            notes.push(b"its panel is left open in the box".to_vec());
        }
        let out_p = a.out.as_ref().map(|o| suffixed(o, "panel"));
        let mut sh: Vec<&[u8]> = vec![b"shot", b"-b", name];
        if let Some(o) = &out_p {
            sh.extend([b"-o".as_slice(), o]);
        }
        let r = ob(&sh);
        panel = if r.ok {
            last_line(&r.out)
        } else {
            problems.push(cat!("shot: ", shell::subst(&r.err)));
            None
        };
    }
    // The shell's warnings that name the plugin (its QML's path holds its id): the box's own (no
    // portal, no system bus) are no problem of the plugin's.
    let log = ob(&[b"log", b"-b", name, b"shell", b"-n", b"all", b"--full"]);
    problems.extend(plugin_warnings(
        &crate::plugins::shell_warnings_of(&log.out),
        id,
    ));
    let mut head: Vec<(&str, &[u8])> = vec![("screenshot", shot.as_deref().unwrap_or(b"none"))];
    if let Some(p) = &panel {
        head.push(("panel", p));
    }
    head.push(("plugin", &line));
    head.push(("text", &text));
    if let Some(t) = &panel_text {
        head.push(("panel text", t));
    }
    if a.own {
        head.push(("box", name));
        // (what this look reported is not new to the next one)
        mark_write(ctx, &mark_now(ctx, name));
    }
    out(report(
        &head,
        &problems,
        &notes,
        None,
        a.keep.then_some(name),
    ))?;
    if problems.is_empty() {
        Ok(())
    } else {
        Err(Exit(1))
    }
}

/// The terminal look theme opens: the theme's 16 colours, as text and as blocks, on its background.
const PALETTE_SH: &str = r#"printf '\033[?25l\n  omabox look theme: the terminal palette\n\n'; for row in 0 8; do printf '  '; for i in 0 1 2 3 4 5 6 7; do n=$((row + i)); printf '\033[38;5;%dm %-6s\033[0m' "$n" "c$n"; done; printf '\n  '; for i in 0 1 2 3 4 5 6 7; do printf '\033[48;5;%dm       \033[0m' "$((row + i))"; done; printf '\n\n'; done; printf '  normal \033[1mbold\033[0m \033[2mdim\033[0m \033[3mitalic\033[0m \033[7mreverse\033[0m\n'; exec sleep 600"#;

fn theme(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let mut a = parse(ctx, args, "theme")?;
    let given = a.dir.clone().unwrap_or_default();
    let dir = crate::repo::readlink_f(&given).unwrap_or_else(|| given.clone());
    // A folder is the theme in the works; a bare word an installed theme's NAME (up resolves it).
    let dir = if shell::path(&dir).is_dir() {
        Some(dir)
    } else if !given.contains(&b'/') && !given.is_empty() {
        None
    } else {
        return Err(ctx.die(cat!("look theme: ", given, " is no folder")));
    };
    let me = std::process::id();
    let (name, guard) = pick(ctx, &mut a, me)?;
    let r = theme_in(ctx, &a, dir.as_deref(), &given, &name, me);
    if let Some(g) = guard {
        g.finish();
    }
    r
}

fn theme_in(ctx: &Ctx, a: &App, dir: Option<&[u8]>, given: &[u8], name: &[u8], me: u32) -> Res<()> {
    let mut problems: Vec<Vec<u8>> = Vec::new();
    let mut notes: Vec<Vec<u8>> = Vec::new();
    // Live from DIR (--theme-dir), applied before the session starts (--theme, by its folder's name);
    // an installed one by its NAME, checked by up itself (its notes).
    let tname = match dir {
        Some(d) => d.rsplit(|&c| c == b'/').next().unwrap_or(d).to_vec(),
        None => given.to_vec(),
    };
    let extra: Vec<&[u8]> = match dir {
        Some(d) => vec![b"--theme-dir", d, b"--theme", &tname],
        None => vec![b"--theme", &tname],
    };
    let up_err = start(ctx, a, name, me, &extra, &mut problems)?;
    // In a box that was up: the theme set there, as `omarchy-theme-set` does on a desk, and said (the
    // box stays on it). A folder is live only in a box started with it (--theme-dir): any other box
    // would show an installed theme of that name, or none.
    let mut own_named: Vec<Vec<u8>> = Vec::new();
    if a.own {
        let meta: Value = std::fs::read(ctx.at("box.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(Value::Null);
        let dirs: Vec<Vec<u8>> = meta
            .get("theme_dirs")
            .and_then(Value::as_array)
            .map(|v| {
                v.iter()
                    .filter_map(|d| d.as_str().map(|d| d.as_bytes().to_vec()))
                    .collect()
            })
            .unwrap_or_default();
        let real = |d: &[u8]| crate::repo::readlink_f(d).unwrap_or_else(|| d.to_vec());
        if let Some(d) = dir {
            if !dirs.iter().any(|b| real(b) == real(d)) {
                return Err(ctx.die(cat!(
                    "look theme: box '",
                    name,
                    "' does not have ",
                    d,
                    " as a live theme (it was not started with --theme-dir): omabox look theme ",
                    given,
                    " --fresh looks at it in a box of its own"
                )));
            }
        }
        let was = meta.get("theme").and_then(Value::as_str).unwrap_or("");
        let r = ob(&[b"run", b"-b", name, b"--", b"omarchy-theme-set", &tname]);
        if !r.ok {
            let _ = std::io::Write::write_all(&mut std::io::stderr(), &r.err);
            return Err(ctx.die(cat!(
                "look theme: omarchy-theme-set ",
                tname,
                " failed in box '",
                name,
                "'"
            )));
        }
        notes.push(cat!(
            "the box is on theme ",
            tname,
            " from now on",
            if was.is_empty() || was.as_bytes() == tname.as_slice() {
                String::new()
            } else {
                format!(" (it started on {was}: omabox run -- omarchy-theme-set {was})")
            }
        ));
        if dir.is_none() {
            let tree = meta
                .get("omarchy")
                .and_then(Value::as_str)
                .filter(|t| t.starts_with('/'))
                .unwrap_or("/usr/share/omarchy");
            own_named = crate::plugins::theme_folders(
                &tname,
                tree.as_bytes(),
                &dirs,
                &shell::env_or_empty("HOME"),
                &crate::jailpath::jail_path,
            );
        }
    }
    let still: [&[u8]; 6] = [b"wait", b"-b", name, b"--timeout", b"8s", b"still"];
    if ob(&still).code == 124 {
        notes.push(
            b"still changing after 8s (an intro video?): the shot is of a moving screen".to_vec(),
        );
    }
    let mut sh: Vec<&[u8]> = vec![b"shot", b"-b", name];
    if let Some(o) = &a.out {
        sh.extend([b"-o".as_slice(), o]);
    }
    let r = ob(&sh);
    let shot = if r.ok {
        last_line(&r.out)
    } else {
        problems.push(cat!("shot: ", shell::subst(&r.err)));
        None
    };
    // A terminal with its palette: the colours a theme is mostly looked at for.
    let mut term = None;
    let mut palette_pid: Option<Vec<u8>> = None;
    let r = ob(&[
        b"run",
        b"-b",
        name,
        b"-d",
        b"-q",
        b"--replace",
        b"--",
        b"foot",
        b"-T",
        b"omabox-palette",
        b"sh",
        b"-c",
        PALETTE_SH.as_bytes(),
    ]);
    if r.ok {
        palette_pid = last_line(&r.out).filter(|p| p.iter().all(u8::is_ascii_digit));
    }
    if r.ok
        && ob(&[
            b"wait",
            b"-b",
            name,
            b"--timeout",
            b"8s",
            b"window",
            b"title:^omabox-palette$",
        ])
        .ok
    {
        ob(&[
            b"wait",
            b"-b",
            name,
            b"--timeout",
            b"5s",
            b"still",
            b"--window",
            b"title:^omabox-palette$",
        ]);
        let out_t = a.out.as_ref().map(|o| suffixed(o, "terminal"));
        let mut sh: Vec<&[u8]> = vec![b"shot", b"-b", name, b"--window", b"title:^omabox-palette$"];
        if let Some(o) = &out_t {
            sh.extend([b"-o".as_slice(), o]);
        }
        let r = ob(&sh);
        term = if r.ok { last_line(&r.out) } else { None };
    } else {
        notes.push(b"no terminal shot: foot did not open in the box".to_vec());
    }
    // The theme's checks (#462), on this box's screen.
    let mon: Value =
        serde_json::from_slice(&ob(&[b"hyprctl", b"-b", name, b"-j", b"monitors"]).out)
            .unwrap_or(Value::Null);
    let m = mon
        .as_array()
        .and_then(|v| v.first())
        .cloned()
        .unwrap_or(Value::Null);
    let n = |k: &str| m.get(k).and_then(Value::as_i64).unwrap_or(0);
    let screen = if n("width") > 0 {
        (n("width"), n("height"))
    } else {
        (1920, 1080)
    };
    match dir {
        Some(d) => problems.extend(
            crate::theme_check::check(d, screen)
                .into_iter()
                .map(String::into_bytes),
        ),
        None if a.own => {
            // An installed theme's checks, as up makes them for --theme NAME.
            problems.extend(
                crate::theme_check::check_layered(&own_named, screen)
                    .into_iter()
                    .map(String::into_bytes),
            );
        }
        None => problems.extend(theme_notes(&up_err)),
    }
    // The palette terminal is the look's own: gone from a box that stays up.
    if a.own {
        if let Some(pid) = palette_pid {
            ob(&[b"stop", b"-b", name, &pid]);
        }
    }
    let mut head: Vec<(&str, &[u8])> = vec![("screenshot", shot.as_deref().unwrap_or(b"none"))];
    if let Some(t) = &term {
        head.push(("terminal", t));
    }
    head.push(("theme", &tname));
    if a.own {
        head.push(("box", name));
        // (what this look reported is not new to the next one)
        mark_write(ctx, &mark_now(ctx, name));
    }
    out(report(
        &head,
        &problems,
        &notes,
        None,
        a.keep.then_some(name),
    ))?;
    if problems.is_empty() {
        Ok(())
    } else {
        Err(Exit(1))
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
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(v: &[&str]) -> Vec<Vec<u8>> {
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

    /// #471: a look app whose program was not executable names the look that runs it.
    #[test]
    fn a_look_at_a_script_not_executable() {
        let cmd = w(&["./hello.py"]);
        let tail = b"setsid: failed to execute ./hello.py: Permission denied\n";
        assert_eq!(
            look_exit_hint(b"exited with status 126", &cmd, tail),
            [b"./hello.py is not executable: omabox look app -- python3 ./hello.py".to_vec()]
        );
        assert!(look_exit_hint(b"exited with status 0", &cmd, tail).is_empty());
        assert!(look_exit_hint(b"exited with status 1", &cmd, tail).is_empty());
        assert_eq!(
            look_exit_hint(b"exited with status 126, x", &cmd, tail).len(),
            1
        );
    }

    /// Finding 743: a look's program named relative to a folder its box does not have: run's note,
    /// with the look that mounts it, and a 126/127 hint on that look, not one that fails again.
    #[test]
    fn a_look_from_a_folder_its_box_lacks() {
        let err = b"omabox: note: hello.py is relative to /p/x, which box 'look-1' does not have: the command runs in the box HOME (/home/sbx); run --overlay /p/x (or --ro-bind, read-only) mounts it\nomabox: run: other\n";
        let cwd = run_cwd_said(err);
        let head = b"hello.py is relative to /p/x, which box 'look-1' does not have: the command runs in the box HOME (/home/sbx)";
        assert_eq!(cwd, Some((head.to_vec(), b"/p/x".to_vec())));
        assert!(run_cwd_said(b"omabox: run: x\n").is_none());
        let tail = b"setsid: failed to execute hello.py: No such file or directory\n";
        let cmd = w(&["hello.py"]);
        let v = look_exit_notes(
            b"exited with status 127",
            cwd.as_ref(),
            false,
            b"look-1",
            &cmd,
            tail,
        );
        assert_eq!(
            v[0],
            cat!(head, "; --overlay /p/x on the look mounts it"),
            "{v:?}"
        );
        assert_eq!(v.len(), 2, "{v:?}");
        // (the 127 hint names a file in the caller's folder only when there is one: here, none)
        let v = look_exit_notes(
            b"exited with status 2",
            cwd.as_ref(),
            false,
            b"look-1",
            &cmd,
            b"",
        );
        assert_eq!(
            v,
            [cat!(
                head,
                "; omabox look app --overlay /p/x -- hello.py mounts it"
            )]
        );
        let v = look_exit_notes(
            b"exited with status 2",
            cwd.as_ref(),
            true,
            b"mine",
            &cmd,
            b"",
        );
        assert_eq!(
            v,
            [cat!(
                head,
                "; down, then up -b mine --overlay /p/x (or --ro-bind, read-only) mounts it"
            )]
        );
        assert_eq!(
            look_exit_notes(b"exited with status 2", None, false, b"look-1", &cmd, b""),
            Vec::<Vec<u8>>::new()
        );
    }

    /// #471: a page's window line names a selector that picks it, not the word chromium.
    #[test]
    fn a_page_window_names_its_selector() {
        assert_eq!(
            report_window_line(
                true,
                "chrome-__x_index.html-Default",
                "Rocks",
                "800x600",
                "tiled",
                "0xab"
            ),
            b"chromium \"Rocks\" 800x600 (tiled; select it with --window 0xab)"
        );
        assert_eq!(
            report_window_line(false, "foot", "t", "800x600", "tiled", "0xab"),
            b"foot \"t\" 800x600 (tiled)"
        );
    }

    /// In a box that stays up a program's window is its own (its pid), else one that was not there
    /// when it was started; a throwaway box has none before, so any.
    #[test]
    fn the_window_of_a_look_in_a_box_that_was_up() {
        let w = br#"[{"address":"0xa","pid":7},{"address":"0xb","pid":9}]"#;
        let addr = |v: Option<Value>| v.and_then(|v| v["address"].as_str().map(String::from));
        assert_eq!(addresses(w), ["0xa", "0xb"]);
        assert_eq!(addr(window_of(w, Some(9), &[])).as_deref(), Some("0xb"));
        assert_eq!(addr(window_of(w, Some(1), &[])).as_deref(), Some("0xa"));
        assert_eq!(
            addr(window_of(w, Some(1), &["0xa".into()])).as_deref(),
            Some("0xb")
        );
        assert_eq!(
            addr(window_of(w, None, &["0xa".into(), "0xb".into()])),
            None
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
    fn notes_on_a_theme() {
        let e = b"omabox: box 'x' up (1920x1080) in 2s\nomabox: note: theme tally: colors.toml: no accent (x: y)\nomabox: note: theme tally: background 1.png: 640x480\nomabox: note: something else\n";
        assert_eq!(
            theme_notes(e),
            [
                b"colors.toml: no accent (x: y)".to_vec(),
                b"background 1.png: 640x480".to_vec()
            ]
        );
        assert!(theme_notes(b"").is_empty());
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

    #[test]
    fn a_pages_console() {
        let log = b"[1:2:1119/140516.305313:ERROR:dbus/bus.cc:405] Failed to connect to the bus\n[1:1:1119/140516.4:INFO:CONSOLE:2] \"log line\", source: file:///x/page/index.html (2)\n[1:1:1119/140516.6:INFO:CONSOLE:9] \"Uncaught ReferenceError: player is not defined\", source: file:///x/page/broken.html (9)\n[1:1:1:INFO:CONSOLE(4)] \"an error happened\", source: http://127.0.0.1:8081/app.js (4)\n";
        let c = console_lines(log);
        assert_eq!(
            c,
            [
                b"log line (index.html:2)".to_vec(),
                b"Uncaught ReferenceError: player is not defined (broken.html:9)".to_vec(),
                b"an error happened (app.js:4)".to_vec()
            ]
        );
        assert_eq!(console_problems(&c), c[1..].to_vec());
    }

    #[test]
    fn the_bar_strip() {
        let m =
            |r: &str| format!(r#"[{{"width":1920,"height":1080,"reserved":{r}}}]"#).into_bytes();
        assert_eq!(bar_strip(&m("[0,26,0,0]")).as_deref(), Some("0,0 1920x26"));
        assert_eq!(
            bar_strip(&m("[0,0,0,40]")).as_deref(),
            Some("0,1040 1920x40")
        );
        assert_eq!(bar_strip(&m("[48,0,0,0]")).as_deref(), Some("0,0 48x1080"));
        assert_eq!(
            bar_strip(&m("[0,0,30,0]")).as_deref(),
            Some("1890,0 30x1080")
        );
        assert_eq!(bar_strip(&m("[0,0,0,0]")), None);
        assert_eq!(suffixed(b"/x/a.png", "panel"), b"/x/a-panel.png");
        assert_eq!(suffixed(b"/x.d/a", "panel"), b"/x.d/a-panel");
        let w = b"Could not connect to DBus. UPower service will not work.\nqs:///p/plugins/me.x/Panel.qml[15:-1]: ReferenceError: y";
        assert_eq!(
            plugin_warnings(w, b"me.x"),
            [b"qs:///p/plugins/me.x/Panel.qml[15:-1]: ReferenceError: y".to_vec()]
        );
    }

    // #467: what a widget shows, from the bar's tree (`tree --layer omarchy-bar --json`, its text
    // items) and the shell's debugBarGeometry; a panel's from the layers its summon mapped.
    #[test]
    fn a_widgets_text() {
        let tree = br#"[{"depth": 0, "role": "frame", "name": "omarchy-bar", "states": [], "x": 0, "y": 30, "w": 1920, "h": 26},
 {"depth": 1, "role": "text", "name": "Thursday 21:25", "states": [], "x": 910, "y": 35, "w": 101, "h": 16},
 {"depth": 1, "role": "text", "name": "cups", "states": [], "x": 1845, "y": 35, "w": 30, "h": 16},
 {"depth": 1, "role": "label", "name": "", "text": "391", "states": ["cut"], "x": 1880, "y": 35, "w": 20, "h": 16}]"#;
        let geom =
            br#"[{"id":"omarchy.clock","x":901,"y":0,"width":118,"height":26,"visible":true},
 {"id":"example.cups","x":1837,"y":0,"width":75,"height":26,"visible":true},
 {"id":"omarchy.tray","x":1729,"y":0,"width":0,"height":0,"visible":false}]"#;
        assert_eq!(
            widget_text(tree, geom, "example.cups"),
            WidgetText::Shown(vec!["cups".into(), "391".into()], true),
            "the items in its slot, the bar's place added (a name, else the text), one cut"
        );
        assert_eq!(
            widget_text(tree, geom, "omarchy.clock"),
            WidgetText::Shown(vec!["Thursday 21:25".into()], false)
        );
        assert_eq!(widget_text(tree, geom, "omarchy.tray"), WidgetText::Hidden);
        assert_eq!(widget_text(tree, geom, "nope"), WidgetText::Absent);
        assert_eq!(widget_text(tree, b"", "example.cups"), WidgetText::Absent);
        assert_eq!(
            widget_text(b"[]", geom, "example.cups"),
            WidgetText::Shown(vec![], false)
        );
        assert_eq!(tree_texts(tree).1, (0, 30));
        assert_eq!(text_line(&["a".into(), "b c".into()], 60), b"a | b c");
        assert_eq!(
            text_line(&["a".into(), "b".into(), "c".into()], 2),
            b"a | b | ... and 1 more"
        );
        assert_eq!(text_line(&[], 60), b"none");
        let layers = br#"{"DP-1": {"levels": {"0": [{"namespace": "omarchy-background"}],
 "2": [{"namespace": "omarchy-bar"}, {"namespace": "omarchy-bar"}], "3": [{"namespace": "omarchy-keyboard-panel"}]}}}"#;
        assert_eq!(
            layer_names(layers),
            [
                "omarchy-background",
                "omarchy-bar",
                "omarchy-keyboard-panel"
            ]
        );
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
}
