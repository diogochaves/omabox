//! A desk box's copy of the user's desktop (#321 step 2, finding 701): their Hyprland config, the
//! config and state of each desk shell found (deskshell's profiles), and the wallpapers they show
//! (a profile's `images` keys, the config's swww/swaybg/hyprpaper commands and hyprpaper.conf:
//! nothing else a copied file names, finding 710), copied into the box's HOME before --seed's
//! (which win). copy says what it did as
//! seed_home's DESK_FILES lines, which Report reads back for box.json's desk_copy, shell_guess and
//! shell_from.
//!
//! What it follows and what stays out: every link is followed (stow, home-manager: even
//! ~/.config/hypr itself a link), its target through refuse_src (the secret stores, omabox's own
//! folders, ~/.config/omarchy but plugins/ and themes/, HOME itself) and the name check on each part
//! of its path; what it leads to is copied as a file or folder of the box's own, so nothing in the
//! box HOME leads back to the host. A link into /usr stays a link (the box has /usr). A folder met
//! again below itself (dev:ino) is a loop, and 8 folders deep is the end. Each file is read from its
//! folder held open (seed::copy_file_in), a desk box's looser name check
//! (seed::desk_box_name_refused) and the content check on the copy (seed::desk_content_refused;
//! images by their magic skip it). Caps per part and in all (the CAPS below), each skip said; a part
//! stops at its cap, the rest not walked, said in one line, and the whole copy at 30 s (finding 710).
//! What the host reads or edits in the box HOME after the copy (a profile's `images` file, its
//! `idle` edit) is opened from its folders held open, no link followed (a save's may be there).

use crate::cat;
use crate::deskshell::{self, Profile};
use crate::exit::{Ctx, Res, say};
use crate::repo::{readlink_e, refuse_src};
use crate::seed::{self, Dest, SeedPlan, Src};
use crate::shell;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;

const MIB: u64 = 1 << 20;

/// A part's caps: each file, and all its files' bytes and count.
#[derive(Clone, Copy, Debug)]
struct Caps {
    file: u64,
    bytes: u64,
    count: u64,
}

/// The Hyprland config's tree.
const HYPR: Caps = Caps {
    file: MIB,
    bytes: 8 * MIB,
    count: 500,
};
/// Each desk shell's config, state and the caches its profile lists.
const SHELL: Caps = Caps {
    file: MIB,
    bytes: 16 * MIB,
    count: 1000,
};
/// The images the copied files name by path.
const IMAGES: Caps = Caps {
    file: 32 * MIB,
    bytes: 128 * MIB,
    count: 32,
};
/// The whole copy.
const ALL: u64 = 160 * MIB;
/// How long the whole copy may take (finding 710: a huge or slow tree ends the walk, said).
const COPY_TIME: std::time::Duration = std::time::Duration::from_secs(30);
/// How many entries (files, folders, links, each one looked at) a part looks at, per file it may
/// copy: a tree of empty folders ends there too.
const ENTRIES_PER_FILE: u64 = 4;
/// How many names of one folder are read.
const NAMES: usize = 10_000;
/// How many folders deep a walk goes.
const DEPTH: u32 = 8;

#[cfg(test)]
thread_local! {
    /// A test's own COPY_TIME.
    static COPY_TIME_TEST: std::cell::Cell<Option<std::time::Duration>> =
        const { std::cell::Cell::new(None) };
}

fn copy_time() -> std::time::Duration {
    #[cfg(test)]
    if let Some(t) = COPY_TIME_TEST.with(std::cell::Cell::get) {
        return t;
    }
    COPY_TIME
}

fn p(v: &[u8]) -> std::path::PathBuf {
    shell::path(v)
}

/// `${VAR:-HOME/DEFAULT}`, VAR taken only when absolute (as the XDG spec says).
fn xdg(var: &str, home: &[u8], dflt: &str) -> Vec<u8> {
    let x = shell::env_or_empty(var);
    if x.starts_with(b"/") {
        x
    } else {
        cat!(home, "/", dflt)
    }
}

/// Where REL (a path in the box HOME) is on the host: under the user's XDG dir for .config,
/// .local/state, .local/share and .cache (the box's are those under its HOME), else under HOME.
fn host_of(home: &[u8], rel: &[u8]) -> Vec<u8> {
    for (pre, var) in [
        (".config", "XDG_CONFIG_HOME"),
        (".local/state", "XDG_STATE_HOME"),
        (".local/share", "XDG_DATA_HOME"),
        (".cache", "XDG_CACHE_HOME"),
    ] {
        if let Some(rest) = rel.strip_prefix(pre.as_bytes()) {
            if rest.is_empty() || rest[0] == b'/' {
                return cat!(xdg(var, home, pre), rest);
            }
        }
    }
    cat!(home, "/", rel)
}

/// An image by its first bytes: PNG, JPEG, GIF, WebP.
pub(crate) fn is_image(f: &[u8]) -> bool {
    use std::io::Read;
    let mut b = [0u8; 12];
    let n = std::fs::File::open(p(f))
        .and_then(|mut x| x.read(&mut b))
        .unwrap_or(0);
    let b = &b[..n];
    b.starts_with(b"\x89PNG\r\n\x1a\n")
        || b.starts_with(b"\xff\xd8\xff")
        || b.starts_with(b"GIF87a")
        || b.starts_with(b"GIF89a")
        || (b.len() == 12 && b.starts_with(b"RIFF") && &b[8..12] == b"WEBP")
}

/// An image's name, as the wallpaper pickers take one: not hidden, .jpg .jpeg .png .gif .bmp .webp
/// in any case.
fn pic_name(n: &[u8]) -> bool {
    let l = n.to_ascii_lowercase();
    n.first().is_some_and(|&c| c != b'.')
        && [".jpg", ".jpeg", ".png", ".gif", ".bmp", ".webp"]
            .iter()
            .any(|e| l.len() > e.len() && l.ends_with(e.as_bytes()))
}

/// Why the path T (resolved) stays out by a name in it: each part below HOME (as written, or
/// HOME_R, resolved), or each part when it is not in HOME.
fn parts_why(home: &[u8], home_r: &[u8], t: &[u8]) -> Option<&'static str> {
    let under = |h: &[u8]| t.strip_prefix(cat!(h, "/").as_slice()).map(<[u8]>::to_vec);
    let rel = under(home_r)
        .or_else(|| under(home))
        .unwrap_or_else(|| t.to_vec());
    rel.split(|&c| c == b'/')
        .filter(|s| !s.is_empty())
        .find_map(seed::desk_box_name_refused)
}

/// Why E, a link (or a top folder) leading to TO, is not followed: refuse_src, or a name on its
/// way.
pub(crate) fn follow_why(home: &[u8], home_r: &[u8], e: &[u8], to: &[u8]) -> Option<Vec<u8>> {
    if let Some(w) = refuse_src(e) {
        return Some(cat!("it leads to ", to, ": ", w));
    }
    parts_why(home, home_r, to).map(|w| cat!("it leads to ", to, ": ", w))
}

/// precheck: what copy refuses (up fails), asked before the box's folder is made (deskguard::checks,
/// finding 709): a Hyprland config folder refuse_src keeps out, or a hyprland.lua or a module it
/// requires (and they require) that the copy would leave out (a link refused on its way, a secret's
/// name, over 1 MiB, what looks like a key in it: read on the host, read-only). The same words as
/// copy's own refusals, which stay for a file changed in between.
pub fn precheck(ctx: &Ctx) -> Res<()> {
    let home = shell::env_or_empty("HOME");
    let home_r = readlink_e(&home).unwrap_or_else(|| home.clone());
    let hypr = cat!(xdg("XDG_CONFIG_HOME", &home, ".config"), "/hypr");
    if std::fs::symlink_metadata(p(&hypr)).is_err() {
        return Ok(());
    }
    let refused = |why: &[u8]| {
        ctx.die(cat!(
            "up: --desktop desk: your Hyprland config (",
            hypr,
            ") cannot go into a box: ",
            why
        ))
    };
    let Some(t) = readlink_e(&hypr) else {
        return Err(refused(b"a link that leads nowhere"));
    };
    if let Some(why) = follow_why(&home, &home_r, &hypr, &t) {
        return Err(refused(&why));
    }
    // (REL in ~/.config/hypr; whether it is hyprland.lua's module, and so may be missing)
    let mut todo: Vec<(Vec<u8>, bool)> = vec![(b"hyprland.lua".to_vec(), false)];
    let mut seen: HashSet<Vec<u8>> = HashSet::new();
    while let Some((rel, module)) = todo.pop() {
        if !seen.insert(rel.clone()) {
            continue;
        }
        let src = cat!(t, "/", rel);
        if std::fs::symlink_metadata(p(&src)).is_err() {
            continue;
        }
        let why: Option<Vec<u8>> =
            if let Some(w) = rel
                .split(|&c| c == b'/')
                .find_map(seed::desk_box_name_refused)
            {
                Some(w.as_bytes().to_vec())
            } else {
                match readlink_e(&src) {
                    None => Some(b"a link that leads nowhere".to_vec()),
                    Some(to) => follow_why(&home, &home_r, &src, &to).or_else(|| {
                        match std::fs::metadata(p(&to)) {
                            Ok(m) if !m.is_file() => Some(b"not a regular file".to_vec()),
                            Ok(m) if m.len() > HYPR.file => Some(b"over 1 MiB".to_vec()),
                            Ok(_) => seed::desk_content_refused(&to).map(|w| w.as_bytes().to_vec()),
                            Err(_) => Some(b"a link that leads nowhere".to_vec()),
                        }
                    }),
                }
            };
        if let Some(why) = why {
            return Err(ctx.die(if module {
                cat!(
                    "up: --desktop desk: ~/.config/hypr/",
                    rel,
                    ", which your Hyprland config requires, was not copied into the box: ",
                    why
                )
            } else {
                cat!(
                    "up: --desktop desk: your hyprland.lua was not copied into the box: ",
                    why
                )
            }));
        }
        for m in requires(&std::fs::read(p(&src)).unwrap_or_default()) {
            if clean_rel(&m) {
                todo.push((cat!(m, ".lua"), true));
                todo.push((cat!(m, "/init.lua"), true));
            }
        }
    }
    Ok(())
}

/// A raw fd of ours, closed when dropped.
struct Fd(libc::c_int);

impl Drop for Fd {
    fn drop(&mut self) {
        // SAFETY: an fd this module opened and owns.
        unsafe { libc::close(self.0) };
    }
}

fn cstr(v: &[u8]) -> Option<std::ffi::CString> {
    std::ffi::CString::new(v).ok()
}

/// Whether NAME in the folder DIR holds is a link (lstat's, never followed).
fn is_link_at(dir: &Fd, name: &std::ffi::CStr) -> bool {
    // SAFETY: an fstatat into a zeroed stat of ours; DIR open, NAME NUL-terminated.
    unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        libc::fstatat(dir.0, name.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) == 0
            && st.st_mode & libc::S_IFMT == libc::S_IFLNK
    }
}

/// The folder REL's file is in (REL a clean path in the box HOME H), held open, and its last name:
/// H opened, then each folder on the way from the one above it, O_NOFOLLOW|O_DIRECTORY, so a link
/// anywhere on the way (a save's, which up restored before the copy) is never followed (finding
/// 710). Ok(None) when a folder on the way is not there; Err(~/PATH) of the first link met.
fn held_parent(h: &[u8], rel: &[u8]) -> Result<Option<(Fd, std::ffi::CString)>, Vec<u8>> {
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    let Some(hc) = cstr(h) else {
        return Ok(None);
    };
    // (H itself is omabox's: the box dir's `home`, a link to the HOME's folder, which no box can
    // change; followed. Only what is below it is the box's.)
    // SAFETY: open of a NUL-terminated path; the fd is owned by Fd from here.
    let fd = unsafe {
        libc::open(
            hc.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Ok(None);
    }
    let mut dir = Fd(fd);
    let parts: Vec<&[u8]> = rel.split(|&c| c == b'/').collect();
    let Some((last, way)) = parts.split_last() else {
        return Ok(None);
    };
    for (i, part) in way.iter().enumerate() {
        let Some(pc) = cstr(part) else {
            return Ok(None);
        };
        // SAFETY: openat from a folder fd of ours; the new fd is owned by Fd.
        let fd = unsafe { libc::openat(dir.0, pc.as_ptr(), flags) };
        if fd < 0 {
            if is_link_at(&dir, &pc) {
                return Err(cat!("~/", parts[..=i].join(&b'/')));
            }
            return Ok(None);
        }
        dir = Fd(fd);
    }
    Ok(cstr(last).map(|n| (dir, n)))
}

/// idle_edit H LINE: a profile's `idle FILE KEY VALUE` (#321 step 5, finding 709) on the box's copy
/// of FILE (a JSON file in the box HOME H, a file of its own: the copy's, never a link): KEY, dotted,
/// set to VALUE (JSON; a word that is none as a string), the folders on its way made objects. So the
/// shell's own idle timeouts never lock or blank the box (the idle hold covers the shells that heed
/// an inhibitor). Ok(Some(FILE)) when it was edited; Ok(None) when the box HOME has no such file, or
/// it is no JSON object; Err(link) when a link is on its way or is the file (finding 710: the box
/// HOME may hold a save's links and the edit is the host's, so none is followed: each folder is
/// opened from the one above, the temp file made anew beside the file, O_EXCL, and renamed there).
fn idle_edit(h: &[u8], line: &[u8]) -> Result<Option<Vec<u8>>, Vec<u8>> {
    use std::io::{Read, Write};
    use std::os::fd::FromRawFd;
    let mut it = line
        .splitn(3, |c| c.is_ascii_whitespace())
        .map(<[u8]>::trim_ascii);
    let (Some(file), Some(key), Some(val)) = (it.next(), it.next(), it.next()) else {
        return Ok(None);
    };
    if !clean_rel(file) || key.is_empty() || val.is_empty() {
        return Ok(None);
    }
    let Some((dir, name)) = held_parent(h, file)? else {
        return Ok(None);
    };
    if is_link_at(&dir, &name) {
        return Err(cat!("~/", file));
    }
    let rd =
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY;
    // SAFETY: openat from a folder fd of ours.
    let fd = unsafe { libc::openat(dir.0, name.as_ptr(), rd) };
    if fd < 0 {
        return Ok(None);
    }
    // SAFETY: FD was just opened and nothing else owns it.
    let mut f = unsafe { std::fs::File::from_raw_fd(fd) };
    let Ok(m) = f.metadata() else {
        return Ok(None);
    };
    if !m.is_file() || m.len() > SHELL.file {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    if (&mut f)
        .take(SHELL.file + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return Ok(None);
    }
    let Ok(mut doc) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(None);
    };
    let new: Value = serde_json::from_slice(val)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(val).into_owned()));
    let keys: Vec<String> = String::from_utf8_lossy(key)
        .split('.')
        .map(str::to_owned)
        .collect();
    let mut at = &mut doc;
    for (i, k) in keys.iter().enumerate() {
        let Some(o) = at.as_object_mut() else {
            return Ok(None);
        };
        if i + 1 == keys.len() {
            o.insert(k.clone(), new);
            break;
        }
        let next = o
            .entry(k.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        if !next.is_object() {
            *next = Value::Object(Map::new());
        }
        at = next;
    }
    let Ok(text) = serde_json::to_vec_pretty(&doc) else {
        return Ok(None);
    };
    let Some(tmp) = cstr(&cat!(name.as_bytes(), ".omabox-idle")) else {
        return Ok(None);
    };
    // (whatever a save left at the temp name goes, the entry itself: a link removed, not followed)
    // SAFETY: unlinkat from a folder fd of ours, a NUL-terminated name.
    unsafe { libc::unlinkat(dir.0, tmp.as_ptr(), 0) };
    let wr = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    // SAFETY: openat from a folder fd of ours; O_EXCL makes it a new file, never a link's target.
    let fd = unsafe { libc::openat(dir.0, tmp.as_ptr(), wr, 0o644 as libc::c_uint) };
    if fd < 0 {
        return Ok(None);
    }
    // SAFETY: FD was just opened and nothing else owns it.
    let mut out = unsafe { std::fs::File::from_raw_fd(fd) };
    let written = out.write_all(&text).is_ok();
    drop(out);
    // SAFETY: renameat within the one folder fd of ours, NUL-terminated names.
    let renamed =
        written && unsafe { libc::renameat(dir.0, tmp.as_ptr(), dir.0, name.as_ptr()) } == 0;
    if !renamed {
        // SAFETY: as above.
        unsafe { libc::unlinkat(dir.0, tmp.as_ptr(), 0) };
        return Ok(None);
    }
    Ok(Some(file.to_vec()))
}

/// The walk: what it copied and skipped, its caps.
struct Walk<'a> {
    ctx: &'a Ctx,
    h: &'a [u8],
    /// The user's HOME as written, and resolved.
    home: Vec<u8>,
    home_r: Vec<u8>,
    out: Vec<u8>,
    caps: Caps,
    /// The part in words, for a cap's skip and the summary.
    what: Vec<u8>,
    g_bytes: u64,
    g_count: u64,
    all: u64,
    /// Each part's (what, files copied), for the summary.
    parts: Vec<(Vec<u8>, u64)>,
    /// The paths in the box HOME copied that are no image (read for the paths they name).
    texts: Vec<Vec<u8>>,
    /// Each path said once (copied or skipped); a skipped one's why.
    seen: HashSet<Vec<u8>>,
    skipped: HashMap<Vec<u8>, Vec<u8>>,
    nskipped: Vec<Vec<u8>>,
    /// Where (the folder, in the box HOME) and why the part stopped at a cap (finding 710): the rest
    /// of it is counted in UNWALKED, not walked, and said in one line at its end.
    stop: Option<(Vec<u8>, Vec<u8>)>,
    unwalked: u64,
    /// The entries the part looked at.
    entries: u64,
    /// Why every part from here on stops (the whole copy's cap, its time).
    stop_all: Option<Vec<u8>>,
    deadline: std::time::Instant,
}

/// One folder's names (at most NAMES), in byte order; PIC: a picture's names only.
fn names_of(dir: &[u8], pic: bool) -> Vec<Vec<u8>> {
    let mut v: Vec<Vec<u8>> = std::fs::read_dir(p(dir))
        .map(|rd| {
            rd.flatten()
                .map(|e| shell::bytes(std::path::Path::new(&e.file_name())).to_vec())
                .filter(|n| !pic || pic_name(n))
                .take(NAMES)
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

impl Walk<'_> {
    fn new<'a>(ctx: &'a Ctx, h: &'a [u8], home: Vec<u8>, home_r: Vec<u8>) -> Walk<'a> {
        Walk {
            ctx,
            h,
            home,
            home_r,
            out: Vec::new(),
            caps: HYPR,
            what: Vec::new(),
            g_bytes: 0,
            g_count: 0,
            all: 0,
            parts: Vec::new(),
            texts: Vec::new(),
            seen: HashSet::new(),
            skipped: HashMap::new(),
            nskipped: Vec::new(),
            stop: None,
            unwalked: 0,
            entries: 0,
            stop_all: None,
            deadline: std::time::Instant::now() + copy_time(),
        }
    }

    fn part(&mut self, what: Vec<u8>, caps: Caps) {
        self.end_part();
        self.what = what;
        self.caps = caps;
        self.g_bytes = 0;
        self.g_count = 0;
        self.entries = 0;
        if let Some(why) = self.stop_all.clone() {
            self.stop = Some((Vec::new(), why));
        }
    }

    fn end_part(&mut self) {
        if let Some((dir, why)) = self.stop.take() {
            if self.unwalked > 0 {
                let at = if dir.is_empty() {
                    cat!("(", self.what, ")")
                } else {
                    cat!(dir, "/...")
                };
                let why = cat!(self.unwalked.to_string(), " more not walked: ", why);
                self.out.extend(cat!("skipped ", at, "\t", why, "\n"));
                self.skipped.insert(at.clone(), why.clone());
                self.nskipped.push(cat!(at, "\t", why));
            }
        }
        self.unwalked = 0;
        if !self.what.is_empty() {
            self.parts
                .push((std::mem::take(&mut self.what), self.g_count));
        }
    }

    /// The part stops at R (an entry in the box HOME) for WHY; R and the rest are counted.
    fn stop_at(&mut self, r: &[u8], why: Vec<u8>) {
        if self.stop.is_none() {
            let dir = r
                .iter()
                .rposition(|&c| c == b'/')
                .map_or_else(|| r.to_vec(), |i| r[..i].to_vec());
            self.stop = Some((dir, why));
        }
        self.unwalked += 1;
    }

    /// Whether the walk stops before R: the part stopped already (R counted), its entries' cap met,
    /// or the copy's time up (every later part stops too).
    fn halted(&mut self, r: &[u8]) -> bool {
        if self.stop.is_some() {
            self.unwalked += 1;
            return true;
        }
        let cap = self.caps.count * ENTRIES_PER_FILE;
        if self.entries >= cap {
            self.stop_at(
                r,
                format!(
                    "over the cap for {}: {cap} entries",
                    String::from_utf8_lossy(&self.what)
                )
                .into_bytes(),
            );
            return true;
        }
        if std::time::Instant::now() > self.deadline {
            let why = format!("the copy took over {} s", copy_time().as_secs()).into_bytes();
            self.stop_all = Some(why.clone());
            self.stop_at(r, why);
            return true;
        }
        self.entries += 1;
        false
    }

    fn skip(&mut self, r: &[u8], why: &[u8]) {
        if self.seen.insert(r.to_vec()) {
            self.out.extend(cat!("skipped ", r, "\t", why, "\n"));
            self.skipped.insert(r.to_vec(), why.to_vec());
            self.nskipped.push(cat!(r, "\t", why));
        }
    }

    fn follow_why(&self, e: &[u8], to: &[u8]) -> Option<Vec<u8>> {
        follow_why(&self.home, &self.home_r, e, to)
    }

    /// A top folder (or file) the copy starts from: SRC on the host, REL in the box HOME. Nothing
    /// when SRC is not there; Err(why) when it may not be copied.
    fn root(&mut self, src: &[u8], rel: &[u8]) -> Res<Result<(), Vec<u8>>> {
        if std::fs::symlink_metadata(p(src)).is_err() {
            return Ok(Ok(()));
        }
        // (REL/: a part that stops here says REL/...)
        if self.halted(&cat!(rel, "/")) {
            return Ok(Ok(()));
        }
        let Some(t) = readlink_e(src) else {
            return Ok(Err(b"a link that leads nowhere".to_vec()));
        };
        if let Some(w) = self.follow_why(src, &t) {
            return Ok(Err(w));
        }
        let Ok(m) = std::fs::metadata(p(&t)) else {
            return Ok(Ok(()));
        };
        if m.is_dir() {
            let Some(d) = Dest::new(rel) else {
                return Ok(Ok(()));
            };
            seed::home_unlink(self.ctx, self.h, &d)?;
            let _ = std::fs::create_dir_all(p(&cat!(self.h, "/", rel)));
            let mut up = vec![(m.dev(), m.ino())];
            self.walk(&t, rel, 0, &mut up)?;
        } else if m.is_file() {
            self.file(&t, rel, m.len())?;
        }
        Ok(Ok(()))
    }

    /// The entries of DIR (resolved), at REL in the box HOME, in byte order.
    fn walk(&mut self, dir: &[u8], rel: &[u8], depth: u32, up: &mut Vec<(u64, u64)>) -> Res<()> {
        let all = names_of(dir, false);
        let n = all.len();
        for (i, name) in all.into_iter().enumerate() {
            if self.stop.is_some() {
                // (the rest of this folder counted, not walked: each folder above counts its own)
                self.unwalked += (n - i) as u64;
                break;
            }
            self.entry(dir, &name, rel, depth, up)?;
        }
        Ok(())
    }

    fn entry(
        &mut self,
        dir: &[u8],
        name: &[u8],
        rel: &[u8],
        depth: u32,
        up: &mut Vec<(u64, u64)>,
    ) -> Res<()> {
        let e = cat!(dir, "/", name);
        let r = cat!(rel, "/", name);
        if self.halted(&r) {
            return Ok(());
        }
        let Ok(lm) = std::fs::symlink_metadata(p(&e)) else {
            return Ok(());
        };
        if let Some(why) = seed::desk_box_name_refused(name) {
            let r = if lm.is_dir() { cat!(r, "/") } else { r };
            self.skip(&r, why.as_bytes());
            return Ok(());
        }
        let t = if lm.file_type().is_symlink() {
            let Some(to) = readlink_e(&e) else {
                self.skip(&r, b"a link that leads nowhere");
                return Ok(());
            };
            if to == b"/usr" || to.starts_with(b"/usr/") {
                // (the box has /usr: the link stays one)
                if let Some(d) = Dest::new(&r) {
                    seed::home_link(self.ctx, self.h, &d, &to)?;
                    self.seen.insert(r.clone());
                    self.out.extend(cat!("copied ", r, "\n"));
                }
                return Ok(());
            }
            if let Some(why) = self.follow_why(&e, &to) {
                self.skip(&r, &why);
                return Ok(());
            }
            to
        } else {
            e
        };
        let Ok(m) = std::fs::metadata(p(&t)) else {
            self.skip(&r, b"a link that leads nowhere");
            return Ok(());
        };
        if m.is_dir() {
            let id = (m.dev(), m.ino());
            if up.contains(&id) {
                self.skip(&cat!(r, "/"), b"a loop: a link to a folder it is in");
            } else if depth >= DEPTH {
                self.skip(&cat!(r, "/"), b"more than 8 folders deep");
            } else if let Some(d) = Dest::new(&r) {
                seed::home_unlink(self.ctx, self.h, &d)?;
                let _ = std::fs::create_dir_all(p(&cat!(self.h, "/", r)));
                up.push(id);
                self.walk(&t, &r, depth + 1, up)?;
                up.pop();
            }
        } else if m.is_file() {
            self.file(&t, &r, m.len())?;
        } else {
            self.skip(&r, b"not a regular file");
        }
        Ok(())
    }

    /// One file, T (resolved), at R in the box HOME, SIZE bytes: copied, or skipped saying why.
    fn file(&mut self, t: &[u8], r: &[u8], size: u64) -> Res<()> {
        if self.seen.contains(r) {
            return Ok(());
        }
        if self.stop.is_some() {
            self.unwalked += 1;
            return Ok(());
        }
        let c = self.caps;
        let over = |n: u64| -> Vec<u8> { format!("over {} MiB", n / MIB).into_bytes() };
        if size > c.file {
            self.skip(r, &over(c.file));
            return Ok(());
        }
        if self.g_bytes + size > c.bytes || self.g_count >= c.count {
            let why = cat!(
                "over the cap for ",
                self.what,
                ": ",
                (c.bytes / MIB).to_string(),
                " MiB and ",
                c.count.to_string(),
                " files"
            );
            self.stop_at(r, why);
            return Ok(());
        }
        if self.all + size > ALL {
            let why = b"over the cap of 160 MiB for the whole copy".to_vec();
            self.stop_all = Some(why.clone());
            self.stop_at(r, why);
            return Ok(());
        }
        let Some(d) = Dest::new(r) else {
            self.skip(r, b"not a path in the box HOME");
            return Ok(());
        };
        if !seed::copy_file_in(self.ctx, self.h, &Src::desk_box(t.to_vec()), &d)? {
            self.skip(r, b"it could not be copied");
            return Ok(());
        }
        let to = cat!(self.h, "/", r);
        let image = is_image(&to);
        let refused = if image {
            None
        } else {
            seed::desk_content_refused(&to)
        };
        let real = std::fs::metadata(p(&to)).map_or(u64::MAX, |m| m.len());
        let why: Option<Vec<u8>> = match refused {
            Some(w) => Some(w.as_bytes().to_vec()),
            // (it grew since it was looked at)
            None if real > c.file => Some(over(c.file)),
            None => None,
        };
        if let Some(why) = why {
            let _ = std::fs::remove_file(p(&to));
            self.skip(r, &why);
            return Ok(());
        }
        self.g_bytes += real;
        self.g_count += 1;
        self.all += real;
        self.seen.insert(r.to_vec());
        if !image {
            self.texts.push(r.to_vec());
        }
        self.out.extend(cat!("copied ", r, "\n"));
        Ok(())
    }

    /// An image (or a folder's top-level images) a copied file names by its path, REL in HOME.
    fn named(&mut self, rel: &[u8], folder: bool) -> Res<()> {
        let src = cat!(self.home, "/", rel);
        let shown = if folder { cat!(rel, "/") } else { rel.to_vec() };
        if self.seen.contains(&shown) || self.halted(&shown) {
            return Ok(());
        }
        if let Some(w) = rel
            .split(|&c| c == b'/')
            .find_map(seed::desk_box_name_refused)
        {
            self.skip(&shown, w.as_bytes());
            return Ok(());
        }
        if std::fs::symlink_metadata(p(&src)).is_err() {
            self.skip(&shown, b"named in your config, not there");
            return Ok(());
        }
        let Some(t) = readlink_e(&src) else {
            self.skip(&shown, b"a link that leads nowhere");
            return Ok(());
        };
        if let Some(why) = self.follow_why(&src, &t) {
            self.skip(&shown, &why);
            return Ok(());
        }
        let Ok(m) = std::fs::metadata(p(&t)) else {
            return Ok(());
        };
        if folder {
            if !m.is_dir() {
                return Ok(());
            }
            self.seen.insert(shown);
            for n in names_of(&t, true) {
                self.named(&cat!(rel, "/", n), false)?;
            }
        } else if m.is_file() {
            self.file(&t, rel, m.len())?;
        } else {
            self.skip(&shown, b"not a regular file");
        }
        Ok(())
    }
}

/// The strings in a Lua line ("..." and '...', a backslash's next character kept), up to a `--`
/// outside them; COMMENT false: the whole line's (a JSON or text file's).
fn strings(line: &[u8], comment: bool) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < line.len() {
        let c = line[i];
        if comment && c == b'-' && line.get(i + 1) == Some(&b'-') {
            break;
        }
        if c == b'"' || c == b'\'' {
            let mut s = Vec::new();
            let mut j = i + 1;
            while j < line.len() && line[j] != c {
                if line[j] == b'\\' && j + 1 < line.len() {
                    s.push(line[j + 1]);
                    j += 2;
                    continue;
                }
                s.push(line[j]);
                j += 1;
            }
            out.push(s);
            i = j + 1;
            continue;
        }
        i += 1;
    }
    out
}

/// A Lua line up to a `--` comment outside its strings.
fn lua_code(line: &[u8]) -> &[u8] {
    let mut q: Option<u8> = None;
    let mut i = 0;
    while i < line.len() {
        let c = line[i];
        match q {
            Some(_) if c == b'\\' => i += 1,
            Some(o) if c == o => q = None,
            Some(_) => {}
            None if c == b'"' || c == b'\'' => q = Some(c),
            None if c == b'-' && line.get(i + 1) == Some(&b'-') => return &line[..i],
            None => {}
        }
        i += 1;
    }
    line
}

/// The modules a Lua text requires (`require("a.b")`, `require 'a'`), as paths: `a/b`.
fn requires(text: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for line in text.split(|&c| c == b'\n') {
        let mut rest = lua_code(line);
        while let Some(k) = rest.windows(7).position(|w| w == b"require") {
            rest = &rest[k + 7..];
            let after: &[u8] = rest.trim_ascii_start();
            let after = after.strip_prefix(b"(").unwrap_or(after).trim_ascii_start();
            if let Some(s) = strings(after, false).first() {
                if after.first().is_some_and(|c| *c == b'"' || *c == b'\'') && !s.is_empty() {
                    out.push(
                        s.iter()
                            .map(|&c| if c == b'.' { b'/' } else { c })
                            .collect(),
                    );
                }
            }
        }
    }
    out
}

/// One path as a desktop's file names it, made absolute: `file://` taken off, `~/`, `$HOME/` and
/// `${HOME}/` as HOME, JSON's `\/` as `/`, a trailing `/` off. None when it is no absolute path.
fn norm_path(c: &[u8], home: &[u8]) -> Option<Vec<u8>> {
    let mut v = Vec::with_capacity(c.len());
    let mut i = 0;
    while i < c.len() {
        if c[i] == b'\\' && c.get(i + 1) == Some(&b'/') {
            i += 1;
        }
        v.push(c[i]);
        i += 1;
    }
    let c = v.trim_ascii();
    let c = c.strip_prefix(b"file://").unwrap_or(c);
    let mut c = if let Some(r) = c.strip_prefix(b"~/") {
        cat!(home, "/", r)
    } else if let Some(r) = c.strip_prefix(b"$HOME/") {
        cat!(home, "/", r)
    } else if let Some(r) = c.strip_prefix(b"${HOME}/") {
        cat!(home, "/", r)
    } else {
        c.to_vec()
    };
    while c.len() > 1 && c.ends_with(b"/") {
        c.pop();
    }
    (c.starts_with(b"/") && !c.contains(&0)).then_some(c)
}

/// The pictures the wallpaper commands of a Hyprland Lua config name (finding 710): in each string
/// of a line (its `--` comment skipped), the path-shaped words after a `swww`, `swaybg` or
/// `hyprpaper` word (`swww img PATH`, `swaybg -i PATH`, `hyprctl hyprpaper wallpaper MON,PATH`).
fn wallpaper_cmd_paths(text: &[u8], home: &[u8]) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    for line in text.split(|&c| c == b'\n') {
        for s in strings(line, true) {
            let mut after = false;
            for word in s
                .split(|c| c.is_ascii_whitespace())
                .filter(|w| !w.is_empty())
            {
                let base = word.rsplit(|&c| c == b'/').next().unwrap_or_default();
                if matches!(base, b"swww" | b"swaybg" | b"hyprpaper") {
                    after = true;
                    continue;
                }
                if !after {
                    continue;
                }
                for part in word.split(|&c| c == b',' || c == b'=') {
                    let q = |c: &u8| *c == b'"' || *c == b'\'';
                    let a = part.iter().position(|c| !q(c)).unwrap_or(part.len());
                    let b = part.iter().rposition(|c| !q(c)).map_or(a, |i| i + 1);
                    let part = &part[a..b.max(a)];
                    if let Some(x) = norm_path(part, home).filter(|x| !out.contains(x)) {
                        out.push(x);
                    }
                }
            }
        }
    }
    out
}

/// hyprpaper.conf's pictures: `preload = PATH`, `wallpaper = MON,PATH`, and its 0.8 blocks'
/// `path = PATH`; `#` comments skipped.
fn hyprpaper_paths(text: &[u8], home: &[u8]) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    for line in text.split(|&c| c == b'\n') {
        let line = line.split(|&c| c == b'#').next().unwrap_or_default();
        let Some(k) = line.iter().position(|&c| c == b'=') else {
            continue;
        };
        let (key, val) = (line[..k].trim_ascii(), line[k + 1..].trim_ascii());
        let val = match key {
            b"preload" | b"path" => val,
            b"wallpaper" => val.splitn(2, |&c| c == b',').nth(1).unwrap_or(val),
            _ => continue,
        };
        if let Some(x) = norm_path(val, home).filter(|x| !out.contains(x)) {
            out.push(x);
        }
    }
    out
}

/// REL's file in the box HOME H read for the host (at most CAP bytes): opened from its folder held
/// open, no link followed on the way or at it, a regular file only (finding 710).
fn read_in_box(h: &[u8], rel: &[u8], cap: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::os::fd::FromRawFd;
    let (dir, name) = held_parent(h, rel).ok()??;
    let rd =
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY;
    // SAFETY: openat from a folder fd of ours.
    let fd = unsafe { libc::openat(dir.0, name.as_ptr(), rd) };
    if fd < 0 {
        return None;
    }
    // SAFETY: FD was just opened and nothing else owns it.
    let f = unsafe { std::fs::File::from_raw_fd(fd) };
    if !f.metadata().ok()?.is_file() {
        return None;
    }
    let mut v = Vec::new();
    f.take(cap).read_to_end(&mut v).ok()?;
    Some(v)
}

/// The strings in V (a string, or those in an array or object, nested), at most 64.
fn json_strings(v: &Value, out: &mut Vec<Vec<u8>>) {
    if out.len() >= 64 {
        return;
    }
    match v {
        Value::String(s) => out.push(s.as_bytes().to_vec()),
        Value::Array(a) => a.iter().for_each(|x| json_strings(x, out)),
        Value::Object(o) => o.values().for_each(|x| json_strings(x, out)),
        _ => {}
    }
}

/// A profile's `images FILE [KEY...]` (finding 710) read in the box's copy of FILE in H: the paths
/// each dotted KEY's value names (JSON), or FILE's first line with no KEY.
fn key_paths(h: &[u8], line: &[u8], home: &[u8]) -> Vec<Vec<u8>> {
    let mut words = line
        .split(|c| c.is_ascii_whitespace())
        .filter(|w| !w.is_empty());
    let Some(file) = words.next().filter(|f| clean_rel(f)) else {
        return Vec::new();
    };
    let keys: Vec<&[u8]> = words.collect();
    let Some(text) = read_in_box(h, file, SHELL.file) else {
        return Vec::new();
    };
    let mut vals: Vec<Vec<u8>> = Vec::new();
    if keys.is_empty() {
        let first = text
            .split(|&c| c == b'\n')
            .find(|l| !l.trim_ascii().is_empty());
        vals.extend(first.map(<[u8]>::to_vec));
    } else if let Ok(doc) = serde_json::from_slice::<Value>(&text) {
        for k in keys {
            let mut at = Some(&doc);
            for part in String::from_utf8_lossy(k).split('.') {
                at = at.and_then(|v| v.get(part));
            }
            if let Some(v) = at {
                json_strings(v, &mut vals);
            }
        }
    }
    let mut out: Vec<Vec<u8>> = Vec::new();
    for v in vals {
        if let Some(x) = norm_path(&v, home).filter(|x| !out.contains(x)) {
            out.push(x);
        }
    }
    out
}

/// A clean path below HOME: REL with no empty, `.` or `..` part.
fn clean_rel(rel: &[u8]) -> bool {
    !rel.is_empty()
        && rel
            .split(|&c| c == b'/')
            .all(|w| !w.is_empty() && w != b"." && w != b"..")
}

/// A desk shell found: its profile's index, the Quickshell config's name for a `{name}` one, what
/// found it (`--shell`, `config`, `unit`) and the program it runs (the command's first word; for a
/// unit, its profile's `start`'s).
struct Found {
    i: usize,
    name: Option<Vec<u8>>,
    from: &'static str,
    prog: Vec<u8>,
}

/// box_path H HOMES CALLER: where a desk box's session looks for a program, as folders on
/// the host (share/session.sh's PATH, finding 57): the box HOME's ~/.local/bin, the caller's PATH
/// (CALLER, exec::caller_path's) as the box sees it, then /usr/bin (the box's own start). A folder
/// of CALLER under HOME (HOMES: HOME/ and where it leads) is the box HOME's (a desk box's HOME is
/// at the user's HOME path); one under /usr, /bin or /sbin the host's (the box has /usr, the rest
/// links into it); any other the box has not. /opt/omabox/share/bin is left out: omabox's
/// stand-ins, never a shell.
fn box_path(h: &[u8], homes: &[Vec<u8>], caller: &[u8]) -> Vec<Vec<u8>> {
    let mut dirs = vec![cat!(h, "/.local/bin")];
    for d in caller.split(|&c| c == b':') {
        if let Some(t) = box_sees(h, homes, d)
            && !dirs.contains(&t)
        {
            dirs.push(t);
        }
    }
    if !dirs.iter().any(|d| d == b"/usr/bin") {
        dirs.push(b"/usr/bin".to_vec());
    }
    dirs
}

/// box_sees H HOMES P: the host path the absolute path P is in a desk box (box_path says which).
fn box_sees(h: &[u8], homes: &[Vec<u8>], p: &[u8]) -> Option<Vec<u8>> {
    if !p.starts_with(b"/") {
        return None;
    }
    let t = p.strip_suffix(b"/").filter(|t| !t.is_empty()).unwrap_or(p);
    for hm in homes {
        let hm = hm.strip_suffix(b"/").unwrap_or(hm);
        if t == hm {
            return Some(h.to_vec());
        }
        if let Some(rest) = t.strip_prefix(hm).filter(|r| r.starts_with(b"/")) {
            return Some(cat!(h, rest));
        }
    }
    ["/usr", "/bin", "/sbin"]
        .iter()
        .any(|r| t == r.as_bytes() || t.starts_with(&cat!(r, "/")))
        .then(|| t.to_vec())
}

/// program_found PROG DIRS H HOMES QUICKSHELL: whether a desk box finds PROG, a command's first
/// word: a path (with a `/`) as the box sees it (box_sees; a relative one is taken as found: it
/// depends on where it runs), else an executable file of that name in one of DIRS (box_path's).
/// QUICKSHELL (`up --quickshell`): its build is the box's `qs` and `quickshell`.
fn program_found(
    prog: &[u8],
    dirs: &[Vec<u8>],
    h: &[u8],
    homes: &[Vec<u8>],
    quickshell: bool,
) -> bool {
    if prog.contains(&b'/') {
        if !prog.starts_with(b"/") {
            return true;
        }
        return box_sees(h, homes, prog)
            .is_some_and(|p| crate::exec::executable(&p) && shell::path(&p).is_file());
    }
    if quickshell && (prog == b"qs" || prog == b"quickshell") {
        return true;
    }
    dirs.iter().any(|d| {
        let p = cat!(d, "/", prog);
        crate::exec::executable(&p) && shell::path(&p).is_file()
    })
}

/// The desk shells to copy and guess (#321 step 2): what --shell CMD starts, then what the copied
/// Lua files of the box's ~/.config/hypr start (hyprland.lua first; `--` comments skipped), then the
/// profiles whose unit is linked under ~/.config/systemd/user's hyprland-session.target.wants or
/// graphical-session.target.wants. Each profile (and Quickshell config) once, in that order.
fn detect(profiles: &[Profile], shell_cmd: &[u8], luas: &[Vec<u8>], config: &[u8]) -> Vec<Found> {
    let mut found: Vec<Found> = Vec::new();
    let mut add = |i: usize, name: Option<Vec<u8>>, from: &'static str, prog: Vec<u8>| {
        if !found.iter().any(|f| f.i == i && f.name == name) {
            found.push(Found {
                i,
                name,
                from,
                prog,
            });
        }
    };
    for (i, n, prog) in deskshell::hits(shell_cmd, profiles) {
        add(i, n, "--shell", prog);
    }
    for text in luas {
        for line in text.split(|&c| c == b'\n') {
            for s in strings(line, true) {
                for (i, n, prog) in deskshell::hits(&s, profiles) {
                    add(i, n, "config", prog);
                }
            }
        }
    }
    for (i, pr) in profiles.iter().enumerate() {
        if pr.unit.is_empty() {
            continue;
        }
        let linked = [
            "hyprland-session.target.wants",
            "graphical-session.target.wants",
        ]
        .iter()
        .any(|t| {
            std::fs::symlink_metadata(p(&cat!(config, "/systemd/user/", t, "/", pr.unit))).is_ok()
        });
        if linked {
            add(i, None, "unit", deskshell::program(&pr.start));
        }
    }
    found
}

/// What a skip's REL is shown as: ~/REL, an absolute path as it is.
fn shown(rel: &[u8]) -> Vec<u8> {
    if rel.starts_with(b"/") || rel.starts_with(b"(") {
        rel.to_vec()
    } else {
        cat!("~/", rel)
    }
}

/// copy PLAN H: the user's desktop into the box HOME H, for a desk box; its lines: `copied PATH`,
/// `skipped PATH<TAB>WHY`, and `guess NAME<TAB>FROM` (the slot's first guess, and what found it).
/// Refused (up fails, no box): a Hyprland config folder refuse_src keeps out, a hyprland.lua or a
/// module it requires that could not be copied.
pub fn copy(ctx: &Ctx, plan: &SeedPlan, h: &[u8]) -> Res<Vec<u8>> {
    let home = shell::env_or_empty("HOME");
    let home_r = readlink_e(&home).unwrap_or_else(|| home.clone());
    let config = xdg("XDG_CONFIG_HOME", &home, ".config");
    let mut w = Walk::new(ctx, h, home.clone(), home_r);
    // 1. The Hyprland config's folder, whole.
    let hypr = cat!(config, "/hypr");
    w.part(b"~/.config/hypr".to_vec(), HYPR);
    if let Err(why) = w.root(&hypr, b".config/hypr")? {
        return Err(ctx.die(cat!(
            "up: --desktop desk: your Hyprland config (",
            hypr,
            ") cannot go into a box: ",
            why
        )));
    }
    let lua_rel = b".config/hypr/hyprland.lua";
    // (what stayed out: skipped, or not walked when the part stopped at a cap before it, finding 710)
    let left = |w: &Walk, rel: &[u8]| -> Option<Vec<u8>> {
        if let Some(why) = w.skipped.get(rel) {
            return Some(why.clone());
        }
        let (_, why) = w.stop.as_ref()?;
        let on_host = rel
            .strip_prefix(b".config/hypr/".as_slice())
            .is_some_and(|r| std::fs::symlink_metadata(p(&cat!(hypr, "/", r))).is_ok());
        (!w.seen.contains(rel) && on_host).then(|| cat!("not walked: ", why))
    };
    if let Some(why) = left(&w, lua_rel) {
        return Err(ctx.die(cat!(
            "up: --desktop desk: your hyprland.lua was not copied into the box: ",
            why
        )));
    }
    let luas: Vec<Vec<u8>> = {
        let mut v: Vec<Vec<u8>> = w
            .texts
            .iter()
            .filter(|r| r.ends_with(b".lua"))
            .cloned()
            .collect();
        v.sort_by_key(|r| r.as_slice() != lua_rel);
        v
    };
    let lua_texts: Vec<Vec<u8>> = luas
        .iter()
        .map(|r| std::fs::read(p(&cat!(h, "/", r))).unwrap_or_default())
        .collect();
    for text in &lua_texts {
        for m in requires(text) {
            for f in [
                cat!(".config/hypr/", m, ".lua"),
                cat!(".config/hypr/", m, "/init.lua"),
            ] {
                if let Some(why) = left(&w, &f) {
                    return Err(ctx.die(cat!(
                        "up: --desktop desk: ~/",
                        f,
                        ", which your Hyprland config requires, was not copied into the box: ",
                        why
                    )));
                }
            }
        }
    }
    // 2. The desk shells found, each one's config, state and listed caches.
    let profiles = deskshell::all(&deskshell::dir()).map_err(|e| ctx.die(cat!("up: ", e)))?;
    let mut found = detect(&profiles, &plan.shell_cmd, &lua_texts, &config);
    // A shell found by the config or a unit whose program the box does not find (a unit link left
    // when its package went): not the box's, its folders not copied, said once, so the slot never
    // starts a program that is not there (finding 701). --shell's runs as asked.
    {
        let homes = [cat!(home, "/"), cat!(w.home_r, "/")];
        let dirs = box_path(h, &homes, &crate::exec::caller_path(true));
        found.retain(|f| {
            if f.from == "--shell"
                || f.prog.is_empty()
                || program_found(&f.prog, &dirs, h, &homes, plan.quickshell)
            {
                return true;
            }
            let name = &profiles[f.i].name;
            let how = if f.from == "unit" {
                "its systemd unit is linked"
            } else {
                "your Hyprland config starts it"
            };
            say(cat!(
                "up: not the ",
                name,
                " shell (",
                how,
                "): its program ",
                f.prog,
                " is not installed"
            ));
            w.out.extend(cat!(
                "skipped (shell ",
                name,
                ")\tits program ",
                f.prog,
                " is not installed\n"
            ));
            false
        });
    }
    for f in &found {
        let pr = &profiles[f.i];
        let label = match &f.name {
            Some(n) => cat!("the ", pr.name, " shell (", n, ")"),
            None => cat!("the ", pr.name, " shell"),
        };
        w.part(label, SHELL);
        for rel in pr.config.iter().chain(&pr.state).chain(&pr.cache) {
            let rel = if crate::exit::contains(rel, b"{name}") {
                let Some(n) = &f.name else { continue };
                let s = String::from_utf8_lossy(rel).replace("{name}", &String::from_utf8_lossy(n));
                s.into_bytes()
            } else {
                rel.clone()
            };
            if !clean_rel(&rel) {
                continue;
            }
            let src = host_of(&home, &rel);
            if let Err(why) = w.root(&src, &rel)? {
                w.skip(&cat!(rel, "/"), &why);
            }
        }
    }
    // Each found shell's own idle timeouts off in its copy (its profile's `idle` lines), said.
    for f in &found {
        for line in &profiles[f.i].idle {
            let mut words = line
                .split(u8::is_ascii_whitespace)
                .filter(|x| !x.is_empty());
            let (file, key) = (
                words.next().unwrap_or_default(),
                words.next().unwrap_or_default(),
            );
            match idle_edit(h, line) {
                Ok(Some(file)) => {
                    say(cat!(
                        "up: ",
                        profiles[f.i].name,
                        "'s own idle timeouts off in the box's ~/",
                        file,
                        " (",
                        key,
                        ")"
                    ));
                    w.out.extend(cat!("edited ", file, "\t", key, "\n"));
                }
                Ok(None) => {}
                Err(link) => {
                    let why = cat!("a link on its way (", link, "), not followed");
                    say(cat!(
                        "up: ",
                        profiles[f.i].name,
                        "'s own idle timeouts not turned off in the box's ~/",
                        file,
                        ": ",
                        why
                    ));
                    w.out
                        .extend(cat!("skipped ", file, "\tits idle edit: ", why, "\n"));
                }
            }
        }
    }
    // 3. The pictures the desk shows (finding 710): what each found shell's profile names in its
    // `images` keys (a folder too), the wallpaper commands of the Hyprland config and hyprpaper.conf
    // (a picture each). Nothing else a copied file names comes in (a bind's ~/Downloads stays out).
    w.part(b"the images they name".to_vec(), IMAGES);
    let mut named: Vec<(Vec<u8>, bool)> = Vec::new();
    for f in &found {
        for line in &profiles[f.i].images {
            let file = line
                .split(|c| c.is_ascii_whitespace())
                .next()
                .unwrap_or_default();
            // (only a file the copy brought: a save's own is the box's word, not the desk's)
            if !w.texts.iter().any(|t| t == file) {
                continue;
            }
            named.extend(key_paths(h, line, &home).into_iter().map(|x| (x, true)));
        }
    }
    for text in &lua_texts {
        named.extend(
            wallpaper_cmd_paths(text, &home)
                .into_iter()
                .map(|x| (x, false)),
        );
    }
    let hp = b".config/hypr/hyprpaper.conf";
    if w.texts.iter().any(|t| t == hp) {
        let text = read_in_box(h, hp, HYPR.file).unwrap_or_default();
        named.extend(
            hyprpaper_paths(&text, &home)
                .into_iter()
                .map(|x| (x, false)),
        );
    }
    let mut files: Vec<Vec<u8>> = Vec::new();
    let mut folders: Vec<Vec<u8>> = Vec::new();
    let homes = [cat!(home, "/"), cat!(w.home_r, "/")];
    for (path, folder_ok) in named {
        let below = homes
            .iter()
            .find_map(|x| path.strip_prefix(x.as_slice()))
            .map(<[u8]>::to_vec);
        let base = path.rsplit(|&c| c == b'/').next().unwrap_or_default();
        match below {
            Some(rel) if !clean_rel(&rel) => {}
            Some(rel) if pic_name(base) => {
                if !files.contains(&rel) {
                    files.push(rel);
                }
            }
            Some(rel) => {
                if folder_ok
                    && std::fs::metadata(p(&path)).is_ok_and(|m| m.is_dir())
                    && !folders.contains(&rel)
                {
                    folders.push(rel);
                }
            }
            None if pic_name(base) && !path.starts_with(b"/usr/") => {
                w.skip(&path, b"outside your HOME");
            }
            None => {}
        }
    }
    for rel in &files {
        w.named(rel, false)?;
    }
    for rel in &folders {
        w.named(rel, true)?;
    }
    // 4. On Omarchy's base, the wallpaper the desk shows (its seeding follows no link).
    if !plan.plain && plan.theme.is_empty() && !plan.stock {
        w.part(b"your wallpapers".to_vec(), IMAGES);
        wallpaper(&mut w, plan)?;
    }
    w.end_part();
    // What up says: the parts copied, the first guess, and what stayed out (box.json has it all).
    // (on Omarchy's base the shell is Omarchy's: no guess for a slot it does not have)
    let guess = found
        .first()
        .filter(|_| plan.plain)
        .map(|f| (profiles[f.i].name.clone(), f.from));
    let n: u64 = w.parts.iter().map(|(_, c)| c).sum();
    if n > 0 || !w.nskipped.is_empty() || guess.is_some() {
        let parts: Vec<Vec<u8>> = w
            .parts
            .iter()
            .filter(|(_, c)| *c > 0)
            .map(|(what, c)| cat!(c.to_string(), " of ", what))
            .collect();
        let mut line = cat!(
            "up: your desktop, copied into the box's HOME: ",
            n.to_string(),
            " files"
        );
        if !parts.is_empty() {
            line = cat!(line, " (", parts.join(&b", "[..]), ")");
        }
        if let Some((name, from)) = &guess {
            line = cat!(
                line,
                "; its shell, first guessed: ",
                name,
                " (from ",
                from,
                ")"
            );
        }
        say(line);
        for s in w.nskipped.iter().take(10) {
            let i = s.iter().position(|&c| c == b'\t').unwrap_or(s.len());
            let why = s.get(i + 1..).unwrap_or_default();
            say(cat!("up: not copied: ", shown(&s[..i]), " (", why, ")"));
        }
        if w.nskipped.len() > 10 {
            say(cat!(
                "up: ...and ",
                (w.nskipped.len() - 10).to_string(),
                " more not copied (box.json's desk_copy has them all)"
            ));
        }
    }
    let mut out = w.out;
    if let Some((name, from)) = guess {
        out.extend(cat!("guess ", name, "\t", from, "\n"));
    }
    Ok(shell::strip_nl(&out))
}

/// The wallpaper an Omarchy desk shows, in a desk box on Omarchy's base (finding 709). Omarchy's
/// seeding (seed::box_background) follows no link: a ~/.config/omarchy/backgrounds/THEME (or
/// backgrounds/) that is one is left out, and a current/background that leads anywhere but the
/// theme or that folder falls back to the theme's first picture. Here, as the rest of the copy: that
/// folder's top-level images when it is a link, else the one image the link leads to (at its path
/// in HOME), each through refuse_src and the name check under the images' caps; the box's
/// current/background then leads to its copy. An intro video beside it stays out (images only).
fn wallpaper(w: &mut Walk, plan: &SeedPlan) -> Res<()> {
    let home = w.home.clone();
    let cur = ".local/state/omarchy/current";
    let link = cat!(home, "/", cur, "/background");
    let Ok(raw) = std::fs::read_link(p(&link)) else {
        return Ok(());
    };
    let raw = shell::bytes(&raw).to_vec();
    // (the theme's own picture: seeding's link to the box's copy of the theme stands)
    if raw.starts_with(b"/usr/") || crate::exit::contains(&raw, b"/current/theme/") {
        return Ok(());
    }
    let Some(t) = readlink_e(&link) else {
        return Ok(());
    };
    if t.starts_with(b"/usr/") {
        return Ok(());
    }
    let theme = std::fs::read(p(&cat!(home, "/", cur, "/theme.name")))
        .map(|t| t.split(|&c| c == b'\n').next().unwrap_or_default().to_vec())
        .unwrap_or_default();
    let is_link =
        |x: &[u8]| std::fs::symlink_metadata(p(x)).is_ok_and(|m| m.file_type().is_symlink());
    let mut to: Option<Vec<u8>> = None;
    if shell::re_match("^[a-zA-Z0-9][a-zA-Z0-9._-]*$", &theme) {
        let bgs = cat!(".config/omarchy/backgrounds/", theme);
        let folder = cat!(home, "/", bgs);
        let linked = is_link(&cat!(home, "/.config/omarchy/backgrounds")) || is_link(&folder);
        if linked {
            wallpaper_folder(w, &folder, &bgs)?;
        }
        let inside = readlink_e(&folder).and_then(|f| {
            t.strip_prefix(cat!(f, "/").as_slice())
                .filter(|n| !n.contains(&b'/'))
                .map(<[u8]>::to_vec)
        });
        if let Some(n) = inside {
            // (a folder of its own: seeding copied it and made the link, or its caps left it out)
            if !linked {
                return Ok(());
            }
            to = Some(cat!(bgs, "/", n));
        }
    }
    let rel = match to {
        Some(r) => r,
        None => {
            let below = [cat!(home, "/"), cat!(w.home_r, "/")]
                .iter()
                .find_map(|x| t.strip_prefix(x.as_slice()))
                .map(<[u8]>::to_vec);
            match below {
                Some(rel) if clean_rel(&rel) => {
                    w.named(&rel, false)?;
                    rel
                }
                _ => {
                    w.skip(&t, b"your wallpaper, outside your HOME");
                    return Ok(());
                }
            }
        }
    };
    let copied = std::fs::symlink_metadata(p(&cat!(w.h, "/", rel))).is_ok_and(|m| m.is_file());
    let Some(d) = Dest::new(&cat!(cur, "/background")) else {
        return Ok(());
    };
    if copied {
        // (relative: on the host, the link leads into the box's folder, not your HOME; finding 710)
        let to = seed::link_target(w.h, &cat!(w.h, "/", d.rel()), plan.box_home(), &rel);
        seed::home_link(w.ctx, w.h, &d, &to)?;
        w.out.extend(cat!("linked ", d.rel(), "\t", rel, "\n"));
    }
    Ok(())
}

/// The top-level images of FOLDER (a link, or reached through one) into the box at REL, its own
/// folder. Where it leads goes through follow_why (refuse_src and the names on its way), and each
/// picture in it too; FOLDER's own path, in ~/.config/omarchy, is the one refuse_src keeps out but
/// for plugins/ and themes/, and Omarchy's seeding copies it when it is no link.
fn wallpaper_folder(w: &mut Walk, folder: &[u8], rel: &[u8]) -> Res<()> {
    let shown = cat!(rel, "/");
    let Some(f) = readlink_e(folder) else {
        w.skip(&shown, b"a link that leads nowhere");
        return Ok(());
    };
    if let Some(why) = w.follow_why(&f, &f) {
        w.skip(&shown, &why);
        return Ok(());
    }
    for n in names_of(&f, true) {
        let r = cat!(rel, "/", n);
        if w.halted(&r) {
            continue;
        }
        if let Some(why) = seed::desk_box_name_refused(&n) {
            w.skip(&r, why.as_bytes());
            continue;
        }
        let e = cat!(f, "/", n);
        let Some(t) = readlink_e(&e) else {
            w.skip(&r, b"a link that leads nowhere");
            continue;
        };
        if let Some(why) = w.follow_why(&e, &t) {
            w.skip(&r, &why);
            continue;
        }
        match std::fs::metadata(p(&t)) {
            Ok(m) if m.is_file() => w.file(&t, &r, m.len())?,
            _ => w.skip(&r, b"not a regular file"),
        }
    }
    Ok(())
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
    /// The desk shells the copy found but left out, their program not installed (finding 701):
    /// each `(shell NAME)` skipped, by NAME (the box's OMABOX_DESK_SHELL_MISSING).
    pub fn missing(&self) -> Vec<Vec<u8>> {
        self.skipped
            .iter()
            .filter_map(|(p, _)| p.strip_prefix(b"(shell ".as_slice())?.strip_suffix(b")"))
            .map(<[u8]>::to_vec)
            .collect()
    }

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

    use std::os::unix::fs::symlink;

    /// A stand-in desk: BASE/home (HOME, its XDG dirs under it), BASE/box (the box HOME), BASE/out
    /// (outside HOME). Never the user's.
    struct Fx {
        base: String,
        fh: String,
        bx: String,
    }

    impl Fx {
        fn new(tag: &str) -> Fx {
            let d =
                std::env::temp_dir().join(format!("omabox-deskcopy-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            let base = d.to_string_lossy().into_owned();
            let fx = Fx {
                fh: format!("{base}/home"),
                bx: format!("{base}/box"),
                base,
            };
            for x in [&fx.fh, &fx.bx] {
                std::fs::create_dir_all(x).unwrap();
            }
            std::fs::create_dir_all(format!("{}/out", fx.base)).unwrap();
            // (the shipped profiles' programs, where the box finds them: its HOME's ~/.local/bin, so
            // a shell found is one installed wherever this runs; finding 701's test has its own)
            for n in ["dms", "caelestia", "qs", "quickshell", "waybar"] {
                prog(&format!("{}/.local/bin/{n}", fx.bx));
            }
            fx
        }

        /// Write TEXT at HOME/REL (its folders made).
        fn put(&self, rel: &str, text: &[u8]) {
            let f = format!("{}/{rel}", self.fh);
            std::fs::create_dir_all(std::path::Path::new(&f).parent().unwrap()).unwrap();
            std::fs::write(f, text).unwrap();
        }

        fn ln(&self, target: &str, rel: &str) {
            let f = format!("{}/{rel}", self.fh);
            std::fs::create_dir_all(std::path::Path::new(&f).parent().unwrap()).unwrap();
            symlink(target, f).unwrap();
        }

        /// copy with this HOME (--shell SHELL), what it returned and what it said.
        fn run(&self, shell: &str) -> (Result<String, u8>, String) {
            self.run_plan(&SeedPlan {
                plain: true,
                desk_box: true,
                shell_cmd: shell.as_bytes().to_vec(),
                ..SeedPlan::default()
            })
        }

        /// copy on Omarchy's base (after its seeding), the box's HOME at this HOME's path.
        fn run_omarchy(&self) -> String {
            self.run_plan(&SeedPlan {
                desk_box: true,
                home: self.fh.as_bytes().to_vec(),
                ..SeedPlan::default()
            })
            .0
            .unwrap()
        }

        fn run_plan(&self, plan: &SeedPlan) -> (Result<String, u8>, String) {
            let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../share/desk-shells");
            self.run_in(plan, dir, None)
        }

        /// copy with this HOME, the profiles in DIR, PATH (None: the caller's).
        fn run_in(
            &self,
            plan: &SeedPlan,
            dir: &str,
            path: Option<&str>,
        ) -> (Result<String, u8>, String) {
            let ctx = {
                let mut c = Ctx::new(b"up", b"/nonexistent".to_vec());
                c.parsed();
                c
            };
            let fh = self.fh.clone();
            let caller = std::env::var("PATH").unwrap_or_default();
            let vars = [
                ("HOME", Some(fh.as_str())),
                ("XDG_CONFIG_HOME", None),
                ("XDG_STATE_HOME", None),
                ("XDG_DATA_HOME", None),
                ("XDG_CACHE_HOME", None),
                ("OMABOX_JAIL", None),
                ("OMABOX_DESK_SHELL_DIR", Some(dir)),
                ("PATH", Some(path.unwrap_or(&caller))),
            ];
            let (r, said) = shell::with_env(&vars, || {
                crate::exit::heard(|| copy(&ctx, plan, self.bx.as_bytes()))
            });
            (
                r.map(|v| String::from_utf8_lossy(&v).into_owned())
                    .map_err(|e| e.0),
                said,
            )
        }

        /// precheck with this HOME (deskguard::checks's, before any box): what it said, if it failed.
        fn pre(&self) -> Result<(), String> {
            let ctx = {
                let mut c = Ctx::new(b"up", b"/nonexistent".to_vec());
                c.parsed();
                c
            };
            let fh = self.fh.clone();
            let vars = [
                ("HOME", Some(fh.as_str())),
                ("XDG_CONFIG_HOME", None),
                ("OMABOX_JAIL", None),
            ];
            let (r, said) = shell::with_env(&vars, || crate::exit::heard(|| precheck(&ctx)));
            r.map_err(|_| said)
        }

        fn in_box(&self, rel: &str) -> String {
            format!("{}/{rel}", self.bx)
        }

        fn real_file(&self, rel: &str) -> bool {
            std::fs::symlink_metadata(self.in_box(rel)).is_ok_and(|m| m.is_file())
        }

        fn there(&self, rel: &str) -> bool {
            std::fs::symlink_metadata(self.in_box(rel)).is_ok()
        }
    }

    impl Drop for Fx {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn lines<'a>(out: &'a str, kind: &str) -> Vec<&'a str> {
        out.lines().filter_map(|l| l.strip_prefix(kind)).collect()
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n0000";
    const LUA: &[u8] = b"hl.config({ general = { gaps_out = 7 } })\n";

    /// Stow's shape: ~/.config/hypr a link into ~/dotfiles, a file in it a link too, a folder link
    /// inside; all copied as the box's own files and folders, none a link.
    #[test]
    fn follows_dotfile_links() {
        if !crate::exec::tools_or_skip("follows_dotfile_links", &["grep", "cp"]) {
            return;
        }
        let f = Fx::new("stow");
        f.put("dotfiles/hypr/.config/hypr/hyprland.lua", LUA);
        f.put("dotfiles/hypr/extra/binds.lua", b"-- binds\n");
        f.put("dotfiles/hypr/extra/sub/x.conf", b"x\n");
        f.ln(
            &format!("{}/dotfiles/hypr/.config/hypr", f.fh),
            ".config/hypr",
        );
        f.ln(
            "../../extra/binds.lua",
            "dotfiles/hypr/.config/hypr/keybinds.lua",
        );
        f.ln(
            &format!("{}/dotfiles/hypr/extra/sub", f.fh),
            "dotfiles/hypr/.config/hypr/sub",
        );
        let (r, said) = f.run("");
        let out = r.unwrap();
        assert_eq!(
            lines(&out, "copied "),
            [
                ".config/hypr/hyprland.lua",
                ".config/hypr/keybinds.lua",
                ".config/hypr/sub/x.conf"
            ],
            "{out}"
        );
        for r in [
            ".config/hypr/hyprland.lua",
            ".config/hypr/keybinds.lua",
            ".config/hypr/sub/x.conf",
        ] {
            assert!(f.real_file(r), "{r}");
        }
        assert!(
            !std::fs::symlink_metadata(f.in_box(".config/hypr"))
                .unwrap()
                .is_symlink()
        );
        assert!(
            !std::fs::symlink_metadata(f.in_box(".config/hypr/sub"))
                .unwrap()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read(f.in_box(".config/hypr/keybinds.lua")).unwrap(),
            b"-- binds\n"
        );
        assert!(said.contains("omabox: up: your desktop, copied into the box's HOME: 3 files (3 of ~/.config/hypr)"), "{said}");
        // (a write in the box's copy leaves the host's file as it was)
        std::fs::write(f.in_box(".config/hypr/hyprland.lua"), b"x").unwrap();
        assert_eq!(
            std::fs::read(format!("{}/.config/hypr/hyprland.lua", f.fh)).unwrap(),
            LUA
        );
    }

    /// A link into /usr stays a link (the box has /usr); one leading nowhere is said.
    #[test]
    fn keeps_usr_links() {
        if !crate::exec::tools_or_skip("keeps_usr_links", &["grep", "cp"]) {
            return;
        }
        let f = Fx::new("usr");
        f.put(".config/hypr/hyprland.lua", LUA);
        f.ln("/usr/share", ".config/hypr/share");
        f.ln("/usr/bin/sh", ".config/hypr/sh");
        f.ln(&format!("{}/gone", f.fh), ".config/hypr/gone.lua");
        let out = f.run("").0.unwrap();
        for (r, to) in [("share", "/usr/share"), ("sh", "/usr/bin/sh")] {
            let l = std::fs::read_link(f.in_box(&format!(".config/hypr/{r}"))).unwrap();
            assert_eq!(
                l.to_string_lossy(),
                std::fs::canonicalize(to).unwrap().to_string_lossy()
            );
        }
        assert_eq!(
            lines(&out, "copied "),
            [
                ".config/hypr/hyprland.lua",
                ".config/hypr/sh",
                ".config/hypr/share"
            ],
            "{out}"
        );
        assert!(
            out.contains("skipped .config/hypr/gone.lua\ta link that leads nowhere"),
            "{out}"
        );
    }

    /// Links to secret stores, omabox's folders, ~/.config/omarchy and HOME are not followed; a
    /// secret's name on a link's way is not either; nothing of them is in the box.
    #[test]
    fn refuses_secrets_through_links() {
        if !crate::exec::tools_or_skip("refuses_secrets_through_links", &["grep", "cp"]) {
            return;
        }
        let f = Fx::new("sec");
        f.put(".config/hypr/hyprland.lua", LUA);
        f.put(".ssh/id_x", b"not a key\n");
        f.put(".config/omarchy/api-keys.env", b"X=1\n");
        f.put(".config/omarchy/themes/t/colors.toml", b"a = 1\n");
        f.put("private/notes.lua", b"-- n\n");
        f.put(".cache/omabox/b/home/f", b"f\n");
        f.ln(&format!("{}/.ssh", f.fh), ".config/hypr/ssh");
        f.ln(&format!("{}/.ssh/id_x", f.fh), ".config/hypr/k.lua");
        f.ln(&format!("{}/.config/omarchy", f.fh), ".config/hypr/om");
        f.ln(
            &format!("{}/.config/omarchy/themes/t", f.fh),
            ".config/hypr/theme",
        );
        f.ln(&f.fh, ".config/hypr/home");
        f.ln(
            &format!("{}/private/notes.lua", f.fh),
            ".config/hypr/notes.lua",
        );
        f.ln(&format!("{}/.cache/omabox", f.fh), ".config/hypr/ob");
        f.put(".config/hypr/tokens.lua", b"x\n");
        f.put(".config/hypr/env.lua", b"api_key = \"abcdefgh123\"\n");
        let out = f.run("").0.unwrap();
        let skipped: Vec<&str> = lines(&out, "skipped ")
            .iter()
            .map(|l| l.split('\t').next().unwrap())
            .collect();
        assert_eq!(
            skipped,
            [
                ".config/hypr/env.lua",
                ".config/hypr/home",
                ".config/hypr/k.lua",
                ".config/hypr/notes.lua",
                ".config/hypr/ob",
                ".config/hypr/om",
                ".config/hypr/ssh",
                ".config/hypr/tokens.lua"
            ],
            "{out}"
        );
        assert!(
            out.contains(".config/hypr/env.lua\tit holds what looks like a key"),
            "{out}"
        );
        assert!(out.contains(".config/hypr/ssh\tit leads to "), "{out}");
        assert!(
            out.contains(".config/hypr/notes.lua\tit leads to ") && out.contains("a secret's name"),
            "{out}"
        );
        // (a theme of ~/.config/omarchy may go in, as --theme-dir's)
        assert!(f.real_file(".config/hypr/theme/colors.toml"), "{out}");
        for r in [
            "ssh",
            "k.lua",
            "om",
            "home",
            "notes.lua",
            "ob",
            "tokens.lua",
            "env.lua",
        ] {
            assert!(!f.there(&format!(".config/hypr/{r}")), "{r}");
        }
    }

    /// A folder linked below itself is a loop; 8 folders deep is the end.
    #[test]
    fn loop_and_depth() {
        if !crate::exec::tools_or_skip("loop_and_depth", &["grep", "cp"]) {
            return;
        }
        let f = Fx::new("loop");
        f.put(".config/hypr/hyprland.lua", LUA);
        f.ln(&format!("{}/.config/hypr", f.fh), ".config/hypr/a/up");
        f.put(".config/hypr/d/1/2/3/4/5/6/7/f8", b"8\n");
        f.put(".config/hypr/d/1/2/3/4/5/6/7/8/f9", b"9\n");
        let out = f.run("").0.unwrap();
        assert!(
            out.contains("skipped .config/hypr/a/up/\ta loop: a link to a folder it is in"),
            "{out}"
        );
        assert!(
            out.contains("copied .config/hypr/d/1/2/3/4/5/6/7/f8\n"),
            "{out}"
        );
        assert!(
            out.contains("skipped .config/hypr/d/1/2/3/4/5/6/7/8/\tmore than 8 folders deep"),
            "{out}"
        );
    }

    /// The caps: a file over 1 MiB in the config, an image over 32 MiB, more than 32 images.
    #[test]
    fn caps() {
        if !crate::exec::tools_or_skip("caps", &["grep", "cp"]) {
            return;
        }
        let f = Fx::new("caps");
        f.put(".config/hypr/hyprland.lua", LUA);
        f.put(
            ".local/state/DankMaterialShell/session.json",
            format!(
                "{{\"wallpaperPath\": \"{}/big.png\", \"monitorWallpapers\": {{\"DP-1\": \"~/w\"}}}}\n",
                f.fh
            )
            .as_bytes(),
        );
        for i in 0..12 {
            let big =
                std::fs::File::create(format!("{}/.config/hypr/big{i:02}.lua", f.fh)).unwrap();
            big.set_len(2 * MIB).unwrap();
        }
        for i in 0..44 {
            f.put(&format!("w/{i:02}.png"), PNG);
        }
        let big = std::fs::File::create(format!("{}/big.png", f.fh)).unwrap();
        big.set_len(33 * MIB).unwrap();
        let (r, said) = f.run("dms run");
        let out = r.unwrap();
        assert!(
            out.contains("skipped .config/hypr/big00.lua\tover 1 MiB"),
            "{out}"
        );
        assert!(out.contains("skipped big.png\tover 32 MiB"), "{out}");
        assert_eq!(lines(&out, "copied w/").len(), 32, "{out}");
        // (the cap stops the part: one line for the 12 pictures left, finding 710)
        assert!(
            out.contains(
                "skipped w/...\t12 more not walked: over the cap for the images they name: 128 MiB and 32 files"
            ),
            "{out}"
        );
        assert!(!out.contains("skipped w/32.png"), "{out}");
        assert!(
            said.contains(
                "omabox: up: ...and 4 more not copied (box.json's desk_copy has them all)"
            ),
            "{said}"
        );
        assert_eq!(
            said.matches("omabox: up: not copied: ").count(),
            10,
            "{said}"
        );
    }

    /// A part stops at its cap (finding 710): a big tree's first 500 files copied, the rest not
    /// walked (no folder of it made, no line each), one line saying how many; a tree of empty
    /// folders stops at its entries' cap; the copy's time up stops every part after it.
    #[test]
    fn caps_stop_the_walk() {
        if !crate::exec::tools_or_skip("caps_stop_the_walk", &["grep", "cp"]) {
            return;
        }
        let f = Fx::new("bigtree");
        f.put(".config/hypr/hyprland.lua", LUA);
        for i in 0..600 {
            f.put(&format!(".config/hypr/m/{i:03}.lua"), b"-- x\n");
        }
        for i in 0..20 {
            f.put(&format!(".config/hypr/z{i:02}/a/b/f.lua"), b"-- z\n");
        }
        let (r, said) = f.run("");
        let out = r.unwrap();
        assert_eq!(lines(&out, "copied ").len(), 500, "{out}");
        let skipped = lines(&out, "skipped ");
        assert_eq!(
            skipped,
            [
                ".config/hypr/m/...\t121 more not walked: over the cap for ~/.config/hypr: 8 MiB and 500 files"
            ],
            "{out}"
        );
        assert!(
            !f.there(".config/hypr/z00"),
            "a folder after the cap: not made"
        );
        assert!(!f.there(".config/hypr/m/499.lua"), "{out}");
        assert!(
            said.contains("omabox: up: not copied: ~/.config/hypr/m/... (121 more not walked: "),
            "{said}"
        );
        // Empty folders: 2000 entries looked at, then the end.
        let f = Fx::new("emptytree");
        f.put(".config/hypr/hyprland.lua", LUA);
        for i in 0..2100 {
            std::fs::create_dir_all(format!("{}/.config/hypr/z/{i:04}", f.fh)).unwrap();
        }
        let out = f.run("").0.unwrap();
        assert_eq!(
            lines(&out, "skipped "),
            [
                ".config/hypr/z/...\t103 more not walked: over the cap for ~/.config/hypr: 2000 entries"
            ],
            "{out}"
        );
        assert!(f.there(".config/hypr/z/1996"), "{out}");
        assert!(!f.there(".config/hypr/z/1997"), "{out}");
        // A cap met before hyprland.lua (a folder sorted before it): refused, as a skipped one is.
        let f = Fx::new("bigfirst");
        f.put(".config/hypr/hyprland.lua", LUA);
        for i in 0..600 {
            f.put(&format!(".config/hypr/a/{i:03}.lua"), b"-- x\n");
        }
        let (r, said) = f.run("");
        assert_eq!(r, Err(1));
        assert_eq!(
            said,
            "omabox: up: --desktop desk: your hyprland.lua was not copied into the box: not walked: over the cap for ~/.config/hypr: 8 MiB and 500 files\n"
        );
        // The copy's time up before it began: hyprland.lua not walked, refused.
        let f = Fx::new("slowtree");
        f.put(".config/hypr/hyprland.lua", LUA);
        COPY_TIME_TEST.with(|t| t.set(Some(std::time::Duration::ZERO)));
        let (r, said) = f.run("");
        COPY_TIME_TEST.with(|t| t.set(None));
        assert_eq!(r, Err(1));
        assert_eq!(
            said,
            "omabox: up: --desktop desk: your hyprland.lua was not copied into the box: not walked: the copy took over 0 s\n"
        );
        // ...up after a part: the parts after it not walked, one line each.
        let f = Fx::new("slowtree2");
        f.put(".config/hypr/hyprland.lua", LUA);
        f.put(".config/DankMaterialShell/settings.json", b"{}\n");
        f.put("Pictures/a.png", PNG);
        let ctx = Ctx::new(b"up", b"/nonexistent".to_vec());
        let fh = f.fh.as_bytes().to_vec();
        let mut w = Walk::new(&ctx, f.bx.as_bytes(), fh.clone(), fh.clone());
        w.part(b"~/.config/hypr".to_vec(), HYPR);
        let _ = w.root(&cat!(fh, "/.config/hypr"), b".config/hypr").unwrap();
        w.deadline = std::time::Instant::now();
        w.part(b"the dms shell".to_vec(), SHELL);
        let _ = w
            .root(
                &cat!(fh, "/.config/DankMaterialShell"),
                b".config/DankMaterialShell",
            )
            .unwrap();
        w.part(b"the images they name".to_vec(), IMAGES);
        w.named(b"Pictures/a.png", false).unwrap();
        w.end_part();
        let out = String::from_utf8_lossy(&w.out).into_owned();
        assert_eq!(
            out,
            "copied .config/hypr/hyprland.lua\n\
             skipped .config/DankMaterialShell/...\t1 more not walked: the copy took over 30 s\n\
             skipped (the images they name)\t1 more not walked: the copy took over 30 s\n"
        );
        assert!(!f.there(".config/DankMaterialShell"), "{out}");
        assert!(!f.there("Pictures"), "{out}");
    }

    /// The pictures a desk shows (finding 710): DMS's session.json keys (a picture, a file:// one, a
    /// folder by a per-monitor key: its top-level pictures), Caelestia's path.txt, a Lua `swww img`,
    /// hyprpaper.conf's; under /usr left alone, elsewhere outside HOME said. What else a copied file
    /// names stays out: a bind's `nautilus ~/Downloads`, a folder in a Lua string, a path in a
    /// shell's settings under no wallpaper key, a commented command.
    #[test]
    fn named_images_found() {
        if !crate::exec::tools_or_skip("named_images_found", &["grep", "cp"]) {
            return;
        }
        let f = Fx::new("img");
        let fh = &f.fh;
        f.put(
            ".config/hypr/hyprland.lua",
            b"hl.exec_once(\"swww img --transition-fps 60 ~/Pictures/a.png\")\nhl.exec_once(\"dms run\")\n\
              -- hl.exec_once(\"swww img ~/Pictures/commented.png\")\n\
              hl.exec_once(\"swaybg -o DP-1 -i /mnt/data/w.jpg\")\n\
              hl.exec_once(\"swaybg -i /usr/share/backgrounds/x.png\")\n\
              hl.bind(\"SUPER + E\", hl.dsp.exec_cmd(\"nautilus ~/Downloads\"))\n\
              hl.bind(\"SUPER + P\", hl.dsp.exec_cmd(\"imv ~/Downloads/shot.png\"))\n\
              local d = \"~/Shots\"\n",
        );
        f.put(
            ".config/hypr/hyprpaper.conf",
            b"preload = ~/Pictures/h.png\nwallpaper = DP-1,~/Pictures/h.png\n# preload = ~/Pictures/no.png\n",
        );
        f.put(
            ".local/state/DankMaterialShell/session.json",
            format!(
                "{{\"wallpaperPath\": \"{fh}\\/Pictures\\/w.png\", \"wallpaperPathDark\": \"file://{fh}/Pictures/g.webp\", \
                 \"monitorWallpapers\": {{\"DP-1\": \"~/Walls\"}}, \"screenshotDir\": \"~/Shots\", \"isLightMode\": true}}\n"
            )
            .as_bytes(),
        );
        f.put(
            ".config/DankMaterialShell/settings.json",
            b"{\"launcherLogo\": \"~/Pictures/logo.png\"}\n",
        );
        for n in [
            "a.png",
            "w.png",
            "h.png",
            "commented.png",
            "no.png",
            "logo.png",
        ] {
            f.put(&format!("Pictures/{n}"), PNG);
        }
        f.put("Pictures/g.webp", b"RIFF0000WEBPVP8 ");
        f.put("Walls/1.jpg", b"\xff\xd8\xff\xe0");
        f.put("Walls/notes.txt", b"n\n");
        f.put("Walls/sub/2.jpg", b"\xff\xd8\xff\xe0");
        f.put("Downloads/shot.png", PNG);
        f.put("Downloads/scan.jpg", b"\xff\xd8\xff\xe0");
        f.put("Shots/s.png", PNG);
        let out = f.run("").0.unwrap();
        for r in [
            "Pictures/a.png",
            "Pictures/w.png",
            "Pictures/g.webp",
            "Pictures/h.png",
            "Walls/1.jpg",
        ] {
            assert!(f.real_file(r), "{r}: {out}");
        }
        for r in [
            "Pictures/commented.png",
            "Pictures/no.png",
            "Pictures/logo.png",
            "Walls/notes.txt",
            "Walls/sub",
            "Downloads",
            "Shots",
        ] {
            assert!(!f.there(r), "{r}: {out}");
        }
        assert!(
            out.contains("skipped /mnt/data/w.jpg\toutside your HOME"),
            "{out}"
        );
        assert!(!out.contains("/usr/share/backgrounds"), "{out}");
        assert!(!out.contains("Downloads"), "{out}");
        // (Caelestia keeps its wallpaper's path in a text file of its state)
        let f = Fx::new("img2");
        f.put(
            ".config/hypr/hyprland.lua",
            b"hl.exec_once(\"caelestia shell -d\")\n",
        );
        f.put(
            ".local/state/caelestia/wallpaper/path.txt",
            format!("{}/Pictures/c.png\n", f.fh).as_bytes(),
        );
        f.put("Pictures/c.png", PNG);
        let out = f.run("").0.unwrap();
        assert!(f.real_file("Pictures/c.png"), "{out}");
        assert!(out.ends_with("guess caelestia\tconfig"), "{out}");
    }

    /// On Omarchy's base, the wallpaper the desk shows (finding 709): a backgrounds folder that is a
    /// link (Omarchy's seeding leaves it out) copied, the box's current/background to its copy of
    /// the desk's pick; one elsewhere in HOME copied at its path; a secret's way, a picture outside
    /// HOME, the theme's own and a plain base: left alone.
    #[test]
    fn omarchy_wallpaper() {
        if !crate::exec::tools_or_skip("omarchy_wallpaper", &["grep", "cp"]) {
            return;
        }
        let cur = ".local/state/omarchy/current";
        let bg = |f: &Fx| {
            std::fs::read_link(f.in_box(&format!("{cur}/background")))
                .map(|l| l.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let desk = |tag: &str| {
            let f = Fx::new(tag);
            f.put(".config/hypr/hyprland.lua", LUA);
            f.put(&format!("{cur}/theme.name"), b"t\n");
            f
        };
        // The folder a link out of ~/.config, the desk's pick its second picture.
        let f = desk("wp1");
        f.put("walls/a.png", PNG);
        f.put("walls/b.jpg", b"\xff\xd8\xff\xe0");
        f.put("walls/intros/b.mp4", b"video");
        f.ln(&format!("{}/walls", f.fh), ".config/omarchy/backgrounds/t");
        f.ln(
            &format!("{}/walls/b.jpg", f.fh),
            &format!("{cur}/background"),
        );
        let out = f.run_omarchy();
        for r in ["a.png", "b.jpg"] {
            assert!(
                f.real_file(&format!(".config/omarchy/backgrounds/t/{r}")),
                "{r}: {out}"
            );
        }
        assert!(!f.there(".config/omarchy/backgrounds/t/intros"), "{out}");
        assert_eq!(
            bg(&f),
            "../../../../.config/omarchy/backgrounds/t/b.jpg",
            "{out}"
        );
        assert_eq!(
            std::fs::canonicalize(f.in_box(&format!("{cur}/background"))).unwrap(),
            std::fs::canonicalize(f.in_box(".config/omarchy/backgrounds/t/b.jpg")).unwrap(),
            "on the host it leads to the box's copy, not HOME's"
        );
        // (a plain base: no Omarchy, none of it)
        let f2 = desk("wp1p");
        f2.put("walls/b.jpg", b"\xff\xd8\xff\xe0");
        f2.ln(&format!("{}/walls", f2.fh), ".config/omarchy/backgrounds/t");
        f2.ln(
            &format!("{}/walls/b.jpg", f2.fh),
            &format!("{cur}/background"),
        );
        let out = f2.run("").0.unwrap();
        assert!(!f2.there(".config/omarchy/backgrounds"), "{out}");
        assert_eq!(bg(&f2), "", "{out}");
        // Elsewhere in HOME: copied at its path there.
        let f = desk("wp2");
        f.put("Pictures/w.png", PNG);
        f.ln(
            &format!("{}/Pictures/w.png", f.fh),
            &format!("{cur}/background"),
        );
        let out = f.run_omarchy();
        assert!(f.real_file("Pictures/w.png"), "{out}");
        assert_eq!(bg(&f), "../../../../Pictures/w.png", "{out}");
        // A secret's way, outside HOME, the theme's own: no copy, no link of ours.
        let f = desk("wp3");
        f.put(".ssh/w.png", PNG);
        f.ln(
            &format!("{}/.ssh/w.png", f.fh),
            &format!("{cur}/background"),
        );
        let out = f.run_omarchy();
        assert!(!f.there(".ssh"), "{out}");
        assert!(out.contains("skipped .ssh/w.png\t"), "{out}");
        assert_eq!(bg(&f), "", "{out}");
        let f = desk("wp4");
        f.put("../outside/w.png", PNG);
        f.ln(
            &format!("{}/outside/w.png", f.base),
            &format!("{cur}/background"),
        );
        let out = f.run_omarchy();
        assert!(
            out.contains(&format!(
                "skipped {}/outside/w.png\tyour wallpaper, outside your HOME",
                f.base
            )),
            "{out}"
        );
        assert_eq!(bg(&f), "", "{out}");
        let f = desk("wp5");
        f.put(&format!("{cur}/theme/backgrounds/1.png"), PNG);
        f.ln(
            &format!("{}/{cur}/theme/backgrounds/1.png", f.fh),
            &format!("{cur}/background"),
        );
        let out = f.run_omarchy();
        assert!(!out.contains("linked "), "{out}");
        assert!(!f.there(cur), "{out}");
    }

    /// A desk box's name check: keybinds.lua and session.json go in, a secret's names stay out.
    #[test]
    fn desk_names() {
        for n in [
            "keybinds.lua",
            "session.json",
            "auth.lua",
            "monkey.png",
            "hyprland.lua",
        ] {
            assert_eq!(seed::desk_box_name_refused(n.as_bytes()), None, "{n}");
        }
        for n in [
            "api-keys.env",
            ".env",
            ".env.local",
            "x.pem",
            "y.key",
            "a.gpg",
            "b.kdbx",
            "id_ed25519",
            "id_x",
            "tokens.json",
            "my-secret",
            "credentials",
            "passwords.txt",
            "cookies.sqlite",
            "private",
            "openai_api_key",
            "API-KEY",
            "apikey.txt",
            ".netrc",
            "authorized_keys",
        ] {
            assert_eq!(
                seed::desk_box_name_refused(n.as_bytes()),
                Some("a secret's name"),
                "{n}"
            );
        }
        assert!(seed::desk_box_name_refused(b".git").is_some());
        assert_eq!(
            seed::desk_box_name_refused(b"hyprland.lua.bak"),
            Some("a backup")
        );
    }

    /// Which shells: --shell first, then the Lua's strings (comments skipped, starters taken off),
    /// then units linked under the session targets; a named profile before a `{name}` one.
    #[test]
    fn detect_profiles() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../share/desk-shells");
        let all = deskshell::all(dir.as_bytes()).unwrap();
        let names = |found: &[Found]| -> Vec<String> {
            found
                .iter()
                .map(|f| {
                    let n = String::from_utf8_lossy(&all[f.i].name).into_owned();
                    match &f.name {
                        Some(q) => format!("{n}({})/{}", String::from_utf8_lossy(q), f.from),
                        None => format!("{n}/{}", f.from),
                    }
                })
                .collect()
        };
        let none = b"/nonexistent".as_slice();
        let lua = b"-- hl.exec_once(\"waybar\")\n\
            hl.exec_once(\"env A=1 setsid -f qs -c noctalia-shell\") -- \"dms run\"\n\
            hl.exec_once(\"uwsm app -- qs -c mybar\")\n\
            hl.exec_once(\"sh -c 'sleep 1; /usr/bin/caelestia shell -d'\")\n\
            hl.exec_once(\"echo dms run\")\n"
            .to_vec();
        let found = detect(&all, b"nohup dms run", &[lua], none);
        assert_eq!(
            names(&found),
            [
                "dms/--shell",
                "noctalia/config",
                "quickshell(mybar)/config",
                "caelestia/config"
            ]
        );
        // (units: a link under either session target's .wants)
        let f = Fx::new("units");
        let cfg = format!("{}/.config", f.fh);
        f.ln(
            "/usr/lib/systemd/user/dms.service",
            ".config/systemd/user/graphical-session.target.wants/dms.service",
        );
        f.ln(
            "/usr/lib/systemd/user/waybar.service",
            ".config/systemd/user/default.target.wants/waybar.service",
        );
        let found = detect(&all, b"", &[], cfg.as_bytes());
        assert_eq!(names(&found), ["dms/unit"]);
        // (each part's shell, read as desk-shell.lua reads it: deskshell::hits)
        let h = |s: &str| names(&detect(&all, s.as_bytes(), &[], none));
        assert_eq!(h("env A=1 B=2 exec dms run"), ["dms/--shell"]);
        assert_eq!(
            h("uwsm app -s b -- qs -c x && waybar"),
            ["quickshell(x)/--shell", "waybar/--shell"]
        );
        assert_eq!(
            h("bash -c 'nohup setsid --fork waybar'"),
            ["waybar/--shell"]
        );
        assert!(h("dms").is_empty(), "dms alone is not `dms run`");
    }

    /// A profile's folders: DMS found by its unit, its config and state copied; a profile folder
    /// refused is said, the rest goes on.
    #[test]
    fn copies_found_shells() {
        if !crate::exec::tools_or_skip("copies_found_shells", &["grep", "cp"]) {
            return;
        }
        let f = Fx::new("dms");
        f.put(".config/hypr/hyprland.lua", LUA);
        f.ln(
            "/usr/lib/systemd/user/dms.service",
            ".config/systemd/user/hyprland-session.target.wants/dms.service",
        );
        f.put(".config/DankMaterialShell/settings.json", b"{\"a\": 1}\n");
        f.put(".local/state/DankMaterialShell/session.json", b"{}\n");
        f.put(".cache/DankMaterialShell/x", b"x\n");
        let (r, said) = f.run("");
        let out = r.unwrap();
        assert!(
            f.real_file(".config/DankMaterialShell/settings.json"),
            "{out}"
        );
        assert!(
            f.real_file(".local/state/DankMaterialShell/session.json"),
            "{out}"
        );
        assert!(!f.there(".cache/DankMaterialShell"), "{out}");
        assert!(!f.there(".config/systemd"), "{out}");
        assert!(out.ends_with("guess dms\tunit"), "{out}");
        assert!(said.contains("(1 of ~/.config/hypr, 2 of the dms shell); its shell, first guessed: dms (from unit)"), "{said}");
    }

    /// An executable file at PATH (its folders made).
    fn prog(path: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
        std::fs::write(path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Finding 701: where a desk box looks for a program, as host paths: its HOME's ~/.local/bin,
    /// the caller's PATH as the box sees it (under HOME: the box HOME's; under /usr, /bin, /sbin: the
    /// host's; elsewhere: not the box's), /usr/bin; a path as the box sees it.
    #[test]
    fn box_path_finds_programs() {
        let f = Fx::new("boxpath");
        let homes = [format!("{}/", f.fh).into_bytes()];
        let (fh, bx, out) = (&f.fh, &f.bx, format!("{}/out", f.base));
        let caller = format!("{fh}/bin:/usr/local/bin:{out}:rel:{fh}:/usr/bin/:/opt/x");
        let dirs = box_path(bx.as_bytes(), &homes, caller.as_bytes());
        let shown: Vec<String> = dirs
            .iter()
            .map(|d| String::from_utf8_lossy(d).into_owned())
            .collect();
        assert_eq!(
            shown,
            [
                format!("{bx}/.local/bin"),
                format!("{bx}/bin"),
                "/usr/local/bin".into(),
                bx.clone(),
                "/usr/bin".into()
            ]
        );
        let found =
            |p: &str, qs: bool| program_found(p.as_bytes(), &dirs, bx.as_bytes(), &homes, qs);
        // (the host's own copy in HOME/bin, or a folder the box has not, is not the box's)
        prog(&format!("{fh}/bin/t-701"));
        prog(&format!("{out}/t-701"));
        assert!(!found("t-701", false));
        assert!(
            !found(&format!("{fh}/bin/t-701"), false),
            "a path: the box HOME's"
        );
        assert!(
            !found(&format!("{out}/t-701"), false),
            "a path outside what the box has"
        );
        prog(&format!("{bx}/bin/t-701"));
        assert!(
            found("t-701", false),
            "in the box HOME's copy of a folder of PATH"
        );
        assert!(found(&format!("{fh}/bin/t-701"), false));
        prog(&format!("{bx}/.local/bin/t-701b"));
        assert!(found("t-701b", false), "the box HOME's ~/.local/bin");
        std::fs::write(format!("{bx}/.local/bin/t-701c"), "").unwrap();
        assert!(!found("t-701c", false), "not executable");
        std::fs::create_dir_all(format!("{bx}/.local/bin/t-701d")).unwrap();
        assert!(!found("t-701d", false), "a folder");
        assert!(found("sh", false), "/usr/bin's");
        assert!(
            found("./rel/x", false),
            "a relative path: cannot tell, taken as there"
        );
        assert!(!found("omabox-no-such-program-701", false));
        assert!(
            found("qs", true) && found("quickshell", true),
            "up --quickshell's build"
        );
    }

    /// Finding 701: a shell found by its unit link, or the config, whose program the box does not
    /// have (a unit left when its package went): not guessed, its folders not copied, said once and
    /// skipped as `(shell NAME)`; with the program in the box HOME's ~/.local/bin, guessed.
    #[test]
    fn shell_not_installed() {
        if !crate::exec::tools_or_skip("shell_not_installed", &["grep", "cp"]) {
            return;
        }
        let f = Fx::new("noprog");
        let dir = format!("{}/profiles", f.base);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            format!("{dir}/t701"),
            "start omabox-t701-prog run\nunit omabox-t701.service\nconfig .config/t701\n",
        )
        .unwrap();
        std::fs::write(
            format!("{dir}/t701b"),
            "start omabox-t701b-prog\nconfig .config/t701b\n",
        )
        .unwrap();
        f.put(
            ".config/hypr/hyprland.lua",
            b"hl.exec_cmd(\"setsid -f omabox-t701b-prog --bar\")\n",
        );
        f.ln(
            "/usr/lib/systemd/user/omabox-t701.service",
            ".config/systemd/user/graphical-session.target.wants/omabox-t701.service",
        );
        f.put(".config/t701/a.json", b"{}\n");
        f.put(".config/t701b/a.json", b"{}\n");
        let path = format!("{}/bin", f.fh);
        prog(&format!("{path}/omabox-t701-prog"));
        let plan = SeedPlan {
            plain: true,
            desk_box: true,
            ..SeedPlan::default()
        };
        let (r, said) = f.run_in(&plan, &dir, Some(&path));
        let out = r.unwrap();
        assert!(!out.contains("\nguess "), "{out}");
        assert_eq!(
            lines(&out, "skipped "),
            [
                "(shell t701b)\tits program omabox-t701b-prog is not installed",
                "(shell t701)\tits program omabox-t701-prog is not installed"
            ],
            "{out}"
        );
        assert!(
            !f.there(".config/t701") && !f.there(".config/t701b"),
            "{out}"
        );
        assert_eq!(said.matches("not the t701 shell").count(), 1, "{said}");
        assert!(said.contains("up: not the t701 shell (its systemd unit is linked): its program omabox-t701-prog is not installed"), "{said}");
        assert!(said.contains("up: not the t701b shell (your Hyprland config starts it): its program omabox-t701b-prog is not installed"), "{said}");
        let rep = Report::parse(out.as_bytes());
        assert_eq!(rep.guess, None);
        assert_eq!(rep.missing(), [b"t701b".to_vec(), b"t701".to_vec()]);
        // (where the box finds it: the unit's shell guessed, its folder copied)
        prog(&format!("{}/bin/omabox-t701-prog", f.bx));
        let (r, _) = f.run_in(&plan, &dir, Some(&path));
        let out = r.unwrap();
        assert!(out.ends_with("guess t701\tunit"), "{out}");
        assert!(f.real_file(".config/t701/a.json"), "{out}");
        assert_eq!(Report::parse(out.as_bytes()).missing(), [b"t701b".to_vec()]);
        // (--shell's runs as asked: never left out)
        let shell = SeedPlan {
            shell_cmd: b"omabox-t701b-prog".to_vec(),
            ..plan
        };
        let (r, _) = f.run_in(&shell, &dir, Some(&path));
        assert!(r.unwrap().ends_with("guess t701b\t--shell"));
    }

    /// Refused, no copy: a Hyprland config folder that is a link to a secret store; a hyprland.lua,
    /// or a module it requires, that could not be copied. No config at all: nothing, said nothing.
    #[test]
    fn refusals() {
        if !crate::exec::tools_or_skip("refusals", &["grep", "cp"]) {
            return;
        }
        let f = Fx::new("ref");
        let (r, said) = f.run("");
        assert_eq!((r, said), (Ok(String::new()), String::new()));
        assert_eq!(
            f.pre(),
            Ok(()),
            "no hypr folder: no refusal here (deskguard says it)"
        );
        f.put(".gnupg/hypr/hyprland.lua", LUA);
        f.ln(&format!("{}/.gnupg/hypr", f.fh), ".config/hypr");
        let (r, said) = f.run("");
        assert_eq!(r, Err(1));
        assert!(said.starts_with(&format!("omabox: up: --desktop desk: your Hyprland config ({}/.config/hypr) cannot go into a box: it leads to {}/.gnupg/hypr: it is inside", f.fh, f.fh)), "{said}");
        // (finding 709: each refusal is precheck's too, before the box's folder, in the same words)
        assert_eq!(f.pre(), Err(said));
        let f = Fx::new("ref2");
        f.put(
            ".config/hypr/hyprland.lua",
            b"local token = \"abcdefghijkl\"\n",
        );
        let (r, said) = f.run("");
        assert_eq!(r, Err(1));
        assert_eq!(
            said,
            "omabox: up: --desktop desk: your hyprland.lua was not copied into the box: it holds what looks like a key, token or password\n"
        );
        assert_eq!(f.pre(), Err(said));
        let f = Fx::new("ref3");
        f.put(
            ".config/hypr/hyprland.lua",
            b"require(\"conf.binds\") -- require(\"gone\")\nrequire 'mine'\n",
        );
        f.put(".config/hypr/mine.lua", b"-- mine\n");
        f.put(".config/hypr/conf/binds.lua", &vec![b'-'; 2 * MIB as usize]);
        let (r, said) = f.run("");
        assert_eq!(r, Err(1));
        assert_eq!(
            said,
            "omabox: up: --desktop desk: ~/.config/hypr/conf/binds.lua, which your Hyprland config requires, was not copied into the box: over 1 MiB\n"
        );
        assert_eq!(f.pre(), Err(said));
        // (a module a module requires; a fine config passes)
        f.put(".config/hypr/conf/binds.lua", b"require('deep.x')\n");
        f.put(
            ".config/hypr/deep/x.lua",
            b"local password = \"hunter2hunter2\"\n",
        );
        assert_eq!(
            f.pre(),
            Err("omabox: up: --desktop desk: ~/.config/hypr/deep/x.lua, which your Hyprland config requires, was not copied into the box: it holds what looks like a key, token or password\n".into())
        );
        f.put(".config/hypr/deep/x.lua", b"-- x\n");
        assert_eq!(f.pre(), Ok(()));
        assert_eq!(
            requires(b"require(\"conf.binds\") -- require(\"gone\")\nrequire 'mine'\n"),
            [b"conf/binds".to_vec(), b"mine".to_vec()]
        );
    }

    /// A profile's `idle FILE KEY VALUE` (Caelestia's): its copy edited, the rest of it kept, said;
    /// the host's file as it was. A file that is no JSON object is left alone.
    #[test]
    fn idle_timeouts_off() {
        if !crate::exec::tools_or_skip("idle_timeouts_off", &["grep", "cp"]) {
            return;
        }
        let f = Fx::new("idle");
        f.put(".config/hypr/hyprland.lua", LUA);
        let host = br#"{"general": {"idle": {"timeouts": [{"timeout": 180, "idleAction": "lock"}]}}, "x": 1}"#;
        f.put(".config/caelestia/shell.json", host);
        let (r, said) = f.run("caelestia shell -d");
        let r = r.unwrap();
        assert!(said.contains("up: caelestia's own idle timeouts off in the box's ~/.config/caelestia/shell.json (general.idle.timeouts)"), "{said}");
        assert!(
            lines(&r, "edited ").contains(&".config/caelestia/shell.json\tgeneral.idle.timeouts"),
            "{r}"
        );
        let v: Value = serde_json::from_slice(
            &std::fs::read(f.in_box(".config/caelestia/shell.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(v["general"]["idle"]["timeouts"], serde_json::json!([]));
        assert_eq!(v["x"], serde_json::json!(1));
        assert_eq!(
            std::fs::read(format!("{}/.config/caelestia/shell.json", f.fh)).unwrap(),
            host
        );
        assert!(f.real_file(".config/caelestia/shell.json"));
        // (idle_edit alone: made on the way, a string value, a file that is not JSON left)
        let h = f.bx.as_bytes();
        std::fs::write(f.in_box("a.json"), b"{}").unwrap();
        assert_eq!(
            idle_edit(h, b"a.json b.c off"),
            Ok(Some(b"a.json".to_vec()))
        );
        assert_eq!(
            std::fs::read_to_string(f.in_box("a.json"))
                .unwrap()
                .split_whitespace()
                .collect::<String>(),
            r#"{"b":{"c":"off"}}"#
        );
        std::fs::write(f.in_box("t.json"), b"not json").unwrap();
        assert_eq!(idle_edit(h, b"t.json a []"), Ok(None));
        assert_eq!(idle_edit(h, b"../t.json a []"), Ok(None));
        assert_eq!(idle_edit(h, b"none.json a []"), Ok(None));
    }

    /// idle_edit never follows a link a box (a save restored first) left in its HOME (finding 710):
    /// its temp file's name a link to a host file, a folder on the way a link to a host folder. The
    /// host's files as they were; the second said, not edited.
    #[test]
    fn idle_edit_follows_no_link() {
        if !crate::exec::tools_or_skip("idle_edit_follows_no_link", &["grep", "cp"]) {
            return;
        }
        let host = br#"{"general": {"idle": {"timeouts": [1]}}}"#;
        // (1) The host has the shell's config; the box's HOME a planted shell.json.omabox-idle.
        let f = Fx::new("idlelink1");
        f.put(".config/hypr/hyprland.lua", LUA);
        f.put(".config/caelestia/shell.json", host);
        f.put(".bashrc", b"# the real one\n");
        std::fs::create_dir_all(f.in_box(".config/caelestia")).unwrap();
        symlink(
            format!("{}/.bashrc", f.fh),
            f.in_box(".config/caelestia/shell.json.omabox-idle"),
        )
        .unwrap();
        let (r, _) = f.run("caelestia shell -d");
        let r = r.unwrap();
        assert_eq!(
            std::fs::read(format!("{}/.bashrc", f.fh)).unwrap(),
            b"# the real one\n",
            "{r}"
        );
        assert!(f.real_file(".config/caelestia/shell.json"), "{r}");
        let v: Value = serde_json::from_slice(
            &std::fs::read(f.in_box(".config/caelestia/shell.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(v["general"]["idle"]["timeouts"], serde_json::json!([]));
        // (2) No config of the shell's on the host: a planted folder link stays, is not followed.
        let f = Fx::new("idlelink2");
        f.put(".config/hypr/hyprland.lua", LUA);
        f.put(".config/x/shell.json", host);
        std::fs::create_dir_all(f.in_box(".config")).unwrap();
        symlink(format!("{}/.config/x", f.fh), f.in_box(".config/caelestia")).unwrap();
        let (r, said) = f.run("caelestia shell -d");
        let r = r.unwrap();
        assert_eq!(
            std::fs::read(format!("{}/.config/x/shell.json", f.fh)).unwrap(),
            host,
            "{r}"
        );
        assert!(
            !lines(&r, "edited ").iter().any(|l| l.contains("caelestia")),
            "{r}"
        );
        assert!(said.contains("up: caelestia's own idle timeouts not turned off in the box's ~/.config/caelestia/shell.json: a link on its way (~/.config/caelestia), not followed"), "{said}");
        // (3) The file itself a link: not followed either.
        let f = Fx::new("idlelink3");
        f.put("victim.json", b"{}");
        std::fs::create_dir_all(f.in_box("c")).unwrap();
        symlink(format!("{}/victim.json", f.fh), f.in_box("c/a.json")).unwrap();
        assert!(idle_edit(f.bx.as_bytes(), b"c/a.json k []").is_err());
        // (H itself a link, as up's box dir `home` is: followed, it is omabox's own)
        std::fs::write(f.in_box("c/b.json"), b"{}").unwrap();
        let hl = format!("{}/home-link", f.base);
        symlink(&f.bx, &hl).unwrap();
        assert_eq!(
            idle_edit(hl.as_bytes(), b"c/b.json k []"),
            Ok(Some(b"c/b.json".to_vec()))
        );
        assert_eq!(
            std::fs::read(format!("{}/victim.json", f.fh)).unwrap(),
            b"{}"
        );
    }
}
