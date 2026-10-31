//! The caller's repo and paths, as `run` looks at them before its command: which repo it is in
//! (repo_top), what a box may not mount (refuse_src, finding 63), and the notes `run` says about a
//! word of its command the box will not have: one reached through a link out of the repo (#235), one
//! relative to a dir the box runs it outside of (#251).

use crate::boxes::{BoxPid, box_pid};
use crate::cat;
use crate::exit::{Ctx, say};
use crate::jail::in_jail;
use crate::json;
use crate::shell::{self, strip_nl};
use serde_json::Value;
use std::collections::HashSet;
use std::process::Stdio;

/// `realpath -ms -- P`: absolute (from the physical cwd), `.` and `..` and repeated slashes taken out
/// as text, no link followed.
pub fn realpath_ms(p: &[u8]) -> Option<Vec<u8>> {
    let mut full = if p.first() == Some(&b'/') {
        p.to_vec()
    } else {
        let cwd = std::env::current_dir().ok()?;
        cat!(shell::bytes(&cwd), "/", p)
    };
    if full.is_empty() {
        full = b"/".to_vec();
    }
    let mut parts: Vec<&[u8]> = Vec::new();
    for c in full.split(|&c| c == b'/') {
        match c {
            b"" | b"." => {}
            b".." => {
                parts.pop();
            }
            _ => parts.push(c),
        }
    }
    Some(cat!("/", parts.join(&b'/')))
}

/// `readlink -e -- P`: every link followed, all of it there.
pub fn readlink_e(p: &[u8]) -> Option<Vec<u8>> {
    std::fs::canonicalize(shell::path(p))
        .ok()
        .map(|r| shell::bytes(&r).to_vec())
}

/// `readlink -f -- P`, as GNU's (gnulib's canonicalize, all but the last part there): each link
/// followed, a dangling last one too (to where it points); a last part that is missing is taken
/// when it is missing and nothing but slashes follows it, any other failure (a parent that cannot
/// be looked into, a file with more after it, a link loop) is None.
pub fn readlink_f(p: &[u8]) -> Option<Vec<u8>> {
    use std::os::unix::fs::MetadataExt;
    if p.is_empty() {
        return None;
    }
    let mut r: Vec<u8> = if p[0] == b'/' {
        b"/".to_vec()
    } else {
        pwd_physical()?
    };
    let mut rest = p.to_vec();
    let mut pos = 0;
    let mut seen: HashSet<(u64, u64, Vec<u8>)> = HashSet::new();
    loop {
        while pos < rest.len() && rest[pos] == b'/' {
            pos += 1;
        }
        if pos >= rest.len() {
            break;
        }
        let start = pos;
        while pos < rest.len() && rest[pos] != b'/' {
            pos += 1;
        }
        let comp = rest[start..pos].to_vec();
        let after = &rest[pos..];
        let more = after.iter().any(|&c| c != b'/');
        if comp == b"." {
            continue;
        }
        let pop = |r: &mut Vec<u8>| {
            if r.len() > 1 {
                let i = r.iter().rposition(|&c| c == b'/').unwrap_or(0);
                r.truncate(i.max(1));
            }
        };
        if comp == b".." {
            pop(&mut r);
            continue;
        }
        if r.last() != Some(&b'/') {
            r.push(b'/');
        }
        r.extend_from_slice(&comp);
        let st = match std::fs::symlink_metadata(shell::path(&r)) {
            Ok(st) => st,
            Err(e) => {
                if more || e.raw_os_error() != Some(libc::ENOENT) {
                    return None;
                }
                continue;
            }
        };
        if st.file_type().is_symlink() {
            // (a loop: the same link with the same rest, again)
            if !seen.insert((st.dev(), st.ino(), after.to_vec())) {
                return None;
            }
            let target = std::fs::read_link(shell::path(&r)).ok()?;
            let target = shell::bytes(&target).to_vec();
            rest = cat!(target, after);
            pos = 0;
            if target.first() == Some(&b'/') {
                r = b"/".to_vec();
            } else {
                pop(&mut r);
            }
            continue;
        }
        if !st.is_dir() && !after.is_empty() {
            return None;
        }
    }
    Some(r)
}

/// `pwd -P`.
fn pwd_physical() -> Option<Vec<u8>> {
    std::env::current_dir()
        .ok()
        .map(|d| shell::bytes(&d).to_vec())
}

/// The repo the caller is in: git's top (outside a jail), or the jail's folder its cwd is in that a
/// box may mount, with what the jail hides in it covered (#210).
pub fn repo_top() -> Option<Vec<u8>> {
    if !in_jail() {
        let o = crate::sig::command("git")
            .args(["rev-parse", "--show-toplevel"])
            .stderr(Stdio::null())
            .output()
            .ok()?;
        return o.status.success().then(|| strip_nl(&o.stdout));
    }
    let r = jail_cwd_root()?;
    std::path::Path::new(&r)
        .join(".git")
        .exists()
        .then(|| r.into_bytes())
}

/// The jail's folder a box may mount that a jailed caller's cwd is in, the deepest.
fn jail_cwd_root() -> Option<String> {
    let cwd = crate::jail::get_raw("cwd");
    if cwd.is_empty() {
        return None;
    }
    let (vals, _) = json::values(&shell::env_or_empty("OMABOX_JAIL"));
    let pol = vals.first()?;
    let cwd = String::from_utf8_lossy(&cwd).into_owned();
    let mut best: Option<String> = None;
    for root in pol.get("roots")?.as_array()? {
        let Some(r) = root.get("path").and_then(Value::as_str) else {
            continue;
        };
        if !coverable(pol, root) || !(cwd == r || cwd.starts_with(&format!("{r}/"))) {
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|b| r.chars().count() >= b.chars().count())
        {
            best = Some(r.to_string());
        }
    }
    best
}

/// norepo_note HOW (#394): a jailed caller in a jail's folder with no .git gets no project in its
/// box (`up`, or `run`'s throwaway: HOW), and nothing else would say so. Outside a jail a box never
/// takes the folder it happens to start in (finding 63): nothing said.
pub fn norepo_note(how: &str) -> Option<Vec<u8>> {
    if !in_jail() || repo_top().is_some() {
        return None;
    }
    let r = no_cntrl(jail_cwd_root()?.as_bytes());
    let fix = match refuse_src(&r) {
        Some(why) => cat!("a box may not mount it (", why, ")"),
        None => cat!(
            how,
            " --ro-bind ",
            r,
            " (or --overlay, to write in the box) mounts it"
        ),
    };
    Some(cat!(
        "note: ",
        r,
        " is not a git repo, so this box does not have it (a box takes a jail's folder only when it has a .git): ",
        fix
    ))
}

/// JAIL_COVERABLE: a jail's folder a box may mount with what the jail hides in it covered again: not
/// one of the hidden paths itself, and one with nothing hidden or whose hidden paths the policy lists.
pub(crate) fn coverable(pol: &Value, root: &Value) -> bool {
    let path = root.get("path");
    let masks = pol.get("masks").and_then(Value::as_array);
    let hidden = masks.is_some_and(|m| m.iter().any(|x| Some(x) == path));
    let masked = root
        .get("masked")
        .is_some_and(|m| !matches!(m, Value::Null | Value::Bool(false)));
    !hidden && (!masked || pol.get("masks").is_some())
}

/// jail_root P: P resolved, when it is one of the jail's folders, whole (nothing of the jail's own
/// mounted inside).
pub fn jail_root(p: &[u8]) -> Option<Vec<u8>> {
    let r = readlink_e(p)?;
    let (vals, _) = json::values(&shell::env_or_empty("OMABOX_JAIL"));
    let rs = String::from_utf8_lossy(&r).into_owned();
    let ok = vals
        .first()
        .and_then(|pol| pol.get("roots")?.as_array())
        .is_some_and(|roots| {
            roots.iter().any(|x| {
                x.get("path").and_then(Value::as_str) == Some(rs.as_str())
                    && x.get("masked")
                        .is_none_or(|m| matches!(m, Value::Null | Value::Bool(false)))
            })
        });
    ok.then_some(r)
}

fn home() -> Vec<u8> {
    shell::env_or_empty("HOME")
}

/// `${VAR:-DEFAULT}`.
fn env_or(name: &str, default: Vec<u8>) -> Vec<u8> {
    shell::env(name)
        .filter(|v| !v.is_empty())
        .unwrap_or(default)
}

/// `"$DIR"/PREFIX*` unquoted in a for list: its matches in glob order, the word itself for none.
fn glob_prefix(dir: &str, prefix: &str) -> Vec<Vec<u8>> {
    let m: Vec<Vec<u8>> = shell::glob(std::path::Path::new(dir), false)
        .into_iter()
        .filter(|n| n.starts_with(prefix.as_bytes()))
        .map(|n| cat!(dir, "/", n))
        .collect();
    if m.is_empty() {
        vec![format!("{dir}/{prefix}*").into_bytes()]
    } else {
        m
    }
}

/// refuse_path ABSPATH [look]: why a box may not mount ABSPATH (taken as it is), or None.
pub fn refuse_path(p: &[u8], look: bool) -> Option<Vec<u8>> {
    let slash = |x: &[u8]| cat!(x.strip_suffix(b"/").unwrap_or(x), "/");
    let s = slash(p);
    let h = home();
    let oc = cat!(h, "/.config/omarchy");
    let o = readlink_f(&oc).unwrap_or_else(|| oc.clone());
    for x in [h.clone(), oc.clone(), o.clone(), b"/tmp".to_vec()] {
        let x = slash(&x);
        if x.starts_with(&s) {
            return Some(cat!("it contains ", x));
        }
    }
    for x in [&oc, &o] {
        let x = slash(x);
        let Some(rest) = s.strip_prefix(x.as_slice()) else {
            continue;
        };
        if rest.starts_with(b"plugins/") || rest.starts_with(b"themes/") {
            continue;
        }
        if look && (rest.starts_with(b"branding/") || rest.starts_with(b"extensions/")) {
            continue;
        }
        return Some(cat!(
            "it is inside ",
            x,
            " (api-keys.env, your hooks): only plugins/ and themes/ may go into a box"
        ));
    }
    let data = env_or("XDG_DATA_HOME", cat!(h, "/.local/share"));
    let cache = env_or("XDG_CACHE_HOME", cat!(h, "/.cache"));
    let uid = crate::procfs::uid();
    let mut list: Vec<Vec<u8>> = [
        "/.ssh",
        "/.gnupg",
        "/.local/share/keyrings",
        "/.password-store",
        "/.aws",
        "/.kube",
        "/.docker",
        "/.netrc",
        "/.config/gh",
        "/.config/gcloud",
        "/.azure",
        "/.config/op",
        "/.git-credentials",
        "/.config/git/credentials",
    ]
    .iter()
    .map(|x| cat!(h, x))
    .collect();
    list.push(cat!(data, "/omabox"));
    list.push(cat!(cache, "/omabox"));
    list.extend(["/.claude", "/.claude.json"].iter().map(|x| cat!(h, x)));
    list.push(shell::env_or_empty("CLAUDE_CONFIG_DIR"));
    list.push(cat!(h, "/.codex"));
    list.push(shell::env_or_empty("CODEX_HOME"));
    list.extend(
        [
            "/.config/opencode",
            "/.local/share/opencode",
            "/.pi",
            "/.hermes",
            "/.gemini",
            "/.mozilla",
            "/.config/chromium",
            "/.config/google-chrome",
            "/.config/BraveSoftware",
            "/.config/vivaldi",
            "/.config/microsoft-edge",
            "/.pki",
        ]
        .iter()
        .map(|x| cat!(h, x)),
    );
    list.push(env_or(
        "XDG_RUNTIME_DIR",
        format!("/run/user/{uid}").into_bytes(),
    ));
    for x in [
        "/run",
        "/dev",
        "/proc",
        "/sys",
        "/tmp/.X11-unix",
        "/tmp/.ICE-unix",
    ] {
        list.push(x.as_bytes().to_vec());
    }
    for pre in ["tmux-", "ssh-", "gpg-"] {
        list.extend(glob_prefix("/tmp", pre));
    }
    for x in list {
        // (a glob that matched nothing; an agent's config dir variable that is not set)
        if x.contains(&b'*') || x.is_empty() {
            continue;
        }
        let x = slash(&x);
        if s.starts_with(&x) {
            return Some(cat!("it is inside ", x, " (secrets or live sockets)"));
        }
        if x.starts_with(&s) {
            return Some(cat!("it contains ", x, " (secrets or live sockets)"));
        }
    }
    None
}

/// refuse_src P: why a box may not mount P, or None: P resolved, and as written.
pub fn refuse_src(p: &[u8]) -> Option<Vec<u8>> {
    refuse_src_as(p, false)
}

/// refuse_src P look: seed_home's own copies of the user's look (branding/, extensions/) allowed too.
pub fn refuse_src_look(p: &[u8]) -> Option<Vec<u8>> {
    refuse_src_as(p, true)
}

fn refuse_src_as(p: &[u8], look: bool) -> Option<Vec<u8>> {
    let r = readlink_f(p).unwrap_or_else(|| p.to_vec());
    let l = realpath_ms(p).unwrap_or_else(|| r.clone());
    refuse_path(&r, look).or_else(|| if l != r { refuse_path(&l, look) } else { None })
}

/// `${V//[[:cntrl:]]/?}`.
fn no_cntrl(v: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let n = if v[i] < 0x80 {
            1
        } else {
            utf8_char_len(&v[i..])
        };
        let ch = std::str::from_utf8(&v[i..i + n])
            .ok()
            .and_then(|s| s.chars().next());
        if ch.is_some_and(char::is_control) {
            out.push(b'?');
        } else {
            out.extend_from_slice(&v[i..i + n]);
        }
        i += n;
    }
    out
}

fn utf8_char_len(v: &[u8]) -> usize {
    (2..=4.min(v.len()))
        .find(|&n| std::str::from_utf8(&v[..n]).is_ok())
        .unwrap_or(1)
}

/// repo_link_out TOP PATH (#235): PATH (under TOP, there on the host) reached through a link in TOP
/// that leads out of it: (that link, where it leads, where PATH is then).
pub fn repo_link_out(top: &[u8], p: &[u8]) -> Option<[Vec<u8>; 3]> {
    let under = |x: &[u8]| x.starts_with(&cat!(top, "/"));
    if !under(p) || !shell::path(p).exists() {
        return None;
    }
    let mut p = p.to_vec();
    let w = cat!("/", p, "/");
    if crate::exit::contains(&w, b"/../") || crate::exit::contains(&w, b"/./") {
        p = realpath_ms(&p)?;
        if !under(&p) {
            return None;
        }
    }
    let mut rest: &[u8] = &p[top.len() + 1..];
    let mut cur = top.to_vec();
    while !rest.is_empty() {
        let (c, r) = match rest.iter().position(|&c| c == b'/') {
            Some(i) => (&rest[..i], &rest[i + 1..]),
            None => (rest, &b""[..]),
        };
        rest = r;
        if c.is_empty() {
            continue;
        }
        cur = cat!(cur, "/", c);
        if !shell::path(&cur).is_symlink() {
            continue;
        }
        let r = readlink_e(&cur)?;
        if r == top || under(&r) {
            continue;
        }
        let rp = readlink_e(&p)?;
        let all = cat!(cur, r, rp);
        if all.contains(&b'\t') || all.contains(&b'\n') {
            return None;
        }
        return Some([cur, r, rp]);
    }
    None
}

/// link_dangles PID LINK RESOLVED: the box has LINK (the repo is mounted) but not where it leads.
fn link_dangles(pid: &[u8], link: &[u8], resolved: &[u8]) -> bool {
    shell::path(&cat!("/proc/", pid, "/root", link)).is_symlink()
        && !shell::path(&cat!("/proc/", pid, "/root", resolved)).exists()
}

/// link_mountable TARGET: why no mount is offered for a link's target, or None when one may be.
pub fn link_mountable(t: &[u8]) -> Option<Vec<u8>> {
    let t = cat!(t.strip_suffix(b"/").unwrap_or(t), "/");
    let h = home();
    let c = env_or("XDG_CACHE_HOME", cat!(h, "/.cache"));
    let c = cat!(c.strip_suffix(b"/").unwrap_or(&c), "/");
    if t.starts_with(&c) && t.len() > c.len() {
        return None;
    }
    let hr = readlink_f(&h).unwrap_or_else(|| h.clone());
    for x in [&h, &hr] {
        let x = cat!(x.strip_suffix(b"/").unwrap_or(x), "/");
        let Some(rest) = t.strip_prefix(x.as_slice()) else {
            continue;
        };
        if crate::exit::contains(&cat!("/", rest), b"/.") {
            return Some(
                b"it is in a hidden folder of your HOME, where settings and keys live".to_vec(),
            );
        }
    }
    for v in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"] {
        let x = shell::env_or_empty(v);
        if x.is_empty() {
            continue;
        }
        if t.starts_with(&cat!(x.strip_suffix(b"/").unwrap_or(&x), "/")) {
            return Some(cat!("it is in ", x, ", where settings and keys live"));
        }
    }
    None
}

/// link_note WHAT LINK TARGET HOW: LINK -> TARGET outside the box, and the mount that gives it, or
/// why a box may not have it.
pub fn link_note(what: &[u8], link: &[u8], target: &[u8], how: &str) {
    let (w, l, t) = (no_cntrl(what), no_cntrl(link), no_cntrl(target));
    let head = cat!(
        "note: ",
        w,
        l,
        " -> ",
        t,
        ": outside the box, where the link dangles; "
    );
    if let Some(why) = refuse_src(target) {
        say(cat!(head, "a box may not mount it (", why, ")"));
    } else if let Some(why) = link_mountable(target) {
        say(cat!(head, "no mount is offered for it (", why, ")"));
    } else {
        say(cat!(head, how, " --ro-bind ", t, " mounts it"));
    }
}

/// The box's PID 1, when its root can be read (`[ -d /proc/PID/root/usr ]`).
fn box_root_pid(ctx: &Ctx) -> Option<Vec<u8>> {
    match box_pid(&ctx.d) {
        BoxPid::Pid(p) if shell::path(&cat!("/proc/", p, "/root/usr")).is_dir() => Some(p),
        _ => None,
    }
}

/// up_link_notes HOW (#235): each link at the top of the repo `up` mounted that leads out of it to
/// what the box does not have, said with the mount that gives it (HOW: `up`, or `run` for a
/// throwaway's); 5 at most. The top only. Not for a jailed caller.
pub fn up_link_notes(ctx: &Ctx, how: &str) {
    if in_jail() {
        return;
    }
    let Some(top) = repo_top().filter(|t| refuse_src(t).is_none()) else {
        return;
    };
    let Some(pid) = box_root_pid(ctx) else { return };
    let (mut n, mut more) = (0, 0);
    // (`"$top"/* "$top"/.[!.]* "$top"/..?*`: byte order, each group)
    let mut names: Vec<Vec<u8>> = std::fs::read_dir(shell::path(&top))
        .map(|rd| {
            rd.flatten()
                .map(|e| std::os::unix::ffi::OsStringExt::into_vec(e.file_name()))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let (dot, plain): (Vec<_>, Vec<_>) = names.into_iter().partition(|e| e.starts_with(b"."));
    let (two, one): (Vec<_>, Vec<_>) = dot.into_iter().partition(|e| e.starts_with(b".."));
    for e in plain.into_iter().chain(one).chain(two) {
        let e = cat!(top, "/", e);
        if !shell::path(&e).is_symlink() {
            continue;
        }
        let Some([c, r, rp]) = repo_link_out(&top, &e) else {
            continue;
        };
        if !link_dangles(&pid, &c, &rp) {
            continue;
        }
        if n >= 5 {
            more += 1;
            continue;
        }
        n += 1;
        link_note(b"", &c, &r, how);
    }
    if more > 0 {
        say(cat!(
            "note: and ",
            more.to_string(),
            " more links at the top of the repo that lead out of the box"
        ));
    }
}

/// run_link_notes HOW TOP-SAID WORD... (#235): each word of the command that is a path in the repo
/// reached through a link out of it to what the box does not have, said before the command runs.
/// TOP_SAID: links at the top of the repo left out (a throwaway's `up` said them).
pub fn run_link_notes(ctx: &Ctx, how: &str, top_said: bool, words: &[Vec<u8>]) {
    if in_jail() {
        return;
    }
    let Some(here) = pwd_physical() else { return };
    let mut said: HashSet<Vec<u8>> = HashSet::new();
    let mut top_pid: Option<(Vec<u8>, Vec<u8>)> = None;
    for w in words {
        if w.is_empty() || w.starts_with(b"-") {
            continue;
        }
        let p = if w.starts_with(b"/") {
            w.clone()
        } else {
            cat!(here, "/", w)
        };
        if !shell::path(&p).exists() {
            continue;
        }
        if top_pid.is_none() {
            let Some(top) = repo_top() else { return };
            let Some(pid) = box_root_pid(ctx) else { return };
            top_pid = Some((top, pid));
        }
        let Some((top, pid)) = &top_pid else { return };
        let Some([c, r, rp]) = repo_link_out(top, &p) else {
            continue;
        };
        if said.contains(&c) {
            continue;
        }
        let parent = &c[..c.iter().rposition(|&x| x == b'/').unwrap_or(0)];
        if top_said && parent == top.as_slice() {
            continue;
        }
        if !link_dangles(pid, &c, &rp) {
            continue;
        }
        said.insert(c.clone());
        link_note(&cat!(w, " leads through "), &c, &r, how);
    }
}

/// cwd_path_word DIR BOXHOME WORD... (#251): the first word of a command, or of a script in one,
/// that is a relative path there in DIR, and not one BOXHOME has too; 64 looked at at most. Only a
/// word that reads as a path counts (#325: a subcommand that shares a folder's name, `omarchy-shell
/// stash modules` beside a `stash` folder, was taken for one): one with a `/`, `.` or `..`, the
/// command itself, a script's command (its first word, or one after `&&`, `||`, `;`, `|`, `then`,
/// `do`, `else`), or `cd`'s folder.
pub fn cwd_path_word(dir: &[u8], home: &[u8], words: &[Vec<u8>]) -> Option<Vec<u8>> {
    let mut n = 0;
    let mut prev = Vec::new();
    for (i, w) in words.iter().enumerate() {
        let toks = shell::words(w);
        let script = i > 0 && toks.len() > 1;
        for (j, t) in toks.into_iter().enumerate() {
            n += 1;
            if n > 64 {
                return None;
            }
            let lead = (i == 0 && j == 0)
                || (script
                    && (j == 0
                        || prev.ends_with(b";")
                        || matches!(
                            prev.as_slice(),
                            b"&&" | b"||" | b"|" | b"then" | b"do" | b"else"
                        )));
            let pathy = lead || prev == b"cd" || t.contains(&b'/') || t == b"." || t == b"..";
            prev.clone_from(&t);
            if !pathy || t.starts_with(b"-") || t.starts_with(b"/") || t.starts_with(b"~") {
                continue;
            }
            if !shell::path(&cat!(dir, "/", t)).exists() {
                continue;
            }
            if !crate::exit::contains(&cat!("/", t, "/"), b"/../")
                && shell::path(&cat!(home, "/", t)).exists()
            {
                continue;
            }
            return Some(t);
        }
    }
    None
}

/// run_cwd_note HOW WORD... (#251): when a word of the command is a path in the caller's dir and
/// the box runs it in its HOME, not there, says so (with the mount that brings the dir in).
pub fn run_cwd_note(ctx: &Ctx, how: &str, words: &[Vec<u8>]) {
    if in_jail() {
        return;
    }
    let pwd = shell::pwd();
    let Some(w) = cwd_path_word(&pwd, &cat!(ctx.d, "/home"), words) else {
        return;
    };
    let BoxPid::Pid(pid) = box_pid(&ctx.d) else {
        return;
    };
    if crate::exec::box_cwd(&ctx.d, &pid, &pwd) != b"/home/sbx" {
        return;
    }
    let norepo = how == "run" && repo_top().is_none();
    say(cwd_note(&ctx.name, how, &w, &pwd, norepo));
}

/// mount_gone_notes (#391): a folder bound into the box at up (box.json's mounts) that is no longer
/// the one bound, deleted and made again on the host or removed, is said once a box and folder. The
/// box keeps the one it bound (empty, when it was deleted), and every command there succeeds.
pub fn mount_gone_notes(ctx: &Ctx) {
    use std::os::unix::fs::MetadataExt;
    let Ok(text) = std::fs::read(ctx.at("box.json")) else {
        return;
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&text) else {
        return;
    };
    let Some(ms) = v.get("mounts").and_then(serde_json::Value::as_array) else {
        return;
    };
    let noted_f = ctx.at("noted-mounts");
    let noted = std::fs::read(&noted_f).unwrap_or_default();
    let mut add = Vec::new();
    for m in ms {
        let (Some(src), Some(dev), Some(ino)) = (
            m.get("src").and_then(serde_json::Value::as_str),
            m.get("dev").and_then(serde_json::Value::as_u64),
            m.get("ino").and_then(serde_json::Value::as_u64),
        ) else {
            continue;
        };
        let now = std::fs::metadata(src).ok().map(|n| (n.dev(), n.ino()));
        if now == Some((dev, ino)) || noted.split(|&c| c == b'\n').any(|l| l == src.as_bytes()) {
            continue;
        }
        let how = if now.is_none() {
            "removed"
        } else {
            "deleted and made again"
        };
        let dest = m
            .get("dest")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(src);
        say(cat!(
            "note: ",
            no_cntrl(src.as_bytes()),
            ", bound into box '",
            ctx.name,
            "' at up, was ",
            how,
            " on the host since: the box still has the folder it bound at ",
            no_cntrl(dest.as_bytes()),
            ", empty if it was deleted (down, then up again binds the new one; a folder kept, its files remade inside it, needs neither)"
        ));
        add.extend_from_slice(&cat!(src, "\n"));
    }
    if !add.is_empty() {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&noted_f)
        {
            let _ = f.write_all(&add);
        }
    }
}

/// host_path_words WORD... (#301): the absolute paths a command names, each once, in order: a word
/// that is one, the value of a `KEY=VAL` word (or `--opt=VAL`) split on `:` as PATH and
/// LD_LIBRARY_PATH are, and the same in a script word (`sh -c '...'`), its quotes taken off; 64 words
/// looked at at most.
pub fn host_path_words(words: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut paths: Vec<Vec<u8>> = Vec::new();
    let mut n = 0;
    for (i, w) in words.iter().enumerate() {
        let toks = shell::words(w);
        let toks = if i > 0 && toks.len() > 1 {
            toks
        } else {
            vec![w.clone()]
        };
        for t in toks {
            n += 1;
            if n > 64 {
                return paths;
            }
            let mut t = t.trim_ascii();
            while let [b'"' | b'\'' | b'(' | b'`', r @ ..] = t {
                t = r;
            }
            while let [r @ .., b'"' | b'\'' | b')' | b';' | b'`'] = t {
                t = r;
            }
            let vals: Vec<&[u8]> = if t.starts_with(b"/") {
                vec![t]
            } else if let Some(e) = t.iter().position(|&c| c == b'=') {
                t[e + 1..].split(|&c| c == b':').collect()
            } else {
                Vec::new()
            };
            for v in vals {
                if v.len() > 1 && v.starts_with(b"/") && !paths.iter().any(|p| p == v) {
                    paths.push(v.to_vec());
                }
            }
        }
    }
    paths
}

/// What a host path is in box NAME, said by run before its command (#301; HOW: the mount's command,
/// `down, then up` or `run` for a throwaway). P: the path; D: the box's folder; MAPS: (host folder,
/// the box's path for it, what it is) for what the box has elsewhere; IN_BOX: the box has P at its
/// own path; ON_HOST: the host has it. None: the box sees P as the host does, or neither has it.
#[allow(clippy::too_many_arguments)]
pub fn host_path_text(
    name: &[u8],
    how: &str,
    p: &[u8],
    d: &[u8],
    uid: u32,
    maps: &[(Vec<u8>, Vec<u8>, Vec<u8>)],
    in_box: &dyn Fn(&[u8]) -> bool,
    on_host: &dyn Fn(&[u8]) -> bool,
) -> Option<Vec<u8>> {
    let rest = |dir: &[u8]| -> Option<Vec<u8>> {
        let dir = dir.strip_suffix(b"/").unwrap_or(dir);
        if p == dir {
            return Some(Vec::new());
        }
        p.strip_prefix(dir)
            .filter(|r| r.starts_with(b"/"))
            .map(<[u8]>::to_vec)
    };
    let (pp, n) = (no_cntrl(p), no_cntrl(name));
    let own: [(&[u8], Vec<u8>, &str, &str); 2] = [
        (b"/home", b"/home/sbx".to_vec(), "HOME", "--home"),
        (
            b"/run",
            format!("/run/user/{uid}").into_bytes(),
            "runtime dir",
            "--run",
        ),
    ];
    for (sub, at, what, flag) in own {
        if let Some(r) = rest(&cat!(d, sub)) {
            return Some(cat!(
                "note: ",
                pp,
                " is box '",
                n,
                "''s ",
                what,
                " as the host has it (omabox path ",
                flag,
                "): in the box it is ",
                at,
                no_cntrl(&r)
            ));
        }
    }
    if rest(d).is_some() {
        return Some(cat!(
            "note: ",
            pp,
            " is in box '",
            n,
            "''s folder on the host, which the box does not see"
        ));
    }
    // (the box has a /proc of its own, which the host cannot read as the box does: /proc/self is the
    // reader's, a pid is another process's; /proc/self/fd/0 is the command's stdin there too)
    if p == b"/proc" || p.starts_with(b"/proc/") || in_box(p) {
        return None;
    }
    // (the deepest folder it is in that the box has elsewhere: a plugin's, a DIR:DEST bind)
    let best = maps
        .iter()
        .filter_map(|(src, dest, what)| rest(src).map(|r| (src, dest, what, r)))
        .max_by_key(|(src, ..)| src.len());
    if let Some((src, dest, what, r)) = best {
        return Some(cat!(
            "note: ",
            pp,
            " is in ",
            no_cntrl(src),
            ", ",
            what,
            ", which box '",
            n,
            "' has at ",
            no_cntrl(dest),
            ": in the box it is ",
            no_cntrl(dest),
            no_cntrl(&r)
        ));
    }
    if !on_host(p) {
        return None;
    }
    let dir = if shell::path(p).is_dir() {
        p.to_vec()
    } else {
        p[..p.iter().rposition(|&c| c == b'/').unwrap_or(0).max(1)].to_vec()
    };
    let mount = match refuse_src(&dir) {
        Some(why) => cat!("a box may not mount ", no_cntrl(&dir), " (", why, ")"),
        None => cat!(how, " --ro-bind ", no_cntrl(&dir), " mounts it"),
    };
    let fix = if how == "run" {
        mount
    } else {
        cat!(
            "copy it into the box HOME (omabox path --home; /home/sbx in the box), or ",
            mount
        )
    };
    Some(cat!(
        "note: ",
        pp,
        " is not in box '",
        n,
        "' (a box sees the repo and what --ro-bind mounts, not /tmp or the rest of HOME): ",
        fix
    ))
}

/// host_path_notes HOW WORD... (#301): each absolute host path the command names (host_path_words)
/// that the box has elsewhere or not at all, said before it runs; 3 at most. A path the box HOME's
/// host path, a /tmp scratch file, a build dir in LD_LIBRARY_PATH: the command fails in the box, or
/// quietly takes something else, and its error names the file, not why. Not for a jailed caller.
pub fn host_path_notes(ctx: &Ctx, how: &str, words: &[Vec<u8>]) {
    if in_jail() {
        return;
    }
    let paths = host_path_words(words);
    if paths.is_empty() {
        return;
    }
    let Some(pid) = box_root_pid(ctx) else { return };
    let maps = box_maps(ctx);
    let in_box =
        |p: &[u8]| std::fs::symlink_metadata(shell::path(&cat!("/proc/", pid, "/root", p))).is_ok();
    let on_host = |p: &[u8]| std::fs::symlink_metadata(shell::path(p)).is_ok();
    let uid = crate::procfs::uid();
    let (mut n, mut more) = (0, 0);
    for p in &paths {
        let Some(t) = host_path_text(&ctx.name, how, p, &ctx.d, uid, &maps, &in_box, &on_host)
        else {
            continue;
        };
        if n >= 3 {
            more += 1;
            continue;
        }
        n += 1;
        say(t);
    }
    if more > 0 {
        say(cat!(
            "note: and ",
            more.to_string(),
            " more host paths in the command that box '",
            no_cntrl(&ctx.name),
            "' has elsewhere or not at all"
        ));
    }
}

/// What box.json says the box has at another path than the host's (#301): each plugin mounted (not
/// linked) at its id, each bind of DIR:DEST.
fn box_maps(ctx: &Ctx) -> Vec<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let Ok(text) = std::fs::read(ctx.at("box.json")) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_slice::<Value>(&text) else {
        return Vec::new();
    };
    let mut maps = Vec::new();
    if let Some(dirs) = v.get("plugin_dirs").and_then(Value::as_object) {
        for (id, dir) in dirs {
            let via = v
                .get("plugin_via")
                .and_then(|x| x.get(id))
                .and_then(Value::as_str);
            if let (Some(dir), Some("mounted")) = (dir.as_str(), via) {
                maps.push((
                    dir.as_bytes().to_vec(),
                    cat!("/home/sbx/.config/omarchy/plugins/", id),
                    cat!("plugin ", id, "'s folder"),
                ));
            }
        }
    }
    for m in v
        .get("mounts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let (Some(src), Some(dest)) = (
            m.get("src").and_then(Value::as_str),
            m.get("dest").and_then(Value::as_str),
        ) else {
            continue;
        };
        if src != dest {
            maps.push((
                src.as_bytes().to_vec(),
                dest.as_bytes().to_vec(),
                b"bound at up".to_vec(),
            ));
        }
    }
    maps
}

/// The note run_cwd_note says (NAME's box; HOW the command; WORD the path word, DIR the caller's
/// dir; NOREPO a throwaway outside any repo), its control characters as ?.
pub fn cwd_note(name: &[u8], how: &str, word: &[u8], dir: &[u8], norepo: bool) -> Vec<u8> {
    let (w, d) = (no_cntrl(word), no_cntrl(dir));
    let wher = if norepo {
        b"is in no repo, so not in this throwaway box".to_vec()
    } else {
        cat!("box '", name, "' does not have")
    };
    let fix = match refuse_src(dir) {
        Some(why) => cat!("a box may not mount it (", why, ")"),
        None => cat!(
            how,
            " --overlay ",
            d,
            " (or --ro-bind, read-only) mounts it"
        ),
    };
    cat!(
        "note: ",
        w,
        " is relative to ",
        d,
        ", which ",
        wher,
        ": the command runs in the box HOME (/home/sbx); ",
        fix
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    proptest::proptest! {
        // S3 (#355): every path at or under HOME's secret stores, ~/.config/omarchy outside plugins/
        // and themes/, the runtime dir and the kernel's trees, and every folder holding one of them
        // (HOME itself, /), is refused as a mount, however it is spelled.
        #[test]
        fn secret_paths_are_refused(
            root in proptest::sample::select(vec![
                ".ssh", ".gnupg", ".local/share/keyrings", ".password-store", ".aws", ".kube",
                ".docker", ".config/gh", ".config/omarchy", ".config/omarchy/hooks", ".claude", ".codex",
            ]),
            tail in proptest::collection::vec(proptest::sample::select(vec!["a", "b.c", ".", "x y"]), 0..3),
            slash: bool,
        ) {
            let home = shell::env_or_empty("HOME");
            let mut p = cat!(home, "/", root);
            for t in &tail {
                p = cat!(p, "/", t);
            }
            if slash {
                p.push(b'/');
            }
            proptest::prop_assert!(refuse_src(&p).is_some(), "{:?}", String::from_utf8_lossy(&p));
            proptest::prop_assert!(refuse_src(&home).is_some());
            proptest::prop_assert!(refuse_src(b"/").is_some());
            for sys in ["/run", "/dev/input", "/proc/1", "/sys"] {
                proptest::prop_assert!(refuse_src(sys.as_bytes()).is_some(), "{}", sys);
            }
        }
    }

    #[test]
    fn lexical_paths() {
        assert_eq!(realpath_ms(b"/a/./b/../c//d/"), Some(b"/a/c/d".to_vec()));
        assert_eq!(realpath_ms(b"/.."), Some(b"/".to_vec()));
        assert_eq!(realpath_ms(b"/"), Some(b"/".to_vec()));
    }

    // readlink_f against GNU's own on links, dangling ones, loops, files with more after them.
    #[test]
    fn readlink_f_as_gnu() {
        let d = std::env::temp_dir().join(format!("omabox-rl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("d")).ok();
        std::fs::write(d.join("f"), b"").ok();
        for (l, t) in [
            ("lf", "f"),
            ("dang", "nope"),
            ("abs", "/tmp/omabox-rl-new.png"),
            ("ld", "d"),
            ("loop1", "loop2"),
            ("loop2", "loop1"),
            ("up", "../"),
        ] {
            std::os::unix::fs::symlink(t, d.join(l)).ok();
        }
        let base = shell::bytes(&d).to_vec();
        for p in [
            "f", "f/", "d/", "ld/x", "ld/x/y", "dang", "dang/", "abs", "abs/x", "nope", "nope/x",
            "lf/x", "loop1", ".", "..", "d/../f", "ld/../f", "lf/", "up/f", "d//./f", "ld/",
        ] {
            let full = cat!(base, "/", p);
            let gnu = crate::sig::command("readlink")
                .arg("-f")
                .arg("--")
                .arg(std::ffi::OsStr::new(
                    std::str::from_utf8(&full).unwrap_or_default(),
                ))
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| strip_nl(&o.stdout));
            assert_eq!(readlink_f(&full), gnu, "{p}");
        }
        assert_eq!(readlink_f(b""), None);
        assert_eq!(readlink_f(b"/root/x"), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn controls_shown_as_question_marks() {
        assert_eq!(no_cntrl(b"a\tb\x1bc"), b"a?b?c");
        assert_eq!(no_cntrl("é\u{85}x".as_bytes()), "é?x".as_bytes());
        assert_eq!(no_cntrl(b"\xffz"), b"\xffz");
    }

    #[test]
    fn path_words() {
        let d = std::env::temp_dir();
        let dir = shell::bytes(&d).to_vec();
        assert_eq!(
            cwd_path_word(&dir, b"/nonexistent", &[b"-x .".to_vec()]),
            Some(b".".to_vec())
        );
        assert_eq!(
            cwd_path_word(&dir, b"/nonexistent", &[b"/abs ~x".to_vec()]),
            None
        );
    }

    // #301: the absolute paths a command names: a word, KEY=VAL's parts, a script's words unquoted.
    #[test]
    fn host_path_words_found() {
        let w = |v: &[&str]| -> Vec<Vec<u8>> { v.iter().map(|x| x.as_bytes().to_vec()).collect() };
        assert_eq!(
            host_path_words(&w(&[
                "env",
                "LD_LIBRARY_PATH=/b/lib:/usr/lib",
                "/tmp/x/app",
                "--conf=/c",
                "rel/x",
                "/tmp/x/app",
            ])),
            w(&["/b/lib", "/usr/lib", "/tmp/x/app", "/c"])
        );
        assert_eq!(
            host_path_words(&w(&[
                "sh",
                "-c",
                "grep -q x \"/r/run-1.log\"; test -s '/h/f'"
            ])),
            w(&["/r/run-1.log", "/h/f"])
        );
        // the command itself is one word, spaces and all; "/" alone is no path
        assert_eq!(host_path_words(&w(&["/a b/c", "/"])), w(&["/a b/c"]));
    }

    // #301: what a host path is in the box: its HOME's and runtime dir's host paths mapped, the box
    // folder unseen, a plugin's or DIR:DEST's mapped, one the box lacks with the mount that gives it.
    #[test]
    fn host_path_said() {
        let maps = vec![
            (
                b"/src/plug".to_vec(),
                b"/home/sbx/.config/omarchy/plugins/p.x".to_vec(),
                b"plugin p.x's folder".to_vec(),
            ),
            (
                b"/nas".to_vec(),
                b"/mnt/nas".to_vec(),
                b"bound at up".to_vec(),
            ),
        ];
        let t = |how: &str, p: &str, in_box: bool, on_host: bool| {
            host_path_text(
                b"B",
                how,
                p.as_bytes(),
                b"/r/omabox/B",
                1000,
                &maps,
                &|_| in_box,
                &|_| on_host,
            )
            .map(|v| String::from_utf8_lossy(&v).into_owned())
        };
        assert_eq!(
            t("down, then up", "/r/omabox/B/home/run-1.log", false, true).as_deref(),
            Some(
                "note: /r/omabox/B/home/run-1.log is box 'B''s HOME as the host has it (omabox path --home): in the box it is /home/sbx/run-1.log"
            )
        );
        assert_eq!(
            t("down, then up", "/r/omabox/B/run/app/s.json", false, true).as_deref(),
            Some(
                "note: /r/omabox/B/run/app/s.json is box 'B''s runtime dir as the host has it (omabox path --run): in the box it is /run/user/1000/app/s.json"
            )
        );
        assert_eq!(
            t("down, then up", "/r/omabox/B/home", false, true).as_deref(),
            Some(
                "note: /r/omabox/B/home is box 'B''s HOME as the host has it (omabox path --home): in the box it is /home/sbx"
            )
        );
        assert_eq!(
            t("down, then up", "/r/omabox/B/box.json", false, true).as_deref(),
            Some(
                "note: /r/omabox/B/box.json is in box 'B''s folder on the host, which the box does not see"
            )
        );
        // (/r/omabox/Bx is another box's: not this one's folder)
        assert_eq!(t("down, then up", "/r/omabox/Bx/f", true, true), None);
        assert_eq!(
            t("down, then up", "/src/plug/bin/h.sh", false, true).as_deref(),
            Some(
                "note: /src/plug/bin/h.sh is in /src/plug, plugin p.x's folder, which box 'B' has at /home/sbx/.config/omarchy/plugins/p.x: in the box it is /home/sbx/.config/omarchy/plugins/p.x/bin/h.sh"
            )
        );
        assert_eq!(
            t("down, then up", "/nas/a", false, true).as_deref(),
            Some(
                "note: /nas/a is in /nas, bound at up, which box 'B' has at /mnt/nas: in the box it is /mnt/nas/a"
            )
        );
        // what the box has at the host's path, or neither has: nothing said
        assert_eq!(t("down, then up", "/usr/bin/true", true, true), None);
        assert_eq!(t("down, then up", "/home/sbx/x", false, false), None);
        let missing = "/nonexistent-301/f";
        assert_eq!(
            t("down, then up", missing, false, true).as_deref(),
            Some(
                "note: /nonexistent-301/f is not in box 'B' (a box sees the repo and what --ro-bind mounts, not /tmp or the rest of HOME): copy it into the box HOME (omabox path --home; /home/sbx in the box), or down, then up --ro-bind /nonexistent-301 mounts it"
            )
        );
        assert_eq!(
            t("run", missing, false, true).as_deref(),
            Some(
                "note: /nonexistent-301/f is not in box 'B' (a box sees the repo and what --ro-bind mounts, not /tmp or the rest of HOME): run --ro-bind /nonexistent-301 mounts it"
            )
        );
        // (/proc is the box's own: /proc/self/fd/0, a command reading its stdin, is not noted)
        assert_eq!(t("run", "/proc/self/fd/0", false, true), None);
        assert_eq!(t("run", "/proc", false, true), None);
        let dev = t("run", "/dev/dri/card0", false, true).unwrap_or_default();
        assert!(
            dev.ends_with(
                "a box may not mount /dev/dri (it is inside /dev/ (secrets or live sockets))"
            ),
            "{dev}"
        );
    }
}
