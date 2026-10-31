//! `omabox window [-b NAME] [--window SEL | --active] (--size WxH | --float | --tile) [--json]`
//! (#362, finding 591): one window of the box floated and sized, or tiled again, by the box's own
//! Hyprland (on_box: never the host's, S8), as `mode` sizes the whole screen. The size is the
//! layout's, as `windows` gives it; the window is centred on its monitor's free area, and what it
//! took is said: an app's smallest or largest size keeps it from another (exit 1).

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
/// one Hyprland refuses (a fullscreen window's resize) is the chunk's error, in Hyprland's words.
pub fn lua(addr: &[u8], act: Act) -> Vec<u8> {
    let head = cat!(
        "local w = 'address:",
        addr,
        "'\nlocal function d(x)\n  local r = hl.dispatch(x)\n  if type(r) == 'table' and r.ok == false then error(tostring(r.error), 0) end\nend\n"
    );
    match act {
        Act::Tile => cat!(
            head,
            "d(hl.dsp.window.float({ action = 'disable', window = w }))"
        ),
        Act::Float => cat!(
            head,
            "d(hl.dsp.window.float({ action = 'enable', window = w }))"
        ),
        Act::Size(wd, ht) => cat!(
            head,
            "d(hl.dsp.window.float({ action = 'enable', window = w }))\nd(hl.dsp.window.resize({ window = w, x = ",
            wd.to_string(),
            ", y = ",
            ht.to_string(),
            " }))\nd(hl.dsp.window.center({ window = w }))"
        ),
    }
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut sels, mut size) = (Vec::new(), Vec::new(), Vec::new());
    let (mut float, mut tile, mut json) = (false, false, false);
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
    let act = match (size.is_empty(), float, tile) {
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
                "window: what to do with it: --size WxH, --float or --tile (the box's windows: omabox windows)",
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
    // A fullscreen window takes no size (Hyprland floats it, then refuses the resize): nothing done.
    if act != Act::Tile && jq_r(".fullscreen", &w) == b"true" {
        return Err(ctx.die(cat!(
            "window: ",
            win_desc(&w),
            " is fullscreen: it takes no size until it leaves fullscreen (omabox lua \"hl.dispatch(hl.dsp.window.fullscreen({ mode = 'fullscreen', action = 'unset', window = 'address:",
            addr,
            "' }))\")"
        )));
    }
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
    // As the box has it now: floating as asked, the same size twice 100 ms apart (an app's own
    // limits may change what it took), 2 s at most.
    let want_float: &[u8] = if act == Act::Tile { b"false" } else { b"true" };
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
        let geom = jq_r(r#""\(.floating) \(.at) \(.size)""#, &now);
        if (geom == last && jq_r(".floating", &now) == want_float) || past(deadline) {
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
            r#""\(.address) \(.class) \(.title | tojson) " + (if .floating then "floating" else "tiled" end) + " at \(.at[0]),\(.at[1]) \(.size[0])x\(.size[1])""#,
            &now,
        );
        out(cat!(line, "\n"))?;
    }
    if jq_r(".floating", &now) != want_float {
        say(cat!(
            "window: ",
            win_desc(&now),
            " is still ",
            if want_float == b"true" {
                "tiled"
            } else {
                "floating"
            },
            " (a window rule? omabox log --grep windowrule)"
        ));
        return Err(Exit(1));
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
        let t = String::from_utf8(lua(b"0xabc", Act::Tile)).unwrap();
        assert!(t.contains("action = 'disable'") && !t.contains("resize"));
        let f = String::from_utf8(lua(b"0xabc", Act::Float)).unwrap();
        assert!(f.contains("action = 'enable'") && !f.contains("resize") && !f.contains("center"));
    }
}
