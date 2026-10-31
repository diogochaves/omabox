//! The screen's watch (finding 82): omabox-still in the box, for `wait still|change` and an action's
//! --wait (keys, click, scroll, drag, run -d). Its options where a command parses its own
//! (settle_opt, settle_opts_check, settle_in_window, mask_arg), the tool started and its `ready` read
//! (settle_begin), its answer in words (settle_end, settle_sent), and, for an action, what the
//! desktop did meanwhile (#274: desk_snap before the input, desk_facts and desk_target after). As
//! bin/omabox's SETTLE_* globals and functions, one struct here.

use crate::boxes::{BoxSys, RealBox, meta, touch};
use crate::cat;
use crate::desk::{Target, desk_facts, desk_snap, desk_target_with};
use crate::exec::{ErrTo, In, Out, on_box, pipe};
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::hypr::{hung_text_with, hypr_answers_with, hypr_json};
use crate::jobs::job_failed;
use crate::jq::{self, Args};
use crate::num::{ms_duration, ms_s, read, xywh_less, xywh_words};
use crate::procfs;
use crate::shell::{self, dec};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const BOX_STILL: &str = "/opt/omabox/bin/omabox-still";
/// --timeout and the like: 10 min at most.
pub const WAIT_MAX_MS: i64 = 600_000;

/// Milliseconds on the monotonic clock since this process first asked: what a deadline or an interval
/// is measured with (finding 450); now_ms is for a time written out.
pub fn mono_ms() -> i64 {
    i64::try_from(mono_ns() / 1_000_000).unwrap_or(i64::MAX)
}

/// mono_ms in nanoseconds.
pub fn mono_ns() -> i128 {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    i128::try_from(T0.get_or_init(std::time::Instant::now).elapsed().as_nanos())
        .unwrap_or(i128::MAX)
}

/// `date +%s%3N`: a time written out (a mark, a stamp); never a deadline (mono_ms).
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(0))
        .unwrap_or(0)
}

/// A deadline N seconds from now, on the monotonic clock (finding 450: bash's `end=$((SECONDS + N))`
/// then `[ $SECONDS -lt $end ]` counted whole seconds of the wall clock, so a wait ended 0 to 1 s
/// short of N, and a clock step moved it).
pub fn after(n: u64) -> std::time::Instant {
    std::time::Instant::now() + std::time::Duration::from_secs(n)
}

/// Whether deadline D has come.
pub fn past(d: std::time::Instant) -> bool {
    std::time::Instant::now() >= d
}

/// geom_parse: a -g value as its X Y W H, from grim's "X,Y WxH" or X,Y,W,H and X,Y W,H (#27); each a
/// number 64 bits hold (finding 448: bash's arithmetic wrapped past them).
pub fn geom_parse(v: &[u8]) -> Option<[Vec<u8>; 4]> {
    let re = shell::Re::new(r"^(-?[0-9]+),(-?[0-9]+)[\ ,]([1-9][0-9]*)[x,]([1-9][0-9]*)$")?;
    let m = re.captures(v, false)?;
    let g = |i: usize| {
        m.get(i)
            .copied()
            .flatten()
            .map(|(a, b)| v[a..b].to_vec())
            .unwrap_or_default()
    };
    let all = [g(1), g(2), g(3), g(4)];
    all.iter().all(|n| shell::dec(n).is_some()).then_some(all)
}

/// geom_parse's answer as grim's "X,Y WxH", its numbers as read (`010` is 10: #384).
pub fn geom_text(g: &[Vec<u8>; 4]) -> Vec<u8> {
    let n = |v: &[u8]| dec(v).map_or_else(|| v.to_vec(), |n| n.to_string().into_bytes());
    cat!(n(&g[0]), ",", n(&g[1]), " ", n(&g[2]), "x", n(&g[3]))
}

/// `${g// /,}` of geom_parse's answer: omabox-still's X,Y,W,H.
pub fn geom_csv(g: &[Vec<u8>; 4]) -> Vec<u8> {
    g.join(&b',')
}

/// `IFS=, read -r A B C D <<<"$V"`: four fields, the last taking the rest.
fn csv4(v: &[u8]) -> [Vec<u8>; 4] {
    let line = v.split(|&c| c == b'\n').next().unwrap_or_default();
    let mut it = line.splitn(4, |&c| c == b',');
    let mut f = || it.next().unwrap_or_default().to_vec();
    [f(), f(), f(), f()]
}

/// rects_meet A B: whether two X,Y,W,H rectangles overlap; None where a number is none (dec).
pub fn rects_meet(a: &[u8], b: &[u8]) -> Option<bool> {
    let n = |v: &[Vec<u8>; 4]| -> Option<[i64; 4]> {
        Some([dec(&v[0])?, dec(&v[1])?, dec(&v[2])?, dec(&v[3])?])
    };
    let [ax, ay, aw, ah] = n(&csv4(a))?;
    let [bx, by, bw, bh] = n(&csv4(b))?;
    Some(
        ax < bx.saturating_add(bw)
            && bx < ax.saturating_add(aw)
            && ay < by.saturating_add(bh)
            && by < ay.saturating_add(ah),
    )
}

/// cursor_rect "X Y": where the software cursor is drawn (findings 82, 305), as omabox-still's
/// rectangle around the point.
pub fn cursor_rect(p: &[u8]) -> Option<Vec<u8>> {
    let sp = p.iter().rposition(|&c| c == b' ').unwrap_or(0);
    let fp = p.iter().position(|&c| c == b' ').map_or(p.len(), |i| i + 1);
    let x = dec(&p[..sp])?;
    let y = dec(&p[fp.min(p.len())..])?;
    Some(format!("{},{},64,64", x.saturating_sub(16), y.saturating_sub(16)).into_bytes())
}

/// cursor_now: where the pointer is, "X Y" (nothing when it is not two numbers); a box not answering
/// ends the command (hypr_json), as does jq failing (`p=$(cursor_now) || exit $?`).
pub fn cursor_now(ctx: &Ctx) -> Res<Vec<u8>> {
    let j = shell::strip_nl(&hypr_json(ctx, &["cursorpos"])?);
    let r = jq::jq(
        &["-r", crate::jqprog::CURSOR_JQ],
        Some(&j),
        jq::Err::Inherit,
    );
    if !r.ok() {
        return Err(Exit(u8::try_from(r.rc).unwrap_or(1)));
    }
    Ok(r.text())
}

/// ws_text WS: a workspace in words.
pub fn ws_text(ws: &[u8]) -> Vec<u8> {
    if ws == b"special:scratchpad" {
        return b"the scratchpad (SUPER+S shows it)".to_vec();
    }
    if let Some(s) = ws.strip_prefix(b"special:") {
        return cat!(
            "special workspace ",
            s,
            ", which the bar widget's Show opens and a workspace switch hides"
        );
    }
    cat!("workspace ", ws)
}

/// kill_tree PID: PID, a child of this process, and everything under it, the children first. Each
/// pinned by a pidfd while it is its parent's child (#377): a pid reaped and reused since it was
/// stored is no child of ours, and is left.
pub fn kill_tree(pid: &[u8]) {
    let me = std::process::id().to_string().into_bytes();
    if let Some(p) = pin_child(pid, &me) {
        kill_pinned(&p);
    }
}

/// PID pinned while PARENT's child.
fn pin_child(pid: &[u8], parent: &[u8]) -> Option<crate::pidfd::Pidfd> {
    crate::pidfd::Pidfd::pin(pid, || procfs::proc_ppid(pid).is_some_and(|p| p == parent))
}

fn kill_pinned(p: &crate::pidfd::Pidfd) {
    for c in procfs::children(p.pid()) {
        if let Some(c) = pin_child(&c, p.pid()) {
            kill_pinned(&c);
        }
    }
    p.signal(libc::SIGKILL);
}

/// One line of the tool's, as `IFS= read -r -t 1` gives it: a whole line, what came of one before
/// the second was over, or what came of one before the end.
pub enum Got {
    Line(Vec<u8>),
    Timeout(Vec<u8>),
    Eof(Vec<u8>),
}

/// The tool's stdout and stderr (SETTLE_OUT), read a line at a time (and with_mods' keyboard tool's,
/// drag --shot's pointer tool's).
pub struct Lines {
    fd: OwnedFd,
    buf: Vec<u8>,
    eof: bool,
}

impl Lines {
    pub fn new(fd: OwnedFd) -> Lines {
        Lines {
            fd,
            buf: Vec::new(),
            eof: false,
        }
    }

    pub fn read(&mut self, t: Duration) -> Got {
        let end = Instant::now() + t;
        loop {
            if let Some(i) = self.buf.iter().position(|&c| c == b'\n') {
                let mut l: Vec<u8> = self.buf.drain(..=i).collect();
                l.pop();
                return Got::Line(l);
            }
            if self.eof {
                return Got::Eof(std::mem::take(&mut self.buf));
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Got::Timeout(std::mem::take(&mut self.buf));
            }
            let mut pfd = libc::pollfd {
                fd: self.fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ms = libc::c_int::try_from(left.as_millis().max(1)).unwrap_or(1000);
            // SAFETY: one pollfd of ours.
            if unsafe { libc::poll(&mut pfd, 1, ms) } <= 0 {
                continue;
            }
            let mut tmp = [0u8; 4096];
            // SAFETY: tmp is ours, its length given.
            let n = unsafe { libc::read(self.fd.as_raw_fd(), tmp.as_mut_ptr().cast(), tmp.len()) };
            match n {
                0 => self.eof = true,
                n if n < 0 => {
                    if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                        self.eof = true;
                    }
                }
                // (read drops NUL bytes)
                n => self.buf.extend(
                    tmp[..usize::try_from(n).unwrap_or(0)]
                        .iter()
                        .filter(|&&c| c != 0),
                ),
            }
        }
    }
}

/// The tool running: its stdin held here (SETTLE_IN: its end, --tied), its output read here
/// (SETTLE_OUT), the background on_box that runs it, and that one's pid (SETTLE_PID's
/// tree).
struct Tool {
    input: Option<OwnedFd>,
    lines: Lines,
    thread: JoinHandle<()>,
    pid: Arc<AtomicI32>,
}

/// SETTLE_*: what one command's watch holds.
#[derive(Default)]
pub struct Settle {
    /// SETTLE_FACTS: the action (keys, click, scroll, drag, run), empty for `wait`.
    pub facts: &'static str,
    /// SETTLE_FXY: the point ("X Y") of click, scroll, drag's start; SETTLE_FPID: run -d's job.
    pub fxy: Vec<u8>,
    pub fpid: Vec<u8>,
    /// SETTLE_ARGS, SETTLE_JSON, SETTLE_WAIT, SETTLE_TOOK, SETTLE_REGION, SETTLE_WINDOW,
    /// SETTLE_ORIGIN ("X Y").
    pub args: Vec<Vec<u8>>,
    pub json: bool,
    pub wait: bool,
    pub took: usize,
    pub region: bool,
    pub window: bool,
    pub origin: Option<(Vec<u8>, Vec<u8>)>,
    /// MASKS: --ignore regions so far.
    pub masks: usize,
    /// SETTLE_LOOKED: a 124 said where the input went and that nothing moved.
    pub looked: bool,
    /// --expect tree TEXT (#319): the text the tree holds once the screen settled, read in the window
    /// EXPECT_SELS picks (the action's) or on the layer EXPECT_LAYER, else the active window.
    pub expect: Vec<u8>,
    pub expect_sels: Vec<Vec<u8>>,
    pub expect_layer: Vec<u8>,
    snap: Option<JoinHandle<Vec<u8>>>,
    before: Vec<u8>,
    line: Vec<u8>,
    err: Vec<u8>,
    t0: i64,
    tool: Option<Tool>,
}

impl Settle {
    /// mask_arg GEOM: an --ignore region (#131) as omabox-still's X,Y,W,H; at most 16.
    pub fn mask_arg(&mut self, ctx: &Ctx, g: &[u8]) -> Res<Vec<u8>> {
        let Some(p) = geom_parse(g) else {
            return Err(ctx.die(cat!("--ignore is \"X,Y WxH\" (or X,Y,W,H), got ", g)));
        };
        self.masks += 1;
        if self.masks > 16 {
            return Err(ctx.die("--ignore: at most 16 regions"));
        }
        Ok(geom_csv(&p))
    }

    /// settle_opt OPT [VALUE]: --wait's options, taken where the command parses its own; `took`, how
    /// many words (0: not one of these).
    pub fn opt(&mut self, ctx: &Ctx, args: &[Vec<u8>]) -> Res<()> {
        self.took = 1;
        let o = args.first().map(Vec::as_slice).unwrap_or_default();
        let v = args.get(1);
        match o {
            b"--ignore" => {
                let Some(v) = v else {
                    return Err(ctx.die("--ignore takes a region: \"X,Y WxH\""));
                };
                let m = self.mask_arg(ctx, v)?;
                self.args.extend([b"--mask".to_vec(), m]);
                self.took = 2;
            }
            b"-g" => {
                let Some(g) = v.and_then(|v| geom_parse(v)) else {
                    return Err(ctx.die(
                        "-g is \"X,Y WxH\" (or X,Y,W,H): the part of the screen --wait watches",
                    ));
                };
                if self.region {
                    return Err(ctx.die("-g once: one region is watched"));
                }
                self.args.extend([b"--region".to_vec(), geom_csv(&g)]);
                self.region = true;
                self.took = 2;
            }
            b"--wait" => self.wait = true,
            b"--json" => self.json = true,
            b"--start" | b"--quiet" | b"--timeout" => {
                let Some(ms) = v.and_then(|v| ms_duration(v)) else {
                    return Err(ctx.die(cat!(o, " takes a duration: 300ms, 2s, 1m")));
                };
                if ms > WAIT_MAX_MS {
                    return Err(ctx.die(cat!(o, " is at most 10m")));
                }
                self.args.extend([o.to_vec(), ms.to_string().into_bytes()]);
                self.took = 2;
            }
            b"--strict" => self.args.push(b"--strict".to_vec()),
            b"--expect" => {
                let t = args.get(2).filter(|t| !t.is_empty());
                let (Some(b"tree"), Some(t)) = (v.map(Vec::as_slice), t) else {
                    return Err(ctx.die(
                        "--expect takes tree TEXT (as tree --find takes it): --expect tree '\"Dark mode\" [checked]'",
                    ));
                };
                if !self.expect.is_empty() {
                    return Err(ctx.die("--expect once: one text is looked for"));
                }
                self.expect = t.clone();
                self.took = 3;
            }
            _ => self.took = 0,
        }
        Ok(())
    }

    /// settle_opts_check [WINDOWS]: the options go with --wait; with one --window selector, -g and
    /// --ignore are the window's coordinates; an --ignore that misses -g's region is said (#162).
    pub fn opts_check(&mut self, ctx: &Ctx, windows: usize) -> Res<()> {
        if !self.wait && !(self.args.is_empty() && !self.json && self.expect.is_empty()) {
            return Err(ctx.die(
                "--start, --quiet, --timeout, --strict, --ignore, -g, --json and --expect go with --wait",
            ));
        }
        if windows != 0 {
            self.window = true;
        }
        let n = self.args.len();
        let mut region: Vec<u8> = Vec::new();
        for i in 0..n.saturating_sub(1) {
            if self.args[i] == b"--region" {
                region = self.args[i + 1].clone();
            }
        }
        if region.is_empty() {
            return Ok(());
        }
        for i in 0..n.saturating_sub(1) {
            if self.args[i] != b"--mask" {
                continue;
            }
            match rects_meet(&self.args[i + 1], &region) {
                Some(true) => continue,
                Some(false) => {}
                // (geom_parse took both: numbers it reads)
                None => {
                    return Err(ctx.die(cat!(
                        "cannot read the numbers of ",
                        self.args[i + 1],
                        " and ",
                        region
                    )));
                }
            }
            say(cat!(
                "--ignore \"",
                xywh_words(&self.args[i + 1]),
                "\" is outside -g's region: it leaves nothing out there"
            ));
        }
        Ok(())
    }

    /// settle_in_window X Y (F8, #287): the caller's regions, given in the window's coordinates,
    /// moved by the window's place (X Y: the window's, read as numbers or the command ends).
    pub fn in_window(&mut self, ctx: &Ctx, x: &[u8], y: &[u8]) -> Res<()> {
        self.origin = Some((x.to_vec(), y.to_vec()));
        let ox = crate::num::read(ctx, "the window's place", x)?;
        let oy = crate::num::read(ctx, "the window's place", y)?;
        for i in 0..self.args.len().saturating_sub(1) {
            if !matches!(self.args[i].as_slice(), b"--region" | b"--mask") {
                continue;
            }
            let [a, b, w, h] = csv4(&self.args[i + 1]);
            let a = crate::num::read(ctx, "a region", &a)?;
            let b = crate::num::read(ctx, "a region", &b)?;
            self.args[i + 1] = cat!(
                a.saturating_add(ox).to_string(),
                ",",
                b.saturating_add(oy).to_string(),
                ",",
                w,
                ",",
                h
            );
        }
        Ok(())
    }

    /// The before snapshot (an action's), read in the background while the tool starts.
    fn snap_start(&mut self, ctx: &Ctx) -> Res<()> {
        if self.facts.is_empty() {
            return Ok(());
        }
        // (bash's snapshot went to a `mktemp` file, removed once read: one that cannot be made
        // ends the command, mktemp's words said; made and removed here all the same)
        let (_, f) = shell::mktemp()?;
        let _ = std::fs::remove_file(shell::path(&f));
        self.before.clear();
        let c = ctx.clone();
        self.snap = Some(std::thread::spawn(move || desk_snap(&c, true)));
        Ok(())
    }

    /// desk_snap_take: the before snapshot, once its read is over.
    fn snap_take(&mut self) {
        if let Some(t) = self.snap.take() {
            self.before = t.join().unwrap_or_default();
        }
    }

    /// settle_begin MODE TOOL_ARGS...: omabox-still in the box, waiting on its first frame (its
    /// `ready`, or its answer), 12 s at most: a hung compositor is the tool's own "unknown hung" at
    /// 10 s (finding 181).
    pub fn begin(&mut self, ctx: &Ctx, tool_args: &[Vec<u8>]) -> Res<()> {
        // OMABOX_STILL_DUMP=DIR (a path in the box; the suite's, for a flaky check: #222), refused
        // before anything is made (finding 426).
        let mut argv = vec![BOX_STILL.as_bytes().to_vec()];
        argv.extend(tool_args.iter().cloned());
        if let Some(d) = shell::env("OMABOX_STILL_DUMP").filter(|d| !d.is_empty()) {
            if !shell::re_match("^/[A-Za-z0-9._/-]+$", &d) {
                return Err(ctx.die(cat!(
                    "OMABOX_STILL_DUMP: a plain absolute path in the box, got ",
                    d
                )));
            }
            argv.extend([b"--dump".to_vec(), d]);
        }
        // (Before any input is sent: a box started by an omabox without the tool cannot watch.)
        let t = on_box(
            ctx,
            &[
                b"test".to_vec(),
                b"-x".to_vec(),
                BOX_STILL.as_bytes().to_vec(),
            ],
            false,
            Out::Inherit,
            ErrTo::Inherit,
        )
        .or_exit()?;
        if t.rc != 0 {
            return Err(ctx.die(cat!(
                "box '",
                ctx.name,
                "' has no ",
                BOX_STILL,
                " (started by an older omabox): omabox down ",
                ctx.name,
                ", then up again"
            )));
        }
        self.snap_start(ctx)?;
        // (the tool's fifos were in a `mktemp -d` dir: the same rule)
        let (_, dir) = shell::mktemp_args(&[b"-d"])?;
        let _ = std::fs::remove_dir(shell::path(&dir));
        argv.push(b"--tied".to_vec());
        let ((in_r, in_w), (out_r, out_w)) = match (pipe(), pipe()) {
            (Ok(a), Ok(b)) => (a, b),
            _ => return Err(ctx.die("could not make the pipes for the screen's watch")),
        };
        let pid = Arc::new(AtomicI32::new(0));
        let (c, p) = (ctx.clone(), pid.clone());
        let thread = std::thread::spawn(move || {
            block_sigpipe();
            let err: RawFd = out_w.as_raw_fd();
            let o = out_w.try_clone().ok();
            if let Some(o) = o {
                // (bash's `&`: SIGINT and SIGQUIT ignored in it)
                let _ = crate::exec::on_box_bg(
                    &c,
                    &argv,
                    In::Fd(in_r),
                    Out::Fd(o),
                    ErrTo::Fd(err),
                    Some(p),
                );
            }
            drop(out_w);
        });
        self.tool = Some(Tool {
            input: Some(in_w),
            lines: Lines {
                fd: out_r,
                buf: Vec::new(),
                eof: false,
            },
            thread,
            pid,
        });
        self.err.clear();
        self.first_line(after(12));
        Ok(())
    }

    /// settle_begin's read of the tool's first line: `ready`, or its answer, by END; what else it
    /// says is SETTLE_ERR.
    fn first_line(&mut self, end: Instant) {
        let (mut line, mut part) = (Vec::new(), Vec::new());
        let answer = shell::Re::new(r"^(satisfied|unsatisfied|unknown)\ ");
        let is_answer = |l: &[u8]| answer.as_ref().is_some_and(|r| r.is_match(l));
        if let Some(tool) = self.tool.as_mut() {
            loop {
                // (IFS=: a line split at a space keeps it, #310.)
                match tool.lines.read(Duration::from_secs(1)) {
                    Got::Line(l) => {
                        line = cat!(part, l);
                        part.clear();
                        if line.starts_with(b"ready") || is_answer(&line) {
                            break;
                        }
                        let sep: &[u8] = if self.err.is_empty() { b"" } else { b"; " };
                        self.err = cat!(self.err, sep, line);
                    }
                    // A read that timed out keeps what it had of a line: the rest comes with the
                    // next one (finding 217).
                    Got::Timeout(p) => {
                        part.extend(p);
                        line.clear();
                        if past(end) {
                            break;
                        }
                    }
                    Got::Eof(p) => {
                        part.extend(p);
                        line.clear();
                        break;
                    }
                }
            }
        }
        self.line = line;
        self.t0 = mono_ms();
    }

    /// settle_ready JSON: before keys, click or run -d act; no `ready` means nothing is sent, and
    /// the answer is unknown (1), never the action done unwatched.
    pub fn ready(&mut self, ctx: &Ctx, json: bool) -> Res<()> {
        self.snap_take();
        if self.line.starts_with(b"ready") {
            return Ok(());
        }
        let _ = self.end(ctx, "settle", json, b"")?;
        say("nothing was sent: the screen could not be watched");
        Err(Exit(1))
    }

    /// The tool's ends closed, then the tool given 2 s to be gone, killed with what runs it after
    /// (#124).
    fn finish_tool(&mut self) {
        let Some(mut t) = self.tool.take() else {
            return;
        };
        t.input.take();
        drop(t.lines);
        let mut gone = false;
        for _ in 0..40 {
            if t.thread.is_finished() {
                gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if !gone {
            let p = t.pid.load(Ordering::SeqCst);
            if p > 0 {
                kill_tree(p.to_string().as_bytes());
            }
            // (Killed with what runs it, its on_box says nothing more: not waited for.)
            return;
        }
        let _ = t.thread.join();
    }

    /// settle_end COND JSON SENT: the tool's answer as a line (or JSON) on stdout; 0, 124 or 1. COND
    /// names it in messages; SENT is what an action already did, said when the answer is not yes.
    pub fn end(&mut self, ctx: &Ctx, cond: &str, json: bool, sent: &[u8]) -> Res<i32> {
        self.end_with(&RealBox, ctx, cond, json, sent)
    }

    /// settle_end, the box (its life, its Hyprland's answers, its desktop after) read through SYS.
    fn end_with<S: BoxSys + ?Sized>(
        &mut self,
        sys: &S,
        ctx: &Ctx,
        cond: &str,
        json: bool,
        sent: &[u8],
    ) -> Res<i32> {
        let mut line = self.line.clone();
        let scr = line.strip_prefix(b"ready ").unwrap_or(&line).to_vec();
        let answer = shell::Re::new(r"^(satisfied|unsatisfied|unknown)\ ");
        let is_answer = |l: &[u8]| answer.as_ref().is_some_and(|r| r.is_match(l));
        if line.starts_with(b"ready") {
            line.clear();
            let mut part = Vec::new();
            let mut used = std::time::Instant::now();
            if let Some(tool) = self.tool.as_mut() {
                loop {
                    match tool.lines.read(Duration::from_secs(1)) {
                        Got::Line(l) => {
                            line = cat!(part, l);
                            part.clear();
                            if is_answer(&line) {
                                break;
                            }
                            let sep: &[u8] = if self.err.is_empty() { b"" } else { b"; " };
                            self.err = cat!(self.err, sep, line);
                            line.clear();
                            continue;
                        }
                        Got::Timeout(p) => {
                            part.extend(p);
                            line.clear();
                        }
                        // EOF: the tool is gone without an answer.
                        Got::Eof(_) => {
                            line.clear();
                            break;
                        }
                    }
                    // Waiting is using the box (idle expiry, finding 59).
                    if used.elapsed() >= std::time::Duration::from_secs(30) {
                        touch(&ctx.at("used"));
                        used = std::time::Instant::now();
                    }
                }
            }
        }
        // Every wait in the tool has a deadline (finding 181), but this one does not depend on that
        // (#124): one still running 2 s after it answered is killed, with what runs it.
        self.finish_tool();
        let w = shell::read_fields(&line, 3);
        let (mut res, mut reason) = (w[0].clone(), w[1].clone());
        let mut f: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for kv in shell::words(&w[2]) {
            let i = kv.iter().position(|&c| c == b'=');
            let (k, v) = match i {
                Some(i) => (kv[..i].to_vec(), kv[i + 1..].to_vec()),
                None => (kv.clone(), kv.clone()),
            };
            if let Some(e) = f.iter_mut().find(|e| e.0 == k) {
                e.1 = v;
            } else {
                f.push((k, v));
            }
        }
        let get = |k: &str| -> Vec<u8> {
            f.iter()
                .find(|e| e.0 == k.as_bytes())
                .map(|e| e.1.clone())
                .unwrap_or_default()
        };
        // `${f[K]:--}`
        let dash = |k: &str| -> Vec<u8> {
            let v = get(k);
            if v.is_empty() { b"-".to_vec() } else { v }
        };
        if res.is_empty() {
            res = b"unknown".to_vec();
            reason = b"lost".to_vec();
        }
        let n = &ctx.name;
        let err_part = if self.err.is_empty() {
            Vec::new()
        } else {
            cat!(" (", self.err, ")")
        };
        if res == b"unknown" {
            reason = match reason.as_slice() {
                b"not-rendered" => {
                    // A hidden interactive box is drawn (finding 90) unless it is older than that
                    // or confirm-close replaced its window; a headless box always draws: no frame
                    // is a Hyprland that stopped answering (#124).
                    if meta(&ctx.d, "mode").0 != b"interactive" {
                        if hypr_answers_with(sys, ctx, "2")? {
                            cat!("box '", n, "' sent no frame")
                        } else {
                            hung_text_with(sys, ctx, "")?
                        }
                    } else if meta(&ctx.d, "drawn_hidden").0 != b"true" {
                        cat!(
                            "box '",
                            n,
                            "' not rendered: an older omabox started it, and it is not drawn while hidden (it needs a restart: ask the user; never switch their workspace or focus)"
                        )
                    } else if ctx.at("run/omabox.reopened").is_file() {
                        cat!(
                            "box '",
                            n,
                            "' not rendered: its window was replaced (confirm-close) and is not drawn while hidden (ask the user; never switch their workspace or focus)"
                        )
                    } else {
                        cat!(
                            "box '",
                            n,
                            "' sent no frame (",
                            ws_text(&meta(&ctx.d, "workspace").0),
                            "; ask the user; never switch their workspace or focus)"
                        )
                    }
                }
                b"hung" => hung_text_with(sys, ctx, "10 s")?,
                b"refused" => cat!("box '", n, "' refused a screen capture", err_part),
                b"off-screen" => cat!("the region watched is not on the screen of box '", n, "'"),
                _ => {
                    // Its compositor gone: the box is (being) taken down.
                    let gone = mono_ms() - self.t0;
                    let end = after(2);
                    while sys.box_alive(ctx)? && !past(end) {
                        std::thread::sleep(Duration::from_millis(200));
                    }
                    if !sys.box_alive(ctx)? {
                        cat!("box '", n, "' went down after ", ms_s(gone.to_string()))
                    } else if !hypr_answers_with(sys, ctx, "2")? {
                        hung_text_with(sys, ctx, "")?
                    } else {
                        cat!("lost the box's screen", err_part)
                    }
                }
            };
        }
        let geo = if dash("change") == b"-" {
            Vec::new()
        } else {
            xywh_words(&get("change"))
        };
        let ign = if dash("ignored") == b"-" {
            Vec::new()
        } else {
            let why = crate::num::awk_v(&get("why"));
            let w: &[u8] = match why.as_slice() {
                b"caret" => b"caret?",
                b"mask" => b"--ignore",
                _ => &why,
            };
            // (awk's fields: each one alone)
            let ig = get("ignored");
            let line = ig.split(|&c| c == b'\n').next().unwrap_or_default();
            let fl: Vec<&[u8]> = line.split(|&c| c == b',').collect();
            let g = |i: usize| fl.get(i).copied().unwrap_or_default();
            cat!(g(2), "x", g(3), " at ", g(0), ",", g(1), ": ", w)
        };
        let ign_semi = if ign.is_empty() {
            Vec::new()
        } else {
            cat!("; ignored ", ign)
        };
        let mut msg: Vec<u8> = match (res.as_slice(), reason.as_slice()) {
            (b"satisfied", b"changed") => cat!("changed after ", ms_s(get("last")), " at ", geo),
            (b"satisfied", b"still" | b"settled") => {
                let word: &[u8] = if reason == b"still" {
                    b"still"
                } else {
                    b"settled"
                };
                let mut m = cat!(word, " after ", ms_s(get("t")));
                if geo.is_empty() {
                    m = cat!(m, " (no change", ign_semi, ")");
                } else {
                    m = cat!(
                        m,
                        " (last change ",
                        ms_s(get("last")),
                        " at ",
                        geo,
                        ign_semi,
                        ")"
                    );
                }
                m
            }
            (b"unsatisfied", b"nothing") => {
                let i = if ign.is_empty() {
                    Vec::new()
                } else {
                    cat!(" (ignored ", ign, ")")
                };
                cat!("nothing changed in ", ms_s(get("t")), i)
            }
            (b"unsatisfied", b"changing") => {
                let mut m = cat!(
                    "still changing after ",
                    ms_s(get("t")),
                    " (last change ",
                    ms_s(get("last")),
                    " at ",
                    geo,
                    ")"
                );
                // Kept from its answer by one small part of the screen (at most a quarter of what
                // was watched: `ready WxH area=N`, finding 231): an animation there is --ignore'd by
                // name (#131).
                let la = dash("late");
                // (`-le "$area"`: test's reading of N; the product, arithmetic's. A number that is
                // none ends the command, said: finding 448, where bash's arithmetic refused an 08.)
                let mut area: Option<Vec<u8>> = None;
                if let Some(re) = shell::Re::new("^([0-9]+)x([0-9]+)( area=([0-9]+))?$") {
                    if let Some(c) = re.captures(&scr, false) {
                        let g = |i: usize| c.get(i).copied().flatten().map(|(a, b)| &scr[a..b]);
                        area = Some(match g(4) {
                            Some(a) => a.to_vec(),
                            None => {
                                let (w, h) = (g(1).unwrap_or_default(), g(2).unwrap_or_default());
                                let w = read(ctx, "the still tool's size", w)?;
                                let h = read(ctx, "the still tool's size", h)?;
                                w.saturating_mul(h).to_string().into_bytes()
                            }
                        });
                    }
                }
                if la != b"-" {
                    if let Some(area) = area {
                        let [_, _, lw, lh] = csv4(&la);
                        let w = read(ctx, "the still tool's late part", &lw)?;
                        let h = read(ctx, "the still tool's late part", &lh)?;
                        let small = shell::test_int(&area)
                            .is_some_and(|a| 4i64.saturating_mul(w).saturating_mul(h) <= a);
                        if small {
                            // (in the window's own coordinates when the --ignore given was, F8)
                            let (ox, oy) = self
                                .origin
                                .clone()
                                .unwrap_or((b"0".to_vec(), b"0".to_vec()));
                            let lo = xywh_less(&la, &ox, &oy);
                            let laz = xywh_words(&la);
                            let t = read(ctx, "the still tool's time", &get("t"))?;
                            let half = ms_s((t / 2).to_string());
                            if self.origin.is_some() {
                                m = cat!(
                                    m,
                                    "; from ",
                                    half,
                                    " on it changed only at ",
                                    laz,
                                    " (the window's ",
                                    lo,
                                    "): --ignore \"",
                                    lo,
                                    "\" if that is an animation"
                                );
                            } else {
                                m = cat!(
                                    m,
                                    "; from ",
                                    half,
                                    " on it changed only at ",
                                    laz,
                                    ": --ignore \"",
                                    laz,
                                    "\" if that is an animation"
                                );
                            }
                        }
                    }
                }
                m
            }
            _ => reason.clone(),
        };
        // run -d --wait (#297, finding 404): a screen that settled after the job had died settled
        // on something else: unsatisfied, so run -d says the job's end and its log as for a 124.
        if res == b"satisfied" && self.facts == "run" && job_failed(ctx, &self.fpid) {
            res = b"unsatisfied".to_vec();
            msg = cat!(msg, "; the command had ended by then: not its screen");
        }
        // --expect tree TEXT (#319): a screen that settled on something else is a 124 too; read
        // until --timeout (from the watch's start), once at least.
        let (mut exp, mut missed) = (Vec::new(), false);
        if !self.expect.is_empty() {
            let mut met: &str = "null";
            let mut said = Vec::new();
            if res == b"satisfied" {
                let tmo = self
                    .args
                    .windows(2)
                    .rev()
                    .find(|a| a[0] == b"--timeout")
                    .and_then(|a| shell::dec(&a[1]))
                    .unwrap_or(10_000);
                let w = crate::treewait::TreeWant {
                    text: self.expect.clone(),
                    sels: self.expect_sels.clone(),
                    layer: self.expect_layer.clone(),
                };
                let q = quoted(&self.expect);
                match crate::treewait::expect(ctx, &w, tmo - (mono_ms() - self.t0)) {
                    Ok((true, l)) => {
                        msg = cat!(msg, "; the tree holds ", q, ": ", l);
                        (met, said) = ("true", l);
                    }
                    Ok((false, why)) => {
                        res = b"unsatisfied".to_vec();
                        msg = cat!(msg, "; but the tree does not hold ", q, " (", why, ")");
                        (met, said, missed) = ("false", why, true);
                    }
                    Err(_) => {
                        res = b"unknown".to_vec();
                        msg = cat!(msg, "; but the tree could not be read for ", q);
                    }
                }
            }
            exp = Args::new(&["-nc"])
                .kv("--arg", "t", &self.expect)
                .kv("--argjson", "m", met)
                .kv("--arg", "d", &said)
                .arg(r#"{tree: $t, met: $m, detail: (if $d == "" then null else $d end)}"#)
                .run(None, jq::Err::Inherit)
                .text();
        }
        // An action's --wait (#274): what changed on the desktop since just before the input; on a
        // 124, where the input went first. Not on unknown (the box may be hung).
        let (mut changes, mut target, mut evs) =
            (b"null".to_vec(), b"null".to_vec(), b"null".to_vec());
        self.looked = false;
        if !self.facts.is_empty() {
            self.snap_take();
            if res != b"unknown" && !self.before.is_empty() {
                let (mut tg, mut tt, mut lw) = (Vec::new(), Vec::new(), Vec::new());
                let after = sys.desk_snap(ctx);
                let fx = desk_facts(&self.before, &after);
                let e = jq::jq(&["-c", ".events"], Some(&self.before), jq::Err::Null);
                evs = if e.ok() { e.text() } else { b"null".to_vec() };
                if res == b"unsatisfied" || json {
                    tg = match self.facts {
                        "run" => {
                            if self.fpid.is_empty() || after.is_empty() {
                                Vec::new()
                            } else {
                                desk_target_with(Target::Run(&self.fpid), &after, || {
                                    sys.kb_layers(ctx)
                                })
                            }
                        }
                        "keys" => {
                            desk_target_with(Target::Keys, &self.before, || sys.kb_layers(ctx))
                        }
                        a => desk_target_with(Target::At(a, &self.fxy), &self.before, || {
                            sys.kb_layers(ctx)
                        }),
                    };
                    if !tg.is_empty() {
                        target = jq::jq(&["-c", "del(.text)"], Some(&tg), jq::Err::Inherit).text();
                        // (run -d's own window is said as opened, among the changes)
                        let k = jq::jq(&["-r", ".kind"], Some(&tg), jq::Err::Inherit);
                        let tk = if k.ok() { k.text() } else { Vec::new() };
                        if !(self.facts == "run" && tk == b"window") {
                            let t = jq::jq(&["-r", ".text"], Some(&tg), jq::Err::Inherit);
                            tt = if t.ok() { t.text() } else { Vec::new() };
                        }
                    }
                }
                if !fx.is_empty() {
                    changes = jq::jq(&["-c", ".changes"], Some(&fx), jq::Err::Inherit).text();
                    lw = jq::jq(&["-r", ".line"], Some(&fx), jq::Err::Inherit).text();
                }
                let _ = &tg;
                if res == b"unsatisfied" {
                    if !tt.is_empty() {
                        msg = cat!(msg, "; ", tt);
                    }
                    if !fx.is_empty() {
                        let l: &[u8] = if lw.is_empty() {
                            b"no window, layer or focus changed"
                        } else {
                            &lw
                        };
                        msg = cat!(msg, "; ", l);
                    }
                    if !tt.is_empty() && !fx.is_empty() {
                        self.looked = true;
                    }
                } else if !lw.is_empty() {
                    msg = cat!(msg, "; ", lw);
                }
            }
        }
        if json {
            let r = Args::new(&["-nc"])
                .kv("--arg", "res", &res)
                .kv("--arg", "cond", cond)
                .kv("--arg", "msg", &msg)
                .kv("--arg", "t", get("t"))
                .kv("--arg", "first", get("first"))
                .kv("--arg", "last", get("last"))
                .kv("--arg", "ch", dash("change"))
                .kv("--arg", "ig", dash("ignored"))
                .kv("--arg", "why", dash("why"))
                .kv("--arg", "frames", get("frames"))
                .kv("--arg", "late", dash("late"))
                .kv("--arg", "facts", self.facts)
                .kv("--argjson", "changes", &changes)
                .kv("--argjson", "target", &target)
                .kv("--argjson", "evs", &evs)
                .kv("--arg", "exp", &exp)
                .arg(crate::jqprog::SETTLE_JSON_JQ)
                .run(None, jq::Err::Inherit);
            // (a write that fails is not settle_end's end: its status is the answer's)
            let _ = out(&r.out);
        } else {
            let _ = out(cat!(res, ": ", msg, "\n"));
        }
        Ok(match res.as_slice() {
            b"satisfied" => 0,
            b"unsatisfied" => {
                if !sent.is_empty() && missed {
                    say(cat!(
                        sent,
                        " all the same (the screen settled; 124 is about the tree): look before sending anything again"
                    ));
                } else if !sent.is_empty() {
                    settle_sent(124, sent, self.looked);
                }
                124
            }
            _ => {
                if !sent.is_empty() {
                    settle_sent(1, sent, self.looked);
                }
                1
            }
        })
    }
}

/// TEXT as a line quotes it: in "double" quotes, or 'single' ones when it holds a double one.
pub fn quoted(text: &[u8]) -> Vec<u8> {
    if text.contains(&b'"') {
        cat!("'", text, "'")
    } else {
        cat!("\"", text, "\"")
    }
}

/// settle_sent RC SENT: what an action did when the answer was not yes.
pub fn settle_sent(rc: i32, sent: &[u8], looked: bool) {
    match rc {
        0 => {}
        // (#274: where the input went, and that nothing on the desktop moved, already said)
        124 => {
            if looked {
                say(cat!(
                    sent,
                    " all the same (124 is about the screen): look before sending anything again"
                ));
            } else {
                say(cat!(
                    sent,
                    " all the same (124 is about the screen): take a shot before sending anything again"
                ));
            }
        }
        _ => say(cat!(sent, " before the screen could be watched to the end")),
    }
}

/// A thread that may write to a pipe whose reader is gone gets EPIPE, not the process's SIGPIPE.
fn block_sigpipe() {
    // SAFETY: a signal set of our own, for this thread's mask only.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGPIPE);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }
}

/// A watch as settle_begin leaves it, for a test: its first line LINE read already, the tool's
/// output OUT, the thread that runs it and the pid it records.
#[cfg(test)]
impl Settle {
    fn given(line: &[u8], out: OwnedFd, thread: JoinHandle<()>, pid: Arc<AtomicI32>) -> Settle {
        Settle {
            line: line.to_vec(),
            t0: mono_ms(),
            tool: Some(Tool {
                input: None,
                lines: Lines::new(out),
                thread,
                pid,
            }),
            ..Settle::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #377: kill_tree ends a child of ours and what is under it, and leaves a pid that is no child
    /// of ours (as one reaped and taken since it was stored would be).
    /// (#[ignore]: it starts and kills processes; test/run.sh's unit tier runs it.)
    #[test]
    #[ignore = "starts processes: run by test/run.sh (cargo test -- --ignored)"]
    fn kill_tree_ends_only_our_children() {
        let mut sh = std::process::Command::new("sh")
            .args(["-c", "sleep 300 & echo $!; wait"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(sh.stdout.take().unwrap()),
            &mut line,
        )
        .unwrap();
        let gc = line.trim().as_bytes().to_vec();
        let running = |p: &[u8]| procfs::running(p);
        kill_tree(&gc);
        std::thread::sleep(Duration::from_millis(200));
        let left = running(&gc);
        kill_tree(sh.id().to_string().as_bytes());
        let _ = sh.wait();
        let end = Instant::now() + Duration::from_secs(3);
        while running(&gc) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        let ended = !running(&gc);
        assert!(left, "a pid that is no child of ours: left");
        assert!(ended, "...our child's tree: ended, its child too");
    }

    #[test]
    fn geometry() {
        assert_eq!(
            geom_parse(b"10,20 30x40").map(|g| geom_csv(&g)),
            Some(b"10,20,30,40".to_vec())
        );
        assert_eq!(
            geom_parse(b"-1,2,3,4").map(|g| geom_csv(&g)),
            Some(b"-1,2,3,4".to_vec())
        );
        assert_eq!(geom_parse(b"1,2 0x4"), None);
        assert_eq!(geom_parse(b"1,2 3x4 "), None);
        // (as read, in grim's form: #384)
        assert_eq!(
            geom_parse(b"00,-010 300,200").map(|g| geom_text(&g)),
            Some(b"0,-10 300x200".to_vec())
        );
        assert_eq!(rects_meet(b"0,0,10,10", b"5,5,10,10"), Some(true));
        assert_eq!(rects_meet(b"0,0,10,10", b"10,0,10,10"), Some(false));
        // (decimal: finding 448; bash read 010 as 8 and refused 08)
        assert_eq!(rects_meet(b"08,0,10,10", b"0,0,1,1"), Some(false));
        assert_eq!(rects_meet(b"010,0,1,1", b"8,0,1,1"), Some(false));
        assert_eq!(rects_meet(b"010,0,1,1", b"10,0,1,1"), Some(true));
        assert_eq!(rects_meet(b"x,0,1,1", b"10,0,1,1"), None);
        assert_eq!(geom_parse(b"99999999999999999999,0 1x1"), None);
        assert_eq!(cursor_rect(b"100 200"), Some(b"84,184,64,64".to_vec()));
        assert_eq!(cursor_rect(b"-5 3"), Some(b"-21,-13,64,64".to_vec()));
    }

    // t_unit_wait's -g, geom_clip, durations and the cursor's rectangle.
    #[test]
    fn wait_values_as_bash() {
        let g = |v: &str| {
            geom_parse(v.as_bytes()).map(|g| g.map(|x| String::from_utf8(x).unwrap()).join(" "))
        };
        assert_eq!(g("1,2 30x40").as_deref(), Some("1 2 30 40"), "-g X,Y WxH");
        assert_eq!(g("1,2,30,40").as_deref(), Some("1 2 30 40"), "-g X,Y,W,H");
        assert_eq!(g("-1,2 30,40").as_deref(), Some("-1 2 30 40"), "-g X,Y W,H");
        assert_eq!(g("1,2 0x4"), None, "-g junk refused");
        let clip = |r: [i64; 4], lw: i64, lh: i64| {
            crate::shots::geom_clip(r, lw, lh).map(|(r, cut)| {
                format!(
                    "{} {} {} {}\t{}",
                    r[0],
                    r[1],
                    r[2],
                    r[3],
                    String::from_utf8(cut).unwrap()
                )
            })
        };
        assert_eq!(
            clip([10, 20, 30, 40], 100, 100).as_deref(),
            Some("10 20 30 40\t"),
            "geom_clip: inside, nothing cut"
        );
        assert_eq!(
            clip([1380, 40, 480, 330], 1854, 1056).as_deref(),
            Some("1380 40 474 330\t6 px past the right edge"),
            "geom_clip: 6 px past the right edge"
        );
        assert_eq!(
            clip([-5, -3, 120, 60], 100, 50).as_deref(),
            Some(
                "0 0 100 50\t5 px past the left edge, 3 px past the top edge, 15 px past the right edge, 7 px past the bottom edge"
            ),
            "geom_clip: every edge"
        );
        assert_eq!(
            clip([100, 0, 10, 10], 100, 50),
            None,
            "geom_clip: nothing inside (right of it)"
        );
        assert_eq!(
            clip([0, -10, 10, 10], 100, 50),
            None,
            "geom_clip: nothing inside (above it)"
        );
        let ms = |v: &str| crate::num::ms_duration(v.as_bytes());
        assert_eq!(ms("300ms"), Some(300), "300ms");
        assert_eq!(ms("1.5s"), Some(1500), "1.5s");
        assert_eq!(ms("2m"), Some(120_000), "2m");
        assert_eq!(ms("10"), Some(10_000), "a bare number is seconds");
        assert_eq!(ms("0"), None, "0 refused");
        assert_eq!(ms("2h"), Some(7_200_000), "2h (#273: hours too)");
        assert_eq!(ms("2d"), None, "junk refused");
        assert_eq!(
            ms("2"),
            Some(2000),
            "drag --hold: a bare number is seconds (t_unit_cli's)"
        );
        assert_eq!(
            cursor_rect(b"960 540"),
            Some(b"944,524,64,64".to_vec()),
            "the cursor's rectangle"
        );
    }

    // t_unit_config's ws_text: a special workspace says how it shows and hides.
    #[test]
    fn ws_text_as_bash() {
        let t: Vec<String> = ["special:omabox", "special:scratchpad", "9"]
            .iter()
            .map(|w| String::from_utf8(ws_text(w.as_bytes())).unwrap())
            .collect();
        assert_eq!(
            t.join("|"),
            "special workspace omabox, which the bar widget's Show opens and a workspace switch hides|the scratchpad (SUPER+S shows it)|workspace 9"
        );
    }

    #[test]
    fn options_as_settle_opt() {
        let c = Ctx::new(b"run", b"/x".to_vec());
        let mut s = Settle::default();
        let w = |v: &[&str]| v.iter().map(|a| a.as_bytes().to_vec()).collect::<Vec<_>>();
        s.opt(&c, &w(&["--timeout", "2s"])).ok();
        assert_eq!(s.took, 2);
        s.opt(&c, &w(&["--wait"])).ok();
        assert_eq!((s.took, s.wait), (1, true));
        s.opt(&c, &w(&["-g", "1,2 3x4"])).ok();
        s.opt(&c, &w(&["--other"])).ok();
        assert_eq!(s.took, 0);
        assert_eq!(s.args, w(&["--timeout", "2000", "--region", "1,2,3,4"]));
        assert!(s.in_window(&c, b"10", b"20").is_ok());
        assert_eq!(s.args[3], b"11,22,3,4");
        assert_eq!(ws_text(b"3"), b"workspace 3");
        assert_eq!(
            ws_text(b"special:scratchpad"),
            b"the scratchpad (SUPER+S shows it)"
        );
    }

    /// t_unit_json's settle_in_window (F8, #287): -g's and --ignore's regions (--region, --mask)
    /// moved by the window's place, the cursor's --ignore and the rest kept, the origin kept; and
    /// settle_opts_check given a --window marks it, none does not.
    #[test]
    fn in_window_and_opts_check_as_bash() {
        let c = Ctx::new(b"wait", b"/x".to_vec());
        let w = |v: &[&str]| v.iter().map(|a| a.as_bytes().to_vec()).collect::<Vec<_>>();
        let mut s = Settle {
            args: w(&[
                "--region",
                "10,5,9,9",
                "--mask",
                "1,1,2,2",
                "--quiet",
                "300",
                "--ignore",
                "5,5,64,64",
                "--strict",
            ]),
            ..Settle::default()
        };
        assert!(s.in_window(&c, b"100", b"50").is_ok());
        assert_eq!(
            s.args,
            w(&[
                "--region",
                "110,55,9,9",
                "--mask",
                "101,51,2,2",
                "--quiet",
                "300",
                "--ignore",
                "5,5,64,64",
                "--strict",
            ])
        );
        assert_eq!(s.origin, Some((b"100".to_vec(), b"50".to_vec())));
        let mut s = Settle {
            wait: true,
            ..Settle::default()
        };
        assert!(s.opts_check(&c, 1).is_ok());
        let a = s.window;
        s.window = false;
        assert!(s.opts_check(&c, 0).is_ok());
        assert_eq!((a, s.window), (true, false));
    }

    /// The box as bash's stubs had it: up (box_alive), its Hyprland not answering (hyprctl's 6) with
    /// no process to name, the desktop after AFTER, no layer holding the keyboard (kb_layers).
    struct Stub {
        after: Vec<u8>,
    }

    impl BoxSys for Stub {
        fn box_alive(&self, _: &Ctx) -> Res<bool> {
            Ok(true)
        }
        fn box_pids(&self, _: &Ctx) -> Vec<Vec<u8>> {
            Vec::new()
        }
        fn on_box_io(&self, _: &Ctx, _: &[Vec<u8>], _: Out, _: ErrTo) -> crate::exec::Ran {
            crate::exec::Ran {
                rc: 6,
                out: Vec::new(),
                refused: false,
                gone_said: false,
            }
        }
        fn desk_snap(&self, _: &Ctx) -> Vec<u8> {
            self.after.clone()
        }
        fn kb_layers(&self, _: &Ctx) -> Vec<u8> {
            Vec::new()
        }
    }

    fn sctx() -> Ctx {
        let mut c = Ctx::new(b"wait", shell::bytes(&std::env::temp_dir()).to_vec());
        c.parsed();
        c.name = b"u".to_vec();
        c.d = b"/nonexistent".to_vec();
        c
    }

    /// The tool's output: PARTS written one after the other, PAUSE apart, then its end (EOF).
    fn tool_says(parts: &[&str], pause: Duration) -> OwnedFd {
        let (r, w) = pipe().unwrap();
        let parts: Vec<String> = parts.iter().map(|p| (*p).to_string()).collect();
        std::thread::spawn(move || {
            let mut f = std::fs::File::from(w);
            for (i, p) in parts.iter().enumerate() {
                if i > 0 {
                    std::thread::sleep(pause);
                }
                let _ = std::io::Write::write_all(&mut f, p.as_bytes());
            }
        });
        r
    }

    /// settle_end COND JSON "" on a watch whose first line is LINE and whose tool then says PARTS:
    /// what it printed and said, then "rc N".
    fn ended(st: &mut Settle, sys: &Stub, cond: &str, json: bool, sent: &[u8]) -> String {
        let ctx = sctx();
        let (rc, said) = crate::exit::heard_all(|| st.end_with(sys, &ctx, cond, json, sent));
        format!("{said}rc {}", rc.unwrap())
    }

    fn watch(line: &str, parts: &[&str], pause: Duration) -> Settle {
        Settle::given(
            line.as_bytes(),
            tool_says(parts, pause),
            std::thread::spawn(|| {}),
            Arc::new(AtomicI32::new(0)),
        )
    }

    // t_unit_settle_read (finding 217, #310): an answer, or a ready line, split across a read's
    // second keeps every part, a space included.
    #[test]
    fn split_answers_read_whole() {
        let none = Stub { after: Vec::new() };
        let rest = "t=2002 first=2 last=1970 change=27,31,7,10 ignored=- why=- late=- frames=42\n";
        let mut st = watch(
            "ready 1920x1080",
            &["u", &format!("nsatisfied changing {rest}")],
            Duration::from_millis(1500),
        );
        let out = ended(&mut st, &none, "still", false, b"");
        assert!(
            out.starts_with("unsatisfied: still changing after 2.00s"),
            "an answer split across a read timeout is read whole: {out}"
        );
        assert!(out.ends_with("rc 124"), "...124: {out}");
        let mut st = watch(
            "ready 1920x1080",
            &["unsatisfied changing ", rest],
            Duration::from_millis(1500),
        );
        let out = ended(&mut st, &none, "still", false, b"");
        assert!(
            out.starts_with("unsatisfied: still changing after 2.00s"),
            "an answer split at a space keeps it (#310): {out}"
        );
        for (parts, what) in [
            (
                ["r", "eady 1920x1080\n"],
                "settle_begin: a ready line split across a read timeout too",
            ),
            (
                ["ready ", "1920x1080\n"],
                "settle_begin: a ready line split at a space too",
            ),
        ] {
            let mut st = watch("", &parts, Duration::from_millis(1500));
            st.first_line(after(12));
            assert_eq!(
                (st.line.as_slice(), st.err.as_slice()),
                (&b"ready 1920x1080"[..], &b""[..]),
                "{what}"
            );
        }
    }

    // t_unit_wait's (#131, #162, finding 231): the 124's hint names a small region that kept
    // changing late as an --ignore; not a large one, nor over a quarter of what several monitors
    // cover.
    #[test]
    fn hint_as_bash() {
        let none = Stub { after: Vec::new() };
        let hint = |ready: &str, answer: &str| {
            let mut st = watch(ready, &[&format!("{answer}\n")], Duration::ZERO);
            ended(&mut st, &none, "still", false, b"")
        };
        assert_eq!(
            hint(
                "ready 1920x1080",
                "unsatisfied changing t=10000 first=10 last=9990 change=28,90,14,900 ignored=- why=- late=27,81,15,918 frames=500"
            ),
            "unsatisfied: still changing after 10.00s (last change 9.99s at 28,90 14x900); from 5.00s on it changed only at 27,81 15x918: --ignore \"27,81 15x918\" if that is an animation\nrc 124",
            "a 124 still changing in one small region: named as an --ignore"
        );
        assert_eq!(
            hint(
                "ready 1920x1080",
                "unsatisfied changing t=10000 first=10 last=9990 change=0,0,1920,1080 ignored=- why=- late=0,0,1000,1080 frames=500"
            ),
            "unsatisfied: still changing after 10.00s (last change 9.99s at 0,0 1920x1080)\nrc 124",
            "...over a quarter of the screen: no hint"
        );
        assert_eq!(
            hint(
                "ready 3000x2370 area=3240000",
                "unsatisfied changing t=10000 first=10 last=9990 change=1920,450,1080,900 ignored=- why=- late=1920,450,1080,900 frames=500"
            ),
            "unsatisfied: still changing after 10.00s (last change 9.99s at 1920,450 1080x900)\nrc 124",
            "...several monitors: a quarter of their own area (area=), not of the box around them"
        );
    }

    // t_unit_wait (#124, finding 181): a tool that never ends (and never answered) is killed after
    // 2 s, with what runs it; a box whose Hyprland does not answer either is said so.
    #[test]
    #[ignore = "starts a process: run by test/run.sh (cargo test -- --ignored)"]
    fn a_tool_that_never_ends_is_killed() {
        let none = Stub { after: Vec::new() };
        let (r, w) = pipe().unwrap();
        drop(w);
        let mut child = std::process::Command::new("sleep")
            .arg("300")
            .spawn()
            .unwrap();
        let pid = Arc::new(AtomicI32::new(i32::try_from(child.id()).unwrap()));
        let cpid = child.id();
        let thread = std::thread::spawn(move || {
            let _ = child.wait();
        });
        let mut st = Settle::given(b"", r, thread, pid);
        let t0 = Instant::now();
        let out = ended(&mut st, &none, "still", false, b"");
        let s = t0.elapsed().as_secs();
        assert_eq!(
            out.lines().next().unwrap_or_default(),
            "unknown: box 'u': its Hyprland did not answer (hung? omabox log -b u; omabox down u)",
            "settle_end: a tool that never ends is not waited for (#124)"
        );
        let gone = !std::path::Path::new(&format!("/proc/{cpid}")).exists();
        assert_eq!(
            (
                out.lines().last().unwrap_or_default(),
                (2..=6).contains(&s),
                gone
            ),
            ("rc 1", true, true),
            "...exit 1 within seconds ({s} s), the tool killed"
        );
    }

    /// wf_snap JQ: the base snapshot of t_unit_wait_facts (foot "K" focused on workspace 1, the bar
    /// drawn, the launcher mapped at 1x1) changed by JQ.
    fn wf_snap(m: &str) -> Vec<u8> {
        let base = r#"{events: 100,
    clients: [{address: "0xa", mapped: true, hidden: false, class: "foot", title: "K", pid: 40, workspace: {id: 1, name: "1"},
      at: [0, 0], size: [960, 1080], floating: false, fullscreen: 0}],
    layers: {"HEADLESS-2": {levels: {"0": [{address: "0x1", namespace: "omarchy-background", x: 0, y: 0, w: 1920, h: 1080}], "1": [],
      "2": [{address: "0x2", namespace: "omarchy-bar", x: 0, y: 1046, w: 1920, h: 34}],
      "3": [{address: "0x3", namespace: "omarchy-menu", x: 0, y: 0, w: 1, h: 1}]}}},
    active: {address: "0xa", class: "foot", title: "K", workspace: {id: 1, name: "1"}},
    workspace: {id: 1, name: "1"},
    monitors: [{id: 0, activeWorkspace: {id: 1}, specialWorkspace: {id: 0}}]}"#;
        let r = jq::jq(
            &["-c", "-n", &format!("{base} | {m}")],
            None,
            jq::Err::Inherit,
        );
        assert!(r.ok());
        r.text()
    }

    fn jqv(js: &str, prog: &str) -> String {
        let line = js.lines().next().unwrap_or_default();
        String::from_utf8_lossy(
            &jq::jq(&["-c", prog], Some(line.as_bytes()), jq::Err::Inherit).text(),
        )
        .into_owned()
    }

    // t_unit_wait_facts (#274): settle_end with the facts. The line keeps its start, the facts after
    // it; JSON adds changes, target and events_since; the exit code is the screen's.
    #[test]
    fn facts_as_bash() {
        let menu = r#".layers["HEADLESS-2"].levels["3"] = [{address: "0x4", namespace: "omarchy-menu", x: 660, y: 300, w: 600, h: 480}]"#;
        let zen = r#"{address: "0xb", mapped: true, hidden: false, class: "zenity", title: "Save as", pid: 41, workspace: {id: 1, name: "1"},
    at: [960, 0], size: [960, 1080], floating: false, fullscreen: 0}"#;
        let s = wf_snap(&format!(
            ".clients += [{zen} | .floating = true | .at = [800, 400] | .size = [300, 200]]"
        ));
        let yes = "satisfied settled t=540 first=10 last=240 change=660,300,600,480 ignored=- why=- frames=30";
        let nothing =
            "unsatisfied nothing t=2000 first=- last=- change=- ignored=- why=- frames=60";
        let se = |answer: &str, facts: &'static str, before: &[u8], after: &[u8], json: bool| {
            let mut st = watch("ready 1920x1080", &[&format!("{answer}\n")], Duration::ZERO);
            st.facts = facts;
            st.fxy = b"900 450".to_vec();
            st.before = before.to_vec();
            let sys = Stub {
                after: after.to_vec(),
            };
            ended(&mut st, &sys, "settle", json, b"the keys were sent")
        };
        assert_eq!(
            se(yes, "keys", &wf_snap("."), &wf_snap(menu), false),
            "satisfied: settled after 0.54s (last change 0.24s at 660,300 600x480); layer omarchy-menu opened\nrc 0",
            "settle_end: the facts after the screen's words"
        );
        assert_eq!(
            se(nothing, "keys", &s, &s, false),
            "unsatisfied: nothing changed in 2.00s; the keys went to foot \"K\" (focused); no window, layer or focus changed\nomabox: the keys were sent all the same (124 is about the screen): look before sending anything again\nrc 124",
            "...a 124: where the keys went, and that nothing moved; look before sending again"
        );
        assert_eq!(
            se(nothing, "click", &s, b"", false),
            "unsatisfied: nothing changed in 2.00s; the click at 900,450 was on zenity \"Save as\" (its 100,50)\nomabox: the keys were sent all the same (124 is about the screen): take a shot before sending anything again\nrc 124",
            "...the desktop not read after: where the click went, nothing claimed; a shot as before"
        );
        assert_eq!(
            se(yes, "keys", b"", &s, false),
            "satisfied: settled after 0.54s (last change 0.24s at 660,300 600x480)\nrc 0",
            "...nor before: the line as it was"
        );
        let out = se(yes, "keys", &wf_snap("."), &wf_snap(menu), true);
        assert_eq!(
            jqv(&out, "keys_unsorted | join(\" \")"),
            "\"result condition message elapsed first_change last_change change late_changes ignored frames changes target events_since\"",
            "--json: today's fields, then changes, target and events_since"
        );
        assert_eq!(
            jqv(
                &out,
                "[.changes, .target.kind, .target.window.class, .events_since]"
            ),
            r#"[[{"kind":"layer_opened","namespace":"omarchy-menu","at":{"x":660,"y":300,"w":600,"h":480}}],"window","foot",100]"#,
            "...their values"
        );
        assert_eq!(
            jqv(&out, ".message"),
            "\"settled after 0.54s (last change 0.24s at 660,300 600x480); layer omarchy-menu opened\"",
            "...the message has the facts too"
        );
        let out = se(yes, "keys", &s, b"", true);
        assert_eq!(
            jqv(&out, ".changes"),
            "null",
            "...changes null when the desktop could not be read after"
        );
        let out = se(yes, "", &s, &wf_snap(menu), true);
        assert_eq!(
            jqv(&out, "keys_unsorted | join(\" \")"),
            "\"result condition message elapsed first_change last_change change late_changes ignored frames\"",
            "...wait (no action): today's fields only"
        );
    }
}
