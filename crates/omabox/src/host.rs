//! The user's own Hyprland, for what omabox does there on purpose (interactive windows, keys-to-box,
//! peek): its session (host_session), hyprctl on it, and the host side of interactive mode
//! (share/passthrough.lua), installed with hyprctl eval: runtime only, never in the user's config.
//!
//! This module is the only one that reaches that Hyprland (S8, #366): hyprctl on it and its Lua are
//! private here. Others read it through host_query (a fixed set of `-j` reads) and act on it only
//! through HostSilent, whose every Lua is built here from checked words: a window opened `silent`
//! on a workspace ws_check gave, without focus; a box's own window floated, sized or moved by its
//! address; the passthrough's hooks; the submap reset. A focus is HostAsked's, on the user's request.
//! t_unit_host_dispatch fails on a hyprctl run, a signature set or a HostAsked made anywhere else.

use crate::cat;
use crate::exit::{Ctx, Exit, Res, say};
use crate::jq::{self, Args};
use crate::shell;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileTypeExt;
use std::process::Stdio;

/// What toggles the passthrough, as share/passthrough.lua is told it.
pub const PASS_KEY: &str = "SUPER + ALT + ESCAPE";
/// The signature the instances call is made under (the agent guard's).
const GUARD: &str = "omabox-guard";

/// host_session: the user's Hyprland session (its signature): the environment's when it names a
/// live one, else the only one in the runtime dir; several and none named are refused (finding 65).
pub fn host_session(ctx: &Ctx) -> Result<Vec<u8>, Vec<u8>> {
    host_session_wl(ctx).map(|(sig, _)| sig)
}

/// host_session's signature and HOST_WL, its Wayland socket's path.
pub fn host_session_wl(ctx: &Ctx) -> Result<(Vec<u8>, Vec<u8>), Vec<u8>> {
    let rt = shell::env_or_empty("XDG_RUNTIME_DIR");
    let sig = shell::env_or_empty("HYPRLAND_INSTANCE_SIGNATURE");
    let mut c = crate::sig::command("hyprctl");
    c.args(["-j", "instances"])
        .env("HYPRLAND_INSTANCE_SIGNATURE", GUARD)
        .stdin(Stdio::inherit())
        .stderr(Stdio::null());
    // (`all=$(... hyprctl -j instances ...)`)
    let all = crate::sig::run_output(c)
        .ok()
        .filter(|(rc, _)| *rc == 0)
        .map(|(_, out)| shell::subst(&out))
        .filter(|a| jq::jq(&["-e", r#"type == "array""#], Some(a), jq::Err::Null).ok())
        .unwrap_or(b"[]".to_vec());
    let live = !sig.is_empty()
        && Args::new(&["-e"])
            .kv("--arg", "s", &sig)
            .arg("any(.[]; .instance == $s)")
            .run(Some(&all), jq::Err::Null)
            .ok();
    let mut sig = sig;
    if !live {
        // (`case $(jq length ...)`, `sig=$(jq ...)`: substitutions, as wl's below)
        match jq::jq(&["length"], Some(&all), jq::Err::Inherit)
            .text()
            .as_slice()
        {
            b"1" => {
                sig = jq::jq(&["-r", ".[0].instance"], Some(&all), jq::Err::Inherit).text();
            }
            b"0" => {
                return Err(cat!(
                    "no Hyprland session in ",
                    rt,
                    "/hypr: this needs the user's desktop"
                ));
            }
            _ => {
                let all_s = jq::jq(
                    &["-r", r#"map(.instance) | join(", ")"#],
                    Some(&all),
                    jq::Err::Inherit,
                )
                .text();
                return Err(cat!(
                    "several Hyprland sessions: set HYPRLAND_INSTANCE_SIGNATURE to one of ",
                    all_s
                ));
            }
        }
    }
    let wl = Args::new(&["-r"])
        .kv("--arg", "s", &sig)
        .arg("first(.[] | select(.instance == $s) | .wl_socket) // empty")
        .run(Some(&all), jq::Err::Inherit)
        .text();
    let sock = std::fs::metadata(shell::path(&cat!(rt, "/", wl)));
    if !(shell::re_match("^[A-Za-z0-9._-]+$", &wl) && sock.is_ok_and(|m| m.file_type().is_socket()))
    {
        return Err(cat!("Hyprland session ", sig, ": no Wayland socket found"));
    }
    let _ = ctx;
    Ok((sig, cat!(rt, "/", wl)))
}

/// `host_session` called by itself: its refusal ends the command (die).
pub fn host_session_die(ctx: &Ctx) -> Res<(Vec<u8>, Vec<u8>)> {
    host_session_wl(ctx).map_err(|m| ctx.die(m))
}

/// What host_query may read: `hyprctl -j` and one of these, nothing that dispatches.
pub enum Query {
    Clients,
    Monitors,
    ActiveWindow,
    /// `getoption NAME`, NAME a literal of omabox's.
    Option(&'static str),
}

/// `$(host_hyprctl -j Q)`: one of the host's reads (host_hyprctl's status and stdout; QUIET as there).
pub fn host_query(ctx: &Ctx, q: Query, quiet: bool) -> (i32, Vec<u8>) {
    let what: Vec<&[u8]> = match q {
        Query::Clients => vec![b"clients"],
        Query::Monitors => vec![b"monitors"],
        Query::ActiveWindow => vec![b"activewindow"],
        Query::Option(o) => vec![b"getoption", o.as_bytes()],
    };
    let mut args: Vec<&[u8]> = vec![b"-j"];
    args.extend(what);
    host_hyprctl(ctx, &args, quiet)
}

/// `$(host_hyprctl ARGS...)`: hyprctl on the user's session, its status and stdout (its stderr this
/// process's, or none with QUIET: `2>/dev/null`). host_session's refusal is said (unless QUIET) and
/// is the call's failure, status 1, as its die ends the `$(...)`.
fn host_hyprctl(ctx: &Ctx, args: &[&[u8]], quiet: bool) -> (i32, Vec<u8>) {
    let sig = match host_session(ctx) {
        Ok(s) => s,
        Err(m) => {
            if !quiet {
                say(m);
            }
            return (i32::from(ctx.die_rc.unwrap_or(1)), Vec::new());
        }
    };
    let mut c = crate::sig::command("hyprctl");
    c.args(args.iter().map(|a| OsStr::from_bytes(a)))
        .env("HYPRLAND_INSTANCE_SIGNATURE", OsStr::from_bytes(&sig))
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(if quiet {
            Stdio::null()
        } else {
            Stdio::inherit()
        });
    // (a foreground child, sig::run_child: its death by SIGINT read inside, sig::FG_INT)
    match crate::sig::run_output(c) {
        Ok(o) => o,
        Err(_) => {
            if !quiet {
                let _ = std::io::Write::write_all(
                    &mut std::io::stderr(),
                    b"omabox: cannot run hyprctl: not found in PATH\n",
                );
            }
            (127, Vec::new())
        }
    }
}

/// `$(host_hyprctl eval LUA 2>&1)` on the session SIG (found once by the caller): what hyprctl said,
/// both streams, without its trailing newlines.
fn host_eval_out(sig: &[u8], lua: &[u8]) -> Vec<u8> {
    let Ok((r, w)) = crate::exec::pipe() else {
        return Vec::new();
    };
    let Ok(w2) = w.try_clone() else {
        return Vec::new();
    };
    let mut c = crate::sig::command("hyprctl");
    c.arg("eval")
        .arg(OsStr::from_bytes(lua))
        .env("HYPRLAND_INSTANCE_SIGNATURE", OsStr::from_bytes(sig))
        .stdin(Stdio::null())
        .stdout(Stdio::from(w))
        .stderr(Stdio::from(w2));
    let child = c.spawn();
    drop(c);
    let out = crate::exec::read_all(r, None);
    if let Ok(mut ch) = child {
        let _ = ch.wait();
    }
    shell::strip_nl(&out)
}

/// host_eval LUA: Lua on the host Hyprland (`out=$(host_hyprctl eval "$1" 2>&1)`); its errors are
/// ours to report. host_session's refusal is said through it, as the words it got.
fn host_eval(ctx: &Ctx, lua: &[u8]) -> Res<()> {
    let out = match host_session(ctx) {
        Err(m) => cat!("omabox: ", m),
        Ok(sig) => {
            let mut c = crate::sig::command("hyprctl");
            c.arg("eval")
                .arg(OsStr::from_bytes(lua))
                .env("HYPRLAND_INSTANCE_SIGNATURE", OsStr::from_bytes(&sig))
                .stdin(Stdio::inherit());
            // (2>&1: one pipe for both, in the order said)
            match crate::exec::pipe() {
                Ok((r, w)) => {
                    let w2 = w.try_clone().ok();
                    c.stdout(Stdio::from(w));
                    if let Some(w2) = w2 {
                        c.stderr(Stdio::from(w2));
                    }
                    // (a foreground child, sig::run_child)
                    match crate::sig::run_child(c, false, |_| crate::exec::read_all(r, None)) {
                        Ok((got, _)) => shell::subst(&got),
                        Err(_) => b"cannot run hyprctl: not found in PATH".to_vec(),
                    }
                }
                Err(_) => Vec::new(),
            }
        }
    };
    if out != b"ok" {
        return Err(ctx.die(cat!("host hyprctl eval failed: ", out)));
    }
    Ok(())
}

/// lua_str V: V for a Lua string literal, or (said) nothing: no quote, backslash or newline.
pub fn lua_str(ctx: &Ctx, v: &[u8]) -> Vec<u8> {
    if v.iter().any(|&c| matches!(c, b'\'' | b'"' | b'\\' | b'\n')) {
        let _ = ctx;
        say(cat!(
            "unsafe characters in a path for the host Hyprland: ",
            v
        ));
        return Vec::new();
    }
    v.to_vec()
}

/// install_passthrough: share/passthrough.lua on the host, with the theme's colours read now (a
/// theme change reloads the config, so it is installed again with the new ones).
fn install_passthrough(ctx: &Ctx) -> Res<()> {
    let home = shell::env_or_empty("HOME");
    let mut colors = cat!(home, "/.local/state/omarchy/current/theme/colors.toml");
    if !shell::path(&colors).is_file() {
        colors = cat!(home, "/.config/omarchy/current/theme/colors.toml");
    }
    let script = std::fs::read(crate::root().join("share/passthrough.lua")).unwrap_or_default();
    let lua = cat!(
        "(function(...)\n",
        shell::strip_nl(&script),
        "\nend)('",
        lua_str(ctx, &ctx.boxes),
        "', '",
        PASS_KEY,
        "', '",
        lua_str(ctx, &colors),
        "')"
    );
    host_eval(ctx, &lua)
}

/// A window's address on the host as `hyprctl -j clients` gives it: 0x and hex (letters and digits,
/// as the suite's stand-ins give them; nothing that could end a Lua literal). How HostSilent names a
/// window: by address, so no focus moves.
pub struct Addr(Vec<u8>);

impl Addr {
    pub fn new(v: &[u8]) -> Option<Addr> {
        shell::re_match("^0x[0-9A-Za-z]{1,32}$", v).then(|| Addr(v.to_vec()))
    }
}

/// The host's Hyprland as omabox acts on it (#355, #366, S8): every Lua it is sent is built here,
/// from checked words. A window opened `silent` on a workspace ws_check gave, with no focus; a rule
/// that opens a box's monitor window the same way; a box's own window floated, sized or moved by
/// its address; the passthrough's hooks and its submap's reset. Nothing here can focus, switch,
/// close or open anything of the user's.
pub struct HostSilent<'a> {
    ctx: &'a Ctx,
}

impl<'a> HostSilent<'a> {
    pub fn new(ctx: &'a Ctx) -> HostSilent<'a> {
        HostSilent { ctx }
    }

    /// WS as ws_check gives it back, else refused.
    fn ws(&self, ws: &[u8]) -> Res<()> {
        if crate::deskwin::ws_check(ws).as_deref() != Some(ws) {
            return Err(self
                .ctx
                .die(cat!("not a workspace for a box's window: ", ws)));
        }
        Ok(())
    }

    /// LUA, built by one of the methods below, on the host (host_eval).
    #[cfg(not(test))]
    fn send(&self, lua: &[u8]) -> Res<()> {
        host_eval(self.ctx, lua)
    }

    /// (the unit tests': what would be sent, kept in tests::SENT)
    #[cfg(test)]
    fn send(&self, lua: &[u8]) -> Res<()> {
        tests::SENT.with(|s| s.borrow_mut().push(lua.to_vec()));
        Ok(())
    }

    /// `hl.exec_cmd('bash SCRIPT', { workspace = 'WS silent', no_initial_focus = true, ... })`:
    /// launched by the host so its exec rules apply to exactly this process's window (finding 70);
    /// tiled whatever another rule for aquamarine windows says (finding 136). WS must be one
    /// ws_check gives back as it is.
    pub fn exec_silent(&self, script: &[u8], ws: &[u8]) -> Res<()> {
        self.ws(ws)?;
        let lua = cat!(
            "hl.exec_cmd('bash ",
            lua_str(self.ctx, script),
            "', { workspace = '",
            ws,
            " silent', no_initial_focus = true, render_unfocused = true, float = false })"
        );
        self.send(&lua)
    }

    /// `hl.exec_cmd('CMD', { workspace = 'WS silent', no_initial_focus = true })`: peek's window,
    /// launched by the host so its exec rules put exactly it on WS, unfocused. CMD is the caller's
    /// (its paths lua_str'd), refused whole if anything in it would end the literal it goes in.
    pub fn exec_peek(&self, cmd: &[u8], ws: &[u8]) -> Res<()> {
        self.ws(ws)?;
        if cmd.iter().any(|&c| matches!(c, b'\'' | b'\\' | b'\n')) {
            return Err(self.ctx.die(cat!(
                "unsafe characters in a command for the host Hyprland: ",
                cmd
            )));
        }
        let lua = cat!(
            "hl.exec_cmd('",
            cmd,
            "', { workspace = '",
            ws,
            " silent', no_initial_focus = true })"
        );
        self.send(&lua)
    }

    /// The rule a box's monitor window opens under (deskwin, finding 429): its class
    /// aquamarine-omabox-TOK alone, `silent` on WS, with no focus, floated at RECT (W H X Y, its
    /// `move` relative to its monitor).
    pub fn mon_rule(&self, tok: &[u8], ws: &[u8], rect: [i64; 4]) -> Res<()> {
        self.ws(ws)?;
        if !shell::re_match("^[A-Za-z0-9_-]{1,64}$", tok) {
            return Err(self.ctx.die(cat!("not a window class token: ", tok)));
        }
        let [w, h, x, y] = rect.map(|v| v.to_string());
        let lua = cat!(
            "hl.window_rule({ name = 'omabox-mon-",
            tok,
            "', match = { initial_class = '^aquamarine-omabox-",
            tok,
            "$' }, workspace = '",
            ws,
            " silent', no_initial_focus = true, render_unfocused = true, float = true, size = '",
            w,
            " ",
            h,
            "', move = '",
            x,
            " ",
            y,
            "' })"
        );
        self.send(&lua)
    }

    /// A box's window floated (ON) or tiled again, by its address.
    pub fn float(&self, a: &Addr, on: bool) -> Res<()> {
        let lua = cat!(
            "hl.dispatch(hl.dsp.window.float({ action = '",
            if on { "enable" } else { "disable" },
            "', window = 'address:",
            a.0,
            "' }))"
        );
        self.send(&lua)
    }

    /// A box's window made W x H, by its address.
    pub fn resize(&self, a: &Addr, w: i64, h: i64) -> Res<()> {
        let lua = cat!(
            "hl.dispatch(hl.dsp.window.resize({ window = 'address:",
            a.0,
            "', x = ",
            w.to_string(),
            ", y = ",
            h.to_string(),
            " }))"
        );
        self.send(&lua)
    }

    /// A box's window moved to X,Y, by its address.
    pub fn move_to(&self, a: &Addr, x: i64, y: i64) -> Res<()> {
        let lua = cat!(
            "hl.dispatch(hl.dsp.window.move({ window = 'address:",
            a.0,
            "', x = ",
            x.to_string(),
            ", y = ",
            y.to_string(),
            " }))"
        );
        self.send(&lua)
    }

    /// share/passthrough.lua on the host (install_passthrough).
    pub fn passthrough(&self) -> Res<()> {
        install_passthrough(self.ctx)
    }

    /// The passthrough's keys applied again to the window focused now (keys-to-box).
    pub fn keys_changed(&self) -> Res<()> {
        self.send(b"omabox_keys_changed()")
    }

    /// The host out of the "omabox" submap, if it is in it (finding 134): quiet, its answer unread.
    pub fn submap_reset(&self) {
        let _ = host_hyprctl(self.ctx, &[b"eval", SUBMAP_LUA.as_bytes()], true);
    }

    /// Whether the session SIG lacks passthrough.lua's hooks of VERSION or later (a host config
    /// reload drops them, finding 118): a read, pass_check's.
    pub fn passthrough_missing(&self, sig: &[u8], version: u32) -> bool {
        host_eval_out(sig, pass_check(version).as_bytes()) == b"error: omabox-missing"
    }
}

/// submap_reset's Lua.
const SUBMAP_LUA: &str =
    r#"if hl.get_current_submap() == "omabox" then hl.dispatch(hl.dsp.submap("reset")) end"#;

/// passthrough_missing's Lua: an error of its own when the hooks on the host are older than VERSION.
pub fn pass_check(version: u32) -> String {
    format!("if (omabox_pass_version or 0) < {version} then error('omabox-missing', 0) end")
}

/// The user's real desktop, reached because the user asked for it: S8's exception. `omabox host --
/// CMD` (finding 67) hands the whole session over to that one command; `peek --focus` (the bar
/// widget's click) focuses the box's window there. Nothing else in omabox acts there unsilenced
/// (HostSilent); a HostAsked is made only by those two (t_unit_host_dispatch holds them to it).
pub struct HostAsked {
    sig: Vec<u8>,
    wl: Vec<u8>,
}

impl HostAsked {
    /// The session as omabox finds it (host_session), for what the user asked for there.
    pub fn user_asked(ctx: &Ctx) -> Res<HostAsked> {
        let (sig, wl) = host_session_die(ctx)?;
        Ok(HostAsked { sig, wl })
    }

    /// The window of PID focused (Hyprland switches to its workspace): peek --focus's.
    pub fn focus_pid(&self, ctx: &Ctx, pid: &[u8]) -> Res<()> {
        if !shell::is_num(pid) {
            return Err(ctx.die(cat!("not a pid to focus: ", pid)));
        }
        host_eval(
            ctx,
            &cat!("hl.dispatch(hl.dsp.focus({ window = 'pid:", pid, "' }))"),
        )
    }

    /// ARGV run in this process's place on the session (`exec "$@"`): DISPLAY and the Qt theme as
    /// the user manager has them (uwsm exports the session there), the caller's PATH without the
    /// guard's, Qt's logging as usual, the guard's browser stand-in (finding 92, any checkout's)
    /// given way to the user manager's value or none, cores as usual. Said first, every argument (a
    /// long one cut, #150), so the exception is on the record. Returns only when ARGV cannot run.
    pub fn exec(self, ctx: &Ctx, argv: &[Vec<u8>]) -> Res<()> {
        use std::os::unix::process::CommandExt;
        let Some(prog) = argv.first() else {
            return Err(ctx.die("host: nothing to run (omabox host -- CMD ...)"));
        };
        let sysenv = {
            let mut c = crate::sig::command("systemctl");
            c.args(["--user", "show-environment"])
                .stdin(Stdio::inherit())
                .stderr(Stdio::null());
            match crate::sig::run_output(c) {
                Ok((0, o)) => shell::subst(&o),
                _ => Vec::new(),
            }
        };
        let mut c = crate::sig::command(OsStr::from_bytes(prog));
        c.args(argv[1..].iter().map(|a| OsStr::from_bytes(a)));
        for v in ["DISPLAY", "QT_QPA_PLATFORMTHEME"] {
            let val = sysenv_value(&sysenv, v);
            let tame = !val.is_empty()
                && val
                    .iter()
                    .all(|&c| c.is_ascii_alphanumeric() || matches!(c, b':' | b'.' | b'_' | b'-'));
            if tame {
                c.env(v, OsStr::from_bytes(&val));
            } else {
                c.env_remove(v);
            }
        }
        let name = self.wl.rsplit(|&c| c == b'/').next().unwrap_or_default();
        c.env("WAYLAND_DISPLAY", OsStr::from_bytes(name))
            .env("HYPRLAND_INSTANCE_SIGNATURE", OsStr::from_bytes(&self.sig))
            .env("PATH", OsStr::from_bytes(&crate::exec::caller_path(false)))
            .env_remove("QT_FORCE_STDERR_LOGGING");
        // A BROWSER the guard did not set stays: Omarchy sets it in the shell, not the user
        // manager. systemctl prints a value with spaces or shell characters quoted, as $'...': not
        // taken as it is.
        for v in ["BROWSER", "GH_BROWSER"] {
            if !shell::env_or_empty(v).ends_with(b"/share/guard/xdg-open") {
                continue;
            }
            let val = sysenv_value(&sysenv, v);
            if val.is_empty() || val.ends_with(b"/share/guard/xdg-open") || val.starts_with(b"$'") {
                c.env_remove(v);
            } else {
                c.env(v, OsStr::from_bytes(&val));
            }
        }
        // (`ulimit -Sc "$(ulimit -Hc)"`)
        let mut lim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: lim is ours, a valid rlimit for both calls.
        unsafe {
            if libc::getrlimit(libc::RLIMIT_CORE, &mut lim) == 0 {
                lim.rlim_cur = lim.rlim_max;
                libc::setrlimit(libc::RLIMIT_CORE, &lim);
            }
        }
        say(cat!(
            "on your real desktop: ",
            shell::strip_nl(&host_record(argv))
        ));
        let err = c.exec();
        let (why, rc) = crate::exec::exec_failed(prog, &err);
        say(cat!("host: cannot run ", prog, ": ", why));
        Err(Exit(rc))
    }
}

/// `sed -n "s/^NAME=//p" <<<"$SYSENV"`: every value of NAME there, a line each.
fn sysenv_value(sysenv: &[u8], name: &str) -> Vec<u8> {
    let lead = cat!(name, "=");
    let vals: Vec<&[u8]> = sysenv
        .split(|&c| c == b'\n')
        .filter_map(|l| l.strip_prefix(lead.as_slice()))
        .collect();
    vals.join(&b'\n')
}

/// host_record ARGS: the line host prints, the record of what ran on the user's desktop (#150): every
/// argument, a long one cut to its start and how much is left out, so an expanded PATH does not
/// drown it. NAME=value past 60 characters of value, anything else past 120.
pub fn host_record(args: &[Vec<u8>]) -> Vec<u8> {
    let cut = |v: &[u8]| {
        cat!(
            shell::head_chars(v, 40),
            "…(+",
            (shell::char_count(v) - 40).to_string(),
            " chars)"
        )
    };
    let rec: Vec<Vec<u8>> = args
        .iter()
        .map(|a| {
            let named = a
                .first()
                .is_some_and(|&c| c.is_ascii_alphabetic() || c == b'_')
                && a.iter().position(|&c| c == b'=').is_some_and(|i| {
                    a[..i]
                        .iter()
                        .all(|&c| c.is_ascii_alphanumeric() || c == b'_')
                });
            if named {
                let i = a.iter().position(|&c| c == b'=').unwrap_or(0);
                let val = &a[i + 1..];
                if shell::char_count(val) <= 60 {
                    a.clone()
                } else {
                    cat!(&a[..=i], cut(val))
                }
            } else if shell::char_count(a) > 120 {
                cut(a)
            } else {
                a.clone()
            }
        })
        .collect();
    rec.join(&b' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        /// What HostSilent's send was given, in order (its cfg(test) form sends nothing).
        pub static SENT: std::cell::RefCell<Vec<Vec<u8>>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// What F sent through a HostSilent, as text each, and whether it was refused.
    fn sent(f: impl FnOnce(&HostSilent) -> Res<()>) -> (bool, Vec<String>) {
        SENT.with(|s| s.borrow_mut().clear());
        let ctx = Ctx::new(b"up", b"/x".to_vec());
        let ok = f(&HostSilent::new(&ctx)).is_ok();
        let got = SENT.with(|s| s.borrow_mut().drain(..).collect::<Vec<_>>());
        (
            ok,
            got.into_iter()
                .map(|l| String::from_utf8_lossy(&l).into_owned())
                .collect(),
        )
    }

    /// #366, S8: each of HostSilent's Lua, word for word; a window it opens is `silent` with no
    /// focus; nothing focuses; what would end a literal, a workspace ws_check would not give back as
    /// it is, an address or token not of their form: refused, nothing sent.
    #[test]
    fn host_silent_sends_only_its_own_lua() {
        let one = |f: &dyn Fn(&HostSilent) -> Res<()>| {
            let (ok, s) = sent(|h| f(h));
            assert!(ok, "refused: {s:?}");
            assert_eq!(s.len(), 1, "{s:?}");
            s[0].clone()
        };
        let refused = |f: &dyn Fn(&HostSilent) -> Res<()>| {
            let (ok, s) = sent(|h| f(h));
            assert!(!ok && s.is_empty(), "{ok} {s:?}");
        };
        let a = Addr::new(b"0x55d0aF").expect("an address");
        let mut all = vec![
            one(&|h| h.exec_silent(b"/b/launch.sh", b"9")),
            one(&|h| {
                h.exec_peek(
                    b"env A=1 /p --box /s --title \"omabox peek: b\"",
                    b"special:omabox",
                )
            }),
            one(&|h| h.mon_rule(b"0a1b", b"3", [800, 600, -10, 20])),
            one(&|h| h.float(&a, true)),
            one(&|h| h.float(&a, false)),
            one(&|h| h.resize(&a, 800, 600)),
            one(&|h| h.move_to(&a, -4, 30)),
            one(&|h| h.keys_changed()),
        ];
        assert_eq!(
            all[..3],
            [
                "hl.exec_cmd('bash /b/launch.sh', { workspace = '9 silent', no_initial_focus = true, render_unfocused = true, float = false })",
                "hl.exec_cmd('env A=1 /p --box /s --title \"omabox peek: b\"', { workspace = 'special:omabox silent', no_initial_focus = true })",
                "hl.window_rule({ name = 'omabox-mon-0a1b', match = { initial_class = '^aquamarine-omabox-0a1b$' }, workspace = '3 silent', no_initial_focus = true, render_unfocused = true, float = true, size = '800 600', move = '-10 20' })",
            ]
        );
        assert_eq!(
            all[3..],
            [
                "hl.dispatch(hl.dsp.window.float({ action = 'enable', window = 'address:0x55d0aF' }))",
                "hl.dispatch(hl.dsp.window.float({ action = 'disable', window = 'address:0x55d0aF' }))",
                "hl.dispatch(hl.dsp.window.resize({ window = 'address:0x55d0aF', x = 800, y = 600 }))",
                "hl.dispatch(hl.dsp.window.move({ window = 'address:0x55d0aF', x = -4, y = 30 }))",
                "omabox_keys_changed()",
            ]
        );
        all.push(SUBMAP_LUA.to_string());
        for l in &all {
            if l.contains("exec_cmd(") || l.contains("window_rule(") {
                assert!(l.contains(" silent', no_initial_focus = true"), "{l}");
            }
            assert!(!l.contains("dsp.focus"), "{l}");
        }
        for ws in [
            &b"special"[..],
            b"0",
            b"100",
            b"9 silent', x = '",
            b"",
            b"special:a b",
        ] {
            refused(&|h| h.exec_silent(b"/b/launch.sh", ws));
            refused(&|h| h.exec_peek(b"/p", ws));
            refused(&|h| h.mon_rule(b"0a1b", ws, [1, 1, 0, 0]));
        }
        for cmd in [&b"/p' }) hl.dispatch(x) --('"[..], b"/p\\", b"/p\nx"] {
            refused(&|h| h.exec_peek(cmd, b"9"));
        }
        for tok in [&b""[..], b"a'b", b"a b", b"a$"] {
            refused(&|h| h.mon_rule(tok, b"9", [1, 1, 0, 0]));
        }
        for v in [
            &b""[..],
            b"0x",
            b"55d0",
            b"0x55' }))",
            b"0x0123456789abcdef0123456789abcdef0",
            b"address:0x1",
            b"0x1 2",
        ] {
            assert!(Addr::new(v).is_none(), "{}", String::from_utf8_lossy(v));
        }
    }

    #[test]
    fn the_record_cuts_long_words() {
        let a = |v: &[&str]| v.iter().map(|x| x.as_bytes().to_vec()).collect::<Vec<_>>();
        assert_eq!(
            host_record(&a(&["printf", "%s", "A=b", "x"])),
            b"printf %s A=b x"
        );
        let long = "a".repeat(130);
        assert_eq!(
            String::from_utf8(host_record(&a(&["true", &long, "x"]))).unwrap_or_default(),
            format!("true {}…(+90 chars) x", "a".repeat(40))
        );
        let path = format!("PATH={}", "/opt/x:".repeat(10));
        assert_eq!(
            String::from_utf8(host_record(&a(&[&path]))).unwrap_or_default(),
            format!("PATH={}…(+30 chars)", &"/opt/x:".repeat(10)[..40])
        );
        // (a value of 60 characters stays whole; one word of 120 too)
        let v60 = format!("K={}", "v".repeat(60));
        assert_eq!(host_record(&a(&[&v60])), v60.as_bytes());
        assert_eq!(
            host_record(&a(&[&"w".repeat(120)])),
            "w".repeat(120).as_bytes()
        );
    }

    #[test]
    fn sysenv_values() {
        let e = b"DISPLAY=:1\nBROWSER=$'/a b'\nXDISPLAY=no\n";
        assert_eq!(sysenv_value(e, "DISPLAY"), b":1");
        assert_eq!(sysenv_value(e, "BROWSER"), b"$'/a b'");
        assert_eq!(sysenv_value(e, "NONE"), b"");
    }
}
