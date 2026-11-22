//! Bare `look`: a box that is up as it is now, and what it logged since the last look in it.

use super::*;

/// `omabox look [-b NAME] [-o FILE]`: the box as it is now, nothing started or changed: the screen
/// shot, its windows, the active one's text, its plugins' states, and the problems its programs and
/// its shell logged since the last look in it (any kind's: each leaves a mark, MARK_FILE).
pub(super) fn plain(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut given, mut o_file) = (Vec::new(), None);
    let mut i = 0;
    while i < args.len() {
        let o = &args[i];
        match o.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, o, args.get(i + 1))?;
                given = args[i + 1].clone();
                i += 1;
            }
            b"-o" => {
                optarg(ctx, o, args.get(i + 1))?;
                o_file = Some(args[i + 1].clone());
                i += 1;
            }
            _ => return Err(look_unknown_opt(ctx, None, o)),
        }
        i += 1;
    }
    select_box(ctx, &given)?;
    if !box_alive(ctx)? && given.is_empty() && !shell::path(&ctx.d).is_dir() {
        return Err(ctx.die(
            "look: no box is up to look into: omabox look app|plugin|theme|page looks at one thing in a box of its own; omabox up starts yours",
        ));
    }
    need_box(ctx)?;
    let name = ctx.name.clone();
    let name = name.as_slice();
    let mut l = Look::new(None);
    let mut sh: Vec<&[u8]> = vec![b"shot", b"-b", name];
    if let Some(o) = &o_file {
        sh.extend([b"-o".as_slice(), o]);
    }
    let r = ob(&sh);
    let shot = if r.ok {
        last_line(&r.out)
    } else {
        l.problem(cat!("shot: ", shell::subst(&r.err)));
        None
    };
    // Its windows, the active one first; that one's text, where its toolkit gives any.
    let wins: Value = serde_json::from_slice(&ob(&[b"windows", b"-b", name, b"--json"]).out)
        .unwrap_or(Value::Null);
    let mut wins: Vec<Value> = wins.as_array().cloned().unwrap_or_default();
    let active = |w: &Value| w.get("active").and_then(Value::as_bool) == Some(true);
    wins.sort_by_key(|w| !active(w));
    let mut lines: Vec<Vec<u8>> = wins.iter().take(WINDOWS_MAX).map(window_line).collect();
    if wins.len() > WINDOWS_MAX {
        lines.push(
            format!("... and {} more (omabox windows)", wins.len() - WINDOWS_MAX).into_bytes(),
        );
    }
    let mut text = None;
    if let Some(a) = wins
        .first()
        .and_then(|w| w.get("address"))
        .and_then(Value::as_str)
    {
        let r = ob(&[b"tree", b"-b", name, b"--window", a.as_bytes(), b"--json"]);
        if r.ok {
            let items: Vec<String> = tree_texts(&r.out).0.into_iter().map(|t| t.0).collect();
            if !items.is_empty() {
                text = Some(text_line(&items, PANEL_TEXT_MAX));
            }
        }
    }
    // Its plugins: each one's state, and one the shell did not load is a problem.
    let me: Value = serde_json::from_slice(&ob(&[b"ls", b"--json"]).out).unwrap_or(Value::Null);
    let me = me
        .as_array()
        .and_then(|v| {
            v.iter()
                .find(|b| b.get("name").and_then(Value::as_str).map(str::as_bytes) == Some(name))
        })
        .cloned()
        .unwrap_or(Value::Null);
    let mut ids: Vec<String> = Vec::new();
    let mut plugins: Vec<String> = Vec::new();
    if let Some(m) = me.get("plugin_status").and_then(Value::as_object) {
        for (id, st) in m {
            let state = st.get("state").and_then(Value::as_str).unwrap_or("unknown");
            if state != "loaded" && state != "hosted" {
                l.problem(format!("plugin {id}: {state}").into_bytes());
            }
            plugins.push(format!("{id} {state}"));
            ids.push(id.clone());
        }
    }
    if let Some(st) = me.get("shell_state").and_then(Value::as_str) {
        if !matches!(st, "running" | "none" | "") {
            l.problem(format!("the box's shell is {st} (omabox restart-shell)").into_bytes());
        }
    }
    // What was logged since the last look.
    let old = mark_read(ctx);
    let now = mark_now(ctx);
    l.problems.extend(new_problems(ctx, name, &old, &now, &ids));
    mark_write(ctx, &now);
    let n = wins.len().to_string();
    l.head("screenshot", shot.as_deref().unwrap_or(b"none"));
    let wl = lines
        .iter()
        .fold(n.into_bytes(), |acc, l| cat!(acc, "\n  ", l));
    l.head("windows", wl);
    if let Some(t) = &text {
        l.head("text", t);
    }
    if !plugins.is_empty() {
        l.head("plugins", plugins.join(", "));
    }
    l.head("box", name);
    if wins.is_empty() && plugins.is_empty() {
        l.note(b"no window is open in the box".to_vec());
    }
    l.new = old.is_some();
    l.finish()
}

/// How many windows a plain look lists.
pub(super) const WINDOWS_MAX: usize = 12;

/// `class "title" WxH (tiled|floating[, active])`.
pub(super) fn window_line(w: &Value) -> Vec<u8> {
    let s = |k: &str| w.get(k).and_then(Value::as_str).unwrap_or("");
    let n = |k: &str| w.get(k).and_then(Value::as_i64).unwrap_or(0);
    let mut how = if w.get("floating").and_then(Value::as_bool) == Some(true) {
        "floating".to_string()
    } else {
        "tiled".to_string()
    };
    if w.get("active").and_then(Value::as_bool) == Some(true) {
        how.push_str(", active");
    }
    format!(
        "{} \"{}\" {}x{} ({how})",
        s("class"),
        s("title"),
        n("w"),
        n("h")
    )
    .into_bytes()
}

/// The problems logged between two marks: each run -d log's new lines that name one (a page's
/// console as look page reads it), under the job's program, and the shell's new warnings that name
/// one of the box's plugins (IDS).
pub(super) fn new_problems(
    ctx: &Ctx,
    name: &[u8],
    old: &Option<Value>,
    now: &Value,
    ids: &[String],
) -> Vec<Vec<u8>> {
    let mut found = Vec::new();
    let was = |k: &str| old.as_ref()?.get("logs")?.get(k)?.as_u64();
    // Whose log: the job's program, while `jobs` lists it.
    let jobs: Value =
        serde_json::from_slice(&ob(&[b"jobs", b"-b", name, b"--json"]).out).unwrap_or(Value::Null);
    let whose = |file: &str| -> String {
        jobs.as_array()
            .and_then(|v| {
                v.iter().find(|j| {
                    j.get("log")
                        .and_then(Value::as_str)
                        .is_some_and(|l| l.ends_with(&format!("/{file}")))
                })
            })
            .and_then(|j| {
                j.get("argv")?
                    .as_array()?
                    .first()?
                    .as_str()
                    .map(String::from)
            })
            .map_or_else(
                || file.to_string(),
                |p| {
                    shell::basename(p.as_bytes())
                        .iter()
                        .map(|&c| c as char)
                        .collect()
                },
            )
    };
    if let Some(logs) = now.get("logs").and_then(Value::as_object) {
        for file in logs.keys() {
            let all = crate::boxes::box_home_read(&ctx.d, file, crate::boxes::READ_CAP)
                .unwrap_or_default();
            let new = since(&all, was(file));
            let page = crate::exit::contains(&all, b"omabox-look-chromium")
                || !console_lines(&all).is_empty();
            let lines = if page {
                console_problems(&console_lines(new))
            } else {
                problem_lines(new)
            };
            let who = whose(file);
            found.extend(lines.into_iter().map(|l| cat!(who.as_str(), ": ", l)));
        }
    }
    let first = now
        .get("shell_first")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let same = old
        .as_ref()
        .and_then(|o| o.get("shell_first")?.as_str())
        .is_some_and(|f| f == first);
    let was_len = old
        .as_ref()
        .and_then(|o| o.get("shell_len")?.as_u64())
        .filter(|_| same);
    let warn = crate::plugins::shell_warnings_of(&shell_log_since(&ctx.d, was_len));
    for id in ids {
        found.extend(plugin_warnings(&warn, id.as_bytes()));
    }
    found
}
