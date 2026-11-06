//! `omabox restart-shell [-b NAME]`: the box's shell again (Omarchy's, after plugin edits, or a
//! plain box's `up --shell CMD`, #247). share/shell.sh
//! keeps the shell's pid in the box's omabox-shell.pid: a pid there once shell.sh wrote back this
//! restart's token is the restart (a project's own quickshell in the box is not the Omarchy shell,
//! finding 63). Not any new pid: while shell.sh stops the old shell, an old launcher's relaunch writes
//! one too, and that shell is stopped with the rest (finding 248).

use crate::args::name_arg;
use crate::boxes::{BoxPid, box_pid, covers_held, meta, need_box, run_read};
use crate::cat;
use crate::exec::{ErrTo, Out, on_box};
use crate::exit::{Ctx, Exit, Res, say};
use crate::ready::{Ready, shell_watch, wait_ready};
use crate::select::select_box;
use crate::shell;
use crate::shellnote::{shell_crash_dirs, shell_hung, shell_live_pid, shell_seen};

/// What kills a hung shell, in the box's pid namespace: its launcher first (it relaunches one killed
/// by a signal; stopped, it ends at its next turn), then the shell.
const KILL_SH: &str = r#"pp=$(sed -n "s/^PPid:[[:space:]]*//p" "/proc/$1/status")
      [ -z "$pp" ] || ! grep -qa omarchy-launch-shell "/proc/$pp/cmdline" || kill "$pp"; kill -KILL "$1""#;

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let name = name_arg(ctx, "restart-shell", args)?;
    select_box(ctx, &name)?;
    need_box(ctx)?;
    // A plain box (#247) has a shell only when `up --shell CMD` named one: nothing to start again.
    if meta(&ctx.d, "shell").0 != b"true" && meta(&ctx.d, "shell_cmd").0.is_empty() {
        crate::omshell::plain_refuse(
            ctx,
            "restart-shell",
            "no shell to restart (up --desktop plain --shell CMD starts one)",
        )?;
    }
    let n = ctx.name.clone();
    let old = run_read(&ctx.d, "omabox-shell.pid")
        .map(|v| shell::strip_nl(&v))
        .unwrap_or_default();
    // A hung shell (#209) is killed here, as a dead one would be gone: shell.sh asks it to quit first
    // and would wait 5 s on it, then 5 s more. (Its process that runs: a plain box's launcher may
    // have left, its shell running on, #438.)
    if let Some(live) = shell_live_pid(ctx).filter(|p| shell_hung(ctx, p)) {
        // In the box's pid namespace, never on_box's (user and mount only): there /proc is the box's,
        // but a kill takes the pid as the host's, and would signal whatever host process has that
        // number.
        let bp = match box_pid(&ctx.d) {
            BoxPid::Pid(p) => p,
            _ => Vec::new(),
        };
        covers_held(ctx)?;
        // (entered natively, finding 490; the box gone meanwhile: nothing to kill)
        let entry = crate::enter::Entry::open(&ctx.d, &bp, crate::enter::PIDS).ok();
        if let Some(e) = entry {
            let argv: Vec<Vec<u8>> = [
                b"/usr/bin/sh".as_slice(),
                b"-c",
                KILL_SH.as_bytes(),
                b"sh",
                &live,
            ]
            .iter()
            .map(|w| w.to_vec())
            .collect();
            let _ = crate::exec::stdio_relay(&argv, false, true, Some(e.plan(true, false, None)));
        }
        say(cat!(
            "box '",
            n,
            "': its shell (pid ",
            live,
            ") did not answer (hung): killed"
        ));
    }
    let before = shell_crash_dirs(ctx);
    // (A box whose shell.sh writes no token, one `up` by an omabox from before: a new pid, as then.)
    let has_token = on_box(
        ctx,
        &words(&[
            b"grep",
            b"-q",
            b"omabox-shell.start",
            b"/opt/omabox/share/shell.sh",
        ]),
        false,
        Out::Inherit,
        ErrTo::Null,
    )
    .or_exit()?
    .rc == 0;
    let tok = if has_token { token() } else { Vec::new() };
    let r = on_box(
        ctx,
        &words(&[
            b"hyprctl",
            b"dispatch",
            &cat!("hl.dsp.exec_cmd('/opt/omabox/share/shell.sh ", tok, "')"),
        ]),
        false,
        Out::Null,
        ErrTo::Inherit,
    )
    .or_exit()?;
    if r.rc != 0 {
        // (bash's set -e: the dispatch's own status, its words said by hyprctl)
        return Err(Exit(u8::try_from(r.rc).unwrap_or(1)));
    }
    // (finding 450: a deadline on the monotonic clock)
    let deadline = crate::settle::mono_ms().saturating_add(15_000);
    loop {
        // (The token read first: shell.sh writes it after removing the old pid, so a pid read after
        // it is new.)
        let read = |f: &str| run_read(&ctx.d, f).map(|v| shell::strip_nl(&v));
        let done = if tok.is_empty() {
            read("omabox-shell.pid").is_some_and(|p| !p.is_empty() && p != old)
        } else {
            read("omabox-shell.start").is_some_and(|t| t == tok)
                && read("omabox-shell.pid").is_some_and(|p| !p.is_empty())
        };
        if done {
            break;
        }
        if crate::settle::mono_ms() >= deadline {
            return Err(ctx.die(cat!(
                ctx.label(),
                "the shell did not restart in 15 s: why is in its log (omabox log -b ",
                n,
                " shell)"
            )));
        }
        crate::sig::sleep(std::time::Duration::from_millis(100));
    }
    let mut r = Ready {
        owner: None,
        covers: false,
        shell_before: before,
        bar_at: 0,
        before_end: &|| {},
    };
    wait_ready(ctx, &mut r, true)?;
    // A shell that crashed just after its bar came is no restart (#126): said with its report, exit 1.
    let crashed = shell_watch(ctx, &r);
    shell_seen(ctx); // said here, not again (shell_note, #147)
    if let Some(c) = crashed {
        say(c);
        return Err(Exit(1));
    }
    say(cat!("shell restarted in box '", n, "'"));
    crate::plugins::plugin_check(ctx, &crate::install::git_rev)?;
    // An Omarchy tree's commit as it is now, as plugin_check does for each plugin's: it was edited
    // (or committed) since `up`, and `ls --json` says what the box tested.
    let tree = meta(&ctx.d, "omarchy").0;
    if !tree.is_empty() {
        let v = crate::install::git_rev(&tree);
        let v = if v.is_empty() {
            b"dev".to_vec()
        } else {
            cat!("dev ", v)
        };
        if !crate::mon::jq_box_json(
            ctx,
            crate::jq::Args::new(&[])
                .kv("--arg", "v", &v)
                .arg(".omarchy_version = $v"),
        ) {
            return Err(Exit(1));
        }
    }
    Ok(())
}

fn words(w: &[&[u8]]) -> Vec<Vec<u8>> {
    w.iter().map(|x| x.to_vec()).collect()
}

/// `$$.$RANDOM`: this process's pid and a number of 0-32767, the restart's token.
fn token() -> Vec<u8> {
    let mut b = [0u8; 2];
    // SAFETY: b is ours, its length given.
    let got = unsafe { libc::getrandom(b.as_mut_ptr().cast(), b.len(), 0) };
    if got != 2 {
        b = u16::try_from(crate::settle::mono_ms() & 0xffff)
            .unwrap_or(0)
            .to_le_bytes();
    }
    let r = u16::from_le_bytes(b) & 0x7fff;
    format!("{}.{r}", std::process::id()).into_bytes()
}
