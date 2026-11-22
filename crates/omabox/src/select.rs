//! Which box a command is about: a name given (-b, NAME, OMABOX=), else the repo's, with the agent
//! session's suffix (finding 88), else this session's box from another directory (#136).

use crate::boxes;
use crate::cat;
use crate::exit::{Ctx, Res, say};
use crate::jail::{in_jail, jail_id};
use crate::procfs;
use crate::shell::{self, env, env_or_empty};
use std::process::Stdio;

/// Whether $OMABOX names a box. `omabox run` marks its commands with OMABOX=1 and OMABOX_NAME: that 1
/// is not a box named "1". A command under it that sets OMABOX=NAME does name one (issue #19).
pub fn env_names_box() -> bool {
    let o = env_or_empty("OMABOX");
    !o.is_empty() && (o != b"1" || env_or_empty("OMABOX_NAME").is_empty())
}

/// Claude Code and Codex export their session's id to every shell command; `omabox guard exec` sets
/// OMABOX_SESSION for other agents, and OMABOX_SESSION= (empty) opts out.
const AGENT_VARS: [&str; 3] = [
    "OMABOX_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CODEX_THREAD_ID",
];

pub fn agent_var() -> Option<&'static str> {
    for v in AGENT_VARS {
        match env(v) {
            None => continue,
            Some(x) if x.is_empty() => return None,
            Some(_) => return Some(v),
        }
    }
    None
}

/// The agent session this command runs in, as 8 characters, or None outside one. The id's tail:
/// Codex's is a UUIDv7, whose head is a timestamp that two sessions started in the same minute share.
pub fn agent_session() -> Option<Vec<u8>> {
    let id: Vec<u8> = env_or_empty(agent_var()?)
        .into_iter()
        .filter(u8::is_ascii_alphanumeric)
        .collect();
    (id.len() >= 8).then(|| id[id.len() - 8..].to_ascii_lowercase())
}

/// The name as it is (bash's `$(echo "$V")` took a name of -n, -e or -E alone for echo's options
/// and printed nothing, so its refusal named no box: finding 438).
pub fn default_name() -> Vec<u8> {
    // A jailed caller's omabox worked its default out where it runs, in the jail (relay_call).
    if in_jail() {
        let d = env_or_empty("OMABOX_RELAY_DEFAULT");
        return shell::strip_nl(if d.is_empty() { b"default" } else { &d });
    }
    if env_names_box() {
        return shell::strip_nl(&env_or_empty("OMABOX"));
    }
    let top = crate::sig::command("git")
        .args(["rev-parse", "--show-toplevel"])
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success());
    let base = match top {
        Some(o) => shell::basename(&shell::strip_nl(&o.stdout)),
        None => b"default".to_vec(),
    };
    // An agent session gets a box of its own (finding 88): two agents in one repo would otherwise
    // share one, and one's `down` would end the other's work. The suffix survives the 40-char cut.
    match agent_session() {
        Some(sid) => shell::strip_nl(&cat!(shell::cut_c(&base, 31), "-", sid)),
        None => shell::strip_nl(&base),
    }
}

/// What select_box makes of a name: other characters made `-`, 40 bytes at most (tr and cut work in
/// bytes).
pub fn tame(v: &[u8]) -> Vec<u8> {
    v.iter()
        .map(|&c| {
            if c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-') {
                c
            } else {
                b'-'
            }
        })
        .take(40)
        .collect()
}

/// select_box's name: GIVEN, or the default; refused when it starts with no letter or digit, or (a
/// name given) when it is not written as a box name is (warned in 0.9, refused since 1.0: #273
/// audit E3, #287, finding 402).
fn pick_name(ctx: &Ctx, given: &[u8]) -> Res<Vec<u8>> {
    let raw = if given.is_empty() {
        default_name()
    } else {
        given.to_vec()
    };
    let name = tame(&raw);
    if !shell::re_match("^[A-Za-z0-9]", &name) {
        let shown = if given.is_empty() { &name } else { given };
        let m = cat!(
            ctx.label(),
            "bad box name ",
            shown,
            ": a box name starts with a letter or digit, then letters, digits, . _ - (40 at most)"
        );
        return Err(if given.is_empty() {
            ctx.die(m)
        } else {
            ctx.usage_die(m)
        });
    }
    if !given.is_empty() && name != given {
        return Err(ctx.usage_die(cat!(
            ctx.label(),
            "box name '",
            given,
            "': a box name is letters, digits, . _ - (40 at most): write ",
            name
        )));
    }
    Ok(name)
}

/// Sets NAME and D (the box dir).
pub fn select_box(ctx: &mut Ctx, given: &[u8]) -> Res<()> {
    ctx.name = pick_name(ctx, given)?;
    ctx.parsed(); // (the command's words are read by the time it picks its box)
    ctx.d = cat!(ctx.boxes, "/", ctx.name);
    ctx.name_derived = given.is_empty();
    if ctx.name_derived && env_or_empty("SELECT_PLAIN").is_empty() {
        session_fallback(ctx);
    }
    // (its command log gets this command's line: #447; a jailed caller's only when the box is its
    // jail's, so the refusal below writes none in the user's or another jail's log)
    crate::cmdlog::selected(&ctx.name, &ctx.d);
    // A jailed agent drives only the boxes its jail started (finding 99): not the user's, nor another
    // jail's, which may see more than this one.
    if in_jail()
        && ctx.at("box.json").is_file()
        && crate::boxmeta::BoxMeta::load(&ctx.d).jail != jail_id()
    {
        return Err(ctx.die(cat!(
            "box '",
            ctx.name,
            "' is not this jail's: an agent inside ai-jail drives only the boxes it started"
        )));
    }
    Ok(())
}

/// Whether the box selected is the one this agent session's commands get by default (finding 88). A
/// jailed agent's boxes go with its jail instead. (The default worked out again: a refusal of it is
/// said, and is a no.)
pub fn session_box(ctx: &Ctx) -> bool {
    if in_jail() || env_names_box() || agent_session().is_none() {
        return false;
    }
    let mut plain = Ctx::new(&ctx.main_cmd, ctx.boxes.clone());
    plain.parsing = false;
    plain.die_rc = ctx.die_rc;
    plain.name_derived = true;
    pick_name(&plain, b"").is_ok_and(|n| n == ctx.name)
}

/// The boxes up that carry this agent session's id (finding 88's suffix), other than the one
/// selected; none outside a session, in a jail (its boxes go by jail) or with OMABOX naming the box.
pub fn session_boxes(ctx: &Ctx) -> Vec<Vec<u8>> {
    let sid = match agent_session() {
        Some(s) if !in_jail() && !env_names_box() => s,
        _ => return Vec::new(),
    };
    let tail = cat!("-", sid);
    shell::words(&boxes::list_names(ctx, false).join(&b'\n'))
        .into_iter()
        .filter(|n| n.ends_with(&tail) && *n != ctx.name)
        .filter(|n| boxes::box_state(&cat!(ctx.boxes, "/", n)) == "up")
        .collect()
}

/// A command run outside any repo derives default-SESSION, while the session's box was started in
/// its repo as REPO-SESSION (issue #136): that box is the one meant, when it is the session's only
/// box and no box has the derived name. Not for `up` (it says so instead), nor a name given (-b,
/// OMABOX=), which select_box takes as it is. Said once per box, on stderr.
fn session_fallback(ctx: &mut Ctx) {
    if !ctx.name.starts_with(b"default-") || ctx.main_cmd == b"up" || shell::path(&ctx.d).exists() {
        return;
    }
    let mine = session_boxes(ctx);
    let [one] = mine.as_slice() else {
        return;
    };
    let from = std::mem::replace(&mut ctx.name, one.clone());
    ctx.d = cat!(ctx.boxes, "/", ctx.name);
    if ctx.at("noted-default").exists() {
        return;
    }
    say(cat!(
        "note: no box '",
        from,
        "' (this directory is in no repo); using this session's box '",
        ctx.name,
        "', started from its repo (-b ",
        ctx.name,
        " names it)"
    ));
    let _ = std::fs::File::create(ctx.at("noted-default"));
}

/// The agent's own process, as (PID, START), or None (finding 93). Claude Code exports its pid as
/// CLAUDE_PID; `guard exec` exports the pid it execs the agent as, OMABOX_AGENT_PID. Codex sets its
/// session variable for its children only, so it is the nearest ancestor without it (compared by
/// value: a `codex` started from another session's shell carries the outer id). OMABOX_SESSION set
/// by hand names no agent; neither does one in a pid namespace of its own, nor a pid this command
/// does not descend from.
pub fn agent_proc() -> Option<(Vec<u8>, Vec<u8>)> {
    agent_proc_with(procfs::proc_start)
}

/// agent_proc with START for proc_start (a #[test]'s agent that exits as it is read).
fn agent_proc_with(start: impl Fn(&[u8]) -> Option<Vec<u8>>) -> Option<(Vec<u8>, Vec<u8>)> {
    let v = agent_var()?;
    let pid = match v {
        "CLAUDE_CODE_SESSION_ID" => env_or_empty("CLAUDE_PID"),
        "OMABOX_SESSION" => env_or_empty("OMABOX_AGENT_PID"),
        _ => {
            let entry = cat!(v, "=", env_or_empty(v));
            let mut pid = std::process::id().to_string().into_bytes();
            while shell::test_int(&pid).is_some_and(|p| p > 1) && environ_has(&pid, &entry) {
                pid = procfs::proc_ppid(&pid)?;
            }
            pid
        }
    };
    if !shell::is_num(&pid)
        || shell::test_int(&pid).is_none_or(|p| p <= 1)
        || !readable(&cat!("/proc/", pid, "/environ"))
        || !descends_from(&pid)
    {
        return None;
    }
    // Gone meanwhile: no start time to record (with none, agent_alive would never find it gone).
    let start = start(&pid).filter(|s| !s.is_empty())?;
    Some((pid, start))
}

/// `grep -qzxF ENTRY /proc/PID/environ`.
fn environ_has(pid: &[u8], entry: &[u8]) -> bool {
    std::fs::read(shell::path(&cat!("/proc/", pid, "/environ")))
        .is_ok_and(|e| e.split(|&c| c == 0).any(|r| r == entry))
}

/// `[ -r PATH ]`.
fn readable(p: &[u8]) -> bool {
    std::ffi::CString::new(p.to_vec()).is_ok_and(|c| {
        // SAFETY: c is a valid C string for the call.
        unsafe { libc::access(c.as_ptr(), libc::R_OK) == 0 }
    })
}

/// Whether this process is PID or runs under it.
pub fn descends_from(target: &[u8]) -> bool {
    let mut p = std::process::id().to_string().into_bytes();
    while p != target {
        if shell::test_int(&p).is_none_or(|n| n <= 1) {
            return false;
        }
        match procfs::proc_ppid(&p) {
            Some(pp) => p = pp,
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// t_unit_agent_session (finding 93): agent_proc fails when its agent's start time cannot be
    /// read (it exited meanwhile), and is the agent, CLAUDE_PID, when it can.
    #[test]
    fn agent_proc_needs_a_start() {
        let me = std::process::id().to_string();
        let vars = [
            ("CLAUDE_CODE_SESSION_ID", Some("t-session")),
            ("CLAUDE_PID", Some(me.as_str())),
            ("OMABOX_SESSION", None),
            ("CODEX_THREAD_ID", None),
        ];
        let (none, mine) = shell::with_env(&vars, || {
            (
                agent_proc_with(|_| None),
                agent_proc_with(|_| Some(b"77".to_vec())),
            )
        });
        assert_eq!(none, None, "no start time: no agent");
        assert_eq!(
            mine,
            Some((me.into_bytes(), b"77".to_vec())),
            "...one: the agent is CLAUDE_PID"
        );
    }

    /// t_unit_agent_session (#136): `up` never takes the session's box for its derived name (it
    /// says so instead: up_session_note).
    #[test]
    fn names_made_tame() {
        assert_eq!(tame(b"my repo"), b"my-repo");
        assert_eq!(tame("dé".as_bytes()), b"d--");
        assert_eq!(tame(b"a/b\nc"), b"a-b-c");
        assert_eq!(tame(&[b'x'; 50]).len(), 40);
        assert_eq!(tame(b"ok_1.2-3"), b"ok_1.2-3");
    }
}
