//! `omabox output [-b NAME] (drop [OUT] [--for DURATION] [--cycles N] | back [OUT])` (#146, finding
//! 210): a screen gone and back, as a monitor dropping off on wake is. `drop [NAME]` takes the main
//! screen, or the monitor NAME (#163); `back [NAME]` brings back NAME, or every one dropped. Each is
//! kept in $D/output.dropped until it is back; back re-creates it under the same name, mode,
//! position and scale (#200, finding 279).

use crate::args::{optarg, unknown_opt, unknown_word};
use crate::boxes::{box_alive, need_box};
use crate::boxmeta::BoxMeta;
use crate::cat;
use crate::exec::{ErrTo, Out, on_box};
use crate::exit::{Ctx, Res, out, say};
use crate::hypr::{HYPR_LIMIT, hypr_json};
use crate::jq::{self, Args};
use crate::mon::{
    dropped_file, dropped_names, dropped_rec, dropped_set, dropped_without, field1, line_starts,
    main_away, monitors_file, monitors_place, monitors_record, records, w, with_line,
};
use crate::num::{ms_duration, ms_s};
use crate::screen::{box_screen, main_screen};
use crate::select::select_box;
use crate::settle::{after, past};
use crate::shell::{self, strip_nl};
use crate::shellnote::{shell_crash_dirs, shell_crashes, shell_seen, shell_state};
use std::time::Duration;

/// The gaps of --cycles without --for: 50 ms to 2 s in turn (one of them found the crash in #146).
const OUTPUT_GAPS: [i64; 8] = [50, 300, 1000, 2000, 100, 600, 1500, 200];

/// `jq -r PROG <<<"$M"` (its words on stderr), as `$(...)` keeps it.
fn jqr(prog: &str, m: &[u8]) -> Vec<u8> {
    jq::jq(&["-r", prog], Some(m), jq::Err::Inherit).text()
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut act, mut outn) = (Vec::new(), Vec::new(), Vec::new());
    let (mut gap, mut cycles): (Option<i64>, Option<Vec<u8>>) = (None, None);
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
            b"--for" => {
                optarg(ctx, a, v)?;
                let g = ms_duration(&args[i + 1]).filter(|&g| g <= 60000);
                let Some(g) = g else {
                    return Err(ctx.die(cat!(
                        "output: --for takes a duration up to 60s, like 300ms or 2s, got ",
                        args[i + 1]
                    )));
                };
                gap = Some(g);
                i += 1;
            }
            b"--cycles" => {
                optarg(ctx, a, v)?;
                if !shell::re_match("^[1-9][0-9]{0,2}$", &args[i + 1]) {
                    return Err(ctx.die(cat!("output: --cycles takes 1-999, got ", args[i + 1])));
                }
                cycles = Some(args[i + 1].clone());
                i += 1;
            }
            b"-h" | b"--help" => return crate::args::usage("output"),
            b"drop" | b"back" => {
                if !act.is_empty() {
                    return Err(ctx.die("output: drop or back, not both"));
                }
                act = a.clone();
            }
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "output", a)),
            _ => {
                if act.is_empty() {
                    return Err(unknown_word(
                        ctx,
                        "output",
                        "subcommand",
                        a,
                        "(drop or back)",
                        &["drop", "back"],
                    ));
                }
                if !outn.is_empty() {
                    return Err(ctx.die(cat!("output: one output's name, got ", outn, " and ", a)));
                }
                if !shell::re_match("^[A-Za-z0-9_-]{1,32}$", a) {
                    return Err(ctx.die(cat!(
                        "output: not an output's name: ",
                        a,
                        " (omabox monitor list)"
                    )));
                }
                outn = a.clone();
            }
        }
        i += 1;
    }
    if act.is_empty() {
        return Err(
            ctx.die("output: drop or back (omabox output drop --for 300ms: gone, then back)")
        );
    }
    if act != b"drop" && (gap.is_some() || cycles.is_some()) {
        return Err(ctx.die("output back takes no --for or --cycles"));
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    if !BoxMeta::load(&ctx.d).headless() {
        return Err(ctx.die(cat!(
            "output: box '",
            ctx.name,
            "' is interactive: its screens are its windows on your desktop, and a dropped one could not come back (its window closes: the last one ends the box, or asks with confirm-close; a window opened again has another name)"
        )));
    }
    if act == b"back" {
        output_back(ctx, &outn, &mut |l| out(cat!(l, "\n")))?;
        return monitors_record(ctx);
    }
    if gap.is_none() && cycles.is_none() {
        let gone = output_drop(ctx, &outn)?;
        monitors_record(ctx)?;
        // With other monitors one is still there (#161); the main screen is never one of them.
        let left = match hypr_json(ctx, &["monitors"]) {
            Ok(m) => {
                let r = jq::tool(
                    "jq",
                    &["-r", crate::jqprog::OUTPUT_LEFT_JQ],
                    Some(&m),
                    jq::Err::Inherit,
                );
                if r.ok() { r.text() } else { Vec::new() }
            }
            Err(_) => Vec::new(),
        };
        // (The ones dropped before it named too: `output back` alone brings every one back.)
        let f = dropped_file(ctx);
        let before: Vec<&[u8]> = records(&f)
            .into_iter()
            .map(field1)
            .filter(|n| *n != gone.as_slice())
            .collect();
        let before = strip_nl(&before.join(&b", "[..]));
        let bnote = if before.is_empty() {
            Vec::new()
        } else {
            cat!(" (", before, " dropped before)")
        };
        let away = main_away(ctx);
        let away1 = away.split(|&c| c == b' ').next().unwrap_or_default();
        let n = &ctx.name;
        if left.is_empty() {
            say(cat!(
                "box '",
                n,
                "' has no screen now (",
                dropped_names(ctx),
                " dropped): omabox output back -b ",
                n
            ));
        } else if gone == away1 {
            say(cat!(
                "box '",
                n,
                "' has its main screen (",
                gone,
                ") off now, ",
                left,
                " still on",
                bnote,
                ": omabox output back ",
                gone,
                " -b ",
                n
            ));
        } else {
            say(cat!(
                "box '",
                n,
                "' has its monitor ",
                gone,
                " off now, ",
                left,
                " still on",
                bnote,
                ": omabox output back ",
                gone,
                " -b ",
                n
            ));
        }
        return Ok(());
    }
    // Cycles: gone for --for (or gaps from 50 ms to 2 s in turn), back, a second to rebuild. The
    // first shell crash ends them: that is what they look for.
    let sh = BoxMeta::load(&ctx.d).shell;
    let before = shell_crash_dirs(ctx);
    let before = strip_nl(&before);
    let n0 = cat!(before, "\n")
        .split(|&c| c == b'\n')
        .filter(|l| shell::grep_dot(l))
        .count();
    let shown = cycles.unwrap_or(b"1".to_vec());
    // (--cycles 1-999, checked, decimal: `010` is 10; bash's arithmetic read it octal, 8, and ended
    // the command at an 08 in its own words: finding 448)
    let total = crate::num::read(ctx, "--cycles", &shown)?;
    for i in 1..=total {
        let g = gap.unwrap_or(OUTPUT_GAPS[usize::try_from((i - 1) % 8).unwrap_or(0)]);
        let dropped = output_drop(ctx, &outn)?;
        std::thread::sleep(Duration::from_millis(u64::try_from(g).unwrap_or(0)));
        // (`screen=$(output_back ...)`: its lines, as the capture keeps them)
        let mut lines = Vec::new();
        output_back(ctx, &dropped, &mut |l| {
            lines.extend_from_slice(&cat!(l, "\n"));
            Ok(())
        })?;
        let screen = strip_nl(&lines);
        std::thread::sleep(Duration::from_secs(1));
        if !box_alive(ctx)? {
            return Err(ctx.die(cat!(
                "output: box '",
                ctx.name,
                "' went down in cycle ",
                i.to_string(),
                " (its Hyprland: omabox log -b ",
                ctx.name,
                " hyprland)"
            )));
        }
        let n = if sh { shell_crashes(ctx) } else { 0 };
        let head = cat!(
            "cycle ",
            i.to_string(),
            "/",
            shown,
            ": gone for ",
            ms_s(g.to_string()),
            ", back as ",
            screen
        );
        if n > n0 {
            out(cat!(head, "; shell crashed\n"))?;
            shell_seen(ctx);
            // (The shell's own report: not another Quickshell's in the box, #303.)
            let new = newest_new(&before, &shell_crash_dirs(ctx));
            let new = if shell::re_match("^[A-Za-z0-9_.-]+$", &new) {
                cat!(
                    "report: ",
                    ctx.d,
                    "/home/.cache/quickshell/crashes/",
                    new,
                    "/report.txt; "
                )
            } else {
                Vec::new()
            };
            return Err(ctx.die(cat!(
                "output: the shell crashed in cycle ",
                i.to_string(),
                " (",
                new,
                "omabox log -b ",
                ctx.name,
                " shell)"
            )));
        }
        let st = if sh {
            cat!("; shell ", shell_state(ctx))
        } else {
            Vec::new()
        };
        out(cat!(head, st, "\n"))?;
    }
    Ok(())
}

/// `comm -13 <(sort <<<"$BEFORE") <(sort <<<"$NOW") | tail -n 1`: the last (in sort's order) of
/// NOW's lines that BEFORE did not have.
fn newest_new(before: &[u8], now: &[u8]) -> Vec<u8> {
    let sorted = |v: &[u8], here: bool| -> Vec<Vec<u8>> {
        let mut l: Vec<Vec<u8>> = if here {
            cat!(v, "\n")
                .split(|&c| c == b'\n')
                .map(<[u8]>::to_vec)
                .collect::<Vec<_>>()
        } else {
            v.split(|&c| c == b'\n').map(<[u8]>::to_vec).collect()
        };
        l.pop();
        l.sort();
        l
    };
    let (a, b) = (sorted(before, true), sorted(now, false));
    let (mut i, mut j, mut last) = (0, 0, Vec::new());
    while j < b.len() {
        if i < a.len() {
            match a[i].cmp(&b[j]) {
                std::cmp::Ordering::Less => {
                    i += 1;
                    continue;
                }
                std::cmp::Ordering::Equal => {
                    i += 1;
                    j += 1;
                    continue;
                }
                std::cmp::Ordering::Greater => {}
            }
        }
        last = b[j].clone();
        j += 1;
    }
    last
}

/// output_drop [NAME]: NAME (without one, the main screen) kept in $D/output.dropped, removed,
/// gone from `monitors` within 5 s; its name. The main screen is never taken from another monitor
/// (#163): dropping it twice is refused.
pub fn output_drop(ctx: &Ctx, given: &[u8]) -> Res<Vec<u8>> {
    let mut m = strip_nl(&hypr_json(ctx, &["monitors"])?);
    let mut outn = given.to_vec();
    let n = ctx.name.clone();
    if outn.is_empty() {
        let away = main_away(ctx);
        if !away.is_empty() {
            let a = shell::read_fields(&away, 2).remove(0);
            return Err(ctx.die(cat!(
                "output: box '",
                n,
                "''s main screen (",
                a,
                ") is dropped already: omabox output back ",
                a,
                " -b ",
                n
            )));
        }
        let r = jq::jq(
            &["-r", &format!("{}{}", main_screen(ctx), ".name // empty")],
            Some(&m),
            jq::Err::Inherit,
        );
        crate::cmd::hyprctl::code(r.rc)?;
        outn = r.text();
        if outn.is_empty() {
            return Err(ctx.die(cat!(
                "output: box '",
                n,
                "' has no main screen to drop (",
                jqr(crate::jqprog::MON_NAMES_JQ, &m),
                ")"
            )));
        }
    } else if !dropped_rec(ctx, &outn).is_empty() {
        return Err(ctx.die(cat!(
            "output: ",
            outn,
            " is dropped already: omabox output back ",
            outn,
            " -b ",
            n
        )));
    }
    if !shell::re_match("^[A-Za-z0-9_-]+$", &outn) || outn == b"FALLBACK" {
        return Err(ctx.die(cat!(
            "output: not an output to drop: ",
            outn,
            " (Hyprland's stand-in while none is there)"
        )));
    }
    let r = Args::new(&["-r"])
        .kv("--arg", "n", &outn)
        .arg(crate::jqprog::DROP_REC_JQ)
        .run(Some(&m), jq::Err::Inherit);
    crate::cmd::hyprctl::code(r.rc)?;
    let rec = r.text();
    if rec.is_empty() {
        return Err(ctx.die(cat!(
            "output: box '",
            n,
            "' has no monitor ",
            outn,
            " (",
            jqr(crate::jqprog::MON_NAMES_NOFB_JQ, &m),
            ")"
        )));
    }
    let before = strip_nl(&dropped_file(ctx));
    if !dropped_set(ctx, &with_line(&before, &rec)) {
        return Err(ctx.die(cat!(
            "output: could not record ",
            outn,
            " in ",
            ctx.d,
            "/output.dropped"
        )));
    }
    let r = on_box(
        ctx,
        &w(&[
            b"timeout",
            HYPR_LIMIT.as_bytes(),
            b"hyprctl",
            b"output",
            b"remove",
            &outn,
        ]),
        false,
        Out::Capture(None),
        ErrTo::Out,
    );
    let res = shell::subst(&r.out);
    if res != b"ok" {
        dropped_set(ctx, &before);
        return Err(ctx.die(cat!("output: removing ", outn, " failed: ", res)));
    }
    let end = after(5);
    loop {
        if let Ok(x) = hypr_json(ctx, &["monitors"]) {
            m = strip_nl(&x);
            if !listed(&m, &outn) {
                break;
            }
        }
        if past(end) {
            return Err(ctx.die(cat!(
                "output: ",
                outn,
                " still listed 5 s after its removal"
            )));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(outn)
}

/// `jq -e --arg n NAME 'any(.[]; .name == $n)' <<<"$M" >/dev/null`.
pub fn listed(m: &[u8], name: &[u8]) -> bool {
    Args::new(&["-e"])
        .kv("--arg", "n", name)
        .arg(crate::jqprog::ANY_NAME_JQ)
        .run(Some(m), jq::Err::Inherit)
        .ok()
}

/// output_back [NAME]: NAME, or every one dropped (the main screen first, then in the order
/// dropped), back as it was; "NAME MODE" for each, to SINK as each is.
pub fn output_back(ctx: &Ctx, want: &[u8], sink: &mut dyn FnMut(&[u8]) -> Res<()>) -> Res<()> {
    let recs = strip_nl(&dropped_file(ctx));
    let n = ctx.name.clone();
    if recs.is_empty() {
        if !want.is_empty() {
            return Err(ctx.die(cat!(
                "output: ",
                want,
                " is not dropped: nothing is (omabox monitor list -b ",
                n,
                ")"
            )));
        }
        let screen = box_screen(ctx)?;
        if !screen.is_empty() {
            return Err(ctx.die(cat!(
                "output: box '",
                n,
                "' has its screen (",
                screen,
                "): nothing to bring back"
            )));
        }
        return Err(ctx.die(cat!(
            "output: box '",
            n,
            "' has no screen and no record of the one it had: omabox down ",
            n
        )));
    }
    if !want.is_empty() {
        let rec = dropped_rec(ctx, want);
        if rec.is_empty() {
            return Err(ctx.die(cat!(
                "output: ",
                want,
                " is not dropped (dropped: ",
                dropped_names(ctx),
                ")"
            )));
        }
        let l = output_back_one(ctx, &rec)?;
        return sink(&l);
    }
    let main = main_away(ctx);
    if !main.is_empty() {
        let l = output_back_one(ctx, &main)?;
        sink(&l)?;
    }
    // (`while read -r rec`: each line, its blanks at either end off)
    for l in recs.split(|&c| c == b'\n') {
        let rec = trim_blanks(l);
        if rec.is_empty() || rec == main.as_slice() {
            continue;
        }
        let l = output_back_one(ctx, rec)?;
        sink(&l)?;
    }
    Ok(())
}

/// `read -r V`'s trimming: blanks (space, tab) off both ends.
fn trim_blanks(l: &[u8]) -> &[u8] {
    let b = |c: &u8| matches!(c, b' ' | b'\t');
    let s = l.iter().position(|c| !b(c)).unwrap_or(l.len());
    let e = l.iter().rposition(|c| !b(c)).map_or(s, |i| i + 1);
    &l[s..e]
}

/// output_back_one "NAME MODE XxY SCALE": that output made again under its name, no longer dropped
/// whatever mode it comes up at (#200, finding 279), placed (output_place_one), done when `monitors`
/// has it at that mode; on NVIDIA set again each second until it holds (#200). Its line.
fn output_back_one(ctx: &Ctx, rec: &[u8]) -> Res<Vec<u8>> {
    let f = shell::read_fields(rec, 4);
    let (o, mode, pos, scale) = (&f[0], &f[1], &f[2], &f[3]);
    if !(shell::re_match("^[A-Za-z0-9_-]+$", o)
        && shell::re_match(r"^[0-9]+x[0-9]+@[0-9.]+$", mode)
        && shell::re_match("^-?[0-9]+x-?[0-9]+$", pos)
        && shell::re_match("^[0-9.]+$", scale))
    {
        return Err(ctx.die(cat!(
            "output: ",
            ctx.d,
            "/output.dropped has a line omabox did not write: ",
            rec
        )));
    }
    let kind: &[u8] = if BoxMeta::load(&ctx.d).wayland_screen {
        b"wayland"
    } else {
        b"headless"
    };
    let r = on_box(
        ctx,
        &w(&[
            b"timeout",
            HYPR_LIMIT.as_bytes(),
            b"hyprctl",
            b"output",
            b"create",
            kind,
            o,
        ]),
        false,
        Out::Capture(None),
        ErrTo::Out,
    );
    let mut res = shell::subst(&r.out);
    if res != b"ok" {
        return Err(ctx.die(cat!("output: creating ", o, " again failed: ", res)));
    }
    if !dropped_set(ctx, &dropped_without(ctx, o)) {
        return Err(ctx.die(cat!(
            "output: ",
            o,
            " is back, but ",
            ctx.d,
            "/output.dropped still has it"
        )));
    }
    if !output_place_one(ctx, o, mode, pos, scale, &mut res) {
        return Err(ctx.die(cat!(
            "output: ",
            o,
            " is back, but placing it failed: ",
            res
        )));
    }
    let (end, mut again) = (after(10), after(1));
    let want = cat!(o, " ", mode);
    let mut line = Vec::new();
    loop {
        if let Ok(m) = hypr_json(ctx, &["monitors"]) {
            let r = Args::new(&["-r"])
                .kv("--arg", "n", o)
                .arg(crate::jqprog::BACK_LINE_JQ)
                .run(Some(&strip_nl(&m)), jq::Err::Inherit);
            // (`line=$(jq ...)`: set whatever jq's status, which then ends the test)
            line = r.text();
            if r.ok() && line == want {
                break;
            }
        }
        if past(end) {
            let l = if line.is_empty() {
                b"nothing".to_vec()
            } else {
                line.clone()
            };
            return Err(ctx.die(cat!(
                "output: ",
                o,
                " is back, but as ",
                l,
                ", not ",
                mode,
                " (omabox monitor list -b ",
                ctx.name,
                ")"
            )));
        }
        if kind == b"wayland" && past(again) {
            output_place_one(ctx, o, mode, pos, scale, &mut res);
            again = after(1);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(line)
}

/// output_place_one NAME MODE XxY SCALE: a monitor of omabox.monitors placed by the box's config,
/// the main screen (or one made by hand) by its line; a failure's reason in RES (the caller's).
fn output_place_one(
    ctx: &Ctx,
    o: &[u8],
    mode: &[u8],
    pos: &[u8],
    scale: &[u8],
    res: &mut Vec<u8>,
) -> bool {
    if line_starts(&monitors_file(ctx), o) {
        res.clear();
        return monitors_place(ctx).is_ok();
    }
    let lua = cat!(
        "hl.monitor({ output = '",
        o,
        "', mode = '",
        mode,
        "', position = '",
        pos,
        "', scale = ",
        scale,
        " })"
    );
    let r = on_box(
        ctx,
        &w(&[b"timeout", HYPR_LIMIT.as_bytes(), b"hyprctl", b"eval", &lua]),
        false,
        Out::Capture(None),
        ErrTo::Out,
    );
    *res = shell::subst(&r.out);
    res == b"ok"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_crash_as_comm_finds_it() {
        assert_eq!(newest_new(b"", b"a\nb\n"), b"b");
        assert_eq!(newest_new(b"a", b"a\nb\n"), b"b");
        assert_eq!(newest_new(b"a\nb", b"a\nb\n"), b"");
        assert_eq!(newest_new(b"b", b"a\nb\nc\n"), b"c");
        assert_eq!(newest_new(b"c", b"a\nb\nc\n"), b"b");
    }

    #[test]
    fn read_trims() {
        assert_eq!(trim_blanks(b"  a b \t"), b"a b");
        assert_eq!(trim_blanks(b"   "), b"");
    }
}
