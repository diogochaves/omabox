//! What click, scroll, pointer, drag and keys did, for those who look on: a peek window's marks
//! (finding 85, $D/marks, the formats in tools/peek) and a recording's input lines (#176, finding 319,
//! $D/record.input). Both in the box dir, which the box cannot see: nothing in it can forge them or
//! read them (a --pass value is only its length there).

use crate::cat;
use crate::exit::Ctx;
use crate::shell::{self, cntrl_to_space};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

/// $D/record.input past this is left as it is.
const REC_INPUT_MAX: u64 = 4_194_304;
/// $D/marks past this is emptied (peek starts over).
const MARKS_MAX: u64 = 65_536;

/// `${EPOCHREALTIME//[!0-9]/}`: now, in µs, as the digits of seconds and their 6 decimals.
pub fn epoch_us() -> Vec<u8> {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}{:06}", d.as_secs(), d.subsec_micros()).into_bytes()
}

/// `printf '%s\n' LINES >> FILE` under umask 077, its failure unsaid.
fn append(p: &std::path::Path, data: &[u8]) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(p)
    {
        let _ = f.write_all(data);
    }
}

/// `stat -c %s FILE`.
fn size(p: &std::path::Path) -> Option<u64> {
    std::fs::metadata(p).ok().map(|m| m.len())
}

/// Whether one of this box's peek windows is open (`peek_pid >/dev/null`).
pub fn peek_open(ctx: &Ctx) -> bool {
    peek_pid(ctx, None).is_some()
}

/// peek_pid [OUTPUT]: the pid of this box's peek window, if one is open, the lowest (`pgrep -f
/// "^$PEEK --box $D/(run/|wayland )" | head -1`): a process whose command line, its words joined by
/// spaces, starts so (the box dir's socket link since finding 751, its runtime dir's socket before);
/// with OUTPUT, the one showing that monitor (#122: `.* --output OUTPUT --fps ` after it).
pub fn peek_pid(ctx: &Ctx, output: Option<&[u8]>) -> Option<Vec<u8>> {
    let want = cat!(
        shell::bytes(&crate::root().join("tools/peek/omabox-peek")),
        " --box ",
        ctx.d,
        "/"
    );
    // (in this process, not a foreground step: an INT here ends omabox, as one during bash's
    // pgrep ends that and bash with it when it is the group's)
    scan(&want, output, true).into_iter().next()
}

/// peek_pids: the pids of this box's peek windows, lowest first (a window per monitor shown, #122).
pub fn peek_pids(ctx: &Ctx) -> Vec<Vec<u8>> {
    let want = cat!(
        shell::bytes(&crate::root().join("tools/peek/omabox-peek")),
        " --box ",
        ctx.d,
        "/"
    );
    scan(&want, None, false)
}

/// The pids whose command line (words joined by spaces) starts with WANT (a box dir's `/`) then
/// `run/` or the `wayland` link (and holds OUTPUT's words
/// after it), lowest first; the first alone with FIRST.
fn scan(want: &[u8], output: Option<&[u8]>, first: bool) -> Vec<Vec<u8>> {
    let tail = output.map(|o| cat!(" --output ", o, " --fps "));
    let me = std::process::id();
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut pids: Vec<u32> = rd
        .flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()))
        .filter(|&p| p != me)
        .collect();
    pids.sort_unstable();
    for p in pids {
        let Ok(mut c) = std::fs::read(format!("/proc/{p}/cmdline")) else {
            continue;
        };
        while c.last() == Some(&0) {
            c.pop();
        }
        for b in &mut c {
            if *b == 0 {
                *b = b' ';
            }
        }
        let Some(rest) = c.strip_prefix(want) else {
            continue;
        };
        if !(rest.starts_with(b"run/") || rest == b"wayland" || rest.starts_with(b"wayland ")) {
            continue;
        }
        if tail
            .as_ref()
            .is_none_or(|t| rest.windows(t.len()).any(|w| w == t.as_slice()))
        {
            found.push(p.to_string().into_bytes());
            if first {
                break;
            }
        }
    }
    found
}

/// rec_input T0 LINE...: while the box records, each line with when it began (T0, µs; now when T0
/// is not a time before now) and ended (now), its control characters spaces.
pub fn rec_input(ctx: &Ctx, t0: &[u8], lines: &[Vec<u8>]) {
    if lines.is_empty() || !ctx.at("record").is_file() {
        return;
    }
    let t1 = epoch_us();
    let num = |v: &[u8]| {
        std::str::from_utf8(v)
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
    };
    let ok =
        shell::re_match("^[0-9]{1,17}$", t0) && num(t0).zip(num(&t1)).is_some_and(|(a, b)| a <= b);
    let t0 = if ok { t0.to_vec() } else { t1.clone() };
    let f = ctx.at("record.input");
    // (`stat ... || echo 0`: one that cannot be read is written to)
    if f.is_file() && size(&f).unwrap_or(0) > REC_INPUT_MAX {
        return;
    }
    let mut data = Vec::new();
    for l in lines {
        data.extend(cat!(t0, " ", t1, " ", cntrl_to_space(l), "\n"));
    }
    append(&f, &data);
}

/// rec_glide: the glide (ms) `record start --glide` set for this box's click, pointer and drag while
/// it records; None when there is none.
pub fn rec_glide(ctx: &Ctx) -> Option<Vec<u8>> {
    if !ctx.at("record").is_file() {
        return None;
    }
    let p = ctx.at("record.glide");
    if !p.is_file() {
        return None;
    }
    // `read -r g`: its first line, which must end in a newline, blanks around it gone.
    let v = std::fs::read(p).ok()?;
    let i = v.iter().position(|&c| c == b'\n')?;
    let g = shell::read_fields(&v[..i], 1).into_iter().next()?;
    shell::re_match("^[1-9][0-9]{0,4}$", &g).then_some(g)
}

/// mark LINE...: for a peek window to draw, only while one is open; and (unless NOREC: the caller
/// said its own) into a recording's input, from T0.
pub fn mark(ctx: &Ctx, t0: &[u8], norec: bool, lines: &[Vec<u8>]) {
    if lines.is_empty() {
        return;
    }
    if !norec {
        rec_input(ctx, t0, lines);
    }
    let f = ctx.at("marks");
    if !peek_open(ctx) {
        let _ = std::fs::remove_file(&f);
        return;
    }
    if f.is_file() && size(&f).is_some_and(|s| s > MARKS_MAX) {
        let _ = std::fs::write(&f, b"");
    }
    let mut data = Vec::new();
    for l in lines {
        data.extend(cat!(l, "\n"));
    }
    append(&f, &data);
}
