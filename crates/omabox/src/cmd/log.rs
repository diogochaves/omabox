//! `omabox log [-b NAME] [LOG...|all] [-n 100|all] [--grep RE [-i]] [-f]` (issue #41, finding 107):
//! a box's logs, a live box's read inside it, a dead box's from the host (logs.rs); -f follows them
//! with tail -F, ending with the box. `log gdb --crash` (#233): only the thread a crash hit.

use crate::args::{optarg, unknown_opt, unknown_word};
use crate::boxes::{box_alive, need_box, take_over};
use crate::cat;
use crate::exec::{ErrTo, Out, on_box};
use crate::exit::{Ctx, Res, out, say};
use crate::logs::{LOG_NAMES, log_dead_path, log_host_path, log_where};
use crate::select::select_box;
use crate::shell;
use std::io::{BufRead, Write};
use std::os::unix::ffi::OsStrExt;
use std::process::{Command, Stdio};

/// Where a log is read: in the box (as it sees the path) or on the host (the box dir's own).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Box,
    Host,
}

/// What `log` reads and how: LOG_LIVE (the box is up), --grep's expression and -i.
pub struct Reading {
    pub live: bool,
    pub re: Vec<u8>,
    pub icase: bool,
}

impl Reading {
    /// log_there: whether log KIND PATH is there to read.
    pub fn there(&self, ctx: &Ctx, kind: Kind, p: &[u8]) -> Res<bool> {
        Ok(match (kind, self.live) {
            (Kind::Host, _) => shell::path(p).is_file(),
            (Kind::Box, true) => {
                let argv = vec![b"test".to_vec(), b"-f".to_vec(), p.to_vec()];
                on_box(ctx, &argv, false, Out::Inherit, ErrTo::Inherit)
                    .or_exit()?
                    .rc
                    == 0
            }
            (Kind::Box, false) => log_dead_path(ctx, p).is_some(),
        })
    }

    /// log_cat: log KIND PATH whole (`$(...)`'s input: a refusal said, nothing read).
    pub fn cat(&self, ctx: &Ctx, kind: Kind, p: &[u8]) -> Vec<u8> {
        let host = |p: &[u8]| {
            crate::sig::command("cat")
                .arg("--")
                .arg(std::ffi::OsStr::from_bytes(p))
                .stderr(Stdio::inherit())
                .output()
                .map(|o| o.stdout)
                .unwrap_or_default()
        };
        match (kind, self.live) {
            (Kind::Host, _) => host(p),
            (Kind::Box, true) => {
                let argv = vec![b"cat".to_vec(), b"--".to_vec(), p.to_vec()];
                on_box(ctx, &argv, false, Out::Capture(None), ErrTo::Inherit).out
            }
            (Kind::Box, false) => host(&log_dead_path(ctx, p).unwrap_or_default()),
        }
    }

    /// log_grep: the lines matching --grep (all without one), each ending in a newline.
    pub fn grep(&self, data: &[u8]) -> Vec<u8> {
        if self.re.is_empty() {
            return lines_ended(data);
        }
        let mut c = grep_cmd(&self.re, self.icase);
        c.stdin(Stdio::piped()).stdout(Stdio::piped());
        let Ok(mut child) = c.spawn() else {
            return Vec::new();
        };
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
        got
    }
}

/// What `hyprctl rollinglog` added to a hyprland.log read (#328, finding 566).
#[derive(Debug, PartialEq, Eq)]
pub enum Rolled {
    /// The file had every line rollinglog holds (or there was none to have).
    Current,
    /// The file's end overlapped rollinglog: the lines past it were added.
    Added,
    /// No overlap: the file is further behind than rollinglog reaches, so lines are in neither.
    Gap,
}

/// The box's hyprland.log with the lines Hyprland logged but has not written to it yet (#328,
/// finding 566): Hyprland writes its file through an 8 KiB buffer and never flushes it, while
/// `hyprctl rollinglog` holds the last 4 KiB it logged, whole lines, byte for byte as the file gets
/// them. Where the file's end is rollinglog's start, the rest of rollinglog is added; an overlap
/// shorter than rollinglog's first line is taken for chance (the file ends mid-line), not a match.
pub fn with_rolling(file: &[u8], roll: &[u8]) -> (Vec<u8>, Rolled) {
    if roll.is_empty() {
        return (file.to_vec(), Rolled::Current);
    }
    let first = roll
        .iter()
        .position(|&c| c == b'\n')
        .map_or(roll.len(), |i| i + 1);
    let k = (first..=roll.len())
        .rev()
        .find(|&k| file.ends_with(&roll[..k]));
    match k {
        Some(k) if k == roll.len() => (file.to_vec(), Rolled::Current),
        Some(k) => (cat!(file, &roll[k..]), Rolled::Added),
        None => {
            let nl: &[u8] = if file.is_empty() || file.ends_with(b"\n") {
                b""
            } else {
                b"\n"
            };
            (cat!(file, nl, roll), Rolled::Gap)
        }
    }
}

/// `hyprctl rollinglog` in a live box, within 2 s (empty when it does not answer: the file alone).
pub fn rolling(ctx: &Ctx) -> Vec<u8> {
    let argv: Vec<Vec<u8>> = ["timeout", "2", "hyprctl", "rollinglog"]
        .iter()
        .map(|w| w.as_bytes().to_vec())
        .collect();
    let r = on_box(ctx, &argv, true, Out::Capture(Some(1_048_576)), ErrTo::Null);
    if r.rc == 0 { r.out } else { Vec::new() }
}

/// `grep --line-buffered -E [-i] -- RE`.
fn grep_cmd(re: &[u8], icase: bool) -> Command {
    let mut c = crate::sig::command("grep");
    c.args(["--line-buffered", "-E"]);
    if icase {
        c.arg("-i");
    }
    c.arg("--").arg(std::ffi::OsStr::from_bytes(re));
    c
}

/// Whether grep -E takes RE (its status 2 is a bad expression): `log --grep`'s and `wait log`'s.
pub fn grep_ok(re: &[u8], icase: bool) -> bool {
    let mut c = grep_cmd(re, icase);
    // (grep --line-buffered is no different here: only its status 2 is read)
    let st = c.stdin(Stdio::null()).stdout(Stdio::null()).status();
    !st.as_ref().map_or(true, |s| s.code() == Some(2))
}

/// awk's `{ print }`: every line ending in a newline.
pub fn lines_ended(data: &[u8]) -> Vec<u8> {
    let mut v = data.to_vec();
    if !v.is_empty() && !v.ends_with(b"\n") {
        v.push(b'\n');
    }
    v
}

/// `tail -n N` of lines that each end in a newline.
pub fn tail_lines(data: &[u8], n: usize) -> Vec<u8> {
    if n == 0 {
        return Vec::new();
    }
    let body = data.strip_suffix(b"\n").unwrap_or(data);
    let mut count = 0;
    for (i, &c) in body.iter().enumerate().rev() {
        if c == b'\n' {
            count += 1;
            if count == n {
                return data[i + 1..].to_vec();
            }
        }
    }
    data.to_vec()
}

/// gdb_crashes: the gdb log's crashes (#233): for each, the --watch header, gdb's signal line and the
/// backtrace of the thread it hit (between `gdb --watch`'s two markers). Markers with no signal line
/// before them (the process exited, or was killed) are no crash.
pub fn gdb_crashes(text: &[u8]) -> Vec<u8> {
    let sig_re = shell::Re::new(r"^(Thread [0-9]+ .*|Program) received signal ");
    let (mut hdr, mut sig, mut on, mut k) = (Vec::new(), Vec::new(), false, 0);
    let mut out = Vec::new();
    let mut lines: Vec<&[u8]> = text.split(|&c| c == b'\n').collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    for l in lines {
        if l.starts_with(b"== omabox gdb --watch ") {
            hdr = l.to_vec();
            sig.clear();
            on = false;
        } else if sig_re.as_ref().is_some_and(|r| r.is_match(l)) {
            sig = l.to_vec();
        } else if l == b"== omabox: a fatal signal; the thread it hit:" {
            if sig.is_empty() {
                continue;
            }
            if k > 0 {
                out.push(b'\n');
            }
            k += 1;
            if !hdr.is_empty() {
                out.extend_from_slice(&cat!(hdr, "\n"));
            }
            out.extend_from_slice(&cat!(sig, "\n"));
            sig.clear();
            on = true;
        } else if l.starts_with(b"== omabox: every thread") || l.starts_with(b"== omabox: end") {
            on = false;
        } else if on && !l.iter().all(|c| matches!(c, b' ' | b'\t')) {
            out.extend_from_slice(&cat!(l, "\n"));
        }
    }
    out
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut follow, mut crash, mut n) = (Vec::new(), false, false, b"100".to_vec());
    let mut rd = Reading {
        live: false,
        re: Vec::new(),
        icase: false,
    };
    let mut srcs: Vec<Vec<u8>> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, args.get(i + 1))?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"-f" | b"--follow" => follow = true,
            b"--crash" => crash = true,
            // `--grep -i RE` as grep users type it (#130).
            b"--grep" => {
                optarg(ctx, a, args.get(i + 1))?;
                if args[i + 1] == b"-i" {
                    rd.icase = true;
                    i += 1;
                    optarg(ctx, &args[i], args.get(i + 1))?;
                }
                let v = &args[i + 1];
                if v.starts_with(b"-") {
                    return Err(ctx.die(cat!(
                        "log: --grep takes RE (-i goes before or after it; an RE starting with - as [-]...), got ",
                        v
                    )));
                }
                rd.re = v.clone();
                i += 1;
            }
            b"-i" => rd.icase = true,
            b"-n" => {
                optarg(ctx, a, args.get(i + 1))?;
                if !shell::re_match("^([0-9]{1,7}|all)$", &args[i + 1]) {
                    return Err(ctx.die("log: -n takes a number of lines, or all"));
                }
                n = args[i + 1].clone();
                i += 1;
            }
            b"-h" | b"--help" => return crate::help::usage("log"),
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "log", a)),
            _ => srcs.push(a.clone()),
        }
        i += 1;
    }
    if !rd.re.is_empty() && !grep_ok(&rd.re, rd.icase) {
        return Err(ctx.die(cat!(
            "log: --grep takes a regular expression (grep -E), got ",
            rd.re
        )));
    }
    let mut all: Vec<&str> = LOG_NAMES.to_vec();
    all.push("all");
    let choices = format!("({}, all)", LOG_NAMES.join(" "));
    for s in &srcs {
        if !all.iter().any(|c| c.as_bytes() == s.as_slice()) {
            return Err(unknown_word(ctx, "log", "log", s, &choices, &all));
        }
    }
    if srcs.is_empty() {
        srcs = vec![b"hyprland".to_vec()];
    }
    if srcs.iter().any(|s| s == b"all") {
        srcs = LOG_NAMES.iter().map(|s| s.as_bytes().to_vec()).collect();
    }
    if crash {
        if srcs != [b"gdb".to_vec()] {
            return Err(ctx.die("log: --crash is the gdb log's alone (omabox log gdb --crash)"));
        }
        if follow {
            return Err(ctx.die("log: --crash prints a crash written already, not -f"));
        }
    }
    select_box(ctx, &name)?;
    // A dead box's logs are what tells why: read them too (need_box says why there are none).
    if !ctx.at("box.json").is_file() {
        need_box(ctx)?;
    }
    rd.live = live(ctx)?;
    if follow && !rd.live {
        say(cat!("box '", ctx.name, "' is down: its logs as they were"));
        follow = false;
    }
    let mut picked: Vec<(Kind, Vec<u8>, Vec<u8>)> = Vec::new();
    for s in &srcs {
        let st = String::from_utf8_lossy(s);
        let Some(p) = log_where(ctx, &st) else {
            if srcs.len() > 1 {
                continue;
            }
            return Err(ctx.die(cat!("box '", ctx.name, "' has no ", s, " log yet")));
        };
        let kind = if s == b"box" || s == b"reap" {
            Kind::Host
        } else {
            Kind::Box
        };
        // A log still to come is followed from its first line (tail -F waits for it).
        if !rd.there(ctx, kind, &p)? {
            if srcs.len() > 1 {
                continue;
            }
            if !follow {
                return Err(ctx.die(cat!(
                    "box '",
                    ctx.name,
                    "' has no ",
                    s,
                    " log yet (",
                    log_host_path(ctx, &p),
                    ")"
                )));
            }
        }
        picked.push((kind, p, s.clone()));
    }
    if picked.is_empty() {
        return Err(ctx.die(cat!(
            "box '",
            ctx.name,
            "' has none of these logs yet: ",
            srcs.join(&b' ')
        )));
    }
    if follow {
        return follow_logs(ctx, &rd, &picked, &n);
    }
    let count = shell::test_int(&n).and_then(|v| usize::try_from(v).ok());
    let mut sep: &[u8] = b"";
    for (kind, p, s) in &picked {
        let text = if crash {
            let c = shell::subst(&gdb_crashes(&rd.cat(ctx, *kind, p)));
            if c.is_empty() {
                return Err(ctx.die(cat!(
                    "box '", ctx.name,
                    "' has no crash in its gdb log (omabox gdb --watch before the step that crashes it)"
                )));
            }
            let c = shell::subst(&rd.grep(&cat!(c, "\n")));
            match count {
                Some(k) => shell::subst(&tail_lines(&cat!(c, "\n"), k)),
                None => c,
            }
        } else {
            let mut data = rd.cat(ctx, *kind, p);
            // Hyprland's file lags what it logged (#328): the rest from its rolling log.
            if rd.live && s == b"hyprland" {
                let (d, how) = with_rolling(&data, &rolling(ctx));
                if how == Rolled::Gap {
                    say(
                        "log: Hyprland's log file is behind by more than its rolling log holds (it writes the file 8 KiB at a time): its last 4 KiB, from `hyprctl rollinglog`, are added, and the lines before them are in neither yet",
                    );
                }
                data = d;
            }
            let g = rd.grep(&data);
            shell::subst(&count.map_or(g.clone(), |k| tail_lines(&g, k)))
        };
        // Several: each under its name, as tail does; an empty one (or none matching) not at all.
        if picked.len() > 1 {
            if text.is_empty() {
                continue;
            }
            out(cat!(sep, "==> ", s, " <==\n"))?;
            sep = b"\n";
        }
        if !text.is_empty() {
            out(cat!(text, "\n"))?;
        }
    }
    Ok(())
}

/// LOG_LIVE: the box is up (and its use counted, as need_box does).
pub fn live(ctx: &Ctx) -> Res<bool> {
    if !box_alive(ctx)? {
        return Ok(false);
    }
    crate::whose::mark_used(ctx);
    if !take_over(ctx, false)? {
        need_box(ctx)?;
    }
    Ok(true)
}

/// -f: tail -F, which ends within a second of the box's PID 1 (--pid), the box's own logs followed
/// inside it and the box dir's here, into one stream; tail's "==> PATH <==" headers get the logs'
/// names, and with --grep only before a match. -n counts lines before --grep here.
fn follow_logs(ctx: &Ctx, rd: &Reading, picked: &[(Kind, Vec<u8>, Vec<u8>)], n: &[u8]) -> Res<()> {
    let pid = match crate::boxes::box_pid(&ctx.d) {
        crate::boxes::BoxPid::Pid(p) => p,
        _ => Vec::new(),
    };
    let from = if n == b"all" {
        b"+1".to_vec()
    } else {
        n.to_vec()
    };
    let mut tail = vec![
        b"tail".to_vec(),
        b"-F".to_vec(),
        cat!("--pid=", pid),
        b"-n".to_vec(),
        from,
    ];
    if picked.len() != 1 {
        tail.push(b"-v".to_vec());
    }
    let (host, inbox): (Vec<_>, Vec<_>) = picked.iter().partition(|(k, _, _)| *k == Kind::Host);
    let mut names: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    for (_, p, s) in picked {
        names.push((p.clone(), s.clone()));
    }
    let re = if rd.re.is_empty() {
        Vec::new()
    } else {
        cat!(rd.re, "|^==> .* <==$")
    };
    let (r, w) = crate::exec::pipe().map_err(|_| ctx.die("log: no pipe"))?;
    crate::sig::follow_on();
    let mut children = Vec::new();
    if !host.is_empty() {
        let mut argv = tail.clone();
        argv.push(b"--".to_vec());
        argv.extend(host.iter().map(|(_, p, _)| p.clone()));
        // (follow_start's `<(...)`, log_tails' `&`: SIGINT and SIGQUIT ignored in them)
        let fds = crate::exec::Fds {
            bg: true,
            ..crate::exec::Fds::default()
        };
        let mut c = crate::exec::command(&argv, fds);
        let wd = w.try_clone().map_err(|_| ctx.die("log: no pipe"))?;
        c.stdout(Stdio::from(wd)).stderr(Stdio::null());
        if let Ok(ch) = c.spawn() {
            crate::sig::follower(ch.id());
            children.push(ch);
        }
    }
    let rc = std::thread::scope(|sc| {
        if !inbox.is_empty() {
            let mut argv = tail.clone();
            argv.push(b"--".to_vec());
            argv.extend(inbox.iter().map(|(_, p, _)| p.clone()));
            if let Ok(wd) = w.try_clone() {
                sc.spawn(move || {
                    crate::exec::on_box_bg(
                        ctx,
                        &argv,
                        crate::exec::In::Inherit,
                        Out::Fd(wd),
                        ErrTo::Null,
                        None,
                    )
                });
            }
        }
        drop(w);
        show(r, &re, rd.icase, &names)
    });
    for mut c in children {
        let _ = c.wait();
    }
    if !box_alive(ctx)? {
        say(cat!("box '", ctx.name, "' went down"));
        return Ok(());
    }
    crate::cmd::hyprctl::code(rc)
}

/// The followed stream (through grep for --grep), its tail headers named, printed as it comes.
fn show(r: std::os::fd::OwnedFd, re: &[u8], icase: bool, names: &[(Vec<u8>, Vec<u8>)]) -> i32 {
    let mut grep = None;
    let src: Box<dyn std::io::Read> = if re.is_empty() {
        Box::new(std::fs::File::from(r))
    } else {
        let mut c = grep_cmd(re, icase);
        c.stdin(Stdio::from(r)).stdout(Stdio::piped());
        crate::exec::background(&mut c);
        let Ok(mut ch) = c.spawn() else { return 1 };
        crate::sig::follower(ch.id());
        let so = ch.stdout.take();
        grep = Some(ch);
        match so {
            Some(so) => Box::new(so),
            None => return 1,
        }
    };
    let mut lines = std::io::BufReader::new(src);
    let (mut h, mut blank) = (Vec::new(), 0);
    let mut stdout = std::io::stdout();
    let mut line = Vec::new();
    loop {
        line.clear();
        match lines.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let l = line.strip_suffix(b"\n").unwrap_or(&line);
        if l.starts_with(b"==> ") && l.ends_with(b" <==") && l.len() >= 8 {
            let f = &l[4..l.len() - 4];
            let shown = names
                .iter()
                .find(|(p, _)| p.as_slice() == f)
                .map_or(f, |(_, s)| s.as_slice());
            h = cat!("==> ", shown, " <==");
            blank = 0;
            continue;
        }
        if l.is_empty() {
            blank += 1;
            continue;
        }
        let mut o = Vec::new();
        if !h.is_empty() {
            o.extend_from_slice(&cat!(h, "\n"));
            h.clear();
        }
        o.extend(std::iter::repeat_n(b'\n', blank));
        blank = 0;
        o.extend_from_slice(&cat!(l, "\n"));
        if stdout.write_all(&o).and_then(|()| stdout.flush()).is_err() {
            return 1;
        }
    }
    if let Some(mut g) = grep {
        let _ = g.wait();
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tails() {
        assert_eq!(tail_lines(b"a\nb\nc\n", 2), b"b\nc\n");
        assert_eq!(tail_lines(b"a\nb\n", 5), b"a\nb\n");
        assert_eq!(tail_lines(b"a\n", 0), b"");
        assert_eq!(lines_ended(b"a\nb"), b"a\nb\n");
        assert_eq!(lines_ended(b""), b"");
    }

    // #328: the lines Hyprland has not written to its file yet, from its rolling log.
    #[test]
    fn rolled() {
        let f = b"a 1\nb 2\nc 3\nd 4\n";
        // (rollinglog holds the last lines: here the file's last two, and two more)
        assert_eq!(
            with_rolling(f, b"c 3\nd 4\ne 5\nf 6\n"),
            (b"a 1\nb 2\nc 3\nd 4\ne 5\nf 6\n".to_vec(), Rolled::Added)
        );
        assert_eq!(
            with_rolling(f, b"c 3\nd 4\n"),
            (f.to_vec(), Rolled::Current)
        );
        assert_eq!(with_rolling(f, b""), (f.to_vec(), Rolled::Current));
        // The file ends mid-line (its buffer was written at 8 KiB, not at a line's end).
        assert_eq!(
            with_rolling(b"a 1\nb 2\nc ", b"b 2\nc 3\nd 4\n"),
            (b"a 1\nb 2\nc 3\nd 4\n".to_vec(), Rolled::Added)
        );
        // No overlap: further behind than rollinglog reaches. The file's partial line ended.
        assert_eq!(
            with_rolling(b"a 1\nb 2\nc", b"x 8\ny 9\n"),
            (b"a 1\nb 2\nc\nx 8\ny 9\n".to_vec(), Rolled::Gap)
        );
        // An end that matches only part of rollinglog's first line is chance, not the overlap.
        assert_eq!(
            with_rolling(b"a 1\nDEB", b"DEBUG x\nDEBUG y\n"),
            (b"a 1\nDEB\nDEBUG x\nDEBUG y\n".to_vec(), Rolled::Gap)
        );
        // The same line many times: the longest overlap, nothing doubled.
        assert_eq!(
            with_rolling(b"z\nr\nr\n", b"r\nr\ns\n"),
            (b"z\nr\nr\ns\n".to_vec(), Rolled::Added)
        );
        assert_eq!(with_rolling(b"", b"a\n"), (b"a\n".to_vec(), Rolled::Gap));
    }

    // As t_unit_gdb_crashes checks bash's.
    #[test]
    fn crashes() {
        let t = b"== omabox gdb --watch Hyprland (pid 5) ==\nThread 1 \"Hyprland\" received signal SIGSEGV, Segmentation fault.\n== omabox: a fatal signal; the thread it hit:\n#0  0x1 in f ()\n\n#1  0x2 in g ()\n== omabox: every thread\n#0 other\n== omabox gdb --watch quickshell (pid 6) ==\n== omabox: a fatal signal; the thread it hit:\n#0 not a crash\n";
        assert_eq!(
            String::from_utf8_lossy(&gdb_crashes(t)),
            "== omabox gdb --watch Hyprland (pid 5) ==\nThread 1 \"Hyprland\" received signal SIGSEGV, Segmentation fault.\n#0  0x1 in f ()\n#1  0x2 in g ()\n"
        );
    }
}
