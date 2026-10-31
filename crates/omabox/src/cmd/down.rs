//! `omabox down`: a box taken down, under its name's lock: its peek windows closed, its PID 1
//! SIGKILLed as box_pid finds it (the kernel then ends everything in its pid namespace; SIGTERM does
//! not work, finding 6) and its pasta with it (ready::kill_box), its reaper ended, its dir and HOME
//! removed. `--all`, `--orphans` (#220). The reaper takes a box down through `down_one`, telling it
//! which box it decided on (DownIf).

use crate::args::{optarg, unknown_opt};
use crate::boxes::{BoxSys, RealBox, box_alive, list_names, lock_file, meta};
use crate::cat;
use crate::exit::{Ctx, Res, muted, say};
use crate::jail::{in_jail, jail_id};
use crate::jq::{self, Args};
use crate::pidfd::Pidfd;
use crate::select::{select_box, session_boxes};
use crate::shell;
use std::time::Duration;

/// down_orphans' word on what started an orphan.
const STARTER_JQ: &str = r#".starter | "\(.command), pid \(.pid)""#;

/// A reaper's down (bash's DOWN_IF_CREATED, DOWN_IF_AGENT_GONE): only the box it decided on, created
/// then, and, when it decided on an agent that exited, only while no agent of its session took it
/// over (take_over). The reaper's down says nothing while it waits, and leaves no `.down-` marker.
#[derive(Default, Clone)]
pub struct DownIf {
    pub created: Option<Vec<u8>>,
    pub agent_gone: bool,
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    run_with(&RealBox, ctx, args)
}

fn run_with<S: BoxSys + ?Sized>(sys: &S, ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut names, mut all, mut orphans) = (Vec::<Vec<u8>>::new(), false, false);
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_slice() {
            b"--all" => all = true,
            b"--orphans" => orphans = true,
            b"-b" | b"--box" => {
                optarg(ctx, a, args.get(i + 1))?;
                names.push(args[i + 1].clone());
                i += 1;
            }
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "down", a)),
            _ => names.push(a.clone()),
        }
        i += 1;
    }
    if orphans {
        if all || !names.is_empty() {
            return Err(ctx.die("down: --orphans takes no names, nor --all"));
        }
        ctx.parsed();
        return down_orphans_with(sys, ctx);
    }
    if all && !names.is_empty() {
        return Err(ctx.die("down: --all or names, not both"));
    }
    ctx.parsed();
    if all {
        names = list_names(ctx, false);
    } else if names.is_empty() {
        // No name: select_box derives it, and outside any repo falls back to the session's box
        // (finding 200); passed the derived name, it would take it as given and not (finding 259).
        names.push(Vec::new());
    }
    let mut ok = true;
    for n in &names {
        ok &= down_one_with(sys, ctx, n, &DownIf::default())?;
    }
    if ok {
        Ok(())
    } else {
        Err(crate::exit::Exit(1))
    }
}

/// down of one name (`""`: the default box): whether it went (or there was none to take down);
/// Err for a refusal that ends the command (a bad name, another jail's box).
pub fn down_one(ctx: &mut Ctx, given: &[u8], cond: &DownIf) -> Res<bool> {
    down_one_with(&RealBox, ctx, given, cond)
}

fn down_one_with<S: BoxSys + ?Sized>(
    sys: &S,
    ctx: &mut Ctx,
    given: &[u8],
    cond: &DownIf,
) -> Res<bool> {
    select_box(ctx, given)?;
    let _ = std::fs::remove_file(ctx.in_boxes(cat!(".expired-", ctx.name)));
    let lk = ctx.in_boxes(cat!(".lock-", ctx.name));
    let there = |ctx: &Ctx| shell::path(&ctx.d).is_dir() || lk.exists();
    // No dir and no lock: no box. A lock without a dir can be an `up` in its first ~40 ms (it makes
    // the dir after taking the lock, finding 147): waited for below, and the box it made taken down.
    // The time it found none, for an `up` of the name that started before but has no lock yet:
    // written before the lock is looked at again (finding 237).
    if !there(ctx) {
        down_mark(sys, ctx, cond);
        if !there(ctx) {
            return Ok(down_none(ctx));
        }
    }
    let _ = std::fs::create_dir_all(shell::path(&ctx.boxes));
    // The reaper takes a box down through here while holding nothing; anyone else waits for an up.
    let lock = match lock_file(&lk, Duration::ZERO) {
        Some(l) => Some(l),
        None => {
            if cond.created.is_none() {
                say(cat!("box '", ctx.name, "': waiting for its up to finish"));
            }
            lock_file(&lk, Duration::from_secs(60))
        }
    };
    let Some(lock) = lock else {
        say(cat!(
            "box '",
            ctx.name,
            "' is busy (an up in progress?); not taken down"
        ));
        return Ok(false);
    };
    // A box gone takes its lock file with it, removed while held (#158). Its marker before the lock
    // goes (finding 237): an `up` waiting on it then reads it.
    if !shell::path(&ctx.d).is_dir() {
        down_mark(sys, ctx, cond);
        let _ = std::fs::remove_file(&lk);
        drop(lock);
        return Ok(down_none(ctx));
    }
    // A jailed caller's box is checked again under the lock: select_box passed a name whose up (the
    // user's, or another jail's) had not written its box.json yet, and that up has finished since.
    if in_jail() && meta(&ctx.d, "jail").0 != jail_id() {
        drop(lock);
        return Err(ctx.die(cat!(
            "box '",
            ctx.name,
            "' is not this jail's: an agent inside ai-jail drives only the boxes it started"
        )));
    }
    // The reaper decided on the box it watched, then waited here: an `up` holding the lock may have
    // started a new box under the name (finding 74). Not that one.
    if let Some(c) = &cond.created
        && meta(&ctx.d, "created").0 != *c
    {
        return Ok(true);
    }
    // Nor, when it decided on an agent that exited, a box its session took over meanwhile.
    if cond.agent_gone && crate::boxes::agent_alive(&ctx.d) {
        return Ok(true);
    }
    // A box whose PID 1 this user namespace cannot read (a sandbox's call) is left as it is, said as
    // up says it: bash's kill_box took it for no box, removed its dir and said it down while its
    // PID 1 ran on (finding 473).
    if matches!(sys.box_pid(&ctx.d), crate::boxes::BoxPid::Unknown) {
        say(cat!(
            "cannot tell whether box '",
            ctx.name,
            "' is up from this user namespace (a sandbox's?): run omabox outside it"
        ));
        return Ok(false);
    }
    // A box another caller started or used just now may be another agent's (#295): said, not
    // refused. Not the reaper's.
    let me = crate::whose::me();
    if cond.created.is_none()
        && let Some(w) = crate::whose::down_warning(ctx, &me)
    {
        say(cat!("warning: ", w));
    }
    // Its peek windows, one per monitor shown (#164), each pinned by a pidfd, then found a peek of
    // the box again (#377): a pid that ended after the first look and was reused gets nothing.
    let mut peeks: Vec<Pidfd> = sys
        .peek_pids(ctx)
        .iter()
        .filter_map(|p| Pidfd::pin(p, || true))
        .collect();
    let again = sys.peek_pids(ctx);
    peeks.retain(|p| again.iter().any(|a| a.as_slice() == p.pid()) && p.alive());
    for p in peeks {
        p.signal(libc::SIGTERM);
    }
    let was_interactive = meta(&ctx.d, "mode").0 == b"interactive";
    if !sys.kill_box(ctx) {
        return Ok(false);
    }
    crate::ready::kill_reapers(ctx);
    sys.rm_box(ctx);
    // (under the lock: an up waiting on it removes the note as it starts the name's next box)
    if cond.created.is_none() {
        crate::whose::write_taken(ctx, &me);
    }
    let _ = std::fs::remove_file(&lk);
    drop(lock);
    // (a host that does not answer must not fail the down)
    if was_interactive {
        muted(|| submap_release(ctx));
    }
    say(cat!("box '", ctx.name, "' down"));
    Ok(true)
}

/// A down that finds no box leaves when it was started (proc_start: clock ticks since boot), for an
/// up started before it that has no lock yet: they compare starts, not when each got to its check
/// (finding 147). A jailed caller's names its jail on a second line: an up cancels for it only when
/// it is that jail's (a jail's down must not cancel the user's up of a name). Written before the
/// down lets go of the name's lock (finding 237). Not the reaper's.
///
/// After the start, on its line, what an up started in the same tick tells the order by (finding
/// 513): the pid its pid namespace made last as the marker is written, that namespace, and this
/// call's time (CALL_US); then, for an up of another pid namespace, CLOCK_BOOTTIME in µs as the
/// marker is written and the time namespace it was read in (#404); each `-` when it cannot be told.
/// Bash's up reads a marker of one number only, so it takes these for none (a bash up is a
/// fallback's, with a bash down).
fn down_mark<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx, cond: &DownIf) {
    if cond.created.is_none() {
        let mark = ctx.in_boxes(cat!(".down-", ctx.name));
        // (a jail's never takes the place of the user's own, which cancels any up)
        let users = in_jail() && std::fs::read(&mark).is_ok_and(|m| user_marker(&m));
        if let (Some(start), false) = (sys.proc_start(b"self"), users) {
            let dash = || b"-".to_vec();
            let last = crate::procfs::last_pid().map(|n| n.to_string().into_bytes());
            let boot = crate::procfs::boot_us().map(|n| n.to_string().into_bytes());
            let ns = crate::procfs::pidns(b"self").filter(|n| !n.is_empty() && !n.contains(&b' '));
            let mut v = cat!(
                start,
                " ",
                last.unwrap_or_else(dash),
                " ",
                ns.unwrap_or_else(dash),
                " ",
                crate::call_us(),
                " ",
                boot.unwrap_or_else(dash),
                " ",
                crate::procfs::timens().unwrap_or_else(dash),
                "\n"
            );
            if in_jail() {
                v.extend(cat!(jail_id(), "\n"));
            }
            let _ = std::fs::create_dir_all(shell::path(&ctx.boxes))
                .and_then(|()| std::fs::write(&mark, v));
        }
    }
    prune_marks(ctx);
}

/// A `.down-` marker of the user's own: a time and no jail line.
fn user_marker(m: &[u8]) -> bool {
    let t = shell::strip_nl(m);
    !t.is_empty() && !t.contains(&b'\n')
}

/// The markers nobody reads any more: a down's after 10 minutes, and an idle box's note
/// (.expired-NAME) and a down's (.taken-NAME, #295) after a day (finding 237).
pub fn prune_marks(ctx: &Ctx) {
    let Ok(rd) = std::fs::read_dir(shell::path(&ctx.boxes)) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for e in rd.flatten() {
        let n = e.file_name().into_encoded_bytes();
        let max = if n.starts_with(b".down-") {
            600
        } else if n.starts_with(b".expired-") || n.starts_with(b".taken-") {
            86_400
        } else {
            continue;
        };
        let Ok(m) = e.metadata() else { continue };
        let old = m
            .modified()
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|d| d.as_secs() >= max);
        if m.is_file() && old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// `down` found no box under the name: said. A name derived here (no -b) while this agent session
/// has boxes up under others, started from other directories (#262), names each and the down that
/// takes it, and fails: the agent's box is still up.
fn down_none(ctx: &Ctx) -> bool {
    let mine = if ctx.name_derived {
        session_boxes(ctx)
    } else {
        Vec::new()
    };
    if mine.is_empty() {
        say(cat!("no box '", ctx.name, "'"));
        return true;
    }
    let home = shell::env_or_empty("HOME");
    let mut has = Vec::new();
    for n in &mine {
        let mut dir = meta(&cat!(ctx.boxes, "/", n), "dir").0;
        if dir == home || dir.starts_with(&cat!(home, "/")) {
            dir = cat!("~", &dir[home.len()..]);
        }
        if !has.is_empty() {
            has.extend_from_slice(b"; ");
        }
        has.extend(cat!("'", n, "'"));
        if !dir.is_empty() {
            has.extend(cat!(" (started in ", dir, ")"));
        }
        has.extend(cat!(": omabox down -b ", n));
    }
    say(cat!("no box '", ctx.name, "'; this session has ", has));
    false
}

/// down --orphans (#220): every box `ls` calls an orphan, each named with what started it, then
/// taken down as that box (not a new one that took its name meanwhile).
fn down_orphans_with<S: BoxSys + ?Sized>(sys: &S, ctx: &mut Ctx) -> Res<()> {
    let mut found: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    for n in list_names(ctx, false) {
        select_box(ctx, &n)?;
        if sys.box_state(&ctx.d) != "up" || !crate::ready::box_orphan(ctx) {
            continue;
        }
        found.push((ctx.name.clone(), meta(&ctx.d, "created").0));
    }
    if found.is_empty() {
        say("no orphan boxes");
        return Ok(());
    }
    let mut ok = true;
    for (n, created) in found {
        select_box(ctx, &n)?;
        let r = Args::new(&["-r", STARTER_JQ])
            .arg(cat!(ctx.d, "/box.json"))
            .run_file(jq::Err::Null);
        let by = if r.ok() { r.text() } else { b"?".to_vec() };
        say(cat!(
            "box '",
            ctx.name,
            "': an orphan (no idle limit; what started it, ",
            by,
            ", is gone): taking it down"
        ));
        let cond = DownIf {
            created: Some(created),
            agent_gone: false,
        };
        ok &= down_one_with(sys, ctx, &n, &cond)?;
    }
    if ok {
        Ok(())
    } else {
        Err(crate::exit::Exit(1))
    }
}

/// After an interactive box went (finding 134): the host out of the "omabox" submap once no
/// interactive box is left (a host config reload keeps the submap and drops its binds, finding 118).
fn submap_release(ctx: &Ctx) {
    for n in list_names(ctx, false) {
        let mut c = ctx.clone();
        c.d = cat!(ctx.boxes, "/", n);
        c.name = n;
        if meta(&c.d, "mode").0 == b"interactive" && box_alive(&c).unwrap_or(false) {
            return;
        }
    }
    crate::host::HostSilent::new(ctx).submap_reset();
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    /// bash's stubs for a down (`kill_box() { :; }; peek_pid() { return 1; }; rm_box() {...}`): no
    /// peek window, PID 1 gone at once; rm_box removes the box dir (RM_DIR) or only says it ran;
    /// START, proc_start's answer when given; a box up while its dir is there. PEEKS: peek_pids'
    /// answers, one a call (none once they run out).
    struct Fake {
        rm_dir: bool,
        start: Option<Box<dyn Fn() -> Option<Vec<u8>> + Sync>>,
        peeks: Mutex<Vec<Vec<Vec<u8>>>>,
        log: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(rm_dir: bool) -> Fake {
            Fake {
                rm_dir,
                start: None,
                peeks: Mutex::new(Vec::new()),
                log: Mutex::new(Vec::new()),
            }
        }
        fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    impl BoxSys for Fake {
        fn peek_pids(&self, _: &Ctx) -> Vec<Vec<u8>> {
            let mut p = self.peeks.lock().unwrap();
            if p.is_empty() {
                Vec::new()
            } else {
                p.remove(0)
            }
        }
        fn kill_box(&self, _: &Ctx) -> bool {
            self.log.lock().unwrap().push("kill_box".into());
            true
        }
        fn rm_box(&self, ctx: &Ctx) {
            self.log.lock().unwrap().push("rm_box".into());
            if self.rm_dir {
                let _ = std::fs::remove_dir_all(shell::path(&ctx.d));
            }
        }
        fn proc_start(&self, pid: &[u8]) -> Option<Vec<u8>> {
            match &self.start {
                Some(f) => f(),
                None => crate::procfs::proc_start(pid),
            }
        }
        fn box_state(&self, d: &[u8]) -> &'static str {
            if shell::path(d).is_dir() {
                "up"
            } else {
                "gone"
            }
        }
    }

    fn tmp(tag: &str) -> String {
        let d = std::env::temp_dir().join(format!("omabox-down-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.to_str().unwrap().to_string()
    }

    fn put(f: &str, t: &str) {
        std::fs::create_dir_all(std::path::Path::new(f).parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    }

    fn there(f: &str) -> bool {
        std::path::Path::new(f).exists()
    }

    /// `cmd_down ARGS` with BOXES the dir B: its status and what it said. No jail, no session.
    fn down(sys: &Fake, b: &str, args: &[&str]) -> (Res<()>, String) {
        let mut ctx = Ctx::new(b"down", b.as_bytes().to_vec());
        let args: Vec<Vec<u8>> = args.iter().map(|a| a.as_bytes().to_vec()).collect();
        let vars = [
            ("OMABOX_JAIL", None),
            ("OMABOX", None),
            ("OMABOX_SESSION", None),
            ("CLAUDE_CODE_SESSION_ID", None),
            ("CODEX_THREAD_ID", None),
        ];
        crate::exit::heard(|| shell::with_env(&vars, || run_with(sys, &mut ctx, &args)))
    }

    /// t_unit_run_named_dead (#115, finding 178): a down whose reaper is gone since it was looked
    /// for is no failure, and the box is cleared (kill_reapers ends what it finds and returns
    /// nothing, so a reaper gone meanwhile cannot fail it).
    /// t_unit_down_peek (#377): down's SIGTERM reaches a peek window still one once it is pinned,
    /// never a process that took a peek's pid after the scan (here: one the second scan does not
    /// find a peek).
    /// (#[ignore]: it starts and kills processes; test/run.sh's unit tier runs it.)
    #[test]
    #[ignore = "starts processes: run by test/run.sh (cargo test -- --ignored)"]
    fn down_ends_only_a_peek_still_one() {
        let b = tmp("peek");
        let mut s = std::process::Command::new("sleep")
            .arg("300")
            .spawn()
            .unwrap();
        let sp = s.id().to_string().into_bytes();
        let gone = |c: &mut std::process::Child| {
            let end = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while std::time::Instant::now() < end {
                if matches!(c.try_wait(), Ok(Some(_))) {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            false
        };
        put(&format!("{b}/p1/box.json"), "{\"mode\":\"headless\"}\n");
        let f = Fake::new(false);
        *f.peeks.lock().unwrap() = vec![vec![sp.clone()], Vec::new()];
        let _ = down(&f, &b, &["p1"]);
        let left = !gone(&mut s);
        put(&format!("{b}/p2/box.json"), "{\"mode\":\"headless\"}\n");
        let f = Fake::new(false);
        *f.peeks.lock().unwrap() = vec![vec![sp.clone()], vec![sp]];
        let _ = down(&f, &b, &["p2"]);
        let ended = gone(&mut s);
        let _ = s.kill();
        let _ = s.wait();
        let _ = std::fs::remove_dir_all(&b);
        assert!(left, "a pid that is no peek once pinned: left");
        assert!(ended, "...a peek still one: ended");
    }

    #[test]
    fn a_reaper_gone_is_no_failure() {
        let b = tmp("race");
        put(
            &format!("{b}/racebox/box.json"),
            "{\"mode\":\"headless\"}\n",
        );
        let f = Fake::new(false);
        let (r, said) = down(&f, &b, &["racebox"]);
        assert_eq!(
            (r, said.as_str(), f.log()),
            (
                Ok(()),
                "omabox: box 'racebox' down\n",
                vec!["kill_box".to_string(), "rm_box".into()]
            ),
            "down: a reaper gone since it was looked for is no failure, the box is cleared"
        );
        std::fs::remove_dir_all(&b).unwrap();
    }

    /// t_unit_lock_markers: a down removes the box's lock file, and one with no box; one that finds
    /// no box sweeps old .down- markers and .expired- notes (finding 237) and writes its own.
    #[test]
    fn lock_files_and_markers_as_bash() {
        let b = tmp("lockm");
        put(&format!("{b}/gone/box.json"), "{\"mode\":\"headless\"}\n");
        put(&format!("{b}/.lock-gone"), "");
        put(&format!("{b}/.lock-nodir"), "");
        let (_, said) = down(&Fake::new(true), &b, &["gone", "nodir"]);
        assert!(
            said.contains("box 'gone' down"),
            "down takes the box down: {said}"
        );
        assert!(
            !there(&format!("{b}/.lock-gone")),
            "...and removes its lock file"
        );
        assert!(
            !there(&format!("{b}/.lock-nodir")),
            "a lock file with no box: down removes it too"
        );
        let aged = |n: &str, secs: u64| {
            let f = format!("{b}/{n}");
            put(&f, "");
            let t = std::time::SystemTime::now() - std::time::Duration::from_secs(secs);
            std::fs::File::options()
                .write(true)
                .open(&f)
                .unwrap()
                .set_modified(t)
                .unwrap();
        };
        aged(".down-old", 11 * 60);
        aged(".down-new", 0);
        aged(".expired-old", 25 * 3600);
        aged(".expired-new", 23 * 3600);
        let _ = down(&Fake::new(true), &b, &["none"]);
        assert!(
            !there(&format!("{b}/.down-old")),
            "a down that finds no box sweeps a .down- marker older than 10 minutes"
        );
        assert!(there(&format!("{b}/.down-new")), "...keeps a fresh one");
        assert!(
            std::fs::metadata(format!("{b}/.down-none")).is_ok_and(|m| m.len() > 0),
            "...and writes its own"
        );
        assert!(
            !there(&format!("{b}/.expired-old")),
            "...and an idle box's .expired- note older than a day (finding 237)"
        );
        assert!(
            there(&format!("{b}/.expired-new")),
            "...keeping a younger one"
        );
        std::fs::remove_dir_all(&b).unwrap();
    }

    /// t_unit_lock_markers, finding 237: a down that finds the lock but no box writes its marker
    /// before it lets go of the lock (here its start takes half a second to read), so an up waiting
    /// on the lock reads it; one that finds neither looks for the lock again once its marker is
    /// written, and goes through a lock that appeared meanwhile.
    #[test]
    fn a_waiter_on_the_lock_reads_the_marker() {
        let b = tmp("lockr");
        let lk = format!("{b}/.lock-raced");
        put(&lk, "");
        let mut f = Fake::new(true);
        f.start = Some(Box::new(|| {
            std::thread::sleep(std::time::Duration::from_millis(500));
            Some(b"12345".to_vec())
        }));
        let got = std::thread::scope(|s| {
            s.spawn(|| down(&f, &b, &["raced"]));
            let p = std::path::Path::new(&lk);
            let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while crate::boxes::lock_file(p, std::time::Duration::ZERO).is_some()
                && std::time::Instant::now() < end
            {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let _w = crate::boxes::lock_file(p, std::time::Duration::from_secs(10));
            std::fs::read_to_string(format!("{b}/.down-raced"))
                .unwrap_or_else(|_| "no marker".into())
        });
        assert_eq!(
            got.split(' ').next(),
            Some("12345"),
            "an up waiting on the lock of a down that finds no box reads its marker once it has the lock: {got}"
        );
        let late = format!("{b}/.lock-late");
        let mut f = Fake::new(true);
        let l2 = late.clone();
        f.start = Some(Box::new(move || {
            let _ = std::fs::write(&l2, "");
            Some(b"12345".to_vec())
        }));
        let _ = down(&f, &b, &["late"]);
        assert!(
            !there(&late),
            "a lock that appears as a down writes its marker: the down goes through it"
        );
        std::fs::remove_dir_all(&b).unwrap();
    }

    /// t_unit_orphan's down --orphans (#220): the orphans only, each named with what started it,
    /// and taken down; none, said so; no names with it. A box is up while its dir is there.
    #[test]
    fn orphans_as_bash() {
        let b = tmp("orph");
        let me = std::process::id().to_string();
        let my_start =
            String::from_utf8(crate::procfs::proc_start(me.as_bytes()).unwrap()).unwrap();
        // (a pid no process has: pid_max is one past the last)
        let gone = std::fs::read_to_string("/proc/sys/kernel/pid_max")
            .unwrap()
            .trim()
            .to_string();
        let dead = format!(
            "{{\"pid\": {gone}, \"start\": \"5\", \"command\": \"sleep\", \"terminal\": false}}"
        );
        let live = format!(
            "{{\"pid\": {me}, \"start\": \"{my_start}\", \"command\": \"bash\", \"terminal\": false}}"
        );
        put(
            &format!("{b}/lost/box.json"),
            &format!(
                "{{\"mode\": \"headless\", \"idle\": 0, \"created\": \"1\", \"starter\": {dead}}}"
            ),
        );
        put(
            &format!("{b}/kept/box.json"),
            &format!(
                "{{\"mode\": \"headless\", \"idle\": 0, \"created\": \"2\", \"starter\": {live}}}"
            ),
        );
        let f = Fake::new(true);
        let (_, said) = down(&f, &b, &["--orphans"]);
        assert!(
            said.contains(&format!(
                "box 'lost': an orphan (no idle limit; what started it, sleep, pid {gone}, is gone)"
            )),
            "down --orphans names each orphan and what started it: {said}"
        );
        assert!(!there(&format!("{b}/lost")), "...and takes it down");
        assert!(
            there(&format!("{b}/kept/box.json")),
            "...not a box whose starter runs"
        );
        assert_eq!(
            down(&f, &b, &["--orphans"]).1,
            "omabox: no orphan boxes\n",
            "...none left: said so"
        );
        assert!(
            down(&f, &b, &["--orphans", "kept"])
                .1
                .contains("takes no names"),
            "...takes no names"
        );
        std::fs::remove_dir_all(&b).unwrap();
    }

    #[test]
    fn markers_of_the_user() {
        assert!(user_marker(b"123\n"));
        assert!(!user_marker(b"123\nj1\n"));
        assert!(!user_marker(b""));
    }
}
