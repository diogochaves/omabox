//! Who started a box, who used it last and who took it down (#295, finding 599). Claude Code's
//! subagents share their session's id and its process, so no command can name the agent it runs
//! for: the agent session (select::agent_session) and the directory a command runs in are what tell
//! two callers apart, and only as far as they differ. `up` records them in box.json (`session`, by
//! `dir`), each command that uses the box in its `used` file (`SESSION<TAB>DIR`, `-` for none), and a
//! `down` in `.taken-NAME` beside the boxes, which the next command of the name reads.

use crate::boxes::{at, touch};
use crate::cat;
use crate::exit::Ctx;
use crate::jail::in_jail;
use crate::shell;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

/// A use this recent of a box makes taking it down for another caller worth a warning.
const RECENT_S: u64 = 600;

/// A caller: its agent session's 8 characters (None outside one) and the directory it ran in.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Caller {
    pub session: Option<Vec<u8>>,
    pub dir: Vec<u8>,
}

/// This command's caller.
pub fn me() -> Caller {
    Caller {
        session: crate::select::agent_session(),
        dir: shell::pwd(),
    }
}

fn line(c: &Caller) -> Vec<u8> {
    cat!(c.session.as_deref().unwrap_or(b"-"), "\t", c.dir, "\n")
}

fn parse(l: &[u8]) -> Option<Caller> {
    let l = shell::strip_nl(l);
    let i = l.iter().position(|&c| c == b'\t')?;
    let s = &l[..i];
    Some(Caller {
        session: (s != b"-" && !s.is_empty()).then(|| s.to_vec()),
        dir: l[i + 1..].to_vec(),
    })
}

/// A use of the box by this command (idle expiry counts from it, as touch's): who, in its `used`
/// file. A jailed caller's (its directory is the jail's) touches it only. Not through a link.
pub fn mark_used(ctx: &Ctx) {
    let p = ctx.at("used");
    if in_jail() {
        return touch(&p);
    }
    let f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(&p);
    match f {
        Ok(mut f) => {
            let _ = f.write_all(&line(&me()));
        }
        Err(_) => touch(&p),
    }
}

/// Who used box dir D last, as its `used` file says (none: a box from before, or a use not said).
pub fn last_user(d: &[u8]) -> Option<Caller> {
    let mut v = Vec::new();
    let f = std::fs::File::open(at(d, "used")).ok()?;
    std::io::Read::read_to_end(&mut std::io::Read::take(f, 4096), &mut v).ok()?;
    parse(&v)
}

/// Who started box dir D, as box.json says; None for a box from before `session` was recorded (and
/// a jailed caller's, which records its jail instead).
pub fn started(d: &[u8]) -> Option<Caller> {
    let text = std::fs::read(at(d, "box.json")).ok()?;
    let (vals, _) = crate::json::values(&text);
    let serde_json::Value::Object(m) = vals.first()? else {
        return None;
    };
    let session = match m.get("session")? {
        serde_json::Value::String(s) => Some(s.as_bytes().to_vec()),
        _ => None,
    };
    let dir = match m.get("dir") {
        Some(serde_json::Value::String(s)) => s.as_bytes().to_vec(),
        _ => Vec::new(),
    };
    Some(Caller { session, dir })
}

/// DIR with $HOME written ~.
pub fn tilde(dir: &[u8]) -> Vec<u8> {
    let home = shell::env_or_empty("HOME");
    if !home.is_empty() && (dir == home || dir.starts_with(&cat!(home, "/"))) {
        cat!("~", &dir[home.len()..])
    } else {
        dir.to_vec()
    }
}

/// C as ME reads it: `in ~/a (this agent session)`, `(agent session 0000beef)`, `(no agent session)`.
pub fn describe(c: &Caller, me: &Caller) -> Vec<u8> {
    let who: Vec<u8> = match &c.session {
        Some(s) if me.session.as_ref() == Some(s) => b"(this agent session)".to_vec(),
        Some(s) => cat!("(agent session ", s, ")"),
        None => b"(no agent session)".to_vec(),
    };
    if c.dir.is_empty() {
        who
    } else {
        cat!("in ", tilde(&c.dir), " ", who)
    }
}

/// Seconds since the box's last use (its `used` file's own time).
fn used_ago(d: &[u8]) -> Option<u64> {
    let t = std::fs::symlink_metadata(at(d, "used"))
        .ok()?
        .modified()
        .ok()?;
    Some(
        std::time::SystemTime::now()
            .duration_since(t)
            .map_or(0, |d| d.as_secs()),
    )
}

fn ago_text(s: u64) -> String {
    if s < 60 {
        format!("{s}s")
    } else {
        format!("{}m", s / 60)
    }
}

/// What a `down` by ME says before it takes the box selected down, when it may be another agent's:
/// used in the last 10 minutes, and started or last used by another caller (another agent session,
/// none, or this one in another directory: a subagent's, or this agent's after a `cd`). Never
/// refused (1.x keeps `down` as it was): the agent is told, and the box's next command says who took
/// it (taken_note). None for a box from before, a jailed caller's, or one this caller alone used.
pub fn down_warning(ctx: &Ctx, me: &Caller) -> Option<Vec<u8>> {
    if !by_another(&ctx.d, me) {
        return None;
    }
    let st = started(&ctx.d)?;
    let ago = used_ago(&ctx.d).filter(|&s| s <= RECENT_S)?;
    let last = last_user(&ctx.d);
    let mut m = cat!(
        "box '",
        ctx.name,
        "' may be another agent's: started ",
        describe(&st, me),
        ", used ",
        ago_text(ago),
        " ago"
    );
    if let Some(l) = last.filter(|l| *l != st) {
        m.extend(cat!(" ", describe(&l, me)));
    }
    m.extend_from_slice(b"; taking it down (its next command will say who did)");
    Some(m)
}

/// Whether ME is another caller than box dir D's: its starter (box.json) or its last user (`used`) is
/// another agent session, none, or this one in another directory. Never for a box from before
/// (no `session`) or a jailed caller's. down_warning's test, and whether a down leaves its note.
pub fn by_another(d: &[u8], me: &Caller) -> bool {
    if in_jail() {
        return false;
    }
    let Some(st) = started(d) else {
        return false;
    };
    st != *me || last_user(d).is_some_and(|l| l != *me)
}

fn taken_file(ctx: &Ctx) -> std::path::PathBuf {
    ctx.in_boxes(cat!(".taken-", ctx.name))
}

/// A `down` by ME took the box selected down: when, by whom, for its next command (`.taken-NAME`,
/// pruned after a day; a new `up` of the name removes it). Only for a down by another caller
/// (by_another, read before the box dir went): a caller's own down leaves none, so its next `run`
/// there starts a throwaway as before.
pub fn write_taken(ctx: &Ctx, me: &Caller) {
    let _ = std::fs::write(
        taken_file(ctx),
        cat!(shell::date("%Y-%m-%d %H:%M:%S"), "\t", line(me)),
    );
}

/// A new box under the name: no note of the last one's down.
pub fn clear_taken(ctx: &Ctx) {
    let _ = std::fs::remove_file(taken_file(ctx));
}

/// What a command of a name whose box a `down` took says, as ME reads it: `box 'X' was taken down
/// at 15:54:03 by an omabox down in ~/c (this agent session)` (the date too when not today).
pub fn taken_note(ctx: &Ctx, me: &Caller) -> Option<Vec<u8>> {
    let v = std::fs::read(taken_file(ctx)).ok()?;
    let i = v.iter().position(|&c| c == b'\t')?;
    let (when, by) = (&v[..i], parse(&v[i + 1..])?);
    let today = shell::date("%Y-%m-%d");
    let when = match when.strip_prefix(today.as_slice()) {
        Some(t) => t.strip_prefix(b" ").unwrap_or(t),
        None => when,
    };
    Some(cat!(
        "box '",
        ctx.name,
        "' was taken down at ",
        when,
        " by an omabox down ",
        describe(&by, me)
    ))
}

/// ls's line under a box: who started it, and who used it last when that is another caller.
pub fn ls_line(d: &[u8], me: &Caller) -> Option<Vec<u8>> {
    let st = started(d)?;
    let mut l = cat!("  started ", describe(&st, me));
    if let Some(u) = last_user(d).filter(|u| *u != st) {
        l.extend(cat!("; last used ", describe(&u, me)));
    }
    l.push(b'\n');
    Some(l)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: Option<&str>, d: &str) -> Caller {
        Caller {
            session: s.map(|s| s.as_bytes().to_vec()),
            dir: d.as_bytes().to_vec(),
        }
    }

    fn tmp(tag: &str) -> String {
        let d = std::env::temp_dir().join(format!("omabox-whose-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("b")).unwrap();
        d.to_str().unwrap().to_string()
    }

    fn ctx_in(boxes: &str) -> Ctx {
        let mut ctx = Ctx::new(b"down", boxes.as_bytes().to_vec());
        ctx.name = b"b".to_vec();
        ctx.d = cat!(boxes, "/b");
        ctx
    }

    fn said(v: Option<Vec<u8>>) -> String {
        v.map(|v| String::from_utf8(v).unwrap())
            .unwrap_or_else(|| "none".into())
    }

    #[test]
    fn a_used_line_read_back() {
        let a = c(Some("1a2b3c4d"), "/x/y z");
        assert_eq!(parse(&line(&a)), Some(a));
        let n = c(None, "/x");
        assert_eq!(parse(&line(&n)), Some(n));
        assert_eq!(
            parse(b""),
            None,
            "an empty used file (a box from before): no one"
        );
    }

    #[test]
    fn callers_described_for_the_reader() {
        let me = c(Some("1a2b3c4d"), "/w");
        let t = |x: &Caller| String::from_utf8(describe(x, &me)).unwrap();
        assert_eq!(t(&c(Some("1a2b3c4d"), "/a")), "in /a (this agent session)");
        assert_eq!(
            t(&c(Some("0000beef"), "/a")),
            "in /a (agent session 0000beef)"
        );
        assert_eq!(t(&c(None, "")), "(no agent session)");
    }

    /// #295: B's down of the box A started in its scratch folder, A having used it a moment ago, is
    /// warned; the down of the one caller that started and used it is not; a box from before (no
    /// `session` in box.json) or one unused for over 10 minutes is not.
    #[test]
    fn a_down_of_another_callers_box_warned() {
        let b = tmp("warn");
        let ctx = ctx_in(&b);
        let me = c(Some("1a2b3c4d"), "/repo");
        let put_json = |j: &str| std::fs::write(format!("{b}/b/box.json"), j).unwrap();
        let used = |who: &Caller| std::fs::write(format!("{b}/b/used"), line(who)).unwrap();
        put_json(r#"{"name":"b","dir":"/repo/scratch-a","session":"1a2b3c4d"}"#);
        used(&c(Some("1a2b3c4d"), "/repo/scratch-a"));
        assert_eq!(
            said(down_warning(&ctx, &me)),
            "box 'b' may be another agent's: started in /repo/scratch-a (this agent session), used 0s ago; taking it down (its next command will say who did)"
        );
        used(&c(Some("0000beef"), "/repo"));
        assert!(
            said(down_warning(&ctx, &me))
                .contains("used 0s ago in /repo (agent session 0000beef);"),
            "...the last user named when another than its starter"
        );
        put_json(r#"{"name":"b","dir":"/repo","session":"1a2b3c4d"}"#);
        used(&me);
        assert_eq!(
            said(down_warning(&ctx, &me)),
            "none",
            "the caller that started and used it: no warning"
        );
        std::fs::write(format!("{b}/b/used"), "").unwrap();
        assert_eq!(
            said(down_warning(&ctx, &me)),
            "none",
            "...nor with no user said"
        );
        put_json(r#"{"name":"b","dir":"/repo","session":null}"#);
        assert!(
            said(down_warning(&ctx, &me)).contains("started in /repo (no agent session)"),
            "a box started outside any agent session, taken down by an agent: warned"
        );
        put_json(r#"{"name":"b","dir":"/elsewhere"}"#);
        assert_eq!(
            said(down_warning(&ctx, &me)),
            "none",
            "a box from before: none"
        );
        put_json(r#"{"name":"b","dir":"/elsewhere","session":"0000beef"}"#);
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(RECENT_S + 60);
        std::fs::File::options()
            .write(true)
            .open(format!("{b}/b/used"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert_eq!(
            said(down_warning(&ctx, &me)),
            "none",
            "unused for over 10 minutes: none"
        );
        std::fs::remove_dir_all(&b).unwrap();
    }

    #[test]
    fn the_down_noted_for_the_next_command() {
        let b = tmp("taken");
        let ctx = ctx_in(&b);
        let by = c(Some("1a2b3c4d"), "/repo");
        assert_eq!(said(taken_note(&ctx, &by)), "none");
        write_taken(&ctx, &by);
        let n = said(taken_note(&ctx, &c(Some("1a2b3c4d"), "/repo/scratch-a")));
        assert!(regex_like(&n), "the note says when and by what: {n}");
        std::fs::write(format!("{b}/.taken-b"), "2026-10-30 15:54:03\t-\t/home/x\n").unwrap();
        assert_eq!(
            said(taken_note(&ctx, &by)),
            "box 'b' was taken down at 2026-10-30 15:54:03 by an omabox down in /home/x (no agent session)",
            "...with its date when not today"
        );
        clear_taken(&ctx);
        assert_eq!(
            said(taken_note(&ctx, &by)),
            "none",
            "an up of the name clears it"
        );
        std::fs::remove_dir_all(&b).unwrap();
    }

    // Whether a down leaves its note (#295's own-down fix): only another caller's, at any age.
    #[test]
    fn only_another_callers_down_is_noted() {
        let b = tmp("own");
        let me = c(Some("1a2b3c4d"), "/repo");
        let put_json = |j: &str| std::fs::write(format!("{b}/b/box.json"), j).unwrap();
        let used = |who: &Caller| std::fs::write(format!("{b}/b/used"), line(who)).unwrap();
        let d = cat!(b.as_str(), "/b");
        put_json(r#"{"name":"b","dir":"/repo","session":"1a2b3c4d"}"#);
        used(&me);
        assert!(
            !by_another(&d, &me),
            "started and used by this caller: its own down"
        );
        std::fs::write(format!("{b}/b/used"), "").unwrap();
        assert!(!by_another(&d, &me), "...no user said: still its own");
        used(&c(Some("1a2b3c4d"), "/repo/scratch-a"));
        assert!(
            by_another(&d, &me),
            "last used in another directory: another's"
        );
        used(&me);
        put_json(r#"{"name":"b","dir":"/repo","session":"0000beef"}"#);
        assert!(by_another(&d, &me), "started by another session: another's");
        put_json(r#"{"name":"b","dir":"/repo","session":null}"#);
        assert!(
            by_another(&d, &me),
            "started outside any session: another's"
        );
        put_json(r#"{"name":"b","dir":"/repo"}"#);
        assert!(!by_another(&d, &me), "a box from before: none");
        std::fs::remove_dir_all(&b).unwrap();
    }

    /// `box 'b' was taken down at HH:MM:SS by an omabox down in /repo (this agent session)`.
    fn regex_like(n: &str) -> bool {
        let Some(r) = n.strip_prefix("box 'b' was taken down at ") else {
            return false;
        };
        let (t, rest) = r.split_at(8.min(r.len()));
        t.len() == 8
            && t.bytes().enumerate().all(|(i, c)| {
                if i == 2 || i == 5 {
                    c == b':'
                } else {
                    c.is_ascii_digit()
                }
            })
            && rest == " by an omabox down in /repo (this agent session)"
    }

    #[test]
    fn ls_says_who_started_it() {
        let b = tmp("ls");
        let me = c(Some("1a2b3c4d"), "/repo");
        let d = format!("{b}/b");
        std::fs::write(
            format!("{d}/box.json"),
            r#"{"dir":"/a","session":"1a2b3c4d"}"#,
        )
        .unwrap();
        std::fs::write(format!("{d}/used"), line(&c(Some("1a2b3c4d"), "/a"))).unwrap();
        assert_eq!(
            said(ls_line(d.as_bytes(), &me)),
            "  started in /a (this agent session)\n"
        );
        std::fs::write(format!("{d}/used"), line(&c(None, "/b"))).unwrap();
        assert_eq!(
            said(ls_line(d.as_bytes(), &me)),
            "  started in /a (this agent session); last used in /b (no agent session)\n"
        );
        std::fs::write(format!("{d}/box.json"), r#"{"dir":"/a"}"#).unwrap();
        assert_eq!(
            said(ls_line(d.as_bytes(), &me)),
            "none",
            "a box from before: no line"
        );
        std::fs::remove_dir_all(&b).unwrap();
    }
}
