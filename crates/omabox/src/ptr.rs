//! The pointer hidden (#228, finding 305): Hyprland's cursor:invisible leaves it out of every frame
//! (shots, recordings, peek); it still moves and clicks, and focus still follows it. What keeps it
//! hidden is $D/pointer, in the box dir the box cannot see, a line each: `hidden` (`pointer hide`),
//! `hold shot PID` (a `shot --no-pointer`, gone with omabox PID), `hold record PID`, `was
//! true|false` (cursor:invisible before omabox hid it). The box's config hides it on every load while
//! its runtime dir's omabox.pointer says so. Changed under $D/.pointer.lock.

use crate::boxes::{box_alive, box_file, lock_file, meta, run_read};
use crate::cat;
use crate::exec::{ErrTo, Out, on_box};
use crate::exit::{Ctx, Exit, Res, say};
use crate::hypr::{HYPR_LIMIT, hung_text, hypr_answers, hypr_down_text};
use crate::input::{wh, xy};
use crate::procfs::running;
use crate::screen::extent;
use crate::settle::cursor_now;
use crate::shell::{self, dec};
use std::sync::Mutex;
use std::time::Duration;

/// PTR_ERR: what Hyprland said when it did not answer as expected. A failure inside `$(...)` (the
/// `live=$(ptr_live)` reads) never sets it in bash: there it stays as it was.
#[derive(Default)]
pub struct Ptr {
    pub err: Vec<u8>,
    /// PTR_HOLDS: the holds left after ptr_change.
    pub holds: usize,
}

/// `out=$(on_box ARGS 2>&1) || true`, as `$(...)` has it.
fn said(ctx: &Ctx, args: &[&[u8]]) -> (i32, Vec<u8>) {
    let a: Vec<Vec<u8>> = args.iter().map(|a| a.to_vec()).collect();
    let r = on_box(ctx, &a, false, Out::Capture(None), ErrTo::Out);
    (r.rc, shell::subst(&r.out))
}

/// ptr_live: cursor:invisible in the box, "true" or "false"; None (PTR_ERR's words) when Hyprland
/// did not answer so.
fn ptr_live_get(ctx: &Ctx) -> Result<Vec<u8>, Vec<u8>> {
    let (_, out) = said(
        ctx,
        &[
            b"timeout",
            HYPR_LIMIT.as_bytes(),
            b"hyprctl",
            b"eval",
            b"error('omabox-ptr:' .. tostring(hl.get_config('cursor.invisible')), 0)",
        ],
    );
    match out.as_slice() {
        b"error: omabox-ptr:true" => Ok(b"true".to_vec()),
        b"error: omabox-ptr:false" => Ok(b"false".to_vec()),
        _ => Err(out),
    }
}

/// `ptr_live true|false`: cursor:invisible set; Hyprland's words when it does not say ok.
fn ptr_live_set(ctx: &Ctx, v: &[u8]) -> Result<(), Vec<u8>> {
    let (_, out) = said(
        ctx,
        &[
            b"timeout",
            HYPR_LIMIT.as_bytes(),
            b"hyprctl",
            b"eval",
            &cat!("hl.config({ cursor = { invisible = ", v, " } })"),
        ],
    );
    if out == b"ok" { Ok(()) } else { Err(out) }
}

/// ptr_seen's watch in the box: the 64x64 around the pointer before the change, then until it
/// differs, 1 s at most (a pointer not drawn either way never changes it).
const SEEN_SH: &str = "b=$(grim -g \"$1\" -t ppm - 2>/dev/null | cksum)\n    r=$(hyprctl eval \"hl.config({ cursor = { invisible = $2 } })\" 2>&1)\n    [ \"$r\" = ok ] || { printf \"%s\" \"$r\"; exit 1; }\n    case $b in *\" 0\") exit 0 ;; esac\n    end=$(($(date +%s%N) / 1000000 + 1000))\n    while [ $(($(date +%s%N) / 1000000)) -lt $end ]; do\n      [ \"$(grim -g \"$1\" -t ppm - 2>/dev/null | cksum)\" = \"$b\" ] || exit 0\n      sleep 0.02\n    done";

/// ptr_seen true|false: set, and waited for in the frame (Hyprland draws it 50-400 ms later, finding
/// 305): the 64x64 around the pointer changing from what it was just before.
fn ptr_seen(ctx: &Ctx, v: &[u8]) -> Result<(), Vec<u8>> {
    let p = crate::exit::muted(|| cursor_now(ctx).ok().filter(|p| !p.is_empty()));
    let Some(p) = p else {
        return ptr_live_set(ctx, v);
    };
    let ext = crate::exit::muted(|| extent(ctx).ok());
    let Some(ext) = ext else {
        return ptr_live_set(ctx, v);
    };
    let (px, py) = xy(&p);
    let (ew, eh) = wh(&ext);
    let n = |v: &[u8]| dec(v).unwrap_or(0);
    let mut x = n(&px).saturating_sub(16);
    let mut y = n(&py).saturating_sub(16);
    x = x.max(0);
    y = y.max(0);
    let w = n(&ew).saturating_sub(x).min(64);
    let h = n(&eh).saturating_sub(y).min(64);
    if w <= 0 || h <= 0 {
        return ptr_live_set(ctx, v);
    }
    let g = format!("{x},{y} {w}x{h}");
    let (rc, out) = said(
        ctx,
        &[
            b"timeout",
            b"15",
            b"sh",
            b"-c",
            SEEN_SH.as_bytes(),
            b"sh",
            g.as_bytes(),
            v,
        ],
    );
    if rc == 0 { Ok(()) } else { Err(out) }
}

/// What ptr_change does to the record.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Hide,
    Show,
    Hold,
    Release,
}

/// ptr_change OP [TAG [WAIT]]: change what keeps the pointer hidden, and make the box match: the
/// record first (a reload meanwhile reads it), then cursor:invisible. TAG `shot PID` or `record PID`
/// (`release record` takes any recording's). WAIT: a pointer it hides or shows is waited for in the
/// frame (ptr_seen). Holds whose omabox is gone are dropped (a recording's once $D/record is gone
/// too). Err(()) when Hyprland did not answer as asked (PTR_ERR in P); a busy lock ends the command.
pub fn ptr_change(ctx: &Ctx, p: &mut Ptr, op: Op, tag: &[u8], wait: bool) -> Res<Result<(), ()>> {
    let Some(lock) = lock_file(&ctx.at(".pointer.lock"), Duration::from_secs(10)) else {
        return Err(ctx.die(cat!(
            "box '",
            ctx.name,
            "''s pointer is busy (another omabox is changing it)"
        )));
    };
    let (mut hidden, mut was, mut holds, mut on0) =
        (false, Vec::new(), Vec::<Vec<u8>>::new(), false);
    let rec = ctx.at("pointer");
    if rec.is_file() {
        let data = std::fs::read(&rec).unwrap_or_default();
        // (`while read`: a last line without its newline is not read)
        let mut lines: Vec<&[u8]> = data.split(|&c| c == b'\n').collect();
        lines.pop();
        for l in lines {
            if l == b"hidden" || l.starts_with(b"hold ") {
                on0 = true;
            }
            let pid_ok = |pid: &[u8]| shell::is_num(pid) && running(pid);
            if l == b"hidden" {
                hidden = true;
            } else if l == b"was true" || l == b"was false" {
                was = l[4..].to_vec();
            } else if let Some(pid) = l.strip_prefix(b"hold shot ".as_slice()) {
                if pid_ok(pid) {
                    holds.push(l.to_vec());
                }
            } else if let Some(pid) = l.strip_prefix(b"hold record ".as_slice()) {
                if shell::is_num(pid) && (running(pid) || ctx.at("record").is_file()) {
                    holds.push(l.to_vec());
                }
            }
        }
    }
    match op {
        Op::Hide => hidden = true,
        Op::Show => {
            hidden = false;
            was = b"false".to_vec();
        }
        Op::Hold | Op::Release => {
            let full = cat!("hold ", tag);
            let pre = cat!("hold ", tag, " ");
            holds.retain(|l| !(l == &full || l.starts_with(&pre)));
            if op == Op::Hold {
                holds.push(full);
            }
        }
    }
    let on1 = hidden || !holds.is_empty();
    p.holds = holds.len();
    let mut ok = true;
    if on1 {
        let Ok(live) = ptr_live_get(ctx) else {
            drop(lock);
            return Ok(Err(()));
        };
        if !(on0 && !was.is_empty()) {
            was = live.clone();
        }
        let mut body = Vec::new();
        if hidden {
            body.extend(b"hidden\n");
        }
        for h in &holds {
            body.extend(cat!(h, "\n"));
        }
        body.extend(cat!("was ", was, "\n"));
        let written = std::fs::write(ctx.at("pointer.new"), &body).is_ok()
            && std::fs::rename(ctx.at("pointer.new"), &rec).is_ok()
            && box_file(ctx, &ctx.at("run/omabox.pointer"), b"hidden");
        if !written {
            drop(lock);
            p.err = cat!("cannot write ", ctx.d, "/pointer");
            return Ok(Err(()));
        }
        if live != b"true" {
            let r = if wait {
                ptr_seen(ctx, b"true")
            } else {
                ptr_live_set(ctx, b"true")
            };
            if let Err(e) = r {
                p.err = e;
                ok = false;
            }
        }
    } else {
        let _ = std::fs::remove_file(&rec);
        let _ = std::fs::remove_file(ctx.at("run/omabox.pointer"));
        if on0 || op == Op::Show {
            // Shown only when it was hidden (a pointer already drawn leaves nothing to wait for).
            let Ok(live) = ptr_live_get(ctx) else {
                drop(lock);
                return Ok(Err(()));
            };
            let to: &[u8] = if was.is_empty() { b"false" } else { &was };
            if live != to {
                let r = if wait {
                    ptr_seen(ctx, to)
                } else {
                    ptr_live_set(ctx, to)
                };
                if let Err(e) = r {
                    p.err = e;
                    ok = false;
                }
            }
        }
    }
    drop(lock);
    Ok(if ok { Ok(()) } else { Err(()) })
}

/// ptr_fail WHAT: the box gone, hung, or its Hyprland's words.
pub fn ptr_fail(ctx: &Ctx, p: &Ptr, what: &[u8]) -> Exit {
    match box_alive(ctx) {
        Err(e) => return e,
        Ok(false) => return ctx.die(hypr_down_text(ctx)),
        Ok(true) => {}
    }
    match hypr_answers(ctx) {
        Err(e) => return e,
        Ok(false) => {
            return match hung_text(ctx, "") {
                Ok(t) => ctx.die(t),
                Err(e) => e,
            };
        }
        Ok(true) => {}
    }
    let e: &[u8] = if p.err.is_empty() { b"nothing" } else { &p.err };
    ctx.die(cat!(
        what,
        ": the box's Hyprland did not answer as expected: ",
        e
    ))
}

/// pointer_shown hide|show (#228): the pointer out of the box's frames until `show`, over reloads (the
/// box's config reads omabox.pointer on every load); `hide` returns once a frame without it is drawn.
pub fn pointer_shown(ctx: &Ctx, hide: bool) -> Res<()> {
    let mut p = Ptr::default();
    let (op, w): (Op, &[u8]) = if hide {
        (Op::Hide, b"hide")
    } else {
        (Op::Show, b"show")
    };
    if ptr_change(ctx, &mut p, op, b"", true)?.is_err() {
        return Err(ptr_fail(ctx, &p, &cat!("pointer ", w)));
    }
    let n = &ctx.name;
    if hide {
        say(cat!(
            "pointer: hidden in box '",
            n,
            "' (shots, recordings, peek) until omabox pointer show; it still moves, and focus follows it"
        ));
        if meta(&ctx.d, "mode").0 == b"interactive" {
            say(
                "pointer: the box is interactive: whoever drives its window sees no pointer over it either",
            );
        }
        let cfg = run_read(&ctx.d, "omabox-hyprland.lua").unwrap_or_default();
        if !crate::exit::contains(&cfg, b"omabox.pointer") {
            say(cat!(
                "pointer: box '",
                n,
                "' runs a config from before this: a config reload shows it again (omabox reload -b ",
                n,
                " takes this omabox's)"
            ));
        }
    } else if p.holds > 0 {
        say(cat!(
            "pointer: box '",
            n,
            "' shows it again once the shot or recording with --no-pointer ends"
        ));
    } else {
        say(cat!("pointer: shown in box '", n, "'"));
    }
    Ok(())
}

/// Held while this process changes the pointer's record (the hold, its release at the end): a
/// signal's release waits for it, as bash runs a trap only once the command in the foreground (the
/// box-side watch that hides the pointer) is over: never a release that reads the pointer shown
/// while that watch is still about to hide it.
static GATE: Mutex<()> = Mutex::new(());
/// F with GATE held, said so (sig::GATED: a foreground child's end does not park main there, where
/// the signal's thread waits for the gate).
fn gated<T>(f: impl FnOnce() -> T) -> T {
    let _g = GATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    crate::sig::GATED.store(true, std::sync::atomic::Ordering::SeqCst);
    let r = f();
    crate::sig::GATED.store(false, std::sync::atomic::Ordering::SeqCst);
    r
}

/// A terminating signal came (sig::STOPPING): main stays where it is until the trap's work ends
/// this process.
fn park_if_stopping() {
    // (at rest there: a trap's undo waits for main to rest, sig::main_rests)
    crate::sig::resting(|| {
        while crate::sig::STOPPING.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    });
}

/// The one-off hold of this process (shot --no-pointer), given back when it ends however it ends.
static HOLD: Mutex<Option<(Ctx, Vec<u8>)>> = Mutex::new(None);

/// Whether a one-off hold (hold_until_exit) is on: its own handlers then take INT, TERM and HUP, so
/// nothing may arm trap.rs's over them (#386).
pub fn holding() -> bool {
    HOLD.lock().map(|h| h.is_some()).unwrap_or(false)
}

/// ptr_release_quiet TAG: the hold given back; quiet when the box went down meanwhile.
fn release_quiet(ctx: &Ctx, tag: &[u8]) {
    if !ctx.at("pointer").is_file() {
        return;
    }
    let done = crate::exit::muted(|| {
        let mut p = Ptr::default();
        matches!(ptr_change(ctx, &mut p, Op::Release, tag, true), Ok(Ok(())))
    });
    if done {
        return;
    }
    let alive = crate::exit::muted(|| box_alive(ctx).unwrap_or(false));
    if alive {
        say(cat!(
            "could not show box '",
            ctx.name,
            "''s pointer again (omabox pointer show -b ",
            ctx.name,
            ")"
        ));
    }
}

/// `trap - EXIT INT TERM HUP` once record start's recording runs (finding 320): the hold stays,
/// `record stop` gives it back; no release at this omabox's end, nor on a signal (bash's way with
/// one from here, trap.rs).
pub fn hold_keep() {
    // (trap.rs's handlers back first: a signal from here is no release's)
    crate::trap::stop_on_signals();
    if let Ok(mut g) = HOLD.lock() {
        *g = None;
    }
}

/// At this process's end (main's, after the command): the hold given back, once (bash's EXIT trap).
pub fn release_at_exit() {
    gated(|| {
        let h = HOLD.lock().ok().and_then(|mut g| g.take());
        if let Some((ctx, tag)) = h {
            release_quiet(&ctx, &tag);
        }
    });
    // (a signal during it: bash's trap runs once the release's subshell is over, then exits with
    // the signal's code, `exit 143` in its EXIT trap: probed)
    park_if_stopping();
}

/// `omabox _ptr-release BOXES NAME DIR TAG CODE`: what a signal leaves for this binary to do in a
/// process of its own once it has replaced this one (hold_until_exit): the hold given back, then
/// the exit bash's trap gives (`trap 'exit 143' TERM`, its EXIT trap after).
pub fn release_main(args: &[Vec<u8>]) -> u8 {
    let [boxes, name, d, tag, code] = args else {
        return 2;
    };
    let mut ctx = Ctx::new(b"shot", boxes.clone());
    ctx.parsed();
    ctx.name = name.clone();
    ctx.d = d.clone();
    release_quiet(&ctx, tag);
    std::str::from_utf8(code)
        .ok()
        .and_then(|c| c.parse().ok())
        .unwrap_or(1)
}

extern "C" fn on_signal(sig: libc::c_int) {
    let _e = crate::sig::Errno::save();
    let fd = SIG_W.load(std::sync::atomic::Ordering::Relaxed);
    if fd >= 0 {
        let b = [u8::try_from(sig).unwrap_or(0)];
        // SAFETY: write is async-signal-safe; b outlives the call.
        unsafe {
            libc::write(fd, b.as_ptr().cast(), 1);
        }
    }
}

static SIG_W: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

/// The argv of `_ptr-release` for each of INT, TERM and HUP, made before any signal comes.
struct Argvs {
    _words: Vec<Vec<std::ffi::CString>>,
    ptrs: Vec<Vec<usize>>,
}

/// ptr_hold_until_exit TAG: a one-off hold (shot --no-pointer) given back when this omabox ends
/// however it ends: at its end (release_at_exit), or on INT, TERM or HUP, then exit 130, 143 or 129
/// as bash's traps do. On a signal this process is replaced (execv) by `_ptr-release`, so the
/// command stops where it is, as bash's trap stops it: nothing more is said or written, its own
/// lock on the pointer's record goes with it (bash's ptr_unlock first), and the release runs
/// single-threaded.
pub fn hold_until_exit(ctx: &Ctx, tag: &[u8]) -> Res<()> {
    if let Ok(mut g) = HOLD.lock() {
        *g = Some((ctx.clone(), tag.to_vec()));
    }
    // (argv[0] its path for ps; the exec by /proc/self/exe, which is this binary even once a
    // rebuild has replaced or removed the file)
    let exe = std::fs::read_link("/proc/self/exe")
        .map(|p| shell::bytes(&p).to_vec())
        .unwrap_or_default();
    let mut words = Vec::new();
    let mut ptrs = Vec::new();
    for code in ["130", "143", "129"] {
        let w: Vec<std::ffi::CString> = [
            exe.as_slice(),
            b"_ptr-release",
            &ctx.boxes,
            &ctx.name,
            &ctx.d,
            tag,
            code.as_bytes(),
        ]
        .iter()
        .filter_map(|a| std::ffi::CString::new(a.to_vec()).ok())
        .collect();
        let mut p: Vec<usize> = w.iter().map(|c| c.as_ptr() as usize).collect();
        p.push(0);
        words.push(w);
        ptrs.push(p);
    }
    let argvs = Argvs {
        _words: words,
        ptrs,
    };
    if let Ok((r, w)) = crate::exec::pipe() {
        use std::os::fd::IntoRawFd;
        SIG_W.store(w.into_raw_fd(), std::sync::atomic::Ordering::Relaxed);
        // (a signal ignored when this omabox started stays ignored: bash cannot trap it, `nohup`)
        for s in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            crate::sig::catch(s, on_signal);
        }
        std::thread::spawn(move || {
            // (the whole of it into the thread: a closure that named only .ptrs would take only
            // them, and the strings they point into would be gone)
            let argvs = argvs;
            let mut f = std::fs::File::from(r);
            let mut b = [0u8; 1];
            if std::io::Read::read_exact(&mut f, &mut b).is_ok() {
                let k = match libc::c_int::from(b[0]) {
                    libc::SIGINT => 0,
                    libc::SIGTERM => 1,
                    _ => 2,
                };
                // Not while the hold is being set (or released): once that is over.
                crate::sig::STOPPING.store(true, std::sync::atomic::Ordering::SeqCst);
                let _g = GATE
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // Nor while a child runs in the foreground: waited for as a trap's stop waits, bounded
                // (finding 453).
                let _f = crate::trap::foreground_over();
                // The hold given back already (release_at_exit, at the command's end): nothing left
                // to release; the signal's exit, as bash's trap gives it there.
                if HOLD.lock().map(|h| h.is_none()).unwrap_or(true) {
                    std::process::exit([130, 143, 129][k]);
                }
                let a = &argvs.ptrs[k];
                if a.len() == 8 {
                    // SAFETY: a is a NUL-ended array of pointers to the NUL-ended strings argvs
                    // owns, alive for the call; execv only returns on failure.
                    unsafe {
                        libc::execv(c"/proc/self/exe".as_ptr(), a.as_ptr().cast());
                    }
                }
                // (no way to replace it: the release from here, its words muted on this thread
                // alone, then the exit)
                let h = HOLD.lock().ok().and_then(|mut g| g.take());
                if let Some((ctx, tag)) = h {
                    release_quiet(&ctx, &tag);
                }
                std::process::exit([130, 143, 129][k]);
            }
        });
    }
    let mut p = Ptr::default();
    let held = gated(|| ptr_change(ctx, &mut p, Op::Hold, tag, true))?;
    // A signal that came meanwhile: bash's trap runs now, before anything more of the command; here
    // the release replaces this process as soon as the gate is free, and nothing more is done.
    park_if_stopping();
    if held.is_err() {
        return Err(ptr_fail(ctx, &p, b"--no-pointer"));
    }
    Ok(())
}
