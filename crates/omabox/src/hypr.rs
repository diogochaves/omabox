//! The box's Hyprland when a call to it gets no answer: gone (how it ended, #232), held by gdb at a
//! crash, or hung (#124, findings 181-182, 350, 353), in words; and what a reload's config errors are
//! (#246). As bin/omabox's hung_text, hypr_lost, hypr_down_text, hypr_answers, hypr_json and
//! hypr_errors_note.

use crate::boxes::{BoxSys, RealBox, box_env, box_home_file, hypr_end, run_read, tail};
use crate::cat;
use crate::exec::{ErrTo, Out};
use crate::exit::{Ctx, Res, say};
use crate::json;
use crate::procfs;
use crate::shell;
use std::time::Duration;

/// HYPR_LIMIT: how long a hyprctl call is given.
pub const HYPR_LIMIT: &str = "10";

fn words(v: &[&str]) -> Vec<Vec<u8>> {
    v.iter().map(|w| w.as_bytes().to_vec()).collect()
}

/// The host pid of the box's Hyprland: Some(None) when none runs, None when it cannot tell. Its pid
/// as the box numbers it, from its instance lock (a box-written file: only a number taken), matched
/// to the host's through NSpid.
fn hypr_host_pid<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx) -> Option<Option<Vec<u8>>> {
    let pids = sys.box_pids(ctx);
    if pids.is_empty() {
        return None;
    }
    let is = box_env(&ctx.d, "HYPRLAND_INSTANCE_SIGNATURE")?;
    let is_s = String::from_utf8_lossy(&is).into_owned();
    let link = |rel: &str| ctx.at(rel).is_symlink();
    let lock = if link("run/hypr") || link(&format!("run/hypr/{is_s}")) {
        Vec::new()
    } else {
        run_read(&ctx.d, &format!("hypr/{is_s}/hyprland.lock"))
            .map(|v| v.split(|&c| c == b'\n').next().unwrap_or_default().to_vec())
            .unwrap_or_default()
    };
    if shell::re_match("^[1-9][0-9]{0,6}$", &lock) {
        for p in pids {
            let s = std::fs::read(shell::path(&cat!("/proc/", p, "/status"))).unwrap_or_default();
            if procfs::status_last(&s, "NSpid") == lock && procfs::running(&p) {
                return Some(Some(p));
            }
        }
    }
    Some(None)
}

/// hypr_gone: for `ls`, at one look (no wait): a box up whose Hyprland did not answer because it is
/// gone, in the moment before the box ends with it (finding 350): its end's first word (crashed,
/// exited) when labwc (or an interactive box's session.sh) said how, else going. None for one that
/// runs, or a box it cannot tell of.
pub fn hypr_gone(ctx: &Ctx) -> Option<String> {
    hypr_gone_with(&RealBox, ctx)
}

fn hypr_gone_with<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx) -> Option<String> {
    if hypr_host_pid(sys, ctx)?.is_some() {
        return None;
    }
    Some(match hypr_end(&ctx.d) {
        Some(e) => e.split(' ').next().unwrap_or_default().to_string(),
        None => "going".to_string(),
    })
}

/// A process's state as /proc/PID/status says it ("S (sleeping)", "t (tracing stop)").
pub fn pid_state(pid: &[u8]) -> Vec<u8> {
    let s = std::fs::read(shell::path(&cat!("/proc/", pid, "/status"))).unwrap_or_default();
    let mut out = Vec::new();
    for l in s.split(|&c| c == b'\n') {
        if let Some(r) = l.strip_prefix(b"State:") {
            let i = r
                .iter()
                .position(|c| !c.is_ascii_whitespace())
                .unwrap_or(r.len());
            out.extend_from_slice(&r[i..]);
            out.push(b'\n');
        }
    }
    shell::strip_nl(&out)
}

/// The fatal signal gdb --watch caught in the box's Hyprland, in its log's last watch of it.
pub fn hypr_gdb_sig(ctx: &Ctx) -> Option<String> {
    let f = box_home_file(&ctx.d, "gdb.log")?;
    let text = tail(&f, 1_048_576);
    let (mut w, mut s) = (false, String::new());
    for l in text.split(|&c| c == b'\n') {
        if l.starts_with(b"== omabox gdb --watch ") {
            w = l.starts_with(b"== omabox gdb --watch Hyprland ");
            s.clear();
            continue;
        }
        if w && shell::re_match(
            r"^(Thread [0-9]+ .*|Program) received signal SIG[A-Z0-9]+",
            l,
        ) {
            if let Some(m) = shell::re_group("(SIG[A-Z0-9]+)", l, 1) {
                s = String::from_utf8_lossy(&m).into_owned();
            }
        }
    }
    (!s.is_empty()).then_some(s)
}

/// A box that went down, or is going, in words.
pub fn hypr_down_text(ctx: &Ctx) -> Vec<u8> {
    let n = &ctx.name;
    let Some(end) = hypr_end(&ctx.d) else {
        return cat!(
            "box '",
            n,
            "' went down (why: omabox log -b ",
            n,
            " all; clear it with: omabox down ",
            n,
            ")"
        );
    };
    let hint = if end.starts_with("crashed") {
        if hypr_gdb_sig(ctx).is_some() {
            cat!("its backtrace: omabox log -b ", n, " gdb --crash")
        } else {
            b"a backtrace: the same call in a new box after omabox gdb --watch; boxes keep no core dumps".to_vec()
        }
    } else {
        cat!("why: omabox log -b ", n, " all")
    };
    cat!(
        "box '",
        n,
        "' went down: its Hyprland ",
        end,
        " (",
        hint,
        "; clear it with: omabox down ",
        n,
        ")"
    )
}

/// Whether the box's Hyprland is gone, or held by gdb --watch at a fatal signal, after a call to it
/// got no answer (#232): up to 2 s for the box or labwc's line to say how it ended; one that runs
/// (twice, 0.2 s apart) is not lost. Some(what happened), None for one that runs or cannot be told.
pub fn hypr_lost(ctx: &Ctx) -> Res<Option<Vec<u8>>> {
    hypr_lost_with(&RealBox, ctx)
}

fn hypr_lost_with<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx) -> Res<Option<Vec<u8>>> {
    let (mut run, mut hp) = (0, None);
    for _ in 0..10 {
        if !sys.box_alive(ctx)? {
            return Ok(Some(hypr_down_text(ctx)));
        }
        let Some(h) = hypr_host_pid(sys, ctx) else {
            // The box can end between that look and this one, a moment after its Hyprland: gone,
            // not hung (#351).
            return Ok((!sys.box_alive(ctx)?).then(|| hypr_down_text(ctx)));
        };
        hp = h;
        if let Some(p) = &hp {
            let st = sys.pid_state(p);
            // gdb --watch stops it at the signal (tracing stop, t) while it writes the backtraces.
            if st.starts_with(b"t") {
                if let Some(sig) = hypr_gdb_sig(ctx) {
                    let n = &ctx.name;
                    return Ok(Some(cat!(
                        "box '",
                        n,
                        "': its Hyprland crashed (",
                        sig,
                        "), held by gdb --watch while it writes the backtrace (omabox log -b ",
                        n,
                        " gdb --crash; the box goes down after it)"
                    )));
                }
            } else {
                run += 1;
            }
            if run >= 2 {
                return Ok(None);
            }
        } else if hypr_end(&ctx.d).is_some() {
            // labwc said how it ended, and the box ends with it (session.sh's kill -1, then its
            // exit): up to 1 s for its PID 1 to go, so the box is down when this says it went down,
            // and the next call finds it dead, not half torn down (finding 527).
            for _ in 0..10 {
                if !sys.box_alive(ctx)? {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            return Ok(Some(hypr_down_text(ctx)));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    if hp.is_some() {
        return Ok(None);
    }
    Ok(Some(hypr_down_text(ctx)))
}

/// A box whose PID 1 lives but whose Hyprland does not answer, in words (IN: how long it was given);
/// one gone meanwhile said so instead.
pub fn hung_text(ctx: &Ctx, within: &str) -> Res<Vec<u8>> {
    hung_text_with(&RealBox, ctx, within)
}

pub fn hung_text_with<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx, within: &str) -> Res<Vec<u8>> {
    if let Some(t) = hypr_lost_with(sys, ctx)? {
        return Ok(t);
    }
    let n = &ctx.name;
    let within = if within.is_empty() {
        String::new()
    } else {
        format!(" in {within}")
    };
    Ok(cat!(
        "box '",
        n,
        "': its Hyprland did not answer",
        within,
        " (hung? omabox log -b ",
        n,
        "; omabox down ",
        n,
        ")"
    ))
}

/// The box's Hyprland answers over IPC within 2 s. (Never inside `$(...)`: on_box's refusal ends
/// the command, its words to /dev/null with the rest.)
pub fn hypr_answers(ctx: &Ctx) -> Res<bool> {
    hypr_answers_in(ctx, "2")
}

/// hypr_answers SECS: the same, given SECS.
pub fn hypr_answers_in(ctx: &Ctx, secs: &str) -> Res<bool> {
    hypr_answers_with(&RealBox, ctx, secs)
}

pub fn hypr_answers_with<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx, secs: &str) -> Res<bool> {
    if !sys.box_alive(ctx)? {
        return Ok(false);
    }
    let r = sys.on_box_io(
        ctx,
        &words(&["timeout", secs, "hyprctl", "version"]),
        Out::Null,
        ErrTo::Null,
    );
    Ok(r.or_exit()?.rc == 0)
}

/// `hyprctl -j ARGS` in the box, its answer checked as JSON (#125, finding 182): one that does not
/// answer is said in words and the command ends (at most 64 MiB read).
pub fn hypr_json(ctx: &Ctx, args: &[&str]) -> Res<Vec<u8>> {
    hypr_json_with(&RealBox, ctx, args)
}

fn hypr_json_with<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx, args: &[&str]) -> Res<Vec<u8>> {
    let mut argv = words(&["timeout", "6", "hyprctl", "-j"]);
    argv.extend(words(args));
    let r = sys.on_box_io(ctx, &argv, Out::Capture(Some(67_108_864)), ErrTo::Null);
    let out = if r.rc == 0 {
        shell::subst(&r.out)
    } else {
        Vec::new()
    };
    let (vals, ok) = json::values(&out);
    if ok && !vals.is_empty() {
        return Ok(cat!(out, "\n"));
    }
    if !sys.box_alive(ctx)? {
        return Err(ctx.die(hypr_down_text(ctx)));
    }
    let t = hung_text_with(sys, ctx, "")?;
    Err(ctx.die(t))
}

/// The config's errors at its last load (#246), said after `hyprctl reload`, which answers ok
/// whatever they are: control characters dropped, 20 lines and 300 characters a line at most. A
/// Hyprland that does not answer is said: false (finding 353).
pub fn hypr_errors_note(ctx: &Ctx) -> Res<bool> {
    hypr_errors_note_with(&RealBox, ctx)
}

fn hypr_errors_note_with<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx) -> Res<bool> {
    let r = sys.on_box_io(
        ctx,
        &words(&["timeout", HYPR_LIMIT, "hyprctl", "configerrors"]),
        Out::Capture(Some(65536)),
        ErrTo::Null,
    );
    let rc = r.rc;
    let t: Vec<u8> = r
        .out
        .iter()
        .copied()
        .filter(|&c| !(c <= 0x08 || (0x0b..=0x1f).contains(&c) || c == 0x7f))
        .collect();
    let t = shell::strip_nl(&t);
    let n = &ctx.name;
    if rc != 0 && rc != 141 {
        if let Some(t) = hypr_lost_with(sys, ctx)? {
            say(t);
            return Ok(false);
        }
        if hypr_answers_with(sys, ctx, "2")? {
            say(cat!(
                "box '",
                n,
                "': could not read its config's errors (hyprctl configerrors: exit ",
                rc.to_string(),
                ")"
            ));
            return Ok(true);
        }
        say(cat!(
            "box '",
            n,
            "': its Hyprland did not answer hyprctl configerrors, so the config's errors are unknown ",
            "(hung? its threads: omabox gdb -b ",
            n,
            "; its log: omabox log -b ",
            n,
            "; then omabox down ",
            n,
            ")"
        ));
        return Ok(false);
    }
    let lines: Vec<&[u8]> = t
        .split(|&c| c == b'\n')
        .filter(|l| !l.iter().all(|c| matches!(c, b' ' | b'\t')))
        .take(20)
        .collect();
    if lines.is_empty() {
        return Ok(true);
    }
    say(cat!(
        "box '",
        n,
        "': its config has errors (the rest of the config loaded):"
    ));
    for l in lines {
        say(cat!("  ", shell::head_chars(l, 300)));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cat;
    use crate::exec::Ran;
    use std::time::Instant;

    /// bash's stubs: box_alive's answer, box_pids, pid_state, and what hyprctl answers in the box
    /// (MODE: errors, none, hung, gone, fails; t_unit_hypr_errors_note's).
    struct Fake {
        alive: bool,
        pids: Vec<Vec<u8>>,
        state: &'static str,
        mode: &'static str,
    }

    impl BoxSys for Fake {
        fn box_alive(&self, _: &Ctx) -> Res<bool> {
            Ok(self.alive)
        }
        fn box_pids(&self, _: &Ctx) -> Vec<Vec<u8>> {
            self.pids.clone()
        }
        fn pid_state(&self, _: &[u8]) -> Vec<u8> {
            self.state.as_bytes().to_vec()
        }
        fn on_box_io(&self, _: &Ctx, args: &[Vec<u8>], _: Out, _: ErrTo) -> Ran {
            let ran = |rc: i32, out: &[u8]| Ran {
                rc,
                out: out.to_vec(),
                refused: false,
                gone_said: false,
            };
            if args.iter().any(|a| a == b"version") {
                return ran(if self.mode == "fails" { 0 } else { 6 }, b"");
            }
            match self.mode {
                "errors" => ran(0, b"a\x1b[31mb\n\n  c\n"),
                "none" => ran(0, b"\n"),
                "hung" | "gone" => ran(6, b"Hyprland IPC didn't respond in time\n"),
                _ => ran(3, b""),
            }
        }
    }

    /// A box dir of this test's own: its Hyprland's signature, its lock naming LOCK.
    fn hctx(tag: &str, lock: &[u8]) -> (Ctx, std::path::PathBuf) {
        let d = std::env::temp_dir().join(format!("omabox-hypr-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("run/hypr/sig")).unwrap();
        std::fs::create_dir_all(d.join("home")).unwrap();
        std::fs::write(
            d.join("run/omabox.env"),
            "HYPRLAND_INSTANCE_SIGNATURE=sig\0",
        )
        .unwrap();
        std::fs::write(d.join("run/hypr/sig/hyprland.lock"), cat!(lock, "\n")).unwrap();
        let mut c = Ctx::new(b"hyprctl", shell::bytes(&std::env::temp_dir()).to_vec());
        c.parsed();
        c.name = b"u".to_vec();
        c.d = shell::bytes(&d).to_vec();
        (c, d)
    }

    // t_unit_hypr_lost: its Hyprland, our own process (its lock our pid), stopped, traced, gone.
    #[test]
    fn hypr_lost_as_bash() {
        let me = std::process::id().to_string().into_bytes();
        // (a pid no process has: past pid_max)
        let dead = b"4194305".to_vec();
        let (ctx, d) = hctx("lost", &me);
        let up = |state| Fake {
            alive: true,
            pids: vec![me.clone()],
            state,
            mode: "",
        };
        let gone = Fake {
            alive: true,
            pids: vec![dead.clone()],
            state: "",
            mode: "",
        };
        let hl = |sys: &Fake| {
            let t0 = Instant::now();
            let t = String::from_utf8_lossy(&hung_text_with(sys, &ctx, "").unwrap()).into_owned();
            (t, t0.elapsed().as_millis())
        };
        let (t, ms) = hl(&up("T (stopped)"));
        assert_eq!(
            t, "box 'u': its Hyprland did not answer (hung? omabox log -b u; omabox down u)",
            "hypr_lost: a stopped Hyprland is hung"
        );
        assert!(ms < 2000, "...said at once ({ms} ms)");
        std::fs::write(
            d.join("home/gdb.log"),
            "\n== omabox gdb --watch Hyprland (pid 55 in the box), T\nThread 1 \"Hyprland\" received signal SIGSEGV, Segmentation fault.\n",
        )
        .unwrap();
        let (t, _) = hl(&up("t (tracing stop)"));
        assert!(
            shell::re_match(
                r"^box 'u': its Hyprland crashed \(SIGSEGV\), held by gdb --watch .*omabox log -b u gdb --crash",
                t.as_bytes()
            ),
            "...held by gdb --watch at a signal: crashed, held, and where the backtrace goes: {t}"
        );
        let (t, _) = hl(&up("S (sleeping)"));
        assert!(
            t.contains("did not answer (hung"),
            "...traced but not stopped at a signal: hung: {t}"
        );
        // hypr_gone: ls's one look (finding 350).
        assert_eq!(
            hypr_gone_with(&up("S (sleeping)"), &ctx),
            None,
            "hypr_gone: a Hyprland that runs is not gone (hung, for ls)"
        );
        assert_eq!(
            hypr_gone_with(&gone, &ctx).as_deref(),
            Some("going"),
            "...gone, no line yet: going"
        );
        std::fs::write(
            d.join("home/labwc.log"),
            "[ERROR] [../labwc/src/server.c:207] spawned child 4194305 terminated with signal 11 (Segmentation fault)\n",
        )
        .unwrap();
        assert_eq!(
            hypr_gone_with(&gone, &ctx).as_deref(),
            Some("crashed"),
            "...labwc's line: crashed"
        );
        let (t, _) = hl(&gone);
        assert!(
            t.starts_with("box 'u' went down: its Hyprland crashed (SIGSEGV) (its backtrace: omabox log -b u gdb --crash; clear it with: omabox down u)"),
            "...gone, labwc's line: went down, crashed (SIGSEGV), the gdb log's backtrace named: {t}"
        );
        std::fs::remove_file(d.join("home/gdb.log")).unwrap();
        let (t, _) = hl(&gone);
        assert!(
            t.starts_with("box 'u' went down: its Hyprland crashed (SIGSEGV) (a backtrace: ")
                && t.contains("omabox gdb --watch; boxes keep no core dumps;"),
            "...with no watch: how to get a backtrace: {t}"
        );
        std::fs::write(
            d.join("home/labwc.log"),
            "x spawned child 9 exited with 1\n",
        )
        .unwrap();
        assert_eq!(
            hypr_end(&ctx.d).as_deref(),
            Some("exited (status 1)"),
            "hypr_end: an exit's status"
        );
        assert_eq!(
            hypr_gone_with(&gone, &ctx).as_deref(),
            Some("exited"),
            "hypr_gone: an exit: exited"
        );
        std::fs::write(d.join("home/labwc.log"), "").unwrap();
        let (t, ms) = hl(&gone);
        assert_eq!(
            t, "box 'u' went down (why: omabox log -b u all; clear it with: omabox down u)",
            "...no line, gone for 2 s: went down, in the usual words"
        );
        assert!((1900..4000).contains(&ms), "...after 2 s ({ms} ms)");
        std::fs::write(
            d.join("home/hyprland.log"),
            "Hyprland says\nomabox: Hyprland terminated with signal 6\n",
        )
        .unwrap();
        assert_eq!(
            hypr_end(&ctx.d).as_deref(),
            Some("crashed (SIGABRT)"),
            "hypr_end: an interactive box's (session.sh's line in hyprland.log)"
        );
        std::fs::write(
            d.join("home/hyprland.log"),
            "omabox: Hyprland exited with 0\n",
        )
        .unwrap();
        assert_eq!(
            hypr_end(&ctx.d).as_deref(),
            Some("exited (status 0)"),
            "...an exit"
        );
        std::fs::remove_file(d.join("home/hyprland.log")).unwrap();
        std::fs::remove_file(d.join("home/labwc.log")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", d.join("home/labwc.log")).unwrap();
        assert_eq!(hypr_end(&ctx.d), None, "hypr_end: a link is not read");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A box whose Hyprland is gone and whose PID 1 lives for its first ALIVE looks (finding 527).
    struct Ending {
        alive: std::cell::Cell<u32>,
        looks: std::cell::Cell<u32>,
    }

    impl BoxSys for Ending {
        fn box_alive(&self, _: &Ctx) -> Res<bool> {
            self.looks.set(self.looks.get() + 1);
            let a = self.alive.get();
            self.alive.set(a.saturating_sub(1));
            Ok(a > 0)
        }
        fn box_pids(&self, _: &Ctx) -> Vec<Vec<u8>> {
            vec![b"4194305".to_vec()]
        }
    }

    // finding 527: labwc said how its Hyprland ended, and the box's PID 1 goes just after: hypr_lost
    // says it went down once it has (a later call finds it dead), and gives up waiting after 1 s.
    #[test]
    fn hypr_lost_waits_for_the_box_to_end() {
        let (ctx, d) = hctx("ending", b"1");
        std::fs::write(
            d.join("home/labwc.log"),
            "spawned child 4194305 terminated with signal 11 (Segmentation fault)\n",
        )
        .unwrap();
        let sys = Ending {
            alive: std::cell::Cell::new(4),
            looks: std::cell::Cell::new(0),
        };
        let t = String::from_utf8_lossy(&hung_text_with(&sys, &ctx, "").unwrap()).into_owned();
        assert!(
            t.starts_with("box 'u' went down: its Hyprland crashed (SIGSEGV)"),
            "went down, how: {t}"
        );
        assert_eq!(
            (sys.alive.get(), sys.looks.get()),
            (0, 5),
            "...said once its PID 1 had gone, not before"
        );
        let sys = Ending {
            alive: std::cell::Cell::new(u32::MAX),
            looks: std::cell::Cell::new(0),
        };
        let t0 = Instant::now();
        let t = String::from_utf8_lossy(&hung_text_with(&sys, &ctx, "").unwrap()).into_owned();
        let ms = t0.elapsed().as_millis();
        assert!(
            t.starts_with("box 'u' went down: "),
            "...one that stays: still said: {t}"
        );
        assert!((900..2000).contains(&ms), "...after 1 s ({ms} ms)");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A box that ends between hypr_lost's look at it and its look for its Hyprland (#351).
    struct Vanishing(std::cell::Cell<bool>);

    impl BoxSys for Vanishing {
        fn box_alive(&self, _: &Ctx) -> Res<bool> {
            Ok(self.0.replace(false))
        }
        fn box_pids(&self, _: &Ctx) -> Vec<Vec<u8>> {
            Vec::new()
        }
    }

    // #351: alive at the first look, gone (no pids) at the next: it went down, not hung.
    #[test]
    fn hypr_lost_box_ends_between_looks() {
        let (ctx, d) = hctx("vanishing", b"1");
        std::fs::write(
            d.join("home/labwc.log"),
            "spawned child 4194305 terminated with signal 11 (Segmentation fault)\n",
        )
        .unwrap();
        let sys = Vanishing(std::cell::Cell::new(true));
        let t = String::from_utf8_lossy(&hung_text_with(&sys, &ctx, "").unwrap()).into_owned();
        assert!(
            t.starts_with("box 'u' went down: its Hyprland crashed (SIGSEGV)"),
            "went down, how: {t}"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    // t_unit_hypr_errors_note (finding 353): hyprctl configerrors as MODE says; hypr_lost and
    // hypr_answers from the same stand-ins (a Hyprland that runs, or a box gone with labwc's line).
    #[test]
    fn hypr_errors_note_as_bash() {
        let me = std::process::id().to_string().into_bytes();
        let (ctx, d) = hctx("errnote", &me);
        std::fs::write(
            d.join("home/labwc.log"),
            "spawned child 4194305 terminated with signal 11 (Segmentation fault)\n",
        )
        .unwrap();
        let hen = |mode: &'static str| {
            let f = Fake {
                alive: mode != "gone",
                pids: vec![me.clone()],
                state: "S (sleeping)",
                mode,
            };
            let (r, said) = crate::exit::heard(|| hypr_errors_note_with(&f, &ctx));
            format!("{said}rc={}", if r.unwrap() { 0 } else { 1 })
        };
        assert_eq!(
            hen("errors"),
            "omabox: box 'u': its config has errors (the rest of the config loaded):\nomabox:   a[31mb\nomabox:     c\nrc=0",
            "hypr_errors_note: the errors, control characters dropped, blank lines out"
        );
        assert_eq!(hen("none"), "rc=0", "...none: nothing");
        let out = hen("hung");
        assert!(
            shell::re_match(
                r"^omabox: box 'u': its Hyprland did not answer hyprctl configerrors, so the config's errors are unknown \(hung\? its threads: omabox gdb -b u; .*omabox down u\)
rc=1$",
                out.as_bytes()
            ),
            "...no answer: said, with what to do, 1: {out}"
        );
        assert!(
            !out.contains("respond in time"),
            "...hyprctl's own words not taken for an error of the config"
        );
        let out = hen("gone");
        assert!(
            out.starts_with("omabox: box 'u' went down: its Hyprland crashed (SIGSEGV)")
                && out.ends_with(")\nrc=1"),
            "...gone: said as gone, 1: {out}"
        );
        assert_eq!(
            hen("fails"),
            "omabox: box 'u': could not read its config's errors (hyprctl configerrors: exit 3)\nrc=0",
            "...answers a ping, configerrors fails: said, 0"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    // t_unit_shot_hidden's: a box whose Hyprland does not answer is said, not a silent exit (shot's
    // first read, hyprctl -j monitors, through hypr_json).
    #[test]
    fn hypr_json_says_a_hung_box() {
        let (ctx, d) = hctx("json", b"1");
        let f = Fake {
            alive: true,
            pids: Vec::new(),
            state: "",
            mode: "hung",
        };
        let (r, said) = crate::exit::heard(|| hypr_json_with(&f, &ctx, &["monitors"]));
        assert!(r.is_err());
        assert!(
            said.starts_with("omabox: box 'u': its Hyprland did not answer (hung?"),
            "said, not a silent exit: {said}"
        );
        let gone = Fake { alive: false, ..f };
        let (r, said) = crate::exit::heard(|| hypr_json_with(&gone, &ctx, &["monitors"]));
        assert!(r.is_err());
        assert!(
            said.starts_with("omabox: box 'u' went down"),
            "...one gone: said as gone: {said}"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
