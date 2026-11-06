//! `omabox version` (`--version`, `-V`): omabox's version, then what a new box would run on: the
//! aquamarine (finding 125), and the Hyprland when OMABOX_HYPRLAND names one (#424, finding 647).
//! Nothing else is taken (finding 348).

use crate::args::unknown_opt;
use crate::cat;
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::install::{Aq, NewBox, new_box};
use crate::json;
use serde_json::{Map, Value};

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let mut as_json = false;
    for a in args {
        if a == b"--json" {
            as_json = true;
        } else if a.starts_with(b"-") {
            return Err(unknown_opt(ctx, "--version", a));
        } else {
            say(cat!(
                "--version: unknown argument ",
                a,
                " (omabox --version [--json])"
            ));
            return Err(Exit(2));
        }
    }
    ctx.parsed();
    // (quiet: a Hyprland ldd cannot read, a bad OMABOX_AQUAMARINE or OMABOX_HYPRLAND, said nowhere)
    let nb = new_box(ctx);
    let version = version_text();
    if as_json {
        return out(cat!(json_line(&version, &nb), "\n"));
    }
    out(cat!("omabox ", version, "\n"))?;
    if let Some(h) = hypr_line(&nb) {
        out(cat!("hyprland: ", h, "\n"))?;
    }
    if let Some(aq) = nb.aq {
        out(cat!("aquamarine: ", aq.desc(), "\n"))?;
    }
    Ok(())
}

/// The Hyprland line: only when OMABOX_HYPRLAND names one, which every `up` then runs.
fn hypr_line(nb: &NewBox) -> Option<Vec<u8>> {
    if nb.given.is_empty() {
        return None;
    }
    Some(match &nb.hypr {
        Some(h) => cat!(h.bin, " (OMABOX_HYPRLAND=", nb.given, ")"),
        None => cat!(
            "OMABOX_HYPRLAND=",
            nb.given,
            " holds no Hyprland up can run (omabox up says why)"
        ),
    })
}

/// The checkout's VERSION when this binary was built (wave 6-front, A5; proposal D2): the same
/// bytes as `$(cat "$ROOT/VERSION")` while binary and tree agree, and the truth for a binary older
/// than the tree it runs from (a pull without ./install.sh), where reading the file would say the
/// pulled tree's.
const VERSION: &str = include_str!("../../../../VERSION");

pub fn version_text() -> Vec<u8> {
    crate::shell::strip_nl(VERSION.as_bytes())
}

/// --json (#273, audit J3): the version, and the aquamarine new boxes use as ls --json names a box's
/// (kind private, system or folder, version, fixed), its words as message; null when none is found.
/// With OMABOX_HYPRLAND set, `hyprland`: {given, binary (null when up refuses it), message}; null
/// otherwise.
fn json_line(version: &[u8], nb: &NewBox) -> String {
    let s = |v: &[u8]| Value::String(String::from_utf8_lossy(v).into_owned());
    let mut m = Map::new();
    m.insert("version".into(), s(version));
    let aq: Option<&Aq> = nb.aq.as_ref();
    let a = aq.map_or(Value::Null, |aq| {
        let mut a = Map::new();
        a.insert("kind".into(), s(&aq.kind));
        a.insert("version".into(), s(&aq.ver));
        a.insert("fixed".into(), Value::Bool(aq.fixed));
        a.insert("message".into(), s(&aq.desc()));
        Value::Object(a)
    });
    m.insert("aquamarine".into(), a);
    let h = hypr_line(nb).map_or(Value::Null, |line| {
        let mut h = Map::new();
        h.insert("given".into(), s(&nb.given));
        h.insert(
            "binary".into(),
            nb.hypr.as_ref().map_or(Value::Null, |x| s(&x.bin)),
        );
        h.insert("message".into(), s(&line));
        Value::Object(h)
    });
    m.insert("hyprland".into(), h);
    json::compact(&Value::Object(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_as_jq_writes_it() {
        assert_eq!(
            json_line(b"1.0.1", &NewBox::default()),
            r#"{"version":"1.0.1","aquamarine":null,"hyprland":null}"#
        );
        let aq = Aq {
            kind: b"system".to_vec(),
            ver: b"0.16.0".to_vec(),
            fixed: true,
            ..Aq::default()
        };
        let nb = NewBox {
            aq: Some(aq),
            ..NewBox::default()
        };
        assert_eq!(
            json_line(b"1.0.1", &nb),
            r#"{"version":"1.0.1","aquamarine":{"kind":"system","version":"0.16.0","fixed":true,"message":"the system's, 0.16.0"},"hyprland":null}"#
        );
    }

    // OMABOX_HYPRLAND (#424, finding 647): the folder's Hyprland and aquamarine said, and a value up
    // refuses said as such.
    #[test]
    fn omabox_hyprland_is_said() {
        let nb = NewBox {
            given: b"/h".to_vec(),
            hypr: Some(crate::install::Hypr {
                bin: b"/h/usr/bin/Hyprland".to_vec(),
                ..Default::default()
            }),
            aq: Some(Aq {
                kind: b"folder".to_vec(),
                lib: b"/h/usr/lib".to_vec(),
                ver: b"0.16.0".to_vec(),
                fixed: true,
                ..Aq::default()
            }),
        };
        assert_eq!(
            hypr_line(&nb).unwrap(),
            b"/h/usr/bin/Hyprland (OMABOX_HYPRLAND=/h)"
        );
        assert_eq!(
            json_line(b"1", &nb),
            r#"{"version":"1","aquamarine":{"kind":"folder","version":"0.16.0","fixed":true,"message":"the --hyprland folder's, 0.16.0 (/h/usr/lib)"},"hyprland":{"given":"/h","binary":"/h/usr/bin/Hyprland","message":"/h/usr/bin/Hyprland (OMABOX_HYPRLAND=/h)"}}"#
        );
        let bad = NewBox {
            given: b"/x".to_vec(),
            ..NewBox::default()
        };
        assert_eq!(
            json_line(b"1", &bad),
            r#"{"version":"1","aquamarine":null,"hyprland":{"given":"/x","binary":null,"message":"OMABOX_HYPRLAND=/x holds no Hyprland up can run (omabox up says why)"}}"#
        );
    }
}
