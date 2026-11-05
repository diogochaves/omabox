//! Exit codes and messages, one meaning in every command (#273; `omabox help exit-codes`): 1 a
//! command that ran and failed, 2 one called wrong (a usage error), 124 not in time, 125 run's own
//! failure. DIE_RC: the exit code of every refusal when set (2 under `check`, 125 under `run`).
//! PARSING: true while the command line is read (until `parsed` or select_box), when a refusal is a
//! usage error: exit 2, and where the command's options are.

use std::io::Write;

/// How a command ends: its exit code, its words said already.
#[derive(Debug, PartialEq, Eq)]
pub struct Exit(pub u8);

pub type Res<T> = Result<T, Exit>;

/// The bytes of each part, one after the other: a message from names, paths and words.
#[macro_export]
macro_rules! cat {
    ($($p:expr),* $(,)?) => {{
        let mut v: Vec<u8> = Vec::new();
        $( v.extend_from_slice(AsRef::<[u8]>::as_ref(&$p)); )*
        v
    }};
}

thread_local! {
    /// This thread's words held back (muted): `( ... ) 2>/dev/null` for one helper's refusals,
    /// without moving fd 2, which other threads and an exec would see.
    static MUTE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// F with what it says held back (say's lines, its refusals' words), on this thread only.
pub fn muted<T>(f: impl FnOnce() -> T) -> T {
    let was = MUTE.with(|m| m.replace(true));
    let r = f();
    MUTE.with(|m| m.set(was));
    r
}

/// Whether this thread's words are held back (inside muted).
pub fn is_muted() -> bool {
    MUTE.with(std::cell::Cell::get)
}

/// `say`: one line on stderr.
pub fn say(m: impl AsRef<[u8]>) {
    if MUTE.with(std::cell::Cell::get) {
        return;
    }
    let mut line = b"omabox: ".to_vec();
    line.extend_from_slice(m.as_ref());
    line.push(b'\n');
    #[cfg(test)]
    if HEARD.with(|h| h.borrow_mut().as_mut().map(|h| h.extend_from_slice(&line))) == Some(()) {
        return;
    }
    let _ = std::io::stderr().write_all(&line);
}

#[cfg(test)]
thread_local! {
    /// This test thread's said lines, while heard runs.
    static HEARD: std::cell::RefCell<Option<Vec<u8>>> = const { std::cell::RefCell::new(None) };
    /// Whether out's lines are heard too (heard_all).
    static HEAR_OUT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether this test thread hears out's lines (heard_all): what a `>/dev/null` around a helper
/// leaves to the hearing instead of moving fd 1, which every test thread shares.
#[cfg(test)]
pub fn hearing_out() -> bool {
    HEAR_OUT.with(std::cell::Cell::get)
}

/// F's result and the lines it said (say's, its refusals' words), as `2>&1` gave the suite; on this
/// thread only.
#[cfg(test)]
pub fn heard<T>(f: impl FnOnce() -> T) -> (T, String) {
    let was = HEARD.with(|h| h.replace(Some(Vec::new())));
    let r = f();
    let got = HEARD.with(|h| h.replace(was)).unwrap_or_default();
    (r, String::from_utf8_lossy(&got).into_owned())
}

/// heard, out's lines too, in the order said: what `2>&1` gave the suite of a helper that prints.
#[cfg(test)]
pub fn heard_all<T>(f: impl FnOnce() -> T) -> (T, String) {
    let was = HEAR_OUT.with(|h| h.replace(true));
    let r = heard(f);
    HEAR_OUT.with(|h| h.set(was));
    r
}

/// What bash's globals hold for one call: the refusal conventions and the box selected.
#[derive(Clone)]
pub struct Ctx {
    pub parsing: bool,
    pub die_rc: Option<u8>,
    /// MAIN_CMD: the command asked for.
    pub main_cmd: Vec<u8>,
    /// BOXES: $XDG_RUNTIME_DIR/omabox, as written.
    pub boxes: Vec<u8>,
    /// NAME and D (the box dir), set by select_box.
    pub name: Vec<u8>,
    pub d: Vec<u8>,
    /// NAME_DERIVED: the name came from default_name, not from the caller.
    pub name_derived: bool,
    /// PASS (run --pass): the caller's variables for the command, by name.
    pub pass: Vec<Vec<u8>>,
    /// ENVKV (run --env-file, then --env): KEY=VAL entries for the command, before --pass's.
    pub envkv: Vec<Vec<u8>>,
    /// BOX_GONE_SAID: a box that went down while a command entered it was said so (ns_said).
    pub box_gone_said: bool,
}

impl Ctx {
    pub fn new(main_cmd: &[u8], boxes: Vec<u8>) -> Ctx {
        Ctx {
            parsing: true,
            die_rc: None,
            main_cmd: main_cmd.to_vec(),
            boxes,
            name: Vec::new(),
            d: Vec::new(),
            name_derived: false,
            pass: Vec::new(),
            envkv: Vec::new(),
            box_gone_said: false,
        }
    }

    /// `${MAIN_CMD:+$MAIN_CMD: }`.
    pub fn label(&self) -> Vec<u8> {
        if self.main_cmd.is_empty() {
            Vec::new()
        } else {
            cat!(self.main_cmd, ": ")
        }
    }

    /// die: a refusal; a usage error while the command line is read.
    pub fn die(&self, m: impl AsRef<[u8]>) -> Exit {
        if self.parsing {
            return self.usage_die(m);
        }
        say(m);
        Exit(self.die_rc.unwrap_or(1))
    }

    /// A usage error: exit 2 (run's 125), the message followed by where the command's options are.
    pub fn usage_die(&self, m: impl AsRef<[u8]>) -> Exit {
        let mut m = m.as_ref().to_vec();
        if !contains(&m, b"omabox help") && !contains(&m, b"--help") {
            m.extend_from_slice(b" (omabox help");
            if !self.main_cmd.is_empty() {
                m.push(b' ');
                m.extend_from_slice(&self.main_cmd);
            }
            m.push(b')');
        }
        say(m);
        Exit(self.die_rc.unwrap_or(2))
    }

    /// deprecated OLD NEW (#287): the one helper every deprecation warning goes through, `omabox:
    /// deprecated: OLD: write NEW (refused in X.Y)`, the next minor release of the one this was
    /// built from; OMABOX_STRICT=1 refuses the form at once (usage_die). 1.0 has no live
    /// deprecation: nothing calls it yet (bash's was called only by the suite too).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn deprecated(&self, old: &[u8], new: &[u8]) -> Res<()> {
        const VERSION: &str = include_str!("../../../VERSION");
        let mut p = VERSION.trim().split('.');
        let v = match (
            p.next().map(str::parse::<u64>),
            p.next().map(str::parse::<u64>),
        ) {
            (Some(Ok(a)), Some(Ok(b))) => format!("{a}.{}", b + 1),
            _ => "the next minor release".to_string(),
        };
        if crate::shell::env_or_empty("OMABOX_STRICT") == b"1" {
            return Err(self.usage_die(crate::cat!(
                "deprecated: ",
                old,
                ": write ",
                new,
                " (refused now: OMABOX_STRICT=1)"
            )));
        }
        say(crate::cat!(
            "deprecated: ",
            old,
            ": write ",
            new,
            format!(" (refused in {v})")
        ));
        Ok(())
    }

    /// deprecated_omarchy WHAT INSTALLED (#411): deprecated's line for the Omarchy a box runs (WHAT,
    /// as install::omarchy_before_45 names it), which omabox 1.3 no longer supports (#312); for the
    /// installed one (not an --omarchy tree), how to update it. Not a form
    /// the caller wrote, so OMABOX_STRICT=1 does not refuse it: nothing in the call can change it.
    /// omarchy<4.5: removed with the 4.0 paths.
    pub fn deprecated_omarchy(&self, what: &[u8], installed: bool) {
        let how: &[u8] = if installed {
            b" (omarchy update brings it)"
        } else {
            b""
        };
        say(crate::cat!(
            "deprecated: ",
            what,
            ": omabox 1.3 needs Omarchy 4.5 or later",
            how
        ));
    }

    /// Exit as die would, for a refusal a helper has said already.
    pub fn refused(&self) -> Exit {
        match self.die_rc {
            Some(rc) => Exit(rc),
            None if self.parsing => Exit(2),
            None => Exit(1),
        }
    }

    /// The command line read: refusals from here are failures (1), not usage errors.
    pub fn parsed(&mut self) {
        self.parsing = false;
    }

    /// `$D/REL`.
    pub fn at(&self, rel: &str) -> std::path::PathBuf {
        crate::shell::path(&cat!(self.d, "/", rel))
    }

    /// `$BOXES/REL`.
    pub fn in_boxes(&self, rel: impl AsRef<[u8]>) -> std::path::PathBuf {
        crate::shell::path(&cat!(self.boxes, "/", rel))
    }
}

pub fn contains(hay: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || hay.windows(needle.len()).any(|w| w == needle)
}

/// Lines on stdout. A write that fails (stdout closed) ends the command, 1, as bash's echo under
/// set -e does.
pub fn out(v: impl AsRef<[u8]>) -> Res<()> {
    #[cfg(test)]
    if hearing_out()
        && HEARD.with(|h| {
            h.borrow_mut()
                .as_mut()
                .map(|h| h.extend_from_slice(v.as_ref()))
        }) == Some(())
    {
        return Ok(());
    }
    let mut o = std::io::stdout().lock();
    o.write_all(v.as_ref())
        .and_then(|()| o.flush())
        .map_err(|_| Exit(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes() {
        let mut c = Ctx::new(b"path", b"/x".to_vec());
        assert_eq!(c.die("a"), Exit(2));
        assert_eq!(c.refused(), Exit(2));
        c.parsed();
        assert_eq!(c.die("a"), Exit(1));
        assert_eq!(c.usage_die("a"), Exit(2));
        c.die_rc = Some(125);
        assert_eq!(c.die("a"), Exit(125));
        assert_eq!(c.refused(), Exit(125));
    }

    // t_unit_deprecated (#287): one line on stderr, naming the new form and the release that
    // refuses it; OMABOX_STRICT=1 refuses it at once (2, or run's own failure).
    #[test]
    fn deprecated_as_bash() {
        let v = include_str!("../../../VERSION");
        let mut p = v.trim().split('.');
        let (a, b): (u64, u64) = (
            p.next().unwrap().parse().unwrap(),
            p.next().unwrap().parse().unwrap(),
        );
        let next = format!("{a}.{}", b + 1);
        let mut ctx = Ctx::new(b"up", Vec::new());
        let dep = |ctx: &Ctx, strict: Option<&str>| {
            heard(|| {
                crate::shell::with_env(&[("OMABOX_STRICT", strict)], || {
                    ctx.deprecated(b"up --old", b"up --new")
                })
            })
        };
        let (r, said) = dep(&ctx, None);
        assert!(r.is_ok());
        assert_eq!(
            said,
            format!("omabox: deprecated: up --old: write up --new (refused in {next})\n"),
            "deprecated: one line on stderr, naming the new form and the release that refuses it; then read"
        );
        assert!(
            dep(&ctx, Some("0")).0.is_ok(),
            "...OMABOX_STRICT=0: the same"
        );
        let (r, said) = dep(&ctx, Some("1"));
        assert_eq!(
            (r.err().map(|e| e.0), said.as_str()),
            (
                Some(2),
                "omabox: deprecated: up --old: write up --new (refused now: OMABOX_STRICT=1) (omabox help up)\n"
            ),
            "...OMABOX_STRICT=1: refused at once, exit 2, nothing read"
        );
        ctx.die_rc = Some(125);
        assert_eq!(
            dep(&ctx, Some("1")).0.err().map(|e| e.0),
            Some(125),
            "...under run: run's own failure, 125"
        );
    }

    // #411: an Omarchy before 4.5 named on one deprecated: line, never refused (not the caller's form);
    // the installed one with how to update it, an --omarchy tree without.
    #[test]
    fn deprecated_omarchy_line() {
        let ctx = Ctx::new(b"up", Vec::new());
        for strict in [None, Some("1")] {
            let ((), said) = heard(|| {
                crate::shell::with_env(&[("OMABOX_STRICT", strict)], || {
                    ctx.deprecated_omarchy(b"Omarchy 4.0.6-1", true)
                })
            });
            assert_eq!(
                said,
                "omabox: deprecated: Omarchy 4.0.6-1: omabox 1.3 needs Omarchy 4.5 or later (omarchy update brings it)\n",
                "one line, the same under OMABOX_STRICT={strict:?}"
            );
        }
        let ((), said) =
            heard(|| ctx.deprecated_omarchy(b"--omarchy /t (an Omarchy before 4.5)", false));
        assert_eq!(
            said,
            "omabox: deprecated: --omarchy /t (an Omarchy before 4.5): omabox 1.3 needs Omarchy 4.5 or later\n",
            "...a tree: no update hint"
        );
    }
}
