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

/// EVENTS_LOG: the box's events log, as the box sees it.
pub const EVENTS_LOG: &str = "/home/sbx/events.log";
const DESK_BATCH: &str = "j/clients; j/layers; j/activewindow; j/activeworkspace; j/monitors";
const SNAP_SH: &str =
    r#"stat -L -c %s -- "$1" 2>/dev/null || echo -; exec timeout 2 hyprctl --batch "$2""#;

/// desk_snap: the desktop now as one JSON object {events (the events log's size, null when unread),
/// clients, layers, active, workspace, monitors}, or nothing when the box does not answer in 2 s.
/// Never dies: a desktop that could not be read leaves the answer as it was. STDIN_NULL: run in the
/// background, where bash gives it /dev/null and ignores SIGINT and SIGQUIT in it.
pub fn desk_snap(ctx: &Ctx, stdin_null: bool) -> Vec<u8> {
    let args: Vec<Vec<u8>> = ["/usr/bin/sh", "-c", SNAP_SH, "sh", EVENTS_LOG, DESK_BATCH]
        .iter()
        .map(|w| w.as_bytes().to_vec())
        .collect();
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

const KBL_LUA: &str = "local e = {}
for _, l in ipairs(hl.get_layers()) do
  if l.mapped and l.interactivity == 1 then e[#e + 1] = (l.namespace ~= '' and l.namespace) or tostring(l.address) end
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
  if l.mapped and l.interactivity ~= 0 then e[#e + 1] = tostring(l.address) .. '=' .. l.interactivity end
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
}
