//! `omabox _reap NAME CREATED`: the idle reaper (finding 59), a host process per box `up` starts
//! (setsid, its output in the box dir's reap.log). Agents forget `down`, and a box holds 400-700 MB
//! and a GPU client. Idle = no omabox command naming the box (need_box and `run` touch $D/used), no
//! peek window on it, and no `omabox run` still running in it. `run`'s throwaway boxes get one too,
//! with no idle limit but an owner: the `omabox run` process; when that is gone (SIGKILLed, so its
//! teardown never ran), so is the box. An agent session's own box (finding 93) goes when the agent's
//! process does. An interactive box closed by its user is cleared; one that died any other way stays,
//! logs and all, until `down` (finding 71). Exits when the box goes, or when a new box took the name.
//! Its command line is `ROOT/bin/omabox _reap NAME CREATED` (up's self-spawn; bash's hand-off made
//! it `ROOT/build/omabox _reap ...` before the switch), which kill_reapers knows; it also writes its
//! pid and start time in the box dir (`reaper.pid`), so a down by another omabox ends it (#383).

use crate::boxes::{BoxPid, agent_alive, box_alive, box_pid, meta, touch};
use crate::cat;
use crate::cmd::down::{DownIf, down_one, prune_marks};
use crate::exit::{Ctx, Res, muted, say};
use crate::ready::owner_alive;
use crate::select::select_box;
use crate::shell;
use std::time::Duration;

/// share/passthrough.lua's VERSION, read from the file at build time (#368: one number, never a
/// copy that a bump leaves behind).
const PASS_VERSION: u32 = lua_version(include_bytes!("../../../../share/passthrough.lua"));
// (no build without it read)
const _: () = assert!(
    PASS_VERSION > 0,
    "no `local VERSION = N` in share/passthrough.lua"
);

/// The N of the first `local VERSION = N` line in a Lua file (0 when there is none).
const fn lua_version(s: &[u8]) -> u32 {
    let k = b"\nlocal VERSION = ";
    let mut i = 0;
    while i + k.len() < s.len() {
        let mut j = 0;
        while j < k.len() && s[i + j] == k[j] {
            j += 1;
        }
        if j == k.len() {
            let (mut n, mut p) = (0u32, i + j);
            while p < s.len() && s[p].is_ascii_digit() {
                n = n * 10 + (s[p] - b'0') as u32;
                p += 1;
            }
            return n;
        }
        i += 1;
    }
    0
}

/// keys_ensure's Lua: whether this omabox's passthrough hooks (PASS_VERSION) are on the host.
fn pass_check() -> String {
    format!("if (omabox_pass_version or 0) < {PASS_VERSION} then error('omabox-missing', 0) end")
}

/// What the reaper keeps of its box from the start (cmd__reap's locals).
struct Reap {
    created: Vec<u8>,
    idle: i64,
    owner: Vec<u8>,
    owner_start: Vec<u8>,
    interactive: bool,
    agent: Vec<u8>,
    retry_said: bool,
    host: Option<Vec<u8>>,
    /// A connected box with protected ports (box.json `protect`): the host's addresses in its rule
    /// kept the host's (#399).
    guard: Option<Guard>,
}

/// What the reaper keeps to keep a box's protected ports' rule on the host's addresses (#399): the
/// rule pasta's shell set at up has the addresses the host had then, and a server listening on all
/// of them is reached on any the host gains after (a VPN that comes up, a docker network made).
struct Guard {
    /// The host's address events (none when the socket cannot be had: then a look every second).
    events: Option<crate::net::AddrEvents>,
    /// The addresses last set in the rule (none yet: the first look sets them, whatever they are).
    last: Option<(Vec<String>, Vec<String>)>,
    /// A failure said in reap.log, not said again until one works.
    said: bool,
    /// When looks are due (nap).
    sched: Sched,
}

/// When a guard looks at the host's addresses (#399): at once on an address event, but at most one
/// look per GAP (a burst of changes is a few looks, each a fib read, a dump and maybe an nft run in
/// every protected box's reaper), with one more look GAP after the last event (an IPv4 address gone
/// is still in fib_trie for a moment after its event: net::host_addrs); and while the last look
/// failed (pasta not up yet, nft refusing or hung), again after GAP, doubling to FAILING_MAX, not at
/// the next poll (up to 60 s), with no event needed.
#[derive(Default)]
struct Sched {
    /// No look before this (the last one's time + GAP, or its failure's wait).
    next_ok: Option<std::time::Instant>,
    /// An event came that no look has followed yet.
    pending: bool,
    /// The trailing look's time, after the last event.
    trailing: Option<std::time::Instant>,
    /// Looks failed in a row.
    failed: u32,
}

const GAP: Duration = Duration::from_millis(250);
const FAILING_MAX: Duration = Duration::from_secs(2);
/// How long the reaper's nft may take (it takes milliseconds).
const NFT_WAIT: Duration = Duration::from_secs(5);

impl Sched {
    /// An event (any number of them) seen at NOW.
    fn event(&mut self, now: std::time::Instant) {
        self.pending = true;
        self.trailing = Some(now + GAP);
    }

    /// Whether a look is wanted at all (now or later), and when it may be: None when none is.
    fn due(&self) -> Option<std::time::Instant> {
        let want = self.pending || self.failed > 0 || self.trailing.is_some();
        if !want {
            return None;
        }
        let mut at = self.next_ok;
        if !self.pending
            && self.failed == 0
            && let Some(t) = self.trailing
        {
            at = Some(at.map_or(t, |a| a.max(t)));
        }
        Some(at.unwrap_or_else(std::time::Instant::now))
    }

    /// A look made at NOW, and whether it worked.
    fn looked(&mut self, now: std::time::Instant, ok: bool) {
        self.pending = false;
        if self.trailing.is_some_and(|t| now >= t) {
            self.trailing = None;
        }
        if ok {
            self.failed = 0;
            self.next_ok = Some(now + GAP);
        } else {
            self.failed = self.failed.saturating_add(1);
            let wait = GAP
                .saturating_mul(1u32 << self.failed.saturating_sub(1).min(4))
                .min(FAILING_MAX);
            self.next_ok = Some(now + wait);
        }
    }
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let name = args.first().cloned().unwrap_or_default();
    select_box(ctx, &name)?;
    let d = ctx.d.clone();
    let idle = meta(&d, "idle").0;
    let mut r = Reap {
        created: args.get(1).cloned().unwrap_or_default(),
        idle: if idle.is_empty() {
            0
        } else {
            shell::test_int(&idle).unwrap_or(0)
        },
        owner: meta(&d, "owner").0,
        owner_start: meta(&d, "owner_start").0,
        interactive: meta(&d, "mode").0 == b"interactive",
        agent: meta(&d, "agent").0,
        retry_said: false,
        host: None,
        guard: (meta(&d, "protect").0 == b"true").then(|| Guard {
            events: crate::net::AddrEvents::open(),
            last: None,
            said: false,
            sched: Sched::default(),
        }),
    };
    if r.idle <= 0
        && r.owner.is_empty()
        && r.agent.is_empty()
        && !r.interactive
        && r.guard.is_none()
    {
        return Ok(());
    }
    // (its pid and start time in the box dir: a down by another omabox ends it too, #383)
    crate::ready::record_reaper(ctx);
    let every = poll_every(r.idle, !r.agent.is_empty(), r.interactive);
    // Its host session, found once (keys_ensure asks it every poll).
    if r.interactive {
        r.host = muted(|| crate::host::host_session(ctx)).ok();
    }
    if !wait_pidns(ctx, &r.created) {
        return Ok(());
    }
    // (said once, in reap.log: how often it looks, which nothing else shows)
    say(cat!(
        shell::date_iso(),
        " box '",
        ctx.name,
        "': watching it every ",
        every.to_string(),
        " s",
        if r.guard.is_some() {
            ", and the host's addresses for its protected ports as they change"
        } else {
            ""
        }
    ));
    // (the host's addresses now, which may have changed since up read them)
    if let Some(g) = r.guard.as_mut() {
        guard_look(ctx, g);
    }
    loop {
        // (every 2 s while its covers wait for their check: a box held by an up that is gone goes
        // soon)
        let secs = if ctx.at("covers-pending").exists() {
            2
        } else {
            every
        };
        nap(ctx, &mut r, secs);
        if !poll(ctx, &mut r)? {
            return Ok(());
        }
        // (at every poll too: a failure tried again, an event the socket lost)
        if let Some(g) = r.guard.as_mut() {
            guard_look(ctx, g);
        }
    }
}

/// The sleep between polls, SECS long; with a guard, its addresses looked at when its Sched says:
/// woken by the host's address events (every second when there is no event socket).
fn nap(ctx: &Ctx, r: &mut Reap, secs: u64) {
    let Some(g) = r.guard.as_mut() else {
        crate::sig::sleep(Duration::from_secs(secs));
        return;
    };
    let end = std::time::Instant::now() + Duration::from_secs(secs);
    let mut ticks = 0u32;
    loop {
        let now = std::time::Instant::now();
        if now >= end {
            return;
        }
        let mut step = (end - now).min(Duration::from_millis(100));
        if let Some(at) = g.sched.due() {
            step = step.min(at.saturating_duration_since(now));
        }
        let event = match &g.events {
            Some(e) => {
                e.wait(step);
                // (a stop on its way parks here, as sig::sleep does)
                crate::sig::sleep(Duration::ZERO);
                e.any()
            }
            None => {
                crate::sig::sleep(step);
                ticks += 1;
                ticks % 10 == 0
            }
        };
        let now = std::time::Instant::now();
        if event {
            g.sched.event(now);
        }
        if g.sched.due().is_some_and(|at| now >= at) {
            guard_look(ctx, g);
        }
    }
}

/// The host's addresses into the box's protected ports' rule when they are not what was last set
/// there. A failure (pasta not up yet, the box going down, nft refusing or hung) is said once in
/// reap.log and tried again soon (Sched).
fn guard_look(ctx: &Ctx, g: &mut Guard) {
    let now = crate::net::host_addrs();
    if g.last.as_ref() == Some(&now) {
        g.sched.looked(std::time::Instant::now(), true);
        return;
    }
    let res = guard_set(&ctx.d, &now.0, &now.1);
    g.sched.looked(std::time::Instant::now(), res.is_ok());
    match res {
        Ok(()) => {
            if g.said {
                say(cat!(
                    when(),
                    " box '",
                    ctx.name,
                    "': its protected ports' addresses set again"
                ));
            }
            g.last = Some(now);
            g.said = false;
        }
        Err(why) => {
            if !g.said {
                say(cat!(
                    when(),
                    " box '",
                    ctx.name,
                    "': could not set the host's addresses in its protected ports' rule (",
                    why,
                    ")"
                ));
            }
            g.said = true;
        }
    }
}

/// net::protect_sets with V4 and V6, run by nft in pasta's user and network namespaces (where pasta's
/// shell set the rule at up). Err: why not, nft's first line when it was nft.
fn guard_set(d: &[u8], v4: &[String], v6: &[String]) -> Result<(), Vec<u8>> {
    use std::io::Write;
    let e = crate::enter::Entry::pasta(d).map_err(|w| {
        shell::strip_nl(w.strip_prefix(crate::enter::FAIL.as_bytes()).unwrap_or(&w))
    })?;
    // (only its stdio: nothing else of the reaper's goes into pasta's namespaces)
    let argv = [
        cat!(crate::cmd::up_box::LAUNCH_PATH, "/nft"),
        b"-f".to_vec(),
        b"-".to_vec(),
    ];
    let fds = crate::exec::Fds {
        only: true,
        enter: Some(e.plan(false, true, None)),
        ..Default::default()
    };
    let mut c = crate::exec::command(&argv, fds);
    c.env_clear()
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let mut ch = c.spawn().map_err(|x| x.to_string().into_bytes())?;
    if let Some(mut i) = ch.stdin.take() {
        let _ = i.write_all(crate::net::protect_sets(v4, v6).as_bytes());
    }
    // (an nft that hangs is killed: the reaper has its box's polls to keep, its down among them)
    let end = std::time::Instant::now() + NFT_WAIT;
    let status = loop {
        match ch.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if std::time::Instant::now() < end => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = ch.kill();
                let _ = ch.wait();
                return Err(cat!(
                    "nft did not answer in ",
                    NFT_WAIT.as_secs().to_string(),
                    " s: killed"
                ));
            }
            Err(x) => return Err(x.to_string().into_bytes()),
        }
    };
    if status.success() {
        return Ok(());
    }
    // (what it said, read without waiting: anything it left holding the pipe cannot stall this)
    let mut err = Vec::new();
    if let Some(mut e) = ch.stderr.take() {
        use std::io::Read;
        use std::os::fd::AsRawFd;
        // SAFETY: fcntl on our pipe's fd, setting O_NONBLOCK on it.
        unsafe {
            let fl = libc::fcntl(e.as_raw_fd(), libc::F_GETFL);
            libc::fcntl(e.as_raw_fd(), libc::F_SETFL, fl | libc::O_NONBLOCK);
        }
        let _ = e.read_to_end(&mut err);
    }
    let first = err.split(|&c| c == b'\n').next().unwrap_or_default();
    Err(if first.is_empty() {
        b"nft failed".to_vec()
    } else {
        first.to_vec()
    })
}

/// How often it looks: a quarter of the idle limit, 5 s to 60 s; with no idle limit and only its
/// agent to watch, as often as a 2 h box; an interactive box, every 2 s (one closed by its user goes
/// from the widget and `ls` within a poll or so).
fn poll_every(idle: i64, agent: bool, interactive: bool) -> u64 {
    if interactive {
        return 2;
    }
    if idle <= 0 && agent {
        return 60;
    }
    u64::try_from((idle / 4).clamp(5, 60)).unwrap_or(60)
}

/// `starting`'s "PID START" (the up still running), as owner_alive takes it.
fn starting_alive(ctx: &Ctx) -> bool {
    let st = std::fs::read(ctx.at("starting"))
        .map(|v| shell::strip_nl(&v))
        .unwrap_or_default();
    if st.is_empty() {
        return false;
    }
    let (p, s) = match st.iter().position(|&c| c == b' ') {
        Some(i) => (&st[..i], &st[i + 1..]),
        None => (&st[..], &st[..]),
    };
    owner_alive(p, s)
}

fn when() -> Vec<u8> {
    shell::date_iso()
}

/// One poll: whether to go on watching. Err ends the reaper (a box this namespace cannot read, a
/// down's refusal).
fn poll(ctx: &mut Ctx, r: &mut Reap) -> Res<bool> {
    let name = ctx.name.clone();
    if meta(&ctx.d, "created").0 != r.created {
        return Ok(false);
    }
    let down = |ctx: &mut Ctx, agent_gone: bool| -> Res<bool> {
        let cond = DownIf {
            created: Some(r.created.clone()),
            agent_gone,
        };
        down_one(ctx, &name, &cond)
    };
    // Every down here may fail (its lock busy for 60 s, PID 1 slow to exit under load): the reaper
    // then tries again at its next poll (retry) instead of ending (finding 162).
    if !box_alive(ctx)? {
        let owner_gone = !r.owner.is_empty() && !owner_alive(&r.owner, &r.owner_start);
        if owner_gone {
            down(ctx, false)?;
        } else if r.interactive && ctx.at("run/omabox.closed").is_file() {
            // Ended on purpose (its window closed, or "Shut down" when asked): nothing to keep.
            say(cat!(
                when(),
                " box '",
                name,
                "': closed by its user: clearing it"
            ));
            down(ctx, false)?;
        } else if !r.owner.is_empty() {
            // Its run still runs: that clears it, or this reaper does once the run is gone.
            return Ok(true);
        } else {
            return Ok(false);
        }
        return Ok(retry(ctx, r));
    }
    // Its covers never checked (#210): the up that would have is gone, and the box's PID 1 waits for
    // it for good. Nothing enters it meanwhile (covers_held); it goes.
    if ctx.at("covers-pending").exists() {
        if !starting_alive(ctx) {
            say(cat!(
                when(),
                " box '",
                name,
                "': its up ended before what its jail hides was checked: taking it down"
            ));
            down(ctx, false)?;
            return Ok(retry(ctx, r));
        }
        return Ok(true);
    }
    if !r.owner.is_empty() && !owner_alive(&r.owner, &r.owner_start) {
        say(cat!(
            when(),
            " box '",
            name,
            "': its omabox run (",
            r.owner,
            ") is gone: taking it down"
        ));
        down(ctx, false)?;
        return Ok(retry(ctx, r));
    }
    // The host's passthrough hooks, back after a config reload there (finding 118). A host that
    // does not answer (its session ending) must not end the reaper.
    if r.interactive {
        muted(|| keys_ensure(ctx, r.host.as_deref()));
    }
    if r.idle <= 0 && r.agent.is_empty() {
        return Ok(true);
    }
    if in_use(ctx) {
        touch(&ctx.at("used"));
        return Ok(true);
    }
    if !r.agent.is_empty() && !agent_alive(&ctx.d) {
        say(cat!(
            when(),
            " box '",
            name,
            "': its agent session (",
            meta(&ctx.d, "agent").0,
            ") ended: taking it down"
        ));
        down(ctx, true)?;
        // Still up, with an agent that runs: its session took it over meanwhile (take_over).
        if !(shell::path(&ctx.d).is_dir() && meta(&ctx.d, "created").0 == r.created) {
            return Ok(false);
        }
        if agent_alive(&ctx.d) {
            say(cat!(
                when(),
                " box '",
                name,
                "': taken over by its session's new agent (",
                meta(&ctx.d, "agent").0,
                "): kept"
            ));
            return Ok(true);
        }
        retry(ctx, r);
        return Ok(true);
    }
    if r.idle <= 0 {
        return Ok(true);
    }
    // Not while its up still runs (this reaper starts before it is done: #137): a slow start is not
    // idle time. An up killed meanwhile left the file: its pid gone, the idle limit counts.
    if starting_alive(ctx) {
        return Ok(true);
    }
    // (its used file gone: made again, so the idle time counts from now; it was never counted, #371)
    let Some(used) = mtime(&ctx.at("used")) else {
        touch(&ctx.at("used"));
        return Ok(true);
    };
    if now().saturating_sub(used) < r.idle {
        return Ok(true);
    }
    let human = crate::num::human_duration(r.idle);
    say(cat!(
        when(),
        " box '",
        name,
        "' idle for ",
        human,
        ": taking it down"
    ));
    down(ctx, false)?;
    if !shell::path(&ctx.d).is_dir() {
        let _ = std::fs::write(ctx.in_boxes(cat!(".expired-", name)), cat!(human, "\n"));
        prune_marks(ctx);
        return Ok(false);
    }
    Ok(retry(ctx, r))
}

/// In use: a peek window on it, or a command entered into it from outside still running (`omabox
/// run`, a tool of omabox's: bash looked for an nsenter into its PID 1 by its command line; the
/// native entry has none, finding 490).
fn in_use(ctx: &Ctx) -> bool {
    if crate::marks::peek_open(ctx) {
        return true;
    }
    match box_pid(&ctx.d) {
        BoxPid::Pid(p) => !crate::procfs::entered(&p).is_empty(),
        _ => false,
    }
}

/// A file's modification time, in seconds since the epoch (`stat -c %Y`).
fn mtime(p: &std::path::Path) -> Option<i64> {
    let t = std::fs::symlink_metadata(p).ok()?.modified().ok()?;
    let d = t.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(d.as_secs()).ok()
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// After a down: whether its box is still there, the same one (a new box that took the name, or a
/// down that left a box on purpose, is not to retry), and wants another try at the next poll. Said
/// once, not every poll.
fn retry(ctx: &Ctx, r: &mut Reap) -> bool {
    if !(shell::path(&ctx.d).is_dir() && meta(&ctx.d, "created").0 == r.created) {
        return false;
    }
    if !r.retry_said {
        say(cat!(
            when(),
            " box '",
            ctx.name,
            "': down did not finish, retrying"
        ));
    }
    r.retry_said = true;
    true
}

/// After a host config reload (finding 118) the hooks and binds are gone, and a submap entered before
/// stays on with nothing bound in it: put back when this version's are not there.
fn keys_ensure(ctx: &Ctx, sig: Option<&[u8]>) {
    let sig = match sig {
        Some(s) => s.to_vec(),
        None => match crate::host::host_session(ctx) {
            Ok(s) => s,
            Err(_) => return,
        },
    };
    if crate::host::host_eval_out(&sig, pass_check().as_bytes()) == b"error: omabox-missing" {
        let _ = crate::host::install_passthrough(ctx);
    }
}

/// A reaper starts as its box is launched (finding 237), before `up` has recorded the box's pid
/// namespace, which box_pid checks from then on. It waits for that record while its `up` (the
/// `starting` file) runs; an `up` gone first (SIGKILLed) leaves it to the reaper: the box's PID 1 once
/// it shows, within 15 s, else the box never started, and the loop finds it dead. False when the box
/// is gone or another took the name.
fn wait_pidns(ctx: &Ctx, created: &[u8]) -> bool {
    let end = crate::settle::after(15);
    while meta(&ctx.d, "pidns").0.is_empty() {
        if !(ctx.at("box.json").is_file() && meta(&ctx.d, "created").0 == created) {
            return false;
        }
        if !starting_alive(ctx) {
            if let BoxPid::Pid(pid) = box_pid(&ctx.d)
                && let Some(ns) = crate::procfs::pidns(&pid)
            {
                if !record_pidns(ctx, &ns) {
                    return false;
                }
                say(cat!(
                    when(),
                    " box '",
                    ctx.name,
                    "': its up ended before it recorded the box's pid namespace: recorded here"
                ));
                return true;
            }
            if crate::settle::past(end) {
                return true;
            }
        }
        crate::sig::sleep(Duration::from_millis(200));
    }
    true
}

/// `jq --arg ns NS '.pidns = $ns' box.json > box.json.tmp && mv box.json.tmp box.json`: whether it
/// was written (no box.json.tmp left when not: finding 443).
fn record_pidns(ctx: &Ctx, ns: &[u8]) -> bool {
    let tmp = shell::Rm(shell::bytes(&ctx.at("box.json.tmp")).to_vec());
    let Ok(text) = std::fs::read(ctx.at("box.json")) else {
        return false;
    };
    let (vals, mut ok) = crate::json::values(&text);
    let ns = serde_json::Value::String(String::from_utf8_lossy(ns).into_owned());
    let mut out = String::new();
    for mut v in vals {
        if !crate::json::set(&mut v, vec![("pidns", ns.clone())]) {
            ok = false;
            break;
        }
        out.push_str(&crate::json::pretty(&v));
        out.push('\n');
    }
    ok && std::fs::write(shell::path(&tmp.0), out).is_ok()
        && std::fs::rename(shell::path(&tmp.0), ctx.at("box.json")).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // #399: when a guard looks. ms(N): N ms after T0.
    #[test]
    fn guard_looks_when() {
        let t0 = std::time::Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut s = Sched::default();
        assert!(s.due().is_none(), "nothing to look at");
        // the first event: at once; then nothing until the trailing look GAP after it
        s.event(ms(0));
        assert!(s.due().is_some_and(|at| at <= std::time::Instant::now()));
        s.looked(ms(5), true);
        assert_eq!(s.due(), Some(ms(255)), "the trailing look");
        // a burst: events every 10 ms wait for GAP after the last look, then one look for all
        for i in 1..20 {
            s.event(ms(5 + 10 * i));
        }
        assert_eq!(
            s.due(),
            Some(ms(255)),
            "one look per GAP, whatever the events"
        );
        s.looked(ms(255), true);
        // (the trailing one: GAP after the last event, 445, but not before GAP after the last look)
        assert_eq!(s.due(), Some(ms(505)), "and the trailing one");
        s.looked(ms(505), true);
        assert!(s.due().is_none(), "then nothing");
        // failures: tried again with no event, GAP, then doubling to FAILING_MAX
        let mut f = Sched::default();
        let mut at = ms(1000);
        let mut waits = Vec::new();
        for _ in 0..7 {
            f.looked(at, false);
            let next = f.due().unwrap_or(at);
            waits.push((next - at).as_millis());
            at = next;
        }
        assert_eq!(waits, vec![250, 500, 1000, 2000, 2000, 2000, 2000]);
        f.looked(at, true);
        assert!(f.due().is_none(), "a look that works ends the retries");
    }

    #[test]
    fn polls() {
        assert_eq!(poll_every(7200, false, false), 60);
        assert_eq!(poll_every(10, false, false), 5);
        assert_eq!(poll_every(120, false, false), 30);
        assert_eq!(poll_every(0, true, false), 60);
        assert_eq!(poll_every(40, true, false), 10);
        assert_eq!(poll_every(7200, false, true), 2);
        assert_eq!(poll_every(0, false, false), 5);
    }

    // #368: the reaper's check is share/passthrough.lua's VERSION, whatever it is bumped to.
    #[test]
    fn pass_check_is_the_lua_version() {
        let lua = include_str!("../../../../share/passthrough.lua");
        assert!(
            lua.contains("\nomabox_pass_version = VERSION\n"),
            "the global the check reads is the one passthrough.lua sets"
        );
        assert!(
            pass_check().starts_with(&format!(
                "if (omabox_pass_version or 0) < {PASS_VERSION} then"
            )),
            "{}",
            pass_check()
        );
        assert_eq!(lua_version(b"x\n"), 0);
    }
}
