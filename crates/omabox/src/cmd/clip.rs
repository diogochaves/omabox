//! `omabox clip [-b NAME] [--from-box]` (issue #23, finding 119): the host's clipboard item into an
//! interactive box, or back with --from-box. One shot: nothing is left watching either clipboard,
//! only wl-copy serving the item it was given. Never for an agent: the guard keeps agents' shells
//! off the user's clipboard (finding 65), and clip must not be the way around it.

use crate::args::{optarg, unexpected, unknown_opt};
use crate::boxes::{BoxPid, box_alive, box_pid, list_names, meta, need_box};
use crate::cat;
use crate::exec::{ExecOpts, Redir, box_exec, dev_null, pipe, read_all};
use crate::exit::{Ctx, Exit, Res, say};
use crate::host::{Query, host_query};
use crate::jail::in_jail;
use crate::jq;
use crate::select::select_box;
use crate::shell;
use std::ffi::OsStr;
use std::fs::File;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::process::Stdio;

/// CLIP_MAX: bytes (a 4K screenshot as PNG is ~10 MB).
const CLIP_MAX: u64 = 64 * 1024 * 1024;
/// The text types an item may offer, best first; text goes across under the first.
const CLIP_TEXT: [&[u8]; 6] = [
    b"text/plain;charset=utf-8",
    b"text/plain;charset=UTF-8",
    b"UTF8_STRING",
    b"text/plain",
    b"STRING",
    b"TEXT",
];
const CLIP_TEXT_TYPE: &[u8] = b"text/plain;charset=utf-8";

/// agent_caller: what gives an agent away (a variable of its environment, a program), looked at in
/// this process and every one it runs under, up to PID 1 or the edge of its pid namespace: a
/// seatbelt, not a fence (finding 119).
fn agent_caller() -> Option<Vec<u8>> {
    if in_jail() || !shell::env_or_empty("OMABOX_BROKER_PEER").is_empty() {
        return Some(b"inside ai-jail".to_vec());
    }
    let marks = shell::Re::new(crate::jqprog::AGENT_MARKS)?;
    let me = std::process::id().to_string().into_bytes();
    let agent_re = shell::Re::new("claude-code|@openai/codex|opencode|pi-coding-agent|hermes")?;
    let mut p = me.clone();
    while shell::is_num(&p) && shell::test_int(&p).is_some_and(|n| n > 1) {
        let env = std::fs::read(shell::path(&cat!("/proc/", p, "/environ"))).unwrap_or_default();
        let kv = env
            .split(|&c| c == 0)
            .find(|r| marks.is_match(r))
            .map(<[u8]>::to_vec)
            .unwrap_or_default();
        // (this process named omabox: bash's was bash, its comm, finding 441)
        let comm = if p == me {
            b"omabox".to_vec()
        } else {
            crate::procfs::read(&p, "comm").unwrap_or_default()
        };
        if !kv.is_empty() {
            let k = kv.split(|&c| c == b'=').next().unwrap_or_default();
            return Some(cat!(k, " set for ", comm, ", pid ", p));
        }
        match comm.as_slice() {
            b"claude" | b"opencode" | b"pi" | b"hermes" | b"hermes-agent" | b"codex" => {
                return Some(cat!(comm, ", pid ", p));
            }
            c if c.starts_with(b"codex-") => return Some(cat!(comm, ", pid ", p)),
            c if (matches!(c, b"node" | b"bun" | b"deno") || c.starts_with(b"python"))
                && cmdline_has(&p, &agent_re) =>
            {
                return Some(cat!("an agent in ", comm, ", pid ", p));
            }
            _ => {}
        }
        match crate::procfs::proc_ppid(&p) {
            Some(pp) => p = pp,
            None => break,
        }
    }
    None
}

/// `grep -qzE RE /proc/PID/cmdline`.
fn cmdline_has(p: &[u8], re: &shell::Re) -> bool {
    std::fs::read(shell::path(&cat!("/proc/", p, "/cmdline")))
        .is_ok_and(|c| c.split(|&b| b == 0).any(|r| re.is_match(r)))
}

/// `grep -qxF -- T <<<"$TYPES"`: a whole line of TYPES.
fn has_line(types: &[u8], t: &[u8]) -> bool {
    types.split(|&c| c == b'\n').any(|l| l == t)
}

/// clip_pick TYPES: the type to hand over: text first, else an image (PNG first), by its own type.
fn clip_pick(types: &[u8]) -> Option<Vec<u8>> {
    for t in CLIP_TEXT {
        if has_line(types, t) {
            return Some(t.to_vec());
        }
    }
    if has_line(types, b"image/png") {
        return Some(b"image/png".to_vec());
    }
    let re = shell::Re::new("^image/[a-z0-9.+-]{1,64}$")?;
    types
        .split(|&c| c == b'\n')
        .find(|l| re.is_match(l))
        .map(<[u8]>::to_vec)
}

fn clip_is_text(t: &[u8]) -> bool {
    CLIP_TEXT.contains(&t)
}

/// clip_types TYPES: a box's list is the box's to write, so only a tame, short version for a message.
fn clip_types(types: &[u8]) -> Vec<u8> {
    // (`head -n 6 <<<"$1" | tr '\n' ' '`: each line read, its newline a space)
    let all = cat!(types, "\n");
    let kept: Vec<u8> = all
        .split_inclusive(|&c| c == b'\n')
        .take(6)
        .flatten()
        .map(|&c| if c == b'\n' { b' ' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || b" ;=/._+-".contains(c))
        .collect();
    crate::shell::cut_line(&kept, 120).to_vec()
}

/// clip_size BYTES.
fn clip_size(b: u64) -> Vec<u8> {
    if b < 1024 {
        return format!("{b} byte{}", if b == 1 { "" } else { "s" }).into_bytes();
    }
    let x = b as f64;
    if b < 1_048_576 {
        format!("{} KiB", crate::num::fixed(x / 1024.0, 1)).into_bytes()
    } else {
        format!("{} MiB", crate::num::fixed(x / 1_048_576.0, 1)).into_bytes()
    }
}

/// `tail -n 1 | tr -cd '[:print:]' | cut -c1-200` of what a tool said.
fn last_line_tame(v: &[u8]) -> Vec<u8> {
    let t = v.strip_suffix(b"\n").unwrap_or(v);
    let l = match t.iter().rposition(|&c| c == b'\n') {
        Some(i) => &t[i + 1..],
        None => t,
    };
    let kept: Vec<u8> = l
        .iter()
        .copied()
        .filter(|c| (0x20..0x7f).contains(c))
        .collect();
    kept[..kept.len().min(200)].to_vec()
}

/// A file of the runtime dir's, open read-write and unlinked at once (on its tmpfs, never on disk,
/// gone with this process): `mktemp -p "$XDG_RUNTIME_DIR" TEMPLATE`, `exec {FD}<>"$f"`, `rm -f`.
fn unlinked(template: &str) -> Option<File> {
    let rt = shell::env_or_empty("XDG_RUNTIME_DIR");
    let (f, p) = shell::mktemp_args(&[b"-p", &rt, template.as_bytes()]).ok()?;
    let _ = std::fs::remove_file(shell::path(&p));
    Some(f)
}

/// `env -u WAYLAND_SOCKET -u DISPLAY WAYLAND_DISPLAY="$HOST_WL" ARGS...` (host_wl): a Wayland client
/// on the user's session.
fn host_wl(wl: &[u8], args: &[&[u8]]) -> std::process::Command {
    let mut c = crate::sig::command("env");
    c.args(["-u", "WAYLAND_SOCKET", "-u", "DISPLAY"])
        .arg(OsStr::from_bytes(&cat!("WAYLAND_DISPLAY=", wl)))
        .args(args.iter().map(|a| OsStr::from_bytes(a)));
    c
}

/// A command's status as bash's `$?` (127 for one that could not start).
fn status(c: std::process::Command) -> i32 {
    crate::sig::run_status(c).unwrap_or(127)
}

/// `>"/dev/fd/N"`: the file open again for writing, emptied, from its start (a description of its
/// own, as bash's redirection opens one).
fn reopen_w(f: &File) -> Option<File> {
    std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(format!("/proc/self/fd/{}", f.as_raw_fd()))
        .ok()
}

/// `<"/dev/fd/N"`: the file open again for reading, from its start.
fn reopen_r(f: &File) -> Option<File> {
    File::open(format!("/proc/self/fd/{}", f.as_raw_fd())).ok()
}

/// `CMD 2>"/dev/fd/$E" | head -c N >"/dev/fd/$F"`: CMD's stdout into F (up to N bytes), its words
/// into E; pipefail's status.
fn into_file(mut c: std::process::Command, f: &File, e: &File, n: usize) -> i32 {
    let Ok((r, w)) = pipe() else {
        return 1;
    };
    c.stdout(Stdio::from(w));
    c.stderr(reopen_w(e).map_or(Stdio::null(), Stdio::from));
    let mut sink = reopen_w(f);
    let reader = std::thread::spawn(move || {
        let got = read_all(r, Some(n));
        if let Some(s) = sink.as_mut() {
            let _ = std::io::Write::write_all(s, &got);
        }
    });
    let rc = crate::sig::run_status(c).unwrap_or(127);
    let _ = reader.join();
    rc
}

/// `box_sys ARGV... 2>/dev/null {CLIP_FD}>&- {CLIP_EFD}>&- | head -c N`: its stdout (up to N bytes)
/// and pipefail's status; its refusal said nowhere and its failure.
fn box_sys_capped(
    ctx: &mut Ctx,
    argv: &[Vec<u8>],
    n: usize,
    into: Option<&File>,
) -> (i32, Vec<u8>) {
    let Ok((r, w)) = pipe() else {
        return (1, Vec::new());
    };
    let reader = std::thread::spawn(move || read_all(r, Some(n)));
    let rc = {
        let _e = Redir::null(2);
        box_exec(
            ctx,
            argv,
            ExecOpts {
                sys: true,
                stdout: Some(w.as_raw_fd()),
                ..ExecOpts::default()
            },
        )
    };
    drop(w);
    let out = reader.join().unwrap_or_default();
    if let Some(mut w) = into.and_then(reopen_w) {
        let _ = std::io::Write::write_all(&mut w, &out);
    }
    match rc {
        Ok(rc) => (rc, out),
        Err(Exit(c)) => (i32::from(c), out),
    }
}

/// clip_select NAME: the box clip works with: -b NAME's; else the interactive box whose window has
/// the host's focus (the one a key binding is pressed in); else the only interactive box up.
fn clip_select(ctx: &mut Ctx, name: &[u8]) -> Res<()> {
    if !name.is_empty() {
        select_box(ctx, name)?;
        return need_box(ctx);
    }
    let (rc, aw) = host_query(ctx, Query::ActiveWindow, true);
    let win = if rc == 0 {
        let r = jq::tool("jq", &["-r", ".pid // empty"], Some(&aw), jq::Err::Null);
        if r.ok() { r.text() } else { Vec::new() }
    } else {
        Vec::new()
    };
    let mut boxes: Vec<Vec<u8>> = Vec::new();
    for n in list_names(ctx, false) {
        select_box(ctx, &n)?;
        if !box_alive(ctx)? || meta(&ctx.d, "mode").0 != b"interactive" {
            continue;
        }
        boxes.push(n.clone());
        // Its window is the outer bwrap's (omabox-wlfd execs it), as peek --focus finds it.
        let pid = match box_pid(&ctx.d) {
            BoxPid::Pid(p) if !p.is_empty() => crate::procfs::proc_ppid(&p).unwrap_or_default(),
            _ => Vec::new(),
        };
        if !win.is_empty() && pid == win {
            return need_box(ctx);
        }
    }
    match boxes.len() {
        0 => Err(ctx.die("clip: no interactive box is up (clip is for interactive boxes only)")),
        1 => {
            select_box(ctx, &boxes[0])?;
            need_box(ctx)
        }
        _ => Err(ctx.die(cat!(
            "clip: interactive boxes ",
            boxes.join(&b' '),
            " are up and none has focus: which one? -b NAME"
        ))),
    }
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    if let Some(why) = agent_caller() {
        ctx.parsed();
        return Err(ctx.die(cat!(
            "clip is not for agents (",
            why,
            "): the user hands their clipboard over themselves, from a terminal of theirs, a key binding or the bar widget"
        )));
    }
    let (mut name, mut from) = (Vec::new(), false);
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, args.get(i + 1))?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"--from-box" => from = true,
            b"-h" | b"--help" => return crate::args::usage("clip"),
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "clip", a)),
            _ => {
                return Err(unexpected(
                    ctx,
                    "clip",
                    a,
                    "(omabox clip [--from-box] [-b NAME])",
                ));
            }
        }
        i += 1;
    }
    ctx.parsed();
    let (_, wl) = crate::host::host_session_die(ctx)?;
    clip_select(ctx, &name)?;
    let n = ctx.name.clone();
    if meta(&ctx.d, "mode").0 != b"interactive" {
        return Err(ctx.die(cat!(
            "clip: box '",
            n,
            "' is headless: clip is for interactive boxes only (headless ones are agents')"
        )));
    }
    // The item, and what the tools say, in files unlinked at once: on the runtime dir's tmpfs, never
    // on disk, gone with this process whatever ends it.
    let Some(f) = unlinked("omabox-clip-item.XXXXXX") else {
        return Err(ctx.die("clip: no temporary file"));
    };
    let Some(e) = unlinked("omabox-clip-err.XXXXXX") else {
        return Err(ctx.die("clip: no temporary file"));
    };
    let err_all = |e: &File| -> Vec<u8> {
        let mut v = Vec::new();
        if let Some(mut r) = reopen_r(e) {
            let _ = std::io::Read::read_to_end(&mut r, &mut v);
        }
        v
    };
    let err_text = |e: &File| last_line_tame(&err_all(e));
    let types;
    let t;
    let rc = if !from {
        let mut c = host_wl(&wl, &[b"timeout", b"10", b"wl-paste", b"--list-types"]);
        c.stdin(Stdio::inherit())
            .stderr(reopen_w(&e).map_or(Stdio::null(), Stdio::from));
        // (`types=$(host_wl ... wl-paste --list-types ...)`)
        let o = crate::sig::run_output(c);
        let (r, out) = o.unwrap_or((127, Vec::new()));
        types = shell::subst(&out);
        if r != 0 {
            if crate::exit::contains(&err_all(&e), b"Nothing is copied") {
                return Err(ctx.die("clip: your clipboard is empty: nothing handed over"));
            }
            return Err(ctx.die(cat!("clip: cannot read your clipboard: ", err_text(&e))));
        }
        let Some(p) = clip_pick(&types) else {
            return Err(ctx.die(cat!(
                "clip: your clipboard holds ",
                clip_types(&types),
                ": only text and images go into a box"
            )));
        };
        t = p;
        let c = host_wl(&wl, &[b"timeout", b"10", b"wl-paste", b"-n", b"--type", &t]);
        into_file(c, &f, &e, (CLIP_MAX + 1) as usize)
    } else {
        let (r, out) = box_sys_capped(
            ctx,
            &[
                b"/usr/bin/bash".to_vec(),
                b"-c".to_vec(),
                crate::jqprog::CLIP_BOX_TYPES.as_bytes().to_vec(),
            ],
            65536,
            None,
        );
        types = shell::subst(&out);
        if r == 3 {
            return Err(ctx.die(cat!(
                "clip: box '",
                n,
                "''s clipboard is empty: nothing handed over"
            )));
        }
        if r != 0 {
            return Err(ctx.die(cat!(
                "clip: cannot read box '",
                n,
                "''s clipboard (exit ",
                r.to_string(),
                ")"
            )));
        }
        let Some(p) = clip_pick(&types) else {
            return Err(ctx.die(cat!(
                "clip: box '",
                n,
                "''s clipboard holds ",
                clip_types(&types),
                ": only text and images come out"
            )));
        };
        t = p;
        let (r, _) = box_sys_capped(
            ctx,
            &[
                b"/usr/bin/timeout".to_vec(),
                b"10".to_vec(),
                b"/usr/bin/wl-paste".to_vec(),
                b"-n".to_vec(),
                b"--type".to_vec(),
                t.clone(),
            ],
            (CLIP_MAX + 1) as usize,
            Some(&f),
        );
        r
    };
    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
    if size > CLIP_MAX {
        return Err(ctx.die(cat!(
            "clip: the item is over ",
            clip_size(CLIP_MAX),
            ": nothing handed over"
        )));
    }
    if rc != 0 {
        let why = if rc == 124 {
            b"no answer in 10 s".to_vec()
        } else {
            cat!("exit ", rc.to_string())
        };
        return Err(ctx.die(cat!("clip: cannot read the item (", t, "): ", why)));
    }
    if size == 0 {
        return Err(ctx.die("clip: the item is empty: nothing handed over"));
    }
    let hand = if clip_is_text(&t) {
        CLIP_TEXT_TYPE.to_vec()
    } else {
        t.clone()
    };
    // A password manager's hint that the item is a secret goes along (wl-copy --sensitive offers
    // it again): clipboard histories, Omarchy's among them, then leave it out.
    let sens = has_line(&types, b"x-kde-passwordManagerHint");
    let what = cat!(
        if clip_is_text(&t) { "text" } else { "an image" },
        " (",
        hand,
        ", ",
        clip_size(size),
        if sens { ", marked sensitive" } else { "" },
        ")"
    );
    if !from {
        let mut argv: Vec<Vec<u8>> = vec![
            b"/usr/bin/bash".to_vec(),
            b"-c".to_vec(),
            crate::jqprog::CLIP_BOX_COPY.as_bytes().to_vec(),
            b"clip".to_vec(),
            hand.clone(),
        ];
        if sens {
            argv.push(b"--sensitive".to_vec());
        }
        // (`err=$(box_sys ... <"/dev/fd/$CLIP_FD" 2>&1 >/dev/null)`: its words, box_sys's own too)
        let (said, ok) = {
            let Ok((r, w)) = pipe() else {
                return Err(ctx.die("clip: could not put it on box's clipboard"));
            };
            let reader = std::thread::spawn(move || read_all(r, None));
            let item: Option<OwnedFd> = reopen_r(&f).map(OwnedFd::from);
            let null = dev_null(true).ok();
            let rc = {
                let _i = item.as_ref().map(|i| Redir::to(0, i.as_raw_fd()));
                let _e = Redir::to(2, w.as_raw_fd());
                box_exec(
                    ctx,
                    &argv,
                    ExecOpts {
                        sys: true,
                        stdout: null.as_ref().map(AsRawFd::as_raw_fd),
                        ..ExecOpts::default()
                    },
                )
            };
            drop(w);
            (reader.join().unwrap_or_default(), matches!(rc, Ok(0)))
        };
        if !ok {
            return Err(ctx.die(cat!(
                "clip: could not put it on box '",
                n,
                "''s clipboard: ",
                last_line_tame(&shell::subst(&said))
            )));
        }
        say(cat!(
            "handed ",
            what,
            " from your clipboard to box '",
            n,
            "'"
        ));
    } else {
        let mut a: Vec<&[u8]> = vec![
            b"timeout", b"10", b"setsid", b"-w", b"wl-copy", b"--type", &hand,
        ];
        if sens {
            a.push(b"--sensitive");
        }
        let mut c = host_wl(&wl, &a);
        c.stdin(reopen_r(&f).map_or(Stdio::null(), Stdio::from))
            .stdout(Stdio::null())
            .stderr(reopen_w(&e).map_or(Stdio::null(), Stdio::from));
        if status(c) != 0 {
            return Err(ctx.die(cat!(
                "clip: could not put it on your clipboard: ",
                err_text(&e)
            )));
        }
        say(cat!(
            "handed ",
            what,
            " from box '",
            n,
            "' to your clipboard"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_as_bash() {
        assert_eq!(
            clip_pick(b"image/png\ntext/html\ntext/plain\ntext/plain;charset=utf-8"),
            Some(b"text/plain;charset=utf-8".to_vec())
        );
        assert_eq!(
            clip_pick(b"STRING\ntext/plain\nUTF8_STRING"),
            Some(b"UTF8_STRING".to_vec())
        );
        assert_eq!(
            clip_pick(b"image/jpeg\nimage/png\ntext/html"),
            Some(b"image/png".to_vec())
        );
        assert_eq!(
            clip_pick(b"text/html\nimage/webp"),
            Some(b"image/webp".to_vec())
        );
        assert_eq!(
            clip_pick(b"text/html\ntext/uri-list\nx-special/gnome-copied-files"),
            None
        );
        assert_eq!(clip_pick(b""), None);
        assert_eq!(clip_pick(b"image/png;x=\x1b[31m\nimage/../x"), None);
        assert_eq!(
            clip_types(b"image/png;x=\x1b[31m\ntext/html"),
            b"image/png;x=31m text/html "
        );
        assert_eq!(clip_size(1), b"1 byte");
        assert_eq!(clip_size(12), b"12 bytes");
        assert_eq!(clip_size(1536), b"1.5 KiB");
        assert_eq!(clip_size(64 << 20), b"64.0 MiB");
        assert_eq!(last_line_tame(b"a\nb\x01c\n"), b"bc");
    }
}
