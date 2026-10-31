//! `omabox shot [-b NAME] [-o FILE | FILE] [--window SEL | --active] [-g "X,Y WxH"] [--fit N | --zoom
//! N] [--no-pointer] [--json]`, with `--changed` (#145, finding 243) and `--burst N` (#132, finding
//! 212): the box's screen, a region or a window's own pixels (finding 81) as a PNG, grim in the box
//! writing to us. Every shot is recorded for `--in` (shots.tsv); a whole one is what a later
//! --changed compares with.

use crate::args::{optarg, unknown_opt};
use crate::boxes::{box_alive, meta, need_box};
use crate::cat;
use crate::exec::{ErrTo, Out, dev_null, on_box};
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::hypr::{hung_text, hypr_answers, hypr_json};
use crate::jail::in_jail;
use crate::jq::{self, Args};
use crate::jqprog::JQ_MON;
use crate::num::{awk_num, fixed, ms_duration, ms_s};
use crate::screen::{box_screen, extent, stamp, tsv_last, win_buf_geom, win_scale};
use crate::select::select_box;
use crate::settle::{Settle, cursor_now, cursor_rect, geom_parse, kill_tree, now_ms};
use crate::shell::{self, dec, strip_nl};
use crate::shellnote::shell_note;
use crate::shots::{
    caret_shaped, diff_box, diff_parts, fd_file, geom_clip, png_size, ratio, shot_fit_default,
    shot_path, shot_record, shot_ref_save, to_image_px, two_files, zero_note,
};
use crate::win::{box_windows, jq_r, win_desc, win_select};
use std::io::{BufRead, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

pub const BOX_BURST: &str = "/opt/omabox/bin/omabox-burst";

/// `command -v NAME`: an executable file of that name on PATH.
pub fn on_path(name: &str) -> bool {
    shell::env_or_empty("PATH").split(|&c| c == b':').any(|d| {
        let d: &[u8] = if d.is_empty() { b"." } else { d };
        let p = cat!(d, "/", name);
        crate::exec::executable(&p) && shell::path(&p).is_file()
    })
}

/// `$(... | cut -c1-10)` of `date +%H%M%S-%N`: the local time to the ms, for a default name.
pub fn hms_ms() -> Vec<u8> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let t = libc::time_t::try_from(now.as_secs()).unwrap_or(0);
    // SAFETY: a zeroed tm is filled in by localtime_r from a time of ours.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm
    };
    format!(
        "{:02}{:02}{:02}-{:03}",
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
        now.subsec_millis()
    )
    .into_bytes()
}

/// V as a number (decimal, whatever its zeros); one that is none ends the command, said, 1 (finding
/// 448: bash's arithmetic read a 0 in front as octal and ended it at an 08 in its own words).
fn bi(v: &[u8]) -> Res<i64> {
    dec(v).ok_or_else(|| {
        crate::exit::say(cat!("shot: cannot read '", v, "' as a number"));
        Exit(1)
    })
}

fn n(v: i64) -> Vec<u8> {
    v.to_string().into_bytes()
}

/// `${V%x*}`/`${V#*x}` and the like: before the last SEP, after the first.
fn split_at(v: &[u8], sep: u8) -> (Vec<u8>, Vec<u8>) {
    let a = v
        .iter()
        .rposition(|&c| c == sep)
        .map_or(v.to_vec(), |i| v[..i].to_vec());
    let b = v
        .iter()
        .position(|&c| c == sep)
        .map_or(v.to_vec(), |i| v[i + 1..].to_vec());
    (a, b)
}

/// What one shot is, as cmd_shot's locals hold it.
#[derive(Default)]
struct Shot {
    out: Vec<u8>,
    file: Vec<u8>,
    /// The folder OUT and FILE are in, held (shot_path): FILE made and renamed OUT only there.
    dir: Option<crate::shots::OutDir>,
    tofd: Vec<u8>,
    zoom: Vec<u8>,
    fit: Vec<u8>,
    monitor: Vec<u8>,
    json: bool,
    changed: bool,
    /// g: the region asked for (-g, --monitor), "X Y W H" as its words.
    g: Option<[Vec<u8>; 4]>,
    gx: Vec<u8>,
    gy: Vec<u8>,
    lw: Vec<u8>,
    lh: Vec<u8>,
    ww: Vec<u8>,
    wh: Vec<u8>,
    win: Vec<u8>,
    where_: Vec<u8>,
    grim: Vec<Vec<u8>>,
    masks: Vec<Vec<u8>>,
    sref: Option<[Vec<u8>; 7]>,
    /// A screen shot's desktop, being read (desk_snap) for layouts.tsv (#333).
    snap: Option<std::thread::JoinHandle<Vec<u8>>>,
}

impl Shot {
    /// layout_save for this shot, recorded in shots.tsv as PATH (FILE: its other name): the
    /// desktop read beside the capture and the pointer now. Window shots have none.
    fn layout_save(&mut self, ctx: &Ctx, path: &[u8], file: &[u8]) {
        let Some(t) = self.snap.take() else {
            return;
        };
        let snap = t.join().unwrap_or_default();
        let ptr = crate::exit::muted(|| cursor_now(ctx).ok().unwrap_or_default());
        crate::shotcmp::layout_save(ctx, path, file, &ptr, &snap);
    }

    /// shot_said PATH X Y W H IW IH [RESULT SINCE WHY]: what shot prints on stdout: the image's
    /// path, or with --json (#273, audit J2) what it is.
    fn said(&self, v: [&[u8]; 7], changed: [&[u8]; 3]) -> Res<()> {
        if !self.json {
            if !v[0].is_empty() {
                out(cat!(v[0], "\n"))?;
            }
            return Ok(());
        }
        let kind = if !self.win.is_empty() {
            "window"
        } else if !self.monitor.is_empty() {
            "monitor"
        } else if self.g.is_some() {
            "region"
        } else {
            "screen"
        };
        let ch: &[u8] = if self.changed { b"1" } else { b"0" };
        let win: &[u8] = if self.win.is_empty() {
            b"null"
        } else {
            &self.win
        };
        let r = Args::new(&["-nc"])
            .kv("--arg", "p", v[0])
            .kv("--arg", "k", kind)
            .kv("--arg", "x", v[1])
            .kv("--arg", "y", v[2])
            .kv("--arg", "w", v[3])
            .kv("--arg", "h", v[4])
            .kv("--arg", "iw", v[5])
            .kv("--arg", "ih", v[6])
            .kv("--argjson", "win", win)
            .kv("--arg", "mon", &self.monitor)
            .kv("--arg", "fit", &self.fit)
            .kv("--arg", "zoom", &self.zoom)
            .kv("--argjson", "ch", ch)
            .kv("--arg", "res", changed[0])
            .kv("--arg", "since", changed[1])
            .kv("--arg", "why", changed[2])
            .arg(crate::jqprog::SHOT_SAID_JQ)
            .run(None, jq::Err::Inherit);
        out(&r.out)?;
        crate::cmd::hyprctl::code(r.rc)
    }
}

/// The note for a shot named as another image format (#346): shot writes PNG only, whatever the
/// name, and an agent that asked for x.ppm went on to read raw pixels from a PNG.
fn png_named(out: &[u8]) -> Option<Vec<u8>> {
    const OTHER: [&str; 17] = [
        "ppm", "pgm", "pbm", "pnm", "pam", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff",
        "avif", "jxl", "qoi", "heic", "raw",
    ];
    let name = shell::basename(out);
    let dot = name.iter().rposition(|&c| c == b'.').filter(|&i| i > 0)?;
    let ext = &name[dot + 1..];
    OTHER
        .iter()
        .any(|o| ext.eq_ignore_ascii_case(o.as_bytes()))
        .then(|| {
            cat!(
                "shot: ",
                out,
                " holds a PNG (shot writes PNG only, whatever the name)"
            )
        })
}

/// The capture's grab, run beside this command: grim in the box (through magick with --zoom), its
/// words appended to ERR; the pids that run it, for a kill.
struct Grab {
    ctx: Ctx,
    grim: Vec<Vec<u8>>,
    zoom: Vec<u8>,
    err: Vec<u8>,
    pid: Arc<AtomicI32>,
    mpid: Arc<AtomicI32>,
    stop: Arc<AtomicBool>,
}

impl Grab {
    fn errfile(&self) -> Option<std::fs::File> {
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(shell::path(&self.err))
            .ok()
    }

    /// grab > TO: its status (the pipeline's, under pipefail).
    fn grab(&self, to: OwnedFd) -> i32 {
        if self.stop.load(Ordering::SeqCst) {
            return 1;
        }
        let Some(ef) = self.errfile() else { return 1 };
        let mut a = vec![b"timeout".to_vec(), b"10".to_vec()];
        a.extend(self.grim.iter().cloned());
        a.push(b"-".to_vec());
        if self.zoom.is_empty() {
            let r = crate::exec::on_box_bg(
                &self.ctx,
                &a,
                crate::exec::In::Inherit,
                Out::Fd(to),
                ErrTo::Fd(ef.as_raw_fd()),
                Some(self.pid.clone()),
            );
            return r.rc;
        }
        let Ok((pr, pw)) = crate::exec::pipe() else {
            return 1;
        };
        let Ok(ef2) = ef.try_clone() else { return 1 };
        let mut mc = crate::sig::command("magick");
        mc.arg("png:-")
            .arg("-scale")
            .arg(format!("{}00%", String::from_utf8_lossy(&self.zoom)))
            .args(["-define", "png:compression-level=1", "png:-"])
            .stdin(std::process::Stdio::from(pr))
            .stdout(std::process::Stdio::from(to))
            .stderr(std::process::Stdio::from(ef2));
        crate::exec::background(&mut mc);
        let m = mc.spawn();
        let mut m = match m {
            Ok(m) => m,
            Err(_) => return 127,
        };
        self.mpid
            .store(i32::try_from(m.id()).unwrap_or(0), Ordering::SeqCst);
        let r = crate::exec::on_box_bg(
            &self.ctx,
            &a,
            crate::exec::In::Inherit,
            Out::Fd(pw),
            ErrTo::Fd(ef.as_raw_fd()),
            Some(self.pid.clone()),
        );
        let mrc = m.wait().map(|s| crate::exec::status(s).0).unwrap_or(1);
        if mrc != 0 { mrc } else { r.rc }
    }

    /// A window's frame copy that Hyprland failed (finding 605): grim 1.5.0 dies of SIGSEGV then
    /// (its failed handler reads the output's name, and a window capture has no output); a later
    /// grim ends 1 saying "failed to copy capture source".
    fn copy_failed(&self, rc: i32) -> bool {
        rc == 139
            || (rc == 1
                && std::fs::read(shell::path(&self.err))
                    .is_ok_and(|e| e.windows(29).any(|w| w == b"failed to copy capture source")))
    }

    /// GRAB of a window, taken again while its copy failed (finding 605: Hyprland fails a window's
    /// copy when its buffer changes size during it, as when the window was just resized or shown),
    /// up to 3 more times 100 ms apart; each failed try's words dropped. Its status.
    fn win_retried(&self, grab: impl Fn() -> i32) -> i32 {
        let mut rc = grab();
        for _ in 0..3 {
            if !self.copy_failed(rc) || self.stop.load(Ordering::SeqCst) {
                break;
            }
            let _ = std::fs::File::create(shell::path(&self.err));
            std::thread::sleep(Duration::from_millis(100));
            rc = grab();
        }
        rc
    }
}

/// `exit $?` under set -e: a status that is not 0 ends the command with it.
fn sh_e(rc: i32) -> Res<()> {
    crate::cmd::hyprctl::code(rc)
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut geom, mut sels, mut as_) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut burst, mut every, mut since): (Vec<u8>, Option<i64>, Vec<u8>) =
        (Vec::new(), None, Vec::new());
    let (mut sheet, mut bdiff, mut nopointer) = (false, false, false);
    let mut after: Vec<Vec<u8>> = Vec::new();
    let mut s = Shot::default();
    let mut masks_st = Settle::default();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let v = args.get(i + 1);
        let val = || args[i + 1].clone();
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, v)?;
                name = val();
                i += 1;
            }
            b"-o" | b"--output" => {
                optarg(ctx, a, v)?;
                two_files(ctx, "shot", &s.out, &val())?;
                s.out = val();
                i += 1;
            }
            b"--no-pointer" => nopointer = true,
            b"--json" => s.json = true,
            // A jailed caller's omabox (relay_shot): the image goes to its file, passed as fd N.
            b"--to-fd" => {
                optarg(ctx, a, v)?;
                if !shell::re_match("^[3-9]$", &val()) {
                    return Err(ctx.die(cat!("shot: --to-fd takes 3-9, got ", val())));
                }
                s.tofd = val();
                i += 1;
            }
            b"--as" => {
                optarg(ctx, a, v)?;
                as_ = val();
                i += 1;
            }
            b"-g" => {
                optarg(ctx, a, v)?;
                geom = val();
                i += 1;
            }
            b"-w" | b"--window" => {
                optarg(ctx, a, v)?;
                sels.push(val());
                i += 1;
            }
            b"--active" => sels.push(b"active".to_vec()),
            b"--monitor" => {
                optarg(ctx, a, v)?;
                if !shell::re_match("^[A-Za-z0-9_-]{1,32}$", &val()) {
                    return Err(ctx.die(cat!(
                        "shot: --monitor takes a monitor's name (omabox monitor list), got ",
                        val()
                    )));
                }
                s.monitor = val();
                i += 1;
            }
            b"--fit" => {
                optarg(ctx, a, v)?;
                if !shell::re_match("^(0|[1-9][0-9]{1,4})$", &val()) {
                    return Err(ctx.die(cat!(
                        "shot: --fit takes the longest side in pixels (10-99999; 0: full size), got ",
                        val()
                    )));
                }
                s.fit = val();
                i += 1;
            }
            b"--zoom" => {
                optarg(ctx, a, v)?;
                if !shell::re_match("^([2-9]|1[0-6])$", &val()) {
                    return Err(ctx.die(cat!(
                        "shot: --zoom takes 2-16 (each pixel N x N), got ",
                        val()
                    )));
                }
                s.zoom = val();
                i += 1;
            }
            b"--burst" => {
                optarg(ctx, a, v)?;
                if !shell::re_match("^([2-9]|[1-9][0-9]|1[0-9][0-9]|200)$", &val()) {
                    return Err(ctx.die(cat!("shot: --burst takes 2-200 frames, got ", val())));
                }
                burst = val();
                i += 1;
            }
            // --every 0: back to back (#191, finding 249: 100 ms apart by default).
            b"--every" => {
                optarg(ctx, a, v)?;
                if shell::re_match("^0+(ms|s)?$", &val()) {
                    every = Some(0);
                } else {
                    match ms_duration(&val()) {
                        Some(e) if e <= 60_000 => every = Some(e),
                        _ => {
                            return Err(ctx.die(cat!(
                                "shot: --every takes a duration up to 60s, like 100ms (0: back to back), got ",
                                val()
                            )));
                        }
                    }
                }
                i += 1;
            }
            b"--sheet" => sheet = true,
            b"--diff" => bdiff = true,
            b"--changed" => s.changed = true,
            b"--since" => {
                optarg(ctx, a, v)?;
                since = val();
                s.changed = true;
                i += 1;
            }
            b"--ignore" => {
                optarg(ctx, a, v)?;
                let m = masks_st.mask_arg(ctx, &val())?;
                s.masks.push(m);
                i += 1;
            }
            b"--after" => {
                if !(v.is_some_and(|v| v == b"--") && args.len() >= i + 3) {
                    return Err(ctx.die(
                        "shot: --after -- ACTION... (an omabox command: keys super+space, click X Y, run -d -- CMD)",
                    ));
                }
                after = args[i + 2..].to_vec();
                break;
            }
            w if w.starts_with(b"-") => return Err(unknown_opt(ctx, "shot", a)),
            w => {
                two_files(ctx, "shot", &s.out, w)?;
                s.out = w.to_vec();
            }
        }
        i += 1;
    }
    if !geom.is_empty() {
        match geom_parse(&geom) {
            Some(g) => s.g = Some(g),
            None => {
                return Err(ctx.die(cat!("shot: -g is \"X,Y WxH\" (or X,Y,W,H), got ", geom)));
            }
        }
    }
    // No --fit: the shot-fit setting's (or $OMABOX_SHOT_FIT's), wherever --fit could go.
    if s.fit.is_empty() && !s.changed && s.zoom.is_empty() {
        s.fit = shot_fit_default().map_err(|()| ctx.refused())?;
    }
    if s.fit == b"0" {
        s.fit.clear();
    }
    if !burst.is_empty() {
        if !s.zoom.is_empty() {
            return Err(ctx.die("shot: --burst or --zoom, not both"));
        }
        if s.json {
            return Err(ctx.die(
                "shot: --json goes with one shot, not --burst (a line each frame: path and time)",
            ));
        }
        let first = after.first().map(Vec::as_slice).unwrap_or_default();
        if !matches!(
            first,
            b"" | b"keys"
                | b"click"
                | b"scroll"
                | b"drag"
                | b"pointer"
                | b"run"
                | b"hyprctl"
                | b"lua"
                | b"output"
                | b"monitor"
                | b"mode"
                | b"window"
                | b"restart-shell"
        ) && !(first == b"tray" && crate::cmd::tray::acts(&after[1..]))
        {
            let note = if first == b"tray" {
                crate::cmd::tray::LISTING_NOTE
            } else {
                ""
            };
            return Err(ctx.die(cat!(
                "shot --after: an omabox command that acts (keys, click, scroll, drag, pointer, run, hyprctl, lua, output, monitor, mode, window, tray menu|activate|click, restart-shell), got ",
                first,
                note
            )));
        }
        // (Only the action's own words, before a `--`: `run -d -- grep -b x` passes its -b to grep.)
        let own: Vec<&Vec<u8>> = after.iter().take_while(|w| *w != b"--").collect();
        if own.iter().any(|w| *w == b"-b" || *w == b"--box") {
            return Err(ctx.die("shot --after: the action runs on the burst's box (no -b)"));
        }
        // shot is on the skill's allowed-tools (#338, finding 621): its action reads no host file.
        if own
            .iter()
            .any(|w| *w == b"--env-file" || w.starts_with(b"--env-file="))
        {
            return Err(ctx.die(
                "shot --after: the action takes no --env-file (a host file: start the app with it first, or --env KEY=VAL)",
            ));
        }
    } else if !(every.is_none() && !sheet && !bdiff && after.is_empty()) {
        return Err(ctx.die("shot: --every, --sheet, --diff and --after go with --burst N"));
    }
    // --changed (#145, finding 243): the whole window or screen is compared, and the crop is its own.
    if s.changed {
        if s.g.is_some()
            || !s.monitor.is_empty()
            || !s.zoom.is_empty()
            || !s.fit.is_empty()
            || !burst.is_empty()
        {
            return Err(ctx.die(
                "shot: --changed crops by itself: not with -g, --monitor, --zoom, --fit or --burst",
            ));
        }
        if !on_path("magick") {
            ctx.parsed();
            return Err(ctx.die(
                "shot: --changed needs ImageMagick's magick (package imagemagick, part of Omarchy)",
            ));
        }
    } else if !s.masks.is_empty() {
        return Err(ctx.die("shot: --ignore goes with --changed (and wait still)"));
    }
    // --zoom (#133): a small crop with each pixel N x N (ImageMagick's -scale repeats pixels).
    if !s.zoom.is_empty() {
        if !s.fit.is_empty() {
            return Err(ctx.die("shot: --zoom or --fit, not both"));
        }
        if s.g.is_none() {
            return Err(
                ctx.die("shot: --zoom goes with -g, a small crop (shot -g \"X,Y 40x30\" --zoom 8)")
            );
        }
        if !on_path("magick") {
            ctx.parsed();
            return Err(ctx.die(
                "shot: --zoom needs ImageMagick's magick (package imagemagick, part of Omarchy)",
            ));
        }
    }
    // A path that cannot be written is said so before anything is captured (issue #63).
    ctx.parsed();
    if !s.tofd.is_empty() {
        if as_.is_empty() || !s.out.is_empty() {
            return Err(ctx.die("shot: --to-fd goes with --as NAME, without -o"));
        }
        fd_file(ctx, &s.tofd, true, "shot")?;
        s.out = as_.clone();
        s.file = cat!("/dev/fd/", s.tofd);
    } else {
        if in_jail() {
            return Err(ctx.die(
                "shot: a jailed agent's shot is written by its own omabox (--to-fd), never at a path here",
            ));
        }
        if !s.out.is_empty() && burst.is_empty() {
            let (o, f, d) = shot_path(ctx, &s.out)?;
            (s.out, s.file, s.dir) = (o, f, Some(d));
        }
    }
    if burst.is_empty()
        && let Some(m) = png_named(&s.out)
    {
        say(m);
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    shell_note(ctx)?;
    // --monitor (#122): that monitor's part of the layout, as -g would take it.
    if !s.monitor.is_empty() {
        if s.g.is_some() || !sels.is_empty() {
            return Err(ctx.die("shot: --monitor, -g or --window: one of them"));
        }
        let mons = strip_nl(&hypr_json(ctx, &["monitors"])?);
        let r = Args::new(&["-r"])
            .kv("--arg", "n", &s.monitor)
            .arg(format!("{JQ_MON}{}", crate::jqprog::SHOT_MON_JQ))
            .run(Some(&mons), jq::Err::Inherit);
        sh_e(r.rc)?;
        let g = r.text();
        if g.is_empty() {
            let names = jq_r(crate::jqprog::MON_NAMES_JQ, &mons);
            return Err(ctx.die(cat!(
                "shot: box '",
                ctx.name,
                "' has no monitor ",
                s.monitor,
                " (",
                names,
                ")"
            )));
        }
        if shell::re_match(r"^[-0-9]+\ [-0-9]+\ (0\ |[0-9]+\ 0$)", &g) {
            let one = Args::new(&["-c"])
                .kv("--arg", "n", &s.monitor)
                .arg(crate::jqprog::MON_NAMED_JQ)
                .run(Some(&mons), jq::Err::Inherit)
                .text();
            let zn = zero_note(&one, &meta(&ctx.d, "wayland_screen").0, None);
            let m = if zn.is_empty() {
                cat!("monitor ", s.monitor, " is listed 0x0: nothing to take")
            } else {
                zn
            };
            return Err(ctx.die(cat!("shot: ", m)));
        }
        let f = shell::read_fields(&g, 4);
        s.g = Some([f[0].clone(), f[1].clone(), f[2].clone(), f[3].clone()]);
    }
    // --since FILE: a whole shot of this box, unchanged, to compare with.
    if !since.is_empty() {
        if in_jail() {
            return Err(ctx.die("shot --since: a jailed agent's paths are never opened here"));
        }
        let sf = crate::repo::readlink_f(&since).filter(|f| shell::path(f).is_file());
        let Some(sf) = sf else {
            return Err(ctx.die(cat!("shot --since: no such file: ", since)));
        };
        let rec = tsv_last(&ctx.at("shots.tsv"), &sf);
        if rec.is_empty() {
            return Err(ctx.die(cat!(
                "shot --since: ",
                since,
                " is not a shot of box '",
                ctx.name,
                "' (take one with omabox shot)"
            )));
        }
        let r = shell::read_ifs(&rec, 5, b"\t");
        let (stp, smode, skind, sgeo) = (r[1].clone(), r[2].clone(), r[3].clone(), r[4].clone());
        if stamp(&sf) != stp {
            return Err(ctx.die(cat!("shot --since: ", since, " changed since the shot")));
        }
        if skind == b"screen 0,0" {
            if !sels.is_empty() {
                return Err(ctx.die(cat!(
                    "shot --since: ",
                    since,
                    " is a shot of the screen, not of a window"
                )));
            }
        } else if shell::re_match("^window .* ", &skind) || skind.starts_with(b"screen ") {
            return Err(ctx.die(cat!(
                "shot --since: ",
                since,
                " is a crop (-g or --monitor); --since takes a whole shot"
            )));
        } else if let Some(a) = skind.strip_prefix(b"window ".as_slice()) {
            if sels.is_empty() {
                sels = vec![a.to_vec()];
            }
        }
        let pl = tsv_last(&ctx.at("changed/pointers.tsv"), &sf);
        // (awk's last $2 of those lines: the field after the first tab, up to the next)
        let sptr = {
            let all = std::fs::read(ctx.at("changed/pointers.tsv")).unwrap_or_default();
            let mut p = Vec::new();
            for l in all.split(|&c| c == b'\n') {
                let mut f = l.split(|&c| c == b'\t');
                if f.next() == Some(sf.as_slice()) && !l.is_empty() {
                    p = f.next().unwrap_or_default().to_vec();
                }
            }
            let _ = pl;
            p
        };
        let t = split_at(&stp, b' ').1;
        let ms = fixed((awk_num(&t) * 1000.0).trunc(), 0);
        s.sref = Some([
            sf,
            since.clone(),
            smode,
            skind,
            sgeo,
            if sptr.is_empty() { b"-".to_vec() } else { sptr },
            ms.into_bytes(),
        ]);
    }
    if s.tofd.is_empty() && s.out.is_empty() && burst.is_empty() {
        let tmp = shell::env("TMPDIR")
            .filter(|t| !t.is_empty())
            .unwrap_or(b"/tmp".to_vec());
        let (o, f, d) = shot_path(ctx, &cat!(tmp, "/omabox-", ctx.name, "-", hms_ms(), ".png"))?;
        (s.out, s.file, s.dir) = (o, f, Some(d));
    }
    // What the image covers: a window (its own pixels, finding 81), a region, or the whole layout.
    // PNG level 1: most of a shot's time was compression, for a file an agent reads once (finding 83).
    s.grim = vec![b"grim".to_vec(), b"-l".to_vec(), b"1".to_vec()];
    let mut wgeo = Vec::new();
    if let Some(g) = &s.g {
        [s.gx, s.gy, s.lw, s.lh] = g.clone();
    }
    if !sels.is_empty() {
        let wins = box_windows(ctx, None)?;
        s.win = win_select(ctx, &wins, &sels)?;
        let id = jq_r(".stableId", &s.win);
        if !shell::re_match("^[0-9a-fA-F]+$", &id) {
            return Err(ctx.die(cat!(
                "shot: window ",
                win_desc(&s.win),
                " has no stableId to capture it by (",
                id,
                ")"
            )));
        }
        s.grim.extend([b"-T".to_vec(), id]);
        s.where_ = cat!("window ", jq_r(".address", &s.win));
        let f = shell::read_fields(&jq_r(crate::jqprog::WIN_SIZE2_JQ, &s.win), 2);
        (s.ww, s.wh) = (f[0].clone(), f[1].clone());
        if s.g.is_some() {
            // A region past the window is cut to it and said so (#240); only one with nothing
            // inside it is refused. Logical px here: win_buf_geom scales what is left.
            let Some((c, note)) = clip(&s.gx, &s.gy, &s.lw, &s.lh, &s.ww, &s.wh) else {
                return Err(ctx.die(cat!(
                    "shot: -g ",
                    s.gx,
                    ",",
                    s.gy,
                    " ",
                    s.lw,
                    "x",
                    s.lh,
                    " has nothing inside window ",
                    win_desc(&s.win),
                    " (",
                    s.ww,
                    "x",
                    s.wh,
                    "; -g is in the window's coordinates)"
                )));
            };
            [s.gx, s.gy, s.lw, s.lh] = c;
            if !note.is_empty() {
                say(cat!(
                    "shot: -g clipped to ",
                    s.gx,
                    ",",
                    s.gy,
                    " ",
                    s.lw,
                    "x",
                    s.lh,
                    " of window ",
                    win_desc(&s.win),
                    " (",
                    s.ww,
                    "x",
                    s.wh,
                    "; -g is in the window's coordinates): ",
                    note
                ));
            }
            // grim takes a window's -g in its buffer's pixels (finding 300): the crop scaled to them.
            let wsc = win_scale(ctx, &jq_r(".address", &s.win))?;
            wgeo = win_buf_geom([&s.gx, &s.gy, &s.lw, &s.lh, &wsc, &s.ww, &s.wh]);
            s.grim.extend([b"-g".to_vec(), wgeo.clone()]);
            s.where_ = cat!(s.where_, " ", s.gx, ",", s.gy, " ", s.ww, "x", s.wh);
        } else {
            (s.lw, s.lh) = (s.ww.clone(), s.wh.clone());
        }
    } else if s.g.is_some() {
        // Past the layout, grim pads or fails: cut to the layout instead (#240).
        let ext = extent(ctx)?;
        let (ew, eh) = crate::input::wh(&ext);
        let Some((c, note)) = clip(&s.gx, &s.gy, &s.lw, &s.lh, &ew, &eh) else {
            return Err(ctx.die(cat!(
                "shot: -g ",
                s.gx,
                ",",
                s.gy,
                " ",
                s.lw,
                "x",
                s.lh,
                " has nothing on the screen (",
                ext,
                ")"
            )));
        };
        [s.gx, s.gy, s.lw, s.lh] = c;
        if !note.is_empty() {
            say(cat!(
                "shot: -g clipped to ",
                s.gx,
                ",",
                s.gy,
                " ",
                s.lw,
                "x",
                s.lh,
                " of the screen (",
                ext,
                "): ",
                note
            ));
        }
        s.grim
            .extend([b"-g".to_vec(), cat!(s.gx, ",", s.gy, " ", s.lw, "x", s.lh)]);
        s.where_ = cat!("screen ", s.gx, ",", s.gy);
    } else {
        let ext = extent(ctx)?;
        s.where_ = b"screen 0,0".to_vec();
        (s.lw, s.lh) = crate::input::wh(&ext);
    }
    let (lw, lh) = (bi(&s.lw)?, bi(&s.lh)?);
    // --zoom's limit on the region left once -g was cut (#240).
    if !s.zoom.is_empty() {
        let z = bi(&s.zoom)?;
        if lw.max(lh).saturating_mul(z) > 2000 {
            return Err(ctx.die(cat!(
                "shot: --zoom ",
                s.zoom,
                " of ",
                s.lw,
                "x",
                s.lh,
                " is ",
                n(lw.saturating_mul(z)),
                "x",
                n(lh.saturating_mul(z)),
                ": at most 2000 px a side, a smaller -g"
            )));
        }
    }
    // --fit: grim scales the capture (-s), truncating the size, so the factor is rounded up. What is
    // compared with N is the image grim writes without -s (#227): the layout's size times the
    // highest scale of the outputs captured; a window's capture is its buffer (finding 300).
    let long = lw.max(lh);
    let mut scaled = false;
    if !s.fit.is_empty() {
        let (native, base): (Vec<u8>, Vec<u8>);
        if !s.win.is_empty() {
            if !wgeo.is_empty() {
                let f = shell::read_ifs(&wgeo, 3, b"x ");
                let (bw, bh) = (bi(&f[1])?, bi(&f[2])?);
                native = n(bw.max(bh));
            } else {
                let wsc = win_scale(ctx, &jq_r(".address", &s.win))?;
                native = fixed((long as f64 * awk_num(&wsc) + 0.5).trunc(), 0).into_bytes();
            }
            base = native.clone();
        } else {
            let mons = strip_nl(&hypr_json(ctx, &["monitors"])?);
            let rx: &[u8] = if s.gx.is_empty() { b"0" } else { &s.gx };
            let ry: &[u8] = if s.gy.is_empty() { b"0" } else { &s.gy };
            let r = Args::new(&["-r"])
                .kv("--argjson", "x", rx)
                .kv("--argjson", "y", ry)
                .kv("--argjson", "w", &s.lw)
                .kv("--argjson", "h", &s.lh)
                .arg(format!("{JQ_MON}{}", crate::jqprog::SHOT_SCALE_JQ))
                .run(Some(&mons), jq::Err::Null);
            let mut sc = if r.ok() { r.text() } else { b"1".to_vec() };
            if !shell::re_match(r"^[0-9]+(\.[0-9]+)?$", &sc) {
                sc = b"1".to_vec();
            }
            native = fixed(long as f64 * awk_num(&sc), 6).into_bytes();
            base = n(long);
        }
        let fitn = awk_num(&s.fit);
        if awk_num(&native) > fitn + 1e-9 {
            scaled = true;
            let f = ((fitn / awk_num(&base) * 1e6 + 1.0).trunc()) / 1e6;
            s.grim.extend([b"-s".to_vec(), fixed(f, 6).into_bytes()]);
        }
    }
    // --no-pointer (#228): hidden from the first frame on, shown again when this omabox ends. A
    // window's own pixels never have it.
    if nopointer && s.win.is_empty() {
        crate::ptr::hold_until_exit(ctx, format!("shot {}", std::process::id()).as_bytes())?;
    }
    if !burst.is_empty() {
        return burst_run(
            ctx,
            &mut s,
            bi(&burst)?,
            every.unwrap_or(100),
            sheet,
            bdiff,
            &after,
        );
    }
    // --changed captures the whole of it beside the references, to compare; the crop is written
    // after.
    if s.changed {
        let dir = cat!(ctx.d, "/changed");
        if !crate::shots::mkdir_p(&dir) {
            return Err(ctx.die(cat!("shot: cannot write in ", ctx.d, "/changed")));
        }
        match shell::mktemp_in(&cat!(dir, "/.new-XXXXXX")) {
            // (a file of omabox's own, by its name: not in OUT's folder)
            Ok((_, f)) => (s.file, s.dir) = (f, None),
            Err(_) => return Err(ctx.die(cat!("shot: cannot write in ", ctx.d, "/changed"))),
        }
    }
    // A screen shot's desktop, read beside the capture and just before it, for --in (#333): what
    // a later click at one of its points would get then.
    if s.win.is_empty() {
        let c = ctx.clone();
        s.snap = Some(std::thread::spawn(move || crate::desk::desk_snap(&c, true)));
    }
    capture(ctx, &mut s, long, lw, lh, scaled)
}

/// geom_clip X Y W H LW LH: the region cut to LW x LH, as numbers (`010` is 10 and written 10:
/// bash's arithmetic clipped it as octal 8 and handed grim the word as given, finding 448). None
/// when nothing of it is inside, or a word that is no number (geom_parse took none such).
fn clip(
    x: &[u8],
    y: &[u8],
    w: &[u8],
    h: &[u8],
    lw: &[u8],
    lh: &[u8],
) -> Option<([Vec<u8>; 4], Vec<u8>)> {
    let v = [dec(x)?, dec(y)?, dec(w)?, dec(h)?];
    let ([cx, cy, cw, ch], note) = geom_clip(v, dec(lw)?, dec(lh)?)?;
    Some(([n(cx), n(cy), n(cw), n(ch)], note))
}

/// The capture, its failures in words, then the record and what shot says.
/// A capture of an interactive box that timed out (or gave nothing): no frame, in words. Never tell
/// an agent to show the window: switching the user's workspace for a frame is what the box exists to
/// avoid. Only such a capture is "no frame": grim refusing what it was asked says why (finding 176).
fn no_frame_words(ctx: &Ctx, grc: i32) -> Option<Vec<u8>> {
    let n_ = &ctx.name;
    if meta(&ctx.d, "mode").0 != b"interactive" || !(grc == 124 || grc == 0) {
        return None;
    }
    if meta(&ctx.d, "drawn_hidden").0 != b"true" {
        return Some(cat!(
            "shot: no frame from interactive box '",
            n_,
            "': it was started by an older omabox, which does not draw its window while hidden. It needs a restart, which ends the user's session in it: ask the user. Never switch the user's workspace or focus to get a frame"
        ));
    }
    if ctx.at("run/omabox.reopened").is_file() {
        return Some(cat!(
            "shot: no frame from interactive box '",
            n_,
            "': the user closed its window and kept the box running (confirm-close), and the new window is not drawn while hidden. It needs a restart, which ends the user's session in it: ask the user. Never switch the user's workspace or focus to get a frame"
        ));
    }
    Some(cat!(
        "shot: no frame from interactive box '",
        n_,
        "' (",
        crate::settle::ws_text(&meta(&ctx.d, "workspace").0),
        ") in 10 s: ask the user; never switch the user's workspace or focus to get one"
    ))
}

/// grim failing otherwise: what it said (WHY, or how it ended when it said nothing, #217), and a
/// 0x0 output's note (ZN, #213).
fn grim_failed_words(mut why: Vec<u8>, grc: i32, zn: &[u8]) -> Vec<u8> {
    if why.is_empty() {
        why = match grc {
            0 => b"it gave an empty image".to_vec(),
            124 => b"it gave nothing in 10 s".to_vec(),
            r => format!("it ended with status {r} and said nothing").into_bytes(),
        };
    }
    let zn = if zn.is_empty() {
        Vec::new()
    } else {
        cat!("; ", zn)
    };
    cat!("shot: grim failed: ", why, zn)
}

fn capture(ctx: &mut Ctx, s: &mut Shot, _long: i64, lw: i64, lh: i64, scaled: bool) -> Res<()> {
    let (pid, mpid) = (Arc::new(AtomicI32::new(0)), Arc::new(AtomicI32::new(0)));
    let (stop, done) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    // A signal during the capture (#386): the grab stopped (its children killed until its thread is
    // over, at most 3 s, so it makes no file after this), then grim's words and the file it was
    // writing removed. Armed with the making. Any other end leaves them to the code below. Not under
    // --no-pointer's hold: its handlers give the pointer back on a signal, and arming trap.rs's would
    // replace them.
    let (gerr, _undo) = crate::trap::made(|| -> Res<_> {
        let (_, gerr) = shell::mktemp()?;
        if crate::ptr::holding() {
            return Ok((gerr, None));
        }
        let (pid, mpid, stop, done) = (pid.clone(), mpid.clone(), stop.clone(), done.clone());
        let (err, file) = (gerr.clone(), s.file.clone());
        let unput = s.tofd.is_empty() || s.changed;
        let undo = crate::trap::on_exit(move || {
            if !crate::trap::signalled() {
                return;
            }
            stop.store(true, Ordering::SeqCst);
            let end = std::time::Instant::now() + Duration::from_secs(3);
            while !done.load(Ordering::SeqCst) && std::time::Instant::now() < end {
                for p in [&pid, &mpid] {
                    let p = p.load(Ordering::SeqCst);
                    if p > 0 {
                        kill_tree(n(i64::from(p)).as_slice());
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = std::fs::remove_file(shell::path(&err));
            if unput {
                let _ = std::fs::remove_file(shell::path(&file));
            }
        });
        Ok((gerr, Some(undo)))
    })?;
    let grab = Arc::new(Grab {
        ctx: ctx.clone(),
        grim: s.grim.clone(),
        zoom: s.zoom.clone(),
        err: gerr.clone(),
        pid,
        mpid,
        stop,
    });
    let to_fd = !s.tofd.is_empty() && !s.changed;
    let tofd = s.tofd.clone();
    let file = s.file.clone();
    let held = s.dir.clone();
    let has_win = !s.win.is_empty();
    // put: into a jailed caller's fd (never reopened by name), else the file.
    let put = {
        let g = grab.clone();
        let (tofd, file, held) = (tofd.clone(), file.clone(), held.clone());
        move || -> i32 {
            // (once the command gave up on the capture, nothing more is made)
            if g.stop.load(Ordering::SeqCst) {
                return 1;
            }
            let target: Option<OwnedFd> = if to_fd {
                let fd: i32 = String::from_utf8_lossy(&tofd).parse().unwrap_or(-1);
                // SAFETY: fd_file checked it is open; dup gives a descriptor of our own.
                let d = unsafe { libc::dup(fd) };
                // SAFETY: d is a new descriptor of ours.
                (d >= 0).then(|| unsafe { OwnedFd::from_raw_fd(d) })
            } else if let Some(d) = &held {
                d.create(&shell::basename(&file)).ok().map(OwnedFd::from)
            } else {
                crate::shots::create_nofollow(&file).ok().map(OwnedFd::from)
            };
            match target {
                Some(t) => g.grab(t),
                None => 1,
            }
        }
    };
    let (tx, rx) = std::sync::mpsc::channel::<i32>();
    let th: Option<std::thread::JoinHandle<()>>;
    {
        let g = grab.clone();
        let file = file.clone();
        let changed = s.changed;
        let tofd_empty = tofd.is_empty();
        th = Some(std::thread::spawn(move || {
            // A window's first capture can be a frame from long ago (finding 81): captured twice.
            let mut rc = 0;
            if has_win {
                rc = g.win_retried(|| match dev_null(true) {
                    Ok(nl) => g.grab(nl),
                    Err(_) => 1,
                });
                if rc == 0 {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            if rc == 0 {
                rc = if has_win { g.win_retried(&put) } else { put() };
            }
            // Under load grim now and then ends 0 with no image at all (#217, finding 318).
            if rc == 0
                && (tofd_empty || changed)
                && !nonempty(&file)
                && !g.stop.load(Ordering::SeqCst)
            {
                std::thread::sleep(Duration::from_millis(100));
                rc = put();
            }
            done.store(true, Ordering::SeqCst);
            let _ = tx.send(rc);
        }));
    }
    let unput = |s: &Shot| {
        if s.tofd.is_empty() || s.changed {
            let _ = std::fs::remove_file(shell::path(&s.file));
        }
    };
    // The capture runs beside: one still going after 2 s is a big one, or a Hyprland that stopped
    // answering (#125, finding 182). Then the box is asked once.
    let grc = match rx.recv_timeout(Duration::from_secs(2)) {
        Ok(r) => r,
        Err(_) => {
            if !hypr_answers(ctx)? {
                // (bash's kill_tree of the capture's subshell: here its children killed until the
                // thread that runs them is over, so it makes no file once these are removed)
                grab.stop.store(true, Ordering::SeqCst);
                let end = std::time::Instant::now() + Duration::from_secs(3);
                loop {
                    for p in [&grab.pid, &grab.mpid] {
                        let p = p.load(Ordering::SeqCst);
                        if p > 0 {
                            kill_tree(n(i64::from(p)).as_slice());
                        }
                    }
                    if th.as_ref().is_none_or(|t| t.is_finished())
                        || std::time::Instant::now() >= end
                    {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                let _ = std::fs::remove_file(shell::path(&gerr));
                unput(s);
                if !box_alive(ctx)? {
                    return Err(ctx.die(cat!(
                        "shot: box '",
                        ctx.name,
                        "' went down (why: omabox log -b ",
                        ctx.name,
                        " all)"
                    )));
                }
                let t = hung_text(ctx, "")?;
                return Err(ctx.die(cat!("shot: ", t)));
            }
            rx.recv().unwrap_or(1)
        }
    };
    let errs = std::fs::read(shell::path(&gerr)).unwrap_or_default();
    let mut why: Vec<u8> = errs
        .iter()
        .map(|&c| if c == b'\n' { b' ' } else { c })
        .filter(|&c| c != 0)
        .collect();
    while why.last() == Some(&b' ') {
        why.pop();
    }
    let _ = std::fs::remove_file(shell::path(&gerr));
    let interactive = meta(&ctx.d, "mode").0 == b"interactive";
    // grim's own timeout: asked again, in case the box stopped answering during it.
    if grc == 124 && !interactive && !hypr_answers(ctx)? {
        unput(s);
        let t = hung_text(ctx, "")?;
        return Err(ctx.die(cat!("shot: ", t)));
    }
    if grc != 0 || !nonempty(&s.file) {
        unput(s);
        if let Some(m) = no_frame_words(ctx, grc) {
            return Err(ctx.die(m));
        }
        // A window's copy that kept failing (finding 605): what grim 1.5.0's SIGSEGV means.
        if why.is_empty() && grc == 139 && has_win {
            why = b"it died of SIGSEGV (status 139) on each of 4 tries, saying nothing: grim 1.5.0 does when Hyprland fails a window's copy, as while the window keeps changing size".to_vec();
        }
        // A 0x0 output in an NVIDIA box (#213) fails a whole shot: said why.
        let mut zn = Vec::new();
        if meta(&ctx.d, "wayland_screen").0 == b"true" {
            let a: Vec<Vec<u8>> = ["timeout", "6", "hyprctl", "-j", "monitors"]
                .iter()
                .map(|w| w.as_bytes().to_vec())
                .collect();
            let r = on_box(ctx, &a, false, Out::Capture(None), ErrTo::Null);
            zn = zero_note(&shell::subst(&r.out), b"true", None);
        }
        return Err(ctx.die(grim_failed_words(why, grc, &zn)));
    }
    if s.changed {
        return changed(ctx, s, lw, lh);
    }
    if s.tofd.is_empty() {
        // (`mv "$file" "$out"`: into a folder of that name, as mv does; in the folder held, where
        // shot_path refused a folder at OUT)
        let moved = match &s.dir {
            Some(d) => d
                .rename(&shell::basename(&s.file), &shell::basename(&s.out))
                .map_err(|e| {
                    say(cat!(
                        "shot: cannot write ",
                        s.out,
                        ": ",
                        crate::shots::why(&e)
                    ))
                })
                .is_ok(),
            None => crate::shots::mv_f(&s.file, &s.out),
        };
        if !moved {
            return Err(Exit(1));
        }
        s.file = s.out.clone();
    }
    let Some((iw, ih)) = png_size(&s.file) else {
        return Err(ctx.die(cat!("shot: ", s.out, " is not a PNG")));
    };
    // Its scale in its comment (#267, finding 616), image pixels to the layout's, which a crop of it
    // keeps: find --image scales it to a box drawn at another. (Not into a jailed caller's fd, which
    // is never read back here.)
    if s.tofd.is_empty()
        && let Some(w) = shell::dec(&s.lw).filter(|&w| w > 0)
    {
        crate::shots::tag_scale(&s.file, iw as f64 / w as f64);
    }
    let screen = box_screen(ctx)?;
    let mode = split_at(&screen, b' ').1;
    let geo = cat!(s.lw, "x", s.lh, " ", n(iw), "x", n(ih));
    shot_record(ctx, &s.out, &mode, &s.where_, &geo, &s.file)?;
    let (o, f) = (s.out.clone(), s.file.clone());
    s.layout_save(ctx, &o, &f);
    // A whole shot is what a later --changed compares with (finding 243); not one taken with
    // OMABOX_SHOT_NO_BASELINE=1 (the suite's evidence shots, #195, finding 266).
    if s.g.is_none()
        && s.zoom.is_empty()
        && !scaled
        && shell::env_or_empty("OMABOX_SHOT_NO_BASELINE") != b"1"
    {
        let (key, ptr) = if !s.win.is_empty() {
            (
                s.where_
                    .strip_prefix(b"window ".as_slice())
                    .unwrap_or(&s.where_)
                    .to_vec(),
                b"-".to_vec(),
            )
        } else {
            let p = crate::exit::muted(|| cursor_now(ctx).ok().filter(|p| !p.is_empty()));
            (b"screen".to_vec(), p.unwrap_or(b"-".to_vec()))
        };
        shot_ref_save(ctx, &s.file, &key, [&s.out, &mode, &geo, &ptr])?;
    }
    // One line on stderr when image pixels are not screen pixels (cropped or scaled).
    let mut k = Vec::new();
    if n(iw) != s.lw {
        k = cat!(" (x", ratio(lw, iw), ")");
    }
    if !s.zoom.is_empty() {
        k = cat!(" (zoomed ", s.zoom, " times)");
    }
    if !s.win.is_empty() {
        let state = jq_r(crate::jqprog::WIN_STATE_JQ, &s.win);
        let crop = if s.g.is_some() {
            cat!("; ", s.lw, "x", s.lh, " of it at ", s.gx, ",", s.gy)
        } else {
            Vec::new()
        };
        let img = if k.is_empty() {
            Vec::new()
        } else {
            cat!("; image ", n(iw), "x", n(ih), k)
        };
        say(cat!(
            "window ",
            win_desc(&s.win),
            " ",
            s.ww,
            "x",
            s.wh,
            " at ",
            jq_r(crate::jqprog::WIN_AT2_JQ, &s.win),
            ", workspace ",
            jq_r(".workspace", &s.win),
            state,
            crop,
            img,
            "; click with --in ",
            s.out
        ));
    } else if cat!(s.where_, " ", s.lw, " ", n(iw)) != cat!("screen 0,0 ", s.lw, " ", s.lw)
        || s.lh != n(ih)
    {
        say(cat!(
            n(iw),
            "x",
            n(ih),
            " image of the screen's ",
            s.lw,
            "x",
            s.lh,
            " at ",
            s.where_
                .strip_prefix(b"screen ".as_slice())
                .unwrap_or(&s.where_),
            k,
            "; click with --in ",
            s.out
        ));
    }
    let gx: &[u8] = if s.gx.is_empty() { b"0" } else { &s.gx };
    let gy: &[u8] = if s.gy.is_empty() { b"0" } else { &s.gy };
    let (lws, lhs) = (s.lw.clone(), s.lh.clone());
    s.said(
        [&s.out, gx, gy, &lws, &lhs, &n(iw), &n(ih)],
        [b"", b"", b""],
    )
}

/// `[ -s FILE ]`.
fn nonempty(f: &[u8]) -> bool {
    std::fs::metadata(shell::path(f)).is_ok_and(|m| m.len() > 0)
}

/// shot --changed (#145, finding 243): the whole frame just taken against the last whole shot of the
/// same window or screen, or --since's; the image is the box around what differs, 16 px more each
/// side, recorded as a -g crop for --in. Not changes: where the pointer was (when it moved since),
/// --ignore's regions, and a caret alone (4 px thin, 40 long at most).
fn changed(ctx: &mut Ctx, s: &mut Shot, lw: i64, lh: i64) -> Res<()> {
    let dir = cat!(ctx.d, "/changed");
    let rmf = |f: &[u8]| {
        let _ = std::fs::remove_file(shell::path(f));
    };
    let Some((iw, ih)) = png_size(&s.file) else {
        rmf(&s.file);
        return Err(ctx.die("shot: grim's image is not a PNG"));
    };
    // (the frame just taken not left in $D/changed when the box stops answering here: finding 427)
    let screen = match box_screen(ctx) {
        Ok(v) => v,
        Err(e) => {
            rmf(&s.file);
            return Err(e);
        }
    };
    let mode = split_at(&screen, b' ').1;
    let (key, what, ptr) = if !s.win.is_empty() {
        (
            s.where_
                .strip_prefix(b"window ".as_slice())
                .unwrap_or(&s.where_)
                .to_vec(),
            cat!("window ", win_desc(&s.win)),
            b"-".to_vec(),
        )
    } else {
        let p = crate::exit::muted(|| cursor_now(ctx).ok().filter(|p| !p.is_empty()));
        (
            b"screen".to_vec(),
            b"the screen".to_vec(),
            p.unwrap_or(b"-".to_vec()),
        )
    };
    let geo = cat!(s.lw, "x", s.lh, " ", n(iw), "x", n(ih));
    let (mut rfile, mut rshot, mut rptr, mut rt) =
        (Vec::new(), Vec::new(), b"-".to_vec(), b"0".to_vec());
    let mut why = Vec::new();
    if let Some(r) = &s.sref {
        let (rmode, rkind, rgeo);
        [rfile, rshot, rmode, rkind, rgeo, rptr, rt] = r.clone();
        if rkind != s.where_ {
            rmf(&s.file);
            return Err(ctx.die(cat!(
                "shot --since: ",
                rshot,
                " is a shot of ",
                rkind,
                ", not of ",
                what
            )));
        }
        if rmode != mode {
            rmf(&s.file);
            return Err(ctx.die(cat!(
                "shot --since: ",
                rshot,
                " was taken at ",
                rmode,
                ", the box's mode is ",
                mode,
                " now"
            )));
        }
        if rgeo != geo {
            rmf(&s.file);
            let (a, b) = match rgeo.iter().position(|&c| c == b' ') {
                Some(i) => (rgeo[..i].to_vec(), rgeo[i + 1..].to_vec()),
                None => (rgeo.clone(), rgeo.clone()),
            };
            return Err(ctx.die(cat!(
                "shot --since: ",
                rshot,
                " is ",
                a,
                " as a ",
                b,
                " image; ",
                what,
                " is ",
                s.lw,
                "x",
                s.lh,
                " as ",
                n(iw),
                "x",
                n(ih),
                " now (--since takes a whole shot of it: no -g or --fit, and --fit 0 while shot-fit is set)"
            )));
        }
    } else if shell::path(&cat!(dir, "/", key, ".png")).is_file()
        && shell::path(&cat!(dir, "/", key, ".tsv")).is_file()
    {
        // `IFS=$'\t' read -r rshot rmode rgeo rptr rt < KEY.tsv`
        let line = std::fs::read(shell::path(&cat!(dir, "/", key, ".tsv"))).unwrap_or_default();
        let r = shell::read_ifs(&line, 5, b"\t");
        let (rmode, rgeo);
        [rshot, rmode, rgeo, rptr, rt] = [
            r[0].clone(),
            r[1].clone(),
            r[2].clone(),
            r[3].clone(),
            r[4].clone(),
        ];
        rfile = cat!(dir, "/", key, ".png");
        if rmode != mode {
            why = cat!("the box's mode was ", rmode, " at the last shot of it");
        } else if rgeo != geo {
            let a = rgeo
                .split(|&c| c == b' ')
                .next()
                .unwrap_or_default()
                .to_vec();
            why = cat!("it was ", a, " at the last shot of it");
        }
    } else {
        why = b"no earlier shot of it to compare with".to_vec();
        // Shots the shot-fit setting scales leave none (#188).
        let sf = crate::exit::muted(|| shot_fit_default().unwrap_or_default());
        if !sf.is_empty() {
            why.extend(b" (a shot scaled by shot-fit leaves none: shot --fit 0 for one)");
        }
    }
    // The image to the caller: renamed into place, or into a jailed caller's fd.
    let put_image = |ctx: &Ctx, s: &Shot, src: &[u8]| -> Res<()> {
        let ok = if !s.tofd.is_empty() {
            let fd: i32 = String::from_utf8_lossy(&s.tofd).parse().unwrap_or(-1);
            let data = std::fs::read(shell::path(src));
            // SAFETY: fd_file checked fd N is open for writing; ManuallyDrop keeps it open.
            let mut f = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd) });
            data.is_ok_and(|d| f.write_all(&d).is_ok())
        } else {
            let part = cat!(s.out, ".part");
            // (never through a link at PART: #338; in the folder held, when there is one)
            let (pb, ob) = (shell::basename(&part), shell::basename(&s.out));
            let made = |d: Option<&crate::shots::OutDir>| match d {
                Some(d) => d.create(&pb),
                None => crate::shots::create_nofollow(&part),
            };
            std::fs::File::open(shell::path(src))
                .and_then(|mut f| std::io::copy(&mut f, &mut made(s.dir.as_ref())?))
                .is_ok()
                && match &s.dir {
                    Some(d) => d.rename(&pb, &ob).is_ok(),
                    None => crate::shots::mv_f(&part, &s.out),
                }
        };
        if !ok {
            let _ = std::fs::remove_file(shell::path(&s.file));
            let _ = std::fs::remove_file(shell::path(&cat!(s.out, ".part")));
            return Err(ctx.die(cat!("shot: cannot write ", s.out)));
        }
        Ok(())
    };
    let rec = if s.tofd.is_empty() {
        s.out.clone()
    } else {
        cat!("/dev/fd/", s.tofd)
    };
    if !why.is_empty() {
        put_image(ctx, s, &s.file)?;
        shot_record(ctx, &s.out, &mode, &s.where_, &geo, &rec)?;
        let o = s.out.clone();
        s.layout_save(ctx, &o, &rec);
        shot_ref_save(ctx, &s.file, &key, [&s.out, &mode, &geo, &ptr])?;
        rmf(&s.file);
        say(cat!(
            "--changed: ",
            why,
            ": a whole shot of ",
            what,
            " (",
            s.lw,
            "x",
            s.lh,
            "); click with --in ",
            s.out
        ));
        return s.said(
            [&s.out, b"0", b"0", &s.lw, &s.lh, &n(iw), &n(ih)],
            [b"whole", b"", &why],
        );
    }
    // What is left out, in image pixels: the pointer where it was, when it moved since; --ignore's.
    let mut rs = s.masks.clone();
    if rptr != b"-" && rptr != ptr {
        rs.push(cursor_rect(&rptr).ok_or(Exit(1))?);
    }
    let mut m = Vec::new();
    for r in &rs {
        let f = shell::read_ifs(r, 4, b",");
        let (x, y, w, h) = (bi(&f[0])?, bi(&f[1])?, bi(&f[2])?, bi(&f[3])?);
        m.push(to_image_px([x, y, w, h], lw, lh, iw, ih).ok_or(Exit(1))?);
    }
    let Some(dbox) = diff_box(&rfile, &s.file, &m) else {
        rmf(&s.file);
        return Err(ctx.die("shot: --changed could not compare the frames (magick)"));
    };
    let ago = ms_s(n(now_ms().saturating_sub(bi(&rt)?)));
    let (mut x0, mut y0, mut x1, mut y1) = (0i64, 0i64, 0i64, 0i64);
    let mut caret = Vec::new();
    let mut boxed = dbox;
    if let Some([dx, dy, dw, dh]) = dbox {
        x0 = dx.saturating_mul(lw) / iw;
        y0 = dy.saturating_mul(lh) / ih;
        x1 = (dx
            .saturating_add(dw)
            .saturating_mul(lw)
            .saturating_add(iw)
            .saturating_sub(1))
            / iw;
        y1 = (dy
            .saturating_add(dh)
            .saturating_mul(lh)
            .saturating_add(ih)
            .saturating_sub(1))
            / ih;
        let (w, h) = (x1 - x0, y1 - y0);
        if caret_shaped(w, h) {
            caret = format!("; a caret? {w}x{h} at {x0},{y0} left out").into_bytes();
            boxed = None;
        }
    }
    // All of it where the pointer is now, which moved there: perhaps the pointer alone.
    let mut alone: &[u8] = b"";
    if boxed.is_some() && ptr != b"-" && ptr != rptr {
        let r = cursor_rect(&ptr).ok_or(Exit(1))?;
        let f = shell::read_ifs(&r, 4, b",");
        let (x, y, w, h) = (bi(&f[0])?, bi(&f[1])?, bi(&f[2])?, bi(&f[3])?);
        if !(x0 < x || y0 < y || x1 > x + w || y1 > y + h) {
            alone = b"; all of it around the pointer, which moved there: perhaps the pointer alone";
        }
    }
    if boxed.is_none() {
        rmf(&s.file);
        say(cat!(
            "--changed: nothing changed in ",
            what,
            " since ",
            rshot,
            " (",
            ago,
            " ago)",
            caret,
            ": no image"
        ));
        return s.said(
            [b"", b"", b"", b"", b"", b"", b""],
            [b"nothing", &rshot, b""],
        );
    }
    // The crop: 16 px around it, inside the target; in image pixels as the image is scaled.
    x0 = if x0 > 16 { x0 - 16 } else { 0 };
    y0 = if y0 > 16 { y0 - 16 } else { 0 };
    x1 = if x1 + 16 < lw { x1 + 16 } else { lw };
    y1 = if y1 + 16 < lh { y1 + 16 } else { lh };
    let (cw, ch) = (x1 - x0, y1 - y0);
    let (ix, iy) = (x0 * iw / lw, y0 * ih / lh);
    let mut ciw = (x1 * iw + lw - 1) / lw - ix;
    let mut cih = (y1 * ih + lh - 1) / lh - iy;
    if ix + ciw > iw {
        ciw = iw - ix;
    }
    if iy + cih > ih {
        cih = ih - iy;
    }
    let crop = match shell::mktemp_in(&cat!(dir, "/.crop-XXXXXX")) {
        Ok((_, c)) => c,
        Err(e) => return Err(e),
    };
    let ok = crate::sig::command("magick")
        .arg(std::ffi::OsStr::from_bytes(&crate::shots::png_arg(&s.file)))
        .arg("-crop")
        .arg(format!("{ciw}x{cih}+{ix}+{iy}"))
        .arg("+repage")
        .args(["-define", "png:compression-level=1"])
        .arg(std::ffi::OsStr::from_bytes(&cat!("png:", crop)))
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        rmf(&s.file);
        rmf(&crop);
        return Err(ctx.die("shot: --changed could not crop the frame (magick)"));
    }
    put_image(ctx, s, &crop)?;
    rmf(&crop);
    let rwhere = if !s.win.is_empty() {
        cat!(s.where_, " ", n(x0), ",", n(y0), " ", s.lw, "x", s.lh)
    } else {
        cat!("screen ", n(x0), ",", n(y0))
    };
    shot_record(
        ctx,
        &s.out,
        &mode,
        &rwhere,
        &cat!(n(cw), "x", n(ch), " ", n(ciw), "x", n(cih)),
        &rec,
    )?;
    let o = s.out.clone();
    s.layout_save(ctx, &o, &rec);
    shot_ref_save(ctx, &s.file, &key, [&s.out, &mode, &geo, &ptr])?;
    // Changes far apart make one big box: each named when they would be much smaller.
    let (mut places, mut cnt, mut area) = (Vec::new(), 0i64, 0i64);
    if cw * ch > 40_000 {
        for [px, py, pw, ph] in diff_parts(&rfile, &s.file, &m) {
            let (px, py, pw, ph) = (bi(&px)?, bi(&py)?, bi(&pw)?, bi(&ph)?);
            cnt += 1;
            area += pw * ph;
            if cnt <= 4 {
                if !places.is_empty() {
                    places.extend(b", ");
                }
                places.extend(
                    format!(
                        "{},{} {}x{}",
                        px * lw / iw,
                        py * lh / ih,
                        pw * lw / iw,
                        ph * lh / ih
                    )
                    .into_bytes(),
                );
            }
        }
        if !(cnt >= 2 && area * 2 * lw * lh / (iw * ih) < cw * ch) {
            places.clear();
        }
        if !places.is_empty() && cnt > 4 {
            places.extend(format!(" and {} more", cnt - 4).into_bytes());
        }
    }
    rmf(&s.file);
    let mut k = Vec::new();
    if ciw != cw {
        k = cat!("; image ", n(ciw), "x", n(cih), " (x", ratio(cw, ciw), ")");
    }
    let coords: &[u8] = if s.win.is_empty() {
        b""
    } else {
        b" (its coordinates)"
    };
    let pl = if places.is_empty() {
        Vec::new()
    } else {
        cat!("; it changed in ", n(cnt), " places: ", places)
    };
    say(cat!(
        "--changed since ",
        rshot,
        " (",
        ago,
        " ago): ",
        n(cw),
        "x",
        n(ch),
        " at ",
        n(x0),
        ",",
        n(y0),
        " of ",
        what,
        coords,
        k,
        pl,
        alone,
        "; click with --in ",
        s.out
    ));
    s.said(
        [&s.out, &n(x0), &n(y0), &n(cw), &n(ch), &n(ciw), &n(cih)],
        [b"changed", &rshot, b""],
    )
}

/// shot --burst N (#132, finding 212): N frames of the same capture, EVERY ms apart (0: back to
/// back), into a folder; each a shot of its own. From tools/burst, one capture client for the whole
/// burst (#196, finding 314); a box without it takes a grim a frame. --after: the action beside the
/// rest, from the first frame on; --diff: per frame, where it differs from the one before; --sheet:
/// one contact sheet.
fn burst_run(
    ctx: &mut Ctx,
    s: &mut Shot,
    nf: i64,
    every: i64,
    sheet: bool,
    bd: bool,
    act_args: &[Vec<u8>],
) -> Res<()> {
    if !s.tofd.is_empty() {
        return Err(ctx.die(
            "shot --burst: not for a jailed agent yet (its frames would each need a file of the jail's)",
        ));
    }
    if (sheet || bd) && !on_path("magick") {
        return Err(ctx.die(
            "shot: --sheet and --diff need ImageMagick's magick (package imagemagick, part of Omarchy)",
        ));
    }
    if s.out.is_empty() {
        // A folder of its own (#167), to the millisecond, made here (no -p): two bursts started
        // together never share one.
        let tmp = shell::env("TMPDIR")
            .filter(|t| !t.is_empty())
            .unwrap_or(b"/tmp".to_vec());
        let base = cat!(tmp, "/omabox-", ctx.name, "-burst-", hms_ms());
        let mut k = 1;
        s.out = base.clone();
        while std::fs::create_dir(shell::path(&s.out)).is_err() {
            if std::fs::symlink_metadata(shell::path(&s.out)).is_err() {
                return Err(ctx.die(cat!("shot --burst: cannot make ", s.out)));
            }
            k += 1;
            s.out = cat!(base, "-", n(k));
        }
    } else {
        let p = shell::path(&s.out);
        if p.exists() && !p.is_dir() {
            return Err(ctx.die(cat!(
                "shot --burst: ",
                s.out,
                " is a file; -o names a folder for the frames"
            )));
        }
        if !crate::shots::mkdir_p(&s.out) {
            return Err(ctx.die(cat!("shot --burst: cannot make ", s.out)));
        }
        // A folder used before (#167): the frames and sheet a burst wrote there go; only those.
        // (`rm -f -- "$out"/frame-[0-9][0-9][0-9].png "$out"/sheet.png`: a glob that matches
        // nothing, or a folder that cannot be listed, is the pattern itself, which rm -f takes as a
        // name: only a name that is not there is no failure.)
        let mut ok = true;
        let mut names: Vec<Vec<u8>> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&p) {
            for e in rd.flatten() {
                let nm = e.file_name().as_encoded_bytes().to_vec();
                if nm.len() == 13
                    && nm.starts_with(b"frame-")
                    && nm.ends_with(b".png")
                    && nm[6..9].iter().all(u8::is_ascii_digit)
                {
                    names.push(nm);
                }
            }
        }
        if names.is_empty() {
            names.push(b"frame-[0-9][0-9][0-9].png".to_vec());
        }
        names.push(b"sheet.png".to_vec());
        // Only what a burst writes goes (#338): a link, or a file that is not an image, refuses it.
        if let Some(m) = names
            .iter()
            .find_map(|nm| crate::shots::overwrite_refusal(&cat!(s.out, "/", nm)))
        {
            return Err(ctx.die(cat!("shot --burst: ", m)));
        }
        for nm in names {
            if let Err(e) = std::fs::remove_file(shell::path(&cat!(s.out, "/", nm))) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    ok = false;
                }
            }
        }
        if !ok {
            return Err(ctx.die(cat!(
                "shot --burst: cannot clear the frames an earlier burst left in ",
                s.out,
                " (frame-NNN.png, sheet.png)"
            )));
        }
    }
    s.out = crate::shots::real_or_end(ctx, &s.out)?;
    let (_, err) = shell::mktemp()?;
    let probe: Vec<Vec<u8>> = vec![
        b"test".to_vec(),
        b"-x".to_vec(),
        BOX_BURST.as_bytes().to_vec(),
    ];
    let one = on_box(ctx, &probe, false, Out::Inherit, ErrTo::Null).rc == 0;
    let (mut ts, mut files): (Vec<Vec<u8>>, Vec<Vec<u8>>) = (Vec::new(), Vec::new());
    let mut act: Option<std::process::Child> = None;
    // (`frame-%03d.png` of the tool's frame number, decimal: the tool writes them with %ld)
    let frame = |out: &[u8], i: &[u8]| -> Res<Vec<u8>> {
        Ok(cat!(out, "/frame-", format!("{:03}", bi(i)?)))
    };
    let spawn_act = |ctx: &Ctx| -> Option<std::process::Child> {
        if act_args.is_empty() {
            return None;
        }
        let mut c = crate::sig::omabox_cmd();
        c.arg("-b").arg(std::ffi::OsStr::from_bytes(&ctx.name));
        for a in act_args {
            c.arg(std::ffi::OsStr::from_bytes(a));
        }
        // (its output on stderr, not holding the frames' pipe; an asynchronous command's stdin is
        // /dev/null in a shell without job control)
        let e = crate::exec::dup_hi(2).ok()?;
        c.stdout(std::process::Stdio::from(e))
            .stdin(std::process::Stdio::null());
        crate::exec::background(&mut c);
        c.spawn().ok()
    };
    let fail =
        |ctx: &Ctx, at: usize, said: Vec<u8>, act: &mut Option<std::process::Child>| -> Exit {
            if let Some(a) = act.as_mut() {
                let _ = a.wait();
            }
            let _ = std::fs::remove_file(shell::path(&err));
            match box_alive(ctx) {
                Err(e) => return e,
                Ok(false) => {
                    return ctx.die(cat!(
                        "shot --burst: box '",
                        ctx.name,
                        "' went down at frame ",
                        n(at as i64)
                    ));
                }
                Ok(true) => {}
            }
            match hypr_answers(ctx) {
                Err(e) => return e,
                Ok(false) => {
                    return match hung_text(ctx, "") {
                        Ok(t) => ctx.die(cat!("shot --burst: ", t)),
                        Err(e) => e,
                    };
                }
                Ok(true) => {}
            }
            ctx.die(cat!(
                "shot --burst: frame ",
                n(at as i64),
                " failed: ",
                said
            ))
        };
    if one {
        // One client in the box (tools/burst), streaming `frame I T LEN` and LEN bytes of PNG. A
        // window is primed in it (finding 81).
        let mut a = vec![
            BOX_BURST.as_bytes().to_vec(),
            b"-n".to_vec(),
            n(nf),
            b"-e".to_vec(),
            n(every),
        ];
        if !s.win.is_empty() {
            a.extend([b"--prime".to_vec(), b"100".to_vec()]);
        }
        a.extend(s.grim[1..].iter().cloned());
        let Ok((pr, pw)) = crate::exec::pipe() else {
            let _ = std::fs::remove_file(shell::path(&err));
            return Err(Exit(1));
        };
        let ef = std::fs::File::create(shell::path(&err)).ok();
        let efd = ef.as_ref().map_or(-1, AsRawFd::as_raw_fd);
        let c = ctx.clone();
        // (bash's `<(...)`: SIGINT and SIGQUIT ignored in it)
        let tool = std::thread::spawn(move || {
            let r = crate::exec::on_box_bg(
                &c,
                &a,
                crate::exec::In::Inherit,
                Out::Fd(pw),
                if efd >= 0 {
                    ErrTo::Fd(efd)
                } else {
                    ErrTo::Null
                },
                None,
            );
            drop(ef);
            r.rc
        });
        let mut rd = std::io::BufReader::new(std::fs::File::from(pr));
        loop {
            let mut line = Vec::new();
            if rd.read_until(b'\n', &mut line).unwrap_or(0) == 0 || line.last() != Some(&b'\n') {
                break;
            }
            line.pop();
            line.retain(|&c| c != 0);
            let f = shell::read_fields(&line, 4);
            let (w, fno, ft, fl) = (&f[0], &f[1], &f[2], &f[3]);
            let digits = cat!(fno, ft, fl);
            if w != b"frame" || digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
                break;
            }
            let fpath = cat!(frame(&s.out, fno)?, ".png");
            let len = fl.iter().fold(0u64, |a, &c| {
                a.saturating_mul(10).saturating_add(u64::from(c - b'0'))
            });
            let mut data = Vec::new();
            let _ = (&mut rd).take(len).read_to_end(&mut data);
            let wrote = crate::shots::create_nofollow(&fpath)
                .and_then(|mut f| f.write_all(&data))
                .is_ok();
            // (`[ "$(stat -c %s -- "$f")" = "$fl" ]`: the words, so a length with a zero in front
            // is not the one written)
            if !wrote || data.len().to_string().as_bytes() != fl.as_slice() {
                let _ = std::fs::remove_file(shell::path(&fpath));
                break;
            }
            ts.push(ft.clone());
            files.push(fpath);
            // The action, once the first frame is there: beside the rest.
            if fno == b"1" {
                act = spawn_act(ctx);
            }
        }
        drop(rd);
        let rc = tool.join().unwrap_or(1);
        if files.len() as i64 != nf {
            let errs = std::fs::read(shell::path(&err)).unwrap_or_default();
            let mut said = Vec::new();
            let re = shell::Re::new("^omabox-burst: (frame [0-9]*: )?");
            for l in errs.split_inclusive(|&c| c == b'\n') {
                let (body, nl) = match l.strip_suffix(b"\n") {
                    Some(b) => (b, true),
                    None => (l, false),
                };
                let cut = re
                    .as_ref()
                    .and_then(|r| r.captures(body, false))
                    .and_then(|m| m[0])
                    .map_or(0, |(_, e)| e);
                said.extend(&body[cut..]);
                if nl {
                    said.push(b' ');
                }
            }
            let said = cat!(said, format!("(status {rc})"));
            return Err(fail(ctx, files.len() + 1, said, &mut act));
        }
    } else {
        say(cat!(
            "box '",
            ctx.name,
            "' has no ",
            BOX_BURST,
            " (started by an omabox without it): a grim a frame, four Hyprland events each (omabox down ",
            ctx.name,
            ", then up again, for one client a burst)"
        ));
        let mut g = vec![b"timeout".to_vec(), b"10".to_vec()];
        g.extend(s.grim.iter().cloned());
        g.push(b"-".to_vec());
        // A window off screen draws on its first capture (finding 81): primed once.
        if !s.win.is_empty() {
            let _ = on_box(ctx, &g, false, Out::Null, ErrTo::Null);
        }
        let t0 = crate::settle::mono_ms();
        for k in 1..=nf {
            let fpath = cat!(frame(&s.out, &n(k))?, ".png");
            let t = crate::settle::mono_ms() - t0;
            let ef = std::fs::File::create(shell::path(&err)).ok();
            let rc = match (crate::shots::create_nofollow(&fpath), ef.as_ref()) {
                (Ok(f), Some(e)) => {
                    on_box(ctx, &g, false, Out::Fd(f.into()), ErrTo::Fd(e.as_raw_fd())).rc
                }
                _ => 1,
            };
            drop(ef);
            if rc != 0 || !nonempty(&fpath) {
                let _ = std::fs::remove_file(shell::path(&fpath));
                let errs = std::fs::read(shell::path(&err)).unwrap_or_default();
                let said: Vec<u8> = errs
                    .iter()
                    .map(|&c| if c == b'\n' { b' ' } else { c })
                    .collect();
                return Err(fail(
                    ctx,
                    usize::try_from(k).unwrap_or(0),
                    strip_nl(&said),
                    &mut act,
                ));
            }
            ts.push(n(t));
            files.push(fpath);
            if k == 1 {
                act = spawn_act(ctx);
            }
            if every > 0 && k < nf {
                let next = t0 + k * every - crate::settle::mono_ms();
                if next > 0 {
                    std::thread::sleep(Duration::from_millis(u64::try_from(next).unwrap_or(0)));
                }
            }
        }
    }
    let _ = std::fs::remove_file(shell::path(&err));
    let mut arc = 0;
    if let Some(mut a) = act.take() {
        arc = a.wait().map(|s| crate::exec::status(s).0).unwrap_or(1);
    }
    let screen = box_screen(ctx)?;
    let mode = split_at(&screen, b' ').1;
    let Some((iw, ih)) = files.first().and_then(|f| png_size(f)) else {
        return Err(ctx.die("shot --burst: frame 1 is not a PNG"));
    };
    let geo = cat!(s.lw, "x", s.lh, " ", n(iw), "x", n(ih));
    for f in &files {
        shot_record(ctx, f, &mode, &s.where_, &geo, b"")?;
    }
    // --diff's box in the coordinates a click takes for this capture (#167): the screen's, or with
    // --window the window's own; the image's pixels scaled back outwards, from where it starts.
    let org = if let Some(o) = s.where_.strip_prefix(b"screen ".as_slice()) {
        o.to_vec()
    } else {
        let o = shell::read_fields(&s.where_, 4)[2].clone();
        if o.is_empty() { b"0,0".to_vec() } else { o }
    };
    let (ox, oy) = (
        bi(&split_at(&org, b',').0)?,
        bi(&org[org.iter().position(|&c| c == b',').map_or(0, |i| i + 1)..])?,
    );
    let (lw, lh) = (bi(&s.lw)?, bi(&s.lh)?);
    let mut prev: Option<&Vec<u8>> = None;
    for (k, f) in files.iter().enumerate() {
        let mut line = cat!(f, " ", ms_s(&ts[k]));
        if bd {
            if let Some(p) = prev {
                match diff_box(p, f, &[]).flatten() {
                    Some([dx, dy, dw, dh]) => {
                        let x0 = dx * lw / iw;
                        let y0 = dy * lh / ih;
                        let x1 = ((dx + dw) * lw + iw - 1) / iw;
                        let y1 = ((dy + dh) * lh + ih - 1) / ih;
                        line.extend(
                            format!(" changed {},{} {}x{}", ox + x0, oy + y0, x1 - x0, y1 - y0)
                                .into_bytes(),
                        );
                    }
                    None => line.extend(b" same"),
                }
            }
        }
        prev = Some(f);
        out(cat!(line, "\n"))?;
    }
    if sheet {
        // Tiles at most 320 px wide, 5 a row, each frame's number and time under it.
        let mut c = crate::sig::command("magick");
        c.arg("montage");
        for (k, f) in files.iter().enumerate() {
            c.arg("-label")
                .arg(format!("{}: {}", k + 1, ms_s(&ts[k])))
                .arg(std::ffi::OsStr::from_bytes(&crate::shots::png_arg(f)));
        }
        c.arg("-tile")
            .arg(format!("{}x", nf.min(5)))
            .args([
                "-geometry",
                "320x>+3+3",
                "-pointsize",
                "16",
                "-background",
                "white",
            ])
            .args(["-alpha", "off", "-depth", "8"])
            .arg(std::ffi::OsStr::from_bytes(&cat!(
                "png:",
                s.out,
                "/sheet.png"
            )))
            .stderr(std::process::Stdio::null());
        if !c.status().is_ok_and(|s| s.success()) {
            return Err(ctx.die("shot --burst: the contact sheet failed (magick montage)"));
        }
        out(cat!(s.out, "/sheet.png\n"))?;
    }
    let last = &ts[ts.len() - 1];
    let apart = n(bi(last)? / (nf - 1));
    say(cat!(
        n(nf),
        " frames of ",
        s.lw,
        "x",
        s.lh,
        " (",
        s.where_,
        ") over ",
        ms_s(last),
        ", ",
        ms_s(&apart),
        " apart, in ",
        s.out,
        "; click with --in a frame"
    ));
    // Frames identical to the one before (#191): the same pixels give the same PNG (grim -l 1).
    let mut same = 0;
    let mut pd: Option<Vec<u8>> = None;
    for f in &files {
        let d = std::fs::read(shell::path(f)).ok();
        if d.is_some() && pd.is_some() && d == pd {
            same += 1;
        }
        pd = d;
    }
    if same > 0 {
        if !one && every < 100 {
            say(format!(
                "{same} of {} frames identical to the one before: nothing new drawn between them, or the grabs held the box up (each is four Hyprland screencast events; a shell or widget that works on each stalls its timers and animations): --every 100ms or slower to tell",
                nf - 1
            ));
        } else {
            say(format!(
                "{same} of {} frames identical to the one before: nothing new drawn between them",
                nf - 1
            ));
        }
    }
    if !act_args.is_empty() && arc != 0 {
        let words: Vec<&[u8]> = act_args.iter().map(Vec::as_slice).collect();
        say(cat!(
            "the action (",
            words.join(&b' '),
            ") failed: exit ",
            n(i64::from(arc))
        ));
        return Err(Exit(1));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // #346: a name of another image format gets the note; .png, no extension or a dot folder none.
    #[test]
    fn png_named_note() {
        assert_eq!(
            png_named(b"/t/x.PPM"),
            Some(b"shot: /t/x.PPM holds a PNG (shot writes PNG only, whatever the name)".to_vec())
        );
        assert!(png_named(b"/t/x.jpg").is_some());
        for n in [
            &b"/t/x.png"[..],
            b"/t/x",
            b"/t/.ppm",
            b"/t.ppm/x",
            b"",
            b"x.ppm.png",
        ] {
            assert_eq!(png_named(n), None, "{}", String::from_utf8_lossy(n));
        }
    }

    // What bash's geom_clip printed for each, but numbers written as numbers and read as decimal
    // (finding 448: it printed X and Y as given, 010, and clipped them as octal).
    // t_unit_shot_hidden (finding 90): when an interactive box gives no frame, shot never says to
    // show its window but to ask the user; for a box started before drawn_hidden, or whose window
    // confirm-close replaced, that it needs a restart. grim failing otherwise: its reason.
    #[test]
    fn no_frame_as_bash() {
        let d = std::env::temp_dir().join(format!("omabox-shoth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let mut ctx = Ctx::new(b"shot", Vec::new());
        ctx.name = b"oldbox".to_vec();
        ctx.d = shell::bytes(&d).to_vec();
        let words = |grc| {
            String::from_utf8_lossy(&no_frame_words(&ctx, grc).unwrap_or_default()).into_owned()
        };
        std::fs::write(
            d.join("box.json"),
            r#"{"mode": "interactive", "workspace": "9"}"#,
        )
        .unwrap();
        let out = words(124);
        assert!(
            shell::re_match("needs a restart.*ask the user", out.as_bytes()),
            "an old interactive box: it needs a restart, ask the user: {out}"
        );
        assert!(!out.contains("omabox down"), "...not told to restart it");
        std::fs::write(
            d.join("box.json"),
            r#"{"mode": "interactive", "workspace": "9", "drawn_hidden": true}"#,
        )
        .unwrap();
        let out = words(124);
        assert!(
            out.contains("never switch the user's workspace or focus"),
            "a new one: never switch the user's workspace for a frame: {out}"
        );
        assert!(out.contains("ask the user"), "...ask the user");
        assert!(!out.contains("restart"), "...and no restart");
        assert_eq!(
            no_frame_words(&ctx, 1),
            None,
            "...grim failing otherwise: not no frame"
        );
        assert_eq!(
            grim_failed_words(b"grim: bad geometry".to_vec(), 1, b""),
            b"shot: grim failed: grim: bad geometry",
            "...its reason, not ask the user"
        );
        std::fs::create_dir_all(d.join("run")).unwrap();
        std::fs::write(d.join("run/omabox.reopened"), "1\n").unwrap();
        let out = words(124);
        assert!(
            shell::re_match(
                "confirm-close.*not drawn while hidden.*ask the user",
                out.as_bytes()
            ),
            "a window confirm-close reopened: not drawn while hidden, ask the user: {out}"
        );
        assert!(
            out.contains("Never switch the user's workspace or focus"),
            "...never switch the user's workspace for a frame"
        );
        std::fs::write(
            d.join("box.json"),
            r#"{"mode": "headless", "workspace": "9", "drawn_hidden": true}"#,
        )
        .unwrap();
        assert_eq!(
            no_frame_words(&ctx, 124),
            None,
            "a headless box: grim's words"
        );
        assert_eq!(
            grim_failed_words(Vec::new(), 124, b"MINE listed 0x0"),
            b"shot: grim failed: it gave nothing in 10 s; MINE listed 0x0",
            "...saying nothing (#217): how it ended, and a 0x0 output's note (#213)"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn clip_as_bash() {
        let c = |v: [&str; 6]| {
            clip(
                v[0].as_bytes(),
                v[1].as_bytes(),
                v[2].as_bytes(),
                v[3].as_bytes(),
                v[4].as_bytes(),
                v[5].as_bytes(),
            )
            .map(|(g, n)| (g.map(|w| String::from_utf8(w).unwrap_or_default()), n))
        };
        assert_eq!(
            c(["010", "0", "100", "50", "1920", "1080"]),
            Some((
                ["10".into(), "0".into(), "100".into(), "50".into()],
                Vec::new()
            ))
        );
        assert_eq!(
            c(["-5", "-5", "100", "50", "1920", "1080"]),
            Some((
                ["0".into(), "0".into(), "95".into(), "45".into()],
                b"5 px past the left edge, 5 px past the top edge".to_vec()
            ))
        );
        assert_eq!(
            c(["08", "0", "1", "1", "10", "10"]),
            Some((["8".into(), "0".into(), "1".into(), "1".into()], Vec::new()))
        );
        assert_eq!(c(["x", "0", "1", "1", "10", "10"]), None);
        assert_eq!(c(["5000", "0", "10", "10", "1920", "1080"]), None);
        assert_eq!(
            split_at(b"1920x1080@60 x", b' '),
            (b"1920x1080@60".to_vec(), b"x".to_vec())
        );
    }
}
