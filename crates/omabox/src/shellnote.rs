//! The box's Omarchy shell mid-session (#147), as shot, wait, windows, tree and pixel say it first
//! (shell_note): gone, hung (#209, finding 289), crashed since the last command said so, or killed
//! and started again by its launcher (#303, finding 408). As bin/omabox's shell_note, shell_state,
//! shell_crashes, shell_relaunches and what they call.

use crate::boxes::{BoxSys, RealBox, box_home_file, meta, run_read, slot_shell};
use crate::cat;
use crate::exec::{ErrTo, Out};
use crate::exit::{Ctx, Res, say};
use crate::jobs::job_how;
use crate::repo::readlink_e;
use crate::shell::{self, strip_nl};

fn words(v: &[&[u8]]) -> Vec<Vec<u8>> {
    v.iter().map(|w| w.to_vec()).collect()
}

/// box_qs: the box's Quickshell (`up --quickshell`'s build, finding 358), else the system's.
pub fn box_qs(ctx: &Ctx) -> Vec<u8> {
    let q = meta(&ctx.d, "quickshell").0;
    if q.is_empty() {
        b"/usr/bin/quickshell".to_vec()
    } else {
        q
    }
}

/// shell_pid: the shell's pid in the box (share/shell.sh writes it), or None.
pub fn shell_pid(ctx: &Ctx) -> Option<Vec<u8>> {
    let p = strip_nl(&run_read(&ctx.d, "omabox-shell.pid")?);
    shell::is_num(&p).then_some(p)
}

/// shell_running PID: a process in the box, and not one that exited and was not reaped yet.
pub fn shell_running_with<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx, pid: &[u8]) -> bool {
    let r = sys.on_box(ctx, &words(&[b"cat", &cat!("/proc/", pid, "/stat")]));
    r.rc == 0 && !crate::exit::contains(&shell::subst(&r.out), b") Z ")
}

/// The box's runtime dir's file naming the slot share/shell.sh started a plain box's shell under
/// (#438): CMD runs with `OMABOX_SHELL_SLOT=` it, and so does everything CMD starts, whatever it does
/// with its session or its parent (a launcher that starts the shell and exits, `qs -d`'s setsid).
pub const SHELL_SLOT: &str = "omabox-shell.slot";

/// The box's processes started under slot $1 (their environment as they were started has
/// `OMABOX_SHELL_SLOT=$1`), a pid a line; a Quickshell crash dialog (`__QUICKSHELL_CRASH_DUMP_PID`)
/// is the crash's, not the shell. A zombie's environment reads empty: none. In the box, run by omabox
/// with a clean environment, so the grep itself is never one.
const SLOT_PIDS_SH: &str = r#"/usr/bin/grep -lsFxz -e "OMABOX_SHELL_SLOT=$1" /proc/[0-9]*/environ |
  while IFS= read -r f; do
    /usr/bin/grep -qsz '^__QUICKSHELL_CRASH_DUMP_PID=' "$f" || { f=${f#/proc/}; echo "${f%/environ}"; }
  done"#;

/// shell_slot: a plain or desk box's shell's slot (SHELL_SLOT; letters, digits, `.`, `_`, `-`), None
/// on an Omarchy box or one whose shell.sh wrote none.
pub fn shell_slot(ctx: &Ctx) -> Option<Vec<u8>> {
    if !slot_shell(&ctx.d) {
        return None;
    }
    let s = strip_nl(&run_read(&ctx.d, SHELL_SLOT)?);
    (!s.is_empty()
        && s.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-')))
    .then_some(s)
}

/// shell_slot_pids SLOT: the box's processes of that slot (SLOT_PIDS_SH), in pid order.
pub fn shell_slot_pids_with<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx, slot: &[u8]) -> Vec<Vec<u8>> {
    let r = sys.on_box(
        ctx,
        &words(&[b"/usr/bin/sh", b"-c", SLOT_PIDS_SH.as_bytes(), b"sh", slot]),
    );
    let mut v: Vec<(i64, Vec<u8>)> = r
        .out
        .split(|&c| c == b'\n')
        .filter(|l| shell::is_num(l))
        .filter_map(|l| Some((shell::dec(l)?, l.to_vec())))
        .collect();
    v.sort();
    v.dedup();
    v.into_iter().map(|(_, p)| p).collect()
}

/// shell_live_pid: the box's shell's process while it runs: its recorded pid (shell_pid) while that
/// runs; on a plain box (#438), else the first process left of its slot (CMD a launcher that started
/// the shell and left). None: the shell is gone.
pub fn shell_live_pid(ctx: &Ctx) -> Option<Vec<u8>> {
    shell_live_pid_with(&RealBox, ctx)
}

pub fn shell_live_pid_with<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx) -> Option<Vec<u8>> {
    if let Some(p) = shell_pid(ctx).filter(|p| shell_running_with(sys, ctx, p)) {
        return Some(p);
    }
    let slot = shell_slot(ctx)?;
    shell_slot_pids_with(sys, ctx, &slot).into_iter().next()
}

/// shell_pids: a plain box's shell's processes (#438): its slot's and its recorded pid while that
/// runs, in pid order; None when it has no slot (an Omarchy box, or a shell.sh that wrote none).
pub fn shell_pids_with<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx) -> Option<Vec<Vec<u8>>> {
    let slot = shell_slot(ctx)?;
    let mut v = shell_slot_pids_with(sys, ctx, &slot);
    if let Some(p) = shell_pid(ctx).filter(|p| !v.contains(p) && shell_running_with(sys, ctx, p)) {
        v.push(p);
        v.sort_by_key(|p| shell::dec(p));
    }
    Some(v)
}

/// shell_hung PID: no answer over its IPC in 2 s, from a shell up for 10 s or more.
pub fn shell_hung(ctx: &Ctx, pid: &[u8]) -> bool {
    shell_hung_with(&RealBox, ctx, pid)
}

fn shell_hung_with<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx, pid: &[u8]) -> bool {
    let qs = box_qs(ctx);
    let r = sys.on_box_io(
        ctx,
        &words(&[b"timeout", b"2", &qs, b"ipc", b"--pid", pid, b"show"]),
        Out::Null,
        ErrTo::Null,
    );
    if r.rc != 124 {
        return false;
    }
    // Its age in s: /proc/uptime less its start (stat's field 22, in clock ticks).
    let age = sys.on_box(
        ctx,
        &words(&[
            b"sh",
            b"-c",
            br#"s=$(cat "/proc/$1/stat") || exit 1; set -- ${s##*") "}; t=$(getconf CLK_TCK) || t=100
    read -r up _ < /proc/uptime; echo $(( ${up%.*} - ${20} / t ))"#,
            b"sh",
            pid,
        ]),
    );
    if age.rc != 0 {
        return false;
    }
    let a = shell::subst(&age.out);
    shell::is_num(&a) && shell::test_int(&a).is_some_and(|n| n >= 10)
}

/// shell_state: none (--no-shell), running, hung, restarting (its launcher about to start it again)
/// or gone.
pub fn shell_state(ctx: &Ctx) -> &'static str {
    shell_state_given(ctx, "")
}

/// shell_state GIVEN: with GIVEN `hung` or `ok`, what shell_hung said already (`ls` asks every box's
/// at once), not asked again; "" asks.
pub fn shell_state_given(ctx: &Ctx, given: &str) -> &'static str {
    shell_state_with(&RealBox, ctx, given)
}

fn shell_state_with<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx, given: &str) -> &'static str {
    if meta(&ctx.d, "shell").0 != b"true" {
        return "none";
    }
    if let Some(p) = shell_live_pid_with(sys, ctx) {
        return if given == "hung" || (given.is_empty() && shell_hung_with(sys, ctx, &p)) {
            "hung"
        } else {
            "running"
        };
    }
    let r = sys.on_box(ctx, &words(&[b"pgrep", b"-cf", b"[o]marchy-launch-shell"]));
    // (`[ "$(...)" -gt 0 ] 2>/dev/null`: what is no number is not more than 0)
    if shell::test_int(&shell::subst(&r.out)).is_some_and(|n| n > 0) {
        "restarting"
    } else {
        "gone"
    }
}

/// `tr -s /`: runs of slashes squeezed to one.
fn squeeze(v: &[u8]) -> Vec<u8> {
    let mut w: Vec<u8> = Vec::new();
    for &c in v {
        if !(c == b'/' && w.last() == Some(&b'/')) {
            w.push(c);
        }
    }
    w
}

/// A Quickshell crash report in the box HOME (#303, #420): its folder's name, its time, the
/// crashed process it names (the child the handler forked to dump, `__QUICKSHELL_CRASH_DUMP_PID`
/// of the dialog's process), the crashed instance's config (its first `Config Path`, slashes
/// squeezed), and whether that config is the box's shell's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub name: Vec<u8>,
    pub mtime: Option<std::time::SystemTime>,
    pub dump: Vec<u8>,
    pub config: Vec<u8>,
    pub shell: bool,
}

impl Report {
    /// Its report.txt, as the host sees it.
    pub fn path(&self, ctx: &Ctx) -> Vec<u8> {
        cat!(
            ctx.d,
            "/home/.cache/quickshell/crashes/",
            self.name,
            "/report.txt"
        )
    }
}

/// The box's runtime dir's files a desk box's slot is filled from (#321 step 4, finding 703), which
/// share/desk-shell.lua writes: the command the slot runs (share/shell.sh reads it), and how it got
/// it (`config`, `guess` or `none`). The box's own files: read as such.
pub const DESK_SHELL_CMD: &str = "omabox-shell.cmd";
pub const DESK_SHELL_FROM: &str = "omabox-desk-shell";

/// What filled a desk box's slot, as its omabox-desk-shell says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeskSlot {
    /// The user's config started it (desk-shell.lua's take).
    Config,
    /// Nothing in the config did: the guess `up` made (box.json shell_guess), by its profile's start.
    Guess,
    /// Nothing did, and there was no guess.
    None,
    /// Nothing said yet (or not one of those words).
    Unsaid,
}

/// desk_slot: what filled the box's slot (DeskSlot), from its runtime dir.
pub fn desk_slot(ctx: &Ctx) -> DeskSlot {
    match run_read(&ctx.d, DESK_SHELL_FROM)
        .map(|v| strip_nl(&v))
        .as_deref()
    {
        Some(b"config") => DeskSlot::Config,
        Some(b"guess") => DeskSlot::Guess,
        Some(b"none") => DeskSlot::None,
        _ => DeskSlot::Unsaid,
    }
}

/// seen_cmd RAW: a slot's command as the box wrote it, fit to show: one line (a newline in it a
/// space, the last one dropped), at most 512 bytes, control characters shown (term::plain_cap).
pub fn seen_cmd(raw: &[u8]) -> Vec<u8> {
    let one: Vec<u8> = strip_nl(raw)
        .iter()
        .map(|&c| if c == b'\n' { b' ' } else { c })
        .collect();
    crate::term::plain_cap(&one, 512)
}

/// slot_cmd: what the box's slot runs, to show: `up --shell CMD`'s (box.json shell_cmd), else a desk
/// box's from its runtime dir (seen_cmd); empty: none (an Omarchy box, a desk whose config started
/// none, or not yet).
pub fn slot_cmd(ctx: &Ctx) -> Vec<u8> {
    let c = meta(&ctx.d, "shell_cmd").0;
    if !c.is_empty() || !slot_shell(&ctx.d) {
        return c;
    }
    run_read(&ctx.d, DESK_SHELL_CMD)
        .map(|v| seen_cmd(&v))
        .unwrap_or_default()
}

/// The box's runtime dir's file that names a plain box's shell's Quickshell configs (#247), a folder
/// a line (ready's shell_configs_note; share/crash-dialog.sh reads it too).
pub const SHELL_CONFIGS: &str = "omabox-shell.configs";

/// The folders the box's shell's config is in, slashes squeezed, no trailing one: the Omarchy tree's
/// `shell`, or a plain box's shell's Quickshell configs (`up --shell CMD`, #247; none known yet:
/// none).
fn shell_config_dirs(ctx: &Ctx) -> Vec<Vec<u8>> {
    let tidy = |v: &[u8]| {
        let w = strip_nl(&squeeze(v));
        w.strip_suffix(b"/").unwrap_or(&w).to_vec()
    };
    if slot_shell(&ctx.d) {
        return run_read(&ctx.d, SHELL_CONFIGS)
            .unwrap_or_default()
            .split(|&c| c == b'\n')
            .filter(|l| !l.is_empty())
            .map(tidy)
            .collect();
    }
    let want = meta(&ctx.d, "omarchy").0;
    let want = if want.is_empty() {
        b"/usr/share/omarchy".to_vec()
    } else {
        want
    };
    vec![tidy(&cat!(want, "/shell"))]
}

/// crash_reports: the box's Quickshell crash reports with their config, oldest first (by time,
/// then name): no link, under the box HOME, a name on one line. One whose config is not written
/// yet is none yet (it counts once it is).
pub fn crash_reports(ctx: &Ctx) -> Vec<Report> {
    let (Some(dir), Some(home)) = (
        readlink_e(&cat!(ctx.d, "/home/.cache/quickshell/crashes")),
        readlink_e(&cat!(ctx.d, "/home")),
    ) else {
        return Vec::new();
    };
    if !dir.starts_with(&cat!(home, "/")) {
        return Vec::new();
    }
    let want = shell_config_dirs(ctx);
    let mut out = Vec::new();
    for rep in shell::glob(&shell::path(&dir), false) {
        let d = cat!(dir, "/", rep);
        let f = cat!(d, "/report.txt");
        if rep.contains(&b'\n')
            || shell::path(&d).is_symlink()
            || shell::path(&f).is_symlink()
            || !shell::path(&f).is_file()
        {
            continue;
        }
        let Ok(text) = std::fs::read(shell::path(&f)) else {
            continue;
        };
        let first = |lead: &[u8]| {
            text.split(|&c| c == b'\n')
                .find_map(|l| l.strip_prefix(lead))
                .map(<[u8]>::to_vec)
        };
        let Some(config) = first(b"Config Path: ").map(|c| squeeze(&c)) else {
            continue;
        };
        // (`sub(/\/[^\/]*$/, "", c)`: its folder)
        let cdir = match config.iter().rposition(|&c| c == b'/') {
            Some(i) => &config[..i],
            None => &config[..],
        };
        out.push(Report {
            mtime: shell::path(&f).metadata().and_then(|m| m.modified()).ok(),
            dump: first(b"Crashed process ID: ").unwrap_or_default(),
            shell: want.iter().any(|w| cdir == w.as_slice()),
            config,
            name: rep,
        });
    }
    out.sort_by(|a, b| (a.mtime, &a.name).cmp(&(b.mtime, &b.name)));
    out
}

/// shell_crash_dirs: the box's Quickshell crash report folders (each a line) of this box's shell:
/// no link, under the box HOME, a name on one line, in name order.
pub fn shell_crash_dirs(ctx: &Ctx) -> Vec<u8> {
    let mut names: Vec<Vec<u8>> = crash_reports(ctx)
        .into_iter()
        .filter(|r| r.shell)
        .map(|r| r.name)
        .collect();
    names.sort();
    names.into_iter().flat_map(|n| cat!(n, "\n")).collect()
}

/// A Quickshell crash dialog (#420): the crashed instance's report when it is written, and whether
/// that instance is the box's shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dialog {
    pub shell: bool,
    pub report: Option<Report>,
}

/// crash_dialog PID: whether the window of process PID (the box's numbering) is Quickshell's crash
/// dialog, told as share/crash-dialog.sh tells it (#303): its process has the reporter's
/// `__QUICKSHELL_CRASH_DUMP_PID`, and the report naming that pid says whose; with none written
/// yet, the shell's when it has a plain box's shell's slot (#438) or the reporter's parent (the
/// crashed process) is the box's shell. Never by its
/// class or title: the class is the crashed instance's app id (`org.quickshell`, or a pragma's, as
/// an app built on Quickshell sets), the title Quickshell's name.
pub fn crash_dialog(ctx: &Ctx, pid: &[u8]) -> Option<Dialog> {
    crash_dialog_with(&RealBox, ctx, pid)
}

fn crash_dialog_with<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx, pid: &[u8]) -> Option<Dialog> {
    if !shell::is_num(pid) {
        return None;
    }
    let env = sys.on_box(ctx, &words(&[b"cat", &cat!("/proc/", pid, "/environ")]));
    if env.rc != 0 {
        return None;
    }
    let dump = env
        .out
        .split(|&c| c == 0)
        .find_map(|e| e.strip_prefix(b"__QUICKSHELL_CRASH_DUMP_PID=".as_slice()))
        .filter(|v| shell::is_num(v))?
        .to_vec();
    if let Some(r) = crash_reports(ctx)
        .into_iter()
        .rev()
        .find(|r| r.dump == dump)
    {
        return Some(Dialog {
            shell: r.shell,
            report: Some(r),
        });
    }
    // A plain box's shell's dialog (#438): forked from the crashed shell, it has its slot.
    if let Some(slot) = shell_slot(ctx)
        && env
            .out
            .split(|&c| c == 0)
            .any(|e| e == cat!("OMABOX_SHELL_SLOT=", slot).as_slice())
    {
        return Some(Dialog {
            shell: true,
            report: None,
        });
    }
    let st = sys.on_box(ctx, &words(&[b"cat", &cat!("/proc/", pid, "/status")]));
    let ppid = st.out.split(|&c| c == b'\n').find_map(|l| {
        l.strip_prefix(b"PPid:".as_slice())
            .map(|v| v.trim_ascii().to_vec())
    });
    Some(Dialog {
        shell: st.rc == 0 && ppid.is_some() && ppid == shell_pid(ctx),
        report: None,
    })
}

/// What a cover warning adds of a crash dialog (#420): whose it is, its report, and the way out.
/// The box shell's is closed as it opens (one left is one omabox missed); another's is its app's.
pub fn dialog_words(ctx: &Ctx, d: &Dialog, pid: &[u8]) -> Vec<u8> {
    let whose = match (&d.report, d.shell) {
        (_, true) => b"the box's shell".to_vec(),
        (Some(r), false) => cat!(
            crate::term::plain_cap(&r.config, 200),
            ", not the box's shell"
        ),
        (None, false) => b"another Quickshell, not the box's shell".to_vec(),
    };
    let rep = match &d.report {
        Some(r) => cat!("report: ", crate::term::plain(&r.path(ctx)), "; "),
        None => b"its report not written yet; ".to_vec(),
    };
    let kill = cat!("omabox run -b ", ctx.name, " -- kill ", pid);
    let way = if d.shell {
        cat!(kill, " closes it")
    } else {
        cat!("closing it is its app's, or ", kill)
    };
    cat!("Quickshell's crash dialog of ", whose, " (", rep, way, ")")
}

/// cover_note PID ADDRESS: what a cover warning says after itself when the window covering is a
/// Quickshell crash dialog (`; ADDRESS is Quickshell's crash dialog of ...`), else nothing.
pub fn cover_note(ctx: &Ctx, pid: &[u8], address: &[u8]) -> Vec<u8> {
    match crash_dialog(ctx, pid) {
        Some(d) => cat!("; ", address, " is ", dialog_words(ctx, &d, pid)),
        None => Vec::new(),
    }
}

/// shell_crashes: how many crash reports since `up`.
pub fn shell_crashes(ctx: &Ctx) -> usize {
    shell_crash_dirs(ctx)
        .split(|&c| c == b'\n')
        .filter(|l| !l.is_empty())
        .count()
}

/// shell_relaunches: "MARK COUNT STATUS", omabox-shell.mark, how many "relaunching" lines the
/// launcher wrote to shell.log, and the last one's status (- for none).
pub fn shell_relaunches(ctx: &Ctx) -> Vec<u8> {
    let mark = run_read(&ctx.d, "omabox-shell.mark")
        .map(|m| strip_nl(&m))
        .filter(|m| shell::is_num(m))
        .unwrap_or(b"0".to_vec());
    let (mut n, mut st) = (0usize, b"-".to_vec());
    if let Some(f) = box_home_file(&ctx.d, "shell.log") {
        let text = std::fs::read(&f).unwrap_or_default();
        (n, st) = relaunch_lines(&text);
    }
    cat!(mark, " ", n.to_string(), " ", st)
}

/// `grep -aoE 'omarchy-shell: Omarchy shell exited with status [0-9]{1,3}; relaunching' | awk ...`:
/// how many such matches, and the last one's status (- for none).
fn relaunch_lines(text: &[u8]) -> (usize, Vec<u8>) {
    const LEAD: &[u8] = b"omarchy-shell: Omarchy shell exited with status ";
    const END: &[u8] = b"; relaunching";
    let (mut n, mut st) = (0, b"-".to_vec());
    for line in text.split(|&c| c == b'\n') {
        let mut i = 0;
        while let Some(at) = line[i..].windows(LEAD.len()).position(|w| w == LEAD) {
            let s = i + at + LEAD.len();
            let d = line[s..]
                .iter()
                .take(4)
                .take_while(|c| c.is_ascii_digit())
                .count();
            if (1..=3).contains(&d) && line[s + d..].starts_with(END) {
                n += 1;
                st = line[s..s + d].to_vec();
                i = s + d + END.len();
            } else {
                i = i + at + 1;
            }
        }
    }
    (n, st)
}

/// shell_note: one line from shot, wait, windows, tree, pixel, keys and click: the shell gone or
/// hung, crashed since the last command said so, or killed and started again by its launcher; then
/// what the next one compares with.
pub fn shell_note(ctx: &Ctx) -> Res<()> {
    shell_note_with(&RealBox, ctx)
}

fn shell_note_with<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx) -> Res<()> {
    let reps = crash_reports(ctx);
    if meta(&ctx.d, "shell").0 == b"true" {
        shell_note_shell(sys, ctx, &reps)?;
    }
    others_note(ctx, &reps);
    Ok(())
}

/// The file others_note compares with: how many reports of other Quickshells it has seen.
const OTHERS_SEEN: &str = "other-crashes-seen";

/// others_note: a Quickshell in the box that is not its shell (an app built on Quickshell, a config
/// of the caller's) crashed since the last command (#420): said once, as itself, never as the
/// shell's (#303), with its config and report. A first look (no count kept: a box an older omabox
/// started) says nothing.
fn others_note(ctx: &Ctx, reps: &[Report]) {
    let others: Vec<&Report> = reps.iter().filter(|r| !r.shell).collect();
    let seen = std::fs::read(ctx.at(OTHERS_SEEN))
        .ok()
        .map(|s| strip_nl(&s))
        .and_then(|s| shell::test_int(&s));
    let _ = std::fs::write(ctx.at(OTHERS_SEEN), format!("{}\n", others.len()));
    let (Some(seen), Some(last)) = (seen, others.last()) else {
        return;
    };
    let new = (others.len() as i64).saturating_sub(seen);
    if new <= 0 {
        return;
    }
    let what = cat!(
        crate::term::plain_cap(&last.config, 200),
        "; report: ",
        crate::term::plain(&last.path(ctx))
    );
    say(if new == 1 {
        cat!(
            "warning: a Quickshell in the box crashed since the last command (not its shell: ",
            what,
            ")"
        )
    } else {
        cat!(
            "warning: Quickshells in the box crashed ",
            new.to_string(),
            " times since the last command (not its shell; the last: ",
            what,
            ")"
        )
    });
}

/// others_seen: what the next others_note compares with, as things are now (`up`: a crash before
/// the box was up is no news).
pub fn others_seen(ctx: &Ctx) {
    let n = crash_reports(ctx).iter().filter(|r| !r.shell).count();
    let _ = std::fs::write(ctx.at(OTHERS_SEEN), format!("{n}\n"));
}

fn shell_note_shell<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx, reps: &[Report]) -> Res<()> {
    let n = reps.iter().filter(|r| r.shell).count();
    let seen = std::fs::read(ctx.at("shell-crashes-seen"))
        .map(|s| strip_nl(&s))
        .unwrap_or(b"0".to_vec());
    let seen = if shell::is_num(&seen) {
        seen
    } else {
        b"0".to_vec()
    };
    let rl = shell_relaunches(ctx);
    let w = shell::read_fields(&rl, 3);
    let (m, c, st) = (w[0].clone(), w[1].clone(), w[2].clone());
    // (None yet, a box an older omabox started: this one is the first look.)
    let old = std::fs::read(ctx.at("shell-relaunches-seen"))
        .map(|o| strip_nl(&o[..o.len().min(100)]))
        .unwrap_or_default();
    let ow = shell::read_fields(&old, 3);
    let (om, oc) = (ow[0].clone(), ow[1].clone());
    let mut new = 0i64;
    if shell::is_num(&om) && shell::is_num(&oc) {
        new = if m == om {
            // (`$((c - oc))`, decimal: an 08 in the seen file is 8, where bash's arithmetic ended
            // the command in its own words: finding 448)
            let ci = crate::num::read(ctx, "the shell's relaunch count", &c)?;
            ci.saturating_sub(shell::dec(&oc).unwrap_or(0))
        } else {
            shell::test_int(&c).unwrap_or(0)
        };
    }
    let state = shell_state_with(sys, ctx, "");
    let name = &ctx.name;
    let seen_n = shell::test_int(&seen);
    let starting: &[u8] = if state == "running" {
        b""
    } else {
        b" (it is starting)"
    };
    if state == "gone" {
        let after = if n == 0 {
            String::new()
        } else {
            format!(" after {n} crash(es)")
        };
        say(cat!(
            "warning: box '",
            name,
            "' has no shell (gone",
            after,
            "; omabox log -b ",
            name,
            " shell; omabox restart-shell -b ",
            name,
            ")"
        ));
    } else if state == "hung" {
        say(cat!(
            "warning: the shell of box '",
            name,
            "' does not answer (hung: its bar is frozen; omabox log -b ",
            name,
            " shell; omabox restart-shell -b ",
            name,
            ")"
        ));
    } else if seen_n.is_some_and(|s| (n as i64) > s) {
        // (#420: its report named, the newest)
        let rep = reps
            .iter()
            .rfind(|r| r.shell)
            .map(|r| cat!("report: ", crate::term::plain(&r.path(ctx)), "; "))
            .unwrap_or_default();
        say(cat!(
            "warning: the shell crashed since the last command and was started again",
            starting,
            " (",
            rep,
            "omabox log -b ",
            name,
            " shell)"
        ));
    } else if new > 0 {
        let how = if shell::is_num(&st) {
            job_how(&st)
        } else {
            "exited".to_string()
        };
        say(cat!(
            "warning: the shell ",
            how,
            " since the last command and was started again",
            starting,
            " (omabox log -b ",
            name,
            " shell)"
        ));
    }
    let _ = std::fs::write(ctx.at("shell-crashes-seen"), format!("{n}\n"));
    let _ = std::fs::write(ctx.at("shell-relaunches-seen"), cat!(rl, "\n"));
    Ok(())
}

/// shell_seen: what the next shell_note compares with, as things are now (a command said it).
pub fn shell_seen(ctx: &Ctx) {
    let n = shell_crashes(ctx);
    let _ = std::fs::write(ctx.at("shell-crashes-seen"), format!("{n}\n"));
    let rl = shell_relaunches(ctx);
    let _ = std::fs::write(ctx.at("shell-relaunches-seen"), cat!(rl, "\n"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relaunches_counted_as_grep_and_awk() {
        let t = b"x omarchy-shell: Omarchy shell exited with status 9; relaunching.\n\
            omarchy-shell: Omarchy shell exited with status 1234; relaunching\n\
            a omarchy-shell: Omarchy shell exited with status 143; relaunching b omarchy-shell: Omarchy shell exited with status 2; relaunching\n";
        assert_eq!(relaunch_lines(t), (3, b"2".to_vec()));
        assert_eq!(relaunch_lines(b"nothing\n"), (0, b"-".to_vec()));
    }

    /// A box whose shell runs: pid 42's stat in the box, and its IPC answers.
    struct Running;

    impl BoxSys for Running {
        fn on_box_io(&self, _: &Ctx, args: &[Vec<u8>], _: Out, _: ErrTo) -> crate::exec::Ran {
            let out = if args.first().is_some_and(|a| a == b"cat") {
                b"42 (quickshell) S 1\n".to_vec()
            } else {
                Vec::new()
            };
            crate::exec::Ran {
                rc: 0,
                out,
                refused: false,
                gone_said: false,
            }
        }
    }

    // t_unit_shell_reports (#303, finding 408): the shell's crash reports are its own, not another
    // Quickshell's in the box, told by the report's config; a shell its launcher started again
    // (killed: no report) is said by the next command, once. Another Quickshell's crash is said as
    // itself, once (#420, finding 639).
    #[test]
    fn shell_reports_as_bash() {
        let d = std::env::temp_dir().join(format!("omabox-shrep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let c = d.join("home/.cache/quickshell/crashes");
        for x in ["ours", "other", "unwritten", "linked", "odd"] {
            std::fs::create_dir_all(c.join(x)).unwrap();
        }
        std::fs::create_dir_all(d.join("run")).unwrap();
        std::fs::create_dir_all(d.join("elsewhere")).unwrap();
        let w = |p: &std::path::Path, t: &str| std::fs::write(p, t).unwrap();
        w(
            &c.join("ours/report.txt"),
            "Signal: x\nCrashed process ID: 77\nConfig Path: /usr/share/omarchy/shell/shell.qml\n",
        );
        w(
            &c.join("other/report.txt"),
            "Crashed process ID: 78\nConfig Path: /tmp/qt/shell.qml\n",
        );
        w(
            &d.join("elsewhere/report.txt"),
            "Config Path: /usr/share/omarchy/shell/shell.qml\n",
        );
        std::os::unix::fs::symlink(d.join("elsewhere/report.txt"), c.join("linked/report.txt"))
            .unwrap();
        w(
            &c.join("odd/report.txt"),
            "Config Path: /usr/share/omarchy//shell/shell.qml\nConfig Path: /tmp/x/shell.qml\n",
        );
        w(&d.join("box.json"), "{\"shell\": true}");
        let mut ctx = Ctx::new(b"windows", shell::bytes(&std::env::temp_dir()).to_vec());
        ctx.parsed();
        ctx.name = b"sr".to_vec();
        ctx.d = shell::bytes(&d).to_vec();
        let mut dirs: Vec<Vec<u8>> = shell_crash_dirs(&ctx)
            .split(|&c| c == b'\n')
            .filter(|l| !l.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
        dirs.sort();
        assert_eq!(
            (dirs, shell_crashes(&ctx)),
            (vec![b"odd".to_vec(), b"ours".to_vec()], 2),
            "the shell's reports only: not another instance's, one not written yet, nor a link (its first Config Path, slashes squeezed)"
        );
        w(
            &d.join("box.json"),
            "{\"shell\": true, \"omarchy\": \"/x/om\"}",
        );
        w(
            &c.join("unwritten/report.txt"),
            "Config Path: /x/om/shell/shell.qml\n",
        );
        assert_eq!(
            shell_crash_dirs(&ctx),
            b"unwritten\n",
            "...an --omarchy tree's shell"
        );
        std::fs::remove_dir_all(&c).unwrap();
        std::fs::create_dir_all(c.join("other")).unwrap();
        w(
            &c.join("other/report.txt"),
            "Config Path: /tmp/qt/shell.qml\n",
        );
        w(&d.join("box.json"), "{\"shell\": true}");
        w(&d.join("run/omabox-shell.mark"), "100\n");
        w(&d.join("run/omabox-shell.pid"), "42\n");
        w(&d.join("home/shell.log"), "");
        let note = || {
            let (r, said) = crate::exit::heard(|| shell_note_with(&Running, &ctx));
            r.unwrap();
            said
        };
        assert_eq!(note(), "", "shell_note: a first look says nothing");
        assert_eq!(note(), "", "...nor another instance's crash");
        let log = d.join("home/shell.log");
        let add = |t: &str| {
            let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
            std::io::Write::write_all(&mut f, t.as_bytes()).unwrap();
        };
        add(
            "2026-10-20 16:00:00.000 omarchy-shell: Omarchy shell exited with status 137; relaunching.\n",
        );
        assert_eq!(
            note(),
            "omabox: warning: the shell was killed by SIGKILL (137) since the last command and was started again (omabox log -b sr shell)\n",
            "...the launcher starting a killed shell again: said"
        );
        assert_eq!(note(), "", "...once");
        // A new start of shell.sh (its mark): a fresh log, every line in it new.
        w(&d.join("run/omabox-shell.mark"), "200\n");
        w(
            &log,
            "omarchy-shell: Omarchy shell exited with status 1; relaunching.\n",
        );
        assert!(
            note().contains("the shell exited with status 1 since the last command"),
            "...after a new start, one that exited: its status"
        );
        std::fs::create_dir_all(c.join("mine")).unwrap();
        w(
            &c.join("mine/report.txt"),
            "Config Path: /usr/share/omarchy/shell/shell.qml\n",
        );
        add("omarchy-shell: Omarchy shell exited with status 134; relaunching.\n");
        assert_eq!(
            note(),
            format!(
                "omabox: warning: the shell crashed since the last command and was started again (report: {}/home/.cache/quickshell/crashes/mine/report.txt; omabox log -b sr shell)\n",
                d.display()
            ),
            "...a crash and its relaunch: the crash said, its report named (#420), once"
        );
        assert_eq!(note(), "", "...and not again");
        // #420: another Quickshell (an app built on it) crashing since the last command: said as
        // itself, never as the shell's, its config and report named, once.
        std::fs::create_dir_all(c.join("x1")).unwrap();
        w(
            &c.join("x1/report.txt"),
            "Crashed process ID: 90\nConfig Path: /opt/app//ui/boot/shell.qml\n",
        );
        assert_eq!(
            note(),
            format!(
                "omabox: warning: a Quickshell in the box crashed since the last command (not its shell: /opt/app/ui/boot/shell.qml; report: {}/home/.cache/quickshell/crashes/x1/report.txt)\n",
                d.display()
            ),
            "another Quickshell's crash: said as itself"
        );
        assert_eq!(note(), "", "...once");
        let stamp = |n: &str, secs: u64| {
            std::fs::File::options()
                .write(true)
                .open(c.join(n).join("report.txt"))
                .unwrap()
                .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(secs))
                .unwrap();
        };
        std::fs::create_dir_all(c.join("x2")).unwrap();
        std::fs::create_dir_all(c.join("x0")).unwrap();
        w(&c.join("x2/report.txt"), "Config Path: /opt/b/shell.qml\n");
        // (the later one said, whatever the names' order; its config made plain)
        w(
            &c.join("x0/report.txt"),
            "Config Path: /opt/\x1b[2Jc/shell.qml\n",
        );
        stamp("x2", 5);
        stamp("x0", 10);
        let said = note();
        assert!(
            said.starts_with(
                "omabox: warning: Quickshells in the box crashed 2 times since the last command (not its shell; the last: /opt/?[2Jc/shell.qml; report: "
            ) && said.ends_with("/x0/report.txt)\n"),
            "...several: how many, the last named: {said}"
        );
        // A box with no shell of its own: another Quickshell's crash is said all the same.
        w(&d.join("box.json"), "{\"shell\": false}");
        std::fs::create_dir_all(c.join("x4")).unwrap();
        w(&c.join("x4/report.txt"), "Config Path: /opt/d/shell.qml\n");
        stamp("x4", 15);
        assert!(
            note().contains(
                "a Quickshell in the box crashed since the last command (not its shell: /opt/d/shell.qml;"
            ),
            "...in a --no-shell box too"
        );
        // A first look (no count kept: a box an older omabox started) says nothing.
        std::fs::remove_file(d.join("other-crashes-seen")).unwrap();
        std::fs::create_dir_all(c.join("x5")).unwrap();
        w(&c.join("x5/report.txt"), "Config Path: /opt/e/shell.qml\n");
        assert_eq!(note(), "", "...a first look says nothing");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A box's process with its environment and status (cat /proc/PID/environ, status).
    struct Proc {
        env: &'static [u8],
        status: &'static [u8],
    }

    impl BoxSys for Proc {
        fn on_box_io(&self, _: &Ctx, args: &[Vec<u8>], _: Out, _: ErrTo) -> crate::exec::Ran {
            let a = args.get(1).cloned().unwrap_or_default();
            let (rc, out) = if a.ends_with(b"/environ") {
                (0, self.env.to_vec())
            } else if a.ends_with(b"/status") {
                (0, self.status.to_vec())
            } else {
                (1, Vec::new())
            };
            crate::exec::Ran {
                rc,
                out,
                refused: false,
                gone_said: false,
            }
        }
    }

    // #420 (finding 639): a window covering a click told as Quickshell's crash dialog by its
    // process's environment and the report naming its dump pid, never by its class or title (the
    // class is the crashed instance's app id: an app's own, as flea's, or org.quickshell).
    #[test]
    fn crash_dialog_told_and_said() {
        let d = std::env::temp_dir().join(format!("omabox-crdlg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let c = d.join("home/.cache/quickshell/crashes");
        for x in ["app", "sh"] {
            std::fs::create_dir_all(c.join(x)).unwrap();
        }
        std::fs::create_dir_all(d.join("run")).unwrap();
        let w = |p: &std::path::Path, t: &str| std::fs::write(p, t).unwrap();
        w(
            &c.join("app/report.txt"),
            "Crashed process ID: 1942\nConfig Path: /usr/share/flea/ui/boot/shell.qml\n",
        );
        w(
            &c.join("sh/report.txt"),
            "Crashed process ID: 77\nConfig Path: /usr/share/omarchy/shell/shell.qml\n",
        );
        w(&d.join("box.json"), "{\"shell\": true}");
        w(&d.join("run/omabox-shell.pid"), "50\n");
        let mut ctx = Ctx::new(b"click", shell::bytes(&std::env::temp_dir()).to_vec());
        ctx.parsed();
        ctx.name = b"cd".to_vec();
        ctx.d = shell::bytes(&d).to_vec();
        let p = |env: &'static [u8], status: &'static [u8]| Proc { env, status };
        let rep = |n: &str| {
            format!(
                "{}/home/.cache/quickshell/crashes/{n}/report.txt",
                d.display()
            )
        };
        let words = |dl: &Dialog| String::from_utf8(dialog_words(&ctx, dl, b"1943")).unwrap();
        // An app's own Quickshell, crashed: its report says whose.
        let app = crash_dialog_with(
            &p(
                b"A=1\0__QUICKSHELL_CRASH_DUMP_PID=1942\0B=2\0",
                b"PPid:\t1891\n",
            ),
            &ctx,
            b"1943",
        )
        .expect("a dialog");
        assert!(!app.shell, "another Quickshell's dialog");
        assert_eq!(
            words(&app),
            format!(
                "Quickshell's crash dialog of /usr/share/flea/ui/boot/shell.qml, not the box's shell (report: {}; closing it is its app's, or omabox run -b cd -- kill 1943)",
                rep("app")
            )
        );
        // The box's shell's, by its report.
        let sh = crash_dialog_with(
            &p(b"__QUICKSHELL_CRASH_DUMP_PID=77\0", b"PPid:\t9\n"),
            &ctx,
            b"1943",
        )
        .expect("a dialog");
        assert!(sh.shell, "the shell's, by its report's config");
        assert_eq!(
            words(&sh),
            format!(
                "Quickshell's crash dialog of the box's shell (report: {}; omabox run -b cd -- kill 1943 closes it)",
                rep("sh")
            )
        );
        // No report written yet: the shell's when the reporter's parent is the box's shell.
        let early = |st: &'static [u8]| {
            crash_dialog_with(&p(b"__QUICKSHELL_CRASH_DUMP_PID=5\0", st), &ctx, b"1943")
        };
        assert_eq!(
            early(b"Name:\tquickshell\nPPid:\t50\n"),
            Some(Dialog {
                shell: true,
                report: None
            }),
            "no report: the shell's by its parent"
        );
        let other = early(b"PPid:\t51\n").expect("a dialog");
        assert_eq!(
            words(&other),
            "Quickshell's crash dialog of another Quickshell, not the box's shell (its report not written yet; closing it is its app's, or omabox run -b cd -- kill 1943)"
        );
        // Not a dialog: no reporter's variable, no pid.
        assert_eq!(
            crash_dialog_with(
                &p(b"QUICKSHELL=1\0__QUICKSHELL_CRASH_DUMP_PIDX=3\0", b""),
                &ctx,
                b"1943"
            ),
            None,
            "an ordinary window"
        );
        assert_eq!(crash_dialog_with(&Running, &ctx, b"x"), None, "no pid");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A plain box's processes (#438): SLOT_PIDS_SH answers OUT, and /proc/PID/stat reads for the
    /// pids in RUNNING; the slot it was asked for kept.
    struct Slot {
        out: &'static [u8],
        running: &'static [&'static [u8]],
        asked: std::cell::RefCell<Vec<Vec<u8>>>,
    }

    impl BoxSys for Slot {
        fn on_box_io(&self, _: &Ctx, args: &[Vec<u8>], _: Out, _: ErrTo) -> crate::exec::Ran {
            let a = |i: usize| args.get(i).cloned().unwrap_or_default();
            let ran = |rc: i32, out: Vec<u8>| crate::exec::Ran {
                rc,
                out,
                refused: false,
                gone_said: false,
            };
            if a(0) == b"/usr/bin/sh" && a(2) == SLOT_PIDS_SH.as_bytes() {
                self.asked.borrow_mut().push(a(4));
                return ran(0, self.out.to_vec());
            }
            if a(0) == b"cat"
                && let Some(p) = a(1)
                    .strip_prefix(b"/proc/".as_slice())
                    .and_then(|r| r.strip_suffix(b"/stat".as_slice()))
                && self.running.contains(&p)
            {
                return ran(0, cat!(p, " (qs) S 1\n"));
            }
            ran(1, Vec::new())
        }
    }

    // #438: a plain box's shell is its slot's processes, CMD a launcher that started it and left: the
    // shell's pid while it runs, else the slot's first; gone when nothing of it is left. An Omarchy box
    // (no shell_cmd) and a shell.sh that wrote no slot keep the recorded pid alone.
    #[test]
    fn plain_shell_is_its_slot() {
        let d = std::env::temp_dir().join(format!("omabox-slot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("run")).unwrap();
        let w = |p: &str, t: &str| std::fs::write(d.join(p), t).unwrap();
        w(
            "box.json",
            r#"{"shell": true, "shell_cmd": "caelestia shell -d"}"#,
        );
        w("run/omabox-shell.pid", "40\n");
        w("run/omabox-shell.slot", "40-1762400000123456\n");
        let mut ctx = Ctx::new(b"ls", shell::bytes(&std::env::temp_dir()).to_vec());
        ctx.parsed();
        ctx.name = b"sl".to_vec();
        ctx.d = shell::bytes(&d).to_vec();
        let sys = |out: &'static [u8], running: &'static [&'static [u8]]| Slot {
            out,
            running,
            asked: std::cell::RefCell::new(Vec::new()),
        };
        assert_eq!(shell_slot(&ctx), Some(b"40-1762400000123456".to_vec()));
        let s = sys(b"40\n", &[b"40"]);
        assert_eq!(
            shell_live_pid_with(&s, &ctx),
            Some(b"40".to_vec()),
            "CMD running: its pid"
        );
        assert!(s.asked.borrow().is_empty(), "...the slot not asked");
        let s = sys(b"212\n57\n57\nx\n", &[b"57", b"212"]);
        assert_eq!(
            shell_live_pid_with(&s, &ctx),
            Some(b"57".to_vec()),
            "the launcher gone: the slot's first process"
        );
        assert_eq!(
            s.asked.borrow().as_slice(),
            &[b"40-1762400000123456".to_vec()],
            "...asked by the slot's name"
        );
        assert_eq!(shell_state_with(&s, &ctx, "ok"), "running", "...running");
        assert_eq!(
            shell_pids_with(&s, &ctx),
            Some(vec![b"57".to_vec(), b"212".to_vec()]),
            "shell_pids: the slot's, in pid order, once each"
        );
        let s = sys(b"57\n", &[b"40", b"57"]);
        assert_eq!(
            shell_pids_with(&s, &ctx),
            Some(vec![b"40".to_vec(), b"57".to_vec()]),
            "...and CMD's recorded pid while it runs (one that cleared its environment)"
        );
        let s = sys(b"", &[]);
        assert_eq!(shell_live_pid_with(&s, &ctx), None, "nothing left: none");
        assert_eq!(shell_state_with(&s, &ctx, "ok"), "gone", "...gone");
        assert_eq!(shell_pids_with(&s, &ctx), Some(Vec::new()));
        // A slot the box wrote oddly is none: the recorded pid alone.
        for bad in ["", "a b", "x;rm", "-\n-"] {
            w("run/omabox-shell.slot", bad);
            assert_eq!(shell_slot(&ctx), None, "an odd slot: none ({bad:?})");
        }
        let s = sys(b"57\n", &[b"57"]);
        assert_eq!(shell_live_pid_with(&s, &ctx), None, "...not asked");
        assert_eq!(shell_pids_with(&s, &ctx), None);
        // An Omarchy box: no slot, whatever the file says.
        w("run/omabox-shell.slot", "40-1\n");
        w("box.json", r#"{"shell": true}"#);
        assert_eq!(shell_slot(&ctx), None, "an Omarchy box: no slot");
        // A desk box (#321 step 4): its shell, the one its config starts, in the slot, with no
        // shell_cmd; up --no-shell (or a config that started none, after_ready): no slot.
        w("box.json", r#"{"shell": true, "desktop": "desk"}"#);
        assert_eq!(
            shell_slot(&ctx),
            Some(b"40-1".to_vec()),
            "a desk box: its slot"
        );
        w("box.json", r#"{"shell": false, "desktop": "desk"}"#);
        assert_eq!(shell_slot(&ctx), None, "a desk box with no shell: none");
        w("box.json", r#"{"shell": false, "desktop": "plain"}"#);
        assert_eq!(shell_slot(&ctx), None, "a plain box with no --shell: none");
        // A crash dialog with no report yet: the shell's by its slot (its parent gone, PPid 1).
        w("box.json", r#"{"shell": true, "shell_cmd": "x"}"#);
        let dl = |env: &'static [u8]| {
            crash_dialog_with(
                &Proc {
                    env,
                    status: b"PPid:\t1\n",
                },
                &ctx,
                b"300",
            )
        };
        assert_eq!(
            dl(b"__QUICKSHELL_CRASH_DUMP_PID=5\0OMABOX_SHELL_SLOT=40-1\0"),
            Some(Dialog {
                shell: true,
                report: None
            }),
            "a dialog with the shell's slot: the shell's"
        );
        assert_eq!(
            dl(b"__QUICKSHELL_CRASH_DUMP_PID=5\0OMABOX_SHELL_SLOT=40-12\0"),
            Some(Dialog {
                shell: false,
                report: None
            }),
            "...another slot's (an old start's): not"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    // #321 step 4: what a desk box's slot runs, as the box wrote it, shown one line, bounded, plain;
    // `up --shell CMD`'s (box.json) over it; how it got there, only by its three words.
    #[test]
    fn seen_cmd_plain_and_bounded() {
        assert_eq!(seen_cmd(b"dms run\n"), b"dms run");
        assert_eq!(seen_cmd(b"a\nb\n\n"), b"a b");
        assert_eq!(seen_cmd(b"qs \x1b[31m-c x"), b"qs ?[31m-c x");
        let long = seen_cmd(&[b'x'; 600]);
        assert!(long.starts_with(&[b'x'; 512]), "the first 512 bytes kept");
        assert!(long.ends_with(b"... (88 bytes more)"), "...the rest said");
        let d = std::env::temp_dir().join(format!("omabox-dslot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("run")).unwrap();
        let w = |p: &str, t: &str| std::fs::write(d.join(p), t).unwrap();
        let mut ctx = Ctx::new(b"ls", shell::bytes(&std::env::temp_dir()).to_vec());
        ctx.parsed();
        ctx.d = shell::bytes(&d).to_vec();
        w("box.json", r#"{"shell": true, "desktop": "desk"}"#);
        assert_eq!(slot_cmd(&ctx), b"", "nothing in the slot yet");
        assert_eq!(desk_slot(&ctx), DeskSlot::Unsaid);
        w("run/omabox-shell.cmd", "qs -p /x\nbar\n");
        w("run/omabox-desk-shell", "config\n");
        assert_eq!(slot_cmd(&ctx), b"qs -p /x bar", "the box's file, one line");
        assert_eq!(desk_slot(&ctx), DeskSlot::Config);
        for (t, s) in [
            ("guess\n", DeskSlot::Guess),
            ("none", DeskSlot::None),
            ("config x\n", DeskSlot::Unsaid),
            ("", DeskSlot::Unsaid),
        ] {
            w("run/omabox-desk-shell", t);
            assert_eq!(desk_slot(&ctx), s, "{t:?}");
        }
        w(
            "box.json",
            r#"{"shell": true, "desktop": "desk", "shell_cmd": "dms run"}"#,
        );
        assert_eq!(slot_cmd(&ctx), b"dms run", "up --shell's over the file");
        w("box.json", r#"{"shell": false, "desktop": "desk"}"#);
        assert_eq!(
            slot_cmd(&ctx),
            b"",
            "no shell: none, whatever the file says"
        );
        w("box.json", r#"{"shell": true}"#);
        assert_eq!(slot_cmd(&ctx), b"", "an Omarchy box: none");
        let _ = std::fs::remove_dir_all(&d);
    }

    // SLOT_PIDS_SH as it runs in a box, here on the test's own processes: the ones started under a
    // slot, whatever they did with their session or parent (a setsid daemon whose launcher left);
    // not another slot's, not a crash dialog's, not the grep's.
    #[test]
    fn slot_pids_as_sh() {
        use std::process::{Command, Stdio};
        let slot = format!("t{}-{}", std::process::id(), crate::settle::mono_ms());
        let other = format!("{slot}0");
        // A launcher that starts a daemon in a session of its own and leaves; and one of another slot,
        // and a crash dialog of this one.
        let out = Command::new("/usr/bin/sh")
            .args(["-c", "setsid sleep 30 </dev/null >/dev/null 2>&1 & echo $!"])
            .env("OMABOX_SHELL_SLOT", &slot)
            .stderr(Stdio::null())
            .output()
            .unwrap();
        let daemon = String::from_utf8(out.stdout).unwrap().trim().to_string();
        let mut oth = Command::new("sleep")
            .arg("30")
            .env("OMABOX_SHELL_SLOT", &other)
            .spawn()
            .unwrap();
        let mut dlg = Command::new("sleep")
            .arg("30")
            .env("OMABOX_SHELL_SLOT", &slot)
            .env("__QUICKSHELL_CRASH_DUMP_PID", "5")
            .spawn()
            .unwrap();
        let run = || {
            let o = Command::new("/usr/bin/sh")
                .args(["-c", SLOT_PIDS_SH, "sh", &slot])
                .env_clear()
                .output()
                .unwrap();
            String::from_utf8(o.stdout).unwrap()
        };
        // (setsid forks when its caller leads a group; its child is the daemon, the pid echoed its parent
        // or itself.)
        let got = run();
        let pids: Vec<&str> = got.lines().collect();
        assert_eq!(pids.len(), 1, "one process of the slot: {got:?}");
        let d = pids[0].to_string();
        assert!(
            std::fs::read(format!("/proc/{d}/cmdline"))
                .unwrap_or_default()
                .starts_with(b"sleep\0"),
            "...the daemon (echoed {daemon}, found {d})"
        );
        let _ = Command::new("kill").arg(&d).status();
        let _ = oth.kill();
        let _ = dlg.kill();
        let _ = oth.wait();
        let _ = dlg.wait();
        for _ in 0..50 {
            if run().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(run(), "", "...none once it is gone");
    }
}
