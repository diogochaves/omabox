//! `omabox gpu [-b NAME] [DURATION] [--json]`: GPU engine time of this box's processes, as % of wall
//! time, by process name. Host-wide tools sum DRM fdinfo by name, so a box's Hyprland and quickshell
//! merge with the real ones; here a process belongs to the box when it is in its pid namespace. A
//! DRM client shared by several fds counts once (drm-pdev + drm-client-id). NVIDIA reports none:
//! there, its driver's own sampling through nvidia-smi (finding 295). `gpu release GPU` (#118):
//! the headless boxes rendering on a GPU taken down.

use crate::args::{optarg, unknown_opt};
use crate::boxes::{box_alive, list_names, need_box};
use crate::boxmeta::BoxMeta;
use crate::cat;
use crate::exit::{Ctx, Res, out, say};
use crate::gpu::{
    comm_word, conf_gpu, cost_context, cost_lines, gpu_is, gpu_kind, render_driver, render_slot,
};
use crate::jail::in_jail;
use crate::jobs::{box_pids, box_pids_try};
use crate::jq::{self, Args};
use crate::num::s_duration;
use crate::select::select_box;
use crate::shell;
use std::os::fd::AsRawFd;
use std::time::Duration;

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    if args.first().is_some_and(|a| a == b"release") {
        return gpu_release(ctx, &args[1..]);
    }
    let (mut name, mut secs, mut json) = (Vec::new(), b"5".to_vec(), false);
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
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "gpu", a)),
            _ => {
                let Some(s) = s_duration(a, false) else {
                    return Err(ctx.die(cat!(
                        "gpu: how long to measure, like 5s or 500ms (a bare number is seconds), got ",
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
    if box_pids_try(ctx).is_none() {
        return Err(ctx.die("gpu: cannot read the box's pid namespace"));
    }
    // (the EXIT trap's rm, also on INT, TERM or HUP; armed with the making: #360)
    let (tmp, _trap) = crate::trap::made(|| -> Res<_> {
        let (_, tmp) = shell::mktemp_args(&[b"-d"])?;
        let t = tmp.clone();
        let trap = crate::trap::on_exit(move || {
            let _ = std::fs::remove_dir_all(shell::path(&t));
        });
        Ok((tmp, trap))
    })?;
    let at = |f: &str| cat!(tmp, "/", f);
    let mut via: &str = "drm";
    let figures = if gpu_on_nvidia(ctx) {
        if !on_path(b"nvidia-smi") {
            return Err(ctx.die(cat!(
                "gpu: box '",
                ctx.name,
                "' draws on NVIDIA, whose driver publishes no per-process DRM counters; its own figures come through nvidia-smi (nvidia-utils), which is not installed"
            )));
        }
        // Whole seconds: the driver samples once a second. Its first sample is the second before.
        via = "nvidia";
        // (`secs=$(awk ...)`, `pci=$(jq ...)`: substitutions' steps)
        secs = Args::new(&["-v"])
            .arg(cat!("s=", secs))
            .arg(crate::jqprog::NV_SECS_AWK)
            .awk(None, jq::Err::Inherit)
            .text();
        let pci = Args::new(&["-r", ".render.pci // empty"])
            .arg(cat!(ctx.d, "/box.json"))
            .run_file(jq::Err::Null);
        let pci = if pci.ok() { pci.text() } else { Vec::new() };
        let n = shell::dec(&secs).unwrap_or(0).saturating_add(1).to_string();
        let mut c = crate::sig::command("nvidia-smi");
        c.args(["pmon", "-s", "u", "-d", "1", "-c", &n]);
        if !pci.is_empty() {
            c.arg("-i").arg(std::ffi::OsStr::from_bytes(&pci));
        }
        let ok = std::fs::File::create(shell::path(&at("pmon")))
            .and_then(|f| {
                let f2 = f.try_clone()?;
                c.stdout(f).stderr(f2);
                crate::sig::run_status(c)
            })
            .is_ok_and(|rc| rc == 0);
        if !ok {
            let p = std::fs::read(shell::path(&at("pmon"))).unwrap_or_default();
            let head: Vec<u8> = p
                .split_inclusive(|&c| c == b'\n')
                .take(3)
                .flatten()
                .map(|&c| if c == b'\n' { b' ' } else { c })
                .collect();
            return Err(ctx.die(cat!("gpu: nvidia-smi pmon failed: ", head)));
        }
        let mut pids = Vec::new();
        for p in box_pids(ctx) {
            pids.extend_from_slice(&cat!(p, " ", comm_word(&p), "\n"));
        }
        let _ = std::fs::write(shell::path(&at("pids")), &pids);
        let pmon = std::fs::read(shell::path(&at("pmon"))).unwrap_or_default();
        pmon_figures(&at("pids"), &pmon)
    } else {
        let t0 = crate::settle::mono_ns();
        let a = snap(ctx);
        let _ = std::fs::write(shell::path(&at("a")), &a);
        crate::trap::sleep(&secs);
        let t1 = crate::settle::mono_ns();
        let b = snap(ctx);
        let _ = std::fs::write(shell::path(&at("b")), &b);
        // A client first seen in the second sample started during the window: all its time
        // counts. Every process holding a client is listed, idle ones at 0.0.
        jq::tool(
            "awk",
            &[
                b"-v".to_vec(),
                format!("wall={}", t1 - t0).into_bytes(),
                crate::jqprog::GPU_DIFF_AWK.as_bytes().to_vec(),
                at("a"),
                at("b"),
            ],
            None,
            jq::Err::Inherit,
        )
        .out
    };
    let _ = std::fs::write(shell::path(&at("r")), &figures);
    let c = cost_context(ctx)?;
    if json {
        let r = Args::new(&["-Rn"])
            .kv("--arg", "box", &ctx.name)
            .kv("--arg", "screen", &c.screen)
            .kv("--argjson", "secs", &secs)
            .kv("--argjson", "aq", &c.aq)
            .kv("--argjson", "render", &c.rj)
            .kv("--argjson", "cr", &c.cr)
            .kv("--arg", "via", via)
            .arg(crate::jqprog::GPU_JSON_JQ);
        let r = jq::tool("jq", &r_words(r), Some(&figures), jq::Err::Inherit);
        out(&r.out)?;
        return crate::cmd::hyprctl::code(r.rc);
    }
    let (sn, sm) = crate::cmd::mode::screen_parts(&c.screen);
    if via == "nvidia" {
        out(cat!(
            "box '",
            ctx.name,
            "': ",
            sm,
            " on ",
            sn,
            ", ",
            secs,
            "s, % of the GPU by process as NVIDIA's driver samples it each second (nvidia-smi pmon: SM, encoders, decoders)\n"
        ))?;
    } else {
        out(cat!(
            "box '",
            ctx.name,
            "': ",
            sm,
            " on ",
            sn,
            ", ",
            secs,
            "s, % of wall time on the GPU (all engines)\n"
        ))?;
    }
    cost_lines(ctx, &c)?;
    gpu_rows(&figures)
}

/// The words of a jq call built with Args (for one fed by a pipe, not a here-string).
fn r_words(a: Args) -> Vec<Vec<u8>> {
    a.into_words()
}

/// `command -v NAME`: on PATH, executable.
fn on_path(n: &[u8]) -> bool {
    shell::env_or_empty("PATH")
        .split(|&c| c == b':')
        .map(|d| cat!(if d.is_empty() { b".".as_slice() } else { d }, "/", n))
        .any(|p| crate::exec::executable(&p) && shell::path(&p).is_file())
}

use std::os::unix::ffi::OsStrExt;

/// snap: one line per DRM client and engine of the box's processes (pdev/client-id, engine, ns,
/// pid, name), each process's fdinfo read by awk as bash reads it.
fn snap(ctx: &Ctx) -> Vec<u8> {
    let mut o = Vec::new();
    for p in box_pids(ctx) {
        let dir = shell::path(&cat!("/proc/", p, "/fdinfo"));
        let files: Vec<Vec<u8>> = shell::glob(&dir, false)
            .into_iter()
            .map(|f| cat!("/proc/", p, "/fdinfo/", f))
            .collect();
        // (none, the process gone or holding no file: nothing to read. Bash handed awk its glob
        // unmatched, which awk failed on, unsaid)
        if files.is_empty() {
            continue;
        }
        let mut a: Vec<Vec<u8>> = vec![
            b"-v".to_vec(),
            cat!("pid=", p),
            b"-v".to_vec(),
            cat!("comm=", comm_word(&p)),
            crate::jqprog::GPU_SNAP_AWK.as_bytes().to_vec(),
        ];
        a.extend(files);
        o.extend_from_slice(&jq::tool("awk", &a, None, jq::Err::Null).out);
    }
    o
}

/// gpu_on_nvidia: this box draws on NVIDIA, whose driver publishes no DRM fdinfo counters (finding
/// 77): a headless box's render node, or an interactive box with a process holding NVIDIA's
/// control node.
fn gpu_on_nvidia(ctx: &Ctx) -> bool {
    // (`[ "$(jq ...)" != nvidia ]`)
    let d = Args::new(&["-r", ".render.driver // empty"])
        .arg(cat!(ctx.d, "/box.json"))
        .run_file(jq::Err::Null)
        .text();
    if d == b"nvidia" {
        return true;
    }
    if !BoxMeta::load(&ctx.d).interactive() {
        return false;
    }
    for p in box_pids(ctx) {
        for f in fd_links(&p) {
            if f == b"/dev/nvidiactl" {
                return true;
            }
        }
    }
    false
}

/// What `/proc/PID/fd/*` point to, in glob order (unreadable ones left out).
fn fd_links(p: &[u8]) -> Vec<Vec<u8>> {
    let dir = cat!("/proc/", p, "/fd");
    shell::glob(&shell::path(&dir), false)
        .into_iter()
        .filter_map(|f| std::fs::read_link(shell::path(&cat!(dir, "/", f))).ok())
        .map(|t| shell::bytes(&t).to_vec())
        .collect()
}

/// pmon_figures PIDS: NVIDIA's per-process samples (nvidia-smi pmon's output) as gpu's figures, the
/// box's processes only (PIDS: "PID COMM" lines; finding 295).
pub fn pmon_figures(pids: &[u8], pmon: &[u8]) -> Vec<u8> {
    jq::tool(
        "awk",
        &[
            b"-v".to_vec(),
            cat!("pidcomm=", pids),
            crate::jqprog::PMON_AWK.as_bytes().to_vec(),
        ],
        Some(pmon),
        jq::Err::Inherit,
    )
    .out
}

/// gpu_rows: gpu's rows from its figures ("proc NAME %" lines), most first; none is "(no GPU
/// time)", not a failure (finding 298). A process whose comm is `engine` is a process (finding 176).
pub fn gpu_rows(r: &[u8]) -> Res<()> {
    let procs: Vec<u8> = r
        .split_inclusive(|&c| c == b'\n')
        .filter(|l| l.starts_with(b"proc "))
        .flatten()
        .copied()
        .collect();
    if procs.is_empty() {
        return out("  (no GPU time)\n");
    }
    let procs = if procs.ends_with(b"\n") {
        procs
    } else {
        cat!(procs, "\n")
    };
    let sorted = jq::tool("sort", &["-k3", "-rn"], Some(&procs), jq::Err::Inherit);
    let rows = jq::tool(
        "awk",
        &[crate::jqprog::GPU_ROWS_AWK],
        Some(&sorted.out),
        jq::Err::Inherit,
    );
    out(&rows.out)?;
    crate::cmd::hyprctl::code(rows.rc)
}

/// gpu release GPU (#118): take down the headless boxes rendering on that GPU (a slot, or nvidia,
/// amd, intel), naming each, so it can be handed elsewhere (a VM). Never an interactive box or one
/// on another GPU; the user's to run, refused to a jailed agent. A box from before box.json named its
/// GPU counts when a process of it has that GPU's node open. The take-down is `down`'s (bash's until
/// wave 5 ports it), run as bin/omabox runs it.
fn gpu_release(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    if args.len() != 1 {
        return Err(ctx.die(
            "gpu release: GPU (a PCI slot, or nvidia, amd, intel; omabox config gpu lists them)",
        ));
    }
    if in_jail() {
        ctx.parsed();
        return Err(ctx.die(
            "gpu release takes down every box on a GPU: the user's to run, not a jailed agent's",
        ));
    }
    let want = match conf_gpu(&args[0]) {
        Some(w) if w != b"auto" => w,
        _ => {
            return Err(ctx.die(cat!(
                "gpu release: not a GPU: ",
                args[0],
                " (a PCI slot, or nvidia, amd, intel)"
            )));
        }
    };
    ctx.parsed();
    gpu_release_wait(ctx)?;
    let mut names: Vec<Vec<u8>> = Vec::new();
    for n in list_names(ctx, false) {
        select_box(ctx, &n)?;
        if !BoxMeta::load(&ctx.d).headless() || !box_alive(ctx)? {
            continue;
        }
        // (`node=$(jq ...)`, `on=$(jq ...)`)
        let node = Args::new(&["-r", ".render.node // empty"])
            .arg(cat!(ctx.d, "/box.json"))
            .run_file(jq::Err::Null);
        let node = if node.ok() { node.text() } else { Vec::new() };
        let on = if !node.is_empty() {
            let r = Args::new(&["-r", r#".render | "\(.driver) \(.pci)""#])
                .arg(cat!(ctx.d, "/box.json"))
                .run_file(jq::Err::Inherit);
            crate::cmd::hyprctl::code(r.rc)?;
            let on = r.text();
            let (drv, pci) = crate::cmd::mode::screen_parts(&on);
            let pci = if on.contains(&b' ') { pci } else { &on[..] };
            let hit = match want.as_slice() {
                b"nvidia" | b"amd" | b"intel" => gpu_kind(drv) == want.as_slice(),
                _ => pci == want.as_slice(),
            };
            if !hit {
                continue;
            }
            on
        } else {
            match box_render_held(ctx, &want) {
                Some(on) => on,
                None => continue,
            }
        };
        say(cat!(
            "gpu release: taking box '",
            n,
            "' down (it renders on ",
            on,
            ")"
        ));
        names.push(n);
    }
    if names.is_empty() {
        say(cat!("gpu release: no headless box renders on ", want));
        return Ok(());
    }
    let mut c = crate::sig::omabox_cmd();
    c.arg("down")
        .args(names.iter().map(|n| std::ffi::OsStr::from_bytes(n)));
    let rc = crate::sig::run_status(c).unwrap_or(126);
    crate::cmd::hyprctl::code(rc)
}

/// gpu_release_wait: an `up` in progress has no box.json yet, or no box running, and would be
/// skipped, its box then holding the GPU (finding 237). Each name's lock busy (an up, or a down)
/// is waited for, 60 s at most, until none is; the lock file opened to read, never made.
fn gpu_release_wait(ctx: &Ctx) -> Res<()> {
    let mut busy = true;
    let mut pass = 0;
    while busy && pass < 5 {
        busy = false;
        let all = std::fs::read_dir(shell::path(&ctx.boxes))
            .map(|rd| {
                let mut v: Vec<Vec<u8>> = rd
                    .flatten()
                    .map(|e| e.file_name().as_bytes().to_vec())
                    .filter(|f| f.starts_with(b".lock-"))
                    .collect();
                v.sort();
                v
            })
            .unwrap_or_default();
        for f in all {
            let n = f[6..].to_vec();
            let p = ctx.in_boxes(&f);
            if !p.is_file() || n.starts_with(b"save-") {
                continue;
            }
            let Ok(file) = std::fs::File::open(&p) else {
                continue;
            };
            let fd = file.as_raw_fd();
            // SAFETY: flock on a descriptor we own, open for the call.
            if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                busy = true;
                if n == b"new" {
                    say("gpu release: waiting for an up --new to choose its name");
                } else {
                    say(cat!(
                        "gpu release: waiting for box '",
                        n,
                        "' (an up or down in progress)"
                    ));
                }
                let end = std::time::Instant::now() + Duration::from_secs(60);
                // SAFETY: as above.
                while unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                    if std::time::Instant::now() >= end {
                        return Err(ctx.die(cat!(
                            "gpu release: box '",
                            n,
                            "' still busy after 60 s; nothing taken down"
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            drop(file);
        }
        pass += 1;
    }
    Ok(())
}

/// box_render_held WANT: for a box with no .render, the driver and slot of the render node of GPU
/// WANT one of its processes has open.
fn box_render_held(ctx: &Ctx, want: &[u8]) -> Option<Vec<u8>> {
    for p in box_pids(ctx) {
        for t in fd_links(&p) {
            if !shell::re_match("^/dev/dri/renderD[0-9]+$", &t) || !gpu_is(&t, want) {
                continue;
            }
            return Some(cat!(render_driver(&t), " ", render_slot(&t)));
        }
    }
    None
}
