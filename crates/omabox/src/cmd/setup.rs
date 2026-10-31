//! `omabox setup` (issue #49, finding 126): what each user of this omabox needs, apart from what a
//! package or install.sh puts on the system: the links (the command, the agent skill, the bar
//! widget), the settings dir, the widget and the agent guard offered. Idempotent; --remove undoes it;
//! --feedback-skill opts in to the omabox-feedback skill (finding 326); --aquamarine is the optional
//! build (findings 125, 247).

use crate::args::unknown_opt;
use crate::cat;
use crate::exit::{Ctx, Exit, Res, muted, out, say};
use crate::install::{
    AQ_BUILD, AQ_COMMIT, AQ_OWN, Aq, aq_resolve, aq_resolve_env, elf_needed, hypr_aq_soname,
};
use crate::shell;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::process::Stdio;

/// Where the skill goes: everywhere Omarchy puts its own agent skills (omarchy-provision-user): the
/// shared ~/.agents/skills and Claude Code always; Codex, pi and Hermes when they are installed.
const SKILL_DIRS: &[&str] = &[".agents", ".claude", ".codex", ".pi/agent", ".hermes"];

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut aq, mut force, mut remove, mut fb) = (false, false, false, "");
    for a in args {
        match a.as_slice() {
            b"--aquamarine" => aq = true,
            b"--force" => force = true,
            b"--remove" => remove = true,
            b"--feedback-skill" => fb = "on",
            b"--no-feedback-skill" => fb = "off",
            b"-h" | b"--help" => return crate::help::usage("setup"),
            _ => return Err(unknown_opt(ctx, "setup", a)),
        }
    }
    if usize::from(aq) + usize::from(remove) + usize::from(!fb.is_empty()) > 1 {
        return Err(ctx
            .die("setup: one of --aquamarine, --remove, --feedback-skill or --no-feedback-skill"));
    }
    if force && !aq {
        return Err(ctx.die("setup: --force is for --aquamarine"));
    }
    ctx.parsed();
    let s = Setup {
        root: crate::paths::root(),
        home: shell::env_or_empty("HOME"),
    };
    if aq {
        s.aquamarine(ctx, force)
    } else if remove {
        s.remove(ctx)
    } else if !fb.is_empty() {
        s.feedback(ctx, fb)
    } else {
        s.user(ctx)
    }
}

struct Setup {
    root: Vec<u8>,
    home: Vec<u8>,
}

fn is_link(p: &[u8]) -> bool {
    std::fs::symlink_metadata(shell::path(p)).is_ok_and(|m| m.file_type().is_symlink())
}

fn exists(p: &[u8]) -> bool {
    shell::path(p).exists()
}

fn readlink(p: &[u8]) -> Option<Vec<u8>> {
    std::fs::read_link(shell::path(p))
        .ok()
        .map(|t| shell::bytes(&t).to_vec())
}

/// `[ -t 0 ] && [ -t 1 ]`.
fn terminal() -> bool {
    crate::exec::isatty(0) && crate::exec::isatty(1)
}

/// Whether a prompt met the end of its input (Ctrl-D): taken as no, never remembered as one.
static INPUT_ENDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn input_ended() -> bool {
    INPUT_ENDED.load(std::sync::atomic::Ordering::Relaxed)
}

/// `read -r -p PROMPT yn`, then whether the answer is a no (`[[ $yn = [nN]* ]]`). At the end of
/// input (Ctrl-D) a no, said once (#373: bash's set -e ended setup there, 1, without a word).
fn ask_no(prompt: &str) -> Res<bool> {
    let _ = std::io::Write::write_all(&mut std::io::stderr(), prompt.as_bytes());
    let mut line = Vec::new();
    let mut b = [0u8; 1];
    loop {
        // SAFETY: b is ours, one byte; fd 0 is the terminal's.
        let n = unsafe { libc::read(0, b.as_mut_ptr().cast(), 1) };
        if n == 0 {
            if !INPUT_ENDED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                let _ = std::io::Write::write_all(
                    &mut std::io::stderr(),
                    b"\n(end of input: no, to this and any question after it)\n",
                );
            }
            return Ok(true);
        }
        if n < 0 {
            return Err(Exit(1));
        }
        if b[0] == b'\n' {
            break;
        }
        line.push(b[0]);
    }
    Ok(answer_no(&line))
}

/// `read -r yn` then `[[ $yn = [nN]* ]]`: the answer is a no when it starts with n or N once read
/// has taken the blanks (space, tab) off its start, as bash's default IFS does: " n" is a no
/// (finding 482).
fn answer_no(line: &[u8]) -> bool {
    let start = line
        .iter()
        .position(|&c| c != b' ' && c != b'\t')
        .unwrap_or(line.len());
    matches!(line.get(start), Some(b'n' | b'N'))
}

/// `PROGRAM ARGS...` with this process's stdio, its status (127 when it cannot run).
fn status(prog: &[u8], args: &[&[u8]]) -> i32 {
    let mut c = crate::sig::command(OsStr::from_bytes(prog));
    c.args(args.iter().map(|a| OsStr::from_bytes(a)));
    crate::sig::run_status(c).unwrap_or(127)
}

/// `command -v NAME >/dev/null`.
fn have(name: &[u8]) -> bool {
    crate::exec::executable_on_path(name)
}

/// Whether the running shell lists the bar widget (`omarchy-shell shell listPlugins`); None when no
/// shell answers (none running, or no omarchy-shell).
fn shell_lists_widget() -> Option<bool> {
    let mut c = crate::sig::command("omarchy-shell");
    c.args(["shell", "listPlugins"]).stderr(Stdio::null());
    match crate::sig::run_output(c) {
        Ok((0, o)) => Some(crate::exit::contains(&o, b"\"chaves.omabox\"")),
        _ => None,
    }
}

/// Finding 539 (#397): the running shell made to know the widget setup just linked, as `omarchy
/// plugin add` does. Its plugins dir watcher reloads on the link, but some 0.3 s later (an Omarchy
/// 4.0.6 VM), and a box's shell has none (finding 196): `omarchy plugin enable` run in that time
/// says "is not known". A shell that does not list it yet is asked to rescan (Omarchy 4.0.0 to 4.5
/// all have `rescanPlugins` and `-q`), then listed again until it does, up to 4 s; one that lists it
/// already is left alone (a rescan reloads every plugin). Some(false): a shell that still does not.
fn shell_knows_widget() -> Option<bool> {
    if shell_lists_widget()? {
        return Some(true);
    }
    let mut c = crate::sig::command("omarchy-shell");
    c.args(["-q", "shell", "rescanPlugins"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _ = crate::sig::run_status(c);
    for _ in 0..20 {
        if shell_lists_widget()? {
            return Some(true);
        }
        crate::sig::sleep(std::time::Duration::from_millis(200));
    }
    Some(false)
}

impl Setup {
    fn omabox(&self) -> Vec<u8> {
        cat!(self.root, "/bin/omabox")
    }

    fn in_home(&self, rel: &str) -> Vec<u8> {
        cat!(self.home, "/", rel)
    }

    fn system_install(&self) -> bool {
        self.root.starts_with(b"/usr/")
    }

    /// setup_link TARGET LINK: a link of ours, made where nothing is or a link was (ln -sfn onto a
    /// real directory would put the link inside it and leave the old copy in use).
    fn link(&self, ctx: &Ctx, target: &[u8], l: &[u8]) -> Res<()> {
        if !is_link(l) && exists(l) {
            return Err(ctx.die(cat!(
                l,
                " exists and is not a link: move it away, then re-run"
            )));
        }
        if let Some(d) = shell::path(l).parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if is_link(l) {
            let _ = std::fs::remove_file(shell::path(l));
        }
        std::os::unix::fs::symlink(shell::path(target), shell::path(l))
            .map_err(|e| ctx.die(cat!("cannot link ", l, ": ", crate::shots::why(&e))))
    }

    /// setup_unlink LINK TARGET: removed only while it still points here.
    fn unlink(&self, l: &[u8], target: &[u8]) -> Res<()> {
        if is_link(l) && readlink(l).as_deref() == Some(target) {
            let _ = std::fs::remove_file(shell::path(l));
            out(cat!("removed ", l, "\n"))?;
        }
        Ok(())
    }

    /// setup_feedback on|off|refresh (finding 326): the omabox-feedback skill linked where the
    /// omabox skill goes (on), unlinked (off), or a link of an omabox's skill-feedback/ already there
    /// pointed here (refresh: a plain setup), none made.
    fn feedback(&self, ctx: &Ctx, mode: &str) -> Res<()> {
        let target = cat!(self.root, "/skill-feedback");
        let mut linked: Vec<Vec<u8>> = Vec::new();
        for dir in SKILL_DIRS {
            let l = self.in_home(&format!("{dir}/skills/omabox-feedback"));
            match mode {
                "on" => {
                    if !matches!(*dir, ".agents" | ".claude")
                        && !shell::path(&self.in_home(dir)).is_dir()
                    {
                        continue;
                    }
                }
                "off" => {
                    self.unlink(&l, &target)?;
                    continue;
                }
                _ => {
                    let ours = is_link(&l)
                        && readlink(&l).is_some_and(|t| shell::basename(&t) == b"skill-feedback");
                    if !ours {
                        continue;
                    }
                }
            }
            self.link(ctx, &target, &l)?;
            linked.push(cat!("~/", dir));
        }
        if !linked.is_empty() {
            out(cat!(
                "agent skill 'omabox-feedback' (opted in) -> ",
                linked.join(&b' '),
                " (skills/)\n"
            ))?;
        }
        if mode == "on" {
            out(
                "it runs only when asked (/omabox-feedback) and asks before posting; omabox setup --no-feedback-skill takes it out\n",
            )?;
        }
        Ok(())
    }

    fn user(&self, ctx: &Ctx) -> Res<()> {
        // A system install (a package: /usr/bin/omabox is the command) needs no link of its own; one
        // left from a checkout is said, and offered to go (finding 133).
        if self.system_install() {
            self.old_link()?;
        } else {
            self.link(ctx, &self.omabox(), &self.in_home(".local/bin/omabox"))?;
            out(cat!(self.home, "/.local/bin/omabox -> bin/omabox\n"))?;
        }
        let mut linked: Vec<Vec<u8>> = Vec::new();
        for dir in SKILL_DIRS {
            if !matches!(*dir, ".agents" | ".claude") && !shell::path(&self.in_home(dir)).is_dir() {
                continue;
            }
            self.link(
                ctx,
                &cat!(self.root, "/skill"),
                &self.in_home(&format!("{dir}/skills/omabox")),
            )?;
            linked.push(cat!("~/", dir));
        }
        out(cat!(
            "agent skill 'omabox' -> ",
            linked.join(&b' '),
            " (skills/)\n"
        ))?;
        self.feedback(ctx, "refresh")?;
        // The bar widget: only ~/.config/omarchy/plugins holds third-party plugins (finding 126).
        self.link(
            ctx,
            &cat!(self.root, "/plugin"),
            &self.in_home(".config/omarchy/plugins/chaves.omabox"),
        )?;
        out("bar widget chaves.omabox -> ~/.config/omarchy/plugins\n")?;
        // Where `omabox config` keeps the settings. The widget watches the file, and a watch only sees
        // the file appear if its directory already exists (finding 72).
        let _ = std::fs::create_dir_all(shell::path(&self.in_home(".config/omabox")));
        if !self.system_install() {
            let p = cat!(":", shell::env_or_empty("PATH"), ":");
            if !crate::exit::contains(&p, &cat!(":", self.home, "/.local/bin:")) {
                out("note: add ~/.local/bin to PATH\n")?;
            }
        }
        self.widget()?;
        self.guard()?;
        let aq = muted(|| hypr_aq_soname(ctx).ok().and_then(|w| aq_resolve(ctx, &w)));
        if let Some(aq) = aq {
            aq_note(&aq)?;
        }
        Ok(())
    }

    /// setup_widget (finding 133): the bar widget in the user's bar, asked in a terminal, yes by
    /// default, a "no" remembered. As `omarchy plugin add` does: the shell rescans its plugins dir
    /// first, and enabling waits for it to know the widget. Without a terminal the line it prints
    /// works at once (finding 539): the same rescan, and a wait until the shell lists the widget.
    fn widget(&self) -> Res<()> {
        let shell_json = std::fs::read(shell::path(&self.in_home(".config/omarchy/shell.json")))
            .unwrap_or_default();
        if crate::exit::contains(&shell_json, b"\"chaves.omabox\"") {
            return out("bar widget: on in your bar\n");
        }
        if !have(b"omarchy") {
            return Ok(());
        }
        let declined = self.in_home(".config/omabox/widget-declined");
        if exists(&declined) || is_link(&declined) {
            return out(cat!(
                "bar widget: not asked (you said no before; rm ",
                declined,
                " to be asked again): omarchy plugin enable chaves.omabox\n"
            ));
        }
        if !terminal() {
            // A shell still blind to the link after the wait: the line names the rescan too.
            let rescan = if shell_knows_widget() == Some(false) {
                "omarchy-shell shell rescanPlugins, then "
            } else {
                ""
            };
            return out(cat!(
                "bar widget: not asked (no terminal): ",
                rescan,
                "omarchy plugin enable chaves.omabox\n"
            ));
        }
        if ask_no("Show the omabox widget in your bar? [Y/n] ")? {
            // (a Ctrl-D is no answer to keep: asked again next time)
            if !input_ended() {
                remember(&declined);
            }
            return Ok(());
        }
        shell_knows_widget();
        let mut said = Vec::new();
        for _ in 0..20 {
            let mut c = crate::sig::command("omarchy");
            c.args(["plugin", "enable", "chaves.omabox"]);
            // (`out=$(... 2>&1)`)
            let (rc, o) = match crate::exec::pipe() {
                Ok((r, w)) => {
                    let w2 = w.try_clone().ok();
                    c.stdout(Stdio::from(w));
                    if let Some(w2) = w2 {
                        c.stderr(Stdio::from(w2));
                    }
                    match crate::sig::run_child(c, false, |_| crate::exec::read_all(r, None)) {
                        Ok((o, (rc, _))) => (rc, shell::subst(&o)),
                        Err(_) => (127, Vec::new()),
                    }
                }
                Err(_) => (127, Vec::new()),
            };
            said = o;
            if rc == 0 {
                return out(cat!(said, "\n"));
            }
            if !crate::exit::contains(&said, b"is not known") {
                break;
            }
            crate::sig::sleep(std::time::Duration::from_millis(200));
        }
        if said.is_empty() {
            said = b"omarchy plugin enable failed".to_vec();
        }
        out(cat!(
            said,
            "\nthe widget is not on (see above): omarchy plugin enable chaves.omabox; the rest of the setup goes on\n"
        ))
    }

    /// setup_old_link (finding 133): ~/.local/bin/omabox from a checkout, met by a system install's
    /// setup. A link is offered to go, in a terminal, yes by default; anything else is only said.
    fn old_link(&self) -> Res<()> {
        let l = self.in_home(".local/bin/omabox");
        let link = is_link(&l);
        if !link && !exists(&l) {
            return Ok(());
        }
        let mut to = Vec::new();
        if link {
            to = crate::repo::readlink_f(&l)
                .or_else(|| readlink(&l))
                .unwrap_or_default();
            if crate::repo::readlink_f(&self.omabox()).is_some_and(|me| me == to) {
                return Ok(());
            }
        }
        out("\n==> An omabox in ~/.local/bin\n")?;
        if !link {
            out(cat!(
                l,
                " is a file of its own, not this omabox (",
                self.root,
                ").\n"
            ))?;
        } else if !exists(&l) {
            out(cat!(
                l,
                " links to ",
                readlink(&l).unwrap_or_default(),
                ", which is gone.\n"
            ))?;
        } else {
            out(cat!(
                l,
                " links to ",
                to,
                ", not this omabox (",
                self.root,
                ").\n"
            ))?;
        }
        if command_v(b"omabox").as_deref() == Some(l.as_slice()) {
            out(
                "It comes first on your PATH: `omabox` (the bar widget's too) runs it, not this one.\n",
            )?;
        } else {
            out(
                "Your PATH finds this one first, but a PATH with ~/.local/bin first would run that one.\n",
            )?;
        }
        if !link {
            return Ok(());
        }
        if !terminal() {
            return out(cat!("not asked (no terminal): rm ", l, "\n"));
        }
        if ask_no("Remove the link? [Y/n] ")? {
            out(cat!("kept ", l, "\n"))
        } else {
            let _ = std::fs::remove_file(shell::path(&l));
            out(cat!("removed ", l, "\n"))
        }
    }

    /// setup_guard (findings 65-67, 74): the agent guard changes the agents' own settings, so it is
    /// only ever asked for, in a terminal, yes by default; a "no" is remembered. An outdated guard is
    /// offered an update whatever was said to turning it on.
    fn guard(&self) -> Res<()> {
        let claude = shell::env("CLAUDE_CONFIG_DIR")
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| self.in_home(".claude"));
        let codex = shell::env("CODEX_HOME")
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| self.in_home(".codex"));
        if !shell::path(&claude).is_dir() && !shell::path(&codex).is_dir() {
            return Ok(());
        }
        out("\n==> Agent guard\n")?;
        let state = self.omabox_out(&[b"guard"], false);
        let states: Vec<&[u8]> = state
            .split(|&c| c == b'\n')
            .filter(|l| {
                [b"Claude Code".as_slice(), b"Codex", b"claude", b"codex"]
                    .iter()
                    .any(|p| l.starts_with(p))
            })
            .collect();
        out(cat!(states.join(&b'\n'), "\n"))?;
        let agent = |l: &[u8]| -> Option<&'static str> {
            let after = |p: &[u8]| {
                l.strip_prefix(p)
                    .is_some_and(|r| matches!(r.first(), Some(b' ' | b':')))
            };
            if after(b"Claude Code") || after(b"claude") {
                Some("claude")
            } else if after(b"Codex") || after(b"codex") {
                Some("codex")
            } else {
                None
            }
        };
        let outdated = |l: &[u8]| crate::exit::contains(l, b": outdated");
        let need_update: Vec<&str> = states
            .iter()
            .filter(|l| outdated(l))
            .filter_map(|l| agent(l))
            .collect();
        let need_on: Vec<&str> = states
            .iter()
            .filter(|l| !l.ends_with(b": on") && !outdated(l))
            .filter_map(|l| agent(l))
            .collect();
        // (which_on: one agent named, or none for every one)
        let which = |v: &[&'static str]| -> &'static str { if v.len() == 1 { v[0] } else { "" } };
        let sp = |w: &str| {
            if w.is_empty() {
                String::new()
            } else {
                format!(" {w}")
            }
        };
        if !need_update.is_empty() {
            let w = which(&need_update);
            out("The guard there is outdated (omabox guard shows what it does).\n")?;
            if terminal() {
                if !ask_no("Update it? [Y/n] ")? && !self.guard_on(w) {
                    out(
                        "the guard is not (fully) updated, see above; the rest of the setup is done\n",
                    )?;
                }
            } else {
                out(format!(
                    "not asked (no terminal): omabox guard on{}\n",
                    sp(w)
                ))?;
            }
        }
        if !need_on.is_empty() {
            let w = which(&need_on);
            out(
                "It gives your agents' shell commands a display that does not exist, so a window, hyprctl or\ngrim outside a box fails instead of reaching your desktop. omabox keeps working, and\n`omabox host -- CMD` runs what you ask for on the real desktop. See: omabox guard\n",
            )?;
            let declined = self.in_home(".config/omabox/guard-declined");
            if exists(&declined) || is_link(&declined) {
                out(cat!(
                    "not asked (you said no before; rm ",
                    declined,
                    " to be asked again): omabox guard on",
                    sp(w),
                    "\n"
                ))?;
            } else if terminal() {
                if ask_no("Turn it on? [Y/n] ")? {
                    remember(&declined);
                } else if !self.guard_on(w) {
                    out("the guard is not (fully) on, see above; the rest of the setup is done\n")?;
                }
            } else {
                out(format!(
                    "not asked (no terminal): omabox guard on{}\n",
                    sp(w)
                ))?;
            }
        }
        Ok(())
    }

    /// `"$ROOT/bin/omabox" guard on [AGENT]`: whether it went.
    fn guard_on(&self, w: &str) -> bool {
        let mut a: Vec<&[u8]> = vec![b"guard", b"on"];
        if !w.is_empty() {
            a.push(w.as_bytes());
        }
        status(&self.omabox(), &a) == 0
    }

    /// `$("$ROOT/bin/omabox" ARGS) || true` (QUIET: `2>/dev/null` too).
    fn omabox_out(&self, args: &[&[u8]], quiet: bool) -> Vec<u8> {
        let mut c = crate::sig::command(OsStr::from_bytes(&self.omabox()));
        c.args(args.iter().map(|a| OsStr::from_bytes(a)));
        if quiet {
            c.stderr(Stdio::null());
        }
        crate::sig::run_output(c)
            .map(|(_, o)| shell::subst(&o))
            .unwrap_or_default()
    }

    /// setup --remove: what setup and setup --aquamarine made, undone: before a package is removed,
    /// or a checkout deleted. Only links that still point here; the guard and the broker turned off
    /// when this HOME has them; the widget disabled when this HOME's shell.json has it. Settings and
    /// saves are the user's: kept, said so.
    fn remove(&self, ctx: &Ctx) -> Res<()> {
        let state = self.omabox_out(&[b"guard"], true);
        let guarded = state.split(|&c| c == b'\n').any(|l| {
            [b"Claude Code".as_slice(), b"Codex", b"claude", b"codex"]
                .iter()
                .any(|p| l.starts_with(p))
                && (crate::exit::contains(l, b": on") || crate::exit::contains(l, b": outdated"))
        });
        if guarded && status(&self.omabox(), &[b"guard", b"off"]) != 0 {
            say("the guard is not (fully) off, see above");
        }
        if exists(&self.in_home(".config/systemd/user/omabox-broker.socket"))
            && status(&self.omabox(), &[b"broker", b"off"]) != 0
        {
            say("the broker is not off, see above");
        }
        let shell_json = std::fs::read(shell::path(&self.in_home(".config/omarchy/shell.json")))
            .unwrap_or_default();
        if crate::exit::contains(&shell_json, b"\"chaves.omabox\"")
            && status(b"omarchy", &[b"plugin", b"disable", b"chaves.omabox"]) != 0
        {
            say("could not disable the bar widget: omarchy plugin disable chaves.omabox");
        }
        self.unlink(
            &self.in_home(".config/omarchy/plugins/chaves.omabox"),
            &cat!(self.root, "/plugin"),
        )?;
        for dir in SKILL_DIRS {
            self.unlink(
                &self.in_home(&format!("{dir}/skills/omabox")),
                &cat!(self.root, "/skill"),
            )?;
        }
        self.feedback(ctx, "off")?;
        self.unlink(&self.in_home(".local/bin/omabox"), &self.omabox())?;
        let aq_user = crate::paths::aq_user();
        let cache = cat!(
            shell::env("XDG_CACHE_HOME")
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| self.in_home(".cache")),
            "/omabox-aquamarine"
        );
        let there = |p: &[u8]| exists(p) || is_link(p);
        if there(&aq_user) || there(&cache) {
            rm_rf(&aq_user);
            rm_rf(&cache);
            out(cat!("removed the aquamarine build in ", aq_user, "\n"))?;
        }
        // The dirs boxes' homes and the build lived in, once nothing is left in them (a box's home
        // stays while it is up; saves stay, said below).
        let _ = std::fs::remove_dir(shell::path(&crate::paths::homes()));
        if let Some(i) = aq_user.iter().rposition(|&c| c == b'/') {
            let _ = std::fs::remove_dir(shell::path(&aq_user[..i]));
        }
        if there(&cat!(self.root, "/.git")) {
            out(
                "kept: this checkout's build/ (the aquamarine build, if any) goes with the checkout\n",
            )?;
        }
        if !crate::boxes::list_names(ctx, false).is_empty() {
            out("boxes are still up (omabox ls): omabox down --all\n")?;
        }
        let baks = super::guard::settings_backups();
        if !baks.is_empty() {
            out(cat!(
                "kept: the guard's backups of your agents' settings: ",
                baks.join(&b' '),
                "\n"
            ))?;
        }
        out(
            "kept: your settings (~/.config/omabox) and saves (~/.local/share/omabox/saves); delete them yourself if you want them gone\n",
        )
    }

    /// setup --aquamarine (issues #47, #49): the fix built where aq_resolve looks for it, with
    /// omabox's own patches. A no-op only while boxes use this omabox's build (AQ_BUILD), unless
    /// --force: a system aquamarine with #415 still lacks omabox's patches (#186, finding 247).
    fn aquamarine(&self, ctx: &Ctx, force: bool) -> Res<()> {
        let want = hypr_aq_soname(ctx)?;
        let aq = aq_resolve_env(ctx, &want, b"").ok_or(Exit(1))?;
        if !crate::install::aq_build_needed(&aq, force) {
            return Ok(());
        }
        let missing: Vec<&str> = [
            "git",
            "cmake",
            "ninja",
            "hyprwayland-scanner",
            "pkg-config",
            "c++",
        ]
        .into_iter()
        .filter(|t| !have(t.as_bytes()))
        .collect();
        if !missing.is_empty() {
            return Err(ctx.die(format!(
                "setup --aquamarine needs {}: {}",
                missing.join(" "),
                super::doctor::install_here(&["aquamarine-build"]).join("; ")
            )));
        }
        // A checkout builds into its own build/ (as install.sh always did); anything else into the
        // user's data dir, its source in the cache (not in HOMES: sweep_homes would take it for a
        // box's).
        let (prefix, src) =
            if exists(&cat!(self.root, "/.git")) || is_link(&cat!(self.root, "/.git")) {
                (
                    cat!(self.root, "/build/prefix"),
                    cat!(self.root, "/build/aquamarine"),
                )
            } else {
                (
                    crate::paths::aq_user(),
                    cat!(
                        shell::env("XDG_CACHE_HOME")
                            .filter(|v| !v.is_empty())
                            .unwrap_or_else(|| self.in_home(".cache")),
                        "/omabox-aquamarine"
                    ),
                )
            };
        say(cat!(
            "building aquamarine ",
            AQ_BUILD,
            " (hyprwm/aquamarine#415, keys held when focus leaves, the pointer and cursor in a monitor's window, a class for each monitor's window) into ",
            prefix
        ));
        let git = |args: &[&[u8]], err_null: bool| {
            let mut c = crate::sig::command("git");
            c.args(args.iter().map(|a| OsStr::from_bytes(a)));
            if err_null {
                c.stderr(Stdio::null());
            }
            crate::sig::run_status(c).unwrap_or(127)
        };
        if !shell::path(&cat!(src, "/.git")).is_dir()
            && git(
                &[
                    b"clone",
                    b"-q",
                    b"https://github.com/hyprwm/aquamarine",
                    &src,
                ],
                false,
            ) != 0
        {
            return Err(ctx.die(cat!("could not clone aquamarine into ", src)));
        }
        if git(
            &[
                b"-C",
                &src,
                b"cat-file",
                b"-e",
                format!("{AQ_COMMIT}^{{commit}}").as_bytes(),
            ],
            true,
        ) != 0
            && git(&[b"-C", &src, b"fetch", b"-q", b"origin"], false) != 0
        {
            return Err(ctx.die(cat!("could not fetch aquamarine in ", src)));
        }
        if git(
            &[b"-C", &src, b"checkout", b"-q", b"-f", AQ_COMMIT.as_bytes()],
            false,
        ) != 0
        {
            return Err(ctx.die(cat!(
                "could not check out aquamarine ",
                AQ_COMMIT,
                " in ",
                src
            )));
        }
        for patch in shell::glob_ext(&cat!(self.root, "/patches/aquamarine"), b".patch") {
            if git(&[b"-C", &src, b"apply", &patch], false) != 0 {
                return Err(ctx.die(cat!(
                    "could not apply ",
                    patch,
                    " to aquamarine ",
                    AQ_COMMIT,
                    " in ",
                    src
                )));
            }
        }
        // Installed next to the old one, then swapped: a failed build leaves the old one in use. (A
        // running box keeps the files it loaded.)
        let new = cat!(prefix, ".new");
        rm_rf(&new);
        let cmake = |args: &[&[u8]], destdir: Option<&[u8]>, quiet: bool| {
            let mut c = crate::sig::command("cmake");
            c.args(args.iter().map(|a| OsStr::from_bytes(a)));
            if let Some(d) = destdir {
                c.env("DESTDIR", OsStr::from_bytes(d));
            }
            if quiet {
                c.stdout(Stdio::null());
            }
            crate::sig::run_status(c).unwrap_or(127) == 0
        };
        let out_dir = cat!(src, "/out");
        let built = cmake(
            &[
                b"-S",
                &src,
                b"-B",
                &out_dir,
                b"-G",
                b"Ninja",
                b"-DCMAKE_BUILD_TYPE=Release",
                &cat!("-DCMAKE_INSTALL_PREFIX=", prefix),
            ],
            None,
            true,
        ) && cmake(&[b"--build", &out_dir], None, false)
            && cmake(&[b"--install", &out_dir], Some(&new), true);
        if !built {
            return Err(ctx.die("building aquamarine failed (above)"));
        }
        let staged = cat!(new, prefix);
        if !exists(&cat!(staged, "/lib/", want)) {
            let has = elf_needed(&cat!(staged, "/lib/libaquamarine.so"), "SONAME")
                .map(|v| v.join(&b'\n'))
                .unwrap_or(b"no libaquamarine".to_vec());
            return Err(ctx.die(cat!(
                "aquamarine ",
                AQ_COMMIT,
                " builds ",
                has,
                ", but Hyprland links ",
                want,
                ": omabox needs a newer AQ_COMMIT (left in ",
                new,
                ")"
            )));
        }
        if let Err(e) = std::fs::write(
            shell::path(&cat!(staged, "/.omabox-commit")),
            cat!(AQ_BUILD, "\n"),
        ) {
            return Err(ctx.die(crate::shots::cant_write(
                &cat!(staged, "/.omabox-commit"),
                &e,
            )));
        }
        let old = cat!(prefix, ".old");
        rm_rf(&old);
        if exists(&prefix) || is_link(&prefix) {
            std::fs::rename(shell::path(&prefix), shell::path(&old))
                .map_err(|e| ctx.die(cat!("cannot move ", prefix, ": ", crate::shots::why(&e))))?;
        }
        std::fs::rename(shell::path(&staged), shell::path(&prefix))
            .map_err(|e| ctx.die(cat!("cannot move ", staged, ": ", crate::shots::why(&e))))?;
        rm_rf(&new);
        rm_rf(&old);
        let aq = aq_resolve(ctx, &want).ok_or(Exit(1))?;
        say(cat!("boxes now use ", aq.desc()));
        Ok(())
    }
}

/// aq_note: what the boxes' aquamarine lacks that `omabox setup --aquamarine` builds, said.
pub fn aq_note(aq: &Aq) -> Res<()> {
    match aq_note_text(aq) {
        Some(t) => out(cat!("note: ", t, "\n")),
        None => Ok(()),
    }
}

/// What the boxes' aquamarine lacks that `omabox setup --aquamarine` builds: #415, and omabox's own
/// patches, which only this omabox's build has (finding 247), ending with that command (setup's
/// note, doctor's warning). None for a current build.
pub fn aq_note_text(aq: &Aq) -> Option<Vec<u8>> {
    if aq.kind == b"private" {
        // Built by an older omabox (finding 132): it lacks a patch omabox carries now.
        let b = aq
            .ver
            .iter()
            .position(|&c| c == b'@')
            .map_or(aq.ver.as_slice(), |i| &aq.ver[i + 1..]);
        if b == AQ_BUILD.as_bytes() {
            return None;
        }
        let lib = aq.lib.strip_suffix(b"/lib").unwrap_or(&aq.lib);
        Some(cat!(
            "the aquamarine build in ",
            lib,
            " is older than this omabox's (it lacks a fix omabox carries now: ",
            AQ_OWN,
            "): omabox setup --aquamarine"
        ))
    } else if !aq.fixed {
        Some(cat!(
            "headless boxes on NVIDIA and confirm-close need aquamarine's fix for nested Wayland outputs, and interactive boxes omabox's own fixes (",
            AQ_OWN,
            "): omabox setup --aquamarine"
        ))
    } else {
        Some(cat!(
            "the system's aquamarine has the fix for nested Wayland outputs, but not omabox's own fixes (",
            AQ_OWN,
            "): omabox setup --aquamarine"
        ))
    }
}

/// A "no" kept: `mkdir -p "$(dirname F)" && date -Is > F`.
fn remember(f: &[u8]) {
    if let Some(d) = shell::path(f).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(shell::path(f), cat!(shell::date_iso(), "\n"));
}

/// `rm -rf P`.
fn rm_rf(p: &[u8]) {
    let path = shell::path(p);
    if is_link(p) || path.is_file() {
        let _ = std::fs::remove_file(&path);
    } else if path.is_dir() {
        let _ = std::fs::remove_dir_all(&path);
    }
}

/// `command -v NAME`: the first executable of that name on PATH.
fn command_v(name: &[u8]) -> Option<Vec<u8>> {
    shell::env_or_empty("PATH")
        .split(|&c| c == b':')
        .map(|d| cat!(if d.is_empty() { b".".as_slice() } else { d }, "/", name))
        .find(|p| crate::exec::executable(p) && shell::path(p).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_read_as_bash_reads_them() {
        for no in ["n", "N", "no", " n", "\tn", "n ", "  No  "] {
            assert!(answer_no(no.as_bytes()), "{no:?}");
        }
        for yes in ["", "y", " y", "yes", "  ", "x n", "\u{a0}n"] {
            assert!(!answer_no(yes.as_bytes()), "{yes:?}");
        }
    }
}
