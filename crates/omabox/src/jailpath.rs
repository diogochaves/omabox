//! A jailed caller's host paths for up (#324 wave 5a, #198, #210): followed only inside its jail's
//! folders, refused as written, covers checked.

use crate::cat;
use crate::exit::{Ctx, Exit};
use crate::json;
use crate::repo::{coverable, readlink_e};
use crate::shell::{self, strip_nl};
use serde_json::Value;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::MetadataExt;

/// The jail's policy (OMABOX_JAIL), as broker_init wrote it.
fn policy() -> Option<Value> {
    json::values(&shell::env_or_empty("OMABOX_JAIL"))
        .0
        .into_iter()
        .next()
}

/// `.roots[].path`.
fn roots(pol: &Value) -> Vec<Vec<u8>> {
    pol.get("roots")
        .and_then(Value::as_array)
        .map(|rs| {
            rs.iter()
                .filter_map(|r| r.get("path").and_then(Value::as_str))
                .map(|s| s.as_bytes().to_vec())
                .collect()
        })
        .unwrap_or_default()
}

/// `.masks[]`, or None where the policy has no `masks` (an older broker: it cannot say what is hidden).
fn masks(pol: &Value) -> Option<Vec<Vec<u8>>> {
    let m = pol.as_object()?.get("masks")?;
    Some(
        m.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.as_bytes().to_vec())
                    .collect()
            })
            .unwrap_or_default(),
    )
}

/// P is R or below it.
fn at_or_under(p: &[u8], r: &[u8]) -> bool {
    p == r || under(p, r)
}

/// P is below R (`startswith($r + "/")`).
fn under(p: &[u8], r: &[u8]) -> bool {
    p.len() > r.len() && p.starts_with(r) && p[r.len()] == b'/'
}

/// `sub("/[^/]*$"; "")`: P without its last component.
fn dir_of(p: &[u8]) -> &[u8] {
    match p.iter().rposition(|&c| c == b'/') {
        Some(i) => &p[..i],
        None => p,
    }
}

/// jail_sees P: P resolved, when the jail sees it (inside one of its folders).
pub fn jail_sees(p: &[u8]) -> Option<Vec<u8>> {
    sees_in(&policy()?, p)
}

fn sees_in(pol: &Value, p: &[u8]) -> Option<Vec<u8>> {
    let r = readlink_e(p)?;
    roots(pol).iter().any(|x| at_or_under(&r, x)).then_some(r)
}

/// jail_root_covered P: as jail_root, but a folder the jail hides paths in too (#210), when the box
/// can cover each of them (jail_cover): for --ro-bind, whose folder nothing on the host reads.
pub fn jail_root_covered(p: &[u8]) -> Option<Vec<u8>> {
    root_covered_in(&policy()?, p)
}

fn root_covered_in(pol: &Value, p: &[u8]) -> Option<Vec<u8>> {
    let r = readlink_e(p)?;
    let ok = pol
        .get("roots")
        .and_then(Value::as_array)
        .is_some_and(|rs| {
            rs.iter().any(|x| {
                x.get("path").and_then(Value::as_str).map(str::as_bytes) == Some(r.as_slice())
                    && coverable(pol, x)
            })
        });
    ok.then_some(r)
}

/// jail_hidden P (#210): P (resolved) at or under one of the jail's hidden paths; true when the
/// policy cannot say which those are.
fn hidden_in(pol: &Value, p: &[u8]) -> bool {
    let Some(ms) = masks(pol) else {
        return true;
    };
    ms.iter().any(|m| at_or_under(p, m))
}

/// The innermost of ROOTS holding X (`max_by(length)`), if any.
fn inner<'a>(roots: &'a [Vec<u8>], x: &[u8]) -> Option<&'a Vec<u8>> {
    // (jq's length counts characters; two roots holding X are one inside the other, so any measure
    // of length picks the same one)
    roots
        .iter()
        .filter(|r| at_or_under(x, r))
        .max_by_key(|r| r.len())
}

/// jail_deep P (#210): the innermost of the jail's folders holding P has a hidden path below a folder
/// of its own rather than straight in it: one the jail can move with the folder above it (finding
/// 282). True when the policy cannot tell (no masks: an older broker), or P is in none of them.
pub fn jail_deep(p: &[u8]) -> bool {
    deep_in(&policy().unwrap_or(Value::Null), p)
}

fn deep_in(pol: &Value, p: &[u8]) -> bool {
    let Some(ms) = masks(pol) else { return true };
    let rs = roots(pol);
    let Some(r) = inner(&rs, p) else { return true };
    ms.iter()
        .any(|m| under(m, r) && inner(&rs, m) == Some(r) && dir_of(m) != r.as_slice())
}

/// jail_readable FILE (#210): FILE (resolved, inside the jail's folders) may be read on the host for
/// a jailed caller: not hidden nor under a hidden path, and where none of those can have moved since
/// the policy was read (straight in one of the jail's folders, or in one where none can: jail_deep).
pub fn jail_readable(f: &[u8]) -> bool {
    readable_in(&policy().unwrap_or(Value::Null), f)
}

fn readable_in(pol: &Value, f: &[u8]) -> bool {
    if hidden_in(pol, f) {
        return false;
    }
    roots(pol).iter().any(|r| r.as_slice() == dir_of(f)) || !deep_in(pol, f)
}

/// jail_masked P (#198 security): P at, under, or holding one of the jail's hidden paths (ai-jail
/// mounts an empty file over it while the host file stays, so a box mounting from the host would get
/// the host's); true for a policy that cannot say which those are, so a seed from it is refused.
pub fn jail_masked(p: &[u8]) -> bool {
    masked_in(&policy().unwrap_or(Value::Null), p)
}

fn masked_in(pol: &Value, p: &[u8]) -> bool {
    let Some(ms) = masks(pol) else {
        return true;
    };
    ms.iter()
        .filter(|m| !m.is_empty())
        .any(|m| at_or_under(p, m) || under(m, p))
}

/// jail_path PATH (#198): where a host path a jailed caller named leads, found without looking at
/// anything outside the jail's folders. PATH (made absolute against the cwd) is followed one
/// component at a time, as the kernel would, and a step is looked at (is it a link?) only once it is
/// inside one of them; one on the way to one is taken as the dir it is. Where it leads, inside one;
/// None when a step leaves them, having looked at nothing there. Whether anything is at the path is
/// the caller's to ask, of what this gives (inside the jail).
pub fn jail_path(p: &[u8]) -> Option<Vec<u8>> {
    path_in(&policy().unwrap_or(Value::Null), p)
}

fn path_in(pol: &Value, p: &[u8]) -> Option<Vec<u8>> {
    let rs = roots(pol);
    let t = if p.first() == Some(&b'/') {
        p.to_vec()
    } else {
        let pwd = shell::pwd();
        cat!(pwd.strip_suffix(b"/").unwrap_or(&pwd), "/", p)
    };
    let split =
        |v: &[u8]| -> Vec<Vec<u8>> { v.split(|&c| c == b'/').map(<[u8]>::to_vec).collect() };
    let mut todo: std::collections::VecDeque<Vec<u8>> = split(&t).into();
    let mut cur: Vec<u8> = Vec::new();
    let mut hops = 0;
    let up_one =
        |cur: &mut Vec<u8>| cur.truncate(cur.iter().rposition(|&x| x == b'/').unwrap_or(0));
    while let Some(c) = todo.pop_front() {
        match c.as_slice() {
            b"" | b"." => continue,
            b".." => {
                up_one(&mut cur);
                continue;
            }
            _ => {}
        }
        cur = cat!(cur, "/", c);
        if !rs.iter().any(|r| at_or_under(&cur, r)) {
            // (on the way to one of them: taken as the dir it is)
            if rs.iter().any(|r| under(r, &cur)) {
                continue;
            }
            return None;
        }
        let cp = shell::path(&cur);
        if !cp.is_symlink() {
            continue;
        }
        // (the kernel's ELOOP)
        hops += 1;
        if hops > 40 {
            return None;
        }
        let target = std::fs::read_link(&cp).ok()?;
        let target = shell::bytes(&target).to_vec();
        if target.first() == Some(&b'/') {
            cur.clear();
        } else {
            up_one(&mut cur);
        }
        for part in split(&target).into_iter().rev() {
            todo.push_front(part);
        }
    }
    let end = if cur.is_empty() { b"/".to_vec() } else { cur };
    rs.iter().any(|r| at_or_under(&end, r)).then_some(end)
}

/// jail_read FILE: FILE's content, when the file it opens is inside the jail's folders, a regular
/// file, not one the jail hides (#210), and at most READ_MAX bytes; None, reading nothing, otherwise
/// (#198). Where FILE leads is found first without looking at anything outside them (jail_path), so
/// nothing there is opened (#367); then it is opened as a path only (O_PATH: no device's or FIFO's
/// open runs, no terminal becomes this process's), its path read back from the fd (a link swapped in
/// since is caught), and only a regular file is opened again to read (O_NONBLOCK, O_NOCTTY).
pub fn jail_read(f: &[u8]) -> Option<Vec<u8>> {
    read_in(&policy()?, f)
}

/// What jail_read reads at most: what it is for (a plugin's manifest, a checkout's .git file, a
/// worktree's commondir) is small; a bigger file is refused, never read whole (#367).
const READ_MAX: u64 = 1 << 20;

fn read_in(pol: &Value, f: &[u8]) -> Option<Vec<u8>> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    let t = path_in(pol, f)?;
    // (links already followed inside the jail's folders: one at its end now was swapped in)
    let at = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_NOFOLLOW)
        .open(shell::path(&t))
        .ok()?;
    if !at.metadata().ok()?.is_file() {
        return None;
    }
    let fd = format!("/proc/self/fd/{}", at.as_raw_fd());
    let p = std::fs::read_link(&fd).ok()?;
    let p = shell::bytes(&p).to_vec();
    if !roots(pol).iter().any(|r| at_or_under(&p, r)) || hidden_in(pol, &p) {
        return None;
    }
    // (the same file, through its fd: never the path again)
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(&fd)
        .ok()?;
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(&file, READ_MAX + 1), &mut out).ok()?;
    (out.len() as u64 <= READ_MAX).then_some(out)
}

/// jail_refuse OPT PATH WHAT (#198): up's refusal of a host path a jailed caller named: the path as
/// written, never where it leads, and the same words whether it is outside the jail's folders, leads
/// out of them through a link, or is not there, so a refusal says nothing of the host. A failure
/// (bash's `fail`: 1), not a usage error.
pub fn jail_refuse(ctx: &Ctx, opt: &str, p: &[u8], what: &str) -> Exit {
    let mut c = ctx.clone();
    c.parsed();
    c.die(cat!(
        "up: ",
        opt,
        " ",
        p,
        " is not ",
        what,
        ", so it does not go into the box"
    ))
}

/// What a cover is: an empty folder over a hidden folder, an empty file over anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverKind {
    /// `$D/cover-dir`.
    Dir,
    /// `$D/cover-file`.
    File,
}

/// jail_cover's answer for one mount: the covers (each the empty thing and its place in the box,
/// mounted after every other mount), and the folder when it goes in `cover_srcs` (what jail_verify
/// checks against the jail's own mounts).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Covers {
    pub src: Option<Vec<u8>>,
    pub binds: Vec<(CoverKind, Vec<u8>)>,
}

/// jail_cover SRC DEST (#210): a jailed caller's folder SRC, mounted at DEST in the box, gets what the
/// jail hides in it hidden there again, as ai-jail hides it: none for a path not there now, a link,
/// or one inside another (the outer one's cover hides it). The paths are the policy's, from the
/// broker's start: one the jail moved since gets no cover where it is now, which jail_verify catches.
/// A path inside one of the jail's folders (mise's installs) instead must hold none and be where none
/// can move to (jail_masked, jail_deep). None when it cannot be done (the policy does not say what is
/// hidden, SRC is hidden whole, or there is no python for jail_verify).
pub fn jail_cover(src: &[u8], dest: &[u8]) -> Option<Covers> {
    cover_in(&policy()?, src, dest)
}

fn cover_in(pol: &Value, src: &[u8], dest: &[u8]) -> Option<Covers> {
    let ms = masks(pol)?;
    if !roots(pol).iter().any(|r| r.as_slice() == src) {
        return (!masked_in(pol, src) && !deep_in(pol, src)).then(Covers::default);
    }
    if ms.iter().any(|m| m.as_slice() == src) {
        return None;
    }
    // (jq's sort of strings: by code point, which is byte order in UTF-8)
    let mut inside: Vec<Vec<u8>> = ms.into_iter().filter(|m| under(m, src)).collect();
    inside.sort();
    if inside.is_empty() {
        return Some(Covers::default());
    }
    if !crate::exec::executable(b"/usr/bin/python3") {
        return None;
    }
    let mut c = Covers {
        src: Some(src.to_vec()),
        binds: Vec::new(),
    };
    let mut kept: Vec<Vec<u8>> = Vec::new();
    for m in inside {
        let mp = shell::path(&m);
        if kept.iter().any(|k| under(&m, k)) || mp.is_symlink() || !mp.exists() {
            continue;
        }
        let kind = if mp.is_dir() {
            CoverKind::Dir
        } else {
            CoverKind::File
        };
        c.binds.push((kind, cat!(dest, &m[src.len()..])));
        kept.push(m);
    }
    Some(c)
}

/// The walk jail_verify runs, as bash's omabox had it (python, read from stdin).
pub const JAIL_VERIFY_PY: &str = include_str!("jail_verify.py");

/// jail_verify D BOXPID SRC... (#210): whether the box (its PID 1 held) hides everything the jail
/// hides in the folders SRC... it mounts, checked on the mounts themselves, not their paths (the jail
/// can rename the folders above a hidden path while up runs). Err with what is not covered (its
/// words, stdout and stderr as `$(... 2>&1)` has them), or empty when it could not run at all.
pub fn jail_verify(d: &[u8], bp: &[u8], srcs: &[Vec<u8>]) -> Result<(), Vec<u8>> {
    if !crate::exec::executable(b"/usr/bin/python3") {
        return Err(Vec::new());
    }
    // (`stat -c %d:%i`: the file itself, not where a link would lead)
    let id = |f: &str| {
        std::fs::symlink_metadata(shell::path(&cat!(d, "/", f)))
            .map(|m| format!("{}:{}", m.dev(), m.ino()))
    };
    let (Ok(cf), Ok(cd)) = (id("cover-file"), id("cover-dir")) else {
        return Err(Vec::new());
    };
    let Ok((r, w)) = crate::exec::pipe() else {
        return Err(Vec::new());
    };
    let Ok(w2) = w.try_clone() else {
        return Err(Vec::new());
    };
    let os = |v: Vec<u8>| std::ffi::OsString::from_vec(v);
    let mut c = crate::sig::command("timeout");
    c.args(["-k", "2", "30", "/usr/bin/python3", "-I", "-"])
        .arg(os(crate::jail::get_raw("mntpid")))
        .arg(os(crate::jail::get_raw("mntns")))
        .arg(os(bp.to_vec()))
        .arg(cf)
        .arg(cd)
        .arg(os(shell::env_or_empty("OMABOX_JAIL")))
        .args(srcs.iter().map(|s| os(s.clone())))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::from(w))
        .stderr(std::process::Stdio::from(w2));
    let ran = crate::sig::run_child(c, false, |ch| {
        // (the script whole first: python reads it all before it runs any of it)
        if let Some(mut i) = ch.stdin.take() {
            let _ = std::io::Write::write_all(&mut i, JAIL_VERIFY_PY.as_bytes());
        }
        crate::exec::read_all(r, None)
    });
    match ran {
        Ok((_, (0, _))) => Ok(()),
        Ok((out, _)) => Err(strip_nl(&out)),
        Err(_) => Err(Vec::new()),
    }
}

/// The jail's view of paths, for what install.rs follows in it (git dirs, an --omarchy tree).
pub struct Jail;

impl crate::install::JailView for Jail {
    fn path(&self, p: &[u8]) -> Option<Vec<u8>> {
        jail_path(p)
    }
    fn readable(&self, p: &[u8]) -> bool {
        jail_readable(p)
    }
    fn read(&self, p: &[u8]) -> Option<Vec<u8>> {
        jail_read(p)
    }
    fn sees(&self, p: &[u8]) -> Option<Vec<u8>> {
        jail_sees(p)
    }
    fn root_covered(&self, p: &[u8]) -> Option<Vec<u8>> {
        jail_root_covered(p)
    }
}

/// The jail's view when this caller is jailed, else None (the host's paths, as they are).
pub fn view() -> Option<&'static dyn crate::install::JailView> {
    if crate::jail::in_jail() {
        Some(&Jail)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy_of(text: &str) -> Value {
        json::values(text.as_bytes()).0.remove(0)
    }

    fn tmp(n: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("omabox-{n}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn paths_under() {
        assert!(under(b"/a/b", b"/a"));
        assert!(!under(b"/ab", b"/a"));
        assert!(!under(b"/a", b"/a"));
        assert_eq!(dir_of(b"/a/b/c"), b"/a/b");
        assert_eq!(dir_of(b"/a"), b"");
    }

    #[test]
    fn deep_hidden_masked_and_covers() {
        let d = tmp("jcov");
        let p = d.join("proj");
        for x in ["cfg", "secrets/in", "a", "plain"] {
            std::fs::create_dir_all(p.join(x)).unwrap();
        }
        for x in [".env", "cfg/key", "secrets/in/k", "a/.env", "README"] {
            std::fs::write(p.join(x), "x").unwrap();
        }
        std::os::unix::fs::symlink(".env", p.join("envlink")).unwrap();
        let ps = p.to_str().unwrap();
        let pb = ps.as_bytes();
        let pol = format!(
            r#"{{"id":"1 2","roots":[{{"path":"{ps}","masked":true}}],"masks":["{ps}/.env","{ps}/cfg/key","{ps}/secrets","{ps}/secrets/in/k","{ps}/a/.env","{ps}/gone/.env","{ps}/envlink"]}}"#
        );
        {
            let pol = policy_of(&pol);
            assert!(deep_in(&pol, &cat!(pb, "/README")));
            assert!(readable_in(&pol, &cat!(pb, "/README")));
            assert!(!readable_in(&pol, &cat!(pb, "/.env")));
            assert!(!readable_in(&pol, &cat!(pb, "/secrets/in/k")));
            assert!(!readable_in(&pol, &cat!(pb, "/plain/x")));
            assert!(masked_in(&pol, &cat!(pb, "/secrets/in")));
            assert!(masked_in(&pol, pb));
            assert_eq!(root_covered_in(&pol, pb), Some(pb.to_vec()));
            if crate::exec::executable(b"/usr/bin/python3") {
                let c = cover_in(&pol, pb, b"/dest").unwrap();
                assert_eq!(c.src.as_deref(), Some(pb));
                assert_eq!(
                    c.binds,
                    vec![
                        (CoverKind::File, b"/dest/.env".to_vec()),
                        (CoverKind::File, b"/dest/a/.env".to_vec()),
                        (CoverKind::File, b"/dest/cfg/key".to_vec()),
                        (CoverKind::Dir, b"/dest/secrets".to_vec()),
                    ]
                );
            }
            // A folder inside, where something hidden could be moved in: none.
            assert_eq!(cover_in(&pol, &cat!(pb, "/plain"), b"/x"), None);
            assert_eq!(
                path_in(&pol, &cat!(pb, "/a/../README")),
                Some(cat!(pb, "/README"))
            );
        }
        let one = format!(r#"{{"roots":[{{"path":"{ps}","masked":true}}],"masks":["{ps}/.env"]}}"#);
        {
            let pol = policy_of(&one);
            assert!(!deep_in(&pol, &cat!(pb, "/README")));
            assert_eq!(
                cover_in(&pol, &cat!(pb, "/plain"), b"/x"),
                Some(Covers::default())
            );
        }
        let whole = format!(r#"{{"roots":[{{"path":"{ps}","masked":true}}],"masks":["{ps}"]}}"#);
        {
            let pol = policy_of(&whole);
            assert_eq!(root_covered_in(&pol, pb), None);
            assert_eq!(cover_in(&pol, pb, b"/x"), None);
        }
        let old = format!(r#"{{"roots":[{{"path":"{ps}","masked":true}}]}}"#);
        {
            let pol = policy_of(&old);
            assert!(hidden_in(&pol, &cat!(pb, "/README")));
            assert!(masked_in(&pol, &cat!(pb, "/README")));
            assert_eq!(root_covered_in(&pol, pb), None);
            assert_eq!(cover_in(&pol, pb, b"/x"), None);
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn path_stays_inside() {
        let d = tmp("jpath");
        let (p, o) = (d.join("proj"), d.join("out"));
        std::fs::create_dir_all(p.join("th")).unwrap();
        std::fs::create_dir_all(o.join("back")).unwrap();
        std::fs::write(p.join("f"), "x").unwrap();
        std::fs::write(o.join("f"), "s").unwrap();
        std::os::unix::fs::symlink(&o, p.join("lout")).unwrap();
        std::os::unix::fs::symlink(p.join("f"), p.join("fin")).unwrap();
        std::os::unix::fs::symlink(p.join("f"), o.join("back/f")).unwrap();
        let ps = p.to_str().unwrap();
        let pol = format!(r#"{{"roots":[{{"path":"{ps}","masked":false}}],"masks":[]}}"#);
        let (pb, ob, db) = (
            ps.as_bytes(),
            o.to_str().unwrap().as_bytes(),
            d.to_str().unwrap().as_bytes(),
        );
        {
            let pol = policy_of(&pol);
            assert_eq!(path_in(&pol, &cat!(pb, "/f")), Some(cat!(pb, "/f")));
            assert_eq!(path_in(&pol, &cat!(pb, "/none")), Some(cat!(pb, "/none")));
            assert_eq!(path_in(&pol, &cat!(pb, "/fin")), Some(cat!(pb, "/f")));
            assert_eq!(path_in(&pol, &cat!(ob, "/f")), None);
            assert_eq!(path_in(&pol, &cat!(pb, "/lout/f")), None);
            assert_eq!(path_in(&pol, &cat!(pb, "/../out/f")), None);
            assert_eq!(path_in(&pol, db), None);
            assert_eq!(path_in(&pol, &cat!(pb, "/lout/back/f")), None);
            assert_eq!(read_in(&pol, &cat!(pb, "/fin")), Some(b"x".to_vec()));
            assert_eq!(read_in(&pol, &cat!(pb, "/lout/f")), None);
            assert_eq!(sees_in(&pol, &cat!(pb, "/fin")), Some(cat!(pb, "/f")));
            assert_eq!(sees_in(&pol, &cat!(pb, "/lout/f")), None);
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// The IN_OPEN events inotify saw on a watch W of fd IN.
    fn opens_seen(fd: i32) -> usize {
        let mut buf = [0u8; 4096];
        // SAFETY: a read into buf of ours, from a non-blocking inotify fd.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        let n = usize::try_from(n).unwrap_or(0);
        let (mut i, mut seen) = (0, 0);
        let head = std::mem::size_of::<libc::inotify_event>();
        while i + head <= n {
            // SAFETY: a whole event header inside what read gave.
            let ev: libc::inotify_event =
                unsafe { std::ptr::read_unaligned(buf[i..].as_ptr().cast()) };
            if ev.mask & libc::IN_OPEN != 0 {
                seen += 1;
            }
            i += head + ev.len as usize;
        }
        seen
    }

    // #367: jail_read looks at nothing outside the jail's folders: a link inside that leads out is
    // refused before anything there is opened (it opened the file, then checked where it was); a
    // file inside bigger than READ_MAX is refused, never read whole; a FIFO inside is not opened.
    #[test]
    fn read_opens_nothing_outside_and_is_bounded() {
        let d = tmp("jread");
        let (p, o) = (d.join("proj"), d.join("out"));
        std::fs::create_dir_all(&p).unwrap();
        std::fs::create_dir_all(&o).unwrap();
        std::fs::write(o.join("f"), "s").unwrap();
        std::os::unix::fs::symlink(o.join("f"), p.join("lout")).unwrap();
        let big = vec![b'x'; usize::try_from(READ_MAX).unwrap() + 1];
        std::fs::write(p.join("big"), &big).unwrap();
        std::fs::write(p.join("max"), &big[1..]).unwrap();
        let fifo = std::ffi::CString::new(p.join("fifo").to_str().unwrap()).unwrap();
        // SAFETY: a FIFO made at a path of ours.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let ps = p.to_str().unwrap();
        let pb = ps.as_bytes();
        let pol = policy_of(&format!(
            r#"{{"roots":[{{"path":"{ps}","masked":false}}],"masks":[]}}"#
        ));
        // SAFETY: an inotify fd of ours, watching two files of ours for opens.
        let ino = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        assert!(ino >= 0);
        for w in [o.join("f"), p.join("fifo")] {
            let c = std::ffi::CString::new(w.to_str().unwrap()).unwrap();
            // SAFETY: as above.
            assert!(unsafe { libc::inotify_add_watch(ino, c.as_ptr(), libc::IN_OPEN) } >= 0);
        }
        assert_eq!(read_in(&pol, &cat!(pb, "/lout")), None);
        assert_eq!(read_in(&pol, &cat!(pb, "/fifo")), None);
        assert_eq!(
            opens_seen(ino),
            0,
            "a file outside, or the FIFO, was opened"
        );
        assert_eq!(read_in(&pol, &cat!(pb, "/big")), None, "past READ_MAX");
        assert_eq!(
            read_in(&pol, &cat!(pb, "/max")).map(|v| v.len()),
            Some(big.len() - 1)
        );
        // SAFETY: our inotify fd, closed once.
        unsafe { libc::close(ino) };
        std::fs::remove_dir_all(&d).unwrap();
    }
}
