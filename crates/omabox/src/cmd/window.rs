//! `omabox window [-b NAME] [--window SEL | --active] (--size WxH | --float | --tile |
//! --fullscreen [off] | --maximize [off]) [--json]` (#362, finding 591): one window of the box
//! floated and sized, or tiled again, by the box's own Hyprland (on_box: never the host's, S8), as
//! `mode` sizes the whole screen. The size is the layout's, as `windows` gives it; the window is
//! centred on its monitor's free area, and what it took is said: an app's smallest or largest size
//! keeps it from another (exit 1). --fullscreen and --maximize (#443, finding 705) set a state and
//! never toggle it: a window already in it is left alone, and `off` leaves only that state.

use crate::args::{optarg, unexpected, unknown_opt};
use crate::boxes::need_box;
use crate::cat;
use crate::exec::{ErrTo, Out, on_box};
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::hypr::{HYPR_LIMIT, hung_text, hypr_answers_in};
use crate::jq;
use crate::select::select_box;
use crate::settle::{after, past};
use crate::shell;
use crate::win::{box_windows, jq_length, jq_r, win_desc, win_select};
use std::time::Duration;

/// What the command does to the window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Act {
    /// Floated, that size (W, H), centred.
    Size(u32, u32),
    /// Floated where Hyprland puts it (the size it last floated at).
    Float,
    /// Tiled again.
    Tile,
    /// Fullscreen (Hyprland's mode 2: nothing over it but overlays, no bar), or out of it.
    Fullscreen(bool),
    /// Maximized (mode 1: the work area, the bar kept), or out of it.
    Maximize(bool),
}

impl Act {
    /// The fullscreen mode it sets (true) or leaves (false), as `windows --json`'s fullscreenMode
    /// names it: None for the others.
    pub fn mode(self) -> Option<(&'static str, bool)> {
        match self {
            Act::Fullscreen(on) => Some(("fullscreen", on)),
            Act::Maximize(on) => Some(("maximized", on)),
            _ => None,
        }
    }

    /// Whether a window in fullscreenMode NOW is as ACT wants it: in the mode it sets, out of the
    /// mode it leaves. Always for the others (they are checked by their floating).
    pub fn mode_ok(self, now: &[u8]) -> bool {
        match self.mode() {
            Some((m, true)) => now == m.as_bytes(),
            Some((m, false)) => now != m.as_bytes(),
            None => true,
        }
    }
}

/// `--fullscreen`'s and `--maximize`'s word: none or `on` sets, `off` leaves; None for another.
pub fn parse_on(v: Option<&[u8]>) -> Option<bool> {
    match v {
        None | Some(b"on") => Some(true),
        Some(b"off") => Some(false),
        _ => None,
    }
}

/// `--size`'s WxH: whole layout pixels, 1 to 16384 each.
pub fn parse_size(v: &[u8]) -> Option<(u32, u32)> {
    if !shell::re_match("^[1-9][0-9]{0,4}x[1-9][0-9]{0,4}$", v) {
        return None;
    }
    let s = std::str::from_utf8(v).ok()?;
    let (w, h) = s.split_once('x')?;
    let (w, h) = (w.parse::<u32>().ok()?, h.parse::<u32>().ok()?);
    (w <= 16384 && h <= 16384).then_some((w, h))
}

/// The Lua the box's Hyprland runs for ACT on the window at ADDR (checked as 0x...): each dispatch
/// one Hyprland refuses (a fullscreen window's resize) is the chunk's error, in Hyprland's words; a
/// dispatcher this Hyprland's hl.dsp does not make from those arguments (nil: another version's
/// hl.dsp) is one too, named.
pub fn lua(addr: &[u8], act: Act) -> Vec<u8> {
    let head = cat!(
        "local w = 'address:",
        addr,
        "'\nlocal function d(x, what)\n  if x == nil then error('this Hyprland makes no dispatcher of ' .. what .. ' as omabox calls it (its hl.dsp differs)', 0) end\n  local r = hl.dispatch(x)\n  if type(r) == 'table' and r.ok == false then error(tostring(r.error), 0) end\nend\n"
    );
    let float = |a: &str| {
        cat!(
            "d(hl.dsp.window.float({ action = '",
            a,
            "', window = w }), 'hl.dsp.window.float')"
        )
    };
    match act {
        Act::Tile => cat!(head, float("disable")),
        Act::Float => cat!(head, float("enable")),
        Act::Size(wd, ht) => cat!(
            head,
            float("enable"),
            "\nd(hl.dsp.window.resize({ window = w, x = ",
            wd.to_string(),
            ", y = ",
            ht.to_string(),
            " }), 'hl.dsp.window.resize')\nd(hl.dsp.window.center({ window = w }), 'hl.dsp.window.center')"
        ),
        // set or unset, never toggle: 0.56.2 and 0.57.1 take both, and set again changes nothing.
        Act::Fullscreen(on) | Act::Maximize(on) => cat!(
            head,
            "d(hl.dsp.window.fullscreen({ mode = '",
            if matches!(act, Act::Fullscreen(_)) {
                "fullscreen"
            } else {
                "maximized"
            },
            "', action = '",
            if on { "set" } else { "unset" },
            "', window = w }), 'hl.dsp.window.fullscreen')"
        ),
    }
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut sels, mut size) = (Vec::new(), Vec::new(), Vec::new());
    let (mut float, mut tile, mut json) = (false, false, false);
    let (mut full, mut max): (Option<bool>, Option<bool>) = (None, None);
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
            b"--size" => {
                optarg(ctx, a, v)?;
                size = args[i + 1].clone();
                i += 1;
            }
            b"--float" => float = true,
            b"--tile" => tile = true,
            b"--fullscreen" | b"--maximize" => {
                let word = v.filter(|v| !v.starts_with(b"-"));
                let Some(on) = parse_on(word.map(|w| w.as_slice())) else {
                    return Err(ctx.usage_die(cat!(
                        "window: ",
                        a,
                        " sets it, ",
                        a,
                        " off leaves it (on or off), got ",
                        word.cloned().unwrap_or_default()
                    )));
                };
                if word.is_some() {
                    i += 1;
                }
                if a == b"--fullscreen" {
                    full = Some(on);
                } else {
                    max = Some(on);
                }
            }
            b"--json" => json = true,
            b"-h" | b"--help" => return crate::args::usage("window"),
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "window", a)),
            _ => {
                return Err(unexpected(
                    ctx,
                    "window",
                    a,
                    "(the window is --window SEL, its size --size WxH)",
                ));
            }
        }
        i += 1;
    }
    let others = !size.is_empty() || float || tile;
    if (full.is_some() && max.is_some()) || ((full.is_some() || max.is_some()) && others) {
        return Err(ctx.usage_die(
            "window: --fullscreen or --maximize goes alone (not with each other, --size, --float or --tile)",
        ));
    }
    let act = match (size.is_empty(), float, tile) {
        _ if full.is_some() => Act::Fullscreen(full == Some(true)),
        _ if max.is_some() => Act::Maximize(max == Some(true)),
        (_, true, true) => return Err(ctx.usage_die("window: --float or --tile, not both")),
        (false, _, true) => {
            return Err(
                ctx.usage_die("window: --size floats the window, --tile tiles it: not both")
            );
        }
        (false, _, false) => match parse_size(&size) {
            Some((w, h)) => Act::Size(w, h),
            None => {
                return Err(ctx.usage_die(cat!(
                    "window: --size is WxH in layout pixels (760x600), 1 to 16384 each, got ",
                    size
                )));
            }
        },
        (true, true, false) => Act::Float,
        (true, false, true) => Act::Tile,
        (true, false, false) => {
            return Err(ctx.usage_die(
                "window: what to do with it: --size WxH, --float, --tile, --fullscreen [off] or --maximize [off] (the box's windows: omabox windows)",
            ));
        }
    };
    select_box(ctx, &name)?;
    need_box(ctx)?;
    let wins = box_windows(ctx, None)?;
    if sels.is_empty() && jq_length(&wins) == b"0" {
        return Err(ctx.die(cat!(
            "window: box '",
            ctx.name,
            "' has no window (omabox windows; the shell's bar, panels and menus are layers)"
        )));
    }
    let active = [b"active".to_vec()];
    let w = win_select(ctx, &wins, if sels.is_empty() { &active } else { &sels })?;
    let addr = jq_r(".address", &w);
    if !shell::re_match("^0x[0-9a-f]+$", &addr) {
        return Err(ctx.die(cat!("window: bad window address: ", addr)));
    }
    let mode_was = jq_r(r#".fullscreenMode // "none""#, &w);
    // A fullscreen (or maximized) window takes no size (Hyprland floats it, then refuses the
    // resize): nothing done.
    if matches!(act, Act::Size(..) | Act::Float) && mode_was != b"none" {
        let off: &str = if mode_was == b"maximized" {
            "--maximize off"
        } else {
            "--fullscreen off"
        };
        return Err(ctx.die(cat!(
            "window: ",
            win_desc(&w),
            " is ",
            mode_was,
            ": it takes no size until it leaves that (omabox window -b ",
            shell::quote_q(&ctx.name),
            " --window ",
            addr,
            " ",
            off,
            ")"
        )));
    }
    // Set, not toggled: a window already as asked is left alone (Hyprland's own unset would also
    // take a maximized window out of maximized when asked to leave fullscreen).
    if !act.mode_ok(&mode_was) || act.mode().is_none() {
        let r = on_box(
            ctx,
            &[
                b"timeout".to_vec(),
                HYPR_LIMIT.as_bytes().to_vec(),
                b"hyprctl".to_vec(),
                b"eval".to_vec(),
                lua(&addr, act),
            ],
            true,
            Out::Capture(None),
            ErrTo::Out,
        );
        let res = shell::subst(&r.out);
        if res != b"ok" {
            if !hypr_answers_in(ctx, "1")? {
                let t = hung_text(ctx, "")?;
                return Err(ctx.die(t));
            }
            return Err(ctx.die(cat!(
                "window: box '",
                ctx.name,
                "''s Hyprland refused it for ",
                win_desc(&w),
                ": ",
                res
            )));
        }
    }
    // As the box has it now: floating (or in the mode) as asked, the same place and size twice
    // 100 ms apart (an app's own limits may change what it took), 2 s at most.
    let want_float: Option<&[u8]> = match act {
        Act::Tile => Some(b"false"),
        Act::Size(..) | Act::Float => Some(b"true"),
        _ => None,
    };
    let done = |now: &[u8]| {
        want_float.is_none_or(|f| jq_r(".floating", now) == f)
            && act.mode_ok(&jq_r(r#".fullscreenMode // "none""#, now))
    };
    let sel = [addr.clone()];
    let deadline = after(2);
    let mut last: Vec<u8> = Vec::new();
    let now = loop {
        std::thread::sleep(Duration::from_millis(100));
        let wins = box_windows(ctx, None)?;
        let hits = crate::win::win_hits(&wins, &sel).unwrap_or_default();
        if jq_length(&hits) != b"1" {
            return Err(ctx.die(cat!("window: ", win_desc(&w), " closed meanwhile")));
        }
        let now = jq::jq(&["-c", ".[0]"], Some(&hits), jq::Err::Inherit).text();
        let geom = jq_r(r#""\(.floating) \(.fullscreenMode) \(.at) \(.size)""#, &now);
        if (geom == last && done(&now)) || past(deadline) {
            break now;
        }
        last = geom;
    };
    if json {
        let r = jq::jq(
            &["-c", crate::jqprog::WINDOWS_JSON_JQ],
            Some(&cat!("[", now, "]")),
            jq::Err::Inherit,
        );
        out(jq::jq(&["-c", ".[0]"], Some(&r.out), jq::Err::Inherit).out)?;
    } else {
        let line = jq_r(
            r#""\(.address) \(.class) \(.title | tojson) " + (if .floating then "floating" else "tiled" end) + (if (.fullscreenMode // "none") != "none" then ", \(.fullscreenMode)" else "" end) + " at \(.at[0]),\(.at[1]) \(.size[0])x\(.size[1])""#,
            &now,
        );
        out(cat!(line, "\n"))?;
    }
    if let Some(f) = want_float
        && jq_r(".floating", &now) != f
    {
        say(cat!(
            "window: ",
            win_desc(&now),
            " is still ",
            if f == b"true" { "tiled" } else { "floating" },
            " (a window rule? omabox log --grep windowrule)"
        ));
        return Err(Exit(1));
    }
    if let Some((m, on)) = act.mode() {
        let is = jq_r(r#".fullscreenMode // "none""#, &now);
        if !act.mode_ok(&is) {
            say(cat!(
                "window: ",
                win_desc(&now),
                if on { " did not become " } else { " is still " },
                m,
                if on {
                    cat!(" (it is ", is, ")")
                } else {
                    Vec::new()
                },
                ": a window rule? (omabox log --grep windowrule)"
            ));
            return Err(Exit(1));
        }
    }
    if let Act::Size(wd, ht) = act {
        let took = jq_r(r#""\(.size[0])x\(.size[1])""#, &now);
        let asked = format!("{wd}x{ht}");
        if took != asked.as_bytes() {
            say(cat!(
                "window: asked for ",
                asked,
                ", the window took ",
                took,
                ": its app's smallest or largest size (Hyprland keeps a window within them)"
            ));
            return Err(Exit(1));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_size(b"760x600"), Some((760, 600)));
        assert_eq!(parse_size(b"1x1"), Some((1, 1)));
        assert_eq!(parse_size(b"16384x16384"), Some((16384, 16384)));
        for bad in [
            &b"0x600"[..],
            b"760x0",
            b"760",
            b"760x",
            b"x600",
            b"760X600",
            b"16385x10",
            b"760x600@60",
            b"-760x600",
            b"760 x 600",
            b"",
        ] {
            assert_eq!(parse_size(bad), None, "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn lua_chunks() {
        let s = String::from_utf8(lua(b"0xabc", Act::Size(760, 540))).unwrap();
        assert!(s.starts_with("local w = 'address:0xabc'\n"));
        let float = s.find("float({ action = 'enable'").unwrap();
        let resize = s.find("resize({ window = w, x = 760, y = 540 })").unwrap();
        let center = s.find("center({ window = w })").unwrap();
        assert!(float < resize && resize < center, "{s}");
        assert!(s.contains("r.ok == false then error(tostring(r.error), 0)"));
        assert!(s.contains("if x == nil then error("), "{s}");
        let t = String::from_utf8(lua(b"0xabc", Act::Tile)).unwrap();
        assert!(t.contains("action = 'disable'") && !t.contains("resize"));
        let f = String::from_utf8(lua(b"0xabc", Act::Float)).unwrap();
        assert!(f.contains("action = 'enable'") && !f.contains("resize") && !f.contains("center"));
    }

    #[test]
    fn fullscreen_set_not_toggled() {
        let l = |a| String::from_utf8(lua(b"0xabc", a)).unwrap();
        assert!(l(Act::Fullscreen(true)).contains(
            "hl.dsp.window.fullscreen({ mode = 'fullscreen', action = 'set', window = w })"
        ));
        assert!(l(Act::Fullscreen(false)).contains(
            "hl.dsp.window.fullscreen({ mode = 'fullscreen', action = 'unset', window = w })"
        ));
        assert!(l(Act::Maximize(true)).contains("mode = 'maximized', action = 'set'"));
        assert!(l(Act::Maximize(false)).contains("mode = 'maximized', action = 'unset'"));
        for a in [true, false] {
            assert!(!l(Act::Fullscreen(a)).contains("toggle"));
            assert!(!l(Act::Maximize(a)).contains("toggle"));
        }
        // Already so: no dispatch (mode_ok); off leaves only its own mode.
        assert!(Act::Fullscreen(true).mode_ok(b"fullscreen"));
        assert!(!Act::Fullscreen(true).mode_ok(b"maximized"));
        assert!(!Act::Fullscreen(true).mode_ok(b"none"));
        assert!(Act::Fullscreen(false).mode_ok(b"none"));
        assert!(Act::Fullscreen(false).mode_ok(b"maximized"));
        assert!(!Act::Fullscreen(false).mode_ok(b"fullscreen"));
        assert!(Act::Maximize(true).mode_ok(b"maximized"));
        assert!(!Act::Maximize(true).mode_ok(b"fullscreen"));
        assert!(Act::Maximize(false).mode_ok(b"fullscreen"));
        assert!(!Act::Maximize(false).mode_ok(b"maximized"));
        assert!(Act::Tile.mode_ok(b"fullscreen") && Act::Tile.mode().is_none());
        assert_eq!(parse_on(None), Some(true));
        assert_eq!(parse_on(Some(b"on")), Some(true));
        assert_eq!(parse_on(Some(b"off")), Some(false));
        assert_eq!(parse_on(Some(b"unset")), None);
    }
}
