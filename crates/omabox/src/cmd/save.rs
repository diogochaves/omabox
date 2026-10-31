//! `omabox save` and `omabox saves`: a save is a box HOME kept on purpose, for `up --from` and `run
//! --from` to start with: what a box's apps set up once (signed in, a PIN set, a library scanned).
//! SAVES/NAME/home and its save.json. Private (the dir is 0700): a save keeps the box's keyring,
//! which holds what agents stored in it.

use crate::args::{optarg, unexpected, unknown_opt, unknown_opt_hint, usage};
use crate::boxes::{meta, need_box};
use crate::cat;
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::json;
use crate::select::select_box;
use crate::shell;
use serde_json::{Map, Value};
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

const SAVES_HINT: &str = "(omabox saves [--json] | saves rm SAVE...)";

/// Save names are paths: as tame as box names, but refused rather than changed to fit.
fn save_name(ctx: &Ctx, n: &[u8]) -> Res<()> {
    if shell::re_match("^[A-Za-z0-9][A-Za-z0-9_.-]{0,39}$", n) {
        return Ok(());
    }
    Err(ctx.die(cat!(
        "bad save name: ",
        n,
        " (letters, digits, . _ -; 40 at most)"
    )))
}

/// The save's dir, or a refusal that says why.
fn save_dir(ctx: &Ctx, n: &[u8]) -> Res<Vec<u8>> {
    save_name(ctx, n)?;
    let d = cat!(crate::paths::saves(), "/", n);
    if !shell::path(&cat!(d, "/home")).is_dir() {
        return Err(ctx.die(cat!("no save '", n, "' (omabox saves lists them)")));
    }
    Ok(d)
}

/// Whether a save's keyring holds a secret: gnome-keyring's unencrypted file has a [N] section per
/// item (`grep -qs '^\[[0-9]' D/home/.local/share/keyrings/*.keyring`). Only regular files are read:
/// a FIFO there hung bash's grep.
fn save_keyring(d: &[u8]) -> bool {
    let dir = cat!(d, "/home/.local/share/keyrings");
    shell::glob_ext(&dir, b".keyring").iter().any(|f| {
        let Ok(file) = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
            .open(shell::path(f))
        else {
            return false;
        };
        if !file.metadata().is_ok_and(|m| m.is_file()) {
            return false;
        }
        let mut text = Vec::new();
        let _ = std::io::Read::read_to_end(&mut &file, &mut text);
        text.split(|&c| c == b'\n')
            .any(|l| l.first() == Some(&b'[') && l.get(1).is_some_and(u8::is_ascii_digit))
    })
}

/// A save or what is left of one being written; a copy of a HOME may have dirs without u+w.
fn rm_save(d: &[u8]) -> bool {
    crate::ready::rm_tree(
        &cat!("save '", shell::basename(d), "'"),
        std::slice::from_ref(&d.to_vec()),
    )
}

/// `du -s[h|k] D | cut -f1`, and whether du (the pipeline, under pipefail) succeeded.
fn du(d: &[u8], flag: &str) -> (Vec<u8>, bool) {
    let mut c = crate::sig::command("du");
    c.arg(flag)
        .arg(OsStr::from_bytes(d))
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit());
    match crate::sig::run_output(c) {
        Ok((rc, o)) => {
            let first = o.split(|&c| c == b'\n').next().unwrap_or_default();
            let f = first.split(|&c| c == b'\t').next().unwrap_or_default();
            (f.to_vec(), rc == 0)
        }
        Err(_) => (Vec::new(), false),
    }
}

pub fn save(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut save, mut force) = (Vec::new(), Vec::new(), false);
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, args.get(i + 1))?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"--force" => force = true,
            // (#273, #287) -f is follow in log and events: save's was warned in 0.9, refused since 1.0.
            b"-f" => {
                return Err(ctx.usage_die(
                    "save -f is refused since 1.0: write save --force (-f is follow in log and events)",
                ));
            }
            b"-h" | b"--help" => return usage("save"),
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "save", a)),
            _ => {
                if !save.is_empty() {
                    return Err(ctx.die(cat!(
                        "save: one save name, got ",
                        save,
                        " and ",
                        a,
                        " (the box goes with -b)"
                    )));
                }
                save = a.clone();
            }
        }
        i += 1;
    }
    if save.is_empty() {
        return Err(ctx.die("save: name it (omabox save SAVE [-b NAME])"));
    }
    save_name(ctx, &save)?;
    select_box(ctx, &name)?;
    need_box(ctx)?;
    let saves = crate::paths::saves();
    let sp = shell::path(&saves);
    if let Err(e) = std::fs::create_dir_all(&sp)
        .and_then(|()| std::fs::set_permissions(&sp, std::fs::Permissions::from_mode(0o700)))
    {
        return Err(ctx.die(cat!("save: cannot make ", saves, ": ", e.to_string())));
    }
    // One save per name at a time (locked where the boxes' are); what a killed one left behind goes
    // now (its own leftovers only: bash's glob took another save's whose name starts with this one's).
    let Some(lock) = crate::boxes::lock_file(
        &ctx.in_boxes(cat!(".lock-save-", save)),
        std::time::Duration::from_secs(60),
    ) else {
        return Err(ctx.die(cat!(
            "save '",
            save,
            "' is being written by another omabox save; try again"
        )));
    };
    for n in leftovers(&sp, &save) {
        let pid = n.rsplit(|&c| c == b'-').next().unwrap_or_default();
        if !alive(pid) {
            rm_save(&cat!(saves, "/", n));
        }
    }
    let dest = cat!(saves, "/", save);
    let destp = shell::path(&dest);
    if (destp.exists() || destp.is_symlink()) && !force {
        return Err(ctx.die(cat!(
            "save '",
            save,
            "' exists (--force replaces it; omabox saves lists them)"
        )));
    }
    // The copy is made with the box paused (SIGSTOP to all of it), so it is what a power cut would
    // have left: an app's database is whole, as after a crash, not torn by a write during the copy.
    let me = std::process::id().to_string();
    let tmp = cat!(saves, "/.new-", save, "-", me);
    let ns = crate::jobs::box_ns(ctx).unwrap_or_default();
    let Some(pids) = crate::jobs::box_pins_try(ctx) else {
        return Err(ctx.die(cat!(
            "save: cannot read the pid namespace of box '",
            ctx.name,
            "'"
        )));
    };
    // A save SIGKILLed (or killed by the OOM killer) runs no trap: a watch of its own resumes the
    // box then (#369, finding 574). None, no pause.
    let Some(watch) = start_watch(&ns, &pids) else {
        return Err(ctx.die(cat!(
            "save: could not start the watch that resumes box '",
            ctx.name,
            "' should this save be killed"
        )));
    };
    // (bash's EXIT trap: the box let go and the copy removed however the save ends, a signal too)
    let held = Arc::new(Mutex::new(Hold {
        pids,
        watch: Some(watch),
    }));
    let (h2, t2) = (held.clone(), tmp.clone());
    let _undo = crate::trap::on_exit(move || {
        if let Ok(mut h) = h2.lock() {
            h.let_go();
        }
        rm_save(&t2);
    });
    let mk = std::fs::DirBuilder::new()
        .mode(0o700)
        .create(shell::path(&tmp));
    let mk = mk.and_then(|()| std::fs::create_dir(shell::path(&cat!(tmp, "/home"))));
    if let Err(e) = mk {
        return Err(ctx.die(cat!("save: cannot make ", tmp, ": ", e.to_string())));
    }
    if let Ok(h) = held.lock() {
        signal_all(&h.pids, libc::SIGSTOP);
    }
    // All of the HOME but .cache (rebuilt by the apps) and the logs at its top (the box's, run -d's).
    let h = cat!(crate::paths::homes(), "/", ctx.name, "/home");
    for e in home_entries(&h) {
        let mut c = crate::sig::command("cp");
        c.args(["-a", "--reflink=auto", "--"])
            .arg(OsStr::from_bytes(&cat!(h, "/", e)))
            .arg(OsStr::from_bytes(&cat!(tmp, "/home/")))
            .stdin(Stdio::inherit());
        if !crate::sig::run_status(c).is_ok_and(|rc| rc == 0) {
            return Err(ctx.die(cat!("save: could not copy ", e)));
        }
    }
    if let Ok(mut h) = held.lock() {
        h.let_go();
    }
    // The Hyprland and Quickshell builds it ran (findings 116, 358), so an `up --from` on another
    // one can say so.
    let sj = save_json(
        &ctx.name,
        &shell::date_iso(),
        &crate::cmd::version::version_text(),
        &meta(&ctx.d, "hyprland").0,
        &meta(&ctx.d, "quickshell").0,
    );
    if let Err(e) = std::fs::write(shell::path(&cat!(tmp, "/save.json")), sj) {
        return Err(ctx.die(cat!(
            "save: cannot write ",
            tmp,
            "/save.json: ",
            e.to_string()
        )));
    }
    let old = cat!(saves, "/.old-", save, "-", me);
    if destp.exists() || destp.is_symlink() {
        if let Err(e) = std::fs::rename(&destp, shell::path(&old)) {
            return Err(ctx.die(cat!("save: cannot move ", dest, " aside: ", e.to_string())));
        }
    }
    if let Err(e) = std::fs::rename(shell::path(&tmp), &destp) {
        return Err(ctx.die(cat!(
            "save: cannot move ",
            tmp,
            " to ",
            dest,
            ": ",
            e.to_string()
        )));
    }
    let oldp = shell::path(&old);
    if oldp.exists() || oldp.is_symlink() {
        rm_save(&old);
    }
    drop(lock);
    let size = du(&dest, "-sh").0;
    let keyring = if save_keyring(&dest) {
        ", keyring secrets included"
    } else {
        ""
    };
    say(cat!(
        "saved box '",
        ctx.name,
        "' as '",
        save,
        "' (",
        size,
        keyring,
        "); start from it: omabox up --from ",
        save,
        " (or run --from ",
        save,
        ")"
    ));
    let kb = shell::dec(&du(&dest, "-sk").0);
    if kb.is_none_or(|k| k >= 1_048_576) {
        say(cat!(
            "save '",
            save,
            "' is ",
            size,
            ": a build or a download in the box HOME? (omabox saves rm ",
            save,
            ")"
        ));
    }
    Ok(())
}

/// What a save holds while it copies: the box's processes it stops, and the watch that resumes
/// them should the save die without letting go (#369).
struct Hold {
    pids: Vec<crate::pidfd::Pidfd>,
    watch: Option<std::process::Child>,
}

impl Hold {
    /// The box resumed, then its watch told so and ended. In that order: a save killed between the
    /// two leaves the watch to resume a box already running, which changes nothing.
    fn let_go(&mut self) {
        signal_all(&self.pids, libc::SIGCONT);
        self.pids.clear();
        if let Some(mut w) = self.watch.take() {
            if let Some(mut i) = w.stdin.take() {
                let _ = i.write_all(WATCH_LET_GO);
            }
            let _ = w.wait();
        }
    }
}

/// What the save writes its watch once the box runs again.
const WATCH_LET_GO: &[u8] = b"x";
/// What the watch says once it holds the box's processes.
const WATCH_READY: &[u8] = b"ready\n";

/// `omabox _save-watch NS PID...` in a session of its own (a kill of the save's process group or
/// session does not reach it), its stdin a pipe only this save writes (close-on-exec: not cp's).
/// Started and ready (it holds a pidfd of each of PIDS still in the box's pid namespace NS) before
/// the box is stopped; None when not.
fn start_watch(ns: &[u8], pids: &[crate::pidfd::Pidfd]) -> Option<std::process::Child> {
    let mut c = crate::sig::command(crate::root().join("bin/omabox"));
    c.arg("_save-watch")
        .arg(OsStr::from_bytes(ns))
        .args(pids.iter().map(|p| OsStr::from_bytes(p.pid())))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe; run between fork and exec.
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(&mut c, || {
            libc::setsid();
            Ok(())
        });
    }
    let mut w = c.spawn().ok()?;
    let mut said = [0u8; WATCH_READY.len()];
    let ready = w
        .stdout
        .take()
        .is_some_and(|mut o| o.read_exact(&mut said).is_ok() && said == WATCH_READY);
    if !ready {
        let _ = w.kill();
        let _ = w.wait();
        return None;
    }
    Some(w)
}

/// `omabox _save-watch NS PID...` (#369, finding 574): says it is ready once it holds a pidfd of
/// each of PIDS (the box's processes a save is about to stop) that is still in the box's pid
/// namespace NS once its pidfd is open (#377: not a process that took the pid of one that ended
/// since the save listed it), then waits on its stdin. The save writes
/// it a byte once it resumed the box; its stdin closing with nothing written means the save died
/// without that (SIGKILL, the OOM killer), and the watch resumes them. Through the pidfds: never a
/// process that took one of their pids since.
pub fn watch_main(args: &[std::ffi::OsString]) -> std::process::ExitCode {
    enum Held {
        Fd(std::os::fd::OwnedFd),
        // (no pidfd for a reason other than its end: too many fds, say)
        Pid(i32),
    }
    let ns = args
        .first()
        .map(|a| a.as_bytes().to_vec())
        .unwrap_or_default();
    let held: Vec<Held> = args
        .iter()
        .skip(1)
        .filter_map(|a| shell::dec(a.as_bytes()).and_then(|n| i32::try_from(n).ok()))
        .filter(|&p| p > 0)
        .filter_map(|p| {
            let fd = crate::sig::pidfd_open(p);
            let gone =
                fd.is_none() && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
            // (read once the pidfd is open, as pidfd::Pidfd::pin does)
            let boxs = crate::procfs::pidns(p.to_string().as_bytes()).is_some_and(|n| n == ns);
            match fd {
                _ if gone || !boxs => None,
                Some(fd) => Some(Held::Fd(fd)),
                None => Some(Held::Pid(p)),
            }
        })
        .collect();
    let mut o = std::io::stdout();
    if o.write_all(WATCH_READY).and_then(|()| o.flush()).is_err() {
        return std::process::ExitCode::from(1);
    }
    let mut b = [0u8; 1];
    let mut stdin = std::io::stdin();
    let got = loop {
        match stdin.read(&mut b) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            r => break r.unwrap_or(0),
        }
    };
    if got == 0 {
        for h in &held {
            match h {
                // SAFETY: a signal through a pidfd this process holds.
                Held::Fd(fd) => unsafe {
                    libc::syscall(
                        libc::SYS_pidfd_send_signal,
                        std::os::fd::AsRawFd::as_raw_fd(fd),
                        libc::SIGCONT,
                        std::ptr::null::<libc::siginfo_t>(),
                        0,
                    );
                },
                // SAFETY: kill with a positive pid and a signal number.
                Held::Pid(p) => unsafe {
                    libc::kill(*p, libc::SIGCONT);
                },
            }
        }
    }
    std::process::ExitCode::SUCCESS
}

/// `kill -0 PID`: a process of that pid, whatever its state.
fn alive(pid: &[u8]) -> bool {
    shell::is_num(pid)
        && shell::dec(pid)
            .and_then(|n| i32::try_from(n).ok())
            .filter(|&n| n > 0)
            // SAFETY: kill with signal 0 only asks whether the pid exists.
            .is_some_and(|n| unsafe { libc::kill(n, 0) } == 0)
}

/// `kill -SIG PIDS` (each, the ones gone ignored), through the pidfds that pinned them as the box's
/// (#377): a box process that ended meanwhile, its pid taken by one of the user's, is not stopped
/// or continued.
fn signal_all(pids: &[crate::pidfd::Pidfd], sig: libc::c_int) {
    for p in pids {
        p.signal(sig);
    }
}

/// The names in SAVES a killed save of SAVE left: `.new-SAVE-PID`, `.old-SAVE-PID`.
fn leftovers(saves: &std::path::Path, save: &[u8]) -> Vec<Vec<u8>> {
    let Ok(rd) = std::fs::read_dir(saves) else {
        return Vec::new();
    };
    let mut v: Vec<Vec<u8>> = rd
        .flatten()
        .map(|e| e.file_name().as_bytes().to_vec())
        .filter(|n| {
            [b".new-".as_slice(), b".old-"].iter().any(|pre| {
                n.strip_prefix(*pre)
                    .and_then(|r| r.strip_prefix(save))
                    .and_then(|r| r.strip_prefix(b"-"))
                    .is_some_and(shell::is_num)
            })
        })
        .collect();
    v.sort();
    v
}

/// The names in the box HOME a save copies: all but `.cache` and the `*.log` files at its top.
fn home_entries(h: &[u8]) -> Vec<Vec<u8>> {
    let Ok(rd) = std::fs::read_dir(shell::path(h)) else {
        return Vec::new();
    };
    let mut v: Vec<Vec<u8>> = rd
        .flatten()
        .map(|e| e.file_name().as_bytes().to_vec())
        .filter(|n| n != b".cache" && !n.ends_with(b".log"))
        .collect();
    v.sort();
    v
}

/// save.json, as `jq -n` writes it.
fn save_json(name: &[u8], saved: &[u8], version: &[u8], hypr: &[u8], qs: &[u8]) -> String {
    let s = |v: &[u8]| Value::String(String::from_utf8_lossy(v).into_owned());
    let mut m = Map::new();
    m.insert("box".into(), s(name));
    m.insert("saved".into(), s(saved));
    m.insert("version".into(), s(version));
    if !hypr.is_empty() {
        m.insert("hyprland".into(), s(hypr));
    }
    if !qs.is_empty() {
        m.insert("quickshell".into(), s(qs));
    }
    json::pretty(&Value::Object(m)) + "\n"
}

/// Arguments it does not take are refused, as ls's are (finding 327).
pub fn saves(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let rm = args.first().is_some_and(|a| a == b"rm");
    let mut as_json = false;
    if !rm {
        for a in args {
            match a.as_slice() {
                b"--json" => as_json = true,
                b"-h" | b"--help" => return usage("saves"),
                b"-b" | b"--box" => {
                    say("saves takes no -b: it is not about one box");
                    return Err(Exit(2));
                }
                _ if a.starts_with(b"-") => {
                    return Err(unknown_opt_hint(ctx, "saves", a, SAVES_HINT));
                }
                _ => return Err(unexpected(ctx, "saves", a, SAVES_HINT)),
            }
        }
        ctx.parsed();
    }
    if rm {
        let names = &args[1..];
        if names.is_empty() {
            return Err(ctx.die("saves rm: which? (omabox saves lists them)"));
        }
        for n in names {
            save_name(ctx, n)?;
        }
        ctx.parsed();
        for n in names {
            let d = save_dir(ctx, n)?;
            if !rm_save(&d) {
                return Err(Exit(1));
            }
            say(cat!("save '", n, "' removed"));
        }
        return Ok(());
    }
    let saves = crate::paths::saves();
    // In byte order, the same in every locale (finding 446; bash's glob: the locale's collation).
    let list: Vec<Vec<u8>> = shell::glob(&shell::path(&saves), true)
        .into_iter()
        .filter(|n| shell::path(&cat!(saves, "/", n, "/home")).is_dir())
        .collect();
    if as_json {
        let mut all = Vec::new();
        for n in &list {
            let d = cat!(saves, "/", n);
            let kb = match du(&d, "-sk") {
                (k, true) if shell::is_num(&k) => k,
                _ => b"0".to_vec(),
            };
            let size = du(&d, "-sh").0;
            all.extend(save_rows(
                n,
                &std::fs::read(shell::path(&cat!(d, "/save.json"))).ok(),
                &size,
                &kb,
                save_keyring(&d),
            ));
        }
        return out(cat!(json::compact(&Value::Array(all)), "\n"));
    }
    let row = |a: &[u8], b: &[u8], c: &[u8], d: &[u8], e: &[u8]| {
        cat!(
            pad(a, 24),
            " ",
            pad(b, 6),
            " ",
            pad(c, 17),
            " ",
            pad(d, 24),
            " ",
            e,
            "\n"
        )
    };
    out(row(b"NAME", b"SIZE", b"SAVED", b"FROM", b"KEYRING"))?;
    for n in &list {
        let d = cat!(saves, "/", n);
        let k: &[u8] = if save_keyring(&d) { b"yes" } else { b"no" };
        let size = du(&d, "-sh").0;
        let sj = std::fs::read(shell::path(&cat!(d, "/save.json"))).ok();
        let (saved, from) = save_cols(sj.as_deref());
        out(row(n, &size, &saved, &from, k))?;
    }
    Ok(())
}

/// printf's `%-Ns`: V, then spaces to N bytes.
pub fn pad(v: &[u8], n: usize) -> Vec<u8> {
    let mut o = v.to_vec();
    o.resize(n.max(v.len()), b' ');
    o
}

/// One save's rows of `saves --json`: `jq -c '{name: $name, box, saved, size: $size, size_kb: $kb,
/// keyring: $keyring}' save.json 2>/dev/null || jq -nc '{name: $name}'`, each value of save.json a
/// row until one jq cannot take; then (or with no save.json) the bare one.
fn save_rows(
    name: &[u8],
    text: &Option<Vec<u8>>,
    size: &[u8],
    kb: &[u8],
    keyring: bool,
) -> Vec<Value> {
    let s = |v: &[u8]| Value::String(String::from_utf8_lossy(v).into_owned());
    let bare = || {
        let mut m = Map::new();
        m.insert("name".into(), s(name));
        Value::Object(m)
    };
    let Some(text) = text else {
        return vec![bare()];
    };
    let (vals, mut ok) = json::values(text);
    let mut rows = Vec::new();
    for v in vals {
        let get = |k: &str| match &v {
            Value::Object(m) => m.get(k).cloned().unwrap_or(Value::Null),
            _ => Value::Null,
        };
        if !matches!(v, Value::Object(_) | Value::Null) {
            ok = false;
            break;
        }
        let mut m = Map::new();
        m.insert("name".into(), s(name));
        m.insert("box".into(), get("box"));
        m.insert("saved".into(), get("saved"));
        m.insert("size".into(), s(size));
        m.insert(
            "size_kb".into(),
            serde_json::from_slice(kb).unwrap_or(Value::from(0)),
        );
        m.insert("keyring".into(), Value::Bool(keyring));
        rows.push(Value::Object(m));
    }
    if !ok {
        rows.push(bare());
    }
    rows
}

/// The table's SAVED and FROM: `jq -r '.saved // "?" | sub("T"; " ") | .[0:16]'` and `jq -r '.box
/// // "?"'` on save.json, as `$(...)` (nothing when jq fails at once; what it printed before).
fn save_cols(text: Option<&[u8]>) -> (Vec<u8>, Vec<u8>) {
    let Some(text) = text else {
        return (Vec::new(), Vec::new());
    };
    let (vals, _) = json::values(text);
    let (mut saved, mut from) = (Vec::new(), Vec::new());
    let field = |v: &Value, k: &str| -> Option<Value> {
        match v {
            Value::Object(m) => Some(match m.get(k) {
                None | Some(Value::Null | Value::Bool(false)) => Value::String("?".into()),
                Some(x) => x.clone(),
            }),
            Value::Null => Some(Value::String("?".into())),
            _ => None,
        }
    };
    for v in &vals {
        let Some(Value::String(t)) = field(v, "saved") else {
            break;
        };
        let t = t.replacen('T', " ", 1);
        saved.extend(t.chars().take(16).collect::<String>().bytes());
        saved.push(b'\n');
    }
    for v in &vals {
        let Some(b) = field(v, "box") else { break };
        from.extend(json::raw(&b).bytes());
        from.push(b'\n');
    }
    (shell::strip_nl(&saved), shell::strip_nl(&from))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_as_jq_gives_them() {
        let t = Some(br#"{"box":"b","saved":"2026-10-24T10:00:00+00:00","version":"1"}"#.to_vec());
        let r = save_rows(b"s", &t, b"12K", b"12", true);
        assert_eq!(
            json::compact(&Value::Array(r)),
            r#"[{"name":"s","box":"b","saved":"2026-10-24T10:00:00+00:00","size":"12K","size_kb":12,"keyring":true}]"#
        );
        assert_eq!(
            json::compact(&Value::Array(save_rows(b"s", &None, b"", b"0", false))),
            r#"[{"name":"s"}]"#
        );
        // (a value jq cannot index: the rows before it, then the bare one)
        let t = Some(br#"{"box":"b"} 1"#.to_vec());
        let r = json::compact(&Value::Array(save_rows(b"s", &t, b"4K", b"4", false)));
        assert_eq!(
            r,
            r#"[{"name":"s","box":"b","saved":null,"size":"4K","size_kb":4,"keyring":false},{"name":"s"}]"#
        );
    }

    #[test]
    fn table_columns() {
        let t = br#"{"box":"b1","saved":"2026-10-24T10:11:12+01:00"}"#;
        assert_eq!(
            save_cols(Some(t)),
            (b"2026-10-24 10:11".to_vec(), b"b1".to_vec())
        );
        assert_eq!(save_cols(Some(b"{}")), (b"?".to_vec(), b"?".to_vec()));
        assert_eq!(save_cols(Some(b"[1]")), (Vec::new(), Vec::new()));
        assert_eq!(
            save_cols(Some(br#"{"saved":5,"box":7}"#)),
            (Vec::new(), b"7".to_vec())
        );
        assert_eq!(pad(b"ab", 4), b"ab  ");
        assert_eq!(pad(b"abcdef", 4), b"abcdef");
    }

    #[test]
    fn save_json_pretty() {
        assert_eq!(
            save_json(b"b", b"T", b"1.0", b"", b"/q"),
            "{\n  \"box\": \"b\",\n  \"saved\": \"T\",\n  \"version\": \"1.0\",\n  \"quickshell\": \"/q\"\n}\n"
        );
    }

    #[test]
    fn leftovers_by_name() {
        let d = std::env::temp_dir().join(format!("omabox-save-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for n in [
            ".new-x-12",
            ".old-x-3",
            ".new-x-y-4",
            ".new-x-",
            "x",
            ".new-xy-5",
        ] {
            std::fs::create_dir_all(d.join(n)).unwrap();
        }
        assert_eq!(
            leftovers(&d, b"x"),
            vec![b".new-x-12".to_vec(), b".old-x-3".to_vec()]
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
