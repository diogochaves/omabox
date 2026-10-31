//! omabox's help (#324 wave 6-front, A1): bash's usage_text compiled in, cut as bash's usage and
//! usage_form cut it: bare `help` is every command's lines, `help --long` all of it, `help CMD` (and
//! `CMD --help`) one command's lines and the paragraphs whose `@` line names it. `@` lines never show;
//! `@short` paragraphs only in the short form, which is the default. help.txt was bash's text with
//! `$PASS_KEY` expanded, the same bytes until the port ended (#324 wave 6b-end); it is the help now.

use crate::exit::{Exit, Res, out, say};

/// usage_text's output.
pub const TEXT: &str = include_str!("help.txt");

/// The commands, for an unknown one's nearest (aliases: screenshot, logs, list, version).
pub const COMMANDS: &[&str] = &[
    "up",
    "run",
    "shot",
    "record",
    "pixel",
    "calibrate",
    "windows",
    "window",
    "jobs",
    "stop",
    "tree",
    "tray",
    "check",
    "keys",
    "keys-to-box",
    "click",
    "scroll",
    "pointer",
    "drag",
    "wait",
    "hyprctl",
    "lua",
    "log",
    "events",
    "peek",
    "restart-shell",
    "reload",
    "mode",
    "output",
    "monitor",
    "plugin",
    "gdb",
    "gpu",
    "cpu",
    "audio",
    "polkit",
    "ports",
    "env",
    "path",
    "down",
    "ls",
    "which",
    "pid",
    "save",
    "saves",
    "config",
    "guard",
    "host",
    "clip",
    "broker",
    "brief",
    "setup",
    "help",
    "version",
];

/// The text's lines, as awk reads them (records split at newlines, no empty one after the last).
fn lines() -> impl Iterator<Item = &'static [u8]> {
    TEXT.as_bytes().split(|&b| b == b'\n').take(
        TEXT.as_bytes().iter().filter(|&&b| b == b'\n').count()
            + usize::from(!TEXT.ends_with('\n')),
    )
}

/// awk's fields: split at blanks, none empty.
fn fields(l: &[u8]) -> Vec<&[u8]> {
    l.split(|&b| b == b' ' || b == b'\t')
        .filter(|f| !f.is_empty())
        .collect()
}

/// usage_form CMD LONG: one command's help, long or short, its trailing newlines cut (as bash's
/// `$(...)` cuts them). run takes up's options (a throwaway box): its help has up's lines too,
/// after its own. Empty for a word the text has no lines for.
pub fn form(c: &[u8], long: bool) -> Vec<u8> {
    let (mut prose, mut on, mut mine, mut gap) = (false, false, false, false);
    let (mut up, mut o) = (Vec::new(), Vec::new());
    for l in lines().skip(2) {
        if !prose && l.is_empty() {
            prose = true;
            o.append(&mut up);
            continue;
        }
        let f = fields(l);
        if !prose {
            if f.first() == Some(&&b"omabox"[..]) {
                on = f.get(1) == Some(&c);
                mine = on || (c == b"run" && f.get(1) == Some(&&b"up"[..]));
            }
            let to = if on {
                &mut o
            } else if mine {
                &mut up
            } else {
                continue;
            };
            to.extend_from_slice(l);
            to.push(b'\n');
            continue;
        }
        if l.starts_with(b"@") {
            let skip: &[u8] = if long { b"@short" } else { b"@" };
            on = f.first() != Some(&skip) && f.iter().skip(1).any(|w| *w == c);
            if on && !gap {
                gap = true;
                o.push(b'\n');
            }
            continue;
        }
        if on {
            o.extend_from_slice(l);
            o.push(b'\n');
        }
    }
    while o.last() == Some(&b'\n') {
        o.pop();
    }
    o
}

/// Bare `help`: the lines of every command (to the first blank line past the title), then where
/// the rest is. `help --long`: the whole text but the `@` lines and the `@short` paragraphs.
fn bare(long: bool) -> Vec<u8> {
    let mut o = Vec::new();
    if long {
        let mut skip = false;
        for l in lines() {
            if l.starts_with(b"@") {
                skip = fields(l).first() == Some(&&b"@short"[..]);
                continue;
            }
            if !skip {
                o.extend_from_slice(l);
                o.push(b'\n');
            }
        }
        return o;
    }
    for (i, l) in lines().enumerate() {
        if i >= 2 && l.is_empty() {
            break;
        }
        o.extend_from_slice(l);
        o.push(b'\n');
    }
    // (#327: how the box is picked without -b was in --long only)
    o.extend_from_slice(b"\nWithout -b, the box is $OMABOX, else the git repo's name (plus the agent session's id), else\n\"default\": omabox help up --long says more.\n");
    o.extend_from_slice(b"\nmore: omabox help CMD (one command's options), omabox help --long (every command's notes too)\n");
    o
}

/// What `help ARGS` prints: Ok(text), or Err(()) for a word with no help (an unknown command).
pub fn text(args: &[Vec<u8>]) -> Result<Vec<u8>, ()> {
    let long = args.iter().any(|a| a == b"--long");
    let Some(first) = args.iter().find(|a| *a != b"--long") else {
        return Ok(bare(long));
    };
    let c: &[u8] = match first.as_slice() {
        // (bash's usage calls itself with nothing: --long is not passed on)
        b"-h" | b"--help" => return Ok(bare(false)),
        b"screenshot" => b"shot",
        b"logs" => b"log",
        b"list" => b"ls",
        b"version" | b"-V" => b"--version",
        c => c,
    };
    let full = form(c, true);
    if full.is_empty() {
        return Err(());
    }
    if !long {
        let short = form(c, false);
        if short != full {
            return Ok(crate::cat!(short, "\n\nmore: omabox help ", c, " --long\n"));
        }
    }
    Ok(crate::cat!(full, "\n\nomabox help: every command.\n"))
}

/// `omabox help [CMD] [--long]`, `omabox CMD --help`: the help on stdout, exit 0; an unknown
/// command's line, exit 2.
pub fn run(args: &[Vec<u8>]) -> Res<()> {
    match text(args) {
        Ok(t) => out(t),
        Err(()) => {
            let first = args.iter().find(|a| *a != b"--long");
            Err(unknown_command(first.map_or(&b""[..], Vec::as_slice)))
        }
    }
}

/// A command's `--help` after its other words (`usage CMD; return`): its help, the command's
/// status that.
pub fn usage(cmd: &str) -> Res<()> {
    run(&[cmd.as_bytes().to_vec()])
}

/// A command omabox does not have: exit 2, with the nearest one it has.
pub fn unknown_command(c: &[u8]) -> Exit {
    say(crate::cat!(
        "unknown command: ",
        c,
        crate::args::did_you_mean(&String::from_utf8_lossy(c), COMMANDS),
        " (omabox help lists the commands and their options)"
    ));
    Exit(2)
}

/// The first paragraph of `omabox help CMD`: its lines (and up's for run), for opt_form.
pub fn head(cmd: &[u8]) -> Vec<u8> {
    let t = text(&[cmd.to_vec()]).unwrap_or_default();
    let end = t
        .windows(2)
        .position(|w| w == b"\n\n")
        .map_or(t.len(), |i| i + 1);
    t[..end].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_has_help() {
        for c in COMMANDS {
            assert!(text(&[c.as_bytes().to_vec()]).is_ok(), "{c}");
        }
        for c in [
            "screenshot",
            "logs",
            "list",
            "-V",
            "--version",
            "exit-codes",
        ] {
            assert!(text(&[c.as_bytes().to_vec()]).is_ok(), "{c}");
        }
        assert!(text(&[b"nosuch".to_vec()]).is_err());
        assert!(text(&[Vec::new()]).is_err());
    }

    #[test]
    fn forms() {
        // the short form names where the long one is, unless they are the same
        let t =
            String::from_utf8(text(&[b"shot".to_vec()]).unwrap_or_default()).unwrap_or_default();
        assert!(t.starts_with("  omabox shot "), "{t}");
        assert!(t.ends_with("\n\nmore: omabox help shot --long\n"), "{t}");
        let l = text(&[b"screenshot".to_vec(), b"--long".to_vec()]).unwrap_or_default();
        assert!(l.ends_with(b"\n\nomabox help: every command.\n"));
        // run's help has up's lines after its own
        let r = head(b"run");
        assert!(r.starts_with(b"  omabox run "));
        assert!(exit_contains(&r, b"\n  omabox up "));
        // no `@` line ever shows
        for a in [&[][..], &[b"--long".to_vec()][..], &[b"up".to_vec()][..]] {
            let t = text(a).unwrap_or_default();
            assert!(!t.split(|&b| b == b'\n').any(|l| l.starts_with(b"@")));
        }
        assert_eq!(
            bare(false),
            text(&[b"-h".to_vec(), b"--long".to_vec()]).unwrap_or_default()
        );
    }

    fn exit_contains(h: &[u8], n: &[u8]) -> bool {
        crate::exit::contains(h, n)
    }
}
