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
use crate::exec::{ExecOpts, box_exec, pipe, read_all};
use crate::exit::{Ctx, Exit, Res, out};
use crate::num::{ms_duration, s_duration};
use crate::select::select_box;
use std::os::fd::AsRawFd;

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
    // Its answer read here, bounded (S7); its words (stderr) are the box's to say.
    let (r, w) = pipe().map_err(|_| ctx.die("polkit: no pipe"))?;
    let reader = std::thread::spawn(move || read_all(r, Some(1 << 20)));
    let rc = box_exec(
        ctx,
        &argv,
        ExecOpts {
            sys: true,
            stdin_null: true,
            stdout: Some(w.as_raw_fd()),
            ..ExecOpts::default()
        },
    );
    drop(w);
    let said = reader.join().unwrap_or_default();
    let rc = rc?;
    out(&said)?;
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
