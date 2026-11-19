//! What a person would notice in an Omarchy theme (#462, finding 720): `colors.toml` missing or
//! missing a key Omarchy's templates read (no fallback resolving it), a value that is no `#rrggbb`,
//! text that cannot be read on its background, a background too small, nearly flat or far too bright
//! for the theme's mode. The palette is read as Omarchy reads it (`omarchy-theme-color`, its aliases
//! and fallbacks), the keys from Omarchy's own templates at run time (a new key needs no release),
//! the backgrounds through ImageMagick. Every threshold is one no stock theme of Omarchy 4.5 trips
//! (calibrated over all 23, NOTES 720): these warn, they never fail a box.

use crate::cat;
use crate::shell::{self, Re};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;

/// Omarchy's theme templates: where the keys it reads are named.
pub const TEMPLATES: &str = "/usr/share/omarchy/default/themed";

/// Contrast pairs a person reads (text, background), each with the ratio it warns under and the
/// range Omarchy 4.5's stock themes score (NOTES 720).
pub const PAIRS: &[(&str, &str, f64, &str)] = &[
    ("foreground", "background", 4.5, "6.7 to 21"),
    ("bright_foreground", "background", 4.5, "6.7 to 21"),
    ("light_foreground", "background", 3.0, "4.6 to 21"),
    ("accent", "background", 3.0, "3.1 to 15"),
    (
        "selection_foreground",
        "selection_background",
        4.5,
        "4.9 to 17",
    ),
];

/// A background smaller than this share of the screen on either side; flatter than this (standard
/// deviation of its grey, 0..1); brighter (mean, 0..1) than DARK_MAX for a dark theme, darker than
/// LIGHT_MIN for a light one. Stock 4.5: 0.80 of 1920 at least, 0.028, 0.63, 0.69.
pub const BG_SHARE: f64 = 0.75;
pub const BG_FLAT: f64 = 0.015;
pub const BG_DARK_MAX: f64 = 0.75;
pub const BG_LIGHT_MIN: f64 = 0.5;

/// The keys Omarchy's templates read, from their `{{ ... }}`: a key, `mix A B N%`'s two (`_strip` and
/// `_rgb` are forms of their key, `theme_type` the mode); a gradient's key has a fallback, so is not
/// needed (omarchy-theme-set-templates' rules).
pub fn template_keys(templates: &[u8]) -> Vec<String> {
    let Some(re) = Re::new(r"\{\{ *([a-z_]+)( +[^}]*)? *\}\}") else {
        return Vec::new();
    };
    let mut keys = std::collections::BTreeSet::new();
    let mut rest = templates;
    while let Some((s, e)) = re
        .captures(rest, true)
        .and_then(|g| g.first().copied().flatten())
    {
        let m = &rest[s..e];
        let inner = String::from_utf8_lossy(&m[2..m.len() - 2]).into_owned();
        let words: Vec<&str> = inner.split_whitespace().collect();
        // (a gradient's key has a fallback of its own: optional; mix's two colours are needed)
        let args: Vec<&str> = match words.first() {
            Some(&("mix" | "mix_strip" | "mix_rgb")) => {
                words.iter().skip(1).take(2).copied().collect()
            }
            Some(&("hypr_gradient" | "gradient_start" | "shell_gradient")) => vec![],
            Some(w) => vec![*w],
            None => vec![],
        };
        for w in args {
            let k = w
                .strip_suffix("_strip")
                .or_else(|| w.strip_suffix("_rgb"))
                .unwrap_or(w);
            if k.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                && !k.is_empty()
                && k != "theme_type"
            {
                keys.insert(k.to_string());
            }
        }
        rest = &rest[e.max(s + 1)..];
    }
    keys.into_iter().collect()
}

/// `omarchy-theme-color --file F --all|--raw`'s pairs.
pub fn palette(out: &[u8]) -> BTreeMap<String, String> {
    out.split(|&c| c == b'\n')
        .filter_map(|l| {
            let t = l.iter().position(|&c| c == b'\t')?;
            Some((
                String::from_utf8_lossy(&l[..t]).into_owned(),
                String::from_utf8_lossy(&l[t + 1..]).trim().to_string(),
            ))
        })
        .collect()
}

/// `#rrggbb`'s channels, 0..1.
pub fn rgb(hex: &str) -> Option<[f64; 3]> {
    let h = hex.strip_prefix('#')?;
    if h.len() != 6 || !h.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let c = |i: usize| {
        u8::from_str_radix(&h[i..i + 2], 16)
            .ok()
            .map(|v| f64::from(v) / 255.0)
    };
    Some([c(0)?, c(2)?, c(4)?])
}

/// WCAG's contrast ratio of two colours (1..21).
pub fn contrast(a: &str, b: &str) -> Option<f64> {
    let lum = |c: [f64; 3]| {
        let f = |v: f64| {
            if v <= 0.039_28 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2])
    };
    let (la, lb) = (lum(rgb(a)?), lum(rgb(b)?));
    Some((la.max(lb) + 0.05) / (la.min(lb) + 0.05))
}

/// The checks of the palette alone: RAW (defined) and ALL (resolved) as omarchy-theme-color gives
/// them, KEYS those Omarchy reads. Each a line for the report.
pub fn palette_warnings(
    raw: &BTreeMap<String, String>,
    all: &BTreeMap<String, String>,
    keys: &[String],
) -> Vec<String> {
    let mut w = Vec::new();
    let missing: Vec<&str> = keys
        .iter()
        .filter(|k| all.get(*k).is_none_or(|v| v.is_empty()))
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        w.push(format!(
            "colors.toml: no {} (Omarchy's templates use {}, and no fallback resolves {})",
            missing.join(", "),
            if missing.len() == 1 { "it" } else { "them" },
            if missing.len() == 1 { "it" } else { "them" }
        ));
    }
    for (k, v) in raw {
        // (mode is a word; Hyprland's borders take its own syntax, gradients and rgba())
        if k == "mode" || k == "theme_type" || k.starts_with("hyprland_") {
            continue;
        }
        if rgb(v).is_none() {
            w.push(format!(
                "colors.toml: {k} = \"{v}\" is no #rrggbb: Omarchy falls back for it"
            ));
        }
    }
    for (fg, bg, min, stock) in PAIRS {
        let (Some(a), Some(b)) = (all.get(*fg), all.get(*bg)) else {
            continue;
        };
        if let Some(r) = contrast(a, b) {
            if r < *min {
                w.push(format!(
                    "{fg} {a} on {bg} {b}: {r:.1}:1, under {min}:1 (stock themes: {stock})"
                ));
            }
        }
    }
    w
}

/// One background's measures: its size, its grey's mean and standard deviation (0..1).
pub struct Bg {
    pub w: i64,
    pub h: i64,
    pub mean: f64,
    pub std: f64,
}

/// ImageMagick's measures of FILE (its first frame; the grey of a 256 px sample, as calibrated).
pub fn measure(file: &[u8]) -> Option<Bg> {
    let mut f = file.to_vec();
    f.extend_from_slice(b"[0]");
    let o = std::process::Command::new("magick")
        .arg(OsStr::from_bytes(&f))
        .args([
            "-format",
            "%w %h ",
            "-write",
            "info:-",
            "-sample",
            "256x256",
            "-colorspace",
            "Gray",
        ])
        .args(["-format", "%[fx:mean] %[fx:standard_deviation]\n", "info:-"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let t = String::from_utf8_lossy(&o.stdout).into_owned();
    let v: Vec<&str> = t.split_whitespace().collect();
    Some(Bg {
        w: v.first()?.parse().ok()?,
        h: v.get(1)?.parse().ok()?,
        mean: v.get(2)?.parse().ok()?,
        std: v.get(3)?.parse().ok()?,
    })
}

/// A background's warnings (NAME its file name), for a screen W x H and the theme's MODE.
pub fn bg_warnings(name: &str, b: &Bg, screen: (i64, i64), mode: &str) -> Vec<String> {
    let mut w = Vec::new();
    let (sw, sh) = screen;
    #[allow(clippy::cast_precision_loss)]
    if (b.w as f64) < BG_SHARE * sw as f64 || (b.h as f64) < BG_SHARE * sh as f64 {
        w.push(format!("background {name}: {}x{}, small for a {sw}x{sh} screen (stock themes: 1536x1024 at least)", b.w, b.h));
    }
    if b.std < BG_FLAT {
        w.push(format!("background {name}: nearly one colour (its grey varies {:.3}; stock themes: 0.028 at least)", b.std));
    }
    if mode == "dark" && b.mean > BG_DARK_MAX {
        w.push(format!("background {name}: bright for a dark theme (mean {:.2}; stock dark themes: 0.63 at most)", b.mean));
    }
    if mode == "light" && b.mean < BG_LIGHT_MIN {
        w.push(format!("background {name}: dark for a light theme (mean {:.2}; stock light themes: 0.69 at least)", b.mean));
    }
    w
}

/// Every warning for theme DIR on a screen W x H: what is not checkable here said as such (no
/// omarchy-theme-color, no ImageMagick).
pub fn check(dir: &[u8], screen: (i64, i64)) -> Vec<String> {
    check_layered(&[dir.to_vec()], screen)
}

/// A theme's colors.toml (none: it has none) and its backgrounds, (name, file) each.
pub type Layered = (Option<Vec<u8>>, Vec<(Vec<u8>, Vec<u8>)>);

/// The theme omarchy-theme-set makes of DIRS, each copied over the one before (Omarchy's folder of
/// a name, then the user's of the same name): the colors.toml it ends with, and its backgrounds by
/// name, each from the last folder that has it, sorted by name.
pub fn layered(dirs: &[Vec<u8>]) -> Layered {
    let colors = dirs
        .iter()
        .rev()
        .map(|d| cat!(d, "/colors.toml"))
        .find(|c| shell::path(c).is_file());
    let mut bgs: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    for d in dirs {
        let b = cat!(d, "/backgrounds");
        for n in shell::glob(&shell::path(&b), false) {
            let f = cat!(b, "/", n);
            if shell::path(&f).is_file() {
                bgs.insert(n, f);
            }
        }
    }
    (colors, bgs.into_iter().collect())
}

/// check for the theme DIRS make together (layered): an installed theme picked by name.
pub fn check_layered(dirs: &[Vec<u8>], screen: (i64, i64)) -> Vec<String> {
    let (colors, bgs) = layered(dirs);
    let Some(colors) = colors else {
        return vec!["no colors.toml: Omarchy reads its palette from it".to_string()];
    };
    let run = |what: &str| {
        std::process::Command::new("omarchy-theme-color")
            .arg("--file")
            .arg(OsStr::from_bytes(&colors))
            .arg(what)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| palette(&o.stdout))
    };
    let mut w = Vec::new();
    let (Some(raw), Some(all)) = (run("--raw"), run("--all")) else {
        return vec![
            "the palette is not checked: no omarchy-theme-color here (Omarchy's)".to_string(),
        ];
    };
    let mut tpl = Vec::new();
    for f in shell::glob(&shell::path(TEMPLATES.as_bytes()), false) {
        if f.ends_with(b".tpl") {
            tpl.extend(std::fs::read(shell::path(&cat!(TEMPLATES, "/", f))).unwrap_or_default());
        }
    }
    w.extend(palette_warnings(&raw, &all, &template_keys(&tpl)));
    let mode = all
        .get("mode")
        .cloned()
        .unwrap_or_else(|| "dark".to_string());
    for (n, f) in &bgs {
        let name = String::from_utf8_lossy(n).into_owned();
        match measure(f) {
            Some(b) => w.extend(bg_warnings(&name, &b, screen, &mode)),
            None => w.push(format!(
                "background {name}: not read (ImageMagick's magick, or not an image)"
            )),
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_from_templates() {
        let t = b"a {{ background_rgb }} b {{ accent_strip }} {{ mix_rgb red foreground 6% }} {{ shell_gradient hyprland_active_border foreground }} {{ hypr_gradient x accent }} {{ theme_type }} {{ Weird }}";
        assert_eq!(
            template_keys(t),
            ["accent", "background", "foreground", "red"]
        );
    }

    #[test]
    fn contrast_and_hex() {
        assert!((contrast("#000000", "#ffffff").unwrap() - 21.0).abs() < 0.01);
        assert!((contrast("#cdd6f4", "#1e1e2e").unwrap() - 11.3).abs() < 0.1); // catppuccin
        assert_eq!(rgb("#abc"), None);
        assert_eq!(rgb("rgba(26a269ee)"), None);
    }

    #[test]
    fn palette_checks() {
        let m = |v: &[(&str, &str)]| {
            v.iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect::<BTreeMap<_, _>>()
        };
        let raw = m(&[
            ("mode", "dark"),
            ("background", "#1e1e2e"),
            ("foreground", "#2a2a3a"),
            ("accent", "blue"),
            ("hyprland_active_border", "rgba(1) rgba(2) 45deg"),
        ]);
        let mut all = raw.clone();
        all.insert("accent".into(), "".into());
        let keys = vec![
            "accent".to_string(),
            "background".to_string(),
            "foreground".to_string(),
            "red".to_string(),
        ];
        let w = palette_warnings(&raw, &all, &keys);
        assert_eq!(
            w[0],
            "colors.toml: no accent, red (Omarchy's templates use them, and no fallback resolves them)"
        );
        assert_eq!(
            w[1],
            "colors.toml: accent = \"blue\" is no #rrggbb: Omarchy falls back for it"
        );
        assert!(
            w[2].starts_with("foreground #2a2a3a on background #1e1e2e: 1.2:1, under 4.5:1"),
            "{}",
            w[2]
        );
        assert_eq!(w.len(), 3);
    }

    #[test]
    fn layers() {
        let d = std::env::temp_dir().join(format!("omabox-layers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (a, b) = (d.join("a"), d.join("b"));
        for p in [a.join("backgrounds"), b.join("backgrounds")] {
            std::fs::create_dir_all(p).unwrap();
        }
        for f in ["colors.toml", "backgrounds/1.png", "backgrounds/2.png"] {
            std::fs::write(a.join(f), "").unwrap();
        }
        std::fs::write(b.join("backgrounds/2.png"), "").unwrap();
        std::fs::write(b.join("backgrounds/3.png"), "").unwrap();
        let v = |p: &std::path::Path| p.as_os_str().as_bytes().to_vec();
        let (c, bgs) = layered(&[v(&a), v(&b)]);
        assert_eq!(c, Some(cat!(v(&a), "/colors.toml")), "a's: b has none");
        let got: Vec<Vec<u8>> = bgs.iter().map(|(_, f)| f.clone()).collect();
        assert_eq!(
            got,
            [
                cat!(v(&a), "/backgrounds/1.png"),
                cat!(v(&b), "/backgrounds/2.png"),
                cat!(v(&b), "/backgrounds/3.png")
            ]
        );
        std::fs::write(b.join("colors.toml"), "").unwrap();
        assert_eq!(
            layered(&[v(&a), v(&b)]).0,
            Some(cat!(v(&b), "/colors.toml"))
        );
        assert_eq!(layered(&[]), (None, Vec::new()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn background_checks() {
        let b = Bg {
            w: 640,
            h: 480,
            mean: 0.9,
            std: 0.001,
        };
        let w = bg_warnings("x.png", &b, (1920, 1080), "dark");
        assert_eq!(w.len(), 3, "{w:?}");
        let ok = Bg {
            w: 1536,
            h: 1024,
            mean: 0.5,
            std: 0.05,
        };
        assert!(bg_warnings("y.png", &ok, (1920, 1080), "dark").is_empty());
        assert_eq!(
            bg_warnings("z.png", &Bg { mean: 0.3, ..ok }, (1920, 1080), "light").len(),
            1
        );
    }
}
