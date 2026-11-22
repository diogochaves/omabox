//! `omabox audio [-b NAME] [DURATION] [--json]` (#226): whether a box made a sound, its speaker's
//! monitor read for DURATION (3 s) by ffmpeg in the box, 100 ms at a time (peak and RMS, dBFS). The
//! speaker is a null sink, exact zeros while nothing plays: a window above AUDIO_FLOOR is a sound.
//! Exit 0 either way: silence is an answer, said as such.

use crate::args::{optarg, unknown_opt};
use crate::boxes::need_box;
use crate::boxmeta::BoxMeta;
use crate::cat;
use crate::exec::{ExecOpts, box_exec, pipe, read_all};
use crate::exit::{Ctx, Res, out};
use crate::jq::{self, Args};
use crate::num::{awk_num, fixed, s_duration};
use crate::select::select_box;
use std::os::fd::AsRawFd;

const AUDIO_FLOOR: &str = "-70";
const AUDIO_SINK: &str = "omabox-speaker";

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut secs, mut json) = (Vec::new(), b"3".to_vec(), false);
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, args.get(i + 1))?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"--json" => json = true,
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "audio", a)),
            _ => {
                let s = s_duration(a, false).filter(|s| {
                    let x = awk_num(s.as_bytes());
                    (0.1..=600.0).contains(&x)
                });
                let Some(s) = s else {
                    return Err(ctx.die(cat!(
                        "audio: how long to listen, 100ms to 10m, like 3s (a bare number is seconds), got ",
                        a
                    )));
                };
                secs = s.into_bytes();
            }
        }
        i += 1;
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    if !BoxMeta::load(&ctx.d).audio {
        return Err(ctx.die(cat!(
            "audio: box '",
            ctx.name,
            "' has no audio: a box gets it at up (omabox up --audio; down first, or up --new --audio)"
        )));
    }
    // In SECONDS and 30 more (finding 356: an ffmpeg 9.0.1 can deadlock), then TERM, and KILL 5 s on.
    let lim = fixed(awk_num(&secs) + 30.0, 1);
    let argv: Vec<Vec<u8>> = [
        b"/usr/bin/timeout".as_slice(),
        b"-k",
        b"5",
        lim.as_bytes(),
        b"/usr/bin/ffmpeg",
        b"-nostdin",
        b"-hide_banner",
        b"-nostats",
        b"-v",
        b"error",
        b"-f",
        b"pulse",
        b"-name",
        b"omabox-audio",
        b"-sample_rate",
        b"48000",
        b"-channels",
        b"2",
        b"-i",
        &cat!(AUDIO_SINK, ".monitor"),
        b"-t",
        &secs,
        b"-af",
        b"asetnsamples=n=4800:p=1,astats=metadata=1:reset=1:measure_perchannel=none:measure_overall=Peak_level+RMS_level,ametadata=mode=print:file=/dev/stdout",
        b"-f",
        b"null",
        b"-",
    ]
    .iter()
    .map(|a| a.to_vec())
    .collect();
    // (`raw=$(box_sys ... </dev/null | head -c 4194304) || die`: the box's stdout read up to 4 MiB,
    // pipefail's status; box_sys's own refusal is said and fails it)
    let (raw, ok) = {
        let Ok((r, w)) = pipe() else {
            return Err(ctx.die(cat!(
                "audio: ffmpeg could not read box '",
                ctx.name,
                "''s sound (omabox log -b ",
                ctx.name,
                " audio)"
            )));
        };
        let reader = std::thread::spawn(move || read_all(r, Some(4_194_304)));
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
        let raw = reader.join().unwrap_or_default();
        (raw, matches!(rc, Ok(0)))
    };
    if !ok {
        return Err(ctx.die(cat!(
            "audio: ffmpeg could not read box '",
            ctx.name,
            "''s sound (omabox log -b ",
            ctx.name,
            " audio)"
        )));
    }
    let raw = crate::shell::subst(&raw);
    let res = Args::new(&["-v"])
        .arg(cat!("floor=", AUDIO_FLOOR))
        .arg("-v")
        .arg(cat!("secs=", secs))
        .arg("-v")
        .arg(cat!("box=", ctx.name))
        .arg(crate::jqprog::AUDIO_AWK)
        .awk(Some(&raw), jq::Err::Inherit)
        .text();
    let has = jq::jq(&["-e", ".levels | length > 0"], Some(&res), jq::Err::Null);
    if !has.ok() {
        return Err(ctx.die(cat!(
            "audio: no levels from box '",
            ctx.name,
            "''s speaker (omabox log -b ",
            ctx.name,
            " audio)"
        )));
    }
    if json {
        return out(cat!(res, "\n"));
    }
    let r = jq::jq(
        &["-r", crate::jqprog::AUDIO_SAY_JQ],
        Some(&res),
        jq::Err::Inherit,
    );
    out(&r.out)?;
    crate::cmd::hyprctl::code(r.rc)
}
