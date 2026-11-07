//! What an action's --wait saw on the box's desktop (#274): one batched hyprctl read before the
//! input and one after the answer (desk_snap), diffed by DESK_FACTS_JQ (desk_facts), and what got the
//! input (desk_target, findings 398, 421). "Since", never "caused by". As bin/omabox's, jq running
//! its programs.

use crate::exec::{ErrTo, Out, on_box};
use crate::exit::Ctx;
use crate::hypr::HYPR_LIMIT;
use crate::jq::{self, Args};
use crate::jqprog::{DESK_FACTS_JQ, DESK_TARGET_JQ, WIN_JQ};
use crate::shell;

/// events_log D: box D's events log, as the box sees it (in its HOME, box_home: #321).
pub fn events_log(d: &[u8]) -> Vec<u8> {
    crate::cat!(crate::boxes::box_home(d), "/events.log")
}
const DESK_BATCH: &str = "j/clients; j/layers; j/activewindow; j/activeworkspace; j/monitors";
const SNAP_SH: &str =
    r#"stat -L -c %s -- "$1" 2>/dev/null || echo -; exec timeout 2 hyprctl --batch "$2""#;

/// desk_snap: the desktop now as one JSON object {events (the events log's size, null when unread),
/// clients, layers, active, workspace, monitors}, or nothing when the box does not answer in 2 s.
/// Never dies: a desktop that could not be read leaves the answer as it was. STDIN_NULL: run in the
/// background, where bash gives it /dev/null and ignores SIGINT and SIGQUIT in it.
pub fn desk_snap(ctx: &Ctx, stdin_null: bool) -> Vec<u8> {
    let args: Vec<Vec<u8>> = vec![
        b"/usr/bin/sh".to_vec(),
        b"-c".to_vec(),
        SNAP_SH.as_bytes().to_vec(),
        b"sh".to_vec(),
        events_log(&ctx.d),
        DESK_BATCH.as_bytes().to_vec(),
    ];
    // (In the background: /dev/null its stdin, SIGINT and SIGQUIT ignored, as bash's `&` has it.)
    let r = if stdin_null {
        crate::exec::on_box_bg(
            ctx,
            &args,
            crate::exec::In::Null,
            Out::Capture(Some(67_108_864)),
            ErrTo::Null,
            None,
        )
    } else {
        on_box(
            ctx,
            &args,
            false,
            Out::Capture(Some(67_108_864)),
            ErrTo::Null,
        )
    };
    if r.rc != 0 {
        return Vec::new();
    }
    snap_of(&r.out)
}

/// desk_snap's reading of what its call printed: the events log's size, then hyprctl --batch's five
/// JSON values; nothing when they are not five.
fn snap_of(printed: &[u8]) -> Vec<u8> {
    let out = shell::subst(printed);
    let (size, js) = match out.iter().position(|&c| c == b'\n') {
        Some(i) => (&out[..i], &out[i + 1..]),
        None => (&out[..], &out[..]),
    };
    Args::new(&["-c", "-s"])
        .kv("--arg", "s", size)
        .arg(crate::jqprog::SNAP_JQ)
        .run(Some(js), jq::Err::Null)
        .text()
}

/// SNAPSHOT marked `toasts: false` when it lists the notifications layer while the box's shell shows
/// no toast (#431, finding 685): DESK_TARGET_JQ then leaves that layer out of what a click, scroll or
/// drag may have met, as input_layers does for --window. As it was otherwise.
pub fn judge_toasts(ctx: &Ctx, snap: Vec<u8>) -> Vec<u8> {
    if snap.is_empty() || !crate::omshell::toasts_idle(ctx, &snap) {
        return snap;
    }
    let r = jq::jq(&["-c", ".toasts = false"], Some(&snap), jq::Err::Null);
    if r.ok() { r.text() } else { snap }
}

/// desk_facts BEFORE AFTER: DESK_FACTS_JQ's {changes, line}, or nothing when either is not a
/// snapshot.
pub fn desk_facts(before: &[u8], after: &[u8]) -> Vec<u8> {
    if before.is_empty() || after.is_empty() {
        return Vec::new();
    }
    let r = jq::jq(&["-r", ".events // empty"], Some(before), jq::Err::Null);
    let since = if r.ok() { r.text() } else { Vec::new() };
    Args::new(&["-c", "-n"])
        .kv("--argjson", "b", before)
        .kv("--argjson", "a", after)
        .kv("--arg", "since", since)
        .arg(DESK_FACTS_JQ)
        .run(None, jq::Err::Null)
        .text()
}

/// What got the input (desk_target's WHAT): keys, run, or a point's (click, scroll, drag).
pub enum Target<'a> {
    Keys,
    Run(&'a [u8]),
    At(&'a str, &'a [u8]),
}

/// desk_target WHAT SNAPSHOT [X Y | PID]: DESK_TARGET_JQ's object, or nothing; keys' layers holding
/// the keyboard now read by KBL_OF (kb_layers).
pub fn desk_target_with(what: Target, s: &[u8], kbl_of: impl FnOnce() -> Vec<u8>) -> Vec<u8> {
    if s.is_empty() {
        return Vec::new();
    }
    let (mut kbl, mut x, mut y, mut pid) =
        (Vec::new(), b"0".to_vec(), b"0".to_vec(), b"0".to_vec());
    let name = match what {
        Target::Keys => {
            kbl = kbl_of();
            "keys"
        }
        Target::Run(p) => {
            if !p.is_empty() {
                pid = p.to_vec();
            }
            "run"
        }
        Target::At(n, xy) => {
            // (`read -r x y <<<"$3"`)
            let f = shell::read_fields(xy, 2);
            (x, y) = (f[0].clone(), f[1].clone());
            n
        }
    };
    let num = |v: &[u8]| shell::is_num(v);
    if !(num(&x) && num(&y) && num(&pid)) {
        return Vec::new();
    }
    let prog = DESK_TARGET_JQ.replacen("(WIN)", &format!("({WIN_JQ})"), 1);
    Args::new(&["-c", "-n"])
        .kv("--argjson", "s", s)
        .kv("--arg", "what", name)
        .kv("--arg", "kbl", kbl)
        .kv("--argjson", "x", x)
        .kv("--argjson", "y", y)
        .kv("--argjson", "pid", pid)
        .arg(prog)
        .run(None, jq::Err::Null)
        .text()
}

// A layer's keyboard interactivity: `interactivity` until Hyprland 0.57, `keyboard_interactivity`
// since (finding 647); the other name is nil on each.
const KBL_LUA: &str = "local e = {}
for _, l in ipairs(hl.get_layers()) do
  local k = l.keyboard_interactivity or l.interactivity
  if l.mapped and k == 1 then e[#e + 1] = (l.namespace ~= '' and l.namespace) or tostring(l.address) end
end
error('omabox-kbl:' .. table.concat(e, ', '), 0)";

/// kb_layers: the mapped layers that hold the keyboard against any window (#201, finding 276),
/// ", " between; empty too when Hyprland did not answer as asked.
pub fn kb_layers(ctx: &Ctx) -> Vec<u8> {
    let args: Vec<Vec<u8>> = ["timeout", HYPR_LIMIT, "hyprctl", "eval", KBL_LUA]
        .iter()
        .map(|w| w.as_bytes().to_vec())
        .collect();
    let r = on_box(ctx, &args, false, Out::Capture(None), ErrTo::Out);
    let out = shell::subst(&r.out);
    out.strip_prefix(b"error: omabox-kbl:".as_slice())
        .map(<[u8]>::to_vec)
        .unwrap_or_default()
}

const KBT_LUA: &str = "local e = {}
for _, l in ipairs(hl.get_layers()) do
  local k = l.keyboard_interactivity or l.interactivity
  if l.mapped and k and k ~= 0 then e[#e + 1] = tostring(l.address) .. '=' .. k end
end
error('omabox-kbt:' .. table.concat(e, ', '), 0)";

/// kb_takers: the mapped layers that may take the keyboard, `ADDRESS=INTERACTIVITY` (1 exclusive,
/// 2 on demand), ", " between; empty too when Hyprland did not answer as asked (#315).
pub fn kb_takers(ctx: &Ctx) -> Vec<u8> {
    let args: Vec<Vec<u8>> = ["timeout", HYPR_LIMIT, "hyprctl", "eval", KBT_LUA]
        .iter()
        .map(|w| w.as_bytes().to_vec())
        .collect();
    let r = on_box(ctx, &args, false, Out::Capture(None), ErrTo::Out);
    let out = shell::subst(&r.out);
    out.strip_prefix(b"error: omabox-kbt:".as_slice())
        .map(<[u8]>::to_vec)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // t_unit_wait_facts': desk_snap's own check of what hyprctl --batch printed.
    #[test]
    fn snap_of_as_bash() {
        assert_eq!(
            snap_of(b"7\n[]\n{}\n{}\n{\"id\":1}\n[]\n"),
            br#"{"events":7,"clients":[],"layers":{},"active":{},"workspace":{"id":1},"monitors":[]}"#,
            "desk_snap: five values read as a snapshot"
        );
        assert_eq!(
            snap_of(b"-\nIPC did not respond\n"),
            b"",
            "...an error from hyprctl: nothing"
        );
    }

    /// KBL_LUA and KBT_LUA run by a host Lua against a stand-in `hl.get_layers()`, layers shaped as
    /// HYPR's (`interactivity` up to 0.56, `keyboard_interactivity` since 0.57; finding 647): the
    /// error they end with. None without a Lua on the host.
    fn lua_layers(prog: &str, key: &str) -> Option<String> {
        let stub = format!(
            "hl = {{ get_layers = function() return {{
               {{ mapped = true, namespace = 'launcher', address = '0xa', {key} = 1 }},
               {{ mapped = true, namespace = 'bar', address = '0xb', {key} = 0 }},
               {{ mapped = true, namespace = '', address = '0xc', {key} = 2 }},
               {{ mapped = false, namespace = 'gone', address = '0xd', {key} = 1 }} }} end }}
             local ok, e = pcall(function() {prog} end) io.write(tostring(e))"
        );
        let out = std::process::Command::new("lua")
            .args(["-e", &stub])
            .output()
            .ok()?;
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    #[test]
    fn layers_keyboard_on_each_hyprland() {
        for key in ["interactivity", "keyboard_interactivity"] {
            let Some(kbl) = lua_layers(KBL_LUA, key) else {
                return;
            };
            assert_eq!(kbl, "omabox-kbl:launcher", "kb_layers with {key}");
            let kbt = lua_layers(KBT_LUA, key).unwrap_or_default();
            assert_eq!(kbt, "omabox-kbt:0xa=1, 0xc=2", "kb_takers with {key}");
        }
    }
}
