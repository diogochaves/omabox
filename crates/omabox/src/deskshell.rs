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

/// parts CMD: the shell command CMD split into its parts (on `&&`, `||`, `;`, `|`, `&`, newlines and
/// parentheses), each its words with quotes taken off as sh takes them. share/desk-shell.lua's
/// M.parts, which takes the commands in the box: the two read a command the same way.
pub fn parts(s: &[u8]) -> Vec<Vec<Vec<u8>>> {
    let (mut parts, mut words, mut w, mut have) = (Vec::new(), Vec::new(), Vec::new(), false);
    let n = s.len();
    let mut i = 0;
    let word = |words: &mut Vec<Vec<u8>>, w: &mut Vec<u8>, have: &mut bool| {
        if *have {
            words.push(std::mem::take(w));
        }
        w.clear();
        *have = false;
    };
    while i < n {
        let c = s[i];
        match c {
            b'\'' => {
                let j = s[i + 1..]
                    .iter()
                    .position(|&d| d == b'\'')
                    .map_or(n, |k| i + 1 + k);
                w.extend_from_slice(&s[i + 1..j]);
                have = true;
                i = j + 1;
            }
            b'"' => {
                i += 1;
                while i < n && s[i] != b'"' {
                    if s[i] == b'\\' && i + 1 < n && matches!(s[i + 1], b'\\' | b'"' | b'$' | b'`')
                    {
                        i += 1;
                    }
                    w.push(s[i]);
                    i += 1;
                }
                have = true;
                i += 1;
            }
            b'\\' => {
                if i + 1 < n && s[i + 1] != b'\n' {
                    w.push(s[i + 1]);
                    have = true;
                }
                i += 2;
            }
            b';' | b'|' | b'&' | b'\n' | b'(' | b')' => {
                word(&mut words, &mut w, &mut have);
                if !words.is_empty() {
                    parts.push(std::mem::take(&mut words));
                }
                i += 1;
            }
            _ if c.is_ascii_whitespace() => {
                word(&mut words, &mut w, &mut have);
                i += 1;
            }
            _ => {
                w.push(c);
                have = true;
                i += 1;
            }
        }
    }
    word(&mut words, &mut w, &mut have);
    if !words.is_empty() {
        parts.push(words);
    }
    parts
}

fn base(w: &[u8]) -> &[u8] {
    w.rsplit(|&c| c == b'/').next().unwrap_or(w)
}

/// `A=B`: a variable's assignment, as sh reads one before a command.
fn assign(w: &[u8]) -> bool {
    match w.iter().position(|&c| c == b'=') {
        Some(k) if k > 0 => {
            (w[0].is_ascii_alphabetic() || w[0] == b'_')
                && w[..k]
                    .iter()
                    .all(|c| c.is_ascii_alphanumeric() || *c == b'_')
        }
        _ => false,
    }
}

/// strip WORDS: what a part runs once what only starts it is taken off: `A=B` words, exec, env (its
/// -i, -u NAME, A=B), setsid and nohup, `uwsm app` / uwsm-app / app2unit (to their --), systemd-run
/// (to its --); `sh -c` / `bash -c` 'SCRIPT' is SCRIPT's parts, each stripped. desk-shell.lua's
/// M.strip.
pub fn strip(words: &[Vec<u8>]) -> Vec<Vec<Vec<u8>>> {
    let mut i = 0;
    let opts = |i: &mut usize| {
        while words
            .get(*i)
            .is_some_and(|w| w.first() == Some(&b'-') && w != b"--")
        {
            *i += 1;
        }
        if words.get(*i).is_some_and(|w| w == b"--") {
            *i += 1;
        }
    };
    while let Some(w) = words.get(i) {
        let b = base(w);
        if assign(w) || b == b"exec" || b == b"nohup" {
            i += 1;
        } else if b == b"setsid" {
            i += 1;
            while words.get(i).is_some_and(|w| w.first() == Some(&b'-')) {
                i += 1;
            }
        } else if b == b"env" {
            i += 1;
            while let Some(e) = words.get(i) {
                if e == b"-u" || e == b"--unset" {
                    i += 2;
                } else if e.first() == Some(&b'-') || assign(e) {
                    i += 1;
                } else {
                    break;
                }
            }
        } else if b == b"uwsm" && words.get(i + 1).is_some_and(|w| w == b"app") {
            i += 2;
            opts(&mut i);
        } else if b == b"uwsm-app" || b == b"app2unit" {
            i += 1;
            opts(&mut i);
        } else if b == b"systemd-run" {
            match words[i + 1..].iter().position(|w| w == b"--") {
                Some(k) => i += k + 2,
                None => break,
            }
        } else if matches!(b, b"sh" | b"bash" | b"dash" | b"zsh")
            && words.get(i + 1).is_some_and(|f| {
                f.len() > 1
                    && f[0] == b'-'
                    && f[1..].iter().all(|c| c.is_ascii_alphabetic())
                    && f.contains(&b'c')
            })
            && let Some(script) = words.get(i + 2)
        {
            return parts(script).iter().flat_map(|p| strip(p)).collect();
        } else {
            break;
        }
    }
    let rest = words.get(i..).unwrap_or_default();
    if rest.is_empty() {
        Vec::new()
    } else {
        vec![rest.to_vec()]
    }
}

/// profile_for CMD PROFILES: the profile whose `start` or a `match` a part of CMD begins with, once
/// stripped (its program by its name, then its words in order, a `{...}` word any one word); a
/// pattern with no `{...}` before any with one, so a named Quickshell config's own profile wins
/// over the generic one. desk-shell.lua's M.profile_for, which takes what the box runs.
pub fn profile_for<'a>(cmd: &[u8], profiles: &'a [Profile]) -> Option<&'a Profile> {
    let runs: Vec<Vec<Vec<u8>>> = parts(cmd).iter().flat_map(|p| strip(p)).collect();
    let wild = |w: &[u8]| w.len() >= 2 && w[0] == b'{' && w[w.len() - 1] == b'}';
    let fits = |argv: &[Vec<u8>], pat: &[&[u8]]| {
        argv.len() >= pat.len()
            && !pat.is_empty()
            && base(&argv[0]) == pat[0]
            && pat[1..]
                .iter()
                .zip(&argv[1..])
                .all(|(p, a)| wild(p) || p == a)
    };
    for want_wild in [false, true] {
        for p in profiles {
            let pats = (!p.start.is_empty())
                .then_some(&p.start)
                .into_iter()
                .chain(&p.matches);
            for pat in pats {
                let pw: Vec<&[u8]> = pat
                    .split(|c| c.is_ascii_whitespace())
                    .filter(|w| !w.is_empty())
                    .collect();
                if pw.iter().any(|w| wild(w)) != want_wild {
                    continue;
                }
                if runs.iter().any(|argv| fits(argv, &pw)) {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// A desk box's shell, once the box is ready (#321 step 4, finding 703): what its slot runs, said
/// (the profile it is, by omabox's own reading of the command the box wrote). A config that never
/// said what it started (no desk-shell.lua ran: ready took it for no shell) leaves the box with
/// none: box.json's shell false, so every command takes it for one with none, as a plain box
/// without --shell. `up --shell CMD` and --no-shell: nothing to do; nor on Omarchy's base, whose
/// shell is Omarchy's (finding 702).
pub fn after_ready(ctx: &Ctx, u: &Up) -> Res<()> {
    use crate::shellnote::{DESK_SHELL_CMD, DeskSlot, desk_slot, slot_cmd};
    if !u.shell || !u.shell_cmd.is_empty() || !u.plain_like() {
        return Ok(());
    }
    if crate::boxes::run_read(&ctx.d, DESK_SHELL_CMD).is_none() {
        let _ = crate::mon::jq_box_json(ctx, crate::jq::Args::new(&[".shell = false"]));
        return Ok(());
    }
    let cmd = slot_cmd(ctx);
    let raw = crate::boxes::run_read(&ctx.d, DESK_SHELL_CMD).unwrap_or_default();
    let name = all(&dir())
        .ok()
        .and_then(|ps| profile_for(&raw, &ps).map(|p| p.name.clone()));
    let from = match desk_slot(ctx) {
        DeskSlot::Guess => "guessed: your config starts none omabox knows",
        _ => "from your config",
    };
    crate::exit::say(cat!(
        "box '",
        ctx.name,
        "': its shell: ",
        cmd,
        " (",
        name.map(|n| cat!(n, ", ")).unwrap_or_default(),
        from,
        ")"
    ));
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
                b"quickshell-path",
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

    /// #321 step 4: which profile a command a config starts is, once what only starts it is off;
    /// as share/desk-shell.lua reads it (t_unit_desk_shell_lua runs the same cases there).
    #[test]
    fn profile_match() {
        let d = concat!(env!("CARGO_MANIFEST_DIR"), "/../../share/desk-shells");
        let ps = all(d.as_bytes()).unwrap();
        let of = |c: &str| {
            profile_for(c.as_bytes(), &ps).map(|p| String::from_utf8_lossy(&p.name).into_owned())
        };
        for (cmd, want) in PROFILE_CASES {
            assert_eq!(of(cmd).as_deref(), *want, "{cmd:?}");
        }
        // The words as sh takes them.
        let words = |c: &str| -> Vec<Vec<String>> {
            parts(c.as_bytes())
                .iter()
                .map(|p| {
                    p.iter()
                        .map(|w| String::from_utf8_lossy(w).into_owned())
                        .collect()
                })
                .collect()
        };
        assert_eq!(
            words(r#"a 'b c'"d\"e" f\ g && h|i;j"#),
            vec![
                vec!["a", r#"b cd"e"#, "f g"],
                vec!["h"],
                vec!["i"],
                vec!["j"]
            ]
        );
    }

    /// profile_match's cases (t_unit_desk_shell_lua runs them, and more, through desk-shell.lua).
    const PROFILE_CASES: &[(&str, Option<&str>)] = &[
        ("dms run", Some("dms")),
        ("env A=1 dms run", Some("dms")),
        ("A=1 B=2 exec dms run --session", Some("dms")),
        ("uwsm app -- dms run", Some("dms")),
        ("uwsm-app -- dms run", Some("dms")),
        ("app2unit -- waybar", Some("waybar")),
        ("systemd-run --user --scope -- waybar -c x", Some("waybar")),
        ("/usr/bin/waybar", Some("waybar")),
        ("setsid -f qs -c noctalia-shell", Some("noctalia")),
        ("nohup quickshell -c noctalia-shell &", Some("noctalia")),
        ("qs -c caelestia", Some("caelestia")),
        ("caelestia shell -d", Some("caelestia")),
        ("qs -c mybar", Some("quickshell")),
        ("qs -p /home/me/bar", Some("quickshell-path")),
        ("sh -c 'sleep 1 && dms run'", Some("dms")),
        ("bash -lc \"env -u X waybar\"", Some("waybar")),
        ("sleep 2; waybar", Some("waybar")),
        ("echo dms run", None),
        ("dms", None),
        ("dmsx run", None),
        ("qs", None),
        ("qs -c", None),
        ("hypridle", None),
        ("systemd-run --user waybar", None),
        ("", None),
    ];
}
