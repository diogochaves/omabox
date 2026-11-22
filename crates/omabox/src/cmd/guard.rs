//! `omabox guard [on|off [claude|codex]]` and `omabox guard exec -- AGENT [ARG...]`: the agent guard
//! (findings 65-67, guard.rs) shown, added to or taken out of the agents' own settings, or put around
//! any other agent whole.

use crate::args::unknown_word;
use crate::cat;
use crate::exit::{Ctx, Res, out, say};
use crate::guard::{CODEX_PY, COMMANDS_JQ, GUARD, GUARD_JQ, Guard, OFF_JQ, ON_JQ, STATE_JQ};
use crate::jq::{self, Args};
use crate::shell;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::process::Stdio;

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let act = args.first().cloned().unwrap_or_default();
    if act == b"exec" {
        return exec(ctx, &args[1..]);
    }
    let which = args
        .get(1)
        .filter(|w| !w.is_empty())
        .cloned()
        .unwrap_or(b"all".to_vec());
    if !matches!(act.as_slice(), b"" | b"on" | b"off") {
        return Err(unknown_word(
            ctx,
            "guard",
            "subcommand",
            &act,
            "(on or off [claude|codex], exec -- CMD, or nothing to show it)",
            &["on", "off", "exec"],
        ));
    }
    if !matches!(which.as_slice(), b"all" | b"claude" | b"codex") {
        return Err(unknown_word(
            ctx,
            "guard",
            "agent",
            &which,
            "(claude or codex)",
            &["claude", "codex"],
        ));
    }
    if args.len() > 2 {
        return Err(ctx.usage_die(cat!(
            "guard: too many arguments, got ",
            args.join(&b' '),
            ": omabox guard [on|off] [claude|codex]"
        )));
    }
    ctx.parsed();
    let g = Guard::new(shell::bytes(&crate::root()));
    // With no agent named: every one that is installed (its config dir exists); Claude Code always.
    let mut agents: Vec<&str> = Vec::new();
    if which != b"codex" {
        agents.push("claude");
    }
    if which == b"codex" || (which == b"all" && shell::path(&codex_home()).is_dir()) {
        agents.push("codex");
    }
    if act.is_empty() {
        // A config it cannot read is reported for that agent; the others are still shown.
        let mut all_on = true;
        for a in &agents {
            let s = match state(&g, a) {
                Ok(s) => s,
                Err(m) => cat!(a, ": ", m),
            };
            all_on &= s.split(|&c| c == b'\n').all(|l| l.ends_with(b": on"));
            out(cat!(s, "\n"))?;
        }
        if shell::env_or_empty("WAYLAND_DISPLAY") == GUARD.as_bytes() {
            out("this shell: guarded\n")?;
        } else {
            out("this shell: reaches the desktop\n")?;
        }
        if !all_on {
            out(cat!(
                "\n`omabox guard on` gives agents' shell commands:\n  ",
                g.env_words(),
                ", and (Claude Code) ",
                g.path,
                " first on PATH and a core limit of 1 byte\nand tells Claude Code's agents: ",
                Guard::note(),
                "\n"
            ))?;
        }
        return Ok(());
    }
    // The guard's paths go into agents' settings files unquoted: only a plain path for the checkout.
    if act != b"off"
        && !g
            .root
            .iter()
            .all(|&c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'/' | b'+' | b'-'))
    {
        return Err(ctx.die(cat!(
            "the guard needs omabox at a path of letters, digits and ._/+- only, not ",
            g.root
        )));
    }
    let act = String::from_utf8_lossy(&act).into_owned();
    for a in &agents {
        let r = match *a {
            "claude" => claude_set(&g, &act),
            _ => codex_set(&g, &act),
        };
        r.map_err(|m| ctx.die(m))?;
    }
    Ok(())
}

fn claude_dir() -> Vec<u8> {
    shell::env("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| cat!(shell::env_or_empty("HOME"), "/.claude"))
}

fn codex_home() -> Vec<u8> {
    shell::env("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| cat!(shell::env_or_empty("HOME"), "/.codex"))
}

/// A settings file through a link to its target (one kept in a dotfiles repo stays a link). A link
/// that cannot be followed (into a folder not there, a loop) is refused (finding 482): written to
/// in its place, it would be replaced by a plain file, the user's link lost.
fn resolved(file: Vec<u8>) -> Result<Vec<u8>, Vec<u8>> {
    crate::conf::link_target(&file)
}

/// An agent's state line, or why it cannot be read (the refusal's words).
fn state(g: &Guard, agent: &str) -> Result<Vec<u8>, Vec<u8>> {
    if agent == "claude" {
        claude_state(g)
    } else {
        codex_state(g)
    }
}

fn claude_file() -> Result<Vec<u8>, Vec<u8>> {
    resolved(cat!(claude_dir(), "/settings.json"))
}

fn claude_state(g: &Guard) -> Result<Vec<u8>, Vec<u8>> {
    let file = claude_file()?;
    let h = hooks_state(&file, &g.hook())?;
    let mut line = cat!("Claude Code (", file, "): ", h);
    // A hook written from a checkout that is gone (a removed worktree) no longer stops xdg-open.
    if line.ends_with(b": outdated") {
        let cmds = Args::new(&["-r"])
            .kv("--arg", "g", GUARD)
            .arg(cat!(GUARD_JQ, COMMANDS_JQ))
            .arg(&file)
            .run_file(jq::Err::Inherit);
        for dir in guard_dirs(&cmds.out) {
            if !crate::exec::executable(&cat!(dir, "/xdg-open")) {
                line.extend_from_slice(&cat!(
                    ": its ",
                    dir,
                    " is gone, so links open on the desktop again"
                ));
            }
        }
    }
    Ok(line)
}

/// `grep -o 'PATH="[^"]*/share/guard:' | sed 's/^PATH="//; s/:$//'`: the guard dirs our hooks put
/// first on PATH, each match of each line.
fn guard_dirs(text: &[u8]) -> Vec<Vec<u8>> {
    let mut dirs = Vec::new();
    let tail = b"/share/guard:";
    for line in text.split(|&c| c == b'\n') {
        let mut i = 0;
        while let Some(p) = find(&line[i..], b"PATH=\"") {
            let start = i + p + 6;
            let seg_end = line[start..]
                .iter()
                .position(|&c| c == b'"')
                .map_or(line.len(), |q| start + q);
            let seg = &line[start..seg_end];
            // (the longest run before the quote that ends in /share/guard:)
            match seg.windows(tail.len()).rposition(|w| w == tail) {
                Some(t) => {
                    dirs.push(seg[..t + tail.len() - 1].to_vec());
                    i = start + t + tail.len();
                }
                None => i = start,
            }
        }
    }
    dirs
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The JSON hooks file's text as guard_hooks reads it (`cur=$(cat FILE)`): '{}' when there is none;
/// refused when it cannot be read or is not a JSON object.
fn hooks_read(file: &[u8]) -> Result<Vec<u8>, Vec<u8>> {
    let file = resolved(file.to_vec())?;
    let p = shell::path(&file);
    if !p.exists() {
        return Ok(b"{}".to_vec());
    }
    let cur = std::fs::read(&p)
        .map(|v| shell::subst(&v))
        .map_err(|_| cat!("cannot read ", file))?;
    if !jq::jq(&["-e", r#"type == "object""#], Some(&cur), jq::Err::Null).ok() {
        return Err(cat!(file, " is not a JSON object: not touched"));
    }
    Ok(cur)
}

/// guard_hooks FILE HOOK state: off, on or outdated.
fn hooks_state(file: &[u8], hook: &[u8]) -> Result<Vec<u8>, Vec<u8>> {
    let cur = hooks_read(file)?;
    let r = Args::new(&["-r"])
        .kv("--arg", "g", GUARD)
        .kv("--arg", "c", hook)
        .arg(cat!(GUARD_JQ, STATE_JQ))
        .run(Some(&cur), jq::Err::Inherit);
    Ok(r.text())
}

/// guard_hooks FILE HOOK on|off: our SessionStart hook added (in place of an older one) or taken
/// out, the rest of the file as it was.
fn hooks_set(file: &[u8], hook: &[u8], act: &str) -> Result<(), Vec<u8>> {
    let cur = hooks_read(file)?;
    let file = resolved(file.to_vec())?;
    let a = Args::new(&[]).kv("--arg", "g", GUARD);
    let a = if act == "on" {
        a.kv("--arg", "c", hook).arg(cat!(GUARD_JQ, ON_JQ))
    } else {
        a.arg(cat!(GUARD_JQ, OFF_JQ))
    };
    let r = a.run(Some(&cur), jq::Err::Inherit);
    if !r.ok() {
        return Err(cat!("jq could not change ", file));
    }
    let want = r.text();
    let sorted = |v: &[u8]| jq::jq(&["-S", "."], Some(v), jq::Err::Inherit).text();
    if sorted(&want) == sorted(&cur) {
        already(&file, act);
        return Ok(());
    }
    write(&file, &cat!(want, "\n"), act)
}

/// guard_write FILE CONTENT: written the way both agents need: through a link to its target, a
/// .bak-<ts> of the old one, its mode kept, renamed into place. Only that backup is kept: the ones
/// earlier changes left go (#398).
fn write(file: &[u8], content: &[u8], act: &str) -> Result<(), Vec<u8>> {
    let p = shell::path(file);
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let (mut tmp, tmp_path) = shell::mktemp_in(&cat!(file, ".omabox.XXXXXX"))
        .map_err(|_| cat!("could not write ", file))?;
    let _rm = shell::Rm(tmp_path.clone());
    // (and on INT, TERM or HUP: bash left it there)
    let t = tmp_path.clone();
    let _trap = crate::trap::on_exit(move || {
        let _ = std::fs::remove_file(shell::path(&t));
    });
    let mut bak = Vec::new();
    if p.exists() {
        // (two changes in one second each keep their own)
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        bak = cat!(
            file,
            ".bak-",
            format!("{}.{:09}", ns.as_secs(), ns.subsec_nanos())
        );
        copy_p(&p, &shell::path(&bak)).map_err(|_| cat!("could not back up ", file))?;
        if let Ok(m) = std::fs::metadata(&p) {
            let _ = std::fs::set_permissions(shell::path(&tmp_path), m.permissions());
        }
    }
    let ok = std::io::Write::write_all(&mut tmp, content).is_ok()
        && std::fs::rename(shell::path(&tmp_path), &p).is_ok();
    if !ok {
        return Err(cat!("could not write ", file));
    }
    let mut was = Vec::new();
    if !bak.is_empty() {
        was = cat!(" (was: ", bak, pruned(file, &shell::basename(&bak)), ")");
    }
    say(cat!(
        "guard ",
        act,
        " in ",
        file,
        was,
        "; new sessions pick it up"
    ));
    Ok(())
}

/// FILE unchanged: still, only its newest backup is kept, so an upgrade (whose guard step finds the
/// guard on) clears the ones older versions left (#398).
fn already(file: &[u8], act: &str) {
    let keep = backups(file).last().map(|b| shell::basename(b));
    let n = keep.map(|k| pruned(file, &k)).unwrap_or_default();
    let n = n
        .strip_prefix(b"; ")
        .map(|v| cat!(" (", v, ")"))
        .unwrap_or_default();
    say(cat!("guard already ", act, " in ", file, n));
}

/// Removes FILE's backups but KEEP (a name: FILE may be spelled with a doubled slash that the
/// listed paths lose); "; N older backup(s) of it removed", or nothing when none went.
fn pruned(file: &[u8], keep: &[u8]) -> Vec<u8> {
    let older: Vec<_> = backups(file)
        .into_iter()
        .filter(|b| shell::basename(b) != keep)
        .collect();
    for b in &older {
        let _ = std::fs::remove_file(shell::path(b));
    }
    if older.is_empty() {
        Vec::new()
    } else {
        format!("; {} older backup(s) of it removed", older.len()).into_bytes()
    }
}

/// The backups `write` made of FILE (FILE.bak-SECONDS.NANOSECONDS), oldest first. A user's own
/// (`settings.json.bak`, `settings.json.bak-mine`) do not match.
fn backups(file: &[u8]) -> Vec<Vec<u8>> {
    let p = shell::path(file);
    let (Some(dir), Some(name)) = (p.parent(), p.file_name()) else {
        return Vec::new();
    };
    let pre = cat!(name.as_bytes(), ".bak-");
    let mut found: Vec<((u64, u32), Vec<u8>)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name();
            let (s, ns) = std::str::from_utf8(n.as_bytes().strip_prefix(pre.as_slice())?)
                .ok()?
                .split_once('.')?;
            let digits = |v: &str| !v.is_empty() && v.bytes().all(|c| c.is_ascii_digit());
            if !digits(s) || !digits(ns) || ns.len() != 9 {
                return None;
            }
            let at = (s.parse().ok()?, ns.parse().ok()?);
            Some((at, shell::bytes(&e.path()).to_vec()))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, f)| f).collect()
}

/// The backups `guard on|off` left of the agents' settings, for `setup --remove` to name.
pub fn settings_backups() -> Vec<Vec<u8>> {
    let h = codex_home();
    [
        cat!(claude_dir(), "/settings.json"),
        cat!(h, "/config.toml"),
        cat!(h, "/hooks.json"),
    ]
    .into_iter()
    .flat_map(|f| backups(&resolved(f.clone()).unwrap_or(f)))
    .collect()
}

/// `cp -p`: the contents, mode and times.
fn copy_p(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::copy(from, to)?;
    let m = std::fs::metadata(from)?;
    let times = std::fs::FileTimes::new()
        .set_accessed(m.accessed()?)
        .set_modified(m.modified()?);
    std::fs::File::options()
        .write(true)
        .open(to)?
        .set_times(times)
}

fn claude_set(g: &Guard, act: &str) -> Result<(), Vec<u8>> {
    hooks_set(&claude_file()?, &g.hook(), act)
}

fn codex_paths() -> Result<(Vec<u8>, Vec<u8>), Vec<u8>> {
    let h = codex_home();
    Ok((resolved(cat!(h, "/config.toml"))?, cat!(h, "/hooks.json")))
}

/// guard_codex's python (CODEX_PY) on MODE: its answer (STATE ..., SAME, WANT\n...), or its refusal.
fn codex_py(g: &Guard, mode: &str, file: &[u8]) -> Result<Vec<u8>, Vec<u8>> {
    // The system's python (#152), not PATH's.
    if !crate::exec::executable(b"/usr/bin/python3") {
        return Err(b"guard: Codex's config is checked with /usr/bin/python3 (package python), which is not installed".to_vec());
    }
    let mut c = crate::sig::command("/usr/bin/python3");
    c.arg("-")
        .arg(mode)
        .arg(OsStr::from_bytes(file))
        .arg(OsStr::from_bytes(&g.codex_block()))
        .stdin(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = c
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|_| b"guard: could not run /usr/bin/python3".to_vec())?;
    if let Some(mut i) = child.stdin.take() {
        let _ = std::io::Write::write_all(&mut i, CODEX_PY.as_bytes());
    }
    let o = child
        .wait_with_output()
        .map_err(|_| b"guard: could not run /usr/bin/python3".to_vec())?;
    let text = shell::subst(&o.stdout);
    if !o.status.success() {
        return Err(text.strip_prefix(b"ERR ").unwrap_or(&text).to_vec());
    }
    Ok(text)
}

fn codex_state(g: &Guard) -> Result<Vec<u8>, Vec<u8>> {
    let (file, hooks) = codex_paths()?;
    let o = codex_py(g, "state", &file)?;
    let Some(line) = o.strip_prefix(b"STATE ") else {
        return Ok(Vec::new());
    };
    let h = hooks_state(&hooks, &g.codex_hook())?;
    // The block and the hook: either one alone, or an old one, is an outdated guard.
    let last = line
        .windows(2)
        .rposition(|w| w == b": ")
        .map_or(line, |i| &line[i + 2..]);
    let head = line
        .windows(2)
        .rposition(|w| w == b": ")
        .map_or(line, |i| &line[..i]);
    Ok(match (last, h.as_slice()) {
        (b"on", b"on") | (b"off", b"off") => line.to_vec(),
        (b"on" | b"off", _) => cat!(head, ": outdated (its hook in ", hooks, ": ", h, ")"),
        _ => line.to_vec(),
    })
}

fn codex_set(g: &Guard, act: &str) -> Result<(), Vec<u8>> {
    let (file, hooks) = codex_paths()?;
    if act == "on" {
        hooks_read(&hooks)?;
    }
    let o = codex_py(g, act, &file)?;
    if o == b"SAME" {
        already(&file, act);
    } else if let Some(w) = o.strip_prefix(b"WANT\n") {
        write(&file, w.strip_suffix(b".").unwrap_or(w), act)?;
    }
    let h = hooks_state(&hooks, &g.codex_hook())?;
    hooks_set(&hooks, &g.codex_hook(), act)?;
    if act != "on" {
        return Ok(());
    }
    // Finding 750: the guard's file, which keeps Omarchy 4.5's omarchy-shell off the user's shell
    // (403), is made for Codex only by its hook, and Codex runs that once it is trusted. Made here for
    // this login; a reboot empties the runtime dir.
    let mark = crate::front::make_guard_mark();
    // Codex runs a new or changed hook only once you trust it; that is yours to do, not ours.
    if h != b"on" {
        say(cat!(
            "Codex asks you to trust its new hook once: /hooks in Codex. Until then its agents get the guard's variables but not its note, and ",
            if mark {
                "the guard's file in your runtime dir (made now) is not made again after a reboot until an omabox command runs"
            } else {
                "nothing makes the guard's file in your runtime dir (it could not be made now) until an omabox command runs"
            },
            ": without it Omarchy's omarchy-shell reaches your desktop's shell"
        ));
    } else if !mark {
        say(
            "could not make the guard's file in your runtime dir ($XDG_RUNTIME_DIR/omabox-guard): until Codex's hook or an omabox command makes it, Omarchy's omarchy-shell reaches your desktop's shell",
        );
    }
    say(
        "Codex gets the guard's variables only, none of its commands first on PATH: `omarchy restart shell` still stops your bar and `hyprctl -i` reaches your Hyprland from its shell. For the whole guard, start it as: omabox guard exec -- codex",
    );
    Ok(())
}

/// `guard exec -- AGENT ...`: any other agent, whole, under the guard: everything it runs inherits
/// it (its own process loses the display too), and a session of its own (finding 88) unless
/// OMABOX_SESSION says otherwise.
fn exec(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    use std::os::unix::process::CommandExt;
    let argv = match args.first() {
        Some(a) if a == b"--" => &args[1..],
        _ => args,
    };
    let Some(prog) = argv.first() else {
        return Err(ctx.die("guard exec: nothing to run (omabox guard exec -- AGENT ...)"));
    };
    ctx.parsed();
    let g = Guard::new(shell::bytes(&crate::root()));
    let mut c = crate::sig::command(OsStr::from_bytes(prog));
    c.args(argv[1..].iter().map(|a| OsStr::from_bytes(a)));
    for (k, v) in g.env() {
        c.env(OsStr::from_bytes(&k), OsStr::from_bytes(&v));
    }
    c.env(
        "PATH",
        OsStr::from_bytes(&cat!(g.path, ":", shell::env_or_empty("PATH"))),
    );
    // A core limit of 1 byte (prlimit --pid $$ --core=1:): a Qt abort with no display reaches no
    // crash notification (see nocore).
    let mut lim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: lim is ours, a valid rlimit for both calls.
    unsafe {
        if libc::getrlimit(libc::RLIMIT_CORE, &mut lim) == 0 && lim.rlim_max >= 1 {
            lim.rlim_cur = 1;
            libc::setrlimit(libc::RLIMIT_CORE, &lim);
        }
    }
    // GUARD_MARK (#290).
    let rt = shell::env_or_empty("XDG_RUNTIME_DIR");
    if !rt.is_empty() {
        let _ = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(shell::path(&cat!(rt, "/", GUARD)));
    }
    // The pid the agent runs as (exec keeps it), so its box can go with it (finding 93). A session it
    // keeps keeps its agent too, while this runs under that one: a guard exec inside a guarded agent
    // shares the outer one's box, which must not go when the inner one exits.
    let me = std::process::id().to_string();
    if std::env::var_os("OMABOX_SESSION").is_none() {
        c.env("OMABOX_SESSION", format!("g{}", session_id()));
        c.env("OMABOX_AGENT_PID", &me);
    } else {
        let ap = shell::env_or_empty("OMABOX_AGENT_PID");
        let keep = !ap.is_empty()
            && ap.iter().all(u8::is_ascii_digit)
            && crate::select::descends_from(&ap);
        c.env(
            "OMABOX_AGENT_PID",
            if keep {
                OsStr::from_bytes(&ap)
            } else {
                OsStr::new(&me)
            },
        );
    }
    let err = c.exec();
    let (why, rc) = crate::exec::exec_failed(prog, &err);
    say(cat!("guard exec: cannot run ", prog, ": ", why));
    Err(crate::exit::Exit(rc))
}

/// `od -An -N6 -tx1 /dev/urandom | tr -d ' \n'`: 12 hex digits.
fn session_id() -> String {
    let mut b = [0u8; 6];
    // SAFETY: b is ours, its length given.
    let got = unsafe { libc::getrandom(b.as_mut_ptr().cast(), b.len(), 0) };
    if got != 6 {
        let t = crate::settle::mono_ns().to_le_bytes();
        b.copy_from_slice(&t[..6]);
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backups_are_ours_only_oldest_first() {
        let d = std::env::temp_dir().join(format!("omabox-guard-bak-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for n in [
            "s.json",
            "s.json.bak",
            "s.json.bak-mine",
            "s.json.bak-20261004",
            "s.json.bak-12.12345678",
            "s.json.bak-9.000000002",
            "s.json.bak-10.000000001",
            "s.json.bak-9.000000001",
            "t.json.bak-1.000000000",
        ] {
            std::fs::write(d.join(n), "").unwrap();
        }
        let f = shell::bytes(&d.join("s.json")).to_vec();
        let names: Vec<_> = backups(&f).iter().map(|b| shell::basename(b)).collect();
        assert_eq!(
            names,
            [
                b"s.json.bak-9.000000001".to_vec(),
                b"s.json.bak-9.000000002".to_vec(),
                b"s.json.bak-10.000000001".to_vec()
            ]
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn write_keeps_its_backup_through_a_doubled_slash() {
        // CLAUDE_CONFIG_DIR=~/.claude/ makes FILE ~/.claude//settings.json: the backup just made
        // must not read as an older one, or every backup goes
        let d = std::env::temp_dir().join(format!("omabox-guard-slash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("s.json"), "{}").unwrap();
        std::fs::write(d.join("s.json.bak-9.000000001"), "").unwrap();
        let f = cat!(shell::bytes(&d), "//s.json");
        write(&f, b"{\"a\":1}\n", "on").unwrap();
        let left = backups(&f);
        assert_eq!(left.len(), 1);
        assert_eq!(std::fs::read(shell::path(&left[0])).unwrap(), b"{}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn dirs_from_hooks() {
        assert_eq!(
            guard_dirs(b"x PATH=\"/a/share/guard:$PATH\" ;; esac PATH=\"/b/share/guard:/c/share/guard:x\"\nPATH=\"/nope\""),
            vec![
                b"/a/share/guard".to_vec(),
                b"/b/share/guard:/c/share/guard".to_vec()
            ]
        );
    }
}
