//! A box's command log (#447): one JSON line for each omabox command run against a box, written by
//! the front once the command has ended: when, the command and its words (secrets hidden), who ran
//! it, whether it only read the box, how long it took, how it ended. `omabox log commands` reads it.
//!
//! Where: `$XDG_RUNTIME_DIR/omabox/.commands/NAME.log`, beside the box dirs on the host. Not the box
//! dir itself, which `down` removes (and the line of a `down` is written after it), nor its HOME or
//! runtime dir, which the box can write: no box sees this folder. Bounded: a log that would pass
//! 256 KiB is renamed NAME.log.1 (the one before it dropped) and begun anew, so a name keeps at most
//! 512 KiB; the logs of boxes that are gone are dropped after 7 days, and past the newest 64.
//!
//! What is never written (S2): a value is logged only when its option is in KEEP (a box, a window, a
//! geometry, a time, a path, a name...); --env's and --detail's KEY=VALUE as KEY=<hidden>; any other
//! value, `keys -t`'s text, `--accept`'s password and an option this module has no word for, as its
//! length. A command's own words (a run's command line, a key combo, coordinates) are kept for the
//! commands in POS_KEEP, which are every box command now; a new command's are lengths until it is
//! added. An action after `shot --after --` or `record --` is redacted as the command it is. The
//! environment of a command is never read. A hidden value is taken out of the error line too.

use crate::args::cmd_opts;
use crate::cat;
use crate::shell;
use serde_json::{Value, json};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The options whose values are logged as given: none of them takes a typed text, a password or an
/// environment's value.
const KEEP: &[&str] = &[
    "-b",
    "--box",
    "-w",
    "--window",
    "-g",
    "-l",
    "--layer",
    "-n",
    "-o",
    "--output",
    "-s",
    "--action",
    "--addr",
    "--after",
    "--allow",
    "--answer",
    "--app",
    "--base",
    "--burst",
    "--cell",
    "--crf",
    "--cycles",
    "--delay",
    "--desktop",
    "--env-file",
    "--every",
    "--expect",
    "--expect-image",
    "--expect-tree",
    "--find",
    "--fit",
    "--for",
    "--fps",
    "--frames",
    "--from",
    "--glide",
    "--grep",
    "--grid",
    "--hold",
    "--hyprland",
    "--icon",
    "--idle",
    "--ignore",
    "--image",
    "--in",
    "--index",
    "--keep",
    "--keys",
    "--like",
    "--message",
    "--mod",
    "--monitor",
    "--name",
    "--net",
    "--no-seed",
    "--offset",
    "--omarchy",
    "--only",
    "--overlay",
    "--pass",
    "--pid",
    "--plugin",
    "--prompt",
    "--quickshell",
    "--quiet",
    "--report",
    "--report-scale",
    "--ro-bind",
    "--runs",
    "--seed",
    "--settle",
    "--shell",
    "--shot",
    "--shot-fit",
    "--shot-g",
    "--shot-monitor",
    "--shots",
    "--similar",
    "--since",
    "--size",
    "--source",
    "--speck",
    "--start",
    "--steps",
    "--theme",
    "--theme-dir",
    "--timeout",
    "--to",
    "--until",
    "--width",
    "--workspace",
    "--zoom",
];

/// The options whose value is KEY=VALUE: logged as KEY=<hidden>.
const NAMES: &[&str] = &["--env", "--detail"];

/// The options whose value is never logged, only its length (as every option not in KEEP or NAMES):
/// named so a test can tell a choice made from one forgotten.
#[cfg(test)]
const LENGTH: &[&str] = &["-t", "--type", "--accept"];

/// The commands whose own words (not an option's) are logged as given.
const POS_KEEP: &[&[u8]] = crate::front::BOX_CMDS;

/// The commands whose words, from their first that is no option of omabox's, are another program's
/// (eq_split's `front`): logged as given, an option among them too.
const FRONT: &[&[u8]] = &[b"run", b"pointer", b"hyprctl", b"lua"];

/// The commands whose words after `--` are an omabox command (an action): redacted as it.
const NESTED: &[&[u8]] = &[b"shot", b"screenshot", b"record"];

/// Each logged word's cut, and how many words a line keeps.
const WORD_CAP: usize = 512;
const WORDS_CAP: usize = 100;

/// The log's size before it is begun anew; how many logs of gone boxes are kept, and for how long.
const LOG_CAP: u64 = 256 * 1024;
const GONE_KEEP: usize = 64;
const GONE_SECS: u64 = 7 * 86_400;

pub struct Redacted {
    pub words: Vec<String>,
    /// The values hidden, as given (taken out of the error line too).
    pub secrets: Vec<Vec<u8>>,
    /// The command's first word that is no option's (its subcommand, for is_read).
    pub first: Option<Vec<u8>>,
    /// The options given.
    pub opts: Vec<Vec<u8>>,
}

fn lossy(v: &[u8]) -> String {
    let s = String::from_utf8_lossy(v);
    if s.len() <= WORD_CAP {
        return s.into_owned();
    }
    let mut at = WORD_CAP;
    while !s.is_char_boundary(at) {
        at -= 1;
    }
    format!("{}… (+{} bytes)", &s[..at], s.len() - at)
}

fn hidden(v: &[u8]) -> String {
    format!("<hidden: {} bytes>", v.len())
}

/// How OPT (one that takes a value) has value V logged, and the secret it hides.
fn value(opt: &[u8], v: &[u8]) -> (String, Option<Vec<u8>>) {
    let o = String::from_utf8_lossy(opt);
    if KEEP.contains(&o.as_ref()) {
        return (lossy(v), None);
    }
    if NAMES.contains(&o.as_ref()) {
        return match v.iter().position(|&c| c == b'=') {
            Some(i) => (
                format!("{}=<hidden>", lossy(&v[..i])),
                Some(v[i + 1..].to_vec()),
            ),
            None => (lossy(v), None),
        };
    }
    (hidden(v), Some(v.to_vec()))
}

/// The words of CMD as the log keeps them (ARGS as the front left them: -b NAME after the command,
/// --opt=value split).
pub fn redact(cmd: &[u8], args: &[Vec<u8>]) -> Redacted {
    let mut r = Redacted {
        words: Vec::new(),
        secrets: Vec::new(),
        first: None,
        opts: Vec::new(),
    };
    let table = cmd_opts(cmd).unwrap_or("");
    let kind = |o: &[u8]| -> Option<u8> {
        table.split_ascii_whitespace().find_map(|w| {
            let w = w.as_bytes();
            match w.last() {
                Some(&k @ (b'=' | b'?')) if &w[..w.len() - 1] == o => Some(k),
                _ if w == o => Some(b' '),
                _ => None,
            }
        })
    };
    let keep_pos = POS_KEEP.contains(&cmd);
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        i += 1;
        if a == b"--" {
            r.words.push("--".into());
            let rest = &args[i..];
            if NESTED.contains(&cmd) {
                nested(&mut r, rest);
            } else {
                for w in rest {
                    pos(&mut r, keep_pos, w);
                }
            }
            break;
        }
        if a.len() > 1 && a[0] == b'-' {
            r.opts.push(a.clone());
            match kind(a) {
                Some(b'=') => {
                    r.words.push(lossy(a));
                    if let Some(v) = args.get(i) {
                        let (shown, secret) = value(a, v);
                        r.words.push(shown);
                        r.secrets.extend(secret);
                        i += 1;
                    }
                }
                Some(_) => r.words.push(lossy(a)),
                // (an option the command does not know: a value written into it is hidden)
                None => match a.iter().position(|&c| c == b'=') {
                    Some(e) if a.starts_with(b"--") => {
                        r.words
                            .push(format!("{}={}", lossy(&a[..e]), hidden(&a[e + 1..])));
                        r.secrets.push(a[e + 1..].to_vec());
                    }
                    _ => r.words.push(lossy(a)),
                },
            }
            continue;
        }
        if r.first.is_none() {
            r.first = Some(a.clone());
        }
        if FRONT.contains(&cmd) {
            r.words.extend(args[i - 1..].iter().map(|w| lossy(w)));
            break;
        }
        pos(&mut r, keep_pos, a);
    }
    if r.words.len() > WORDS_CAP {
        let more = r.words.len() - WORDS_CAP;
        r.words.truncate(WORDS_CAP);
        r.words.push(format!("… (+{more} words)"));
    }
    r
}

fn pos(r: &mut Redacted, keep: bool, w: &[u8]) {
    if keep {
        r.words.push(lossy(w));
    } else {
        r.words.push(hidden(w));
        r.secrets.push(w.to_vec());
    }
}

/// An action after `--`: an omabox command, its words split as the front splits them, redacted as
/// it; anything else hidden.
fn nested(r: &mut Redacted, rest: &[Vec<u8>]) {
    let Some((c, words)) = rest.split_first() else {
        return;
    };
    let split = crate::exit::muted(|| {
        let ctx = crate::exit::Ctx::new(c, Vec::new());
        crate::front::eq_split(&ctx, c, words).ok()
    });
    match split {
        Some(words) if crate::front::BOX_CMDS.contains(&c.as_slice()) => {
            r.words.push(lossy(c));
            let n = redact(c, &words);
            r.words.extend(n.words);
            r.secrets.extend(n.secrets);
        }
        _ => {
            for w in rest {
                pos(r, false, w);
            }
        }
    }
}

/// Whether CMD (its words redacted as R) only reads the box: looks at it, waits on it, reads its logs.
pub fn is_read(cmd: &[u8], r: &Redacted) -> bool {
    let first = r.first.as_deref();
    let has = |o: &[u8]| r.opts.iter().any(|x| x == o);
    match cmd {
        b"shot" | b"screenshot" | b"windows" | b"tree" | b"pixel" | b"find" | b"jobs" | b"log"
        | b"logs" | b"events" | b"path" | b"env" | b"pid" | b"brief" | b"gpu" | b"cpu"
        | b"audio" | b"ports" | b"peek" | b"wait" | b"gdb" => true,
        // (#446: they read the box HOME; a snapshot's record is on the host side, not in the box)
        b"snapshot" | b"diff" => true,
        // (no kind: the box as it is; a kind starts, restarts or sets something in it)
        b"look" => first.is_none(),
        b"tray" => matches!(first, None | Some(b"menu")),
        b"monitor" => first == Some(b"list"),
        b"polkit" => first == Some(b"agents"),
        b"record" => matches!(first, Some(b"status" | b"sheet")),
        b"check" => matches!(first, Some(b"point" | b"wants" | b"diff")),
        b"dock" | b"dnd" | b"mode" | b"keys-to-box" => first.is_none() && !has(b"--clear"),
        b"window" => ![
            &b"--size"[..],
            b"--float",
            b"--tile",
            b"--fullscreen",
            b"--maximize",
        ]
        .iter()
        .any(|o| has(o)),
        _ => false,
    }
}

/// LINE with each hidden value taken out: one of 4 bytes or more replaced, a shorter one found
/// anywhere in it leaves out the whole line.
pub fn scrub(line: &[u8], secrets: &[Vec<u8>]) -> Vec<u8> {
    let mut l = line.to_vec();
    for s in secrets.iter().filter(|s| !s.is_empty()) {
        if !crate::exit::contains(&l, s) {
            continue;
        }
        if s.len() < 4 {
            return b"(left out: it may hold a hidden value)".to_vec();
        }
        let mut out = Vec::with_capacity(l.len());
        let mut k = 0;
        while k < l.len() {
            if l[k..].starts_with(s) {
                out.extend_from_slice(b"<hidden>");
                k += s.len();
            } else {
                out.push(l[k]);
                k += 1;
            }
        }
        l = out;
    }
    l
}

/// The command under way, set by `begin` for one about a box: logged by `end` (or a signal's).
struct Pending {
    boxes: Vec<u8>,
    cmd: Vec<u8>,
    args: Vec<Vec<u8>>,
    start: std::time::Instant,
    time: std::time::SystemTime,
}

static PENDING: Mutex<Option<Pending>> = Mutex::new(None);
/// The boxes this command selected (select_box), in order, each once: whether its box dir was there,
/// and whether it was the caller's then (`callers`).
static SELECTED: Mutex<Vec<(Vec<u8>, bool, bool)>> = Mutex::new(Vec::new());

/// While a command walks every box to read each (ports' table, down --orphans' search), the boxes it
/// passes are not boxes it was run against: no line of it in their logs (finding 736).
static WALKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A walk over every box begins (ON) or ends.
pub fn walking(on: bool) {
    WALKING.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// select_box picked box NAME, its box dir D.
pub fn selected(name: &[u8], d: &[u8]) {
    if WALKING.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    if let Ok(mut s) = SELECTED.lock()
        && !s.iter().any(|(n, _, _)| n == name)
    {
        s.push((name.to_vec(), shell::path(d).is_dir(), callers(d)));
    }
}

/// Whether the box at D may get the caller's line in its log: for a jailed caller (a broker's call),
/// only a box its jail started (its box.json names the jail), never the user's or another jail's
/// (S9); for anyone else, any.
fn callers(d: &[u8]) -> bool {
    !crate::jail::in_jail()
        || (shell::path(&cat!(d, "/box.json")).is_file()
            && crate::boxmeta::BoxMeta::load(d).jail == crate::jail::jail_id())
}

/// The front hands CMD ARGS to its command, called at START: kept for `end` when it is a command
/// about a box.
pub fn begin(boxes: &[u8], cmd: &[u8], args: &[Vec<u8>], start: std::time::Instant) {
    if !crate::front::BOX_CMDS.contains(&cmd) {
        return;
    }
    let time = std::time::SystemTime::now()
        .checked_sub(start.elapsed())
        .unwrap_or_else(std::time::SystemTime::now);
    if let Ok(mut p) = PENDING.lock() {
        *p = Some(Pending {
            boxes: boxes.to_vec(),
            cmd: cmd.to_vec(),
            args: args.to_vec(),
            start,
            time,
        });
    }
}

/// The command ended with RC (WHY: a signal's words; else the last line it said, when RC is not
/// 0): a line in the log of each box it selected that was there when it did, or is now, or that an
/// `up` was to make (not a name nothing has: a typo makes no log); or of the box `-b NAME` named,
/// when it failed before it selected one (a usage error) and that box is there.
pub fn end(rc: u8, why: Option<String>) {
    let Some(p) = PENDING.lock().ok().and_then(|mut p| p.take()) else {
        return;
    };
    let mut names: Vec<Vec<u8>> = SELECTED
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|(n, was, _)| {
            *was || p.cmd == b"up" || shell::path(&cat!(p.boxes, "/", n)).is_dir()
        })
        // (a jailed caller's: a box its jail had when it was selected (a down's), or has now (an up's))
        .filter(|(n, _, ours)| *ours || callers(&cat!(p.boxes, "/", n)))
        .map(|(n, _, _)| n)
        .collect();
    if names.is_empty()
        && let Some(n) = named_box(&p.args)
        && shell::path(&cat!(p.boxes, "/", n)).is_dir()
        && callers(&cat!(p.boxes, "/", n))
    {
        names.push(n);
    }
    if names.is_empty() {
        return;
    }
    let r = redact(&p.cmd, &p.args);
    let error = if rc == 0 {
        None
    } else {
        why.map(String::into_bytes).or_else(|| {
            let l = crate::exit::last_said();
            (!l.is_empty()).then_some(l)
        })
    };
    let error = error.map(|e| {
        let e = scrub(&e, &r.secrets);
        let e = shell::strip_nl(e.strip_prefix(b"omabox: ").unwrap_or(&e));
        String::from_utf8_lossy(&crate::term::plain_cap(&e, 300)).into_owned()
    });
    let caller = Caller::now();
    for n in names {
        let rec = record(&p, &n, &r, rc, error.as_deref(), &caller);
        append(
            &p.boxes,
            &n,
            cat!(crate::json::compact(&rec), "\n").as_slice(),
        );
    }
}

/// A jailed caller's command the broker refused before it ran (broker_side::check), with RC and WHY
/// (the refusal): its line, as `end` writes one, in the log of the box it is about (`-b NAME`, else
/// the jail's default), only when that box is there and the jail's own (`callers`): a name the jail
/// only typed makes no log, and the user's or another jail's box gets no line.
pub fn refused(boxes: &[u8], cmd: &[u8], args: &[Vec<u8>], rc: u8, why: &[u8]) {
    if !crate::jail::in_jail() || !crate::front::BOX_CMDS.contains(&cmd) {
        return;
    }
    let name = if args.iter().any(|a| a == b"-b" || a == b"--box") {
        named_box(args)
    } else {
        let n = crate::select::tame(&crate::select::default_name());
        n.first()
            .is_some_and(u8::is_ascii_alphanumeric)
            .then_some(n)
    };
    let Some(n) = name else { return };
    begin(boxes, cmd, args, std::time::Instant::now());
    selected(&n, &cat!(boxes, "/", n));
    end(rc, Some(String::from_utf8_lossy(why).into_owned()));
}

/// The box `-b NAME` names in ARGS, when it is written as a box name is.
fn named_box(args: &[Vec<u8>]) -> Option<Vec<u8>> {
    let k = args.iter().position(|a| a == b"-b" || a == b"--box")?;
    let n = args.get(k + 1)?;
    (crate::select::tame(n) == *n && n.first().is_some_and(u8::is_ascii_alphanumeric))
        .then(|| n.clone())
}

/// Who ran the command: omabox's own pid and its parent's, with the agent session; or a jailed
/// caller's, as the broker made sure of it, with its jail.
struct Caller {
    pid: Option<u64>,
    ppid: Option<u64>,
    session: Option<String>,
    jail: Option<String>,
}

impl Caller {
    fn now() -> Caller {
        let num = |v: Option<Vec<u8>>| {
            v.and_then(|v| shell::dec(&v))
                .and_then(|n| u64::try_from(n).ok())
        };
        if crate::jail::in_jail() {
            let pid = crate::broker_side::caller().map(|c| c.0.clone());
            let ppid = pid.as_deref().and_then(crate::procfs::proc_ppid);
            return Caller {
                pid: num(pid),
                ppid: num(ppid),
                session: None,
                jail: Some(String::from_utf8_lossy(&crate::jail::jail_id()).into_owned()),
            };
        }
        Caller {
            pid: Some(u64::from(std::process::id())),
            ppid: num(crate::procfs::proc_ppid(b"self")),
            session: crate::select::agent_session()
                .map(|s| String::from_utf8_lossy(&s).into_owned()),
            jail: None,
        }
    }
}

fn record(
    p: &Pending,
    name: &[u8],
    r: &Redacted,
    rc: u8,
    error: Option<&str>,
    c: &Caller,
) -> Value {
    let ms = p
        .time
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let took = p.start.elapsed().as_millis();
    let mut v = json!({
        "time": (ms as f64) / 1000.0,
        "box": String::from_utf8_lossy(name),
        "command": String::from_utf8_lossy(&p.cmd),
        "args": r.words,
        "read": is_read(&p.cmd, r),
        "exit": rc,
        "secs": (took as f64) / 1000.0,
        "pid": c.pid,
        "ppid": c.ppid,
        "jailed": c.jail.is_some(),
    });
    if let Some(o) = v.as_object_mut() {
        if let Some(s) = &c.session {
            o.insert("session".into(), json!(s));
        }
        if let Some(j) = &c.jail {
            o.insert("jail".into(), json!(j));
        }
        if let Some(e) = error {
            o.insert("error".into(), json!(e));
        }
    }
    v
}

/// Box NAME's log: `$BOXES/.commands/NAME.log`.
pub fn log_path(boxes: &[u8], name: &[u8]) -> PathBuf {
    shell::path(&cat!(boxes, "/.commands/", name, ".log"))
}

/// The log before it: `NAME.log.1`.
fn older(p: &Path) -> PathBuf {
    let mut o = p.as_os_str().to_owned();
    o.push(".1");
    PathBuf::from(o)
}

/// A file of the log opened to read or append: never through a link, never a FIFO's wait, a
/// regular file only.
pub fn open(p: &Path, append: bool) -> Option<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    if append {
        o.append(true).create(true).mode(0o600);
    } else {
        o.read(true);
    }
    let f = o
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(p)
        .ok()?;
    f.metadata().ok()?.is_file().then_some(f)
}

/// LINE at the end of box NAME's log, under a lock on it: a log it would take past LOG_CAP is
/// renamed `.1` first, and a log begun prunes those of gone boxes. Nothing said when it cannot be.
pub fn append(boxes: &[u8], name: &[u8], line: &[u8]) {
    if !shell::path(boxes).is_dir() {
        return;
    }
    let d = shell::path(&cat!(boxes, "/.commands"));
    match std::fs::symlink_metadata(&d) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return,
        Err(_) => {
            if std::fs::DirBuilder::new().mode(0o700).create(&d).is_err() {
                return;
            }
        }
    }
    let p = log_path(boxes, name);
    for _ in 0..4 {
        let begun = std::fs::symlink_metadata(&p).is_err();
        let Some(mut f) = open(&p, true) else {
            return;
        };
        // SAFETY: flock on an fd of ours; it waits for another omabox's append.
        if unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&f), libc::LOCK_EX) } != 0 {
            return;
        }
        let (Ok(held), Ok(now)) = (f.metadata(), std::fs::symlink_metadata(&p)) else {
            continue;
        };
        // (another omabox began it anew meanwhile: the one at the path now)
        if held.ino() != now.ino() || held.dev() != now.dev() {
            continue;
        }
        if held.len() > 0 && held.len() + line.len() as u64 > LOG_CAP {
            let _ = std::fs::rename(&p, older(&p));
            continue;
        }
        let _ = f.write_all(line);
        drop(f);
        if begun {
            prune(boxes, name);
        }
        return;
    }
}

/// The logs of boxes that are gone (no box dir), NAME's aside: those untouched for GONE_SECS, and
/// those past the newest GONE_KEEP.
fn prune(boxes: &[u8], name: &[u8]) {
    let Ok(rd) = std::fs::read_dir(shell::path(&cat!(boxes, "/.commands"))) else {
        return;
    };
    let now = std::time::SystemTime::now();
    let mut gone: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for e in rd.flatten() {
        let f = e.file_name().into_encoded_bytes();
        let Some(n) = f.strip_suffix(b".log") else {
            continue;
        };
        if n == name || shell::path(&cat!(boxes, "/", n)).exists() {
            continue;
        }
        let Ok(m) = e.metadata() else { continue };
        let t = m.modified().unwrap_or(now);
        if now
            .duration_since(t)
            .is_ok_and(|d| d.as_secs() >= GONE_SECS)
        {
            let _ = std::fs::remove_file(older(&e.path()));
            let _ = std::fs::remove_file(e.path());
        } else {
            gone.push((t, e.path()));
        }
    }
    gone.sort();
    let over = gone.len().saturating_sub(GONE_KEEP);
    for (_, p) in gone.into_iter().take(over) {
        let _ = std::fs::remove_file(older(&p));
        let _ = std::fs::remove_file(p);
    }
}

/// Box NAME's log as written, from FROM: the `.1` before it, then it. None when it has none.
pub fn read(boxes: &[u8], name: &[u8]) -> Option<Vec<u8>> {
    let p = log_path(boxes, name);
    let mut out = Vec::new();
    let mut any = false;
    for f in [older(&p), p] {
        if let Some(mut f) = open(&f, false) {
            any = true;
            let _ = std::io::Read::read_to_end(&mut f, &mut out);
        }
    }
    any.then_some(out)
}

/// The records of DATA (its whole lines); a jailed reader's (JAIL) only those of its jail.
pub fn records(data: &[u8], jail: Option<&[u8]>) -> Vec<Value> {
    crate::notices::records(data)
        .into_iter()
        .map(|(_, v)| v)
        .filter(|v| match jail {
            None => true,
            Some(j) => v.get("jail").and_then(Value::as_str).map(str::as_bytes) == Some(j),
        })
        .collect()
}

/// A word of a command line as text: quoted when it is empty or has a space, a quote or a backslash.
fn shown_word(w: &str) -> String {
    if !w.is_empty()
        && !w
            .chars()
            .any(|c| c.is_whitespace() || c == '\'' || c == '"' || c == '\\')
    {
        return w.to_string();
    }
    format!("'{}'", w.replace('\'', r"'\''"))
}

/// A record as text: `HH:MM:SS.mmm COMMAND ARGS (read; exit RC, S s; pid P, session S): ERROR`.
pub fn text(v: &Value) -> Vec<u8> {
    let t = v.get("time").and_then(Value::as_f64).unwrap_or(0.0);
    let ms = (t * 1000.0).round() as i64;
    let when = format!(
        "{}.{:03}",
        crate::cmd::events::hms(ms.div_euclid(1000)),
        ms.rem_euclid(1000)
    );
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default();
    let mut l = format!("{when} {}", s("command"));
    for w in v
        .get("args")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        l.push(' ');
        l.push_str(&shown_word(w.as_str().unwrap_or_default()));
    }
    l.push_str(" (");
    if v.get("read").and_then(Value::as_bool) == Some(true) {
        l.push_str("read; ");
    }
    let num = |k: &str| v.get(k).map(crate::json::raw).unwrap_or_default();
    l.push_str(&format!(
        "exit {}, {} s; pid {}",
        num("exit"),
        num("secs"),
        num("pid")
    ));
    if !s("session").is_empty() {
        l.push_str(&format!(", session {}", s("session")));
    }
    if v.get("jailed").and_then(Value::as_bool) == Some(true) {
        l.push_str(", jailed");
    }
    l.push(')');
    if !s("error").is_empty() {
        l.push_str(": ");
        l.push_str(s("error"));
    }
    crate::term::plain(l.as_bytes())
}

/// `log commands`' lines of RECS: as text or JSON, those whose text --grep's RE matches, the last N.
pub fn show(recs: &[Value], json: bool, re: &[u8], icase: bool, n: Option<usize>) -> Vec<u8> {
    let lines: Vec<Vec<u8>> = recs.iter().map(text).collect();
    let keep: Vec<usize> = if re.is_empty() {
        (0..recs.len()).collect()
    } else {
        crate::notices::grep_lines(re, icase, &lines)
    };
    let from = n.map_or(0, |n| keep.len().saturating_sub(n));
    let mut out = Vec::new();
    for &i in &keep[from..] {
        if json {
            out.extend_from_slice(crate::json::compact(&recs[i]).as_bytes());
        } else {
            out.extend_from_slice(&lines[i]);
        }
        out.push(b'\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &[&str]) -> Vec<Vec<u8>> {
        s.iter().map(|a| a.as_bytes().to_vec()).collect()
    }

    fn red(cmd: &str, a: &[&str]) -> Vec<String> {
        redact(cmd.as_bytes(), &w(a)).words
    }

    #[test]
    fn typed_text_is_its_length_only() {
        assert_eq!(
            red("keys", &["-b", "x", "-t", "hunter2", "Return", "-t", ""]),
            [
                "-b",
                "x",
                "-t",
                "<hidden: 7 bytes>",
                "Return",
                "-t",
                "<hidden: 0 bytes>"
            ]
        );
        let r = redact(b"keys", &w(&["-t", "hunter2"]));
        assert_eq!(r.secrets, w(&["hunter2"]));
    }

    #[test]
    fn env_and_details_by_name_only() {
        assert_eq!(
            red(
                "run",
                &[
                    "--env",
                    "TOKEN=s3cret",
                    "--pass",
                    "API_KEY",
                    "--env-file",
                    "/x/.env",
                    "--",
                    "sh",
                    "-c",
                    "echo $TOKEN"
                ]
            ),
            [
                "--env",
                "TOKEN=<hidden>",
                "--pass",
                "API_KEY",
                "--env-file",
                "/x/.env",
                "--",
                "sh",
                "-c",
                "echo $TOKEN"
            ]
        );
        assert_eq!(
            red(
                "up",
                &["b", "--env", "A=1", "--env", "LONE", "--size", "800x600"]
            ),
            [
                "b",
                "--env",
                "A=<hidden>",
                "--env",
                "LONE",
                "--size",
                "800x600"
            ]
        );
        assert_eq!(
            red(
                "polkit",
                &[
                    "ask",
                    "--accept",
                    "pw0rd",
                    "--detail",
                    "user=bob",
                    "--message",
                    "Hi"
                ]
            ),
            [
                "ask",
                "--accept",
                "<hidden: 5 bytes>",
                "--detail",
                "user=<hidden>",
                "--message",
                "Hi"
            ]
        );
        let r = redact(
            b"polkit",
            &w(&["ask", "--accept", "pw0rd", "--detail", "user=bob"]),
        );
        assert_eq!(r.secrets, w(&["pw0rd", "bob"]));
    }

    #[test]
    fn runs_command_line_is_kept() {
        // (from its first word, run's words are its program's: an --env there is that program's)
        assert_eq!(
            red("run", &["-d", "foot", "--env=X", "-t", "y"]),
            ["-d", "foot", "--env=X", "-t", "y"]
        );
        assert_eq!(
            red("hyprctl", &["-b", "x", "dispatch", "exec", "foot"]),
            ["-b", "x", "dispatch", "exec", "foot"]
        );
    }

    #[test]
    fn unlisted_values_are_hidden() {
        // An option this module has no word for, or one written --x=y the parser does not know.
        assert_eq!(
            red("keys", &["--acept=pw", "--json"]),
            ["--acept=<hidden: 2 bytes>", "--json"]
        );
        assert_eq!(
            red("nosuch", &["word", "--x", "v"]),
            ["<hidden: 4 bytes>", "--x", "<hidden: 1 bytes>"]
        );
    }

    #[test]
    fn an_action_after_dashes_is_redacted_as_its_command() {
        assert_eq!(
            red("record", &["-o", "v.mp4", "--", "keys", "-t", "secret"]),
            ["-o", "v.mp4", "--", "keys", "-t", "<hidden: 6 bytes>"]
        );
        assert_eq!(
            red(
                "shot",
                &[
                    "--burst",
                    "3",
                    "--after",
                    "--",
                    "polkit",
                    "ask",
                    "--accept=pw"
                ]
            ),
            [
                "--burst",
                "3",
                "--after",
                "--",
                "polkit",
                "ask",
                "--accept",
                "<hidden: 2 bytes>"
            ]
        );
        assert_eq!(
            red("shot", &["--after", "--", "nosuch", "x"]),
            ["--after", "--", "<hidden: 6 bytes>", "<hidden: 1 bytes>"]
        );
    }

    /// Every option that takes a value, of every command, has its way in the log chosen: kept,
    /// by name, or by length. (One left out would be logged by length: safe, but unthought.)
    #[test]
    fn every_value_option_is_chosen() {
        let mut unchosen = Vec::new();
        for c in crate::args::cmd_opts_keys() {
            for w in cmd_opts(c.as_bytes())
                .unwrap_or("")
                .split_ascii_whitespace()
            {
                let Some(o) = w.strip_suffix('=') else {
                    continue;
                };
                let n = [KEEP, NAMES, LENGTH]
                    .iter()
                    .filter(|l| l.contains(&o))
                    .count();
                if n != 1 {
                    unchosen.push(format!("{c} {o} ({n})"));
                }
            }
        }
        assert!(unchosen.is_empty(), "{unchosen:?}");
    }

    #[test]
    fn a_long_line_is_cut() {
        let long = "x".repeat(WORD_CAP + 10);
        let r = red("run", &["--", "echo", &long]);
        assert_eq!(r[2], format!("{}… (+10 bytes)", &long[..WORD_CAP]));
        let many: Vec<String> = (0..WORDS_CAP + 5).map(|i| i.to_string()).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        let r = red("keys", &many);
        assert_eq!(r.len(), WORDS_CAP + 1);
        assert_eq!(r[WORDS_CAP], "… (+5 words)");
    }

    #[test]
    fn reads_are_marked() {
        let rd = |c: &str, a: &[&str]| is_read(c.as_bytes(), &redact(c.as_bytes(), &w(a)));
        assert!(rd("shot", &["-o", "x.png"]));
        assert!(rd("windows", &[]));
        assert!(rd("tree", &["--window", "foot"]));
        assert!(rd("log", &["commands"]));
        assert!(rd("tray", &["menu", "x"]));
        assert!(!rd("tray", &["click", "x", "Quit"]));
        assert!(rd("monitor", &["list"]));
        assert!(!rd("monitor", &["add", "800x600"]));
        assert!(rd("dnd", &[]));
        assert!(!rd("dnd", &["on"]));
        assert!(!rd("dnd", &["--clear"]));
        assert!(rd("window", &["--json"]));
        assert!(!rd("window", &["--float"]));
        assert!(rd("record", &["status"]));
        assert!(!rd("record", &["start"]));
        assert!(rd("snapshot", &["before"]));
        assert!(rd("diff", &["before", "--ignore", ".config/x"]));
        for c in [
            "keys", "click", "run", "up", "down", "reload", "hyprctl", "lua", "plugin",
        ] {
            assert!(!rd(c, &["x"]), "{c}");
        }
    }

    #[test]
    fn hidden_values_leave_the_error_line() {
        let s = w(&["hunter2", "pw"]);
        assert_eq!(
            scrub(b"keys: no such key hunter2!", &s[..1]),
            b"keys: no such key <hidden>!"
        );
        assert_eq!(
            scrub(b"bad pw", &s),
            b"(left out: it may hold a hidden value)"
        );
        assert_eq!(scrub(b"all fine", &s), b"all fine");
    }

    fn scratch(tag: &str) -> Vec<u8> {
        let d = std::env::temp_dir().join(format!("omabox-cmdlog-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        shell::bytes(&d).to_vec()
    }

    #[test]
    fn appended_in_order_and_begun_anew_past_its_cap() {
        let b = scratch("cap");
        let line = |i: usize| format!("{{\"n\":{i},\"pad\":\"{}\"}}\n", "p".repeat(1000));
        for i in 0..600 {
            append(&b, b"x", line(i).as_bytes());
        }
        let p = log_path(&b, b"x");
        let cur = std::fs::metadata(&p).unwrap().len();
        let old = std::fs::metadata(older(&p)).unwrap().len();
        assert!(cur <= LOG_CAP && old <= LOG_CAP, "{cur} {old}");
        let recs = records(&read(&b, b"x").unwrap(), None);
        let ns: Vec<u64> = recs.iter().map(|v| v["n"].as_u64().unwrap()).collect();
        assert_eq!(*ns.last().unwrap(), 599);
        assert!(ns.windows(2).all(|p| p[1] == p[0] + 1), "in order");
        assert!(ns.len() > 256 && ns.len() < 520, "{} kept", ns.len());
        // (the folder and its files are the user's alone)
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(p.parent().unwrap()), 0o700);
        assert_eq!(mode(&p), 0o600);
        // A link in the log's place is neither written through nor read.
        let q = log_path(&b, b"y");
        let target = shell::path(&cat!(b, "/target"));
        std::fs::write(&target, "{\"theirs\":1}\n").unwrap();
        std::os::unix::fs::symlink(&target, &q).unwrap();
        append(&b, b"y", b"{\"n\":1}\n");
        assert_eq!(std::fs::read(&target).unwrap(), b"{\"theirs\":1}\n");
        assert_eq!(read(&b, b"y"), None);
        let _ = std::fs::remove_dir_all(shell::path(&b));
    }

    #[test]
    fn gone_boxes_logs_are_pruned() {
        let b = scratch("prune");
        std::fs::create_dir_all(shell::path(&cat!(b, "/live"))).unwrap();
        for i in 0..GONE_KEEP + 3 {
            append(&b, format!("g{i:03}").as_bytes(), b"{}\n");
            // (their times apart: the oldest go)
            let t = std::time::SystemTime::now() - std::time::Duration::from_secs(1000 - i as u64);
            let f = std::fs::File::options()
                .append(true)
                .open(log_path(&b, format!("g{i:03}").as_bytes()))
                .unwrap();
            f.set_modified(t).unwrap();
        }
        append(&b, b"live", b"{}\n");
        let old = log_path(&b, b"stale");
        append(&b, b"stale", b"{}\n");
        let f = std::fs::File::options().append(true).open(&old).unwrap();
        f.set_modified(
            std::time::SystemTime::now() - std::time::Duration::from_secs(GONE_SECS + 5),
        )
        .unwrap();
        append(&b, b"new", b"{}\n");
        let there = |n: &str| log_path(&b, n.as_bytes()).exists();
        assert!(there("live") && there("new"));
        assert!(!there("stale"), "a week untouched");
        assert!(
            !there("g000") && !there("g001"),
            "the oldest past {GONE_KEEP}"
        );
        assert!(there(&format!("g{:03}", GONE_KEEP + 2)));
        let _ = std::fs::remove_dir_all(shell::path(&b));
    }

    #[test]
    fn a_record_as_text() {
        let v: Value = serde_json::from_str(
            r#"{"time":100.25,"box":"b","command":"keys","args":["-t","<hidden: 3 bytes>","ctrl+a"],"read":false,"exit":1,"secs":0.5,"pid":42,"ppid":41,"jailed":false,"session":"abcd1234","error":"keys: no window\u001b[31m"}"#,
        )
        .unwrap();
        let t = String::from_utf8(text(&v)).unwrap();
        let (_, rest) = t.split_once(' ').unwrap();
        assert_eq!(
            rest,
            "keys -t '<hidden: 3 bytes>' ctrl+a (exit 1, 0.5 s; pid 42, session abcd1234): keys: no window?[31m"
        );
        assert!(t.starts_with(&format!("{}.250 ", crate::cmd::events::hms(100))));
        // A jail's reader: only its jail's records.
        let data = b"{\"n\":1}\n{\"n\":2,\"jail\":\"9 1\"}\n{\"n\":3,\"jail\":\"8 1\"}\n";
        assert_eq!(records(data, None).len(), 3);
        let mine = records(data, Some(b"9 1"));
        assert_eq!((mine.len(), &mine[0]["n"]), (1, &json!(2)));
    }
}
