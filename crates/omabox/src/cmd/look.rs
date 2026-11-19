//! `omabox look app [--keys "K..."] [--type TEXT] [--after DURATION] [--timeout 10s] [-o FILE]
//! [--keep-box] [UP OPTIONS] -- CMD [ARG...]` and `omabox look plugin DIR [--open] [-o FILE]
//! [--keep-box] [UP OPTIONS]` (#461): "I built this, does it look right and did it log errors?" in one
//! call. A throwaway box of its own (as a throwaway `run`'s), then the commands an agent would chain:
//! for an app `run -d`, its window waited for, keys sent, `wait still`, `shot --window`, the program's
//! output read; for a plugin `up --plugin`, its state, the bar shot, with --open its panel summoned and
//! shot, the shell's warnings. The box is taken down however this ends, unless --keep-box. Nothing new
//! is isolated here: every step is one of omabox's own commands, run as a subprocess, so S1-S10 hold
//! as they are. One short report on stdout; exit 0 when it found no problem, 1 when it did, and an
//! error of omabox's own as its other commands do.

use crate::args::optarg;
use crate::cat;
use crate::exit::{Ctx, Exit, Res, out};
use crate::num::ms_duration;
use crate::select::select_box;
use crate::shell::{self, Re};
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
    match args.first().map(Vec::as_slice) {
        Some(b"app") => app(ctx, &args[1..]),
        Some(b"plugin") => plugin(ctx, &args[1..]),
        Some(b"theme") => theme(ctx, &args[1..]),
        Some(b"page") => page(ctx, &args[1..]),
        Some(b"-h" | b"--help") | None => crate::help::usage("look"),
        Some(o) if o.starts_with(b"-") => Err(crate::args::unknown_opt(ctx, "look", o)),
        Some(k) => Err(ctx.die(cat!("look: app, plugin, theme or page, got ", k))),
    }
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

/// The first window of the box: the job's (its pid), else any; None before one is up.
fn window_of(windows: &[u8], pid: Option<i64>) -> Option<Value> {
    let v: Value = serde_json::from_slice(windows).ok()?;
    let all = v.as_array()?;
    all.iter()
        .find(|w| pid.is_some() && w.get("pid").and_then(Value::as_i64) == pid)
        .or_else(|| all.first())
        .cloned()
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
    let a = parse(ctx, args, "app")?;
    let me = std::process::id();
    let name = format!("look-{me}").into_bytes();
    select_box(ctx, &name)?;
    // Taken down however this ends, as a throwaway run's (the box's reaper retries once we are gone).
    let guard = (!a.keep).then(|| crate::sig::Teardown::new(name.clone()));
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
        a.up.extend([b"--ro-bind".to_vec(), dirn]);
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
    let name = format!("look-{me}").into_bytes();
    select_box(ctx, &name)?;
    let guard = (!a.keep).then(|| crate::sig::Teardown::new(name.clone()));
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
    let mut run: Vec<&[u8]> = vec![b"run", b"-b", name, b"-d", b"-q", b"--print-log", b"--"];
    run.extend(a.cmd.iter().map(Vec::as_slice));
    let r = ob(&run);
    if !r.ok {
        // One that ended at once ran: its end and its output are the report.
        if let Some((end, log)) = ended_at_once(&r.err) {
            problems.push(cat!("it ", end, ", before opening a window"));
            let output = std::fs::read(shell::path(&log)).unwrap_or_default();
            problems.extend(problem_lines(&output));
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
        win = window_of(&ob(&[b"windows", b"-b", name, b"--json"]).out, pid);
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
            window_line = cat!(
                if a.page {
                    "chromium".to_string()
                } else {
                    s("class")
                },
                " \"",
                s("title"),
                "\" ",
                size,
                " (",
                layout,
                ")"
            );
            // Ended meanwhile (a crash after its first frame).
            if pid.is_some() && job_pid(name, &log).is_none() {
                if let Some(e) = pid.and_then(|p| job_end(name, p)) {
                    problems.push(cat!("it ", e, " after opening its window"));
                }
            }
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
) -> Res<()> {
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
    Ok(())
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
    let a = parse(ctx, args, "plugin")?;
    let dir = a.dir.clone().unwrap_or_default();
    let Some(id) = plugin_id(&dir) else {
        return Err(ctx.die(cat!(
            "look plugin: ",
            dir,
            " has no manifest.json with an id: not a plugin's folder"
        )));
    };
    let me = std::process::id();
    let name = format!("look-{me}").into_bytes();
    select_box(ctx, &name)?;
    let guard = (!a.keep).then(|| crate::sig::Teardown::new(name.clone()));
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
    let ls: Value = serde_json::from_slice(&ob(&[b"ls", b"--json"]).out).unwrap_or(Value::Null);
    let st = ls
        .as_array()
        .and_then(|v| {
            v.iter()
                .find(|b| b.get("name").and_then(Value::as_str).map(str::as_bytes) == Some(name))
        })
        .and_then(|b| {
            b.get("plugin_status")?
                .get(std::str::from_utf8(id).ok()?)
                .cloned()
        })
        .unwrap_or(Value::Null);
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
    // --open: its panel summoned, then the whole screen.
    let mut panel = None;
    if a.open {
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
    let a = parse(ctx, args, "theme")?;
    let dir = a.dir.clone().unwrap_or_default();
    let dir = crate::repo::readlink_f(&dir).unwrap_or(dir);
    if !shell::path(&dir).is_dir() {
        return Err(ctx.die(cat!("look theme: ", dir, " is no folder")));
    }
    let me = std::process::id();
    let name = format!("look-{me}").into_bytes();
    select_box(ctx, &name)?;
    let guard = (!a.keep).then(|| crate::sig::Teardown::new(name.clone()));
    let r = theme_in(ctx, &a, &dir, &name, me);
    if let Some(g) = guard {
        g.finish();
    }
    r
}

fn theme_in(ctx: &Ctx, a: &App, dir: &[u8], name: &[u8], me: u32) -> Res<()> {
    let mut problems: Vec<Vec<u8>> = Vec::new();
    let mut notes: Vec<Vec<u8>> = Vec::new();
    // Live from DIR (--theme-dir), applied before the session starts (--theme, by its folder's name).
    let tname = dir.rsplit(|&c| c == b'/').next().unwrap_or(dir).to_vec();
    start(
        ctx,
        a,
        name,
        me,
        &[b"--theme-dir", dir, b"--theme", &tname],
        &mut problems,
    )?;
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
    let r = ob(&[
        b"run",
        b"-b",
        name,
        b"-d",
        b"-q",
        b"--",
        b"foot",
        b"-T",
        b"omabox-palette",
        b"sh",
        b"-c",
        PALETTE_SH.as_bytes(),
    ]);
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
    problems.extend(
        crate::theme_check::check(dir, screen)
            .into_iter()
            .map(String::into_bytes),
    );
    let mut head: Vec<(&str, &[u8])> = vec![("screenshot", shot.as_deref().unwrap_or(b"none"))];
    if let Some(t) = &term {
        head.push(("terminal", t));
    }
    head.push(("theme", &tname));
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
        for p in problems.iter().take(PROBLEMS_MAX) {
            r.extend(cat!("  ", cut(p), "\n"));
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
