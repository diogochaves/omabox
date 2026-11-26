//! `omabox frames [-b NAME] [--monitor NAME] [DURATION | [--hold DURATION] -- ACTION...] [--idle
//! DURATION] [--json]` (#488, finding 763): how a box's screen ran. The times of the compositor's
//! frames on one output, watched by tools/burst (`omabox-burst frames`: one wlr-screencopy
//! copy_with_damage after another, each completing at the output's next commit), for DURATION (5s)
//! or around one action as `record -- ACTION` runs it; then the time between frames (median, p95,
//! p99, max) and the frames that came late: more than LATE refreshes after the one before.
//!
//! A compositor draws only when something changed, so the time between two frames is how long the
//! second took only while something kept changing: a longer one is a frame that came late, or a
//! moment with nothing to draw, and the frames cannot say which. Up to --idle (250 ms) it counts as
//! late; past that as a pause, listed on its own and left out of the figures.

use crate::args::{optarg, unknown_opt};
use crate::boxes::need_box;
use crate::boxmeta::BoxMeta;
use crate::cat;
use crate::cmd::shot::BOX_BURST;
use crate::exec::{ErrTo, In, Out, on_box_bg, pipe, read_all};
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::gpu::{cost_context, cost_lines};
use crate::jail::in_jail;
use crate::num::ms_duration;
use crate::select::select_box;
use serde_json::{Value, json};
use std::io::BufRead;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::sync::mpsc;
use std::time::Duration;

/// A frame is late when it came more than this many refreshes after the one before.
pub const LATE: f64 = 1.5;
/// --idle's default: no frame for longer is a pause, not a late frame (ms).
const IDLE_MS: i64 = 250;

/// What the tool said: the output, and each frame's time (the compositor's, ns) with the tool's own
/// clock when it heard of it.
#[derive(Debug, Default, PartialEq)]
pub struct Watch {
    pub output: Vec<u8>,
    pub mode: Vec<u8>,
    pub mhz: i64,
    pub how: Vec<u8>,
    pub ready: Option<(i64, i64)>,
    pub frames: Vec<(i64, i64)>,
    pub end: Option<i64>,
}

/// The tool's lines (`watch`, `ready`, `f`, `end`); one it does not know is left out.
pub fn parse_watch(raw: &[u8]) -> Watch {
    let mut w = Watch::default();
    let num = |v: &[u8]| -> Option<i64> { std::str::from_utf8(v).ok()?.parse().ok() };
    for l in raw.split(|&c| c == b'\n') {
        let f: Vec<&[u8]> = l.split(|&c| c == b' ').collect();
        match (f[0], f.len()) {
            (b"watch", 5) => {
                w.output = f[1].to_vec();
                w.mode = f[2].to_vec();
                w.mhz = num(f[3]).unwrap_or(0);
                w.how = f[4].to_vec();
            }
            (b"ready", 3) => w.ready = num(f[1]).zip(num(f[2])),
            (b"f", 3) => {
                if let Some(p) = num(f[1]).zip(num(f[2])) {
                    w.frames.push(p);
                }
            }
            (b"end", 2) => w.end = num(f[1]),
            _ => {}
        }
    }
    w
}

/// The figures of one watch. Times in ms.
#[derive(Debug, Default, PartialEq)]
pub struct Stats {
    pub frames: usize,
    /// The times between frames that count: all but the pauses.
    pub intervals: usize,
    pub median: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    pub mean: f64,
    /// Their sum: how long the screen drew.
    pub drawing: f64,
    pub late: usize,
    /// The refreshes the late ones went without a frame.
    pub skipped: u64,
    /// Late ones after a frame the watcher heard of more than half a refresh after it was drawn:
    /// it asked for the next one that late, and may have missed a frame between.
    pub unsure: usize,
    pub pauses: usize,
    pub pause_total: f64,
    pub pause_longest: f64,
    /// The longest the watcher took to hear of a frame.
    pub heard_max: f64,
}

/// The P-th percentile of sorted V, nearest rank (the smallest value with P% of them at or under it).
pub fn percentile(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let rank = (p / 100.0 * v.len() as f64).ceil() as usize;
    v[rank.clamp(1, v.len()) - 1]
}

/// stats FRAMES REFRESH IDLE: FRAMES as the tool gave them (ns), REFRESH and IDLE in ms.
pub fn stats(frames: &[(i64, i64)], refresh: f64, idle: f64) -> Stats {
    let mut s = Stats {
        frames: frames.len(),
        ..Stats::default()
    };
    let ms = |ns: i64| ns as f64 / 1e6;
    let mut ivs: Vec<f64> = Vec::new();
    for (k, &(ts, recv)) in frames.iter().enumerate() {
        s.heard_max = s.heard_max.max(ms(recv - ts));
        if k == 0 {
            continue;
        }
        let (pts, precv) = frames[k - 1];
        let iv = ms(ts - pts);
        if iv > idle {
            s.pauses += 1;
            s.pause_total += iv;
            s.pause_longest = s.pause_longest.max(iv);
            continue;
        }
        if refresh > 0.0 && iv > LATE * refresh {
            s.late += 1;
            s.skipped += ((iv / refresh).round() as u64).saturating_sub(1).max(1);
            if ms(precv - pts) > refresh / 2.0 {
                s.unsure += 1;
            }
        }
        ivs.push(iv);
    }
    s.intervals = ivs.len();
    if ivs.is_empty() {
        return s;
    }
    s.drawing = ivs.iter().sum();
    s.mean = s.drawing / ivs.len() as f64;
    ivs.sort_by(f64::total_cmp);
    s.median = percentile(&ivs, 50.0);
    s.p95 = percentile(&ivs, 95.0);
    s.p99 = percentile(&ivs, 99.0);
    s.max = ivs[ivs.len() - 1];
    s
}

fn f1(v: f64) -> String {
    format!("{v:.1}")
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The report's own lines (the box's and the GPU's follow): REFRESH, IDLE and WATCHED in ms.
pub fn report(name: &[u8], w: &Watch, s: &Stats, refresh: f64, idle: f64, watched: f64) -> Vec<u8> {
    let hz = w.mhz as f64 / 1000.0;
    let hz = if hz.fract() == 0.0 {
        format!("{hz:.0}")
    } else {
        format!("{hz:.2}")
    };
    let mut o = cat!(
        "box '",
        name,
        "': ",
        w.mode,
        "@",
        hz,
        " on ",
        w.output,
        format!(", watched {:.2}s: ", watched / 1000.0)
    );
    if s.frames == 0 {
        o.extend_from_slice(
            b"no frame: nothing on the screen changed (a compositor draws only what changes)\n",
        );
        return o;
    }
    if s.intervals == 0 {
        o.extend_from_slice(
            cat!(
                plural(s.frames, "frame", "frames"),
                ", no two of them close enough to time (frames come only while something changes)\n"
            )
            .as_slice(),
        );
    } else {
        o.extend_from_slice(
            format!(
                "{}, {:.1} a second while it drew ({:.2}s)\n",
                plural(s.frames, "frame", "frames"),
                1000.0 / s.mean,
                s.drawing / 1000.0
            )
            .as_bytes(),
        );
        o.extend_from_slice(
            format!(
                "frame time: median {} ms, p95 {}, p99 {}, max {} (a refresh is {} ms)\n",
                f1(s.median),
                f1(s.p95),
                f1(s.p99),
                f1(s.max),
                f1(refresh)
            )
            .as_bytes(),
        );
        if s.late == 0 {
            o.extend_from_slice(
                format!(
                    "late: none of {} came over {LATE} refreshes ({} ms) after the frame before\n",
                    s.intervals,
                    f1(LATE * refresh)
                )
                .as_bytes(),
            );
        } else {
            o.extend_from_slice(
                format!(
                    "late: {} of {} ({:.1}%) came over {LATE} refreshes ({} ms) after the frame before, {} without a frame",
                    s.late,
                    s.intervals,
                    100.0 * s.late as f64 / s.intervals as f64,
                    f1(LATE * refresh),
                    plural(s.skipped as usize, "refresh", "refreshes"),
                )
                .as_bytes(),
            );
            if s.unsure > 0 {
                o.extend_from_slice(
                    format!(
                        "; {} of them unsure (the watcher heard of the frame before over half a refresh late, and may have missed one)",
                        s.unsure
                    )
                    .as_bytes(),
                );
            }
            o.push(b'\n');
        }
    }
    if s.pauses > 0 {
        o.extend_from_slice(
            format!(
                "pauses: {} ({:.2}s, the longest {:.0} ms): no frame for over {:.0} ms, the screen idle or stalled; not in the figures above\n",
                s.pauses,
                s.pause_total / 1000.0,
                s.pause_longest,
                idle
            )
            .as_bytes(),
        );
    }
    o
}

/// What the numbers are not, for this box: said under the report, and in --json's `notes`.
fn notes(interactive: bool, software: Option<&str>) -> Vec<String> {
    let mut n = Vec::new();
    if let Some(r) = software {
        n.push(format!(
            "software rendering ({r}): these are the CPU's frame times, not a GPU's"
        ));
    }
    if interactive {
        n.push(
            "an interactive box draws when your compositor lets it: hidden (its workspace not shown) that is about 15 frames a second whatever it shows, so these are your compositor's pace, not the box's cost (a headless box draws at its own mode's rate)"
                .into(),
        );
    }
    n
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut mon, mut json) = (Vec::new(), Vec::new(), false);
    let (mut dur, mut hold, mut idle): (Option<i64>, Option<i64>, i64) = (None, None, IDLE_MS);
    let mut act: Option<Vec<Vec<u8>>> = None;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, args.get(i + 1))?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"--monitor" => {
                optarg(ctx, a, args.get(i + 1))?;
                mon = args[i + 1].clone();
                i += 1;
            }
            b"--hold" => {
                optarg(ctx, a, args.get(i + 1))?;
                match ms_duration(&args[i + 1]).filter(|&x| x <= 60_000) {
                    Some(x) => hold = Some(x),
                    None => {
                        return Err(ctx.die(cat!(
                            "frames: --hold takes a duration up to 60s, like 500ms or 2s, got ",
                            args[i + 1]
                        )));
                    }
                }
                i += 1;
            }
            b"--idle" => {
                optarg(ctx, a, args.get(i + 1))?;
                match ms_duration(&args[i + 1]).filter(|x| (50..=60_000).contains(x)) {
                    Some(x) => idle = x,
                    None => {
                        return Err(ctx.die(cat!(
                            "frames: --idle takes a duration from 50ms to 60s (no frame for longer is a pause, not a late frame; 250ms by default), got ",
                            args[i + 1]
                        )));
                    }
                }
                i += 1;
            }
            b"--json" => json = true,
            b"--" => {
                act = Some(args[i + 1..].to_vec());
                break;
            }
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "frames", a)),
            _ => {
                let d = ms_duration(a).filter(|x| (100..=600_000).contains(x));
                let (Some(d), None) = (d, dur) else {
                    return Err(ctx.die(cat!(
                        "frames: how long to watch, 100ms to 10m, like 5s (a bare number is seconds), or -- ACTION (an omabox command: keys, click, run -d ...), got ",
                        a
                    )));
                };
                dur = Some(d);
            }
        }
        i += 1;
    }
    if act.is_some() && dur.is_some() {
        return Err(ctx.die(
            "frames: DURATION or -- ACTION, not both (after an action: --hold DURATION)",
        ));
    }
    if act.is_none() && hold.is_some() {
        return Err(ctx.die(
            "frames: --hold goes with -- ACTION (how long it watches on after the action)",
        ));
    }
    if let Some(a) = &act {
        crate::cmd::record::action_check_as(ctx, "frames", a)?;
        // (as record's: the broker runs no action for a jail)
        if in_jail() {
            ctx.parsed();
            return Err(ctx.die(
                "frames -- ACTION: not for a jailed agent yet (start the action, then: omabox frames DURATION)",
            ));
        }
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    crate::trap::stop_on_signals();
    if mon.is_empty() {
        let screen = crate::screen::box_screen(ctx)?;
        mon = crate::cmd::mode::screen_parts(&screen).0.to_vec();
    }
    let mut argv: Vec<Vec<u8>> = vec![
        BOX_BURST.as_bytes().to_vec(),
        b"frames".to_vec(),
        b"--output".to_vec(),
        mon.clone(),
    ];
    match dur.or(act.is_none().then_some(5000)) {
        Some(d) => argv.extend([b"--for".to_vec(), d.to_string().into_bytes()]),
        None => argv.push(b"--tied".to_vec()),
    }
    // The tool beside: its lines read as they come (a long watch is more than a pipe holds), its
    // words kept; `ready` said here, so an action starts on a watched screen. --tied: it ends when
    // its stdin closes, which this process's end does too.
    let (pr, pw) = pipe().map_err(|_| Exit(1))?;
    let (er, ew) = pipe().map_err(|_| Exit(1))?;
    let (tie_r, tie_w) = pipe().map_err(|_| Exit(1))?;
    let c = ctx.clone();
    let tied = act.is_some();
    let tool = std::thread::spawn(move || {
        let stdin = if tied { In::Fd(tie_r) } else { In::Null };
        let r = on_box_bg(
            &c,
            &argv,
            stdin,
            Out::Fd(pw),
            ErrTo::Fd(ew.as_raw_fd()),
            None,
        );
        drop(ew);
        r.rc
    });
    let errs = std::thread::spawn(move || read_all(er, Some(4096)));
    let (tx, rx) = mpsc::channel::<()>();
    let lines = std::thread::spawn(move || {
        let mut rd = std::io::BufReader::new(std::fs::File::from(pr));
        let (mut all, mut line) = (Vec::new(), Vec::new());
        loop {
            line.clear();
            if rd.read_until(b'\n', &mut line).unwrap_or(0) == 0 {
                break;
            }
            if line.starts_with(b"ready ") {
                let _ = tx.send(());
            }
            all.extend_from_slice(&line);
        }
        all
    });
    let mut act_rc = 0;
    // (the first frame: the tool's own deadline is 10 s)
    let ready = rx.recv_timeout(Duration::from_secs(20)).is_ok();
    if let (true, Some(a)) = (ready, &act) {
        let mut c = crate::sig::omabox_cmd();
        c.arg("-b").arg(std::ffi::OsStr::from_bytes(&ctx.name));
        c.args(a.iter().map(|x| std::ffi::OsStr::from_bytes(x)));
        // (`>&2`: its output on this one's stderr, the report alone on stdout)
        if let Ok(e) = crate::exec::dup_hi(2) {
            c.stdout(std::process::Stdio::from(e));
        }
        act_rc = crate::sig::run_status(c).unwrap_or(127);
        let h = hold.unwrap_or(500);
        crate::trap::sleep(format!("{}.{:03}", h / 1000, h % 1000).as_bytes());
    }
    drop(tie_w);
    let rc = tool.join().unwrap_or(1);
    let raw = lines.join().unwrap_or_default();
    let said = errs.join().unwrap_or_default();
    let w = parse_watch(&raw);
    if rc != 0 || w.ready.is_none() || w.end.is_none() {
        let said = crate::shell::strip_nl(&said);
        let said = said
            .strip_prefix(b"omabox-burst: ")
            .unwrap_or(&said)
            .to_vec();
        if said.starts_with(b"usage: ") {
            return Err(ctx.die(cat!(
                "frames: box '",
                ctx.name,
                "' was started by an omabox whose tools do not time frames: omabox down ",
                ctx.name,
                ", then up again"
            )));
        }
        return Err(ctx.die(cat!(
            "frames: could not watch box '",
            ctx.name,
            "''s screen: ",
            if said.is_empty() {
                format!("the watcher ended {rc}").into_bytes()
            } else {
                said
            }
        )));
    }
    let refresh = if w.mhz > 0 {
        1e6 / w.mhz as f64
    } else {
        0.0
    };
    let s = stats(&w.frames, refresh, idle as f64);
    let watched = (w.end.unwrap_or(0) - w.ready.map_or(0, |r| r.1)) as f64 / 1e6;
    let cost = cost_context(ctx)?;
    let cr: Value = serde_json::from_slice(&cost.cr).unwrap_or(Value::Null);
    let software = (cr["software"] == json!(true)).then(|| {
        let r = cr["renderer"].as_str().unwrap_or("software");
        r.split(" (").next().unwrap_or(r).to_string()
    });
    let interactive = BoxMeta::load(&ctx.d).interactive();
    let notes = notes(interactive, software.as_deref());
    if json {
        let r3 = |v: f64| (v * 1000.0).round() / 1000.0;
        let t0 = w.ready.map_or(0, |r| r.0);
        let timed = s.intervals > 0;
        let v = json!({
            "box": String::from_utf8_lossy(&ctx.name),
            "output": String::from_utf8_lossy(&w.output),
            "mode": String::from_utf8_lossy(&w.mode),
            "refresh_hz": w.mhz as f64 / 1000.0,
            "refresh_ms": r3(refresh),
            "seconds": r3(watched / 1000.0),
            "copy": String::from_utf8_lossy(&w.how),
            "frames": s.frames,
            "intervals": s.intervals,
            "fps": if timed { json!(r3(1000.0 / s.mean)) } else { Value::Null },
            "drawing_seconds": r3(s.drawing / 1000.0),
            "frame_ms": if timed {
                json!({"median": r3(s.median), "p95": r3(s.p95), "p99": r3(s.p99), "max": r3(s.max), "mean": r3(s.mean)})
            } else { Value::Null },
            "late": {"over_ms": r3(LATE * refresh), "count": s.late,
                "percent": if timed { json!(r3(100.0 * s.late as f64 / s.intervals as f64)) } else { Value::Null },
                "refreshes_skipped": s.skipped, "unsure": s.unsure},
            "pauses": {"over_ms": idle, "count": s.pauses, "seconds": r3(s.pause_total / 1000.0), "longest_ms": r3(s.pause_longest)},
            "watcher_heard_ms_max": r3(s.heard_max),
            "interactive": interactive,
            "software": software.is_some(),
            "notes": notes,
            "aquamarine": serde_json::from_slice::<Value>(&cost.aq).unwrap_or(Value::Null),
            "render": serde_json::from_slice::<Value>(&cost.rj).unwrap_or(Value::Null),
            "client_render": cr,
            "times_ms": w.frames.iter().map(|f| r3((f.0 - t0) as f64 / 1e6)).collect::<Vec<f64>>(),
        });
        out(cat!(crate::json::pretty(&v), "\n"))?;
    } else {
        out(report(&ctx.name, &w, &s, refresh, idle as f64, watched))?;
        for n in &notes {
            out(cat!(n, "\n"))?;
        }
        cost_lines(ctx, &cost)?;
    }
    if act_rc != 0 {
        say(cat!(
            "frames: the action ended ",
            act_rc.to_string(),
            " (the frames watched are above)"
        ));
        return Err(Exit(u8::try_from(act_rc & 0xff).unwrap_or(1)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: f64 = 1000.0 / 60.0;

    /// Frames at these ms, each heard HEARD ms after it was drawn.
    fn at(ms: &[f64], heard: f64) -> Vec<(i64, i64)> {
        ms.iter()
            .map(|&t| ((t * 1e6) as i64, ((t + heard) * 1e6) as i64))
            .collect()
    }

    #[test]
    fn watch_lines() {
        let w = parse_watch(
            b"watch HEADLESS-2 1920x1080 60000 corner\nready 100 150\nf 200 260\nnoise\nf 300 x\nf 400 410\nend 500\n",
        );
        assert_eq!(w.output, b"HEADLESS-2");
        assert_eq!(w.mode, b"1920x1080");
        assert_eq!((w.mhz, w.how.as_slice()), (60000, &b"corner"[..]));
        assert_eq!(w.ready, Some((100, 150)));
        assert_eq!(w.frames, vec![(200, 260), (400, 410)]);
        assert_eq!(w.end, Some(500));
        // (cut short: no end)
        assert_eq!(parse_watch(b"watch A 1x1 0 whole\nready 1 2\nf 3").end, None);
    }

    #[test]
    fn percentiles_nearest_rank() {
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&v, 50.0), 50.0);
        assert_eq!(percentile(&v, 95.0), 95.0);
        assert_eq!(percentile(&v, 99.0), 99.0);
        assert_eq!(percentile(&v, 100.0), 100.0);
        assert_eq!(percentile(&[7.0], 99.0), 7.0);
        assert_eq!(percentile(&[1.0, 2.0, 3.0], 50.0), 2.0);
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0], 50.0), 2.0);
        assert_eq!(percentile(&[], 50.0), 0.0);
    }

    #[test]
    fn smooth_has_no_late_frame() {
        let t: Vec<f64> = (0..61).map(|k| f64::from(k) * R).collect();
        let s = stats(&at(&t, 2.0), R, 250.0);
        assert_eq!((s.frames, s.intervals), (61, 60));
        assert_eq!((s.late, s.skipped, s.unsure, s.pauses), (0, 0, 0, 0));
        assert!((s.median - R).abs() < 0.01 && (s.max - R).abs() < 0.01);
        assert!((s.drawing - 1000.0).abs() < 0.1);
        assert!((s.heard_max - 2.0).abs() < 0.01);
    }

    #[test]
    fn late_frames_and_the_refreshes_they_skipped() {
        // one frame two refreshes after the one before (1 skipped), one four after (3 skipped)
        let t = [0.0, R, 3.0 * R, 4.0 * R, 8.0 * R, 9.0 * R];
        let s = stats(&at(&t, 1.0), R, 250.0);
        assert_eq!((s.frames, s.intervals, s.late, s.skipped), (6, 5, 2, 4));
        assert_eq!(s.unsure, 0);
        assert!((s.max - 4.0 * R).abs() < 0.01);
        assert!((s.median - R).abs() < 0.01);
        // just under the rule: 1.4 refreshes is not late; just over is, and skipped one at least
        let s = stats(&at(&[0.0, 1.4 * R, 2.95 * R], 1.0), R, 250.0);
        assert_eq!((s.late, s.skipped), (1, 1));
    }

    #[test]
    fn a_late_frame_after_one_heard_late_is_unsure() {
        let mut f = at(&[0.0, R, 3.0 * R, 4.0 * R], 1.0);
        f[1].1 = f[1].0 + (10.0 * 1e6) as i64; // heard 10 ms after: over half a refresh
        let s = stats(&f, R, 250.0);
        assert_eq!((s.late, s.unsure), (1, 1));
        assert!((s.heard_max - 10.0).abs() < 0.01);
    }

    #[test]
    fn pauses_are_not_late_frames_nor_in_the_figures() {
        let t = [0.0, R, 2.0 * R, 2.0 * R + 800.0, 3.0 * R + 800.0];
        let s = stats(&at(&t, 1.0), R, 250.0);
        assert_eq!((s.frames, s.intervals, s.late, s.pauses), (5, 3, 0, 1));
        assert!((s.pause_total - 800.0).abs() < 0.01 && (s.pause_longest - 800.0).abs() < 0.01);
        assert!((s.max - R).abs() < 0.01);
        // --idle longer: the same gap is a late frame
        let s = stats(&at(&t, 1.0), R, 1000.0);
        assert_eq!((s.intervals, s.late, s.pauses, s.skipped), (4, 1, 0, 47));
    }

    #[test]
    fn nothing_drawn() {
        assert_eq!(stats(&[], R, 250.0), Stats::default());
        let s = stats(&at(&[5.0], 1.0), R, 250.0);
        assert_eq!((s.frames, s.intervals, s.late), (1, 0, 0));
        // two frames a pause apart: none to time
        let s = stats(&at(&[0.0, 900.0], 1.0), R, 250.0);
        assert_eq!((s.frames, s.intervals, s.pauses), (2, 0, 1));
    }

    fn text(w: &Watch, s: &Stats) -> String {
        String::from_utf8(report(b"b", w, s, R, 250.0, 5000.0)).unwrap()
    }

    #[test]
    fn report_words() {
        let w = Watch {
            output: b"HEADLESS-2".to_vec(),
            mode: b"1920x1080".to_vec(),
            mhz: 60000,
            ..Watch::default()
        };
        assert_eq!(
            text(&w, &Stats::default()),
            "box 'b': 1920x1080@60 on HEADLESS-2, watched 5.00s: no frame: nothing on the screen changed (a compositor draws only what changes)\n"
        );
        let t: Vec<f64> = (0..61).map(|k| f64::from(k) * R).collect();
        assert_eq!(
            text(&w, &stats(&at(&t, 2.0), R, 250.0)),
            "box 'b': 1920x1080@60 on HEADLESS-2, watched 5.00s: 61 frames, 60.0 a second while it drew (1.00s)\n\
             frame time: median 16.7 ms, p95 16.7, p99 16.7, max 16.7 (a refresh is 16.7 ms)\n\
             late: none of 60 came over 1.5 refreshes (25.0 ms) after the frame before\n"
        );
        let mut f = at(&[0.0, R, 3.0 * R, 4.0 * R, 4.0 * R + 800.0], 1.0);
        f[1].1 = f[1].0 + 10_000_000;
        assert_eq!(
            text(&w, &stats(&f, R, 250.0)),
            "box 'b': 1920x1080@60 on HEADLESS-2, watched 5.00s: 5 frames, 45.0 a second while it drew (0.07s)\n\
             frame time: median 16.7 ms, p95 33.3, p99 33.3, max 33.3 (a refresh is 16.7 ms)\n\
             late: 1 of 3 (33.3%) came over 1.5 refreshes (25.0 ms) after the frame before, 1 refresh without a frame; 1 of them unsure (the watcher heard of the frame before over half a refresh late, and may have missed one)\n\
             pauses: 1 (0.80s, the longest 800 ms): no frame for over 250 ms, the screen idle or stalled; not in the figures above\n"
        );
        let w2 = Watch {
            mhz: 59940,
            ..Watch::default()
        };
        assert!(text(&w2, &stats(&at(&[0.0, 900.0], 1.0), R, 250.0)).contains(
            "@59.94 on , watched 5.00s: 2 frames, no two of them close enough to time (frames come only while something changes)\npauses: 1 "
        ));
    }

    #[test]
    fn notes_say_what_the_numbers_are_not() {
        assert!(notes(false, None).is_empty());
        let n = notes(true, Some("llvmpipe"));
        assert!(n[0].starts_with("software rendering (llvmpipe): these are the CPU's frame times"));
        assert!(n[1].contains("about 15 frames a second"));
    }
}
