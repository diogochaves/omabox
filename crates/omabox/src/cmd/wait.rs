//! `omabox wait [-b NAME] [--timeout 10s] [--json] COND` (finding 82): until the screen holds still
//! or changes (omabox-still in the box: settle.rs), a window or layer is there or gone, a command
//! succeeds (polled here), a run -d job ends or logs a line (#299), a box log gets a new line
//! (wait_log.rs, #330), or the accessibility tree holds a text (treewait.rs, #319); `wait for` is a
//! fixed time, said as one. One line on stdout (or --json); 0 satisfied, 124 not in time, 1 unknown.

use crate::args::{optarg, unexpected, unknown_opt, unknown_word};
use crate::boxes::{box_alive, need_box, touch};
use crate::cat;
use crate::cmd::wait_log::{LogWait, for_wait, log_wait};
use crate::exec::{ErrTo, ExecOpts, Out, Redir, box_exec, on_box};
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::hypr::{hung_text, hypr_answers_in, hypr_json};
use crate::jail::in_jail;
use crate::jobs::{Job, job_find, job_how, job_latest, job_procs_now, job_status};
use crate::jq::{self, Args};
use crate::jqprog::WIN_DESC_JQ;
use crate::num::{fixed, ms_duration, ms_s, xywh_less};
use crate::select::select_box;
use crate::settle::{
    Settle, WAIT_MAX_MS, cursor_now, cursor_rect, geom_csv, geom_parse, mono_ms, rects_meet,
};
use crate::shell::{dec, strip_nl};
use crate::shellnote::shell_note;
use crate::shotcmp::{Diff, ShotRef};
use crate::win::{
    box_windows, joined, jq_error_text, jq_length, jq_r, win_desc, win_hits, win_select,
};
use std::os::unix::ffi::OsStrExt;
use std::time::Duration;

/// A polled condition's one try (win_probe, layer_probe, until_probe, tree_probe): "yes DETAIL" or
/// "no DETAIL"; status 2 a selector never answered (a bad regex), 3 a failure already said (exit 1),
/// any other failure the box not answering.
enum Probe<'a> {
    Win(&'a str, &'a [Vec<u8>]),
    Layer(&'a str, &'a [u8], &'a [u8]),
    Until(&'a [Vec<u8>]),
    /// A `run -d` job's end (#299): its command's, or with GONE its whole session's.
    Job(&'a Job, bool),
    /// A line matching RE (grep -E, ICASE: -i) in a `run -d` job's log (#299).
    Log(&'a Job, &'a [u8], bool),
    Tree(&'a crate::treewait::TreeProbe),
}

impl Probe<'_> {
    /// Its name, as poll_until's --json condition has it.
    fn name(&self) -> &'static str {
        match self {
            Probe::Win(..) => "win_probe",
            Probe::Layer(..) => "layer_probe",
            Probe::Until(..) => "until_probe",
            Probe::Job(..) => "job_probe",
            Probe::Log(..) => "log_probe",
            Probe::Tree(..) => "tree_probe",
        }
    }

    /// `out=$(PROBE)`: its status and its output (trailing newlines off).
    fn run(&self, ctx: &mut Ctx, t0: i64, tmo: i64) -> (i32, Vec<u8>) {
        match self {
            Probe::Win(want, sels) => {
                let wins = {
                    let _quiet = Redir::null(2);
                    box_windows(ctx, None)
                };
                // (`wins=$(box_windows 2>/dev/null) || return 1`)
                match wins {
                    Ok(w) => win_answer(want, &w, sels),
                    Err(_) => (1, Vec::new()),
                }
            }
            Probe::Layer(want, ns, mon) => {
                let layers = {
                    let _quiet = Redir::null(2);
                    hypr_json(ctx, &["layers"])
                };
                let Ok(layers) = layers else {
                    return (1, Vec::new());
                };
                (0, layer_answer(want, &strip_nl(&layers), ns, mon))
            }
            Probe::Job(job, gone) => {
                match box_alive(ctx) {
                    Ok(true) => {}
                    Ok(false) => return (1, Vec::new()),
                    Err(Exit(rc)) => return (i32::from(rc), Vec::new()),
                }
                let procs = job_procs_now(ctx, job);
                let st = job_status(ctx, job);
                let how = job_how(st.as_deref().unwrap_or_default());
                let n = procs.len();
                let o = if procs.is_empty() {
                    cat!("yes ", how)
                } else if *gone {
                    format!("no {n} of its processes still run").into_bytes()
                } else if st.is_some() {
                    let s = if n == 1 { "" } else { "es" };
                    format!(
                        "yes {how}; {n} process{s} it started still run (--gone waits for them)"
                    )
                    .into_bytes()
                } else {
                    b"no its command still runs".to_vec()
                };
                (0, o)
            }
            Probe::Log(job, re, icase) => {
                match box_alive(ctx) {
                    Ok(true) => {}
                    Ok(false) => return (1, Vec::new()),
                    Err(Exit(rc)) => return (i32::from(rc), Vec::new()),
                }
                // (whether it still ran, read before its log: a line it wrote as it ended is seen)
                let ended = job_procs_now(ctx, job).is_empty();
                let Some(log) = job.box_log() else {
                    return (0, b"stop no log: the job has no log of run -d's".to_vec());
                };
                let mut argv = vec![
                    b"/usr/bin/timeout".to_vec(),
                    b"2".to_vec(),
                    b"/usr/bin/grep".to_vec(),
                    b"-m1".to_vec(),
                    b"-a".to_vec(),
                    b"-E".to_vec(),
                ];
                if *icase {
                    argv.push(b"-i".to_vec());
                }
                argv.extend([b"-e".to_vec(), re.to_vec(), b"--".to_vec(), log]);
                let r = on_box(ctx, &argv, true, Out::Capture(Some(4096)), ErrTo::Null);
                if r.refused {
                    return (r.rc, Vec::new());
                }
                if r.rc == 0 {
                    let line = r.out.split(|&c| c == b'\n').next().unwrap_or_default();
                    return (0, cat!("yes ", crate::shell::cut_line(line, 200)));
                }
                if ended {
                    let how = job_how(job_status(ctx, job).as_deref().unwrap_or_default());
                    return (
                        0,
                        cat!(
                            "stop job ended: job ",
                            job.pid,
                            " ",
                            how,
                            ", and its log has no line matching ",
                            *re
                        ),
                    );
                }
                (0, b"no".to_vec())
            }
            Probe::Until(cmd) => {
                let left = tmo - (mono_ms() - t0);
                match box_alive(ctx) {
                    Ok(true) => {}
                    Ok(false) => return (1, Vec::new()),
                    Err(Exit(rc)) => return (i32::from(rc), Vec::new()),
                }
                let secs = fixed(left.max(100) as f64 / 1000.0, 3);
                let mut argv = vec![
                    b"/usr/bin/timeout".to_vec(),
                    b"-k".to_vec(),
                    b"1".to_vec(),
                    secs.into_bytes(),
                ];
                argv.extend(cmd.iter().cloned());
                // (its stderr kept, each try's over the last: the deciding one's is said, #400)
                let errf = std::fs::File::create(ctx.at(UNTIL_ERR)).ok();
                let rc = {
                    let _o = Redir::null(1);
                    let _e = match &errf {
                        Some(f) => Redir::to(2, std::os::fd::AsRawFd::as_raw_fd(f)),
                        None => Redir::null(2),
                    };
                    box_exec(ctx, &argv, ExecOpts::default())
                };
                // box_exec's die ends the probe's `$(...)` with its status: no answer from the
                // box, poll_until's to say (its "went down" or "could not ask").
                let rc = match rc {
                    Ok(rc) => rc,
                    Err(Exit(rc)) => return (i32::from(rc), Vec::new()),
                };
                // (126, 127: it cannot run in the box, which no retry changes: a host path is not
                // there, #400; it was retried to the timeout)
                match rc {
                    0 => (0, b"yes".to_vec()),
                    127 => (0, b"stop exit 127: command not found in the box".to_vec()),
                    126 => (
                        0,
                        b"stop exit 126: command found but could not run in the box".to_vec(),
                    ),
                    _ => (0, format!("no exit {rc}").into_bytes()),
                }
            }
            Probe::Tree(p) => crate::treewait::answer(ctx, p, t0, tmo),
        }
    }
}

/// layer_probe's answer from LAYERS (`hyprctl -j layers`), WANT present or gone: "yes at X,Y WxH on
/// MONITOR" for the drawn layer of namespace NS ("yes N layers: ...; ..." for several), on MON alone
/// when MON is not empty (#316); "no" with where it is drawn instead, or the monitor the box does not
/// have. An answer jq could not read is "no".
fn layer_answer(want: &str, layers: &[u8], ns: &[u8], mon: &[u8]) -> Vec<u8> {
    let r = Args::new(&["-r"])
        .kv("--arg", "want", want)
        .kv("--arg", "n", ns)
        .kv("--arg", "mon", mon)
        .arg(crate::jqprog::LAYER_WAIT_JQ)
        .run(Some(layers), jq::Err::Null);
    let t = r.text();
    if r.ok() && (t.starts_with(b"yes") || t.starts_with(b"no")) {
        t
    } else {
        b"no".to_vec()
    }
}

/// The file in the box dir (host-only) that holds `wait until`'s last try's stderr.
const UNTIL_ERR: &str = "until.err";

/// The last N lines of TEXT that hold anything, each cut to 200 bytes.
fn last_lines(text: &[u8], n: usize) -> Vec<Vec<u8>> {
    let ls: Vec<&[u8]> = text
        .split(|&c| c == b'\n')
        .filter(|l| !l.trim_ascii().is_empty())
        .collect();
    ls[ls.len().saturating_sub(n)..]
        .iter()
        .map(|l| l[..l.len().min(200)].to_vec())
        .collect()
}

/// `wait until`'s note when CMD (a word of it, or a `sh -c` string) names a path the box cannot
/// see (#400): one under the box's own folder here (its HOME is /home/sbx in the box), or under the
/// caller's $TMPDIR (the box has a /tmp of its own). Such a command fails in the box; behind a `!`
/// it succeeded at once.
fn host_path_note(ctx: &Ctx, cmd: &[Vec<u8>]) -> Option<Vec<u8>> {
    host_path_note_with(&ctx.d, crate::shell::env("TMPDIR"), cmd)
}

/// host_path_note for box folder D and TMPDIR TMP.
fn host_path_note_with(d: &[u8], tmp: Option<Vec<u8>>, cmd: &[Vec<u8>]) -> Option<Vec<u8>> {
    let home = cat!(d, "/home");
    let tmp = tmp
        .map(|t| t.strip_suffix(b"/").map_or(t.clone(), <[u8]>::to_vec))
        .filter(|t| !t.is_empty() && t != b"/tmp" && t.starts_with(b"/"));
    let under = |w: &[u8], dir: &[u8]| {
        w.windows(dir.len() + 1)
            .any(|x| x.starts_with(dir) && matches!(x[dir.len()], b'/'))
            || w.ends_with(dir)
    };
    for w in cmd {
        if under(w, &home) {
            return Some(cat!(
                "wait until: the command names ",
                home,
                "/..., this box's HOME as the host has it: in the box it is /home/sbx/..."
            ));
        }
        if under(w, d) {
            return Some(cat!(
                "wait until: the command names ",
                d,
                "/..., this box's folder on the host: the box does not see it"
            ));
        }
        if let Some(t) = tmp.as_deref().filter(|t| under(w, t)) {
            return Some(cat!(
                "wait until: the command names ",
                t,
                "/..., your $TMPDIR on the host: the box does not see it (it has a /tmp of its own; omabox up --ro-bind DIR binds a folder)"
            ));
        }
    }
    None
}

/// win_answer WANT WINDOWS SEL...: win_probe's answer for these windows.
pub fn win_answer(want: &str, wins: &[u8], sels: &[Vec<u8>]) -> (i32, Vec<u8>) {
    // No list: the box not answering, not "none match" (finding 176).
    if wins.is_empty() {
        return (1, Vec::new());
    }
    let hits = match win_hits(wins, sels) {
        Ok(h) => h,
        Err(e) => {
            say(cat!(
                "bad window selector (",
                joined(sels),
                "): ",
                jq_error_text(&e)
            ));
            return (2, Vec::new());
        }
    };
    let n = jq_length(&hits);
    let prog = match want {
        "gone" => {
            let o = if n == b"0" {
                b"yes".to_vec()
            } else {
                cat!("no ", n, " matching")
            };
            return (0, o);
        }
        "focused" => crate::jqprog::WIN_FOCUSED_JQ,
        _ => crate::jqprog::WIN_PRESENT_JQ,
    };
    let r = jq::jq(
        &["-r", &format!("{WIN_DESC_JQ}{prog}")],
        Some(&hits),
        jq::Err::Inherit,
    );
    (r.rc, r.text())
}

/// poll_until YES NO NEED TIMEOUT_MS JSON PROBE: PROBE every 100 ms until it says yes NEED times in a
/// row (2 for a window or layer: one caught mid-map or mid-close is not an answer), or the deadline.
#[allow(clippy::too_many_arguments)]
fn poll_until(
    ctx: &mut Ctx,
    yes_text: &[u8],
    no_text: &[u8],
    need: u32,
    tmo: i64,
    json: bool,
    probe: Probe,
) -> Res<()> {
    // (the monotonic clock: finding 450)
    let t0 = mono_ms();
    let mut used = std::time::Instant::now();
    let (mut yes, mut detail) = (0u32, Vec::new());
    let mut now: i64;
    let (res, msg): (&str, Vec<u8>);
    loop {
        let (rc, o) = probe.run(ctx, t0, tmo);
        now = mono_ms() - t0;
        if rc == 2 {
            return Err(Exit(2));
        }
        if rc == 3 {
            return Err(Exit(1));
        }
        if rc != 0 {
            // A probe that could not ask a box still up: its Hyprland not answering, said so (#125).
            if box_alive(ctx)? {
                if !hypr_answers_in(ctx, "1")? {
                    let t = hung_text(ctx, "")?;
                    return Err(ctx.die(t));
                }
                return Err(ctx.die(cat!("wait: could not ask box '", ctx.name, "'")));
            }
            res = "unknown";
            msg = cat!(
                "box '",
                ctx.name,
                "' went down after ",
                ms_s(now.to_string())
            );
            break;
        }
        if let Some(d) = o.strip_prefix(b"stop ") {
            // (an answer no retry changes: said at once, not known)
            let (code, why) = match d.iter().position(|&c| c == b':') {
                Some(i) => (&d[..i], d[i + 1..].trim_ascii_start()),
                None => (d, &b""[..]),
            };
            detail = code.to_vec();
            res = "unknown";
            msg = cat!(why, " (", code, ") after ", ms_s(now.to_string()));
            break;
        }
        detail = match o.iter().position(|&c| c == b' ') {
            Some(i) => o[i + 1..].to_vec(),
            None => Vec::new(),
        };
        if o.starts_with(b"yes") {
            yes += 1;
            if yes >= need {
                let d = if detail.is_empty() {
                    Vec::new()
                } else {
                    cat!(": ", detail)
                };
                res = "satisfied";
                msg = cat!(yes_text, " after ", ms_s(now.to_string()), d);
                break;
            }
        } else {
            yes = 0;
        }
        if now >= tmo {
            let d = if detail.is_empty() {
                Vec::new()
            } else {
                cat!(" (", detail, ")")
            };
            res = "unsatisfied";
            msg = cat!(no_text, " after ", ms_s(now.to_string()), d);
            break;
        }
        if used.elapsed() >= std::time::Duration::from_secs(30) {
            touch(&ctx.at("used"));
            used = std::time::Instant::now();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // A window or layer that never came: a run -d job that ended meanwhile is said now, as the next
    // command would (job_note), not after it (#335: its app may have died of a protocol error).
    if res == "unsatisfied" && matches!(probe, Probe::Win(..) | Probe::Layer(..)) {
        crate::jobs::job_note(ctx);
    }
    if json {
        let r = Args::new(&["-nc"])
            .kv("--arg", "res", res)
            .kv("--arg", "cond", probe.name())
            .kv("--arg", "msg", &msg)
            .kv("--arg", "t", now.to_string())
            .kv("--arg", "d", &detail)
            .arg(crate::jqprog::POLL_JSON_JQ)
            .run(None, jq::Err::Inherit);
        out(&r.out)?;
    } else {
        out(cat!(res, ": ", msg, "\n"))?;
    }
    match res {
        "satisfied" => Ok(()),
        "unsatisfied" => Err(Exit(124)),
        _ => Err(Exit(1)),
    }
}

/// A number omabox cannot read in WHAT (one past 64 bits; finding 448: bash's arithmetic refused an
/// X of 08 in its own words): said, the command ends, 1.
fn num_fail(ctx: &Ctx, what: &[u8]) -> Exit {
    ctx.die(cat!("wait: cannot read the numbers of ", what))
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut json, mut tmo, mut cond) = (Vec::new(), false, 10_000i64, Vec::new());
    let (mut quiet, mut strict, mut geom) = (Vec::new(), false, Vec::new());
    let (mut sels, mut ns, mut cmd): (Vec<Vec<u8>>, Vec<u8>, Vec<Vec<u8>>) =
        (Vec::new(), Vec::new(), Vec::new());
    let mut want = "";
    let (mut jobw, mut logw, mut icase): (Vec<u8>, Vec<Vec<u8>>, bool) =
        (Vec::new(), Vec::new(), false);
    let mut mon = Vec::new();
    // wait tree TEXT [--layer SEL] [--shot FILE] (#319)
    let (mut text, mut layer, mut shot): (Option<Vec<u8>>, Vec<u8>, Vec<u8>) =
        (None, Vec::new(), Vec::new());
    // a box log's wait and a fixed one (#330): --grep RE (log's way of giving RE), --since, DURATION
    let (mut grep, mut since, mut tmo_set, mut dur): (Option<Vec<u8>>, Option<i64>, bool, Vec<u8>) =
        (None, None, false, Vec::new());
    let mut masks: Vec<Vec<u8>> = Vec::new();
    // --since SHOT (change) or --like SHOT (still): the screen against an earlier shot (#344).
    let (mut base, mut base_opt): (Vec<u8>, &str) = (Vec::new(), "");
    let mut s = Settle::default();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let v = args.get(i + 1);
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, v)?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"--json" => json = true,
            b"--ignore" => {
                optarg(ctx, a, v)?;
                let m = s.mask_arg(ctx, &args[i + 1])?;
                masks.extend([b"--mask".to_vec(), m]);
                i += 1;
            }
            b"--timeout" => {
                optarg(ctx, a, v)?;
                let Some(ms) = ms_duration(&args[i + 1]) else {
                    return Err(ctx.die("wait: --timeout takes a duration: 500ms, 10s, 2m"));
                };
                if ms > WAIT_MAX_MS {
                    return Err(ctx.die(cat!(
                        "wait: --timeout is at most 10m (got ",
                        args[i + 1],
                        ")"
                    )));
                }
                tmo = ms;
                tmo_set = true;
                i += 1;
            }
            // `--grep -i RE` as grep users type it, as log takes it.
            b"--grep" => {
                optarg(ctx, a, v)?;
                if args[i + 1] == b"-i" {
                    icase = true;
                    i += 1;
                    optarg(ctx, &args[i], args.get(i + 1))?;
                }
                let r = &args[i + 1];
                if r.starts_with(b"-") {
                    return Err(ctx.die(cat!(
                        "wait: --grep takes RE (-i goes before or after it; an RE starting with - as [-]...), got ",
                        r
                    )));
                }
                grep = Some(r.clone());
                i += 1;
            }
            b"--quiet" => {
                optarg(ctx, a, v)?;
                let Some(ms) = ms_duration(&args[i + 1]) else {
                    return Err(ctx.die("wait: --quiet takes a duration: 300ms, 1s"));
                };
                if ms > WAIT_MAX_MS {
                    return Err(ctx.die(cat!(
                        "wait: --quiet is at most 10m (got ",
                        args[i + 1],
                        ")"
                    )));
                }
                quiet = ms.to_string().into_bytes();
                i += 1;
            }
            b"--strict" => strict = true,
            b"--since" | b"--like" => {
                optarg(ctx, a, v)?;
                crate::shots::two_files(ctx, "wait", &base, &args[i + 1])?;
                base = args[i + 1].clone();
                base_opt = if a == b"--since" { "--since" } else { "--like" };
                i += 1;
            }
            b"-g" => {
                optarg(ctx, a, v)?;
                geom = args[i + 1].clone();
                i += 1;
            }
            b"-w" | b"--window" => {
                optarg(ctx, a, v)?;
                sels.push(args[i + 1].clone());
                i += 1;
            }
            b"--active" => sels.push(b"active".to_vec()),
            b"--monitor" => {
                optarg(ctx, a, v)?;
                if !crate::shell::re_match("^[A-Za-z0-9_-]{1,32}$", &args[i + 1]) {
                    return Err(ctx.die(cat!(
                        "wait: --monitor takes a monitor's name (omabox monitor list), got ",
                        args[i + 1]
                    )));
                }
                mon = args[i + 1].clone();
                i += 1;
            }
            b"-l" | b"--layer" => {
                optarg(ctx, a, v)?;
                layer = args[i + 1].clone();
                i += 1;
            }
            b"--shot" => {
                optarg(ctx, a, v)?;
                shot = args[i + 1].clone();
                i += 1;
            }
            b"--gone" => want = "gone",
            b"--focused" => want = "focused",
            b"-i" => icase = true,
            b"--" => {
                if cond != b"until" {
                    return Err(ctx.die("wait: -- goes with until (omabox wait until -- CMD ...)"));
                }
                cmd = args[i + 1..].to_vec();
                break;
            }
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "wait", a)),
            // `wait cmd` was until's name up to 0.8.0 (#272): refused since 1.0 (#287).
            _ => {
                if cond.is_empty() {
                    if a == b"cmd" {
                        return Err(ctx.usage_die(
                            "wait cmd is refused since 1.0: write wait until -- CMD (the same flags)",
                        ));
                    }
                    cond = a.clone();
                } else if cond == b"window" {
                    sels.push(a.clone());
                } else if cond == b"layer" && ns.is_empty() {
                    ns = a.clone();
                } else if cond == b"job" && jobw.is_empty() {
                    jobw = a.clone();
                } else if cond == b"log" && logw.len() < 2 {
                    logw.push(a.clone());
                } else if cond == b"tree" && text.is_none() {
                    text = Some(a.clone());
                } else if cond == b"for" && dur.is_empty() {
                    dur = a.clone();
                } else {
                    return Err(unexpected(ctx, cat!("wait ", cond), a, ""));
                }
            }
        }
        i += 1;
    }
    let none = geom.is_empty() && quiet.is_empty();
    if icase && cond != b"log" {
        return Err(ctx.die("wait: -i goes with log (wait log [JOB] RE -i)"));
    }
    if !mon.is_empty() && cond != b"layer" {
        return Err(
            ctx.die("wait: --monitor goes with layer (wait layer NAMESPACE --monitor NAME)")
        );
    }
    if (!layer.is_empty() || !shot.is_empty()) && cond != b"tree" && !cond.is_empty() {
        return Err(ctx.die(
            "wait: --layer and --shot go with tree (wait layer takes its NAMESPACE as a word)",
        ));
    }
    // --since is a SHOT for change (#344) and a byte offset into the log for log (#330).
    if cond == b"log" && base_opt == "--since" {
        if !crate::shell::re_match("^[0-9]{1,15}$", &base) {
            return Err(ctx.die(cat!(
                "wait log: --since takes a byte offset into the log (0: its start; --json's offset: where the line waited for ends), got ",
                base
            )));
        }
        since = dec(&base);
        (base, base_opt) = (Vec::new(), "");
    }
    if cond != b"log" && grep.is_some() {
        return Err(ctx.die("wait: --grep goes with log"));
    }
    // wait log's source (#330): none, a job's pid or `run` (a run -d job's log, #299), or a box log.
    let mut boxlog: Option<Vec<u8>> = None;
    let plain = sels.is_empty() && none && !strict && masks.is_empty();
    match cond.as_slice() {
        b"still" | b"change" => {
            if !want.is_empty() {
                return Err(ctx.die("wait: --gone and --focused go with window, layer or job (--gone with tree too)"));
            }
            if !geom.is_empty() && !sels.is_empty() {
                return Err(ctx.die("wait: -g or --window, not both"));
            }
        }
        b"window" => {
            if sels.is_empty() {
                return Err(ctx.die("wait window: which window? (a SEL, as for shot --window)"));
            }
            if !(none && !strict && masks.is_empty()) {
                return Err(ctx.die(
                    "wait window: -g, --quiet, --strict and --ignore go with still and change",
                ));
            }
        }
        b"layer" => {
            if ns.is_empty() {
                return Err(ctx.die(
                    "wait layer: which layer? its namespace (omabox hyprctl layers): omarchy-menu, ...",
                ));
            }
            if want == "focused" {
                return Err(ctx.die("wait layer: --gone or nothing"));
            }
            if !(sels.is_empty() && none && !strict && masks.is_empty()) {
                return Err(ctx.die("wait layer: only --gone and --monitor"));
            }
        }
        b"job" => {
            if want == "focused" {
                return Err(ctx.die("wait job: --gone or nothing"));
            }
            if !plain {
                return Err(ctx.die("wait job: only --gone"));
            }
            if !jobw.is_empty() && !crate::shell::is_num(&jobw) {
                return Err(ctx.die(cat!(
                    "wait job: JOB is a job's pid (omabox jobs), got ",
                    jobw
                )));
            }
        }
        b"log" => {
            if !want.is_empty() || !plain {
                return Err(ctx.die("wait log: only -i (and --since OFFSET for a box log)"));
            }
            if let Some(g) = grep.take() {
                if logw.len() == 2 {
                    return Err(ctx.die("wait log: RE once: --grep RE or a word, not both"));
                }
                logw.push(g);
            }
            if logw.is_empty() {
                return Err(ctx.die(
                    "wait log: for which line? a regular expression (grep -E): wait log [JOB | LOG] RE",
                ));
            }
            if logw.len() == 2 && logw[0] == b"run" {
                logw.remove(0);
            }
            if logw.len() == 2 && !crate::shell::is_num(&logw[0]) {
                let names = crate::logs::LOG_NAMES;
                if !names.iter().any(|n| n.as_bytes() == logw[0].as_slice()) {
                    return Err(unknown_word(
                        ctx,
                        "wait log",
                        "log",
                        &logw[0],
                        "(JOB is a job's pid, omabox jobs; LOG a box log: shell, hyprland, apps, journal, ...: omabox help log)",
                        &names,
                    ));
                }
                boxlog = Some(logw.remove(0));
            }
            if since.is_some() && boxlog.is_none() {
                return Err(ctx.die(
                    "wait log: --since goes with a box log (wait log LOG RE --since OFFSET); a job's log is read from its first line",
                ));
            }
            if boxlog.as_deref() == Some(b"hyprland") && since.is_some() {
                return Err(ctx.die(
                    "wait log hyprland: no --since (its new lines come from its rolling log: Hyprland writes its file 8 KiB late)",
                ));
            }
            let re = logw.last().cloned().unwrap_or_default();
            let st = crate::sig::command("grep")
                .arg("-E")
                .arg("-e")
                .arg(std::ffi::OsStr::from_bytes(&re))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            if st.as_ref().map_or(true, |s| s.code() == Some(2)) {
                return Err(ctx.die(cat!(
                    "wait log: RE is a regular expression (grep -E), got ",
                    re
                )));
            }
        }
        b"tree" => {
            if text.as_deref().is_none_or(<[u8]>::is_empty) {
                return Err(ctx.die(
                    "wait tree: what text? as tree --find takes it: wait tree '\"Save\"', wait tree 'Dark mode' --window SEL",
                ));
            }
            if want == "focused" {
                return Err(ctx.die("wait tree: --gone or nothing"));
            }
            if !layer.is_empty() && !sels.is_empty() {
                return Err(ctx.die("wait tree: --window or --layer, not both"));
            }
            if !(none && !strict && masks.is_empty()) {
                return Err(ctx.die(
                    "wait tree: -g, --quiet, --strict and --ignore go with still and change",
                ));
            }
            if !shot.is_empty() && crate::jail::in_jail() {
                ctx.parsed();
                return Err(ctx.die(
                    "wait tree: --shot is not for an agent inside ai-jail yet: omabox shot after the wait",
                ));
            }
        }
        b"until" => {
            if cmd.is_empty() {
                return Err(ctx.die("wait until: nothing to run (omabox wait until -- CMD ...)"));
            }
            if !masks.is_empty() {
                return Err(ctx.die("wait until: --ignore goes with still and change"));
            }
        }
        b"for" => {
            if dur.is_empty() {
                return Err(ctx.die("wait for: how long? a duration: 500ms, 2s, 1m"));
            }
            if tmo_set {
                return Err(ctx.die("wait for: DURATION is the wait (no --timeout)"));
            }
            if !(plain && want.is_empty()) {
                return Err(ctx.die("wait for: only DURATION (and --json)"));
            }
        }
        b"" => {
            return Err(ctx.die(
                "wait: for what? still, change, window SEL, layer NAMESPACE, job [JOB], log [JOB | LOG] RE, tree TEXT, until -- CMD or for DURATION (omabox help)",
            ));
        }
        _ => {
            return Err(unknown_word(
                ctx,
                "wait",
                "condition",
                &cond,
                "(still, change, window SEL, layer NAMESPACE, job [JOB], log [JOB | LOG] RE, tree TEXT, until -- CMD or for DURATION)",
                &[
                    "still", "change", "window", "layer", "job", "log", "tree", "until", "for",
                ],
            ));
        }
    }
    if cond == b"change" && !quiet.is_empty() {
        return Err(ctx.die("wait change: no --quiet (that is still's)"));
    }
    if !base.is_empty() {
        match (cond.as_slice(), base_opt) {
            (b"change", "--since") | (b"still", "--like") => {}
            (b"still", _) => {
                return Err(ctx.die(
                    "wait still: --since goes with change (still takes --like SHOT: back as it was)",
                ));
            }
            (b"change", _) => {
                return Err(ctx.die(
                    "wait change: --like goes with still (change takes --since SHOT: changed since it)",
                ));
            }
            _ => {
                return Err(ctx.die(cat!(
                    "wait ",
                    cond,
                    ": ",
                    base_opt,
                    " goes with change (--since) or still (--like)"
                )));
            }
        }
        if !sels.is_empty() {
            return Err(ctx.die(cat!(
                "wait: ",
                base_opt,
                " SHOT says what is watched: no --window (a shot of a window watches that window)"
            )));
        }
        if !quiet.is_empty() {
            return Err(ctx.die(
                "wait still --like: no --quiet (it is satisfied once the screen looks like SHOT)",
            ));
        }
        if in_jail() {
            return Err(ctx.die(cat!(
                "wait ",
                base_opt,
                ": not for a jailed agent yet (its paths are never opened here)"
            )));
        }
    }
    let for_ms = if cond == b"for" {
        let Some(ms) = ms_duration(&dur) else {
            return Err(ctx.die(cat!(
                "wait for: DURATION is a duration: 500ms, 2s, 1m (got ",
                dur,
                ")"
            )));
        };
        if ms > WAIT_MAX_MS {
            return Err(ctx.die(cat!("wait for: at most 10m (got ", dur, ")")));
        }
        ms
    } else {
        0
    };
    select_box(ctx, &name)?;
    // (a fixed time needs no box: the one selected counts it as use when it is up)
    if cond == b"for" {
        return for_wait(ctx, for_ms, json);
    }
    need_box(ctx)?;
    shell_note(ctx)?;
    if let Some(log) = boxlog {
        let w = LogWait {
            log,
            re: logw.pop().unwrap_or_default(),
            icase,
            since,
        };
        return log_wait(ctx, &w, tmo, json);
    }
    match cond.as_slice() {
        b"window" => {
            let w = cat!("window ", joined(&sels));
            let (yes, no) = match want {
                "gone" => (cat!(w, " gone"), cat!(w, " still there")),
                "focused" => (cat!(w, " focused"), cat!(w, " not focused")),
                _ => (w.clone(), cat!("no ", w)),
            };
            let probe = Probe::Win(if want.is_empty() { "present" } else { want }, &sels);
            poll_until(ctx, &yes, &no, 2, tmo, json, probe)
        }
        b"layer" => {
            let mut l = cat!("layer ", ns);
            if !mon.is_empty() {
                l = cat!(l, " on ", mon);
            }
            let (yes, no) = if want == "gone" {
                (cat!(l, " gone"), cat!(l, " still there"))
            } else {
                (l.clone(), cat!("no ", l))
            };
            let probe = Probe::Layer(if want == "gone" { "gone" } else { "present" }, &ns, &mon);
            poll_until(ctx, &yes, &no, 2, tmo, json, probe)
        }
        b"job" | b"log" => {
            let pid = if cond == b"job" {
                jobw.clone()
            } else if logw.len() == 2 {
                logw[0].clone()
            } else {
                Vec::new()
            };
            let job = if pid.is_empty() {
                job_latest(ctx)
            } else {
                job_find(ctx, &pid)
            };
            let Some(job) = job else {
                return Err(ctx.die(if pid.is_empty() {
                    cat!(
                        "wait ",
                        cond,
                        ": box '",
                        ctx.name,
                        "' has no run -d job yet"
                    )
                } else {
                    cat!(
                        "wait ",
                        cond,
                        ": box '",
                        ctx.name,
                        "' has no run -d job of pid ",
                        pid,
                        " (omabox -b ",
                        ctx.name,
                        " jobs)"
                    )
                }));
            };
            let j = cat!("job ", job.pid, " (", job.words(), ")");
            if cond == b"job" {
                let gone = want == "gone";
                let (yes, no) = if gone {
                    (cat!(j, " gone"), cat!(j, " still running"))
                } else {
                    (cat!(j, " ended"), cat!(j, " still running"))
                };
                poll_until(ctx, &yes, &no, 1, tmo, json, Probe::Job(&job, gone))
            } else {
                let re = logw.last().cloned().unwrap_or_default();
                let (yes, no) = (
                    cat!("a line matching ", re, " in the log of ", j),
                    cat!("no line matching ", re, " in the log of ", j),
                );
                poll_until(ctx, &yes, &no, 1, tmo, json, Probe::Log(&job, &re, icase))
            }
        }
        b"tree" => {
            let text = text.unwrap_or_default();
            let place = if !layer.is_empty() {
                cat!(" on layer ", layer)
            } else if !sels.is_empty() {
                cat!(" in window ", joined(&sels))
            } else {
                Vec::new()
            };
            let t = cat!("tree ", crate::settle::quoted(&text), place);
            let gone = want == "gone";
            let (yes, no) = if gone {
                (cat!(t, " gone"), cat!(t, " still there"))
            } else {
                (t.clone(), cat!("no ", t))
            };
            let shot = if shot.is_empty() {
                Vec::new()
            } else {
                vec![b"-b".to_vec(), ctx.name.clone(), b"-o".to_vec(), shot]
            };
            // (gone: two reads in a row without it, as a window's; there: the first read with it,
            // for a state that lasts a moment)
            let need = if gone { 2 } else { 1 };
            let p = crate::treewait::TreeProbe {
                want: crate::treewait::TreeWant { text, sels, layer },
                gone,
                shot,
                need,
                streak: std::cell::Cell::new(0),
            };
            poll_until(ctx, &yes, &no, need, tmo, json, Probe::Tree(&p))
        }
        b"until" => {
            if let Some(n) = host_path_note(ctx, &cmd) {
                say(n);
            }
            let r = poll_until(
                ctx,
                b"command succeeded",
                b"command still failing",
                1,
                tmo,
                json,
                Probe::Until(&cmd),
            );
            // (the deciding try's own words, when it did not succeed: #400, they were hidden)
            let err = std::fs::read(ctx.at(UNTIL_ERR)).unwrap_or_default();
            let _ = std::fs::remove_file(ctx.at(UNTIL_ERR));
            let tail = last_lines(&err, 3);
            if r.is_err() && !tail.is_empty() {
                say("wait until: the command's last words (stderr):");
                let mut e = Vec::new();
                for l in tail {
                    e.extend_from_slice(&cat!("  ", l, "\n"));
                }
                let _ = std::io::Write::write_all(&mut std::io::stderr(), &e);
            }
            r
        }
        _ if !base.is_empty() => {
            let mut b = Base::new(ctx, base_opt, &base, &geom, &masks, strict)?;
            if base_opt == "--like" {
                return b.like(ctx, tmo, json);
            }
            let (g, w) = b.watched();
            screen(
                ctx,
                &mut s,
                &cond,
                tmo,
                &quiet,
                strict,
                &g,
                &w,
                masks,
                json,
                Some(&mut b),
            )
        }
        _ => screen(
            ctx, &mut s, &cond, tmo, &quiet, strict, &geom, &sels, masks, json, None,
        ),
    }
}

/// wait still|change: omabox-still over the screen, a region of it (-g) or a window's place.
#[allow(clippy::too_many_arguments)]
fn screen(
    ctx: &mut Ctx,
    s: &mut Settle,
    cond: &[u8],
    tmo: i64,
    quiet: &[u8],
    strict: bool,
    geom: &[u8],
    sels: &[Vec<u8>],
    mut masks: Vec<Vec<u8>>,
    json: bool,
    base: Option<&mut Base>,
) -> Res<()> {
    let mut args: Vec<Vec<u8>> = vec![b"--timeout".to_vec(), tmo.to_string().into_bytes()];
    if !quiet.is_empty() {
        args.extend([b"--quiet".to_vec(), quiet.to_vec()]);
    }
    if strict {
        args.push(b"--strict".to_vec());
    }
    let (mut region, mut w) = (Vec::new(), Vec::new());
    if !geom.is_empty() {
        let Some(g) = geom_parse(geom) else {
            return Err(ctx.die(cat!("wait: -g is \"X,Y WxH\" (or X,Y,W,H), got ", geom)));
        };
        region = geom_csv(&g);
    } else if !sels.is_empty() {
        // Its place on the screen: what is drawn there is what is watched.
        let wins = box_windows(ctx, None)?;
        w = win_select(ctx, &wins, sels)?;
        if jq_r(".onscreen", &w) != b"true" {
            return Err(ctx.die(cat!(
                "wait: window ",
                win_desc(&w),
                " is not on screen (workspace ",
                jq_r(".workspace", &w),
                "): ",
                cond,
                " watches its place on the screen"
            )));
        }
        region = jq_r(crate::jqprog::WIN_REGION_JQ, &w);
    }
    if !region.is_empty() {
        args.extend([b"--region".to_vec(), region.clone()]);
    }
    // --ignore: the screen's coordinates, or with --window the window's own (F8, #287): moved by its
    // place. One that misses the region watched leaves nothing out, said (#162).
    let (mut ox, mut oy) = (Vec::new(), Vec::new());
    let mut place: Vec<u8> = b"the region watched".to_vec();
    if !sels.is_empty() {
        let f: Vec<Vec<u8>> = {
            let line = region.split(|&c| c == b'\n').next().unwrap_or_default();
            let mut it = line.splitn(4, |&c| c == b',');
            (0..4)
                .map(|_| it.next().unwrap_or_default().to_vec())
                .collect()
        };
        (ox, oy) = (f[0].clone(), f[1].clone());
        s.origin = Some((ox.clone(), oy.clone()));
        place = cat!(
            "window ",
            win_desc(&w),
            " (",
            f[2],
            "x",
            f[3],
            ", in its own coordinates)"
        );
    }
    let mut k = 1;
    while k < masks.len() {
        if !ox.is_empty() {
            let line = masks[k].clone();
            let mut it = line.splitn(4, |&c| c == b',');
            let mut f = || it.next().unwrap_or_default().to_vec();
            let (x, y, mw, mh) = (f(), f(), f(), f());
            let add = |a: &[u8], b: &[u8]| -> Option<i64> { Some(dec(a)?.saturating_add(dec(b)?)) };
            let (Some(nx), Some(ny)) = (add(&x, &ox), add(&y, &oy)) else {
                return Err(num_fail(ctx, &cat!(line, " (at ", ox, ",", oy, ")")));
            };
            masks[k] = cat!(nx.to_string(), ",", ny.to_string(), ",", mw, ",", mh);
        }
        if !region.is_empty() {
            match rects_meet(&masks[k], &region) {
                None => return Err(num_fail(ctx, &cat!(masks[k], " and ", region))),
                Some(true) => {}
                Some(false) => {
                    let (zx, zy): (&[u8], &[u8]) = if ox.is_empty() {
                        (b"0", b"0")
                    } else {
                        (&ox, &oy)
                    };
                    say(cat!(
                        "wait: --ignore \"",
                        xywh_less(&masks[k], zx, zy),
                        "\" is outside ",
                        place,
                        ": it leaves nothing out"
                    ));
                }
            }
        }
        k += 2;
    }
    args.extend(masks);
    let p = cursor_now(ctx)?;
    if !p.is_empty() {
        let Some(r) = cursor_rect(&p) else {
            return Err(num_fail(ctx, &cat!("the pointer's place ", p)));
        };
        args.extend([b"--ignore".to_vec(), r]);
    }
    let c = String::from_utf8_lossy(cond).into_owned();
    let mut tool = vec![cond.to_vec()];
    tool.extend(args);
    s.begin(ctx, &tool)?;
    // --since SHOT: the screen against it once the tool has its first frame, so a change before
    // that is the shot's difference and one after it the tool's change (#344).
    if let Some(b) = base {
        if s.is_ready() {
            let d = b.compare(ctx);
            if !matches!(d, Ok(Ok(_))) {
                s.stop();
            }
            if let Err(w) = d? {
                let m = cat!(
                    "changed since ",
                    b.r.key,
                    w.since(),
                    "; seen after ",
                    ms_s((mono_ms() - b.t0).to_string())
                );
                return b.say("change", "satisfied", &m, w.csv.as_deref(), json);
            }
        }
    }
    let rc = s.end(ctx, &c, json, b"")?;
    crate::cmd::hyprctl::code(rc)
}

/// How a new frame differs from a shot: where (in words: "at X,Y WxH", and as X,Y,W,H on the
/// screen), or a window of that shot now another size.
struct Differs {
    words: Vec<u8>,
    csv: Option<Vec<u8>>,
}

impl Differs {
    /// After "changed since SHOT".
    fn since(&self) -> Vec<u8> {
        if self.csv.is_some() {
            cat!(" at ", self.words)
        } else {
            cat!(": ", self.words)
        }
    }

    /// In "not like SHOT after T (...)".
    fn like(&self) -> Vec<u8> {
        if self.csv.is_some() {
            cat!("it differs at ", self.words)
        } else {
            self.words.clone()
        }
    }
}

/// wait change --since SHOT and wait still --like SHOT (#344, finding 593): the screen (a region of
/// it, a window) against an earlier shot's image, through shotcmp's grab and differ.
struct Base {
    r: ShotRef,
    label: String,
    /// What the compare leaves out, in the shot's coordinates: -g's outside, --ignore's, the
    /// pointer where it was at the shot and where it is now (a screen shot draws it).
    masks: Vec<[i64; 4]>,
    /// A screen shot's top left on the screen; 0,0 for a window's.
    org: (i64, i64),
    /// What is watched on the screen: -g cut to the shot, else all of it; None for a window's
    /// (the window, wherever it is).
    region: Option<[i64; 4]>,
    strict: bool,
    t0: i64,
}

/// "X,Y,W,H" (a --mask, a pointer's rect) as numbers.
fn rect4(v: &[u8]) -> Option<[i64; 4]> {
    let f = crate::shell::read_ifs(v, 4, b",");
    Some([dec(&f[0])?, dec(&f[1])?, dec(&f[2])?, dec(&f[3])?])
}

impl Base {
    /// Base OPT FILE GEOM MASKS STRICT: the shot read (shotcmp::load), what is watched and what the
    /// compare leaves out.
    fn new(
        ctx: &Ctx,
        opt: &str,
        file: &[u8],
        geom: &[u8],
        masks: &[Vec<u8>],
        strict: bool,
    ) -> Res<Base> {
        let t0 = mono_ms();
        let label = format!("wait {opt}");
        let r = crate::shotcmp::load(ctx, file, &label)?;
        let Some([lw, lh, _, _]) = r.sizes() else {
            return Err(ctx.die(cat!(
                label,
                ": ",
                r.key,
                ": its record in shots.tsv is not one omabox reads"
            )));
        };
        let mut ign = Vec::new();
        for m in masks.chunks(2) {
            if let [k, v] = m {
                if k == b"--mask" {
                    ign.push(rect4(v).ok_or_else(|| num_fail(ctx, v))?);
                }
            }
        }
        let mut m = Vec::new();
        let (org, region) = if let Some((ox, oy)) = r.screen_at() {
            let mut reg = [ox, oy, lw, lh];
            if !geom.is_empty() {
                let g = geom_parse(geom).and_then(|g| rect4(&geom_csv(&g)));
                let Some([gx, gy, gw, gh]) = g else {
                    return Err(ctx.die(cat!("wait: -g is \"X,Y WxH\" (or X,Y,W,H), got ", geom)));
                };
                let (x0, y0) = (gx.max(ox), gy.max(oy));
                let (x1, y1) = (
                    gx.saturating_add(gw).min(ox + lw),
                    gy.saturating_add(gh).min(oy + lh),
                );
                if x1 <= x0 || y1 <= y0 {
                    return Err(ctx.die(cat!(
                        label,
                        ": -g ",
                        geom,
                        " has nothing inside ",
                        r.key,
                        " (",
                        format!("{ox},{oy} {lw}x{lh}"),
                        " of the screen)"
                    )));
                }
                reg = [x0, y0, x1 - x0, y1 - y0];
                m.extend(crate::shotcmp::outside(
                    [x0 - ox, y0 - oy, x1 - x0, y1 - y0],
                    lw,
                    lh,
                ));
            }
            let mut ptrs = Vec::new();
            if let Some(p) = crate::shotcmp::pointer_then(ctx, &r) {
                ptrs.push(p);
            }
            let p = cursor_now(ctx)?;
            if !p.is_empty() {
                ptrs.push(p);
            }
            for p in ptrs {
                if let Some(c) = cursor_rect(&p).and_then(|c| rect4(&c)) {
                    ign.push(c);
                }
            }
            for [x, y, w, h] in ign {
                m.push([x - ox, y - oy, w, h]);
            }
            ((ox, oy), Some(reg))
        } else {
            if r.window().is_some_and(|(_, c)| !c.is_empty()) {
                return Err(ctx.die(cat!(
                    label,
                    ": ",
                    r.key,
                    " is a crop of a window (-g): it takes a whole shot of a window, or any shot of the screen"
                )));
            }
            if !geom.is_empty() {
                return Err(ctx.die(cat!(
                    label,
                    ": -g goes with a shot of the screen; ",
                    r.key,
                    " is a window's (--ignore is in its coordinates)"
                )));
            }
            m = ign;
            ((0, 0), None)
        };
        Ok(Base {
            r,
            label,
            masks: m,
            org,
            region,
            strict,
            t0,
        })
    }

    /// What screen() watches: -g's region, or the shot's window (by its address).
    fn watched(&self) -> (Vec<u8>, Vec<Vec<u8>>) {
        match (self.region, self.r.window()) {
            (Some([x, y, w, h]), _) => (format!("{x},{y},{w},{h}").into_bytes(), Vec::new()),
            (None, Some((a, _))) => (Vec::new(), vec![a]),
            (None, None) => (Vec::new(), Vec::new()),
        }
    }

    /// One new frame against the shot: Ok (a caret alone's note, or nothing) when it looks the
    /// same, else how it differs.
    fn compare(&self, ctx: &Ctx) -> Res<Result<Vec<u8>, Differs>> {
        let f = match crate::shotcmp::grab(ctx, &self.label, &self.r)? {
            Ok(f) => f,
            Err(resized) => {
                return Ok(Err(Differs {
                    words: resized,
                    csv: None,
                }));
            }
        };
        let _rm = crate::shell::Rm(f.clone());
        Ok(
            match crate::shotcmp::differ(ctx, &self.label, &self.r, &f, &self.masks, self.strict)? {
                Diff::Same(note) => Ok(note),
                Diff::At([x, y, w, h]) => {
                    let (sx, sy) = (x + self.org.0, y + self.org.1);
                    let mut words = format!("{sx},{sy} {w}x{h}").into_bytes();
                    if self.region.is_none() {
                        words.extend(b" of the window, in its coordinates");
                    }
                    Err(Differs {
                        words,
                        csv: Some(format!("{sx},{sy},{w},{h}").into_bytes()),
                    })
                }
            },
        )
    }

    /// wait still --like: a new frame every 200 ms until one looks like the shot (0), or --timeout
    /// (124, saying the last difference).
    fn like(&mut self, ctx: &Ctx, tmo: i64, json: bool) -> Res<()> {
        let mut used = std::time::Instant::now();
        loop {
            let c = self.compare(ctx)?;
            let now = ms_s((mono_ms() - self.t0).to_string());
            match c {
                Ok(note) => {
                    let n = if note.is_empty() {
                        Vec::new()
                    } else {
                        cat!("; ", note)
                    };
                    let m = cat!("like ", self.r.key, " after ", now, n);
                    return self.say("still", "satisfied", &m, None, json);
                }
                Err(d) if mono_ms() - self.t0 >= tmo => {
                    let m = cat!("not like ", self.r.key, " after ", now, " (", d.like(), ")");
                    return self.say("still", "unsatisfied", &m, d.csv.as_deref(), json);
                }
                Err(_) => {}
            }
            if used.elapsed() >= Duration::from_secs(30) {
                touch(&ctx.at("used"));
                used = std::time::Instant::now();
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// The answer, as settle_end has it (a line, or its --json: CHANGE the difference, X,Y,W,H):
    /// 0 satisfied, 124 unsatisfied.
    fn say(&self, cond: &str, res: &str, msg: &[u8], change: Option<&[u8]>, json: bool) -> Res<()> {
        let t = (mono_ms() - self.t0).to_string();
        if json {
            let r = Args::new(&["-nc"])
                .kv("--arg", "res", res)
                .kv("--arg", "cond", cond)
                .kv("--arg", "msg", msg)
                .kv("--arg", "t", &t)
                .kv("--arg", "first", "-")
                .kv("--arg", "last", "-")
                .kv("--arg", "ch", change.unwrap_or(b"-"))
                .kv("--arg", "ig", "-")
                .kv("--arg", "why", "-")
                .kv("--arg", "frames", "")
                .kv("--arg", "late", "-")
                .kv("--arg", "facts", "")
                .kv("--argjson", "changes", "null")
                .kv("--argjson", "target", "null")
                .kv("--argjson", "evs", "null")
                .kv("--arg", "exp", "")
                .arg(crate::jqprog::SETTLE_JSON_JQ)
                .run(None, jq::Err::Inherit);
            out(&r.out)?;
        } else {
            out(cat!(res, ": ", msg, "\n"))?;
        }
        if res == "satisfied" {
            Ok(())
        } else {
            Err(Exit(124))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // #316: wait layer names the monitor each drawn layer is on, and --monitor waits on one.
    #[test]
    fn layer_answers() {
        let l = br#"{"HEADLESS-2": {"levels": {"2": [{"namespace": "bar", "x": 0, "y": 0, "w": 1920, "h": 34},
              {"namespace": "menu", "x": 0, "y": 0, "w": 1, "h": 1}],
            "3": [{"namespace": "panel", "x": 0, "y": 0, "w": 1920, "h": 1080}]}},
          "HEADLESS-10": {"levels": {"2": [{"namespace": "bar", "x": 1920, "y": 0, "w": 1280, "h": 34}],
            "3": [{"namespace": "menu", "x": 1920, "y": 0, "w": 1, "h": 1}]}}}"#;
        let a = |want: &str, ns: &str, mon: &str| {
            String::from_utf8(layer_answer(want, l, ns.as_bytes(), mon.as_bytes())).unwrap()
        };
        assert_eq!(
            a("present", "panel", ""),
            "yes at 0,0 1920x1080 on HEADLESS-2"
        );
        assert_eq!(
            a("present", "bar", ""),
            "yes 2 layers: at 0,0 1920x34 on HEADLESS-2; at 1920,0 1280x34 on HEADLESS-10",
            "one on each monitor, in the box's order"
        );
        assert_eq!(
            a("present", "bar", "HEADLESS-10"),
            "yes at 1920,0 1280x34 on HEADLESS-10"
        );
        assert_eq!(
            a("present", "panel", "HEADLESS-10"),
            "no at 0,0 1920x1080 on HEADLESS-2",
            "not on that monitor: where it is instead"
        );
        assert_eq!(
            a("present", "menu", ""),
            "no",
            "1x1 is a closed menu (finding 146)"
        );
        assert_eq!(
            a("present", "bar", "DP-1"),
            "no no monitor DP-1 in the box (HEADLESS-2, HEADLESS-10)"
        );
        assert_eq!(a("gone", "menu", ""), "yes");
        assert_eq!(
            a("gone", "panel", "HEADLESS-10"),
            "yes",
            "gone from that monitor"
        );
        assert_eq!(a("gone", "panel", ""), "no at 0,0 1920x1080 on HEADLESS-2");
        assert_eq!(
            a("gone", "bar", "DP-1"),
            "yes no monitor DP-1 in the box (HEADLESS-2, HEADLESS-10)",
            "said, not silently gone"
        );
        assert_eq!(
            String::from_utf8(layer_answer("present", b"not json", b"bar", b"")).unwrap(),
            "no"
        );
    }

    // #400: the deciding try's last lines; a command naming a path the box cannot see.
    #[test]
    fn until_words() {
        assert_eq!(
            last_lines(b"a\n\n  \nb\nc\nd\n", 3),
            vec![b"b".to_vec(), b"c".to_vec(), b"d".to_vec()]
        );
        assert!(last_lines(b"\n \n", 3).is_empty());
        let w = |v: &[&str]| -> Vec<Vec<u8>> { v.iter().map(|s| s.as_bytes().to_vec()).collect() };
        let note = |tmp: Option<&str>, v: &[&str]| {
            host_path_note_with(
                b"/run/u/omabox/b",
                tmp.map(|t| t.as_bytes().to_vec()),
                &w(v),
            )
            .map(|n| String::from_utf8(n).unwrap())
        };
        assert!(
            note(
                None,
                &["sh", "-c", "! /run/u/omabox/b/home/left.sh | grep -q X"]
            )
            .is_some_and(|n| n.contains("in the box it is /home/sbx/")),
            "its HOME, inside a sh -c string"
        );
        assert!(
            note(None, &["test", "-e", "/run/u/omabox/b/run/x"])
                .is_some_and(|n| n.contains("this box's folder on the host"))
        );
        assert!(
            note(Some("/h/.cache/tmp/"), &["/h/.cache/tmp/left.sh"])
                .is_some_and(|n| n.contains("your $TMPDIR on the host"))
        );
        // (the box's own /tmp, a sibling of the box's folder, a TMPDIR of /tmp: nothing said)
        assert_eq!(note(Some("/tmp"), &["test", "-e", "/tmp/ready"]), None);
        assert_eq!(note(None, &["test", "-e", "/run/u/omabox/bb/x"]), None);
        assert_eq!(note(Some("/h/t"), &["/h/tt/x"]), None);
    }
}
