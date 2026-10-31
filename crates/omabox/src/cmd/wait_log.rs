//! `omabox wait log LOG RE [-i] [--since OFFSET]` and `omabox wait for DURATION` (#330, finding
//! 585): until a box log (`log`'s names: shell, hyprland, ...; a run -d job's is wait.rs's, #299)
//! gets a new line RE matches, and a fixed wait that says it is one. Agents used `wait until -- false` as a sleep (a whole timeout) and `wait
//! until -- true` as one (no wait at all) before reading a log by luck.
//!
//! A file log is read in the box (as `log` reads it) from the byte the wait started at, every 100 ms;
//! one begun anew meanwhile (restart-shell's shell.log) is read from its start. Hyprland's file lags
//! what it logged by up to 8 KiB (finding 566), so its new lines come from `hyprctl rollinglog`, each
//! try's against the last's (log.rs with_rolling's overlap), every 250 ms.

use crate::boxes::{box_alive, touch};
use crate::cat;
use crate::cmd::log::{Kind, Rolled, rolling, with_rolling};
use crate::exec::{ErrTo, Out, on_box};
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::logs::log_where;
use crate::num::ms_s;
use crate::settle::mono_ms;
use serde_json::{Map, Value};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::ffi::OsStrExt;
use std::process::Stdio;
use std::time::{Duration, Instant};

/// What `wait log` waits for: LOG's name, --grep's RE (grep -E's, as `log --grep`), -i, --since.
pub struct LogWait {
    pub log: Vec<u8>,
    pub re: Vec<u8>,
    pub icase: bool,
    pub since: Option<i64>,
}

/// The first line of DATA (whole lines, each ending in a newline) that RE matches, as `grep -E`
/// matches it (the dialect `log --grep` takes): the byte just past its newline, and the line.
pub fn first_match(re: &[u8], icase: bool, data: &[u8]) -> Option<(usize, Vec<u8>)> {
    if data.is_empty() {
        return None;
    }
    let mut c = crate::sig::command("grep");
    c.args(["-a", "-n", "-m", "1", "-E"]);
    if icase {
        c.arg("-i");
    }
    c.arg("--").arg(std::ffi::OsStr::from_bytes(re));
    c.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = c.spawn().ok()?;
    let feeder = child
        .stdin
        .take()
        .map(|si| crate::exec::feed(si.into(), data.to_vec()));
    let got = child
        .stdout
        .take()
        .map(|so| crate::exec::read_all(so.into(), None))
        .unwrap_or_default();
    if let Some(f) = feeder {
        let _ = f.join();
    }
    let _ = child.wait();
    let colon = got.iter().position(|&c| c == b':')?;
    let n: usize = std::str::from_utf8(&got[..colon]).ok()?.parse().ok()?;
    let line = got[colon + 1..]
        .strip_suffix(b"\n")
        .unwrap_or(&got[colon + 1..]);
    // (the byte past line N's newline)
    let mut seen = 0;
    for (i, &c) in data.iter().enumerate() {
        if c == b'\n' {
            seen += 1;
            if seen == n {
                return Some((i + 1, line.to_vec()));
            }
        }
    }
    Some((data.len(), line.to_vec()))
}

/// DATA up to its last newline: its whole lines.
fn whole(data: &[u8]) -> &[u8] {
    match data.iter().rposition(|&c| c == b'\n') {
        Some(i) => &data[..=i],
        None => &[],
    }
}

/// The lines ROLL has past PREV, an earlier read of the rolling log (with_rolling's overlap), and
/// whether they did not overlap at all (more was logged between the two reads than it holds).
pub fn rolled_past(prev: &[u8], roll: &[u8]) -> (Vec<u8>, bool) {
    if roll.is_empty() {
        return (Vec::new(), false);
    }
    let (merged, how) = with_rolling(prev, roll);
    match how {
        Rolled::Current => (Vec::new(), false),
        Rolled::Added => (merged[prev.len()..].to_vec(), false),
        Rolled::Gap => (roll.to_vec(), !prev.is_empty()),
    }
}

/// A file log's read from byte OFF: its size then, and what it holds from OFF (from its start when
/// it is shorter than OFF: begun anew). None when there is no such regular file; Err when the box
/// could not be asked.
fn fetch(ctx: &Ctx, kind: Kind, path: &[u8], off: i64) -> Result<Option<(i64, Vec<u8>)>, ()> {
    if kind == Kind::Host {
        // (box.log and reap.log: the box dir's, which the box never sees)
        let p = crate::shell::path(path);
        let Ok(mut f) = std::fs::File::open(&p) else {
            return Ok(None);
        };
        let Ok(m) = f.metadata() else { return Ok(None) };
        if !m.is_file() {
            return Ok(None);
        }
        let size = i64::try_from(m.len()).unwrap_or(i64::MAX);
        let from = if size < off { 0 } else { off };
        let _ = f.seek(SeekFrom::Start(u64::try_from(from).unwrap_or(0)));
        let mut v = Vec::new();
        let _ = f.read_to_end(&mut v);
        return Ok(Some((size, v)));
    }
    // In the box, where the path resolves as the box sees it; a regular file only (a FIFO would
    // hold the read). Its size first, then its bytes from OFF.
    const SCRIPT: &str = r#"[ -f "$1" ] || exit 3
s=$(stat -Lc %s -- "$1") || exit 3
echo "$s"
if [ "$s" -lt "$2" ]; then exec cat -- "$1"; fi
exec tail -c +"$(($2 + 1))" -- "$1""#;
    let argv: Vec<Vec<u8>> = vec![
        b"sh".to_vec(),
        b"-c".to_vec(),
        SCRIPT.as_bytes().to_vec(),
        b"sh".to_vec(),
        path.to_vec(),
        off.to_string().into_bytes(),
    ];
    let r = on_box(ctx, &argv, true, Out::Capture(Some(64 << 20)), ErrTo::Null);
    if r.refused {
        return Err(());
    }
    if r.rc != 0 {
        return Ok(None);
    }
    let nl = r
        .out
        .iter()
        .position(|&c| c == b'\n')
        .unwrap_or(r.out.len());
    let Some(size) = crate::shell::dec(&r.out[..nl]) else {
        return Ok(None);
    };
    Ok(Some((
        size,
        r.out.get(nl + 1..).unwrap_or_default().to_vec(),
    )))
}

/// How a wait ended: its line (or --json), and its exit code.
#[allow(clippy::too_many_arguments)]
fn finish(
    res: &str,
    cond: &str,
    msg: &[u8],
    t: i64,
    detail: Option<&[u8]>,
    json: bool,
    extra: Vec<(&str, Value)>,
) -> Res<()> {
    if json {
        let mut m = Map::new();
        let s = |b: &[u8]| Value::String(String::from_utf8_lossy(b).into_owned());
        m.insert("result".into(), Value::String(res.into()));
        m.insert("condition".into(), Value::String(cond.into()));
        m.insert("message".into(), s(msg));
        m.insert(
            "elapsed".into(),
            serde_json::Number::from_f64(t as f64 / 1000.0).map_or(Value::Null, Value::Number),
        );
        m.insert("detail".into(), detail.map_or(Value::Null, s));
        for (k, v) in extra {
            m.insert(k.into(), v);
        }
        out(cat!(crate::json::compact(&Value::Object(m)), "\n"))?;
    } else {
        let d = detail.map_or(Vec::new(), |d| cat!(": ", d));
        out(cat!(res, ": ", msg, d, "\n"))?;
    }
    match res {
        "satisfied" => Ok(()),
        "unsatisfied" => Err(Exit(124)),
        _ => Err(Exit(1)),
    }
}

/// `wait log`: the first line from the wait's start on (or from --since) that RE matches, or 124 at
/// TMO, 1 when the box goes down first.
pub fn log_wait(ctx: &mut Ctx, w: &LogWait, tmo: i64, json: bool) -> Res<()> {
    let name = String::from_utf8_lossy(&w.log).into_owned();
    let Some(path) = log_where(ctx, &name) else {
        return Err(ctx.die(cat!("box '", ctx.name, "' has no ", w.log, " log")));
    };
    let kind = if w.log == b"box" || w.log == b"reap" {
        Kind::Host
    } else {
        Kind::Box
    };
    let what = cat!("a line matching ", w.re, " in the ", w.log, " log");
    let t0 = mono_ms();
    let mut used = Instant::now();
    let mut alive_at = Instant::now();
    let extra = |off: Option<i64>| {
        vec![
            ("log", Value::String(name.clone())),
            (
                "offset",
                off.map_or(Value::Null, |o| Value::Number(o.into())),
            ),
        ]
    };
    let gone = |ctx: &Ctx, t0: i64| -> Res<()> {
        let t = mono_ms() - t0;
        let msg = cat!("box '", ctx.name, "' went down after ", ms_s(t.to_string()));
        finish("unknown", "log", &msg, t, None, json, extra(None))
    };
    if w.log == b"hyprland" {
        // (each try's rolling log against the last's: what it has past it is new)
        let mut prev = rolling(ctx);
        let before = prev.clone();
        let mut gap_said = false;
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let roll = rolling(ctx);
            if roll.is_empty() && !box_alive(ctx)? {
                return gone(ctx, t0);
            }
            let (new, gap) = if prev.is_empty() {
                // (no first read: this one is where the wait starts)
                (Vec::new(), false)
            } else {
                rolled_past(&prev, &roll)
            };
            if gap && !gap_said {
                gap_said = true;
                say(
                    "wait log: Hyprland logged more between two reads than its rolling log holds (4 KiB): a line in between may be missed",
                );
            }
            if !roll.is_empty() {
                prev = roll;
            }
            let now = mono_ms() - t0;
            if let Some((_, line)) = first_match(&w.re, w.icase, whole(&new)) {
                let msg = cat!(what, " after ", ms_s(now.to_string()));
                return finish(
                    "satisfied",
                    "log",
                    &msg,
                    now,
                    Some(&line),
                    json,
                    extra(None),
                );
            }
            if now >= tmo {
                if let Some((_, line)) = first_match(&w.re, w.icase, whole(&before)) {
                    say(cat!(
                        "wait log: one was logged before the wait began (omabox log hyprland --grep ...): ",
                        line
                    ));
                }
                let msg = cat!("no new ", &what[2..], " after ", ms_s(now.to_string()));
                return finish("unsatisfied", "log", &msg, now, None, json, extra(None));
            }
            if used.elapsed() >= Duration::from_secs(30) {
                touch(&ctx.at("used"));
                used = Instant::now();
            }
        }
    }
    // A file: where the wait starts (its end now, or --since), what was there before it.
    let first = match fetch(ctx, kind, &path, 0) {
        Ok(f) => f,
        Err(()) => {
            if !box_alive(ctx)? {
                return gone(ctx, t0);
            }
            return Err(ctx.die(cat!("wait: could not ask box '", ctx.name, "'")));
        }
    };
    let have = first.map(|(_, d)| d).unwrap_or_default();
    let len = i64::try_from(have.len()).unwrap_or(i64::MAX);
    let mut off = match w.since {
        Some(s) if s > len => {
            say(format!(
                "wait log: --since {s} is past the end of the {name} log (it is {len} bytes): reading it all"
            ));
            0
        }
        Some(s) => s,
        None => len,
    };
    let before = have[..usize::try_from(off).unwrap_or(0).min(have.len())].to_vec();
    // (a line begun before the wait, ended after it, is not a new one)
    let mut skip_partial = w.since.is_none() && !have.is_empty() && !have.ends_with(b"\n");
    let mut anew_said = false;
    loop {
        let got = match fetch(ctx, kind, &path, off) {
            Ok(g) => g,
            Err(()) => {
                if !box_alive(ctx)? {
                    return gone(ctx, t0);
                }
                None
            }
        };
        if let Some((size, data)) = got {
            let mut base = off;
            if size < off {
                // (begun anew: restart-shell's shell.log, a log rotated)
                base = 0;
                skip_partial = false;
                if !anew_said {
                    anew_said = true;
                    say(cat!(
                        "wait log: the ",
                        w.log,
                        " log was begun anew during the wait: read from its start"
                    ));
                }
            }
            let mut chunk = whole(&data);
            if skip_partial && !chunk.is_empty() {
                let i = chunk.iter().position(|&c| c == b'\n').unwrap_or(0) + 1;
                chunk = &chunk[i..];
                base += i64::try_from(i).unwrap_or(0);
                skip_partial = false;
            }
            if let Some((end, line)) = first_match(&w.re, w.icase, chunk) {
                let now = mono_ms() - t0;
                let at = base + i64::try_from(end).unwrap_or(0);
                let msg = cat!(what, " after ", ms_s(now.to_string()));
                return finish(
                    "satisfied",
                    "log",
                    &msg,
                    now,
                    Some(&line),
                    json,
                    extra(Some(at)),
                );
            }
            off = base + i64::try_from(chunk.len()).unwrap_or(0);
        }
        let now = mono_ms() - t0;
        if now >= tmo {
            if let Some((_, line)) = first_match(&w.re, w.icase, whole(&before)) {
                say(cat!(
                    "wait log: one was there before the wait began (--since 0 counts it): ",
                    line
                ));
            }
            let msg = cat!("no new ", &what[2..], " after ", ms_s(now.to_string()));
            return finish(
                "unsatisfied",
                "log",
                &msg,
                now,
                None,
                json,
                extra(Some(off)),
            );
        }
        if used.elapsed() >= Duration::from_secs(30) {
            touch(&ctx.at("used"));
            used = Instant::now();
        }
        // (a host log is read without the box: whether it is still up, once a second)
        if kind == Kind::Host && alive_at.elapsed() >= Duration::from_secs(1) {
            if !box_alive(ctx)? {
                return gone(ctx, t0);
            }
            alive_at = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `wait for DURATION`: that long, and said as a fixed wait. Counts as use of the box selected
/// when it is up; needs none.
pub fn for_wait(ctx: &Ctx, ms: i64, json: bool) -> Res<()> {
    let up = box_alive(ctx).unwrap_or(false);
    let t0 = mono_ms();
    let mut used = Instant::now();
    loop {
        let left = ms - (mono_ms() - t0);
        if left <= 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(
            u64::try_from(left.min(1000)).unwrap_or(1),
        ));
        if up && used.elapsed() >= Duration::from_secs(30) {
            touch(&ctx.at("used"));
            used = Instant::now();
        }
    }
    let t = mono_ms() - t0;
    if up {
        touch(&ctx.at("used"));
    }
    let msg = cat!(
        "waited ",
        ms_s(t.to_string()),
        ", a fixed time (wait log, change, window or until waits for the thing itself)"
    );
    finish("satisfied", "for", &msg, t, None, json, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    // #330: the first matching line and where it ends, as grep -E reads RE.
    #[test]
    fn first_lines() {
        let d = b"a one\nb two\nc two\n";
        assert_eq!(first_match(b"two", false, d), Some((12, b"b two".to_vec())));
        assert_eq!(first_match(b"^c", false, d), Some((18, b"c two".to_vec())));
        assert_eq!(first_match(b"TWO", false, d), None);
        assert_eq!(first_match(b"TWO", true, d), Some((12, b"b two".to_vec())));
        assert_eq!(first_match(b"x", false, b""), None);
        // (grep's dialect: an interval, a class)
        assert_eq!(
            first_match(b"o{1}ne( [[:alpha:]])?$|^b", false, d),
            Some((6, b"a one".to_vec()))
        );
        assert_eq!(whole(b"a\nb"), b"a\n");
        assert_eq!(whole(b"ab"), b"");
    }

    // #330: a rolling log's new lines against the last read (finding 566's overlap).
    #[test]
    fn rolling_news() {
        assert_eq!(
            rolled_past(b"a 1\nb 2\nc 3\n", b"b 2\nc 3\nd 4\n"),
            (b"d 4\n".to_vec(), false)
        );
        assert_eq!(
            rolled_past(b"a 1\nb 2\n", b"a 1\nb 2\n"),
            (Vec::new(), false)
        );
        // (no overlap: more came between than it holds; all of it is new, and said)
        assert_eq!(
            rolled_past(b"a 1\nb 2\n", b"x 8\ny 9\n"),
            (b"x 8\ny 9\n".to_vec(), true)
        );
        assert_eq!(rolled_past(b"a 1\n", b""), (Vec::new(), false));
    }
}
