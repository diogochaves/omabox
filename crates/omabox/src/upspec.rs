//! up's option values (#324 wave 5a): mounts, seeds, addresses, ports, --clean's, as up reads them.

// up --clean (#252, finding 357): what it sets, and what --keep can take back. The theme (--stock's
// too) is Omarchy's default; the wallpaper a colour no theme uses, so any of it inside a window or the bar is a hole (a
// 16x16 PNG of it, scaled); the clock a literal Qt format, which keeps its widget, place and width.
// The locale and timezone (finding 363): English in UTF-8 and UTC for the whole session.
pub const CLEAN_THEME: &str = "tokyo-night";
pub const CLEAN_WALLPAPER: &str = "#ff00ff";
pub const CLEAN_WALLPAPER_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAAAGUlEQVQoz2P8z/CfgRTARJLqUQ2jGoaUBgBKTgIeHXO4xgAAAABJRU5ErkJggg==";
pub const CLEAN_CLOCK: &str = "'Friday 12:00'";
pub const CLEAN_CLOCK_VERTICAL: &str = "'12'\n'\u{2014}'\n'00'";
pub const CLEAN_TZ: &str = "UTC";
pub const CLEAN_KEEPS: &str = "animations bar clock fonts identity locale pointer wallpaper";
/// A clean box's user and host names (#247, from desk-plain's eba7dea): nothing of the user's in a
/// shot (a shell's greeting, a terminal's title, a prompt). `--keep identity` keeps the user's.
pub const CLEAN_USER: &str = "user";
pub const CLEAN_HOST: &str = "omabox";
/// A clean plain box's sentinel (#247): the colour its Hyprland fills the screen with where no
/// surface is (plain.lua's misc.background_color, from omabox.clean), CLEAN_WALLPAPER's.
pub const CLEAN_BACKGROUND: &str = "rgb(ff00ff)";
// up --stock (#265, finding 613): what of the desk a stock box leaves out (seed_home's `plan.stock`),
// as up says it. A clean box is a stock one with the freezes above.
pub const STOCK_LEFT_OUT: &str =
    "terminal configs, branding, themes, wallpapers, the plugins' settings, Hyprland toggles";

use crate::boxes;
use crate::cat;
use crate::exit::{Ctx, Res, say};
use crate::jail::in_jail;
use crate::jailpath::{jail_deep, jail_masked, jail_path, jail_root_covered, jail_sees};
use crate::procfs;
use crate::repo::{jail_root, readlink_e, readlink_f, refuse_src, repo_top};
use crate::shell::{self, re_match};

/// A box's desktop (`up --desktop`, #247): what its Hyprland runs and what of Omarchy's it gets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Desktop {
    /// Omarchy's: its Hyprland config, its shell, the user's Omarchy look.
    #[default]
    Omarchy,
    /// omabox's own small config (share/plain.lua), and nothing of Omarchy's: its tree hidden in
    /// the box, no shell, a HOME with no Omarchy look.
    Plain,
    /// The user's own desk (#321): a plain box (plain_like) whose HOME sits at the user's own HOME
    /// path (desk_home), so a desktop that keeps absolute paths in its settings finds them there.
    Desk,
}

/// What `up --desktop` takes, as help says it.
pub const DESKTOPS: &str = "omarchy, plain or desk";

impl Desktop {
    /// --desktop's value; None for one that is not a desktop.
    pub fn parse(v: &[u8]) -> Option<Desktop> {
        match v {
            b"omarchy" => Some(Desktop::Omarchy),
            b"plain" => Some(Desktop::Plain),
            b"desk" => Some(Desktop::Desk),
            _ => None,
        }
    }

    /// Its name, as box.json, OMABOX_DESKTOP and --desktop have it.
    pub fn name(self) -> &'static str {
        match self {
            Desktop::Omarchy => "omarchy",
            Desktop::Plain => "plain",
            Desktop::Desk => "desk",
        }
    }

    /// Plain or desk (#321): nothing of Omarchy's, its tree hidden, a plain HOME. A desk box is a
    /// plain one whose HOME is at the user's HOME path.
    pub fn plain_like(self) -> bool {
        matches!(self, Desktop::Plain | Desktop::Desk)
    }

    /// A box.json's `desktop`: a box from before #247 has none, and is Omarchy's.
    pub fn of_box(v: &[u8]) -> Desktop {
        if v.is_empty() {
            Desktop::Omarchy
        } else {
            Desktop::parse(v).unwrap_or(Desktop::Omarchy)
        }
    }
}

/// How a box's desktop was chosen (#247): what up says of a plain one it was not asked for, and the
/// words of a refusal of what needs Omarchy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Chosen {
    /// `--desktop`, or `--omarchy DIR` (a tree of Omarchy's: an Omarchy box).
    #[default]
    Given,
    /// `omabox config desktop omarchy|plain`.
    Config,
    /// By what the machine has: Omarchy's when it is installed (install::omarchy_at, the test doctor
    /// and setup use), plain when it is not.
    Machine,
}

/// The config's `desktop` as a Desktop: None for `auto` (the default: by what the machine has).
/// Never desk (#321): a desk box is asked for with --desktop desk until it is complete.
pub fn conf_desktop(v: &[u8]) -> Option<Desktop> {
    Desktop::parse(v).filter(|d| *d != Desktop::Desk)
}

/// desk_home HOME (#321): where a desk box's HOME sits in the box, the user's $HOME as written
/// (trailing slashes dropped; no canonicalising, so a HOME through a link stays that path). Err
/// says why it cannot be: not absolute, `/`, `.` or `..` or empty parts, not UTF-8, a control
/// character, or what refuse_dest keeps for the box itself (its system dirs, runtime dir, /tmp,
/// /home/sbx, omabox's own, and anything above them).
pub fn desk_home(home: &[u8]) -> Result<Vec<u8>, Vec<u8>> {
    let mut h = home;
    while h.len() > 1 && h.ends_with(b"/") {
        h = &h[..h.len() - 1];
    }
    let why: Option<Vec<u8>> = if h.is_empty() {
        Some(b"it is empty".to_vec())
    } else if !h.starts_with(b"/") {
        Some(b"it is not an absolute path".to_vec())
    } else if h == b"/" {
        Some(b"it is /".to_vec())
    } else if std::str::from_utf8(h).is_err() {
        // (paths built from it are read lossily in places, e.g. record's folder: refused whole)
        Some(b"it is not valid UTF-8".to_vec())
    } else if h.iter().any(|c| c.is_ascii_control()) {
        Some(b"it has a control character".to_vec())
    } else if !boxes::clean_abs(h) {
        Some(b"it has an empty, . or .. part".to_vec())
    } else {
        refuse_dest(h, boxes::SBX_HOME.as_bytes())
    };
    match why {
        Some(w) => Err(cat!(
            "a desk box needs your HOME at its own path, and ",
            h,
            " cannot be: ",
            w
        )),
        None => Ok(h.to_vec()),
    }
}

/// The desktop a box gets, and how it was chosen: --desktop's; an Omarchy box for `--omarchy DIR`;
/// the config's (`omabox config desktop`); else by what the machine has: Omarchy's where Omarchy is
/// installed (OMARCHY_HERE: install::omarchy_at), plain where it is not. The one place the default
/// is chosen (#247).
pub fn resolve_desktop(
    given: Option<Desktop>,
    omarchy_dir: bool,
    conf: Option<Desktop>,
    omarchy_here: bool,
) -> (Desktop, Chosen) {
    if let Some(d) = given {
        return (d, Chosen::Given);
    }
    if omarchy_dir {
        return (Desktop::Omarchy, Chosen::Given);
    }
    if let Some(d) = conf {
        return (d, Chosen::Config);
    }
    if omarchy_here {
        (Desktop::Omarchy, Chosen::Machine)
    } else {
        (Desktop::Plain, Chosen::Machine)
    }
}

/// Why a box is plain, as a refusal of what needs Omarchy ends: "OPT needs Omarchy, and ...".
/// A desk box (#321) is only ever asked for with --desktop desk.
pub fn plain_why(d: Desktop, how: Chosen) -> &'static str {
    match how {
        Chosen::Given if d == Desktop::Desk => "--desktop desk has none of it",
        Chosen::Given => "--desktop plain has none of it",
        Chosen::Config => {
            "the box is plain (omabox config desktop plain): --desktop omarchy for an Omarchy box"
        }
        Chosen::Machine => "this machine has none (the box is plain)",
    }
}

/// What up says of a plain box it was not asked for with --desktop; None for any other.
pub fn chosen_text(d: Desktop, how: Chosen) -> Option<&'static str> {
    match (d, how) {
        (Desktop::Plain, Chosen::Machine) => Some(
            "desktop: plain, as Omarchy is not installed here (omabox's own small Hyprland config, no shell)",
        ),
        (Desktop::Plain, Chosen::Config) => Some(
            "desktop: plain, from omabox config desktop (--desktop omarchy for an Omarchy box)",
        ),
        _ => None,
    }
}

/// up --idle and $OMABOX_IDLE as whole seconds: `check run` shares bash's idle_duration.
#[allow(unused_imports)]
pub use crate::cmd::check::idle_duration;

/// P is DIR or below it (`[ "$p" = "$d" ] || [[ $p == "$d"/* ]]`).
fn at_or_under(p: &[u8], d: &[u8]) -> bool {
    p == d || (p.len() > d.len() && p.starts_with(d) && p[d.len()] == b'/')
}

/// `${src/#\~/$HOME}`: a leading `~` (only the `~`) as HOME.
fn tilde(p: &[u8]) -> Vec<u8> {
    match p.strip_prefix(b"~") {
        Some(rest) => cat!(shell::env_or_empty("HOME"), rest),
        None => p.to_vec(),
    }
}

/// `realpath -m -- P` (GNU's, canonicalize-missing): absolute from the physical cwd, each link that is
/// there followed (`..` after it from where it leads), what is missing taken as written. None for a
/// link loop (ELOOP).
pub fn realpath_m(p: &[u8]) -> Option<Vec<u8>> {
    let full = if p.first() == Some(&b'/') {
        p.to_vec()
    } else {
        let cwd = std::env::current_dir().ok()?;
        cat!(shell::bytes(&cwd), "/", p)
    };
    let parts =
        |v: &[u8]| -> Vec<Vec<u8>> { v.split(|&c| c == b'/').map(<[u8]>::to_vec).collect() };
    let mut todo: std::collections::VecDeque<Vec<u8>> = parts(&full).into();
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut hops = 0;
    while let Some(c) = todo.pop_front() {
        match c.as_slice() {
            b"" | b"." => continue,
            b".." => {
                out.pop();
                continue;
            }
            _ => out.push(c),
        }
        let cur = cat!("/", out.join(&b'/'));
        let Ok(m) = std::fs::symlink_metadata(shell::path(&cur)) else {
            continue;
        };
        if !m.file_type().is_symlink() {
            continue;
        }
        hops += 1;
        if hops > 40 {
            return None;
        }
        let t = std::fs::read_link(shell::path(&cur)).ok()?;
        let t = shell::bytes(&t).to_vec();
        if t.first() == Some(&b'/') {
            out.clear();
        } else {
            out.pop();
        }
        for x in parts(&t).into_iter().rev() {
            todo.push_front(x);
        }
    }
    Some(cat!("/", out.join(&b'/')))
}

/// robind_spec SPEC (DIR or DIR:DEST, DEST absolute, e.g. ~/nas:/mnt/nas to test path mapping): the
/// (source, dest) a read-only mount takes, both resolved; None when DIR is not there. A jailed
/// caller's DIR (#198) is followed only inside its jail's folders, and must be one of them, whole,
/// or with what the jail hides in it covered in the box (#210).
pub fn robind_spec(spec: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let (src, dest) = match spec.windows(2).position(|w| w == b":/") {
        Some(i) => (&spec[..i], Some(cat!("/", &spec[i + 2..]))),
        None => (spec, None),
    };
    let src = tilde(src);
    let src = if in_jail() {
        let q = jail_path(&src)?;
        if !shell::path(&q).exists() {
            return None;
        }
        jail_root_covered(&q)?
    } else {
        if !shell::path(&src).exists() {
            return None;
        }
        readlink_f(&src)?
    };
    // (//usr and /a/../usr are /usr)
    // (a realpath that fails, a link loop, leaves DEST empty in bash's `||` list: the source's path)
    let dest = dest
        .and_then(|d| realpath_m(&d))
        .unwrap_or_else(|| src.clone());
    Some((src, dest))
}

/// Why seed_spec refused: 1 a bad value (said), 3 a jailed caller's SRC it may not have (said; the
/// command line is read by then: `parsed; refused`).
#[derive(Debug, PartialEq, Eq)]
pub enum SeedRefused {
    Bad,
    Jail,
}

/// seed_spec SRC:DEST HOME (--seed): SRC resolved and DEST relative to the box HOME (HOME: the box's
/// HOME inside it, /home/sbx or a desk box's, #321), or why not (said). A jailed caller's SRC (#198)
/// is followed only inside its jail's folders, then resolved and checked there (a link swapped in
/// since is caught); refused as written, one way for all.
pub fn seed_spec(spec: &[u8], home: &[u8]) -> Result<(Vec<u8>, Vec<u8>), SeedRefused> {
    let i = spec.iter().position(|&c| c == b':');
    let (src, dest) = match i {
        Some(i) => (&spec[..i], &spec[i + 1..]),
        None => (spec, &b""[..]),
    };
    if i.is_none() || src.is_empty() || dest.is_empty() {
        say(cat!(
            "--seed: SRC:DEST (a file or folder, and where in the box HOME), got ",
            spec
        ));
        return Err(SeedRefused::Bad);
    }
    let home_pre = cat!(home, "/");
    let dest = if let Some(d) = dest.strip_prefix(b"~/") {
        d
    } else if let Some(d) = dest.strip_prefix(home_pre.as_slice()) {
        d
    } else if dest.first() == Some(&b'/') {
        say(cat!(
            "--seed: DEST is in the box HOME (.config/x, ~/.config/x), got ",
            dest
        ));
        return Err(SeedRefused::Bad);
    } else {
        dest
    };
    // (`realpath -m --relative-base=/h`: /h is no folder of the host's)
    let r = realpath_m(&cat!("/h/", dest)).unwrap_or_default();
    let r = if r == b"/h" {
        b".".to_vec()
    } else {
        r.strip_prefix(b"/h/").map_or(r.clone(), <[u8]>::to_vec)
    };
    if r.first() == Some(&b'/') || r == b"." || r.is_empty() {
        say(cat!("--seed: DEST must be inside the box HOME, got ", dest));
        return Err(SeedRefused::Bad);
    }
    let src = tilde(src);
    let src = if in_jail() {
        let ok = jail_path(&src)
            .filter(|q| shell::path(q).exists())
            .and_then(|q| jail_sees(&q))
            .filter(|s| !jail_masked(s) && !jail_deep(s));
        match ok {
            Some(s) => s,
            None => {
                say(cat!(
                    "up: --seed ",
                    &spec[..spec.iter().position(|&c| c == b':').unwrap_or(spec.len())],
                    " is not a file or folder in a folder this jail sees, so it does not go into the box"
                ));
                return Err(SeedRefused::Jail);
            }
        }
    } else {
        if !shell::path(&src).exists() {
            say(cat!("--seed: no such path: ", src));
            return Err(SeedRefused::Bad);
        }
        // (a readlink -f that fails ends bash's `$(seed_spec)` under set -e, unsaid)
        readlink_f(&src).ok_or(SeedRefused::Bad)?
    };
    Ok((src, r))
}

/// `IFS=, ; for w in $LIST`: LIST's comma-separated words, empty ones left out. (Bash's unquoted
/// expansion also globbed each word against the current folder; omabox takes them as written,
/// finding 439.)
fn comma_words(v: &[u8]) -> impl Iterator<Item = &[u8]> {
    ifs_comma(v).into_iter().filter(|w| !w.is_empty())
}

/// `IFS=, ; for w in $LIST`'s fields as bash splits them: an empty one between two commas or before
/// the first is kept, the one after a last comma is not (a word clean_keep refuses: `got `).
fn ifs_comma(v: &[u8]) -> Vec<&[u8]> {
    let mut f: Vec<&[u8]> = v.split(|&c| c == b',').collect();
    if f.last().is_some_and(|w| w.is_empty()) {
        f.pop();
    }
    f
}

/// no_seed_spec LIST HAVE (#231): what of the desk a box starts without, comma-separated, repeatable
/// (HAVE is what earlier ones gave): extensions and fonts. The list, each once; Err when a word is
/// none of them (said).
pub fn no_seed_spec(list: &[u8], have: &[u8]) -> Result<Vec<u8>, ()> {
    let mut out: Vec<&[u8]> = Vec::new();
    for w in comma_words(have).chain(comma_words(list)) {
        if w != b"extensions" && w != b"fonts" {
            say(cat!(
                "--no-seed: what a box starts without of your desk, comma-separated: extensions (~/.config/omarchy/extensions, your plugins' settings), fonts (~/.local/share/fonts and ~/.fonts), got '",
                w,
                "'"
            ));
            return Err(());
        }
        if !out.contains(&w) {
            out.push(w);
        }
    }
    Ok(out.join(&b','))
}

/// --allow's ports (S5, #355): each 1-65535, in order, once (finding 175: 8082,8081,8081 is 8081,8082,
/// so a box up with them compares equal however they were given). What an isolated box's pasta
/// forwards, and nothing else.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AllowedPorts(Vec<u16>);

impl AllowedPorts {
    /// allow_ports LIST: Err for a port out of 1-65535 (pasta's own message came late, from inside
    /// the box). An empty LIST allows none.
    pub fn parse(list: &[u8]) -> Result<AllowedPorts, ()> {
        let mut ps: Vec<u16> = Vec::new();
        if list.is_empty() {
            return Ok(AllowedPorts(ps));
        }
        // (`IFS=, read -ra`: a trailing comma's empty field is dropped, any other is refused)
        let mut fields: Vec<&[u8]> = list.split(|&c| c == b',').collect();
        if fields.last().is_some_and(|f| f.is_empty()) {
            fields.pop();
        }
        for f in fields {
            if !re_match("^[0-9]{1,5}$", f) {
                return Err(());
            }
            match shell::dec(f).and_then(|n| u16::try_from(n).ok()) {
                Some(n) if n >= 1 => ps.push(n),
                _ => return Err(()),
            }
        }
        ps.sort_unstable();
        ps.dedup();
        Ok(AllowedPorts(ps))
    }

    pub fn ports(&self) -> &[u16] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// As box.json and pasta's -T take them: 8081,8082 (empty for none).
    pub fn csv(&self) -> Vec<u8> {
        self.0
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(",")
            .into_bytes()
    }
}

/// box_addr ADDR (#229): --addr's address as the box has it (203.0.113.010 is 203.0.113.10; IPv6 in
/// its short form, RFC 5952), or None for one outside the documentation ranges: RFC 5737's
/// 192.0.2.0/24, 198.51.100.0/24 and 203.0.113.0/24, RFC 3849's 2001:db8::/32. No real host has one,
/// so an address on the box's loopback never hides a real one from its apps.
pub fn box_addr(addr: &[u8]) -> Option<Vec<u8>> {
    let a = addr.to_ascii_lowercase();
    if re_match("^[0-9]{1,3}(\\.[0-9]{1,3}){3}$", &a) {
        let os: Vec<i64> = a
            .split(|&c| c == b'.')
            .map(shell::dec)
            .collect::<Option<_>>()?;
        if os.iter().any(|&o| o > 255) {
            return None;
        }
        let v4 = format!("{}.{}.{}.{}", os[0], os[1], os[2], os[3]);
        let ok = ["192.0.2.", "198.51.100.", "203.0.113."]
            .iter()
            .any(|p| v4.starts_with(p));
        return ok.then(|| v4.into_bytes());
    }
    if !re_match("^[0-9a-f:]+$", &a) || crate::exit::contains(&a, b":::") {
        return None;
    }
    // (`IFS=: read -ra`: a trailing empty field dropped, an empty string none)
    let read_ra = |v: &[u8]| -> Vec<Vec<u8>> {
        let mut f: Vec<Vec<u8>> = v.split(|&c| c == b':').map(<[u8]>::to_vec).collect();
        if f.last().is_some_and(Vec::is_empty) {
            f.pop();
        }
        f
    };
    let g: Vec<Vec<u8>> = if let Some(i) = a.windows(2).position(|w| w == b"::") {
        let (head, tail) = (&a[..i], &a[i + 2..]);
        if crate::exit::contains(tail, b"::") {
            return None;
        }
        let (hs, ts) = (read_ra(head), read_ra(tail));
        if hs.len() + ts.len() > 7 {
            return None;
        }
        let zeros = 8 - hs.len() - ts.len();
        hs.into_iter()
            .chain(std::iter::repeat_n(b"0".to_vec(), zeros))
            .chain(ts)
            .collect()
    } else {
        if a.first() == Some(&b':') || a.last() == Some(&b':') {
            return None;
        }
        let g = read_ra(&a);
        if g.len() != 8 {
            return None;
        }
        g
    };
    let mut gs: Vec<String> = Vec::with_capacity(8);
    for x in &g {
        if !re_match("^[0-9a-f]{1,4}$", x) {
            return None;
        }
        let n = u16::from_str_radix(std::str::from_utf8(x).ok()?, 16).ok()?;
        gs.push(format!("{n:x}"));
    }
    if gs[0] != "2001" || gs[1] != "db8" {
        return None;
    }
    // The longest run of zero groups (the first of two as long), two or more, is ::.
    let (mut run, mut best, mut bl) = (0, 0, 0);
    for (i, x) in gs.iter().enumerate() {
        if x == "0" {
            run += 1;
            if run > bl {
                bl = run;
                best = i + 1 - run;
            }
        } else {
            run = 0;
        }
    }
    let s = if bl >= 2 {
        format!("{}::{}", gs[..best].join(":"), gs[best + bl..].join(":"))
    } else {
        gs.join(":")
    };
    Some(s.into_bytes())
}

/// What omabox keeps in a box's HOME and reads back (its logs, its record and config folders, the
/// plugins it mounts): nothing is mounted at, in or above them in a desk box's HOME (refuse_dest,
/// #321). `run-*.log` is each of run -d's logs.
pub const OWN_IN_HOME: [&str; 6] = [
    "events.log",
    "run-*.log",
    "gdb.log",
    ".cache/omabox-record",
    ".config/omabox",
    ".config/omarchy/plugins",
];

/// refuse_dest PATH HOME (S3): why a path inside the box must not be mounted over, or None when it
/// may: the system dirs, the runtime dir, /home/sbx, and omabox's own mounts or anything above them.
/// Inside the box's /tmp (its own tmpfs) is fine, so a repo in /tmp mounts at its own path; /tmp
/// itself and Xwayland's socket dirs are not. HOME is the box's HOME inside it (#321): never it nor
/// above it; inside /home/sbx nothing; inside a desk box's (the user's HOME path) anything but
/// omabox's own files there (OWN_IN_HOME) and what holds them.
pub fn refuse_dest(p: &[u8], home: &[u8]) -> Option<Vec<u8>> {
    let d = cat!(p.strip_suffix(b"/").unwrap_or(p), "/");
    let own = d == b"/"
        || d == b"/tmp/"
        || [
            "/usr/",
            "/etc/",
            "/proc/",
            "/dev/",
            "/sys/",
            "/run/",
            "/bin/",
            "/lib/",
            "/lib64/",
            "/tmp/.X11-unix/",
            "/tmp/.ICE-unix/",
            "/opt/omabox/",
            "/home/sbx/",
        ]
        .iter()
        .any(|x| d.starts_with(x.as_bytes()));
    if own {
        return Some(b"that is the box's own system, runtime dir, /tmp or HOME".to_vec());
    }
    if let Some(x) = ["/home/sbx/", "/opt/omabox/", "/tmp/"]
        .iter()
        .find(|x| x.as_bytes().starts_with(&d))
    {
        return Some(cat!("it would hide ", x));
    }
    // A desk box's HOME (#321; all of /home/sbx is refused above): never it nor above it, nor
    // omabox's own files in it.
    let h = cat!(home.strip_suffix(b"/").unwrap_or(home), "/");
    if h.starts_with(&d) {
        return Some(cat!(
            "that is the box's HOME (",
            &h[..h.len() - 1],
            ") or above it"
        ));
    }
    let rel = d.strip_prefix(h.as_slice())?;
    let first = rel.split(|&c| c == b'/').next().unwrap_or_default();
    for own in OWN_IN_HOME {
        let o = cat!(own, "/");
        let run_log = own == "run-*.log" && first.starts_with(b"run-") && first.ends_with(b".log");
        if run_log || rel.starts_with(&o) {
            return Some(cat!("that is omabox's own ~/", own, " in the box HOME"));
        }
        if o.starts_with(rel) {
            return Some(cat!(
                "it would hide omabox's own ~/",
                own,
                " in the box HOME"
            ));
        }
    }
    None
}

/// own_path_visible PATH ROBINDS OVERLAYS (findings 240, 241): PATH is in the box at its own host
/// path by a mount up makes anyway: under the repo up runs from (unless refuse_src keeps it out), a
/// same-path --ro-bind (ROBINDS: up's, which --hyprland, --omarchy and mise's add to) or an --overlay,
/// with no mount from elsewhere at or above it. A jailed caller's (#210): the jail sees PATH (not
/// through a link out of its folders), and each mount is of a folder of the jail's, whole (what is
/// linked to this way is read on the host, where a hidden path could be moved in with its folder).
pub fn own_path_visible(p: &[u8], robinds: &[(Vec<u8>, Vec<u8>)], overlays: &[Vec<u8>]) -> bool {
    let Some(p) = readlink_e(p) else {
        return false;
    };
    if in_jail() && jail_sees(&p).is_none() {
        return false;
    }
    let mut specs: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    if let Some(top) = repo_top().filter(|t| refuse_src(t).is_none()) {
        specs.push((top.clone(), top));
    }
    specs.extend(robinds.iter().cloned());
    specs.extend(overlays.iter().map(|x| (x.clone(), x.clone())));
    own_path_among(&p, &specs)
}

/// own_path_visible's rule over the mounts SPECS (source, destination), for P resolved: P is at or
/// under one of their destinations, and each such is a mount of the same path (`plugin add` reads a
/// running box's from its box.json, #345).
pub fn own_path_among(p: &[u8], specs: &[(Vec<u8>, Vec<u8>)]) -> bool {
    let mut seen = false;
    for (src, dest) in specs {
        if !at_or_under(p, dest) {
            continue;
        }
        if src != dest || (in_jail() && jail_root(src).is_none()) {
            return false;
        }
        seen = true;
    }
    seen
}

/// clean_locale: en_US.UTF-8 when the system has it (`locale -a`), else C.UTF-8 (glibc's own).
pub fn clean_locale() -> &'static str {
    let mut c = crate::sig::command("locale");
    c.arg("-a").stderr(std::process::Stdio::null());
    let has = crate::sig::run_output(c).is_ok_and(|(_, out)| {
        out.split(|&b| b == b'\n').any(|l| {
            let l = l.to_ascii_lowercase();
            l == b"en_us.utf8" || l == b"en_us.utf-8"
        })
    });
    if has { "en_US.UTF-8" } else { "C.UTF-8" }
}

/// clean_keep LIST (--keep): comma-separated, each one of CLEAN_KEEPS, given back once each; Err
/// (said) for any other, or none.
pub fn clean_keep(list: &[u8]) -> Result<Vec<u8>, ()> {
    let takes = CLEAN_KEEPS.replace(' ', ", ");
    let mut out: Vec<&[u8]> = Vec::new();
    for k in ifs_comma(list) {
        if !CLEAN_KEEPS.as_bytes().split(|&c| c == b' ').any(|x| x == k) {
            say(cat!(
                "up: --keep takes ",
                takes,
                " (comma-separated), got ",
                k
            ));
            return Err(());
        }
        if !out.contains(&k) {
            out.push(k);
        }
    }
    if out.is_empty() {
        say(cat!("up: --keep takes ", takes, " (comma-separated)"));
        return Err(());
    }
    Ok(out.join(&b','))
}

/// clean_has KEEP WHAT: --keep (comma-separated, as clean_keep gives it) keeps WHAT.
pub fn clean_has(keep: &[u8], what: &str) -> bool {
    keep.split(|&c| c == b',').any(|k| k == what.as_bytes())
}

/// The host's LANG and LANGUAGE and LC_* as a clean box --keep locale gets them: "LANG NAME=V ...",
/// the LC_ names in byte order (bash's ${!LC_*}), empty ones left out.
fn host_locale() -> Vec<u8> {
    let lang = shell::env("LANG")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| b"C.UTF-8".to_vec());
    let mut lc: Vec<(Vec<u8>, Vec<u8>)> = std::env::vars_os()
        .map(|(k, v)| (k.into_encoded_bytes(), v.into_encoded_bytes()))
        // (a name bash takes as a variable: one with other characters is not imported)
        .filter(|(k, _)| {
            k.starts_with(b"LC_") && k.iter().all(|&c| c.is_ascii_alphanumeric() || c == b'_')
        })
        .collect();
    lc.sort();
    let mut s = lang;
    let language = shell::env_or_empty("LANGUAGE");
    for (k, v) in std::iter::once((b"LANGUAGE".to_vec(), language)).chain(lc) {
        if !v.is_empty() {
            s = cat!(s, " ", k, "=", v);
        }
    }
    s
}

/// clean_locale_rec CLEAN_LANG: a clean box's locale as its box.json records it (check.json's, which
/// a check compares): its own, or with --keep locale (CLEAN_LANG empty) the host's.
pub fn clean_locale_rec(clean_lang: &[u8]) -> Vec<u8> {
    if !clean_lang.is_empty() {
        return clean_lang.to_vec();
    }
    host_locale()
}

/// clean_zone_rec ENVS CLEAN_ZONE: a clean box's timezone as its box.json records it: a TZ --env
/// gives (the last), else its own, else the host's /etc/localtime's zone.
pub fn clean_zone_rec(envs: &[(Vec<u8>, Vec<u8>)], clean_zone: &[u8]) -> Vec<u8> {
    if let Some((_, tz)) = envs.iter().rev().find(|(k, _)| k == b"TZ")
        && !tz.is_empty()
    {
        return tz.clone();
    }
    if !clean_zone.is_empty() {
        return clean_zone.to_vec();
    }
    let z = readlink_f(b"/etc/localtime").unwrap_or_default();
    z.strip_prefix(b"/usr/share/zoneinfo/")
        .map_or(z.clone(), <[u8]>::to_vec)
}

/// clean_text KEEP THEME NET CLEAN_LANG CLEAN_ZONE PLAIN: what a clean box is, as up says it. A plain
/// box (#247) has no bar, theme or clock of Omarchy's, and none of the user's look but fonts.
pub fn clean_text(
    keep: &[u8],
    theme: &[u8],
    net: &[u8],
    clean_lang: &[u8],
    clean_zone: &[u8],
    plain: bool,
) -> Vec<u8> {
    if plain {
        return plain_clean_text(keep, net, clean_lang, clean_zone);
    }
    let mut t = look_text(true, keep, theme, net, clean_lang, clean_zone);
    if !clean_has(keep, "identity") {
        t.extend_from_slice(&cat!(", as ", CLEAN_USER, "@", CLEAN_HOST));
    }
    t
}

/// A clean plain box's words: its background the sentinel, then the freezes as on Omarchy's.
fn plain_clean_text(keep: &[u8], net: &[u8], clean_lang: &[u8], clean_zone: &[u8]) -> Vec<u8> {
    let has = |w| clean_has(keep, w);
    let mut parts: Vec<Vec<u8>> = Vec::new();
    if !has("wallpaper") {
        parts.push(cat!(
            "background ",
            CLEAN_WALLPAPER,
            " (any of it inside a window is a hole)"
        ));
    }
    if !has("locale") {
        parts.push(cat!(
            "locale ",
            or_else(clean_lang, clean_locale().as_bytes()),
            " in ",
            or_else(clean_zone, CLEAN_TZ.as_bytes())
        ));
    }
    if !has("fonts") {
        parts.push(b"none of your fonts".to_vec());
    }
    if !has("pointer") {
        parts.push(b"no pointer".to_vec());
    }
    if net == b"isolated" {
        parts.push(b"no network".to_vec());
    }
    if !has("animations") {
        parts.push(b"animations off".to_vec());
    }
    if !has("identity") {
        parts.push(cat!("as ", CLEAN_USER, "@", CLEAN_HOST));
    }
    parts.join(b", ".as_slice())
}

/// V, or D when V is empty.
fn or_else(v: &[u8], d: &[u8]) -> Vec<u8> {
    if v.is_empty() { d.to_vec() } else { v.to_vec() }
}

/// clean_ids TEXT ME PASSWD HOME (#247): /etc/passwd (PASSWD) or /etc/group as a clean box has them:
/// ME's entry named CLEAN_USER, in passwd with no full name and the box HOME (HOME, #321), and ME
/// among a group's members CLEAN_USER too. Every other line as it was; uid and gid stay (the box's
/// files are ME's).
pub fn clean_ids(text: &[u8], me: &[u8], passwd: bool, home: &[u8]) -> Vec<u8> {
    if me.is_empty() {
        return text.to_vec();
    }
    let mut out = Vec::with_capacity(text.len());
    let mut lines = text.split(|&c| c == b'\n').peekable();
    while let Some(l) = lines.next() {
        let last = lines.peek().is_none();
        let mut f: Vec<Vec<u8>> = l.split(|&c| c == b':').map(<[u8]>::to_vec).collect();
        if f.len() >= 4 && f[0] == me {
            f[0] = CLEAN_USER.as_bytes().to_vec();
            if passwd && f.len() == 7 {
                f[4].clear();
                f[5] = home.to_vec();
            }
        }
        if !passwd && f.len() == 4 {
            f[3] = f[3]
                .split(|&c| c == b',')
                .map(|m| if m == me { CLEAN_USER.as_bytes() } else { m })
                .collect::<Vec<_>>()
                .join(&b',');
        }
        out.extend_from_slice(&f.join(&b':'));
        if !last {
            out.push(b'\n');
        }
    }
    out
}

/// What up says under its stock (or clean) line when the box starts --from SAVE (#265, finding
/// 638): --from keeps the save's HOME, so "none of your look" holds for the desk's, not the save's.
pub fn stock_over_save_text(save: &[u8]) -> Vec<u8> {
    cat!(
        "...over save '",
        save,
        "': its HOME as saved (its files, configs and their look); the bar, theme and locale above are stock"
    )
}

/// stock_text THEME CLEAN_LANG CLEAN_ZONE: what a stock box is (#265, finding 613), as up says it.
pub fn stock_text(theme: &[u8], clean_lang: &[u8], clean_zone: &[u8]) -> Vec<u8> {
    look_text(false, b"", theme, b"", clean_lang, clean_zone)
}

/// The words of a stock box, and with CLEAN those of a clean one (a stock box with the run-to-run
/// freezes): one list, so the two cannot drift apart.
fn look_text(
    clean: bool,
    keep: &[u8],
    theme: &[u8],
    net: &[u8],
    clean_lang: &[u8],
    clean_zone: &[u8],
) -> Vec<u8> {
    let has = |w| clean_has(keep, w);
    let mut parts: Vec<Vec<u8>> = Vec::new();
    parts.push(if has("bar") {
        b"your bar".to_vec()
    } else {
        b"Omarchy's bar".to_vec()
    });
    // (a stock box's wallpaper is the theme's own, never one of yours for it; a clean one's too with
    // --keep wallpaper)
    if clean && !has("wallpaper") {
        parts.push(cat!("theme ", theme));
        parts.push(cat!(
            "wallpaper ",
            CLEAN_WALLPAPER,
            " (any of it inside a window or the bar is a hole)"
        ));
    } else {
        parts.push(cat!("theme ", theme, " with its own wallpaper"));
    }
    if clean && !has("clock") {
        parts.push(cat!("the clock at ", CLEAN_CLOCK.replace('\'', "")));
    }
    if !has("locale") {
        let lang = if clean_lang.is_empty() {
            clean_locale().as_bytes().to_vec()
        } else {
            clean_lang.to_vec()
        };
        let zone = if clean_zone.is_empty() {
            CLEAN_TZ.as_bytes()
        } else {
            clean_zone
        };
        parts.push(cat!("locale ", lang, " in ", zone));
    }
    parts.push(if has("fonts") {
        cat!("none of your look but your fonts (", STOCK_LEFT_OUT, ")")
    } else {
        cat!("none of your look (", STOCK_LEFT_OUT, ", fonts)")
    });
    if clean && !has("pointer") {
        parts.push(b"no pointer".to_vec());
    }
    if net == b"isolated" {
        parts.push(b"no network".to_vec());
    }
    if clean && !has("animations") {
        parts.push(b"animations off".to_vec());
    }
    parts.join(b", ".as_slice())
}

/// box_starter (#220): who started this box, for `ls` to tell an orphan, as "PID START COMM": the
/// nearest ancestor that is not a shell running a command line (`bash -c`, a script's subshell: the
/// same command line as its parent's), timeout, flock or time, which end with their command. None
/// when none is found below PID 1 or this pid namespace's first process.
pub fn box_starter() -> Option<Vec<u8>> {
    // SAFETY: getppid has no failure and no side effect.
    let mut p = unsafe { libc::getppid() }.to_string().into_bytes();
    while shell::is_num(&p) && shell::dec(&p).is_some_and(|n| n > 1) {
        let comm = procfs::read(&p, "comm")?;
        let pp = procfs::proc_ppid(&p)?;
        let mut skip = false;
        match comm.as_slice() {
            b"timeout" | b"flock" | b"time" => skip = true,
            b"sh" | b"bash" | b"dash" | b"zsh" | b"ksh" | b"mksh" | b"fish" | b"busybox" => {
                let args =
                    std::fs::read(shell::path(&cat!("/proc/", p, "/cmdline"))).unwrap_or_default();
                let mut args: Vec<&[u8]> = args.split(|&c| c == 0).collect();
                // (mapfile -d '': no empty field after the last NUL)
                if args.last().is_some_and(|a| a.is_empty()) {
                    args.pop();
                }
                for a in args.iter().skip(1) {
                    if *a == b"--command" || a.starts_with(b"--command=") {
                        skip = true;
                        break;
                    } else if a.starts_with(b"--") {
                    } else if a.starts_with(b"-") && a.contains(&b'c') {
                        skip = true;
                        break;
                    } else if a.starts_with(b"-") {
                    } else {
                        break;
                    }
                }
            }
            _ => {}
        }
        if !skip && shell::dec(&pp).is_some_and(|n| n > 0) {
            let a = std::fs::read(shell::path(&cat!("/proc/", p, "/cmdline")));
            let b = std::fs::read(shell::path(&cat!("/proc/", pp, "/cmdline")));
            if let (Ok(a), Ok(b)) = (a, b)
                && a == b
            {
                skip = true;
            }
        }
        if skip {
            p = pp;
            continue;
        }
        let start = procfs::proc_start(&p).filter(|s| !s.is_empty())?;
        return Some(cat!(p, " ", start, " ", comm));
    }
    None
}

/// up_session_note (#136): `up` outside any repo, while this session has a box from its repo: a
/// second box starts, but the first one is named.
pub fn up_session_note(ctx: &Ctx) {
    if !ctx.name.starts_with(b"default-") || boxes::box_state(&ctx.d) == "up" {
        return;
    }
    let mine = crate::select::session_boxes(ctx);
    if !mine.is_empty() {
        say(cat!(
            "note: this session already has box ",
            mine.join(&b' '),
            " (started from another directory); -b NAME to use it"
        ));
    }
}

/// write_confirm_flag RUNDIR VALUE: the flag the box's Hyprland reads when its window closes.
pub fn write_confirm_flag(ctx: &Ctx, run: &[u8], value: &[u8]) {
    if !boxes::box_file(
        ctx,
        &shell::path(&cat!(run, "/omabox.confirm-close")),
        value,
    ) {
        let b = run.strip_suffix(b"/run").unwrap_or(run);
        say(cat!("could not update ", b, "'s confirm-close"));
    }
}

/// save_dir NAME: the save's dir (`omabox save`), or a refusal that says why.
pub fn save_dir(ctx: &Ctx, name: &[u8]) -> Res<Vec<u8>> {
    // (save names are paths: as tame as box names, but refused rather than changed to fit)
    if !re_match("^[A-Za-z0-9][A-Za-z0-9_.-]{0,39}$", name) {
        return Err(ctx.die(cat!(
            "bad save name: ",
            name,
            " (letters, digits, . _ -; 40 at most)"
        )));
    }
    let d = cat!(crate::paths::saves(), "/", name);
    if !shell::path(&cat!(d, "/home")).is_dir() {
        return Err(ctx.die(cat!("no save '", name, "' (omabox saves lists them)")));
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // clean_keep and clean_text (t_unit_check's four cases).
    #[test]
    fn desktops() {
        assert_eq!(Desktop::parse(b"omarchy"), Some(Desktop::Omarchy));
        assert_eq!(Desktop::parse(b"plain"), Some(Desktop::Plain));
        assert_eq!(Desktop::parse(b"desk"), Some(Desktop::Desk));
        for bad in [&b""[..], b"Plain", b"Desk", b"plain ", b"omarchy,plain"] {
            assert_eq!(Desktop::parse(bad), None, "{bad:?}");
        }
        assert!(!Desktop::Omarchy.plain_like());
        assert!(Desktop::Plain.plain_like());
        assert!(Desktop::Desk.plain_like());
        for d in [Desktop::Omarchy, Desktop::Plain, Desktop::Desk] {
            assert_eq!(Desktop::parse(d.name().as_bytes()), Some(d));
            assert_eq!(Desktop::of_box(d.name().as_bytes()), d);
            // --desktop wins over --omarchy, the config and the machine.
            for c in [None, Some(Desktop::Omarchy), Some(Desktop::Plain)] {
                for here in [false, true] {
                    assert_eq!(resolve_desktop(Some(d), false, c, here), (d, Chosen::Given));
                }
            }
            // The config's, wherever.
            for here in [false, true] {
                assert_eq!(
                    resolve_desktop(None, false, Some(d), here),
                    (d, Chosen::Config)
                );
            }
        }
        // A box from before #247 (no `desktop` in its box.json): Omarchy's.
        assert_eq!(Desktop::of_box(b""), Desktop::Omarchy);
        // None asked, no config (auto): by what the machine has.
        assert_eq!(
            resolve_desktop(None, false, None, true),
            (Desktop::Omarchy, Chosen::Machine)
        );
        assert_eq!(
            resolve_desktop(None, false, None, false),
            (Desktop::Plain, Chosen::Machine)
        );
        // --omarchy DIR is a tree of Omarchy's: an Omarchy box, over the config and the machine.
        for c in [None, Some(Desktop::Plain)] {
            assert_eq!(
                resolve_desktop(None, true, c, false),
                (Desktop::Omarchy, Chosen::Given)
            );
        }
        assert_eq!(conf_desktop(b"auto"), None);
        assert_eq!(conf_desktop(b"plain"), Some(Desktop::Plain));
        // Desk only by --desktop (#321), never the config's.
        assert_eq!(conf_desktop(b"desk"), None);
        // Said only for a plain box not asked for.
        assert!(chosen_text(Desktop::Plain, Chosen::Machine).is_some());
        assert!(chosen_text(Desktop::Plain, Chosen::Config).is_some());
        assert!(chosen_text(Desktop::Plain, Chosen::Given).is_none());
        assert!(chosen_text(Desktop::Omarchy, Chosen::Machine).is_none());
        assert!(chosen_text(Desktop::Omarchy, Chosen::Config).is_none());
        assert_eq!(DESKTOPS, "omarchy, plain or desk");
    }

    // desk_home (#321): the user's HOME as written, or why it cannot be a desk box's.
    #[test]
    fn desk_home_validation() {
        for (h, want) in [
            (&b"/home/u"[..], &b"/home/u"[..]),
            (b"/home/u/", b"/home/u"),
            (b"/home/u//", b"/home/u"),
            (b"/var/home/u", b"/var/home/u"),
            (b"/root", b"/root"),
            (b"/tmp/x/h", b"/tmp/x/h"),
            (b"/home/a b", b"/home/a b"),
        ] {
            assert_eq!(desk_home(h), Ok(want.to_vec()), "{h:?}");
        }
        for h in [
            &b""[..],
            b"/",
            b"//",
            b"home/u",
            b"~",
            b"./u",
            b"/home/./u",
            b"/home/../u",
            b"/home/u/..",
            b"/home//u",
            b"/home/u\n",
            b"/tmp",
            b"/tmp/",
            b"/tmp/.X11-unix/u",
            b"/usr/u",
            b"/etc",
            b"/proc/self",
            b"/dev/shm/u",
            b"/sys/u",
            b"/run/user/1000",
            b"/bin/u",
            b"/lib/u",
            b"/lib64/u",
            b"/opt/omabox/u",
            b"/opt",
            b"/home",
            b"/home/sbx",
            b"/home/sbx/u",
            b"/home/u\xff",
            b"/home/\xc3(",
        ] {
            let e = desk_home(h).expect_err(&String::from_utf8_lossy(h));
            assert!(
                e.starts_with(b"a desk box needs your HOME at its own path, and "),
                "{}",
                String::from_utf8_lossy(&e)
            );
        }
        // (a HOME that is not UTF-8 is said so: paths built from it are lossy in places, #321)
        assert!(crate::exit::contains(
            &desk_home(b"/home/u\xff").unwrap_err(),
            b"cannot be: it is not valid UTF-8"
        ));
        assert_eq!(
            desk_home("/home/jo\u{e3}o".as_bytes()),
            Ok("/home/jo\u{e3}o".as_bytes().to_vec())
        );
    }

    #[test]
    fn clean_as_bash() {
        assert_eq!(
            clean_keep(b"clock,animations,clock"),
            Ok(b"clock,animations".to_vec()),
            "--keep: comma-separated, once each"
        );
        let (r, said) = crate::exit::heard(|| clean_keep(b"clock,blur"));
        assert_eq!(r, Err(()));
        assert!(
            said.contains(
                "takes animations, bar, clock, fonts, identity, locale, pointer, wallpaper (comma-separated), got blur"
            ),
            "--keep: anything else refused, naming what it takes: {said}"
        );
        assert_eq!(
            String::from_utf8(clean_text(
                b"",
                b"tokyo-night",
                b"isolated",
                b"en_US.UTF-8",
                b"UTC",
                false
            ))
            .unwrap(),
            "Omarchy's bar, theme tokyo-night, wallpaper #ff00ff (any of it inside a window or the bar is a hole), the clock at Friday 12:00, locale en_US.UTF-8 in UTC, none of your look (terminal configs, branding, themes, wallpapers, the plugins' settings, Hyprland toggles, fonts), no pointer, no network, animations off, as user@omabox",
            "clean_text: all of it"
        );
        assert_eq!(
            String::from_utf8(clean_text(
                b"bar,wallpaper,clock,animations,locale,fonts,identity",
                b"nord",
                b"connected",
                b"",
                b"",
                false
            ))
            .unwrap(),
            "your bar, theme nord with its own wallpaper, none of your look but your fonts (terminal configs, branding, themes, wallpapers, the plugins' settings, Hyprland toggles), no pointer",
            "...what --keep takes back, and a network"
        );
        // up --stock over --from (#265, finding 638): the save's HOME stays, said.
        assert_eq!(
            String::from_utf8(stock_over_save_text(b"demo")).unwrap(),
            "...over save 'demo': its HOME as saved (its files, configs and their look); the bar, theme and locale above are stock",
            "stock_over_save_text"
        );
        // up --stock (#265, finding 613): the same words, less the freezes.
        assert_eq!(
            String::from_utf8(stock_text(b"tokyo-night", b"en_US.UTF-8", b"UTC")).unwrap(),
            "Omarchy's bar, theme tokyo-night with its own wallpaper, locale en_US.UTF-8 in UTC, none of your look (terminal configs, branding, themes, wallpapers, the plugins' settings, Hyprland toggles, fonts)",
            "stock_text"
        );
    }

    #[test]
    fn addresses() {
        let ok = |a: &str, want: &str| {
            assert_eq!(
                box_addr(a.as_bytes()),
                Some(want.as_bytes().to_vec()),
                "{a}"
            )
        };
        let no = |a: &str| assert_eq!(box_addr(a.as_bytes()), None, "{a}");
        ok("203.0.113.010", "203.0.113.10");
        ok("192.0.2.0", "192.0.2.0");
        ok("198.51.100.255", "198.51.100.255");
        ok("192.000.002.001", "192.0.2.1");
        no("198.51.100.256");
        no("10.0.0.1");
        no("192.0.3.1");
        no("192.0.2");
        no("192.0.2.1/24");
        no("1.2.3.4.5");
        ok("2001:DB8::1", "2001:db8::1");
        ok("2001:0db8:0000:0000:0000:0000:0000:0001", "2001:db8::1");
        ok("2001:db8:0:0:1:0:0:1", "2001:db8::1:0:0:1");
        ok("2001:db8:0:1:0:0:0:1", "2001:db8:0:1::1");
        ok("2001:db8:1:1:1:1:0:1", "2001:db8:1:1:1:1:0:1");
        ok("2001:db8::", "2001:db8::");
        ok("2001:db8::0:1", "2001:db8::1");
        // (bash's read -ra drops a trailing empty field after ::, so it takes this)
        ok("2001:db8::1:", "2001:db8::1");
        no("2001:db9::1");
        no("2001:db8:::1");
        no("2001:db8::1::2");
        no("2001:db8:1:2:3:4:5:6:7");
        no("2001:db8:1:2:3:4:5");
        no(":2001:db8::1");
        no("2001:db8:1:2:3:4:5:6:");
        no("2001:db8::10000");
        no("2001:db8::g");
        no("::");
        no("");
    }

    proptest! {
        // Every address in 2001:db8::/32 comes out as RFC 5952 writes it (std's Display is that).
        #[test]
        fn v6_as_rfc5952(rest in proptest::array::uniform6(prop_oneof![Just(0u16), any::<u16>()]), upper in any::<bool>()) {
            let g = [0x2001, 0xdb8, rest[0], rest[1], rest[2], rest[3], rest[4], rest[5]];
            let a = std::net::Ipv6Addr::new(g[0], g[1], g[2], g[3], g[4], g[5], g[6], g[7]);
            let full = g.iter().map(|x| format!("{x:04x}")).collect::<Vec<_>>().join(":");
            let full = if upper { full.to_uppercase() } else { full };
            prop_assert_eq!(box_addr(full.as_bytes()), Some(a.to_string().into_bytes()));
            prop_assert_eq!(box_addr(a.to_string().as_bytes()), Some(a.to_string().into_bytes()));
        }

        #[test]
        fn v4_ranges(a in any::<[u8; 4]>(), pad in any::<bool>()) {
            let s = if pad { format!("{:03}.{:03}.{:03}.{:03}", a[0], a[1], a[2], a[3]) } else { format!("{}.{}.{}.{}", a[0], a[1], a[2], a[3]) };
            let plain = format!("{}.{}.{}.{}", a[0], a[1], a[2], a[3]);
            let doc = matches!(a, [192, 0, 2, _] | [198, 51, 100, _] | [203, 0, 113, _]);
            prop_assert_eq!(box_addr(s.as_bytes()), doc.then(|| plain.into_bytes()));
        }

        // S5: what pasta forwards is exactly the ports given, each once, in order, 1-65535.
        #[test]
        fn allowed_ports(ps in proptest::collection::vec(1u16..=65535, 1..8)) {
            let list = ps.iter().map(u16::to_string).collect::<Vec<_>>().join(",");
            let a = AllowedPorts::parse(list.as_bytes()).unwrap();
            let mut want = ps.clone();
            want.sort_unstable();
            want.dedup();
            prop_assert_eq!(a.ports(), want.as_slice());
        }

        // S3: nothing is mounted at or under the box's own system, runtime dir, omabox's mounts or
        // HOME, nor over /tmp, its socket dirs, or any folder above /home/sbx, /opt/omabox or /tmp;
        // for either HOME a box has (#321): /home/sbx, or a desk box's at the user's HOME path.
        #[test]
        fn dest_refused(top in prop_oneof![
                Just("/usr"), Just("/etc"), Just("/proc"), Just("/dev"), Just("/sys"), Just("/run"),
                Just("/bin"), Just("/lib"), Just("/lib64"), Just("/opt/omabox"), Just("/home/sbx"),
                Just("/tmp/.X11-unix"), Just("/tmp/.ICE-unix")],
            rest in proptest::collection::vec("[a-zA-Z0-9._-]{1,8}", 0..4), slash in any::<bool>(),
            home in prop_oneof![Just(SBX), Just(DESK), Just(DESK_TMP)]) {
            let mut p = top.to_string();
            for r in &rest { p.push('/'); p.push_str(r); }
            if slash { p.push('/'); }
            prop_assert!(refuse_dest(p.as_bytes(), home.as_bytes()).is_some(), "{}", p);
        }

        #[test]
        fn dest_above_refused(i in 0usize..6, cut in 0usize..12,
            home in prop_oneof![Just(SBX), Just(DESK), Just(DESK_TMP)]) {
            let x = ["/home/sbx", "/opt/omabox", "/tmp", home, home, home][i];
            let n = x.match_indices('/').count();
            let k = cut % (n + 1);
            let above: String = x.split('/').take(k + 1).collect::<Vec<_>>().join("/");
            let above = if above.is_empty() { "/".to_string() } else { above };
            prop_assert!(refuse_dest(above.as_bytes(), home.as_bytes()).is_some(), "{}", above);
            let slash = format!("{}/", above.trim_end_matches('/'));
            prop_assert!(refuse_dest(slash.as_bytes(), home.as_bytes()).is_some(), "{}", slash);
        }

        #[test]
        fn dest_elsewhere_allowed(rest in proptest::collection::vec("[a-z]{1,8}", 1..4),
            home in prop_oneof![Just(SBX), Just(DESK), Just(DESK_TMP)]) {
            let p = format!("/mnt/{}", rest.join("/"));
            prop_assert!(refuse_dest(p.as_bytes(), home.as_bytes()).is_none());
            let t = format!("/srv/{}", rest.join("/"));
            prop_assert!(refuse_dest(t.as_bytes(), home.as_bytes()).is_none());
            if home != DESK_TMP {
                let t = format!("/tmp/{}", rest.join("/"));
                prop_assert!(refuse_dest(t.as_bytes(), home.as_bytes()).is_none());
            }
        }

        // #321: inside a desk box's HOME, anything (the repo, --ro-bind's) but omabox's own files
        // there and what holds them; inside /home/sbx, nothing.
        #[test]
        fn dest_in_home(rest in proptest::collection::vec("[a-z][a-z0-9._-]{0,7}", 1..4),
            own in 0usize..OWN_IN_HOME.len(), below in proptest::collection::vec("[a-z]{1,8}", 0..3),
            home in prop_oneof![Just(DESK), Just(DESK_TMP)]) {
            let p = format!("{home}/{}", rest.join("/"));
            let mine = OWN_IN_HOME.iter().any(|o| {
                let o = o.replace("*", "x");
                format!("{o}/").starts_with(&format!("{}/", rest.join("/")))
            });
            let run_log = rest[0].starts_with("run-") && rest[0].ends_with(".log");
            let ev = matches!(rest[0].as_str(), "events.log" | "gdb.log");
            if !mine && !run_log && !ev {
                prop_assert!(refuse_dest(p.as_bytes(), home.as_bytes()).is_none(), "{}", p);
            }
            prop_assert!(refuse_dest(p.as_bytes(), SBX.as_bytes()).is_none(), "{}", p);
            let s = format!("/home/sbx/{}", rest.join("/"));
            prop_assert!(refuse_dest(s.as_bytes(), SBX.as_bytes()).is_some(), "{}", s);
            prop_assert!(refuse_dest(s.as_bytes(), home.as_bytes()).is_some(), "{}", s);
            // omabox's own files in a desk HOME, at, under and above them.
            let o = OWN_IN_HOME[own].replace("*", "7");
            let mut at = format!("{home}/{o}");
            prop_assert!(refuse_dest(at.as_bytes(), home.as_bytes()).is_some(), "{}", at);
            for b in &below { at.push('/'); at.push_str(b); }
            prop_assert!(refuse_dest(at.as_bytes(), home.as_bytes()).is_some(), "{}", at);
            if let Some((up, _)) = o.rsplit_once('/') {
                let above = format!("{home}/{up}");
                prop_assert!(refuse_dest(above.as_bytes(), home.as_bytes()).is_some(), "{}", above);
            }
        }
    }

    const SBX: &str = crate::boxes::SBX_HOME;
    const DESK: &str = "/home/u";
    const DESK_TMP: &str = "/tmp/x/h";

    #[test]
    fn dests() {
        let sbx = SBX.as_bytes();
        assert_eq!(
            refuse_dest(b"/home", sbx),
            Some(b"it would hide /home/sbx/".to_vec())
        );
        assert_eq!(
            refuse_dest(b"/opt", sbx),
            Some(b"it would hide /opt/omabox/".to_vec())
        );
        assert!(refuse_dest(b"/tmp", sbx).is_some());
        assert!(refuse_dest(b"/", sbx).is_some());
        assert!(refuse_dest(b"/tmp/repo", sbx).is_none());
        assert!(refuse_dest(b"/mnt/nas", sbx).is_none());
        assert!(refuse_dest(b"/usrx", sbx).is_none());
        // A desk box's HOME (#321).
        let h = DESK.as_bytes();
        assert_eq!(
            refuse_dest(b"/home/u", h),
            Some(b"that is the box's HOME (/home/u) or above it".to_vec())
        );
        assert_eq!(
            refuse_dest(b"/var/home/u/", b"/var/home/u"),
            Some(b"that is the box's HOME (/var/home/u) or above it".to_vec())
        );
        assert!(refuse_dest(b"/var/home", b"/var/home/u").is_some());
        assert!(refuse_dest(b"/var", b"/var/home/u").is_some());
        assert!(refuse_dest(b"/home/u/code/omabox", h).is_none());
        assert!(refuse_dest(b"/home/u/.local/share/fonts", h).is_none());
        assert!(refuse_dest(b"/home/u/.configx", h).is_none());
        assert!(refuse_dest(b"/home/u/events.logs", h).is_none());
        assert!(refuse_dest(b"/home/u/run.log", h).is_none());
        assert!(refuse_dest(b"/home/uu", h).is_none());
        assert_eq!(
            refuse_dest(b"/home/u/.config/omabox/x", h),
            Some(b"that is omabox's own ~/.config/omabox in the box HOME".to_vec())
        );
        assert_eq!(
            refuse_dest(b"/home/u/.config", h),
            Some(b"it would hide omabox's own ~/.config/omabox in the box HOME".to_vec())
        );
        for p in [
            "/home/u/events.log",
            "/home/u/gdb.log",
            "/home/u/run-3.log",
            "/home/u/run-3.log/",
            "/home/u/.cache",
            "/home/u/.cache/omabox-record/x",
            "/home/u/.config/omarchy",
            "/home/u/.config/omarchy/plugins/p",
        ] {
            assert!(refuse_dest(p.as_bytes(), h).is_some(), "{p}");
        }
        // The desk HOME's own path in another box is no HOME of that box's.
        assert!(refuse_dest(b"/home/u", sbx).is_none());
    }

    /// t_unit_refusals's allow_ports and box_addr (--addr, #229) cases, as bash had them.
    #[test]
    fn refusals_as_bash() {
        let csv = |l: &str| AllowedPorts::parse(l.as_bytes()).unwrap().csv();
        assert_eq!(csv("8082,22,8081,08081"), b"22,8081,8082");
        assert_eq!(csv(""), b"");
        let a = |x: &str| box_addr(x.as_bytes()).map(|v| String::from_utf8(v).unwrap());
        assert_eq!(a("203.0.113.010").as_deref(), Some("203.0.113.10"));
        assert_eq!(a("192.0.2.1").as_deref(), Some("192.0.2.1"));
        assert_eq!(a("198.51.100.255").as_deref(), Some("198.51.100.255"));
        assert_eq!(
            a("2001:DB8:0:0:1:0:0:1").as_deref(),
            Some("2001:db8::1:0:0:1")
        );
        assert_eq!(
            a("2001:0db8:0:1:0:0:0:a").as_deref(),
            Some("2001:db8:0:1::a")
        );
        for bad in [
            "10.0.0.1",
            "127.0.0.2",
            "203.0.114.1",
            "203.0.113.256",
            "203.0.113.10/32",
            "2001:db9::1",
            "::1",
            "2001:db8::1::2",
            "2001:db8:1:2:3:4:5",
            "2001:db8:",
            "",
        ] {
            assert_eq!(a(bad), None, "{bad}");
        }
    }

    #[test]
    fn ports() {
        assert_eq!(
            AllowedPorts::parse(b"8082,8081,8081").unwrap().csv(),
            b"8081,8082"
        );
        assert_eq!(AllowedPorts::parse(b"08081").unwrap().csv(), b"8081");
        assert_eq!(AllowedPorts::parse(b"8081,").unwrap().csv(), b"8081");
        assert!(AllowedPorts::parse(b"").unwrap().is_empty());
        assert!(AllowedPorts::parse(b"0").is_err());
        assert!(AllowedPorts::parse(b"65536").is_err());
        assert!(AllowedPorts::parse(b"123456").is_err());
        assert!(AllowedPorts::parse(b"80,,81").is_err());
        assert!(AllowedPorts::parse(b",").is_err());
    }

    #[test]
    fn clean_ids_renames_only_me() {
        let pw = b"root:x:0:0::/root:/bin/bash\nme:x:1000:1000:Me Myself:/home/me:/bin/bash\nmeh:x:1001:1001::/home/meh:/bin/sh\n";
        assert_eq!(
            clean_ids(pw, b"me", true, SBX.as_bytes()),
            b"root:x:0:0::/root:/bin/bash\nuser:x:1000:1000::/home/sbx:/bin/bash\nmeh:x:1001:1001::/home/meh:/bin/sh\n".to_vec()
        );
        let gr = b"wheel:x:998:me,meh\nme:x:1000:\nvideo:x:985:meh,me";
        assert_eq!(
            clean_ids(gr, b"me", false, SBX.as_bytes()),
            b"wheel:x:998:user,meh\nuser:x:1000:\nvideo:x:985:meh,user".to_vec()
        );
        // No name to take (no passwd entry for the uid): as it was.
        assert_eq!(clean_ids(pw, b"", true, SBX.as_bytes()), pw.to_vec());
        // A comment or short line is left alone.
        assert_eq!(
            clean_ids(b"# me:x\n", b"me", true, SBX.as_bytes()),
            b"# me:x\n".to_vec()
        );
        // The box's HOME, whichever it is (#321).
        assert!(
            clean_ids(pw, b"me", true, b"/home/u")
                .windows(23)
                .any(|w| w == b"1000:1000::/home/u:/bin")
        );
    }

    #[test]
    fn lists() {
        assert_eq!(
            no_seed_spec(b"fonts,extensions,fonts", b"fonts").unwrap(),
            b"fonts,extensions"
        );
        assert_eq!(no_seed_spec(b",,", b"").unwrap(), b"");
        assert!(no_seed_spec(b"font", b"").is_err());
        assert_eq!(clean_keep(b"bar,clock,bar").unwrap(), b"bar,clock");
        assert!(clean_keep(b"").is_err());
        assert!(clean_keep(b"bar, clock").is_err());
        assert!(clean_has(b"bar,clock", "clock"));
        assert!(!clean_has(b"bar,clock", "clo"));
    }

    #[test]
    fn clean_words() {
        let t = clean_text(
            b"fonts,pointer,animations",
            b"tokyo-night",
            b"isolated",
            b"C.UTF-8",
            b"UTC",
            false,
        );
        assert_eq!(
            String::from_utf8(t).unwrap(),
            "Omarchy's bar, theme tokyo-night, wallpaper #ff00ff (any of it inside a window or the bar is a hole), the clock at Friday 12:00, locale C.UTF-8 in UTC, none of your look but your fonts (terminal configs, branding, themes, wallpapers, the plugins' settings, Hyprland toggles), no network, as user@omabox"
        );
        let t = clean_text(
            b"bar,wallpaper,clock,locale,fonts,pointer,animations,identity",
            b"x",
            b"connected",
            b"",
            b"",
            false,
        );
        assert_eq!(
            String::from_utf8(t).unwrap(),
            "your bar, theme x with its own wallpaper, none of your look but your fonts (terminal configs, branding, themes, wallpapers, the plugins' settings, Hyprland toggles)"
        );
        // A plain box (#247): its background the sentinel, no bar, theme or clock of Omarchy's.
        let t = clean_text(b"", b"", b"isolated", b"C.UTF-8", b"UTC", true);
        assert_eq!(
            String::from_utf8(t).unwrap(),
            "background #ff00ff (any of it inside a window is a hole), locale C.UTF-8 in UTC, none of your fonts, no pointer, no network, animations off, as user@omabox"
        );
        let t = clean_text(
            b"wallpaper,locale,fonts,pointer,animations,identity",
            b"",
            b"connected",
            b"",
            b"",
            true,
        );
        assert_eq!(t, b"");
        assert_eq!(
            clean_zone_rec(
                &[
                    (b"TZ".to_vec(), b"Europe/Lisbon".to_vec()),
                    (b"TZ".to_vec(), Vec::new())
                ],
                b"UTC"
            ),
            b"UTC"
        );
        assert_eq!(
            clean_zone_rec(&[(b"TZ".to_vec(), b"Europe/Lisbon".to_vec())], b"UTC"),
            b"Europe/Lisbon"
        );
        assert_eq!(clean_locale_rec(b"en_US.UTF-8"), b"en_US.UTF-8");
    }

    #[test]
    fn paths_resolved() {
        let d = std::env::temp_dir().join(format!("omabox-us-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("a/b")).unwrap();
        std::os::unix::fs::symlink(d.join("a/b"), d.join("l")).unwrap();
        std::os::unix::fs::symlink("loop2", d.join("loop1")).unwrap();
        std::os::unix::fs::symlink("loop1", d.join("loop2")).unwrap();
        let db = d.to_str().unwrap().as_bytes().to_vec();
        assert_eq!(
            realpath_m(b"//usr/../usr/./lib"),
            Some(b"/usr/lib".to_vec())
        );
        assert_eq!(realpath_m(&cat!(db, "/l/../x/y")), Some(cat!(db, "/a/x/y")));
        assert_eq!(realpath_m(&cat!(db, "/none/../a")), Some(cat!(db, "/a")));
        assert_eq!(realpath_m(&cat!(db, "/loop1")), None);
        assert_eq!(realpath_m(b"/"), Some(b"/".to_vec()));
        assert_eq!(
            robind_spec(&cat!(db, "/l:/mnt//x/../nas")),
            Some((cat!(db, "/a/b"), b"/mnt/nas".to_vec()))
        );
        assert_eq!(
            robind_spec(&cat!(db, "/l")),
            Some((cat!(db, "/a/b"), cat!(db, "/a/b")))
        );
        assert_eq!(robind_spec(&cat!(db, "/none")), None);
        assert_eq!(
            seed_spec(&cat!(db, "/l:~/.config/x"), SBX.as_bytes()),
            Ok((cat!(db, "/a/b"), b".config/x".to_vec()))
        );
        assert_eq!(
            seed_spec(&cat!(db, "/l:/home/sbx/y/../z"), SBX.as_bytes()),
            Ok((cat!(db, "/a/b"), b"z".to_vec()))
        );
        assert_eq!(
            seed_spec(&cat!(db, "/l:../x"), SBX.as_bytes()),
            Err(SeedRefused::Bad)
        );
        assert_eq!(
            seed_spec(&cat!(db, "/l:~/"), SBX.as_bytes()),
            Err(SeedRefused::Bad)
        );
        assert_eq!(
            seed_spec(&cat!(db, "/l:/etc/x"), SBX.as_bytes()),
            Err(SeedRefused::Bad)
        );
        assert_eq!(
            seed_spec(&cat!(db, "/l"), SBX.as_bytes()),
            Err(SeedRefused::Bad)
        );
        assert_eq!(
            seed_spec(&cat!(db, "/none:x"), SBX.as_bytes()),
            Err(SeedRefused::Bad)
        );
        // A desk box's HOME (#321): DEST under it, as under /home/sbx in any other box; /home/sbx
        // is no HOME of a desk box's, nor the desk HOME's path one of another box's.
        assert_eq!(
            seed_spec(&cat!(db, "/l:/home/u/Pictures"), DESK.as_bytes()),
            Ok((cat!(db, "/a/b"), b"Pictures".to_vec()))
        );
        assert_eq!(
            seed_spec(&cat!(db, "/l:~/Pictures"), DESK.as_bytes()),
            Ok((cat!(db, "/a/b"), b"Pictures".to_vec()))
        );
        assert_eq!(
            seed_spec(&cat!(db, "/l:/home/sbx/x"), DESK.as_bytes()),
            Err(SeedRefused::Bad)
        );
        assert_eq!(
            seed_spec(&cat!(db, "/l:/home/u/x"), SBX.as_bytes()),
            Err(SeedRefused::Bad)
        );
        assert_eq!(
            seed_spec(&cat!(db, "/l:/home/u/"), DESK.as_bytes()),
            Err(SeedRefused::Bad)
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn starter_is_an_ancestor() {
        if let Some(s) = box_starter() {
            let f: Vec<&[u8]> = s.splitn(3, |&c| c == b' ').collect();
            assert!(
                f.len() == 3 && shell::is_num(f[0]) && shell::is_num(f[1]),
                "{s:?}"
            );
        }
    }
}
