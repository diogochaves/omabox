//! What a box needs installed (#324 wave 5a): omabox's tools, aquamarine and its fix, a --hyprland or
//! --quickshell build, an --omarchy tree, audio. Builds are read (readelf, ldd), never run here; a
//! jailed caller's files are followed only through a JailView (jailpath.rs's), and git is never run on
//! them (#198).

use crate::boxes::{BoxSys, RealBox};
use crate::cat;
use crate::exit::{Ctx, Exit, Res, muted, say};
use crate::jail::in_jail;
use crate::render::RenderNode;
use crate::shell;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::process::Stdio;

/// The aquamarine commit `omabox setup --aquamarine` builds (after v0.15.1), and AQ_BUILD, what a build
/// of it with omabox's own patches records (patches/aquamarine; finding 247).
pub const AQ_COMMIT: &str = "7bb8bdf4";
pub const AQ_BUILD: &str = "7bb8bdf4+keys+layout+cursor+appid";
/// What omabox's own patches fix, which no system aquamarine has (finding 247).
pub const AQ_OWN: &str = "keys held when an interactive box's window loses focus, the pointer and cursor in its monitors' windows, and those windows kept off your workspace";
/// What a refusal of aquamarine's fix says to do.
pub const AQ_HOWTO: &str = "omabox setup --aquamarine builds it for you (a minute or two)";
/// What #415 added that the library exports: a system aquamarine with it has the fix whatever its
/// version says (a package patched with #415 keeps the version it patched; finding 246).
pub const AQ_FIX_SYMBOL: &str = "_ZN10Aquamarine14CWaylandOutput14applyConfigureEv";
/// The release after this one comes from a main that has #415.
pub const AQ_FIXED_AFTER: &str = "0.15.1";
/// Where the system's aquamarine is (under a ROOT in aq_resolve_in: "/", or a unit test's tree).
pub const AQ_SYSDIR: &str = "/usr/lib";
/// The box's Qt (the system's: a box has /usr).
pub const QT_CORE: &str = "/usr/lib/libQt6Core.so.6";
pub const TITLEBARS_SO: &str = "/usr/lib/omarchy-hyprland-titlebars/titlebars.so";
/// The box's audio devices (#226).
pub const AUDIO_SINK: &str = "omabox-speaker";
pub const AUDIO_MIC: &str = "omabox-mic";

/// The jq programs here, as bash's omabox had them (byte for byte until the port ended, #324).
pub const JQ_INLINE: &[&str] = &[
    ".hyprland_version = $v",
    ".version // empty",
    ".quickshell_version = $v",
    "any(.roots[]; .path == $p)",
];

/// A jailed caller's paths as jailpath.rs follows them (#198, #210): only inside the jail's folders.
/// What git_link_dirs, repo_git_dirs and omarchy_tree need of it.
pub trait JailView {
    /// jail_path P: P followed only inside the jail's folders.
    fn path(&self, p: &[u8]) -> Option<Vec<u8>>;
    /// jail_readable P.
    fn readable(&self, p: &[u8]) -> bool;
    /// jail_read P: the file's bytes, read only from inside the jail's folders.
    fn read(&self, p: &[u8]) -> Option<Vec<u8>>;
    /// jail_sees P: P resolved, when the jail sees it.
    fn sees(&self, p: &[u8]) -> Option<Vec<u8>>;
    /// jail_root_covered P: P, one of the jail's folders with what it hides covered.
    fn root_covered(&self, p: &[u8]) -> Option<Vec<u8>>;
}

/// Which aquamarine a box runs (aq_resolve's AQ_KIND, AQ_LIB, AQ_VER, AQ_FIXED, AQ_MAIN).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Aq {
    /// `private` or `system`.
    pub kind: Vec<u8>,
    /// A private build's lib dir; empty for the system's (the box's loader finds it).
    pub lib: Vec<u8>,
    pub ver: Vec<u8>,
    /// It has the fix for nested Wayland outputs (hyprwm/aquamarine#415, finding 125).
    pub fixed: bool,
    /// The main checkout's build/prefix, when the box's is that (a worktree's: aq_main_lib).
    pub main: Vec<u8>,
}

impl Aq {
    /// aq_desc.
    pub fn desc(&self) -> Vec<u8> {
        if !self.main.is_empty() {
            cat!(
                "a private build, ",
                self.ver,
                " (",
                self.main,
                ", the main checkout's: this worktree has no usable one)"
            )
        } else if self.kind == b"private" {
            let l = self.lib.strip_suffix(b"/lib").unwrap_or(&self.lib);
            cat!("a private build, ", self.ver, " (", l, ")")
        } else if self.fixed && aq_ver_fixed(&self.ver) {
            cat!("the system's, ", self.ver)
        } else if self.fixed {
            cat!(
                "the system's, ",
                self.ver,
                ", patched with the fix for nested Wayland outputs (hyprwm/aquamarine#415)"
            )
        } else {
            cat!(
                "the system's, ",
                self.ver,
                ", without the fix for nested Wayland outputs (hyprwm/aquamarine#415): headless NVIDIA boxes and confirm-close need omabox setup --aquamarine"
            )
        }
    }

    /// aq_lacks: what a refusal says when it has not the fix (only the system's can lack it).
    pub fn lacks(&self) -> Vec<u8> {
        cat!(
            "aquamarine's fix for nested Wayland outputs (hyprwm/aquamarine#415), which the system's aquamarine ",
            self.ver,
            " lacks"
        )
    }
}

/// `access(P, MODE) == 0`.
fn access(p: &[u8], mode: libc::c_int) -> bool {
    std::ffi::CString::new(p.to_vec()).is_ok_and(|c| {
        // SAFETY: c is a valid C string for the call.
        unsafe { libc::access(c.as_ptr(), mode) == 0 }
    })
}

/// `command -v NAME` for a program: the first executable file of that name in PATH.
fn command_v(name: &[u8]) -> Option<Vec<u8>> {
    shell::env_or_empty("PATH")
        .split(|&c| c == b':')
        .map(|d| cat!(if d.is_empty() { b".".as_slice() } else { d }, "/", name))
        .find(|p| access(p, libc::X_OK) && shell::path(p).is_file())
}

/// `TOOL ARGS 2>/dev/null` as `$(...)`: its stdout whole (None when it could not start), with ENV.
fn output(tool: &str, args: &[&[u8]], env: &[(&str, &[u8])]) -> Option<Vec<u8>> {
    let mut c = crate::sig::command(tool);
    c.args(args.iter().map(|a| OsStr::from_bytes(a)))
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    for (k, v) in env {
        c.env(k, OsStr::from_bytes(v));
    }
    crate::sig::run_output(c).ok().map(|(_, o)| o)
}

/// `$(cat FILE)`.
fn read_subst(p: &[u8]) -> Option<Vec<u8>> {
    std::fs::read(shell::path(p)).ok().map(|v| shell::subst(&v))
}

/// `${F##*.so.}`: what follows the last `.so.`, or F itself.
fn after_so(f: &[u8]) -> Vec<u8> {
    match f.windows(4).rposition(|w| w == b".so.") {
        Some(i) => f[i + 4..].to_vec(),
        None => f.to_vec(),
    }
}

// --- ELF and versions ----------------------------------------------------------------------------

/// `sed -n "s/.*(TAG).*\[\(.*\)\]\$/\1/p"` over one of readelf's lines: what its last `[...]` holds
/// when the line names TAG before it and ends there.
fn dyn_value(l: &[u8], tag: &[u8]) -> Option<Vec<u8>> {
    let l = l.strip_suffix(b"]")?;
    let open = l.iter().rposition(|&c| c == b'[')?;
    let t = cat!("(", tag, ")");
    l[..open]
        .windows(t.len())
        .any(|w| w == t.as_slice())
        .then(|| l[open + 1..].to_vec())
}

/// elf_needed FILE [TAG]: the values of FILE's ELF dynamic entries TAG (NEEDED by default), from
/// readelf (read, never run); None when there are none.
pub fn elf_needed(file: &[u8], tag: &str) -> Option<Vec<Vec<u8>>> {
    let o = output("readelf", &[b"-d", b"--", file], &[])?;
    let v: Vec<Vec<u8>> = o
        .split(|&c| c == b'\n')
        .filter_map(|l| dyn_value(l, tag.as_bytes()))
        .filter(|x| !x.is_empty())
        .collect();
    (!v.is_empty()).then_some(v)
}

/// `[ "$(head -c 4 -- "$p" | od -An -tx1 | tr -d ' \n')" = 7f454c46 ]`.
fn is_elf(p: &[u8]) -> bool {
    use std::io::Read;
    let mut b = [0u8; 4];
    std::fs::File::open(shell::path(p))
        .and_then(|mut f| f.read_exact(&mut b))
        .is_ok()
        && b == *b"\x7fELF"
}

/// hypr_bin and qs_bin: PATH resolved, when it is an executable ELF file.
fn elf_bin(ctx: &Ctx, opt: &str, given: &[u8], what: &str) -> Res<Vec<u8>> {
    let p = match crate::repo::readlink_f(given) {
        Some(p) if shell::path(&p).is_file() => p,
        _ => return Err(ctx.die(cat!(opt, ": no such file: ", given))),
    };
    if !access(&p, libc::X_OK) {
        return Err(ctx.die(cat!(opt, ": ", p, " is not executable")));
    }
    if !is_elf(&p) {
        return Err(ctx.die(cat!(opt, ": ", p, " is not an ELF binary (", what, ")")));
    }
    Ok(p)
}

/// hypr_bin PATH: `up --hyprland PATH` (finding 116), resolved, when it is an executable ELF file.
/// What it links is check_install's.
pub fn hypr_bin(ctx: &Ctx, given: &[u8]) -> Res<Vec<u8>> {
    elf_bin(
        ctx,
        "--hyprland",
        given,
        "a Hyprland build's own binary, e.g. build/Hyprland; not a wrapper script",
    )
}

/// qs_bin PATH: `up --quickshell PATH` (finding 358), as hypr_bin.
pub fn qs_bin(ctx: &Ctx, given: &[u8]) -> Res<Vec<u8>> {
    elf_bin(
        ctx,
        "--quickshell",
        given,
        "a Quickshell build's own binary, e.g. build/src/quickshell; not a wrapper script",
    )
}

/// gnulib's verrevcmp (GNU sort -V's): non-digit runs by `order`, digit runs as numbers.
fn verrevcmp(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    let order = |c: u8| -> i32 {
        if c.is_ascii_digit() {
            0
        } else if c.is_ascii_alphabetic() {
            i32::from(c)
        } else if c == b'~' {
            -1
        } else {
            i32::from(c) + 256
        }
    };
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        while (i < a.len() && !a[i].is_ascii_digit()) || (j < b.len() && !b[j].is_ascii_digit()) {
            let ca = if i == a.len() { 0 } else { order(a[i]) };
            let cb = if j == b.len() { 0 } else { order(b[j]) };
            if ca != cb {
                return ca.cmp(&cb);
            }
            i += 1;
            j += 1;
        }
        while i < a.len() && a[i] == b'0' {
            i += 1;
        }
        while j < b.len() && b[j] == b'0' {
            j += 1;
        }
        let mut first = std::cmp::Ordering::Equal;
        while i < a.len() && j < b.len() && a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            if first.is_eq() {
                first = a[i].cmp(&b[j]);
            }
            i += 1;
            j += 1;
        }
        if i < a.len() && a[i].is_ascii_digit() {
            return std::cmp::Ordering::Greater;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            return std::cmp::Ordering::Less;
        }
        if !first.is_eq() {
            return first;
        }
    }
    std::cmp::Ordering::Equal
}

/// gnulib's file_prefixlen: the length before a file suffix, `(\.[A-Za-z~][A-Za-z0-9~]*)*$`.
fn prefix_len(s: &[u8]) -> usize {
    let n = s.len();
    let mut pre = 0;
    let mut i = 0;
    loop {
        if i == n {
            return pre;
        }
        i += 1;
        pre = i;
        while i + 1 < n && s[i] == b'.' && (s[i + 1].is_ascii_alphabetic() || s[i + 1] == b'~') {
            i += 2;
            while i < n && (s[i].is_ascii_alphanumeric() || s[i] == b'~') {
                i += 1;
            }
        }
    }
}

/// `sort -V`'s order of two lines: gnulib's filevercmp, then (equal there) the bytes, sort's last
/// resort. Names with a leading `.` get filevercmp's own place ("." first, then "..", then the rest).
pub fn version_cmp(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let fv = || -> Ordering {
        if a == b {
            return Ordering::Equal;
        }
        if a.is_empty() || b.is_empty() {
            return a.len().cmp(&b.len());
        }
        let dots = |s: &[u8]| match s {
            b"." => 0,
            b".." => 1,
            _ if s[0] == b'.' => 2,
            _ => 3,
        };
        let (da, db) = (dots(a), dots(b));
        if da != db || da < 2 {
            return da.cmp(&db);
        }
        let (a, b) = if da == 2 { (&a[1..], &b[1..]) } else { (a, b) };
        let (pa, pb) = (prefix_len(a), prefix_len(b));
        let r = verrevcmp(&a[..pa], &b[..pb]);
        if r.is_eq() && !(pa == a.len() && pb == b.len()) {
            verrevcmp(a, b)
        } else {
            r
        }
    };
    fv().then_with(|| a.cmp(b))
}

/// aq_ver_fixed VERSION: a release after AQ_FIXED_AFTER (`sort -V | tail -1` is not it).
pub fn aq_ver_fixed(v: &[u8]) -> bool {
    v != b"unknown" && version_cmp(v, AQ_FIXED_AFTER.as_bytes()).is_gt()
}

/// aq_has_fix FILE: the library exports #415's CWaylandOutput::applyConfigure (AQ_FIX_SYMBOL): its
/// name in the file, read, never loaded (finding 246).
pub fn aq_has_fix(f: &[u8]) -> bool {
    std::fs::read(shell::path(f)).is_ok_and(|b| {
        let s = AQ_FIX_SYMBOL.as_bytes();
        b.windows(s.len()).any(|w| w == s)
    })
}

// --- aquamarine ----------------------------------------------------------------------------------

/// hypr_aq_soname: the libaquamarine soname the installed Hyprland links (ldd).
pub fn hypr_aq_soname(ctx: &Ctx) -> Res<Vec<u8>> {
    let h = command_v(b"Hyprland").unwrap_or(b"/usr/bin/Hyprland".to_vec());
    let o = output("ldd", &[&h], &[]).unwrap_or_default();
    let want = o
        .split(|&c| c == b'\n')
        .find(|l| exit_contains(l, b"libaquamarine"))
        .and_then(|l| l.split(|c| c.is_ascii_whitespace()).find(|w| !w.is_empty()))
        .map(<[u8]>::to_vec)
        .unwrap_or_default();
    if want.is_empty() {
        return Err(ctx.die(cat!(
            "cannot tell which libaquamarine Hyprland links (ldd ",
            h,
            ")"
        )));
    }
    Ok(want)
}

fn exit_contains(hay: &[u8], needle: &[u8]) -> bool {
    crate::exit::contains(hay, needle)
}

/// aq_pick SONAME SYSDIR PRIVATE_LIBDIR...: which aquamarine a box runs. The first private build that
/// has SONAME, else the system's in SYSDIR; one with another soname is stale (built before an
/// aquamarine upgrade): skipped, said so. FIXED for every private build, and a system one that exports
/// #415's symbol or is after AQ_FIXED_AFTER. OMABOX_AQUAMARINE=system skips the private builds (the
/// suite on stock, finding 125). None after its words, when it cannot.
/// ENV is OMABOX_AQUAMARINE's value (`OMABOX_AQUAMARINE="" aq_resolve`: setup --aquamarine's).
fn aq_pick_env(ctx: &Ctx, want: &[u8], sys: &[u8], dirs: &[Vec<u8>], env: &[u8]) -> Option<Aq> {
    if want.is_empty() {
        return None;
    }
    let dirs: &[Vec<u8>] = if env == b"system" { &[] } else { dirs };
    if !env.is_empty() && env != b"system" {
        let _ = ctx.die(cat!("OMABOX_AQUAMARINE is system or unset, got ", env));
        return None;
    }
    for d in dirs {
        let p = cat!(d, "/", want);
        let there = shell::path(&p).exists();
        // One the box could not load is no build to use (an unreadable copy, another user's checkout).
        if there && !access(&p, libc::R_OK) {
            say(cat!(
                "skipping the aquamarine in ",
                d,
                ": ",
                want,
                " is not readable"
            ));
            continue;
        }
        if there {
            let f = crate::repo::readlink_f(&p).unwrap_or_default();
            let mut v = after_so(&f);
            let commit = read_subst(&cat!(d, "/../.omabox-commit")).unwrap_or_default();
            if !commit.is_empty() {
                v = cat!(v, "@", commit);
            }
            return Some(Aq {
                kind: b"private".to_vec(),
                lib: d.clone(),
                ver: v,
                fixed: true,
                main: Vec::new(),
            });
        }
        // (`compgen -G "$d/libaquamarine.so.*" | head -1`, in byte order: finding 446)
        let Some(other) = shell::glob(&shell::path(d), false)
            .into_iter()
            .find(|n| n.starts_with(b"libaquamarine.so."))
        else {
            continue;
        };
        let f = cat!(d, "/", other);
        let has = elf_needed(&f, "SONAME")
            .map(|v| v.join(&b'\n'))
            .unwrap_or(b"another".to_vec());
        say(cat!(
            "skipping the aquamarine in ",
            d,
            ": Hyprland links ",
            want,
            ", it has ",
            has,
            " (built before an aquamarine upgrade; omabox setup --aquamarine makes a new one)"
        ));
    }
    let p = cat!(sys, "/", want);
    if !shell::path(&p).exists() {
        let _ = ctx.die(cat!("Hyprland links ", want, ", which is not in ", sys));
        return None;
    }
    let f = crate::repo::readlink_f(&p).unwrap_or_default();
    let mut v = after_so(&f);
    if !shell::re_match("^[0-9]+(\\.[0-9]+)*$", &v) {
        v = b"unknown".to_vec();
    }
    let fixed = aq_has_fix(&f) || aq_ver_fixed(&v);
    Some(Aq {
        kind: b"system".to_vec(),
        lib: Vec::new(),
        ver: v,
        fixed,
        main: Vec::new(),
    })
}

/// aq_main_lib CHECKOUT: a linked worktree's main checkout's build/prefix/lib (issue #182, finding
/// 271), found where git would find it, never by running git (git_link_dirs): the common dir's parent
/// when that is a dir named .git. None for a main checkout, an installed omabox, a submodule, any
/// other layout, or a lib dir missing or not readable.
pub fn aq_main_lib(co: &[u8]) -> Option<Vec<u8>> {
    let (g, c) = git_link_dirs(co, None)?;
    if g == c || !c.ends_with(b"/.git") {
        return None;
    }
    let top = &c[..c.len() - 5];
    let same = |a: &[u8], b: &[u8]| {
        use std::os::unix::fs::MetadataExt;
        match (
            std::fs::metadata(shell::path(a)),
            std::fs::metadata(shell::path(b)),
        ) {
            (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
            _ => false,
        }
    };
    if same(top, co) {
        return None;
    }
    let lib = cat!(top, "/build/prefix/lib");
    (shell::path(&lib).is_dir() && access(&lib, libc::R_OK) && access(&lib, libc::X_OK))
        .then_some(lib)
}

/// aq_resolve SONAME: aq_pick's answer for a box. The checkout's build/prefix, a worktree's main
/// checkout's (aq_main_lib), the user's data dir, and in a box (an omabox started there: the suite's
/// stand-in hosts) the box's own, last (finding 130).
pub fn aq_resolve(ctx: &Ctx, soname: &[u8]) -> Option<Aq> {
    aq_resolve_env(ctx, soname, &shell::env_or_empty("OMABOX_AQUAMARINE"))
}

/// aq_pick as OMABOX_AQUAMARINE=system gives it, with SYS the system's lib dir: the suite's probe of a
/// stand-in system aquamarine (`_rust probe aq_pick`).
pub fn aq_pick_system(ctx: &Ctx, soname: &[u8], sys: &[u8]) -> Option<Aq> {
    aq_pick_env(ctx, soname, sys, &[], b"system")
}

/// aq_pick SONAME SYS DIRS..., with OMABOX_AQUAMARINE as the environment has it (`_rust probe
/// aq_pick_dirs`).
pub fn aq_pick_dirs(ctx: &Ctx, soname: &[u8], sys: &[u8], dirs: &[Vec<u8>]) -> Option<Aq> {
    aq_pick_env(
        ctx,
        soname,
        sys,
        dirs,
        &shell::env_or_empty("OMABOX_AQUAMARINE"),
    )
}

/// config --json's confirm_close_available: false only when the aquamarine boxes would use (from
/// checkout CO, the system's under ROOT) lacks the fix; quiet.
pub fn confirm_close_available_in(ctx: &Ctx, co: &[u8], root: &[u8]) -> bool {
    let env = shell::env_or_empty("OMABOX_AQUAMARINE");
    !muted(|| {
        hypr_aq_soname(ctx)
            .ok()
            .and_then(|w| aq_resolve_in(ctx, &w, &env, co, root))
            .is_some_and(|a| !a.fixed)
    })
}

/// What `up` refuses without aquamarine's fix on a headless box (finding 125): one on an NVIDIA
/// render node, naming the other GPUs it could render on. RENDER's driver and the render nodes read
/// under ROOT ("/", or a unit test's tree).
pub fn nvidia_needs_fix_in(ctx: &Ctx, aq: &Aq, render: &RenderNode, root: &[u8]) -> Res<()> {
    if aq.fixed || render.driver_in(root) != b"nvidia" {
        return Ok(());
    }
    let mut others = Vec::new();
    for n in crate::render::render_nodes_in(root) {
        let d = n.driver_in(root);
        if d != b"nvidia" {
            others.extend_from_slice(&cat!(" ", n.as_bytes(), " (", d, ")"));
        }
    }
    let tail = if others.is_empty() {
        Vec::new()
    } else {
        cat!(
            "; or render on another GPU (omabox config gpu amd, intel or a slot; or OMABOX_RENDER_NODE):",
            others
        )
    };
    Err(ctx.die(cat!(
        "a headless box on NVIDIA (",
        render.as_bytes(),
        ") needs ",
        aq.lacks(),
        ": ",
        AQ_HOWTO,
        tail
    )))
}

/// Whether `setup --aquamarine` builds over AQ, what boxes use now, and what it says first: never
/// over this omabox's build (AQ_BUILD) unless FORCE; a system aquamarine with the fix still lacks
/// omabox's own patches (#186, finding 247), said.
pub fn aq_build_needed(aq: &Aq, force: bool) -> bool {
    let ver_build = aq
        .ver
        .iter()
        .position(|&c| c == b'@')
        .map_or(aq.ver.as_slice(), |i| &aq.ver[i + 1..]);
    if !force && aq.kind == b"private" && ver_build == AQ_BUILD.as_bytes() {
        say(cat!(
            "boxes use ",
            aq.desc(),
            ", this omabox's build: nothing to build"
        ));
        return false;
    }
    if aq.kind == b"system" && aq.fixed {
        say(cat!(
            "boxes use ",
            aq.desc(),
            ", which has the fix for nested Wayland outputs but not omabox's own (",
            AQ_OWN,
            ")"
        ));
    }
    true
}

/// aq_resolve with ENV as OMABOX_AQUAMARINE.
pub fn aq_resolve_env(ctx: &Ctx, soname: &[u8], env: &[u8]) -> Option<Aq> {
    aq_resolve_in(ctx, soname, env, &crate::paths::root(), b"/")
}

/// aq_resolve_env for a box from checkout CO (bash's ROOT), the system's dirs (AQ_SYSDIR, a box's
/// /opt/omabox) under ROOT: "/", or a unit test's stand-in tree.
pub fn aq_resolve_in(ctx: &Ctx, soname: &[u8], env: &[u8], co: &[u8], root: &[u8]) -> Option<Aq> {
    use crate::gpu::at;
    let main = aq_main_lib(co).unwrap_or_default();
    let mut dirs = vec![cat!(co, "/build/prefix/lib")];
    if !main.is_empty() {
        dirs.push(main.clone());
    }
    dirs.push(cat!(crate::paths::aq_user(), "/lib"));
    if shell::path(&at(root, b"/opt/omabox/share/session.sh")).is_file() {
        dirs.push(at(root, b"/opt/omabox/lib"));
    }
    let mut aq = aq_pick_env(ctx, soname, &at(root, AQ_SYSDIR.as_bytes()), &dirs, env)?;
    if aq.kind == b"private" && !main.is_empty() && aq.lib == main {
        aq.main = main.strip_suffix(b"/lib").unwrap_or(&main).to_vec();
    }
    Some(aq)
}

/// `LD_LIBRARY_PATH=LIB ldd -- FILE | awk '/=> not found/ {print $1}' | paste -sd ' '`.
fn ldd_missing(file: &[u8], lib: Option<&[u8]>) -> Vec<u8> {
    let env: Vec<(&str, &[u8])> = lib.map(|l| ("LD_LIBRARY_PATH", l)).into_iter().collect();
    let o = output("ldd", &[b"--", file], &env).unwrap_or_default();
    let v: Vec<&[u8]> = o
        .split(|&c| c == b'\n')
        .filter(|l| exit_contains(l, b"=> not found"))
        .filter_map(|l| l.split(|c| c.is_ascii_whitespace()).find(|w| !w.is_empty()))
        .collect();
    v.join(&b' ')
}

/// check_install RENDER [HYPRLAND]: what `up` needs from install.sh: the helpers, and an aquamarine
/// with the soname the box's Hyprland links (aq_resolve): the installed Hyprland's, or `up
/// --hyprland`'s build (finding 116), which is refused here rather than left to crash in the box.
pub fn check_install(ctx: &Ctx, render: &RenderNode, hyprland: Option<&[u8]>) -> Res<Aq> {
    let root = crate::paths::root();
    // A linked worktree (.git a file) builds its own with make, as test/run.sh does: install.sh there
    // would point your omabox, skill and widget at the worktree (issue #182).
    let how = if shell::path(&cat!(root, "/.git")).is_file() {
        cat!(
            "in a git worktree, build them: for m in ",
            root,
            "/tools/*/Makefile; do make -C \"${m%/*}\"; done (test/run.sh builds them itself)"
        )
    } else {
        b"run install.sh".to_vec()
    };
    let tools = ["keyboard", "pointer", "still", "events"].map(crate::paths::tool);
    if !tools.iter().all(|t| crate::exec::executable(t)) {
        return Err(ctx.die(cat!(
            ctx.label(),
            "missing ",
            tools[0],
            ", ",
            tools[1],
            ", ",
            tools[2],
            " or ",
            tools[3],
            ": ",
            how
        )));
    }
    // Only an NVIDIA box sizes its screen with wlr-randr (finding 78).
    if !crate::exec::executable(b"/usr/bin/wlr-randr") && render.is_nvidia() {
        return Err(ctx.die(cat!(
            ctx.label(),
            "missing /usr/bin/wlr-randr, which NVIDIA boxes need: run install.sh"
        )));
    }
    let Some(h) = hyprland.filter(|h| !h.is_empty()) else {
        // (`aq_resolve "$(hypr_aq_soname)" || exit 1`: either one's words said, then 1)
        let so = hypr_aq_soname(ctx).map_err(|_| Exit(1))?;
        return aq_resolve(ctx, &so).ok_or(Exit(1));
    };
    // A build's own libraries: its ELF dynamic section, read, never run. A soname, nothing else:
    // aq_pick looks for it in library dirs by name, and one with a / in it (a jailed caller's build
    // can say anything) would have it test any path on the host (#198).
    let want = elf_needed(h, "NEEDED")
        .unwrap_or_default()
        .into_iter()
        .find(|l| shell::re_match("^libaquamarine\\.so(\\.[0-9A-Za-z_]+)*$", l));
    let Some(want) = want else {
        return Err(ctx.die(cat!(
            "--hyprland ",
            h,
            " links no libaquamarine (readelf -d): not a Hyprland build?"
        )));
    };
    let Some(aq) = muted(|| aq_resolve(ctx, &want)) else {
        let sys = muted(|| hypr_aq_soname(ctx)).unwrap_or(b"no libaquamarine".to_vec());
        return Err(ctx.die(cat!(
            "--hyprland ",
            h,
            " links ",
            want,
            ", which neither the system nor a private build of omabox's has (the system has ",
            sys,
            "): build Hyprland against the installed aquamarine, or against omabox's private build (PKG_CONFIG_PATH=<its prefix>/lib/pkgconfig, omabox setup --aquamarine)"
        )));
    };
    // Any other library it needs and the box lacks: ldd, as the box's loader sees them. Not for a
    // jailed agent's file: the broker does not load what the jail hands it.
    if in_jail() {
        return Ok(aq);
    }
    let missing = ldd_missing(h, Some(&aq.lib));
    if !missing.is_empty() {
        let with = if aq.lib.is_empty() {
            Vec::new()
        } else {
            cat!(" and ", aq.lib)
        };
        return Err(ctx.die(cat!(
            "--hyprland ",
            h,
            " needs ",
            missing,
            ", which the box lacks (it has the system's libraries",
            with,
            ")"
        )));
    }
    Ok(aq)
}

// --- Quickshell and Qt ---------------------------------------------------------------------------

/// `readelf -W --dyn-syms -- FILE`'s lines.
fn dyn_syms(file: &[u8]) -> Vec<u8> {
    output("readelf", &[b"-W", b"--dyn-syms", b"--", file], &[]).unwrap_or_default()
}

/// `6.` and digits at the start of S: how many bytes.
fn qt_minor_len(s: &[u8]) -> Option<usize> {
    let r = s.strip_prefix(b"6.")?;
    Some(2 + r.iter().take_while(|c| c.is_ascii_digit()).count())
}

/// qt_built FILE: the Qt minor a program was built against (the qt_version_tag it refers to):
/// `s/.* UND qt_version_tag@Qt_\(6\.[0-9]*\)\( .*\)\{0,1\}$/\1/p`, the first line.
pub fn qt_built(file: &[u8]) -> Vec<u8> {
    let key = b" UND qt_version_tag@Qt_";
    for l in dyn_syms(file).split(|&c| c == b'\n') {
        // (sed's greedy `.*`: the last place the rest matches from)
        let hits: Vec<usize> = l
            .windows(key.len())
            .enumerate()
            .filter(|(_, w)| *w == key)
            .map(|(i, _)| i + key.len())
            .collect();
        for &i in hits.iter().rev() {
            let r = &l[i..];
            if let Some(n) = qt_minor_len(r)
                && (n == r.len() || r[n] == b' ')
            {
                return r[..n].to_vec();
            }
        }
    }
    Vec::new()
}

/// qt_box: the Qt minor the box's Qt stands for (its default qt_version_tag):
/// `s/.* qt_version_tag@@Qt_\(6\.[0-9]*\)$/\1/p`, the first line.
pub fn qt_box() -> Vec<u8> {
    let key = b" qt_version_tag@@Qt_";
    for l in dyn_syms(QT_CORE.as_bytes()).split(|&c| c == b'\n') {
        let hits: Vec<usize> = l
            .windows(key.len())
            .enumerate()
            .filter(|(_, w)| *w == key)
            .map(|(i, _)| i + key.len())
            .collect();
        for &i in hits.iter().rev() {
            let r = &l[i..];
            if qt_minor_len(r) == Some(r.len()) {
                return r.to_vec();
            }
        }
    }
    Vec::new()
}

/// qt_version: the box's Qt version, from the library's name (6.11.2), else its minor (qt_box).
pub fn qt_version() -> Vec<u8> {
    if let Some(f) = crate::repo::readlink_f(QT_CORE.as_bytes())
        && let Some(v) = shell::re_group("\\.so\\.(6\\.[0-9]+\\.[0-9]+)$", &f, 1)
    {
        return v;
    }
    qt_box()
}

/// qs_check PATH: a Quickshell build checked before the box dir is made, never run here: its ELF read
/// (readelf), and `ldd` as the box's loader sees it. Quickshell uses Qt's private API, so a build
/// against another Qt minor than the box's dies as the shell starts: refused here, naming both.
pub fn qs_check(ctx: &Ctx, p: &[u8]) -> Res<()> {
    let links = elf_needed(p, "NEEDED")
        .unwrap_or_default()
        .iter()
        .any(|l| l == b"libQt6Core.so.6");
    if !links {
        return Err(ctx.die(cat!(
            "--quickshell ",
            p,
            " links no libQt6Core.so.6 (readelf -d): not a Quickshell build?"
        )));
    }
    let (want, have) = (qt_built(p), qt_box());
    if !want.is_empty() && !have.is_empty() && want != have {
        return Err(ctx.die(cat!(
            "--quickshell ",
            p,
            " was built against Qt ",
            want,
            ", but the box has Qt ",
            qt_version(),
            ": Quickshell uses Qt's private API, so it would die as the shell starts. Build it again against the installed Qt"
        )));
    }
    // Not for a jailed agent's file: the broker does not load what the jail hands it.
    if in_jail() {
        return Ok(());
    }
    let missing = ldd_missing(p, None);
    if !missing.is_empty() {
        return Err(ctx.die(cat!(
            "--quickshell ",
            p,
            " needs ",
            missing,
            ", which the box lacks (it has the system's libraries)"
        )));
    }
    Ok(())
}

// --- what the box says of its versions -----------------------------------------------------------

/// `on_box timeout 6 ARGS 2>/dev/null | head -c LIMIT | head -n 1`.
fn box_first_line<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx, args: &[&[u8]], limit: usize) -> Vec<u8> {
    let a: Vec<Vec<u8>> = args.iter().map(|x| x.to_vec()).collect();
    let r = sys.on_box_io(
        ctx,
        &a,
        crate::exec::Out::Capture(Some(limit)),
        crate::exec::ErrTo::Null,
    );
    let first = r.out.split(|&c| c == b'\n').next().unwrap_or_default();
    shell::subst(first)
}

/// hypr_version: the box's Hyprland version as it says it (the first line of `hyprctl version`), into
/// box.json for `ls` and `windows` (#238). A box on its own Hyprland (finding 116) also gets a warning
/// when the host's hyprctl (the installed Hyprland's) is of another version: their IPC may not match.
pub fn hypr_version(ctx: &Ctx) {
    hypr_version_with(&RealBox, ctx);
}

fn hypr_version_with<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx) {
    let own = crate::boxes::meta(&ctx.d, "hyprland").0;
    let line = box_first_line(sys, ctx, &[b"timeout", b"6", b"hyprctl", b"version"], 65536);
    if line.is_empty() {
        let o = if own.is_empty() {
            b"the installed one".to_vec()
        } else {
            own
        };
        say(cat!(
            "box '",
            ctx.name,
            "': its Hyprland (",
            o,
            ") did not say its version (hyprctl version)"
        ));
        return;
    }
    crate::mon::jq_box_json(
        ctx,
        crate::jq::Args::new(&["--arg", "v"])
            .arg(&line)
            .arg(JQ_INLINE[0]),
    );
    if own.is_empty() {
        return;
    }
    let a: Vec<Vec<u8>> = [&b"timeout"[..], b"6", b"hyprctl", b"version", b"-j"]
        .iter()
        .map(|x| x.to_vec())
        .collect();
    let r = sys.on_box_io(
        ctx,
        &a,
        crate::exec::Out::Capture(Some(65536)),
        crate::exec::ErrTo::Null,
    );
    let jver =
        |o: &[u8]| crate::jq::jq(&["-r", JQ_INLINE[1]], Some(o), crate::jq::Err::Null).text();
    let ver = jver(&r.out);
    let host = output("/usr/bin/Hyprland", &[b"--version-json"], &[])
        .map(|o| jver(&o))
        .unwrap_or_default();
    if !ver.is_empty() && !host.is_empty() && ver != host {
        say(cat!(
            "warning: box '",
            ctx.name,
            "' runs Hyprland ",
            ver,
            ", but hyprctl and hyprpm are the installed ",
            host,
            "'s: their IPC may not match (omabox hyprctl, the shell's calls)"
        ));
    }
}

/// qs_version: the box's Quickshell version as it says it (`quickshell --version`, its first line),
/// into box.json for `ls --json`.
pub fn qs_version(ctx: &Ctx) {
    qs_version_with(&RealBox, ctx);
}

fn qs_version_with<S: BoxSys + ?Sized>(sys: &S, ctx: &Ctx) {
    let qs = crate::shellnote::box_qs(ctx);
    let line = box_first_line(sys, ctx, &[b"timeout", b"6", &qs, b"--version"], 4096);
    if line.is_empty() {
        let own = crate::boxes::meta(&ctx.d, "quickshell").0;
        if !own.is_empty() {
            say(cat!(
                "box '",
                ctx.name,
                "': its Quickshell (",
                own,
                ") did not say its version (quickshell --version)"
            ));
        }
        return;
    }
    crate::mon::jq_box_json(
        ctx,
        crate::jq::Args::new(&["--arg", "v"])
            .arg(&line)
            .arg(JQ_INLINE[2]),
    );
}

// --- Omarchy -------------------------------------------------------------------------------------

/// omarchy_tree DIR: `up --omarchy DIR` (finding 135), resolved, when it is an Omarchy tree (what a
/// box runs from it is there); empty for /usr/share/omarchy itself, the default. A jailed caller's
/// tree (#198): each file followed only inside the jail's folders (JAIL; a link out is none).
pub fn omarchy_tree(ctx: &Ctx, given: &[u8], jail: Option<&dyn JailView>) -> Res<Vec<u8>> {
    let p = match crate::repo::readlink_f(given) {
        Some(p) if shell::path(&p).is_dir() => p,
        _ => return Err(ctx.die(cat!("--omarchy: no such directory: ", given))),
    };
    for f in ["bin", "default/hypr/bootstrap.lua", "shell/shell.qml"] {
        let mut q = cat!(p, "/", f);
        if let Some(j) = jail {
            q = j.path(&q).unwrap_or_default();
        }
        if q.is_empty() || !shell::path(&q).exists() {
            return Err(ctx.die(cat!(
                "--omarchy: ",
                p,
                " is not an Omarchy tree (no ",
                f,
                ")"
            )));
        }
    }
    Ok(if p == b"/usr/share/omarchy" {
        Vec::new()
    } else {
        p
    })
}

/// `grep -w WORD`'s test on one line: WORD with no word character (alnum or _) either side.
fn has_word(l: &[u8], w: &[u8]) -> bool {
    let wc = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut i = 0;
    while let Some(k) = l
        .get(i..)
        .and_then(|r| r.windows(w.len()).position(|x| x == w))
    {
        let s = i + k;
        let e = s + w.len();
        if (s == 0 || !wc(l[s - 1])) && (e == l.len() || !wc(l[e])) {
            return true;
        }
        i = s + 1;
    }
    false
}

/// `grep -qsw WORD FILE`.
fn file_has_word(f: &[u8], w: &[u8]) -> bool {
    std::fs::read(shell::path(f)).is_ok_and(|b| b.split(|&c| c == b'\n').any(|l| has_word(l, w)))
}

/// `grep -rqsw WORD DIR`: DIR's files and those below it, links below it not followed (grep -r).
fn tree_has_word(dir: &[u8], w: &[u8]) -> bool {
    let Ok(rd) = std::fs::read_dir(shell::path(dir)) else {
        return file_has_word(dir, w);
    };
    for e in rd.flatten() {
        let p = cat!(dir, "/", e.file_name().as_bytes());
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            if tree_has_word(&p, w) {
                return true;
            }
        } else if t.is_file() && file_has_word(&p, w) {
            return true;
        }
    }
    false
}

/// omarchy_host_gaps TREE: what the Omarchy tree TREE runs that this host has not installed (#293,
/// finding 420), a line each, "PACKAGE: what goes without it". Each only when the tree uses it.
pub fn omarchy_host_gaps(t: &[u8]) -> Vec<Vec<u8>> {
    omarchy_host_gaps_with(t, |c| command_v(c).is_some(), TITLEBARS_SO.as_bytes())
}

/// omarchy_host_gaps with HAVE for whether the host has a command and SO the titlebars plugin's
/// path (the suite's #[test] gives a host with or without them).
fn omarchy_host_gaps_with(t: &[u8], have: impl Fn(&[u8]) -> bool, so: &[u8]) -> Vec<Vec<u8>> {
    let mut v = Vec::new();
    if file_has_word(&cat!(t, "/bin/omarchy-theme-bg-boot-intro"), b"owe") && !have(b"owe") {
        v.push(b"owe: no video wallpapers and no theme intros".to_vec());
    }
    if tree_has_word(&cat!(t, "/bin"), b"omasnap") && !have(b"omasnap") {
        v.push(b"omasnap: no Omasnap screenshots".to_vec());
    }
    let has_so = std::fs::read(shell::path(&cat!(t, "/default/hypr/workspace-modes.lua")))
        .is_ok_and(|b| crate::exit::contains(&b, so));
    if has_so && !shell::path(so).exists() {
        v.push(b"omarchy-hyprland-titlebars: no titlebars on floating workspaces".to_vec());
    }
    v
}

/// omarchy_version: the installed Omarchy's version, as Omarchy's omarchy-version reads it
/// (omarchy-dev on the edge channel, else omarchy), for the box's stand-in: a box has no pacman
/// database. Inside a box (the suite's stand-in hosts), the box's own (OMABOX_OMARCHY_VERSION).
pub fn omarchy_version() -> Vec<u8> {
    for p in ["omarchy-dev", "omarchy"] {
        // (`pacman -Q P 2>/dev/null | awk '{ print $2 }'`)
        let o = output("pacman", &[b"-Q", p.as_bytes()], &[]).unwrap_or_default();
        let v: Vec<&[u8]> = o
            .split(|&c| c == b'\n')
            .filter(|l| !l.is_empty())
            .map(|l| {
                l.split(|c| c.is_ascii_whitespace())
                    .filter(|w| !w.is_empty())
                    .nth(1)
                    .unwrap_or_default()
            })
            .collect();
        let v = shell::subst(&v.join(&b'\n'));
        if !v.is_empty() {
            return v;
        }
    }
    shell::env_or_empty("OMABOX_OMARCHY_VERSION")
}

/// git_rev DIR: a checkout's short commit, "+dirty" with uncommitted changes; nothing outside git.
/// Never git on a jailed caller's path (#198 security): its .git/config is the jail's to write, and
/// core.fsmonitor and other settings there run programs here, outside the jail.
pub fn git_rev(dir: &[u8]) -> Vec<u8> {
    git_rev_jailed(in_jail(), dir)
}

/// git_rev, JAILED telling whether the caller is a jailed agent's (in_jail).
fn git_rev_jailed(jailed: bool, dir: &[u8]) -> Vec<u8> {
    if jailed {
        return Vec::new();
    }
    let git = |args: &[&[u8]]| -> Option<(i32, Vec<u8>)> {
        let mut c = crate::sig::command("git");
        c.arg("-C")
            .arg(OsStr::from_bytes(dir))
            .args(args.iter().map(|a| OsStr::from_bytes(a)))
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        crate::sig::run_output(c).ok()
    };
    let c = match git(&[b"rev-parse", b"--short", b"HEAD"]) {
        Some((0, o)) => shell::subst(&o),
        _ => return Vec::new(),
    };
    let dirty = git(&[b"status", b"--porcelain"]).is_some_and(|(_, o)| !o.is_empty());
    if dirty { cat!(c, "+dirty") } else { c }
}

// --- git dirs ------------------------------------------------------------------------------------

/// git_link_path P [jail]: P resolved (git_link_dirs'); with JAIL, only inside the jail's folders.
fn git_link_path(p: &[u8], jail: Option<&dyn JailView>) -> Option<Vec<u8>> {
    match jail {
        Some(j) => j.sees(&j.path(p)?),
        None => crate::repo::readlink_e(p),
    }
}

/// git_link_dirs CHECKOUT [jail]: a linked worktree's (or submodule's) gitdir and common dir, where
/// git finds them, from the files git reads and never by running git (findings 271, 275). None for a
/// main checkout (.git a dir), no .git, a gitdir or common dir gone, or a path with a newline. With
/// JAIL (a jailed caller's checkout: #193), each path followed only inside the jail's folders and
/// each file read through it, so nothing outside them is looked at.
pub fn git_link_dirs(co: &[u8], jail: Option<&dyn JailView>) -> Option<(Vec<u8>, Vec<u8>)> {
    let t = match jail {
        Some(j) => {
            let t = j.path(&cat!(co, "/.git"))?;
            if !j.readable(&t) || !shell::path(&t).is_file() {
                return None;
            }
            shell::subst(&j.read(&t)?)
        }
        None => {
            let p = cat!(co, "/.git");
            if !shell::path(&p).is_file() {
                return None;
            }
            read_subst(&p)?
        }
    };
    // (`sed -n '1s/^gitdir: //p'`)
    let first = t.split(|&c| c == b'\n').next().unwrap_or_default();
    let mut g = first.strip_prefix(b"gitdir: ")?.to_vec();
    if g.is_empty() {
        return None;
    }
    if !g.starts_with(b"/") {
        g = cat!(co, "/", g);
    }
    let g = git_link_path(&g, jail).filter(|g| shell::path(g).is_dir())?;
    let t = match jail {
        Some(j) => {
            let t = j.path(&cat!(g, "/commondir"))?;
            if !j.readable(&t) {
                return None;
            }
            t
        }
        None => cat!(g, "/commondir"),
    };
    let tp = shell::path(&t);
    let c = if tp.is_file() {
        let mut c = match jail {
            Some(j) => shell::subst(&j.read(&t)?),
            None => read_subst(&t)?,
        };
        if c.is_empty() {
            return None;
        }
        if !c.starts_with(b"/") {
            c = cat!(g, "/", c);
        }
        git_link_path(&c, jail).filter(|c| shell::path(c).is_dir())?
    } else if tp.exists() || tp.is_symlink() {
        return None;
    } else {
        g.clone()
    };
    if g.contains(&b'\n') || c.contains(&b'\n') {
        return None;
    }
    Some((g, c))
}

/// repo_git_dirs TOP: the dirs outside the repo's top level that its git data lives in, for `up` to
/// mount read-only at their own paths (finding 250): a linked worktree's common dir (the main
/// checkout's .git, holding its gitdir), a submodule's. None for a main checkout. A jailed caller's
/// (#193, finding 275): git never run, the files read through JAIL, and only a linked worktree's
/// common dir that is a dir named .git and one of the jail's folders itself, with what the jail hides
/// in it covered (jail_root_covered); anything else is nothing, quietly.
pub fn repo_git_dirs(given: &[u8], jail: Option<&dyn JailView>) -> Vec<Vec<u8>> {
    let top = crate::repo::readlink_e(given).unwrap_or_else(|| given.to_vec());
    let top = top.strip_suffix(b"/").unwrap_or(&top).to_vec();
    let in_top = |d: &[u8]| cat!(d, "/").starts_with(&cat!(top, "/"));
    if let Some(j) = jail {
        let Some((g, d)) = git_link_dirs(given, Some(j)) else {
            return Vec::new();
        };
        let wt = cat!(d, "/worktrees/");
        let ok = d.ends_with(b"/.git")
            && g.starts_with(&wt)
            && !g[wt.len()..].contains(&b'/')
            && g.len() > wt.len();
        if !ok {
            return Vec::new();
        }
        let pol = shell::env_or_empty("OMABOX_JAIL");
        let r = crate::jq::Args::new(&["-e", "--arg", "p"])
            .arg(&d)
            .arg(JQ_INLINE[3])
            .run(Some(&pol), crate::jq::Err::Null);
        if !r.ok() || in_top(&d) || j.root_covered(&d).is_none() {
            return Vec::new();
        }
        return vec![d];
    }
    let mut c = crate::sig::command("git");
    c.arg("-C")
        .arg(OsStr::from_bytes(given))
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
            "--git-dir",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    let out = match crate::sig::run_output(c) {
        Ok((0, o)) => shell::subst(&o),
        _ => return Vec::new(),
    };
    let mut v: Vec<Vec<u8>> = Vec::new();
    let mut prev: Vec<u8> = Vec::new();
    for l in out.split(|&c| c == b'\n') {
        let Some(d) = crate::repo::readlink_e(l) else {
            continue;
        };
        if !shell::path(&d).is_dir() || in_top(&d) {
            continue;
        }
        // (in the common dir: mounted with it)
        if !prev.is_empty() && cat!(d, "/").starts_with(&cat!(prev, "/")) {
            continue;
        }
        prev = d.clone();
        v.push(d);
    }
    v
}

// --- audio ---------------------------------------------------------------------------------------

/// audio_packages: what `up --audio` needs (#226), or the programs missing.
pub fn audio_packages() -> Result<(), Vec<u8>> {
    let miss: Vec<&[u8]> = [
        &b"pipewire"[..],
        b"wireplumber",
        b"pipewire-pulse",
        b"pactl",
    ]
    .into_iter()
    .filter(|b| !crate::exec::executable_on_path(b))
    .collect();
    if miss.is_empty() {
        return Ok(());
    }
    // "pipewire, pactl", as its comment meant: bash's `local IFS=,; echo "${miss[*]//,/, }"` ran the
    // substitution on each word, which has none, and wrote a bare comma (finding 462)
    Err(miss.join(b", ".as_slice()))
}

/// polkit_ready: what `up --polkit` needs (#258, finding 627), or what is missing: the stand-in
/// authority and its session shim (tools/polkit), and dbus-daemon for the box's own system bus.
pub fn polkit_ready() -> Result<(), Vec<u8>> {
    let (tool, shim) = (crate::paths::tool("polkit"), crate::paths::polkit_session());
    if !crate::exec::executable(&tool) || !shell::path(&shim).is_file() {
        let how: &[u8] = if shell::path(&cat!(crate::paths::root(), "/.git")).is_file() {
            b"make -C"
        } else {
            b"install.sh builds it; by hand: make -C"
        };
        return Err(cat!(
            tool,
            " and ",
            shim,
            " (",
            how,
            " ",
            crate::paths::root(),
            "/tools/polkit)"
        ));
    }
    if !crate::exec::executable(b"/usr/bin/dbus-daemon") {
        return Err(
            b"/usr/bin/dbus-daemon, for the box's own system bus (sudo pacman -S dbus)".to_vec(),
        );
    }
    Ok(())
}

/// audio_ready: the box's audio is up (#226): its pipewire-pulse answers, its default output is
/// omabox-speaker and its default input omabox-mic (10 s at most).
pub fn audio_ready(ctx: &mut Ctx) -> bool {
    let want = cat!(AUDIO_SINK, " ", AUDIO_MIC);
    let end = crate::settle::after(10);
    // (pactl's own words are dropped inside the box: bash's `2>/dev/null` on box_exec)
    let argv: Vec<Vec<u8>> = [
        &b"sh"[..],
        b"-c",
        b"echo \"$(pactl get-default-sink 2>/dev/null) $(pactl get-default-source 2>/dev/null)\"",
    ]
    .iter()
    .map(|a| a.to_vec())
    .collect();
    loop {
        let got = muted(|| box_output(ctx, &argv)).unwrap_or_default();
        if got == want {
            return true;
        }
        if crate::settle::past(end) {
            return false;
        }
        crate::trap::sleep(b"0.1");
    }
}

/// `$(box_exec ARGS)`: its stdout, when it ran and exited 0.
fn box_output(ctx: &mut Ctx, argv: &[Vec<u8>]) -> Option<Vec<u8>> {
    use std::os::fd::AsRawFd;
    let (r, w) = crate::exec::pipe().ok()?;
    let reader = std::thread::spawn(move || crate::exec::read_all(r, Some(65536)));
    let rc = crate::exec::box_exec(
        ctx,
        argv,
        crate::exec::ExecOpts {
            stdin_null: true,
            stdout: Some(w.as_raw_fd()),
            ..crate::exec::ExecOpts::default()
        },
    );
    drop(w);
    let out = reader.join().unwrap_or_default();
    matches!(rc, Ok(0)).then(|| shell::subst(&out))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// t_unit_omarchy45's omarchy_host_gaps (#291, #293): each piece an --omarchy tree uses and the
    /// host lacks, saying what goes; none on a host with them, none for a tree that uses none.
    #[test]
    fn host_gaps_as_bash() {
        let d = std::env::temp_dir().join(format!("omabox-o45-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let d = shell::bytes(&d).to_vec();
        let put = |rel: &str, text: String| {
            let f = shell::path(&cat!(d, "/", rel));
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, text).unwrap();
        };
        put(
            "t/bin/omarchy-theme-bg-boot-intro",
            "#!/bin/bash\nowe status\n".into(),
        );
        put(
            "t/bin/omarchy-capture-screenshot",
            "#!/bin/bash\nomasnap --region\n".into(),
        );
        let none_so = cat!(d, "/none.so");
        put(
            "t/default/hypr/workspace-modes.lua",
            format!(
                "local TITLEBARS = \"{}\"\n",
                String::from_utf8_lossy(&none_so)
            ),
        );
        std::fs::create_dir_all(shell::path(&cat!(d, "/om/bin"))).unwrap();
        let t = cat!(d, "/t");
        let gaps: Vec<String> = omarchy_host_gaps_with(&t, |_| false, &none_so)
            .iter()
            .map(|g| String::from_utf8_lossy(g).into_owned())
            .collect();
        assert_eq!(
            gaps,
            [
                "owe: no video wallpapers and no theme intros",
                "omasnap: no Omasnap screenshots",
                "omarchy-hyprland-titlebars: no titlebars on floating workspaces"
            ],
            "each the tree uses and the host lacks, saying what goes"
        );
        // (the plugin there: a file the tree names, here the screenshot script)
        let so = cat!(t, "/bin/omarchy-capture-screenshot");
        put(
            "t/default/hypr/workspace-modes.lua",
            format!("local TITLEBARS = \"{}\"\n", String::from_utf8_lossy(&so)),
        );
        assert!(
            omarchy_host_gaps_with(&t, |_| true, &so).is_empty(),
            "...none with them installed"
        );
        assert!(
            omarchy_host_gaps_with(&cat!(d, "/om"), |_| false, b"/nonexistent").is_empty(),
            "...none for a tree that uses none (an older Omarchy)"
        );
        std::fs::remove_dir_all(shell::path(&d)).unwrap();
    }
    use std::cmp::Ordering;

    /// git_rev for a jailed caller (t_unit_jail_policy's, #198 security): nothing, and git never run
    /// on the jail's repo, whose core.fsmonitor would run a program here.
    /// git_rev in a jail (OMABOX_JAIL set, as the broker sets it for a jailed caller) runs no git: a
    /// real repo whose fsmonitor hook leaves a marker when git status runs it. The control, out of a
    /// jail: git runs, the hook leaves its marker, and the commit is read. (Without git on PATH, as
    /// in a build root without it, the control has nothing to run: only the jailed half.)
    #[test]
    fn git_rev_runs_no_git_for_a_jailed_caller() {
        let d = std::env::temp_dir().join(format!("omabox-gitrev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&d)
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        let have_git = git(&["init", "-q"]) && git(&["commit", "-q", "--allow-empty", "-m", "one"]);
        let ran = d.join("fsmonitor-ran");
        let hook = d.join("fsmonitor");
        std::fs::write(
            &hook,
            format!("#!/bin/sh\ntouch '{}'\nexit 1\n", ran.display()),
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        if have_git {
            assert!(git(&["config", "core.fsmonitor", hook.to_str().unwrap()]));
        }
        let db = d.to_str().unwrap().as_bytes().to_vec();
        let jailed = shell::with_env(&[("OMABOX_JAIL", Some(r#"{"id":"x"}"#))], || git_rev(&db));
        assert_eq!(jailed, b"".to_vec(), "jailed: no answer");
        assert!(
            !ran.exists(),
            "...and git never ran (its fsmonitor hook did not)"
        );
        if have_git {
            let free = shell::with_env(&[("OMABOX_JAIL", None)], || git_rev(&db));
            assert!(!free.is_empty(), "out of a jail: the commit read");
            assert!(ran.exists(), "...git ran, its fsmonitor hook with it");
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn versions_as_sort_v() {
        let gt = [
            ("0.15.2", "0.15.1"),
            ("0.16.0", "0.15.1"),
            ("0.15.10", "0.15.9"),
            ("1.0", "0.15.1"),
            ("0.15.1@7bb8bdf4", "0.15.1"),
            ("0.15.1.1", "0.15.1"),
            ("0.15.1", "0.15.01"),
            ("0.15.1", "0.15.1~rc1"),
            ("0.15.1a", "0.15.1"),
        ];
        for (a, b) in gt {
            assert_eq!(
                version_cmp(a.as_bytes(), b.as_bytes()),
                Ordering::Greater,
                "{a} {b}"
            );
            assert_eq!(
                version_cmp(b.as_bytes(), a.as_bytes()),
                Ordering::Less,
                "{b} {a}"
            );
        }
        assert!(aq_ver_fixed(b"0.15.2") && aq_ver_fixed(b"0.16"));
        assert!(!aq_ver_fixed(b"0.15.1") && !aq_ver_fixed(b"0.15.0") && !aq_ver_fixed(b"unknown"));
        assert!(!aq_ver_fixed(b"0.15.01"));
    }

    // The same pairs and more, against GNU sort -V itself where it is there (`sort -V | tail -1`).
    #[test]
    fn versions_as_the_real_sort_v() {
        let pairs = [
            "0.15.1 0.15.2",
            "0.15.1 0.15.01",
            "0.15.1 0.15.1~rc1",
            "0.15.1 0.15.1@7bb8bdf4",
            "0.15.1 0.15.1-1",
            "0.15.1 0.15.1a",
            "0.15.1 0.15.1.tar.gz",
            "0.15.1 1.0.0",
            "0.15.1 0.9.99",
            "0.15.1 0.15.1+appid",
            "a.b c",
            "1.2.3 1.2.3",
            ".a b",
            "x.1 x.01",
            "0.15.1 0.15",
        ];
        for p in pairs {
            let (a, b) = p.split_once(' ').unwrap();
            let Ok(o) = std::process::Command::new("sort")
                .arg("-V")
                .env("LC_ALL", "C")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .and_then(|mut c| {
                    use std::io::Write;
                    c.stdin
                        .take()
                        .unwrap()
                        .write_all(format!("{a}\n{b}\n").as_bytes())?;
                    c.wait_with_output()
                })
            else {
                return;
            };
            let last = String::from_utf8_lossy(&o.stdout)
                .lines()
                .last()
                .unwrap_or("")
                .to_string();
            let mine = if version_cmp(a.as_bytes(), b.as_bytes()).is_gt() {
                a
            } else {
                b
            };
            assert_eq!(mine, last, "{p}");
        }
    }

    #[test]
    fn readelf_lines() {
        let l = b" 0x0000000000000001 (NEEDED)             Shared library: [libaquamarine.so.8]";
        assert_eq!(
            dyn_value(l, b"NEEDED"),
            Some(b"libaquamarine.so.8".to_vec())
        );
        assert_eq!(dyn_value(l, b"SONAME"), None);
        let s = b" 0x000000000000000e (SONAME)             Library soname: [libaquamarine.so.7]";
        assert_eq!(
            dyn_value(s, b"SONAME"),
            Some(b"libaquamarine.so.7".to_vec())
        );
        assert_eq!(dyn_value(b"(NEEDED) [a] x", b"NEEDED"), None);
        assert_eq!(
            dyn_value(b"(NEEDED) [a [b]", b"NEEDED"),
            Some(b"b".to_vec())
        );
    }

    #[test]
    fn so_suffix_and_words() {
        assert_eq!(after_so(b"/usr/lib/libaquamarine.so.0.15.1"), b"0.15.1");
        assert_eq!(after_so(b"/a.so.1/libx.so.2.3"), b"2.3");
        assert_eq!(after_so(b"/usr/lib/libx"), b"/usr/lib/libx");
        assert!(has_word(b"exec owe --intro", b"owe"));
        assert!(has_word(b"owe", b"owe"));
        assert!(!has_word(b"owed owes", b"owe"));
        assert!(has_word(b"lowe owe_ x owe", b"owe"));
        assert!(!has_word(b"_owe", b"owe"));
    }

    #[test]
    fn git_link_dirs_from_the_files() {
        let t = std::env::temp_dir().join(format!("omabox-gl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let tb = crate::repo::readlink_f(shell::bytes(&t)).unwrap_or(shell::bytes(&t).to_vec());
        std::fs::create_dir_all(t.join("main/.git/worktrees/wt")).unwrap();
        std::fs::create_dir_all(t.join("wt")).unwrap();
        std::fs::write(t.join("wt/.git"), "gitdir: ../main/.git/worktrees/wt\n").unwrap();
        std::fs::write(t.join("main/.git/worktrees/wt/commondir"), "../..\n").unwrap();
        let tb = crate::repo::readlink_f(&tb).unwrap_or(tb);
        let (g, c) = git_link_dirs(&cat!(tb, "/wt"), None).unwrap();
        assert_eq!(g, cat!(tb, "/main/.git/worktrees/wt"));
        assert_eq!(c, cat!(tb, "/main/.git"));
        // (a main checkout: .git a dir)
        assert!(git_link_dirs(&cat!(tb, "/main"), None).is_none());
        // (a submodule's: no commondir, the gitdir is its own)
        std::fs::remove_file(t.join("main/.git/worktrees/wt/commondir")).unwrap();
        let (g, c) = git_link_dirs(&cat!(tb, "/wt"), None).unwrap();
        assert_eq!(g, c);
        // (a stale one: its gitdir gone)
        std::fs::write(t.join("wt/.git"), "gitdir: /nonexistent/x\n").unwrap();
        assert!(git_link_dirs(&cat!(tb, "/wt"), None).is_none());
        std::fs::remove_dir_all(&t).unwrap();
    }

    #[test]
    fn audio_list_as_bash() {
        // (only its shape: what is installed here varies) the missing names, in their order, each
        // once, joined by ", " (finding 462)
        if let Err(m) = audio_packages() {
            let all = ["pipewire", "wireplumber", "pipewire-pulse", "pactl"];
            let m = String::from_utf8(m).unwrap();
            let at: Vec<Option<usize>> = m
                .split(", ")
                .map(|n| all.iter().position(|a| *a == n))
                .collect();
            assert!(
                at.iter().all(Option::is_some) && at.windows(2).all(|w| w[0] < w[1]),
                "{m}"
            );
        }
    }

    /// The box's answers as t_unit_hyprland's and t_unit_quickshell's stubs gave them: hyprctl
    /// version (-j: JSON) says 0.0.1; a Quickshell at /b/quickshell or /usr/bin/quickshell says its
    /// version, naming itself.
    struct Says;

    impl crate::boxes::BoxSys for Says {
        fn on_box_io(
            &self,
            _: &Ctx,
            args: &[Vec<u8>],
            _: crate::exec::Out,
            _: crate::exec::ErrTo,
        ) -> crate::exec::Ran {
            let ran = |rc: i32, out: Vec<u8>| crate::exec::Ran {
                rc,
                out,
                refused: false,
                gone_said: false,
            };
            let a = |i: usize| args.get(i).map_or(&b""[..], Vec::as_slice);
            if a(3) == b"version" {
                return ran(
                    0,
                    if args.last().is_some_and(|l| l == b"-j") {
                        b"{\"version\": \"0.0.1\"}\n".to_vec()
                    } else {
                        b"Hyprland 0.0.1 built from branch x\n".to_vec()
                    },
                );
            }
            if a(2) == b"/b/quickshell" || a(2) == b"/usr/bin/quickshell" {
                return ran(0, cat!("Quickshell 0.0.1 (revision abc, by ", a(2), ")\n"));
            }
            ran(1, Vec::new())
        }
    }

    fn vbox(tag: &str, json: &str) -> (Ctx, std::path::PathBuf) {
        let d = std::env::temp_dir().join(format!("omabox-ver-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("box.json"), json).unwrap();
        let mut ctx = Ctx::new(b"up", Vec::new());
        ctx.name = b"x".to_vec();
        ctx.d = shell::bytes(&d).to_vec();
        (ctx, d)
    }

    fn field(d: &std::path::Path, k: &str) -> Option<String> {
        let v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(d.join("box.json")).ok()?).ok()?;
        v.get(k)?.as_str().map(str::to_string)
    }

    // t_unit_hyprland's: another version than the installed hyprctl's, a warning; box.json gets the
    // box's version line either way (#238). (#[ignore]: the host's /usr/bin/Hyprland says its own
    // version.)
    #[test]
    #[ignore = "runs the host's Hyprland --version-json: run by test/run.sh (cargo test -- --ignored)"]
    fn hypr_version_as_bash() {
        let (ctx, d) = vbox("h", r#"{"hyprland": "/b/Hyprland"}"#);
        let (_, said) = crate::exit::heard(|| hypr_version_with(&Says, &ctx));
        assert!(
            said.contains(
                "warning: box 'x' runs Hyprland 0.0.1, but hyprctl and hyprpm are the installed"
            ),
            "another Hyprland version than hyprctl's: a warning: {said}"
        );
        assert_eq!(
            field(&d, "hyprland_version").as_deref(),
            Some("Hyprland 0.0.1 built from branch x"),
            "...and box.json has its version line"
        );
        std::fs::write(d.join("box.json"), "{}").unwrap();
        let (_, said) = crate::exit::heard(|| hypr_version_with(&Says, &ctx));
        assert_eq!(said, "", "a box on the installed Hyprland: no warning");
        assert_eq!(
            field(&d, "hyprland_version").as_deref(),
            Some("Hyprland 0.0.1 built from branch x"),
            "...and box.json has its version line too (#238)"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    // t_unit_quickshell's: the version line, from inside the box, every box's, the build's run by
    // its path (finding 358).
    #[test]
    fn qs_version_as_bash() {
        let (ctx, d) = vbox("q", r#"{"quickshell": "/b/quickshell"}"#);
        qs_version_with(&Says, &ctx);
        assert_eq!(
            crate::shellnote::box_qs(&ctx),
            b"/b/quickshell",
            "a build: the box's quickshell is it"
        );
        assert_eq!(
            field(&d, "quickshell_version").as_deref(),
            Some("Quickshell 0.0.1 (revision abc, by /b/quickshell)"),
            "...and box.json has its version line, from it"
        );
        std::fs::write(d.join("box.json"), "{}").unwrap();
        qs_version_with(&Says, &ctx);
        assert_eq!(
            crate::shellnote::box_qs(&ctx),
            b"/usr/bin/quickshell",
            "the installed one otherwise"
        );
        assert_eq!(
            field(&d, "quickshell_version").as_deref(),
            Some("Quickshell 0.0.1 (revision abc, by /usr/bin/quickshell)"),
            "...and its version line too"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
