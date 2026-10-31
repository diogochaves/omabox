//! `omabox tree [-b NAME] [--window SEL | --layer SEL] [--find TEXT] [--json] [--all]` (#178,
//! finding 317): a window's accessibility tree as text, read by omabox-tree in the box (its session
//! bus, its pids), in the coordinates `click --window` takes; --layer SEL a layer surface's (#254),
//! in the screen's.

use crate::args::{optarg, unexpected, unknown_opt};
use crate::boxes::{box_alive, need_box};
use crate::cat;
use crate::exec::{ErrTo, Out, Redir, on_box};
use crate::exit::{Ctx, Exit, Res, say};
use crate::hypr::hypr_json;
use crate::jq::{self, Args};
use crate::layer::{
    BOX_TREE, box_sys_err, layer_desc, layer_lines, layer_qs_note, layer_read, layer_select,
    layer_tied,
};
use crate::select::select_box;
use crate::shell::{self, strip_nl};
use crate::shellnote::shell_note;
use crate::win::{box_windows, jq_length, jq_r, win_desc, win_select};
use std::io::{Read, Seek};
use std::os::fd::AsRawFd;

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut sels, mut layer, mut find) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut json, mut all) = (false, false);
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let v = args.get(i + 1);
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, v)?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"-w" | b"--window" => {
                optarg(ctx, a, v)?;
                sels.push(args[i + 1].clone());
                i += 1;
            }
            b"--active" => sels.push(b"active".to_vec()),
            b"-l" | b"--layer" => {
                optarg(ctx, a, v)?;
                layer = args[i + 1].clone();
                i += 1;
            }
            b"--find" => {
                optarg(ctx, a, v)?;
                find = args[i + 1].clone();
                i += 1;
            }
            b"--json" => json = true,
            b"--all" => all = true,
            b"-h" | b"--help" => return crate::args::usage("tree"),
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "tree", a)),
            _ => {
                return Err(unexpected(
                    ctx,
                    "tree",
                    a,
                    "(the window is --window SEL, a layer --layer SEL; text to look for, --find TEXT)",
                ));
            }
        }
        i += 1;
    }
    if !layer.is_empty() && !sels.is_empty() {
        return Err(ctx.usage_die("tree: --window or --layer, not both"));
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    shell_note(ctx)?;
    let mut targs: Vec<Vec<u8>> = Vec::new();
    if !find.is_empty() {
        targs.extend([b"--find".to_vec(), find]);
    }
    if json {
        targs.push(b"--json".to_vec());
    }
    if all {
        targs.push(b"--all".to_vec());
    }
    let r = read(ctx, "tree", &sels, &layer, targs, false)?;
    match r.rc {
        0 => {
            if !r.why.is_empty() {
                say(cat!("tree: ", r.why));
            }
            Ok(())
        }
        1 => {
            say(cat!("tree: ", r.why));
            Err(Exit(1))
        }
        _ => fail(ctx, "tree", &r),
    }
}

/// What `read` read: omabox-tree's status, its words (stderr, each line's prefix off) and, when
/// captured, its output; the window or layer it read, as said in a message, and the window (its
/// box_windows object; empty for a layer).
pub struct Reading {
    pub rc: i32,
    pub why: Vec<u8>,
    pub out: Vec<u8>,
    pub what: Vec<u8>,
    pub win: Vec<u8>,
    layered: bool,
    l: Vec<u8>,
    lys: Vec<u8>,
}

/// omabox-tree with TARGS on the window SELS picks (none: the active one) or the layer LAYER: its
/// output on stdout, or in Read's `out` when CAPTURE. CMD names the command in what it says.
pub fn read(
    ctx: &mut Ctx,
    cmd: &str,
    sels: &[Vec<u8>],
    layer: &[u8],
    targs: Vec<Vec<u8>>,
    capture: bool,
) -> Res<Reading> {
    let t = on_box(
        ctx,
        &[
            b"test".to_vec(),
            b"-x".to_vec(),
            BOX_TREE.as_bytes().to_vec(),
        ],
        false,
        Out::Inherit,
        ErrTo::Null,
    )
    .or_exit()?;
    if t.rc != 0 {
        return Err(ctx.die(cat!(
            cmd,
            ": box '",
            ctx.name,
            "' has no ",
            BOX_TREE,
            " (started by an omabox without it): omabox down ",
            ctx.name,
            ", then up again"
        )));
    }
    let (what, rc, mut err, path, l, lys, win, out);
    if !layer.is_empty() {
        lys = strip_nl(&hypr_json(ctx, &["layers"])?);
        l = layer_select(ctx, &lys, layer)?;
        what = cat!("layer ", layer_desc(&l));
        win = Vec::new();
        let (f, p) = shell::mktemp()?;
        (rc, out) = captured(capture, || layer_read(ctx, &l, &lys, &f, &targs))?;
        (err, path) = (f, p);
    } else {
        (l, lys) = (Vec::new(), Vec::new());
        let wins = box_windows(ctx, None)?;
        if sels.is_empty() && jq_length(&wins) == b"0" {
            say(cat!(
                cmd,
                ": the box has no window; the shell's bar, panels and menus are layers: ",
                cmd,
                " --layer NAMESPACE (omabox hyprctl layers)"
            ));
            return Err(Exit(2));
        }
        let active = [b"active".to_vec()];
        let w = win_select(ctx, &wins, if sels.is_empty() { &active } else { sels })?;
        let pid = jq_r(".pid", &w);
        let title = jq_r(".title", &w);
        let size = jq_r(crate::jqprog::WIN_SIZE_JQ, &w);
        what = win_desc(&w);
        if !shell::re_match("^[1-9][0-9]*$", &pid) {
            return Err(ctx.die(cat!(
                cmd,
                ": window ",
                what,
                " has no pid to find its app by (",
                pid,
                ")"
            )));
        }
        // Its app's other windows of its size: its only frame stands for it only when there are
        // none (#302).
        let twins = Args::new(&[])
            .kv("--argjson", "w", &w)
            .arg(crate::jqprog::TREE_TWINS_JQ)
            .run(Some(&wins), jq::Err::Inherit)
            .text();
        let (f, p) = shell::mktemp()?;
        let mut argv: Vec<Vec<u8>> = vec![
            b"/usr/bin/timeout".to_vec(),
            b"30".to_vec(),
            BOX_TREE.as_bytes().to_vec(),
            b"--pid".to_vec(),
            pid,
            b"--title".to_vec(),
            title,
            b"--size".to_vec(),
            size,
            b"--twins".to_vec(),
            twins,
        ];
        argv.extend(targs);
        (rc, out) = captured(capture, || box_sys_err(ctx, &argv, f.as_raw_fd()))?;
        (err, path, win) = (f, p, w);
    }
    // `why=$(sed 's/^omabox-tree: //' "$err"); rm -f "$err"`
    let mut text = Vec::new();
    let _ = err.rewind();
    let _ = err.read_to_end(&mut text);
    let why = tool_words(&text);
    let _ = std::fs::remove_file(shell::path(&path));
    Ok(Reading {
        rc,
        why,
        out,
        what,
        win,
        layered: !layer.is_empty(),
        l,
        lys,
    })
}

/// F's result, and what it wrote on stdout when CAPTURE (else it goes to ours).
fn captured(capture: bool, f: impl FnOnce() -> Res<i32>) -> Res<(i32, Vec<u8>)> {
    if !capture {
        return Ok((f()?, Vec::new()));
    }
    let (mut file, p) = shell::mktemp()?;
    let rc = {
        let _o = Redir::to(1, file.as_raw_fd());
        f()
    };
    let mut out = Vec::new();
    let _ = file.rewind();
    let _ = file.read_to_end(&mut out);
    let _ = std::fs::remove_file(shell::path(&p));
    Ok((rc?, out))
}

/// What a read that failed (R's status, not 0 or 1) says: CMD's error, its exit code.
pub fn fail(ctx: &mut Ctx, cmd: &str, r: &Reading) -> Res<()> {
    let (n, what, why) = (ctx.name.clone(), &r.what, &r.why);
    match r.rc {
        3 => {
            say(cat!(cmd, ": ", what, ": ", why));
            if r.layered {
                layer_qs_note(ctx, &r.l);
            } else {
                say(
                    "GTK, Qt and egui/winit (AccessKit) apps have one; Chromium and Electron only when started with --force-renderer-accessibility; terminals (foot) have none: look with a shot (shot --window SEL). The shell's bar, panels and menus are layers: tree --layer NAMESPACE",
                );
            }
            Err(Exit(1))
        }
        5 | 7 => {
            say(cat!(cmd, ": ", what, ": ", why));
            Err(Exit(1))
        }
        6 => {
            if r.layered {
                say(cat!(
                    cmd,
                    ": ",
                    what,
                    ": ",
                    why,
                    "; the layers of that pid and size:"
                ));
                layer_lines(&layer_tied(&r.l, &r.lys));
            } else {
                say(cat!(cmd, ": ", what, ": ", why));
            }
            Err(Exit(1))
        }
        4 => {
            let w: &[u8] = if why.is_empty() {
                b"no accessibility bus"
            } else {
                why
            };
            Err(ctx.die(cat!(
                cmd,
                ": ",
                w,
                " (a box started by an omabox before 0.5.3, or with --env ATSPI_DBUS_IMPLEMENTATION=dbus-broker: omabox down ",
                n,
                ", then up again)"
            )))
        }
        124 => Err(ctx.die(cat!(
            cmd,
            ": ",
            what,
            " did not answer in 30 s (busy or hung?)"
        ))),
        _ => {
            if !box_alive(ctx)? {
                return Err(ctx.die(cat!(cmd, ": box '", n, "' went down")));
            }
            let w = if why.is_empty() {
                Vec::new()
            } else {
                cat!(": ", why)
            };
            Err(ctx.die(cat!(
                cmd,
                ": reading the tree failed",
                w,
                " (status ",
                r.rc.to_string(),
                ")"
            )))
        }
    }
}

/// `$(sed 's/^omabox-tree: //' FILE)`: the tool's words, each line's own prefix off.
fn tool_words(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut lines: Vec<&[u8]> = text.split(|&c| c == b'\n').collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    for l in lines {
        out.extend_from_slice(l.strip_prefix(b"omabox-tree: ").unwrap_or(l));
        out.push(b'\n');
    }
    shell::subst(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_prefix_off_each_line() {
        assert_eq!(
            tool_words(b"omabox-tree: a\nb omabox-tree: c\nomabox-tree: d\n\n"),
            b"a\nb omabox-tree: c\nd".to_vec()
        );
    }
}
