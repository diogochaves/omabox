//! The desk shells omabox knows (#321): a desk box runs the user's own shell (DMS, Noctalia,
//! Caelestia, Waybar, a Quickshell config) in its slot, found by a profile each in
//! share/desk-shells/<name>. A profile is `key value` lines (`#` starts a comment line):
//!
//! - `start CMD`: what starts it when nothing in the user's config does (a systemd unit's shell);
//!   it is a `match` too. Once.
//! - `match CMD`: the words a command that starts it begins with, once env, exec, setsid and the
//!   like are taken off (step 4's normaliser). `{name}` stands for one word (a Quickshell config's).
//! - `unit NAME`: its systemd user unit, linked under the session targets' .wants/. Once.
//! - `config PATH`, `state PATH`, `cache PATH`: HOME-relative folders a desk box copies (step 2).
//! - `layer NS`: the layer namespace that shows it is up.
//! - `idle FILE KEY VALUE`: how its own idle timeouts are turned off in the copied FILE (step 5).
//!
//! Step 0 has the profiles and their parser; steps 2 (what to copy), 4 (the slot) and 5 (idle) use
//! them. OMABOX_DESK_SHELL_DIR, test-only and never a jailed caller's, reads another folder.

use crate::cat;
use crate::cmd::up::Up;
use crate::exit::{Ctx, Res};
use crate::jail::in_jail;
use crate::shell;

/// One desk shell, as its profile says it (the module's comment has the keys).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[allow(dead_code)] // (read by steps 2, 4 and 5 of #321)
pub struct Profile {
    pub name: Vec<u8>,
    pub start: Vec<u8>,
    pub matches: Vec<Vec<u8>>,
    pub unit: Vec<u8>,
    pub config: Vec<Vec<u8>>,
    pub state: Vec<Vec<u8>>,
    pub cache: Vec<Vec<u8>>,
    pub layer: Vec<Vec<u8>>,
    pub idle: Vec<Vec<u8>>,
}

/// A profile's name: a file name of lowercase letters, digits and `-`, not starting with `-`.
fn name_ok(n: &[u8]) -> bool {
    !n.is_empty()
        && n[0] != b'-'
        && n.iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// A HOME-relative folder: not absolute, no empty, `.` or `..` part.
fn rel_ok(p: &[u8]) -> bool {
    !p.is_empty()
        && p[0] != b'/'
        && p.split(|&c| c == b'/')
            .all(|w| !w.is_empty() && w != b"." && w != b"..")
}

/// parse NAME TEXT: the profile NAME's file read, or why not ("line N: ...").
pub fn parse(name: &[u8], text: &[u8]) -> Result<Profile, String> {
    let lossy = |v: &[u8]| String::from_utf8_lossy(v).into_owned();
    if !name_ok(name) {
        return Err(format!("not a profile's name: {}", lossy(name)));
    }
    let mut p = Profile {
        name: name.to_vec(),
        ..Profile::default()
    };
    for (i, raw) in text.split(|&c| c == b'\n').enumerate() {
        let line = raw.trim_ascii();
        if line.is_empty() || line[0] == b'#' {
            continue;
        }
        let n = i + 1;
        let (key, val) = match line.iter().position(|c| c.is_ascii_whitespace()) {
            Some(k) => (&line[..k], line[k..].trim_ascii()),
            None => (line, &b""[..]),
        };
        if val.is_empty() {
            return Err(format!("line {n}: {} has no value", lossy(key)));
        }
        let v = val.to_vec();
        let once = |slot: &mut Vec<u8>| {
            if slot.is_empty() {
                *slot = v.clone();
                Ok(())
            } else {
                Err(format!("line {n}: a second {}", lossy(key)))
            }
        };
        match key {
            b"start" => once(&mut p.start)?,
            b"unit" => once(&mut p.unit)?,
            b"match" => p.matches.push(v),
            b"layer" => p.layer.push(v),
            b"idle" => p.idle.push(v),
            b"config" | b"state" | b"cache" => {
                if !rel_ok(val) {
                    return Err(format!(
                        "line {n}: {} must be a folder in HOME, got {}",
                        lossy(key),
                        lossy(val)
                    ));
                }
                match key {
                    b"config" => p.config.push(v),
                    b"state" => p.state.push(v),
                    _ => p.cache.push(v),
                }
            }
            _ => return Err(format!("line {n}: no key {}", lossy(key))),
        }
    }
    if p.start.is_empty() && p.matches.is_empty() && p.unit.is_empty() {
        return Err("no start, match or unit: nothing finds it".into());
    }
    Ok(p)
}

/// dir: the profiles' folder, share/desk-shells; OMABOX_DESK_SHELL_DIR, the suite's, never a jailed
/// caller's.
#[allow(dead_code)] // (read by steps 2 and 4 of #321)
pub fn dir() -> Vec<u8> {
    match shell::env("OMABOX_DESK_SHELL_DIR") {
        Some(d) if !d.is_empty() && !in_jail() => d,
        _ => cat!(crate::paths::root(), "/share/desk-shells"),
    }
}

/// all DIR: every profile in DIR, by name; a file that is not one is an error (it ships with
/// omabox, so a broken one is omabox's bug, never skipped unseen).
#[allow(dead_code)] // (read by steps 2 and 4 of #321)
pub fn all(dir: &[u8]) -> Result<Vec<Profile>, String> {
    let lossy = |v: &[u8]| String::from_utf8_lossy(v).into_owned();
    let rd = std::fs::read_dir(shell::path(dir)).map_err(|e| format!("{}: {e}", lossy(dir)))?;
    let mut names: Vec<Vec<u8>> = rd
        .filter_map(|e| e.ok())
        .map(|e| shell::bytes(std::path::Path::new(&e.file_name())).to_vec())
        .filter(|n| !n.starts_with(b"."))
        .collect();
    names.sort();
    names
        .iter()
        .map(|n| {
            let f = cat!(dir, "/", n);
            let text = std::fs::read(shell::path(&f)).map_err(|e| format!("{}: {e}", lossy(&f)))?;
            parse(n, &text).map_err(|e| format!("{}: {e}", lossy(&f)))
        })
        .collect()
}

/// A desk box's shell, once the box is ready (#321 step 4). Nothing yet.
pub fn after_ready(_ctx: &Ctx, _u: &Up) -> Res<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(v: &[&str]) -> Vec<Vec<u8>> {
        v.iter().map(|s| s.as_bytes().to_vec()).collect()
    }

    #[test]
    fn parses_every_key() {
        let p = parse(
            b"x",
            b"# a comment\n\nstart  x run -d \nmatch qs -c x\nmatch quickshell -c x\nunit x.service\n\
              config .config/x\nstate .local/state/x\ncache .cache/x\nlayer x:bar\n\
              idle .config/x/a.json general.idle.timeouts []\n",
        )
        .unwrap();
        assert_eq!(p.name, b"x");
        assert_eq!(p.start, b"x run -d");
        assert_eq!(p.matches, w(&["qs -c x", "quickshell -c x"]));
        assert_eq!(p.unit, b"x.service");
        assert_eq!(p.config, w(&[".config/x"]));
        assert_eq!(p.state, w(&[".local/state/x"]));
        assert_eq!(p.cache, w(&[".cache/x"]));
        assert_eq!(p.layer, w(&["x:bar"]));
        assert_eq!(p.idle, w(&[".config/x/a.json general.idle.timeouts []"]));
    }

    #[test]
    fn refuses_what_is_not_a_profile() {
        let e = |name: &[u8], t: &str| parse(name, t.as_bytes()).unwrap_err();
        assert_eq!(e(b"x", "start a\nstart b\n"), "line 2: a second start");
        assert_eq!(e(b"x", "unit a\nunit b\n"), "line 2: a second unit");
        assert_eq!(e(b"x", "start a\nshell b\n"), "line 2: no key shell");
        assert_eq!(e(b"x", "start a\nlayer\n"), "line 2: layer has no value");
        assert_eq!(
            e(b"x", "config .config/x\n"),
            "no start, match or unit: nothing finds it"
        );
        for bad in [
            "/etc/x",
            "../x",
            ".config/../x",
            ".config//x",
            "./x",
            ".config/x/",
        ] {
            assert_eq!(
                e(b"x", &format!("start a\nconfig {bad}\n")),
                format!("line 2: config must be a folder in HOME, got {bad}")
            );
        }
        for bad in [&b""[..], b"-x", b"X", b"a/b", b"a b", b"a.b"] {
            assert!(parse(bad, b"start a\n").is_err(), "{bad:?}");
        }
    }

    /// The shipped profiles all parse, and each is found by something.
    #[test]
    fn shipped_profiles() {
        let d = concat!(env!("CARGO_MANIFEST_DIR"), "/../../share/desk-shells");
        let all = all(d.as_bytes()).unwrap();
        let names: Vec<&[u8]> = all.iter().map(|p| p.name.as_slice()).collect();
        assert_eq!(
            names,
            [
                &b"caelestia"[..],
                b"dms",
                b"noctalia",
                b"quickshell",
                b"waybar"
            ]
        );
        let dms = &all[1];
        assert_eq!(dms.start, b"dms run");
        assert_eq!(dms.unit, b"dms.service");
        assert_eq!(dms.config, w(&[".config/DankMaterialShell"]));
        assert_eq!(dms.state, w(&[".local/state/DankMaterialShell"]));
        assert_eq!(dms.layer, w(&["dms:bar"]));
        let qs = &all[3];
        assert_eq!(qs.matches, w(&["qs -c {name}", "quickshell -c {name}"]));
        assert_eq!(qs.config, w(&[".config/quickshell/{name}"]));
    }
}
