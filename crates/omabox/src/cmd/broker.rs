//! `omabox broker [on|off]` (issue #16, finding 99): the broker as a systemd user socket (started on
//! the first connection), and what to add to ai-jail's config (`ai_jail_config`) so a jail reaches
//! it. omabox never edits that file. The broker itself (a jailed caller's policy read from its jail's bwrap command line,
//! broker_init, and what it refuses, broker_check) runs in bash's front until wave 6: this is only
//! the switch.

use crate::args::unknown_word;
use crate::cat;
use crate::exit::{Ctx, Exit, Res, out};
use crate::shell;
use std::process::Stdio;

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let act = args.first().cloned().unwrap_or_default();
    if !matches!(act.as_slice(), b"on" | b"off" | b"") {
        return Err(unknown_word(
            ctx,
            "broker",
            "subcommand",
            &act,
            "(on, off, or nothing to see it)",
            &["on", "off"],
        ));
    }
    if args.len() > 1 {
        return Err(ctx.usage_die(cat!("broker: one of on or off, got ", args.join(&b' '))));
    }
    ctx.parsed();
    let units = cat!(shell::env_or_empty("HOME"), "/.config/systemd/user");
    let sock = cat!(units, "/omabox-broker.socket");
    let svc = cat!(units, "/omabox-broker.service");
    let root = shell::bytes(&crate::root()).to_vec();
    let relay = cat!(root, "/tools/relay/omabox-relay");
    let bsock = broker_sock(ctx);
    match act.as_slice() {
        b"on" => {
            if !crate::exec::executable(&relay) {
                let mut c = crate::sig::command("make");
                c.arg("-s")
                    .arg("-C")
                    .arg(shell::path(&cat!(root, "/tools/relay")));
                if crate::sig::run_status(c).unwrap_or(127) != 0 {
                    return Err(ctx.die("building tools/relay failed"));
                }
            }
            let _ = std::fs::create_dir_all(shell::path(&units));
            let socket_unit = cat!(
                "# Written by omabox broker on (",
                root,
                "): lets an agent inside ai-jail drive its own boxes.\n[Unit]\nDescription=omabox broker socket (agents inside ai-jail)\n\n[Socket]\nListenStream=%t/omabox/.broker/sock\nSocketMode=0600\nDirectoryMode=0700\nRemoveOnStop=yes\n\n[Install]\nWantedBy=sockets.target\n"
            );
            // KillMode=process: the boxes it starts outlive the broker (their own reapers end them).
            let service_unit = cat!(
                "# Written by omabox broker on (",
                root,
                ").\n[Unit]\nDescription=omabox broker (agents inside ai-jail)\n\n[Service]\nExecStart=",
                shell::quote_q(&relay),
                " listen -- ",
                shell::quote_q(&cat!(root, "/bin/omabox")),
                "\nKillMode=process\n"
            );
            for (p, body) in [(&sock, socket_unit), (&svc, service_unit)] {
                if let Err(e) = std::fs::write(shell::path(p), body) {
                    return Err(ctx.die(crate::shots::cant_write(p, &e)));
                }
            }
            systemctl(&["daemon-reload"], false)?;
            if systemctl(&["enable", "--now", "omabox-broker.socket"], true).is_err() {
                return Err(ctx.die("systemctl --user enable --now omabox-broker.socket failed"));
            }
            systemctl(&["try-restart", "omabox-broker.service"], false)?;
            out(cat!("broker on: ", bsock, "\n"))?;
            out(recipe(&root, &relay, &bsock))
        }
        b"off" => {
            let _ = systemctl(&["disable", "--now", "omabox-broker.socket"], true);
            let _ = systemctl_quiet_err(&["stop", "omabox-broker.service"]);
            let _ = std::fs::remove_file(shell::path(&sock));
            let _ = std::fs::remove_file(shell::path(&svc));
            // The unit's ListenStream (%t, the runtime dir this omabox's boxes live in, not
            // OMABOX_BROKER_SOCK: that is where a jail looks for it). RemoveOnStop takes it as the
            // socket stops; an older omabox's unit had none (#393).
            clear_sock(&cat!(ctx.boxes, "/.broker"));
            systemctl(&["daemon-reload"], false)?;
            out(cat!(
                "broker off (the ro_maps for omabox in ",
                ai_jail_config(),
                ", if you added them, can go)\n"
            ))
        }
        _ => {
            let mut c = crate::sig::command("systemctl");
            c.args(["--user", "is-active", "-q", "omabox-broker.socket"])
                .stderr(Stdio::null());
            if crate::sig::run_status(c).unwrap_or(127) == 0 {
                out(cat!("broker on: ", bsock, "\n"))?;
                out(recipe(&root, &relay, &bsock))
            } else {
                out("broker off (omabox broker on turns it on)\n")
            }
        }
    }
}

/// BROKER_SOCK: $OMABOX_BROKER_SOCK, else the runtime dir's.
fn broker_sock(ctx: &Ctx) -> Vec<u8> {
    let s = shell::env_or_empty("OMABOX_BROKER_SOCK");
    if s.is_empty() {
        cat!(ctx.boxes, "/.broker/sock")
    } else {
        s
    }
}

/// What the broker's socket unit left in DIR (`.broker/`), gone (#393): its `sock` when it is a socket
/// (a file of that name that is not one is not ours), then DIR itself when nothing else is in it.
fn clear_sock(dir: &[u8]) {
    use std::os::unix::fs::FileTypeExt;
    let s = cat!(dir, "/sock");
    if std::fs::symlink_metadata(shell::path(&s)).is_ok_and(|m| m.file_type().is_socket()) {
        let _ = std::fs::remove_file(shell::path(&s));
    }
    let _ = std::fs::remove_dir(shell::path(dir));
}

/// `systemctl --user ARGS` (QUIET: `>/dev/null 2>&1`): its failure ends the command with its status,
/// as bash's set -e did.
fn systemctl(args: &[&str], quiet: bool) -> Res<()> {
    let mut c = crate::sig::command("systemctl");
    c.arg("--user").args(args);
    if quiet {
        c.stdout(Stdio::null()).stderr(Stdio::null());
    }
    match crate::sig::run_status(c).unwrap_or(127) {
        0 => Ok(()),
        rc => Err(Exit(u8::try_from(rc).unwrap_or(1))),
    }
}

/// `systemctl --user ARGS 2>/dev/null`.
fn systemctl_quiet_err(args: &[&str]) -> i32 {
    let mut c = crate::sig::command("systemctl");
    c.arg("--user").args(args).stderr(Stdio::null());
    crate::sig::run_status(c).unwrap_or(127)
}

/// ai-jail's trusted global config: `$XDG_CONFIG_HOME/ai-jail/config.toml`, `~/.config` without one
/// (ai-jail 3.0 moved it there; 2.x read `~/.ai-jail`, which 3.x migrates on its first launch).
fn ai_jail_config() -> Vec<u8> {
    let base = shell::env("XDG_CONFIG_HOME")
        .filter(|x| x.starts_with(b"/"))
        .unwrap_or_else(|| cat!(shell::env_or_empty("HOME"), "/.config"));
    cat!(base, "/ai-jail/config.toml")
}

/// broker_recipe: what to add to ai-jail's config for a jail to reach the broker.
fn recipe(root: &[u8], relay: &[u8], bsock: &[u8]) -> Vec<u8> {
    let bin = cat!(shell::env_or_empty("HOME"), "/.local/bin");
    let dir = bsock
        .iter()
        .rposition(|&c| c == b'/')
        .map_or(bsock, |i| &bsock[..i]);
    cat!(
        "\nFor ai-jail, add to ",
        ai_jail_config(),
        " (merge into the ro_maps there, if it has\nsome; or under [commands.claude] for one agent only; ai-jail before 3.0 reads ~/.ai-jail):\n\nro_maps = [\"",
        root,
        "/bin/omabox:",
        bin,
        "/omabox\", \"",
        relay,
        ":",
        bin,
        "/omabox-relay\", \"",
        root,
        "/skill\", \"",
        dir,
        "\"]\n\nor for one run: ai-jail --map ",
        root,
        "/bin/omabox:",
        bin,
        "/omabox --map ",
        relay,
        ":",
        bin,
        "/omabox-relay --map ",
        root,
        "/skill --map ",
        dir,
        " claude\n\n(omabox itself and its relay, the agent skill your agents link to, and the broker's socket; all read-only.)\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn clear_sock_takes_only_the_socket_and_an_empty_folder() {
        let d = std::env::temp_dir().join(format!("omabox-brk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let b = d.join(".broker");
        let bb = shell::bytes(&b).to_vec();
        // The socket left behind: it and its folder go, the runtime dir above stays.
        std::fs::create_dir_all(&b).expect("dirs");
        drop(UnixListener::bind(b.join("sock")).expect("a socket"));
        clear_sock(&bb);
        assert!(!b.exists());
        assert!(d.is_dir());
        // Something else beside it: the socket goes, the folder and the other file stay.
        std::fs::create_dir_all(&b).expect("dirs");
        drop(UnixListener::bind(b.join("sock")).expect("a socket"));
        std::fs::write(b.join("notes"), "mine").expect("a file");
        clear_sock(&bb);
        assert!(!b.join("sock").exists());
        assert_eq!(
            std::fs::read_to_string(b.join("notes")).ok().as_deref(),
            Some("mine")
        );
        // A `sock` that is no socket is not ours.
        std::fs::remove_file(b.join("notes")).expect("rm");
        std::fs::write(b.join("sock"), "mine").expect("a file");
        clear_sock(&bb);
        assert_eq!(
            std::fs::read_to_string(b.join("sock")).ok().as_deref(),
            Some("mine")
        );
        // Nothing there at all: nothing to do.
        let _ = std::fs::remove_dir_all(&d);
        clear_sock(&bb);
        assert!(!d.exists());
    }
}
