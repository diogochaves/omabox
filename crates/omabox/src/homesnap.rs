//! A box HOME recorded and compared (#446): `omabox snapshot` and `omabox diff`. What an app, a
//! plugin or an install script wrote into the box HOME, as a list: added, removed, changed.
//!
//! The walk reads what the box wrote (S7): every folder is opened from its parent's fd with
//! O_NOFOLLOW | O_DIRECTORY, every entry is looked at with fstatat(AT_SYMLINK_NOFOLLOW), a link is
//! recorded with its target (readlinkat) and never followed, only a regular file is opened (from its
//! folder's fd, O_NOFOLLOW | O_NONBLOCK, its own fstat checked) and only one of HASH_MAX bytes at most
//! is hashed: a FIFO is recorded as one and never opened. The walk stays on the HOME's file system and
//! stops at MAX_ENTRIES entries, MAX_DEPTH folders deep, HASH_TOTAL bytes hashed (past it, files are
//! compared by size and mtime, as large ones are). A snapshot is kept in the box's folder on the host
//! (`<box dir>/snapshots`), which the box never sees, not in its HOME, which it can change.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

/// Past this many entries the walk stops (the snapshot says it was cut).
pub const MAX_ENTRIES: usize = 100_000;
/// Folders deeper than this are recorded but not opened.
pub const MAX_DEPTH: usize = 48;
/// Regular files up to this size are hashed; larger ones are compared by size and mtime.
pub const HASH_MAX: u64 = 1 << 20;
/// At most this many bytes hashed in one walk; past it, files are compared by size and mtime.
pub const HASH_TOTAL: u64 = 256 << 20;
/// A link's target is read up to this many bytes.
const LINK_MAX: usize = 4096;
/// The first line of a snapshot file.
const MAGIC: &[u8] = b"omabox-snapshot 1\n";

/// One entry of a HOME: what it is, its permissions, its size, its mtime (ns) and, for a regular file
/// hashed, its content's hash; a link's target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// `f` a regular file, `d` a folder, `l` a link, `p` a FIFO, `s` a socket, `o` anything else.
    pub kind: u8,
    pub mode: u32,
    pub size: u64,
    pub mtime: i64,
    pub hash: Option<u64>,
    pub target: Vec<u8>,
}

/// A HOME: its entries by path (relative to the HOME, `/`-separated), and whether the walk was cut
/// (a cap reached) or some folder could not be read.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Snap {
    pub entries: BTreeMap<Vec<u8>, Entry>,
    pub cut: bool,
    pub unread: usize,
}

/// FNV-1a, 64 bits: a change of content, not a defence (the box writes the file; it gains nothing by
/// hiding a change from the one who asked).
fn fnv(data: &[u8], mut h: u64) -> u64 {
    for b in data {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}
const FNV_START: u64 = 0xcbf2_9ce4_8422_2325;

fn c_name(n: &[u8]) -> Option<CString> {
    CString::new(n).ok()
}

/// openat(DIR, NAME, FLAGS) as an owned fd.
fn open_at(dir: RawFd, name: &CString, flags: libc::c_int) -> Option<OwnedFd> {
    // SAFETY: DIR is an open fd of ours, NAME NUL-terminated; openat only adds a descriptor, owned
    // from here.
    let fd = unsafe { libc::openat(dir, name.as_ptr(), flags | libc::O_CLOEXEC) };
    // SAFETY: a new fd openat returned, owned by no one else.
    (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) })
}

/// fstatat(DIR, NAME) without following a link.
fn lstat_at(dir: RawFd, name: &CString) -> Option<libc::stat> {
    // SAFETY: an all-zero stat is a valid out buffer; DIR open, NAME NUL-terminated.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: as above.
    let r = unsafe { libc::fstatat(dir, name.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) };
    (r == 0).then_some(st)
}

/// The names in the folder open at FD (not `.` and `..`), at most LEFT of them.
fn names(fd: &OwnedFd, left: usize) -> Option<Vec<Vec<u8>>> {
    // SAFETY: dup of an fd of ours; fdopendir takes the copy and closedir closes it.
    let d = unsafe { libc::dup(fd.as_raw_fd()) };
    if d < 0 {
        return None;
    }
    // SAFETY: D is an open folder fd of ours; on failure it is still ours to close.
    let dir = unsafe { libc::fdopendir(d) };
    if dir.is_null() {
        // SAFETY: D is ours, not taken by fdopendir.
        unsafe { libc::close(d) };
        return None;
    }
    let mut out = Vec::new();
    loop {
        // SAFETY: DIR is an open DIR stream of ours; readdir's entry is read before the next call.
        let e = unsafe { libc::readdir(dir) };
        if e.is_null() {
            break;
        }
        // SAFETY: d_name is NUL-terminated within the entry readdir returned.
        let n = unsafe { std::ffi::CStr::from_ptr((*e).d_name.as_ptr()) }.to_bytes();
        if n == b"." || n == b".." {
            continue;
        }
        if out.len() > left {
            break;
        }
        out.push(n.to_vec());
    }
    // SAFETY: DIR came from fdopendir; closedir closes it and its fd.
    unsafe { libc::closedir(dir) };
    out.sort();
    Some(out)
}

fn kind_of(mode: u32) -> u8 {
    match mode & libc::S_IFMT {
        libc::S_IFREG => b'f',
        libc::S_IFDIR => b'd',
        libc::S_IFLNK => b'l',
        libc::S_IFIFO => b'p',
        libc::S_IFSOCK => b's',
        _ => b'o',
    }
}

struct Walk {
    snap: Snap,
    dev: u64,
    hashed: u64,
    /// Paths (relative) whose files are not hashed: noise, compared by size and mtime.
    skip_hash: Noise,
}

impl Walk {
    /// The hash of regular file NAME in DIR, read through its own fd: none when it is no regular file
    /// by that fd (swapped since the look), larger than HASH_MAX, or the walk's budget is spent.
    fn hash(&mut self, dir: RawFd, name: &CString, size: u64) -> Option<u64> {
        if size > HASH_MAX || self.hashed + size > HASH_TOTAL {
            return None;
        }
        let fl = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY;
        let f = std::fs::File::from(open_at(dir, name, fl)?);
        let m = f.metadata().ok()?;
        if !m.is_file() {
            return None;
        }
        let mut buf = Vec::new();
        (&f).take(HASH_MAX + 1).read_to_end(&mut buf).ok()?;
        if buf.len() as u64 > HASH_MAX {
            return None;
        }
        self.hashed += buf.len() as u64;
        Some(fnv(&buf, fnv(&(buf.len() as u64).to_le_bytes(), FNV_START)))
    }

    fn dir(&mut self, fd: &OwnedFd, rel: &[u8], depth: usize) {
        let left = MAX_ENTRIES.saturating_sub(self.snap.entries.len());
        let Some(list) = names(fd, left) else {
            self.snap.unread += 1;
            return;
        };
        for n in list {
            if self.snap.entries.len() >= MAX_ENTRIES {
                self.snap.cut = true;
                return;
            }
            let Some(c) = c_name(&n) else { continue };
            let Some(st) = lstat_at(fd.as_raw_fd(), &c) else {
                continue;
            };
            let path = if rel.is_empty() {
                n.clone()
            } else {
                crate::cat!(rel, "/", n)
            };
            let kind = kind_of(st.st_mode);
            let size = u64::try_from(st.st_size).unwrap_or(0);
            let mut e = Entry {
                kind,
                mode: st.st_mode & 0o7777,
                size: if kind == b'd' { 0 } else { size },
                mtime: st.st_mtime.saturating_mul(1_000_000_000) + st.st_mtime_nsec,
                hash: None,
                target: Vec::new(),
            };
            match kind {
                b'f' if !self.skip_hash.matches(&path) => {
                    e.hash = self.hash(fd.as_raw_fd(), &c, size)
                }
                b'l' => {
                    let mut buf = vec![0u8; LINK_MAX];
                    // SAFETY: readlinkat into a buffer of ours of the length given; fd open, c
                    // NUL-terminated.
                    let r = unsafe {
                        libc::readlinkat(
                            fd.as_raw_fd(),
                            c.as_ptr(),
                            buf.as_mut_ptr().cast(),
                            LINK_MAX,
                        )
                    };
                    if r > 0 {
                        buf.truncate(r as usize);
                        e.target = buf;
                    }
                }
                _ => {}
            }
            self.snap.entries.insert(path.clone(), e);
            if kind == b'd' {
                if depth >= MAX_DEPTH || st.st_dev != self.dev {
                    self.snap.cut |= depth >= MAX_DEPTH;
                    continue;
                }
                let fl = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK;
                match open_at(fd.as_raw_fd(), &c, fl) {
                    Some(sub) => self.dir(&sub, &path, depth + 1),
                    None => self.snap.unread += 1,
                }
            }
        }
    }
}

/// The HOME at HOME (a folder the box cannot see the path of: `<HOMES>/NAME/home`), walked. None
/// when it cannot be opened as a folder (a link there is not followed).
pub fn walk(home: &[u8]) -> Option<Snap> {
    let c = c_name(home)?;
    let fl = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK;
    let fd = open_at(libc::AT_FDCWD, &c, fl)?;
    let dev = std::fs::File::from(fd.try_clone().ok()?).metadata().ok()?;
    let mut w = Walk {
        snap: Snap::default(),
        dev: std::os::unix::fs::MetadataExt::dev(&dev),
        hashed: 0,
        skip_hash: Noise::default_list(),
    };
    w.dir(&fd, b"", 0);
    Some(w.snap)
}

/// A snapshot as kept in its file: MAGIC, a line `cut UNREAD` when the walk was cut or some folder
/// unread, then one record an entry: `KIND MODE SIZE MTIME HASH` (HASH 16 hex digits, or `-`), NUL,
/// the path, NUL, the link's target (empty but for a link), NUL. A name can hold any byte but NUL.
pub fn encode(s: &Snap) -> Vec<u8> {
    let mut v = MAGIC.to_vec();
    v.extend_from_slice(format!("{} {}\n", u8::from(s.cut), s.unread).as_bytes());
    for (p, e) in &s.entries {
        let h = e
            .hash
            .map_or_else(|| "-".to_string(), |h| format!("{h:016x}"));
        v.extend_from_slice(
            format!(
                "{} {:o} {} {} {h}\0",
                e.kind as char, e.mode, e.size, e.mtime
            )
            .as_bytes(),
        );
        v.extend_from_slice(p);
        v.push(0);
        v.extend_from_slice(&e.target);
        v.push(0);
    }
    v
}

/// A snapshot file read back; None when it is not one.
pub fn decode(v: &[u8]) -> Option<Snap> {
    let rest = v.strip_prefix(MAGIC)?;
    let nl = rest.iter().position(|&c| c == b'\n')?;
    let head = std::str::from_utf8(&rest[..nl]).ok()?;
    let mut hw = head.split(' ');
    let mut s = Snap {
        cut: hw.next()? == "1",
        unread: hw.next()?.parse().ok()?,
        ..Snap::default()
    };
    let mut parts = rest[nl + 1..].split(|&c| c == 0);
    while let Some(h) = parts.next() {
        // (the empty piece after the last record's NUL ends it; one anywhere else is no snapshot)
        if h.is_empty() {
            return parts.next().is_none().then_some(s);
        }
        let (p, t) = (parts.next()?, parts.next()?);
        let h = std::str::from_utf8(h).ok()?;
        let f: Vec<&str> = h.split(' ').collect();
        let [k, m, sz, mt, hs] = f.as_slice() else {
            return None;
        };
        let e = Entry {
            kind: *k.as_bytes().first()?,
            mode: u32::from_str_radix(m, 8).ok()?,
            size: sz.parse().ok()?,
            mtime: mt.parse().ok()?,
            hash: if *hs == "-" {
                None
            } else {
                Some(u64::from_str_radix(hs, 16).ok()?)
            },
            target: t.to_vec(),
        };
        if p.is_empty() || s.entries.len() >= MAX_ENTRIES {
            return None;
        }
        s.entries.insert(p.to_vec(), e);
    }
    Some(s)
}

/// What is left out of a diff: patterns of path parts relative to the HOME, each part a name or a
/// glob with `*` (any run of characters but `/`), a pattern covering the paths it names and all
/// below them.
#[derive(Debug, Clone, Default)]
pub struct Noise(Vec<Vec<Vec<u8>>>);

/// What a box changes in its HOME on its own, measured in idle boxes, Omarchy's and plain (finding
/// 732): caches; the logs omabox keeps at the HOME's top; the Omarchy shell's own state (its
/// notifications, clipboard images, monitor workspaces, wallpaper intro), Hyprland's `lastVersion`;
/// what the box's session writes as it starts (its keyring, PipeWire's and WirePlumber's settings,
/// dconf's database, written by the GNOME theme step).
pub const NOISE: &[&str] = &[
    ".cache",
    "*.log",
    "*.log.old",
    ".local/state/omarchy/background-intro.*",
    ".local/state/omarchy/clipboard-images",
    ".local/state/omarchy/monitor-workspaces*",
    ".local/state/omarchy/notifications",
    // (Omarchy 4.5's shell: its agents' usage, read as it starts; mise, as a shell activates it)
    ".local/state/omarchy/agents",
    ".local/state/mise",
    ".local/share/hyprland/lastVersion",
    // (Omarchy 4.0's Hyprland writes its update notice's time beside it)
    ".local/share/hyprland/lastNag",
    ".local/share/keyrings/login.keyring",
    ".local/share/keyrings/default",
    ".config/pipewire/*/90-omabox.conf",
    ".config/wireplumber/*/90-omabox.conf",
    ".config/dconf/user",
];

/// Folders the box's session makes, left out when they came (or went) with nothing in them.
pub const HOLLOW: &[&str] = &[".local/bin"];

/// Whether glob G (`*` any run of characters) matches name N.
fn glob(g: &[u8], n: &[u8]) -> bool {
    match g.iter().position(|&c| c == b'*') {
        None => g == n,
        Some(i) => {
            if !n.starts_with(&g[..i]) {
                return false;
            }
            let (g, n) = (&g[i + 1..], &n[i..]);
            (0..=n.len()).any(|k| glob(g, &n[k..]))
        }
    }
}

impl Noise {
    pub fn default_list() -> Noise {
        let mut n = Noise::default();
        for p in NOISE {
            n.add(p.as_bytes());
        }
        n
    }

    /// Adds pattern P (relative to the HOME: a leading `~/`, `./` or `/` is dropped). False for one
    /// that names nothing (empty, or a `..` part).
    pub fn add(&mut self, p: &[u8]) -> bool {
        let p = p
            .strip_prefix(b"~/")
            .or_else(|| p.strip_prefix(b"./"))
            .unwrap_or(p);
        let parts: Vec<Vec<u8>> = p
            .split(|&c| c == b'/')
            .filter(|c| !c.is_empty() && *c != b".")
            .map(<[u8]>::to_vec)
            .collect();
        if parts.is_empty() || parts.iter().any(|c| c == b"..") {
            return false;
        }
        self.0.push(parts);
        true
    }

    /// Whether path P (relative to the HOME) is one of the patterns or below one.
    pub fn matches(&self, p: &[u8]) -> bool {
        let parts: Vec<&[u8]> = p.split(|&c| c == b'/').collect();
        self.0
            .iter()
            .any(|pat| pat.len() <= parts.len() && pat.iter().zip(&parts).all(|(g, n)| glob(g, n)))
    }
}

/// The areas a diff is grouped in, in the order it shows them: a path's area is the first whose
/// folder (or file) it is or is below; `elsewhere in HOME` for the rest (writes outside the XDG dirs).
pub const AREAS: &[(&str, &str)] = &[
    (".config/autostart", "autostart (~/.config/autostart)"),
    (
        ".config/systemd/user",
        "systemd user units (~/.config/systemd/user)",
    ),
    (".config/mimeapps.list", "mime associations (mimeapps.list)"),
    (
        ".local/share/applications/mimeapps.list",
        "mime associations (mimeapps.list)",
    ),
    (
        ".local/share/applications",
        "desktop entries (~/.local/share/applications)",
    ),
    (
        ".config/omarchy",
        "the shell's settings (~/.config/omarchy)",
    ),
    (".config", "~/.config"),
    (".local/share", "~/.local/share"),
    (".local/state", "~/.local/state"),
    (".cache", "~/.cache"),
];
pub const ELSEWHERE: &str = "elsewhere in HOME";

/// The area of path P (relative to the HOME).
pub fn area(p: &[u8]) -> &'static str {
    AREAS
        .iter()
        .find(|(a, _)| crate::paths::at_or_under(p, a.as_bytes()))
        .map_or(ELSEWHERE, |(_, l)| l)
}

/// One line of a diff: what happened to a path (`added`, `removed`, `changed`), what it is (in B, or
/// in A when removed), what changed (`content`, `mode`, `type`, `target`) and, for a folder that came
/// or went (or a path that became one), how many entries are in it, which are not lines of their own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub op: &'static str,
    pub kind: u8,
    pub what: Vec<&'static str>,
    pub inside: usize,
}

fn ch(op: &'static str, kind: u8, what: Vec<&'static str>) -> Change {
    Change {
        op,
        kind,
        what,
        inside: 0,
    }
}

/// What changed between entries A and B of one path: none for the same. A folder's own size and times
/// never count; a file not hashed on both sides is compared by size and mtime.
fn changed(ea: &Entry, eb: &Entry) -> Vec<&'static str> {
    let mut what = Vec::new();
    if ea.kind != eb.kind {
        what.push("type");
        return what;
    }
    match eb.kind {
        b'f' => {
            let differ = match (ea.hash, eb.hash) {
                (Some(x), Some(y)) => x != y || ea.size != eb.size,
                _ => ea.size != eb.size || ea.mtime != eb.mtime,
            };
            if differ {
                what.push("content");
            }
        }
        b'l' if ea.target != eb.target => what.push("target"),
        _ => {}
    }
    if ea.mode != eb.mode && eb.kind != b'l' {
        what.push("mode");
    }
    what
}

/// The topmost of HEADS that P is below (not P itself), if any.
fn topmost(heads: &BTreeSet<Vec<u8>>, p: &[u8]) -> Option<Vec<u8>> {
    let mut top = None;
    let mut q = p;
    while let Some(i) = q.iter().rposition(|&c| c == b'/') {
        q = &q[..i];
        if heads.contains(q) {
            top = Some(q.to_vec());
        }
    }
    top
}

/// The diff of A and B: each change by path, in path order; a new (or gone) folder collapsed to its
/// topmost new (or gone) one with a count of what is in it; the paths NOISE matches left out (how many:
/// the second value, counted before collapsing).
pub fn diff(a: &Snap, b: &Snap, noise: &Noise) -> (Vec<(Vec<u8>, Change)>, usize) {
    let mut raw: Vec<(Vec<u8>, Change)> = Vec::new();
    for (p, eb) in &b.entries {
        match a.entries.get(p) {
            None => raw.push((p.clone(), ch("added", eb.kind, Vec::new()))),
            Some(ea) => {
                let what = changed(ea, eb);
                if !what.is_empty() {
                    raw.push((p.clone(), ch("changed", eb.kind, what)));
                }
            }
        }
    }
    for (p, ea) in &a.entries {
        if !b.entries.contains_key(p) {
            raw.push((p.clone(), ch("removed", ea.kind, Vec::new())));
        }
    }
    raw.sort_by(|x, y| x.0.cmp(&y.0));
    let mut left_out = 0;
    let mut quiet: BTreeSet<(bool, Vec<u8>)> = BTreeSet::new();
    raw.retain(|(p, c)| {
        let n = noise.matches(p);
        if n {
            left_out += 1;
            quiet.insert((c.op == "removed", p.clone()));
        }
        !n
    });
    // A folder that came (or went) with nothing in it but noise is noise too (the session's
    // ~/.config/pipewire/client.conf.d, in a box whose ~/.config it made); a HOLLOW one, with nothing
    // in it at all. Deepest first, so a folder of such folders goes too.
    if !noise.0.is_empty() {
        let mut dirs: Vec<(bool, Vec<u8>)> = raw
            .iter()
            .filter(|(_, c)| c.kind == b'd' && c.op != "changed")
            .map(|(p, c)| (c.op == "removed", p.clone()))
            .collect();
        dirs.sort_by_key(|(_, p)| std::cmp::Reverse(p.iter().filter(|&&c| c == b'/').count()));
        for (gone, d) in dirs {
            let kept = raw
                .iter()
                .any(|(p, c)| (c.op == "removed") == gone && crate::paths::below(p, &d));
            let hushed = quiet
                .iter()
                .any(|(g, p)| *g == gone && crate::paths::below(p, &d));
            let hollow = HOLLOW.iter().any(|h| d == h.as_bytes());
            if !kept && (hushed || hollow) {
                raw.retain(|(p, c)| !(p == &d && (c.op == "removed") == gone));
                quiet.insert((gone, d));
                left_out += 1;
            }
        }
    }
    // A folder that came (or became one) holds what came below it; one that went, what went.
    let (mut came, mut went) = (BTreeSet::new(), BTreeSet::new());
    for (p, c) in &raw {
        if c.op == "removed" && c.kind == b'd' {
            went.insert(p.clone());
        } else if c.op != "removed" && c.kind == b'd' && (c.op == "added" || c.what == ["type"]) {
            came.insert(p.clone());
        }
    }
    let mut counts: BTreeMap<(bool, Vec<u8>), usize> = BTreeMap::new();
    let mut out = Vec::new();
    for (p, c) in raw {
        let heads = match c.op {
            "added" => Some((true, &came)),
            "removed" => Some((false, &went)),
            _ => None,
        };
        if let Some((side, h)) = heads
            && let Some(top) = topmost(h, &p)
        {
            *counts.entry((side, top)).or_default() += 1;
            continue;
        }
        out.push((p, c));
    }
    for (p, c) in &mut out {
        let side = c.op != "removed";
        c.inside = counts.get(&(side, p.clone())).copied().unwrap_or(0);
    }
    (out, left_out)
}

/// A kind's word, for the JSON and the text.
pub fn kind_word(k: u8) -> &'static str {
    match k {
        b'f' => "file",
        b'd' => "dir",
        b'l' => "link",
        b'p' => "fifo",
        b's' => "socket",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(kind: u8, size: u64, hash: Option<u64>) -> Entry {
        Entry {
            kind,
            mode: 0o644,
            size,
            mtime: 1,
            hash,
            target: Vec::new(),
        }
    }

    fn snap(list: &[(&str, Entry)]) -> Snap {
        Snap {
            entries: list
                .iter()
                .map(|(p, e)| (p.as_bytes().to_vec(), e.clone()))
                .collect(),
            ..Snap::default()
        }
    }

    fn lines(d: &[(Vec<u8>, Change)]) -> Vec<String> {
        d.iter()
            .map(|(p, c)| {
                format!(
                    "{} {} {} {} {}",
                    String::from_utf8_lossy(p),
                    c.op,
                    kind_word(c.kind),
                    c.what.join(","),
                    c.inside
                )
            })
            .collect()
    }

    #[test]
    fn globs() {
        assert!(glob(b"*.log", b"shell.log"));
        assert!(glob(b"*.log", b".log"));
        assert!(!glob(b"*.log", b"shell.log.old"));
        assert!(glob(b"a*b*c", b"aXbYc"));
        assert!(!glob(b"a*b*c", b"aXbY"));
        assert!(glob(b"x", b"x"));
        let n = Noise::default_list();
        assert!(n.matches(b".cache"));
        assert!(n.matches(b".cache/quickshell/x"));
        assert!(n.matches(b"shell.log"));
        assert!(!n.matches(b".config/app/shell.log"), "only at the top");
        assert!(!n.matches(b".cachex"));
        assert!(!n.matches(b".config/omarchy/shell.json"));
        let mut m = Noise::default();
        assert!(m.add(b"~/.config/app/"));
        assert!(m.add(b"./x/*.tmp"));
        assert!(!m.add(b"../etc"));
        assert!(!m.add(b"~/"));
        assert!(m.matches(b".config/app/a/b"));
        assert!(m.matches(b"x/1.tmp"));
        assert!(!m.matches(b".config/apps"));
    }

    #[test]
    fn diff_collapses_and_filters() {
        let a = snap(&[
            (".config", e(b'd', 0, None)),
            (".config/btop", e(b'd', 0, None)),
            (".config/btop/btop.conf", e(b'f', 3, Some(1))),
            (".config/old", e(b'd', 0, None)),
            (".config/old/a", e(b'f', 1, Some(2))),
            (".config/old/b", e(b'f', 1, Some(3))),
            (".cache", e(b'd', 0, None)),
            ("big", e(b'f', 5 << 20, None)),
        ]);
        let mut b = a.clone();
        b.entries.remove(b".config/old".as_slice());
        b.entries.remove(b".config/old/a".as_slice());
        b.entries.remove(b".config/old/b".as_slice());
        b.entries
            .insert(b".config/btop/btop.conf".to_vec(), e(b'f', 3, Some(9)));
        for p in [".config/app", ".config/app/x", ".config/app/x/y"] {
            b.entries.insert(p.as_bytes().to_vec(), e(b'd', 0, None));
        }
        b.entries
            .insert(b".config/app/x/y/z".to_vec(), e(b'f', 1, Some(4)));
        b.entries
            .insert(b".cache/junk".to_vec(), e(b'f', 1, Some(4)));
        let mut big = e(b'f', 5 << 20, None);
        big.mtime = 2;
        b.entries.insert(b"big".to_vec(), big);
        let (d, out) = diff(&a, &b, &Noise::default_list());
        assert_eq!(
            lines(&d),
            [
                ".config/app added dir  3",
                ".config/btop/btop.conf changed file content 0",
                ".config/old removed dir  2",
                "big changed file content 0",
            ]
        );
        assert_eq!(out, 1);
        let (d, out) = diff(&a, &b, &Noise::default());
        assert_eq!(d.len(), 5);
        assert_eq!(out, 0);
        // the same, no change at all: a touched file of the same content is none
        let mut t = a.clone();
        t.entries
            .get_mut(b".config/btop/btop.conf".as_slice())
            .unwrap()
            .mtime = 99;
        assert!(diff(&a, &t, &Noise::default()).0.is_empty());
    }

    /// What a plain box's session makes as it starts (finding 732): folders that hold only noise, an
    /// empty ~/.local/bin, all left out; an app's file in them is not, nor its folders.
    #[test]
    fn diff_leaves_out_noise_folders() {
        let a = snap(&[(".local", e(b'd', 0, None))]);
        let mut b = a.clone();
        for p in [
            ".config",
            ".config/pipewire",
            ".config/pipewire/client.conf.d",
            ".config/dconf",
            ".local/bin",
        ] {
            b.entries.insert(p.as_bytes().to_vec(), e(b'd', 0, None));
        }
        for p in [
            ".config/pipewire/client.conf.d/90-omabox.conf",
            ".config/dconf/user",
            "events.log",
        ] {
            b.entries.insert(p.as_bytes().to_vec(), e(b'f', 1, Some(1)));
        }
        let (d, out) = diff(&a, &b, &Noise::default_list());
        assert_eq!(lines(&d), Vec::<String>::new());
        assert_eq!(out, 8);
        let mut c = b.clone();
        c.entries
            .insert(b".config/app.conf".to_vec(), e(b'f', 1, Some(1)));
        c.entries
            .insert(b".local/bin/tool".to_vec(), e(b'f', 1, Some(1)));
        let (d, _) = diff(&a, &c, &Noise::default_list());
        assert_eq!(
            lines(&d),
            [".config added dir  1", ".local/bin added dir  1"]
        );
        // --all: everything
        assert_eq!(
            lines(&diff(&a, &b, &Noise::default()).0),
            [
                ".config added dir  5",
                ".local/bin added dir  0",
                "events.log added file  0"
            ]
        );
    }

    #[test]
    fn diff_types_modes_links() {
        let mut l = e(b'l', 3, None);
        l.target = b"/a".to_vec();
        let a = snap(&[
            ("x", e(b'f', 1, Some(1))),
            ("l", l.clone()),
            ("m", e(b'f', 1, Some(1))),
        ]);
        let mut b = a.clone();
        b.entries.insert(b"x".to_vec(), e(b'd', 0, None));
        b.entries.insert(b"x/in".to_vec(), e(b'f', 1, Some(1)));
        l.target = b"/b".to_vec();
        b.entries.insert(b"l".to_vec(), l);
        b.entries.get_mut(b"m".as_slice()).unwrap().mode = 0o755;
        let (d, _) = diff(&a, &b, &Noise::default());
        assert_eq!(
            lines(&d),
            [
                "l changed link target 0",
                "m changed file mode 0",
                "x changed dir type 1",
            ]
        );
    }

    #[test]
    fn encode_decode() {
        let mut l = e(b'l', 3, None);
        l.target = b"../t\nab\tc".to_vec();
        let s = Snap {
            cut: true,
            unread: 2,
            ..snap(&[
                ("a b\n\tc", e(b'f', 7, Some(u64::MAX))),
                ("lnk", l),
                ("d", e(b'd', 0, None)),
            ])
        };
        assert_eq!(decode(&encode(&s)), Some(s));
        assert_eq!(decode(&encode(&Snap::default())), Some(Snap::default()));
        assert_eq!(decode(b"nope"), None);
        let mut bad = encode(&snap(&[("a", e(b'f', 1, None))]));
        bad.truncate(bad.len() - 2);
        assert_eq!(decode(&bad), None);
    }

    #[test]
    fn areas() {
        assert_eq!(
            area(b".config/autostart/x.desktop"),
            "autostart (~/.config/autostart)"
        );
        assert_eq!(
            area(b".config/mimeapps.list"),
            "mime associations (mimeapps.list)"
        );
        assert_eq!(
            area(b".config/omarchy/plugins/p"),
            "the shell's settings (~/.config/omarchy)"
        );
        assert_eq!(area(b".config/x"), "~/.config");
        assert_eq!(area(b".configx"), ELSEWHERE);
        assert_eq!(
            area(b".local/share/applications/a.desktop"),
            "desktop entries (~/.local/share/applications)"
        );
        assert_eq!(area(b".myplugin"), ELSEWHERE);
    }

    /// The walk on a folder of the test's: a link (to a file and to a folder outside) recorded with
    /// its target and never followed, a FIFO recorded and never opened (it would hang a read), a
    /// file hashed, one over HASH_MAX not.
    #[test]
    fn walk_is_safe() {
        let t = std::env::temp_dir().join(format!("omabox-homesnap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let (home, out) = (t.join("home"), t.join("out"));
        std::fs::create_dir_all(home.join(".config/app")).unwrap();
        std::fs::create_dir_all(out.join("deep")).unwrap();
        std::fs::write(out.join("secret"), "s").unwrap();
        std::fs::write(out.join("deep/x"), "x").unwrap();
        std::fs::write(home.join(".config/app/conf"), "hello").unwrap();
        std::fs::write(home.join("big"), vec![0u8; (HASH_MAX + 1) as usize]).unwrap();
        std::os::unix::fs::symlink(out.join("secret"), home.join("lf")).unwrap();
        std::os::unix::fs::symlink(&out, home.join("ld")).unwrap();
        let fifo = CString::new(home.join("fifo").as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: mkfifo on a NUL-terminated path of ours.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let s = walk(home.as_os_str().as_encoded_bytes()).unwrap();
        let k: Vec<String> = s
            .entries
            .iter()
            .map(|(p, e)| {
                format!(
                    "{} {} {}",
                    String::from_utf8_lossy(p),
                    e.kind as char,
                    e.hash.is_some()
                )
            })
            .collect();
        assert_eq!(
            k,
            [
                ".config d false",
                ".config/app d false",
                ".config/app/conf f true",
                "big f false",
                "fifo p false",
                "ld l false",
                "lf l false",
            ]
        );
        assert_eq!(
            s.entries[b"ld".as_slice()].target,
            out.as_os_str().as_encoded_bytes()
        );
        assert!(!s.cut);
        // the HOME itself a link: not followed
        std::os::unix::fs::symlink(&home, t.join("hl")).unwrap();
        assert!(walk(t.join("hl").as_os_str().as_encoded_bytes()).is_none());
        let _ = std::fs::remove_dir_all(&t);
    }
}
