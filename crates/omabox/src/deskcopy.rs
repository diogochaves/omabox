//! A desk box's copy of the user's desktop (#321 step 2): their Hyprland config, the shell's config
//! and state its profile names (deskshell), and the images they name, copied into the box's HOME
//! before --seed's (which win). copy says what it did as seed_home's DESK_FILES lines, which
//! Report reads back for box.json's desk_copy, shell_guess and shell_from.
//! Step 0's placeholder: copies nothing, guesses nothing.

use crate::exit::{Ctx, Res};
use crate::seed::SeedPlan;
use serde_json::{Map, Value};

/// copy PLAN H: the user's desktop into the box HOME H, for a desk box; its lines: `copied PATH`,
/// `skipped PATH<TAB>WHY`, and `guess NAME<TAB>FROM` (the slot's first guess, and what found it).
pub fn copy(_ctx: &Ctx, _plan: &SeedPlan, _h: &[u8]) -> Res<Vec<u8>> {
    Ok(Vec::new())
}

/// What copy said, read back from its lines.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub copied: Vec<Vec<u8>>,
    /// (path, why) each.
    pub skipped: Vec<(Vec<u8>, Vec<u8>)>,
    /// The desk shell's profile the slot guesses first (OMABOX_DESK_SHELL, box.json shell_guess),
    /// and what found it (box.json shell_from).
    pub guess: Option<Vec<u8>>,
    pub from: Option<Vec<u8>>,
}

impl Report {
    pub fn parse(lines: &[u8]) -> Report {
        let mut r = Report::default();
        let tab = |v: &[u8]| -> (Vec<u8>, Vec<u8>) {
            let mut it = v.splitn(2, |&c| c == b'\t');
            (
                it.next().unwrap_or_default().to_vec(),
                it.next().unwrap_or_default().to_vec(),
            )
        };
        for l in lines.split(|&c| c == b'\n') {
            if let Some(v) = l.strip_prefix(b"copied ".as_slice()) {
                r.copied.push(v.to_vec());
            } else if let Some(v) = l.strip_prefix(b"skipped ".as_slice()) {
                r.skipped.push(tab(v));
            } else if let Some(v) = l.strip_prefix(b"guess ".as_slice()) {
                let (name, from) = tab(v);
                r.guess = Some(name).filter(|n| !n.is_empty());
                r.from = Some(from).filter(|f| !f.is_empty());
            }
        }
        r
    }

    /// box.json's desk_copy: {copied: [PATH...], skipped: [{path, why}...]}.
    pub fn json(&self) -> Value {
        let s = |v: &[u8]| Value::String(String::from_utf8_lossy(v).into_owned());
        let mut o = Map::new();
        o.insert(
            "copied".into(),
            Value::Array(self.copied.iter().map(|p| s(p)).collect()),
        );
        let skipped = self
            .skipped
            .iter()
            .map(|(p, why)| {
                let mut m = Map::new();
                m.insert("path".into(), s(p));
                m.insert("why".into(), s(why));
                Value::Object(m)
            })
            .collect();
        o.insert("skipped".into(), Value::Array(skipped));
        Value::Object(o)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_reads_copy_lines() {
        let r = Report::parse(
            b"copied .config/hypr/hyprland.lua\nskipped .config/hypr/big.png\ttoo big\nguess dms\tunit\n",
        );
        assert_eq!(r.copied, [b".config/hypr/hyprland.lua".to_vec()]);
        assert_eq!(
            r.skipped,
            [(b".config/hypr/big.png".to_vec(), b"too big".to_vec())]
        );
        assert_eq!(r.guess.as_deref(), Some(&b"dms"[..]));
        assert_eq!(r.from.as_deref(), Some(&b"unit"[..]));
        assert_eq!(
            r.json().to_string(),
            r#"{"copied":[".config/hypr/hyprland.lua"],"skipped":[{"path":".config/hypr/big.png","why":"too big"}]}"#
        );
        let none = Report::parse(b"");
        assert_eq!(none, Report::default());
        assert_eq!(none.json().to_string(), r#"{"copied":[],"skipped":[]}"#);
    }
}
