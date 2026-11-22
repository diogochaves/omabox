//! Where omabox keeps things, as bin/omabox's globals name them (#324 wave 5a): box HOMEs, saves,
//! the tools in the checkout and where a box sees them.

use crate::cat;
use crate::shell;

/// `${VAR:-DEFAULT}`.
fn env_or(name: &str, default: Vec<u8>) -> Vec<u8> {
    shell::env(name)
        .filter(|v| !v.is_empty())
        .unwrap_or(default)
}

/// HOMES: box HOMEs, on disk rather than the runtime dir's tmpfs (finding 50).
pub fn homes() -> Vec<u8> {
    let h = shell::env_or_empty("HOME");
    cat!(env_or("XDG_CACHE_HOME", cat!(h, "/.cache")), "/omabox")
}

/// SAVES: box HOMEs kept on purpose (`omabox save`).
pub fn saves() -> Vec<u8> {
    let h = shell::env_or_empty("HOME");
    cat!(
        env_or("XDG_DATA_HOME", cat!(h, "/.local/share")),
        "/omabox/saves"
    )
}

/// RECORDS: the videos of a jailed agent's recordings stopped outside the jail, beside SAVES: never
/// in HOMES, where sweep_homes took a folder for a gone box's HOME (#433, finding 684).
pub fn records() -> Vec<u8> {
    let h = shell::env_or_empty("HOME");
    cat!(
        env_or("XDG_DATA_HOME", cat!(h, "/.local/share")),
        "/omabox/records"
    )
}

/// AQ_USER: the user's private aquamarine build.
pub fn aq_user() -> Vec<u8> {
    let h = shell::env_or_empty("HOME");
    cat!(
        env_or("XDG_DATA_HOME", cat!(h, "/.local/share")),
        "/omabox/aquamarine"
    )
}

/// A tool of the checkout's: `$ROOT/tools/NAME/omabox-NAME` (KEYBOARD, POINTER, STILL, ...).
pub fn tool(name: &str) -> Vec<u8> {
    let root = crate::root();
    cat!(shell::bytes(&root), "/tools/", name, "/omabox-", name)
}

/// `up --polkit`'s session shim (#258), preloaded in the box: `$ROOT/tools/polkit/omabox-polkit-session.so`.
pub fn polkit_session() -> Vec<u8> {
    cat!(tool("polkit"), "-session.so")
}

/// The checkout's root as bytes ($ROOT).
pub fn root() -> Vec<u8> {
    shell::bytes(&crate::root()).to_vec()
}

/// READY_TIMEOUT as given: `${OMABOX_READY_TIMEOUT:-30}`.
pub fn ready_timeout_raw() -> Vec<u8> {
    env_or("OMABOX_READY_TIMEOUT", b"30".to_vec())
}

/// DIR without its trailing slashes: `/a/` is `/a`; `/` (or `//`) is empty, standing for the root.
fn trim_end_slashes(dir: &[u8]) -> &[u8] {
    let end = dir.iter().rposition(|&c| c != b'/').map_or(0, |i| i + 1);
    &dir[..end]
}

/// P is DIR or below it: the rest of P after DIR, without the slashes between (`Some(b"")` when P is
/// DIR), else None. Part by part (`/ab` is not under `/a`) and lexical, by these rules:
/// - an empty DIR holds nothing (never "everything is under");
/// - trailing slashes on DIR are ignored (`/a/` is `/a`), and `/` holds every absolute P, `/` itself
///   included; a relative P is never under `/`;
/// - trailing slashes on P too: `/a/` is at `/a` (rest empty), and `/a//b` is under `/a` (rest `b`);
/// - nothing is normalised: `/a/../b` is under `/a` (rest `../b`), `/a/./b` too, and `/a//b` is not
///   under `/a/b`. A caller that must not be fooled by those resolves both first (readlink_e,
///   bwrap::lexical).
pub fn under<'a>(p: &'a [u8], dir: &[u8]) -> Option<&'a [u8]> {
    if dir.is_empty() {
        return None;
    }
    let d = trim_end_slashes(dir);
    let rest = p.strip_prefix(d)?;
    if !rest.starts_with(b"/") && !(rest.is_empty() && !d.is_empty()) {
        return None;
    }
    let skip = rest.iter().position(|&c| c != b'/').unwrap_or(rest.len());
    Some(&rest[skip..])
}

/// P is DIR or below it (`[ "$p" = "$d" ] || [[ $p == "$d"/* ]]`), by `under`'s rules.
pub fn at_or_under(p: &[u8], dir: &[u8]) -> bool {
    under(p, dir).is_some()
}

/// P is below DIR, not DIR itself (`[[ $p == "$d"/* ]]`), by `under`'s rules: `/a/` is not below `/a`.
pub fn below(p: &[u8], dir: &[u8]) -> bool {
    under(p, dir).is_some_and(|r| !r.is_empty())
}

/// DIR with HOME written `~` (`~`, `~/rest`), as messages show a path; DIR as it is when HOME is
/// empty or `/` (a `~` there would say nothing).
pub fn home_short(dir: &[u8], home: &[u8]) -> Vec<u8> {
    if trim_end_slashes(home).is_empty() {
        return dir.to_vec();
    }
    match under(dir, home) {
        Some([]) => b"~".to_vec(),
        Some(rest) => cat!("~/", rest),
        None => dir.to_vec(),
    }
}

/// `${P/#\~/$HOME}`: a leading `~` (only the `~`: `~user` too, as bash's substitution has it) as
/// HOME.
pub fn tilde(p: &[u8]) -> Vec<u8> {
    match p.strip_prefix(b"~") {
        Some(rest) => cat!(shell::env_or_empty("HOME"), rest),
        None => p.to_vec(),
    }
}

/// `$(dirname -- "$P")`, as coreutils has it: the folder P is in, trailing slashes first dropped;
/// `/` for `/x`, `/` and `//x`, `.` for `x`, `x/` and an empty P. (Not `${P%/*}`, which gives `""`
/// for `/x` and `x` for `x`.)
pub fn dir_of(p: &[u8]) -> Vec<u8> {
    if p.is_empty() {
        return b".".to_vec();
    }
    let mut end = p.len();
    while end > 1 && p[end - 1] == b'/' {
        end -= 1;
    }
    match p[..end].iter().rposition(|&c| c == b'/') {
        None => b".".to_vec(),
        Some(i) => {
            let mut j = i;
            while j > 0 && p[j - 1] == b'/' {
                j -= 1;
            }
            if j == 0 {
                b"/".to_vec()
            } else {
                p[..j].to_vec()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn under_rules() {
        let u = |p: &str, d: &str| under(p.as_bytes(), d.as_bytes()).map(<[u8]>::to_vec);
        let s = |x: &str| Some(x.as_bytes().to_vec());
        // part by part
        assert_eq!(u("/a/b", "/a"), s("b"));
        assert_eq!(u("/a", "/a"), s(""));
        assert_eq!(u("/ab", "/a"), None);
        assert_eq!(u("/a", "/a/b"), None);
        // an empty DIR holds nothing
        assert_eq!(u("/a", ""), None);
        assert_eq!(u("", ""), None);
        assert_eq!(u("a", ""), None);
        // `/` holds every absolute path, itself too, and no relative one
        assert_eq!(u("/", "/"), s(""));
        assert_eq!(u("/x", "/"), s("x"));
        assert_eq!(u("//x", "/"), s("x"));
        assert_eq!(u("/x", "//"), s("x"));
        assert_eq!(u("x", "/"), None);
        assert_eq!(u("", "/"), None);
        // trailing slashes, DIR's and P's
        assert_eq!(u("/a/b", "/a/"), s("b"));
        assert_eq!(u("/a", "/a/"), s(""));
        assert_eq!(u("/a/", "/a"), s(""));
        assert_eq!(u("/a//b", "/a"), s("b"));
        assert_eq!(u("/a//b", "/a/b"), None);
        // nothing normalised
        assert_eq!(u("/a/../b", "/a"), s("../b"));
        assert_eq!(u("/a/./b", "/a"), s("./b"));
        assert_eq!(u("/b", "/a/.."), None);
        // relative paths, as written
        assert_eq!(u("a/b", "a"), s("b"));
        assert_eq!(u("/a/b", "a"), None);
        assert!(below(b"/a/b", b"/a") && !below(b"/a", b"/a") && !below(b"/a/", b"/a"));
        assert!(at_or_under(b"/a", b"/a") && !at_or_under(b"/a", b""));
    }

    #[test]
    fn tilde_as_bash() {
        shell::with_env(&[("HOME", Some("/home/u"))], || {
            assert_eq!(tilde(b"~"), b"/home/u");
            assert_eq!(tilde(b"~/x"), b"/home/u/x");
            assert_eq!(tilde(b"~x"), b"/home/ux");
            assert_eq!(tilde(b"/a/~"), b"/a/~");
            assert_eq!(tilde(b""), b"");
        });
    }

    /// dir_of is coreutils' dirname: the rows checked by hand, and against `dirname` itself where it
    /// is installed.
    #[test]
    fn dir_of_as_coreutils() {
        let rows = [
            ("/x", "/"),
            ("/", "/"),
            ("x", "."),
            ("x/", "."),
            ("//x", "/"),
            ("", "."),
            ("//", "/"),
            ("/a/", "/"),
            ("/a/b", "/a"),
            ("/a/b/", "/a"),
            ("/a//b", "/a"),
            ("//a/b", "//a"),
            ("a//b///", "a"),
            ("///a", "/"),
            ("..", "."),
            ("a/..", "a"),
        ];
        let coreutils = std::path::Path::new("/usr/bin/dirname").exists();
        for (p, d) in rows {
            assert_eq!(dir_of(p.as_bytes()), d.as_bytes(), "{p:?}");
            if coreutils {
                let out = std::process::Command::new("/usr/bin/dirname")
                    .arg("--")
                    .arg(p)
                    .output()
                    .unwrap();
                assert_eq!(
                    out.stdout,
                    format!("{d}\n").as_bytes(),
                    "coreutils on {p:?}"
                );
            }
        }
    }

    #[test]
    fn home_short_rules() {
        let h = |d: &str, home: &str| {
            String::from_utf8(home_short(d.as_bytes(), home.as_bytes())).unwrap()
        };
        assert_eq!(h("/home/u", "/home/u"), "~");
        assert_eq!(h("/home/u/x/y", "/home/u"), "~/x/y");
        assert_eq!(h("/home/u/x", "/home/u/"), "~/x");
        assert_eq!(h("/home/ux", "/home/u"), "/home/ux");
        assert_eq!(h("/x", ""), "/x");
        assert_eq!(h("/x", "/"), "/x");
        assert_eq!(h("/", "/"), "/");
    }

    /// The parts of P as a list, for the oracle below: P made of plain names and slashes only.
    fn names(p: &[u8]) -> Vec<&[u8]> {
        p.split(|&c| c == b'/').filter(|c| !c.is_empty()).collect()
    }

    fn path() -> impl Strategy<Value = Vec<u8>> {
        // names that collide (a, ab), `.` and `..`, and slashes in runs
        prop::collection::vec(
            prop_oneof![
                Just(b"a".to_vec()),
                Just(b"ab".to_vec()),
                Just(b"b".to_vec()),
                Just(b".".to_vec()),
                Just(b"..".to_vec()),
                Just(b"/".to_vec()),
                Just(b"//".to_vec()),
            ],
            0..7,
        )
        .prop_map(|v| v.concat())
    }

    proptest! {
        #[test]
        fn under_holds_its_rules(p in path(), d in path()) {
            let r = under(&p, &d);
            // an empty DIR holds nothing
            if d.is_empty() {
                prop_assert!(r.is_none());
            }
            // a trailing slash on DIR changes nothing
            if !d.is_empty() {
                prop_assert_eq!(r, under(&p, &cat!(d, "/")));
            }
            // the rest never starts with a slash, and P is DIR (trimmed), slashes, then the rest
            if let Some(rest) = r {
                prop_assert!(!rest.starts_with(b"/"));
                let t = trim_end_slashes(&d);
                prop_assert!(p.starts_with(t) && p.ends_with(rest));
                let mid = &p[t.len()..p.len() - rest.len()];
                prop_assert!(mid.iter().all(|&c| c == b'/'));
                prop_assert!(!mid.is_empty() || (rest.is_empty() && !t.is_empty()));
            }
            // `/` holds exactly the absolute paths
            if !d.is_empty() && trim_end_slashes(&d).is_empty() {
                prop_assert_eq!(r.is_some(), p.starts_with(b"/"));
            }
            // with no `.`, `..` or doubled slash, it is the part-by-part answer for absolute paths
            let plain = |x: &[u8]| {
                x.starts_with(b"/")
                    && !crate::exit::contains(x, b"//")
                    && names(x).iter().all(|n| *n != b"." && *n != b"..")
            };
            if plain(&p) && plain(&d) {
                prop_assert_eq!(r.is_some(), names(&p).starts_with(&names(&d)));
            }
            prop_assert_eq!(at_or_under(&p, &d), r.is_some());
            prop_assert_eq!(below(&p, &d), r.is_some_and(|x| !x.is_empty()));
        }
    }
}
