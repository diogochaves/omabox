//! omabox (#324): one binary, built from this crate into build/omabox, which bin/omabox links to (an
//! install has it at bin/omabox). It runs the front itself (front.rs): `-b NAME`, `--opt=value`,
//! `--help`, the relay and the broker.
mod args;
mod boxes;
mod broker_side;
mod bwrap;
mod cmd;
mod conf;
mod desk;
mod deskwin;
mod enter;
mod exec;
mod exit;
mod front;
mod gpu;
mod guard;
mod help;
mod homeuse;
mod host;
mod hypr;
mod input;
mod install;
mod jail;
mod jailpath;
mod jobs;
mod jq;
mod jqprog;
mod json;
mod layer;
mod logs;
mod marks;
mod mon;
mod net;
mod num;
mod paths;
mod pidfd;
mod plugins;
mod probe;
mod procfs;
mod ptr;
mod ready;
mod recprog;
mod relay;
mod render;
mod repo;
mod screen;
mod seed;
mod select;
mod settle;
mod shell;
mod shelljson;
mod shellnote;
mod shots;
mod sig;
mod trap;
mod upspec;
mod win;

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::process::ExitCode;

/// When this omabox was called, in µs (bash's CALL_US, taken before anything else).
static CALL_US: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
/// When this process was made, at the latest, in µs of CLOCK_BOOTTIME (procfs::made_us): taken
/// first, before anything that may sleep (#404).
static MADE_US: std::sync::OnceLock<Option<i64>> = std::sync::OnceLock::new();

type Run = fn(&mut exit::Ctx, &[Vec<u8>]) -> exit::Res<()>;

fn main() -> ExitCode {
    let _ = MADE_US.set(procfs::made_us());
    // As bash: a write to a closed pipe ends the command (Rust ignores SIGPIPE), unless SIGPIPE came
    // ignored: bash keeps it so, for itself and what it runs (sig::command gives the children it
    // back: std's spawn resets it). No setlocale: text is UTF-8 and lists byte order in every locale
    // (findings 445-447).
    if !sig::pipe_ignored() {
        // SAFETY: at the start, before any other thread; SIG_DFL is a valid disposition.
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        }
    }
    // The native box entry for a command line script runs (exec.rs's box_pty): before anything else,
    // as nsenter was (one thread, nothing caught). Never for a broker's caller: its words go through
    // broker_check, which refuses `_enter` (S9); broker_init takes OMABOX_BROKER_PEER out of the
    // environment, so the broker's own box_pty children still come here.
    if std::env::args_os().nth(1).is_some_and(|a| a == "_enter")
        && std::env::var_os("OMABOX_BROKER_PEER").is_none_or(|p| p.is_empty())
    {
        let rest: Vec<OsString> = std::env::args_os().skip(2).collect();
        return enter::main(&rest);
    }
    // A save's watch (#369): resumes the box a save paused if that save dies before it lets go. As
    // `_enter`, never for a broker's caller (the front refuses it there).
    if std::env::args_os()
        .nth(1)
        .is_some_and(|a| a == "_save-watch")
        && std::env::var_os("OMABOX_BROKER_PEER").is_none_or(|p| p.is_empty())
    {
        let rest: Vec<OsString> = std::env::args_os().skip(2).collect();
        return cmd::save::watch_main(&rest);
    }
    // Bash ignores SIGQUIT for itself (a Ctrl-\ does not end the script), and what it runs in the
    // foreground gets the default back: a handler that does nothing is that (reset at exec). One
    // that came ignored stays ignored, for it and its children (sig::catch leaves it).
    sig::catch(libc::SIGQUIT, quit_ignored);
    sig::set_main();
    let _ = CALL_US.set(marks::epoch_us());
    let _ = ROOT.set(find_root());
    let args = std::env::args_os().skip(1);
    // BOXES: bash's first line refuses a call without XDG_RUNTIME_DIR, whatever it asks.
    let runtime = shell::env_or_empty("XDG_RUNTIME_DIR");
    if runtime.is_empty() {
        exit::say("XDG_RUNTIME_DIR is not set");
        return ExitCode::from(1);
    }
    let argv: Vec<Vec<u8>> = args.map(OsString::into_vec).collect();
    let (cmd, rest): (OsString, Vec<OsString>) =
        match front::front(&cat!(runtime, "/omabox"), &argv) {
            Ok(Some(c)) => (
                OsString::from_vec(c.cmd),
                c.args.into_iter().map(OsString::from_vec).collect(),
            ),
            Ok(None) => return ExitCode::SUCCESS,
            Err(exit::Exit(rc)) => return ExitCode::from(rc),
        };
    let ported: Option<Run> = match cmd.to_str() {
        Some("path") => Some(cmd::path::run),
        Some("env") => Some(cmd::env::run),
        Some("which") => Some(cmd::which::run),
        Some("hyprctl") => Some(cmd::hyprctl::run),
        Some("lua") => Some(cmd::lua::run),
        Some("run") => Some(cmd::run::run),
        Some("log" | "logs") => Some(cmd::log::run),
        Some("events") => Some(cmd::events::run),
        Some("wait") => Some(cmd::wait::run),
        Some("windows") => Some(cmd::windows::run),
        Some("jobs") => Some(cmd::jobs::run),
        Some("stop") => Some(cmd::jobs::stop),
        Some("tree") => Some(cmd::tree::run),
        Some("pixel") => Some(cmd::pixel::run),
        Some("keys") => Some(cmd::keys::run),
        Some("click") => Some(cmd::click::run),
        Some("scroll") => Some(cmd::click::scroll),
        Some("pointer") => Some(cmd::pointer::run),
        Some("drag") => Some(cmd::drag::run),
        Some("shot" | "screenshot") => Some(cmd::shot::run),
        Some("calibrate") => Some(cmd::calibrate::run),
        Some("keys-to-box") => Some(cmd::keys_to_box::run),
        Some("monitor") => Some(cmd::monitor::run),
        Some("output") => Some(cmd::output::run),
        Some("mode") => Some(cmd::mode::run),
        Some("gpu") => Some(cmd::gpu::run),
        Some("cpu") => Some(cmd::cpu::run),
        Some("audio") => Some(cmd::audio::run),
        Some("ports") => Some(cmd::ports::run),
        Some("clip") => Some(cmd::clip::run),
        Some("peek") => Some(cmd::peek::run),
        Some("gdb") => Some(cmd::gdb::run),
        Some("check") => Some(cmd::check::run),
        Some("record") => Some(cmd::record::run),
        Some("up") => Some(cmd::up::run),
        Some("down") => Some(cmd::down::run),
        Some("_reap") => Some(cmd::reap::run),
        Some("ls" | "list") => Some(cmd::ls::run),
        Some("save") => Some(cmd::save::save),
        Some("saves") => Some(cmd::save::saves),
        Some("version" | "-V" | "--version") => Some(cmd::version::run),
        Some("config") => Some(cmd::config::run),
        Some("brief") => Some(cmd::brief::run),
        Some("host") => Some(cmd::host::run),
        Some("broker") => Some(cmd::broker::run),
        Some("guard") => Some(cmd::guard::run),
        Some("setup") => Some(cmd::setup::run),
        Some("reload") => Some(cmd::reload::run),
        Some("restart-shell") => Some(cmd::restart_shell::run),
        Some("help" | "-h" | "--help") => Some(|_, a| help::run(a)),
        _ => None,
    };
    if let Some(run) = ported {
        let words: Vec<Vec<u8>> = rest.into_iter().map(OsString::into_vec).collect();
        let mut ctx = exit::Ctx::new(cmd.as_encoded_bytes(), cat!(runtime, "/omabox"));
        let r = run(&mut ctx, &words);
        // (shot --no-pointer's hold, given back however the command ended: bash's EXIT trap)
        ptr::release_at_exit();
        return match r {
            Ok(()) => ExitCode::SUCCESS,
            Err(exit::Exit(rc)) => ExitCode::from(rc),
        };
    }
    match cmd.to_str() {
        // The suite's read-only probe of the helpers it once sourced from bin/omabox (probe.rs).
        Some("_rust") if rest.first().is_some_and(|a| a == "probe") => {
            let words: Vec<Vec<u8>> = rest.into_iter().skip(1).map(OsString::into_vec).collect();
            match probe::run(cat!(runtime, "/omabox"), &words) {
                Ok(()) => ExitCode::SUCCESS,
                Err(exit::Exit(rc)) => ExitCode::from(rc),
            }
        }
        // The options table (args.rs), for the suite's check of it against each command's help
        // (t_unit_cli_opts).
        Some("_rust-opts") => match rest
            .first()
            .and_then(|c| args::cmd_opts(c.as_encoded_bytes()))
        {
            Some(t) => {
                println!("{t}");
                ExitCode::SUCCESS
            }
            None => ExitCode::from(1),
        },
        // The suite's reads of a jail, as the broker reads them (broker_side::probe).
        Some("_rust-jail") => {
            let words: Vec<Vec<u8>> = rest.into_iter().map(OsString::into_vec).collect();
            ExitCode::from(broker_side::probe(&words))
        }
        // A signal's release of shot --no-pointer's hold (ptr::hold_until_exit), in this process.
        Some("_ptr-release") => {
            let words: Vec<Vec<u8>> = rest.into_iter().map(OsString::into_vec).collect();
            ExitCode::from(ptr::release_main(&words))
        }
        // A jq program by its name (jqprog::by_name), for the suite to run with jq of its own.
        Some("_rust-jq") => match rest
            .first()
            .and_then(|n| jqprog::by_name(&n.to_string_lossy()))
        {
            Some(p) => {
                print!("{p}");
                ExitCode::SUCCESS
            }
            None => ExitCode::from(2),
        },
        // (the front's: a command omabox does not have)
        _ => {
            let exit::Exit(rc) = help::unknown_command(cmd.as_encoded_bytes());
            ExitCode::from(rc)
        }
    }
}

extern "C" fn quit_ignored(_: libc::c_int) {}

/// ROOT: the checkout (or install) this binary belongs to, found once at the start, before anything
/// could replace the file: the parent of the directory the binary is in, through any link (a
/// checkout's build/omabox, an install's bin/omabox, a jail's ~/.local/bin/omabox: D1).
static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

fn find_root() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent()
        .and_then(|p| p.parent())
        .map(PathBuf::from)
        .unwrap_or_default()
}

pub fn root() -> PathBuf {
    ROOT.get_or_init(find_root).clone()
}

/// MADE_US: when this process was made, at the latest, in µs of CLOCK_BOOTTIME.
pub fn made_us() -> Option<i64> {
    *MADE_US.get_or_init(procfs::made_us)
}

/// CALL_US: when this omabox was called, in µs.
pub fn call_us() -> Vec<u8> {
    CALL_US.get().cloned().unwrap_or_else(marks::epoch_us)
}
