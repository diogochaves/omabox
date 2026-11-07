//! `omabox gdb [-b NAME] [--shell | --pid PID] [--watch] [-- GDB ARGS]`: the host's gdb attached to a
//! process in the box (Hyprland by default), in every namespace of the box, its network's too
//! (finding 229): gdb parses the box's ELF and DWARF holding CAP_SYS_PTRACE there only, so it
//! reaches nothing of the host's. Without --watch every thread's backtrace (or the given gdb
//! commands) and gdb's status; --watch leaves it attached in a session of its own in the box, a
//! fatal signal's backtraces into the box's gdb log.

use crate::args::{optarg, unexpected, unknown_opt_hint};
use crate::boxes::{BoxPid, box_home, box_pid, covers_held, need_box};
use crate::cat;
use crate::enter::Plan;
use crate::exec::{Relayed, stdio_relay};
use crate::exit::{Ctx, Res, say};
use crate::select::select_box;
use crate::shell;
use crate::shellnote::{shell_live_pid, shell_state};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Hyprland,
    Shell,
    Pid,
}

impl Target {
    fn word(self) -> &'static [u8] {
        match self {
            Target::Hyprland => b"hyprland",
            Target::Shell => b"shell",
            Target::Pid => b"pid",
        }
    }
}

fn w(v: &[&str]) -> Vec<Vec<u8>> {
    v.iter().map(|s| s.as_bytes().to_vec()).collect()
}

/// `stdio_relay fds_only "" ARGV...` as a foreground step of bash's, in the box when ENTER says so:
/// its failure to start one is bash's 1 (a fork that failed).
fn relay(argv: &[Vec<u8>], capture: bool, err_null: bool, enter: Option<Plan>) -> Relayed {
    stdio_relay(argv, capture, err_null, enter).unwrap_or(Relayed {
        rc: 1,
        out: Vec::new(),
    })
}

/// `${what//[^A-Za-z0-9 ._:+@-]/?}`: a process's name as the box set it (prctl: newlines and
/// escapes allowed), each other character a ?; a name with a byte that starts no character is
/// matched byte by byte, as bash falls back to then.
fn tame_comm(v: &[u8]) -> Vec<u8> {
    let ok = |c: u8| c.is_ascii_alphanumeric() || b" ._:+@-".contains(&c);
    shell::chars_map(v, |ch| match ch {
        [c] if ok(*c) => vec![*c],
        _ => b"?".to_vec(),
    })
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut target, mut pid, mut watch) =
        (Vec::new(), Target::Hyprland, Vec::new(), false);
    let mut extra: Vec<Vec<u8>> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, args.get(i + 1))?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"--shell" => target = Target::Shell,
            b"--pid" => {
                optarg(ctx, a, args.get(i + 1))?;
                let p = &args[i + 1];
                if !shell::re_match("^[1-9][0-9]{0,6}$", p) {
                    return Err(ctx.die(cat!(
                        "gdb: --pid takes a pid in the box (omabox run -- ps -e), got ",
                        p
                    )));
                }
                target = Target::Pid;
                pid = p.clone();
                i += 1;
            }
            b"--watch" => watch = true,
            b"-h" | b"--help" => return crate::args::usage("gdb"),
            b"--" => {
                extra = args[i + 1..].to_vec();
                break;
            }
            x if x.starts_with(b"-") => {
                return Err(unknown_opt_hint(ctx, "gdb", a, "(gdb's own go after --)"));
            }
            x => {
                return Err(unexpected(
                    ctx,
                    "gdb",
                    x,
                    "(gdb's own arguments go after --)",
                ));
            }
        }
        i += 1;
    }
    if watch && !extra.is_empty() {
        return Err(ctx.die("gdb: --watch runs its own commands; -- ARGS go with a backtrace"));
    }
    // (an INT, TERM or HUP stops it once gdb is over, bounded: trap.rs)
    crate::trap::stop_on_signals();
    select_box(ctx, &name)?;
    need_box(ctx)?;
    if !crate::exec::executable(b"/usr/bin/gdb") {
        return Err(ctx.die("gdb: no /usr/bin/gdb (package gdb); a box runs the host's"));
    }
    // (A box this user namespace cannot read, said as box_alive says it, 1: bash's set -e ended it
    // with box_pid's 3, unsaid, finding 442.)
    let bp = match box_pid(&ctx.d) {
        BoxPid::Pid(p) => p,
        BoxPid::Unknown => {
            return Err(ctx.die(cat!(
                "cannot tell whether box '",
                ctx.name,
                "' is up from this user namespace (a sandbox's?): run omabox outside it"
            )));
        }
        BoxPid::None => return Err(ctx.die(cat!("box '", ctx.name, "' is not up"))),
    };
    covers_held(ctx)?;
    // Every namespace of the box, its network too (finding 229; enter::all), entered natively
    // (finding 490), in a session of its own (finding 491). The box went as it was entered: as
    // nsenter's failure was, a step that fails (1).
    let entry = crate::enter::Entry::open(&ctx.d, &bp, crate::enter::all(&bp)).ok();
    // (a step that is `timeout ...` keeps the caller's INT and QUIT from it, as its own process
    // group did: enter::Plan::shield_timeout)
    let inside = |keep_caps: bool, argv0: &[u8]| {
        entry
            .as_ref()
            .map(|e| e.plan(true, keep_caps, None).shield_timeout(Some(argv0)))
    };
    let step = |tail: &[&[u8]], capture: bool, err_null: bool| -> Relayed {
        let argv: Vec<Vec<u8>> = tail.iter().map(|t| t.to_vec()).collect();
        match inside(false, argv.first().map_or(&b""[..], |a| a)) {
            Some(p) => relay(&argv, capture, err_null, Some(p)),
            None => Relayed {
                rc: 1,
                out: Vec::new(),
            },
        }
    };
    match target {
        Target::Hyprland => {
            let r = step(&[b"pgrep", b"-xo", b"Hyprland"], true, false);
            if r.rc != 0 {
                return Err(ctx.die(cat!("gdb: box '", ctx.name, "' has no Hyprland running")));
            }
            pid = shell::strip_nl(&r.out);
        }
        Target::Shell => {
            let p = shell_live_pid(ctx);
            match p {
                Some(p) => pid = p,
                None => {
                    return Err(ctx.die(cat!(
                        "gdb: box '",
                        ctx.name,
                        "' has no shell running (",
                        shell_state(ctx),
                        ")"
                    )));
                }
            }
        }
        Target::Pid => {
            let d = cat!("/proc/", pid);
            if step(&[b"test", b"-d", &d], false, false).rc != 0 {
                return Err(ctx.die(cat!(
                    "gdb: box '",
                    ctx.name,
                    "' has no process ",
                    pid,
                    " (omabox run -b ",
                    ctx.name,
                    " -- ps -e)"
                )));
            }
        }
    }
    let comm = step(&[b"cat", &cat!("/proc/", pid, "/comm")], true, true);
    let mut what = if comm.rc == 0 {
        tame_comm(&shell::strip_nl(&comm.out))
    } else {
        target.word().to_vec()
    };
    if what.is_empty() {
        what = target.word().to_vec();
    }
    // A traced process's tracer, 0 for none (`$(tracer_of PID) || tracer=0`).
    let status = cat!("/proc/", pid, "/status");
    let tracer_of = || {
        let r = step(
            &[b"sed", b"-n", b"s/^TracerPid:[[:space:]]*//p", &status],
            true,
            true,
        );
        if r.rc == 0 {
            shell::strip_nl(&r.out)
        } else {
            b"0".to_vec()
        }
    };
    let traced = |t: &[u8]| !t.is_empty() && t != b"0";
    let tracer = tracer_of();
    if traced(&tracer) {
        return Err(ctx.die(cat!(
            "gdb: ",
            what,
            " (pid ",
            pid,
            " in box '",
            ctx.name,
            "') is traced already, by pid ",
            tracer,
            " (a gdb --watch?): omabox run -b ",
            ctx.name,
            " -- kill ",
            tracer,
            " ends that"
        )));
    }
    // (nsenter's --keep-caps: Plan's keep_caps, for setpriv to narrow)
    let caps = w(&[
        "setpriv",
        "--inh-caps=-all,+sys_ptrace",
        "--ambient-caps=-all,+sys_ptrace",
        "--bounding-set=-all,+sys_ptrace",
        "--",
    ]);
    let lang = shell::env("LANG")
        .filter(|l| !l.is_empty())
        .unwrap_or(b"C.UTF-8".to_vec());
    let mut gdb = w(&["env", "-i", "PATH=/usr/bin"]);
    gdb.push(cat!("HOME=", box_home(&ctx.d)));
    gdb.push(cat!("LANG=", lang));
    gdb.extend(w(&[
        "gdb",
        "-nx",
        "-batch",
        "-iex",
        "set debuginfod enabled off",
        "-iex",
        "set pagination off",
        "-iex",
        "set width 0",
        "-iex",
        "set print thread-events off",
    ]));
    if !watch {
        if extra.is_empty() {
            extra = w(&["-ex", "thread apply all bt"]);
        }
        // (timeout in the box too, as the step's first word: it is what the entry starts)
        let mut argv = w(&["timeout", "120"]);
        argv.extend(caps);
        argv.extend(gdb);
        argv.push(b"-p".to_vec());
        argv.push(pid.clone());
        argv.extend(extra);
        // (not a step that takes a Ctrl-C: `timeout` puts itself and gdb in a process group of their
        // own, which the terminal's never reaches; a Ctrl-C stops the command, gdb sent a TERM if it
        // runs on, which timeout passes on: finding 454)
        let rc = match inside(true, argv.first().map_or(&b""[..], |a| a)) {
            Some(p) => relay(&argv, false, false, Some(p)).rc,
            None => 1,
        };
        if rc == 124 {
            return Err(ctx.die(cat!(
                "gdb: no answer in 120 s: the process may be hung in a way gdb cannot stop (omabox log -b ",
                ctx.name,
                " hyprland; another process: --pid PID; omabox down ",
                ctx.name,
                " ends it)"
            )));
        }
        return crate::cmd::hyprctl::code(rc);
    }
    // Watched: in a session of its own in the box (it ends with the box), into the gdb log, which is
    // written from inside the box, its header too (finding 229): a link there resolves to the box's
    // own files; a link or FIFO is replaced (log reads only a regular file, finding 107; a FIFO would
    // hang this write, 10 s at most).
    let log = cat!(box_home(&ctx.d), "/gdb.log");
    let date = shell::date_iso();
    let head = cat!(
        "\n== omabox gdb --watch ",
        what,
        " (pid ",
        pid,
        " in the box), ",
        date
    );
    let mut argv = w(&["timeout", "10", "sh", "-c", GDB_LOG_SH, "sh"]);
    argv.push(log.clone());
    argv.push(head);
    if step(
        &argv.iter().map(Vec::as_slice).collect::<Vec<_>>(),
        false,
        false,
    )
    .rc != 0
    {
        return Err(ctx.die("gdb: cannot write the box's gdb log"));
    }
    let mut argv = caps;
    argv.extend(w(&["setsid", "-f", "sh", "-c", GDB_RUN_SH, "sh"]));
    argv.push(log);
    argv.extend(gdb);
    argv.push(b"-p".to_vec());
    argv.push(pid.clone());
    argv.extend(w(&[
        "-ex",
        "handle all nostop noprint pass",
        "-ex",
        "handle SIGSEGV SIGABRT SIGBUS SIGFPE SIGILL stop print pass",
        "-ex",
        "continue",
        "-ex",
        "echo \\n== omabox: a fatal signal; the thread it hit:\\n",
        "-ex",
        "thread",
        "-ex",
        "bt",
        "-ex",
        "echo \\n== omabox: every thread:\\n",
        "-ex",
        "thread apply all bt",
        "-ex",
        "echo == omabox: end\\n",
    ]));
    // (A failure here said, 1: bash's set -e ended it with the step's status, unsaid, finding 442.)
    let rc = match inside(true, argv.first().map_or(&b""[..], |a| a)) {
        Some(p) => relay(&argv, false, false, Some(p)).rc,
        None => 1,
    };
    if rc != 0 {
        return Err(ctx.die(cat!(
            "gdb: cannot start gdb in box '",
            ctx.name,
            "' (status ",
            rc.to_string(),
            ")"
        )));
    }
    let end = crate::settle::after(10);
    loop {
        if traced(&tracer_of()) {
            break;
        }
        if crate::settle::past(end) {
            return Err(ctx.die(cat!(
                "gdb: did not attach to ",
                what,
                " in 10 s (omabox log -b ",
                ctx.name,
                " gdb)"
            )));
        }
        crate::trap::sleep(b"0.1");
    }
    say(cat!(
        "gdb watches ",
        what,
        " (pid ",
        pid,
        " in box '",
        ctx.name,
        "'): on a crash, every thread's backtrace goes to omabox log -b ",
        ctx.name,
        " gdb (--crash: the thread it hit)"
    ));
    Ok(())
}

/// The gdb log's header, written in the box (bin/omabox's, byte for byte).
pub const GDB_LOG_SH: &str =
    r#"{ [ ! -L "$1" ] && [ ! -p "$1" ]; } || rm -f -- "$1"; printf "%s\n" "$2" >> "$1""#;
/// gdb --watch's start in its session (bin/omabox's, byte for byte).
pub const GDB_RUN_SH: &str = r#"log=$1; shift; exec "$@" >> "$log" 2>&1 </dev/null"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_tamed() {
        assert_eq!(tame_comm(b"Hyprland"), b"Hyprland");
        assert_eq!(tame_comm(b"a b.c_d:e+f@g-h"), b"a b.c_d:e+f@g-h");
        assert_eq!(tame_comm(b"x\ny\x1b"), b"x?y?");
        assert_eq!(tame_comm(b""), b"");
    }
}
