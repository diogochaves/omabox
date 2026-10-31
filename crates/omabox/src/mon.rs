//! A headless box's screens as bin/omabox keeps and places them (#122, #146, #161, #163, #165): the
//! main screen's mode and scale (split_scale, parse_mode, size_text: finding 296), the monitors
//! `monitor add` made (omabox.monitors) and the outputs `output drop` took away (output.dropped),
//! the box's config placing them (omabox_place_monitors, the one place that places), box.json's
//! list of them, and an NVIDIA box's labwc told their windows' sizes (finding 234).
//!
//! A step that fails ends the command the same way in every caller, and a record of box.json's
//! that cannot be written is said and the command goes on (bash's set -e held in some callers and
//! not in others, by where the call sat: finding 442).

use crate::boxes::{box_alive, box_file, lock_file, meta, run_read};
use crate::cat;
use crate::exec::{self, ErrTo, In, Out, Redir, on_box, on_box_io};
use crate::exit::{Ctx, Exit, Res, say};
use crate::hypr::{HYPR_LIMIT, hypr_json};
use crate::jq::{self, Args};
use crate::num::awk_num;
use crate::screen::main_screen;
use crate::shell::{self, strip_nl};
use std::time::Duration;

/// The words of an argv.
pub fn w(v: &[&[u8]]) -> Vec<Vec<u8>> {
    v.iter().map(|a| a.to_vec()).collect()
}

/// printf's `%g` of X, as awk and printf(1) write it.
pub fn fmt_g(x: f64) -> String {
    let mut buf = [0u8; 64];
    // SAFETY: buf has its length given; the format is NUL-ended and takes one double.
    let n = unsafe { libc::snprintf(buf.as_mut_ptr().cast(), buf.len(), c"%g".as_ptr(), x) };
    let n = usize::try_from(n).unwrap_or(0).min(buf.len() - 1);
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

/// `printf '%-Ns'`: V padded with spaces to N bytes (bash counts bytes, not characters).
pub fn pad(v: &[u8], n: usize) -> Vec<u8> {
    let mut o = v.to_vec();
    while o.len() < n {
        o.push(b' ');
    }
    o
}

/// awk's first field of a line (blanks around).
pub fn field1(l: &[u8]) -> &[u8] {
    awk_fields(l).first().copied().unwrap_or_default()
}

/// A line's fields as awk's default FS splits them: runs of blanks (space, tab).
pub fn awk_fields(l: &[u8]) -> Vec<&[u8]> {
    l.split(|c| matches!(c, b' ' | b'\t'))
        .filter(|f| !f.is_empty())
        .collect()
}

/// awk's records of V: its lines, a last one without its newline too.
pub fn records(v: &[u8]) -> Vec<&[u8]> {
    let mut r: Vec<&[u8]> = v.split(|&c| c == b'\n').collect();
    if r.last().is_some_and(|l| l.is_empty()) {
        r.pop();
    }
    r
}

/// `IFS=, read -r -a P <<<"$V"`: V's first line at each comma, a last empty field dropped.
pub fn comma_fields(v: &[u8]) -> Vec<Vec<u8>> {
    let line = v.split(|&c| c == b'\n').next().unwrap_or_default();
    if line.is_empty() {
        return Vec::new();
    }
    let mut f: Vec<Vec<u8>> = line.split(|&c| c == b',').map(<[u8]>::to_vec).collect();
    if line.ends_with(b",") {
        f.pop();
    }
    f
}

/// `awk -v s="$S" 'BEGIN { exit !(s >= 1 && s <= 4) }'` for an S that looks like a number.
fn in_1_4(s: &[u8]) -> bool {
    let x = awk_num(s);
    (1.0..=4.0).contains(&x)
}

/// split_scale WHO SPEC: WxH[@HZ][,scale=S] or host[,scale=S] as "SIZE" and "SCALE" (S 1 to 4, up to
/// 2 decimals, `%g`; none is 1: finding 296). SIZE is not parsed yet (parse_mode).
pub fn split_scale(ctx: &Ctx, who: &str, spec: &[u8]) -> Res<(Vec<u8>, Vec<u8>)> {
    let comma = spec.iter().position(|&c| c == b',');
    let m = spec[..comma.unwrap_or(spec.len())].to_vec();
    let Some(i) = comma else {
        return Ok((m, b"1".to_vec()));
    };
    let rest = &spec[i + 1..];
    if !(rest.starts_with(b"scale=") && !rest.contains(&b',')) {
        return Err(ctx.die(cat!(
            who,
            ": a size is WxH[@HZ][,scale=S] or host[,scale=S], got ",
            spec
        )));
    }
    let sc = &rest[6..];
    if !(shell::re_match(r"^[1-4](\.[0-9]{1,2})?$", sc) && in_1_4(sc)) {
        return Err(ctx.die(cat!(who, ": scale is 1 to 4, up to 2 decimals, got ", sc)));
    }
    Ok((m, fmt_g(awk_num(sc)).into_bytes()))
}

/// size_text SIZE SCALE: ",scale=S" after it when S is not 1.
pub fn size_text(size: &[u8], sc: &[u8]) -> Vec<u8> {
    if sc == b"1" {
        size.to_vec()
    } else {
        cat!(size, ",scale=", sc)
    }
}

/// `$(parse_mode M)`: a screen mode for a headless box, WxH (@60), WxH@HZ, or `host` for the user's
/// focused monitor (a read-only hyprctl on the host), as WxH@HZ with the rate as box_screen prints
/// it. Its refusals end the command.
pub fn parse_mode(ctx: &Ctx, given: &[u8]) -> Res<Vec<u8>> {
    let mut m = given.to_vec();
    if m == b"host" {
        crate::host::host_session_die(ctx)?;
        let (hrc, out) = crate::host::host_query(ctx, crate::host::Query::Monitors, false);
        // (`host_hyprctl ... | jq -r`, pipefail: either one failing fails it)
        let r = jq::tool(
            "jq",
            &["-r", crate::jqprog::HOST_MODE_JQ],
            Some(&out),
            jq::Err::Inherit,
        );
        m = r.text();
        if hrc != 0 || !r.ok() || m.is_empty() {
            return Err(ctx.die("no focused monitor on the host"));
        }
    }
    if !shell::re_match(r"^[1-9][0-9]*x[1-9][0-9]*(@[0-9]+(\.[0-9]+)?)?$", &m) {
        return Err(ctx.die(cat!("a size is WxH, WxH@HZ or host, got ", given)));
    }
    if !m.contains(&b'@') {
        m.extend_from_slice(b"@60");
    }
    let at = m.iter().position(|&c| c == b'@').unwrap_or(m.len());
    let h = awk_num(&m[at + 1..]);
    let h = (h * 100.0 + 0.5).trunc() / 100.0;
    if h <= 0.0 {
        return Err(ctx.die(cat!("a refresh rate is above 0, got ", &m[at + 1..])));
    }
    Ok(cat!(&m[..at], "@", fmt_g(h)))
}

/// `$(main_scale_live)`: the main screen's scale as the box's Hyprland has it (rounded to 2
/// decimals), or nothing while it is away. hypr_json's end, or jq's status, end the command.
pub fn main_scale_live(ctx: &Ctx) -> Res<Vec<u8>> {
    let m = strip_nl(&hypr_json(ctx, &["monitors"])?);
    let prog = format!("{}{}", main_screen(ctx), crate::jqprog::MAIN_SCALE_JQ);
    let r = jq::jq(&["-r", &prog], Some(&m), jq::Err::Inherit);
    if !r.ok() {
        return Err(Exit(u8::try_from(r.rc).unwrap_or(1)));
    }
    Ok(r.text())
}

/// parent_display: an NVIDIA box's parent display (finding 77), labwc's socket in the box's runtime
/// dir by the name share/start-hyprland.sh wrote there (the box's to change: checked).
pub fn parent_display(ctx: &Ctx) -> Option<Vec<u8>> {
    let p = strip_nl(&run_read(&ctx.d, "omabox-parent-display")?);
    let f = ctx.at(&format!("run/{}", String::from_utf8_lossy(&p)));
    let sock = std::fs::metadata(&f).is_ok_and(|m| {
        use std::os::unix::fs::FileTypeExt;
        m.file_type().is_socket()
    });
    (shell::re_match("^wayland-[0-9]+$", &p) && sock && !f.is_symlink()).then_some(p)
}

/// dropped_file: what `output drop` took away (#163), "NAME MODE XxY SCALE" a line, as the file has
/// it.
pub fn dropped_file(ctx: &Ctx) -> Vec<u8> {
    std::fs::read(ctx.at("output.dropped")).unwrap_or_default()
}

/// `$(dropped_rec NAME)`: its line, or nothing.
pub fn dropped_rec(ctx: &Ctx, n: &[u8]) -> Vec<u8> {
    let f = dropped_file(ctx);
    records(&f)
        .into_iter()
        .find(|l| field1(l) == n)
        .map(<[u8]>::to_vec)
        .unwrap_or_default()
}

/// `$(main_away)`: the main screen's line while it is away.
pub fn main_away(ctx: &Ctx) -> Vec<u8> {
    let f = dropped_file(ctx);
    records(&f)
        .into_iter()
        .find(|l| matches!(field1(l), b"HEADLESS-2" | b"WAYLAND-1"))
        .map(<[u8]>::to_vec)
        .unwrap_or_default()
}

/// The lines of the record other than NAME's (`awk -v n=NAME '$1 != n'`), as `$(...)` keeps them.
pub fn dropped_without(ctx: &Ctx, n: &[u8]) -> Vec<u8> {
    let f = dropped_file(ctx);
    let kept: Vec<&[u8]> = records(&f).into_iter().filter(|l| field1(l) != n).collect();
    strip_nl(&kept.join(&b'\n'))
}

/// `$(dropped_names)`: the names in the record, "A, B" (cut -d' ' -f1).
pub fn dropped_names(ctx: &Ctx) -> Vec<u8> {
    let f = dropped_file(ctx);
    let names: Vec<&[u8]> = records(&f)
        .into_iter()
        .map(|l| l.split(|&c| c == b' ').next().unwrap_or_default())
        .collect();
    strip_nl(&names.join(&b", "[..]))
}

/// dropped_set LINES: the record is LINES now (none: no file); false when it could not be.
pub fn dropped_set(ctx: &Ctx, lines: &[u8]) -> bool {
    let f = ctx.at("output.dropped");
    if lines.is_empty() {
        if std::fs::symlink_metadata(&f).is_err() {
            return true;
        }
        // (rm's own words when it cannot)
        return crate::sig::command("rm")
            .args(["-f", "--"])
            .arg(&f)
            .stdin(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
    }
    // (gone on every return: bash left it when the write or the mv failed, finding 443)
    let tmp = cat!(ctx.d, "/output.dropped.tmp");
    let _tmp = shell::Rm(tmp.clone());
    if let Err(e) = std::fs::write(shell::path(&tmp), cat!(lines, "\n")) {
        say(cat!(ctx.label(), crate::shots::cant_write(&tmp, &e)));
        return false;
    }
    crate::shots::mv_f(&tmp, shell::bytes(&f))
}

/// monitors_file: the monitors `monitor add` made, as the box's config reads them.
pub fn monitors_file(ctx: &Ctx) -> Vec<u8> {
    run_read(&ctx.d, "omabox.monitors").unwrap_or_default()
}

/// `{ echo "$REST"; echo "$LINE"; } | grep .`: REST's lines and LINE, empty ones left out.
pub fn with_line(rest: &[u8], line: &[u8]) -> Vec<u8> {
    shell::grep_lines(&cat!(rest, "\n", line))
}

/// monitor_spec SPEC: WxH[@HZ][,scale=S][,right|below|X,Y] as its MODE SCALE PLACE (scale 1-4, up
/// to 2 decimals; right of the last monitor made by default, below it, or X,Y in the layout from
/// 0,0); a refusal said (the caller ends).
pub fn monitor_spec(spec: &[u8]) -> Result<[Vec<u8>; 3], ()> {
    let no = |m: Vec<u8>| -> Result<[Vec<u8>; 3], ()> {
        say(m);
        Err(())
    };
    let parts = comma_fields(spec);
    let Some(first) = parts.first() else {
        return no(b"--monitor: WxH[@HZ][,scale=S][,right|below|X,Y], got nothing".to_vec());
    };
    let mut mode = first.clone();
    if !shell::re_match(
        r"^[1-9][0-9]{1,4}x[1-9][0-9]{1,4}(@[0-9]{1,3}(\.[0-9]{1,3})?)?$",
        &mode,
    ) {
        return no(cat!(
            "--monitor: a size is WxH or WxH@HZ, got ",
            mode,
            " (",
            spec,
            ")"
        ));
    }
    if !mode.contains(&b'@') {
        mode.extend_from_slice(b"@60");
    }
    let (mut scale, mut place, mut x) = (b"1".to_vec(), b"right".to_vec(), Vec::new());
    for part in &parts[1..] {
        if let Some(s) = part.strip_prefix(b"scale=") {
            scale = s.to_vec();
            if !shell::re_match(r"^[1-4](\.[0-9]{1,2})?$", &scale) || !in_1_4(&scale) {
                return no(cat!(
                    "--monitor: scale is 1 to 4, up to 2 decimals, got ",
                    scale,
                    " (",
                    spec,
                    ")"
                ));
            }
        } else if part == b"right" || part == b"below" {
            place = part.clone();
        } else if part == b"left" || part == b"above" {
            return no(cat!(
                "--monitor: the layout starts at 0,0: put this one first with X,Y and the others after it, got ",
                part,
                " (",
                spec,
                ")"
            ));
        } else if shell::re_match("^[0-9]{1,5}$", part) {
            if x.is_empty() {
                x = part.clone();
                place.clear();
            } else {
                place = cat!(x, ",", part);
                x.clear();
            }
        } else {
            return no(cat!(
                "--monitor: scale=S, right, below or X,Y, got ",
                part,
                " (",
                spec,
                ")"
            ));
        }
    }
    if !(x.is_empty() && !place.is_empty()) {
        return no(cat!(
            "--monitor: a position is X,Y, got ",
            x,
            " alone (",
            spec,
            ")"
        ));
    }
    Ok([mode, scale, place])
}

/// monitor_place PLACE: omabox.monitors' PLACE for a SPEC's (right, below, or X,Y as XxY).
pub fn monitor_place(p: &[u8]) -> Vec<u8> {
    if p == b"right" || p == b"below" {
        return p.to_vec();
    }
    match p.iter().position(|&c| c == b',') {
        Some(i) => cat!(&p[..i], "x", &p[i + 1..]),
        None => p.to_vec(),
    }
}

/// labwc_rc: an NVIDIA box's labwc config (#165, finding 234): session.sh's rules, then one per
/// monitor of omabox.monitors sizing its window and putting it at 0,0.
pub fn labwc_rc(ctx: &Ctx) -> Vec<u8> {
    let mut o = cat!(
        "<?xml version=\"1.0\"?>\n<labwc_config>\n  <windowRules>\n",
        "    <windowRule identifier=\"aquamarine\" serverDecoration=\"no\"/>\n",
        "    <windowRule title=\"aquamarine - WAYLAND-1\"><action name=\"Maximize\"/></windowRule>\n"
    );
    let f = monitors_file(ctx);
    for l in records(&f) {
        let r = shell::read_fields(l, 3);
        let (n, m) = (&r[0], &r[1]);
        let re = shell::Re::new("^([0-9]+)x([0-9]+)@");
        let (Some(wd), Some(ht)) = (
            re.as_ref().and_then(|re| re.group(m, 1)),
            re.as_ref().and_then(|re| re.group(m, 2)),
        ) else {
            continue;
        };
        if !shell::re_match("^[A-Za-z0-9_-]+$", n) {
            continue;
        }
        o.extend_from_slice(&cat!(
            "    <windowRule title=\"aquamarine - ",
            n,
            "\"><action name=\"ResizeTo\" width=\"",
            wd,
            "\" height=\"",
            ht,
            "\"/><action name=\"MoveTo\" x=\"0\" y=\"0\"/></windowRule>\n"
        ));
    }
    o.extend_from_slice(b"  </windowRules>\n</labwc_config>\n");
    o
}

/// labwc_apply: the rules in place before a monitor's window maps (the config written in the box's
/// mount namespace, a new file renamed over the old; labwc told to read it again; a round trip to
/// labwc, which it answers only after the signal). Ok(false) for a step that failed; a box_exec
/// that cannot enter ends the command (its words to /dev/null), as bash's die does.
pub fn labwc_apply(ctx: &mut Ctx) -> Res<bool> {
    let Some(parent) = parent_display(ctx) else {
        return Ok(false);
    };
    // `labwc_rc | on_box sh -c ...`: the pipe's on_box in a subshell of its own, whose refusal ends
    // only that.
    let Ok((r, wr)) = exec::pipe() else {
        return Ok(false);
    };
    let feeder = exec::feed(wr, labwc_rc(ctx));
    let ran = on_box_io(
        ctx,
        &w(&[
            b"sh",
            b"-c",
            br#"d=$XDG_RUNTIME_DIR/labwc; [ -d "$d" ] && cat > "$d/rc.xml.new" && mv -f "$d/rc.xml.new" "$d/rc.xml""#,
        ]),
        In::Fd(r),
        Out::Inherit,
        ErrTo::Inherit,
        None,
    );
    let _ = feeder.join();
    if ran.rc != 0 {
        return Ok(false);
    }
    let rc = {
        let _o = Redir::null(1);
        let _e = Redir::null(2);
        exec::box_exec(
            ctx,
            &w(&[b"/usr/bin/labwc", b"--reconfigure"]),
            exec::ExecOpts::default(),
        )?
    };
    if rc != 0 {
        return Ok(false);
    }
    let r = on_box(
        ctx,
        &w(&[
            b"env",
            &cat!("WAYLAND_DISPLAY=", parent),
            b"/usr/bin/wlr-randr",
        ]),
        false,
        Out::Null,
        ErrTo::Null,
    )
    .or_exit()?;
    Ok(r.rc == 0)
}

/// `jq ARGS "$D/box.json" > "$D/box.json.tmp" && mv "$D/box.json.tmp" "$D/box.json"`: whether
/// box.json was written, why not said (jq's or mv's words, or omabox's). The temporary file is gone
/// either way (bash left it when jq or the mv failed: finding 443).
pub fn jq_box_json(ctx: &Ctx, args: Args) -> bool {
    let tmp = cat!(ctx.d, "/box.json.tmp");
    let _tmp = crate::shell::Rm(tmp.clone());
    let f = match std::fs::File::create(shell::path(&tmp)) {
        Ok(f) => f,
        Err(e) => {
            say(cat!(ctx.label(), crate::shots::cant_write(&tmp, &e)));
            return false;
        }
    };
    let r = args
        .arg(cat!(ctx.d, "/box.json"))
        .run_file(jq::Err::Inherit);
    if let Err(e) = std::io::Write::write_all(&mut &f, &r.out) {
        say(cat!(ctx.label(), crate::shots::cant_write(&tmp, &e)));
        return false;
    }
    drop(f);
    r.ok() && crate::shots::mv_f(&tmp, &cat!(ctx.d, "/box.json"))
}

/// monitors_record: box.json's list of every monitor (for ls --json), from Hyprland, under the box's
/// lock as mode writes it; not an interactive box's (the user closes and resizes its windows). Its
/// returns are quiet but for hypr_json's words and a box.json it could not write, said, the command
/// going on.
pub fn monitors_record(ctx: &Ctx) -> Res<()> {
    if meta(&ctx.d, "mode").0 != b"headless" {
        return Ok(());
    }
    let Ok(m) = hypr_json(ctx, &["monitors"]) else {
        return Ok(());
    };
    let m = strip_nl(&m);
    // `up` holds the lock already (UP_LOCKED); anything else takes it.
    let _lock = if !UP_LOCKED.load(std::sync::atomic::Ordering::SeqCst)
        && shell::env_or_empty("UP_LOCKED").is_empty()
    {
        match lock_file(
            &ctx.in_boxes(cat!(".lock-", ctx.name)),
            Duration::from_secs(60),
        ) {
            Some(l) => Some(l),
            None => return Ok(()),
        }
    } else {
        None
    };
    if !box_alive(ctx)? {
        return Ok(());
    }
    if !jq_box_json(
        ctx,
        Args::new(&[])
            .kv("--argjson", "m", &m)
            .arg(crate::jqprog::MONITORS_RECORD_JQ),
    ) {
        say(cat!(
            ctx.label(),
            "box '",
            ctx.name,
            "''s list of monitors was not updated: ls --json may show the old one"
        ));
    }
    Ok(())
}

/// `$(box_layout [LINE])`: the box's layout as it places its monitors (omabox_monitor_layout,
/// share/hyprland.lua), "NAME X Y W H" a monitor, the main screen as `main`; with LINE, as if
/// omabox.monitors had it last. None when the box's config has no such function (an older box).
pub fn box_layout(ctx: &Ctx, line: Option<&[u8]>) -> Option<Vec<u8>> {
    let arg = match line {
        // (lua_str: our own lines only; anything that could end the string refused, said)
        Some(l) if l.iter().any(|&c| matches!(c, b'\'' | b'"' | b'\\' | b'\n')) => {
            say(cat!(
                "unsafe characters in a path for the host Hyprland: ",
                l
            ));
            b"''".to_vec()
        }
        Some(l) => cat!("'", l, "'"),
        None => Vec::new(),
    };
    let r = on_box(
        ctx,
        &w(&[
            b"timeout",
            HYPR_LIMIT.as_bytes(),
            b"hyprctl",
            b"eval",
            &cat!("omabox_monitor_layout(", arg, ")"),
        ]),
        false,
        Out::Capture(None),
        ErrTo::Out,
    );
    let out = shell::subst(&r.out);
    let rest = out.strip_prefix(b"error: omabox-layout:")?;
    let lines: Vec<&[u8]> = rest
        .split(|&c| c == b',' || c == b'\n')
        .filter(|l| shell::grep_dot(l))
        .collect();
    if lines.is_empty() {
        return None;
    }
    Some(lines.join(&b'\n'))
}

/// placed_as MONITORS LAYOUT: every monitor of the layout's rows ("NAME X Y W H", the main screen as
/// `main`) is at its X,Y in MONITORS (Hyprland's JSON).
pub fn placed_as(ctx: &Ctx, m: &[u8], lay: &[u8]) -> bool {
    let prog = format!("{} | .name // empty", main_screen(ctx));
    let main = jq::jq(&["-r", &prog], Some(m), jq::Err::Inherit).text();
    let main = if main.is_empty() {
        b"main".to_vec()
    } else {
        main
    };
    let rows = jq::awk(
        &[
            b"-v".to_vec(),
            cat!("m=", main),
            crate::jqprog::PLACED_ROWS_AWK.as_bytes().to_vec(),
        ],
        Some(lay),
        jq::Err::Inherit,
    )
    .text();
    Args::new(&["-e"])
        .kv("--arg", "rows", &rows)
        .arg(crate::jqprog::PLACED_AS_JQ)
        .run(Some(m), jq::Err::Inherit)
        .ok()
}

/// `$(monitors_place)`: the box's config places the monitors in omabox.monitors as they are now
/// (omabox_place_monitors); the ones that moved, "NAME to X,Y, ...". Err once said why (or when
/// hypr_json's words were), as bash's `return 1`. (Always in `$(...)` or an `||` list: no step of it
/// ends the command.)
pub fn monitors_place(ctx: &Ctx) -> Result<Vec<u8>, ()> {
    let m0 = strip_nl(&hypr_json(ctx, &["monitors"]).map_err(|_| ())?);
    let r = on_box(
        ctx,
        &w(&[
            b"timeout",
            HYPR_LIMIT.as_bytes(),
            b"hyprctl",
            b"eval",
            b"omabox_place_monitors()",
        ]),
        false,
        Out::Capture(None),
        ErrTo::Out,
    );
    let res = shell::subst(&r.out);
    if res != b"ok" {
        say(cat!(
            "placing box '",
            ctx.name,
            "''s monitors failed: ",
            res
        ));
        return Err(());
    }
    // Hyprland moves them on its next tick, and a read right after the eval once still had the old
    // places (finding 260): read until each is where the layout puts it, 1.5 s at most.
    let lay = box_layout(ctx, None).unwrap_or_default();
    let mut m = Vec::new();
    for _ in 0..15 {
        m = strip_nl(&hypr_json(ctx, &["monitors"]).map_err(|_| ())?);
        if lay.is_empty() || placed_as(ctx, &m, &lay) {
            break;
        }
        // (`sleep 0.1`, a foreground child as bash's: a trap's test for an INT to omabox alone)
        crate::trap::sleep(b"0.1");
    }
    let r = Args::new(&["-r"])
        .kv("--argjson", "a", &m0)
        .arg(crate::jqprog::PLACE_MOVED_JQ)
        .run(Some(&m), jq::Err::Inherit);
    if !r.ok() {
        return Err(());
    }
    Ok(r.text())
}

/// An interactive box's screens at the scales its config gives them (omabox_monitor_scales: the main
/// screen's and each view's, from their windows' sizes), SECS at most (#382, finding 519): after a
/// `monitor remove` Hyprland takes the main screen's new scale a while after the place, and a read
/// right after the remove still had the old one (0.825 for 1). True at once for a box whose config
/// has no such function (started by an older omabox). False when the time ran out.
pub fn scales_settled(ctx: &Ctx, secs: u64) -> bool {
    let end = crate::settle::after(secs);
    loop {
        let r = on_box(
            ctx,
            &w(&[
                b"timeout",
                HYPR_LIMIT.as_bytes(),
                b"hyprctl",
                b"eval",
                b"omabox_monitor_scales()",
            ]),
            false,
            Out::Capture(None),
            ErrTo::Out,
        );
        let out = shell::subst(&r.out);
        let Some(rest) = out.strip_prefix(b"error: omabox-scales:") else {
            return true;
        };
        let want = scale_rows(rest);
        if let Ok(m) = hypr_json(ctx, &["monitors"]) {
            let live = jq::jq(
                &["-r", r#".[] | "\(.name) \(.scale)""#],
                Some(&m),
                jq::Err::Inherit,
            )
            .text();
            let scale_of = |n: &[u8]| -> Option<f64> {
                live.split(|&c| c == b'\n').find_map(|l| {
                    let v = l.strip_prefix(n)?.strip_prefix(b" ")?;
                    std::str::from_utf8(v).ok()?.trim().parse().ok()
                })
            };
            if want
                .iter()
                .all(|(n, s)| scale_of(n).is_some_and(|l| (l - s).abs() <= 0.001))
            {
                return true;
            }
        }
        if crate::settle::past(end) {
            return false;
        }
        crate::trap::sleep(b"0.05");
    }
}

/// omabox_monitor_scales()'s rows, "NAME SCALE" comma-separated: each name and its scale (a row that
/// is not one left out).
fn scale_rows(rest: &[u8]) -> Vec<(Vec<u8>, f64)> {
    rest.split(|&c| c == b',' || c == b'\n')
        .filter_map(|row| {
            let i = row.iter().position(|&c| c == b' ')?;
            let s = std::str::from_utf8(&row[i + 1..])
                .ok()?
                .trim()
                .parse()
                .ok()?;
            Some((row[..i].to_vec(), s))
        })
        .collect()
}

/// `[ -n "$(monitors_file | grep . || true)" ]`: the box has monitors of `monitor add`'s.
pub fn has_monitors(ctx: &Ctx) -> bool {
    !shell::grep_lines(&monitors_file(ctx)).is_empty()
}

/// `grep -q "^NAME " <(monitors_file; dropped_file)` and the like: a line of V starting "NAME ".
pub fn line_starts(v: &[u8], name: &[u8]) -> bool {
    let p = cat!(name, " ");
    v.split(|&c| c == b'\n').any(|l| l.starts_with(&p))
}

/// The record without NAME's line (`monitors_file | grep -v "^NAME "`), as `$(...)` keeps it.
pub fn monitors_without(ctx: &Ctx, name: &[u8]) -> Vec<u8> {
    let f = monitors_file(ctx);
    let p = cat!(name, " ");
    let kept: Vec<&[u8]> = records(&f)
        .into_iter()
        .filter(|l| !l.starts_with(&p))
        .collect();
    strip_nl(&kept.join(&b'\n'))
}

/// box_file $D/run/omabox.monitors VALUE.
pub fn monitors_write(ctx: &Ctx, v: &[u8]) -> bool {
    box_file(ctx, &ctx.at("run/omabox.monitors"), v)
}

/// `up` holds the box's lock while it adds its --monitor ones (bash's UP_LOCKED=1): the add takes
/// none of its own.
static UP_LOCKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_up_locked(on: bool) {
    UP_LOCKED.store(on, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    // t_unit_parse_mode's modes, the main screen's scale (finding 296) and size_text.
    #[test]
    fn modes_as_bash() {
        let ctx = Ctx::new(b"up", b"/x".to_vec());
        let pm = |m: &str| {
            crate::exit::muted(|| parse_mode(&ctx, m.as_bytes()))
                .ok()
                .map(|v| String::from_utf8(v).unwrap())
        };
        assert_eq!(
            pm("1920x1080").as_deref(),
            Some("1920x1080@60"),
            "WxH is @60"
        );
        assert_eq!(
            pm("3440x1440@144").as_deref(),
            Some("3440x1440@144"),
            "WxH@HZ kept"
        );
        assert_eq!(
            pm("2560x1440@59.95").as_deref(),
            Some("2560x1440@59.95"),
            "fractional HZ"
        );
        assert_eq!(
            pm("1280x720@60.0").as_deref(),
            Some("1280x720@60"),
            "Hz normalised (@60.0 is @60)"
        );
        assert_eq!(pm("bogus"), None, "junk refused");
        assert_eq!(pm("1x1@0"), None, "0 Hz refused");
        assert_eq!(pm("0x1080"), None, "zero width refused");
        let ss = |who: &str, spec: &str| {
            let (r, said) = crate::exit::heard(|| split_scale(&ctx, who, spec.as_bytes()));
            match r {
                Ok((m, s)) => format!(
                    "{} {}",
                    String::from_utf8(m).unwrap(),
                    String::from_utf8(s).unwrap()
                ),
                Err(_) => said,
            }
        };
        assert_eq!(
            ss("--size", "1920x1080"),
            "1920x1080 1",
            "split_scale: none is 1"
        );
        assert_eq!(
            ss("--size", "3840x2160@120,scale=2.0"),
            "3840x2160@120 2",
            "...scale=2 (2.0 is 2), the rate left to parse_mode"
        );
        assert_eq!(
            ss("--size", "host,scale=1.25"),
            "host 1.25",
            "...two decimals, and host"
        );
        assert!(
            ss("--size", "1920x1080,scale=0.5")
                .contains("scale is 1 to 4, up to 2 decimals, got 0.5"),
            "...below 1 refused, said"
        );
        assert!(
            ss("mode", "1920x1080,scale=5").contains("scale is 1 to 4"),
            "...above 4 refused"
        );
        assert!(
            ss("--size", "1920x1080,right").contains("a size is WxH[@HZ][,scale=S]"),
            "...anything but scale=S refused"
        );
        assert_eq!(
            (
                size_text(b"1920x1080@60", b"1"),
                size_text(b"3840x2160@60", b"2")
            ),
            (b"1920x1080@60".to_vec(), b"3840x2160@60,scale=2".to_vec()),
            "size_text: scale 1 is the size alone, else ,scale=S"
        );
    }

    #[test]
    fn scale_rows_read() {
        assert_eq!(
            scale_rows(b"WAYLAND-1 0.825,WAYLAND-2 0.825\n"),
            vec![
                (b"WAYLAND-1".to_vec(), 0.825),
                (b"WAYLAND-2".to_vec(), 0.825)
            ]
        );
        assert_eq!(
            scale_rows(b"WAYLAND-1 1"),
            vec![(b"WAYLAND-1".to_vec(), 1.0)]
        );
        assert!(scale_rows(b"").is_empty());
        assert!(scale_rows(b"WAYLAND-1 x,nospace").is_empty());
    }

    #[test]
    fn specs_as_bash_reads_them() {
        let ok = |s: &str| {
            monitor_spec(s.as_bytes())
                .ok()
                .map(|v| v.map(|x| String::from_utf8(x).unwrap()))
        };
        assert_eq!(
            ok("800x600"),
            Some(["800x600@60".into(), "1".into(), "right".into()])
        );
        assert_eq!(
            ok("800x600@30,scale=2,below"),
            Some(["800x600@30".into(), "2".into(), "below".into()])
        );
        assert_eq!(
            ok("800x600,10,20"),
            Some(["800x600@60".into(), "1".into(), "10,20".into()])
        );
        assert_eq!(
            ok("800x600,scale=1.50"),
            Some(["800x600@60".into(), "1.50".into(), "right".into()])
        );
        assert_eq!(
            ok("800x600,"),
            Some(["800x600@60".into(), "1".into(), "right".into()])
        );
        assert_eq!(ok(""), None);
        assert_eq!(ok("800x600,10"), None);
        assert_eq!(ok("800x600,left"), None);
        assert_eq!(ok("800x600,scale=4.5"), None);
        assert_eq!(ok("800x600,,"), None);
        assert_eq!(ok("8x600"), None);
    }

    #[test]
    fn fields_as_read_splits_them() {
        assert_eq!(
            comma_fields(b"a,,b"),
            vec![b"a".to_vec(), vec![], b"b".to_vec()]
        );
        assert_eq!(comma_fields(b"a,"), vec![b"a".to_vec()]);
        assert_eq!(comma_fields(b"a,,"), vec![b"a".to_vec(), vec![]]);
        assert_eq!(comma_fields(b",a"), vec![vec![], b"a".to_vec()]);
        assert!(comma_fields(b"").is_empty());
        assert_eq!(comma_fields(b","), vec![Vec::<u8>::new()]);
        assert_eq!(
            comma_fields(b" a , b "),
            vec![b" a ".to_vec(), b" b ".to_vec()]
        );
    }

    #[test]
    fn places_and_sizes() {
        assert_eq!(monitor_place(b"right"), b"right");
        assert_eq!(monitor_place(b"10,20"), b"10x20");
        assert_eq!(size_text(b"1920x1080@60", b"1"), b"1920x1080@60");
        assert_eq!(
            size_text(b"1920x1080@60", b"1.5"),
            b"1920x1080@60,scale=1.5"
        );
        assert_eq!(fmt_g(1.5), "1.5");
        assert_eq!(fmt_g(2.0), "2");
        assert_eq!(fmt_g(59.94), "59.94");
        assert_eq!(fmt_g(1e20), "1e+20");
        assert_eq!(pad("é".as_bytes(), 3), b"\xc3\xa9 ");
    }
}
