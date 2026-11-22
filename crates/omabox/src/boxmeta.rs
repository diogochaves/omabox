//! box.json read back: `BoxMeta`, the keys the program reads from it, parsed once per read instead
//! of once per key (struct-big's cross-cutting point; the keys before it were 103 `meta(d, "key")`
//! calls, each reparsing the file).
//!
//! Written only by `up` (`box_json`) and changed only through `edit_box_json` (finding 753): this is
//! the reading side alone. Every field reads its key as `meta` did, `jq -r '.[$k] // empty'` as
//! `$(...)`: a text is the value's raw form (a string as it is, anything else as jq prints it; a
//! missing key, null or false is empty), and a flag is that text being `true`. So a box.json from an
//! older omabox (a key missing), from a newer one (a key it does not know, kept in `other`), or with
//! a key of an unexpected type reads as it did, never fails as a whole. A file that is not one JSON
//! object (several values, as a torn write once left, or none) reads as jq read it: each key the
//! values' raw forms a line each.

use crate::boxes::at;
use crate::json;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// BoxMeta's keys (a test checks this list against the struct).
const KNOWN: [&str; 40] = [
    "mode",
    "size",
    "scale",
    "created",
    "net",
    "from",
    "theme",
    "bar",
    "allow",
    "idle",
    "desktop",
    "desk_base",
    "home",
    "shell_cmd",
    "shell_guess",
    "shell_from",
    "jail",
    "dir",
    "hyprland",
    "hyprland_version",
    "hyprctl",
    "quickshell",
    "quickshell_version",
    "omarchy",
    "owner",
    "owner_start",
    "agent",
    "agent_start",
    "workspace",
    "pidns",
    "shell",
    "wayland_screen",
    "systemd",
    "xwayland",
    "autoreload",
    "audio",
    "polkit",
    "protect",
    "drawn_hidden",
    "mount_ns",
];

/// The keys of box.json the program reads (see the module's words for how each is read).
#[derive(Debug, Default, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct BoxMeta {
    /// `interactive` or `headless`.
    #[serde(with = "text")]
    pub mode: Vec<u8>,
    /// WxH[@HZ], `window` for an interactive box; `omabox mode` changes it.
    #[serde(with = "text")]
    pub size: Vec<u8>,
    /// The main monitor's scale, a number; empty in a box.json from before it (see `scale_or_1`).
    #[serde(with = "text")]
    pub scale: Vec<u8>,
    #[serde(with = "text")]
    pub created: Vec<u8>,
    /// host, connected, isolated or none (empty: before it).
    #[serde(with = "text")]
    pub net: Vec<u8>,
    /// The save it started from (`up --from`).
    #[serde(with = "text")]
    pub from: Vec<u8>,
    /// Its `up --theme`.
    #[serde(with = "text")]
    pub theme: Vec<u8>,
    /// stock or user (desk: a desk box's).
    #[serde(with = "text")]
    pub bar: Vec<u8>,
    /// The --allow ports, comma-separated.
    #[serde(with = "text")]
    pub allow: Vec<u8>,
    /// The idle limit in seconds, a number (empty: none).
    #[serde(with = "text")]
    pub idle: Vec<u8>,
    #[serde(with = "text")]
    pub desktop: Vec<u8>,
    #[serde(with = "text")]
    pub desk_base: Vec<u8>,
    /// The box's HOME inside it (boxes::box_home checks it).
    #[serde(with = "text")]
    pub home: Vec<u8>,
    #[serde(with = "text")]
    pub shell_cmd: Vec<u8>,
    #[serde(with = "text")]
    pub shell_guess: Vec<u8>,
    #[serde(with = "text")]
    pub shell_from: Vec<u8>,
    #[serde(with = "text")]
    pub jail: Vec<u8>,
    /// The directory `up` ran in.
    #[serde(with = "text")]
    pub dir: Vec<u8>,
    #[serde(with = "text")]
    pub hyprland: Vec<u8>,
    #[serde(with = "text")]
    pub hyprland_version: Vec<u8>,
    #[serde(with = "text")]
    pub hyprctl: Vec<u8>,
    #[serde(with = "text")]
    pub quickshell: Vec<u8>,
    #[serde(with = "text")]
    pub quickshell_version: Vec<u8>,
    #[serde(with = "text")]
    pub omarchy: Vec<u8>,
    /// A throwaway's `omabox run` (a pid) and its start time.
    #[serde(with = "text")]
    pub owner: Vec<u8>,
    #[serde(with = "text")]
    pub owner_start: Vec<u8>,
    /// The agent the box is given to (a pid) and its start time; written after `up` too.
    #[serde(with = "text")]
    pub agent: Vec<u8>,
    #[serde(with = "text")]
    pub agent_start: Vec<u8>,
    /// An interactive box's host workspace.
    #[serde(with = "text")]
    pub workspace: Vec<u8>,
    /// The box's pid namespace, written once it is launched.
    #[serde(with = "text")]
    pub pidns: Vec<u8>,
    #[serde(with = "flag")]
    pub shell: bool,
    #[serde(with = "flag")]
    pub wayland_screen: bool,
    #[serde(with = "flag")]
    pub systemd: bool,
    #[serde(with = "flag")]
    pub xwayland: bool,
    #[serde(with = "flag")]
    pub autoreload: bool,
    #[serde(with = "flag")]
    pub audio: bool,
    #[serde(with = "flag")]
    pub polkit: bool,
    #[serde(with = "flag")]
    pub protect: bool,
    #[serde(with = "flag")]
    pub drawn_hidden: bool,
    /// The mount namespace `up` saw its PID 1 in (latemount): its `mnt` and `owner` inode numbers,
    /// None unless both are unsigned integers.
    #[serde(with = "mount_ns")]
    pub mount_ns: Option<(u64, u64)>,
    /// Every other key, as it is: a key a newer omabox writes, or one only jq programs read. Filled
    /// by `parse`, not by serde: serde_json's arbitrary_precision numbers do not go through
    /// `flatten` when read (a number in any other key failed the whole struct).
    #[serde(flatten, skip_deserializing)]
    pub other: Map<String, Value>,
    /// Whether the file was read as jq would exit 0 (`meta`'s second value): false for a missing
    /// file, a syntax error or a value that is no object.
    #[serde(skip)]
    pub read: bool,
}

impl BoxMeta {
    /// Box dir D's box.json.
    pub fn load(d: &[u8]) -> BoxMeta {
        match std::fs::read(at(d, "box.json")) {
            Ok(t) => BoxMeta::parse(&t),
            Err(_) => BoxMeta::default(),
        }
    }

    /// box.json's TEXT.
    pub fn parse(text: &[u8]) -> BoxMeta {
        let (vals, mut ok) = json::values(text);
        let obj = match vals.as_slice() {
            [Value::Object(m)] => m.clone(),
            _ => joined(&vals, &mut ok),
        };
        // Every field reads any value (text, flag, mount_ns) and other keys are skipped, so this
        // cannot fail.
        let mut other = obj.clone();
        other.retain(|k, _| !KNOWN.contains(&k.as_str()));
        let mut m: BoxMeta = serde_json::from_value(Value::Object(obj)).unwrap_or_default();
        m.other = other;
        m.read = ok;
        m
    }

    /// `mode` is interactive.
    pub fn interactive(&self) -> bool {
        self.mode == b"interactive"
    }

    /// `mode` is headless (a box.json with no mode is neither).
    pub fn headless(&self) -> bool {
        self.mode == b"headless"
    }

    /// The scale, 1 when box.json has none.
    pub fn scale_or_1(&self) -> &[u8] {
        if self.scale.is_empty() {
            b"1"
        } else {
            &self.scale
        }
    }
}

/// Several values (or one that is no object) as `jq -r '.[$k] // empty'` reads each key: the
/// values' raw forms a line each, as one string; OK false for a value that is neither an object nor
/// null (jq's error), as json::field.
fn joined(vals: &[Value], ok: &mut bool) -> Map<String, Value> {
    let mut keys: Vec<&String> = Vec::new();
    for v in vals {
        match v {
            Value::Object(m) => {
                for k in m.keys() {
                    if !keys.contains(&k) {
                        keys.push(k);
                    }
                }
            }
            Value::Null => {}
            _ => *ok = false,
        }
    }
    let mut out = Map::new();
    for k in keys {
        let mut s = Vec::new();
        for v in vals {
            if let Value::Object(m) = v
                && let Some(x) = m.get(k.as_str())
                && !matches!(x, Value::Null | Value::Bool(false))
            {
                s.extend_from_slice(json::raw(x).as_bytes());
                s.push(b'\n');
            }
        }
        let s = crate::shell::strip_nl(&s);
        if !s.is_empty() {
            out.insert(
                k.clone(),
                Value::String(String::from_utf8_lossy(&s).into_owned()),
            );
        }
    }
    out
}

/// A value as `$(jq -r '.[$k] // empty')` gives it.
fn raw_text(v: &Value) -> Vec<u8> {
    match v {
        Value::Null | Value::Bool(false) => Vec::new(),
        x => crate::shell::strip_nl(json::raw(x).as_bytes()),
    }
}

mod text {
    use super::*;

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        Ok(raw_text(&Value::deserialize(d)?))
    }

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&String::from_utf8_lossy(v))
    }
}

mod flag {
    use super::*;

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
        Ok(raw_text(&Value::deserialize(d)?) == b"true")
    }

    pub fn serialize<S: Serializer>(v: &bool, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bool(*v)
    }
}

mod mount_ns {
    use super::*;

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<(u64, u64)>, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(v.get("mnt")
            .and_then(Value::as_u64)
            .zip(v.get("owner").and_then(Value::as_u64)))
    }

    pub fn serialize<S: Serializer>(v: &Option<(u64, u64)>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some((mnt, owner)) => serde_json::json!({"mnt": mnt, "owner": owner}).serialize(s),
            None => s.serialize_none(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `meta`'s read before BoxMeta (json::field on the file), for comparing.
    fn field(text: &[u8], key: &str) -> (Vec<u8>, bool) {
        let p = std::env::temp_dir().join(format!(
            "omabox-boxmeta-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&p, text).unwrap();
        let r = json::field(&p, key);
        std::fs::remove_file(&p).unwrap();
        r
    }

    /// Each field of M against json::field of KEY on TEXT, as every call site read it.
    fn same_as_field(text: &[u8]) {
        let m = BoxMeta::parse(text);
        let texts: [(&str, &Vec<u8>); 30] = [
            ("mode", &m.mode),
            ("size", &m.size),
            ("scale", &m.scale),
            ("created", &m.created),
            ("net", &m.net),
            ("from", &m.from),
            ("theme", &m.theme),
            ("bar", &m.bar),
            ("allow", &m.allow),
            ("idle", &m.idle),
            ("desktop", &m.desktop),
            ("desk_base", &m.desk_base),
            ("home", &m.home),
            ("shell_cmd", &m.shell_cmd),
            ("shell_guess", &m.shell_guess),
            ("shell_from", &m.shell_from),
            ("jail", &m.jail),
            ("dir", &m.dir),
            ("hyprland", &m.hyprland),
            ("hyprland_version", &m.hyprland_version),
            ("hyprctl", &m.hyprctl),
            ("quickshell", &m.quickshell),
            ("quickshell_version", &m.quickshell_version),
            ("omarchy", &m.omarchy),
            ("owner", &m.owner),
            ("owner_start", &m.owner_start),
            ("agent", &m.agent),
            ("agent_start", &m.agent_start),
            ("workspace", &m.workspace),
            ("pidns", &m.pidns),
        ];
        let t = String::from_utf8_lossy(text);
        for (k, v) in texts {
            let (want, ok) = field(text, k);
            assert_eq!(
                String::from_utf8_lossy(v),
                String::from_utf8_lossy(&want),
                "{k} in {t}"
            );
            assert_eq!(m.read, ok, "read, {k} in {t}");
        }
        let flags = [
            ("shell", m.shell),
            ("wayland_screen", m.wayland_screen),
            ("systemd", m.systemd),
            ("xwayland", m.xwayland),
            ("autoreload", m.autoreload),
            ("audio", m.audio),
            ("polkit", m.polkit),
            ("protect", m.protect),
            ("drawn_hidden", m.drawn_hidden),
        ];
        for (k, v) in flags {
            assert_eq!(v, field(text, k).0 == b"true", "{k} in {t}");
        }
    }

    /// box.json as omabox 0.1.0's `up` wrote it (bin/omabox's jq -n, an interactive box with an
    /// owner): no scale, shell, desktop, home, wayland_screen, pidns.
    const V0_1_0: &str = r#"{
  "name": "b",
  "mode": "interactive",
  "size": "window",
  "created": "2026-09-25T10:00:00+01:00",
  "plugins": ["omagauge"],
  "net": "host",
  "allow": "",
  "idle": 0,
  "bar": "user",
  "systemd": false,
  "overlays": [],
  "owner": 4242,
  "workspace": "special:omabox",
  "confirm_close": true,
  "confirm_close_from": "config"
}
"#;

    /// The keys `up` writes today (cmd/up_box.rs box_json) for a headless plain box with a shell
    /// command, plus the ones written after it (pidns, agent, mount_ns, quickshell_version).
    const TODAY: &str = r#"{
  "name": "p",
  "mode": "headless",
  "size": "1920x1080@60",
  "scale": 1.5,
  "created": "2026-11-22T09:00:00+00:00",
  "plugins": [],
  "plugin_dirs": {},
  "plugin_via": {},
  "net": "isolated",
  "allow": "8081,8082",
  "protect": true,
  "idle": 7200,
  "bar": "user",
  "stock": false,
  "systemd": false,
  "audio": true,
  "polkit": false,
  "overlays": [],
  "mounts": [],
  "helpers": [],
  "wayland_screen": false,
  "xwayland": false,
  "autoreload": false,
  "shell": true,
  "desktop": "plain",
  "home": "/home/sbx",
  "shell_cmd": "waybar\n",
  "aquamarine": {"kind": "system", "version": "0.16.0", "fixed": true},
  "render": {"node": "/dev/dri/renderD128", "pci": "0000:0e:00.0", "driver": "amdgpu"},
  "jail": "",
  "dir": "/home/u/r",
  "hyprland": "/opt/h",
  "hyprctl": "/opt/h/bin/hyprctl",
  "quickshell": "/opt/q",
  "omarchy": "/home/u/omarchy",
  "starter": {"terminal": false},
  "pidns": "pid:[4026532000]",
  "mount_ns": {"mnt": 4026532001, "owner": 4026531837},
  "agent": 99,
  "agent_start": "12345",
  "quickshell_version": "0.3.2",
  "hyprland_version": "0.57.2"
}
"#;

    #[test]
    fn reads_older_and_todays_box_json_as_meta_did() {
        for t in [
            V0_1_0,
            TODAY,
            "{}",
            "",
            "null",
            "[1]",
            "{\"mode\": \"headless\"} {\"mode\": \"interactive\", \"shell\": true}",
            "{\"shell\": true} {\"shell\": false}",
            "{\"shell\": \"true\", \"idle\": 1e3, \"owner\": null, \"agent\": false, \"home\": [\"/h\"]}",
            "{\"mode\": \"headless\", \"scale\": 2",
            "{\"mode\": 1} 7",
        ] {
            same_as_field(t.as_bytes());
        }
        let old = BoxMeta::parse(V0_1_0.as_bytes());
        assert!(old.read && old.mode == b"interactive");
        assert_eq!(old.scale_or_1(), b"1");
        assert_eq!(old.owner, b"4242");
        assert!(!old.shell && !old.wayland_screen && old.desktop.is_empty());
        let now = BoxMeta::parse(TODAY.as_bytes());
        assert!(now.mode == b"headless" && now.shell && now.audio && now.protect && !now.polkit);
        assert_eq!(now.scale_or_1(), b"1.5");
        assert_eq!(now.idle, b"7200");
        assert_eq!(now.agent, b"99");
        assert_eq!(now.shell_cmd, b"waybar", "trailing newlines go, as $(...)");
        assert_eq!(now.mount_ns, Some((4026532001, 4026531837)));
        assert_eq!(now.quickshell_version, b"0.3.2");
        // (a missing file: nothing read, as meta)
        let gone = BoxMeta::load(b"/nonexistent-omabox-boxmeta");
        assert!(!gone.read && gone == BoxMeta::default());
    }

    #[test]
    fn mount_ns_only_when_both_are_numbers() {
        for (t, want) in [
            (r#"{"mount_ns": {"mnt": 1, "owner": 2}}"#, Some((1, 2))),
            (r#"{"mount_ns": {"mnt": 1}}"#, None),
            (r#"{"mount_ns": {"mnt": "1", "owner": 2}}"#, None),
            (r#"{"mount_ns": [1, 2]}"#, None),
            (r#"{"mount_ns": {"mnt": -1, "owner": 2}}"#, None),
            (r#"{"mount_ns": {"mnt": 1, "owner": 2}} {}"#, None),
        ] {
            assert_eq!(BoxMeta::parse(t.as_bytes()).mount_ns, want, "{t}");
        }
    }

    #[test]
    fn known_is_every_field() {
        let v = serde_json::to_value(BoxMeta::default()).unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, KNOWN);
    }

    /// A key this omabox does not know (a newer one's) is kept, as it is, and written back out.
    #[test]
    fn unknown_keys_round_trip() {
        let t = r#"{"mode": "headless", "future_key": {"a": [1, 2.50, "x"]}, "other": 12345678901234567890123, "flag2": true}"#;
        let m = BoxMeta::parse(t.as_bytes());
        assert_eq!(m.mode, b"headless");
        let want: Map<String, Value> = serde_json::from_str(
            r#"{"future_key": {"a": [1, 2.50, "x"]}, "other": 12345678901234567890123, "flag2": true}"#,
        )
        .unwrap();
        assert_eq!(m.other, want);
        let back = serde_json::to_value(&m).unwrap();
        for (k, v) in &want {
            assert_eq!(back.get(k), Some(v), "{k}");
        }
        assert_eq!(back["mode"], Value::String("headless".into()));
        // ...and read again, the same.
        let again = BoxMeta::parse(serde_json::to_string(&back).unwrap().as_bytes());
        assert_eq!(again.other, m.other);
        assert_eq!(again.mode, m.mode);
        assert!(!again.wayland_screen);
    }
}
