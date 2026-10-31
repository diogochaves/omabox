//! `run -d` jobs, as far as need_box says their early ends (job_note).

use crate::boxes::{BoxPid, at, box_pid, run_read};
use crate::cat;
use crate::exit::{Ctx, say};
use crate::json;
use crate::procfs;
use crate::shell::{self, Re, re_group};
use serde_json::Value;
use std::io::Write;

/// A `run -d` job that ended within its first JOB_EARLY_MS, said by the box's next command, once
/// (#286, finding 399): run -d's own watch (0.3 s) misses a server that dies a second later. Each
/// job's waiter (share/run-job.sh) writes its status as it ends to omabox.run-NS.exit in the box's
/// runtime dir ($D/run here), NS its start (its log is run-NS.log). $D/jobs-noted (host-only) names
/// those dealt with: said here, said by run -d itself, stopped by --replace, ended with status 0 (a
/// one-shot finished; no death), or later than that. A box an older omabox started has no such
/// file: what is there by then is taken as dealt with. Said whatever -q.
const JOB_EARLY_MS: i64 = 30000;
/// A job killed while an `omabox run` ran in the box, or this long after its end, was stopped by
/// the agent itself (#300, finding 406).
const JOB_RUN_GRACE_MS: i64 = 2000;

pub fn job_noted(ctx: &Ctx, names: &[Vec<u8>]) {
    if names.is_empty() || !shell::path(&ctx.d).is_dir() {
        return;
    }
    let lines: Vec<u8> = names.iter().flat_map(|n| cat!(n, "\n")).collect();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(ctx.at("jobs-noted"))
    {
        let _ = f.write_all(&lines);
    }
}

/// The lines `while IFS= read -r line` reads: a last one without its newline is not one.
pub fn read_lines(v: &[u8]) -> Vec<&[u8]> {
    let mut l: Vec<&[u8]> = v.split(|&c| c == b'\n').collect();
    l.pop();
    l
}

pub fn now_us() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_micros()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// `stat -c %.3Y`, as ms: the mtime, truncated to the ms.
fn mtime_ms(p: &std::path::Path) -> Option<i64> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(p).ok()?;
    (m.mtime() >= 0).then(|| m.mtime() * 1000 + m.mtime_nsec() / 1_000_000)
}

pub fn job_note(ctx: &Ctx) {
    job_note_with(ctx, |f| !job_live(ctx, &[f.to_path_buf()]).is_empty());
}

/// job_note, a record's job running as LIVE says (job_live).
fn job_note_with(ctx: &Ctx, live: impl Fn(&std::path::Path) -> bool) {
    let run = ctx.at("run");
    let ends: Vec<Vec<u8>> = shell::glob(&run, false)
        .into_iter()
        .filter(|n| n.starts_with(b"omabox.run-") && n.ends_with(b".exit"))
        .collect();
    if ends.is_empty() {
        return;
    }
    let noted = std::fs::read(ctx.at("jobs-noted"));
    let old = !ctx.at("jobs-noted").is_file();
    let seen: Vec<&[u8]> = match (&noted, old) {
        (Ok(v), false) => read_lines(v),
        _ => Vec::new(),
    };
    let fresh: Vec<Vec<u8>> = ends
        .into_iter()
        .filter(|n| !seen.contains(&n.as_slice()))
        .collect();
    if fresh.is_empty() {
        return;
    }
    if old {
        job_noted(ctx, &fresh);
        return;
    }
    let now = now_us();
    let newest = shell::glob(&ctx.at("home"), false)
        .into_iter()
        .rev()
        .find(|l| shell::re_match(r"^run-[0-9]+\.log$", l));
    let re_exit = Re::new(r"^omabox\.run-([0-9]{1,19})\.exit$");
    for n in fresh {
        let Some(ns) = re_exit.as_ref().and_then(|r| r.group(&n, 1)) else {
            job_noted(ctx, &[n]);
            continue;
        };
        // Its end: the status file's time (a link or anything but a file is not the waiter's).
        let f = at(&ctx.d, &format!("run/{}", String::from_utf8_lossy(&n)));
        let end = if f.is_symlink() || !f.is_file() {
            None
        } else {
            mtime_ms(&f)
        };
        let Some(end) = end else {
            job_noted(ctx, &[n]);
            continue;
        };
        let ms = end.saturating_sub(shell::dec(&ns).unwrap_or(0) / 1_000_000);
        if !(0..=JOB_EARLY_MS).contains(&ms) {
            job_noted(ctx, &[n]);
            continue;
        }
        let name = String::from_utf8_lossy(&n).into_owned();
        let mut st = run_read(&ctx.d, &name).unwrap_or_default();
        st.truncate(64);
        let st = shell::strip_nl(&st);
        let st = if shell::is_num(&st) { st } else { Vec::new() };
        // Made, not yet written (the waiter's `echo $? >`): the next command reads it; one still
        // empty 2 s on was not kept.
        if st.is_empty() && now / 1000 - end < 2000 {
            continue;
        }
        job_noted(ctx, std::slice::from_ref(&n));
        if st == b"0" {
            continue;
        }
        // Stopped by the agent itself (#300, finding 406): killed while an `omabox run` ran in the
        // box (`omabox run -- pkill APP`), or just after. An exit or a crash is still said, but not
        // during a run that kills (#329: foot traps SIGTERM and exits 230, its own code).
        match job_run_stopped(ctx, end) {
            Some(true) => continue,
            Some(false) if matches!(st.as_slice(), b"130" | b"137" | b"143") => continue,
            _ => {}
        }
        // What it was: its record ($D/jobs, the host's), kept until now (finding 405). Found by its
        // log's name, each record a line read on its own. A leader that exited leaving its session
        // running (a launcher) is no end.
        let mut words = Vec::new();
        let mut pid = Vec::new();
        if let Some(rec) = record_by_log(ctx, &cat!("/run-", ns, ".log")) {
            let (file, rest) = split_tab(&rec);
            let (p, w) = split_tab(rest);
            if live(&shell::path(file)) {
                continue;
            }
            pid = p.to_vec();
            words = w.to_vec();
            let text = String::from_utf8_lossy(&words).into_owned();
            if text.chars().count() > 60 {
                words = format!("{}…", text.chars().take(59).collect::<String>()).into_bytes();
            }
        }
        let log = cat!("run-", ns, ".log");
        let wher = if newest.as_deref() == Some(log.as_slice()) {
            cat!("omabox log -b ", ctx.name, " run")
        } else {
            cat!("log: ", ctx.d, "/home/", log)
        };
        let words = if words.is_empty() {
            b"(its words were not kept)".to_vec()
        } else {
            words
        };
        let pid = if pid.is_empty() {
            Vec::new()
        } else {
            cat!(" (pid ", pid, ")")
        };
        say(cat!(
            "warning: run -d job ",
            words,
            pid,
            " ",
            job_how(&st),
            format!(" after {}.{}s", ms / 1000, ms % 1000 / 100),
            ", since the last command (",
            if wher.starts_with(b"log: ") {
                Vec::new()
            } else {
                b"log: ".to_vec()
            },
            wher,
            ")"
        ));
    }
}

/// `${rec%%$'\t'*}` and `${rec#*$'\t'}`.
pub fn split_tab(v: &[u8]) -> (&[u8], &[u8]) {
    match v.iter().position(|&c| c == b'\t') {
        Some(i) => (&v[..i], &v[i + 1..]),
        None => (v, v),
    }
}

/// The first record in $D/jobs (a JSON object a line) whose log ends with LOG, as
/// "FILE<TAB>PID<TAB>ARGV" (jq's `\(.pid)`, `.argv | join(" ")`), up to its first newline.
fn record_by_log(ctx: &Ctx, log: &[u8]) -> Option<Vec<u8>> {
    let dir = ctx.at("jobs");
    let files = shell::glob(&dir, false);
    if files.is_empty() {
        return None;
    }
    for name in files {
        let file = cat!(ctx.d, "/jobs/", name);
        let Ok(text) = std::fs::read(shell::path(&file)) else {
            continue;
        };
        let mut lines: Vec<&[u8]> = text.split(|&c| c == b'\n').collect();
        if lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        for line in lines {
            let (vals, ok) = json::values(line);
            let [v] = vals.as_slice() else { continue };
            if !ok {
                continue;
            }
            let Value::Object(m) = v else { continue };
            let Some(Value::String(l)) = m.get("log") else {
                continue;
            };
            if !l.as_bytes().ends_with(log) {
                continue;
            }
            let pid = match m.get("pid") {
                Some(Value::String(s)) => s.clone(),
                Some(x) => json::compact(x),
                None => "null".to_string(),
            };
            let Some(argv) = join_argv(m.get("argv")) else {
                continue;
            };
            let rec = cat!(file, "\t", pid, "\t", argv);
            let first = rec.split(|&c| c == b'\n').next().unwrap_or_default();
            return Some(first.to_vec());
        }
    }
    None
}

/// `.argv | join(" ")`, None where jq fails.
fn join_argv(v: Option<&Value>) -> Option<String> {
    let Value::Array(a) = v? else { return None };
    let parts: Option<Vec<String>> = a
        .iter()
        .map(|x| match x {
            Value::String(s) => Some(s.clone()),
            Value::Null => Some(String::new()),
            Value::Number(_) | Value::Bool(_) => Some(json::compact(x)),
            _ => None,
        })
        .collect();
    Some(parts?.join(" "))
}

/// How a job ended, in words, from its exit status.
pub fn job_how(st: &[u8]) -> String {
    let s = String::from_utf8_lossy(st);
    if st.is_empty() {
        return "exited (its exit status was not kept)".to_string();
    }
    if let Some(n) = shell::test_int(st).filter(|&n| n > 128) {
        if let Some(sig) = procfs::signame((n - 128).to_string().as_bytes()) {
            return format!("was killed by SIG{sig} ({s})");
        }
    }
    format!("exited with status {s}")
}

/// Whether a job that ended at END (ms) did so while an `omabox run` ran in the box, or within
/// JOB_RUN_GRACE_MS of one's end: Some(whether one of them was a run that kills), None when none
/// was. $D/runs (host-only) holds o.PID.START while one runs and c.START.END once it ended; ok. and
/// ck. for a run that kills (#329: kill_cmd).
fn job_run_stopped(ctx: &Ctx, end: i64) -> Option<bool> {
    let num = |v: &[u8]| shell::test_int(v);
    let mut any = None;
    for f in shell::glob(&ctx.at("runs"), false) {
        let c = |i| re_group(r"^c(k?)\.([0-9]+)\.([0-9]+)$", &f, i);
        let o = |i| re_group(r"^o(k?)\.([0-9]+)\.([0-9]+)$", &f, i);
        let hit = if let (Some(k), Some(a), Some(b)) = (c(1), c(2), c(3)) {
            (num(&a).is_some_and(|a| a <= end)
                && num(&b).is_some_and(|b| end <= b + JOB_RUN_GRACE_MS))
            .then_some(!k.is_empty())
        } else if let (Some(k), Some(p), Some(s)) = (o(1), o(2), o(3)) {
            (num(&s).is_some_and(|s| s <= end) && shell::path(&cat!("/proc/", p)).is_dir())
                .then_some(!k.is_empty())
        } else {
            None
        };
        if let Some(k) = hit {
            any = Some(any.unwrap_or(false) || k);
        }
    }
    any
}

/// Whether a command (a word of it, or of a `sh -c` script in it) runs kill, pkill or killall: a
/// job that ends during it was stopped by the agent, whatever its status (#329).
pub fn kill_cmd(cmd: &[Vec<u8>]) -> bool {
    cmd.iter().flat_map(|w| shell::words(w)).any(|t| {
        let b = t.rsplit(|&c| c == b'/').next().unwrap_or_default();
        matches!(b, b"kill" | b"pkill" | b"killall")
    })
}

/// The box's processes, as host pids: everything in its pid namespace.
pub fn box_pids(ctx: &Ctx) -> Vec<Vec<u8>> {
    box_pids_try(ctx).unwrap_or_default()
}

/// box_pids, None where bash's fails (the box's pid namespace cannot be read).
pub fn box_pids_try(ctx: &Ctx) -> Option<Vec<Vec<u8>>> {
    let pid = match box_pid(&ctx.d) {
        BoxPid::Pid(p) => p,
        _ => Vec::new(),
    };
    let ns = procfs::pidns(&pid)?;
    Some(
        shell::glob(std::path::Path::new("/proc"), false)
            .into_iter()
            .filter(|p| p.iter().all(u8::is_ascii_digit))
            .filter(|p| procfs::pidns(p).is_some_and(|n| n == ns))
            .collect(),
    )
}

/// The pid namespace of the box's PID 1, None when it cannot be read.
pub fn box_ns(ctx: &Ctx) -> Option<Vec<u8>> {
    match box_pid(&ctx.d) {
        BoxPid::Pid(p) => procfs::pidns(&p),
        _ => None,
    }
}

/// Host pid PID pinned by a pidfd while it is a process of the box whose pid namespace is NS
/// (box_ns): a signal through it never reaches a process of the user's that took the pid of one
/// of the box's that ended since it was listed (#377).
pub fn pin_in_box(ns: &[u8], pid: &[u8]) -> Option<crate::pidfd::Pidfd> {
    crate::pidfd::Pidfd::pin(pid, || procfs::pidns(pid).is_some_and(|n| n == ns))
}

/// box_pids_try, each process pinned (pin_in_box).
pub fn box_pins_try(ctx: &Ctx) -> Option<Vec<crate::pidfd::Pidfd>> {
    let ns = box_ns(ctx)?;
    let pids = box_pids_try(ctx)?;
    Some(pids.iter().filter_map(|p| pin_in_box(&ns, p)).collect())
}

/// Each live process of the box: "HOST_PID BOX_PID BOX_SID START".
pub fn box_procs(ctx: &Ctx) -> Vec<[Vec<u8>; 4]> {
    let mut out = Vec::new();
    for p in box_pids(ctx) {
        let Ok(s) = std::fs::read(shell::path(&cat!("/proc/", p, "/status"))) else {
            continue;
        };
        let state = s
            .split(|&c| c == b'\n')
            .filter(|l| l.starts_with(b"State:"))
            .filter_map(|l| shell::words(l).into_iter().nth(1))
            .next_back()
            .unwrap_or_default();
        let ids = cat!(
            state,
            " ",
            procfs::status_last(&s, "NSpid"),
            " ",
            procfs::status_last(&s, "NSsid")
        );
        let re = r"^[A-Za-z] ([0-9]+) ([0-9]+)$";
        let (Some(bp), Some(sid)) = (re_group(re, &ids, 1), re_group(re, &ids, 2)) else {
            continue;
        };
        if state == b"Z" {
            continue;
        }
        out.push([
            p.clone(),
            bp,
            sid,
            procfs::proc_start(&p).unwrap_or_default(),
        ]);
    }
    out
}

/// awk's `==`: as numbers when both look like numbers, else as strings.
fn awk_eq(a: &[u8], b: &[u8]) -> bool {
    let num = |v: &[u8]| {
        std::str::from_utf8(v)
            .ok()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|s| s.parse::<f64>().ok())
    };
    match (num(a), num(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// Of box_procs', job PID's, as (HOST_PID, BOX_PID): its session's, and its leader by its pid before
/// it has made that session (finding 405). START is its leader's start time, - when the leader had
/// exited before the job was recorded: a leader that started at another time (or at all, for -) has
/// the pid after the job ended, and the job has none.
pub fn job_procs(procs: &[[Vec<u8>; 4]], pid: &[u8], start: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut out = Vec::new();
    for [host, bp, sid, st] in procs {
        if awk_eq(sid, pid) || awk_eq(bp, pid) {
            if awk_eq(bp, pid) && !awk_eq(st, start) {
                return Vec::new();
            }
            out.push((host.clone(), bp.clone()));
        }
    }
    out
}

/// job_failed PID (#297, finding 404): run -d --wait's job has died, killed or with a status other
/// than 0 (its waiter's status file, read here in $D/run), nothing of its session left. A 0 is no
/// death: a one-shot changes the screen and ends with 0.
pub fn job_failed(ctx: &Ctx, pid: &[u8]) -> bool {
    if !shell::is_num(pid) {
        return false;
    }
    let f = ctx.at("jobs").join(shell::path(pid));
    if !f.is_file() {
        return false;
    }
    let (log, ok) = crate::json::field(&f, "log");
    if !ok {
        return false;
    }
    let base = log.rsplit(|&c| c == b'/').next().unwrap_or(&log).to_vec();
    let Some(ns) = shell::re_group(r"^run-([0-9]{1,19})\.log$", &base, 1) else {
        return false;
    };
    let st = run_read(
        &ctx.d,
        &format!("omabox.run-{}.exit", String::from_utf8_lossy(&ns)),
    )
    .map(|v| shell::subst(&v[..v.len().min(64)]))
    .unwrap_or_default();
    if !shell::is_num(&st) || st == b"0" {
        return false;
    }
    job_live(ctx, std::slice::from_ref(&f)).is_empty()
}

/// The processes of the jobs recorded in FILES. A record whose job has ended is removed once its end
/// has been dealt with (job_said): until then job_note needs it to name what ended (finding 405).
pub fn job_live(ctx: &Ctx, files: &[std::path::PathBuf]) -> Vec<(Vec<u8>, Vec<u8>)> {
    job_live_with(ctx, files, &box_procs(ctx))
}

/// job_live, the box's processes PROCS (box_procs).
fn job_live_with(
    ctx: &Ctx,
    files: &[std::path::PathBuf],
    procs: &[[Vec<u8>; 4]],
) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut out = Vec::new();
    for f in files {
        let fields = std::fs::read(f).ok().and_then(|t| record_fields(&t));
        let Some([pid, start, log]) = fields else {
            let _ = std::fs::remove_file(f);
            continue;
        };
        let p = job_procs(procs, &pid, &start);
        if !p.is_empty() {
            out.extend(p);
        } else if job_said(ctx, &log) {
            let _ = std::fs::remove_file(f);
        }
    }
    out
}

/// `{ read -r pid; read -r start; read -r log; } < <(jq -r '.pid, .start, .log' FILE)`.
fn record_fields(text: &[u8]) -> Option<[Vec<u8>; 3]> {
    let (vals, _) = json::values(text);
    let mut lines = Vec::new();
    for v in &vals {
        let Value::Object(m) = v else {
            if v.is_null() {
                lines.extend(["null"; 3].map(String::from));
                continue;
            }
            break;
        };
        for k in ["pid", "start", "log"] {
            lines.push(json::raw(m.get(k).unwrap_or(&Value::Null)));
        }
    }
    let all: Vec<u8> = lines.iter().flat_map(|l| cat!(l, "\n")).collect();
    let mut it = all.split(|&c| c == b'\n');
    let mut take = || it.next().map(ifs_trim);
    let (a, b, c) = (take()?, take()?, take()?);
    // The third read needs its line's newline: jq always ends with one; one missing is no line.
    (all.iter().filter(|&&c| c == b'\n').count() >= 3).then_some([a, b, c])
}

/// What `read` keeps of a line: IFS's blanks at either end gone.
fn ifs_trim(v: &[u8]) -> Vec<u8> {
    let blank = |c: &u8| matches!(c, b' ' | b'\t' | b'\n');
    let a = v.iter().position(|c| !blank(c)).unwrap_or(v.len());
    let b = v.iter().rposition(|c| !blank(c)).map_or(a, |i| i + 1);
    v[a..b].to_vec()
}

/// Whether the end of the job whose log is LOG has been dealt with ($D/jobs-noted): said, or not to
/// be said. A box an older omabox started (no jobs-noted) and a log not named as run -d names them:
/// yes.
pub fn job_said(ctx: &Ctx, log: &[u8]) -> bool {
    let noted = ctx.at("jobs-noted");
    let base = log.rsplit(|&c| c == b'/').next().unwrap_or(log);
    let Some(ns) = re_group(r"^run-([0-9]{1,19})\.log$", base, 1) else {
        return true;
    };
    if !noted.is_file() {
        return true;
    }
    let want = cat!("omabox.run-", ns, ".exit");
    std::fs::read(noted).is_ok_and(|t| t.split(|&c| c == b'\n').any(|l| l == want.as_slice()))
}

// --- run -d's own: recording a job, --replace, its first moments, run's marks --------------------------

/// job_exe ARGV0: a job's program, for --replace to name a job of it run with other arguments (#255):
/// a path resolved lexically against this directory; a bare name as it is.
pub fn job_exe(a0: &[u8]) -> Vec<u8> {
    if a0.contains(&b'/') {
        crate::repo::realpath_ms(a0).unwrap_or_else(|| a0.to_vec())
    } else {
        a0.to_vec()
    }
}

/// A word as `jq --arg` takes it: a byte that starts no UTF-8 character is U+FFFD.
fn jstr(v: &[u8]) -> Value {
    Value::String(String::from_utf8_lossy(v).into_owned())
}

/// job_record PID LOG ARGV...: after a launch, the record of job PID (in the box), with the start
/// time of the process with that pid (- when it is gone already: finding 405).
pub fn job_record(ctx: &Ctx, pid: &[u8], log: &[u8], argv: &[Vec<u8>]) {
    job_record_with(ctx, pid, log, argv, box_procs(ctx));
}

/// job_record, the box's processes PROCS (box_procs).
fn job_record_with(ctx: &Ctx, pid: &[u8], log: &[u8], argv: &[Vec<u8>], procs: Vec<[Vec<u8>; 4]>) {
    let mut start = Vec::new();
    for [_, bp, _, st] in procs {
        if awk_eq(&bp, pid) {
            start = st;
        }
    }
    let start = if start.is_empty() {
        b"-".to_vec()
    } else {
        start
    };
    let dir = ctx.at("jobs");
    let _ = std::fs::create_dir_all(&dir);
    let mut m = serde_json::Map::new();
    m.insert("pid".into(), jstr(pid));
    m.insert("start".into(), jstr(&start));
    m.insert("log".into(), jstr(log));
    m.insert(
        "exe".into(),
        jstr(&job_exe(argv.first().map_or(&b""[..], |a| a))),
    );
    m.insert(
        "argv".into(),
        Value::Array(argv.iter().map(|a| jstr(a)).collect()),
    );
    let text = cat!(json::compact(&Value::Object(m)), "\n");
    let _ = std::fs::write(
        dir.join(std::ffi::OsStr::new(
            &String::from_utf8_lossy(pid).into_owned(),
        )),
        text,
    );
}

/// A record file's one JSON value, as jq reads it (None for anything but one value).
fn record(f: &std::path::Path) -> Option<Value> {
    let text = std::fs::read(f).ok()?;
    let (vals, ok) = json::values(&text);
    match (vals.as_slice(), ok) {
        ([v], true) => Some(v.clone()),
        _ => None,
    }
}

/// `"\(.x)"`: a string as it is, anything else as jq writes it.
fn interp(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(x) => json::compact(x),
        None => "null".into(),
    }
}

/// The record files in $D/jobs, in glob order (`[ -f ]`: regular files, links followed).
fn job_files(ctx: &Ctx) -> Vec<std::path::PathBuf> {
    let dir = ctx.at("jobs");
    shell::glob(&dir, false)
        .into_iter()
        .map(|n| dir.join(shell::path(&n)))
        .filter(|f| f.is_file())
        .collect()
}

/// job_others ARGV...: the live `run -d` jobs of ARGV's program, as "ARGV0 (pid PID, ARGS)" by pid.
fn job_others(ctx: &Ctx, argv: &[Vec<u8>]) -> Vec<String> {
    let a0 = argv.first().cloned().unwrap_or_default();
    job_others_with(ctx, argv, &job_exe(&a0), |f| {
        !job_live(ctx, std::slice::from_ref(&f.to_path_buf())).is_empty()
    })
}

/// job_others, ARGV's program EXE (job_exe), a record's job running as LIVE says (job_live).
fn job_others_with(
    ctx: &Ctx,
    argv: &[Vec<u8>],
    exe: &[u8],
    live: impl Fn(&std::path::Path) -> bool,
) -> Vec<String> {
    let a0 = argv.first().cloned().unwrap_or_default();
    let exe = String::from_utf8_lossy(exe).into_owned();
    let a0 = String::from_utf8_lossy(&a0).into_owned();
    let mut out: Vec<(Vec<u8>, String)> = Vec::new();
    for f in job_files(ctx) {
        let Some(v) = record(&f) else { continue };
        let first = v
            .get("argv")
            .and_then(|a| a.get(0))
            .cloned()
            .unwrap_or(Value::Null);
        let mine = match v.get("exe") {
            Some(e) if !matches!(e, Value::Null | Value::Bool(false)) => {
                *e == Value::String(exe.clone())
            }
            _ => first == Value::String(a0.clone()) || first == Value::String(exe.clone()),
        };
        if !mine || !live(&f) {
            continue;
        }
        let args = match v.get("argv") {
            Some(Value::Array(a)) if a.len() > 1 => {
                let parts: Option<Vec<String>> = a[1..]
                    .iter()
                    .map(|x| match x {
                        Value::String(s) => Some(s.clone()),
                        Value::Null => Some(String::new()),
                        Value::Number(_) | Value::Bool(_) => Some(json::compact(x)),
                        _ => None,
                    })
                    .collect();
                let Some(p) = parts else { continue };
                p.join(" ")
            }
            Some(Value::Array(_)) => "no arguments".into(),
            _ => continue,
        };
        let line = format!(
            "{} (pid {}, {args})",
            interp(Some(&first)),
            interp(v.get("pid"))
        );
        let pid = json::raw(v.get("pid").unwrap_or(&Value::Null)).into_bytes();
        out.push((cat!(pid, "\t", line), line));
    }
    // `sort -n | cut -f2-`: by the pid's number, then the whole line.
    let num = |l: &[u8]| -> u128 {
        let d: Vec<u8> = l.iter().copied().take_while(u8::is_ascii_digit).collect();
        String::from_utf8_lossy(&d).parse().unwrap_or(0)
    };
    out.sort_by(|a, b| num(&a.0).cmp(&num(&b.0)).then_with(|| a.0.cmp(&b.0)));
    out.into_iter().map(|(_, l)| l).collect()
}

fn now_ms() -> i64 {
    now_us() / 1000
}

/// run -d --replace (issue #29): stop the jobs `run -d` started in this box with these same words,
/// and wait until they and their windows are gone: SIGTERM to each job's processes, SIGKILL to what is
/// left after 5 s. Refuses, before anything is started, when one cannot be stopped or a window of it
/// stays.
pub fn job_replace(ctx: &Ctx, quiet: bool, argv: &[Vec<u8>]) -> crate::exit::Res<()> {
    let want = Value::Array(argv.iter().map(|a| jstr(a)).collect());
    let recs: Vec<std::path::PathBuf> = job_files(ctx)
        .into_iter()
        .filter(|f| record(f).is_some_and(|v| v.get("argv") == Some(&want)))
        .collect();
    let mut procs = if recs.is_empty() {
        Vec::new()
    } else {
        job_live(ctx, &recs)
    };
    // Stopped on purpose: not an early end for the next command to say (job_note).
    if !procs.is_empty() {
        let mut stop = Vec::new();
        for f in &recs {
            let Some(log) =
                record(f).and_then(|v| v.get("log").and_then(Value::as_str).map(String::from))
            else {
                continue;
            };
            if let Some(ns) = re_group(r"/run-([0-9]+)\.log$", log.as_bytes(), 1) {
                stop.push(cat!("omabox.run-", ns, ".exit"));
            }
        }
        job_noted(ctx, &stop);
    }
    if procs.is_empty() {
        // A job of the same program with other arguments still runs (#255): named, said under -q too.
        let others = job_others(ctx, argv);
        if !others.is_empty() {
            let verb = if others.len() == 1 {
                "still runs: stop it, or --replace with its arguments"
            } else {
                "still run: stop them, or --replace with the arguments of one"
            };
            say(format!(
                "--replace: no earlier job with these arguments; {} {verb}",
                others.join(", ")
            ));
        } else if !quiet {
            say(cat!(
                "--replace: no earlier job of this command in box '",
                ctx.name,
                "'"
            ));
        }
        return Ok(());
    }
    // Its windows, to see them go (Hyprland's pids are the box's).
    let bpids: Vec<Value> = procs
        .iter()
        .filter_map(|(_, b)| serde_json::from_slice::<Value>(b).ok())
        .collect();
    let windows = |cj: &[u8]| -> Vec<Vec<u8>> {
        let (vals, _) = json::values(cj);
        let mut out = Vec::new();
        for v in vals {
            let Value::Array(a) = v else { continue };
            for c in a {
                if c.get("pid").is_some_and(|p| bpids.contains(p)) {
                    out.push(json::raw(c.get("address").unwrap_or(&Value::Null)).into_bytes());
                }
            }
        }
        out
    };
    let cj = crate::hypr::hypr_json(ctx, &["clients"])?;
    let wins = windows(&cj);
    let t0 = crate::settle::mono_ms();
    let (mut end, mut killed) = (t0 + 5000, false);
    // (each through a pidfd, while it is still the box's: #377)
    let ns = box_ns(ctx).unwrap_or_default();
    let signal = |procs: &[(Vec<u8>, Vec<u8>)], sig: i32| {
        for (h, _) in procs {
            if let Some(p) = pin_in_box(&ns, h) {
                p.signal(sig);
            }
        }
    };
    signal(&procs, libc::SIGTERM);
    loop {
        std::thread::sleep(std::time::Duration::from_millis(100));
        procs = job_live(ctx, &recs);
        if procs.is_empty() {
            break;
        }
        if crate::settle::mono_ms() < end {
            continue;
        }
        if killed {
            let ids: Vec<Vec<u8>> = procs.iter().map(|(_, b)| b.clone()).collect();
            return Err(ctx.die(cat!(
                "--replace: the earlier job still runs after SIGKILL (pids in the box: ",
                ids.join(&b' '),
                "); nothing started"
            )));
        }
        killed = true;
        end = crate::settle::mono_ms() + 3000;
        signal(&procs, libc::SIGKILL);
    }
    // A client's windows go when it does; a window still there after 3 s is not what --replace is for.
    let mut n = 0;
    if !wins.is_empty() {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let cj = crate::hypr::hypr_json(ctx, &["clients"])?;
            let (vals, _) = json::values(&cj);
            let mut still: Vec<Vec<u8>> = Vec::new();
            for v in vals {
                let Value::Array(a) = v else { continue };
                for c in a {
                    let addr = json::raw(c.get("address").unwrap_or(&Value::Null)).into_bytes();
                    if wins.contains(&addr) {
                        still.extend(shell::words(&addr));
                    }
                }
            }
            if still.is_empty() {
                break;
            }
            if std::time::Instant::now() >= until {
                return Err(ctx.die(cat!(
                    "--replace: the earlier job ended, but its window ",
                    still.join(&b' '),
                    " is still there; nothing started"
                )));
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        n = wins.iter().filter(|w| !w.is_empty()).count();
    }
    if !quiet {
        let s = if n == 1 { "" } else { "s" };
        let k = if killed {
            "; it ignored SIGTERM for 5 s: killed"
        } else {
            ""
        };
        say(format!(
            "--replace: stopped the earlier job ({n} window{s}) in {:.2}s{k}",
            (crate::settle::mono_ms() - t0) as f64 / 1000.0
        ));
    }
    Ok(())
}

/// job_ended WHEN QUIET PID LOG EXIT_FILE ARGV... (#215, #286): whether job PID had ended, nothing of
/// its session left running; if so it is said, with its exit status (run-job.sh's EXIT_FILE, read in
/// the box) and its log's last lines, and its JOB_RC returned: the status (`start`, a plain run -d's
/// first moments: 0 too, a 0 under QUIET not said; WAIT, run -d --wait's answer that did not settle:
/// a 0 is 124, said, as its screen is not the command's), 1 when it was not kept.
#[allow(clippy::too_many_arguments)]
pub fn job_ended(
    ctx: &Ctx,
    wait: bool,
    quiet: bool,
    jpid: &[u8],
    log: &[u8],
    ex: &[u8],
    argv: &[Vec<u8>],
) -> crate::exit::Res<Option<i32>> {
    use crate::exec::{ErrTo, Out, on_box};
    if !shell::is_num(jpid) {
        return Ok(None);
    }
    let rec = ctx.at("jobs").join(shell::path(jpid));
    if rec.is_file() && !job_live(ctx, std::slice::from_ref(&rec)).is_empty() {
        return Ok(None);
    }
    // The waiter writes the status once the job's process is reaped: a moment after it is gone.
    let mut st = Vec::new();
    for i in 1..=10 {
        let argv = vec![
            b"timeout".to_vec(),
            b"2".to_vec(),
            b"cat".to_vec(),
            b"--".to_vec(),
            ex.to_vec(),
        ];
        let r = on_box(ctx, &argv, false, Out::Capture(Some(64)), ErrTo::Null);
        st = shell::subst(&r.out);
        if shell::is_num(&st) {
            break;
        }
        st.clear();
        if i != 10 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    let base = ex.rsplit(|&c| c == b'/').next().unwrap_or(ex).to_vec();
    job_noted(ctx, &[base]);
    let _ = std::fs::remove_file(&rec);
    let what = job_how(&st);
    let rc = if st.is_empty() {
        1
    } else if st == b"0" && wait {
        124
    } else {
        shell::test_int(&st).map_or(1, |n| (n & 0xff) as i32)
    };
    if !wait && st == b"0" && quiet {
        return Ok(Some(rc));
    }
    let mut miss = Vec::new();
    for a in argv {
        if a.starts_with(b"/") && shell::path(a).exists() {
            let t = vec![b"test".to_vec(), b"-e".to_vec(), a.clone()];
            if on_box(ctx, &t, false, Out::Inherit, ErrTo::Inherit)
                .or_exit()?
                .rc
                != 0
            {
                miss.push(a.clone());
            }
        }
    }
    let t = vec![
        b"timeout".to_vec(),
        b"2".to_vec(),
        b"tail".to_vec(),
        b"-n".to_vec(),
        b"5".to_vec(),
        b"--".to_vec(),
        cat!("/home/sbx/", log),
    ];
    let r = on_box(ctx, &t, false, Out::Capture(Some(65536)), ErrTo::Null);
    let mut tail = Vec::new();
    if !r.out.is_empty() {
        let body = r.out.strip_suffix(b"\n").unwrap_or(&r.out);
        for l in body.split(|&c| c == b'\n') {
            tail.extend_from_slice(&cat!("  ", shell::cut_line(l, 300), "\n"));
        }
    }
    let tail = shell::subst(&tail);
    if wait {
        let zero: &[u8] = if st == b"0" {
            b": what the screen shows is not its (124 is about the screen)"
        } else {
            b""
        };
        say(cat!(
            "the command ",
            what,
            " before the screen settled, and left nothing running",
            zero
        ));
    } else {
        say(cat!(
            "the command ",
            what,
            " at once in box '",
            ctx.name,
            "', and left nothing running (log: ",
            ctx.d,
            "/home/",
            log,
            ")"
        ));
    }
    if !miss.is_empty() {
        say(cat!(
            "its arguments name host paths box '",
            ctx.name,
            "' does not have: ",
            miss.join(&b' '),
            " (a box sees the repo and what --ro-bind mounts, not /tmp or the rest of HOME)"
        ));
    }
    if tail.is_empty() {
        say("its log is empty");
    } else {
        say(cat!("its log ends (omabox log -b ", ctx.name, " run):"));
        let _ = std::io::stderr().write_all(&cat!(tail, "\n"));
    }
    Ok(Some(rc))
}

/// While an `omabox run` runs in the box, $D/runs/o.PID.START; once it ended, c.START.END (#300);
/// ok. and ck. for one that KILLs (#329).
pub fn run_mark_begin(ctx: &Ctx, kill: bool) -> Option<(i64, bool)> {
    let mark = now_ms();
    let dir = ctx.at("runs");
    std::fs::create_dir_all(&dir).ok()?;
    let k = if kill { "k" } else { "" };
    std::fs::write(dir.join(format!("o{k}.{}.{mark}", std::process::id())), b"").ok()?;
    Some((mark, kill))
}

/// Its end; dropped: a run that ended 10 s before this one began, and one whose omabox is gone. An
/// ended run within JOB_RUN_GRACE_MS of this one is merged with it into one c.START.END (#392): what
/// job_run_stopped answers is the same, and back-to-back runs keep one file, not one each (a run
/// read them all: 630 in 3 s made each run 2x slower). Each file is removed only after one covering
/// it is there, so runs ending at once lose none.
pub fn run_mark_end(ctx: &Ctx, mark: Option<(i64, bool)>) {
    let Some((mark, kill)) = mark else { return };
    let k = if kill { "k" } else { "" };
    let dir = ctx.at("runs");
    let mut ended = Vec::new();
    for f in shell::glob(&dir, false) {
        let c = |i| re_group(r"^c(k?)\.([0-9]+)\.([0-9]+)$", &f, i);
        let ends = |i| c(i).and_then(|v| shell::test_int(&v));
        let gone = if let (Some(kind), Some(x), Some(y)) = (c(1), ends(2), ends(3)) {
            let old = y < mark - 10000;
            // (merged with its own kind only: a kill's window stays its own)
            if !old && kind == k.as_bytes() {
                ended.push((x, y, f.clone()));
            }
            old
        } else if let Some(p) = re_group(r"^ok?\.([0-9]+)\.[0-9]+$", &f, 1) {
            !shell::path(&cat!("/proc/", p)).is_dir()
        } else {
            false
        };
        if gone {
            let _ = std::fs::remove_file(dir.join(shell::path(&f)));
        }
    }
    // (until none is left that meets it: one that meets it only once it grew is merged too)
    let (mut a, mut b) = (mark, now_ms());
    let mut merged = Vec::new();
    while let Some(i) = ended
        .iter()
        .position(|&(x, y, _)| x <= b + JOB_RUN_GRACE_MS && a <= y + JOB_RUN_GRACE_MS)
    {
        let (x, y, f) = ended.swap_remove(i);
        (a, b) = (a.min(x), b.max(y));
        merged.push(f);
    }
    let name = format!("c{k}.{a}.{b}");
    let _ = std::fs::rename(
        dir.join(format!("o{k}.{}.{mark}", std::process::id())),
        dir.join(&name),
    );
    for f in merged.iter().filter(|f| f.as_slice() != name.as_bytes()) {
        let _ = std::fs::remove_file(dir.join(shell::path(f)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #377: a pid is pinned as the box's only while it is in the box's pid namespace.
    #[test]
    fn pinned_only_in_the_box() {
        let me = std::process::id().to_string().into_bytes();
        let ns = procfs::pidns(&me).unwrap();
        assert!(
            pin_in_box(&ns, &me).is_some(),
            "a process in that namespace"
        );
        assert!(
            pin_in_box(b"pid:[1]", &me).is_none(),
            "...not one in another"
        );
        assert!(pin_in_box(&ns, b"").is_none(), "...nor no pid");
    }

    fn line(a: &str) -> [Vec<u8>; 4] {
        let w: Vec<Vec<u8>> = a.split(' ').map(|s| s.as_bytes().to_vec()).collect();
        [w[0].clone(), w[1].clone(), w[2].clone(), w[3].clone()]
    }

    // As t_unit_cli's job_procs checks.
    #[test]
    fn job_procs_as_bash() {
        let ps = vec![
            line("100 7 7 500"),
            line("101 8 7 510"),
            line("102 9 9 520"),
        ];
        let hp = |v: Vec<(Vec<u8>, Vec<u8>)>| {
            v.into_iter()
                .map(|(a, b)| {
                    format!(
                        "{} {}",
                        String::from_utf8_lossy(&a),
                        String::from_utf8_lossy(&b)
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(hp(job_procs(&ps, b"7", b"500")), vec!["100 7", "101 8"]);
        assert!(job_procs(&ps, b"7", b"499").is_empty());
        assert_eq!(
            hp(job_procs(&[line("101 8 7 510")], b"7", b"-")),
            vec!["101 8"]
        );
        assert!(job_procs(&ps, b"7", b"-").is_empty());
        assert_eq!(
            hp(job_procs(&[line("103 7 6 500")], b"7", b"500")),
            vec!["103 7"]
        );
        assert!(job_procs(&[line("103 7 6 499")], b"7", b"500").is_empty());
    }

    #[test]
    fn how_it_ended() {
        assert_eq!(job_how(b""), "exited (its exit status was not kept)");
        assert_eq!(job_how(b"4"), "exited with status 4");
        assert_eq!(job_how(b"143"), "was killed by SIGTERM (143)");
        assert_eq!(job_how(b"160"), "was killed by SIG (160)");
        assert_eq!(job_how(b"300"), "was killed by SIGRTMIN+10 (300)");
    }

    #[test]
    fn records() {
        assert_eq!(
            record_fields(br#"{"pid":"12","start":"5","log":"/home/sbx/run-1.log"}"#),
            Some([
                b"12".to_vec(),
                b"5".to_vec(),
                b"/home/sbx/run-1.log".to_vec()
            ])
        );
        assert_eq!(record_fields(b"x"), None);
        assert_eq!(
            join_argv(Some(&serde_json::json!(["a", 1, null, true]))),
            Some("a 1  true".into())
        );
        assert_eq!(join_argv(Some(&serde_json::json!(["a", []]))), None);
    }

    // t_unit_replace_others (#255): run -d --replace names the live jobs of the same program with
    // other arguments; job_exe the program as resolved, job_live a record's "live" field.
    #[test]
    fn job_others_as_bash() {
        let d = std::env::temp_dir().join(format!("omabox-ro-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("r/jobs")).unwrap();
        let app = format!("{}/build/app", d.display());
        let rec = |pid: &str, live: &str, exe: &str, argv: &[&str]| {
            let mut v = serde_json::json!({"pid": pid, "start": "1", "log": "l", "live": live, "argv": argv});
            if exe != "-" {
                v["exe"] = serde_json::Value::String(exe.into());
            }
            std::fs::write(d.join("r/jobs").join(pid), v.to_string()).unwrap();
        };
        rec("115", "live", &app, &["./build/app", "--demo", "details"]);
        rec("120", "dead", &app, &["./build/app", "--demo", "old"]);
        rec("130", "live", "qml", &["qml", "other.qml"]);
        rec("140", "live", "-", &["./build/app"]);
        let mut ctx = Ctx::new(b"run", Vec::new());
        ctx.d = shell::bytes(&d.join("r")).to_vec();
        let live = |f: &std::path::Path| {
            std::fs::read_to_string(f).is_ok_and(|t| t.contains(r#""live":"live""#))
        };
        let others = |exe: &str, argv: &[&str]| {
            let a: Vec<Vec<u8>> = argv.iter().map(|w| w.as_bytes().to_vec()).collect();
            job_others_with(&ctx, &a, exe.as_bytes(), live).join("\n")
        };
        assert_eq!(
            others(&app, &["./build/app", "--demo", "table"]),
            "./build/app (pid 115, --demo details)\n./build/app (pid 140, no arguments)",
            "--replace with other arguments: the live job of the program named, the ended one not"
        );
        assert_eq!(
            others(&app, &["./app", "--x"]),
            "./build/app (pid 115, --demo details)",
            "...the same program by another path to it (a record without its path: by the same words only)"
        );
        assert_eq!(
            others("qml", &["qml", "main.qml"]),
            "qml (pid 130, other.qml)",
            "...a bare name by its name"
        );
        assert_eq!(
            others(
                &format!("{}/build/other", d.display()),
                &["./build/other", "--demo", "details"]
            ),
            "",
            "...another program: none"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A file's mtime set to MS since the epoch (`touch -d @S.mmm`).
    fn set_mtime(p: &std::path::Path, ms: i64) {
        let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).unwrap();
        let t = libc::timespec {
            tv_sec: ms.div_euclid(1000),
            tv_nsec: ms.rem_euclid(1000) * 1_000_000,
        };
        // SAFETY: a NUL-ended path of ours and two timespecs.
        let r = unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), [t, t].as_ptr(), 0) };
        assert_eq!(r, 0);
    }

    // t_unit_job_note (#286, finding 399; #298, finding 405; #300, finding 406): a `run -d` job that
    // ended within its first 30 s, said once by the box's next command; a box dir of its own, status
    // files dated as the waiter's would be, records as run -d writes them, a job live while its
    // record says "live". Then the run marks, job_live and job_record on the box's processes given.
    #[test]
    fn job_note_as_bash() {
        let d = std::env::temp_dir().join(format!("omabox-jn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for x in ["run", "home", "jobs"] {
            std::fs::create_dir_all(d.join(x)).unwrap();
        }
        std::fs::write(d.join("jobs-noted"), "").unwrap();
        let ds = d.display().to_string();
        let mut ctx = Ctx::new(b"shot", Vec::new());
        ctx.name = b"u".to_vec();
        ctx.d = ds.as_bytes().to_vec();
        let now = now_us() / 1_000_000;
        let n = std::cell::Cell::new(0u32);
        // jn_job NS_OFFSET_S END_AFTER_MS STATUS [PID LIVE ARGV...]: a job started NS_OFFSET_S
        // seconds ago that ended END_AFTER_MS after its start, with its log and, given a PID, its
        // record.
        let jn_job = |off: i64, ms: i64, st: &str, rec: Option<(&str, &str, &[&str])>| {
            let t0 = now - off;
            n.set(n.get() + 1);
            let ns = format!("{t0}{:09}", n.get());
            std::fs::write(d.join(format!("home/run-{ns}.log")), "").unwrap();
            let f = d.join(format!("run/omabox.run-{ns}.exit"));
            std::fs::write(&f, format!("{st}\n")).unwrap();
            set_mtime(&f, t0 * 1000 + ms);
            if let Some((pid, live, argv)) = rec {
                let v = serde_json::json!({"pid": pid, "start": "1", "log": format!("{ds}/home/run-{ns}.log"),
                    "live": live, "exe": "x", "argv": argv});
                std::fs::write(d.join("jobs").join(pid), format!("{v}\n")).unwrap();
            }
            ns
        };
        let note = || {
            let live = |f: &std::path::Path| {
                std::fs::read_to_string(f).is_ok_and(|t| t.contains(r#""live":"live""#))
            };
            crate::exit::heard(|| job_note_with(&ctx, live)).1
        };
        let append = |f: &str, t: &str| {
            let mut h = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(d.join(f))
                .unwrap();
            std::io::Write::write_all(&mut h, t.as_bytes()).unwrap();
        };
        assert_eq!(note(), "", "no job ended: nothing");
        let ns = jn_job(60, 400, "1", Some(("100", "dead", &["false"])));
        jn_job(
            50,
            30500,
            "1",
            Some(("101", "dead", &["./server", "--port", "1"])),
        );
        jn_job(40, 1500, "0", Some(("102", "dead", &["xdg-open", "URL"])));
        assert_eq!(
            note(),
            format!(
                "omabox: warning: run -d job false (pid 100) exited with status 1 after 0.4s, since the last command (log: {ds}/home/run-{ns}.log)\n"
            ),
            "one that ran past 30 s, one that exited 0 (a one-shot), and one a minute old that ran 0.4 s: only that one said"
        );
        assert_eq!(note(), "", "...once");
        jn_job(
            10,
            2100,
            "3",
            Some(("115", "dead", &["sh", "-c", "sleep 1; exit 3"])),
        );
        assert_eq!(
            note(),
            "omabox: warning: run -d job sh -c sleep 1; exit 3 (pid 115) exited with status 3 after 2.1s, since the last command (log: omabox log -b u run)\n",
            "a job that ended 2.1 s in, its log the newest: its words, pid, status, and log -b NAME run"
        );
        assert_eq!(note(), "", "...said once");
        let ns = jn_job(5, 1000, "143", Some(("116", "dead", &["./app"])));
        jn_job(5, 1200, "143", None);
        std::fs::write(d.join(format!("home/run-9{ns}.log")), "").unwrap();
        let out = note();
        assert_eq!(
            out.lines().count(),
            2,
            "killed, and one whose record went: both said: {out}"
        );
        assert!(
            out.lines().any(|l| l == format!("omabox: warning: run -d job ./app (pid 116) was killed by SIGTERM (143) after 1.0s, since the last command (log: {ds}/home/run-{ns}.log)")),
            "...by its signal, its log by path when not the newest: {out}"
        );
        assert!(
            out.contains(
                "run -d job (its words were not kept) was killed by SIGTERM (143) after 1.2s"
            ),
            "...one with no record: {out}"
        );
        let ns = jn_job(3, 500, "1", Some(("117", "dead", &["./x"])));
        append("jobs-noted", &format!("omabox.run-{ns}.exit\n"));
        assert_eq!(
            note(),
            "",
            "one noted already (--replace stopped it, run -d said it): not said"
        );
        jn_job(
            3,
            100,
            "1",
            Some(("118", "live", &["sh", "-c", "server & exit 1"])),
        );
        assert_eq!(note(), "", "a launcher whose session still runs: no end");
        let ns = jn_job(3, 100, "", Some(("122", "dead", &["./w"])));
        let wf = d.join(format!("run/omabox.run-{ns}.exit"));
        set_mtime(&wf, now_us() / 1000);
        assert_eq!(
            note(),
            "",
            "a status file not written yet: left for the next command"
        );
        std::fs::write(&wf, "4\n").unwrap();
        assert!(
            note().contains("job ./w (pid 122) exited with status 4"),
            "...which says it"
        );
        let long = "a".repeat(80);
        jn_job(3, 100, "2", Some(("119", "dead", &[long.as_str()])));
        assert!(
            note().contains(&format!("job {}… (pid 119)", "a".repeat(59))),
            "long words cut to 60"
        );
        std::os::unix::fs::symlink(
            "/etc/passwd",
            d.join(format!("run/omabox.run-1{}.exit", "0".repeat(18))),
        )
        .unwrap();
        assert_eq!(note(), "", "a link at a status file's name: not read");
        std::fs::remove_file(d.join("jobs-noted")).unwrap();
        jn_job(3, 100, "1", Some(("120", "dead", &["./y"])));
        assert_eq!(
            note(),
            "",
            "a box from an older omabox (no jobs-noted): what is there taken as dealt with"
        );
        assert!(
            !std::fs::read(d.join("jobs-noted"))
                .unwrap_or_default()
                .is_empty(),
            "...and noted"
        );
        jn_job(2, 100, "1", Some(("121", "dead", &["./z"])));
        assert!(
            note().contains("job ./z (pid 121) exited with status 1"),
            "...a job ending after that is said"
        );
        // #298, finding 405: a record found by its log's name (another caller's D spelled
        // otherwise), and one half written beside it (read each on its own).
        jn_job(3, 100, "6", Some(("123", "dead", &["./v"])));
        let r = std::fs::read_to_string(d.join("jobs/123")).unwrap();
        std::fs::write(d.join("jobs/123"), r.replace("/home/", "//home/")).unwrap();
        std::fs::write(d.join("jobs/0"), "{\"pid\":").unwrap();
        assert!(
            note().contains("job ./v (pid 123) exited with status 6"),
            "a record by its log's name, a half-written one beside it: its words"
        );
        std::fs::remove_file(d.join("jobs/0")).unwrap();
        // #300, finding 406: a job killed while an `omabox run` ran in the box ($D/runs:
        // c.START.END, ms; o.PID.START while one runs), or up to 2 s after, was stopped by the agent:
        // not said. An exit, a kill 3 s after, one during a run whose omabox is gone: said.
        let t0 = (now - 20) * 1000;
        std::fs::create_dir_all(d.join("runs")).unwrap();
        std::fs::write(d.join(format!("runs/c.{}.{}", t0 - 500, t0 + 900)), "").unwrap();
        jn_job(20, 800, "143", Some(("124", "dead", &["./k1"])));
        jn_job(20, 2700, "137", Some(("125", "dead", &["./k2"])));
        jn_job(20, 300, "130", Some(("126", "dead", &["./k3"])));
        jn_job(20, 800, "1", Some(("127", "dead", &["./e1"])));
        jn_job(20, 4000, "143", Some(("128", "dead", &["./k4"])));
        let out = note();
        let mut said: Vec<String> = out
            .lines()
            .filter_map(|l| {
                let w = l.strip_prefix("omabox: warning: run -d job ")?;
                let (job, rest) = w.split_once(" (pid ")?;
                let how = rest.split_once(") ")?.1.split(" after ").next()?;
                Some(format!("{job} {how}"))
            })
            .collect();
        said.sort();
        assert_eq!(
            said.join("|"),
            "./e1 exited with status 1|./k4 was killed by SIGTERM (143)",
            "killed during an omabox run or 2 s after: not said; an exit then, and a kill 3 s after: said"
        );
        // (one still running: this test's own process)
        let me = std::process::id();
        std::fs::write(d.join(format!("runs/o.{me}.{}", t0 + 9000)), "").unwrap();
        std::fs::write(d.join(format!("runs/o.999999999.{}", t0 + 9000)), "").unwrap();
        jn_job(20, 9500, "143", Some(("129", "dead", &["./k5"])));
        assert_eq!(note(), "", "...during one still running: not said");
        std::fs::remove_file(d.join(format!("runs/o.{me}.{}", t0 + 9000))).unwrap();
        jn_job(20, 9600, "143", Some(("130", "dead", &["./k6"])));
        assert!(
            note().contains("job ./k6 (pid 130) was killed by SIGTERM"),
            "...one whose omabox is gone without its end: said"
        );
        // run_mark_begin/end: the run's mark open, then closed with its end; one ended long before
        // and one whose omabox is gone dropped.
        std::fs::remove_dir_all(d.join("runs")).unwrap();
        std::fs::create_dir_all(d.join("runs")).unwrap();
        std::fs::write(d.join("runs/c.1000.2000"), "").unwrap();
        std::fs::write(d.join("runs/o.999999999.1000"), "").unwrap();
        let ls = || -> Vec<String> {
            let mut v: Vec<String> = std::fs::read_dir(d.join("runs"))
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        };
        let mark = run_mark_begin(&ctx, false);
        let open = ls()
            .iter()
            .filter(|f| shell::re_match(&format!("^o\\.{me}\\.[0-9]{{13}}$"), f.as_bytes()))
            .count();
        run_mark_end(&ctx, mark);
        let after = ls();
        assert_eq!(open, 1, "run marks: open while it runs");
        assert!(
            after.len() == 1 && shell::re_match(r"^c\.[0-9]{13}\.[0-9]{13}$", after[0].as_bytes()),
            "...then closed; an old one and a dead one's dropped: {after:?}"
        );
        // #392: runs back to back (an ended one within 2 s of the next's start) keep one mark, from
        // the first's start to the last's end; one apart from them by more is kept as it is.
        std::fs::remove_dir_all(d.join("runs")).unwrap();
        std::fs::create_dir_all(d.join("runs")).unwrap();
        let t = now_ms();
        std::fs::write(d.join(format!("runs/c.{}.{}", t - 9000, t - 8000)), "").unwrap();
        std::fs::write(d.join(format!("runs/c.{}.{}", t - 3000, t - 2500)), "").unwrap();
        std::fs::write(d.join(format!("runs/c.{}.{}", t - 2400, t - 1500)), "").unwrap();
        for _ in 0..50 {
            let m = run_mark_begin(&ctx, false);
            run_mark_end(&ctx, m);
        }
        let after = ls();
        assert_eq!(
            after.len(),
            2,
            "back to back: one mark, the one apart kept: {after:?}"
        );
        assert!(
            after.contains(&format!("c.{}.{}", t - 9000, t - 8000)),
            "{after:?}"
        );
        let joined = after
            .iter()
            .find(|f| f.starts_with(&format!("c.{}.", t - 3000)));
        let end = joined.and_then(|f| f.rsplit('.').next()?.parse::<i64>().ok());
        assert!(
            end.is_some_and(|e| e >= t),
            "...from the first's start to the last's end: {after:?}"
        );
        // (what job_run_stopped answers: a job that ended in the 0.1 s between two of them, or in
        // the gap the merge closed, was stopped during runs either way)
        let st = |e| job_run_stopped(&ctx, e);
        assert_eq!(
            [st(t - 1800), st(t - 2450), st(t - 5000), st(t - 8500)],
            [Some(false), Some(false), None, Some(false)]
        );
        // #329: a run that kills keeps a mark of its own kind, not merged with a plain one beside it;
        // a job ending in its window was stopped whatever its status (foot exits 230 on SIGTERM).
        let m = run_mark_begin(&ctx, true);
        run_mark_end(&ctx, m);
        let after = ls();
        assert_eq!(
            after.iter().filter(|f| f.starts_with("ck.")).count(),
            1,
            "a kill's own mark: {after:?}"
        );
        assert_eq!(
            st(now_ms()),
            Some(true),
            "...a job ending now: stopped by it"
        );
        assert!(kill_cmd(&[
            b"pkill".to_vec(),
            b"-f".to_vec(),
            b"app-id x".to_vec()
        ]));
        assert!(kill_cmd(&[
            b"sh".to_vec(),
            b"-c".to_vec(),
            b"/usr/bin/killall foot; true".to_vec()
        ]));
        assert!(!kill_cmd(&[
            b"foot".to_vec(),
            b"--app-id".to_vec(),
            b"skill".to_vec()
        ]));
        // The record kept until the end is dealt with: job_live (the box's processes as BOX_PID SID
        // START, host pids from 901) lists a live job, keeps an ended one's record until job_note
        // has noted its end, then drops it.
        let ps = |rows: &[&str]| -> Vec<[Vec<u8>; 4]> {
            rows.iter()
                .enumerate()
                .map(|(i, r)| line(&format!("{} {r}", 901 + i)))
                .collect()
        };
        std::fs::write(d.join("jobs/50"), format!("{{\"pid\":\"50\",\"start\":\"777\",\"log\":\"{ds}/home/run-1000.log\",\"argv\":[\"a\"]}}\n")).unwrap();
        std::fs::write(d.join("jobs/51"), format!("{{\"pid\":\"51\",\"start\":\"778\",\"log\":\"{ds}/home/run-1001.log\",\"argv\":[\"b\"]}}\n")).unwrap();
        assert_eq!(
            job_live_with(
                &ctx,
                &[d.join("jobs/50"), d.join("jobs/51")],
                &ps(&["50 50 777"])
            ),
            vec![(b"901".to_vec(), b"50".to_vec())],
            "job_live: a live job listed, an ended one not"
        );
        assert!(
            d.join("jobs/51").is_file(),
            "...the ended one's record kept: its end not noted yet"
        );
        append("jobs-noted", "omabox.run-1001.exit\n");
        job_live_with(&ctx, &[d.join("jobs/51")], &ps(&["50 50 777"]));
        assert!(!d.join("jobs/51").exists(), "...dropped once it is");
        // job_record: a job gone before it is recorded still is (start -), and one caught before
        // its setsid (its pid in run-job.sh's session) by its own start time.
        job_record_with(
            &ctx,
            b"52",
            format!("{ds}/home/run-1002.log").as_bytes(),
            &[b"./gone".to_vec()],
            Vec::new(),
        );
        let rec = |pid: &str| -> serde_json::Value {
            serde_json::from_slice(&std::fs::read(d.join("jobs").join(pid)).unwrap()).unwrap()
        };
        let r = rec("52");
        assert_eq!(
            (
                r["pid"].as_str(),
                r["start"].as_str(),
                r["argv"][0].as_str()
            ),
            (Some("52"), Some("-"), Some("./gone")),
            "job_record: a job gone already is recorded, start -"
        );
        job_record_with(
            &ctx,
            b"53",
            format!("{ds}/home/run-1003.log").as_bytes(),
            &[b"./early".to_vec()],
            ps(&["53 49 555"]),
        );
        assert_eq!(
            rec("53")["start"].as_str(),
            Some("555"),
            "...one before its setsid: its start"
        );
        assert_eq!(
            job_live_with(&ctx, &[d.join("jobs/53")], &ps(&["53 49 555"])),
            vec![(b"901".to_vec(), b"53".to_vec())],
            "...and live"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
