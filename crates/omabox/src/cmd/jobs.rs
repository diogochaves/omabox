//! `omabox jobs [-b NAME] [--json]` and `omabox stop [-b NAME] (JOB... | --all)` (#299, finding
//! 587): the box's `run -d` jobs, and stopping one by its pid without a host pgrep or kill.

use crate::args::{optarg, unexpected, unknown_opt};
use crate::boxes::need_box;
use crate::cat;
use crate::exit::{Ctx, Res, out, say};
use crate::jobs::{Job, Stop, age_text, job_named, job_stop, jobs_now, now_us};
use crate::select::select_box;
use crate::shell;
use crate::shellnote::shell_note;
use serde_json::{Value, json};

/// A job's age in seconds, from its start (its log's name).
fn age_s(j: &Job) -> Option<u64> {
    let now = u128::try_from(now_us()).ok()? * 1000;
    let ns = j.ns?;
    u64::try_from(now.saturating_sub(ns) / 1_000_000_000).ok()
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
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
            b"-h" | b"--help" => return crate::help::usage("jobs"),
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "jobs", a)),
            _ => return Err(unexpected(ctx, "jobs", a, "")),
        }
        i += 1;
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    shell_note(ctx)?;
    let jobs = jobs_now(ctx);
    if json {
        let list: Vec<Value> = jobs
            .iter()
            .map(|j| {
                json!({
                    "pid": shell::dec(&j.pid),
                    "age": age_s(j),
                    "processes": j.procs.len(),
                    "log": String::from_utf8_lossy(&j.log),
                    "argv": j.argv,
                })
            })
            .collect();
        return out(cat!(crate::json::compact(&Value::Array(list)), "\n"));
    }
    if jobs.is_empty() {
        say(cat!("no run -d job runs in box '", ctx.name, "'"));
        return Ok(());
    }
    // (oldest first; PID is the job's in the box, as `windows` shows its apps')
    let mut t = format!(
        "{:<8} {:<8} {:<6} {:<40} {}\n",
        "PID", "AGE", "PROCS", "LOG", "COMMAND"
    );
    for j in &jobs {
        let age = age_s(j).map_or_else(|| "?".to_string(), age_text);
        t.push_str(&format!(
            "{:<8} {:<8} {:<6} {:<40} {}\n",
            String::from_utf8_lossy(&j.pid),
            age,
            j.procs.len(),
            String::from_utf8_lossy(&j.log),
            j.words()
        ));
    }
    out(t)
}

pub fn stop(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut all, mut want) = (Vec::new(), false, Vec::new());
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, args.get(i + 1))?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"--all" => all = true,
            b"-h" | b"--help" => return crate::help::usage("stop"),
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "stop", a)),
            _ if shell::is_num(a) => want.push(a.clone()),
            _ => {
                return Err(unexpected(
                    ctx,
                    "stop",
                    a,
                    "(a job's pid, as omabox jobs lists it)",
                ));
            }
        }
        i += 1;
    }
    if all == !want.is_empty() {
        return Err(ctx.usage_die(if all {
            "stop: --all or pids, not both"
        } else {
            "stop: which job? its pid (omabox jobs), or --all"
        }));
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    shell_note(ctx)?;
    let now = jobs_now(ctx);
    // Every pid a job's, before any is stopped.
    let mut picked: Vec<Job> = Vec::new();
    if all {
        picked = now.clone();
        if picked.is_empty() {
            say(cat!("stop: no run -d job runs in box '", ctx.name, "'"));
            return Ok(());
        }
    }
    for p in &want {
        let Some(j) = job_named(&now, p) else {
            return Err(ctx.die(cat!(
                "stop: no run -d job of pid ",
                p,
                " runs in box '",
                ctx.name,
                "' (omabox -b ",
                ctx.name,
                " jobs lists them; any other process of the box: omabox -b ",
                ctx.name,
                " run -- kill ",
                p,
                ")"
            )));
        };
        if !picked.iter().any(|x| x.file == j.file) {
            picked.push(j.clone());
        }
    }
    for j in &picked {
        let what = cat!("job ", j.pid, " (", j.words(), ")");
        match job_stop(ctx, std::slice::from_ref(&j.file))? {
            Stop::Done {
                windows,
                killed,
                ms,
            } => {
                let s = if windows == 1 { "" } else { "s" };
                let k = if killed {
                    "; it ignored SIGTERM for 5 s: killed"
                } else {
                    ""
                };
                say(cat!(
                    "stopped ",
                    what,
                    format!(", {windows} window{s}, in {:.2}s{k}", ms as f64 / 1000.0)
                ));
            }
            Stop::Stuck(ids) => {
                return Err(ctx.die(cat!(
                    "stop: ",
                    what,
                    " still runs after SIGKILL (pids in the box: ",
                    ids.join(&b' '),
                    ")"
                )));
            }
            Stop::Window(still) => {
                return Err(ctx.die(cat!(
                    "stop: ",
                    what,
                    " ended, but its window ",
                    still.join(&b' '),
                    " is still there"
                )));
            }
        }
    }
    Ok(())
}
