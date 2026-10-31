//! `omabox _rust probe NAME [ARG...]`: the suite's read-only view of the helpers it used to source from
//! bin/omabox (#324 wave 6-suite, D5): its startup (the host's session, the render node) and what its
//! box tests read (a box's processes, a recording's overlay, which aquamarine a system dir gives).
//!
//! Internal: an `_` word, so the front never parses it and a broker refuses it (broker_check). Only
//! the names in NAMES, each of which reads and prints, never changes anything: no box, setting or file
//! is made or removed, no host command is run but the read-only ones the helper itself runs (hyprctl
//! instances, ldd, sort and awk). Not `host_hyprctl`: it passes its words to hyprctl on the real
//! desktop (a dispatch); the suite runs hyprctl itself with the signature host_session prints.

use crate::cat;
use crate::exit::{Ctx, Exit, Res, out};
use crate::install::{AQ_BUILD, AQ_FIX_SYMBOL, AQ_FIXED_AFTER};

/// The helpers the probe answers for, and the words each takes.
pub const NAMES: &[(&str, &str)] = &[
    // ROOT, as the binary found it from where it is (t_unit_layout, #324 wave 6-flip).
    ("root", ""),
    ("host_session", ""),
    // A screen mode as up reads it: `host` the user's focused monitor (a read-only hyprctl there).
    ("parse_mode", "MODE"),
    // The GPUs, their devices under ROOT ("/" by default; a stand-in tree's for the unit tests).
    ("render_node", "[ROOT]"),
    ("render_nodes", "[ROOT]"),
    ("render_driver", "NODE [ROOT]"),
    ("render_fallback", "NODE [ROOT]"),
    ("gpu_why_not", "WANT [ROOT]"),
    ("gpu_usable", "WANT [ROOT]"),
    ("gpu_list", "[ROOT]"),
    ("gpu_json", "[ROOT]"),
    ("nvidia_card", "NODE [ROOT]"),
    ("egl_render", ""),
    ("soft_note", "RECORD DRIVER [OTHER]"),
    ("gpu_rows", "FILE"),
    ("pmon_figures", "PIDS"),
    // Which aquamarine boxes run, and what follows from it; the system's under ROOT.
    ("hypr_aq_soname", ""),
    ("aq_pick", "SONAME SYSDIR"),
    ("aq_pick_dirs", "SONAME SYSDIR [DIR...]"),
    ("aq_main_lib", "CHECKOUT"),
    ("aq_resolve", "SONAME CHECKOUT ROOT"),
    ("confirm_close_available", "CHECKOUT ROOT"),
    ("nvidia_needs_fix", "NODE CHECKOUT ROOT"),
    ("aq_build_needed", "FORCE CHECKOUT ROOT"),
    ("aq_note", "KIND LIB VER FIXED"),
    ("check_install", ""),
    ("clean_locale", ""),
    ("qt_box", ""),
    ("qt_version", ""),
    ("aq_has_fix", "FILE"),
    ("aq_ver_fixed", "VERSION"),
    ("aq_const", "AQ_FIX_SYMBOL|AQ_FIXED_AFTER|AQ_BUILD"),
    ("desk_facts", "BEFORE AFTER"),
    ("box_pid", "NAME"),
    ("box_pids", "NAME"),
    // Where the input went: keys with the layers holding the keyboard given (kb_layers' answer).
    (
        "desk_target",
        "keys|click|scroll|drag|run SNAPSHOT [KB_LAYERS | X_Y | PID]",
    ),
    (
        "record_overlay",
        "INPUT MARKS FIRST X0 Y0 K W H DUR SHOW CAPS [AREA]",
    ),
    // A jailed caller's paths (OMABOX_JAIL the policy, as the broker sets it).
    ("jail_path", "PATH"),
    ("jail_read", "FILE"),
    ("jail_root", "PATH"),
    ("jail_root_covered", "PATH"),
    ("jail_deep", "PATH"),
    ("jail_readable", "FILE"),
    ("repo_top", ""),
    ("seed_spec", "SRC:DEST"),
    ("seed_blocked", "SRC REL PLACE..."),
    ("jail_cover", "D SRC DEST"),
    ("box_held", "D PID"),
    ("jail_verify", "D BOXPID SRC..."),
    ("covers_held", "NAME"),
    ("jail_sees", "PATH"),
    ("jail_masked", "PATH"),
    ("default_name", ""),
    ("caller_env", "NAME"),
    // The CLI's tables, for the suite's checks of help against them.
    ("cmd_opts_keys", ""),
    ("cmd_opts", "CMD"),
    ("up_box_opts", ""),
    ("usage_text", ""),
    ("shot_fit_default", ""),
    ("repo_git_dirs", "[TOP]"),
    // Durations whose refusals are said (their words checked).
    ("idle_duration", "VALUE HOW"),
    ("pause_ms", "WHAT VALUE"),
    ("ready_timeout", ""),
    ("mods_add", "SO_FAR MODS"),
    ("events_since", "VALUE [D NAME]"),
    // cpu's samples read from a stand-in /proc, and its awk on samples (stdin).
    ("cpu_sample", "PROC PID..."),
    ("cpu_threads", "PROC PID..."),
    ("cpu_tame", "NAME"),
    ("cpu_report", "CLK [PID [APP]]"),
    ("cpu_table", ""),
    // Where two images differ (magick reads them).
    ("diff_box", "A B [MASK...]"),
    ("diff_parts", "A B [MASK...]"),
    // What a box may mount, and where (the reason on stdout, 0, when refused; 1 when it may).
    ("refuse_src", "PATH [look]"),
    ("refuse_dest", "PATH"),
    ("robind_spec", "DIR[:DEST]"),
    ("monitor_spec", "SPEC"),
    // up's seeding and the desk, as read (nothing copied).
    (
        "own_path_visible",
        "PATH [--robind SRC<TAB>DEST | --overlay DIR]...",
    ),
    ("theme_snapshot_note", "THEME_DIR TOP"),
    ("theme_named", "NAME TREE [THEME_DIR...]"),
    ("shell_warnings", "FILE"),
    ("desk_name_refused", "NAME"),
    ("desk_content_refused", "FILE"),
    ("desk_files", "DESK"),
    ("desk_plugins", "DESK [ID<TAB>DIR...]"),
    ("desk_dir", ""),
    ("desk_home", ""),
    ("no_seed_spec", "LIST [HAVE]"),
    ("bar_seed_note", "DESK OMARCHY KEEP NOTIFY PAGERS [SPLIT]"),
    ("shell_json_filter", "KEEP NOTIFY PAGERS [SPLIT]"),
    ("place_plugin", "ID SECTION KINDS MOUNTED"),
    ("widget_section", "SECTION SPLIT"),
    ("workspace_widgets", ""),
    ("cwd_path_word", "DIR HOME WORD..."),
    // A repo's links that lead out of it (#235): which, the note up says, whether one may be mounted.
    ("job_exe", "ARGV0"),
    // What windows, layers, a crash log and the monitors give, as the commands read them.
    ("layer_select", "LAYERS SEL"),
    ("gdb_crashes", ""),
    ("desk_picture", ""),
    ("zero_note", "MONITORS WAYLAND_SCREEN OTHER"),
    ("box_cmds", ""),
    ("repo_link_out", "TOP PATH"),
    ("link_note", "WHAT LINK TARGET HOW"),
    ("link_mountable", "PATH"),
    ("cwd_note", "NAME HOW WORD DIR NOREPO"),
    // Processes: an agent's, a run's owner, by pid and start time.
    ("agent_proc", ""),
    ("proc_start", "PID"),
    ("proc_stat", "PID FIELD"),
    ("agent_alive", "D"),
    ("owner_alive", "PID START"),
    // Who started a box (from the probe's parent up), and whether box.json makes D an orphan.
    ("box_starter", ""),
    ("up_session_note", "NAME"),
    ("box_orphan", "D"),
    // A box's windows as the commands read them (the lists given: stdin, or words).
    ("windows_table", ""),
    ("windows_layers", ""),
    ("win_annotate", "CLIENTS MONITORS ACTIVE [LAYERS]"),
    ("win_select", "WINDOWS SEL..."),
    ("win_answer", "WANT WINDOWS SEL..."),
    ("win_blocked", "WINDOW X Y"),
    ("win_layer", "WINDOW X Y"),
];

/// One line a word: the probe's own usage, for a name it does not have.
fn usage() -> Exit {
    let mut m = b"omabox _rust probe: one of".to_vec();
    for (n, a) in NAMES {
        m.extend_from_slice(b"\n  ");
        m.extend_from_slice(n.as_bytes());
        if !a.is_empty() {
            m.push(b' ');
            m.extend_from_slice(a.as_bytes());
        }
    }
    m.push(b'\n');
    let _ = std::io::Write::write_all(&mut std::io::stderr(), &m);
    Exit(2)
}

fn line(v: &[u8]) -> Res<()> {
    out(cat!(v, "\n"))
}

/// `_rust probe NAME ARG...`: NAME's answer on stdout, its refusal on stderr, as the helper gives
/// them; 2 for a name not on the list or the wrong number of words.
pub fn run(boxes: Vec<u8>, words: &[Vec<u8>]) -> Res<()> {
    let mut ctx = Ctx::new(b"", boxes);
    ctx.parsed();
    let Some((name, args)) = words.split_first() else {
        return Err(usage());
    };
    let a: Vec<&[u8]> = args.iter().map(Vec::as_slice).collect();
    match (name.as_slice(), a.as_slice()) {
        (b"root", []) => line(&crate::paths::root()),
        (b"host_session", []) => {
            let (sig, wl) = crate::host::host_session_die(&ctx)?;
            line(&cat!(sig, " ", wl))
        }
        (b"parse_mode", [m]) => line(&crate::mon::parse_mode(&ctx, m)?),
        (b"render_node", r) if r.len() <= 1 => line(
            crate::render::render_node_in(
                &ctx,
                crate::shell::env_or_empty("OMABOX_RENDER_NODE"),
                root(r),
            )?
            .as_bytes(),
        ),
        (b"render_nodes", r) if r.len() <= 1 => {
            for n in crate::render::render_nodes_in(root(r)) {
                line(n.as_bytes())?;
            }
            Ok(())
        }
        (b"render_driver", [n, r @ ..]) if r.len() <= 1 => {
            line(&crate::gpu::render_driver_in(n, root(r)))
        }
        (b"render_fallback", [n, r @ ..]) if r.len() <= 1 => {
            let n = crate::render::RenderNode::new(n).ok_or(Exit(2))?;
            yes(crate::render::render_fallback_in(&n, root(r)))
        }
        (b"gpu_why_not", [w, r @ ..]) if r.len() <= 1 => {
            line(&crate::render::gpu_why_not_in(w, root(r)))
        }
        (b"gpu_usable", [w, r @ ..]) if r.len() <= 1 => {
            yes(crate::render::gpu_usable_in(w, root(r)))
        }
        (b"gpu_list", r) if r.len() <= 1 => out(crate::render::gpu_list_in(&ctx, root(r))),
        (b"gpu_json", r) if r.len() <= 1 => {
            let (auto, now) = crate::render::gpu_auto_now_in(&ctx, root(r));
            let s = |v: Vec<u8>| {
                if v.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::String(String::from_utf8_lossy(&v).into_owned())
                }
            };
            let j = serde_json::json!({
                "gpus": crate::render::gpu_json_in(root(r)),
                "gpu_auto": s(auto),
                "gpu_now": s(now),
            });
            line(j.to_string().as_bytes())
        }
        (b"nvidia_card", [n, r @ ..]) if r.len() <= 1 => {
            let sys = crate::gpu::at(root(r), crate::render::SYSDRM.as_bytes());
            line(&crate::render::nvidia_card_in(n, &sys).unwrap_or_default())
        }
        (b"egl_render", []) => line(&crate::ready::egl_render(&crate::shell::subst(
            &stdin_all()?
        ))),
        (b"soft_note", [rec, drv, other @ ..]) if other.len() <= 1 => {
            line(&crate::ready::soft_note(rec, drv, other.first().copied()))
        }
        (b"gpu_rows", [f]) => {
            let r = std::fs::read(crate::shell::path(f)).unwrap_or_default();
            crate::cmd::gpu::gpu_rows(&r)
        }
        (b"pmon_figures", [pids]) => out(crate::cmd::gpu::pmon_figures(pids, &stdin_all()?)),
        (b"hypr_aq_soname", []) => line(&crate::install::hypr_aq_soname(&ctx)?),
        // As OMABOX_AQUAMARINE=system with SYSDIR the system's lib dir: KIND|VER|FIXED|DESC.
        (b"aq_pick", [so, sys]) => {
            let aq = crate::install::aq_pick_system(&ctx, so, sys).ok_or(Exit(1))?;
            line(&cat!(
                aq.kind,
                "|",
                aq.ver,
                "|",
                if aq.fixed { "1" } else { "0" },
                "|",
                aq.desc()
            ))
        }
        (b"aq_pick_dirs", [so, sys, dirs @ ..]) => {
            let aq = crate::install::aq_pick_dirs(&ctx, so, sys, &owned(dirs)).ok_or(Exit(1))?;
            line(&cat!(
                aq.kind,
                "|",
                aq.lib,
                "|",
                aq.ver,
                "|",
                if aq.fixed { "1" } else { "0" }
            ))
        }
        (b"aq_main_lib", [co]) => line(&crate::install::aq_main_lib(co).unwrap_or_default()),
        (b"aq_resolve", [so, co, r]) => {
            let env = crate::shell::env_or_empty("OMABOX_AQUAMARINE");
            let aq = crate::install::aq_resolve_in(&ctx, so, &env, co, r).ok_or(Exit(1))?;
            line(&cat!(aq.kind, "|", aq.lib, "|", aq.main, "|", aq.desc()))
        }
        (b"confirm_close_available", [co, r]) => {
            line(if crate::install::confirm_close_available_in(&ctx, co, r) {
                b"true"
            } else {
                b"false"
            })
        }
        (b"nvidia_needs_fix", [n, co, r]) => {
            let n = crate::render::RenderNode::new(n).ok_or(Exit(2))?;
            let w = crate::install::hypr_aq_soname(&ctx)?;
            let env = crate::shell::env_or_empty("OMABOX_AQUAMARINE");
            let aq = crate::install::aq_resolve_in(&ctx, &w, &env, co, r).ok_or(Exit(1))?;
            crate::install::nvidia_needs_fix_in(&ctx, &aq, &n, r)
        }
        (b"aq_build_needed", [force, co, r]) => {
            let w = crate::install::hypr_aq_soname(&ctx)?;
            let aq = crate::install::aq_resolve_in(&ctx, &w, b"", co, r).ok_or(Exit(1))?;
            yes(crate::install::aq_build_needed(&aq, *force == b"1"))
        }
        (b"aq_note", [kind, lib, ver, fixed]) => crate::cmd::setup::aq_note(&crate::install::Aq {
            kind: kind.to_vec(),
            lib: lib.to_vec(),
            ver: ver.to_vec(),
            fixed: *fixed == b"1",
            main: Vec::new(),
        }),
        // (the render node is checked after the helpers: a placeholder one, never opened here)
        (b"check_install", []) => {
            let n = crate::render::RenderNode::new(b"/dev/dri/renderD128").ok_or(Exit(2))?;
            let aq = crate::install::check_install(&ctx, &n, None)?;
            line(&aq.desc())
        }
        (b"clean_locale", []) => line(crate::upspec::clean_locale().as_bytes()),
        (b"qt_box", []) => line(&crate::install::qt_box()),
        (b"qt_version", []) => line(&crate::install::qt_version()),
        (b"aq_has_fix", [f]) => crate::install::aq_has_fix(f).then_some(()).ok_or(Exit(1)),
        (b"aq_ver_fixed", [v]) => crate::install::aq_ver_fixed(v).then_some(()).ok_or(Exit(1)),
        (b"aq_const", [c]) => match *c {
            b"AQ_FIX_SYMBOL" => line(AQ_FIX_SYMBOL.as_bytes()),
            b"AQ_FIXED_AFTER" => line(AQ_FIXED_AFTER.as_bytes()),
            b"AQ_BUILD" => line(AQ_BUILD.as_bytes()),
            _ => Err(usage()),
        },
        (b"desk_facts", [before, after]) => out(crate::desk::desk_facts(before, after)),
        (b"desk_target", [what, snap, rest @ ..]) if rest.len() <= 1 => {
            use crate::desk::Target;
            let arg = rest.first().copied().unwrap_or_default();
            let t = match *what {
                b"keys" => Target::Keys,
                b"run" => Target::Run(arg),
                b"click" => Target::At("click", arg),
                b"scroll" => Target::At("scroll", arg),
                b"drag" => Target::At("drag", arg),
                _ => return Err(usage()),
            };
            out(crate::desk::desk_target_with(t, snap, || arg.to_vec()))
        }
        (b"job_exe", [a0]) => line(&crate::jobs::job_exe(a0)),
        (b"layer_select", [lys, sel]) => line(&crate::layer::layer_select(&ctx, lys, sel)?),
        (b"gdb_crashes", []) => out(crate::cmd::log::gdb_crashes(&stdin_all()?)),
        (b"desk_picture", []) => awk_stdin(&[crate::jqprog::DESK_PICTURE_AWK.as_bytes().to_vec()]),
        (b"zero_note", [m, ws, other]) => line(&crate::shots::zero_note(m, ws, Some(other))),
        (b"box_cmds", []) => {
            // (bash's BOX_CMDS: one line, a space around each)
            let mut v = b" ".to_vec();
            for c in crate::front::BOX_CMDS {
                v.extend_from_slice(c);
                v.push(b' ');
            }
            line(&v)
        }
        (b"repo_link_out", [top, p]) => {
            let [l, t, at] = crate::repo::repo_link_out(top, p).ok_or(Exit(1))?;
            line(&cat!(l, "\t", t, "\t", at))
        }
        (b"link_note", [what, l, t, how]) => {
            let how = std::str::from_utf8(how).map_err(|_| usage())?;
            crate::repo::link_note(what, l, t, how);
            Ok(())
        }
        (b"link_mountable", [p]) => yes(crate::repo::link_mountable(p).is_none()),
        (b"box_pid", [n]) => match crate::boxes::box_pid(&cat!(ctx.boxes, "/", n)) {
            crate::boxes::BoxPid::Pid(p) => line(&p),
            crate::boxes::BoxPid::None => Ok(()),
            crate::boxes::BoxPid::Unknown => Err(Exit(3)),
        },
        (b"box_pids", [n]) => {
            ctx.name = n.to_vec();
            ctx.d = cat!(ctx.boxes, "/", n);
            for p in crate::jobs::box_pids(&ctx) {
                line(&p)?;
            }
            Ok(())
        }
        (
            b"record_overlay",
            [
                input,
                marks,
                first,
                x0,
                y0,
                k,
                w,
                h,
                dur,
                show,
                caps,
                area @ ..,
            ],
        ) if area.len() <= 1 => {
            let area = match area.first() {
                Some(a) => a.to_vec(),
                None => cat!("0 0 ", w, " ", h),
            };
            let ass = crate::cmd::record_stop::record_overlay(
                input,
                marks,
                first,
                x0,
                y0,
                k,
                w,
                h,
                dur,
                *show == b"1",
                *caps == b"1",
                &area,
            )
            .ok_or(Exit(1))?;
            out(ass)
        }
        (b"jail_path", [p]) => line(&crate::jailpath::jail_path(p).ok_or(Exit(1))?),
        (b"jail_read", [f]) => out(crate::jailpath::jail_read(f).ok_or(Exit(1))?),
        (b"jail_root", [p]) => line(&crate::repo::jail_root(p).ok_or(Exit(1))?),
        (b"jail_root_covered", [p]) => line(&crate::jailpath::jail_root_covered(p).ok_or(Exit(1))?),
        (b"jail_deep", [p]) => yes(crate::jailpath::jail_deep(p)),
        (b"jail_readable", [f]) => yes(crate::jailpath::jail_readable(f)),
        (b"repo_top", []) => line(&crate::repo::repo_top().ok_or(Exit(1))?),
        (b"norepo_note", [how]) => {
            line(&crate::repo::norepo_note(&String::from_utf8_lossy(how)).ok_or(Exit(1))?)
        }
        // SRC, a tab, DEST; a refusal said, 1 (3 for a jailed caller's).
        (b"seed_spec", [spec]) => match crate::upspec::seed_spec(spec) {
            Ok((src, dest)) => line(&cat!(src, "\t", dest)),
            Err(crate::upspec::SeedRefused::Bad) => Err(Exit(1)),
            Err(crate::upspec::SeedRefused::Jail) => Err(Exit(3)),
        },
        // Each PLACE "AT<tab>KIND<tab>NAME", as bash's list; the one blocked, a tab, in or over.
        (b"seed_blocked", [src, rel, places @ ..]) => {
            let mut ps = Vec::new();
            for x in places {
                let f: Vec<&[u8]> = x.split(|&c| c == b'\t').collect();
                let [at, kind, name] = f.as_slice() else {
                    return Err(usage());
                };
                let kind = match *kind {
                    b"plugin" => "plugin",
                    b"theme" => "theme",
                    _ => return Err(usage()),
                };
                ps.push(crate::seed::Place {
                    at: at.to_vec(),
                    kind,
                    name: name.to_vec(),
                });
            }
            let (x, how) = crate::seed::seed_blocked(src, rel, &ps, &crate::jailpath::jail_path)
                .ok_or(Exit(1))?;
            line(&cat!(x.at, "\t", x.kind, "\t", x.name, "\t", how))
        }
        // bash's covers (--ro-bind EMPTY PLACE, one word a line), "--", then cover_srcs.
        (b"jail_cover", [d, src, dest]) => {
            let c = crate::jailpath::jail_cover(src, dest).ok_or(Exit(1))?;
            for (k, at) in &c.binds {
                let empty = match k {
                    crate::jailpath::CoverKind::File => "cover-file",
                    crate::jailpath::CoverKind::Dir => "cover-dir",
                };
                line(&cat!("--ro-bind\n", d, "/", empty, "\n", at))?;
            }
            line(b"--")?;
            match c.src {
                Some(s) => line(&s),
                None => Ok(()),
            }
        }
        (b"box_held", [d, pid]) => {
            ctx.d = d.to_vec();
            yes(crate::ready::box_held(&ctx, pid))
        }
        // What is not covered on stdout (its words, as `$(jail_verify ... 2>&1)` has them), 1.
        (b"jail_verify", [d, bp, srcs @ ..]) => crate::jailpath::jail_verify(d, bp, &owned(srcs))
            .or_else(|w| {
                out(w)?;
                Err(Exit(1))
            }),
        (b"jail_sees", [p]) => line(&crate::jailpath::jail_sees(p).ok_or(Exit(1))?),
        (b"jail_masked", [p]) => yes(crate::jailpath::jail_masked(p)),
        (b"default_name", []) => line(&crate::select::default_name()),
        // (printf '%s': no newline added)
        (b"caller_env", [n]) => out(crate::exec::caller_env(n).ok_or(Exit(1))?),
        (b"cmd_opts_keys", []) => {
            for k in crate::args::cmd_opts_keys() {
                line(k.as_bytes())?;
            }
            Ok(())
        }
        // One option a line, as bash's cmd_opts (an alias's: its command's).
        (b"cmd_opts", [c]) => {
            for o in crate::args::cmd_opts(c).ok_or(Exit(1))?.split(' ') {
                line(o.as_bytes())?;
            }
            Ok(())
        }
        (b"up_box_opts", []) => line(crate::args::UP_BOX_OPTS.as_bytes()),
        (b"usage_text", []) => out(crate::help::TEXT),
        // (nothing for off; a bad OMABOX_SHOT_FIT said, 1)
        (b"shot_fit_default", []) => match crate::shots::shot_fit_default() {
            Ok(v) if v.is_empty() => Ok(()),
            Ok(v) => line(&v),
            Err(()) => Err(Exit(1)),
        },
        // TOP, or repo_top's (none: 1), a jailed caller's read through its jail.
        (b"repo_git_dirs", top @ ([] | [_])) => {
            let top = match top.first() {
                Some(t) => t.to_vec(),
                None => crate::repo::repo_top().ok_or(Exit(1))?,
            };
            for d in crate::install::repo_git_dirs(&top, crate::jailpath::view()) {
                line(&d)?;
            }
            Ok(())
        }
        (b"idle_duration", [v, how]) => {
            let how = String::from_utf8_lossy(how);
            match crate::cmd::check::idle_duration(v, &how) {
                Ok(s) => line(s.to_string().as_bytes()),
                Err(rc) => Err(Exit(u8::try_from(rc).unwrap_or(1))),
            }
        }
        (b"pause_ms", [what, v]) => {
            let what = String::from_utf8_lossy(what);
            line(
                crate::input::pause_ms(&ctx, &what, v)?
                    .to_string()
                    .as_bytes(),
            )
        }
        // (OMABOX_READY_TIMEOUT's, whole seconds)
        (b"ready_timeout", []) => line(crate::ready::ready_timeout(&ctx)?.to_string().as_bytes()),
        (b"mods_add", [so_far, mods]) => line(&crate::input::mods_add(&ctx, so_far, mods)?),
        // OFFSET MIN_MS; a mark's from D's events.marks (NAME the box's, for its words).
        (b"events_since", [v, rest @ ..]) if matches!(rest.len(), 0 | 2) => {
            if let [d, n] = rest {
                ctx.d = d.to_vec();
                ctx.name = n.to_vec();
            }
            let (off, ms) = crate::cmd::events::events_since(&ctx, v)?;
            line(format!("{off} {ms}").as_bytes())
        }
        (b"cpu_sample", [proc, pids @ ..]) => {
            for l in crate::cmd::cpu::cpu_sample_in(proc, &owned(pids)) {
                line(&l)?;
            }
            Ok(())
        }
        (b"cpu_threads", [proc, pids @ ..]) => {
            for l in crate::cmd::cpu::cpu_threads_in(proc, &owned(pids)) {
                line(&l)?;
            }
            Ok(())
        }
        (b"cpu_tame", [n]) => line(&crate::cmd::cpu::cpu_tame(n)),
        // (awk -v clk= -v want= -v app= CPU_REPORT_AWK, as cpu runs it; then CPU_TABLE_AWK)
        (b"cpu_report", [clk, rest @ ..]) if rest.len() <= 2 => {
            let none: &[u8] = b"";
            let want = rest.first().copied().unwrap_or(none);
            let app = rest.get(1).copied().unwrap_or(none);
            let a = crate::jq::Args::new(&["-v"])
                .arg(cat!("clk=", clk))
                .arg("-v")
                .arg(cat!("want=", want))
                .arg("-v")
                .arg(cat!("app=", app))
                .arg(crate::jqprog::CPU_REPORT_AWK)
                .into_words();
            awk_stdin(&a)
        }
        (b"cpu_table", []) => awk_stdin(&[crate::jqprog::CPU_TABLE_AWK.as_bytes().to_vec()]),
        (b"diff_box", [a, b, masks @ ..]) => {
            match crate::shots::diff_box(a, b, &owned(masks)).ok_or(Exit(1))? {
                Some([x, y, w, h]) => line(format!("{x} {y} {w} {h}").as_bytes()),
                None => Ok(()),
            }
        }
        (b"diff_parts", [a, b, masks @ ..]) => {
            for p in crate::shots::diff_parts(a, b, &owned(masks)) {
                line(&p.join(&b' '))?;
            }
            Ok(())
        }
        (b"refuse_src", [p]) => refused(crate::repo::refuse_src(p)),
        (b"refuse_src", [p, l]) if *l == b"look" => refused(crate::repo::refuse_src_look(p)),
        (b"refuse_dest", [p]) => refused(crate::upspec::refuse_dest(p)),
        (b"robind_spec", [spec]) => {
            let (src, dest) = crate::upspec::robind_spec(spec).ok_or(Exit(1))?;
            line(&cat!(src, "\t", dest))
        }
        (b"monitor_spec", [spec]) => {
            let m = crate::mon::monitor_spec(spec).map_err(|()| Exit(1))?;
            line(&m.join(&b' '))
        }
        (b"own_path_visible", [path, rest @ ..]) => {
            let (mut rb, mut ov) = (Vec::new(), Vec::new());
            for pair in rest.chunks(2) {
                match pair {
                    [o, v] if *o == b"--robind" => {
                        let i = v.iter().position(|&c| c == b'\t').ok_or_else(usage)?;
                        rb.push((v[..i].to_vec(), v[i + 1..].to_vec()));
                    }
                    [o, v] if *o == b"--overlay" => ov.push(v.to_vec()),
                    _ => return Err(usage()),
                }
            }
            yes(crate::upspec::own_path_visible(path, &rb, &ov))
        }
        (b"theme_snapshot_note", [dir, top]) => {
            crate::seed::theme_snapshot_note(dir, top);
            Ok(())
        }
        (b"theme_named", [name, tree, dirs @ ..]) => line(&crate::plugins::theme_named(
            &ctx,
            name,
            tree,
            &owned(dirs),
            &crate::jailpath::jail_path,
        )?),
        (b"shell_warnings", [f]) => out(crate::plugins::shell_warnings(&crate::shell::path(f))),
        (b"desk_name_refused", [n]) => {
            refused(crate::seed::desk_name_refused(n).map(|m| m.as_bytes().to_vec()))
        }
        (b"desk_content_refused", [f]) => {
            refused(crate::seed::desk_content_refused(f).map(|m| m.as_bytes().to_vec()))
        }
        (b"desk_files", [desk]) => line(&crate::seed::desk_files(&ctx, desk, None)?),
        (b"desk_plugins", [desk, have @ ..]) => {
            let mut hv = Vec::new();
            for x in have {
                let i = x.iter().position(|&c| c == b'\t').ok_or_else(usage)?;
                hv.push(crate::plugins::Plugin {
                    id: x[..i].to_vec(),
                    dir: x[i + 1..].to_vec(),
                });
            }
            for p in crate::plugins::desk_plugins(desk, &hv) {
                line(&cat!(p.id, "\t", p.dir))?;
            }
            Ok(())
        }
        (b"desk_dir", []) => line(&crate::seed::desk_dir()),
        (b"desk_home", []) => line(&crate::seed::desk_home()),
        (b"no_seed_spec", [list, have @ ..]) if have.len() <= 1 => {
            let have = have.first().copied().unwrap_or(b"");
            line(&crate::upspec::no_seed_spec(list, have).map_err(|()| Exit(1))?)
        }
        (b"bar_seed_note", [desk, omarchy, keep, notify, pagers, split @ ..])
            if split.len() <= 1 =>
        {
            let split = split.first().is_some_and(|s| *s == b"true");
            crate::seed::bar_seed_note(desk, omarchy, keep, *notify == b"true", pagers, split);
            Ok(())
        }
        (b"shell_json_filter", [keep, notify, pagers, split @ ..]) if split.len() <= 1 => {
            let split = split.first().is_some_and(|s| *s == b"true");
            ran(crate::shelljson::shell_json_filter(
                keep,
                *notify == b"true",
                pagers,
                split,
                &stdin_all()?,
            ))
        }
        (b"place_plugin", [id, sec, kinds, mounted]) => ran(crate::shelljson::place_plugin(
            id,
            sec,
            kinds,
            mounted,
            &stdin_all()?,
        )),
        (b"widget_section", [sec, split]) => {
            line(&crate::shelljson::widget_section(sec, *split == b"true"))
        }
        // The mounted plugins as bash's PLUGINS lines on stdin (ID<TAB>DIR), their dirs read.
        (b"workspace_widgets", []) => {
            let dirs: Vec<Vec<u8>> = stdin_all()?
                .split(|&c| c == b'\n')
                .filter(|l| !l.is_empty())
                .map(|l| match l.iter().position(|&c| c == b'\t') {
                    Some(i) => l[i + 1..].to_vec(),
                    None => l.to_vec(),
                })
                .collect();
            ran(crate::shelljson::workspace_widgets(&dirs))
        }
        (b"cwd_path_word", [dir, home, words @ ..]) => {
            line(&crate::repo::cwd_path_word(dir, home, &owned(words)).ok_or(Exit(1))?)
        }
        (b"cwd_note", [name, how, w, dir, norepo]) => {
            crate::exit::say(crate::repo::cwd_note(
                name,
                &String::from_utf8_lossy(how),
                w,
                dir,
                *norepo == b"1",
            ));
            Ok(())
        }
        (b"agent_proc", []) => {
            let (pid, start) = crate::select::agent_proc().ok_or(Exit(1))?;
            line(&cat!(pid, " ", start))
        }
        (b"proc_start", [pid]) => line(&crate::procfs::proc_start(pid).ok_or(Exit(1))?),
        (b"proc_stat", [pid, i]) => {
            let i: usize = std::str::from_utf8(i)
                .ok()
                .and_then(|i| i.parse().ok())
                .ok_or_else(usage)?;
            line(&crate::procfs::proc_stat(pid, i).ok_or(Exit(1))?)
        }
        (b"agent_alive", [d]) => yes(crate::boxes::agent_alive(d)),
        (b"owner_alive", [pid, start]) => yes(crate::ready::owner_alive(pid, start)),
        (b"up_session_note", [n]) => {
            ctx.main_cmd = b"up".to_vec();
            ctx.name = n.to_vec();
            ctx.d = cat!(ctx.boxes, "/", n);
            crate::upspec::up_session_note(&ctx);
            Ok(())
        }
        (b"box_starter", []) => line(&crate::upspec::box_starter().ok_or(Exit(1))?),
        (b"box_orphan", [d]) => {
            ctx.d = d.to_vec();
            yes(crate::ready::box_orphan(&ctx))
        }
        (b"covers_held", [n]) => {
            ctx.name = n.to_vec();
            ctx.d = cat!(ctx.boxes, "/", n);
            crate::boxes::covers_held(&ctx)
        }
        (b"windows_table", []) => ran(crate::jq::jq(
            &["-r", crate::jqprog::WINDOWS_TABLE_JQ],
            Some(&stdin_all()?),
            crate::jq::Err::Inherit,
        )),
        (b"windows_layers", []) => ran(crate::jq::jq(
            &["-r", crate::jqprog::WINDOWS_LAYERS_JQ],
            Some(&stdin_all()?),
            crate::jq::Err::Inherit,
        )),
        (b"win_annotate", [c, m, act, l @ ..]) if l.len() <= 1 => {
            ran(crate::win::win_annotate(c, m, act, l.first().copied()))
        }
        (b"win_select", [w, sels @ ..]) => line(&crate::win::win_select(&ctx, w, &owned(sels))?),
        (b"win_answer", [want, w, sels @ ..]) => {
            let want = std::str::from_utf8(want).map_err(|_| usage())?;
            let (rc, o) = crate::cmd::wait::win_answer(want, w, &owned(sels));
            if !o.is_empty() {
                line(&o)?;
            }
            if rc == 0 {
                Ok(())
            } else {
                Err(Exit(u8::try_from(rc).unwrap_or(1)))
            }
        }
        (b"win_blocked", [w, x, y]) => {
            ran(crate::win::jq_xy(crate::jqprog::WIN_BLOCKED_JQ, w, x, y))
        }
        (b"win_layer", [w, x, y]) => ran(crate::win::jq_xy(crate::jqprog::WIN_LAYER_JQ, w, x, y)),
        _ => Err(usage()),
    }
}

/// awk ARGS over this process's stdin, as the command runs it on its own samples.
fn awk_stdin(args: &[Vec<u8>]) -> Res<()> {
    ran(crate::jq::tool(
        "awk",
        args,
        Some(&stdin_all()?),
        crate::jq::Err::Inherit,
    ))
}

/// The ROOT word, "/" when not given.
fn root<'a>(r: &[&'a [u8]]) -> &'a [u8] {
    r.first().copied().unwrap_or(b"/")
}

fn owned(v: &[&[u8]]) -> Vec<Vec<u8>> {
    v.iter().map(|x| x.to_vec()).collect()
}

/// A refusal's reason said (0), or 1 when there is none.
fn refused(why: Option<Vec<u8>>) -> Res<()> {
    match why {
        Some(m) => line(&m),
        None => Err(Exit(1)),
    }
}

fn stdin_all() -> Res<Vec<u8>> {
    let mut v = Vec::new();
    std::io::Read::read_to_end(&mut std::io::stdin(), &mut v).map_err(|_| Exit(1))?;
    Ok(v)
}

/// A jq or awk run's output, and its status (1 for any failure).
fn ran(r: crate::jq::Ran) -> Res<()> {
    out(&r.out)?;
    if r.rc == 0 { Ok(()) } else { Err(Exit(1)) }
}

/// A test's answer as its status: 0 yes, 1 no.
fn yes(b: bool) -> Res<()> {
    if b { Ok(()) } else { Err(Exit(1)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_none_writes() {
        let mut seen = std::collections::HashSet::new();
        for (n, _) in NAMES {
            assert!(seen.insert(*n), "{n} twice");
            // (host_hyprctl passes its words to the real desktop's hyprctl: never a probe)
            assert_ne!(*n, "host_hyprctl");
        }
    }
}
