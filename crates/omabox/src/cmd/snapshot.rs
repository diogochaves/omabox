//! `omabox snapshot` and `omabox diff` (#446): a box HOME recorded under a name, and what changed in
//! it since (or between two records): what an app, a plugin or an install script wrote, added,
//! removed or changed. The walk and the comparison are homesnap's; snapshots are kept in the box's
//! folder (`<box dir>/snapshots`), which the box never sees, and go with the box. `up` records what it
//! seeded before the box starts (`diff --seed`).

use crate::args::{optarg, unknown_opt, usage};
use crate::boxes::need_box;
use crate::cat;
use crate::exit::{Ctx, Res, out, say};
use crate::homesnap::{self, Change, Noise, Snap};
use crate::json;
use crate::select::select_box;
use crate::shell;
use crate::term::plain;
use serde_json::{Map, Value};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

/// The file `up` records the seeded HOME in (a name no snapshot can have: they start with a letter or
/// a digit).
pub const SEED: &str = ".seed";
/// A snapshot file is read up to this size (one of MAX_ENTRIES entries is well below it).
const FILE_MAX: u64 = 64 << 20;

/// Snapshot names are file names in the box's folder: as tame as save names.
fn snap_name(ctx: &Ctx, n: &[u8]) -> Res<()> {
    if shell::re_match("^[A-Za-z0-9][A-Za-z0-9_.-]{0,39}$", n) {
        return Ok(());
    }
    Err(ctx.die(cat!(
        "bad snapshot name: ",
        plain(n),
        " (letters, digits, . _ -; 40 at most)"
    )))
}

/// The box's HOME on the host, by its own folder (HOMES/NAME/home), not the box dir's link to it.
fn home_of(name: &[u8]) -> Vec<u8> {
    cat!(crate::paths::homes(), "/", name, "/home")
}

/// SNAP written as file NAME of box dir D's snapshots folder (made 0700), through a new file renamed
/// into place. Whether it was.
pub fn write_snap(d: &[u8], name: &str, s: &Snap) -> bool {
    let dir = cat!(d, "/snapshots");
    let made = std::fs::DirBuilder::new()
        .mode(0o700)
        .create(shell::path(&dir));
    if made.is_err() && !shell::path(&dir).is_dir() {
        return false;
    }
    let f = cat!(dir, "/", name);
    let tmp = cat!(dir, "/.new-", std::process::id().to_string(), "-", name);
    let w = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(shell::path(&tmp))
        .and_then(|mut o| o.write_all(&homesnap::encode(s)));
    if w.is_err() || std::fs::rename(shell::path(&tmp), shell::path(&f)).is_err() {
        let _ = std::fs::remove_file(shell::path(&tmp));
        return false;
    }
    true
}

/// `up`'s record of what it seeded (#446): box dir D's HOME walked before the box starts. Best
/// effort: a box with none says so at `diff --seed`.
pub fn record_seed(d: &[u8], name: &[u8]) {
    if let Some(s) = homesnap::walk(&home_of(name)) {
        write_snap(d, SEED, &s);
    }
}

/// Where the box's mounts sit in its HOME, each relative to it (box.json BJ): a plugin mounted
/// into the shell's folder (`plugin_via` "mounted"), and each of `mounts` whose place is under the
/// box's `home`.
fn mount_points(bj: &Value) -> Vec<Vec<u8>> {
    let mut v: Vec<Vec<u8>> = Vec::new();
    for (id, how) in bj
        .get("plugin_via")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        if how.as_str() == Some("mounted") {
            v.push(cat!(".config/omarchy/plugins/", id.as_bytes()));
        }
    }
    let home = bj.get("home").and_then(Value::as_str).unwrap_or("");
    if !home.is_empty() {
        for m in bj
            .get("mounts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(rel) = m
                .get("dest")
                .and_then(Value::as_str)
                .and_then(|d| crate::paths::under(d.as_bytes(), home.as_bytes()))
            {
                if !rel.is_empty() {
                    v.push(rel.to_vec());
                }
            }
        }
    }
    v
}

/// SEED given the folders of POINTS (and those on the way to each) as NOW has them, where SEED
/// has nothing and NOW a folder: what holds a mount is not a change since the seed.
fn mounts_into_seed(seed: &mut Snap, now: &Snap, points: &[Vec<u8>]) {
    for p in points {
        let mut end = 0;
        loop {
            end = match p[end..].iter().position(|&c| c == b'/') {
                Some(i) => end + i,
                None => p.len(),
            };
            let part = &p[..end];
            if let Some(e) = now.entries.get(part) {
                if e.kind == b'd' && !seed.entries.contains_key(part) {
                    seed.entries.insert(part.to_vec(), e.clone());
                }
            }
            if end == p.len() {
                break;
            }
            end += 1;
        }
    }
}

/// Snapshot file NAME of the box, read back; None when there is none or it is not one.
fn read_snap(ctx: &Ctx, name: &str) -> Option<Snap> {
    let f = crate::boxes::open_box_file(&ctx.at(&format!("snapshots/{name}")))?;
    let mut v = Vec::new();
    f.take(FILE_MAX).read_to_end(&mut v).ok()?;
    homesnap::decode(&v)
}

/// The box's snapshots by name (not up's own record), sorted.
fn snap_names(ctx: &Ctx) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(ctx.at("snapshots"))
        .map(|r| {
            r.filter_map(|e| e.ok()?.file_name().into_string().ok())
                .filter(|n| !n.starts_with('.'))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn walk_now(ctx: &Ctx) -> Res<Snap> {
    homesnap::walk(&home_of(&ctx.name))
        .ok_or_else(|| ctx.die(cat!(ctx.label(), "cannot read box '", ctx.name, "''s HOME")))
}

/// A walk that did not see all of the HOME, said.
fn say_cut(ctx: &Ctx, what: &str, s: &Snap) {
    if s.cut {
        say(cat!(
            ctx.label(),
            what,
            " is not the whole HOME: the walk stopped at ",
            homesnap::MAX_ENTRIES.to_string(),
            " entries or ",
            homesnap::MAX_DEPTH.to_string(),
            " folders deep"
        ));
    }
    if s.unread > 0 {
        say(cat!(
            ctx.label(),
            what,
            ": ",
            s.unread.to_string(),
            if s.unread == 1 {
                " folder could not be read"
            } else {
                " folders could not be read"
            }
        ));
    }
}

/// `omabox snapshot NAME [-b BOX]`.
pub fn snapshot(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut snap) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, args.get(i + 1))?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"-h" | b"--help" => return usage("snapshot"),
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "snapshot", a)),
            _ if !snap.is_empty() => {
                return Err(ctx.die(cat!(
                    "snapshot: one name, got ",
                    plain(&snap),
                    " and ",
                    plain(a),
                    " (the box goes with -b)"
                )));
            }
            _ => snap = a.clone(),
        }
        i += 1;
    }
    if snap.is_empty() {
        return Err(ctx.die("snapshot: name it (omabox snapshot NAME [-b BOX])"));
    }
    snap_name(ctx, &snap)?;
    select_box(ctx, &name)?;
    need_box(ctx)?;
    let s = walk_now(ctx)?;
    let n = String::from_utf8_lossy(&snap).into_owned();
    let had = ctx.at(&format!("snapshots/{n}")).exists();
    if !write_snap(&ctx.d, &n, &s) {
        return Err(ctx.die(cat!(
            "snapshot: could not write ",
            ctx.d,
            "/snapshots/",
            snap
        )));
    }
    say_cut(ctx, &format!("snapshot '{n}'"), &s);
    out(cat!(
        if had { "replaced" } else { "took" },
        " snapshot '",
        snap,
        "' of box '",
        ctx.name,
        "''s HOME: ",
        s.entries.len().to_string(),
        " entries (omabox diff ",
        snap,
        " shows what changes from here)\n"
    ))
}

/// Path P relative to the HOME, as shown: `~/P`, a `/` after a folder.
fn shown(p: &[u8], kind: u8) -> Vec<u8> {
    cat!("~/", p, if kind == b'd' { "/" } else { "" })
}

/// One change, as a line of text (after its area's heading).
fn line(p: &[u8], c: &Change, b: &Snap) -> Vec<u8> {
    let sign = match c.op {
        "added" => "+",
        "removed" => "-",
        _ => "~",
    };
    let mut l = cat!("  ", sign, " ", plain(&shown(p, c.kind)));
    if c.kind == b'l'
        && let Some(e) = b.entries.get(p)
    {
        l.extend_from_slice(&cat!(" -> ", plain(&e.target)));
    }
    let mut notes: Vec<String> = Vec::new();
    if !c.what.is_empty() {
        notes.push(c.what.join(", "));
    }
    if c.kind != b'f' && c.kind != b'd' && c.kind != b'l' {
        notes.push(homesnap::kind_word(c.kind).into());
    }
    if c.inside > 0 {
        notes.push(format!("{} inside", c.inside));
    }
    if !notes.is_empty() {
        l.extend_from_slice(format!("  ({})", notes.join("; ")).as_bytes());
    }
    l.push(b'\n');
    l
}

/// `omabox diff A [B] [-b BOX] [--seed] [--ignore PATH]... [--all] [--json]`.
pub fn diff(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut words) = (Vec::new(), Vec::<Vec<u8>>::new());
    let (mut seed, mut all, mut as_json) = (false, false, false);
    let mut noise = Noise::default_list();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_slice() {
            b"-b" | b"--box" | b"--ignore" => {
                optarg(ctx, a, args.get(i + 1))?;
                i += 1;
                let v = args[i].clone();
                if a == b"--ignore" {
                    if !noise.add(&v) {
                        return Err(ctx.usage_die(cat!(
                            "diff: --ignore takes a path in the box HOME (~/.config/app, or a glob: ~/*.tmp), got ",
                            plain(&v)
                        )));
                    }
                } else {
                    name = v;
                }
            }
            b"--seed" => seed = true,
            b"--all" => all = true,
            b"--json" => as_json = true,
            b"-h" | b"--help" => return usage("diff"),
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "diff", a)),
            _ => words.push(a.clone()),
        }
        i += 1;
    }
    let most = if seed { 1 } else { 2 };
    if words.len() > most {
        return Err(ctx.die(cat!(
            "diff: ",
            if seed {
                "--seed and one snapshot at most"
            } else {
                "two snapshots at most"
            },
            ", got ",
            plain(&words.join(&b' ')),
            " (the box goes with -b)"
        )));
    }
    if !seed && words.is_empty() {
        return Err(ctx
            .die("diff: name a snapshot (omabox diff NAME [NAME2]; --seed: from what up seeded)"));
    }
    for w in &words {
        snap_name(ctx, w)?;
    }
    if all {
        noise = Noise::default();
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    let missing = |w: &str| {
        let have = snap_names(ctx);
        let has = if have.is_empty() {
            "it has none: omabox snapshot NAME takes one".to_string()
        } else {
            format!("it has: {}", have.join(", "))
        };
        ctx.die(cat!(
            "diff: box '",
            ctx.name,
            "' has no snapshot '",
            w,
            "' (",
            has,
            ")"
        ))
    };
    let mut words = words
        .iter()
        .map(|w| String::from_utf8_lossy(w).into_owned());
    let (from_label, from_json, a) = if seed {
        let a = read_snap(ctx, SEED).ok_or_else(|| {
            ctx.die(cat!(
                "diff --seed: box '",
                ctx.name,
                "' has no record of what up seeded (an older omabox started it, or its HOME could not be read then)"
            ))
        })?;
        ("what up seeded".to_string(), "seed".to_string(), a)
    } else {
        let w = words.next().unwrap_or_default();
        let a = read_snap(ctx, &w).ok_or_else(|| missing(&w))?;
        (format!("snapshot '{w}'"), w, a)
    };
    let (to_label, to_json, b) = match words.next() {
        Some(w) => {
            let b = read_snap(ctx, &w).ok_or_else(|| missing(&w))?;
            (format!("snapshot '{w}'"), w, b)
        }
        None => ("now".to_string(), "now".to_string(), walk_now(ctx)?),
    };
    say_cut(ctx, &from_label, &a);
    say_cut(ctx, &to_label, &b);
    let mut a = a;
    if seed && to_json == "now" {
        // The folders the box's mounts sit on are made in its HOME when it starts, after up's
        // record: up's own, not a change (#483: a desk box listed each plugin's as added).
        let bj = std::fs::read(ctx.at("box.json"))
            .ok()
            .and_then(|t| serde_json::from_slice::<Value>(&t).ok())
            .unwrap_or(Value::Null);
        mounts_into_seed(&mut a, &b, &mount_points(&bj));
    }
    let (changes, left_out) = homesnap::diff(&a, &b, &noise);
    if as_json {
        let list: Vec<Value> = changes
            .iter()
            .map(|(p, c)| {
                let mut o = Map::new();
                let s = |v: &[u8]| Value::String(String::from_utf8_lossy(v).into_owned());
                o.insert("path".into(), s(&shown(p, c.kind)));
                o.insert("change".into(), Value::String(c.op.into()));
                o.insert(
                    "type".into(),
                    Value::String(homesnap::kind_word(c.kind).into()),
                );
                o.insert("area".into(), Value::String(homesnap::area(p).into()));
                if !c.what.is_empty() {
                    o.insert(
                        "what".into(),
                        Value::Array(c.what.iter().map(|w| Value::String((*w).into())).collect()),
                    );
                }
                if c.kind == b'l'
                    && let Some(e) = if c.op == "removed" { &a } else { &b }.entries.get(p)
                {
                    o.insert("target".into(), s(&e.target));
                }
                if c.inside > 0 {
                    o.insert("inside".into(), Value::from(c.inside));
                }
                Value::Object(o)
            })
            .collect();
        let mut o = Map::new();
        o.insert(
            "box".into(),
            Value::String(String::from_utf8_lossy(&ctx.name).into()),
        );
        o.insert("from".into(), Value::String(from_json));
        o.insert("to".into(), Value::String(to_json));
        o.insert("changes".into(), Value::Array(list));
        o.insert("left_out".into(), Value::from(left_out));
        o.insert("cut".into(), Value::Bool(a.cut || b.cut));
        return out(cat!(json::compact(&Value::Object(o)), "\n"));
    }
    let noise_note = if left_out == 0 || all {
        String::new()
    } else {
        format!("; {left_out} left out as noise (--all shows them)")
    };
    if changes.is_empty() {
        return out(format!(
            "no changes from {from_label} to {to_label}{noise_note}\n"
        ));
    }
    let mut text = Vec::new();
    for (_, heading) in homesnap::AREAS
        .iter()
        .map(|(p, h)| (*p, *h))
        .chain([("", homesnap::ELSEWHERE)])
        .filter({
            let mut seen = Vec::new();
            move |(_, h)| {
                let fresh = !seen.contains(h);
                seen.push(*h);
                fresh
            }
        })
    {
        let mine: Vec<&(Vec<u8>, Change)> = changes
            .iter()
            .filter(|(p, _)| homesnap::area(p) == heading)
            .collect();
        if mine.is_empty() {
            continue;
        }
        text.extend_from_slice(heading.as_bytes());
        text.push(b'\n');
        for (p, c) in mine {
            let side = if c.op == "removed" { &a } else { &b };
            text.extend_from_slice(&line(p, c, side));
        }
    }
    let count = |op: &str| changes.iter().filter(|(_, c)| c.op == op).count();
    text.extend_from_slice(
        format!(
            "{} added, {} changed, {} removed, from {from_label} to {to_label}{noise_note}\n",
            count("added"),
            count("changed"),
            count("removed")
        )
        .as_bytes(),
    );
    out(text)
}

#[cfg(test)]
mod mount_tests {
    use super::*;

    /// #483: where a box's mounts sit in its HOME, and those folders taken as seeded.
    #[test]
    fn mount_folders_are_the_seeds() {
        let bj: Value = serde_json::from_str(
            r#"{"home": "/home/u", "plugin_via": {"a.b": "mounted", "c.d": "linked"},
                "mounts": [{"src": "/home/u/.local/share/mise/installs", "dest": "/home/u/.local/share/mise/installs"},
                           {"src": "/srv/x", "dest": "/srv/x"}, {"src": "/home/u", "dest": "/home/u"}]}"#,
        )
        .unwrap();
        let pts = mount_points(&bj);
        assert_eq!(
            pts,
            vec![
                b".config/omarchy/plugins/a.b".to_vec(),
                b".local/share/mise/installs".to_vec()
            ]
        );
        let dir = |m: i64| homesnap::Entry {
            kind: b'd',
            mode: 0o755,
            size: 0,
            mtime: m,
            hash: None,
            target: Vec::new(),
        };
        let mut seed = Snap::default();
        let mut now = Snap::default();
        seed.entries.insert(b".local".to_vec(), dir(1));
        for p in [
            ".local",
            ".local/share",
            ".local/share/mise",
            ".local/share/mise/installs",
            ".config/omarchy/plugins/a.b",
            ".config/omarchy/plugins/other",
        ] {
            now.entries.insert(p.as_bytes().to_vec(), dir(2));
        }
        mounts_into_seed(&mut seed, &now, &pts);
        let keys: Vec<String> = seed
            .entries
            .keys()
            .map(|k| String::from_utf8_lossy(k).into_owned())
            .collect();
        assert_eq!(
            keys,
            [
                ".config/omarchy/plugins/a.b",
                ".local",
                ".local/share",
                ".local/share/mise",
                ".local/share/mise/installs"
            ]
        );
        assert_eq!(
            seed.entries[&b".local"[..]].mtime,
            1,
            "what the seed had stays its own"
        );
    }
}
