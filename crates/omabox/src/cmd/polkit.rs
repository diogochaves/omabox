//! `omabox polkit [-b NAME] (ask [OPTIONS] | agents [--json])` (#258, finding 627): an `up --polkit`
//! box's polkit agent asked for a password, as polkitd asks one, by omabox-polkit in the box (its
//! stand-in authority); or the agents registered there. ask waits for the request's end, as the
//! agent ends it (authenticated, not-authenticated, cancelled, agent-gone, no-agent; timeout when the
//! authority cancelled it after --timeout), and says it: exit 0 authenticated, 124 timeout, 1 any
//! other end. The password --accept names is the only one the box's helper takes: a test's, never
//! yours (it is on omabox-polkit's command line in the box).

use crate::args::{optarg, unexpected, unknown_opt, unknown_word};
use crate::boxes::{meta, need_box};
use crate::cat;
use crate::cmd::up_box::BOX_POLKIT;
use crate::exec::{ExecOpts, Redir, box_exec, pipe, read_all};
use crate::exit::{Ctx, Exit, Res, out};
use crate::num::{ms_duration, s_duration};
use crate::select::select_box;
use crate::term;
use std::io::Write;
use std::os::fd::AsRawFd;

/// What of the box's answer is read: a request's end, or the agents registered (a few lines).
const ANSWER_CAP: usize = 64 << 10;
/// What of its words (stderr) is read.
const WORDS_CAP: usize = 4 << 10;

/// The box's answer as said here (S7: it is the box's, whoever there answered): its text plain, at
/// most ANSWER_CAP of it; with JSON, JSON as serde writes it again (None when it is none).
pub fn answer(said: &[u8], json: bool) -> Option<Vec<u8>> {
    if !json {
        if said.len() > ANSWER_CAP {
            return Some(cat!(
                term::plain_cap(&said[..ANSWER_CAP], ANSWER_CAP),
                "... (cut at 64 KiB)\n"
            ));
        }
        return Some(term::plain_cap(said, ANSWER_CAP));
    }
    if said.len() > ANSWER_CAP {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(said).ok()?;
    Some(cat!(crate::json::compact(&v), "\n"))
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut act, mut json) = (Vec::new(), Vec::new(), false);
    // What goes to omabox-polkit ask, checked here.
    let mut ask: Vec<Vec<u8>> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let v = args.get(i + 1);
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, v)?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"--action" | b"--message" | b"--icon" | b"--prompt" | b"--accept" => {
                optarg(ctx, a, v)?;
                ask.extend([a.clone(), args[i + 1].clone()]);
                i += 1;
            }
            b"--detail" => {
                optarg(ctx, a, v)?;
                let d = &args[i + 1];
                if d.first().is_none_or(|&c| c == b'=') || !d.contains(&b'=') {
                    return Err(ctx.usage_die(cat!("polkit ask: --detail is KEY=VALUE, got ", d)));
                }
                ask.extend([a.clone(), d.clone()]);
                i += 1;
            }
            b"--timeout" => {
                optarg(ctx, a, v)?;
                let Some(s) = s_duration(&args[i + 1], false)
                    .filter(|s| crate::num::awk_num(s.as_bytes()) <= 3600.0)
                else {
                    return Err(ctx.usage_die(cat!(
                        "polkit ask: --timeout is a duration up to 1h, like 60s (a bare number is seconds), got ",
                        args[i + 1]
                    )));
                };
                ask.extend([a.clone(), s.into_bytes()]);
                i += 1;
            }
            b"--delay" => {
                optarg(ctx, a, v)?;
                let ms = if crate::shell::re_match(r"^0+(\.0+)?(ms|s)?$", &args[i + 1]) {
                    Some(0)
                } else {
                    ms_duration(&args[i + 1]).filter(|&m| m <= 60_000)
                };
                let Some(ms) = ms else {
                    return Err(ctx.usage_die(cat!(
                        "polkit ask: --delay is a duration up to 1m, like 500ms (a bare number is seconds), got ",
                        args[i + 1]
                    )));
                };
                ask.extend([a.clone(), ms.to_string().into_bytes()]);
                i += 1;
            }
            b"--json" => json = true,
            b"-h" | b"--help" => return crate::args::usage("polkit"),
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "polkit", a)),
            _ if act.is_empty() => act = a.clone(),
            _ => return Err(unexpected(ctx, "polkit", a, "")),
        }
        i += 1;
    }
    match act.as_slice() {
        b"ask" => {}
        b"agents" => {
            if !ask.is_empty() {
                return Err(ctx.usage_die(
                    "polkit agents: takes only --json (--action, --accept and the rest are ask's)",
                ));
            }
        }
        b"" => {
            return Err(ctx.usage_die("polkit: ask or agents? (omabox help polkit)"));
        }
        w => {
            return Err(unknown_word(
                ctx,
                "polkit",
                "subcommand",
                w,
                "(ask or agents)",
                &["ask", "agents"],
            ));
        }
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    if meta(&ctx.d, "polkit").0 != b"true" {
        return Err(ctx.die(cat!(
            "polkit: box '",
            ctx.name,
            "' has no polkit: a box gets it at up (omabox up --polkit; down first, or up --new --polkit)"
        )));
    }
    let mut argv: Vec<Vec<u8>> = vec![BOX_POLKIT.as_bytes().to_vec(), act.clone()];
    argv.extend(ask);
    if json {
        argv.push(b"--json".to_vec());
    }
    // Its answer and its words (stderr) read here, bounded, and said plain (S7): every process in
    // the box has the stand-in's uid, so one that ended it can answer on its bus in its place.
    let (r1, w1) = pipe().map_err(|_| ctx.die("polkit: no pipe"))?;
    let (r2, w2) = pipe().map_err(|_| ctx.die("polkit: no pipe"))?;
    let o = std::thread::spawn(move || read_all(r1, Some(ANSWER_CAP + 1)));
    let e = std::thread::spawn(move || read_all(r2, Some(WORDS_CAP + 1)));
    let rc = {
        let _e = Redir::to(2, w2.as_raw_fd());
        box_exec(
            ctx,
            &argv,
            ExecOpts {
                sys: true,
                stdin_null: true,
                stdout: Some(w1.as_raw_fd()),
                ..ExecOpts::default()
            },
        )
    };
    drop((w1, w2));
    let (said, words) = (o.join().unwrap_or_default(), e.join().unwrap_or_default());
    if !words.is_empty() {
        let _ = std::io::stderr().write_all(&term::plain_cap(&words, WORDS_CAP));
    }
    let rc = rc?;
    match answer(&said, json) {
        Some(a) => out(a)?,
        None => {
            return Err(ctx.die(cat!(
                "polkit: the box's answer was not the JSON asked (omabox log -b ",
                ctx.name,
                " polkit)"
            )));
        }
    }
    match rc {
        0 => Ok(()),
        // timeout: the request had no end in time
        124 => Err(Exit(124)),
        1 => Err(Exit(1)),
        _ => Err(ctx.die(cat!(
            "polkit: the box's stand-in authority did not answer (omabox log -b ",
            ctx.name,
            " polkit)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the box answers reaches the terminal with no escape, bounded; JSON as JSON (S7).
    #[test]
    fn answer_is_plain_and_bounded() {
        assert_eq!(
            answer(
                b"authenticated\x1b]0;x\x07: the agent at :1.2 /a, 1 password (0 wrong)\n",
                false
            )
            .unwrap(),
            b"authenticated?]0;x?: the agent at :1.2 /a, 1 password (0 wrong)\n"
        );
        assert_eq!(answer(b"a\x9bb\xff\tc\n", false).unwrap(), b"a?b?\tc\n");
        let long = vec![b'w'; ANSWER_CAP + 1];
        let a = answer(&long, false).unwrap();
        assert!(a.starts_with(&[b'w'; ANSWER_CAP]) && a.ends_with(b"w... (cut at 64 KiB)\n"));
        assert_eq!(
            answer(br#"[{"subject":"s\u001b]0;x","fallback":false}]"#, true).unwrap(),
            br#"[{"subject":"s\u001b]0;x","fallback":false}]"#
                .to_vec()
                .into_iter()
                .chain(*b"\n")
                .collect::<Vec<u8>>()
        );
        assert_eq!(
            answer(b"{\"a\": \"\x1b\"}", true),
            None,
            "a raw escape is no JSON"
        );
        assert_eq!(answer(b"not json", true), None);
    }
}
