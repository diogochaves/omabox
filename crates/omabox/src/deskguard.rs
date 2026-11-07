//! A desk box's guards (#321 step 5, finding 704): what up refuses for a desk box beyond step 1's
//! (by whether the desk is Omarchy's, and a Hyprland config omabox can drive), and what holds once it
//! is up: its idle, held off by tools/idlehold, so the user's own shell's idle timeouts never lock,
//! blank or suspend it under an agent that sends no input between its steps.

use crate::boxes::run_read;
use crate::cat;
use crate::cmd::up::Up;
use crate::exit::{Ctx, Res, say};
use crate::shell;
use crate::upspec::DeskBase;

/// What an Omarchy desk refuses (an Omarchy machine whose Hyprland config loads Omarchy's, finding
/// 708): what gives Omarchy's stock desktop, or part of it, in place of the user's. --plugin, --theme,
/// --theme-dir, --dnd, --no-dock and --omarchy are the desk's own Omarchy's, allowed. A plain desk
/// refuses what needs Omarchy as a plain box does (up.rs plain_refusals, later in checks); --clean
/// and --desk-shell are step 1's, refused for both before the base is known (up.rs desk_refusal).
fn flag_refusal(u: &Up, base: DeskBase) -> Option<Vec<u8>> {
    if base != DeskBase::Omarchy {
        return None;
    }
    let (opt, what): (&str, &str) = if u.stock {
        ("--stock", "a new Omarchy user's desktop")
    } else if u.stock_bar {
        ("--stock-bar", "Omarchy's own bar")
    } else if !u.clean_keep.is_empty() {
        ("--keep", "part of a clean Omarchy desktop")
    } else {
        return None;
    };
    Some(cat!(
        "up: ",
        opt,
        " is not for a desk box: it gives ",
        what,
        ", not your desk (--desktop omarchy for that)"
    ))
}

/// The folder the user's Hyprland reads its config from: $XDG_CONFIG_HOME/hypr when that is
/// absolute, else ~/.config/hypr (as up.rs user_hypr_lua has it).
fn hypr_dir() -> Vec<u8> {
    let x = shell::env_or_empty("XDG_CONFIG_HOME");
    let base = if x.starts_with(b"/") {
        x
    } else {
        cat!(shell::env_or_empty("HOME"), "/.config")
    };
    cat!(base, "/hypr")
}

/// What a desk's Hyprland config is: Lua, only the old hyprland.conf, or none.
#[derive(Debug, PartialEq, Eq)]
enum Config {
    Lua,
    ConfOnly,
    None,
}

fn config_in(dir: &[u8]) -> Config {
    let has = |f: &str| shell::path(&cat!(dir, "/", f)).is_file();
    if has("hyprland.lua") {
        Config::Lua
    } else if has("hyprland.conf") {
        Config::ConfOnly
    } else {
        Config::None
    }
}

/// config_refusal DIR HOME: why a desk box cannot run the config in DIR (HOME's prefix said as ~).
fn config_refusal(dir: &[u8], home: &[u8]) -> Option<Vec<u8>> {
    match config_in(dir) {
        Config::Lua => None,
        Config::ConfOnly => Some(
            b"up: --desktop desk needs a Lua config: Hyprland 0.56 still reads hyprland.conf, but `hyprctl eval` refuses it, and keys, click, pointer, mode, monitor and lua need that"
                .to_vec(),
        ),
        Config::None => {
            let shown = match dir.strip_prefix(home) {
                Some(rest) if home.len() > 1 && rest.starts_with(b"/") => cat!("~", rest),
                _ => dir.to_vec(),
            };
            Some(cat!("up: --desktop desk: no Hyprland config in ", shown))
        }
    }
}

/// checks U JAILED: up's checks for a desk box, once its HOME and base are known (up.rs checks).
/// The flags its base refuses (usage errors), then the config it needs. (A jailed caller's desk box
/// is refused before, by step 1.)
pub fn checks(ctx: &Ctx, u: &mut Up, _jailed: bool) -> Res<()> {
    if let Some(why) = flag_refusal(u, u.desk_base) {
        return Err(ctx.usage_die(why));
    }
    if let Some(why) = config_refusal(&hypr_dir(), &shell::env_or_empty("HOME")) {
        return Err(ctx.die(why));
    }
    Ok(())
}

/// The idle hold's launcher in the box: omabox-idlehold as a session of its own (the box's pid
/// namespace: it ends with the box), its words in the runtime dir's omabox-idlehold.log. Exit 3 when
/// the box has no idle hold (tools/idlehold not built when it started).
const HOLD_SH: &str = r#"[ -x "$1" ] || exit 3
exec /usr/bin/setsid -f "$1" > "$XDG_RUNTIME_DIR/omabox-idlehold.log" 2>&1 < /dev/null"#;

/// What the hold's log says of its start: Some(true) holding, Some(false) it failed (its words in
/// the log), None not yet.
fn hold_said(log: &[u8]) -> Option<bool> {
    if crate::exit::contains(log, b"omabox-idlehold: holding idle off on ") {
        return Some(true);
    }
    // (any other line is its refusal or failure: it says nothing else before it holds)
    log.contains(&b'\n').then_some(false)
}

/// A desk box's guards once it is ready (up_box.rs make, after its monitors): its idle held off.
/// Never fails the box: a hold that could not start is said as a warning.
pub fn after_ready(ctx: &mut Ctx, _u: &Up) -> Res<()> {
    let argv: Vec<Vec<u8>> = vec![
        b"/usr/bin/sh".to_vec(),
        b"-c".to_vec(),
        HOLD_SH.as_bytes().to_vec(),
        b"omabox-idlehold".to_vec(),
        crate::cmd::up_box::BOX_IDLEHOLD.as_bytes().to_vec(),
    ];
    let rc = crate::cmd::record::sys_q(ctx, &argv)?;
    let name = ctx.name.clone();
    let warn = |why: &[u8]| {
        say(cat!(
            "warning: box '",
            name,
            "' has no idle hold (",
            why,
            "): your shell's own idle timeouts may lock or blank it"
        ));
    };
    if rc == 3 {
        warn(&cat!(
            "tools/idlehold was not built: make -C ",
            crate::paths::root(),
            "/tools/idlehold, then up again"
        ));
        return Ok(());
    }
    if rc != 0 {
        warn(b"it could not be started");
        return Ok(());
    }
    // Its first holds in place before up says the box is ready: a hold takes one round trip or three.
    for _ in 0..60 {
        let log = run_read(&ctx.d, "omabox-idlehold.log").unwrap_or_default();
        match hold_said(&log) {
            Some(true) => return Ok(()),
            Some(false) => {
                let first = log.split(|&c| c == b'\n').next().unwrap_or_default();
                warn(&crate::term::plain(&first[..first.len().min(200)]));
                return Ok(());
            }
            None => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    }
    warn(&cat!(
        "it said nothing in 3 s; its log: ",
        ctx.d,
        "/run/omabox-idlehold.log"
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: Option<Vec<u8>>) -> String {
        String::from_utf8(v.unwrap_or_default()).unwrap()
    }

    // #321 step 5: an Omarchy desk takes its own Omarchy's options and refuses Omarchy's stock
    // desktop; a plain desk leaves them to plain_refusals.
    #[test]
    fn desk_flags_by_machine() {
        let u = Up {
            plugin_args: vec![b"x".to_vec()],
            theme: b"t".to_vec(),
            theme_dirs: vec![b"/d".to_vec()],
            dnd: true,
            nodock: true,
            omarchy: b"/o".to_vec(),
            ..Up::default()
        };
        assert_eq!(flag_refusal(&u, DeskBase::Omarchy), None);
        assert_eq!(flag_refusal(&u, DeskBase::Plain), None);
        for (set, opt) in [
            (
                0,
                "--stock is not for a desk box: it gives a new Omarchy user's desktop, not your desk",
            ),
            (
                1,
                "--stock-bar is not for a desk box: it gives Omarchy's own bar, not your desk",
            ),
            (
                2,
                "--keep is not for a desk box: it gives part of a clean Omarchy desktop, not your desk",
            ),
        ] {
            let mut u = Up::default();
            match set {
                0 => u.stock = true,
                1 => u.stock_bar = true,
                _ => u.clean_keep = b"bar".to_vec(),
            }
            assert_eq!(
                s(flag_refusal(&u, DeskBase::Omarchy)),
                format!("up: {opt} (--desktop omarchy for that)")
            );
            assert_eq!(flag_refusal(&u, DeskBase::Plain), None, "{opt}");
        }
    }

    #[test]
    fn desk_config_lua_conf_or_none() {
        let base = std::env::temp_dir().join(format!("omabox-deskguard-{}", std::process::id()));
        let dir = base.join("home/.config/hypr");
        std::fs::create_dir_all(&dir).unwrap();
        let d = shell::bytes(&dir).to_vec();
        let home = shell::bytes(&base.join("home")).to_vec();
        assert_eq!(
            s(config_refusal(&d, &home)),
            "up: --desktop desk: no Hyprland config in ~/.config/hypr"
        );
        // (a HOME that is not its prefix: the path as it is; nor a HOME of / or one sharing a prefix)
        assert_eq!(
            s(config_refusal(&d, b"/elsewhere")),
            format!(
                "up: --desktop desk: no Hyprland config in {}",
                dir.display()
            )
        );
        assert!(s(config_refusal(&d, &home[..home.len() - 1])).contains(&*dir.to_string_lossy()));
        std::fs::write(dir.join("hyprland.conf"), "").unwrap();
        assert!(s(config_refusal(&d, &home)).starts_with(
            "up: --desktop desk needs a Lua config: Hyprland 0.56 still reads hyprland.conf, but `hyprctl eval` refuses it"
        ));
        std::fs::write(dir.join("hyprland.lua"), "").unwrap();
        assert_eq!(config_refusal(&d, &home), None);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_hold_said_by_its_log() {
        assert_eq!(hold_said(b""), None);
        assert_eq!(hold_said(b"omabox-idlehold: hold"), None);
        assert_eq!(
            hold_said(b"omabox-idlehold: holding idle off on 1 of 1 outputs\n"),
            Some(true)
        );
        assert_eq!(
            hold_said(
                b"omabox-idlehold: the compositor has no layer shell (zwlr_layer_shell_v1)\n"
            ),
            Some(false)
        );
    }
}
