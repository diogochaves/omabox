//! `look theme DIR|NAME`: the box on the theme, its screen and a terminal with its palette shot, the
//! theme's checks.

use super::*;

/// up's notes on a theme (`omabox: note: theme NAME: LINE`, #462): each LINE.
pub fn theme_notes(err: &[u8]) -> Vec<Vec<u8>> {
    err.split(|&c| c == b'\n')
        .filter_map(|l| l.strip_prefix(b"omabox: note: theme "))
        .filter_map(|l| {
            let i = l.windows(2).position(|w| w == b": ")?;
            Some(l[i + 2..].to_vec())
        })
        .collect()
}

/// The terminal look theme opens: the theme's 16 colours, as text and as blocks, on its background.
pub(super) const PALETTE_SH: &str = r#"printf '\033[?25l\n  omabox look theme: the terminal palette\n\n'; for row in 0 8; do printf '  '; for i in 0 1 2 3 4 5 6 7; do n=$((row + i)); printf '\033[38;5;%dm %-6s\033[0m' "$n" "c$n"; done; printf '\n  '; for i in 0 1 2 3 4 5 6 7; do printf '\033[48;5;%dm       \033[0m' "$((row + i))"; done; printf '\n\n'; done; printf '  normal \033[1mbold\033[0m \033[2mdim\033[0m \033[3mitalic\033[0m \033[7mreverse\033[0m\n'; exec sleep 600"#;

pub(super) fn theme(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let mut a = parse(ctx, args, "theme")?;
    let given = a.dir.clone().unwrap_or_default();
    let dir = crate::repo::readlink_f(&given).unwrap_or_else(|| given.clone());
    // A folder is the theme in the works; a bare word an installed theme's NAME (up resolves it).
    let dir = if shell::path(&dir).is_dir() {
        Some(dir)
    } else if !given.contains(&b'/') && !given.is_empty() {
        None
    } else {
        return Err(ctx.die(cat!("look theme: ", given, " is no folder")));
    };
    // A folder with no palette whose subfolder is the theme (#478: `look theme .` beside `./ember`).
    if let Some(d) = &dir {
        let colors = |d: &[u8]| shell::path(&cat!(d, "/colors.toml")).is_file();
        let subs = subfolders_with(&given, &colors);
        if !colors(d) && !subs.is_empty() {
            return Err(ctx.die(cat!(
                "look theme: ",
                given,
                " has no colors.toml: not a theme's folder",
                subfolder_hint("theme", args, &given, &subs)
            )));
        }
    }
    let me = std::process::id();
    let (name, guard) = pick(ctx, &mut a, me)?;
    let r = theme_in(ctx, &a, dir.as_deref(), &given, &name, me);
    if let Some(g) = guard {
        g.finish();
    }
    r
}

pub(super) fn theme_in(
    ctx: &Ctx,
    a: &App,
    dir: Option<&[u8]>,
    given: &[u8],
    name: &[u8],
    me: u32,
) -> Res<()> {
    let mut l = Look::new(a.keep.then_some(name));
    // Live from DIR (--theme-dir), applied before the session starts (--theme, by its folder's name);
    // an installed one by its NAME, checked by up itself (its notes).
    let tname = match dir {
        Some(d) => d.rsplit(|&c| c == b'/').next().unwrap_or(d).to_vec(),
        None => given.to_vec(),
    };
    let extra: Vec<&[u8]> = match dir {
        Some(d) => vec![b"--theme-dir", d, b"--theme", &tname],
        None => vec![b"--theme", &tname],
    };
    let up_err = start(ctx, a, name, me, &extra, &mut l)?;
    // In a box that was up: the theme set there, as `omarchy-theme-set` does on a desk, and said (the
    // box stays on it). A folder is live only in a box started with it (--theme-dir): any other box
    // would show an installed theme of that name, or none.
    let mut own_named: Vec<Vec<u8>> = Vec::new();
    if a.own {
        let meta: Value = std::fs::read(ctx.at("box.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(Value::Null);
        let dirs: Vec<Vec<u8>> = meta
            .get("theme_dirs")
            .and_then(Value::as_array)
            .map(|v| {
                v.iter()
                    .filter_map(|d| d.as_str().map(|d| d.as_bytes().to_vec()))
                    .collect()
            })
            .unwrap_or_default();
        let real = |d: &[u8]| crate::repo::readlink_f(d).unwrap_or_else(|| d.to_vec());
        if let Some(d) = dir {
            if !dirs.iter().any(|b| real(b) == real(d)) {
                return Err(ctx.die(cat!(
                    "look theme: box '",
                    name,
                    "' does not have ",
                    d,
                    " as a live theme (it was not started with --theme-dir): omabox look theme ",
                    given,
                    " --fresh looks at it in a box of its own"
                )));
            }
        }
        let was = meta.get("theme").and_then(Value::as_str).unwrap_or("");
        let r = ob(&[b"run", b"-b", name, b"--", b"omarchy-theme-set", &tname]);
        if !r.ok {
            let _ = std::io::Write::write_all(&mut std::io::stderr(), &r.err);
            return Err(ctx.die(cat!(
                "look theme: omarchy-theme-set ",
                tname,
                " failed in box '",
                name,
                "'"
            )));
        }
        l.note(cat!(
            "the box is on theme ",
            tname,
            " from now on",
            if was.is_empty() || was.as_bytes() == tname.as_slice() {
                String::new()
            } else {
                format!(" (it started on {was}: omabox run -- omarchy-theme-set {was})")
            }
        ));
        if dir.is_none() {
            let tree = meta
                .get("omarchy")
                .and_then(Value::as_str)
                .filter(|t| t.starts_with('/'))
                .unwrap_or("/usr/share/omarchy");
            own_named = crate::plugins::theme_folders(
                &tname,
                tree.as_bytes(),
                &dirs,
                &shell::env_or_empty("HOME"),
                &crate::jailpath::jail_path,
            );
        }
    }
    let still: [&[u8]; 6] = [b"wait", b"-b", name, b"--timeout", b"8s", b"still"];
    if ob(&still).code == 124 {
        l.note(
            b"still changing after 8s (an intro video?): the shot is of a moving screen".to_vec(),
        );
    }
    let mut sh: Vec<&[u8]> = vec![b"shot", b"-b", name];
    if let Some(o) = &a.out {
        sh.extend([b"-o".as_slice(), o]);
    }
    let r = ob(&sh);
    let shot = if r.ok {
        last_line(&r.out)
    } else {
        l.problem(cat!("shot: ", shell::subst(&r.err)));
        None
    };
    // A terminal with its palette: the colours a theme is mostly looked at for.
    let mut term = None;
    let mut palette_pid: Option<Vec<u8>> = None;
    let r = ob(&[
        b"run",
        b"-b",
        name,
        b"-d",
        b"-q",
        b"--replace",
        b"--",
        b"foot",
        b"-T",
        b"omabox-palette",
        b"sh",
        b"-c",
        PALETTE_SH.as_bytes(),
    ]);
    if r.ok {
        palette_pid = last_line(&r.out).filter(|p| p.iter().all(u8::is_ascii_digit));
    }
    if r.ok
        && ob(&[
            b"wait",
            b"-b",
            name,
            b"--timeout",
            b"8s",
            b"window",
            b"title:^omabox-palette$",
        ])
        .ok
    {
        ob(&[
            b"wait",
            b"-b",
            name,
            b"--timeout",
            b"5s",
            b"still",
            b"--window",
            b"title:^omabox-palette$",
        ]);
        let out_t = a.out.as_ref().map(|o| suffixed(o, "terminal"));
        let mut sh: Vec<&[u8]> = vec![b"shot", b"-b", name, b"--window", b"title:^omabox-palette$"];
        if let Some(o) = &out_t {
            sh.extend([b"-o".as_slice(), o]);
        }
        let r = ob(&sh);
        term = if r.ok { last_line(&r.out) } else { None };
    } else {
        l.note(b"no terminal shot: foot did not open in the box".to_vec());
    }
    // The theme's checks (#462), on this box's screen.
    let mon: Value =
        serde_json::from_slice(&ob(&[b"hyprctl", b"-b", name, b"-j", b"monitors"]).out)
            .unwrap_or(Value::Null);
    let m = mon
        .as_array()
        .and_then(|v| v.first())
        .cloned()
        .unwrap_or(Value::Null);
    let n = |k: &str| m.get(k).and_then(Value::as_i64).unwrap_or(0);
    let screen = if n("width") > 0 {
        (n("width"), n("height"))
    } else {
        (1920, 1080)
    };
    match dir {
        Some(d) => l.problems.extend(
            crate::theme_check::check(d, screen)
                .into_iter()
                .map(String::into_bytes),
        ),
        None if a.own => {
            // An installed theme's checks, as up makes them for --theme NAME.
            l.problems.extend(
                crate::theme_check::check_layered(&own_named, screen)
                    .into_iter()
                    .map(String::into_bytes),
            );
        }
        None => l.problems.extend(theme_notes(&up_err)),
    }
    // The palette terminal is the look's own: gone from a box that stays up.
    if a.own {
        if let Some(pid) = palette_pid {
            ob(&[b"stop", b"-b", name, &pid]);
        }
    }
    l.head("screenshot", shot.as_deref().unwrap_or(b"none"));
    if let Some(t) = &term {
        l.head("terminal", t);
    }
    l.head("theme", &tname);
    l.own_box(ctx, a.own, name);
    l.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_on_a_theme() {
        let e = b"omabox: box 'x' up (1920x1080) in 2s\nomabox: note: theme tally: colors.toml: no accent (x: y)\nomabox: note: theme tally: background 1.png: 640x480\nomabox: note: something else\n";
        assert_eq!(
            theme_notes(e),
            [
                b"colors.toml: no accent (x: y)".to_vec(),
                b"background 1.png: 640x480".to_vec()
            ]
        );
        assert!(theme_notes(b"").is_empty());
    }
}
