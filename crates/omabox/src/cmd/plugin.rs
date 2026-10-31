//! `omabox plugin [-b NAME] (add PATH|ID | remove ID)` (#345, finding 594): a shell plugin into a box
//! that is up, or out of it, as `up --plugin` puts one in. The folder resolved as up resolves it, and
//! refused where up refuses it (S1, S3); one the box has at its own path (a same-path mount up made,
//! as box.json's `mounts` lists them, its source still the folder up bound) is linked to from the box
//! HOME, any other mounted read-only there (latemount.rs, S4). Then the box's shell is made to know
//! it (a rescan, then listed again until it is: finding 539's race, which is why a link made by hand
//! and `omarchy plugin enable` at once said "is not known"), it is enabled as `omarchy plugin enable`
//! does, and plugin_check says what the shell made of it (box.json's plugin_status). `remove` takes
//! out one the box has, either way: disabled in the shell, unmounted or unlinked, rescanned.
//! What the box HOME holds is read and written from inside the box only (S7).

use crate::args::{optarg, unexpected, unknown_opt, unknown_word};
use crate::boxes::{box_alive, lock_file, need_box};
use crate::cat;
use crate::exec::{ErrTo, Out, on_box};
use crate::exit::{Ctx, Res, say};
use crate::jq::Args;
use crate::latemount::BoxNs;
use crate::mon::jq_box_json;
use crate::plugins::{Plugin, id_ok, resolve_plugin, session_out};
use crate::select::select_box;
use crate::shell;
use std::time::Duration;

/// The box HOME's plugins dir, as the box has it.
const PLUGINS: &[u8] = b"/home/sbx/.config/omarchy/plugins";

/// box.json with a plugin added ($i its id, $d its dir, $v linked or mounted).
const ADD_JQ: &str =
    ".plugins = ((.plugins // []) + [$i]) | .plugin_dirs[$i] = $d | .plugin_via[$i] = $v";
/// box.json with plugin $i gone: its dir, how it came and its state, and a --desk-shell box's list.
const REMOVE_JQ: &str = r#".plugins = ((.plugins // []) - [$i]) | del(.plugin_dirs[$i], .plugin_via[$i], .plugin_status[$i])
    | if (.desk_shell.plugins | type) == "array" then .desk_shell.plugins -= [$i] else . end"#;

/// In the box (S7): the place for plugin $2 made, a link to $3 ($1 link) or an empty folder for a
/// mount ($1 dir). A link already there (one made by hand) is replaced, said on stdout as `link
/// TARGET`; anything else there is left as it is, exit 3.
const PLACE_SH: &str = r#"p=/home/sbx/.config/omarchy/plugins t=/home/sbx/.config/omarchy/plugins/$2
if [ -L "$t" ]; then printf 'link %s\n' "$(readlink "$t")"; rm -f "$t" || exit 1
elif [ -e "$t" ]; then exit 3
fi
mkdir -p "$p" || exit 1
if [ "$1" = link ]; then ln -s "$3" "$t"; else mkdir "$t"; fi"#;

/// In the box: plugin $2's place gone, the link ($1 link) or the empty folder left by its mount
/// ($1 dir), never anything else.
const UNPLACE_SH: &str = r#"t=/home/sbx/.config/omarchy/plugins/$2
if [ "$1" = link ]; then if [ -L "$t" ]; then rm -f "$t"; fi
elif [ -d "$t" ] && [ ! -L "$t" ]; then rmdir "$t"; fi"#;

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut words) = (Vec::new(), Vec::<Vec<u8>>::new());
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, args.get(i + 1))?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"-h" | b"--help" => return crate::args::usage("plugin"),
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "plugin", a)),
            _ => words.push(a.clone()),
        }
        i += 1;
    }
    let (act, rest) = words
        .split_first()
        .map_or((&b""[..], &[][..]), |(a, r)| (a, r));
    match act {
        b"add" | b"remove" => {}
        b"" => return Err(ctx.usage_die("plugin: add PATH|ID or remove ID")),
        _ => {
            return Err(unknown_word(
                ctx,
                "plugin",
                "subcommand",
                act,
                "(add PATH|ID or remove ID)",
                &["add", "remove"],
            ));
        }
    }
    let Some(what) = rest.first() else {
        return Err(ctx.usage_die(cat!(
            "plugin ",
            act,
            ": which? ",
            if act == b"add" {
                "a plugin dir with manifest.json, or an id under ~/.config/omarchy/plugins"
            } else {
                "a plugin's id (omabox ls --json: plugins)"
            }
        )));
    };
    if let Some(extra) = rest.get(1) {
        return Err(unexpected(
            ctx,
            cat!("plugin ", act),
            extra,
            "(one plugin at a time)",
        ));
    }
    if act == b"add" {
        // (resolved as up resolves a --plugin, while the command line is read: a usage error)
        let p = resolve_plugin(ctx, what, &crate::jailpath::jail_read, false)?;
        select_box(ctx, &name)?;
        need_box(ctx)?;
        add(ctx, &p)
    } else {
        if !id_ok(what) {
            return Err(ctx.usage_die(cat!(
                "plugin remove: not a plugin's id: ",
                what,
                " (letters, digits, . _ -)"
            )));
        }
        select_box(ctx, &name)?;
        need_box(ctx)?;
        remove(ctx, what)
    }
}

/// box.json as JSON (null when it cannot be read).
fn box_json(ctx: &Ctx) -> serde_json::Value {
    std::fs::read(ctx.at("box.json"))
        .ok()
        .and_then(|t| serde_json::from_slice(&t).ok())
        .unwrap_or(serde_json::Value::Null)
}

fn text(v: &serde_json::Value) -> Option<Vec<u8>> {
    v.as_str().map(|s| s.as_bytes().to_vec())
}

/// The box's plugins and each one's dir and how it came (linked, mounted), from box.json.
fn box_plugins(bj: &serde_json::Value) -> Vec<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let ids = bj
        .get("plugins")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    ids.iter()
        .filter_map(|i| {
            let id = i.as_str()?;
            let dir = bj
                .get("plugin_dirs")
                .and_then(|v| v.get(id))
                .and_then(text)
                .unwrap_or_default();
            let via = bj
                .get("plugin_via")
                .and_then(|v| v.get(id))
                .and_then(text)
                .unwrap_or(b"mounted".to_vec());
            Some((id.as_bytes().to_vec(), dir, via))
        })
        .collect()
}

/// Whether the box sees DIR at its own path (finding 241's rule, own_path_among): through one of the
/// same-path mounts box.json lists, each whose source is still the folder up bound (one deleted and
/// made again on the host is the old one in the box: #391).
fn seen_at_own_path(bj: &serde_json::Value, dir: &[u8]) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Some(p) = crate::repo::readlink_e(dir) else {
        return false;
    };
    let mut specs = Vec::new();
    for m in bj
        .get("mounts")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let (Some(src), Some(dest)) = (m.get("src").and_then(text), m.get("dest").and_then(text))
        else {
            continue;
        };
        let was = (
            m.get("dev").and_then(serde_json::Value::as_u64),
            m.get("ino").and_then(serde_json::Value::as_u64),
        );
        let now = std::fs::metadata(shell::path(&src))
            .ok()
            .map(|n| (Some(n.dev()), Some(n.ino())));
        // (a source no longer the one bound counts as a mount from elsewhere: never linked through)
        let src = if now == Some(was) { src } else { Vec::new() };
        specs.push((src, dest));
    }
    crate::upspec::own_path_among(&p, &specs)
}

/// The box's lock, as up and down take it (mode's, finding 93), and the box still up under it.
fn locked(ctx: &Ctx, what: &[u8]) -> Res<std::fs::File> {
    let Some(lock) = lock_file(
        &ctx.in_boxes(cat!(".lock-", ctx.name)),
        Duration::from_secs(60),
    ) else {
        return Err(ctx.die(cat!(
            "plugin ",
            what,
            ": box '",
            ctx.name,
            "' is busy (an up or down)"
        )));
    };
    if !box_alive(ctx)? {
        return Err(ctx.die(cat!(
            "plugin ",
            what,
            ": box '",
            ctx.name,
            "' went down meanwhile"
        )));
    }
    Ok(lock)
}

/// A sh script run in the box (on_box: its user and mount namespaces, a clean environment), its
/// arguments after it; its status and stdout.
fn in_box(ctx: &Ctx, script: &str, args: &[&[u8]]) -> Res<(i32, Vec<u8>)> {
    let mut argv: Vec<Vec<u8>> = vec![
        b"/usr/bin/sh".to_vec(),
        b"-c".to_vec(),
        script.as_bytes().to_vec(),
        b"sh".to_vec(),
    ];
    argv.extend(args.iter().map(|a| a.to_vec()));
    // (its stderr dropped: what the box's names make sh say is the box's; failures go by status)
    let r = on_box(ctx, &argv, true, Out::Capture(Some(65536)), ErrTo::Null).or_exit()?;
    Ok((r.rc, r.out))
}

/// What the box wrote, fit for the caller's terminal: a control character (but newline and tab) or a
/// byte that is no UTF-8 as `?`, at most 400 bytes of it, what is cut said (S7: an escape sequence
/// the box wrote never reaches the terminal as one).
fn plain(v: &[u8]) -> Vec<u8> {
    const CAP: usize = 400;
    let (head, cut) = if v.len() > CAP {
        (&v[..CAP], v.len() - CAP)
    } else {
        (v, 0)
    };
    let mut out = Vec::new();
    for chunk in head.utf8_chunks() {
        for c in chunk.valid().chars() {
            if c.is_control() && c != '\n' && c != '\t' {
                out.push(b'?');
            } else {
                let mut b = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
            }
        }
        out.extend(std::iter::repeat_n(b'?', chunk.invalid().len()));
    }
    if cut > 0 {
        out.extend_from_slice(format!("... ({cut} bytes more)").as_bytes());
    }
    out
}

/// Whether the box was started with its shell (not --no-shell).
fn has_shell(bj: &serde_json::Value) -> bool {
    bj.get("shell") != Some(&serde_json::Value::Bool(false))
}

/// The shell's plugin list (listPlugins): ID's entry, None when the shell does not list it (or does
/// not answer).
fn listed(ctx: &mut Ctx, id: &[u8]) -> Option<serde_json::Value> {
    let out = crate::plugins::list_plugins(ctx)?;
    let v: serde_json::Value = serde_json::from_slice(&out).ok()?;
    v.as_array()?
        .iter()
        .find(|p| {
            p.get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::as_bytes)
                == Some(id)
        })
        .cloned()
}

/// The shell rescans its plugins (as `omarchy plugin add` asks it), then is listed again every 200 ms
/// until WANT holds of ID's entry, up to 4 s (finding 539: the rescan returns before it is done).
fn rescan_until(ctx: &mut Ctx, id: &[u8], want: bool) -> bool {
    let _ = session_out(ctx, &[b"omarchy-shell", b"-q", b"shell", b"rescanPlugins"]);
    for _ in 0..20 {
        if listed(ctx, id).is_some() == want {
            return true;
        }
        crate::sig::sleep(Duration::from_millis(200));
    }
    false
}

fn add(ctx: &mut Ctx, p: &Plugin) -> Res<()> {
    let n = ctx.name.clone();
    let dest = cat!(PLUGINS, "/", p.id);
    // S1 and S3 as up's checked_ro has them for a --plugin: refuse_src, the seat's paths.
    let runtime = shell::env_or_empty("XDG_RUNTIME_DIR");
    crate::bwrap::Bwrap::new(&runtime, &ctx.d, Vec::new(), Vec::new())
        .checked_ro(&p.dir, &dest)
        .map_err(|r| ctx.die(cat!(ctx.label(), r.0)))?;
    let lock = locked(ctx, b"add")?;
    let bj = box_json(ctx);
    // A jail's box (S9): what its jail hides is covered on the mounts up makes (jail_cover), never
    // on one made later, so a plugin goes in there with up --plugin.
    if bj.get("jail").is_some_and(|j| !j.is_null()) {
        return Err(ctx.die(cat!(
            "plugin add: box '",
            n,
            "' was started inside ai-jail: what its jail hides is checked on the mounts up makes only; start it with up --plugin there"
        )));
    }
    if let Some((_, dir, via)) = box_plugins(&bj).into_iter().find(|(i, _, _)| *i == p.id) {
        if dir == p.dir {
            say(cat!(
                "box '",
                n,
                "' has plugin ",
                p.id,
                " already (",
                via,
                ": ",
                dir,
                ")"
            ));
            return Ok(());
        }
        return Err(ctx.die(cat!(
            "plugin add: box '",
            n,
            "' has plugin ",
            p.id,
            " from ",
            dir,
            " already: omabox plugin remove ",
            p.id,
            " -b ",
            n,
            " first"
        )));
    }
    let linked = seen_at_own_path(&bj, &p.dir);
    let (rc, out) = in_box(
        ctx,
        PLACE_SH,
        &[if linked { b"link" } else { b"dir" }, &p.id, &p.dir],
    )?;
    if rc == 3 {
        return Err(ctx.die(cat!(
            "plugin add: box '",
            n,
            "' has ~/.config/omarchy/plugins/",
            p.id,
            " already, not one of omabox's (an omarchy plugin add's?): omabox run -b ",
            n,
            " -- omarchy plugin remove ",
            p.id,
            " --yes, then add it again"
        )));
    }
    if rc != 0 {
        return Err(ctx.die(cat!(
            "plugin add: could not make ~/.config/omarchy/plugins/",
            p.id,
            " in box '",
            n,
            "'"
        )));
    }
    if let Some(t) = out.strip_prefix(b"link ") {
        say(cat!(
            "plugin add: replaced the link at ~/.config/omarchy/plugins/",
            p.id,
            " in box '",
            n,
            "' (to ",
            plain(&shell::strip_nl(t)),
            ")"
        ));
    }
    if !linked {
        let r = BoxNs::open(&ctx.d).and_then(|b| b.mount_ro(&p.dir, PLUGINS, &p.id));
        if let Err(why) = r {
            let _ = in_box(ctx, UNPLACE_SH, &[b"dir", &p.id]);
            return Err(ctx.die(cat!(
                "plugin add: cannot mount ",
                p.dir,
                " in box '",
                n,
                "': ",
                why
            )));
        }
    }
    let via: &[u8] = if linked { b"linked" } else { b"mounted" };
    if !jq_box_json(
        ctx,
        Args::new(&[])
            .kv("--arg", "i", &p.id)
            .kv("--arg", "d", &p.dir)
            .kv("--arg", "v", via)
            .arg(ADD_JQ),
    ) {
        return Err(ctx.die(cat!(
            "plugin add: ",
            p.id,
            " is in box '",
            n,
            "', but box.json was not written (ls will not show it)"
        )));
    }
    drop(lock);
    say(cat!(
        "plugin ",
        p.id,
        " added to box '",
        n,
        "': ",
        if linked {
            cat!("linked to ", p.dir, " (the box has it at its own path)")
        } else {
            cat!("mounted read-only from ", p.dir)
        }
    ));
    if !has_shell(&bj) {
        say(cat!(
            "box '",
            n,
            "' has no shell (--no-shell): nothing loads the plugin"
        ));
        return Ok(());
    }
    if !rescan_until(ctx, &p.id, true) {
        say(cat!(
            "plugin add: the box's shell does not list ",
            p.id,
            " after a rescan: omabox restart-shell -b ",
            n,
            " loads it (why: omabox log -b ",
            n,
            " shell)"
        ));
    } else {
        match session_out(ctx, &[b"omarchy", b"plugin", b"enable", &p.id]) {
            Some((0, _)) => {}
            r => {
                let said = r
                    .map(|(_, o)| plain(&shell::strip_nl(&o)))
                    .unwrap_or_default();
                say(cat!(
                    "plugin add: omarchy plugin enable ",
                    p.id,
                    " failed in box '",
                    n,
                    "'",
                    if said.is_empty() {
                        Vec::new()
                    } else {
                        cat!(": ", said)
                    }
                ));
            }
        }
    }
    crate::plugins::plugin_check(ctx, &crate::install::git_rev)
}

fn remove(ctx: &mut Ctx, id: &[u8]) -> Res<()> {
    let n = ctx.name.clone();
    let bj = box_json(ctx);
    let have = box_plugins(&bj);
    let Some((_, _, via)) = have.iter().find(|(i, _, _)| i == id).cloned() else {
        let ids: Vec<Vec<u8>> = have.into_iter().map(|(i, _, _)| i).collect();
        return Err(ctx.die(cat!(
            "plugin remove: box '",
            n,
            "' has no plugin ",
            id,
            " (",
            if ids.is_empty() {
                b"it has none".to_vec()
            } else {
                cat!("it has ", ids.join(&b' '))
            },
            ")"
        )));
    };
    // Off in the shell first, while its files are there (as omarchy plugin remove does).
    if has_shell(&bj)
        && listed(ctx, id)
            .and_then(|e| e.get("enabled").cloned())
            .is_some_and(|e| e == serde_json::Value::Bool(true))
    {
        let _ = session_out(
            ctx,
            &[
                b"omarchy-shell",
                b"shell",
                b"setPluginEnabled",
                id,
                b"false",
            ],
        );
    }
    let lock = locked(ctx, b"remove")?;
    let linked = via == b"linked";
    if !linked {
        let r = BoxNs::open(&ctx.d).and_then(|b| b.umount(PLUGINS, id));
        if let Err(why) = r {
            return Err(ctx.die(cat!(
                "plugin remove: cannot unmount ",
                id,
                " in box '",
                n,
                "': ",
                why
            )));
        }
    }
    let _ = in_box(
        ctx,
        UNPLACE_SH,
        &[if linked { b"link" } else { b"dir" }, id],
    )?;
    if !jq_box_json(ctx, Args::new(&[]).kv("--arg", "i", id).arg(REMOVE_JQ)) {
        return Err(ctx.die(cat!(
            "plugin remove: ",
            id,
            " is out of box '",
            n,
            "', but box.json was not written (ls still shows it)"
        )));
    }
    drop(lock);
    if has_shell(&bj) && !rescan_until(ctx, id, false) {
        say(cat!(
            "plugin remove: the box's shell still lists ",
            id,
            ": omabox restart-shell -b ",
            n,
            " drops it"
        ));
    }
    say(cat!("plugin ", id, " removed from box '", n, "'"));
    // (the others' states as the shell has them now)
    crate::plugins::plugin_check(ctx, &crate::install::git_rev)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bj(mounts: &str) -> serde_json::Value {
        serde_json::from_str(&format!(r#"{{"mounts": {mounts}}}"#)).unwrap()
    }

    /// #345: a plugin is linked when box.json's same-path mounts hold it, as up's
    /// own_path_visible decides from its own; mounted otherwise.
    #[test]
    fn linked_only_through_a_same_path_mount_still_bound() {
        use std::os::unix::fs::MetadataExt;
        let d = std::env::temp_dir().join(format!("omabox-pa-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("repo/widget")).unwrap();
        std::fs::create_dir_all(d.join("other/widget")).unwrap();
        let repo = d.join("repo").to_string_lossy().into_owned();
        let m = std::fs::metadata(&repo).unwrap();
        let w = format!("{repo}/widget");
        let o = d.join("other/widget").to_string_lossy().into_owned();
        let same = format!(
            r#"[{{"src": "{repo}", "dest": "{repo}", "dev": {}, "ino": {}}}]"#,
            m.dev(),
            m.ino()
        );
        assert!(
            seen_at_own_path(&bj(&same), w.as_bytes()),
            "in a same-path mount"
        );
        assert!(!seen_at_own_path(&bj(&same), o.as_bytes()), "outside it");
        assert!(!seen_at_own_path(&bj("[]"), w.as_bytes()), "no mounts");
        let moved = format!(r#"[{{"src": "/elsewhere", "dest": "{repo}", "dev": 1, "ino": 1}}]"#);
        assert!(
            !seen_at_own_path(&bj(&moved), w.as_bytes()),
            "a mount from elsewhere"
        );
        let remade = format!(
            r#"[{{"src": "{repo}", "dest": "{repo}", "dev": {}, "ino": {}}}]"#,
            m.dev(),
            m.ino() + 1
        );
        assert!(
            !seen_at_own_path(&bj(&remade), w.as_bytes()),
            "a source deleted and made again since up"
        );
        let over = format!(
            r#"[{{"src": "{repo}", "dest": "{repo}", "dev": {}, "ino": {}}}, {{"src": "/x", "dest": "{w}", "dev": 1, "ino": 1}}]"#,
            m.dev(),
            m.ino()
        );
        assert!(
            !seen_at_own_path(&bj(&over), w.as_bytes()),
            "a mount from elsewhere over it"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// #345: what the box wrote reaches the terminal with no control character but newline and tab,
    /// no byte that is not UTF-8, and capped.
    #[test]
    fn plain_keeps_escapes_off_the_terminal() {
        assert_eq!(plain(b"/a/b\tc\nd"), b"/a/b\tc\nd");
        assert_eq!(plain(b"x\x1b]52;c;aGk=\x07y"), b"x?]52;c;aGk=?y");
        assert_eq!(plain(b"\x1b[31mred\r"), b"?[31mred?");
        assert_eq!(plain("a\u{9b}b é".as_bytes()), "a?b é".as_bytes());
        assert_eq!(plain(b"a\x9bb\xff"), b"a?b?");
        let long = vec![b'z'; 65536];
        let p = plain(&long);
        assert!(p.starts_with(&[b'z'; 400]) && p.ends_with(b"... (65136 bytes more)"));
    }

    #[test]
    fn box_plugins_reads_ids_dirs_and_how() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"plugins": ["a.b", "c"], "plugin_dirs": {"a.b": "/p/a", "c": "/p/c"}, "plugin_via": {"a.b": "linked"}}"#,
        )
        .unwrap();
        assert_eq!(
            box_plugins(&v),
            vec![
                (b"a.b".to_vec(), b"/p/a".to_vec(), b"linked".to_vec()),
                (b"c".to_vec(), b"/p/c".to_vec(), b"mounted".to_vec()),
            ]
        );
        assert!(box_plugins(&serde_json::Value::Null).is_empty());
    }
}
