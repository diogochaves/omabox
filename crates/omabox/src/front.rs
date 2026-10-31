//! The front of the CLI (#324 wave 6-front, A2): what bash's main did with a call's words before
//! a command reads them. `-b NAME` before the command moved after it (global_box), `--opt=value`
//! split (eq_split), `CMD --help` answered, the guard's mark made.

use crate::args::cmd_opts;
use crate::cat;
use crate::exit::{Ctx, Exit, Res, say};

/// The commands about one box, which take `-b NAME` before them (issue #36).
pub const BOX_CMDS: &[&[u8]] = &[
    b"up",
    b"run",
    b"shot",
    b"screenshot",
    b"record",
    b"pixel",
    b"calibrate",
    b"windows",
    b"window",
    b"jobs",
    b"stop",
    b"tree",
    b"tray",
    b"find",
    b"check",
    b"keys",
    b"keys-to-box",
    b"click",
    b"scroll",
    b"pointer",
    b"drag",
    b"wait",
    b"hyprctl",
    b"lua",
    b"log",
    b"logs",
    b"gdb",
    b"pid",
    b"events",
    b"peek",
    b"restart-shell",
    b"reload",
    b"mode",
    b"output",
    b"monitor",
    b"plugin",
    b"gpu",
    b"cpu",
    b"audio",
    b"polkit",
    b"ports",
    b"env",
    b"path",
    b"down",
    b"save",
    b"clip",
    b"brief",
];

/// A command that is not about one box, given -b: one line, exit 2.
fn not_one_box(c: &[u8]) -> Exit {
    say(cat!(c, " takes no -b: it is not about one box"));
    Exit(2)
}

/// global_box: `omabox -b NAME CMD ...` is `omabox CMD -b NAME ...` for every command that takes
/// -b; `help` and `--version` drop it; the rest are not about one box and say so. Before anything
/// reads the command (the broker's policy and the relay too). CTX has no command yet.
pub fn global_box(ctx: &Ctx, args: &[Vec<u8>]) -> Res<Vec<Vec<u8>>> {
    let is_box = |a: &[u8]| a == b"-b" || a == b"--box" || a.starts_with(b"--box=");
    if !args.first().is_some_and(|a| is_box(a)) {
        return Ok(args.to_vec());
    }
    let (mut name, mut flag, mut i) = (Vec::new(), Vec::new(), 0);
    while let Some(a) = args.get(i).filter(|a| is_box(a)) {
        if let Some(n) = a.strip_prefix(b"--box=") {
            if n.is_empty() {
                crate::args::optarg(ctx, b"--box", None)?;
            }
            (name, flag) = (n.to_vec(), b"--box".to_vec());
            i += 1;
        } else {
            crate::args::optarg(ctx, a, args.get(i + 1))?;
            (name, flag) = (args[i + 1].clone(), a.clone());
            i += 2;
        }
    }
    let rest = &args[i..];
    let Some(c) = rest.first() else {
        say(cat!(
            flag,
            " NAME goes with a command: omabox ",
            flag,
            " NAME COMMAND ... (omabox help)"
        ));
        return Err(Exit(2));
    };
    match c.as_slice() {
        // (an empty word is no command: bash's `${1:-}` reads it as none)
        b"" => {
            say(cat!(
                flag,
                " NAME goes with a command: omabox ",
                flag,
                " NAME COMMAND ... (omabox help)"
            ));
            return Err(Exit(2));
        }
        b"help" | b"-h" | b"--help" | b"version" | b"-V" | b"--version" => return Ok(rest.to_vec()),
        _ => {}
    }
    if !BOX_CMDS.contains(&c.as_slice()) {
        return Err(match c.as_slice() {
            b"ls" | b"list" | b"which" | b"saves" | b"config" | b"guard" | b"host" | b"broker" => {
                not_one_box(c)
            }
            _ => crate::help::unknown_command(c),
        });
    }
    let mut out = vec![c.clone(), flag, name];
    out.extend_from_slice(&rest[1..]);
    Ok(out)
}

/// eq_split (#273, audit F9): every option that takes a value takes it after `=` too, split here
/// before any parser, the relay or the broker sees the words. Only words in an option's place: a
/// value stays as given, so does everything after `--`, and for a command whose remaining words are
/// another program's (run, pointer, hyprctl, lua) everything from its first word that is not one
/// of omabox's options. An option that takes no value refuses one; an unknown --x=y is left for the
/// parser, which names its nearest option.
pub fn eq_split(ctx: &Ctx, cmd: &[u8], args: &[Vec<u8>]) -> Res<Vec<Vec<u8>>> {
    let Some(table) = cmd_opts(cmd) else {
        return Ok(args.to_vec());
    };
    let kind = |o: &[u8]| -> Option<u8> {
        table.split_ascii_whitespace().find_map(|w| {
            let w = w.as_bytes();
            match w.last() {
                Some(&k @ (b'=' | b'?')) if &w[..w.len() - 1] == o => Some(k),
                _ if w == o => Some(b' '),
                _ => None,
            }
        })
    };
    let front = matches!(cmd, b"run" | b"pointer" | b"hyprctl" | b"lua");
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        i += 1;
        if a == b"--" {
            out.extend_from_slice(&args[i - 1..]);
            return Ok(out);
        }
        // (`--?*=*`: two dashes, a character, then an `=` somewhere; the option is what comes
        // before the first `=`)
        if a.starts_with(b"--") && a.get(3..).is_some_and(|t| t.contains(&b'=')) {
            let eq = a.iter().position(|&b| b == b'=').unwrap_or(a.len());
            let o = &a[..eq];
            match kind(o) {
                Some(b'=' | b'?') => {
                    out.push(o.to_vec());
                    out.push(a.get(eq + 1..).unwrap_or_default().to_vec());
                    continue;
                }
                Some(_) => {
                    return Err(ctx.usage_die(cat!(cmd, ": ", o, " takes no value, got ", a)));
                }
                None => {}
            }
        } else if a.len() > 1 && a[0] == b'-' {
            match kind(a) {
                Some(b'=') => {
                    out.push(a.clone());
                    if let Some(v) = args.get(i) {
                        out.push(v.clone());
                        i += 1;
                    }
                    continue;
                }
                Some(b'?') => {
                    out.push(a.clone());
                    if let Some(v) = args.get(i).filter(|v| !v.starts_with(b"-")) {
                        out.push(v.clone());
                        i += 1;
                    }
                    continue;
                }
                Some(_) => {
                    out.push(a.clone());
                    continue;
                }
                None => {}
            }
        }
        if front {
            out.extend_from_slice(&args[i - 1..]);
            return Ok(out);
        }
        out.push(a.clone());
    }
    Ok(out)
}

/// Under the guard (WAYLAND_DISPLAY=omabox-guard), its file in the runtime dir (GUARD_MARK, #290),
/// should the session's hook be older than it. Nothing said when it cannot be made.
pub fn guard_mark() {
    if crate::shell::env_or_empty("WAYLAND_DISPLAY") != crate::guard::GUARD.as_bytes() {
        return;
    }
    let rt = crate::shell::env_or_empty("XDG_RUNTIME_DIR");
    if rt.is_empty() {
        return;
    }
    let f = crate::shell::path(&cat!(rt, "/", crate::guard::GUARD));
    if std::fs::symlink_metadata(&f).is_ok() || f.exists() {
        return;
    }
    let _ = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(f);
}

/// What the front leaves for the command: its name and its words, and run's exit code for a
/// refusal of its own (125, #273 audit X3).
pub struct Call {
    pub cmd: Vec<u8>,
    pub args: Vec<Vec<u8>>,
}

/// The front, as bash's main runs it up to the dispatch: Ok(None) when it answered the call itself
/// (a command's --help).
pub fn front(boxes: &[u8], argv: &[Vec<u8>]) -> Res<Option<Call>> {
    let ctx = Ctx::new(b"", boxes.to_vec());
    let words = global_box(&ctx, argv)?;
    let (cmd, rest) = match words.split_first() {
        Some((c, r)) if !c.is_empty() => (c.clone(), r.to_vec()),
        Some((_, r)) => (b"help".to_vec(), r.to_vec()),
        None => (b"help".to_vec(), Vec::new()),
    };
    let mut ctx = Ctx::new(&cmd, boxes.to_vec());
    guard_mark();
    if cmd == b"run" {
        ctx.die_rc = Some(125);
    }
    let rest = eq_split(&ctx, &cmd, &rest)?;
    // CMD --help (after any -b NAME): that command's help, before a broker or relay would take it.
    // Not hyprctl's: its words are hyprctl's own.
    let mut j = 0;
    while rest.get(j).is_some_and(|a| a == b"-b" || a == b"--box") {
        j += 2;
    }
    if rest.get(j).is_some_and(|a| a == b"-h" || a == b"--help")
        && cmd != b"hyprctl"
        && cmd != b"_reap"
    {
        crate::help::run(std::slice::from_ref(&cmd))?;
        return Ok(None);
    }
    // The broker's caller (omabox-relay listen ran this omabox): its jail found and its call checked
    // before anything runs, omabox's own `_` commands included. calibrate draws and reads its card
    // where it is called, in a jail too: only the box's size goes by the broker.
    if !crate::shell::env_or_empty("OMABOX_BROKER_PEER").is_empty() {
        crate::broker_side::serve(&mut ctx, &cmd, &rest)?;
    } else if cmd != b"calibrate" && relay_wanted(boxes) {
        // A jailed caller: the command goes to the broker (relay.rs), never on from here.
        crate::relay::call(&mut ctx, &cmd, &rest)?;
        return Ok(None);
    }
    Ok(Some(Call { cmd, args: rest }))
}

/// BROKER_SOCK: the broker's socket, how omabox inside ai-jail reaches its boxes (issue #16).
pub fn broker_sock(boxes: &[u8]) -> Vec<u8> {
    crate::shell::env("OMABOX_BROKER_SOCK")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cat!(boxes, "/.broker/sock"))
}

/// relay_wanted: whether this omabox hands its command to the broker: it cannot enter a box here (a
/// sandbox refuses it new namespaces: ai-jail), and the broker's socket is in reach. Not the
/// broker's own call, nor one it started (OMABOX_JAIL).
pub fn relay_wanted(boxes: &[u8]) -> bool {
    use std::os::unix::fs::FileTypeExt;
    let e = |k| crate::shell::env_or_empty(k).is_empty();
    if !e("OMABOX_BROKER_PEER") || !e("OMABOX_JAIL") {
        return false;
    }
    let sock = std::fs::metadata(crate::shell::path(&broker_sock(boxes)));
    if !sock.is_ok_and(|m| m.file_type().is_socket()) {
        return false;
    }
    // (`unshare -U true`, found by PATH as bash's was)
    let mut c = crate::sig::command("unshare");
    c.args(["-U", "true"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    !c.status().is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &[&str]) -> Vec<Vec<u8>> {
        s.iter().map(|a| a.as_bytes().to_vec()).collect()
    }

    #[test]
    fn eq_splits() {
        let c = Ctx::new(b"shot", b"/x".to_vec());
        assert_eq!(
            eq_split(
                &c,
                b"shot",
                &w(&["--fit=2", "-o", "--x=y", "--json", "a=b"])
            ),
            Ok(w(&["--fit", "2", "-o", "--x=y", "--json", "a=b"]))
        );
        assert_eq!(eq_split(&c, b"shot", &w(&["--json=1"])), Err(Exit(2)));
        assert_eq!(
            eq_split(&c, b"shot", &w(&["--", "--fit=2"])),
            Ok(w(&["--", "--fit=2"]))
        );
        let r = Ctx::new(b"run", b"/x".to_vec());
        assert_eq!(
            eq_split(&r, b"run", &w(&["--size=1x1", "ls", "--size=2"])),
            Ok(w(&["--size", "1x1", "ls", "--size=2"]))
        );
        let e = Ctx::new(b"events", b"/x".to_vec());
        assert_eq!(
            eq_split(
                &e,
                b"events",
                &w(&["--mark", "-n", "--mark", "m", "--mark=x"])
            ),
            Ok(w(&["--mark", "-n", "--mark", "m", "--mark", "x"]))
        );
        // (no table: the words as they came)
        assert_eq!(eq_split(&e, b"help", &w(&["--x=1"])), Ok(w(&["--x=1"])));
    }

    /// t_unit_exit_codes's --opt=value cases (audit F9), as bash's eq_split had them.
    #[test]
    fn eq_splits_as_bash() {
        let sp = |cmd: &str, a: &[&str]| {
            let c = Ctx::new(cmd.as_bytes(), b"/x".to_vec());
            let r = eq_split(&c, cmd.as_bytes(), &w(a)).unwrap();
            r.iter()
                .map(|x| format!("[{}]", String::from_utf8_lossy(x)))
                .collect::<String>()
        };
        assert_eq!(
            sp(
                "shot",
                &[
                    "--fit=300",
                    "--window=title:a=b",
                    "-g",
                    "--x=y",
                    "--after",
                    "--",
                    "--fit=3"
                ]
            ),
            "[--fit][300][--window][title:a=b][-g][--x=y][--after][--][--fit=3]"
        );
        assert_eq!(
            sp("log", &["--grep", "--x=y", "-n", "5"]),
            "[--grep][--x=y][-n][5]"
        );
        assert_eq!(
            sp("run", &["--size=800x600", "echo", "--size=1"]),
            "[--size][800x600][echo][--size=1]"
        );
        assert_eq!(
            sp("keys", &["-t", "--pass=x", "--pass=V"]),
            "[-t][--pass=x][--pass][V]"
        );
        assert_eq!(sp("events", &["--mark=m1"]), "[--mark][m1]");
        assert_eq!(
            sp("record", &["--cut-idle", "--fps=30"]),
            "[--cut-idle][--fps][30]"
        );
        assert_eq!(sp("shot", &["--zz=1"]), "[--zz=1]");
    }

    #[test]
    fn global_boxes() {
        let c = Ctx::new(b"", b"/nonexistent".to_vec());
        assert_eq!(
            global_box(&c, &w(&["-b", "a", "--box=b", "shot", "x"])),
            Ok(w(&["shot", "--box", "b", "x"]))
        );
        assert_eq!(
            global_box(&c, &w(&["-b", "a", "help", "x"])),
            Ok(w(&["help", "x"]))
        );
        assert_eq!(
            global_box(&c, &w(&["ls", "-b", "a"])),
            Ok(w(&["ls", "-b", "a"]))
        );
        assert_eq!(global_box(&c, &w(&["-b", "a", "ls"])), Err(Exit(2)));
        assert_eq!(global_box(&c, &w(&["-b", "a"])), Err(Exit(2)));
        assert_eq!(global_box(&c, &w(&["-b"])), Err(Exit(2)));
        assert_eq!(global_box(&c, &w(&["--box="])), Err(Exit(2)));
    }
}
