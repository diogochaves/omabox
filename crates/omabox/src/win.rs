//! The box's windows as bin/omabox reads them (finding 81): Hyprland's lists annotated by WIN_JQ
//! (box_windows, win_annotate), the one window a selector picks (win_select, #20, #37, #244), and
//! their words in messages (win_desc, win_lines). jq runs the programs, so the answers are jq's.

use crate::cat;
use crate::exec::{ErrTo, Out, on_box};
use crate::exit::{Ctx, Exit, Res, say};
use crate::hypr::{HYPR_LIMIT, hypr_json};
use crate::jq::{self, Args};
use crate::jqprog::{WIN_JQ, WIN_SEL_JQ};
use crate::settle::{after, past};
use crate::shell::{self, strip_nl};
use std::io::Write;

/// win_annotate CLIENTS MONITORS ACTIVEWINDOW [LAYERS]: WIN_JQ's list (its status and output as
/// jq's; its errors on this process's stderr).
pub fn win_annotate(c: &[u8], m: &[u8], a: &[u8], l: Option<&[u8]>) -> jq::Ran {
    Args::new(&["-c", "-n"])
        .kv("--argjson", "c", c)
        .kv("--argjson", "m", m)
        .kv("--argjson", "a", a)
        .kv("--argjson", "l", l.unwrap_or(b"null"))
        .arg(WIN_JQ)
        .run(None, jq::Err::Inherit)
}

/// box_windows [LAYERS]: one question at a time, the first left unanswered the end (hypr_json, its
/// die); then win_annotate, whose status is the function's (`wins=$(box_windows) || exit $?`).
pub fn box_windows(ctx: &Ctx, layers: Option<&[u8]>) -> Res<Vec<u8>> {
    let c = strip_nl(&hypr_json(ctx, &["clients"])?);
    let m = strip_nl(&hypr_json(ctx, &["monitors"])?);
    let a = strip_nl(&hypr_json(ctx, &["activewindow"])?);
    let r = win_annotate(&c, &m, &a, layers.filter(|l| !l.is_empty()));
    if !r.ok() {
        return Err(Exit(u8::try_from(r.rc).unwrap_or(1)));
    }
    Ok(r.text())
}

/// `${hits#jq: error*: }`: jq's error without its own lead ("jq: error (at <stdin>:1): ").
pub fn jq_error_text(hits: &[u8]) -> &[u8] {
    const LEAD: &[u8] = b"jq: error";
    if !hits.starts_with(LEAD) {
        return hits;
    }
    match hits[LEAD.len()..].windows(2).position(|w| w == b": ") {
        Some(i) => &hits[LEAD.len() + i + 2..],
        None => hits,
    }
}

/// `$*` of the selectors.
pub fn joined(sels: &[Vec<u8>]) -> Vec<u8> {
    sels.join(&b' ')
}

/// WIN_SEL_JQ's matches of SELS among WINS, as `hits=$(jq ... 2>&1)`: Ok(hits), or Err(jq's words)
/// for a selector jq cannot read (a bad regex).
pub fn win_hits(wins: &[u8], sels: &[Vec<u8>]) -> Result<Vec<u8>, Vec<u8>> {
    let mut a = Args::new(&["-c"]).arg(WIN_SEL_JQ).arg("--args");
    for s in sels {
        a = a.arg(s);
    }
    let r = a.run(Some(wins), jq::Err::Out);
    if r.ok() { Ok(r.text()) } else { Err(r.text()) }
}

/// `jq length <<<"$X"`.
pub fn jq_length(x: &[u8]) -> Vec<u8> {
    jq::jq(&["length"], Some(x), jq::Err::Inherit).text()
}

/// win_select WINDOWS SEL...: the one window (win_annotate's object); none or several are exit 2,
/// with the candidates.
pub fn win_select(ctx: &Ctx, wins: &[u8], sels: &[Vec<u8>]) -> Res<Vec<u8>> {
    if sels.is_empty() {
        let c: &[u8] = if ctx.main_cmd.is_empty() {
            b"omabox"
        } else {
            &ctx.main_cmd
        };
        return Err(ctx.die(cat!(c, ": --window SEL: no selector given")));
    }
    // No list at all (not an empty one): the box did not answer (finding 176).
    if wins.is_empty() {
        return Err(ctx.die(cat!(
            "box '",
            ctx.name,
            "' did not list its windows (is it going down?)"
        )));
    }
    let hits = match win_hits(wins, sels) {
        Ok(h) => h,
        Err(e) => {
            say(cat!(
                "bad window selector (",
                joined(sels),
                "): ",
                jq_error_text(&e)
            ));
            return Err(Exit(2));
        }
    };
    let n = jq_length(&hits);
    match n.as_slice() {
        b"1" => return Ok(jq::jq(&["-c", ".[0]"], Some(&hits), jq::Err::Inherit).text()),
        b"0" => {
            say(cat!(
                "no window matches ",
                joined(sels),
                "; the box's windows:"
            ));
            win_lines(wins);
        }
        _ => {
            say(cat!(
                joined(sels),
                " matches ",
                n,
                " windows: name one by its address (or add a selector):"
            ));
            win_lines(&hits);
        }
    }
    Err(Exit(2))
}

/// win_lines LIST >&2: a window a line, for a refusal.
pub fn win_lines(list: &[u8]) {
    let r = jq::jq(
        &["-r", crate::jqprog::WIN_LINES_JQ],
        Some(list),
        jq::Err::Inherit,
    );
    let _ = std::io::stderr().write_all(&r.out);
}

/// win_desc WINDOW: `0x... class "title"`, for messages.
pub fn win_desc(w: &[u8]) -> Vec<u8> {
    jq::jq(
        &["-r", crate::jqprog::WIN_DESC_ONE_JQ],
        Some(w),
        jq::Err::Inherit,
    )
    .text()
}

/// `jq -r PROGRAM <<<"$X"` as `$(...)`.
pub fn jq_r(prog: &str, x: &[u8]) -> Vec<u8> {
    jq::jq(&["-r", prog], Some(x), jq::Err::Inherit).text()
}

/// `out=$(on_box ARGS... 2>&1) || true`.
fn said(ctx: &Ctx, args: &[&[u8]]) -> Vec<u8> {
    let a: Vec<Vec<u8>> = args.iter().map(|a| a.to_vec()).collect();
    let r = on_box(ctx, &a, false, Out::Capture(None), ErrTo::Out);
    shell::subst(&r.out)
}

/// A window's address, checked as one: it goes into Hyprland's Lua.
fn address(ctx: &Ctx, w: &[u8]) -> Res<Vec<u8>> {
    let addr = jq_r(".address", w);
    if !shell::re_match("^0x[0-9a-f]+$", &addr) {
        return Err(ctx.die(cat!("bad window address: ", addr)));
    }
    Ok(addr)
}

/// win_raise WINDOW: focused as a user would focus it (its workspace to the screen, a group's tab to
/// the front, a floating one raised; a tiled one stays under the floats), until it is the active
/// window, 3 s at most. Focused again if it had the focus and lost it at once: Hyprland warps the
/// pointer to a window it focuses on another workspace, and its follow-the-mouse focus, a moment on,
/// takes the window under that point by where windows are drawn, so one moved while its workspace
/// was hidden, not yet drawn there, lost the focus to the window it had been over (t_window's click
/// --in after a move, finding 516). Focused again a quarter second on, the window is drawn where
/// the pointer is, and the focus stays. Only for a focus it had and lost (finding 518): a window
/// that never takes the focus is focused once, as by bash, and none once it has it (it returns then).
pub fn win_raise(ctx: &Ctx, w: &[u8]) -> Res<()> {
    let addr = address(ctx, w)?;
    // (audit E12: which window, and the way round: --no-raise acts on it where it is)
    let way = "--no-raise acts on it where it is, if uncovered; shot --window takes it covered";
    let focus = || -> Res<()> {
        let out = said(
            ctx,
            &[
                b"hyprctl",
                b"dispatch",
                &cat!("hl.dsp.focus({ window = 'address:", addr, "' })"),
            ],
        );
        if out != b"ok" {
            return Err(ctx.die(cat!(
                ctx.label(),
                "could not focus window ",
                win_desc(w),
                ": ",
                out,
                " (",
                way,
                ")"
            )));
        }
        if jq_r(".floating", w) == b"true" {
            let out = said(
                ctx,
                &[
                    b"hyprctl",
                    b"dispatch",
                    &cat!(
                        "hl.dsp.window.alter_zorder({ mode = 'top', window = 'address:",
                        addr,
                        "' })"
                    ),
                ],
            );
            if out != b"ok" {
                return Err(ctx.die(cat!(
                    ctx.label(),
                    "could not raise window ",
                    win_desc(w),
                    ": ",
                    out,
                    " (",
                    way,
                    ")"
                )));
            }
        }
        Ok(())
    };
    focus()?;
    let deadline = after(3);
    let gap = std::time::Duration::from_millis(250);
    let mut next = std::time::Instant::now() + gap;
    loop {
        let a = strip_nl(&hypr_json(ctx, &["activewindow"])?);
        let active = jq_r(crate::jqprog::ACTIVE_ADDR_JQ, &a);
        if active == addr {
            return Ok(());
        }
        // Had it and lost it: the window focused just before the one that has it now (Hyprland's
        // focusHistoryID 1). A quarter second on, so the window is drawn where it is.
        if !active.is_empty() && std::time::Instant::now() >= next {
            let c = hypr_json(ctx, &["clients"])?;
            let prog = format!(
                r#"[.[] | select(.address == "{}") | .focusHistoryID] | .[0] // empty"#,
                String::from_utf8_lossy(&addr)
            );
            if jq_r(&prog, &c) == b"1" {
                focus()?;
                next = std::time::Instant::now() + gap;
                continue;
            }
        }
        if past(deadline) {
            return Err(ctx.die(cat!(
                ctx.label(),
                "window ",
                win_desc(w),
                " did not take focus in 3 s: something holds it (a layer, a fullscreen window: omabox windows; ",
                way,
                ")"
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// win_refocus WINDOW: the active window focused again, the pointer left where it is (keys --window,
/// #201): Hyprland shows a window active while the keyboard is elsewhere; a focus gives it back. Its
/// warp is held off.
pub fn win_refocus(ctx: &Ctx, w: &[u8]) -> Res<()> {
    let addr = address(ctx, w)?;
    let lua = cat!(
        "local nw = hl.get_config('cursor.no_warps')\nhl.config({ cursor = { no_warps = true } })\nlocal ok, err = pcall(hl.dispatch, hl.dsp.focus({ window = 'address:",
        addr,
        "' }))\nhl.config({ cursor = { no_warps = nw } })\nif not ok then error(err, 0) end"
    );
    let out = said(
        ctx,
        &[b"timeout", HYPR_LIMIT.as_bytes(), b"hyprctl", b"eval", &lua],
    );
    if out != b"ok" {
        return Err(ctx.die(cat!(
            ctx.label(),
            "could not focus window ",
            win_desc(w),
            " again: ",
            out,
            " (omabox windows; keys without --window types into what has the keyboard)"
        )));
    }
    Ok(())
}

/// `jq -r --argjson x X --argjson y Y PROGRAM <<<"$W"` as `$(...)`.
pub fn jq_xy(prog: &str, w: &[u8], x: &[u8], y: &[u8]) -> jq::Ran {
    Args::new(&["-r"])
        .kv("--argjson", "x", x)
        .kv("--argjson", "y", y)
        .arg(prog)
        .run(Some(w), jq::Err::Inherit)
}

/// How win_point treats what is over the point: click and scroll raise the window (1) or not (0,
/// --no-raise); pointer never raises it, refuses a window over it and only says a layer.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Raise {
    Yes,
    No,
    Pointer,
}

/// win_point RAISE X Y SEL...: the screen point "SX SY" for window coordinates X,Y of the window
/// SEL picks, raised first when it is not on screen or something covers the point (unless RAISE
/// says not); a point still covered is refused, naming what covers it. A layer surface over the
/// point (#263) is said, and refused under --no-raise; one spanning its output is narrowed by where
/// it draws (#283, finding 618): on one of its items as certain, nowhere near one not at all, and
/// where the pixels cannot tell as a may, never refused (findings 398, 421).
pub fn win_point(ctx: &Ctx, raise: Raise, x: &[u8], y: &[u8], sels: &[Vec<u8>]) -> Res<Vec<u8>> {
    // The layers read once: raising a window changes none of them.
    // (a dock hidden on its workspace taken out: it takes no input there, #309)
    let lys = crate::omshell::input_layers(ctx)?;
    let wins = box_windows(ctx, Some(&lys))?;
    let mut w = win_select(ctx, &wins, sels)?;
    let desc = win_desc(&w);
    let inside = Args::new(&["-e"])
        .kv("--argjson", "x", x)
        .kv("--argjson", "y", y)
        .arg(crate::jqprog::WIN_INSIDE_JQ)
        .run(Some(&w), jq::Err::Inherit);
    if !inside.ok() {
        return Err(ctx.die(cat!(
            x,
            ",",
            y,
            " is outside window ",
            desc,
            " (",
            jq_r(crate::jqprog::WIN_SIZE_JQ, &w),
            "; coordinates are the window's own)"
        )));
    }
    let mut why = jq_xy(crate::jqprog::WIN_BLOCKED_JQ, &w, x, y).text();
    if !why.is_empty() {
        if raise != Raise::Yes {
            return Err(ctx.die(cat!(
                "window ",
                desc,
                " is ",
                why,
                " (click --window raises it; --no-raise and pointer do not)"
            )));
        }
        win_raise(ctx, &w)?;
        let wins = box_windows(ctx, Some(&lys))?;
        w = win_select(ctx, &wins, &[jq_r(".address", &w)])?;
        why = jq_xy(crate::jqprog::WIN_BLOCKED_JQ, &w, x, y).text();
        if !why.is_empty() {
            return Err(ctx.die(cat!(
                "window ",
                desc,
                " is still ",
                why,
                " after focusing it: move or close that window first"
            )));
        }
    }
    let mut lay = jq_xy(crate::jqprog::WIN_LAYER_JQ, &w, x, y).text();
    if lay.starts_with(b"possible ") {
        let at = jq_xy(crate::jqprog::WIN_AT_JQ, &w, x, y).text();
        lay = narrow(ctx, &w, lay, &at);
    }
    if !lay.is_empty() {
        let (certain, said, way) = layer_words(&lay, Whose::Window, x, y, &desc);
        if certain && raise == Raise::No {
            let w = if way.is_empty() {
                Vec::new()
            } else {
                cat!("; ", way)
            };
            return Err(ctx.die(cat!(
                said,
                " (refused under --no-raise; without it, this is a warning",
                w,
                ")"
            )));
        }
        say(with_way(said, &way));
    }
    Ok(jq_xy(crate::jqprog::WIN_AT_JQ, &w, x, y).text())
}

/// Whose X,Y a layer's sentence names: the window's own (--window) or the screen's (#361).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Whose {
    Window,
    Screen,
}

/// A layer's sentence with its way out after it, in brackets.
fn with_way(said: Vec<u8>, way: &[u8]) -> Vec<u8> {
    if way.is_empty() {
        said
    } else {
        cat!(said, " (", way, ")")
    }
}

/// narrow LINE "SX SY": a WIN_LAYER_JQ line of a layer spanning the screen, narrowed by where it
/// draws over the window at the screen's SX,SY (#283, finding 618): `drawn ...` when one of its items
/// is there, nothing when none is, the line as it was when that cannot be told.
fn narrow(ctx: &Ctx, w: &[u8], lay: Vec<u8>, at: &[u8]) -> Vec<u8> {
    use crate::drawn::{Drawn, layer_drawn, line_rect};
    let p: Vec<&[u8]> = at.splitn(2, |&c| c == b' ').collect();
    let (Some(r), [sx, sy]) = (line_rect(&lay), &p[..]) else {
        return lay;
    };
    let (Some(sx), Some(sy)) = (shell::dec(sx), shell::dec(sy)) else {
        return lay;
    };
    match layer_drawn(ctx, w, r, sx, sy) {
        Drawn::Here => cat!("drawn", &lay[b"possible".len()..]),
        Drawn::NotHere => Vec::new(),
        Drawn::Unknown => lay,
    }
}

/// A layer over a point (WIN_LAYER_JQ's line: `certain|possible "NAMESPACE" LAYER WxH at X,Y`, only
/// its rectangle known, or narrow's `drawn` for one spanning the screen that draws there) in words:
/// whether it is certain, the sentence, and the way out to say after it.
fn layer_words(
    lay: &[u8],
    whose: Whose,
    x: &[u8],
    y: &[u8],
    desc: &[u8],
) -> (bool, Vec<u8>, Vec<u8>) {
    let (how, lay) = match lay.iter().position(|&c| c == b' ') {
        Some(i) => (lay[..i].to_vec(), lay[i + 1..].to_vec()),
        None => (lay.to_vec(), lay.to_vec()),
    };
    let (first, rest) = match lay.iter().position(|&c| c == b' ') {
        Some(i) => (lay[..i].to_vec(), lay[i + 1..].to_vec()),
        None => (lay.clone(), lay.clone()),
    };
    let n = jq::jq(&["-r", "."], Some(&first), jq::Err::Null);
    let ns = if n.ok() { n.text() } else { Vec::new() };
    let at: &[u8] = if whose == Whose::Screen {
        b"the screen's "
    } else {
        b""
    };
    // (Omarchy 4.5's notifications stay mapped while idle: wait layer --gone would not return;
    // a dock said here is shown: a hidden one never gets here, #309)
    let toasts: &[u8] = if ns == b"omarchy-notifications" {
        b"omabox dnd --clear clears the toasts; omabox dnd on keeps new ones off"
    } else if ns == crate::omshell::DOCK_NS.as_bytes() {
        b"omabox dock says where it takes input; omabox dock off takes it away"
    } else {
        b""
    };
    if how == b"possible" {
        let said = cat!(
            "a layer spanning the screen, ",
            first,
            " (",
            rest,
            "), is up over window ",
            desc,
            ": input at ",
            at,
            x,
            ",",
            y,
            " may go to it if one of its items (a toast, a dock icon) is there; elsewhere it goes to the window"
        );
        return (false, said, toasts.to_vec());
    }
    let over = if whose == Whose::Screen {
        cat!("the screen's ", x, ",", y, ", on window ", desc)
    } else {
        cat!(x, ",", y, " of window ", desc)
    };
    if how == b"drawn" {
        let said = cat!(
            "a layer spanning the screen, ",
            first,
            " (",
            rest,
            "), draws over ",
            over,
            ": one of its items (a toast, a dock icon) is there, and input there goes to it, not to the window"
        );
        return (true, said, toasts.to_vec());
    }
    let said = cat!(
        "a layer surface ",
        first,
        " (",
        rest,
        ") is over ",
        over,
        ": where it takes input (a notification, a panel), input there goes to it, not to the window"
    );
    let way = if ns.is_empty() {
        Vec::new()
    } else {
        cat!("omabox wait layer ", ns, " --gone waits it out")
    };
    (true, said, way)
}

/// What narrows a WIN_LAYER_JQ line of a layer spanning the screen (the window, the line): narrow, or
/// a probe's stand-in.
pub type Narrow<'a> = dyn Fn(&[u8], Vec<u8>) -> Vec<u8> + 'a;

/// screen_layer_of SNAPSHOT X Y NARROW: for input at the screen's X,Y (no --window), what win_point
/// says of a layer certainly over the window there (#361, finding 562), from a desk_snap snapshot;
/// one spanning the screen is said only when NARROW (the window, the WIN_LAYER_JQ line) finds it
/// draws there (#283); nothing when no layer is over a window at the point (a click meant for the
/// bar or the desktop) or the snapshot is missing.
pub fn screen_layer_of(snap: &[u8], x: &[u8], y: &[u8], narrow: &Narrow<'_>) -> Vec<u8> {
    let (Some(sx), Some(sy)) = (shell::dec(x), shell::dec(y)) else {
        return Vec::new();
    };
    if snap.is_empty() {
        return Vec::new();
    }
    let prog = crate::jqprog::SCREEN_WIN_JQ.replacen("(WIN)", &format!("({WIN_JQ})"), 1);
    let r = Args::new(&["-c", "-n"])
        .kv("--argjson", "s", snap)
        .kv("--argjson", "x", x)
        .kv("--argjson", "y", y)
        .arg(prog)
        .run(None, jq::Err::Null);
    let w = if r.ok() {
        strip_nl(&r.text())
    } else {
        Vec::new()
    };
    if w.is_empty() {
        return Vec::new();
    }
    // (WIN_AT_JQ of its 0,0: where the window is)
    let at = jq_xy(crate::jqprog::WIN_AT_JQ, &w, b"0", b"0").text();
    let Some(i) = at.iter().position(|&c| c == b' ') else {
        return Vec::new();
    };
    let (Some(ax), Some(ay)) = (shell::dec(&at[..i]), shell::dec(&at[i + 1..])) else {
        return Vec::new();
    };
    let (wx, wy) = ((sx - ax).to_string(), (sy - ay).to_string());
    let lay = jq_xy(
        crate::jqprog::WIN_LAYER_JQ,
        &w,
        wx.as_bytes(),
        wy.as_bytes(),
    )
    .text();
    if lay.is_empty() {
        return Vec::new();
    }
    let lay = if lay.starts_with(b"possible ") {
        narrow(&w, lay)
    } else {
        lay
    };
    if lay.is_empty() {
        return Vec::new();
    }
    // (one spanning the screen that only may take it: said for --window, not here, where Omarchy
    // 4.5's idle notifications column and dock would speak at every plain click under them)
    match layer_words(&lay, Whose::Screen, x, y, &win_desc(&w)) {
        (true, said, way) => with_way(said, &way),
        _ => Vec::new(),
    }
}

/// screen_layer X Y: screen_layer_of the box's desktop now, said on stderr. Never refuses and never
/// dies: a desktop that does not answer leaves the input to its own errors.
pub fn screen_layer(ctx: &Ctx, x: &[u8], y: &[u8]) {
    let at = cat!(x, " ", y);
    let said = screen_layer_of(&crate::desk::desk_snap(ctx, false), x, y, &|w, lay| {
        narrow(ctx, w, lay, &at)
    });
    if !said.is_empty() {
        say(said);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jq_lead_taken_off() {
        assert_eq!(
            jq_error_text(b"jq: error (at <stdin>:1): x: y"),
            b"x: y".as_slice()
        );
        assert_eq!(jq_error_text(b"jq: errorless"), b"jq: errorless".as_slice());
        assert_eq!(jq_error_text(b"other"), b"other".as_slice());
    }
}
