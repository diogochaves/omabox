//! `omabox find --image FILE [--similar 0.95] [-g GEOM | --window SEL] [--all] [--json]` (#267,
//! finding 616): where an image (a crop of an earlier shot) is on the box's screen, or in a window's
//! own pixels, for what has no accessibility tree (the shell, its plugins, the bar, terminals). Each
//! match is a box in the layout's coordinates with its score; exit 1 none. The search (imgfind.rs)
//! is shared with `click --image` and `wait image`: prepare reads the template once (scaled to the
//! box when its shot's comment says another scale), grab takes a frame of the screen or the window
//! each time.

use crate::args::{optarg, unexpected, unknown_opt};
use crate::boxes::{box_alive, need_box};
use crate::cat;
use crate::exec::{ErrTo, Out, on_box};
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::hypr::{hung_text, hypr_answers};
use crate::imgfind::{self, Found, Rgb, Tpl};
use crate::jail::in_jail;
use crate::jq::{self, Args};
use crate::screen::extent;
use crate::select::select_box;
use crate::settle::geom_parse;
use crate::shell::{self, dec};
use crate::shellnote::shell_note;
use crate::win::{box_windows, jq_r, win_desc, win_select};
use std::io::{Read, Write};

/// The largest template file read.
const MAX_FILE: u64 = 32 << 20;

/// The score a match needs by default; a template scaled to the box (drawn at another scale, its
/// edges resampled) SCALED_SIM.
pub const SIM: f64 = 0.95;
pub const SCALED_SIM: f64 = 0.8;

/// What to look for and where: the template's file (a jailed caller's /dev/fd/N), the score a match
/// needs (--similar; None: SIM, SCALED_SIM for a scaled template), a region of the screen or a window.
#[derive(Clone, Default)]
pub struct Spec {
    pub file: Vec<u8>,
    pub sim: Option<f64>,
    pub geom: Option<[i64; 4]>,
    pub sels: Vec<Vec<u8>>,
}

/// --similar's value: a number over 0, at most 1.
pub fn similar_arg(ctx: &Ctx, cmd: &str, v: &[u8]) -> Res<f64> {
    let s: Option<f64> = shell::re_match(r"^(0?\.[0-9]{1,4}|0|1|1\.0*)$", v)
        .then(|| std::str::from_utf8(v).ok()?.parse().ok())
        .flatten();
    match s {
        Some(s) if s > 0.0 => Ok(s),
        _ => Err(ctx.die(cat!(
            cmd,
            ": --similar takes a score over 0, at most 1 (0.95: nearly the same pixels), got ",
            v
        ))),
    }
}

/// Where a search looks: a region of the screen, or a window's own pixels (by address: the same
/// window each time).
#[derive(Clone)]
enum Area {
    Screen([i64; 4]),
    Window { addr: Vec<u8>, desc: Vec<u8> },
}

/// A frame: its pixels, and where they are: the layout's region they cover (a window's place and
/// size), the window's address when it is one.
pub struct Frame {
    img: Rgb,
    at: [i64; 4],
    window: Option<Vec<u8>>,
    /// Where the pointer is drawn, in the image's pixels: not compared.
    ptr: imgfind::Ignore,
}

/// Why there is no frame: the window gone (or not to be captured), or the capture failed.
pub enum NoFrame {
    Gone(Vec<u8>),
    Failed(Vec<u8>),
}

/// A match on the box: its box in the layout's coordinates, in the window's own when in one, its
/// score.
#[derive(Clone)]
pub struct Match {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    pub score: f64,
    pub win: Option<(Vec<u8>, i64, i64)>,
}

impl Match {
    /// `X,Y WxH SCORE`.
    pub fn line(&self) -> Vec<u8> {
        format!(
            "{},{} {}x{} {:.3}",
            self.x, self.y, self.w, self.h, self.score
        )
        .into_bytes()
    }

    /// Its middle, in the layout.
    pub fn middle(&self) -> (i64, i64) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }
}

/// The matches of one frame, the best score seen (with where), whether there were more than kept.
pub struct Seen {
    pub hits: Vec<Match>,
    pub best: Option<Match>,
    pub more: bool,
    pub flat: bool,
}

/// A template read and scaled for its area: what grab and look need.
pub struct Finder {
    area: Area,
    tpl: Tpl,
    pub name: Vec<u8>,
    pub sim: f64,
    /// Said once: the template scaled to the box.
    pub note: Vec<u8>,
    /// The template was scaled to the box: both softened before they are compared.
    soften: bool,
}

/// The template's bytes and its name: the file (a jailed caller's /dev/fd/N its omabox opened, named
/// as $OMABOX_RELAY_IN_AS), at most MAX_FILE.
fn read_template(ctx: &Ctx, label: &str, file: &[u8]) -> Res<(Vec<u8>, Vec<u8>)> {
    let (path, name) = if in_jail() {
        let as_ = shell::env_or_empty("OMABOX_RELAY_IN_AS");
        if !(shell::re_match("^/dev/fd/[3-9]$", file)
            && shell::path(file).is_file()
            && !as_.is_empty())
        {
            let mut c = ctx.clone();
            c.parsed();
            return Err(c.die(cat!(
                label,
                ": a jailed agent's image is read by its own omabox, never at a path here"
            )));
        }
        (file.to_vec(), as_)
    } else {
        if !shell::path(file).is_file() {
            return Err(ctx.die(cat!(label, ": no such file: ", file)));
        }
        (file.to_vec(), file.to_vec())
    };
    let mut b = Vec::new();
    let r = std::fs::File::open(shell::path(&path))
        .and_then(|f| f.take(MAX_FILE + 1).read_to_end(&mut b));
    if let Err(e) = r {
        return Err(ctx.die(cat!(
            label,
            ": cannot read ",
            name,
            ": ",
            crate::shots::why(&e)
        )));
    }
    if b.len() as u64 > MAX_FILE {
        return Err(ctx.die(cat!(label, ": ", name, " is over 32 MiB: crop it")));
    }
    if !imgfind::is_png(&b) {
        return Err(ctx.die(cat!(
            label,
            ": ",
            name,
            " is not a PNG (crop one from a shot: omabox shot -g \"X,Y WxH\" -o FILE)"
        )));
    }
    Ok((b, name))
}

/// The PNG's pixels and alpha (ImageMagick, its PNG reader only), resized to W x H when asked.
fn decode(b: &[u8], size: Option<(usize, usize)>) -> Option<Tpl> {
    let mut c = crate::sig::command("magick");
    c.arg("png:-");
    if let Some((w, h)) = size {
        c.arg("-resize").arg(format!("{w}x{h}!"));
    }
    c.args([
        "-alpha",
        "set",
        "-type",
        "TrueColorAlpha",
        "-depth",
        "8",
        "pam:-",
    ])
    .stdin(std::process::Stdio::piped())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::null());
    let mut ch = c.spawn().ok()?;
    let mut stdin = ch.stdin.take()?;
    let data = b.to_vec();
    let w = std::thread::spawn(move || {
        let _ = stdin.write_all(&data);
    });
    let mut o = Vec::new();
    let _ = ch.stdout.take()?.take(512 << 20).read_to_end(&mut o);
    let st = ch.wait().ok()?;
    let _ = w.join();
    if !st.success() {
        return None;
    }
    imgfind::pam(&o)
}

/// prepare LABEL SPEC: the template read, the area resolved, a first frame grabbed, and the template
/// scaled to it when its shot was taken at another scale (said in .note).
pub fn prepare(ctx: &Ctx, label: &str, spec: &Spec) -> Res<(Finder, Frame)> {
    let (bytes, name) = read_template(ctx, label, &spec.file)?;
    let area = if spec.sels.is_empty() {
        let ext = extent(ctx)?;
        let e = shell::read_ifs(&ext, 2, b"x");
        let (Some(lw), Some(lh)) = (dec(&e[0]), dec(&e[1])) else {
            return Err(ctx.die(cat!(label, ": cannot read the screen's size (", ext, ")")));
        };
        match spec.geom {
            None => Area::Screen([0, 0, lw, lh]),
            Some(r) => match crate::shots::geom_clip(r, lw, lh) {
                Some((g, _)) => Area::Screen(g),
                None => {
                    return Err(ctx.die(cat!(
                        label,
                        ": -g is off the screen (",
                        lw.to_string(),
                        "x",
                        lh.to_string(),
                        ")"
                    )));
                }
            },
        }
    } else {
        let wins = box_windows(ctx, None)?;
        let w = win_select(ctx, &wins, &spec.sels)?;
        Area::Window {
            addr: jq_r(".address", &w),
            desc: win_desc(&w),
        }
    };
    let mut f = Finder {
        area,
        tpl: Tpl {
            w: 0,
            h: 0,
            px: Vec::new(),
            opaque: None,
        },
        name,
        sim: spec.sim.unwrap_or(SIM),
        note: Vec::new(),
        soften: false,
    };
    // (a window's first capture can be an old frame: taken twice, as shot does)
    if matches!(f.area, Area::Window { .. }) {
        let _ = f.grab(ctx);
    }
    let frame = match f.grab(ctx)? {
        Ok(fr) => fr,
        Err(NoFrame::Gone(why) | NoFrame::Failed(why)) => {
            return Err(f.no_frame(ctx, label, &why));
        }
    };
    let lw = frame.at[2].max(1) as f64;
    let here = frame.img.w as f64 / lw;
    let mut size = None;
    if let Some(s) = imgfind::png_scale(&bytes) {
        let k = here / s;
        if (k - 1.0).abs() > 0.02 {
            // (its size from its pixels as decoded: the PNG's header)
            let (w, h) = imgfind::png_dims(&bytes).unwrap_or((0, 0));
            let (nw, nh) = (
                ((w as f64 * k).round() as usize).max(1),
                ((h as f64 * k).round() as usize).max(1),
            );
            size = Some((nw, nh));
            f.soften = true;
            let sim = match spec.sim {
                Some(v) => cat!("--similar ", fmt_scale(v)),
                None => {
                    f.sim = SCALED_SIM;
                    cat!("--similar ", fmt_scale(SCALED_SIM), " as it is scaled")
                }
            };
            f.note = cat!(
                label,
                ": ",
                f.name,
                " was shot at scale ",
                fmt_scale(s),
                ", the box draws at ",
                fmt_scale(here),
                ": matched at ",
                nw.to_string(),
                "x",
                nh.to_string(),
                ", ",
                sim
            );
        }
    }
    let Some(tpl) = decode(&bytes, size) else {
        return Err(ctx.die(cat!(label, ": ImageMagick could not read ", f.name)));
    };
    if tpl.w < imgfind::MIN_SIDE || tpl.h < imgfind::MIN_SIDE {
        return Err(ctx.die(cat!(
            label,
            ": ",
            f.name,
            " is ",
            tpl.w.to_string(),
            "x",
            tpl.h.to_string(),
            ": too small to find (4 px a side at least)"
        )));
    }
    if tpl.counted() == 0 {
        return Err(ctx.die(cat!(label, ": ", f.name, " is all transparent")));
    }
    if tpl.w > frame.img.w || tpl.h > frame.img.h {
        return Err(ctx.die(cat!(
            label,
            ": ",
            f.name,
            " (",
            tpl.w.to_string(),
            "x",
            tpl.h.to_string(),
            " px) is larger than where it is looked for (",
            frame.img.w.to_string(),
            "x",
            frame.img.h.to_string(),
            " px)"
        )));
    }
    f.tpl = tpl;
    Ok((f, frame))
}

/// Where the pointer at P ("X Y", the layout's) is drawn in an image of AT (a region of the layout)
/// IW x IH px: 12 px up and left of its point to 28 down and right, which holds an arrow (its point at
/// the top left) and an I-beam (its middle); None off the image. (A window's frame is its own pixels:
/// the pointer over its place is not in them, and they are left out the same, harmlessly.)
fn ptr_region(p: &[u8], at: [i64; 4], iw: usize, ih: usize) -> imgfind::Ignore {
    let w = shell::words(p);
    let (px, py) = (dec(w.first()?)?, dec(w.get(1)?)?);
    let (rx, ry) = (
        iw as f64 / at[2].max(1) as f64,
        ih as f64 / at[3].max(1) as f64,
    );
    let to = |v: i64, o: i64, r: f64, max: usize| -> usize {
        (((v - o) as f64 * r).round().max(0.0) as usize).min(max)
    };
    let (x0, y0) = (to(px - 12, at[0], rx, iw), to(py - 12, at[1], ry, ih));
    let (x1, y1) = (to(px + 28, at[0], rx, iw), to(py + 28, at[1], ry, ih));
    (x1 > x0 && y1 > y0).then_some([x0, y0, x1, y1])
}

/// A scale as said: 1, 1.5, 1.25.
fn fmt_scale(s: f64) -> String {
    let t = format!("{s:.3}");
    t.trim_end_matches('0').trim_end_matches('.').to_string()
}

impl Finder {
    /// grab: a frame of the area now; Err(NoFrame) when there is none: the window gone (an answer), or
    /// grim's failure. A box that is not there to ask ends the command (on_box's refusal).
    pub fn grab(&self, ctx: &Ctx) -> Res<Result<Frame, NoFrame>> {
        let mut grim: Vec<Vec<u8>> = ["timeout", "10", "grim", "-t", "ppm"]
            .iter()
            .map(|w| w.as_bytes().to_vec())
            .collect();
        let (at, window) = match &self.area {
            Area::Screen(g) => {
                grim.extend([
                    b"-g".to_vec(),
                    format!("{},{} {}x{}", g[0], g[1], g[2], g[3]).into_bytes(),
                ]);
                (*g, None)
            }
            Area::Window { addr, .. } => {
                let wins = crate::exit::muted(|| box_windows(ctx, None));
                let Ok(wins) = wins else {
                    return Ok(Err(NoFrame::Failed(
                        b"the box did not list its windows".to_vec(),
                    )));
                };
                let w = Args::new(&["-c"])
                    .kv("--arg", "a", addr)
                    .arg(crate::jqprog::BY_ADDRESS_JQ)
                    .run(Some(&wins), jq::Err::Null)
                    .text();
                if w.is_empty() {
                    return Ok(Err(NoFrame::Gone(b"the window is gone".to_vec())));
                }
                let id = jq_r(".stableId", &w);
                let n = |p: &str| dec(&jq_r(p, &w)).unwrap_or(0);
                let g = [n(".at[0]"), n(".at[1]"), n(".size[0]"), n(".size[1]")];
                if !shell::re_match("^[0-9a-fA-F]+$", &id) || g[2] < 1 || g[3] < 1 {
                    return Ok(Err(NoFrame::Gone(cat!(
                        "window ",
                        win_desc(&w),
                        " cannot be captured"
                    ))));
                }
                grim.extend([b"-T".to_vec(), id]);
                (g, Some(addr.clone()))
            }
        };
        grim.push(b"-".to_vec());
        // (bounded: the region at scale 4, and the header)
        let cap = usize::try_from(at[2].saturating_mul(at[3]))
            .unwrap_or(0)
            .saturating_mul(3 * 16)
            .saturating_add(4096);
        // The pointer, read just before: a box's frames have it drawn in, over what it is on (a
        // click --image leaves it on the match). Its place no match is judged by.
        let p = crate::exit::muted(|| crate::settle::cursor_now(ctx)).unwrap_or_default();
        let r = on_box(ctx, &grim, true, Out::Capture(Some(cap)), ErrTo::Null).or_exit()?;
        match imgfind::ppm(&r.out) {
            Some(img) if r.rc == 0 => {
                let ptr = ptr_region(&p, at, img.w, img.h);
                Ok(Ok(Frame {
                    img,
                    at,
                    window,
                    ptr,
                }))
            }
            _ => Ok(Err(NoFrame::Failed(
                format!("grim: exit {}", r.rc).into_bytes(),
            ))),
        }
    }

    /// The command ended on a frame it could not take: the box gone or hung said, else why.
    pub fn no_frame(&self, ctx: &Ctx, label: &str, why: &[u8]) -> Exit {
        match box_alive(ctx) {
            Ok(true) => {}
            Ok(false) => return ctx.die(cat!(label, ": box '", ctx.name, "' went down")),
            Err(e) => return e,
        }
        match hypr_answers(ctx) {
            Ok(true) => {}
            Ok(false) => {
                return match hung_text(ctx, "") {
                    Ok(t) => ctx.die(cat!(label, ": ", t)),
                    Err(e) => e,
                };
            }
            Err(e) => return e,
        }
        let what: Vec<u8> = match &self.area {
            Area::Screen(_) => b"the screen".to_vec(),
            Area::Window { desc, .. } => cat!("window ", desc),
        };
        ctx.die(cat!(label, ": no frame of ", what, " (", why, ")"))
    }

    /// The matches in FRAME.
    pub fn look(&self, fr: &Frame) -> Seen {
        let Found {
            hits,
            best,
            more,
            flat,
        } = imgfind::search(&fr.img, &self.tpl, self.sim, fr.ptr, self.soften);
        let (rx, ry) = (
            fr.img.w as f64 / fr.at[2].max(1) as f64,
            fr.img.h as f64 / fr.at[3].max(1) as f64,
        );
        let (tw, th) = (self.tpl.w, self.tpl.h);
        let to = |h: imgfind::Hit| {
            let (lx, ly) = (
                (h.x as f64 / rx).round() as i64,
                (h.y as f64 / ry).round() as i64,
            );
            Match {
                x: fr.at[0] + lx,
                y: fr.at[1] + ly,
                w: ((tw as f64 / rx).round() as i64).max(1),
                h: ((th as f64 / ry).round() as i64).max(1),
                score: h.score,
                win: fr.window.as_ref().map(|a| (a.clone(), lx, ly)),
            }
        };
        Seen {
            hits: hits.into_iter().map(to).collect(),
            best: best.map(to),
            more,
            flat,
        }
    }

    /// What was looked in, as said: "the screen", "the screen's X,Y WxH", "window DESC".
    pub fn where_(&self) -> Vec<u8> {
        match &self.area {
            Area::Screen(g) if g[0] == 0 && g[1] == 0 => b"the screen".to_vec(),
            Area::Screen(g) => {
                format!("the screen's {},{} {}x{}", g[0], g[1], g[2], g[3]).into_bytes()
            }
            Area::Window { desc, .. } => cat!("window ", desc),
        }
    }

    /// "no match of NAME in WHERE at SIM (best S at X,Y WxH)".
    pub fn none_words(&self, s: &Seen) -> Vec<u8> {
        let best = match &s.best {
            Some(b) => {
                format!(" (best {:.3} at {},{} {}x{})", b.score, b.x, b.y, b.w, b.h).into_bytes()
            }
            None => Vec::new(),
        };
        cat!(
            "no match of ",
            self.name,
            " in ",
            self.where_(),
            " at --similar ",
            fmt_scale(self.sim),
            best
        )
    }
}

/// A probe's detail of what it saw: "at X,Y WxH (S)", "N matches: ...; ...", "best S at ...".
pub fn seen_words(s: &Seen) -> Vec<u8> {
    let one = |m: &Match| format!("{},{} {}x{} ({:.3})", m.x, m.y, m.w, m.h, m.score);
    match s.hits.len() {
        0 => s.best.as_ref().map_or_else(Vec::new, |b| {
            format!("best {:.3} at {},{} {}x{}", b.score, b.x, b.y, b.w, b.h).into_bytes()
        }),
        1 => format!("at {}", one(&s.hits[0])).into_bytes(),
        n => {
            let l: Vec<String> = s.hits.iter().take(3).map(one).collect();
            let more = if n > 3 { "; ..." } else { "" };
            format!("{n} matches: {}{more}", l.join("; ")).into_bytes()
        }
    }
}

/// The lines of several matches, numbered for --index, at most CAP (and how many more).
pub fn numbered(hits: &[Match], cap: usize) -> Vec<u8> {
    let mut t = Vec::new();
    for (i, m) in hits.iter().take(cap).enumerate() {
        t.extend_from_slice(&cat!("  ", (i + 1).to_string(), ": ", m.line(), "\n"));
    }
    if hits.len() > cap {
        t.extend_from_slice(format!("  ... and {} more\n", hits.len() - cap).as_bytes());
    }
    t
}

/// A match as JSON.
fn match_json(m: &Match) -> String {
    let (cx, cy) = m.middle();
    let win = match &m.win {
        Some((a, x, y)) => format!(
            ",\"window\":{{\"address\":{},\"x\":{x},\"y\":{y}}}",
            crate::json::quote(&String::from_utf8_lossy(a))
        ),
        None => String::new(),
    };
    format!(
        "{{\"x\":{},\"y\":{},\"w\":{},\"h\":{},\"middle\":[{cx},{cy}],\"score\":{:.4}{win}}}",
        m.x, m.y, m.w, m.h, m.score
    )
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut json, mut all) = (Vec::new(), false, false);
    let mut spec = Spec::default();
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
            b"--image" => {
                optarg(ctx, a, v)?;
                spec.file = args[i + 1].clone();
                i += 1;
            }
            b"--similar" => {
                optarg(ctx, a, v)?;
                spec.sim = Some(similar_arg(ctx, "find", &args[i + 1])?);
                i += 1;
            }
            b"-g" => {
                optarg(ctx, a, v)?;
                let Some(g) = geom_parse(&args[i + 1]) else {
                    return Err(ctx.die(cat!("find: -g takes \"X,Y WxH\", got ", args[i + 1])));
                };
                let n = |k: usize| dec(&g[k]).unwrap_or(0);
                spec.geom = Some([n(0), n(1), n(2), n(3)]);
                i += 1;
            }
            b"-w" | b"--window" => {
                optarg(ctx, a, v)?;
                spec.sels.push(args[i + 1].clone());
                i += 1;
            }
            b"--active" => spec.sels.push(b"active".to_vec()),
            b"--all" => all = true,
            b"--json" => json = true,
            b"-h" | b"--help" => return crate::args::usage("find"),
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "find", a)),
            _ => {
                return Err(unexpected(
                    ctx,
                    "find",
                    a,
                    "(find --image FILE: a PNG, a crop of a shot)",
                ));
            }
        }
        i += 1;
    }
    if spec.file.is_empty() {
        return Err(ctx.die(
            "find: what? --image FILE (a crop of a shot: omabox shot -g \"X,Y WxH\" -o FILE)",
        ));
    }
    if spec.geom.is_some() && !spec.sels.is_empty() {
        return Err(ctx.die("find: -g or --window, not both"));
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    shell_note(ctx)?;
    let (f, fr) = prepare(ctx, "find", &spec)?;
    if !f.note.is_empty() {
        say(&f.note);
    }
    let s = f.look(&fr);
    if json {
        let ms: Vec<String> = s.hits.iter().map(match_json).collect();
        let best = s.best.as_ref().map_or("null".to_string(), match_json);
        out(format!(
            "{{\"image\":{},\"similar\":{},\"matches\":[{}],\"more\":{},\"best\":{best}}}\n",
            crate::json::quote(&String::from_utf8_lossy(&f.name)),
            fmt_scale(f.sim),
            ms.join(","),
            s.more
        ))?;
        return if s.hits.is_empty() {
            Err(Exit(1))
        } else {
            Ok(())
        };
    }
    if s.hits.is_empty() {
        say(cat!("find: ", f.none_words(&s)));
        return Err(Exit(1));
    }
    let cap = if all { usize::MAX } else { 20 };
    let mut t = Vec::new();
    for m in s.hits.iter().take(cap) {
        t.extend_from_slice(&cat!(m.line(), "\n"));
    }
    out(t)?;
    let n = s.hits.len();
    if n > 1 {
        let many = if s.more { "more than " } else { "" };
        let one = if s.flat {
            " (an image of one colour matches any place of that colour)"
        } else {
            ""
        };
        say(format!(
            "find: {many}{n} matches in reading order{one}: click --image FILE --index N picks one"
        ));
        if n > cap {
            say(format!(
                "find: {} more not listed (--all lists them)",
                n - cap
            ));
        }
    }
    Ok(())
}
