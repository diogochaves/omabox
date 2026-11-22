//! `omabox run [-b NAME] [-d ...] [--pass VAR]... [--env-file FILE]... [UP OPTIONS] -- CMD [ARG...]`:
//! a command in the box (box_exec), its exit status passed through, run's own failures 125 (#273);
//! with -d a job of its own, recorded (#215, #286); with no box up, a throwaway box for the one
//! command (its own `up` and `down`, as subprocesses until those are ported). run -d --wait: until
//! what it opens has settled (settle.rs, the machinery `wait`, `keys` and `click` share), and what
//! the desktop did meanwhile (#274).

use crate::args::{optarg, unknown_opt_hint};
use crate::boxes::{box_alive, box_home, need_box, take_over, touch};
use crate::cat;
use crate::exec::{ExecOpts, box_exec, pipe, read_all};
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::jail::in_jail;
use crate::jobs::{job_ended, job_note, job_record, job_replace, run_mark_begin, run_mark_end};
use crate::num::ms_duration;
use crate::repo::{jail_root, norepo_note, refuse_src, repo_top, run_cwd_note, run_link_notes};
use crate::select::{env_names_box, select_box, session_boxes};
use crate::settle::{Settle, cursor_now, cursor_rect, settle_sent};
use crate::shell::{self, Re};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::ExitStatusExt;
use std::time::Duration;

/// up's options, for a throwaway box: those that take a value, and the flags.
pub(crate) const UP_VALUED: [&str; 20] = [
    "--size",
    "--plugin",
    "--ro-bind",
    "--overlay",
    "--seed",
    "--no-seed",
    "--theme-dir",
    "--theme",
    "--monitor",
    "--net",
    "--allow",
    "--addr",
    "--idle",
    "--from",
    "--hyprland",
    "--quickshell",
    "--omarchy",
    "--keep",
    "--desktop",
    "--shell",
];
pub(crate) const UP_FLAGS: [&str; 13] = [
    "--xwayland",
    "--autoreload",
    "--no-shell",
    "--no-pointer",
    "--dnd",
    "--no-dock",
    "--stock-bar",
    "--desk-shell",
    "--systemd",
    "--audio",
    "--polkit",
    "--clean",
    "--stock",
];

/// env_reserved: a variable the box session sets, or that changes how every program in it runs
/// (finding 175); OMABOX_BOX names the box (#172); OMABOX_THEME is up --theme's (finding 245).
pub fn env_reserved(k: &[u8]) -> bool {
    matches!(
        k,
        b"PATH"
            | b"HOME"
            | b"XDG_RUNTIME_DIR"
            | b"XDG_DATA_HOME"
            | b"XDG_CONFIG_HOME"
            | b"XDG_CACHE_HOME"
            | b"XDG_STATE_HOME"
            | b"WAYLAND_DISPLAY"
            | b"WAYLAND_SOCKET"
            | b"DISPLAY"
            | b"HYPRLAND_INSTANCE_SIGNATURE"
            | b"LD_PRELOAD"
            | b"LD_LIBRARY_PATH"
            | b"BASH_ENV"
            | b"ENV"
            | b"OMABOX_BOX"
            | b"OMABOX_THEME"
    )
}

/// run --env-file FILE (#30): its KEY=VAL lines. `export ` and # comment lines allowed, a value in
/// matching quotes loses them; nothing expanded; a bad line refused with its number, never its text.
pub fn envfile_read(ctx: &Ctx, f: &[u8]) -> Res<Vec<Vec<u8>>> {
    let p = shell::path(f);
    let readable = std::ffi::CString::new(f.to_vec()).is_ok_and(|c| {
        // SAFETY: c is a valid C string for the call.
        unsafe { libc::access(c.as_ptr(), libc::R_OK) == 0 }
    });
    let text = match std::fs::read(&p) {
        Ok(t) if p.is_file() && readable => t,
        _ => return Err(ctx.die(cat!("--env-file: cannot read ", f))),
    };
    let skip = Re::new(r"^[[:space:]]*(#|$)");
    let kv = Re::new(r"^[[:space:]]*(export[[:space:]]+)?([A-Za-z_][A-Za-z0-9_]*)=(.*)$");
    let dq = Re::new("^\"(.*)\"$");
    let sq = Re::new("^'(.*)'$");
    let mut out = Vec::new();
    let mut lines: Vec<&[u8]> = text.split(|&c| c == b'\n').collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    for (n, line) in lines.into_iter().enumerate() {
        // (read drops NUL bytes)
        let line: Vec<u8> = line.iter().copied().filter(|&c| c != 0).collect();
        let line = line.strip_suffix(b"\r").unwrap_or(&line).to_vec();
        if skip.as_ref().is_some_and(|r| r.is_match(&line)) {
            continue;
        }
        let m = kv
            .as_ref()
            .and_then(|r| Some((r.group(&line, 2)?, r.group(&line, 3)?)));
        let Some((k, mut v)) = m else {
            return Err(ctx.die(cat!(
                "--env-file ",
                f,
                ": line ",
                (n + 1).to_string(),
                " is not KEY=VALUE"
            )));
        };
        if env_reserved(&k) {
            return Err(ctx.die(cat!(
                "--env-file ",
                f,
                ": line ",
                (n + 1).to_string(),
                " sets ",
                k,
                ": it is the box session's own"
            )));
        }
        if let Some(x) = dq
            .as_ref()
            .and_then(|r| r.group(&v, 1))
            .or_else(|| sq.as_ref().and_then(|r| r.group(&v, 1)))
        {
            v = x;
        }
        out.push(cat!(k, "=", v));
    }
    Ok(out)
}

/// The settle options (--wait's), taken by settle_opt.
const SETTLE: [&str; 7] = [
    "--wait",
    "--start",
    "--timeout",
    "--strict",
    "--json",
    "--ignore",
    "-g",
];

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    // run's own failures are 125, never a code its command could have given (#273, audit X3).
    ctx.die_rc = Some(125);
    let (mut name, mut detach, mut quiet, mut print_log, mut replace) =
        (Vec::new(), false, false, false, false);
    let (mut up_args, mut envs): (Vec<Vec<u8>>, Vec<Vec<u8>>) = (Vec::new(), Vec::new());
    let mut st = Settle::default();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let s = std::str::from_utf8(a).unwrap_or("");
        match s {
            "-b" | "--box" => {
                optarg(ctx, a, args.get(i + 1))?;
                name = args[i + 1].clone();
                i += 1;
            }
            "-d" | "--detach" => detach = true,
            "-q" => quiet = true,
            "--print-log" => print_log = true,
            "--replace" => replace = true,
            "--quiet" => {
                if !args.get(i + 1).is_some_and(|v| ms_duration(v).is_some()) {
                    return Err(ctx.die(
                        "run: --quiet takes a duration (--wait's quiet period); -q is the flag that silences run -d",
                    ));
                }
                st.opt(ctx, &args[i..])?;
                i += st.took - 1;
            }
            _ if SETTLE.contains(&s) => {
                st.opt(ctx, &args[i..])?;
                i += st.took - 1;
            }
            "--pass" => {
                optarg(ctx, a, args.get(i + 1))?;
                let v = &args[i + 1];
                if !shell::re_match("^[A-Za-z_][A-Za-z0-9_]*$", v) {
                    return Err(ctx.die(cat!("run: --pass takes a variable name, got ", v)));
                }
                if env_reserved(v) {
                    return Err(ctx.die(cat!(
                        "run: --pass cannot set ",
                        v,
                        ": it is the box session's own"
                    )));
                }
                if crate::exec::caller_env(v).is_none() {
                    return Err(ctx.die(cat!(
                        "run: --pass ",
                        v,
                        ": not in this command's environment (export it first)"
                    )));
                }
                ctx.pass.push(v.clone());
                i += 1;
            }
            "--env-file" => {
                optarg(ctx, a, args.get(i + 1))?;
                if in_jail() {
                    ctx.parsed();
                    return Err(ctx.die(
                        "run: --env-file is read by a jailed agent's own omabox, never at a path here",
                    ));
                }
                let kvs = envfile_read(ctx, &args[i + 1])?;
                ctx.envkv.extend(kvs);
                i += 1;
            }
            // The command's on a box that is up (#130), a throwaway box's otherwise.
            "--env" => {
                optarg(ctx, a, args.get(i + 1))?;
                let v = &args[i + 1];
                if !shell::re_match("^[A-Za-z_][A-Za-z0-9_]*=", v) {
                    return Err(ctx.die(cat!("run: --env wants KEY=VAL, got ", v)));
                }
                let k = &v[..v.iter().position(|&c| c == b'=').unwrap_or(v.len())];
                if env_reserved(k) {
                    return Err(ctx.die(cat!(
                        "run: --env cannot set ",
                        k,
                        ": it is the box session's own"
                    )));
                }
                envs.push(v.clone());
                i += 1;
            }
            _ if UP_VALUED.contains(&s) => {
                optarg(ctx, a, args.get(i + 1))?;
                up_args.extend([a.clone(), args[i + 1].clone()]);
                i += 1;
            }
            _ if UP_FLAGS.contains(&s) => up_args.push(a.clone()),
            "-h" | "--help" => return crate::args::usage("run"),
            "--" => {
                i += 1;
                break;
            }
            _ if a.starts_with(b"-") => {
                return Err(unknown_opt_hint(
                    ctx,
                    "run",
                    a,
                    "(the command goes after --; --interactive is up's only)",
                ));
            }
            _ => break,
        }
        i += 1;
    }
    let cmd: Vec<Vec<u8>> = args[i.min(args.len())..].to_vec();
    if cmd.is_empty() {
        return Err(ctx.die("run: nothing to run (omabox run -- CMD ...)"));
    }
    st.opts_check(ctx, 0)?;
    if st.wait && !detach {
        return Err(
            ctx.die("run: --wait goes with -d (a command run to its end has nothing to wait for)")
        );
    }
    if (quiet || print_log || replace) && !detach {
        return Err(ctx.die("run: -q, --print-log and --replace go with -d"));
    }
    select_box(ctx, &name)?;
    if box_alive(ctx)? {
        if !up_args.is_empty() {
            say(cat!(
                "box '",
                ctx.name,
                "' is already up: ",
                up_args.join(&b' '),
                " ignored"
            ));
        }
        ctx.envkv.extend(envs); // after --env-file's, so the flag wins; --pass wins over both
        crate::whose::mark_used(ctx); // use, for idle expiry (so is its end, below)
        if !take_over(ctx, false)? {
            need_box(ctx)?; // gone while it waited: says so
        }
        job_note(ctx); // (need_box's, when it did not run)
        run_link_notes(ctx, "down, then up", false, &cmd); // through a link out of the repo (#235)
        run_cwd_note(ctx, "down, then up", &cmd); // a path in this dir the box does not have (#251)
        crate::repo::mount_gone_notes(ctx); // a folder bound at up, replaced on the host since (#391)
        let words = [cmd.as_slice(), &ctx.envkv].concat();
        crate::repo::host_path_notes(ctx, "down, then up", &words); // one the box has elsewhere, or not (#301)
        if detach {
            return detached(ctx, &mut st, quiet, print_log, replace, &cmd);
        }
        // (a job it stops is no news: job_note; any status, when it kills: #329)
        let mark = run_mark_begin(ctx, crate::jobs::kill_cmd(&cmd));
        let opts = ExecOpts {
            kill_say: true,
            ..ExecOpts::default()
        };
        let note = WaitNote::start(wait_note_text(&ctx.name, &cmd, false));
        let rc = box_exec(ctx, &cmd, opts);
        note.stop();
        let rc = rc?;
        run_mark_end(ctx, mark);
        touch(&ctx.at("used"));
        let rc = run_gone(ctx, rc)?;
        // hyprctl keyword in a box: hyprctl's bare `unknown request`, then what sets the option (#471).
        for l in crate::cmd::hyprctl::run_hint(&ctx.name, &cmd) {
            say(l);
        }
        return code(rc);
    }
    if detach {
        // This session's box, up under another directory's name: named, not an `up` that would
        // start a second box (#296).
        if ctx.name_derived && !session_boxes(ctx).is_empty() {
            need_box(ctx)?;
        }
        return Err(ctx.die(cat!(
            "run -d needs a box that is up (omabox up ",
            ctx.name,
            ")"
        )));
    }
    for kv in envs {
        up_args.extend([b"--env".to_vec(), kv]);
    }
    // A box asked for by name, one that was up and died, or one that expired or a down took (#295)
    // is not replaced by a throwaway: that would hide what happened to it. Nor this session's box up
    // under another directory's name, as run -d (#327: the command ran in a throwaway without its
    // plugin).
    if !name.is_empty()
        || (ctx.name_derived && !session_boxes(ctx).is_empty())
        || env_names_box()
        || shell::path(&ctx.d).is_dir()
        || ctx.in_boxes(cat!(".expired-", ctx.name)).is_file()
        || ctx.in_boxes(cat!(".taken-", ctx.name)).is_file()
    {
        need_box(ctx)?;
    }
    throwaway(ctx, up_args, &cmd)
}

/// `return RC`.
fn code(rc: i32) -> Res<()> {
    crate::cmd::hyprctl::code(rc)
}

/// run_gone RC: a box ended from inside while its command ran (#128) is said, exit 125 (#273): its
/// command's status was the box's end. A command the box's end killed (137) can return a moment
/// before its PID 1 is gone: waited for then only.
fn run_gone(ctx: &Ctx, rc: i32) -> Res<i32> {
    if rc == 137 {
        for _ in 0..10 {
            if !box_alive(ctx)? {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    if rc != 0 && !box_alive(ctx)? {
        let n = &ctx.name;
        say(cat!(
            "box '",
            n,
            "' went down while this command ran, exit 125 (its Hyprland, omabox-labwc and ",
            "quickshell are the box's own: stop your own process by its PID; why: omabox log -b ",
            n,
            " all)"
        ));
        return Ok(125);
    }
    Ok(rc)
}

/// The launcher run -d starts in the box: its log opened there as a new file (finding 229), the job
/// started by share/run-job.sh (#215) as a session of its own, its pid said on fd 3. Bash's, byte for
/// byte.
const RUN_D_SH: &str = r#"/usr/bin/rm -f -- "$1" && set -C && exec >"$1" 2>&1 || exit 125; set +C; ex=$2; shift 2
        [ ! -x /opt/omabox/share/run-job.sh ] || exec /usr/bin/setsid -f /opt/omabox/share/run-job.sh "$ex" 4< <(printf "%s\0" "$@")
        (trap - INT QUIT; exec 3>&-; exec /usr/bin/setsid -- "$@") & echo "$!" >&3"#;

/// run -d: the command as a job of its own, recorded; its first moments watched (#286).
fn detached(
    ctx: &mut Ctx,
    st: &mut Settle,
    quiet: bool,
    print_log: bool,
    replace: bool,
    cmd: &[Vec<u8>],
) -> Res<()> {
    // --replace: the earlier job of this command stopped and its windows gone first (finding 110).
    if replace {
        job_replace(ctx, quiet, cmd)?;
    }
    // (bash's `$(date +%s%N)`, with no `date` run; OMABOX_TEST_RUN_NS under the suite, which names
    // a log by it: digits only, anything else ignored)
    let ns = Some(shell::env_or_empty("OMABOX_TEST_RUN_NS"))
        .filter(|v| {
            !v.is_empty()
                && v.iter().all(u8::is_ascii_digit)
                && !shell::env_or_empty("OMABOX_SUITE").is_empty()
        })
        .unwrap_or_else(shell::epoch_ns);
    let lname = cat!("run-", ns, ".log");
    let ex = cat!(
        format!("/run/user/{}/omabox.run-", crate::procfs::uid()),
        ns,
        ".exit"
    );
    // --wait: until what it opens has settled (a first frame within 5 s unless --start says).
    if st.wait {
        let p = cursor_now(ctx)?;
        if !p.is_empty() {
            let r = cursor_rect(&p).ok_or_else(|| {
                ctx.die(cat!(
                    "run: cannot read the pointer's place as numbers: '",
                    p,
                    "'"
                ))
            })?;
            st.args.extend([b"--ignore".to_vec(), r]);
        }
        st.facts = "run";
        let mut tool: Vec<Vec<u8>> =
            vec![b"settle".to_vec(), b"--start".to_vec(), b"5000".to_vec()];
        tool.extend(st.args.iter().cloned());
        st.begin(ctx, &tool)?;
        let json = st.json;
        st.ready(ctx, json)?;
    }
    let mut argv = vec![
        b"/usr/bin/bash".to_vec(),
        b"-c".to_vec(),
        RUN_D_SH.as_bytes().to_vec(),
        b"omabox-run".to_vec(),
        cat!(box_home(&ctx.d), "/", lname),
        ex.clone(),
    ];
    argv.extend(cmd.iter().cloned());
    // (fd 3 to a pipe read here, its first 64 bytes: the job's pid in the box)
    let (r, w0) = pipe().map_err(|_| Exit(125))?;
    let w = crate::exec::dup_hi(std::os::fd::AsRawFd::as_raw_fd(&w0)).map_err(|_| Exit(125))?;
    drop(w0);
    let reader = std::thread::spawn(move || read_all(r, Some(64)));
    let opts = ExecOpts {
        stdin_null: true,
        stdout_to_stderr: true,
        fd3: Some(w),
        ..ExecOpts::default()
    };
    let started = box_exec(ctx, &argv, opts);
    let jpid = shell::strip_nl(&reader.join().unwrap_or_default());
    if !matches!(started, Ok(0)) {
        let down = if box_alive(ctx)? {
            String::new()
        } else {
            ": the box went down".to_string()
        };
        return Err(ctx.die(cat!(
            "run -d: could not start the command in box '",
            ctx.name,
            "'",
            down,
            " (what the box said, if anything, is above; omabox log -b ",
            ctx.name,
            " all)"
        )));
    }
    let log = cat!(ctx.d, "/home/", lname);
    let numeric = shell::is_num(&jpid);
    if numeric {
        job_record(ctx, &jpid, &log, cmd);
        st.fpid = jpid.clone();
    }
    // stdout's first line, data even under -q: the job's pid (`J=$(omabox run -d -- APP)`, then
    // `log run "$J"`, `wait job "$J"`, `stop "$J"`: #429), or the log's path with --print-log;
    // --wait --json keeps stdout one JSON document, the pid its "job". The pid, unasked for, never
    // fails a job that started: a closed stdout only goes without it.
    if print_log {
        out(cat!(log, "\n"))?;
    } else if numeric && !(st.wait && st.json) {
        let _ = out(cat!(jpid, "\n"));
    }
    // Its first moments (#286): a job over by then is said with its status and log instead of
    // "started", and run -d exits with that status. --wait has its own answer, below.
    if !st.wait && numeric {
        let exf = ex.rsplit(|&c| c == b'/').next().unwrap_or(&ex).to_vec();
        let there = ctx.at("run").join(std::ffi::OsStr::from_bytes(&exf));
        for _ in 0..6 {
            if there.exists() || there.is_symlink() {
                if let Some(rc) = job_ended(ctx, false, quiet, &jpid, &lname, &ex, cmd)? {
                    return code(rc);
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    // (its pid, as `jobs` lists it: what `stop` and `wait job` take, #299)
    if !quiet {
        // (and as the box has it, for a command run there: a grep of it in wait until, #301)
        let pid = if numeric {
            cat!("pid ", jpid, ", ")
        } else {
            Vec::new()
        };
        say(cat!(
            "started in box '",
            ctx.name,
            "' (",
            pid,
            "log: ",
            log,
            ", ",
            box_home(&ctx.d),
            "/",
            lname,
            " in the box)"
        ));
    }
    if !st.wait {
        return Ok(());
    }
    let json = st.json;
    let rc = st.end(ctx, "settle", json, b"")?;
    // Not settled, and the job already over: that, not the screen, is the answer (#215).
    if rc == 124 {
        if let Some(r) = job_ended(ctx, true, true, &jpid, &lname, &ex, cmd)? {
            return code(r);
        }
    }
    settle_sent(rc, b"the command was started", st.looked);
    code(rc)
}

/// A throwaway box for this one command, its repo as an overlay (the host checkout never changes);
/// only a repo that may go into a box, never the directory we happen to be in (finding 63).
fn throwaway(ctx: &mut Ctx, up_args: Vec<Vec<u8>>, cmd: &[Vec<u8>]) -> Res<()> {
    let mut overlay: Vec<Vec<u8>> = Vec::new();
    if let Some(top) = repo_top() {
        if let Some(why) = refuse_src(&top) {
            say(cat!("not overlaying ", top, ": ", why));
        } else if in_jail() && jail_root(&top).is_none() {
            // A jailed caller's project with paths its jail hides (#210): read-only, those hidden.
            say(cat!(
                "not overlaying ",
                top,
                ": this jail hides paths in it; the command sees it read-only, with those hidden"
            ));
        } else {
            overlay = vec![b"--overlay".to_vec(), top];
        }
    } else if let Some(n) = norepo_note("run") {
        say(n);
    }
    // The name keeps its -run<pid> whatever the base's length: two runs in one repo never share a box.
    let base = ctx.name.clone();
    let me = std::process::id();
    let name = cat!(shell::cut_c(&ctx.name, 28), format!("-run{me}"));
    select_box(ctx, &name)?;
    // Taken down however this ends (a teardown that fails says why; the box's reaper retries it
    // once this process is gone): on the way out, and on INT, TERM and HUP, as bash's EXIT trap.
    let guard = crate::sig::Teardown::new(ctx.name.clone());
    let r = throwaway_run(ctx, overlay, up_args, cmd, me);
    guard.finish();
    // hyprctl keyword (#471): what sets the option, in a box that stays up.
    let hint = crate::cmd::hyprctl::run_hint(&base, cmd);
    if !hint.is_empty() {
        say(cat!(
            "with no box up this ran in a box of its own, gone with it: omabox up ",
            shell::quote_q(&base),
            " first, then"
        ));
        for l in hint {
            say(l);
        }
    }
    r
}

fn throwaway_run(
    ctx: &mut Ctx,
    overlay: Vec<Vec<u8>>,
    up_args: Vec<Vec<u8>>,
    cmd: &[Vec<u8>],
    me: u32,
) -> Res<()> {
    // `up` as its own process: its errors reach the user. --owner: if this process is killed
    // outright, the box's reaper takes the box down.
    let mut c = crate::sig::omabox_cmd();
    c.arg("up").arg(std::ffi::OsStr::from_bytes(&ctx.name));
    c.args(overlay.iter().map(|a| std::ffi::OsStr::from_bytes(a)));
    c.args(["--idle", "0", "--owner"]).arg(me.to_string());
    c.args(up_args.iter().map(|a| std::ffi::OsStr::from_bytes(a)));
    c.stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::null());
    let o = c.output();
    let (ok, err, how) = match o {
        Ok(o) => (
            o.status.success(),
            shell::subst(&o.stderr),
            match (o.status.code(), o.status.signal()) {
                (Some(n), _) => UpEnd::Status(n),
                (None, Some(s)) => UpEnd::Signal(s),
                _ => UpEnd::NotStarted,
            },
        ),
        Err(_) => (false, Vec::new(), UpEnd::NotStarted),
    };
    if !ok {
        // Its logs, out of the dir the teardown removes: the message names the copies.
        let kept = keep_run_logs(ctx);
        let mut err = err;
        if let Some(k) = &kept {
            err = replace_all(&err, &cat!(ctx.d, "/home/"), &cat!(k, "/"));
            err = replace_all(&err, &cat!(ctx.d, "/"), &cat!(k, "/"));
        }
        let up_line = Re::new(&format!(
            "^omabox: box '{}' up ",
            String::from_utf8_lossy(&ctx.name)
        ));
        let mut text = Vec::new();
        for l in err.split(|&c| c == b'\n') {
            if !up_line.as_ref().is_some_and(|r| r.is_match(l)) {
                text.extend_from_slice(&cat!(l, "\n"));
            }
        }
        let _ = std::io::Write::write_all(&mut std::io::stderr(), &text);
        let said = text.iter().any(|c| !c.is_ascii_whitespace());
        return Err(ctx.die(failed_start_text(said, &how, kept.as_deref(), cmd)));
    }
    // Its warnings and notes still reach the caller (finding 116).
    let mut notes = Vec::new();
    for l in err.split(|&c| c == b'\n') {
        if l.starts_with(b"omabox: warning:") || l.starts_with(b"omabox: note:") {
            notes.extend_from_slice(&cat!(l, "\n"));
        }
    }
    let _ = std::io::Write::write_all(&mut std::io::stderr(), &notes);
    run_link_notes(ctx, "run", true, cmd); // (links at the top of the repo: its up said them)
    run_cwd_note(ctx, "run", cmd); // outside a repo: nothing of this dir in the box (#251)
    // (and --env's values, which went to its up)
    let envs = up_args
        .windows(2)
        .filter(|w| w[0] == b"--env")
        .map(|w| w[1].clone());
    let words: Vec<Vec<u8>> = cmd
        .iter()
        .cloned()
        .chain(ctx.envkv.iter().cloned())
        .chain(envs)
        .collect();
    crate::repo::host_path_notes(ctx, "run", &words); // a host path the box has elsewhere, or not (#301)
    let opts = ExecOpts {
        kill_say: true,
        ..ExecOpts::default()
    };
    let note = WaitNote::start(wait_note_text(&ctx.name, cmd, true));
    let rc = box_exec(ctx, cmd, opts);
    note.stop();
    code(run_gone(ctx, rc?)?)
}

/// How a throwaway box's `up` ended.
enum UpEnd {
    Status(i32),
    Signal(i32),
    NotStarted,
}

/// run's line when its throwaway box's up failed (#478: `omabox run hyprland &` got only `throwaway
/// box failed to start`: its up was killed with the shell that left it in the background, and said
/// nothing). SAID: up said why (its lines are above); else how it ended, and what to run instead.
/// KEPT: where its logs were kept. CMD: the command run was to run.
fn failed_start_text(said: bool, how: &UpEnd, kept: Option<&[u8]>, cmd: &[Vec<u8>]) -> Vec<u8> {
    let k = kept
        .map(|k| cat!("; its logs are kept in ", k, " (box.log first)"))
        .unwrap_or_default();
    if said {
        return cat!("throwaway box failed to start: its up said why, above", k);
    }
    let words: Vec<u8> = cmd
        .iter()
        .map(|a| crate::jobs::word(a))
        .collect::<Vec<_>>()
        .join(&b' ');
    let shown = crate::cmd::log::cut_line(&words, 60);
    let then = cat!(
        "omabox up, then omabox run -- ",
        shown,
        " (run -d for an app, to leave it running)"
    );
    let why = match how {
        UpEnd::Signal(s @ (1 | 2 | 15)) => {
            let n = match s {
                1 => "HUP",
                2 => "INT",
                _ => "TERM",
            };
            cat!(
                "its up was killed by signal ",
                s.to_string(),
                " (",
                n,
                ") before it said why: run and its box go with the shell or job that started them (a run left with & ends with its shell); a box that stays: ",
                then
            )
        }
        UpEnd::Signal(s) => cat!(
            "its up was killed by signal ",
            s.to_string(),
            " before it said why; its start, by hand: ",
            then
        ),
        UpEnd::Status(n) => cat!(
            "its up ended with status ",
            n.to_string(),
            " and said nothing; its start, by hand: ",
            then
        ),
        UpEnd::NotStarted => b"its up could not be started (omabox doctor)".to_vec(),
    };
    cat!("throwaway box failed to start: ", why, k)
}

/// How long `omabox run` (no -d) waits on its command before it says so: 10 s, or under the suite
/// OMABOX_TEST_RUN_NOTE_MS.
fn wait_note_after() -> Duration {
    let ms = Some(shell::env_or_empty("OMABOX_TEST_RUN_NOTE_MS"))
        .filter(|v| !shell::env_or_empty("OMABOX_SUITE").is_empty() && !v.is_empty())
        .and_then(|v| std::str::from_utf8(&v).ok()?.parse::<u64>().ok())
        .unwrap_or(10_000);
    Duration::from_millis(ms)
}

/// What `omabox run` says when its command CMD still runs after wait_note_after (#471: a benchmark
/// model's `omabox run python3 hello.py` waited on the GTK app, silent, until its harness killed it
/// at 120 s). In box NAME, or in a throwaway box of its own (THROWAWAY), where -d is not there.
fn wait_note_text(name: &[u8], cmd: &[Vec<u8>], throwaway: bool) -> Vec<u8> {
    let words: Vec<u8> = cmd
        .iter()
        .map(|a| crate::jobs::word(a))
        .collect::<Vec<_>>()
        .join(&b' ');
    let shown = crate::cmd::log::cut_line(&words, 80);
    let secs = wait_note_after().as_secs_f64();
    let head = cat!(
        "note: ",
        shown,
        format!(
            " still runs after {secs:.0} s: run waits for it to exit (an app, until its window is closed); "
        )
    );
    if throwaway {
        cat!(
            head,
            "with no box up it runs in a box of its own; to leave an app running: omabox up, then omabox run -d -- ",
            words,
            "; to see it once: omabox look app -- ",
            words
        )
    } else {
        cat!(
            head,
            "omabox run -d -b ",
            shell::quote_q(name),
            " -- ",
            words,
            " leaves it running and returns"
        )
    }
}

/// The note wait_note_text makes, said on stderr once the command has run wait_note_after, unless
/// it ended first; never to a terminal (a person running an interactive program knows).
struct WaitNote(Option<(std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>)>);

impl WaitNote {
    fn start(text: Vec<u8>) -> WaitNote {
        if crate::exec::isatty(0) {
            return WaitNote(None);
        }
        let after = wait_note_after();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let h = std::thread::spawn(move || {
            if rx.recv_timeout(after) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
                say(text);
            }
        });
        WaitNote(Some((tx, h)))
    }

    fn stop(self) {
        if let Some((tx, h)) = self.0 {
            drop(tx);
            let _ = h.join();
        }
    }
}

/// `${V//FROM/TO}`.
fn replace_all(v: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    if from.is_empty() {
        return v.to_vec();
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < v.len() {
        if v[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(v[i]);
            i += 1;
        }
    }
    out
}

/// What keep_run_logs copies of one log at most.
const KEEP_CAP: u64 = 64 << 20;

/// keep_run_logs: a throwaway whose `up` failed: its logs, copied out of the box dir its `down`
/// removes (finding 167), into <state>/omabox/failed-runs/<name>-<date>; None when there was none to
/// copy. Only regular files inside the box's own dirs, opened with no link on their way (S7), each
/// cut at KEEP_CAP. The 10 newest such dirs kept.
fn keep_run_logs(ctx: &Ctx) -> Option<Vec<u8>> {
    let state = shell::env("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| cat!(shell::env_or_empty("HOME"), "/.local/state"));
    let base = cat!(state, "/omabox/failed-runs");
    if !ctx.at("box.json").is_file() {
        return None;
    }
    let date = shell::date("%Y%m%d-%H%M%S");
    let keep = cat!(base, "/", ctx.name, "-", date);
    std::fs::create_dir_all(shell::path(&base)).ok()?;
    std::fs::create_dir(shell::path(&keep)).ok()?;
    // (each copy written from the fd the box's file was opened by, no link followed on its way and
    // no FIFO waited on, bounded: never a look at its path, then a copy by name, S7)
    let copy = |from: Option<std::fs::File>, to: &[u8]| {
        let Some(from) = from else { return false };
        use std::os::unix::fs::OpenOptionsExt;
        let Ok(mut out) = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(shell::path(to))
        else {
            return false;
        };
        std::io::copy(&mut std::io::Read::take(from, KEEP_CAP), &mut out).is_ok()
    };
    let mut n = 0;
    if copy(
        crate::boxes::open_box_file(&ctx.at("box.log")),
        &cat!(keep, "/box.log"),
    ) {
        n += 1;
    }
    for f in shell::glob(&ctx.at("home"), false) {
        if !f.ends_with(b".log") {
            continue;
        }
        let name = String::from_utf8_lossy(&f).into_owned();
        if copy(
            crate::boxes::box_home_file(&ctx.d, &name),
            &cat!(keep, "/", f),
        ) {
            n += 1;
        }
    }
    for s in shell::glob(&ctx.at("run/hypr"), true) {
        let rel = cat!("hypr/", s, "/hyprland.log");
        let f = crate::boxes::open_beneath(&ctx.at("run"), &String::from_utf8_lossy(&rel));
        if copy(f, &cat!(keep, "/hyprland.log")) {
            n += 1;
        }
    }
    if n == 0 {
        let _ = std::fs::remove_dir(shell::path(&keep));
        return None;
    }
    // Past 10, the oldest go: only names a box's dir can have, so the rm is inside base.
    // (find -mindepth 1 -maxdepth 1 -type d: hidden ones too, no link followed)
    let mut dirs: Vec<(f64, Vec<u8>)> = std::fs::read_dir(shell::path(&base))
        .map(|rd| {
            rd.flatten()
                .map(|e| std::os::unix::ffi::OsStringExt::into_vec(e.file_name()))
                .collect()
        })
        .unwrap_or_else(|_| Vec::new())
        .into_iter()
        .filter_map(|d: Vec<u8>| {
            let m = std::fs::symlink_metadata(shell::path(&cat!(base, "/", d))).ok()?;
            use std::os::unix::fs::MetadataExt;
            m.is_dir()
                .then(|| (m.mtime() as f64 + m.mtime_nsec() as f64 / 1e9, d))
        })
        .collect();
    dirs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, d) in dirs.into_iter().skip(10) {
        if shell::re_match("^[A-Za-z0-9][A-Za-z0-9_.-]*$", &d) {
            let _ = std::fs::remove_dir_all(shell::path(&cat!(base, "/", d)));
        }
    }
    Some(keep)
}

/// `date +%Y%m%d-%H%M%S`, local time.
#[cfg(test)]
mod tests {
    use super::*;

    /// #478: a throwaway box whose up failed says why, or how up ended and what to run.
    #[test]
    fn a_failed_start_said() {
        let cmd = vec![b"hyprland".to_vec()];
        let t = |said, how: UpEnd, kept: Option<&[u8]>| {
            String::from_utf8(failed_start_text(said, &how, kept, &cmd)).unwrap()
        };
        assert_eq!(
            t(true, UpEnd::Status(1), Some(b"/s/fr/x")),
            "throwaway box failed to start: its up said why, above; its logs are kept in /s/fr/x (box.log first)"
        );
        assert_eq!(
            t(false, UpEnd::Signal(1), None),
            "throwaway box failed to start: its up was killed by signal 1 (HUP) before it said why: run and its box go with the shell or job that started them (a run left with & ends with its shell); a box that stays: omabox up, then omabox run -- hyprland (run -d for an app, to leave it running)"
        );
        assert!(
            t(false, UpEnd::Signal(9), None).starts_with(
                "throwaway box failed to start: its up was killed by signal 9 before it said why; its start, by hand: omabox up, then"
            )
        );
        assert!(
            t(false, UpEnd::Status(3), Some(b"/k")).starts_with(
                "throwaway box failed to start: its up ended with status 3 and said nothing; its start, by hand: omabox up, then omabox run -- hyprland"
            )
        );
        assert!(t(false, UpEnd::NotStarted, None).contains("could not be started"));
    }

    #[test]
    fn reserved() {
        assert!(env_reserved(b"PATH"));
        assert!(env_reserved(b"OMABOX_BOX"));
        assert!(!env_reserved(b"OMABOX_X"));
        assert!(!env_reserved(b"FOO"));
    }

    /// #471: run without -d says, once its command has run a while, that it waits, and what leaves
    /// an app running.
    #[test]
    fn a_long_run_says_it_waits() {
        let cmd: Vec<Vec<u8>> = vec![b"python3".to_vec(), b"hello world.py".to_vec()];
        assert_eq!(
            String::from_utf8(wait_note_text(b"b1", &cmd, false)).unwrap(),
            "note: python3 'hello world.py' still runs after 10 s: run waits for it to exit (an app, until its window is closed); omabox run -d -b b1 -- python3 'hello world.py' leaves it running and returns"
        );
        assert_eq!(
            String::from_utf8(wait_note_text(b"b1", &cmd, true)).unwrap(),
            "note: python3 'hello world.py' still runs after 10 s: run waits for it to exit (an app, until its window is closed); with no box up it runs in a box of its own; to leave an app running: omabox up, then omabox run -d -- python3 'hello world.py'; to see it once: omabox look app -- python3 'hello world.py'"
        );
    }

    #[test]
    fn replaces_every_one() {
        assert_eq!(replace_all(b"a/x/b/x/", b"/x/", b"/y/"), b"a/y/b/y/");
        assert_eq!(replace_all(b"abc", b"", b"z"), b"abc");
    }

    // t_failed_up's: past 10 kept dirs the oldest go, and only dirs a box name can have.
    #[test]
    fn keep_run_logs_keeps_ten() {
        let t = std::env::temp_dir().join(format!("omabox-fr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let (d, st) = (t.join("fr-box"), t.join("state"));
        let fr = st.join("omabox/failed-runs");
        std::fs::create_dir_all(d.join("home")).unwrap();
        std::fs::create_dir_all(fr.join(".x")).unwrap();
        std::fs::write(d.join("box.json"), "{}").unwrap();
        std::fs::write(d.join("box.log"), "log\n").unwrap();
        let at = |p: &std::path::Path, secs: i64| {
            let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).unwrap();
            let ts = libc::timespec {
                tv_sec: secs,
                tv_nsec: 0,
            };
            // SAFETY: a NUL-ended path of ours and two timespecs.
            assert_eq!(
                unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), [ts, ts].as_ptr(), 0) },
                0
            );
        };
        // (2026-01-01 on, a day apart; .x a year before)
        for i in 1..=11 {
            let o = fr.join(format!("old-{i:02}"));
            std::fs::create_dir(&o).unwrap();
            at(&o, 1_767_225_600 + (i - 1) * 86_400);
        }
        at(&fr.join(".x"), 1_735_689_600);
        let mut ctx = Ctx::new(b"run", Vec::new());
        ctx.name = b"new".to_vec();
        ctx.d = shell::bytes(&d).to_vec();
        let st_s = st.display().to_string();
        let kept = shell::with_env(&[("XDG_STATE_HOME", Some(&st_s))], || keep_run_logs(&ctx));
        let kept = kept.expect("kept");
        assert!(
            shell::path(&kept).join("box.log").is_file(),
            "its box.log copied"
        );
        let mut old: Vec<String> = std::fs::read_dir(&fr)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("old-"))
            .collect();
        old.sort();
        assert_eq!(
            old.join(" "),
            "old-03 old-04 old-05 old-06 old-07 old-08 old-09 old-10 old-11",
            "...the 10 newest kept dirs stay"
        );
        assert!(
            fr.join(".x").is_dir(),
            "...and a name no box has is left alone"
        );
        let _ = std::fs::remove_dir_all(&t);
    }
}
