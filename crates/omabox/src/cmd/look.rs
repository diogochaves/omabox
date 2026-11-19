//! `omabox look app [--keys "K..."] [--type TEXT] [--after DURATION] [--timeout 10s] [-o FILE]
//! [--keep-box] [UP OPTIONS] -- CMD [ARG...]` (#461): "I built this, does it look right and did it
//! log errors?" in one call. A throwaway box of its own (as a throwaway `run`'s), then the commands
//! an agent would chain: `run -d`, its window waited for, keys sent, `wait still`, `shot --window`,
//! the program's output read; the box taken down however this ends, unless --keep-box. Nothing new
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
        Some(b"-h" | b"--help") | None => crate::help::usage("look"),
        Some(o) if o.starts_with(b"-") => Err(crate::args::unknown_opt(ctx, "look", o)),
        Some(k) => Err(ctx.die(cat!(
            "look: app (more kinds to come: plugin, theme, page), got ",
            k
        ))),
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
}

fn parse(ctx: &Ctx, args: &[Vec<u8>]) -> Res<App> {
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
            b"--" => {
                a.cmd = args[i + 1..].to_vec();
                break;
            }
            b"--keys" => {
                a.keys.extend(
                    val(i)?
                        .split(|c| c.is_ascii_whitespace())
                        .filter(|w| !w.is_empty())
                        .map(<[u8]>::to_vec),
                );
                i += 1;
            }
            b"--type" => {
                a.keys.extend([b"-t".to_vec(), val(i)?.clone()]);
                i += 1;
            }
            b"--after" => {
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
            _ => {
                return Err(ctx.die(cat!(
                    "look app: the program goes after --: omabox look app [OPTIONS] -- CMD [ARG...], got ",
                    o
                )));
            }
        }
        i += 1;
    }
    if a.cmd.is_empty() {
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
    let a = parse(ctx, args)?;
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

fn app_in(ctx: &Ctx, a: &App, name: &[u8], me: u32) -> Res<()> {
    let mut problems: Vec<Vec<u8>> = Vec::new();
    let mut notes: Vec<Vec<u8>> = Vec::new();
    // up: its own process, its errors the caller's; --owner and no idle limit unless the box is kept.
    let owner = me.to_string();
    let mut up: Vec<&[u8]> = vec![b"up", name];
    if !a.keep {
        up.extend([b"--idle".as_slice(), b"0", b"--owner", owner.as_bytes()]);
    }
    up.extend(a.up.iter().map(Vec::as_slice));
    let r = ob(&up);
    if !r.ok {
        let _ = std::io::Write::write_all(&mut std::io::stderr(), &r.err);
        return Err(ctx.die("look: the box did not start"));
    }
    warnings(&r.err, &mut problems);
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
                None,
                b"",
                &problems,
                &notes,
                &output,
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
            let still = [
                b"wait".as_slice(),
                b"-b",
                name,
                b"--timeout",
                b"5s",
                b"still",
                b"--window",
                sel,
            ];
            if !a.keys.is_empty() {
                ob(&still);
            }
            let mut keys: Vec<&[u8]> = vec![b"keys", b"-b", name, b"--window", sel, b"--wait"];
            keys.extend(a.keys.iter().map(Vec::as_slice));
            if !a.keys.is_empty() {
                let r = ob(&keys);
                if !r.ok && r.code != 124 {
                    problems.push(cat!("keys: ", shell::subst(&r.err)));
                }
                warnings(&r.err, &mut problems);
            }
            if let Some(d) = &a.after {
                ob(&[b"wait", b"--json", b"for", d]);
            }
            let r = ob(&still);
            if r.code == 124 {
                notes.push(b"still changing after 5s: the shot is of a moving window".to_vec());
            }
            let mut sh: Vec<&[u8]> = vec![b"shot", b"-b", name, b"--window", sel];
            if let Some(o) = &a.out {
                sh.extend([b"-o".as_slice(), o]);
            }
            let r = ob(&sh);
            if r.ok {
                shot = r
                    .out
                    .split(|&c| c == b'\n')
                    .rfind(|l| !l.is_empty())
                    .map(<[u8]>::to_vec);
            } else {
                problems.push(cat!("shot: ", shell::subst(&r.err)));
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
                s("class"),
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
    let output = std::fs::read(shell::path(&log)).unwrap_or_default();
    problems.extend(problem_lines(&output));
    out(report(
        shot.as_deref(),
        &window_line,
        &problems,
        &notes,
        &output,
        a.keep.then_some(name),
    ))?;
    if problems.is_empty() {
        Ok(())
    } else {
        Err(Exit(1))
    }
}

/// The report, as the issue drew it.
pub fn report(
    shot: Option<&[u8]>,
    window: &[u8],
    problems: &[Vec<u8>],
    notes: &[Vec<u8>],
    output: &[u8],
    kept: Option<&[u8]>,
) -> Vec<u8> {
    let cut = |l: &[u8]| crate::cmd::log::cut_line(l, LINE_WIDTH);
    let mut r = Vec::new();
    r.extend(cat!("screenshot: ", shot.unwrap_or(b"none"), "\n"));
    if !window.is_empty() {
        r.extend(cat!("window: ", window, "\n"));
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
    fn the_report() {
        let r = report(
            Some(b"/tmp/x.png"),
            b"foot \"t\" 800x600 (tiled)",
            &[],
            &[],
            b"",
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
            None,
            b"",
            &[b"no window".to_vec()],
            &[],
            &out,
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
