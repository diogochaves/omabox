//! up, from the box dir on (#324 wave 5a): the dir and HOME made and seeded, box.json written, the
//! box's /etc files, the bwrap words (bwrap.rs) and pasta's (net.rs), launch.sh written and run, and
//! the box waited for until it is ready.
//!
//! From the dir's making on, the box goes if up does not finish (finding 434, #356): a code trap
//! whose undo ends the box (and clears a half-made dir), run on a failure and on an INT, TERM or HUP
//! at any step, after which up exits 130, 143 or 129 (bash's `trap 'exit 143' TERM`), never 1 for a
//! signal whatever the step it lands on (the interactive --monitor add's included: its own undo, the
//! output's removal, runs first).

use super::up::{Up, pid_self};
use crate::boxes::{self, BoxPid, box_pid, meta};
use crate::bwrap::{Bwrap, DevNode};
use crate::cat;
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::install;
use crate::jail::in_jail;
use crate::json;
use crate::net::{self, Net};
use crate::paths;
use crate::procfs;
use crate::ready::{self, Owner, Ready};
use crate::render::{self, CardPlaceholder, NvidiaDev, RenderNode};
use crate::shell::{self, quote_q};
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Where a box sees omabox's own tools (bash's BOX_* globals).
const BOX_KEYBOARD: &str = "/opt/omabox/bin/omabox-keyboard";
const BOX_POINTER: &str = "/opt/omabox/bin/omabox-pointer";
const BOX_STILL: &str = "/opt/omabox/bin/omabox-still";
const BOX_EVENTS: &str = "/opt/omabox/bin/omabox-events";
const BOX_BURST: &str = "/opt/omabox/bin/omabox-burst";
const BOX_TREE: &str = "/opt/omabox/bin/omabox-tree";

/// The PATH a box's start (launch.sh, and pasta's shell in it) runs with: up looks there for what
/// that start runs, as nft (#364), not on the caller's.
pub const LAUNCH_PATH: &[u8] = b"/usr/bin";
/// What pasta's shell says before it ends the start when it could not set the box's network up: the
/// --addr addresses (#229) or the protected ports' rule (#355). up says it at once (#364).
const ADDR_FAILED: &str = "omabox: could not add the --addr addresses to the box loopback";
const NFT_FAILED: &str = "omabox: could not set the protected ports rule in the box network";

fn os(v: &[u8]) -> &std::ffi::OsStr {
    std::os::unix::ffi::OsStrExt::from_bytes(v)
}

fn write(p: &[u8], data: &[u8]) -> bool {
    std::fs::write(shell::path(p), data).is_ok()
}

pub fn make(ctx: &mut Ctx, mut u: Up) -> Res<()> {
    let d = ctx.d.clone();
    let name = ctx.name.clone();
    let homes = paths::homes();
    let home_dir = cat!(homes, "/", name, "/home");
    // A HOME under this name that is another box's (one up in another runtime dir, which this up
    // cannot see): never removed, nor shared (finding 584).
    if let Some(why) = ready::home_taken(ctx) {
        return Err(ctx.die(cat!(
            "up: box '",
            name,
            "''s HOME (",
            homes,
            "/",
            name,
            ") ",
            why,
            ": down that box first, or give this one another name"
        )));
    }
    // From here a box dir exists, and from launch.sh on a box may be running: if up fails, it is
    // killed rather than left running unseen. Once box.json is written the dir stays, so the logs
    // up's refusal points at can be read; before that it goes (finding 163). Whatever ends up:
    // a failure, or an INT, TERM or HUP at any step (finding 434, #356). A signal is the caller
    // stopping up, not a failure with logs to read: the dir and its reaper go too, as down's do (#388).
    let undo_ctx = ctx.clone();
    let launched: Arc<Mutex<Launched>> = Arc::default();
    let undo_launched = Arc::clone(&launched);
    let trap = RefCell::new(Some(crate::trap::on_exit_code(move || {
        // (a box handed off but not yet showing its pid is waited for, then ended: H1 of the
        // review, finding 460)
        let mut l = undo_launched
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if l.child.is_some() || l.asked {
            launched_wait(&undo_ctx, &mut l.child);
        }
        let gone = crate::exit::muted(|| ready::kill_box(&undo_ctx));
        if let Some(c) = l.child.as_mut()
            && matches!(c.try_wait(), Ok(None))
        {
            let _ = c.kill();
        }
        drop(l);
        let _ = std::fs::remove_file(undo_ctx.at("starting"));
        // (a PID 1 still exiting keeps its dir: a box that may yet run is never hidden)
        if crate::trap::signalled() && gone {
            ready::kill_reapers(&undo_ctx);
            ready::rm_box(&undo_ctx);
        } else if !undo_ctx.at("box.json").is_file() {
            ready::rm_box(&undo_ctx);
        }
    })));
    let drop_trap = || {
        if let Some(t) = trap.borrow_mut().take() {
            t.forget();
        }
    };
    ready::rm_box(ctx);
    let _ = std::fs::remove_file(ctx.in_boxes(cat!(".expired-", name)));
    // (the box dir, then the HOME's record of it, then the HOME: sweep_homes never takes a HOME
    // whose box is being made, from any runtime dir)
    let made = std::fs::create_dir_all(ctx.at("run")).is_ok()
        && std::fs::create_dir_all(shell::path(&cat!(homes, "/", name))).is_ok()
        && crate::homeuse::write_record(&cat!(homes, "/", name), &d)
        && std::fs::create_dir_all(shell::path(&home_dir)).is_ok()
        && std::os::unix::fs::symlink(shell::path(&home_dir), ctx.at("home")).is_ok();
    if !made {
        return Err(ctx.die(cat!("up: could not make box '", name, "''s dir in ", d)));
    }
    // (no run -d job has ended yet: job_note)
    write(&cat!(d, "/jobs-noted"), b"");
    // The box's XDG_RUNTIME_DIR, 0700 as a session's (#153).
    let _ = std::fs::set_permissions(ctx.at("run"), std::fs::Permissions::from_mode(0o700));
    // What a jailed caller's covers hide its hidden paths with, and the pipe the box's PID 1 waits on
    // until jail_verify has checked them (#210). In the box dir: no jail sees it.
    if !u.cover_srcs.is_empty() && !make_covers(&d) {
        return Err(ctx.die(cat!("up: could not make the box's covers in ", d)));
    }
    if !u.from_dir.is_empty() {
        let mut c = crate::sig::command("cp");
        c.args(["-a", "--reflink=auto"])
            .arg(os(&cat!(u.from_dir, "/home/.")))
            .arg(os(&cat!(home_dir, "/")))
            .stdin(std::process::Stdio::inherit());
        if crate::sig::run_status(c).unwrap_or(127) != 0 {
            return Err(ctx.die(cat!(
                "could not copy save '",
                u.from,
                "' into box '",
                name,
                "'"
            )));
        }
    }
    // The name is taken now (its dir exists): the next `up --new` may choose.
    u.newlock = None;
    ready::sweep_homes(ctx);
    let h = cat!(d, "/home");
    let plan = seed_plan(&u);
    let desk_files = crate::seed::seed_home(ctx, &plan, &h)?;
    for (src, rel) in &u.seeds {
        let dest = crate::seed::Dest::new(rel)
            .ok_or_else(|| ctx.die(cat!("--seed: DEST must be inside the box HOME, got ", rel)))?;
        crate::seed::seed_copy_in(ctx, &h, &crate::seed::Src::asked(src.clone()), &dest)?;
    }
    // A clean box's shell.json is also the defaults its shell reads (finding 400).
    let defaults = cat!(d, "/shell-defaults.json");
    let _ = std::fs::remove_file(shell::path(&defaults));
    if u.clean
        && !u.clean_has("clock")
        && shell::path(&cat!(u.tree(), "/config/omarchy/shell.json")).is_file()
        && std::fs::copy(
            shell::path(&cat!(d, "/home/.config/omarchy/shell.json")),
            shell::path(&defaults),
        )
        .is_err()
    {
        say(
            "up --clean: could not copy the box's shell.json as its shell's defaults: its clock may show the real time",
        );
    }
    // (after the seeds: one into its folder keeps the box's own)
    let desk_fonts = crate::seed::seed_fonts(ctx, &plan, &h)?;
    let shell_before = crate::shellnote::shell_crash_dirs(ctx);
    // Read again as late as possible: a change made while this box was being set up would otherwise
    // be lost for it; once box.json exists, confirm_live sees the box.
    if u.confirm_from != "flag" {
        u.confirm = super::up::confirm_conf();
    }
    if u.interactive && !u.aq.fixed && u.confirm == b"on" {
        u.confirm = b"off".to_vec();
        say(cat!(
            "confirm-close is on in your settings, but it needs ",
            u.aq.lacks(),
            ": off for this box (closing its window ends it). ",
            install::AQ_HOWTO
        ));
    }
    let render = u
        .render
        .clone()
        .ok_or_else(|| ctx.die("up: no render node"))?;
    let mut renders: Vec<RenderNode> = Vec::new();
    let mut nvidia_devs: Vec<NvidiaDev> = Vec::new();
    let mut nvidia_cards: Vec<Vec<u8>> = Vec::new();
    let mut wayland_screen = false;
    if u.interactive {
        renders = render::render_nodes();
        // An NVIDIA node whose userspace nodes are missing is left out: the desktop is not rendering
        // on it.
        for n in &renders {
            if n.driver() != b"nvidia" {
                continue;
            }
            if let Some(dev) = crate::exit::muted(|| render::nvidia_device(ctx, n, false).ok()) {
                nvidia_devs.push(dev);
                nvidia_cards.push(render::nvidia_card(n).unwrap_or_default());
            }
        }
    } else if render.driver() == b"nvidia" {
        renders.push(render.clone());
        let dev = render::nvidia_device(ctx, &render, true)
            .map_err(|_| ctx.die(cat!("cannot use NVIDIA render node ", render.as_bytes())))?;
        nvidia_devs.push(dev);
        nvidia_cards.push(render::nvidia_card(&render).unwrap_or_default());
        wayland_screen = true;
    } else {
        renders.push(render.clone());
    }
    // The owner's start time with its pid, as for an agent: a pid reused after a SIGKILLed run is
    // not it.
    let owner = (!u.owner.is_empty()).then(|| Owner {
        pid: u.owner.clone(),
        start: procfs::proc_start(&u.owner).unwrap_or_default(),
    });
    // Its stderr is a pipe to the run: a write to it once the run is gone must not end this up on the
    // spot, leaving the box to the reaper (#222): ignored, and the owner checked at the next wait.
    if owner.is_some() {
        // SAFETY: SIGPIPE's disposition set to ignored; no handler of ours is replaced.
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        }
    }
    // Who started it (#220), for `ls` to tell an orphan. One started at a terminal outside any agent
    // session is a person's, on purpose: never an orphan.
    let starter = crate::upspec::box_starter().unwrap_or_default();
    let at_term = crate::exec::isatty(0) && crate::select::agent_var().is_none() && !in_jail();
    let bj = box_json(
        ctx,
        &u,
        &render,
        wayland_screen,
        &owner,
        &starter,
        at_term,
        &desk_files,
        &desk_fonts,
    );
    if !write(&cat!(d, "/box.json"), &bj) {
        return Err(ctx.die(cat!("up: could not write ", d, "/box.json")));
    }
    if u.interactive {
        crate::upspec::write_confirm_flag(ctx, &cat!(d, "/run"), &u.confirm);
    }
    // --clean (#252): animations off, read by the box's config on every load.
    if u.clean
        && !u.clean_has("animations")
        && !boxes::box_file(ctx, &ctx.at("run/omabox.clean"), b"animations off")
    {
        return Err(ctx.die("up: could not write the box's clean settings"));
    }
    // --no-pointer (#228): as `pointer hide` keeps it, read by the box's config from its first load.
    if u.nopointer {
        write(&cat!(d, "/pointer"), b"hidden\nwas false\n");
        if !boxes::box_file(ctx, &ctx.at("run/omabox.pointer"), b"hidden") {
            return Err(ctx.die("up: could not write the box's pointer setting"));
        }
    }
    let etc = etc_files(ctx, &u)?;
    let protect = u.protect.clone();
    let args = bwrap_args(
        ctx,
        &u,
        &render,
        &renders,
        &nvidia_devs,
        &nvidia_cards,
        wayland_screen,
        &etc,
        &desk_fonts,
    )?;
    let script = launch_script(ctx, &u, &args, &protect)?;
    if !write(&cat!(d, "/launch.sh"), &script) {
        return Err(ctx.die(cat!("up: could not write ", d, "/launch.sh")));
    }
    // Its reaper now, before the box starts (#137): an up killed after the launch left a box nothing
    // took down. `starting` (this up, as pid and start time) keeps the reaper from counting the rest
    // as idle.
    write(
        &cat!(d, "/starting"),
        &cat!(
            pid_self(),
            " ",
            procfs::proc_start(&pid_self()).unwrap_or_default(),
            "\n"
        ),
    );
    boxes::touch(&ctx.at("used"));
    if u.idle_s != 0 || !u.owner.is_empty() || !u.agent.is_empty() || u.interactive {
        start_reaper(ctx);
    }
    // From here a box may be running.
    // What is launched is marked for the undo under one lock with the launch itself: a signal's undo
    // either finds it marked (and waits for the box to show, then ends it) or comes first, and then
    // nothing is launched (main parks; the stop ends this process).
    if u.interactive {
        let host = crate::host::HostSilent::new(ctx);
        host.passthrough()?;
        {
            let mut l = launched
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if stopping() {
                drop(l);
                stop_parks();
            }
            l.asked = true;
        }
        host.exec_silent(&cat!(d, "/launch.sh"), &u.ws)?;
    } else {
        let script = cat!(d, "/launch.sh");
        let mut c = match launch_delay() {
            // (the suite's seam: the launcher held this long before it runs launch.sh, so a signal
            // lands between the hand-off and the box's first pid)
            Some(secs) => {
                let mut c = crate::sig::command("sh");
                c.args(["-c", "sleep \"$1\"; exec bash \"$0\""])
                    .arg(os(&script))
                    .arg(os(&secs));
                c
            }
            None => {
                let mut c = crate::sig::command("bash");
                c.arg(os(&script));
                c
            }
        };
        c.stdin(std::process::Stdio::null());
        // (`bash launch.sh &`: INT and QUIT ignored in it and stdin /dev/null, as bash starts what
        // it puts in the background with job control off; the box is not waited for)
        crate::exec::background(&mut c);
        let mut l = launched
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if stopping() {
            drop(l);
            stop_parks();
        }
        match c.spawn() {
            Ok(ch) => l.child = Some(ch),
            Err(_) => {
                drop(l);
                return Err(ctx.die("up: could not run the box's launch.sh"));
            }
        }
    }
    let r_owner = owner;
    let mut r = Ready {
        owner: r_owner,
        covers: !u.cover_srcs.is_empty(),
        shell_before,
        bar_at: 0,
        before_end: &drop_trap,
    };
    let end = crate::settle::after(10);
    let pid = loop {
        if let BoxPid::Pid(p) = box_pid(&d) {
            break p;
        }
        // pasta's shell could not set the box's network up, and ended the start: said now, with
        // what failed, not as "bwrap did not start" after the wait (#364).
        if let Some(why) = net_failed(&std::fs::read(ctx.at("box.log")).unwrap_or_default()) {
            return Err(ctx.die(cat!(ctx.label(), why, "; see ", d, "/box.log")));
        }
        if crate::settle::past(end) {
            // bwrap writes info.json as it starts: with it there, the box started and was already
            // gone (finding 95).
            if std::fs::metadata(ctx.at("info.json")).map_or(true, |m| m.len() == 0) {
                return Err(ctx.die(cat!(
                    ctx.label(),
                    "the box's bwrap did not start; see ",
                    d,
                    "/box.log"
                )));
            }
            return Err(ready::died_starting(ctx, r.covers));
        }
        crate::sig::sleep(Duration::from_millis(50));
    };
    // What box_pid checks from now on: this box's pid namespace, not a later owner of the pid.
    let Some(ns) = procfs::pidns(&pid) else {
        return Err(ready::died_starting(ctx, r.covers));
    };
    if !crate::mon::jq_box_json(
        ctx,
        crate::jq::Args::new(&[])
            .kv("--arg", "ns", &ns)
            .arg(".pidns = $ns"),
    ) {
        return Err(ctx.die(cat!(
            "up: could not record box '",
            name,
            "''s pid namespace"
        )));
    }
    // A jailed caller's covers (#210): checked on the box's own mounts while its PID 1 waits.
    if !u.cover_srcs.is_empty() {
        covers_check(ctx, &u, &r)?;
    }
    ready::wait_hyprland(ctx, &r)?;
    // S1 (#355): the started box's own mounts read back; one of the seat's paths there ends it. Once
    // its Hyprland is up: bwrap names its PID 1 before it makes its mounts, and a root read before
    // that is still the host's (/dev/input there).
    seat_check(ctx, &pid)?;
    install::hypr_version(ctx);
    install::qs_version(ctx);
    ready::wait_ready(ctx, &mut r, u.shell)?;
    // More monitors (#122), as `monitor add` makes them: one at a time, each placed by what the last
    // one really became. Under up's lock (UP_LOCKED). An interactive one's own undo removes its output
    // on a signal, before up's ends the box (finding 434).
    crate::mon::set_up_locked(true);
    for spec in &u.monitors {
        if u.interactive {
            crate::deskwin::monitor_add_window(ctx, spec)?;
        } else {
            crate::cmd::monitor::monitor_add(ctx, spec, b"")?;
        }
    }
    crate::mon::set_up_locked(false);
    // The shell crashing just after its bar came (#126): the box stays up, and up says so, exit 1.
    let crashed = if u.shell {
        ready::shell_watch(ctx, &r)
    } else {
        None
    };
    // Crashes said here are not said again by the next command (shell_note, #147).
    crate::shellnote::shell_seen(ctx);
    if crashed.is_none() {
        crate::plugins::plugin_check(ctx, &install::git_rev)?;
    }
    ready::client_render_check(ctx)?;
    // --audio (#226): its speaker the default output before up returns.
    if u.audio && !install::audio_ready(ctx) {
        return Err(ctx.die(cat!(
            "up --audio: the box's PipeWire did not come up in 10 s (omabox log -b ",
            name,
            " audio)"
        )));
    }
    ready::up_owner_check(ctx, r.owner.as_ref(), r.before_end)?;
    drop_trap();
    if u.interactive {
        say(cat!(
            "box '",
            name,
            "' is a window on ",
            crate::settle::ws_text(&u.ws),
            "; ",
            crate::host::PASS_KEY,
            " passes SUPER keys to it (again to stop; omabox keys-to-box -b ",
            name,
            " on: whenever its window has focus and the pointer)"
        ));
    }
    let _ = std::fs::remove_file(ctx.at("starting"));
    boxes::touch(&ctx.at("used"));
    let screen = crate::screen::box_screen(ctx)?;
    let live_sc = crate::mon::main_scale_live(ctx).unwrap_or_default();
    let sp = screen
        .iter()
        .position(|&c| c == b' ')
        .unwrap_or(screen.len());
    let (main, size) = (
        screen[..sp].to_vec(),
        screen.get(sp + 1..).unwrap_or_default().to_vec(),
    );
    let sc = if live_sc.is_empty() {
        b"1".to_vec()
    } else {
        live_sc
    };
    say(cat!(
        "box '",
        name,
        "' up (",
        crate::mon::size_text(&size, &sc),
        ready::more_screens(ctx, &main),
        ") in ",
        d
    ));
    if u.clean {
        say(cat!(
            "clean: ",
            crate::upspec::clean_text(
                &u.clean_keep,
                &u.theme,
                &u.net,
                &u.clean_lang,
                &u.clean_zone
            )
        ));
    }
    // A Hyprland that stops answering here (finding 353): said, the box left up, exit 1 at the end.
    let unanswered = !crate::hypr::hypr_errors_note(ctx)?;
    // No idle limit and no agent or owner to go with (#220): it outlives its caller, said to a caller
    // that is not a person at a terminal.
    if !u.interactive && u.idle_s == 0 && u.owner.is_empty() && u.agent.is_empty() && !at_term {
        let by = if starter.is_empty() {
            Vec::new()
        } else {
            let w: Vec<&[u8]> = starter.splitn(3, |&c| c == b' ').collect();
            cat!(
                " (",
                w.get(2).copied().unwrap_or_default(),
                ", pid ",
                w.first().copied().unwrap_or_default(),
                ")"
            )
        };
        say(cat!(
            "note: box '",
            name,
            "' has no idle limit and goes with no agent: nothing takes it down but omabox down ",
            name,
            "; once what started it",
            by,
            " is gone, ls calls it an orphan"
        ));
    }
    // Links at the top of the repo out of it, to what the box lacks (#235); a throwaway's: run's.
    crate::repo::up_link_notes(ctx, if u.owner.is_empty() { "up" } else { "run" });
    if u.json {
        out(ready::box_json(ctx, false, "", ""))?;
    } else if u.new {
        out(cat!(name, "\n"))?;
    }
    if let Some(c) = crashed {
        say(c);
        return Err(Exit(1));
    }
    if unanswered {
        return Err(Exit(1));
    }
    Ok(())
}

/// What up launched, for its undo: the headless box's launcher (`bash launch.sh`, which becomes
/// pasta, or bwrap without it), or that the host Hyprland was asked to run it (interactive).
#[derive(Default)]
struct Launched {
    child: Option<std::process::Child>,
    asked: bool,
}

/// Main, about to launch under the lock, finds a stop already on its way: it launches nothing and
/// waits here for the stop to end this process.
fn stop_parks() -> ! {
    loop {
        crate::sig::sleep(Duration::from_secs(1));
    }
}

fn stopping() -> bool {
    crate::sig::STOPPING.load(std::sync::atomic::Ordering::SeqCst)
}

/// The undo of a box launched but perhaps not showing yet: at most 10 s (up's own wait for its
/// pid) until it shows (its PID 1, its pasta, or bwrap's info file) or its launcher is gone, so
/// kill_box then finds it (H1: a signal in the ~5 ms between the hand-off and pasta.pid, or while an
/// interactive box's Hyprland runs it, left a box up that the reaper adopted).
fn launched_wait(ctx: &Ctx, child: &mut Option<std::process::Child>) {
    let end = crate::settle::after(10);
    loop {
        let shown = matches!(box_pid(&ctx.d), BoxPid::Pid(_))
            || crate::boxes::box_pasta(&ctx.d).is_some()
            || std::fs::metadata(ctx.at("info.json")).is_ok_and(|m| m.len() > 0);
        let gone = child
            .as_mut()
            .is_some_and(|c| !matches!(c.try_wait(), Ok(None)));
        if shown || gone || crate::settle::past(end) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// OMABOX_TEST_LAUNCH_DELAY: the suite's hold of a headless launcher, whole seconds 1-30.
fn launch_delay() -> Option<Vec<u8>> {
    let v = shell::env("OMABOX_TEST_LAUNCH_DELAY")?;
    let n = shell::dec(&v).filter(|n| (1..=30).contains(n) && v.iter().all(u8::is_ascii_digit))?;
    Some(n.to_string().into_bytes())
}

/// The cover file and folder, 444 and 555, the go pipe and covers-pending (#210).
fn make_covers(d: &[u8]) -> bool {
    let p = |rel: &str| shell::path(&cat!(d, "/", rel));
    let fifo = std::ffi::CString::new(cat!(d, "/go")).ok();
    std::fs::write(p("cover-file"), b"").is_ok()
        && std::fs::create_dir(p("cover-dir")).is_ok()
        && std::fs::set_permissions(p("cover-file"), std::fs::Permissions::from_mode(0o444)).is_ok()
        && std::fs::set_permissions(p("cover-dir"), std::fs::Permissions::from_mode(0o555)).is_ok()
        // SAFETY: mkfifo with a NUL-terminated path of ours.
        && fifo.is_some_and(|f| unsafe { libc::mkfifo(f.as_ptr(), 0o600) } == 0)
        && std::fs::write(p("covers-pending"), b"").is_ok()
}

fn seed_plan(u: &Up) -> crate::seed::SeedPlan {
    crate::seed::SeedPlan {
        from: !u.from.is_empty(),
        stock_bar: u.stock_bar,
        desk_shell: u.desk_shell,
        desk: u.desk.clone(),
        desk_shown: u.desk_shown.clone(),
        desk_ids: u
            .desk_ids
            .split(|&c| c == b' ')
            .filter(|w| !w.is_empty())
            .map(<[u8]>::to_vec)
            .collect(),
        plugins: u
            .plugins
            .iter()
            .map(|p| (p.id.clone(), p.dir.clone()))
            .collect(),
        linked: u.linked.clone(),
        theme: u.theme.clone(),
        theme_dirs: u.theme_dirs.clone(),
        no_seed: u.no_seed.clone(),
        clean: u.clean,
        clean_keep: u.clean_keep.clone(),
        omarchy: u.omarchy.clone(),
    }
}

/// `setsid -f "$ROOT/bin/omabox" _reap NAME CREATED </dev/null >>"$D/reap.log" 2>&1`.
fn start_reaper(ctx: &Ctx) {
    let created = meta(&ctx.d, "created").0;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ctx.at("reap.log"));
    let Ok(log) = log else { return };
    let Ok(log2) = log.try_clone() else { return };
    let mut c = crate::sig::command("setsid");
    c.arg("-f")
        .arg(crate::root().join("bin/omabox"))
        .arg("_reap")
        .arg(os(&ctx.name))
        .arg(os(&created))
        .stdin(std::process::Stdio::null())
        .stdout(log)
        .stderr(log2);
    // (setsid -f forks the reaper off and returns at once)
    let _ = crate::sig::run_status(c);
}

/// box.json as cmd_up's jq writes it, in its key order.
#[allow(clippy::too_many_arguments)]
fn box_json(
    ctx: &Ctx,
    u: &Up,
    render: &RenderNode,
    wayland_screen: bool,
    owner: &Option<Owner>,
    starter: &[u8],
    at_term: bool,
    desk_files: &[u8],
    desk_fonts: &[(Vec<u8>, Vec<u8>)],
) -> Vec<u8> {
    let s = |v: &[u8]| Value::String(String::from_utf8_lossy(v).into_owned());
    let arr = |v: &[Vec<u8>]| Value::Array(v.iter().map(|x| s(x)).collect());
    let numv = |lit: &[u8]| {
        serde_json::from_str::<Value>(&json::number(&String::from_utf8_lossy(lit)))
            .unwrap_or(Value::Null)
    };
    let mut o = Map::new();
    o.insert("name".into(), s(&ctx.name));
    o.insert(
        "mode".into(),
        s(if u.interactive {
            b"interactive"
        } else {
            b"headless"
        }),
    );
    o.insert(
        "size".into(),
        s(if u.interactive { b"window" } else { &u.size }),
    );
    o.insert("scale".into(), numv(&u.scale));
    o.insert("created".into(), s(&shell::date_iso()));
    let ids: Vec<Vec<u8>> = u.plugins.iter().map(|p| p.id.clone()).collect();
    o.insert("plugins".into(), arr(&ids));
    let mut dirs = Map::new();
    for p in &u.plugins {
        dirs.insert(String::from_utf8_lossy(&p.id).into_owned(), s(&p.dir));
    }
    o.insert("plugin_dirs".into(), Value::Object(dirs));
    let mut via = Map::new();
    for id in &ids {
        let how: &[u8] = if u.linked.contains(id) {
            b"linked"
        } else {
            b"mounted"
        };
        via.insert(String::from_utf8_lossy(id).into_owned(), s(how));
    }
    o.insert("plugin_via".into(), Value::Object(via));
    o.insert("net".into(), s(&u.net));
    o.insert("allow".into(), s(&u.allow));
    o.insert("idle".into(), Value::from(u.idle_s));
    let bar: &[u8] = if u.stock_bar {
        b"stock"
    } else if u.desk_shell {
        b"desk"
    } else {
        b"user"
    };
    o.insert("bar".into(), s(bar));
    o.insert("systemd".into(), Value::Bool(u.systemd));
    o.insert("audio".into(), Value::Bool(u.audio));
    o.insert("overlays".into(), arr(&u.overlays));
    // The folders bound in, each source's device and inode as up found it (#391): `run` tells one
    // deleted and made again on the host, which the box keeps as it was (empty).
    let mounts: Vec<Value> = u
        .ro_args
        .iter()
        .map(|(src, dest)| (src, dest))
        .chain(u.overlays.iter().map(|p| (p, p)))
        .filter_map(|(src, dest)| {
            use std::os::unix::fs::MetadataExt;
            let m = std::fs::metadata(shell::path(src)).ok()?;
            let mut e = Map::new();
            e.insert("src".into(), s(src));
            e.insert("dest".into(), s(dest));
            e.insert("dev".into(), Value::from(m.dev()));
            e.insert("ino".into(), Value::from(m.ino()));
            Some(Value::Object(e))
        })
        .collect();
    o.insert("mounts".into(), Value::Array(mounts));
    o.insert("wayland_screen".into(), Value::Bool(wayland_screen));
    o.insert("xwayland".into(), Value::Bool(u.xwayland));
    o.insert("autoreload".into(), Value::Bool(u.autoreload));
    o.insert("shell".into(), Value::Bool(u.shell));
    let ov = if u.omarchy.is_empty() {
        install::omarchy_version()
    } else {
        let v = install::git_rev(&u.omarchy);
        if v.is_empty() {
            b"dev".to_vec()
        } else {
            cat!("dev ", v)
        }
    };
    o.insert("omarchy_version".into(), s(&ov));
    let mut aq = Map::new();
    aq.insert("kind".into(), s(&u.aq.kind));
    aq.insert("version".into(), s(&u.aq.ver));
    aq.insert("fixed".into(), Value::Bool(u.aq.fixed));
    if !u.aq.main.is_empty() {
        aq.insert("main_checkout".into(), s(&u.aq.main));
    }
    o.insert("aquamarine".into(), Value::Object(aq));
    if !u.interactive {
        let mut r = Map::new();
        r.insert("node".into(), s(render.as_bytes()));
        r.insert("pci".into(), s(&render.slot()));
        r.insert("driver".into(), s(&render.driver()));
        let fb = render::render_fallback(render);
        r.insert("fallback".into(), Value::Bool(fb));
        if fb {
            let wanted = crate::conf::conf_get("gpu", crate::gpu::conf_gpu, b"auto");
            r.insert("wanted".into(), s(&wanted));
            r.insert("why".into(), s(&render::gpu_why_not(&wanted)));
        }
        o.insert("render".into(), Value::Object(r));
    }
    if !u.seeds.is_empty() {
        let v: Vec<Vec<u8>> = u.seeds.iter().map(|(a, b)| cat!(a, "\t", b)).collect();
        o.insert("seeds".into(), arr(&v));
    }
    if !u.no_seed.is_empty() {
        let v: Vec<Vec<u8>> = u
            .no_seed
            .split(|&c| c == b',')
            .map(<[u8]>::to_vec)
            .collect();
        o.insert("no_seed".into(), arr(&v));
    }
    if !desk_fonts.is_empty() {
        let v: Vec<Vec<u8>> = desk_fonts.iter().map(|(_, rel)| cat!("~/", rel)).collect();
        o.insert("desk_fonts".into(), arr(&v));
    }
    if u.desk_shell {
        let mut ds = Map::new();
        ds.insert("config".into(), s(&u.desk));
        let ids: Vec<Vec<u8>> = u
            .desk_ids
            .split(|&c| c == b' ')
            .filter(|w| !w.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
        ds.insert("plugins".into(), arr(&ids));
        let mut copied = Vec::new();
        let mut skipped = Vec::new();
        for l in desk_files.split(|&c| c == b'\n') {
            if let Some(r) = l.strip_prefix(b"copied ".as_slice()) {
                copied.push(r.to_vec());
            } else if let Some(r) = l.strip_prefix(b"skipped ".as_slice()) {
                let mut m = Map::new();
                let mut it = r.split(|&c| c == b'\t');
                m.insert("path".into(), s(it.next().unwrap_or_default()));
                m.insert("why".into(), s(it.next().unwrap_or_default()));
                skipped.push(Value::Object(m));
            }
        }
        ds.insert("copied".into(), arr(&copied));
        ds.insert("skipped".into(), Value::Array(skipped));
        o.insert("desk_shell".into(), Value::Object(ds));
    }
    if !u.net_addrs.is_empty() {
        o.insert("addrs".into(), arr(&u.net_addrs));
    }
    if !u.theme_dirs.is_empty() {
        o.insert("theme_dirs".into(), arr(&u.theme_dirs));
    }
    if !u.theme.is_empty() {
        o.insert("theme".into(), s(&u.theme));
    }
    if !u.monitors.is_empty() {
        o.insert("monitor_specs".into(), arr(&u.monitors));
    }
    if in_jail() {
        o.insert("jail".into(), s(&crate::jail::jail_id()));
    } else {
        o.insert("dir".into(), s(&shell::pwd()));
    }
    if !u.from.is_empty() {
        o.insert("from".into(), s(&u.from));
    }
    if !u.hyprland.is_empty() {
        o.insert("hyprland".into(), s(&u.hyprland));
    }
    if !u.quickshell.is_empty() {
        o.insert("quickshell".into(), s(&u.quickshell));
    }
    if !u.omarchy.is_empty() {
        o.insert("omarchy".into(), s(&u.omarchy));
    }
    if !u.owner.is_empty() {
        o.insert("owner".into(), numv(&u.owner));
    }
    if u.clean {
        let mut c = Map::new();
        let keep: Vec<Vec<u8>> = u
            .clean_keep
            .split(|&c| c == b',')
            .filter(|w| !w.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
        c.insert("keep".into(), arr(&keep));
        c.insert(
            "locale".into(),
            s(&crate::upspec::clean_locale_rec(&u.clean_lang)),
        );
        c.insert(
            "timezone".into(),
            s(&crate::upspec::clean_zone_rec(&u.envs, &u.clean_zone)),
        );
        o.insert("clean".into(), Value::Object(c));
    }
    if let Some(ow) = owner
        && !ow.start.is_empty()
    {
        o.insert("owner_start".into(), s(&ow.start));
    }
    if !u.agent.is_empty() {
        let mut w = u.agent.split(|&c| c == b' ');
        let pid = w.next().unwrap_or_default();
        let start = w.next().unwrap_or_default();
        o.insert("agent".into(), numv(pid));
        o.insert("agent_start".into(), s(start));
    }
    let mut st = Map::new();
    let w: Vec<&[u8]> = starter.splitn(3, |&c| c == b' ').collect();
    if let [pid, start, command] = w.as_slice()
        && !pid.is_empty()
        && pid.iter().all(u8::is_ascii_digit)
        && !start.is_empty()
        && start.iter().all(u8::is_ascii_digit)
        && !command.is_empty()
    {
        st.insert("pid".into(), numv(pid));
        st.insert("start".into(), s(start));
        st.insert("command".into(), s(command));
    }
    st.insert("terminal".into(), Value::Bool(at_term));
    o.insert("starter".into(), Value::Object(st));
    if u.interactive {
        o.insert("workspace".into(), s(&u.ws));
        o.insert("confirm_close".into(), Value::Bool(u.confirm == b"on"));
        o.insert("confirm_close_from".into(), s(u.confirm_from.as_bytes()));
        o.insert("drawn_hidden".into(), Value::Bool(true));
    }
    cat!(json::pretty(&Value::Object(o)), "\n")
}

/// The box's own /etc files in $D/etc (finding 135, 363, #293): /etc/omarchy.conf, --omarchy's xkb,
/// --clean's locale and timezone. Each goes over the host's; one the host lacks, or a link, makes /etc
/// a read-only overlay with $D/etc on top. Returns (overlay, binds): the overlay's dir, or each file's
/// (src, dest).
struct Etc {
    overlay: bool,
    files: Vec<(Vec<u8>, Vec<u8>)>,
}

fn etc_files(ctx: &Ctx, u: &Up) -> Res<Etc> {
    let d = &ctx.d;
    let etc = cat!(d, "/etc");
    let _ = std::fs::create_dir_all(shell::path(&etc));
    // /etc/omarchy.conf, which Omarchy's env-bootstrap sources in every bash: a box's own, the
    // packaged Omarchy or --omarchy's tree (finding 135).
    if std::path::Path::new("/etc/omarchy.conf").exists() || !u.omarchy.is_empty() {
        write(
            &cat!(etc, "/omarchy.conf"),
            &cat!("export OMARCHY_PATH=", quote_q(&u.tree()), "\n"),
        );
    }
    // --omarchy's own xkb files (#293, finding 420), over the host's when they differ; what the tree
    // runs that this host lacks, said.
    let xkb = cat!(etc, "/xkb");
    crate::ready::rm_tree(b"", std::slice::from_ref(&xkb));
    if !u.omarchy.is_empty() {
        let tx = cat!(u.omarchy, "/etc/xkb");
        if shell::path(&tx).is_dir() && !same_tree(&tx, b"/etc/xkb") {
            let ok = (!std::path::Path::new("/etc/xkb").is_dir() || cp_a(b"/etc/xkb", &xkb))
                && std::fs::create_dir_all(shell::path(&xkb)).is_ok()
                && cp_a(&cat!(tx, "/."), &cat!(xkb, "/"));
            if !ok {
                return Err(ctx.die(cat!(
                    "up: --omarchy: could not lay ",
                    u.omarchy,
                    "/etc/xkb over the box's /etc/xkb"
                )));
            }
        }
        for gap in install::omarchy_host_gaps(&u.omarchy) {
            say(cat!("up: --omarchy: this host has no ", gap));
        }
    }
    // --clean's locale and timezone (finding 363): localtime is a link, as the host's, which goes in
    // through the overlay.
    let _ = std::fs::remove_file(shell::path(&cat!(etc, "/locale.conf")));
    let _ = std::fs::remove_file(shell::path(&cat!(etc, "/localtime")));
    if !u.clean_lang.is_empty() {
        write(
            &cat!(etc, "/locale.conf"),
            &cat!("LANG=", u.clean_lang, "\n"),
        );
        let _ = std::os::unix::fs::symlink(
            shell::path(&cat!("/usr/share/zoneinfo/", u.clean_zone)),
            shell::path(&cat!(etc, "/localtime")),
        );
    }
    let mut e = Etc {
        overlay: false,
        files: Vec::new(),
    };
    // (`for f in "$D"/etc/*`: byte order, no hidden ones)
    for n in shell::glob(&shell::path(&etc), false) {
        if n.starts_with(b".") {
            continue;
        }
        let f = cat!(etc, "/", n);
        let host = cat!("/etc/", n);
        let fp = shell::path(&f);
        if !fp.exists() {
            continue;
        }
        if shell::path(&host).exists() && !fp.is_symlink() {
            e.files.push((f, host));
        } else {
            e.overlay = true;
        }
    }
    Ok(e)
}

/// `diff -rq A B >/dev/null 2>&1` succeeds: the two trees hold the same.
fn same_tree(a: &[u8], b: &[u8]) -> bool {
    let mut c = crate::sig::command("diff");
    c.arg("-rq")
        .arg("--")
        .arg(os(a))
        .arg(os(b))
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    crate::sig::run_status(c).is_ok_and(|rc| rc == 0)
}

/// `cp -a -- SRC DEST`.
fn cp_a(src: &[u8], dest: &[u8]) -> bool {
    let mut c = crate::sig::command("cp");
    c.args(["-a", "--"])
        .arg(os(src))
        .arg(os(dest))
        .stdin(std::process::Stdio::inherit());
    crate::sig::run_status(c).is_ok_and(|rc| rc == 0)
}

/// `uname -n`.
fn hostname() -> Vec<u8> {
    // SAFETY: uname fills a struct of ours; nodename is NUL-terminated within it.
    unsafe {
        let mut u: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut u) != 0 {
            return Vec::new();
        }
        std::ffi::CStr::from_ptr(u.nodename.as_ptr())
            .to_bytes()
            .to_vec()
    }
}

/// cmd_up's `args`: the bwrap words after `--unshare-user --uid --gid --die-with-parent`, through
/// Bwrap's checked methods (S1, S3, S4), in bash's order; ends with the session's command.
#[allow(clippy::too_many_arguments)]
fn bwrap_args(
    ctx: &Ctx,
    u: &Up,
    render: &RenderNode,
    renders: &[RenderNode],
    nvidia_devs: &[NvidiaDev],
    nvidia_cards: &[Vec<u8>],
    wayland_screen: bool,
    etc: &Etc,
    desk_fonts: &[(Vec<u8>, Vec<u8>)],
) -> Res<Vec<Vec<u8>>> {
    let d = &ctx.d;
    let run = cat!("/run/user/", procfs::uid().to_string());
    let root = paths::root();
    let home_dir = cat!(paths::homes(), "/", ctx.name, "/home");
    let runtime = shell::env_or_empty("XDG_RUNTIME_DIR");
    let mut own_roots = vec![root.clone()];
    if !u.aq.lib.is_empty() {
        own_roots.push(u.aq.lib.clone());
    }
    let mut b = Bwrap::new(
        &runtime,
        d,
        vec![cat!(d, "/run"), home_dir.clone()],
        own_roots,
    );
    let refused = |e: crate::bwrap::Refused| ctx.die(cat!(ctx.label(), e.0));
    // The host's name in both network modes (finding 54).
    b.words(&[
        b"--info-fd",
        b"3",
        b"--unshare-pid",
        b"--unshare-ipc",
        b"--unshare-uts",
        b"--hostname",
        &hostname(),
    ]);
    b.sys_ro("/usr").map_err(refused)?;
    b.words(&[
        b"--symlink",
        b"usr/bin",
        b"/bin",
        b"--symlink",
        b"usr/lib",
        b"/lib",
        b"--symlink",
        b"usr/lib",
        b"/lib64",
    ]);
    if etc.overlay {
        b.etc_overlay(&cat!(d, "/etc")).map_err(refused)?;
    } else {
        b.sys_ro("/etc").map_err(refused)?;
        for (src, dest) in &etc.files {
            b.etc_file(src, dest).map_err(refused)?;
        }
    }
    b.words(&[b"--proc", b"/proc", b"--dev", b"/dev"]);
    b.sys_ro("/sys").map_err(refused)?;
    b.words(&[b"--tmpfs", b"/tmp"]);
    b.own_rw(&cat!(d, "/run"), &run).map_err(refused)?;
    b.own_rw(&home_dir, b"/home/sbx").map_err(refused)?;
    b.own_ro(&cat!(root, "/share"), b"/opt/omabox/share")
        .map_err(refused)?;
    for (tool, dest) in [
        ("keyboard", BOX_KEYBOARD),
        ("pointer", BOX_POINTER),
        ("still", BOX_STILL),
        ("events", BOX_EVENTS),
    ] {
        b.own_ro(&paths::tool(tool), dest.as_bytes())
            .map_err(refused)?;
    }
    // shot --burst's one capture client (#196), `tree`'s reader (#178): when built.
    for (tool, dest) in [("burst", BOX_BURST), ("tree", BOX_TREE)] {
        let t = paths::tool(tool);
        if crate::exec::executable(&t) {
            b.own_ro(&t, dest.as_bytes()).map_err(refused)?;
        }
    }
    // A private aquamarine (finding 125), which share/start-hyprland.sh puts first.
    if !u.aq.lib.is_empty() {
        b.own_ro(&u.aq.lib, b"/opt/omabox/lib").map_err(refused)?;
    }
    let dev = |p: &[u8]| {
        DevNode::new(p).ok_or_else(|| ctx.die(cat!(p, " is not a device a box may have")))
    };
    for n in renders {
        b.dev(&dev(n.as_bytes())?).map_err(refused)?;
    }
    if !nvidia_devs.is_empty() {
        b.dev(&dev(NvidiaDev::ctl().as_bytes())?).map_err(refused)?;
        // CUDA allocates through /dev/nvidia-uvm (#149), there when the driver made it.
        if is_char(b"/dev/nvidia-uvm") {
            b.dev(&dev(NvidiaDev::uvm().as_bytes())?).map_err(refused)?;
        }
    }
    for n in nvidia_devs {
        b.dev(&dev(n.as_bytes())?).map_err(refused)?;
    }
    // NVIDIA's Wayland EGL stats the GPU's card node (#204, finding 294): an empty read-only file at
    // that path, never the device.
    if !nvidia_cards.is_empty() {
        for c in nvidia_cards {
            if c.is_empty() {
                // (the file is made all the same, as bash's `: >` before the loop)
                let _ = std::fs::write(shell::path(&cat!(d, "/card-placeholder")), b"");
                continue;
            }
            let ph = CardPlaceholder::make(d, c).map_err(|e| {
                ctx.die(cat!(
                    "up: could not make the card placeholder in ",
                    d,
                    ": ",
                    e.to_string()
                ))
            })?;
            b.card_placeholder(ph.src(), ph.dest()).map_err(refused)?;
        }
    }
    // The desk's fonts (#261), read-only at their place in the box HOME; before --ro-bind's.
    for (src, rel) in desk_fonts {
        b.checked_ro(src, &cat!("/home/sbx/", rel))
            .map_err(refused)?;
    }
    for (src, dest) in &u.ro_args {
        b.user_ro(src, dest).map_err(refused)?;
    }
    // A real user manager (finding 61): the box's own cgroup writable as /sys/fs/cgroup.
    if u.systemd {
        b.words(&[b"--unshare-cgroup"]);
        b.cgroup_rw();
        b.words(&[b"--dir", b"/run/systemd/system"]);
    }
    // The box's journal (#234): a tmpfs of its own, never the host's /run/systemd/journal.
    b.words(&[b"--tmpfs", b"/run/systemd/journal"]);
    // DNS (finding 52): a connected box gets the host's resolver file when it links into /run.
    if u.net == b"connected"
        && let Some(resolv) = crate::repo::readlink_f(b"/etc/resolv.conf")
        && resolv.starts_with(b"/run/")
        && shell::path(&resolv).is_file()
    {
        b.resolv(&resolv).map_err(refused)?;
    }
    for p in &u.plugins {
        if !u.linked.contains(&p.id) {
            b.checked_ro(&p.dir, &cat!("/home/sbx/.config/omarchy/plugins/", p.id))
                .map_err(refused)?;
        }
    }
    for p in &u.overlays {
        b.tmp_overlay(p).map_err(refused)?;
    }
    // A clean box's shell.json as its shell's defaults too (finding 400; after the overlays).
    let defaults = cat!(d, "/shell-defaults.json");
    if shell::path(&defaults).is_file() {
        b.own_ro(&defaults, &cat!(u.tree(), "/config/omarchy/shell.json"))
            .map_err(refused)?;
    }
    // Last, so nothing is mounted over them: what a jailed caller's jail hides (#210), with the box's
    // PID 1 held on fd 4 (the go pipe) until jail_verify has seen them made.
    if !u.cover_srcs.is_empty() {
        for (kind, dest) in &u.covers {
            let src = match kind {
                crate::jailpath::CoverKind::Dir => cat!(d, "/cover-dir"),
                crate::jailpath::CoverKind::File => cat!(d, "/cover-file"),
            };
            b.checked_ro(&src, dest).map_err(refused)?;
        }
        b.words(&[b"--block-fd", b"4"]);
    }
    let mut hypr_env: Vec<(&[u8], Vec<u8>)> = Vec::new();
    if !u.hyprland.is_empty() {
        hypr_env.push((b"OMABOX_HYPRLAND", u.hyprland.clone()));
    }
    if !u.quickshell.is_empty() {
        hypr_env.push((b"OMABOX_QUICKSHELL", u.quickshell.clone()));
    }
    if !u.omarchy.is_empty() {
        hypr_env.push((b"OMABOX_OMARCHY", u.omarchy.clone()));
    }
    if let Some(v) = shell::env("OMABOX_AQUAMARINE").filter(|v| !v.is_empty()) {
        hypr_env.push((b"OMABOX_AQUAMARINE", v));
    }
    if !u.theme.is_empty() {
        hypr_env.push((b"OMABOX_THEME", u.theme.clone()));
    }
    if u.clean && !u.clean_has("wallpaper") {
        hypr_env.push((
            b"OMABOX_CLEAN_WALLPAPER",
            b"/home/sbx/.config/omabox/clean-wallpaper.png".to_vec(),
        ));
    }
    let bit = |v: bool| if v { b"1".as_slice() } else { b"0" };
    for (k, v) in [
        (
            b"PATH".as_slice(),
            b"/opt/omabox/share/bin:/usr/bin".to_vec(),
        ),
        (b"XDG_RUNTIME_DIR", run.clone()),
        (b"HOME", b"/home/sbx".to_vec()),
        (b"XDG_DATA_HOME", b"/home/sbx/.local/share".to_vec()),
        (b"XDG_CONFIG_HOME", b"/home/sbx/.config".to_vec()),
        (b"XDG_CACHE_HOME", b"/home/sbx/.cache".to_vec()),
        (b"XDG_STATE_HOME", b"/home/sbx/.local/state".to_vec()),
        (b"QT_FORCE_STDERR_LOGGING", b"1".to_vec()),
        (b"ATSPI_DBUS_IMPLEMENTATION", b"dbus-daemon".to_vec()),
        (b"OMABOX_RENDER_NODE", render.as_bytes().to_vec()),
        (b"OMABOX_CALLER_PATH", crate::exec::caller_path(true)),
        (b"OMABOX_WAYLAND_SCREEN", bit(wayland_screen).to_vec()),
        (b"OMABOX_SIZE", u.size.clone()),
        (b"OMABOX_SCALE", u.scale.clone()),
        (b"OMABOX_INTERACTIVE", bit(u.interactive).to_vec()),
        (b"OMABOX_XWAYLAND", bit(u.xwayland).to_vec()),
        (b"OMABOX_AUTORELOAD", bit(u.autoreload).to_vec()),
        (b"OMABOX_SYSTEMD", bit(u.systemd).to_vec()),
        (b"OMABOX_AUDIO", bit(u.audio).to_vec()),
        (b"OMABOX_SHELL", bit(u.shell).to_vec()),
        (b"OMABOX_OMARCHY_VERSION", install::omarchy_version()),
        (b"OMABOX_BOX", ctx.name.clone()),
    ] {
        b.setenv(k, &v);
    }
    for (k, v) in &hypr_env {
        b.setenv(k, v);
    }
    for (k, v) in &u.envs {
        b.setenv(k, v);
    }
    b.words(&[b"/opt/omabox/share/session.sh"]);
    Ok(b.into_args())
}

fn is_char(p: &[u8]) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(shell::path(p)).is_ok_and(|m| m.file_type().is_char_device())
}

/// The launch script (launch.sh): only fds 0-2 go on into the box, and the info fd (#112); env -i
/// with the session's few variables (findings 45, 49, 58, 363); a core limit of one byte when the
/// hard limit allows cores (nocore); setsid and the launch words (pasta's shell, wlfd for an
/// interactive box, systemd-run's scope for --systemd), then bwrap's.
fn launch_script(ctx: &Ctx, u: &Up, args: &[Vec<u8>], protect: &net::Ports) -> Res<Vec<u8>> {
    let d = &ctx.d;
    let mut launch: Vec<Vec<u8>> = vec![b"bwrap".to_vec()];
    if u.interactive {
        let (_, wl) = crate::host::host_session_die(ctx)?;
        let wlfd = paths::tool("wlfd");
        if !crate::exec::executable(&wlfd) {
            return Err(ctx.die(cat!(ctx.label(), "missing ", wlfd, ": run install.sh")));
        }
        launch = vec![wlfd, wl, b"bwrap".to_vec()];
    }
    // Every box has its own network namespace (finding 89): pasta's tap (connected), a loopback and
    // the --allow ports (isolated), or only a loopback (none: no pasta).
    let netmode = match u.net.as_slice() {
        b"none" => Net::None,
        b"isolated" => Net::Isolated {
            allow: u.allow_ports.clone(),
        },
        _ => Net::Connected {
            protect: protect.clone(),
        },
    };
    if netmode == Net::None {
        launch.push(b"--unshare-net".to_vec());
    } else {
        let pasta = net::pasta_args(&netmode, &cat!(d, "/pasta.pid"), &net::ephemeral());
        // pasta closes inherited fds: bwrap's info fd is opened inside it, as the go pipe (#210).
        let mut wrap = b"exec 3>\"$0\"; exec \"$@\"".to_vec();
        if !u.cover_srcs.is_empty() {
            wrap = b"exec 3>\"$0\" 4<>\"${0%/*}/go\"; exec \"$@\"".to_vec();
        }
        // --addr (#229) goes on the loopback here, the one place that can: this sh is pasta's root,
        // with CAP_NET_ADMIN in the network namespace pasta made; bwrap's user namespace below maps
        // the box back to the user with none. Checked by box_addr: a plain address, safe in the line.
        let mut add = Vec::new();
        for a in &u.net_addrs {
            if a.contains(&b':') {
                add.extend_from_slice(&cat!("ip -6 addr add ", a, "/128 dev lo nodad && "));
            } else {
                add.extend_from_slice(&cat!("ip addr add ", a, "/32 dev lo && "));
            }
        }
        if !add.is_empty() {
            wrap = cat!(
                "{ ",
                add,
                ":; } || { echo '",
                ADDR_FAILED,
                "' >&2; exit 1; }; ",
                wrap
            );
        }
        // The protected ports' rule (#355, S10), likewise before anything of the box runs, by the
        // same shell: nothing in the box can take it out. Its words are numbers and addresses only.
        if let Net::Connected { protect } = &netmode {
            let (v4, v6) = net::host_addrs();
            if let Some(rules) = net::protect_rules(protect, &v4, &v6) {
                wrap = cat!(
                    "nft ",
                    quote_q(rules.as_bytes()),
                    " || { echo '",
                    NFT_FAILED,
                    "' >&2; exit 1; }; ",
                    wrap
                );
            }
        }
        let mut l: Vec<Vec<u8>> = vec![b"pasta".to_vec()];
        l.extend(pasta);
        l.push(b"--".to_vec());
        l.push(b"sh".to_vec());
        l.push(b"-c".to_vec());
        l.push(wrap);
        l.push(cat!(d, "/info.json"));
        l.extend(launch);
        l.push(b"--unshare-user".to_vec());
        l.push(b"--uid".to_vec());
        l.push(procfs::uid().to_string().into_bytes());
        l.push(b"--gid".to_vec());
        // SAFETY: getgid cannot fail and touches no memory of ours.
        l.push(unsafe { libc::getgid() }.to_string().into_bytes());
        l.push(b"--die-with-parent".to_vec());
        launch = l;
    }
    // --systemd: the whole box in a transient scope that delegates its cgroup (finding 61).
    let mut sdenv: Vec<Vec<u8>> = Vec::new();
    if u.systemd {
        let mut l: Vec<Vec<u8>> = [
            "systemd-run",
            "--user",
            "--scope",
            "--quiet",
            "--collect",
            "--expand-environment=no",
            "-p",
            "Delegate=yes",
            "--unit",
        ]
        .iter()
        .map(|w| w.as_bytes().to_vec())
        .collect();
        l.push(cat!("omabox-", ctx.name, "-", pid_self()));
        l.push(b"bash".to_vec());
        l.push(b"-c".to_vec());
        l.push(
            b"cg=/sys/fs/cgroup$(sed -n \"s/^0:://p\" /proc/self/cgroup); exec \"${@//@CGROUP@/$cg}\""
                .to_vec(),
        );
        l.push(b"omabox-cgroup".to_vec());
        l.extend(launch);
        launch = l;
        sdenv.push(cat!(
            "XDG_RUNTIME_DIR=",
            shell::env_or_empty("XDG_RUNTIME_DIR")
        ));
    }
    let user = procfs::user_name(procfs::uid())
        .unwrap_or_default()
        .into_bytes();
    let login_shell = login_shell(procfs::uid());
    let lang = if !u.clean_lang.is_empty() {
        u.clean_lang.clone()
    } else {
        shell::env("LANG")
            .filter(|v| !v.is_empty())
            .unwrap_or(b"C.UTF-8".to_vec())
    };
    let mut s = b"#!/usr/bin/env bash\nfor f in /proc/self/fd/*; do f=${f##*/}; [ \"$f\" -le 2 ] || eval \"exec $f>&-\"; done 2>/dev/null; exec".to_vec();
    let mut q = |w: &[u8]| {
        s.push(b' ');
        s.extend_from_slice(&quote_q(w));
    };
    for w in [
        b"env".to_vec(),
        b"-i".to_vec(),
        cat!("PATH=", LAUNCH_PATH),
        b"TERM=dumb".to_vec(),
        cat!("LANG=", lang),
        cat!("USER=", user),
        cat!("LOGNAME=", user),
        cat!("SHELL=", login_shell),
    ] {
        q(&w);
    }
    for w in &sdenv {
        q(w);
    }
    if u.clean_lang.is_empty() {
        for (k, v) in host_locale_vars() {
            q(&cat!(k, "=", v));
        }
    }
    s.extend_from_slice(b" ${HL_EXEC_RULE_TOKEN:+HL_EXEC_RULE_TOKEN=$HL_EXEC_RULE_TOKEN}");
    let mut q = |w: &[u8]| {
        s.push(b' ');
        s.extend_from_slice(&quote_q(w));
    };
    // nocore, for the whole box
    if core_hard_limit() != 0 {
        q(b"prlimit");
        q(b"--core=1:");
    }
    q(b"setsid");
    for w in &launch {
        q(w);
    }
    q(b"--unsetenv");
    q(b"HL_EXEC_RULE_TOKEN");
    for w in args {
        q(w);
    }
    s.extend_from_slice(&cat!(
        " 3>",
        quote_q(&cat!(d, "/info.json")),
        " </dev/null >",
        quote_q(&cat!(d, "/box.log")),
        " 2>&1"
    ));
    // The go pipe (#210), for a box with no pasta (behind pasta, its sh opens it again).
    if !u.cover_srcs.is_empty() {
        s.extend_from_slice(&cat!(" 4<>", quote_q(&cat!(d, "/go"))));
    }
    s.push(b'\n');
    Ok(s)
}

/// What pasta's shell said in BOXLOG when it could not set the box's network up (ADDR_FAILED,
/// NFT_FAILED), as up says it: `could not ... (the first line before it: what failed, ip's or nft's)`.
fn net_failed(boxlog: &[u8]) -> Option<Vec<u8>> {
    let lines: Vec<&[u8]> = boxlog.split(|&c| c == b'\n').collect();
    let i = lines
        .iter()
        .position(|l| *l == ADDR_FAILED.as_bytes() || *l == NFT_FAILED.as_bytes())?;
    let said = lines[i].strip_prefix(b"omabox: ").unwrap_or(lines[i]);
    Some(
        match lines[..i]
            .iter()
            .find(|l| !l.iter().all(u8::is_ascii_whitespace))
        {
            Some(first) => cat!(said, " (", shell::cut_line(first, 300), ")"),
            None => said.to_vec(),
        },
    )
}

/// LANGUAGE and every LC_* the caller has set to something, in bash's order for `${!LC_*}` (byte
/// order of the names), as (name, value).
fn host_locale_vars() -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut v: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    if let Some(x) = shell::env("LANGUAGE").filter(|x| !x.is_empty()) {
        v.push((b"LANGUAGE".to_vec(), x));
    }
    let mut lc: Vec<(Vec<u8>, Vec<u8>)> = std::env::vars_os()
        .filter_map(|(k, val)| {
            let k = k.into_encoded_bytes();
            let val = val.into_encoded_bytes();
            (k.starts_with(b"LC_")
                && shell::re_match("^[A-Za-z_][A-Za-z0-9_]*$", &k)
                && !val.is_empty())
            .then_some((k, val))
        })
        .collect();
    lc.sort();
    v.extend(lc);
    v
}

/// `ulimit -Hc` as a number (0: no cores at all; unlimited as u64::MAX).
fn core_hard_limit() -> u64 {
    let mut r = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit fills a struct of ours.
    if unsafe { libc::getrlimit(libc::RLIMIT_CORE, &mut r) } != 0 {
        return 1;
    }
    r.rlim_max
}

/// `getent passwd "$(id -un)" | cut -d: -f7`, when it is executable; else /bin/bash.
fn login_shell(uid: u32) -> Vec<u8> {
    let mut buf = vec![0u8; 16384];
    // SAFETY: passwd is plain data getpwuid_r fills in.
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut res: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: every pointer is to memory we own, buf's length given.
    let rc =
        unsafe { libc::getpwuid_r(uid, &mut pw, buf.as_mut_ptr().cast(), buf.len(), &mut res) };
    let sh = if rc == 0 && !res.is_null() && !pw.pw_shell.is_null() {
        // SAFETY: pw_shell points into buf, NUL-terminated by getpwuid_r.
        unsafe { std::ffi::CStr::from_ptr(pw.pw_shell) }
            .to_bytes()
            .to_vec()
    } else {
        Vec::new()
    };
    if !sh.is_empty() && crate::exec::executable(&sh) {
        sh
    } else {
        b"/bin/bash".to_vec()
    }
}

/// A jailed caller's covers (#210): checked on the box's own mounts while its PID 1 waits, before
/// anything runs in it; the box goes (the trap) when one is missing.
fn covers_check(ctx: &Ctx, u: &Up, r: &Ready) -> Res<()> {
    let held = crate::settle::after(15);
    let bp = loop {
        if let BoxPid::Pid(p) = box_pid(&ctx.d)
            && ready::box_held(ctx, &p)
        {
            break p;
        }
        ready::starting_ok(ctx, r)?;
        if crate::settle::past(held) {
            return Err(ctx.die(
                "up: the box's start is not held for the check of what this jail hides: not started",
            ));
        }
        crate::sig::sleep(Duration::from_millis(50));
    };
    if let Err(miss) = crate::jailpath::jail_verify(&ctx.d, &bp, &u.cover_srcs) {
        let miss = shell::subst(&miss);
        let list: Vec<u8> = miss
            .split(|&c| c == b'\n')
            .collect::<Vec<_>>()
            .join(b", ".as_slice());
        let srcs = u.cover_srcs.join(&b' ');
        return Err(ctx.die(cat!(
            "up: the box could not hide what this jail hides in ",
            srcs,
            " (",
            list,
            "): not started. If the jail renamed a folder meanwhile, try again"
        )));
    }
    let _ = std::fs::remove_file(ctx.at("covers-pending"));
    // (`exec {go}<>go; printf go >&"$go"`: read-write, so the open never waits for a reader)
    let go = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(ctx.at("go"));
    if let Ok(mut f) = go {
        let _ = std::io::Write::write_all(&mut f, b"go");
    }
    Ok(())
}

/// S1's self-check (#355, #363): the started box's own mounts, read back through its PID 1, hold
/// none of the seat's paths (a card node that is a device, /dev/input, seatd's socket, the system
/// bus, the user's runtime dir or anything in it), at their own path or anywhere else. One there ends
/// the box, said: the bwrap words were wrong somewhere.
fn seat_check(ctx: &Ctx, pid: &[u8]) -> Res<()> {
    let runtime = shell::env_or_empty("XDG_RUNTIME_DIR");
    if let Some(why) = seat_visible(pid, &runtime, &ctx.d) {
        return Err(ctx.die(cat!(
            "up: box '",
            ctx.name,
            "' has ",
            why,
            " (S1: never the seat's paths, checked on its own mounts): not started"
        )));
    }
    Ok(())
}

/// What of the seat the box whose PID 1 is PID has, or None. Its mounts (seat_mounts: by where they
/// are and by what they are of, OWN its box dir); each mount point of its mountinfo, as the box sees
/// it: a DRM card node there must be the box's empty placeholder file (finding 294), and so must
/// every card in its /dev/dri; no mounted device may be a card or an input device by its number;
/// none of /dev/input, /run/seatd.sock or /run/dbus may be one or be there; and the runtime dir at
/// its path must not be the user's own (the same file). What cannot be read is no pass (fail closed).
pub fn seat_visible(pid: &[u8], runtime: &[u8], own: &[u8]) -> Option<Vec<u8>> {
    let root = cat!("/proc/", pid, "/root");
    let mi = cat!("/proc/", pid, "/mountinfo");
    let unread = |p: &[u8], e: std::io::Error| {
        Some(cat!(
            "mounts that cannot be read (",
            p,
            ": ",
            e.to_string(),
            ")"
        ))
    };
    let info = match std::fs::read(shell::path(&mi)) {
        Ok(i) => i,
        Err(e) => return unread(&mi, e),
    };
    // (omabox's own view of the mounts: what names each of the box's sources)
    let host = match std::fs::read("/proc/self/mountinfo") {
        Ok(i) => i,
        Err(e) => return unread(b"/proc/self/mountinfo", e),
    };
    let real = |p: &[u8]| crate::repo::readlink_f(p).unwrap_or_else(|| crate::bwrap::lexical(p));
    let rt = if runtime.is_empty() {
        Vec::new()
    } else {
        real(runtime)
    };
    if let Some(why) = seat_mounts(&info, &host, &rt, &real(own)) {
        return Some(why);
    }
    // (`there`: Some(true) when the path exists in the box, Some(false) when it does not, None when
    // the look itself fails)
    let there = |p: &[u8]| match std::fs::symlink_metadata(shell::path(&cat!(root, p))) {
        Ok(_) => Some(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    };
    for m in mounts(&info) {
        // (a device's number is looked at only where devices are: a stat of any mount point could
        // wait on a FUSE or network folder bound in)
        if !m.at.starts_with(b"/dev/") && m.fstype != b"devtmpfs" {
            if let Some(why) = crate::bwrap::seat_path(&m.at, b"") {
                return Some(cat!(why, " at ", m.at));
            }
            continue;
        }
        let meta = std::fs::symlink_metadata(shell::path(&cat!(root, m.at)));
        if let Some(why) = meta.as_ref().ok().and_then(seat_device) {
            return Some(cat!(why, " at ", m.at));
        }
        if let Some(why) = crate::bwrap::seat_path(&m.at, b"") {
            if m.at.starts_with(b"/dev/dri/card") && meta.as_ref().is_ok_and(placeholder) {
                continue;
            }
            return Some(cat!(why, " at ", m.at));
        }
    }
    if let Some(why) = dri_seat(&cat!(root, "/dev/dri")) {
        return Some(why);
    }
    for p in ["/dev/input", "/run/seatd.sock", "/run/dbus"] {
        match there(p.as_bytes()) {
            Some(false) => {}
            Some(true) => return Some(cat!(p, " in it")),
            None => return Some(cat!("a root that cannot be read (", root, p, ")")),
        }
    }
    if !runtime.is_empty() {
        use std::os::unix::fs::MetadataExt;
        let host = std::fs::metadata(shell::path(runtime)).map(|m| (m.dev(), m.ino()));
        let inbox =
            std::fs::metadata(shell::path(&cat!(root, runtime))).map(|m| (m.dev(), m.ino()));
        match (host, inbox) {
            (Ok(a), Ok(b)) if a == b => {
                return Some(cat!("your runtime dir (", runtime, ") in it"));
            }
            (Ok(_), Err(e)) if e.kind() != std::io::ErrorKind::NotFound => {
                return Some(cat!("a root that cannot be read (", root, runtime, ")"));
            }
            _ => {}
        }
    }
    None
}

/// Whether M is the box's card placeholder: an empty regular file (finding 294).
fn placeholder(m: &std::fs::Metadata) -> bool {
    m.file_type().is_file() && m.len() == 0
}

/// Why a device M is the seat's by its number, or None.
fn seat_device(m: &std::fs::Metadata) -> Option<&'static str> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    if m.file_type().is_char_device() {
        seat_rdev(m.rdev())
    } else {
        None
    }
}

/// Why a character device numbered RDEV is the seat's: a DRM card (major 226 below minor 128; the
/// render nodes are from 128), an input device (major 13).
fn seat_rdev(rdev: u64) -> Option<&'static str> {
    match (libc::major(rdev), libc::minor(rdev)) {
        (226, n) if n < 128 => Some("a DRM card node (the seat's display)"),
        (13, _) => Some("an input device"),
        _ => None,
    }
}

/// What of the seat the box's /dev/dri (DRI, through its root) holds, however it got there (a
/// /dev/dri folder bound whole brings every card), or None: each card there must be the box's empty
/// placeholder file, and nothing there a card or an input device by its number. A folder that cannot
/// be read is no pass; none at all is (a box with no GPU node).
fn dri_seat(dri: &[u8]) -> Option<Vec<u8>> {
    let unread = || Some(cat!("a root that cannot be read (", dri, ")"));
    let rd = match std::fs::read_dir(shell::path(dri)) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => return unread(),
    };
    for e in rd {
        let Ok(e) = e else {
            return unread();
        };
        let at = cat!(
            "/dev/dri/",
            std::os::unix::ffi::OsStrExt::as_bytes(e.file_name().as_os_str())
        );
        let meta = std::fs::symlink_metadata(e.path());
        if let Some(why) = meta.as_ref().ok().and_then(seat_device) {
            return Some(cat!(why, " at ", at));
        }
        if let Some(why) = crate::bwrap::seat_path(&at, b"")
            && !meta.as_ref().is_ok_and(placeholder)
        {
            return Some(cat!(why, " at ", at));
        }
    }
    None
}

/// One line of a mountinfo: the source's device (major:minor), the folder of that filesystem it
/// mounts (its root), the mount point (octal escapes undone) and the filesystem's type.
#[derive(Debug, PartialEq, Eq)]
struct Mount {
    dev: Vec<u8>,
    root: Vec<u8>,
    at: Vec<u8>,
    fstype: Vec<u8>,
}

/// The mounts of a mountinfo (its third, fourth and fifth fields, and the one after its `-`).
fn mounts(info: &[u8]) -> Vec<Mount> {
    let mut out = Vec::new();
    for l in info.split(|&c| c == b'\n') {
        let f: Vec<&[u8]> = l.split(|&c| c == b' ').collect();
        if f.len() < 5 {
            continue;
        }
        // (the optional fields run from the seventh to the `-`)
        let fstype = f
            .iter()
            .skip(6)
            .position(|x| *x == b"-")
            .and_then(|i| f.get(6 + i + 1))
            .map(|t| unescape(t))
            .unwrap_or_default();
        out.push(Mount {
            dev: f[2].to_vec(),
            root: unescape(f[3]),
            at: unescape(f[4]),
            fstype,
        });
    }
    out
}

/// A mountinfo field with its octal escapes (`\040` a space...) undone.
fn unescape(f: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(f.len());
    let mut i = 0;
    while i < f.len() {
        if f[i] == b'\\'
            && i + 3 < f.len()
            && f[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            let n = (f[i + 1] - b'0') * 64 + (f[i + 2] - b'0') * 8 + (f[i + 3] - b'0');
            v.push(n);
            i += 4;
        } else {
            v.push(f[i]);
            i += 1;
        }
    }
    v
}

/// Where in omabox's own view (HOST, its mountinfo) the source of M is: for each mount of the same
/// device whose root holds M's, that mount point and the rest of M's root (a filesystem mounted
/// twice gives both). None when no mount of HOST shows it (a source omabox cannot see).
fn sources(m: &Mount, host: &[Mount]) -> Vec<Vec<u8>> {
    if !m.root.starts_with(b"/") {
        return Vec::new();
    }
    let mut out = Vec::new();
    for h in host
        .iter()
        .filter(|h| h.dev == m.dev && h.root.starts_with(b"/"))
    {
        let rest = if h.root == b"/" {
            Some(m.root.as_slice())
        } else if m.root == h.root {
            Some(b"".as_slice())
        } else {
            m.root
                .strip_prefix(h.root.as_slice())
                .filter(|r| r.starts_with(b"/"))
        };
        if let Some(rest) = rest {
            out.push(crate::bwrap::lexical(&cat!(h.at, "/", rest)));
        }
    }
    out
}

/// The seat a box's mounts (BOX, its mountinfo) bring in, or None: a mount point under the user's
/// runtime dir (RUNTIME; the box's own run dir is bound at its path, nothing in it), or a mount whose
/// source, named through omabox's own mounts (HOST), is one of the seat's paths or holds one, at
/// whatever path the box has it (a source in the box dir OWN is the box's own: its run dir, its
/// placeholder files).
fn seat_mounts(boxm: &[u8], host: &[u8], runtime: &[u8], own: &[u8]) -> Option<Vec<u8>> {
    let host = mounts(host);
    let rt = crate::bwrap::lexical(runtime);
    let own = crate::bwrap::lexical(own);
    for m in mounts(boxm) {
        let at = crate::bwrap::lexical(&m.at);
        if !runtime.is_empty() && at.starts_with(&cat!(rt, "/")) {
            return Some(cat!("a mount under the runtime dir's path at ", m.at));
        }
        for src in sources(&m, &host) {
            if src == own || src.starts_with(&cat!(own, "/")) {
                continue;
            }
            let why = crate::bwrap::seat_path(&src, runtime)
                .or_else(|| crate::bwrap::holds_seat(&src, runtime));
            if let Some(why) = why {
                return Some(cat!(src, " at ", m.at, " (", why, ")"));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // #364: pasta's shell's own line, with the first thing said before it (what failed), is what up
    // says at once; a box.log without it (a start under way) is nothing yet.
    #[test]
    fn net_failed_says_what_failed() {
        let log = cat!(
            "\nError: Could not process rule: No such file or directory\nadd table inet omabox\n^^^^^\n",
            NFT_FAILED,
            "\n"
        );
        assert_eq!(
            String::from_utf8_lossy(&net_failed(&log).unwrap_or_default()),
            "could not set the protected ports rule in the box network (Error: Could not process rule: No such file or directory)"
        );
        assert_eq!(
            net_failed(&cat!("sh: line 1: ip: command not found\n", ADDR_FAILED)),
            Some(
                b"could not add the --addr addresses to the box loopback (sh: line 1: ip: command not found)"
                    .to_vec()
            )
        );
        assert_eq!(
            net_failed(NFT_FAILED.as_bytes()),
            Some(b"could not set the protected ports rule in the box network".to_vec())
        );
        assert_eq!(net_failed(b""), None);
        assert_eq!(net_failed(b"bwrap: Can't mount proc\n"), None);
        // (a line that only starts like it, from anything else, is not it)
        assert_eq!(net_failed(&cat!("x ", NFT_FAILED, "\n")), None);
    }

    #[test]
    fn mountinfo() {
        let m = b"22 1 0:20 / / rw - tmpfs t rw\n23 22 0:21 /dri/renderD128 /dev/dri/renderD128 rw shared:3 master:1 - devtmpfs d rw\n24 22 0:22 /x\\040y /a\\040b rw - x\\040t y rw\n25 22 0:5 net:[4026531833] /n rw - nsfs nsfs rw\n";
        let at: Vec<_> = mounts(m).into_iter().map(|m| m.at).collect();
        assert_eq!(
            at,
            vec![
                b"/".to_vec(),
                b"/dev/dri/renderD128".to_vec(),
                b"/a b".to_vec(),
                b"/n".to_vec()
            ]
        );
        assert_eq!(
            mounts(m)[1],
            Mount {
                dev: b"0:21".to_vec(),
                root: b"/dri/renderD128".to_vec(),
                at: b"/dev/dri/renderD128".to_vec(),
                fstype: b"devtmpfs".to_vec(),
            }
        );
        assert_eq!(mounts(m)[2].root, b"/x y");
        assert_eq!(mounts(m)[2].fstype, b"x t");
    }

    // #363's fixtures: a host's own mounts (a btrfs root and home, devtmpfs, /run, the runtime dir's
    // own tmpfs, sysfs, a second mount of /run's tmpfs) and the mounts of a box `b1` as up makes it.
    const HOST: &str = "\
1 0 0:30 /@ / rw - btrfs /dev/x rw
2 1 0:7 / /dev rw - devtmpfs devtmpfs rw
3 1 0:28 / /run rw - tmpfs tmpfs rw
4 3 0:28 /credentials/@encrypted /run/credentials/@encrypted rw - tmpfs tmpfs rw
5 3 0:84 / /run/user/1000 rw - tmpfs tmpfs rw
6 1 0:30 /@home /home rw - btrfs /dev/x rw
7 1 0:25 / /sys rw - sysfs sysfs rw
8 3 0:5 net:[4026533623] /run/docker/netns/a rw - nsfs nsfs rw
";
    const BOX: &str = "\
100 99 0:200 / / rw - tmpfs tmpfs rw
101 100 0:30 /@/usr /usr ro - btrfs /dev/x rw
102 100 0:201 / /dev rw - tmpfs tmpfs rw
103 102 0:7 /null /dev/null rw - devtmpfs devtmpfs rw
104 102 0:7 /dri/renderD128 /dev/dri/renderD128 rw - devtmpfs devtmpfs rw
105 102 0:7 /nvidia0 /dev/nvidia0 rw - devtmpfs devtmpfs rw
106 100 0:84 /omabox/b1/run /run/user/1000 rw - tmpfs tmpfs rw
107 102 0:84 /omabox/b1/card-placeholder /dev/dri/card1 ro - tmpfs tmpfs rw
108 100 0:28 /systemd/resolve/stub-resolv.conf /run/systemd/resolve/stub-resolv.conf ro - tmpfs tmpfs rw
109 100 0:30 /@home/chaves/.cache/omabox/b1/home /home/sbx rw - btrfs /dev/x rw
110 100 0:25 / /sys ro - sysfs sysfs rw
111 100 0:202 / /run/systemd/journal rw - tmpfs tmpfs rw
112 100 0:84 /omabox/b1/cover-dir /home/chaves/p/secret ro - tmpfs tmpfs rw
113 100 0:5 net:[4026533623] /n rw - nsfs nsfs rw
";
    const RT: &[u8] = b"/run/user/1000";
    const OWN: &[u8] = b"/run/user/1000/omabox/b1";

    fn seat_with(line: &str) -> Option<String> {
        let boxm = format!("{BOX}{line}\n");
        seat_mounts(boxm.as_bytes(), HOST.as_bytes(), RT, OWN)
            .map(|w| String::from_utf8_lossy(&w).into_owned())
    }

    #[test]
    fn seat_mounts_a_box_as_up_makes_it_passes() {
        assert_eq!(seat_with(""), None);
        // (a source no mount of omabox's shows, a root that is no path: nothing to name)
        assert_eq!(
            seat_with("120 100 0:999 /input /opt/x ro - tmpfs t rw"),
            None
        );
        assert_eq!(seat_with("120 100 0:5 net:[1] /opt/n ro - nsfs n rw"), None);
        // (the root of a second mount of /run's tmpfs: what it is in /run)
        assert_eq!(
            seat_with("120 100 0:28 /credentials/@encrypted/k /opt/k ro - tmpfs t rw"),
            None
        );
    }

    // #363: a /dev/dri folder bound whole, a seat path bound at another destination (named by its
    // source device and root through omabox's own mounts), a mount in the runtime dir's place.
    #[test]
    fn seat_mounts_finds_the_seat_by_source_and_place() {
        for (line, want) in [
            (
                "120 102 0:7 /dri /dev/dri rw - devtmpfs d rw",
                "/dev/dri at /dev/dri (it holds the DRM card nodes)",
            ),
            (
                "120 100 0:7 /dri /opt/gpu rw - devtmpfs d rw",
                "/dev/dri at /opt/gpu (it holds the DRM card nodes)",
            ),
            (
                "120 100 0:7 /dri/card0 /home/sbx/c rw - devtmpfs d rw",
                "/dev/dri/card0 at /home/sbx/c (a DRM card node (the seat's display))",
            ),
            (
                "120 100 0:7 /input /opt/x ro - devtmpfs d rw",
                "/dev/input at /opt/x (the input devices)",
            ),
            (
                "120 100 0:7 /input/event3 /tmp/e ro - devtmpfs d rw",
                "/dev/input/event3 at /tmp/e (the input devices)",
            ),
            (
                "120 100 0:7 / /mnt/d ro - devtmpfs d rw",
                "/dev at /mnt/d (it holds the DRM card nodes)",
            ),
            (
                "120 100 0:28 /seatd.sock /tmp/s ro - tmpfs t rw",
                "/run/seatd.sock at /tmp/s (seatd's socket)",
            ),
            (
                "120 100 0:28 /dbus /opt/bus ro - tmpfs t rw",
                "/run/dbus at /opt/bus (the system bus)",
            ),
            (
                "120 100 0:28 /user /opt/u ro - tmpfs t rw",
                "/run/user at /opt/u (it holds your runtime dir)",
            ),
            (
                "120 100 0:84 / /mnt/rt rw - tmpfs t rw",
                "/run/user/1000 at /mnt/rt (your runtime dir)",
            ),
            (
                "120 100 0:84 /wayland-1 /home/sbx/w rw - tmpfs t rw",
                "/run/user/1000/wayland-1 at /home/sbx/w (your runtime dir)",
            ),
            // (another box's dir, and one whose name only starts like this box's)
            (
                "120 100 0:84 /omabox/b2/run /opt/r rw - tmpfs t rw",
                "/run/user/1000/omabox/b2/run at /opt/r (your runtime dir)",
            ),
            (
                "120 100 0:84 /omabox/b10 /opt/r rw - tmpfs t rw",
                "/run/user/1000/omabox/b10 at /opt/r (your runtime dir)",
            ),
            (
                "120 106 0:203 / /run/user/1000/wayland-1 rw - tmpfs t rw",
                "a mount under the runtime dir's path at /run/user/1000/wayland-1",
            ),
            (
                "120 106 0:30 /@home/chaves/x /run/user/1000/bus rw - btrfs /dev/x rw",
                "a mount under the runtime dir's path at /run/user/1000/bus",
            ),
        ] {
            assert_eq!(seat_with(line).as_deref(), Some(want), "{line}");
        }
    }

    // #363: with no runtime dir known, the rest still holds (nothing is the runtime dir's).
    #[test]
    fn seat_mounts_without_a_runtime_dir() {
        let boxm = format!("{BOX}120 100 0:7 /input /opt/x ro - devtmpfs d rw\n");
        assert!(seat_mounts(boxm.as_bytes(), HOST.as_bytes(), b"", OWN).is_some());
        let boxm = format!("{BOX}120 100 0:84 /wayland-1 /run/user/1000/w ro - tmpfs t rw\n");
        assert_eq!(
            seat_mounts(boxm.as_bytes(), HOST.as_bytes(), b"", OWN),
            None
        );
    }

    #[test]
    fn seat_devices_by_number() {
        for (maj, min, seat) in [
            (226, 0, true),
            (226, 1, true),
            (226, 127, true),
            (226, 128, false),
            (226, 129, false),
            (13, 64, true),
            (13, 63, true),
            (1, 3, false),
            (195, 0, false),
            (195, 255, false),
        ] {
            assert_eq!(
                seat_rdev(libc::makedev(maj, min)).is_some(),
                seat,
                "{maj}:{min}"
            );
        }
    }

    // #363: a card in the box's /dev/dri is the empty placeholder file or the box is ended; a
    // folder bound whole brings its cards in, which a look at the mount points alone would miss.
    #[test]
    fn dri_cards_must_be_placeholders() {
        let d = std::env::temp_dir().join(format!("omabox-dri-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = |n: &str| d.join(n);
        let bytes = std::os::unix::ffi::OsStrExt::as_bytes(d.as_os_str()).to_vec();
        assert_eq!(dri_seat(&cat!(bytes, "/none")), None);
        std::fs::write(p("renderD128"), b"x").unwrap();
        std::fs::write(p("card1"), b"").unwrap();
        std::fs::create_dir(p("by-path")).unwrap();
        assert_eq!(dri_seat(&bytes), None);
        std::fs::write(p("card0"), b"x").unwrap();
        assert_eq!(
            dri_seat(&bytes),
            Some(b"a DRM card node (the seat's display) at /dev/dri/card0".to_vec())
        );
        std::fs::remove_file(p("card0")).unwrap();
        std::fs::create_dir(p("card2")).unwrap();
        assert!(dri_seat(&bytes).is_some(), "a folder at a card's name");
        std::fs::remove_dir(p("card2")).unwrap();
        std::os::unix::fs::symlink("card1", p("card3")).unwrap();
        assert!(dri_seat(&bytes).is_some(), "a link at a card's name");
        std::fs::remove_file(p("card3")).unwrap();
        assert_eq!(dri_seat(&bytes), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    fn seg() -> impl proptest::strategy::Strategy<Value = Vec<u8>> {
        use proptest::prelude::*;
        let parts = prop::sample::select(vec![
            "dev",
            "dri",
            "card0",
            "renderD128",
            "input",
            "event1",
            "run",
            "dbus",
            "seatd.sock",
            "user",
            "1000",
            "..",
            ".",
            "usr",
            "home",
            "sbx",
            "tmp",
            "nvidia0",
            "opt",
            "omabox",
            "srv",
        ]);
        prop::collection::vec(parts, 1..5).prop_map(|v| format!("/{}", v.join("/")).into_bytes())
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(48))]
        // S1 (#355): whatever combination of up's options (its mounts, overlays, plugins, desk fonts,
        // net mode, --systemd, --interactive, NVIDIA's nodes and card placeholders) the bwrap words
        // come out with no seat path as a bind's source or destination, or up is refused.
        #[test]
        fn up_options_never_bind_the_seat(
            interactive: bool, systemd: bool, clean: bool, net in 0u8..3,
            overlays in proptest::collection::vec(seg(), 0..3),
            ro in proptest::collection::vec((seg(), seg()), 0..4),
            plugins in proptest::collection::vec(seg(), 0..3),
            fonts in proptest::collection::vec((seg(), seg()), 0..2),
            cards in proptest::collection::vec(0u8..4, 0..3),
        ) {
            let rt = shell::env_or_empty("XDG_RUNTIME_DIR");
            let base = std::env::temp_dir().join(format!("omabox-up-prop-{}", std::process::id()));
            let d = cat!(shell::bytes(&base), "/box");
            let _ = std::fs::create_dir_all(shell::path(&cat!(d, "/run")));
            let mut ctx = Ctx::new(b"up", shell::bytes(&base).to_vec());
            ctx.name = b"box".to_vec();
            ctx.d = d.clone();
            let u = Up {
                interactive,
                systemd,
                clean,
                net: [b"connected".to_vec(), b"isolated".to_vec(), b"none".to_vec()][usize::from(net)].clone(),
                size: b"1920x1080@60".to_vec(),
                scale: b"1".to_vec(),
                overlays,
                ro_args: ro,
                plugins: plugins
                    .into_iter()
                    .enumerate()
                    .map(|(i, dir)| crate::plugins::Plugin { id: format!("p{i}").into_bytes(), dir })
                    .collect(),
                ..Up::default()
            };
            let render = RenderNode::new(b"/dev/dri/renderD128").ok_or(proptest::test_runner::TestCaseError::fail("node"))?;
            let devs: Vec<NvidiaDev> = NvidiaDev::new(b"/dev/nvidia0").into_iter().collect();
            let cards: Vec<Vec<u8>> = cards.iter().map(|n| format!("card{n}").into_bytes()).collect();
            let fonts: Vec<(Vec<u8>, Vec<u8>)> = fonts;
            let etc = Etc { overlay: false, files: Vec::new() };
            let r = crate::exit::muted(|| {
                bwrap_args(&ctx, &u, &render, std::slice::from_ref(&render), &devs, &cards, false, &etc, &fonts)
            });
            if let Ok(args) = r {
                let mut i = 0;
                while i < args.len() {
                    let w = args[i].as_slice();
                    if matches!(w, b"--bind" | b"--ro-bind" | b"--dev-bind") {
                        let (src, dest) = (&args[i + 1], &args[i + 2]);
                        let own = crate::bwrap::lexical(src).starts_with(&cat!(d, "/"));
                        let placeholder = own && dest.starts_with(b"/dev/dri/card");
                        proptest::prop_assert!(own || src == b"@CGROUP@" || crate::bwrap::seat_path(src, &rt).is_none(), "source {:?}", String::from_utf8_lossy(src));
                        proptest::prop_assert!(placeholder || crate::bwrap::seat_path(dest, b"").is_none(), "dest {:?}", String::from_utf8_lossy(dest));
                        i += 3;
                    } else {
                        i += 1;
                    }
                }
            }
            let _ = std::fs::remove_dir_all(&base);
        }
    }

    #[test]
    fn locale_order() {
        let v = host_locale_vars();
        let names: Vec<&Vec<u8>> = v.iter().map(|(k, _)| k).collect();
        let mut sorted = names.clone();
        sorted.sort();
        if names.first().is_some_and(|n| n.as_slice() == b"LANGUAGE") {
            assert_eq!(&names[1..], &sorted[..sorted.len() - 1]);
        }
    }
}
