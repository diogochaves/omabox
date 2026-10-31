//! The accessibility tree as a condition (#319, finding 595): `wait tree TEXT [--gone]` polls it
//! (wait.rs's poll_until), an action's `--wait --expect tree TEXT` reads it once the screen settled
//! (settle.rs). TEXT is matched as `tree --find` matches it (any case, the whole strings), in the
//! window SELS picks (none: the active one, picked again at each read) or on the layer LAYER.

use crate::cat;
use crate::cmd::tree;
use crate::exit::{Ctx, Exit, Res, muted, say};
use crate::hypr::hypr_json;
use crate::jq::{self, Args};
use crate::settle::mono_ms;
use crate::shell::strip_nl;
use crate::win::{box_windows, joined, jq_error_text, jq_length, jq_r, win_hits, win_select};
use std::time::Duration;

/// try_once's failure for a box that did not answer (its windows or layers not listed), not said:
/// the caller says it (poll_until's "went down", "could not ask").
pub const NO_ANSWER: u8 = 75;

/// What is looked for, and where.
pub struct TreeWant {
    pub text: Vec<u8>,
    pub sels: Vec<Vec<u8>>,
    pub layer: Vec<u8>,
}

/// One read's answer.
pub enum Try {
    /// TEXT is there: the lines holding it (as `tree --find` prints them).
    Held(Vec<u8>),
    /// The tree was read and holds no TEXT: omabox-tree's words.
    Not(Vec<u8>),
    /// No tree to read for the window or layer (not on the bus yet, a terminal): why.
    NoTree(Vec<u8>),
    /// No such window or layer now: why.
    Absent(Vec<u8>),
}

/// The window or layer to read now, as a selector `tree::read` takes (its address), or why none.
enum Target {
    Win(Vec<u8>),
    Layer(Vec<u8>),
    Absent(Vec<u8>),
}

fn target(ctx: &mut Ctx, w: &TreeWant) -> Res<Target> {
    if !w.layer.is_empty() {
        let lys = strip_nl(&muted(|| hypr_json(ctx, &["layers"])).map_err(|_| Exit(NO_ANSWER))?);
        let all = jq::jq(
            &["-c", crate::jqprog::LAYER_LIST_JQ],
            Some(&lys),
            jq::Err::Null,
        );
        if !all.ok() {
            return Err(Exit(NO_ANSWER));
        }
        let hits = Args::new(&["-c"])
            .kv("--arg", "s", &w.layer)
            .arg(crate::jqprog::LAYER_HITS_JQ)
            .run(Some(&all.text()), jq::Err::Inherit)
            .text();
        let big = jq::jq(
            &["-c", crate::jqprog::LAYER_BIG_JQ],
            Some(&hits),
            jq::Err::Inherit,
        )
        .text();
        return Ok(match jq_length(&big).as_slice() {
            b"1" => Target::Layer(cat!("address:", jq_r(".[0].address", &big))),
            b"0" if jq_length(&hits) != b"0" => {
                Target::Absent(cat!("layer ", w.layer, " is mapped at 1x1, not drawn"))
            }
            b"0" => Target::Absent(cat!("no layer ", w.layer)),
            // (several: refused as tree --layer refuses them, with the candidates)
            _ => {
                crate::layer::layer_select(ctx, &lys, &w.layer)?;
                return Err(Exit(2));
            }
        });
    }
    let wins = muted(|| box_windows(ctx, None)).map_err(|_| Exit(NO_ANSWER))?;
    // No list: the box not answering (finding 176).
    if wins.is_empty() {
        return Err(Exit(NO_ANSWER));
    }
    let active = [b"active".to_vec()];
    let sels: &[Vec<u8>] = if w.sels.is_empty() { &active } else { &w.sels };
    let hits = match win_hits(&wins, sels) {
        Ok(h) => h,
        Err(e) => {
            say(cat!(
                "bad window selector (",
                joined(sels),
                "): ",
                jq_error_text(&e)
            ));
            return Err(Exit(2));
        }
    };
    Ok(match jq_length(&hits).as_slice() {
        b"1" => Target::Win(jq_r(".[0].address", &hits)),
        b"0" if w.sels.is_empty() => Target::Absent(b"no window is active".to_vec()),
        b"0" => Target::Absent(cat!("no window matches ", joined(sels))),
        _ => {
            win_select(ctx, &wins, sels)?;
            return Err(Exit(2));
        }
    })
}

/// The seconds a read may take before DEADLINE (a mono_ms time), as `timeout` takes them: the time
/// left, half a second at least (a read takes ~0.1 s) and 30 s at most, as `tree`'s.
pub fn secs_left(deadline: i64, now: i64) -> Vec<u8> {
    let ms = (deadline - now).clamp(500, 30_000);
    crate::num::fixed(ms as f64 / 1000.0, 3).into_bytes()
}

/// One read of what W names with omabox-tree's TARGS, given until DEADLINE (mono_ms; secs_left): the
/// window or layer read (its address, as `tree::read` takes it) and the reading, or why there is
/// none to read now. A window closing between the pick and the read is gone, not a failure.
fn read_target(
    ctx: &mut Ctx,
    w: &TreeWant,
    targs: &[Vec<u8>],
    deadline: i64,
) -> Res<Result<(Target, tree::Reading), Vec<u8>>> {
    let read = |ctx: &mut Ctx, t: &Target| -> Res<tree::Reading> {
        let secs = secs_left(deadline, mono_ms());
        let (sels, l): (&[Vec<u8>], &[u8]) = match t {
            Target::Win(a) => (std::slice::from_ref(a), &[]),
            Target::Layer(l) => (&[], l),
            Target::Absent(_) => return Err(Exit(1)),
        };
        tree::read_within(ctx, "wait", sels, l, targs.to_vec(), true, &secs)
    };
    let t = target(ctx, w)?;
    if let Target::Absent(why) = t {
        return Ok(Err(why));
    }
    // (a window closing between the two reads is no failure: it is gone; else read again, said. A
    // refusal's list of windows goes to fd 2 itself: that too held back, one thread here)
    let first_read = {
        let _quiet = crate::exec::Redir::null(2);
        muted(|| read(ctx, &t))
    };
    Ok(Ok(match first_read {
        Ok(r) => (t, r),
        Err(_) => match target(ctx, w)? {
            Target::Absent(why) => return Ok(Err(why)),
            t => {
                let r = read(ctx, &t)?;
                (t, r)
            }
        },
    }))
}

/// One read of the tree for TEXT (FIND: what is looked for, TEXT or a part of it), given until
/// DEADLINE (mono_ms; secs_left): one that takes longer is "did not answer", not held. A failure that
/// no retry changes (no accessibility bus) is said, exit 1; several windows or layers matching, exit
/// 2, with them.
pub fn try_once(ctx: &mut Ctx, w: &TreeWant, find: &[u8], deadline: i64) -> Res<Try> {
    let targs = [b"--find".to_vec(), find.to_vec()];
    let r = match read_target(ctx, w, &targs, deadline)? {
        Ok((_, r)) => r,
        Err(why) => return Ok(Try::Absent(why)),
    };
    let first = |v: &[u8]| v.split(|&c| c == b'\n').next().unwrap_or_default().to_vec();
    Ok(match r.rc {
        0 => Try::Held(r.out),
        1 => Try::Not(first(&r.why)),
        3 | 5 | 6 | 7 => Try::NoTree(cat!(r.what, ": ", first(&r.why))),
        // (killed at the deadline: the app busy or hung, said at the 124)
        124 => Try::NoTree(cat!(r.what, " did not answer in time (busy or hung?)")),
        _ => {
            tree::fail(ctx, "wait", &r)?;
            return Err(Exit(1));
        }
    })
}

/// --wait --tree (#288): the whole tree of one window or layer (`tree --json`), read before the
/// input and again once the screen settled: the same one both times, by its address.
pub struct Snap {
    /// The window's or layer's address, as `tree::read` takes it; what a message calls it.
    pub sel: Vec<u8>,
    pub layer: bool,
    pub what: Vec<u8>,
    /// omabox-tree --json's array.
    pub json: Vec<u8>,
}

/// One whole read of what W names (none: the active window), given until DEADLINE: Ok(the snap), or
/// Err(why there is none: no such window, no tree for it, the app did not answer, a failure). Never
/// a refusal, nothing said: the input goes all the same, and the line says why nothing is compared.
pub fn snap(ctx: &mut Ctx, w: &TreeWant, deadline: i64) -> Result<Snap, Vec<u8>> {
    let targs = [b"--json".to_vec()];
    let got = {
        let _quiet = crate::exec::Redir::null(2);
        muted(|| read_target(ctx, w, &targs, deadline))
    };
    let (t, r) = match got {
        Ok(Ok(x)) => x,
        Ok(Err(why)) => return Err(why),
        Err(Exit(2)) => return Err(b"several windows or layers match".to_vec()),
        Err(_) => return Err(b"the tree could not be read".to_vec()),
    };
    let first = |v: &[u8]| v.split(|&c| c == b'\n').next().unwrap_or_default().to_vec();
    match r.rc {
        0 => {}
        124 => return Err(cat!(r.what, " did not answer in time (busy or hung?)")),
        4 => return Err(b"the box has no accessibility bus".to_vec()),
        _ if r.why.is_empty() => {
            return Err(cat!(r.what, ": no tree (status ", r.rc.to_string(), ")"));
        }
        _ => return Err(cat!(r.what, ": ", first(&r.why))),
    }
    let (sel, layer) = match t {
        Target::Win(a) => (a, false),
        Target::Layer(a) => (a, true),
        Target::Absent(why) => return Err(why),
    };
    Ok(Snap {
        sel,
        layer,
        what: r.what,
        json: r.out,
    })
}

/// S's window or layer read again, given until DEADLINE: its tree now, or why there is none ("it
/// closed" when it is gone).
pub fn snap_again(ctx: &mut Ctx, s: &Snap, deadline: i64) -> Result<Vec<u8>, Vec<u8>> {
    let w = TreeWant {
        text: Vec::new(),
        sels: if s.layer {
            Vec::new()
        } else {
            vec![s.sel.clone()]
        },
        layer: if s.layer { s.sel.clone() } else { Vec::new() },
    };
    match snap(ctx, &w, deadline) {
        Ok(n) => Ok(n.json),
        Err(why) if why.starts_with(b"no window matches") || why.starts_with(b"no layer") => {
            Err(b"it closed".to_vec())
        }
        Err(why) => Err(why),
    }
}

/// The window at the screen's X Y as a click there reaches it (the topmost shown there, nothing
/// over it at the point): its address, or nothing.
pub fn window_at(ctx: &Ctx, x: &[u8], y: &[u8]) -> Vec<u8> {
    let Ok(wins) = muted(|| box_windows(ctx, None)) else {
        return Vec::new();
    };
    let r = Args::new(&["-r"])
        .kv("--argjson", "x", x)
        .kv("--argjson", "y", y)
        .arg(crate::jqprog::WIN_AT_POINT_JQ)
        .run(Some(&wins), jq::Err::Null);
    if r.ok() {
        strip_nl(&r.text())
    } else {
        Vec::new()
    }
}

/// wait tree TEXT: what is looked for, whether until it is gone, the shot to take the moment it holds
/// (--shot FILE: shot's words, -b and -o), and the yes in a row so far.
pub struct TreeProbe {
    pub want: TreeWant,
    pub gone: bool,
    pub shot: Vec<Vec<u8>>,
    pub need: u32,
    pub streak: std::cell::Cell<u32>,
}

/// wait.rs's tree_probe, one try: "yes DETAIL" / "no DETAIL"; the shot taken at the yes that ends the wait.
pub fn answer(ctx: &mut Ctx, p: &TreeProbe, t0: i64, tmo: i64) -> (i32, Vec<u8>) {
    let w = &p.want;
    let got = match try_once(ctx, w, &w.text, t0 + tmo) {
        Ok(t) => t,
        Err(Exit(2)) => return (2, Vec::new()),
        // (the box not answering: poll_until says why; any other failure was said)
        Err(Exit(NO_ANSWER)) => return (1, Vec::new()),
        Err(_) => return (3, Vec::new()),
    };
    let (yes, detail) = match (got, p.gone) {
        (Try::Held(l), false) => (true, first_and_more(&l)),
        (Try::Held(l), true) => {
            let n = l.split(|&c| c == b'\n').filter(|x| !x.is_empty()).count();
            (
                false,
                cat!(n.to_string(), " holding it: ", first_and_more(&l)),
            )
        }
        (Try::Not(_), true) => (true, Vec::new()),
        (Try::Absent(why), true) => (true, why),
        (Try::Not(why) | Try::NoTree(why) | Try::Absent(why), false) => {
            // (the last try: what the tree has instead, said)
            let why = if mono_ms() - t0 >= tmo {
                instead(ctx, w, &why)
            } else {
                why
            };
            (false, why)
        }
        (Try::NoTree(why), true) => (false, why),
    };
    if !yes {
        p.streak.set(0);
        return (0, cat!("no ", detail));
    }
    p.streak.set(p.streak.get() + 1);
    let mut detail = detail;
    if p.streak.get() >= p.need && !p.shot.is_empty() {
        let mut sub = ctx.clone();
        let r = {
            let _o = crate::exec::Redir::null(1);
            crate::cmd::shot::run(&mut sub, &p.shot)
        };
        if r.is_err() {
            return (
                0,
                b"stop shot: the tree was as waited for, but the shot failed".to_vec(),
            );
        }
        let f = &p.shot[3];
        detail = if detail.is_empty() {
            cat!("shot ", f)
        } else {
            cat!(detail, "; shot ", f)
        };
    }
    if detail.is_empty() {
        (0, b"yes".to_vec())
    } else {
        (0, cat!("yes ", detail))
    }
}

/// LINES' first line, and how many more.
pub fn first_and_more(lines: &[u8]) -> Vec<u8> {
    let ls: Vec<&[u8]> = lines
        .split(|&c| c == b'\n')
        .filter(|l| !l.is_empty())
        .collect();
    match ls.len() {
        0 => Vec::new(),
        1 => ls[0].to_vec(),
        n => cat!(ls[0], " (+", (n - 1).to_string(), " more)"),
    }
}

/// The part of TEXT naming the node, to say what the tree has instead: its first "quoted" name, else
/// what comes before its states, text or value (` [`, ` '`, ` =`); None when that is TEXT itself.
pub fn name_part(text: &[u8]) -> Option<Vec<u8>> {
    let t = text.trim_ascii();
    let part: &[u8] = match t.iter().position(|&c| c == b'"') {
        Some(i) => match t[i + 1..].iter().position(|&c| c == b'"') {
            Some(j) => &t[i..i + j + 2],
            None => t,
        },
        None => {
            let cut = [&b" ["[..], b" '", b" ="]
                .iter()
                .filter_map(|p| t.windows(2).position(|x| x == *p))
                .min();
            cut.map_or(t, |c| t[..c].trim_ascii())
        }
    };
    (!part.is_empty() && part != t && part != b"\"\"").then(|| part.to_vec())
}

/// What the tree has instead of TEXT, for a wait that ended without it: the lines holding its name
/// (name_part), else WHY.
pub fn instead(ctx: &mut Ctx, w: &TreeWant, why: &[u8]) -> Vec<u8> {
    if let Some(n) = name_part(&w.text) {
        if let Ok(Try::Held(l)) = muted(|| try_once(ctx, w, &n, mono_ms() + 1000)) {
            return cat!("instead: ", first_and_more(&l));
        }
    }
    why.to_vec()
}

/// --expect tree TEXT: read until it holds TEXT or LEFT_MS is up (one read at least). Ok((true,
/// the line)) held, Ok((false, what it had instead)) not; Err: a failure, said.
pub fn expect(ctx: &Ctx, w: &TreeWant, left_ms: i64) -> Res<(bool, Vec<u8>)> {
    let mut c = ctx.clone();
    let t0 = mono_ms();
    loop {
        let got = match try_once(&mut c, w, &w.text, t0 + left_ms) {
            Err(Exit(NO_ANSWER)) => {
                say(cat!("--expect: box '", c.name, "' did not answer"));
                return Err(Exit(1));
            }
            g => g?,
        };
        let why = match got {
            Try::Held(l) => return Ok((true, first_and_more(&l))),
            Try::Not(why) | Try::NoTree(why) | Try::Absent(why) => why,
        };
        if mono_ms() - t0 >= left_ms {
            return Ok((false, instead(&mut c, w, &why)));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_lines() {
        let n = |t: &str| name_part(t.as_bytes()).map(|v| String::from_utf8(v).unwrap());
        assert_eq!(
            n(r#""Dark mode" [checked]"#).as_deref(),
            Some(r#""Dark mode""#)
        );
        assert_eq!(
            n(r#"check box "Dark mode" [checked]"#).as_deref(),
            Some(r#""Dark mode""#)
        );
        assert_eq!(n("Dark mode [checked]").as_deref(), Some("Dark mode"));
        assert_eq!(n("entry 'hello' [editable]").as_deref(), Some("entry"));
        assert_eq!(n("slider =40").as_deref(), Some("slider"));
        // (TEXT alone: nothing else to look for)
        assert_eq!(n(r#""Dark mode""#), None);
        assert_eq!(n("  Saved  "), None);
        assert_eq!(n(r#"an "unclosed quote"#), None);
        assert_eq!(first_and_more(b""), b"".to_vec());
        assert_eq!(first_and_more(b"a b\n"), b"a b".to_vec());
        assert_eq!(first_and_more(b"a\nb\nc\n"), b"a (+2 more)".to_vec());
        // (a read given the time left: half a second at least, 30 s at most, #319)
        assert_eq!(secs_left(10_000, 8_000), b"2.000".to_vec());
        assert_eq!(secs_left(10_000, 10_400), b"0.500".to_vec());
        assert_eq!(secs_left(100_000, 0), b"30.000".to_vec());
    }
}
