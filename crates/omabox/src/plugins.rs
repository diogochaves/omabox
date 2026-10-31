//! Plugins and themes for up (#324 wave 5a): --plugin and the desk's resolved, --theme named, and after
//! up what the shell made of each plugin. As bin/omabox's resolve_plugin, desk_config, desk_plugins,
//! plugin_check (with shell_warnings), theme_named and tree_dir.

use crate::boxes::{box_home_file, meta};
use crate::cat;
use crate::exec::{ExecOpts, Redir, box_exec, pipe, read_all};
use crate::exit::{Ctx, Exit, Res, say};
use crate::jail::in_jail;
use crate::jq::{self, Args};
use crate::repo::{readlink_e, readlink_f, refuse_src};
use crate::shell::{self, strip_nl};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;

/// A manifest's id.
const ID_JQ: &str = ".id // empty";
/// plugin_check's programs, each as bash's omabox had it.
const IDS_JQ: &str = ".plugins[]?";
const DESK_IDS_JQ: &str = r#".desk_shell.plugins // [] | join(" ")"#;
const SHELL_JQ: &str = ".shell | tostring";
const LISTED_JQ: &str = "map({(.id): {enabled, kinds}}) | add // {}";
const DIR_JQ: &str = ".plugin_dirs[$i] // empty";
const KINDS_JQ: &str = r#".[$i].kinds // [] | join(" ")"#;
const HAS_JQ: &str = "has($i)";
const ENABLED_JQ: &str = ".[$i].enabled";
/// A widget in another mounted plugin's own entry (a sidebar that hosts widgets, #127).
const HOST_JQ: &str = r#"[(.plugins // [])[] | select(.id != $id and (.id as $p | $m | index($p)))
            | select([.. | if type == "object" then .id? elif type == "string" then . else empty end] | index($id)) | .id][0] // empty"#;
/// Its own `plugins[]` entry, not in `disabledPlugins`: what the registry's isEnabled asks of a plugin
/// off the bar (#326). `listPlugins`'s `enabled` is `inBar` for any plugin with a bar widget, so a
/// service with a widget, loaded from its entry, reads as off there.
const OWN_JQ: &str = r#"((.disabledPlugins // []) | index($i) | not)
            and ([(.plugins // [])[]? | objects | .id] | index($i) != null)"#;
/// The other mounted plugins running with more than a bar widget (a service, a panel, a bar...): any
/// of them may place a widget through its own config and IPC, where shell.json does not show it (#326).
const PLACERS_JQ: &str = r#". as $c
            | def own($p): (($c.disabledPlugins // []) | index($p) | not)
                and ([($c.plugins // [])[]? | objects | .id] | index($p) != null);
            [$m[] | . as $p | select($p != $i and $l[$p] != null)
              | select(($l[$p].kinds // []) | any(. != "bar-widget"))
              | select($l[$p].enabled == true or own($p))] | join(" or ")"#;
const STATUS_JQ: &str = r#".[$i] = ({state: $s} + (if $h != "" then {host: $h} else {} end) + (if $c != "" then {commit: $c} else {} end) + (if $w != "" then {log: $w} else {} end)
        + (if $v != "" then {validate: $v} else {} end))"#;
const PLUGIN_STATUS_JQ: &str = ".plugin_status = $s";

/// A plugin a box mounts: its manifest's id and its dir, resolved (bash's "id<TAB>dir").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plugin {
    pub id: Vec<u8>,
    pub dir: Vec<u8>,
}

/// `^[A-Za-z0-9_][A-Za-z0-9._-]*$`, ASCII (finding 445): a manifest id that names a dir in the box
/// HOME and is split on whitespace (finding 175).
pub fn id_ok(id: &[u8]) -> bool {
    id.first()
        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
        && id
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

/// resolve_plugin P: a plugin dir with manifest.json, or an id under ~/.config/omarchy/plugins (not
/// for a jailed caller), its id read from its manifest. A jailed caller's manifest (#198) only
/// through JAIL_READ (jailpath.rs's jail_read: a link swapped in since up followed it is not read
/// through). QUIET: its refusals and jq's words held back (the jailed caller's `2>/dev/null`). A
/// refusal is up's (`|| refused`): a usage error while the command line is read.
pub fn resolve_plugin(
    ctx: &Ctx,
    p: &[u8],
    jail_read: &dyn Fn(&[u8]) -> Option<Vec<u8>>,
    quiet: bool,
) -> Res<Plugin> {
    let jerr = if quiet {
        jq::Err::Null
    } else {
        jq::Err::Inherit
    };
    let die = |m: Vec<u8>| -> Exit {
        if quiet {
            crate::exit::muted(|| ctx.die(m))
        } else {
            ctx.die(m)
        }
    };
    let home_p = cat!(shell::env_or_empty("HOME"), "/.config/omarchy/plugins/", p);
    let dir = if shell::path(&cat!(p, "/manifest.json")).is_file() {
        readlink_f(p).unwrap_or_else(|| p.to_vec())
    } else if !in_jail() && shell::path(&cat!(home_p, "/manifest.json")).is_file() {
        readlink_f(&home_p).unwrap_or(home_p)
    } else {
        return Err(die(cat!(
            "plugin not found: ",
            p,
            " (a plugin dir with manifest.json, or an id under ~/.config/omarchy/plugins)"
        )));
    };
    let mf = cat!(dir, "/manifest.json");
    let id = if in_jail() {
        // (`id=$(jail_read ... | jq ...) || id=""`)
        match jail_read(&mf) {
            Some(t) => {
                let r = jq::tool("jq", &["-r", ID_JQ], Some(&t), jerr);
                if r.ok() { r.text() } else { Vec::new() }
            }
            None => Vec::new(),
        }
    } else {
        // (a plain assignment under set -e: jq's failure ends up, its words said)
        let r = Args::new(&["-r", ID_JQ]).arg(&mf).run_file(jerr);
        if !r.ok() {
            return Err(ctx.refused());
        }
        r.text()
    };
    if id.is_empty() {
        return Err(die(cat!("no id in ", dir, "/manifest.json")));
    }
    if !id_ok(&id) {
        return Err(die(cat!(
            "plugin ",
            dir,
            ": its manifest id '",
            id,
            "' must be letters, digits, . _ - (not starting with . or -)"
        )));
    }
    Ok(Plugin { id, dir })
}

/// desk_config (#206): the desk's Omarchy config dir, resolved (a dotfiles link): ~/.config/omarchy,
/// or OMABOX_DESK_CONFIG, the suite's stand-in (never a jailed caller's: --desk-shell is refused in
/// a jail). Its refusal is a failure, 1: up reads it past its command line (`PARSING=0`).
pub fn desk_config(ctx: &Ctx) -> Res<Vec<u8>> {
    let d = shell::env("OMABOX_DESK_CONFIG")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| cat!(shell::env_or_empty("HOME"), "/.config/omarchy"));
    match readlink_e(&d) {
        Some(r) if shell::path(&r).is_dir() => Ok(r),
        _ => {
            let mut c = ctx.clone();
            c.parsing = false;
            Err(c.die(cat!("--desk-shell: no Omarchy config folder at ", d)))
        }
    }
}

/// `"$DIR"/* "$DIR"/.[!.]*`: DIR's entries as bash lists them, the plain ones then the hidden ones
/// (not . and .., nor a name starting with ..), each in byte order (bash's glob: the locale's
/// collation, finding 446). An unmatched glob's word is left out: `[ -e ]` drops it anyway.
fn entries(dir: &[u8]) -> Vec<Vec<u8>> {
    let Ok(rd) = std::fs::read_dir(shell::path(dir)) else {
        return Vec::new();
    };
    let (mut plain, mut hidden): (Vec<Vec<u8>>, Vec<Vec<u8>>) = (Vec::new(), Vec::new());
    for e in rd.flatten() {
        let n = e.file_name().into_encoded_bytes();
        match (n.first(), n.get(1)) {
            (Some(b'.'), Some(b'.') | None) => {}
            (Some(b'.'), Some(_)) => hidden.push(n),
            _ => plain.push(n),
        }
    }
    plain.sort();
    hidden.sort();
    plain.into_iter().chain(hidden).collect()
}

/// desk_plugins DESK HAVE (#206): the plugins in the desk's plugins/, as resolve_plugin has them; one
/// HAVE has already (a --plugin of the same id: a working copy) is left to it. What omabox adds by
/// itself is skipped, said, where a --plugin is refused: no manifest, an id that is not a name, a
/// dir refuse_src keeps out of a box, a second dir with the same id, omabox's own widget (#304).
pub fn desk_plugins(desk: &[u8], have: &[Plugin]) -> Vec<Plugin> {
    let mut ids: Vec<Vec<u8>> = have.iter().map(|p| p.id.clone()).collect();
    let mut out = Vec::new();
    let pd = cat!(desk, "/plugins");
    for n in entries(&pd) {
        let e = cat!(pd, "/", n);
        if !shell::path(&e).exists() {
            continue;
        }
        if !shell::path(&cat!(e, "/manifest.json")).is_file() {
            say(cat!(
                "--desk-shell: not mounting plugins/",
                n,
                ": no manifest.json in it"
            ));
            continue;
        }
        let Some(dir) = readlink_f(&e) else { continue };
        let r = Args::new(&["-r", ID_JQ])
            .arg(cat!(dir, "/manifest.json"))
            .run_file(jq::Err::Null);
        let id = if r.ok() { r.text() } else { Vec::new() };
        if !id_ok(&id) {
            say(cat!(
                "--desk-shell: not mounting plugins/",
                n,
                ": its manifest id '",
                id,
                "' is not letters, digits, . _ -"
            ));
            continue;
        }
        if ids.contains(&id) {
            continue;
        }
        // omabox's own bar widget (#304): it lists your boxes with `omabox ls`, which a box has not;
        // --plugin mounts it all the same.
        if id == b"chaves.omabox" {
            say(cat!(
                "--desk-shell: not mounting plugins/",
                n,
                ": omabox's own widget, which lists your boxes (none in a box)"
            ));
            continue;
        }
        if let Some(why) = refuse_src(&dir) {
            say(cat!(
                "--desk-shell: not mounting plugins/",
                n,
                " (",
                dir,
                "): ",
                why
            ));
            continue;
        }
        ids.push(id.clone());
        out.push(Plugin { id, dir });
    }
    out
}

/// shell_warnings's filter of a shell log (finding 372): its warnings and errors, colour codes, the
/// time and the level/category prefix dropped; not Quickshell's about every plugin dir outside the
/// shell's config folder. Lines joined by newlines, none after the last (`$(...)`). Matched as bytes
/// (sed's `.` in a UTF-8 locale takes no invalid byte: finding 445's ASCII rule).
pub fn shell_warnings_of(log: &[u8]) -> Vec<u8> {
    let (Some(skip), Some(lead)) = (
        shell::Re::new(
            r" quickshell\.qmlscanner: Module path .* is outside of the config folder\.$",
        ),
        shell::Re::new(r"^([0-9]{4}-[0-9]{2}-[0-9]{2} [0-9:.]+)? *(WARN|ERROR) +[^:]*: "),
    ) else {
        return Vec::new();
    };
    let mut lines: Vec<&[u8]> = log.split(|&c| c == b'\n').collect();
    if log.ends_with(b"\n") {
        lines.pop();
    }
    let mut out: Vec<Vec<u8>> = Vec::new();
    for l in lines {
        let l = no_colours(l);
        if skip.is_match(&l) {
            continue;
        }
        if let Some(m) = lead.captures(&l, false)
            && let Some(Some((_, end))) = m.first().copied()
        {
            out.push(l[end..].to_vec());
        }
    }
    strip_nl(&out.join(&b'\n'))
}

/// `sed 's/\x1b\[[0-9;]*m//g'`.
fn no_colours(l: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(l.len());
    let mut i = 0;
    while i < l.len() {
        if l[i] == 0x1b && l.get(i + 1) == Some(&b'[') {
            let mut j = i + 2;
            while j < l.len() && (l[j].is_ascii_digit() || l[j] == b';') {
                j += 1;
            }
            if l.get(j) == Some(&b'm') {
                i = j + 1;
                continue;
            }
        }
        out.push(l[i]);
        i += 1;
    }
    out
}

/// shell_warnings FILE: the filter over a shell log (empty when it cannot be read).
pub fn shell_warnings(f: &std::path::Path) -> Vec<u8> {
    std::fs::read(f)
        .map(|t| shell_warnings_of(&t))
        .unwrap_or_default()
}

/// `grep -E "(^|[^A-Za-z0-9._-])ID([^A-Za-z0-9._-]|$)" <<<"$LOG"`: the lines naming ID, its id
/// between characters an id cannot have (bytes: grep's bracket takes no invalid one in a UTF-8 locale,
/// finding 445's ASCII rule), joined by newlines.
fn lines_naming(log: &[u8], id: &[u8]) -> Vec<u8> {
    let idc = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-');
    let mut out: Vec<&[u8]> = Vec::new();
    for l in log.split(|&c| c == b'\n') {
        if id.is_empty() || l.len() < id.len() {
            continue;
        }
        let hit = (0..=l.len() - id.len()).any(|i| {
            &l[i..i + id.len()] == id
                && (i == 0 || !idc(l[i - 1]))
                && l.get(i + id.len()).is_none_or(|&c| !idc(c))
        });
        if hit {
            out.push(l);
        }
    }
    out.join(&b'\n')
}

/// The first line of V.
fn first_line(v: &[u8]) -> Vec<u8> {
    v.split(|&c| c == b'\n').next().unwrap_or_default().to_vec()
}

/// `$(box_exec omarchy-shell shell listPlugins 2>/dev/null | head -c 1048576)`: its output, None when
/// it failed (pipefail).
pub fn list_plugins(ctx: &mut Ctx) -> Option<Vec<u8>> {
    match session_out(ctx, &[b"omarchy-shell", b"shell", b"listPlugins"]) {
        Some((0, out)) => Some(out),
        _ => None,
    }
}

/// WORDS in the box's session (box_exec), stderr dropped, stdout read up to 1 MiB:
/// its status and what it printed, None when it could not run.
pub fn session_out(ctx: &mut Ctx, words: &[&[u8]]) -> Option<(i32, Vec<u8>)> {
    let (r, wr) = pipe().ok()?;
    let reader = std::thread::spawn(move || read_all(r, Some(1_048_576)));
    let rc = {
        let _e = Redir::null(2);
        box_exec(
            ctx,
            &crate::mon::w(words),
            ExecOpts {
                stdout: Some(wr.as_raw_fd()),
                ..ExecOpts::default()
            },
        )
    };
    drop(wr);
    let out = reader.join().unwrap_or_default();
    rc.ok().map(|rc| (rc, out))
}

/// `"$VALIDATE" "$DIR" 2>&1 >/dev/null`: whether the validator passed, and what it said.
fn validate(v: &[u8], dir: &[u8]) -> (bool, Vec<u8>) {
    let mut c = crate::sig::command(std::ffi::OsStr::from_bytes(v));
    c.arg(std::ffi::OsStr::from_bytes(dir))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let r = crate::sig::run_child(c, false, |ch| {
        let mut e = Vec::new();
        if let Some(mut s) = ch.stderr.take() {
            let _ = std::io::Read::read_to_end(&mut s, &mut e);
        }
        e
    });
    match r {
        Ok((e, (rc, _))) => (rc == 0, strip_nl(&e)),
        // (a validator that could not start: said in omabox's words, finding 441)
        Err(e) => (false, cat!("omabox: cannot run ", v, ": ", e.to_string())),
    }
}

/// `[[ " $KINDS " =~ \ (bar-widget|bar|service)\  ]]`: a kind whose QML loads with the shell.
fn kinds_load(kinds: &[u8]) -> bool {
    shell::words(kinds)
        .iter()
        .any(|k| matches!(k.as_slice(), b"bar-widget" | b"bar" | b"service"))
}

/// One of plugin_check's jq programs over LISTED with `--arg i ID`: its run.
fn listed_jq(flag: &str, id: &[u8], prog: &str, listed: &[u8]) -> jq::Ran {
    jq::jq(
        &[flag.as_bytes(), b"--arg", b"i", id, prog.as_bytes()],
        Some(listed),
        jq::Err::Inherit,
    )
}

/// plugin_check (finding 138): why a mounted plugin is not in the box's bar, after up (and
/// restart-shell). Omarchy's validator on each mounted plugin's dir (the box's Omarchy's; for a
/// jailed caller the host's installed one, never the jail's tree: #198), then the shell's plugin list
/// and its log. Warnings, never a refusal. Each plugin's state goes into box.json's plugin_status:
/// loaded, failed, not loaded, disabled, hosted (#127), off the bar (#326) or enabled. GIT_REV:
/// install.rs's git_rev (a plugin dir's commit; never run for a jailed caller).
pub fn plugin_check(ctx: &mut Ctx, git_rev: &dyn Fn(&[u8]) -> Vec<u8>) -> Res<()> {
    let bj = ctx.at("box.json");
    let bjs = shell::bytes(&bj).to_vec();
    let ids = Args::new(&["-r", IDS_JQ]).arg(&bjs).run_file(jq::Err::Null);
    if !ids.ok() {
        return Ok(());
    }
    let ids = ids.text();
    // An --desk-shell box's (#206): the desk's plugins it has off stay off, unsaid.
    let dr = Args::new(&["-r", DESK_IDS_JQ])
        .arg(&bjs)
        .run_file(jq::Err::Null);
    let desk: Vec<Vec<u8>> = if dr.ok() {
        shell::words(&dr.text())
    } else {
        Vec::new()
    };
    if ids.is_empty() {
        return Ok(());
    }
    let tree = meta(&ctx.d, "omarchy").0;
    let mut v_bin = cat!(
        if tree.is_empty() {
            b"/usr/share/omarchy".as_slice()
        } else {
            &tree
        },
        "/bin/omarchy-plugin-validate"
    );
    // A jailed caller's --omarchy tree is theirs to write: its validator never runs on the host
    // (#198), the host's installed one does (it only reads the plugin dir).
    if in_jail() {
        v_bin = b"/usr/share/omarchy/bin/omarchy-plugin-validate".to_vec();
    }
    if !crate::exec::executable(&v_bin) {
        v_bin.clear();
    }
    let (mut listed, mut cfg, mut log): (Option<Vec<u8>>, Option<std::path::PathBuf>, Vec<u8>) =
        (None, None, Vec::new());
    let sh = Args::new(&["-r", SHELL_JQ])
        .arg(&bjs)
        .run_file(jq::Err::Inherit)
        .text();
    if sh != b"false" {
        // (the session's omarchy-shell, an --omarchy tree's too: its answer read up to 1 MiB)
        listed = list_plugins(ctx).and_then(|o| {
            let r = jq::tool("jq", &["-c", LISTED_JQ], Some(&o), jq::Err::Null);
            let t = r.text();
            (r.ok() && !t.is_empty()).then_some(t)
        });
        cfg = box_home_file(&ctx.d, ".config/omarchy/shell.json");
        if let Some(f) = box_home_file(&ctx.d, "shell.log") {
            log = shell_warnings(&f);
        }
    }
    let id_list = shell::words(&ids);
    // (`jq -R . <<<"$ids" | jq -sc .`)
    let m = cat!(
        "[",
        id_list
            .iter()
            .map(|i| crate::json::quote(&String::from_utf8_lossy(i)).into_bytes())
            .collect::<Vec<_>>()
            .join(&b','),
        "]"
    );
    let mut status = b"{}".to_vec();
    let mut bad = false;
    for id in &id_list {
        let dr = Args::new(&["-r", "--arg", "i"])
            .arg(id)
            .arg(DIR_JQ)
            .arg(&bjs)
            .run_file(jq::Err::Inherit);
        if !dr.ok() {
            // (a plain assignment under set -e: up ends with jq's status)
            return Err(Exit(u8::try_from(dr.rc).unwrap_or(1)));
        }
        let dir = dr.text();
        let mut v = Vec::new();
        if !v_bin.is_empty() && !dir.is_empty() {
            let (ok, said) = validate(&v_bin, &dir);
            if !ok {
                v = if said.is_empty() {
                    cat!(v_bin, " failed")
                } else {
                    said
                };
                say(cat!("warning: plugin ", id, ": ", v));
                bad = true;
            }
        }
        let (mut state, mut why, mut host): (&str, Vec<u8>, Vec<u8>) =
            ("unchecked", Vec::new(), Vec::new());
        if let Some(l) = &listed {
            // Lines naming it: its id between characters an id cannot have.
            let lines = lines_naming(&log, id);
            let kinds = listed_jq("-r", id, KINDS_JQ, l).text();
            let failed = lines.split(|&c| c == b'\n').find(|x| {
                [
                    b" failed".as_slice(),
                    b"failed to load",
                    b"has no barWidget entry point",
                ]
                .iter()
                .any(|n| crate::exit::contains(x, n))
            });
            if !listed_jq("-e", id, HAS_JQ, l).ok() {
                state = "not loaded";
                why = first_line(&lines);
                if why.is_empty() {
                    why = b"the shell does not list it".to_vec();
                }
            } else if let Some(f) = failed {
                state = "failed";
                why = f.to_vec();
            } else if !listed_jq("-e", id, ENABLED_JQ, l).ok() {
                // A widget in another mounted plugin's own entry (a sidebar that hosts widgets) is
                // that plugin's to load, not the bar's: listed, not enabled, and fine (#127).
                if let Some(c) = &cfg {
                    let h = Args::new(&["-r", "--arg", "id"])
                        .arg(id)
                        .kv("--argjson", "m", &m)
                        .arg(HOST_JQ)
                        .arg(shell::bytes(c))
                        .run_file(jq::Err::Null);
                    if h.ok() {
                        host = h.text();
                    }
                }
                let words = shell::words(&kinds);
                let widget = words.iter().any(|k| k == b"bar-widget");
                let more = words.iter().any(|k| k != b"bar-widget");
                if !host.is_empty() {
                    state = "hosted";
                } else if desk.contains(id) {
                    state = "disabled";
                    why = b"off in the desk's shell.json".to_vec();
                } else if widget
                    && more
                    && cfg.as_ref().is_some_and(|c| {
                        Args::new(&["-e", "--arg", "i"])
                            .arg(id)
                            .arg(OWN_JQ)
                            .arg(shell::bytes(c))
                            .run_file(jq::Err::Null)
                            .ok()
                    })
                {
                    // A service (or panel...) with a widget, loaded from its own entry, its widget
                    // off the bar (#326): what the shell loads, not what the bar shows.
                    state = if words.iter().any(|k| k == b"service") {
                        "loaded"
                    } else {
                        "enabled"
                    };
                    why = first_line(&lines);
                } else if widget {
                    let placers = cfg
                        .as_ref()
                        .map(|c| {
                            Args::new(&["-r", "--arg", "i"])
                                .arg(id)
                                .kv("--argjson", "m", &m)
                                .kv("--argjson", "l", l)
                                .arg(PLACERS_JQ)
                                .arg(shell::bytes(c))
                                .run_file(jq::Err::Null)
                                .text()
                        })
                        .unwrap_or_default();
                    why = b"not in the bar's layout".to_vec();
                    if placers.is_empty() {
                        state = "disabled";
                    } else {
                        // omabox cannot see a widget another plugin places itself: said, not warned.
                        state = "off the bar";
                        say(cat!(
                            "plugin ",
                            id,
                            " is not in the bar's layout: if ",
                            placers,
                            " places it itself, ignore this (check with its IPC or a shot); to put it there: omabox -b ",
                            ctx.name,
                            " run -- omarchy-shell shell putBarWidget ",
                            id,
                            " '{}'"
                        ));
                    }
                } else {
                    state = "disabled";
                    why = b"listed but not enabled".to_vec();
                }
            } else if kinds_load(&kinds) {
                state = "loaded";
                why = first_line(&lines);
            } else {
                state = "enabled";
                why = first_line(&lines);
            }
            match state {
                "hosted" | "off the bar" => {}
                "disabled" => {
                    if !desk.contains(id) {
                        let put = if why == b"not in the bar's layout" {
                            cat!(
                                " (to put it there: omabox -b ",
                                ctx.name,
                                " run -- omarchy-shell shell putBarWidget ",
                                id,
                                " '{}')"
                            )
                        } else {
                            Vec::new()
                        };
                        say(cat!("warning: plugin ", id, " ", state, ": ", why, put));
                        bad = true;
                    }
                }
                "loaded" | "enabled" => {
                    if !why.is_empty() {
                        say(cat!(
                            "plugin ",
                            id,
                            " ",
                            state,
                            ", with a warning in the shell log: ",
                            why
                        ));
                    }
                }
                _ => {
                    say(cat!("warning: plugin ", id, " ", state, ": ", why));
                    bad = true;
                }
            }
        }
        let c = if dir.is_empty() {
            Vec::new()
        } else {
            git_rev(&dir)
        };
        let args: [&[u8]; 20] = [
            b"-c",
            b"--arg",
            b"i",
            id,
            b"--arg",
            b"s",
            state.as_bytes(),
            b"--arg",
            b"w",
            &why,
            b"--arg",
            b"v",
            &v,
            b"--arg",
            b"c",
            &c,
            b"--arg",
            b"h",
            &host,
            STATUS_JQ.as_bytes(),
        ];
        status = jq::jq(&args, Some(&status), jq::Err::Inherit).text();
    }
    if bad {
        say(cat!("the shell's log: omabox log -b ", ctx.name, " shell"));
    }
    let tmp = ctx.at("box.json.tmp");
    let j = Args::new(&["--argjson", "s"])
        .arg(&status)
        .arg(PLUGIN_STATUS_JQ)
        .arg(&bjs)
        .run_file(jq::Err::Inherit);
    if j.ok() && std::fs::write(&tmp, &j.out).is_ok() {
        let _ = std::fs::rename(&tmp, &bj);
    } else {
        // (bash left its box.json.tmp there: none left, finding 443's rule)
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(())
}

/// theme_named NAME TREE THEME_DIRS (finding 245): NAME as omarchy-theme-set names it (tags dropped,
/// ASCII lower case, spaces as dashes), when a theme of that name is there: in TREE's themes, the
/// user's, or one of THEME_DIRS (by its dir's name). Refused saying which there are, in byte order
/// (bash's sort: the locale's collation, finding 446). JAIL_PATH: jailpath.rs's jail_path, for a
/// jailed caller's --omarchy tree (tree_dir).
pub fn theme_named(
    ctx: &Ctx,
    name: &[u8],
    tree: &[u8],
    theme_dirs: &[Vec<u8>],
    jail_path: &dyn Fn(&[u8]) -> Option<Vec<u8>>,
) -> Res<Vec<u8>> {
    let n = theme_word(name);
    if !shell::re_match("^[a-z0-9][a-z0-9._-]{0,63}$", &n) {
        return Err(ctx.die(cat!("--theme: not a theme's name: ", name)));
    }
    if theme_dirs.iter().any(|d| shell::basename(d) == n) {
        return Ok(n);
    }
    let user = cat!(shell::env_or_empty("HOME"), "/.config/omarchy/themes");
    for d in [cat!(tree, "/themes/", n), cat!(user, "/", n)] {
        if tree_dir(&d, tree, jail_path) {
            return Ok(n);
        }
    }
    let mut names: Vec<Vec<u8>> = Vec::new();
    for base in [cat!(tree, "/themes"), user] {
        for e in shell::glob(&shell::path(&base), true) {
            if tree_dir(&cat!(base, "/", e, "/"), tree, jail_path) {
                names.push(e);
            }
        }
    }
    names.extend(theme_dirs.iter().map(|d| shell::basename(d)));
    names.retain(|n| !n.is_empty());
    names.sort();
    names.dedup();
    let names = names.join(&b' ');
    Err(ctx.die(cat!(
        "--theme: no theme ",
        n,
        " in ",
        tree,
        "/themes, ~/.config/omarchy/themes",
        if theme_dirs.iter().any(|d| !d.is_empty()) {
            " or --theme-dir"
        } else {
            ""
        },
        if names.is_empty() {
            Vec::new()
        } else {
            cat!(" (there are: ", names, ")")
        }
    )))
}

/// `sed -E 's/<[^>]+>//g' | tr '[:upper:]' '[:lower:]' | tr ' ' '-'`, its trailing newlines off:
/// omarchy-theme-set's name for a theme (tags within a line; tr lowers ASCII only).
fn theme_word(name: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for (k, l) in name.split(|&c| c == b'\n').enumerate() {
        if k > 0 {
            out.push(b'\n');
        }
        let mut i = 0;
        while i < l.len() {
            if l[i] == b'<'
                && let Some(j) = l[i + 1..].iter().position(|&c| c == b'>')
                && j > 0
            {
                i += j + 2;
                continue;
            }
            out.push(match l[i] {
                b' ' => b'-',
                c => c.to_ascii_lowercase(),
            });
            i += 1;
        }
    }
    strip_nl(&out)
}

/// tree_dir DIR TREE: DIR is a folder. One in a jailed caller's --omarchy TREE (not
/// /usr/share/omarchy) only where JAIL_PATH leads (#198): its themes/, or a theme in it, linked out
/// of the jail's folders is none, never followed.
pub fn tree_dir(d: &[u8], tree: &[u8], jail_path: &dyn Fn(&[u8]) -> Option<Vec<u8>>) -> bool {
    if in_jail() && tree != b"/usr/share/omarchy" && d.starts_with(&cat!(tree, "/")) {
        return jail_path(d).is_some_and(|q| shell::path(&q).is_dir());
    }
    shell::path(d).is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        assert!(id_ok(b"chaves.omabox"));
        assert!(id_ok(b"_x-1"));
        assert!(!id_ok(b".x"));
        assert!(!id_ok(b"-x"));
        assert!(!id_ok(b"a b"));
        assert!(!id_ok(b""));
        assert!(!id_ok("é".as_bytes()));
    }

    #[test]
    fn warnings_of_a_log() {
        let log =
            b"\x1b[33m2026-10-01 12:00:00.123 WARN qml: Plugin widget x.y failed: boom\x1b[0m\n\
            INFO scene: fine\n \
            WARN quickshell.qmlscanner: Module path /p is outside of the config folder.\n\
            ERROR plugins: PluginRegistry: x.z refused\n";
        assert_eq!(
            shell_warnings_of(log),
            b"Plugin widget x.y failed: boom\nPluginRegistry: x.z refused"
        );
        assert_eq!(shell_warnings_of(b""), b"");
    }

    #[test]
    fn lines_that_name_an_id() {
        let log = b"widget x.y failed\nwidget x.yz failed\nx.y\nax.y\n(x.y)";
        assert_eq!(lines_naming(log, b"x.y"), b"widget x.y failed\nx.y\n(x.y)");
        assert_eq!(lines_naming(b"", b"x.y"), b"");
        assert!(kinds_load(b"panel bar-widget"));
        assert!(!kinds_load(b"panel menu"));
    }

    #[test]
    fn theme_words() {
        assert_eq!(theme_word(b"Tokyo Night"), b"tokyo-night");
        assert_eq!(theme_word(b"<b>Rose</b> Pine"), b"rose-pine");
        assert_eq!(theme_word(b"a<>b"), b"a<>b");
        assert_eq!(theme_word("ÉA".as_bytes()), "Éa".as_bytes());
    }

    #[test]
    fn desk_entries_plain_then_hidden() {
        let d = std::env::temp_dir().join(format!("omabox-entries-{}", std::process::id()));
        for n in ["b", "A", ".h", "..x", "a"] {
            std::fs::create_dir_all(d.join(n)).unwrap();
        }
        let got = entries(shell::bytes(&d));
        std::fs::remove_dir_all(&d).unwrap();
        assert_eq!(
            got,
            vec![b"A".to_vec(), b"a".to_vec(), b"b".to_vec(), b".h".to_vec()]
        );
    }
}
