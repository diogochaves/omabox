//! The box's bwrap command (#324 wave 5a, #355), built only through methods that hold SECURITY.md's
//! rules by construction:
//! - S1, the seat: no bind's source or destination is one of the seat's paths (a DRM card node,
//!   `/dev/input`, `/run/seatd.sock`, the system bus's `/run/dbus`, your real `$XDG_RUNTIME_DIR`) or a
//!   folder holding one; devices go in only as render nodes and NVIDIA's own nodes (DevNode); the one
//!   card path a box has is an empty file of the box dir's (finding 294's placeholder), never the
//!   device (seat_path, check_bind).
//! - S4, host code read-only: a host folder goes in read-only (`--ro-bind`) or as an overlay whose
//!   writes are thrown away (`--tmp-overlay`); the only writable binds are the box's own runtime dir
//!   and HOME, and a --systemd box's own cgroup (own_rw, cgroup_rw).
//! - S3: what a user or a repo names goes through refuse_src and refuse_dest (user_ro); the fixed
//!   system binds (/usr, /etc, /sys) are a list of their own (sys_ro), and omabox's own files must be
//!   under the checkout or the box's own dirs (own_ro).
//!
//! The words come out in bash's order (cmd_up's `args`), so launch.sh is the same both ways.

use crate::cat;

/// Why P is the seat's (S1), or None: a DRM card node, the input devices, seatd's socket, the
/// system bus, the user's runtime dir (RUNTIME), at or under them.
pub fn seat_path(p: &[u8], runtime: &[u8]) -> Option<&'static str> {
    let p = lexical(p);
    let under = |root: &[u8]| p == root || p.starts_with(&cat!(root, "/"));
    if let Some(n) = p.strip_prefix(b"/dev/dri/card".as_slice()) {
        let n = n.split(|&c| c == b'/').next().unwrap_or_default();
        if !n.is_empty() && n.iter().all(u8::is_ascii_digit) {
            return Some("a DRM card node (the seat's display)");
        }
    }
    if under(b"/dev/input") {
        return Some("the input devices");
    }
    if under(b"/run/seatd.sock") {
        return Some("seatd's socket");
    }
    if under(b"/run/dbus") {
        return Some("the system bus");
    }
    if !runtime.is_empty() && under(&lexical(runtime)) {
        return Some("your runtime dir");
    }
    None
}

/// Why a bind of folder P would bring a seat path in (P holds one), or None.
pub fn holds_seat(p: &[u8], runtime: &[u8]) -> Option<&'static str> {
    let p = lexical(p);
    let holds = |x: &[u8]| p.as_slice() == b"/" || x.starts_with(&cat!(p, "/"));
    for (x, why) in [
        (b"/dev/dri/card0".as_slice(), "it holds the DRM card nodes"),
        (b"/dev/input", "it holds the input devices"),
        (b"/run/seatd.sock", "it holds seatd's socket"),
        (b"/run/dbus", "it holds the system bus"),
    ] {
        if holds(x) {
            return Some(why);
        }
    }
    if !runtime.is_empty() && holds(&lexical(runtime)) {
        return Some("it holds your runtime dir");
    }
    None
}

/// P made absolute-lexical: `.` and empty parts dropped, `..` taken back (no links followed: the
/// sources reach here resolved already; this keeps `/dev//input` and `/dev/./input` from slipping by).
pub fn lexical(p: &[u8]) -> Vec<u8> {
    let mut parts: Vec<&[u8]> = Vec::new();
    for c in p.split(|&c| c == b'/') {
        match c {
            b"" | b"." => {}
            b".." => {
                parts.pop();
            }
            _ => parts.push(c),
        }
    }
    let mut out = Vec::new();
    for c in parts {
        out.push(b'/');
        out.extend_from_slice(c);
    }
    if out.is_empty() {
        out.push(b'/');
    }
    out
}

/// Why D cannot be a box dir (finding 607), or None: D, as written or as it resolves, is or holds
/// the runtime dir (RUNTIME), HOME, `/`, or any seat path (`/dev`, `/run` among the folders that
/// hold one), or lies in one of the seat's paths but the runtime dir. A box dir is the runtime dir's
/// `omabox/NAME`; a source in it skips the seat check, so a D past those would let them in.
fn box_dir_refused(d: &[u8], runtime: &[u8], home: &[u8]) -> Option<&'static str> {
    let canon = |p: &[u8]| -> Vec<Vec<u8>> {
        let mut v = vec![lexical(p)];
        if let Ok(c) = std::fs::canonicalize(crate::shell::path(p)) {
            let c = lexical(crate::shell::bytes(&c));
            if !v.contains(&c) {
                v.push(c);
            }
        }
        v
    };
    let at_or_above = |x: &[u8], y: &[u8]| x == b"/" || y == x || y.starts_with(&cat!(x, "/"));
    for own in canon(d) {
        if own == b"/" {
            return Some("it is /");
        }
        if !runtime.is_empty() && canon(runtime).iter().any(|r| at_or_above(&own, r)) {
            return Some("it is or holds your runtime dir");
        }
        if !home.is_empty() && canon(home).iter().any(|h| at_or_above(&own, h)) {
            return Some("it is or holds your HOME");
        }
        if let Some(why) = seat_path(&own, b"").or_else(|| holds_seat(&own, b"")) {
            return Some(why);
        }
    }
    None
}

/// A device a box may have (S1): a render node, NVIDIA's numbered nodes, its control node and its
/// unified memory node (#149). Nothing else can be made one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevNode(Vec<u8>);

impl DevNode {
    pub fn new(p: &[u8]) -> Option<DevNode> {
        let digits = |s: &[u8]| !s.is_empty() && s.iter().all(u8::is_ascii_digit);
        let ok = p
            .strip_prefix(b"/dev/dri/renderD".as_slice())
            .is_some_and(digits)
            || p.strip_prefix(b"/dev/nvidia".as_slice())
                .is_some_and(digits)
            || p == b"/dev/nvidiactl"
            || p == b"/dev/nvidia-uvm";
        ok.then(|| DevNode(p.to_vec()))
    }

    pub fn path(&self) -> &[u8] {
        &self.0
    }
}

/// A bind the bwrap command is refused (one S1, S3 or S4 does not allow), in words.
#[derive(Debug, PartialEq, Eq)]
pub struct Refused(pub Vec<u8>);

/// The bwrap words, in order, each bind checked as it is added.
pub struct Bwrap {
    args: Vec<Vec<u8>>,
    /// The user's runtime dir (the seat's, S1), and the box's own dir in it (D): a source under the
    /// runtime dir is allowed only in D.
    runtime: Vec<u8>,
    own: Vec<u8>,
    /// The box's own writable folders (S4): its runtime dir and its HOME.
    rw: Vec<Vec<u8>>,
    /// Where omabox's own files may come from besides D: the checkout, a private aquamarine.
    own_roots: Vec<Vec<u8>>,
}

/// The fixed system binds (S3's list of its own): read-only, at their own path.
pub const SYS_RO: [&str; 3] = ["/usr", "/etc", "/sys"];

impl Bwrap {
    /// RUNTIME: the user's runtime dir; D: the box dir; RW: the box's runtime dir and HOME;
    /// OWN_ROOTS: where omabox's own files may be besides D. Refused for a D whose by-path
    /// exemption (a source in it skips the seat check) would cover the seat, HOME or the runtime
    /// dir (finding 607).
    pub fn new(
        runtime: &[u8],
        d: &[u8],
        rw: Vec<Vec<u8>>,
        own_roots: Vec<Vec<u8>>,
    ) -> Result<Bwrap, Refused> {
        if let Some(why) = box_dir_refused(d, runtime, &crate::shell::env_or_empty("HOME")) {
            return Err(Refused(cat!("refusing box dir ", d, ": ", why)));
        }
        Ok(Bwrap {
            args: Vec::new(),
            runtime: runtime.to_vec(),
            own: lexical(d),
            rw: rw.iter().map(|p| lexical(p)).collect(),
            own_roots: own_roots.iter().map(|p| lexical(p)).collect(),
        })
    }

    pub fn into_args(self) -> Vec<Vec<u8>> {
        self.args
    }

    /// Plain words (`--unshare-pid`, `--proc /proc`, ...): never a bind, in a release build too
    /// (#365): a bind handed here would skip every check. Only omabox's own literals come here; the
    /// host's name goes in through `hostname`.
    pub fn words(&mut self, w: &[&[u8]]) {
        if let Some(f) = w.iter().find(|f| is_bind(f)) {
            panic!(
                "bwrap: {} handed to words(), past the bind checks",
                String::from_utf8_lossy(f)
            );
        }
        self.args.extend(w.iter().map(|x| x.to_vec()));
    }

    /// `--hostname NAME`: the host's name, whatever it is (a value, never read as an option).
    pub fn hostname(&mut self, name: &[u8]) {
        self.args.push(b"--hostname".to_vec());
        self.args.push(name.to_vec());
    }

    pub fn setenv(&mut self, k: &[u8], v: &[u8]) {
        self.args.push(b"--setenv".to_vec());
        self.args.push(k.to_vec());
        self.args.push(v.to_vec());
    }

    /// The seat check for one bind (S1): its source neither the seat's nor holding it (a source in
    /// the runtime dir only in the box's own dir), its destination none of the seat's.
    fn check_bind(&self, src: &[u8], dest: &[u8]) -> Result<(), Refused> {
        let s = lexical(src);
        let in_own = s == self.own || s.starts_with(&cat!(self.own, "/"));
        let why = if in_own {
            None
        } else {
            seat_path(&s, &self.runtime).or_else(|| holds_seat(&s, &self.runtime))
        };
        if let Some(why) = why {
            return Err(Refused(cat!(
                "refusing to mount ",
                src,
                " into a box: ",
                why
            )));
        }
        // (a destination at the runtime dir's path is the box's own run dir, bound there)
        let d = lexical(dest);
        if let Some(why) = seat_path(&d, b"").or_else(|| holds_seat(&d, b"")) {
            return Err(Refused(cat!(
                "refusing to mount at ",
                dest,
                " in a box: ",
                why
            )));
        }
        Ok(())
    }

    fn push_bind(&mut self, how: &[u8], src: &[u8], dest: &[u8]) -> Result<(), Refused> {
        self.check_bind(src, dest)?;
        self.args.push(how.to_vec());
        self.args.push(src.to_vec());
        self.args.push(dest.to_vec());
        Ok(())
    }

    /// One of SYS_RO, read-only at its own path.
    pub fn sys_ro(&mut self, p: &str) -> Result<(), Refused> {
        if !SYS_RO.contains(&p) {
            return Err(Refused(cat!(p, " is not one of the box's system folders")));
        }
        self.push_bind(b"--ro-bind", p.as_bytes(), p.as_bytes())
    }

    /// One of omabox's own files or folders (the checkout's share and tools, the box dir's, a private
    /// aquamarine), read-only.
    pub fn own_ro(&mut self, src: &[u8], dest: &[u8]) -> Result<(), Refused> {
        let s = lexical(src);
        let ok = self
            .own_roots
            .iter()
            .chain(std::iter::once(&self.own))
            .any(|r| s == *r || s.starts_with(&cat!(r, "/")));
        if !ok {
            return Err(Refused(cat!(src, " is not one of omabox's own files")));
        }
        self.push_bind(b"--ro-bind", src, dest)
    }

    /// What a user, a repo or a desk names (S3): refused by refuse_src or refuse_dest, else
    /// read-only (S4).
    pub fn user_ro(&mut self, src: &[u8], dest: &[u8]) -> Result<(), Refused> {
        if let Some(why) = crate::repo::refuse_src(src) {
            return Err(Refused(cat!(
                "refusing to mount ",
                src,
                " into a box: ",
                why
            )));
        }
        if let Some(why) = crate::upspec::refuse_dest(dest) {
            return Err(Refused(cat!(
                "refusing to mount ",
                src,
                " at ",
                dest,
                " in a box: ",
                why
            )));
        }
        self.push_bind(b"--ro-bind", src, dest)
    }

    /// A read-only bind under a rule of its own (a desk font folder at its place in the box HOME, a
    /// jailed caller's cover from the box dir): refuse_src unless it is the box dir's, and the seat
    /// check.
    pub fn checked_ro(&mut self, src: &[u8], dest: &[u8]) -> Result<(), Refused> {
        let s = lexical(src);
        let own = s == self.own || s.starts_with(&cat!(self.own, "/"));
        if !own && let Some(why) = crate::repo::refuse_src(src) {
            return Err(Refused(cat!(
                "refusing to mount ",
                src,
                " into a box: ",
                why
            )));
        }
        self.push_bind(b"--ro-bind", src, dest)
    }

    /// The box's own runtime dir or HOME, writable (S4: the only writable host folders).
    pub fn own_rw(&mut self, src: &[u8], dest: &[u8]) -> Result<(), Refused> {
        if !self.rw.contains(&lexical(src)) {
            return Err(Refused(cat!(
                "refusing to mount ",
                src,
                " writable into a box: only its own runtime dir and HOME are"
            )));
        }
        self.push_bind(b"--bind", src, dest)
    }

    /// A --systemd box's own cgroup, writable as its /sys/fs/cgroup (finding 61): bash's
    /// `@CGROUP@`, filled in inside the scope, never a host path named here.
    pub fn cgroup_rw(&mut self) {
        self.args.push(b"--bind".to_vec());
        self.args.push(b"@CGROUP@".to_vec());
        self.args.push(b"/sys/fs/cgroup".to_vec());
    }

    /// A device node (S1: DevNode's only).
    pub fn dev(&mut self, n: &DevNode) -> Result<(), Refused> {
        self.push_bind(b"--dev-bind", n.path(), n.path())
    }

    /// NVIDIA's EGL stats the GPU's card node (finding 294): an empty read-only file of the box
    /// dir's at DEST, /dev/dri/cardN, never the device. FILE must be that file, empty and regular.
    pub fn card_placeholder(&mut self, file: &[u8], dest: &[u8]) -> Result<(), Refused> {
        let s = lexical(file);
        let ok_card = dest
            .strip_prefix(b"/dev/dri/card".as_slice())
            .is_some_and(|n| !n.is_empty() && n.iter().all(u8::is_ascii_digit));
        let ok_file = s.starts_with(&cat!(self.own, "/"))
            && std::fs::symlink_metadata(crate::shell::path(&s))
                .is_ok_and(|m| m.file_type().is_file() && m.len() == 0);
        if !ok_card || !ok_file {
            return Err(Refused(cat!(
                "refusing a card placeholder ",
                file,
                " at ",
                dest,
                ": not an empty file of the box's"
            )));
        }
        self.args.push(b"--ro-bind".to_vec());
        self.args.push(file.to_vec());
        self.args.push(dest.to_vec());
        Ok(())
    }

    /// A folder at its own path whose writes are thrown away (`--overlay`, S4).
    pub fn tmp_overlay(&mut self, p: &[u8]) -> Result<(), Refused> {
        if let Some(why) = crate::repo::refuse_src(p).or_else(|| crate::upspec::refuse_dest(p)) {
            return Err(Refused(cat!("refusing to overlay ", p, " in a box: ", why)));
        }
        self.check_bind(p, p)?;
        for w in [b"--overlay-src".as_slice(), p, b"--tmp-overlay", p] {
            self.args.push(w.to_vec());
        }
        Ok(())
    }

    /// /etc as a read-only overlay with the box dir's ETC on top (findings 135, 363).
    pub fn etc_overlay(&mut self, etc: &[u8]) -> Result<(), Refused> {
        self.check_bind(b"/etc", b"/etc")?;
        self.check_bind(etc, b"/etc")?;
        for w in [
            b"--overlay-src".as_slice(),
            b"/etc",
            b"--overlay-src",
            etc,
            b"--ro-overlay",
            b"/etc",
        ] {
            self.args.push(w.to_vec());
        }
        Ok(())
    }

    /// One of the box dir's /etc files over the host's (the box's own omarchy.conf, locale.conf).
    pub fn etc_file(&mut self, src: &[u8], dest: &[u8]) -> Result<(), Refused> {
        let s = lexical(src);
        if !s.starts_with(&cat!(self.own, "/etc/")) || !dest.starts_with(b"/etc/") {
            return Err(Refused(cat!(src, " is not one of the box's /etc files")));
        }
        self.push_bind(b"--ro-bind", src, dest)
    }

    /// A resolv.conf the host's /etc/resolv.conf links to under /run (finding 52), at its own path.
    pub fn resolv(&mut self, p: &[u8]) -> Result<(), Refused> {
        if !p.starts_with(b"/run/") {
            return Err(Refused(cat!(p, " is not a resolver file under /run")));
        }
        self.push_bind(b"--ro-bind", p, p)
    }
}

/// Whether W is one of bwrap's words that put a host path or an fd's content in the box (what
/// `words` must never be handed): every bind, overlay and fd copy bwrap 0.11 has, and `--args`, which
/// reads more words from an fd, past every check (#365).
fn is_bind(w: &[u8]) -> bool {
    matches!(
        w,
        b"--bind"
            | b"--ro-bind"
            | b"--dev-bind"
            | b"--bind-try"
            | b"--ro-bind-try"
            | b"--dev-bind-try"
            | b"--bind-fd"
            | b"--ro-bind-fd"
            | b"--overlay-src"
            | b"--overlay"
            | b"--ro-overlay"
            | b"--tmp-overlay"
            | b"--bind-data"
            | b"--ro-bind-data"
            | b"--file"
            | b"--args"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const RT: &[u8] = b"/run/user/1000";

    #[test]
    fn seat_paths() {
        for p in [
            "/dev/dri/card0",
            "/dev/dri/card12",
            "/dev/input",
            "/dev/input/event3",
            "/dev//input/./mice",
            "/run/seatd.sock",
            "/run/dbus",
            "/run/dbus/system_bus_socket",
            "/run/user/1000",
            "/run/user/1000/wayland-1",
            "/dev/dri/../dri/card1",
        ] {
            assert!(seat_path(p.as_bytes(), RT).is_some(), "{p}");
        }
        for p in [
            "/dev/dri/renderD128",
            "/dev/dri/cardx",
            "/run/user/10000",
            "/usr",
            "/run/systemd/resolve/stub-resolv.conf",
        ] {
            assert!(seat_path(p.as_bytes(), RT).is_none(), "{p}");
        }
        for p in ["/", "/dev", "/dev/dri", "/run", "/run/user"] {
            assert!(holds_seat(p.as_bytes(), RT).is_some(), "{p}");
        }
        assert!(holds_seat(b"/usr", RT).is_none());
    }

    #[test]
    fn devices() {
        for p in [
            "/dev/dri/renderD128",
            "/dev/nvidia0",
            "/dev/nvidiactl",
            "/dev/nvidia-uvm",
        ] {
            assert!(DevNode::new(p.as_bytes()).is_some(), "{p}");
        }
        for p in [
            "/dev/dri/card0",
            "/dev/dri/renderD",
            "/dev/dri/renderD1x",
            "/dev/input/event0",
            "/dev/nvidia-modeset",
            "/dev/dri/renderD128/../card0",
            "/dev/nvidia",
        ] {
            assert!(DevNode::new(p.as_bytes()).is_none(), "{p}");
        }
    }

    fn bw() -> Bwrap {
        Bwrap::new(
            RT,
            b"/run/user/1000/omabox/b",
            vec![
                b"/run/user/1000/omabox/b/run".to_vec(),
                b"/home/u/.cache/omabox/b/home".to_vec(),
            ],
            vec![b"/home/u/omabox".to_vec()],
        )
        .unwrap()
    }

    #[test]
    fn binds() {
        let mut b = bw();
        assert!(b.sys_ro("/usr").is_ok());
        assert!(b.sys_ro("/dev").is_err());
        assert!(
            b.own_rw(b"/run/user/1000/omabox/b/run", RT).is_ok(),
            "the box's run dir at the runtime path"
        );
        assert!(b.own_rw(RT, RT).is_err(), "the real runtime dir");
        assert!(b.own_rw(b"/home/u/src", b"/home/u/src").is_err());
        assert!(
            b.own_ro(b"/home/u/omabox/share", b"/opt/omabox/share")
                .is_ok()
        );
        assert!(b.own_ro(b"/home/u/.ssh", b"/opt/omabox/share").is_err());
        assert!(
            b.own_ro(b"/run/user/1000/omabox/b/../c/run", b"/x")
                .is_err()
        );
        assert!(b.resolv(b"/run/systemd/resolve/stub-resolv.conf").is_ok());
        assert!(b.resolv(b"/run/dbus/system_bus_socket").is_err());
        assert!(b.resolv(b"/etc/passwd").is_err());
        assert_eq!(b.into_args()[0], b"--ro-bind");
    }

    // Finding 607: a box dir whose by-path exemption would cover the seat, HOME or the runtime dir
    // is refused, as written or as it resolves; a box dir of the runtime dir's, or anywhere plain, is
    // taken.
    #[test]
    fn box_dir_never_covers_the_seat() {
        let home = crate::shell::env_or_empty("HOME");
        let parent = home[..home.iter().rposition(|&c| c == b'/').unwrap_or(0)].to_vec();
        let base = std::env::temp_dir().join(format!("omabox-box-dir-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let link = base.join("to-run");
        let _ = std::os::unix::fs::symlink("/run", &link);
        let link = crate::shell::bytes(&link).to_vec();
        let new = |d: &[u8]| Bwrap::new(RT, d, Vec::new(), Vec::new()).map(|_| ());
        for d in [
            b"/".as_slice(),
            b"/run",
            b"/dev",
            b"/run/user",
            RT,
            b"/run/user/1000/",
            b"/run/user/1000/omabox/..",
            b"/run/user/1000/omabox/b/../../..",
            b"/dev/dri",
            b"/dev/dri/card0",
            b"/dev/input/x",
            b"/run/dbus",
            b"/run/seatd.sock",
            &home,
            &parent,
            &link,
        ] {
            assert!(new(d).is_err(), "{:?}", String::from_utf8_lossy(d));
        }
        assert!(new(b"/run/user/1000/omabox/b").is_ok());
        assert!(new(crate::shell::bytes(&base)).is_ok());
        assert!(new(&cat!(home, "/.cache/omabox/b")).is_ok());
        std::fs::remove_dir_all(&base).unwrap();
    }

    // #365: words() refuses every bind-like word in a release build too, the ones the first list
    // lacked among them.
    #[test]
    fn words_takes_no_bind() {
        for w in [
            "--bind",
            "--ro-bind",
            "--dev-bind",
            "--bind-try",
            "--ro-bind-try",
            "--dev-bind-try",
            "--bind-fd",
            "--ro-bind-fd",
            "--overlay-src",
            "--overlay",
            "--ro-overlay",
            "--tmp-overlay",
            "--bind-data",
            "--ro-bind-data",
            "--file",
            "--args",
        ] {
            let r = std::panic::catch_unwind(|| {
                let mut b = bw();
                b.words(&[b"--unshare-pid", w.as_bytes(), b"/dev", b"/dev"]);
                b.into_args()
            });
            assert!(r.is_err(), "words() took {w}");
        }
        let mut b = bw();
        b.words(&[
            b"--proc",
            b"/proc",
            b"--dev",
            b"/dev",
            b"--symlink",
            b"usr/bin",
            b"/bin",
        ]);
        b.hostname(b"--bind");
        assert_eq!(
            b.into_args().len(),
            9,
            "plain words, and a host named like an option"
        );
    }

    /// The biased paths (#365): the seat's own paths exactly and the folders that hold them, paths
    /// under the box dir, its writable dirs and the own roots (and out of them by `..`), and loose
    /// ones built of the seat's names.
    fn path_strategy() -> impl Strategy<Value = Vec<u8>> {
        let loose = prop::collection::vec(
            prop::sample::select(vec![
                "dev",
                "dri",
                "card0",
                "card7",
                "renderD128",
                "input",
                "event1",
                "run",
                "dbus",
                "seatd.sock",
                "user",
                "1000",
                "..",
                ".",
                "",
                "usr",
                "home",
                "u",
                "tmp",
                "nvidia0",
            ]),
            1..6,
        )
        .prop_map(|v| format!("/{}", v.join("/")).into_bytes());
        let exact = prop::sample::select(vec![
            "/dev/dri/card0",
            "/dev/dri/card12",
            "/dev/dri//card0",
            "/dev/input",
            "/dev/input/event1",
            "/run/seatd.sock",
            "/run/dbus",
            "/run/dbus/system_bus_socket",
            "/run/user/1000",
            "/run/user/1000/",
            "/run/user/1000/wayland-1",
            "/",
            "/dev",
            "/dev/dri",
            "/run",
            "/run/user",
            "/dev/dri/renderD128",
            "/dev/nvidia0",
            "/usr",
            "/run/systemd/resolve/stub-resolv.conf",
        ])
        .prop_map(|p| p.as_bytes().to_vec());
        let rooted = (
            prop::sample::select(vec![
                "/run/user/1000/omabox/b",
                "/run/user/1000/omabox/b/run",
                "/run/user/1000/omabox/b/etc",
                "/home/u/.cache/omabox/b/home",
                "/home/u/omabox",
                "/opt/aq/lib",
            ]),
            prop::collection::vec(
                prop::sample::select(vec!["share", "x", "..", ".", "card0", "input", "dbus"]),
                0..4,
            ),
        )
            .prop_map(|(r, t)| {
                let mut p = r.to_string();
                for c in t {
                    p.push('/');
                    p.push_str(c);
                }
                p.into_bytes()
            });
        prop_oneof![3 => exact, 3 => rooted, 2 => loose]
    }

    /// A box's roots, some of them the seat's (an own root or a writable dir a broken caller could
    /// give): own_ro and own_rw then reach check_bind with seat paths (#365).
    fn roots_strategy() -> impl Strategy<Value = (Vec<Vec<u8>>, Vec<Vec<u8>>)> {
        let pool = vec![
            "/home/u/omabox",
            "/opt/aq/lib",
            "/home/u/.cache/omabox/b/home",
            "/dev",
            "/dev/dri",
            "/dev/input",
            "/run",
            "/run/dbus",
            "/run/user/1000",
            "/",
        ];
        (
            prop::collection::vec(prop::sample::select(pool.clone()), 1..3),
            prop::collection::vec(prop::sample::select(pool), 1..3),
        )
            .prop_map(|(r, w)| {
                let v = |x: Vec<&str>| x.iter().map(|p| p.as_bytes().to_vec()).collect::<Vec<_>>();
                let mut rw = vec![b"/run/user/1000/omabox/b/run".to_vec()];
                rw.extend(v(w));
                (v(r), rw)
            })
    }

    // #365: check_bind's own seat logic, against the oracle written apart from it: a bind is let
    // through exactly when its source is the box dir's or touches no seat path, and its destination
    // touches none. Both sides come up often (counted).
    #[test]
    fn check_bind_is_the_oracle() {
        let [ok, refused] =
            oracle::run_counted(512, (path_strategy(), path_strategy()), |(s, d), n| {
                let b = bw();
                let own = oracle::under(&s, b"/run/user/1000/omabox/b");
                let want = (own || !oracle::touches_seat(&s, RT)) && !oracle::touches_seat(&d, b"");
                let got = b.check_bind(&s, &d);
                prop_assert_eq!(
                    got.is_ok(),
                    want,
                    "{:?} -> {:?}: {:?}",
                    String::from_utf8_lossy(&s),
                    String::from_utf8_lossy(&d),
                    got
                );
                oracle::bump(n, usize::from(got.is_err()));
                Ok(())
            });
        assert!(ok >= 64 && refused >= 64, "ok {ok}, refused {refused}");
    }

    // S1: whatever a bind is asked for, through every method, no seat path is ever a mount's source
    // or destination in what comes out: the oracle reads every word as bwrap would (#365). own_ro and
    // own_rw reach check_bind with seat paths (roots of the seat's) and are refused there (counted).
    #[test]
    fn no_seat_path_gets_through() {
        let calls = prop::collection::vec(
            (0u8..12, path_strategy(), path_strategy(), 0usize..4),
            1..10,
        );
        let [_, by_seat] =
            oracle::run_counted(256, (roots_strategy(), calls), |((roots, rw), calls), n| {
                let mut b = Bwrap::new(RT, b"/run/user/1000/omabox/b", rw.clone(), roots.clone())
                    .map_err(|e| {
                    TestCaseError::fail(String::from_utf8_lossy(&e.0).into_owned())
                })?;
                for (k, s, d, i) in &calls {
                    // (10, 11: a file of one of the box's own roots, one of its writable dirs)
                    let root = cat!(roots[i % roots.len()], "/share");
                    let rw = &rw[i % rw.len()];
                    let r = match k {
                        10 => b.own_ro(&root, d),
                        11 => b.own_rw(rw, d),
                        0 => b.user_ro(s, d),
                        1 => b.own_ro(s, d),
                        2 => b.own_rw(s, d),
                        3 => b.checked_ro(s, d),
                        4 => b.tmp_overlay(s),
                        5 => b.resolv(s),
                        6 => b.etc_overlay(s),
                        7 => b.etc_file(s, d),
                        8 => b.sys_ro(&String::from_utf8_lossy(s)),
                        _ => match DevNode::new(s) {
                            Some(n) => b.dev(&n),
                            None => Ok(()),
                        },
                    };
                    // (refused by the seat check itself, past the method's own rule)
                    if matches!(k, 1 | 2 | 10 | 11)
                        && r.as_ref().is_err_and(|e| {
                            (crate::exit::contains(&e.0, b" into a box: ")
                                && !crate::exit::contains(&e.0, b" writable into a box: "))
                                || e.0.starts_with(b"refusing to mount at ")
                        })
                    {
                        oracle::bump(n, 1);
                    }
                }
                let args = b.into_args();
                if let Err(e) = oracle::seat_ok(&args, RT, b"/run/user/1000/omabox/b") {
                    return Err(TestCaseError::fail(e));
                }
                Ok(())
            });
        assert!(
            by_seat >= 32,
            "own_ro/own_rw refused by the seat check {by_seat} times"
        );
    }

    // The oracle itself: what it reads in bwrap's words, and what it refuses to read.
    #[test]
    fn oracle_reads_bwrap() {
        let w = |v: &[&str]| v.iter().map(|x| x.as_bytes().to_vec()).collect::<Vec<_>>();
        let own = b"/run/user/1000/omabox/b";
        assert!(
            oracle::seat_ok(
                &w(&[
                    "--hostname",
                    "--bind",
                    "--ro-bind",
                    "/usr",
                    "/usr",
                    "--dev",
                    "/dev",
                    "--setenv",
                    "XDG_RUNTIME_DIR",
                    "/run/user/1000",
                    "--bind",
                    "/run/user/1000/omabox/b/run",
                    "/run/user/1000",
                    "--ro-bind",
                    "/run/user/1000/omabox/b/card-placeholder",
                    "/dev/dri/card1",
                    "--dev-bind",
                    "/dev/dri/renderD128",
                    "/dev/dri/renderD128",
                    "--bind",
                    "@CGROUP@",
                    "/sys/fs/cgroup",
                    "/opt/omabox/share/session.sh",
                    "--bind",
                    "/dev",
                    "/dev",
                ]),
                RT,
                own
            )
            .is_ok()
        );
        for bad in [
            &["--ro-bind", "/run/dbus", "/x"][..],
            &["--ro-bind", "/run", "/x"],
            &["--ro-bind", "/run/user/1000/../1000", "/x"],
            &["--ro-bind", "/x", "/dev/input/event0"],
            &["--ro-bind", "/x", "/dev"],
            &["--overlay-src", "/dev", "--tmp-overlay", "/x"],
            &["--overlay-src", "/x", "--tmp-overlay", "/dev"],
            &["--overlay-src", "/x", "--ro-overlay", "/run/dbus"],
            &["--dev-bind", "/dev/dri/card0", "/dev/dri/card0"],
            &["--dev-bind", "/dev/dri/renderD128", "/dev/dri/renderD129"],
            &["--ro-bind", "/home/u/x", "/dev/dri/card0"],
            &["--overlay", "/a", "/b", "/c"],
            &["--file", "5", "/x"],
            &["--args", "5"],
            &["--bind-data", "5", "/x"],
            &["--made-up", "/x"],
            &["--ro-bind", "/x"],
        ] {
            assert!(oracle::seat_ok(&w(bad), RT, own).is_err(), "{bad:?}");
        }
        assert!(oracle::touches_seat(b"/dev/dri/card3/x", b""));
        assert!(!oracle::touches_seat(b"/dev/dri/cardx", b""));
        assert!(!oracle::touches_seat(b"/run/user/10000", RT));
        assert!(oracle::touches_seat(b"/run/user", RT));
        assert!(!oracle::touches_seat(b"/run/user", b""));
    }
}

/// The tests' oracle for S1 (#365), written apart from seat_path, holds_seat and lexical, so a slip
/// in those is not mirrored here: bwrap's words read as bwrap 0.11 reads them, each mount's host
/// source and box destination checked against the seat's paths part by part.
#[cfg(test)]
pub(crate) mod oracle {
    use proptest::prelude::*;

    /// F over CASES values of S, as proptest! runs it (a failure shrunk and said), with two
    /// counters F bumps: what the cases reached, for a test to say its generators are not idle.
    pub fn run_counted<S: Strategy>(
        cases: u32,
        s: S,
        f: impl Fn(S::Value, &std::cell::Cell<[u32; 2]>) -> Result<(), TestCaseError>,
    ) -> [u32; 2] {
        let n = std::cell::Cell::new([0u32; 2]);
        let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
            failure_persistence: None,
            ..ProptestConfig::with_cases(cases)
        });
        if let Err(e) = runner.run(&s, |v| f(v, &n)) {
            panic!("{e}");
        }
        n.get()
    }

    pub fn bump(n: &std::cell::Cell<[u32; 2]>, i: usize) {
        let mut c = n.get();
        c[i] += 1;
        n.set(c);
    }

    /// P's parts: empty and `.` ones dropped, `..` taking the one before back.
    fn parts(p: &[u8]) -> Vec<&[u8]> {
        let mut out: Vec<&[u8]> = Vec::new();
        for c in p.split(|&c| c == b'/') {
            if c == b".." {
                out.pop();
            } else if !c.is_empty() && c != b"." {
                out.push(c);
            }
        }
        out
    }

    /// Whether P is ROOT or below it, part by part.
    pub fn under(p: &[u8], root: &[u8]) -> bool {
        parts(p).starts_with(&parts(root))
    }

    /// Whether a mount of P would show any of the seat: P a DRM card node (or under one), a folder
    /// holding them (`/dev/dri` and above), at, under or above `/dev/input`, `/run/seatd.sock`,
    /// `/run/dbus` or RT (the user's runtime dir; empty for a destination, where the box's own run
    /// dir sits at its path).
    pub fn touches_seat(p: &[u8], rt: &[u8]) -> bool {
        let c = parts(p);
        let card = |n: &[u8]| {
            n.strip_prefix(b"card".as_slice())
                .is_some_and(|d| !d.is_empty() && d.iter().all(u8::is_ascii_digit))
        };
        if c.len() >= 3 && c[0] == b"dev" && c[1] == b"dri" && card(c[2]) {
            return true;
        }
        if [b"dev".as_slice(), b"dri"].starts_with(&c) {
            return true;
        }
        let mut roots: Vec<Vec<&[u8]>> = vec![
            vec![b"dev", b"input"],
            vec![b"run", b"seatd.sock"],
            vec![b"run", b"dbus"],
        ];
        if !rt.is_empty() {
            roots.push(parts(rt));
        }
        roots.iter().any(|r| c.starts_with(r) || r.starts_with(&c))
    }

    /// A mount in bwrap's words: how, its host source (none for an overlay's destination), where.
    pub struct Mount {
        pub how: Vec<u8>,
        pub src: Option<Vec<u8>>,
        pub dest: Vec<u8>,
    }

    /// Every mount in ARGS, read with bwrap's arities up to the command (the first word that is not
    /// an option). Err for a word omabox must never write (an fd's content, `--args`, a writable
    /// `--overlay`), one this table does not know, or words cut short.
    pub fn mounts(args: &[Vec<u8>]) -> Result<Vec<Mount>, String> {
        let mut out = Vec::new();
        let mut i = 0;
        let arg = |i: usize| {
            args.get(i)
                .cloned()
                .ok_or_else(|| format!("{} cut short", String::from_utf8_lossy(&args[i - 1])))
        };
        while i < args.len() {
            let w = args[i].as_slice();
            let n = match w {
                b"--unshare-user"
                | b"--unshare-user-try"
                | b"--unshare-pid"
                | b"--unshare-ipc"
                | b"--unshare-uts"
                | b"--unshare-net"
                | b"--unshare-cgroup"
                | b"--unshare-cgroup-try"
                | b"--unshare-all"
                | b"--share-net"
                | b"--die-with-parent"
                | b"--new-session"
                | b"--as-pid-1"
                | b"--clearenv"
                | b"--disable-userns"
                | b"--assert-userns-disabled"
                | b"--level-prefix" => 0,
                b"--info-fd" | b"--block-fd" | b"--sync-fd" | b"--json-status-fd"
                | b"--userns-block-fd" | b"--hostname" | b"--uid" | b"--gid" | b"--unsetenv"
                | b"--cap-add" | b"--cap-drop" | b"--perms" | b"--size" | b"--exec-label"
                | b"--file-label" | b"--argv0" | b"--seccomp" | b"--add-seccomp-fd" | b"--proc"
                | b"--dev" | b"--tmpfs" | b"--mqueue" | b"--dir" | b"--remount-ro" | b"--chdir"
                | b"--lock-file" => 1,
                b"--setenv" | b"--symlink" | b"--chmod" => 2,
                b"--bind" | b"--ro-bind" | b"--dev-bind" | b"--bind-try" | b"--ro-bind-try"
                | b"--dev-bind-try" => {
                    out.push(Mount {
                        how: w.to_vec(),
                        src: Some(arg(i + 1)?),
                        dest: arg(i + 2)?,
                    });
                    2
                }
                b"--overlay-src" => {
                    out.push(Mount {
                        how: w.to_vec(),
                        src: Some(arg(i + 1)?),
                        dest: Vec::new(),
                    });
                    1
                }
                b"--tmp-overlay" | b"--ro-overlay" => {
                    out.push(Mount {
                        how: w.to_vec(),
                        src: None,
                        dest: arg(i + 1)?,
                    });
                    1
                }
                _ if !w.starts_with(b"--") => break,
                _ => {
                    return Err(format!(
                        "{} is not a word omabox writes",
                        String::from_utf8_lossy(w)
                    ));
                }
            };
            if i + n >= args.len() {
                return Err(format!("{} cut short", String::from_utf8_lossy(w)));
            }
            i += n + 1;
        }
        Ok(out)
    }

    /// S1 over ARGS: no mount's source touches the seat (but one in OWN, the box dir, or a --systemd
    /// box's cgroup), no destination does (but OWN's empty file at a card node's path), and a device
    /// goes in only as a render node or NVIDIA's, at its own path.
    pub fn seat_ok(args: &[Vec<u8>], rt: &[u8], own: &[u8]) -> Result<(), String> {
        let lossy = |p: &[u8]| String::from_utf8_lossy(p).into_owned();
        for m in mounts(args)? {
            let how = lossy(&m.how);
            if let Some(s) = &m.src {
                let ok = s.as_slice() == b"@CGROUP@"
                    && m.how == b"--bind"
                    && m.dest == b"/sys/fs/cgroup";
                if !ok && !under(s, own) && touches_seat(s, rt) {
                    return Err(format!("{how} source {}: the seat's", lossy(s)));
                }
            }
            if m.how == b"--overlay-src" {
                continue;
            }
            let d = parts(&m.dest);
            let placeholder = m.how == b"--ro-bind"
                && m.src.as_ref().is_some_and(|s| under(s, own))
                && d.len() == 3
                && d[0] == b"dev"
                && d[1] == b"dri"
                && d[2].starts_with(b"card");
            if !placeholder && touches_seat(&m.dest, b"") {
                return Err(format!("{how} at {}: the seat's", lossy(&m.dest)));
            }
            if m.how.starts_with(b"--dev-bind") {
                let s = m.src.unwrap_or_default();
                let node = |p: &[u8], pre: &[u8]| {
                    p.strip_prefix(pre)
                        .is_some_and(|d| !d.is_empty() && d.iter().all(u8::is_ascii_digit))
                };
                let dev = node(&s, b"/dev/dri/renderD")
                    || node(&s, b"/dev/nvidia")
                    || s == b"/dev/nvidiactl"
                    || s == b"/dev/nvidia-uvm";
                if !dev || s != m.dest {
                    return Err(format!(
                        "{how} {} {}: not a device a box may have",
                        lossy(&s),
                        lossy(&m.dest)
                    ));
                }
            }
        }
        Ok(())
    }
}
