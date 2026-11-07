//! `omabox events` (issue #39, finding 108): the box's Hyprland events, which tools/events records in
//! the box HOME's events.log, "SECONDS.MILLIS EVENT>>DATA" a line, never truncated. "From here" is a
//! byte offset into it (--mark, kept in the box dir, which the box cannot see). Read as `log` reads;
//! -f follows, --until waits for the first event that matches (124 at --timeout).

use crate::args::{optarg, unexpected, unknown_opt};
use crate::boxes::{BoxPid, box_alive, box_pid, need_box, touch};
use crate::cat;
use crate::cmd::log::{Kind, Reading, live, tail_lines};
use crate::desk::events_log;
use crate::exec::{ErrTo, Out, on_box};
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::json;
use crate::logs::log_dead_path;
use crate::select::select_box;
use crate::shell::{self, Re};
use std::io::Write;
use std::os::fd::{AsRawFd, OwnedFd};
use std::time::{Duration, Instant};

/// WAIT_MAX_MS: --timeout's most, 10 min.
const WAIT_MAX_MS: i64 = 600_000;

fn now_ms() -> i64 {
    crate::jobs::now_us() / 1000
}

pub use crate::num::ms_duration;

/// events_show's first step, bash's awk program as it is.
const AWK_KEEP: &str = r#"
    { sp = index($0, " "); ts = substr($0, 1, sp - 1); ev = substr($0, sp + 1) }
    ts !~ /^[0-9]+\.[0-9]+$/ { next }
    ENVIRON["EV_MIN"] > 0 && ts * 1000 < ENVIRON["EV_MIN"] { next }
    ENVIRON["EV_RE"] != "" { IGNORECASE = ENVIRON["EV_I"] == 1; if (ev !~ ENVIRON["EV_RE"]) next }
    { print; fflush() }"#;

/// What events keeps and prints: --grep's expression (gawk's, as bash's awk reads it), -i, --json,
/// and --since's time (MIN_MS: events from then on).
struct Show {
    re: Vec<u8>,
    icase: bool,
    as_json: bool,
    min: i64,
}

impl Show {
    /// events_show, the first step: the lines kept, as awk prints them. With --grep, awk itself (its
    /// regex dialect and IGNORECASE are gawk's).
    fn keep(&self, data: &[u8]) -> Vec<u8> {
        if !self.re.is_empty() {
            return self.awk(data);
        }
        let mut out = Vec::new();
        let mut lines: Vec<&[u8]> = data.split(|&c| c == b'\n').collect();
        if lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        for l in lines {
            let Some(sp) = l.iter().position(|&c| c == b' ') else {
                continue;
            };
            let ts = &l[..sp];
            let dot = ts.iter().position(|&c| c == b'.');
            let ok = dot.is_some_and(|d| {
                d > 0
                    && d + 1 < ts.len()
                    && ts
                        .iter()
                        .enumerate()
                        .all(|(i, c)| i == d || c.is_ascii_digit())
            });
            if !ok {
                continue;
            }
            if self.min > 0 {
                let t: f64 = String::from_utf8_lossy(ts).parse().unwrap_or(0.0);
                if t * 1000.0 < self.min as f64 {
                    continue;
                }
            }
            out.extend_from_slice(&cat!(l, "\n"));
        }
        out
    }

    /// The expression through the environment: awk -v would take the backslashes out of it.
    fn awk_cmd(&self) -> std::process::Command {
        let mut c = crate::sig::command("awk");
        c.arg(AWK_KEEP)
            .env(
                "EV_RE",
                <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(&self.re),
            )
            .env("EV_MIN", self.min.to_string())
            .env("EV_I", if self.icase { "1" } else { "0" });
        c
    }

    fn awk(&self, data: &[u8]) -> Vec<u8> {
        let mut c = self.awk_cmd();
        c.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped());
        let Ok(mut ch) = c.spawn() else {
            return Vec::new();
        };
        let feeder = ch
            .stdin
            .take()
            .map(|si| crate::exec::feed(si.into(), data.to_vec()));
        let got = ch
            .stdout
            .take()
            .map(|so| crate::exec::read_all(so.into(), None))
            .unwrap_or_default();
        if let Some(f) = feeder {
            let _ = f.join();
        }
        let _ = ch.wait();
        got
    }

    /// events_show, the second step: HH:MM:SS.mmm EVENT>>DATA, or with --json {"time": SECONDS,
    /// "event": ..., "data": ...}, a line each.
    fn format(&self, kept: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut lines: Vec<&[u8]> = kept.split(|&c| c == b'\n').collect();
        if lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        for l in lines {
            out.extend_from_slice(&self.line(l));
        }
        out
    }

    fn line(&self, l: &[u8]) -> Vec<u8> {
        let sp = l.iter().position(|&c| c == b' ').unwrap_or(l.len());
        let ts = &l[..sp];
        let rest = l.get(sp + 1..).unwrap_or_default();
        if self.as_json {
            // capture("^(?<t>[0-9]+\.[0-9]+) (?<e>[^>]*)>>(?<d>.*)$"): jq -R reads the line as text.
            let s = String::from_utf8_lossy(l).into_owned();
            let Some((t, r)) = s.split_once(' ') else {
                return Vec::new();
            };
            let tok = t.split_once('.').is_some_and(|(a, b)| {
                !a.is_empty()
                    && !b.is_empty()
                    && a.bytes().all(|c| c.is_ascii_digit())
                    && b.bytes().all(|c| c.is_ascii_digit())
            });
            let Some(gt) = r.find('>') else {
                return Vec::new();
            };
            if !tok || !r[gt..].starts_with(">>") || s.contains('\n') {
                return Vec::new();
            }
            let (e, d) = (&r[..gt], &r[gt + 2..]);
            return format!(
                "{{\"time\":{},\"event\":{},\"data\":{}}}\n",
                json::number(t),
                json::quote(e),
                json::quote(d)
            )
            .into_bytes();
        }
        let secs: i64 = {
            let int: Vec<u8> = ts.iter().copied().take_while(u8::is_ascii_digit).collect();
            String::from_utf8_lossy(&int).parse().unwrap_or(0)
        };
        let frac = &ts[ts.iter().position(|&c| c == b'.').map_or(0, |i| i + 1)..];
        cat!(hms(secs), ".", frac, " ", rest, "\n")
    }

    fn show(&self, data: &[u8]) -> Vec<u8> {
        self.format(&self.keep(data))
    }
}

/// strftime("%H:%M:%S", T), local time.
pub(crate) fn hms(t: i64) -> String {
    // SAFETY: localtime_r fills plain data of ours.
    unsafe {
        let tt: libc::time_t = t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&tt, &mut tm);
        format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
}

/// events_size: the log's size now, where a mark taken now points.
fn events_size(ctx: &Ctx) -> Option<Vec<u8>> {
    let argv = vec![
        b"stat".to_vec(),
        b"-L".to_vec(),
        b"-c".to_vec(),
        b"%s".to_vec(),
        b"--".to_vec(),
        events_log(&ctx.d),
    ];
    let r = on_box(ctx, &argv, false, Out::Capture(None), ErrTo::Null);
    (r.rc == 0).then(|| shell::subst(&r.out))
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut since, mut mark, mut domark, mut follow) =
        (Vec::new(), Vec::new(), Vec::new(), false, false);
    let (mut until, mut tmo, mut n) = (Vec::new(), 10_000i64, b"all".to_vec());
    let mut sh = Show {
        re: Vec::new(),
        icase: false,
        as_json: false,
        min: 0,
    };
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let next = args.get(i + 1);
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, next)?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"--since" => {
                optarg(ctx, a, next)?;
                since = args[i + 1].clone();
                i += 1;
            }
            b"--mark" => {
                domark = true;
                if let Some(v) = next.filter(|v| !v.starts_with(b"-")) {
                    if !shell::re_match("^[A-Za-z][A-Za-z0-9_.-]{0,39}$", v) {
                        return Err(ctx.die(cat!(
                            "events: a mark's name starts with a letter (letters, digits, . _ -; 40 at most), got ",
                            v
                        )));
                    }
                    mark = v.clone();
                    i += 1;
                }
            }
            b"--grep" => {
                optarg(ctx, a, next)?;
                if args[i + 1] == b"-i" {
                    sh.icase = true;
                    i += 1;
                    optarg(ctx, &args[i], args.get(i + 1))?;
                }
                sh.re = args[i + 1].clone();
                i += 1;
            }
            b"-i" => sh.icase = true,
            b"--json" => sh.as_json = true,
            b"-f" | b"--follow" => follow = true,
            b"--until" => {
                optarg(ctx, a, next)?;
                until = args[i + 1].clone();
                i += 1;
            }
            b"--timeout" => {
                optarg(ctx, a, next)?;
                let v = &args[i + 1];
                let Some(ms) = ms_duration(v) else {
                    return Err(ctx.die("events: --timeout takes a duration: 500ms, 10s, 2m"));
                };
                if ms > WAIT_MAX_MS {
                    return Err(ctx.die(cat!("events: --timeout is at most 10m (got ", v, ")")));
                }
                tmo = ms;
                i += 1;
            }
            b"-n" => {
                optarg(ctx, a, next)?;
                if !shell::re_match("^([0-9]{1,7}|all)$", &args[i + 1]) {
                    return Err(ctx.die("events: -n takes a number of events, or all"));
                }
                n = args[i + 1].clone();
                i += 1;
            }
            b"-h" | b"--help" => return crate::help::usage("events"),
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "events", a)),
            _ => {
                return Err(unexpected(
                    ctx,
                    "events",
                    a,
                    "(events takes no word of its own: a mark's name goes after --mark or --since)",
                ));
            }
        }
        i += 1;
    }
    for re in [&sh.re, &until] {
        if !re.is_empty() && Re::from_bytes(re, false).is_none() {
            return Err(ctx.die(cat!("events: not a regular expression (as grep -E): ", re)));
        }
    }
    if domark && (!since.is_empty() || !sh.re.is_empty() || !until.is_empty() || follow) {
        let m = if mark.is_empty() {
            b"OFFSET".to_vec()
        } else {
            mark.clone()
        };
        return Err(ctx.die(cat!(
            "events: --mark goes alone (then --since ",
            m,
            " in the next call)"
        )));
    }
    if !until.is_empty() && follow {
        return Err(ctx.die("events: --until or -f, not both"));
    }
    select_box(ctx, &name)?;
    if !ctx.at("box.json").is_file() {
        need_box(ctx)?;
    }
    let rd = Reading {
        live: live(ctx)?,
        re: Vec::new(),
        icase: false,
    };
    // Its log is the events helper's (#421): one replaced on the host since up is said.
    if rd.live {
        crate::helpers::warn_stale(ctx);
    }
    if !rd.there(ctx, Kind::Box, &events_log(&ctx.d))? {
        return Err(ctx.die(cat!(
            "box '", ctx.name,
            "' records no events (it was started by an omabox that did not record them yet: omabox down ",
            ctx.name, ", then up)"
        )));
    }
    if domark {
        if !rd.live {
            need_box(ctx)?;
        }
        let Some(size) = events_size(ctx) else {
            return Err(ctx.die(cat!(
                "events: cannot read the size of box '",
                ctx.name,
                "''s events.log"
            )));
        };
        if mark.is_empty() {
            say(cat!("events: from byte ", size, " (--since ", size, ")"));
        } else {
            // The name as a field, not a regex (finding 176).
            let old = std::fs::read(ctx.at("events.marks")).unwrap_or_default();
            let mut kept = Vec::new();
            let mut lines: Vec<&[u8]> = old.split(|&c| c == b'\n').collect();
            if lines.last().is_some_and(|l| l.is_empty()) {
                lines.pop();
            }
            for l in lines {
                if shell::words(l).first().map_or(&b""[..], |w| w.as_slice()) != mark.as_slice() {
                    kept.extend_from_slice(&cat!(l, "\n"));
                }
            }
            kept.extend_from_slice(&cat!(mark, " ", size, "\n"));
            let _ = std::fs::write(ctx.at("events.marks.new"), kept);
            let _ = std::fs::rename(ctx.at("events.marks.new"), ctx.at("events.marks"));
            say(cat!(
                "events: mark '",
                mark,
                "' at byte ",
                size,
                " (--since ",
                mark,
                ")"
            ));
        }
        // --json (#273, audit J9): {mark (null without a name), offset}.
        if sh.as_json {
            let m = if mark.is_empty() {
                "null".to_string()
            } else {
                json::quote(&String::from_utf8_lossy(&mark))
            };
            return out(cat!(
                "{\"mark\":",
                m,
                ",\"offset\":",
                json::number(&String::from_utf8_lossy(&size)),
                "}\n"
            ));
        }
        return out(cat!(size, "\n"));
    }
    let mut off = 0i64;
    if !since.is_empty() {
        let (o, m) = events_since(ctx, &since)?;
        off = o;
        sh.min = m;
    }
    if (follow || !until.is_empty()) && !rd.live {
        if !until.is_empty() {
            say(cat!("box '", ctx.name, "' is down"));
            return Err(Exit(1));
        }
        say(cat!(
            "box '",
            ctx.name,
            "' is down: its events as they were"
        ));
        follow = false;
    }
    if !follow && until.is_empty() {
        let shown = sh.show(&events_from(ctx, &rd, off));
        let shown = match shell::test_int(&n).and_then(|k| usize::try_from(k).ok()) {
            Some(k) => tail_lines(&shown, k),
            None => shown,
        };
        return out(shown);
    }
    follow_events(ctx, &mut sh, since.is_empty(), off, follow, &until, tmo)
}

/// events_since MARK: (OFFSET, MIN_MS): a mark's name or a byte offset (MIN_MS 0), or a time ago such
/// as 30s, 2m or 1h (offset 0, the events from then on).
pub fn events_since(ctx: &Ctx, v: &[u8]) -> Res<(i64, i64)> {
    if shell::re_match("^[0-9]{1,15}$", v) {
        return Ok((shell::dec(v).unwrap_or(0), 0));
    }
    if let Some(re) = Re::new("^([0-9]{1,6})(ms|s|m|h)$") {
        if let (Some(k), Some(u)) = (re.group(v, 1), re.group(v, 2)) {
            let k = shell::dec(&k).unwrap_or(0);
            let ms = match u.as_slice() {
                b"ms" => k,
                b"s" => k * 1000,
                b"m" => k * 60_000,
                _ => k * 3_600_000,
            };
            return Ok((0, now_ms() - ms));
        }
    }
    if shell::re_match("^[A-Za-z][A-Za-z0-9_.-]{0,39}$", v) {
        let text = std::fs::read(ctx.at("events.marks")).unwrap_or_default();
        let mut o: Option<Vec<u8>> = None;
        for l in text.split(|&c| c == b'\n') {
            let w = shell::words(l);
            if w.first().is_some_and(|f| f.as_slice() == v) {
                o = Some(w.get(1).cloned().unwrap_or_default());
            }
        }
        return match o.filter(|o| !o.is_empty()) {
            Some(o) => Ok((shell::test_int(&o).unwrap_or(0), 0)),
            None => Err(ctx.die(cat!(
                "events: no mark '",
                v,
                "' in box '",
                ctx.name,
                "' (omabox events --mark ",
                v,
                " makes one)"
            ))),
        };
    }
    Err(ctx.die(cat!(
        "events: --since takes a mark's name, a byte offset or a time ago (30s, 2m, 1h), got ",
        v
    )))
}

/// events_from OFFSET: the log from byte OFFSET on, whole lines only.
fn events_from(ctx: &Ctx, rd: &Reading, from: i64) -> Vec<u8> {
    let size = if rd.live {
        events_size(ctx)
            .and_then(|s| shell::test_int(&s))
            .unwrap_or(0)
    } else {
        log_dead_path(ctx, &events_log(&ctx.d))
            .and_then(|p| std::fs::metadata(shell::path(&p)).ok())
            .map_or(0, |m| i64::try_from(m.len()).unwrap_or(0))
    };
    let mut from = from;
    if from > size {
        say(format!(
            "events: the mark is past the end of the log (it is {size} bytes): reading it all"
        ));
        from = 0;
    }
    if from == 0 {
        return rd.cat(ctx, Kind::Box, &events_log(&ctx.d));
    }
    let part = if rd.live {
        let argv = vec![
            b"tail".to_vec(),
            b"-c".to_vec(),
            format!("+{from}").into_bytes(),
            b"--".to_vec(),
            events_log(&ctx.d),
        ];
        on_box(ctx, &argv, false, Out::Capture(None), ErrTo::Inherit).out
    } else {
        let p = log_dead_path(ctx, &events_log(&ctx.d)).unwrap_or_default();
        crate::sig::command("tail")
            .arg("-c")
            .arg(format!("+{from}"))
            .arg("--")
            .arg(shell::path(&p))
            .output()
            .map(|o| o.stdout)
            .unwrap_or_default()
    };
    // (`tail -n +2`: from the second line)
    match part.iter().position(|&c| c == b'\n') {
        Some(i) => part[i + 1..].to_vec(),
        None => Vec::new(),
    }
}

/// -f, or --until RE: tail -F in the box's mount namespace from the mark (or from now), ending with
/// its PID 1.
#[allow(clippy::too_many_arguments)]
fn follow_events(
    ctx: &Ctx,
    sh: &mut Show,
    from_now: bool,
    off: i64,
    follow: bool,
    until: &[u8],
    tmo: i64,
) -> Res<()> {
    let pid = match box_pid(&ctx.d) {
        BoxPid::Pid(p) => p,
        _ => Vec::new(),
    };
    let off = if from_now {
        events_size(ctx)
            .and_then(|s| shell::test_int(&s))
            .unwrap_or(0)
    } else {
        off
    };
    // tail -c +1 is the whole file; from a mark, one byte early (the rest of the line before it first).
    let start = if off > 0 { off } else { 1 };
    let (r, w) = crate::exec::pipe().map_err(|_| Exit(1))?;
    crate::sig::follow_on();
    let argv = vec![
        b"tail".to_vec(),
        b"-c".to_vec(),
        format!("+{start}").into_bytes(),
        b"-F".to_vec(),
        cat!("--pid=", pid),
        b"--".to_vec(),
        events_log(&ctx.d),
    ];
    std::thread::scope(|sc| {
        // (follow_start's `<(...)`: SIGINT and SIGQUIT ignored in it)
        sc.spawn(move || {
            crate::exec::on_box_bg(
                ctx,
                &argv,
                crate::exec::In::Inherit,
                Out::Fd(w),
                ErrTo::Inherit,
                None,
            )
        });
        let mut lr = LineReader::new(r);
        if off != 0 {
            let _ = lr.read_line(Some(Duration::from_secs(5)));
        }
        if follow {
            let rc = follow_show(sh, &mut lr);
            if !box_alive(ctx)? {
                say(cat!("box '", ctx.name, "' went down"));
                return Ok(());
            }
            return crate::cmd::hyprctl::code(rc);
        }
        wait_until(ctx, sh, &mut lr, until, tmo)
    })
}

/// -f: each line as it comes, through events_show (with --grep, one awk for the stream).
fn follow_show(sh: &Show, lr: &mut LineReader) -> i32 {
    let mut stdout = std::io::stdout();
    if !sh.re.is_empty() {
        let mut c = sh.awk_cmd();
        c.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped());
        // (events_show's `&`)
        crate::exec::background(&mut c);
        let Ok(mut ch) = c.spawn() else { return 1 };
        crate::sig::follower(ch.id());
        let (Some(si), Some(so)) = (ch.stdin.take(), ch.stdout.take()) else {
            return 1;
        };
        let held = std::mem::take(&mut lr.buf);
        let fd = lr.fd.try_clone();
        let feeder = std::thread::spawn(move || {
            let mut si = si;
            let _ = si.write_all(&held);
            if let Ok(fd) = fd {
                let _ = std::io::copy(&mut std::fs::File::from(fd), &mut si);
            }
        });
        let mut kept = LineReader::new(OwnedFd::from(so));
        while let Line::Full(l) = kept.read_line(None) {
            let o = sh.format(&cat!(l, "\n"));
            if !o.is_empty() && stdout.write_all(&o).and_then(|()| stdout.flush()).is_err() {
                return 1;
            }
        }
        let _ = feeder.join();
        let _ = ch.wait();
        return 0;
    }
    loop {
        let l = match lr.read_line(None) {
            Line::Full(l) => cat!(l, "\n"),
            Line::Eof(l) if !l.is_empty() => cat!(l, "\n"),
            _ => return 0,
        };
        let o = sh.show(&l);
        if !o.is_empty() && stdout.write_all(&o).and_then(|()| stdout.flush()).is_err() {
            return 1;
        }
    }
}

/// --until RE: the first event from there on that matches it (and --grep, --since's time), or 124 at
/// --timeout, 1 when the box goes down first. Waiting counts as use, as `wait` does.
fn wait_until(ctx: &Ctx, sh: &Show, lr: &mut LineReader, until: &[u8], tmo: i64) -> Res<()> {
    let icase = sh.icase;
    let re_until = Re::from_bytes(until, icase);
    let re_grep = if sh.re.is_empty() {
        None
    } else {
        Re::from_bytes(&sh.re, icase)
    };
    let ts_re = Re::new(r"^[0-9]+\.[0-9]{3}$");
    let t0 = crate::settle::mono_ms();
    let mut used = Instant::now();
    let mut part = Vec::new();
    loop {
        let left = tmo - (crate::settle::mono_ms() - t0);
        if left <= 0 {
            say(format!(
                "events: no event matching {} in {:.2}s",
                String::from_utf8_lossy(until),
                tmo as f64 / 1000.0
            ));
            return Err(Exit(124));
        }
        let left = left.min(1000);
        match lr.read_line(Some(Duration::from_millis(
            u64::try_from(left).unwrap_or(0),
        ))) {
            Line::Full(l) => {
                let line = cat!(std::mem::take(&mut part), l);
                let sp = line.iter().position(|&c| c == b' ');
                let ts = sp.map_or(&line[..], |i| &line[..i]);
                let ev = sp.map_or(&line[..], |i| &line[i + 1..]);
                if !ts_re.as_ref().is_some_and(|r| r.is_match(ts)) {
                    continue;
                }
                if sh.min != 0 {
                    let (a, b) =
                        ts.split_at(ts.iter().position(|&c| c == b'.').unwrap_or(ts.len()));
                    let t = shell::dec(a).unwrap_or(0) * 1000 + shell::dec(&b[1..]).unwrap_or(0);
                    if t < sh.min {
                        continue;
                    }
                }
                if let Some(g) = &re_grep {
                    if !g.is_match(ev) {
                        continue;
                    }
                }
                if !re_until.as_ref().is_some_and(|r| r.is_match(ev)) {
                    continue;
                }
                return out(sh.show(&cat!(line, "\n")));
            }
            Line::Timeout(l) => {
                part.extend_from_slice(&l);
                if used.elapsed() >= Duration::from_secs(30) {
                    touch(&ctx.at("used"));
                    used = Instant::now();
                }
            }
            Line::Eof(_) => {
                say(cat!("box '", ctx.name, "' went down"));
                return Err(Exit(1));
            }
        }
    }
}

enum Line {
    Full(Vec<u8>),
    /// A read that timed out, with what it had of a line (the rest comes with the next).
    Timeout(Vec<u8>),
    Eof(Vec<u8>),
}

/// `IFS= read -r [-t SECS] line <&FD`: a line, its newline taken off (NUL bytes dropped, as read
/// does).
struct LineReader {
    fd: OwnedFd,
    buf: Vec<u8>,
    eof: bool,
}

impl LineReader {
    fn new(fd: OwnedFd) -> LineReader {
        LineReader {
            fd,
            buf: Vec::new(),
            eof: false,
        }
    }

    fn read_line(&mut self, timeout: Option<Duration>) -> Line {
        let end = timeout.map(|t| Instant::now() + t);
        loop {
            if let Some(i) = self.buf.iter().position(|&c| c == b'\n') {
                let l: Vec<u8> = self.buf.drain(..=i).take(i).filter(|&c| c != 0).collect();
                return Line::Full(l);
            }
            if self.eof {
                let l: Vec<u8> = self.buf.drain(..).filter(|&c| c != 0).collect();
                return Line::Eof(l);
            }
            let wait_ms = match end {
                Some(e) => {
                    let now = Instant::now();
                    if now >= e {
                        let l: Vec<u8> = self.buf.drain(..).filter(|&c| c != 0).collect();
                        return Line::Timeout(l);
                    }
                    i32::try_from((e - now).as_millis())
                        .unwrap_or(i32::MAX)
                        .max(1)
                }
                None => -1,
            };
            let mut p = libc::pollfd {
                fd: self.fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one pollfd of ours.
            let n = unsafe { libc::poll(&mut p, 1, wait_ms) };
            if n <= 0 {
                continue;
            }
            let mut tmp = [0u8; 8192];
            // SAFETY: tmp is ours, its length given.
            let got =
                unsafe { libc::read(self.fd.as_raw_fd(), tmp.as_mut_ptr().cast(), tmp.len()) };
            match got {
                0 => self.eof = true,
                g if g > 0 => self
                    .buf
                    .extend_from_slice(&tmp[..usize::try_from(g).unwrap_or(0)]),
                _ => {
                    if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                        self.eof = true;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shown() {
        let s = Show {
            re: Vec::new(),
            icase: false,
            as_json: true,
            min: 0,
        };
        assert_eq!(
            s.show(b"0001.500 a>>b>c\nx y\n12.5 nogt\n"),
            b"{\"time\":1.500,\"event\":\"a\",\"data\":\"b>c\"}\n".to_vec()
        );
        let s = Show {
            re: Vec::new(),
            icase: false,
            as_json: false,
            min: 1_700_000_000_500,
        };
        assert_eq!(
            s.keep(b"1700000000.400 a>>b\n1700000000.600 c>>d"),
            b"1700000000.600 c>>d\n"
        );
    }
}
