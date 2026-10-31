//! The box's Omarchy shell's Do Not Disturb and dock (#309, finding 611): what `dnd`, `dock`,
//! `up --dnd` and `up --no-dock` ask the box's own shell over its IPC (`omarchy-shell`, run inside
//! the box: never the host's shell), and whether the dock takes input where a window is.
//!
//! Do Not Disturb is the notifications service's `setDnd`/`isDnd` on Omarchy 4.0 and 4.5 alike. The
//! dock came with 4.5: a top layer `omarchy-dock` along a screen edge, mapped on every workspace,
//! shown (its icons taking input) only on a floating one with no fullscreen window; elsewhere its
//! input mask is empty. Floating is not a Hyprland layout: Omarchy keeps each workspace's mode in
//! `~/.local/state/omarchy/workspace-layouts/N.lua` (`mode = "floating"`), a default for the rest in
//! `default.lua`, as `omarchy-hyprland-workspace-layout-current` reads them.

use crate::boxes::{box_home_file, meta};
use crate::cat;
use crate::exec::{ExecOpts, Redir, box_exec, pipe, read_all};
use crate::exit::{Ctx, Res};
use crate::jq::{self, Args};
use crate::shell::strip_nl;
use std::os::fd::AsRawFd;

/// The dock's layer namespace and plugin id on Omarchy 4.5.
pub const DOCK_NS: &str = "omarchy-dock";
pub const DOCK_ID: &str = "omarchy.dock";

/// The Omarchy tree the box runs: its --omarchy DIR, else the installed one.
pub fn tree(ctx: &Ctx) -> Vec<u8> {
    let t = meta(&ctx.d, "omarchy").0;
    if t.is_empty() {
        b"/usr/share/omarchy".to_vec()
    } else {
        t
    }
}

/// Whether the tree TREE has the dock (Omarchy 4.5 and later).
pub fn tree_has_dock(tree: &[u8]) -> bool {
    crate::shell::path(&cat!(tree, "/shell/plugins/dock/manifest.json")).is_file()
}

/// Whether the tree TREE's notifications answer `setDnd` (Omarchy 4.0 and later).
pub fn tree_has_dnd(tree: &[u8]) -> bool {
    std::fs::read(crate::shell::path(&cat!(
        tree,
        "/shell/plugins/notifications/Service.qml"
    )))
    .is_ok_and(|t| {
        let n: &[u8] = b"function setDnd(";
        t.windows(n.len()).any(|w| w == n)
    })
}

/// A box without Omarchy's shell (`up --no-shell`) has nothing to ask: CMD's refusal.
pub fn need_shell(ctx: &Ctx, cmd: &str) -> Res<()> {
    if meta(&ctx.d, "shell").0 != b"true" {
        return Err(ctx.die(cat!(
            cmd,
            ": box '",
            ctx.name,
            "' runs no Omarchy shell (up --no-shell): it has no notifications nor dock"
        )));
    }
    Ok(())
}

/// `omarchy-shell TARGET METHOD ARG...` in the box, 5 s at most: Ok(its answer, one line) when it
/// answered, Err(what it said, or why it did not) when not.
pub fn ipc(ctx: &mut Ctx, words: &[&[u8]]) -> Result<Vec<u8>, Vec<u8>> {
    let mut argv: Vec<Vec<u8>> = vec![
        b"/usr/bin/sh".to_vec(),
        b"-c".to_vec(),
        b"exec timeout 5 omarchy-shell \"$@\" 2>&1".to_vec(),
        b"sh".to_vec(),
    ];
    argv.extend(words.iter().map(|w| w.to_vec()));
    let (r, wr) = pipe().map_err(|e| e.to_string().into_bytes())?;
    let reader = std::thread::spawn(move || read_all(r, Some(65_536)));
    let rc = {
        let _e = Redir::null(2);
        box_exec(
            ctx,
            &argv,
            ExecOpts {
                stdin_null: true,
                stdout: Some(wr.as_raw_fd()),
                ..ExecOpts::default()
            },
        )
    };
    drop(wr);
    let said = strip_nl(&reader.join().unwrap_or_default());
    match rc {
        Ok(0) => Ok(said),
        Ok(124) => Err(b"its shell did not answer in 5 s".to_vec()),
        Ok(_) if said.is_empty() => Err(b"its shell did not answer".to_vec()),
        Ok(_) => Err(said),
        Err(_) => Err(b"the box went down".to_vec()),
    }
}

/// CMD's refusal of a box whose Omarchy lacks what WHAT names (the dock before 4.5).
fn lacks(ctx: &Ctx, cmd: &str, what: &str, since: &str) -> crate::exit::Exit {
    ctx.die(cat!(
        cmd,
        ": box '",
        ctx.name,
        "' runs an Omarchy with no ",
        what,
        " (",
        tree(ctx),
        "; ",
        since,
        ")"
    ))
}

/// The box's shell can be asked about Do Not Disturb: it runs one, and its Omarchy has it.
pub fn need_dnd(ctx: &Ctx, cmd: &str) -> Res<()> {
    need_shell(ctx, cmd)?;
    if !tree_has_dnd(&tree(ctx)) {
        return Err(lacks(
            ctx,
            cmd,
            "Do Not Disturb to set",
            "its notifications answer no setDnd",
        ));
    }
    Ok(())
}

/// The box's shell can be asked about the dock: it runs one, and its Omarchy has a dock.
pub fn need_dock(ctx: &Ctx, cmd: &str) -> Res<()> {
    need_shell(ctx, cmd)?;
    if !tree_has_dock(&tree(ctx)) {
        return Err(lacks(ctx, cmd, "dock", "Omarchy 4.5 brought it"));
    }
    Ok(())
}

/// ipc's answer, or CMD's death with what the shell said.
fn ask(ctx: &mut Ctx, cmd: &str, words: &[&[u8]]) -> Res<Vec<u8>> {
    ipc(ctx, words).map_err(|why| {
        ctx.die(cat!(
            cmd,
            ": box '",
            ctx.name,
            "': omarchy-shell ",
            words.join(&b' '),
            ": ",
            why
        ))
    })
}

/// Whether Do Not Disturb is on in the box: "on" or "off", as its shell says.
pub fn dnd_state(ctx: &mut Ctx, cmd: &str) -> Res<Vec<u8>> {
    ask(ctx, cmd, &[b"notifications", b"isDnd"])
}

/// Do Not Disturb on or off in the box, checked by its shell's answer.
pub fn dnd_set(ctx: &mut Ctx, cmd: &str, on: bool) -> Res<()> {
    let want: &[u8] = if on { b"on" } else { b"off" };
    let got = ask(ctx, cmd, &[b"notifications", b"setDnd", want])?;
    if got != want {
        return Err(ctx.die(cat!(
            cmd,
            ": box '",
            ctx.name,
            "': its shell answered ",
            got,
            " to Do Not Disturb ",
            want
        )));
    }
    Ok(())
}

/// The toasts on the box's screen dismissed (into its history, as a click on each would): how
/// many there were, when its shell counts them (4.5).
pub fn toasts_clear(ctx: &mut Ctx, cmd: &str) -> Res<Option<i64>> {
    let n = ipc(ctx, &[b"notifications", b"count"])
        .ok()
        .and_then(|c| crate::shell::dec(&c));
    let got = ask(ctx, cmd, &[b"notifications", b"dismissAll"])?;
    if got != b"ok" {
        return Err(ctx.die(cat!(
            cmd,
            ": box '",
            ctx.name,
            "': its shell answered ",
            got,
            " to dismissAll"
        )));
    }
    Ok(n)
}

/// The dock's service on or off in the box (its shell.json's disabledPlugins, written by its own
/// shell), checked by its shell's answer.
pub fn dock_set(ctx: &mut Ctx, cmd: &str, on: bool) -> Res<()> {
    let v: &[u8] = if on { b"true" } else { b"false" };
    let got = ask(
        ctx,
        cmd,
        &[b"shell", b"setPluginEnabled", DOCK_ID.as_bytes(), v],
    )?;
    if got != b"ok" {
        return Err(ctx.die(cat!(
            cmd,
            ": box '",
            ctx.name,
            "': its shell answered ",
            got,
            " to turning the dock ",
            if on { "on" } else { "off" }
        )));
    }
    Ok(())
}

/// Whether the dock's service is enabled in the box, as its shell.json says (a disabled one is in
/// disabledPlugins; the shell enables a first-party service by default).
pub fn dock_enabled(ctx: &Ctx) -> bool {
    let Some(p) = box_home_file(&ctx.d, ".config/omarchy/shell.json") else {
        return true;
    };
    let r = Args::new(&["-e"])
        .kv("--arg", "id", DOCK_ID)
        .arg(r#"(.disabledPlugins // []) | index([$id]) | not"#)
        .run(Some(&std::fs::read(p).unwrap_or_default()), jq::Err::Null);
    r.rc != 1
}

/// The mode Omarchy keeps for workspace ID in the box's HOME ("floating", "dwindle", ...), or
/// nothing when it keeps none (the workspace follows Hyprland's layout, never floating).
pub fn ws_mode(ctx: &Ctx, id: i64) -> Vec<u8> {
    let read = |f: &str, key: &str| -> Vec<u8> {
        let Some(p) = box_home_file(
            &ctx.d,
            &format!(".local/state/omarchy/workspace-layouts/{f}"),
        ) else {
            return Vec::new();
        };
        let text = std::fs::read(p).unwrap_or_default();
        let lead = format!("{key} = \"");
        for l in text.split(|&c| c == b'\n') {
            if let Some(i) = l.windows(lead.len()).position(|w| w == lead.as_bytes()) {
                let rest = &l[i + lead.len()..];
                let n = rest.iter().take_while(|c| c.is_ascii_lowercase()).count();
                if rest.get(n) == Some(&b'"') && n > 0 {
                    return rest[..n].to_vec();
                }
            }
        }
        Vec::new()
    };
    if id < 1 {
        return Vec::new();
    }
    let own = read(&format!("{id}.lua"), "mode");
    if !own.is_empty() {
        return own;
    }
    read("default.lua", "default")
}

/// The monitors (by name) of MONITORS (`hyprctl -j monitors`) whose dock is hidden: their active
/// workspace is not floating, so the dock there takes no input. A fullscreen window hides it too,
/// which WIN_JQ already knows (it drops top layers behind one).
pub fn dock_hidden_on(ctx: &Ctx, monitors: &[u8]) -> Vec<Vec<u8>> {
    let r = jq::jq(
        &["-r", ".[]? | \"\\(.activeWorkspace.id // 0) \\(.name)\""],
        Some(monitors),
        jq::Err::Null,
    );
    let mut hidden = Vec::new();
    if !r.ok() {
        return hidden;
    }
    for l in r.text().split(|&c| c == b'\n') {
        let Some(i) = l.iter().position(|&c| c == b' ') else {
            continue;
        };
        let id = crate::shell::dec(&l[..i]).unwrap_or(0);
        if ws_mode(ctx, id) != b"floating" {
            hidden.push(l[i + 1..].to_vec());
        }
    }
    hidden
}

/// LAYERS (`hyprctl -j layers`) without the dock on the monitors HIDDEN names: there it takes no
/// input, so no window is under it. LAYERS as it is when jq cannot read it.
pub fn drop_docks(layers: &[u8], hidden: &[Vec<u8>]) -> Vec<u8> {
    if hidden.is_empty() {
        return layers.to_vec();
    }
    let names: Vec<String> = hidden
        .iter()
        .map(|h| crate::json::quote(&String::from_utf8_lossy(h)))
        .collect();
    let r = Args::new(&["-c"])
        .kv("--argjson", "h", format!("[{}]", names.join(",")).as_bytes())
        .kv("--arg", "ns", DOCK_NS)
        .arg(
            r#"if type != "object" then . else with_entries(
                 if (.key as $k | $h | index([$k])) and (.value | type) == "object"
                    and (.value.levels | type) == "object"
                 then .value.levels |= map_values(if type == "array" then map(select(.namespace != $ns)) else . end)
                 else . end) end"#,
        )
        .run(Some(layers), jq::Err::Null);
    if r.ok() && !r.text().is_empty() {
        strip_nl(&r.text())
    } else {
        layers.to_vec()
    }
}

/// `hyprctl -j layers` for what a window's input meets (windows, click/scroll/drag/pointer
/// --window): a hidden dock taken out (#309). The monitors and the box's files are read only when
/// a dock is there.
pub fn input_layers(ctx: &Ctx) -> Res<Vec<u8>> {
    let lys = strip_nl(&crate::hypr::hypr_json(ctx, &["layers"])?);
    let quoted = cat!("\"", DOCK_NS, "\"");
    if !lys.windows(quoted.len()).any(|w| w == quoted.as_slice()) {
        return Ok(lys);
    }
    let m = strip_nl(&crate::hypr::hypr_json(ctx, &["monitors"])?);
    Ok(drop_docks(&lys, &dock_hidden_on(ctx, &m)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_dock_dropped_from_its_monitor_only() {
        let l = br#"{"A":{"levels":{"0":[],"2":[{"namespace":"omarchy-dock","x":0},{"namespace":"other"}]}},"B":{"levels":{"2":[{"namespace":"omarchy-dock"}]}}}"#;
        let out = drop_docks(l, &[b"A".to_vec()]);
        let s = String::from_utf8(out).unwrap();
        assert_eq!(
            s,
            r#"{"A":{"levels":{"0":[],"2":[{"namespace":"other"}]}},"B":{"levels":{"2":[{"namespace":"omarchy-dock"}]}}}"#
        );
        assert_eq!(drop_docks(l, &[]), l.to_vec());
        assert_eq!(
            drop_docks(b"not json", &[b"A".to_vec()]),
            b"not json".to_vec()
        );
    }
}
