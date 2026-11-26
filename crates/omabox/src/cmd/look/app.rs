//! `look app` and `look page`: the program (Chromium on the page) as a job of the box's, its window
//! waited for, keys sent, shot (a page's contact sheet), its output (a page's console) read.

use super::*;

/// After a look app's program ended before its window, as E (`exited with status 126`), its log
/// TAIL: jobs::exit_hint's line for 126 and 127, as the look that runs it.
pub(super) fn look_exit_hint(e: &[u8], cmd: &[Vec<u8>], tail: &[u8]) -> Vec<Vec<u8>> {
    look_exit_hint_as(e, b"omabox look app --", cmd, tail)
}

/// look_exit_hint with RUN the look that runs the command again (`omabox look app --overlay DIR --`).
pub(super) fn look_exit_hint_as(
    e: &[u8],
    run: &[u8],
    cmd: &[Vec<u8>],
    tail: &[u8],
) -> Vec<Vec<u8>> {
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
pub(super) fn run_cwd_said(err: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
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
pub(super) fn look_exit_notes(
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
pub(super) fn pos(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The report's `window:` line, `CLASS "TITLE" WxH (LAYOUT)`. A page's window is called chromium,
/// and Chromium gives an --app window a class of its own (`chrome-<the page's path>-Default`) that
/// `--window chromium` does not select (#471): its address is said as the selector.
pub(super) fn report_window_line(
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
pub(super) fn stop_pages(name: &[u8]) {
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

/// The job's log LOG, `<box dir>/home/run-NS.log` as `run -d --print-log` names it: a file in the
/// box HOME, which the box writes, so it is read as S7 reads one (box_home_file: by the fd it opened
/// with no link followed and no FIFO waited on, a regular file only, its first READ_CAP bytes), never
/// by its path (finding 747). Empty when it is not such a file.
pub fn job_log(log: &[u8]) -> Vec<u8> {
    let Some(i) = log.windows(6).rposition(|w| w == b"/home/") else {
        return Vec::new();
    };
    let (d, file) = (&log[..i], &log[i + 6..]);
    if file.is_empty() || file.contains(&b'/') {
        return Vec::new();
    }
    crate::boxes::box_home_read(d, &String::from_utf8_lossy(file), crate::boxes::READ_CAP)
        .unwrap_or_default()
}

/// The job whose log is LOG: its pid, while `jobs` lists it.
pub(super) fn job_pid(name: &[u8], log: &[u8]) -> Option<i64> {
    let r = ob(&[b"jobs", b"-b", name, b"--json"]);
    let v: Value = serde_json::from_slice(&r.out).ok()?;
    v.as_array()?
        .iter()
        .find(|j| j.get("log").and_then(Value::as_str).map(str::as_bytes) == Some(log))
        .and_then(|j| j.get("pid").and_then(Value::as_i64))
}

/// How the job PID ended ("exited with status 3"), once it has; None: the box's latest job, the
/// look's own when it ended before its pid was read.
pub(super) fn job_end(name: &[u8], pid: Option<i64>) -> Option<Vec<u8>> {
    let p = pid.map(|p| p.to_string());
    let mut w: Vec<&[u8]> = vec![b"wait", b"-b", name, b"--json", b"--timeout", b"1s", b"job"];
    if let Some(p) = &p {
        w.push(p.as_bytes());
    }
    let r = ob(&w);
    let v: Value = serde_json::from_slice(&r.out).ok()?;
    (v.get("result").and_then(Value::as_str) == Some("satisfied")).then(|| {
        v.get("detail")
            .and_then(Value::as_str)
            .unwrap_or("ended")
            .as_bytes()
            .to_vec()
    })
}

pub(super) fn app(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
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
pub(super) fn page(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let mut a = parse(ctx, args, "page")?;
    let what = a.dir.take().unwrap_or_default();
    // A file, however it is written (#491): a path, a file:// URL, either with a #fragment or a
    // ?query the page reads. Its folder is what the box must have, so a URL is no way around that.
    let (url, file) = if Re::new("^https?://").is_some_and(|r| r.is_match(&what)) {
        (what, None)
    } else {
        let Some((f, tail)) = page_file(&what) else {
            return Err(ctx.die(cat!(
                "look page: no such file: ",
                what,
                " (a FILE, or an http:// or file:// URL)"
            )));
        };
        (cat!("file://", f, tail), Some(f))
    };
    let bind = file
        .as_ref()
        .map(|f| f[..f.iter().rposition(|&c| c == b'/').unwrap_or(0)].to_vec());
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
        if let Some(f) = &file {
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
    let r = page_in(ctx, &a, &name, me);
    if let Some(g) = guard {
        g.finish();
    }
    r
}

/// look page's FILE as a file here: its real path and what follows it for the page (a `#fragment`
/// or a `?query`, kept as written). WHAT is a path or a `file://` URL (`file://localhost/` too, its
/// `%XX` read); the whole of it first, a file's name may hold `#` or `?`. None: no such file.
pub(super) fn page_file(what: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let (path, url) = match what.strip_prefix(b"file://") {
        Some(p) => (p.strip_prefix(b"localhost").unwrap_or(p), true),
        None => (what, false),
    };
    if url && !path.starts_with(b"/") {
        return None;
    }
    let cut = path.iter().position(|&c| c == b'#' || c == b'?');
    let mut tries = vec![(path, &b""[..])];
    if let Some(i) = cut {
        tries.push((&path[..i], &path[i..]));
    }
    for (p, tail) in tries {
        let p = if url { percent_decode(p) } else { p.to_vec() };
        let f = crate::repo::readlink_f(&p).unwrap_or_default();
        if !f.is_empty() && shell::path(&f).is_file() {
            return Some((f, tail.to_vec()));
        }
    }
    None
}

/// An http(s) URL's host and port (80 or 443 when it names none), a host of letters, digits, dots
/// and dashes only: what a connection check may be made to. None for anything else.
pub(super) fn http_host_port(url: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let (rest, default) = match url.strip_prefix(b"http://") {
        Some(r) => (r, &b"80"[..]),
        None => (url.strip_prefix(b"https://")?, &b"443"[..]),
    };
    let end = rest
        .iter()
        .position(|&c| matches!(c, b'/' | b'?' | b'#'))
        .unwrap_or(rest.len());
    let auth = &rest[..end];
    let (host, port) = match auth.iter().rposition(|&c| c == b':') {
        Some(i) => (&auth[..i], &auth[i + 1..]),
        None => (auth, default),
    };
    let ok = !host.is_empty()
        && host
            .iter()
            .all(|&c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-')
        && !host.starts_with(b"-")
        && !port.is_empty()
        && port.len() <= 5
        && port.iter().all(u8::is_ascii_digit);
    ok.then(|| (host.to_vec(), port.to_vec()))
}

/// A URL path's `%XX` as bytes; anything else as written.
fn percent_decode(p: &[u8]) -> Vec<u8> {
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(p.len());
    let mut i = 0;
    while i < p.len() {
        match (
            p[i],
            p.get(i + 1).copied().and_then(hex),
            p.get(i + 2).copied().and_then(hex),
        ) {
            (b'%', Some(h), Some(l)) => {
                out.push(h * 16 + l);
                i += 3;
            }
            (c, _, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Chromium on $1 as an app window: the first of chromium, google-chrome-stable, chrome on the box's
/// PATH, its profile in the box HOME, its log (the page's console) on stderr.
pub(super) const CHROMIUM_SH: &str = r#"b=$(command -v chromium || command -v google-chrome-stable || command -v chrome) || { echo "fatal: no Chromium in the box (chromium, google-chrome-stable)" >&2; exit 127; }; exec "$b" --app="$1" --user-data-dir="$HOME/.cache/omabox-look-chromium" --enable-logging=stderr --no-first-run --no-default-browser-check --password-store=basic"#;

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

pub(super) fn app_in(ctx: &Ctx, a: &App, name: &[u8], me: u32) -> Res<()> {
    let mut l = Look::new(a.keep.then_some(name));
    start(ctx, a, name, me, &[], &mut l)?;
    let Some(j) = launch(ctx, a, name, &mut l, || {})? else {
        return l.finish();
    };
    let mut seen = Seen::default();
    if let Some(w) = &j.win {
        let sel = address(w).as_bytes();
        settle(a, name, sel, b"5s");
        seen.shot = one_shot(a, name, sel, b"5s", true, &mut l);
        seen.window = window_end(name, &j, w, false, &mut l);
    }
    job_report(ctx, a, name, &j, l, seen, |output, l| {
        l.problems.extend(problem_lines(&output));
        output
    })
}

/// A page's look: as an app's, its earlier Chromium stopped first in a box that was up, the keys
/// sent while --shots' shots are taken (a contact sheet), and its console (console_lines) as its
/// output, the console's errors as its problems.
pub(super) fn page_in(ctx: &Ctx, a: &App, name: &[u8], me: u32) -> Res<()> {
    let mut l = Look::new(a.keep.then_some(name));
    start(ctx, a, name, me, &[], &mut l)?;
    // An http(s) page nothing answers at is shot as Chromium's error page, which neither its log nor
    // its window says (#491, finding 761): one connection from the box first, no request sent.
    if let Some((host, port)) = a.cmd.last().and_then(|u| http_host_port(u)) {
        let r = ob(&[
            b"run",
            b"-b",
            name,
            b"--",
            b"timeout",
            b"3",
            b"bash",
            b"-c",
            b"exec 3<>\"/dev/tcp/$1/$2\"",
            b"_",
            &host,
            &port,
        ]);
        if !r.ok {
            l.problem(cat!(
                "nothing answers at ",
                host,
                ":",
                port,
                " from the box (no server there, or the box's network does not reach it: --net, --allow): the page did not load, the shot is the browser's error page"
            ));
        }
    }
    let prep = || {
        if a.own {
            stop_pages(name);
        }
    };
    let Some(j) = launch(ctx, a, name, &mut l, prep)? else {
        return l.finish();
    };
    let mut seen = Seen::default();
    if let Some(w) = &j.win {
        let sel = address(w).as_bytes();
        settle(a, name, sel, b"2s");
        if a.shots > 1 {
            (seen.shot, seen.sheet) = sheet_shots(a, name, sel, &mut l);
        } else {
            seen.shot = one_shot(a, name, sel, b"2s", false, &mut l);
        }
        seen.window = window_end(name, &j, w, true, &mut l);
    }
    job_report(ctx, a, name, &j, l, seen, |output, l| {
        let c = console_lines(&output);
        l.problems.extend(console_problems(&c));
        c.iter().flat_map(|l| cat!(l, "\n")).collect()
    })
}

/// A look's program, a job of the box's: its log (its output), its pid while `jobs` listed it, and
/// its window (None: it ended, or none came within --timeout, which the look's problems say).
pub(super) struct Job {
    log: Vec<u8>,
    pid: Option<i64>,
    win: Option<Value>,
}

/// What a look saw of its program's window: its shot, a page's contact sheet, its line.
#[derive(Default)]
pub(super) struct Seen {
    shot: Option<Vec<u8>>,
    sheet: Option<Vec<u8>>,
    window: Vec<u8>,
}

/// The program, a job of the box's (its log is its output), then its window, or its end, or the
/// timeout. In a box that was up: the same program still running from the last look is stopped first
/// (--replace; PREP runs before that start, after the windows there were read), and its window is the
/// job's, else one that was not there before it (never another program's). None: it ended at once,
/// and L holds its report.
pub(super) fn launch(
    ctx: &Ctx,
    a: &App,
    name: &[u8],
    l: &mut Look,
    prep: impl FnOnce(),
) -> Res<Option<Job>> {
    let before: Vec<String> = if a.own {
        addresses(&ob(&[b"windows", b"-b", name, b"--json"]).out)
    } else {
        Vec::new()
    };
    prep();
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
            l.problem(cat!("it ", end, ", before opening a window"));
            let output = job_log(&log);
            l.problems.extend(problem_lines(&output));
            l.notes.extend(look_exit_notes(
                &end,
                cwd.as_ref(),
                a.own,
                name,
                &a.cmd,
                &output,
            ));
            l.head("screenshot", b"none");
            l.output = Some(output);
            return Ok(None);
        }
        let _ = std::io::Write::write_all(&mut std::io::stderr(), &r.err);
        return Err(ctx.die("look: the program could not be started in the box"));
    }
    let log = shell::subst(&r.out);
    let pid = job_pid(name, &log);
    let start = Instant::now();
    let mut win = None;
    // (one that ended before its pid was read, as a script does at once, has ended: its end now,
    // not a wait for a window to the timeout)
    let mut ended = if pid.is_none() {
        job_end(name, None)
    } else {
        None
    };
    while ended.is_none() && start.elapsed() < a.timeout {
        win = window_of(&ob(&[b"windows", b"-b", name, b"--json"]).out, pid, &before);
        if win.is_some() {
            break;
        }
        if pid.is_some() && job_pid(name, &log).is_none() {
            ended = job_end(name, pid);
            break;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    if win.is_none() {
        l.problem(match &ended {
            Some(e) => cat!("it ", e, " before opening a window"),
            None => cat!(
                "no window within ",
                format!("{:.0}s", a.timeout.as_secs_f64()),
                " (--timeout for a slower one)"
            ),
        });
        // A script not executable (126) or named without ./ (127): the look that runs it (#471).
        if let Some(e) = &ended {
            let tail = job_log(&log);
            l.notes
                .extend(look_exit_notes(e, cwd.as_ref(), a.own, name, &a.cmd, &tail));
        }
    }
    Ok(Some(Job { log, pid, win }))
}

/// A window's selector: its address (`active` without one).
pub(super) fn address(w: &Value) -> &str {
    w.get("address").and_then(Value::as_str).unwrap_or("active")
}

/// `wait still` on the window SEL, for WAIT at most.
pub(super) fn still_cmd<'a>(name: &'a [u8], sel: &'a [u8], wait: &'a [u8]) -> [&'a [u8]; 8] {
    [
        b"wait",
        b"-b",
        name,
        b"--timeout",
        wait,
        b"still",
        b"--window",
        sel,
    ]
}

/// Before keys: the window's first frames drawn (a shell still starting drops what it is sent).
pub(super) fn settle(a: &App, name: &[u8], sel: &[u8], wait: &[u8]) {
    if !a.keys.is_empty() {
        ob(&still_cmd(name, sel, wait));
    }
}

/// `keys` with the look's keys to the window SEL.
pub(super) fn keys_cmd<'a>(a: &'a App, name: &'a [u8], sel: &'a [u8]) -> Vec<&'a [u8]> {
    let mut keys: Vec<&[u8]> = vec![b"keys", b"-b", name, b"--window", sel, b"--wait"];
    keys.extend(a.keys.iter().map(Vec::as_slice));
    keys
}

/// How the keys went: a failure but their --wait's timeout is a problem, and their warnings.
pub(super) fn keys_done(r: Ran, problems: &mut Vec<Vec<u8>>) {
    if !r.ok && r.code != 124 {
        problems.push(cat!("keys: ", shell::subst(&r.err)));
    }
    warnings(&r.err, problems);
}

/// The keys, --after, the window still (for WAIT; MOVING: one still moving said), then its shot.
pub(super) fn one_shot(
    a: &App,
    name: &[u8],
    sel: &[u8],
    wait: &[u8],
    moving: bool,
    l: &mut Look,
) -> Option<Vec<u8>> {
    if !a.keys.is_empty() {
        keys_done(ob(&keys_cmd(a, name, sel)), &mut l.problems);
    }
    if let Some(d) = &a.after {
        ob(&[b"wait", b"--json", b"for", d]);
    }
    let r = ob(&still_cmd(name, sel, wait));
    if r.code == 124 && moving {
        l.note(b"still changing after 5s: the shot is of a moving window".to_vec());
    }
    let mut sh: Vec<&[u8]> = vec![b"shot", b"-b", name, b"--window", sel];
    if let Some(o) = &a.out {
        sh.extend([b"-o".as_slice(), o]);
    }
    let r = ob(&sh);
    if r.ok {
        last_line(&r.out)
    } else {
        l.problem(cat!("shot: ", shell::subst(&r.err)));
        None
    }
}

/// A contact sheet: the keys (a held one too) sent while --shots' shots are taken over --after (2 s
/// by default), on real time and real frames. The last shot, and the sheet (None: each shot named
/// in the notes, in order).
pub(super) fn sheet_shots(
    a: &App,
    name: &[u8],
    sel: &[u8],
    l: &mut Look,
) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    let keys = keys_cmd(a, name, sel);
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
        if let Some(Ok(r)) = k.map(|k| k.join()) {
            keys_done(r, &mut l.problems);
        }
    });
    let shot = paths.last().cloned();
    let out_s = a.out.as_ref().map_or_else(
        || cat!(shot.clone().unwrap_or_default(), "-sheet.png"),
        |o| suffixed(o, "sheet"),
    );
    match contact_sheet(&paths, a.shots.min(4), &out_s) {
        Ok(p) => (shot, Some(p)),
        Err(why) => {
            l.note(cat!(
                "no contact sheet (",
                why,
                "): the shots are each named in order"
            ));
            for (n, p) in paths.iter().enumerate() {
                l.note(cat!(format!("shot {}: ", n + 1), p));
            }
            (shot, None)
        }
    }
}

/// The window W's line (a PAGE's names its selector), and the job's end once it opened (a crash
/// after its first frame): a problem.
pub(super) fn window_end(name: &[u8], j: &Job, w: &Value, page: bool, l: &mut Look) -> Vec<u8> {
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
    let line = report_window_line(page, &s("class"), &s("title"), &size, layout, address(w));
    if j.pid.is_some() && job_pid(name, &j.log).is_none() {
        if let Some(e) = j.pid.and_then(|p| job_end(name, Some(p))) {
            l.problem(cat!("it ", e, " after opening its window"));
        }
    }
    line
}

/// The end of an app's or a page's look: in a box that stays up the program does too (said, with
/// what stops it); its log read (job_log), made the report's output by OUTPUT; what was SEEN; the
/// report.
pub(super) fn job_report(
    ctx: &Ctx,
    a: &App,
    name: &[u8],
    j: &Job,
    mut l: Look,
    seen: Seen,
    output: impl FnOnce(Vec<u8>, &mut Look) -> Vec<u8>,
) -> Res<()> {
    if a.own {
        if let Some(p) = j.pid.filter(|_| job_pid(name, &j.log).is_some()) {
            l.note(cat!(
                "left running in the box: omabox stop -b ",
                name,
                " ",
                p.to_string()
            ));
        }
    }
    let output = output(job_log(&j.log), &mut l);
    l.head("screenshot", seen.shot.as_deref().unwrap_or(b"none"));
    if let Some(sh) = &seen.sheet {
        l.head("sheet", sh);
    }
    if !seen.window.is_empty() {
        l.head("window", &seen.window);
    }
    l.own_box(ctx, a.own, name);
    l.output = Some(output);
    l.finish()
}

/// PATH with -SUFFIX before its extension (`/x/a.png`, `panel`: `/x/a-panel.png`).
/// look --shots N's contact sheet of PATHS (COLS a row) at OUT (finding 749): each shot read as the
/// regular file it is (no link followed: a shot's -o may be in a box HOME) into a folder of omabox's
/// own, montaged there by magick as PNGs (`png:`, no coder taken from a name), and the sheet put at
/// OUT in its folder held, made and renamed there: never through a link at OUT or OUT.part, never
/// over a file that is not an image. OUT resolved, or the words for why there is none.
pub(super) fn contact_sheet(paths: &[Vec<u8>], cols: u32, out: &[u8]) -> Result<Vec<u8>, Vec<u8>> {
    use crate::shots::{OutFile, open_regular, png_arg};
    let to = OutFile::at(out, true)?;
    let magick = || b"ImageMagick's magick montage".to_vec();
    let Ok((_, tmp)) = shell::mktemp_args(&[b"-d"]) else {
        return Err(magick());
    };
    let r = (|| {
        let mut args: Vec<Vec<u8>> = vec![b"montage".to_vec()];
        for (n, p) in paths.iter().enumerate() {
            // (each in a folder of its own, under its own name: magick's %t label is its name)
            let d = cat!(tmp, "/", (n + 1).to_string());
            let f = cat!(d, "/", shell::basename(p));
            let mut src = open_regular(p).ok_or_else(|| cat!("cannot read the shot ", p))?;
            std::fs::create_dir(shell::path(&d))
                .and_then(|()| std::fs::File::create_new(shell::path(&f)))
                .and_then(|mut dst| std::io::copy(&mut src, &mut dst))
                .map_err(|_| magick())?;
            args.push(png_arg(&f));
        }
        let made = cat!(tmp, "/sheet.png");
        args.extend(
            [
                "-tile".to_string(),
                format!("{cols}x"),
                "-geometry".into(),
                "480x+4+4".into(),
                "-label".into(),
                "%t".into(),
            ]
            .map(String::into_bytes),
        );
        args.push(png_arg(&made));
        let mut m = std::process::Command::new("magick");
        m.args(args.iter().map(|a| OsStr::from_bytes(a)))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if !m.status().is_ok_and(|st| st.success()) {
            return Err(magick());
        }
        let mut f = open_regular(&made).ok_or_else(magick)?;
        to.put(&mut f)?;
        Ok(to.path())
    })();
    let _ = std::fs::remove_dir_all(shell::path(&tmp));
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::look::tests::w;

    /// Finding 749: look --shots' sheet never written through a link at its name, a shot read
    /// through none (a box's -o), a text file at the name kept; else made, a PNG.
    #[test]
    #[ignore = "runs magick: run by test/run.sh (cargo test -- --ignored)"]
    fn contact_sheet_through_no_link() {
        let t = std::env::temp_dir().join(format!("omabox-lsheet-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        std::fs::create_dir_all(&t).unwrap();
        let p = |n: &str| shell::bytes(&t.join(n)).to_vec();
        for n in ["a.png", "b.png"] {
            let ok = std::process::Command::new("magick")
                .args(["-size", "8x8", "xc:red"])
                .arg(format!("png:{}", t.join(n).display()))
                .status()
                .is_ok_and(|s| s.success());
            assert!(ok, "magick made {n}");
        }
        let secret = t.join("secret");
        std::fs::write(&secret, "keep").unwrap();
        let shots = vec![p("a.png"), p("b.png")];
        std::os::unix::fs::symlink(&secret, t.join("s1.png")).unwrap();
        let e = contact_sheet(&shots, 2, &p("s1.png")).unwrap_err();
        assert!(e.ends_with(
            b" is a link: omabox writes through none (name the file itself, or a new path)"
        ));
        std::os::unix::fs::symlink(&secret, t.join("s2.png.part")).unwrap();
        assert!(contact_sheet(&shots, 2, &p("s2.png")).is_err());
        std::fs::write(t.join("s3.png"), "text").unwrap();
        assert!(contact_sheet(&shots, 2, &p("s3.png")).is_err());
        std::os::unix::fs::symlink(&secret, t.join("c.png")).unwrap();
        let e = contact_sheet(&[p("a.png"), p("c.png")], 2, &p("s4.png")).unwrap_err();
        assert!(
            e.starts_with(b"cannot read the shot "),
            "a shot that is a link"
        );
        assert_eq!(std::fs::read_to_string(&secret).unwrap(), "keep");
        assert_eq!(std::fs::read_to_string(t.join("s3.png")).unwrap(), "text");
        assert_eq!(contact_sheet(&shots, 2, &p("s5.png")), Ok(p("s5.png")));
        assert!(crate::shots::png_size(&p("s5.png")).is_some());
        let _ = std::fs::remove_dir_all(&t);
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

    /// Finding 747: the job's log is read from the box HOME's fd, never by its path: a link there to
    /// a host file reads nothing, a FIFO does not hang the look, a path not in a box HOME is not read.
    #[test]
    fn a_jobs_log_is_read_as_the_boxs() {
        let d = std::env::temp_dir().join(format!("omabox-joblog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("home")).unwrap();
        std::fs::write(d.join("secret"), "host secret\n").unwrap();
        std::fs::write(d.join("home/run-1.log"), "hello\n").unwrap();
        std::os::unix::fs::symlink(d.join("secret"), d.join("home/run-2.log")).unwrap();
        let c = std::ffi::CString::new(d.join("home/run-3.log").as_os_str().as_encoded_bytes())
            .unwrap();
        // SAFETY: a path of ours, NUL-terminated.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let log = |n: &str| cat!(shell::bytes(&d), "/home/", n);
        let (l1, l2, l3) = (log("run-1.log"), log("run-2.log"), log("run-3.log"));
        let s = shell::bytes(&d.join("secret")).to_vec();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send((job_log(&l1), job_log(&l2), job_log(&l3), job_log(&s)));
        });
        let r = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the look waited on a FIFO");
        assert_eq!(r.0, b"hello\n");
        assert_eq!(r.1, b"", "a link to a host file");
        assert_eq!(r.2, b"", "a FIFO");
        assert_eq!(r.3, b"", "not a file of a box HOME");
        let _ = std::fs::remove_dir_all(&d);
    }
}
