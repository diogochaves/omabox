//! The in-box helpers `up` binds one file each (#421, finding 640): keyboard, pointer, still,
//! events, burst, tree, notices, polkit and its session shim. A file bind pins the inode it found, so a helper
//! replaced on the host since (`make`, `install`, a package upgrade: each writes a new inode) leaves
//! the box on the old one while the new CLI drives it. `up` records each source's device and inode
//! in box.json's `helpers`; a command that runs one in the box compares them with the host's path
//! now and says which differ, once per command. A box from before (no record) says nothing, and
//! neither does a source gone from the host (a rebuild under way, a `make clean`): there is nothing
//! newer to bind.

use crate::cat;
use crate::exit::{Ctx, say};
use crate::shell;
use serde_json::{Map, Value};
use std::os::unix::fs::MetadataExt;
use std::sync::Mutex;

/// Where `up` binds the helpers in a box: an in-box command naming a path under it runs one.
pub const BOX_BIN: &[u8] = b"/opt/omabox/bin/omabox-";

/// box.json's `helpers`: each bound helper's name, source and the source's device and inode as `up`
/// found it. One it cannot stat is left out (the bind would fail first).
pub fn record(binds: &[(&str, Vec<u8>)]) -> Value {
    Value::Array(
        binds
            .iter()
            .filter_map(|(name, src)| {
                let m = std::fs::metadata(shell::path(src)).ok()?;
                let mut e = Map::new();
                e.insert("name".into(), Value::String((*name).into()));
                e.insert(
                    "src".into(),
                    Value::String(String::from_utf8_lossy(src).into_owned()),
                );
                e.insert("dev".into(), Value::from(m.dev()));
                e.insert("ino".into(), Value::from(m.ino()));
                Some(Value::Object(e))
            })
            .collect(),
    )
}

/// The recorded helpers (box.json V) whose source is now another file, as STAT (device, inode of a
/// path, None when it is gone) sees it, in their recorded order. None recorded: none.
pub fn stale(v: &Value, stat: &dyn Fn(&str) -> Option<(u64, u64)>) -> Vec<String> {
    let Some(hs) = v.get("helpers").and_then(Value::as_array) else {
        return Vec::new();
    };
    hs.iter()
        .filter_map(|h| {
            let name = h.get("name")?.as_str()?;
            let src = h.get("src")?.as_str()?;
            let was = (h.get("dev")?.as_u64()?, h.get("ino")?.as_u64()?);
            let now = stat(src)?;
            (now != was).then(|| name.to_string())
        })
        .collect()
}

/// The warning for box NAME's stale helpers.
pub fn words(name: &[u8], stale: &[String]) -> Vec<u8> {
    cat!(
        "warning: box '",
        name,
        "' started with older omabox helpers (",
        stale.join(", "),
        "), replaced on the host since: omabox down ",
        name,
        ", then up again"
    )
}

/// Whether ARGS (an in-box command) run one of the bound helpers.
pub fn runs_helper(args: &[Vec<u8>]) -> bool {
    args.iter().any(|a| a.starts_with(BOX_BIN))
}

/// The boxes this command has checked (by dir): each is checked and said once.
static CHECKED: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

/// warn_stale: the box's helpers replaced on the host since `up`, said once in this command. A
/// muted caller's check does not count, so the next unmuted one says it.
pub fn warn_stale(ctx: &Ctx) {
    if crate::exit::is_muted() {
        return;
    }
    {
        let Ok(mut seen) = CHECKED.lock() else {
            return;
        };
        if seen.contains(&ctx.d) {
            return;
        }
        seen.push(ctx.d.clone());
    }
    let Ok(text) = std::fs::read(ctx.at("box.json")) else {
        return;
    };
    let Ok(v) = serde_json::from_slice::<Value>(&text) else {
        return;
    };
    let s = stale(&v, &|p| {
        std::fs::metadata(p).ok().map(|m| (m.dev(), m.ino()))
    });
    if !s.is_empty() {
        say(words(&ctx.name, &s));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(entries: &[(&str, &str, u64, u64)]) -> Value {
        let hs: Vec<Value> = entries
            .iter()
            .map(|(n, s, d, i)| serde_json::json!({"name": n, "src": s, "dev": d, "ino": i}))
            .collect();
        serde_json::json!({ "name": "b", "helpers": hs })
    }

    #[test]
    fn a_helper_with_another_inode_is_stale() {
        let v = rec(&[
            ("keyboard", "/r/tools/keyboard/omabox-keyboard", 1, 10),
            ("pointer", "/r/tools/pointer/omabox-pointer", 1, 11),
            ("still", "/r/tools/still/omabox-still", 1, 12),
        ]);
        let stat = |p: &str| match p {
            "/r/tools/keyboard/omabox-keyboard" => Some((1, 20)),
            "/r/tools/pointer/omabox-pointer" => Some((1, 11)),
            "/r/tools/still/omabox-still" => Some((2, 12)),
            _ => None,
        };
        assert_eq!(stale(&v, &stat), vec!["keyboard", "still"]);
    }

    #[test]
    fn the_same_files_say_nothing() {
        let v = rec(&[("tree", "/r/t", 3, 4)]);
        assert!(stale(&v, &|_| Some((3, 4))).is_empty());
    }

    #[test]
    fn a_box_from_before_or_a_source_gone_says_nothing() {
        let old = serde_json::json!({"name": "b", "mounts": []});
        assert!(stale(&old, &|_| Some((9, 9))).is_empty());
        let v = rec(&[("still", "/r/s", 1, 2)]);
        assert!(stale(&v, &|_| None).is_empty());
        // (a malformed entry is skipped, not a stale one)
        let bad = serde_json::json!({"helpers": [{"name": "still", "src": "/r/s"}]});
        assert!(stale(&bad, &|_| Some((5, 5))).is_empty());
    }

    #[test]
    fn the_warning_names_them_and_the_way_out() {
        assert_eq!(
            String::from_utf8(words(b"b", &["keyboard".into(), "still".into()])).unwrap(),
            "warning: box 'b' started with older omabox helpers (keyboard, still), replaced on the host since: omabox down b, then up again"
        );
    }

    #[test]
    fn only_a_command_naming_a_helper_runs_one() {
        let w = |v: &[&str]| v.iter().map(|s| s.as_bytes().to_vec()).collect::<Vec<_>>();
        assert!(runs_helper(&w(&["/opt/omabox/bin/omabox-still", "--x"])));
        assert!(runs_helper(&w(&[
            "/usr/bin/timeout",
            "30",
            "/opt/omabox/bin/omabox-tree"
        ])));
        assert!(!runs_helper(&w(&["hyprctl", "-j", "clients"])));
        assert!(!runs_helper(&w(&["cat", "/opt/omabox/share/x"])));
    }

    #[test]
    fn record_keeps_what_it_can_stat() {
        let dir = std::env::temp_dir().join(format!("omabox-helpers-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("omabox-still");
        std::fs::write(&f, b"x").unwrap();
        let src = f.to_str().unwrap().as_bytes().to_vec();
        let v = record(&[("still", src.clone()), ("tree", cat!(src, ".gone"))]);
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        let m = std::fs::metadata(&f).unwrap();
        assert_eq!(arr[0]["name"], "still");
        assert_eq!(arr[0]["ino"], m.ino());
        // Replaced the way install does it (a new file put in its place): stale.
        let box_json = serde_json::json!({ "helpers": v });
        let tmp = dir.join("omabox-still.new");
        std::fs::write(&tmp, b"y").unwrap();
        std::fs::rename(&tmp, &f).unwrap();
        let stat = |p: &str| std::fs::metadata(p).ok().map(|m| (m.dev(), m.ino()));
        assert_eq!(stale(&box_json, &stat), vec!["still"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
