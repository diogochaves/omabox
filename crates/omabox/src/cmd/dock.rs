//! `omabox dock [-b NAME] [on|off] [--json]` (#309, finding 611): Omarchy 4.5's dock in the box,
//! and where it takes input. Bare: one line per monitor, whether the dock there is shown (its icons
//! take input), tucked away (auto-hide, or a maximised window: only the strip along its edge does,
//! until the pointer reaches it) or hidden (a workspace that is not floating, a fullscreen window:
//! none at all, its layer still mapped). off: its service disabled in the box's shell, the layer
//! gone; on: back. The box's own shell only, never the host's.

use crate::args::{optarg, unexpected, unknown_opt, unknown_word};
use crate::boxes::{box_home_file, need_box};
use crate::cat;
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::hypr::hypr_json;
use crate::jq::{self, Args};
use crate::omshell::{self, DOCK_NS};
use crate::select::select_box;
use crate::shell::strip_nl;

/// The dock's state per monitor, from $m (monitors), $l (layers), $c (clients), $w (workspaces),
/// $modes ({workspace id: Omarchy's mode}), $cfg (shell.json's dock), $on (its service enabled).
const DOCK_JQ: &str = r#"
  (($cfg.position // "bottom") | ascii_downcase | if IN("top","bottom","left","right") then . else "bottom" end) as $pos
  | ($cfg.autohide == true) as $ah
  | {enabled: $on, position: $pos, autohide: $ah,
     monitors: [$m[] | . as $mon | (.activeWorkspace.id // 0) as $id
       | ($modes[$id | tostring] // "") as $own
       | (first($w[]? | select(.id == $id) | .tiledLayout) // "dwindle") as $tiled
       | (if $own != "" then $own else $tiled end) as $mode
       | ([$c[]? | select(.mapped != false and .workspace.id == $id) | .fullscreen // 0] | max // 0) as $fs
       | (first($l[$mon.name]?.levels? // {} | .[]? | .[]? | select(.namespace == $ns)) // null) as $lay
       | {name, workspace: $id, layout: $mode, fullscreen: $fs,
          layer: (if $lay then {at: [$lay.x, $lay.y], size: [$lay.w, $lay.h]} else null end)}
       | .state = (if ($on | not) then "off" elif $lay == null then "none"
                   elif $mode != "floating" or $fs >= 2 then "hidden"
                   elif $ah or $fs == 1 then "tucked" else "shown" end)
       | .input = ({shown: "icons", tucked: "edge"}[.state] // "none")]}"#;

/// DOCK_JQ's object as lines.
const DOCK_TEXT_JQ: &str = r#"
  if .enabled | not then "off: its service is disabled in this box (omabox dock on brings it back)"
  else .position as $p | .autohide as $ah | .monitors[]
    | (if .layer then " (\(.layer.size[0])x\(.layer.size[1]) at \(.layer.at[0]),\(.layer.at[1]))" else "" end) as $g
    | "\(.name): " + (
        if .state == "none" then "no dock layer: its shell has not drawn one (omabox log shell)"
        elif .state == "hidden" and .layout != "floating" then
          "hidden\($g): workspace \(.workspace) is \(.layout), not floating; the dock takes no input there"
        elif .state == "hidden" then
          "hidden\($g): a fullscreen window on workspace \(.workspace); the dock takes no input there"
        elif .state == "tucked" then
          "tucked away\($g) on workspace \(.workspace) (floating; " + (if $ah then "auto-hide on" else "a maximised window" end)
          + "): input only on the strip along the \($p) edge, until the pointer reaches it"
        else "shown along the \($p)\($g) on workspace \(.workspace) (floating): input on its icons, elsewhere to the window" end)
  end"#;

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut want, mut json) = (Vec::new(), Vec::new(), false);
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, args.get(i + 1))?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"on" | b"off" => {
                if !want.is_empty() {
                    return Err(ctx.usage_die("dock: on or off, once"));
                }
                want = a.clone();
            }
            b"--json" => json = true,
            b"-h" | b"--help" => return crate::args::usage("dock"),
            w if w.starts_with(b"-") => return Err(unknown_opt(ctx, "dock", a)),
            w => {
                if ctx.in_boxes(w).is_dir() {
                    return Err(unexpected(ctx, "dock", w, ""));
                }
                return Err(unknown_word(
                    ctx,
                    "dock",
                    "argument",
                    w,
                    "(on, off, or nothing to see where it takes input; a box is -b NAME)",
                    &["on", "off"],
                ));
            }
        }
        i += 1;
    }
    if json && !want.is_empty() {
        return Err(ctx.usage_die("dock: --json shows the dock, with no on or off"));
    }
    ctx.parsed();
    select_box(ctx, &name)?;
    need_box(ctx)?;
    omshell::need_dock(ctx, "dock")?;
    if !want.is_empty() {
        return switch(ctx, want == b"on", true);
    }
    let st = state(ctx)?;
    if json {
        return out(cat!(st, "\n"));
    }
    let r = jq::jq(&["-r", DOCK_TEXT_JQ], Some(&st), jq::Err::Inherit);
    if !r.ok() {
        return Err(Exit(1));
    }
    out(&r.out)
}

/// DOCK_JQ's object for the box now.
fn state(ctx: &Ctx) -> Res<Vec<u8>> {
    let m = strip_nl(&hypr_json(ctx, &["monitors"])?);
    let l = strip_nl(&hypr_json(ctx, &["layers"])?);
    let c = strip_nl(&hypr_json(ctx, &["clients"])?);
    let w = strip_nl(&hypr_json(ctx, &["workspaces"])?);
    let ids = jq::jq(
        &["-r", ".[]? | .activeWorkspace.id // 0"],
        Some(&m),
        jq::Err::Null,
    )
    .text();
    let mut modes = Vec::new();
    for id in ids.split(|&c| c == b'\n').filter(|v| !v.is_empty()) {
        let n = crate::shell::dec(id).unwrap_or(0);
        let mode = omshell::ws_mode(ctx, n);
        modes.push(format!(
            "{}:{}",
            crate::json::quote(&n.to_string()),
            crate::json::quote(&String::from_utf8_lossy(&mode))
        ));
    }
    let cfg = box_home_file(&ctx.d, ".config/omarchy/shell.json")
        .and_then(|p| std::fs::read(p).ok())
        .map(|t| jq::jq(&["-c", ".dock // {} | objects"], Some(&t), jq::Err::Null).text())
        .map(|t| strip_nl(&t))
        .filter(|t| !t.is_empty())
        .unwrap_or(b"{}".to_vec());
    let on = if omshell::dock_enabled(ctx) {
        "true"
    } else {
        "false"
    };
    let r = Args::new(&["-c", "-n"])
        .kv("--argjson", "m", &m)
        .kv("--argjson", "l", &l)
        .kv("--argjson", "c", &c)
        .kv("--argjson", "w", &w)
        .kv("--argjson", "modes", format!("{{{}}}", modes.join(",")))
        .kv("--argjson", "cfg", &cfg)
        .kv("--argjson", "on", on)
        .kv("--arg", "ns", DOCK_NS)
        .arg(DOCK_JQ)
        .run(None, jq::Err::Inherit);
    if !r.ok() {
        return Err(Exit(1));
    }
    Ok(strip_nl(&r.text()))
}

/// Whether the box's layers list the dock.
fn has_layer(ctx: &Ctx) -> Res<bool> {
    let l = hypr_json(ctx, &["layers"])?;
    let q = cat!("\"", DOCK_NS, "\"");
    Ok(l.windows(q.len()).any(|w| w == q.as_slice()))
}

/// dock on|off: its service switched in the box's shell, then its layer come or gone (5 s); SAY:
/// a line on stderr that it did.
pub fn switch(ctx: &mut Ctx, on: bool, say_it: bool) -> Res<()> {
    omshell::dock_set(ctx, "dock", on)?;
    let t0 = std::time::Instant::now();
    while has_layer(ctx)? != on {
        if t0.elapsed() > std::time::Duration::from_secs(5) {
            return Err(ctx.die(cat!(
                "dock: box '",
                ctx.name,
                "': its shell turned the dock ",
                if on { "on" } else { "off" },
                ", but its layer is ",
                if on { "not there" } else { "still there" },
                " after 5 s (omabox log -b ",
                ctx.name,
                " shell)"
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if !say_it {
        return Ok(());
    }
    say(if on {
        cat!(
            "box '",
            ctx.name,
            "': dock on: shown on floating workspaces (omabox dock says where it takes input)"
        )
    } else {
        cat!(
            "box '",
            ctx.name,
            "': dock off: its service disabled in the box's shell, its layer gone (omabox dock on brings it back)"
        )
    });
    Ok(())
}
