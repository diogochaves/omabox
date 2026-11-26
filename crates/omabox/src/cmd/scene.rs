//! `omabox scene [-b NAME] (FILE | -) [--timeout DURATION] [--json]` (#489, finding 762): a box's
//! windows as a scene file lists them, each opened by its command (a `run -d` job) and put where
//! the file says: a workspace (a number or `special:NAME`), tiled or floating, a size, a place,
//! pinned. It returns once every window is mapped as asked, or names each that is not (exit 1).
//!
//! A scene's window is known by its entry's name: `<box dir>/scene.json` keeps the address each
//! name opened, so the same scene applied again opens nothing whose window is still mapped, and
//! only puts it back where the file says. `up --scene FILE` applies one to the box it starts.

use crate::args::{optarg, unexpected, unknown_opt};
use crate::boxes::need_box;
use crate::cat;
use crate::cmd::window::{lua_head, lua_pin, parse_size};
use crate::exec::{ErrTo, Out, on_box};
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::hypr::{HYPR_LIMIT, hung_text, hypr_answers_in, hypr_json};
use crate::jail::in_jail;
use crate::json;
use crate::num::ms_duration;
use crate::select::select_box;
use crate::shell;
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::time::{Duration, Instant};

/// Where a window goes: a numbered workspace, or a special one by its name.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ws {
    Id(u32),
    Special(String),
}

impl Ws {
    /// As `windows --json`'s workspace has it, and as Hyprland's selector takes it: `3`,
    /// `special:notes`.
    pub fn name(&self) -> String {
        match self {
            Ws::Id(n) => n.to_string(),
            Ws::Special(s) => format!("special:{s}"),
        }
    }
}

/// One window of a scene.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    pub name: String,
    /// Its command's words (a string in the file: `bash -c STRING`).
    pub run: Vec<String>,
    /// None only for a pinned window (it shows on every workspace).
    pub ws: Option<Ws>,
    /// As the file says: None leaves the window as its app (or a rule) made it.
    pub floating: Option<bool>,
    pub pinned: bool,
    pub size: Option<(u32, u32)>,
    pub at: Option<(i32, i32)>,
}

impl Entry {
    /// Whether the window is to float: asked for, or implied by a size, a place or a pin. None:
    /// left as it is.
    pub fn floats(&self) -> Option<bool> {
        if self.pinned || self.size.is_some() || self.at.is_some() {
            Some(true)
        } else {
            self.floating
        }
    }
}

const KEYS: &str = "name, run, workspace, floating, pinned, size, at";

/// `"at"`'s X,Y: whole layout pixels, as `windows` gives a window's place.
pub fn parse_at(v: &str) -> Option<(i32, i32)> {
    let (x, y) = v.split_once(',')?;
    let n = |s: &str| -> Option<i32> {
        let d = s.strip_prefix('-').unwrap_or(s);
        if d.is_empty() || d.len() > 5 || !d.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        s.parse().ok()
    };
    Some((n(x)?, n(y)?))
}

/// `"workspace"`: 1 and up, or `special:NAME` (letters, digits, `.`, `_`, `-`).
pub fn parse_ws(v: &Value) -> Option<Ws> {
    let id = |s: &str| -> Option<Ws> {
        if s.is_empty() || s.len() > 9 || !s.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        s.parse::<u32>().ok().filter(|&n| n >= 1).map(Ws::Id)
    };
    match v {
        Value::Number(n) => id(&n.to_string()),
        Value::String(s) => match s.strip_prefix("special:") {
            Some(n) => (!n.is_empty()
                && n.len() <= 64
                && n.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-')))
            .then(|| Ws::Special(n.to_string())),
            None => id(s),
        },
        _ => None,
    }
}

fn name_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

/// A scene file's windows, or what is wrong with it, in words that name the window.
pub fn parse(text: &[u8]) -> Result<Vec<Entry>, String> {
    let v: Value = serde_json::from_slice(text).map_err(|e| format!("not JSON ({e})"))?;
    let Some(top) = v.as_object() else {
        return Err(r#"a scene is a JSON object, {"windows": [...]}"#.to_string());
    };
    if let Some(k) = top.keys().find(|k| *k != "windows") {
        return Err(format!(r#"unknown key "{k}" (a scene has one: "windows")"#));
    }
    let Some(list) = top.get("windows").and_then(Value::as_array) else {
        return Err(r#""windows" is a list of windows, [{"name": ..., "run": ...}]"#.to_string());
    };
    if list.is_empty() {
        return Err(r#""windows" lists no window"#.to_string());
    }
    if list.len() > 64 {
        return Err(format!(
            r#""windows" lists {} windows: 64 at most"#,
            list.len()
        ));
    }
    let mut out: Vec<Entry> = Vec::new();
    for (i, w) in list.iter().enumerate() {
        let n = i + 1;
        let Some(o) = w.as_object() else {
            return Err(format!("window {n} is not an object ({KEYS})"));
        };
        let who = match o.get("name").and_then(Value::as_str) {
            Some(s) if name_ok(s) => format!("window {n} (\"{s}\")"),
            _ => format!("window {n}"),
        };
        if let Some(k) = o
            .keys()
            .find(|k| !KEYS.split(", ").any(|x| x == k.as_str()))
        {
            return Err(format!(r#"{who}: unknown key "{k}" (its keys: {KEYS})"#));
        }
        let name = match o.get("name") {
            Some(Value::String(s)) if name_ok(s) => s.clone(),
            Some(_) => {
                return Err(format!(
                    r#"{who}: "name" is 1 to 64 letters, digits, ".", "_" or "-", the first a letter or digit"#
                ));
            }
            None => {
                return Err(format!(
                    r#"{who} has no "name" (what the scene knows its window by)"#
                ));
            }
        };
        if out.iter().any(|e| e.name == name) {
            return Err(format!(r#"{who}: two windows are named "{name}""#));
        }
        let run = match o.get("run") {
            Some(Value::String(s)) if !s.trim().is_empty() => {
                vec!["bash".to_string(), "-c".to_string(), s.clone()]
            }
            Some(Value::Array(a))
                if !a.is_empty()
                    && a.iter().all(Value::is_string)
                    && a[0].as_str().is_some_and(|s| !s.is_empty()) =>
            {
                a.iter()
                    .map(|s| s.as_str().unwrap_or_default().to_string())
                    .collect()
            }
            Some(_) => {
                return Err(format!(
                    r#"{who}: "run" is the command that opens it: its words, ["foot", "-T", "x"], or one string for bash"#
                ));
            }
            None => {
                return Err(format!(r#"{who} has no "run" (the command that opens it)"#));
            }
        };
        if run.iter().any(|s| s.contains('\0')) {
            return Err(format!(r#"{who}: "run" has a NUL byte"#));
        }
        let flag = |k: &str| -> Result<Option<bool>, String> {
            match o.get(k) {
                None => Ok(None),
                Some(Value::Bool(b)) => Ok(Some(*b)),
                Some(_) => Err(format!(r#"{who}: "{k}" is true or false"#)),
            }
        };
        let floating = flag("floating")?;
        let pinned = flag("pinned")?.unwrap_or(false);
        let size = match o.get("size") {
            None => None,
            Some(v) => Some(
                v.as_str()
                    .and_then(|s| parse_size(s.as_bytes()))
                    .ok_or_else(|| {
                        format!(
                            r#"{who}: "size" is "WxH" in layout pixels ("760x600"), 1 to 16384 each"#
                        )
                    })?,
            ),
        };
        let at = match o.get("at") {
            None => None,
            Some(v) => Some(v.as_str().and_then(parse_at).ok_or_else(|| {
                format!(
                    r#"{who}: "at" is "X,Y" in layout pixels ("100,80"), as windows gives a place"#
                )
            })?),
        };
        let ws = match o.get("workspace") {
            None => None,
            Some(v) => Some(parse_ws(v).ok_or_else(|| {
                format!(r#"{who}: "workspace" is a number from 1, or "special:NAME""#)
            })?),
        };
        if pinned && ws.is_some() {
            return Err(format!(
                r#"{who}: a pinned window shows on every workspace: "pinned" or "workspace", not both"#
            ));
        }
        if !pinned && ws.is_none() {
            return Err(format!(
                r#"{who} has no "workspace" (a number, or "special:NAME"; only a pinned window has none)"#
            ));
        }
        if floating == Some(false) && (pinned || size.is_some() || at.is_some()) {
            return Err(format!(
                r#"{who}: "size", "at" and "pinned" float the window: not with "floating": false"#
            ));
        }
        out.push(Entry {
            name,
            run,
            ws,
            floating,
            pinned,
            size,
            at,
        });
    }
    Ok(out)
}

/// A scene from FILE, or stdin for `-`: read and checked before any box is looked at. LABEL is
/// who asks (`scene`, `up: --scene`). A jailed caller's scene comes on stdin only: a path there
/// would be read on the user's side of the jail.
pub fn load(ctx: &Ctx, label: &str, file: &[u8]) -> Res<Vec<Entry>> {
    let text = if file == b"-" {
        let mut v = Vec::new();
        if std::io::stdin().read_to_end(&mut v).is_err() {
            return Err(ctx.die(cat!(label, ": cannot read the scene from stdin")));
        }
        v
    } else {
        if in_jail() {
            return Err(ctx.die(cat!(
                label,
                ": a jailed agent's scene comes on stdin (- for the file, the scene piped in), never from a path here"
            )));
        }
        match std::fs::read(shell::path(file)) {
            Ok(t) => t,
            Err(e) => {
                return Err(ctx.die(cat!(
                    label,
                    ": cannot read ",
                    file,
                    " (",
                    e.kind().to_string(),
                    ")"
                )));
            }
        }
    };
    parse(&text).map_err(|e| {
        ctx.die(cat!(
            label,
            ": ",
            if file == b"-" { b"stdin" } else { file },
            ": ",
            e,
            " (omabox help scene)"
        ))
    })
}

/// What an entry's window is to be made, given how the box has it now (C, an element of `hyprctl
/// -j clients`): the chunk's lines after its head, none when it is already so. FRESH: just opened
/// (a sized window with no place is centred then, and whenever its size changes).
pub fn place_lua(e: &Entry, c: &Value, fresh: bool) -> Vec<String> {
    let mut l = Vec::new();
    let is = |k: &str| c.get(k).and_then(Value::as_bool).unwrap_or(false);
    let pair = |k: &str| -> Option<(i64, i64)> {
        let a = c.get(k)?.as_array()?;
        Some((num(a.first()?)?, num(a.get(1)?)?))
    };
    if is("pinned") && !e.pinned {
        l.push(String::from_utf8_lossy(&lua_pin(false)).into_owned());
    }
    match e.floats() {
        Some(f) if f != is("floating") => l.push(format!(
            "d(hl.dsp.window.float({{ action = '{}', window = w }}), 'hl.dsp.window.float')",
            if f { "enable" } else { "disable" }
        )),
        _ => {}
    }
    if let Some(ws) = &e.ws
        && ws_name(c) != ws.name()
    {
        l.push(format!(
            "d(hl.dsp.window.move({{ workspace = '{}', follow = false, window = w }}), 'hl.dsp.window.move')",
            ws.name()
        ));
    }
    let mut sized = false;
    if let Some((w, h)) = e.size
        && pair("size") != Some((i64::from(w), i64::from(h)))
    {
        sized = true;
        l.push(format!(
            "d(hl.dsp.window.resize({{ window = w, x = {w}, y = {h} }}), 'hl.dsp.window.resize')"
        ));
    }
    match e.at {
        Some((x, y)) if pair("at") != Some((i64::from(x), i64::from(y))) => l.push(format!(
            "d(hl.dsp.window.move({{ x = {x}, y = {y}, window = w }}), 'hl.dsp.window.move')"
        )),
        None if e.size.is_some() && (sized || fresh) => {
            l.push("d(hl.dsp.window.center({ window = w }), 'hl.dsp.window.center')".to_string())
        }
        _ => {}
    }
    if e.pinned && !is("pinned") {
        l.push(String::from_utf8_lossy(&lua_pin(true)).into_owned());
    }
    l
}

fn num(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().map(|f| f.round() as i64))
}

fn ws_name(c: &Value) -> String {
    c.pointer("/workspace/name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// What of entry E the window C is not (yet): None when it is as asked.
pub fn problem(e: &Entry, c: &Value) -> Option<String> {
    let is = |k: &str| c.get(k).and_then(Value::as_bool).unwrap_or(false);
    let pair = |k: &str| -> Option<(i64, i64)> {
        let a = c.get(k)?.as_array()?;
        Some((num(a.first()?)?, num(a.get(1)?)?))
    };
    if let Some(ws) = &e.ws
        && ws_name(c) != ws.name()
    {
        return Some(format!("is on workspace {}, not {}", ws_name(c), ws.name()));
    }
    if let Some(f) = e.floats()
        && f != is("floating")
    {
        return Some(
            if f {
                "is still tiled"
            } else {
                "is still floating"
            }
            .to_string(),
        );
    }
    if e.pinned != is("pinned") {
        return Some(
            if e.pinned {
                "is not pinned"
            } else {
                "is still pinned"
            }
            .to_string(),
        );
    }
    if let Some((w, h)) = e.size
        && let Some((cw, ch)) = pair("size")
        && (cw, ch) != (i64::from(w), i64::from(h))
    {
        return Some(format!(
            "took {cw}x{ch}, not {w}x{h} (its app's smallest or largest size?)"
        ));
    }
    if let Some((x, y)) = e.at
        && let Some((cx, cy)) = pair("at")
        && (cx, cy) != (i64::from(x), i64::from(y))
    {
        return Some(format!("is at {cx},{cy}, not {x},{y}"));
    }
    None
}

/// The box's mapped windows, as `hyprctl -j clients` lists them.
fn clients(ctx: &Ctx) -> Res<Vec<Value>> {
    let c = hypr_json(ctx, &["clients"])?;
    let v: Value = serde_json::from_slice(&c).unwrap_or(Value::Null);
    Ok(v.as_array()
        .map(|a| {
            a.iter()
                .filter(|c| c.get("mapped").and_then(Value::as_bool).unwrap_or(false))
                .cloned()
                .collect()
        })
        .unwrap_or_default())
}

fn addr_of(c: &Value) -> &str {
    c.get("address").and_then(Value::as_str).unwrap_or_default()
}

fn find<'a>(cs: &'a [Value], addr: &str) -> Option<&'a Value> {
    cs.iter().find(|c| addr_of(c) == addr)
}

/// A chunk in the box's Hyprland: Ok, or its words for why not (a hung one ends the command).
fn eval(ctx: &Ctx, chunk: Vec<u8>) -> Res<Result<(), String>> {
    let r = on_box(
        ctx,
        &[
            b"timeout".to_vec(),
            HYPR_LIMIT.as_bytes().to_vec(),
            b"hyprctl".to_vec(),
            b"eval".to_vec(),
            chunk,
        ],
        true,
        Out::Capture(None),
        ErrTo::Out,
    );
    let res = shell::subst(&r.out);
    if res == b"ok" {
        return Ok(Ok(()));
    }
    if !hypr_answers_in(ctx, "1")? {
        let t = hung_text(ctx, "")?;
        return Err(ctx.die(t));
    }
    Ok(Err(String::from_utf8_lossy(&res).into_owned()))
}

/// `<box dir>/scene.json`: the address each name's window was opened at.
fn record_load(ctx: &Ctx) -> Map<String, Value> {
    std::fs::read(ctx.at("scene.json"))
        .ok()
        .and_then(|t| serde_json::from_slice::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn record_save(ctx: &Ctx, rec: &Map<String, Value>) {
    let p = ctx.at("scene.json");
    let tmp = ctx.at(&format!("scene.json.{}", std::process::id()));
    if std::fs::write(&tmp, json::compact(&Value::Object(rec.clone()))).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// E's command as a `run -d` job of the box (its own omabox: the job recorded, its log kept, as
/// any other): the job's pid in the box, or why it did not start.
fn launch(ctx: &Ctx, e: &Entry) -> Result<Option<u64>, String> {
    let mut c = crate::sig::omabox_cmd();
    c.arg("run")
        .arg("-b")
        .arg(std::ffi::OsStr::from_bytes(&ctx.name))
        .args(["-d", "-q", "--"])
        .args(&e.run);
    c.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit());
    let r = crate::sig::run_child(c, false, |ch| {
        let mut o = Vec::new();
        if let Some(mut s) = ch.stdout.take() {
            let _ = s.read_to_end(&mut o);
        }
        o
    });
    match r {
        Ok((o, (0, None))) => Ok(String::from_utf8_lossy(&o).trim().parse().ok()),
        Ok((_, (rc, None))) => Err(format!(
            "its command did not start, or ended at once (status {rc}; what it said is above)"
        )),
        Ok((_, (_, Some(sig)))) => Err(format!("its run was stopped by signal {sig}")),
        Err(e) => Err(format!("omabox run could not start ({e})")),
    }
}

/// One window's part of the answer.
struct Done {
    name: String,
    addr: String,
    opened: bool,
    problem: Option<String>,
}

/// The scene applied to the box (up, selected): each window opened unless its name's window is
/// still mapped, then placed; true when every one is as asked. With REPORT, a line a window on
/// stdout (or one JSON document); what is not as asked is said on stderr either way.
pub fn apply(ctx: &Ctx, scene: &[Entry], timeout_ms: i64, report: Option<bool>) -> Res<bool> {
    let mut rec = record_load(ctx);
    let before = clients(ctx)?;
    let was_active = crate::win::jq_r(
        crate::jqprog::ACTIVE_ADDR_JQ,
        &hypr_json(ctx, &["activewindow"])?,
    );
    let mut known: HashSet<String> = before.iter().map(|c| addr_of(c).to_string()).collect();
    let mut done: Vec<Done> = Vec::new();
    for e in scene {
        let kept = rec
            .get(&e.name)
            .and_then(Value::as_str)
            .filter(|a| find(&before, a).is_some())
            .map(str::to_string);
        let mut d = Done {
            name: e.name.clone(),
            addr: kept.clone().unwrap_or_default(),
            opened: kept.is_none(),
            problem: None,
        };
        let mut now = kept.as_deref().and_then(|a| find(&before, a)).cloned();
        if kept.is_none() {
            match launch(ctx, e) {
                Err(why) => d.problem = Some(why),
                Ok(job) => {
                    // The window this command opened: one the box did not have, its job's own
                    // when two came at once.
                    let deadline =
                        Instant::now() + Duration::from_millis(timeout_ms.unsigned_abs());
                    loop {
                        let cs = clients(ctx)?;
                        let new: Vec<&Value> =
                            cs.iter().filter(|c| !known.contains(addr_of(c))).collect();
                        let hit = new
                            .iter()
                            .find(|c| job.is_some() && c.get("pid").and_then(Value::as_u64) == job)
                            .or(new.first());
                        if let Some(c) = hit {
                            d.addr = addr_of(c).to_string();
                            now = Some((*c).clone());
                            break;
                        }
                        if Instant::now() >= deadline {
                            d.problem = Some(format!(
                                "opened no window in {} s (its job: omabox jobs, omabox log run)",
                                timeout_ms as f64 / 1000.0
                            ));
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            }
            if !d.addr.is_empty() {
                known.insert(d.addr.clone());
                rec.insert(e.name.clone(), Value::String(d.addr.clone()));
                record_save(ctx, &rec);
            }
        }
        if let Some(c) = &now
            && shell::re_match("^0x[0-9a-f]+$", d.addr.as_bytes())
        {
            let lines = place_lua(e, c, d.opened);
            if !lines.is_empty() {
                let chunk = cat!(lua_head(d.addr.as_bytes()), lines.join("\n"));
                if let Err(why) = eval(ctx, chunk)? {
                    d.problem = Some(format!("the box's Hyprland refused it: {why}"));
                }
            }
        }
        done.push(d);
    }
    // As the box has them now: each as asked, 3 s at most for what was just moved to land.
    let deadline = Instant::now() + Duration::from_secs(3);
    let cs = loop {
        let cs = clients(ctx)?;
        let mut waiting = false;
        for (e, d) in scene.iter().zip(done.iter()) {
            if d.problem.is_none() && find(&cs, &d.addr).is_none_or(|c| problem(e, c).is_some()) {
                waiting = true;
            }
        }
        if !waiting || Instant::now() >= deadline {
            break cs;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    for (e, d) in scene.iter().zip(done.iter_mut()) {
        if d.problem.is_none() {
            d.problem = match find(&cs, &d.addr) {
                Some(c) => problem(e, c),
                None => Some("closed meanwhile".to_string()),
            };
        }
    }
    refocus(ctx, &was_active, &cs)?;
    let ok = done.iter().all(|d| d.problem.is_none());
    if let Some(as_json) = report {
        let rows: Vec<Value> = done
            .iter()
            .map(|d| {
                let c = find(&cs, &d.addr);
                json!({
                    "name": d.name,
                    "address": if d.addr.is_empty() { Value::Null } else { json!(d.addr) },
                    "opened": d.opened && !d.addr.is_empty(),
                    "ok": d.problem.is_none(),
                    "problem": d.problem,
                    "workspace": c.map(ws_name),
                    "floating": c.and_then(|c| c.get("floating").cloned()),
                    "pinned": c.and_then(|c| c.get("pinned").cloned()),
                    "at": c.and_then(|c| c.get("at").cloned()),
                    "size": c.and_then(|c| c.get("size").cloned()),
                })
            })
            .collect();
        if as_json {
            out(cat!(
                json::compact(&json!({ "ok": ok, "windows": rows })),
                "\n"
            ))?;
        } else {
            for (d, r) in done.iter().zip(rows.iter()) {
                let Some(c) = find(&cs, &d.addr) else {
                    continue;
                };
                let b = |k: &str| r.get(k).and_then(Value::as_bool).unwrap_or(false);
                let xy = |k: &str, sep: &str| {
                    let a = c.get(k).and_then(Value::as_array);
                    let n = |i: usize| a.and_then(|a| a.get(i)).and_then(num).unwrap_or(0);
                    format!("{}{sep}{}", n(0), n(1))
                };
                out(format!(
                    "{} {} {} workspace {} {}{} at {} {}\n",
                    d.name,
                    d.addr,
                    if d.opened { "opened" } else { "kept" },
                    ws_name(c),
                    if b("floating") { "floating" } else { "tiled" },
                    if b("pinned") { ", pinned" } else { "" },
                    xy("at", ","),
                    xy("size", "x"),
                ))?;
            }
        }
    }
    for d in &done {
        if let Some(p) = &d.problem {
            let which = if d.addr.is_empty() {
                String::new()
            } else {
                format!(" ({})", d.addr)
            };
            say(format!("scene: window \"{}\"{which} {p}", d.name));
        }
    }
    Ok(ok)
}

/// The window that had the focus before has it again, if the scene's windows took it and it is
/// still on a workspace the screen shows; the pointer left where it is (win_refocus's way).
fn refocus(ctx: &Ctx, was: &[u8], cs: &[Value]) -> Res<()> {
    if !shell::re_match("^0x[0-9a-f]+$", was) {
        return Ok(());
    }
    let was_s = String::from_utf8_lossy(was).into_owned();
    let Some(c) = find(cs, &was_s) else {
        return Ok(());
    };
    let now = crate::win::jq_r(
        crate::jqprog::ACTIVE_ADDR_JQ,
        &hypr_json(ctx, &["activewindow"])?,
    );
    if now == was {
        return Ok(());
    }
    let mons: Value =
        serde_json::from_slice(&hypr_json(ctx, &["monitors"])?).unwrap_or(Value::Null);
    let ws = ws_name(c);
    let shown = mons.as_array().is_some_and(|m| {
        m.iter().any(|m| {
            ["/activeWorkspace/name", "/specialWorkspace/name"]
                .iter()
                .any(|p| m.pointer(p).and_then(Value::as_str) == Some(ws.as_str()))
        })
    });
    if !shown {
        return Ok(());
    }
    let lua = cat!(
        "local nw = hl.get_config('cursor.no_warps')\nhl.config({ cursor = { no_warps = true } })\nlocal ok, err = pcall(hl.dispatch, hl.dsp.focus({ window = 'address:",
        was,
        "' }))\nhl.config({ cursor = { no_warps = nw } })\nif not ok then error(err, 0) end"
    );
    // (a focus that did not come back is no failure of the scene: its windows are as asked)
    let _ = eval(ctx, lua)?;
    Ok(())
}

/// `--timeout`'s value: how long each window may take to open, 100 ms to 10 min.
pub fn parse_timeout(v: &[u8]) -> Option<i64> {
    ms_duration(v).filter(|ms| (100..=600_000).contains(ms))
}

pub const TIMEOUT_MS: i64 = 10_000;

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut json, mut file): (Vec<u8>, bool, Option<Vec<u8>>) =
        (Vec::new(), false, None);
    let mut timeout = TIMEOUT_MS;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let v = args.get(i + 1);
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, v)?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"--timeout" => {
                optarg(ctx, a, v)?;
                timeout = parse_timeout(&args[i + 1]).ok_or_else(|| {
                    ctx.usage_die(cat!(
                        "scene: --timeout is how long each window may take to open, 100ms to 10m, like 10s (a bare number is seconds), got ",
                        args[i + 1]
                    ))
                })?;
                i += 1;
            }
            b"--json" => json = true,
            b"-h" | b"--help" => return crate::args::usage("scene"),
            b"-" => {
                if file.is_some() {
                    return Err(unexpected(
                        ctx,
                        "scene",
                        a,
                        "(one scene: FILE, or - for stdin)",
                    ));
                }
                file = Some(a.clone());
            }
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "scene", a)),
            _ => {
                if file.is_some() {
                    return Err(unexpected(
                        ctx,
                        "scene",
                        a,
                        "(one scene: FILE, or - for stdin)",
                    ));
                }
                file = Some(a.clone());
            }
        }
        i += 1;
    }
    let Some(file) = file else {
        return Err(ctx.usage_die(
            "scene: which scene: omabox scene FILE, or omabox scene - with it on stdin",
        ));
    };
    let scene = load(ctx, "scene", &file)?;
    select_box(ctx, &name)?;
    need_box(ctx)?;
    crate::whose::mark_used(ctx);
    if apply(ctx, &scene, timeout, Some(json))? {
        Ok(())
    } else {
        Err(Exit(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Result<Vec<Entry>, String> {
        parse(s.as_bytes())
    }

    #[test]
    fn a_scene_read() {
        let s = p(r#"{"windows": [
            {"name": "ed", "run": ["foot", "-T", "ed"], "workspace": 1},
            {"name": "notes", "run": "foot -T notes", "workspace": "special:notes"},
            {"name": "calc", "run": ["foot"], "workspace": "3", "floating": true, "size": "600x400", "at": "100,-80"},
            {"name": "clock", "run": ["foot"], "pinned": true}
        ]}"#)
        .unwrap();
        assert_eq!(s.len(), 4);
        assert_eq!(s[0].run, ["foot", "-T", "ed"]);
        assert_eq!(s[0].ws, Some(Ws::Id(1)));
        assert_eq!(
            (s[0].floating, s[0].floats(), s[0].pinned),
            (None, None, false)
        );
        assert_eq!(s[1].run, ["bash", "-c", "foot -T notes"]);
        assert_eq!(s[1].ws.as_ref().unwrap().name(), "special:notes");
        assert_eq!(s[2].ws.as_ref().unwrap().name(), "3");
        assert_eq!((s[2].size, s[2].at), (Some((600, 400)), Some((100, -80))));
        assert_eq!(s[2].floats(), Some(true));
        assert_eq!(
            (s[3].ws.clone(), s[3].pinned, s[3].floats()),
            (None, true, Some(true))
        );
    }

    #[test]
    fn scenes_refused() {
        let w = |body: &str| p(&format!(r#"{{"windows": [{body}]}}"#)).unwrap_err();
        for (text, want) in [
            ("", "not JSON"),
            ("[]", "a scene is a JSON object"),
            (r#"{"window": []}"#, r#"unknown key "window""#),
            (r#"{"windows": {}}"#, r#""windows" is a list"#),
            (r#"{"windows": []}"#, "lists no window"),
        ] {
            let e = p(text).unwrap_err();
            assert!(e.contains(want), "{text}: {e}");
        }
        for (body, want) in [
            ("1", "window 1 is not an object"),
            (
                r#"{"run": ["a"], "workspace": 1}"#,
                r#"window 1 has no "name""#,
            ),
            (
                r#"{"name": "a b", "run": ["a"], "workspace": 1}"#,
                r#"window 1: "name" is 1 to 64"#,
            ),
            (
                r#"{"name": "a", "workspace": 1}"#,
                r#"window 1 ("a") has no "run""#,
            ),
            (
                r#"{"name": "a", "run": [], "workspace": 1}"#,
                r#""run" is the command"#,
            ),
            (
                r#"{"name": "a", "run": ["x", 1], "workspace": 1}"#,
                r#""run" is the command"#,
            ),
            (
                r#"{"name": "a", "run": " ", "workspace": 1}"#,
                r#""run" is the command"#,
            ),
            (r#"{"name": "a", "run": ["x"]}"#, r#"has no "workspace""#),
            (
                r#"{"name": "a", "run": ["x"], "workspace": 0}"#,
                r#""workspace" is a number from 1"#,
            ),
            (
                r#"{"name": "a", "run": ["x"], "workspace": -1}"#,
                r#""workspace" is a number"#,
            ),
            (
                r#"{"name": "a", "run": ["x"], "workspace": 1.5}"#,
                r#""workspace" is a number"#,
            ),
            (
                r#"{"name": "a", "run": ["x"], "workspace": "special:"}"#,
                r#""workspace" is a number"#,
            ),
            (
                r#"{"name": "a", "run": ["x"], "workspace": "special:a'b"}"#,
                r#""workspace" is a number"#,
            ),
            (
                r#"{"name": "a", "run": ["x"], "workspace": "name:x"}"#,
                r#""workspace" is a number"#,
            ),
            (
                r#"{"name": "a", "run": ["x"], "workspace": 1, "float": true}"#,
                r#"window 1 ("a"): unknown key "float" (its keys: name, run"#,
            ),
            (
                r#"{"name": "a", "run": ["x"], "workspace": 1, "floating": "yes"}"#,
                r#""floating" is true or false"#,
            ),
            (
                r#"{"name": "a", "run": ["x"], "workspace": 1, "size": "600"}"#,
                r#""size" is "WxH""#,
            ),
            (
                r#"{"name": "a", "run": ["x"], "workspace": 1, "size": [600, 400]}"#,
                r#""size" is "WxH""#,
            ),
            (
                r#"{"name": "a", "run": ["x"], "workspace": 1, "at": "10"}"#,
                r#""at" is "X,Y""#,
            ),
            (
                r#"{"name": "a", "run": ["x"], "workspace": 1, "at": "1,2,3"}"#,
                r#""at" is "X,Y""#,
            ),
            (
                r#"{"name": "a", "run": ["x"], "workspace": 1, "pinned": true}"#,
                r#""pinned" or "workspace", not both"#,
            ),
            (
                r#"{"name": "a", "run": ["x"], "workspace": 1, "floating": false, "size": "9x9"}"#,
                r#"not with "floating": false"#,
            ),
            (
                r#"{"name": "a", "run": ["x"], "pinned": true, "floating": false}"#,
                r#"not with "floating": false"#,
            ),
        ] {
            let e = w(body);
            assert!(e.contains(want), "{body}: {e}");
        }
        let e = w(
            r#"{"name": "a", "run": ["x"], "workspace": 1}, {"name": "a", "run": ["y"], "workspace": 2}"#,
        );
        assert!(
            e.contains(r#"window 2 ("a"): two windows are named "a""#),
            "{e}"
        );
    }

    #[test]
    fn places_and_timeouts() {
        assert_eq!(parse_at("0,0"), Some((0, 0)));
        assert_eq!(parse_at("-20,1080"), Some((-20, 1080)));
        for bad in [
            "", "1", "1,", ",1", "1 ,2", "1,2,3", "+1,2", "1.5,2", "123456,1", "-,1",
        ] {
            assert_eq!(parse_at(bad), None, "{bad}");
        }
        assert_eq!(parse_timeout(b"2s"), Some(2000));
        assert_eq!(parse_timeout(b"3"), Some(3000));
        assert_eq!(parse_timeout(b"50ms"), None);
        assert_eq!(parse_timeout(b"11m"), None);
        assert_eq!(parse_timeout(b"x"), None);
    }

    fn entry(body: &str) -> Entry {
        p(&format!(r#"{{"windows": [{body}]}}"#)).unwrap().remove(0)
    }

    #[test]
    fn placement_only_what_differs() {
        let tiled: Value = serde_json::from_str(
            r#"{"address": "0xa", "workspace": {"id": 1, "name": "1"}, "floating": false, "pinned": false, "at": [10, 50], "size": [900, 700]}"#,
        )
        .unwrap();
        // On its workspace, nothing said of floating: nothing to do.
        let e = entry(r#"{"name": "a", "run": ["x"], "workspace": 1}"#);
        assert!(place_lua(&e, &tiled, true).is_empty());
        assert_eq!(problem(&e, &tiled), None);
        // Another workspace: one silent move.
        let e = entry(r#"{"name": "a", "run": ["x"], "workspace": "special:notes"}"#);
        let l = place_lua(&e, &tiled, true);
        assert_eq!(l.len(), 1, "{l:?}");
        assert!(l[0].contains("workspace = 'special:notes', follow = false, window = w"));
        assert_eq!(
            problem(&e, &tiled).unwrap(),
            "is on workspace 1, not special:notes"
        );
        // Floated, sized, placed, in that order; centred only with no place.
        let e = entry(
            r#"{"name": "a", "run": ["x"], "workspace": 2, "size": "600x400", "at": "100,80"}"#,
        );
        let l = place_lua(&e, &tiled, true).join("\n");
        let at = |s: &str| l.find(s).unwrap_or_else(|| panic!("{s} in {l}"));
        assert!(at("float({ action = 'enable'") < at("workspace = '2'"));
        assert!(at("workspace = '2'") < at("resize({ window = w, x = 600, y = 400 })"));
        assert!(at("resize(") < at("move({ x = 100, y = 80, window = w })"));
        assert!(!l.contains("center") && !l.contains("pin"));
        let e = entry(r#"{"name": "a", "run": ["x"], "workspace": 1, "size": "600x400"}"#);
        assert!(
            place_lua(&e, &tiled, true)
                .join("\n")
                .contains("center({ window = w })")
        );
        // Pinned: floated, then pinned last; a window as asked gets no chunk at all.
        let e = entry(r#"{"name": "a", "run": ["x"], "pinned": true}"#);
        let l = place_lua(&e, &tiled, true);
        assert_eq!(l.len(), 2, "{l:?}");
        assert!(l[0].contains("float({ action = 'enable'"));
        assert!(l[1].contains("pin({ action = 'enable'"));
        assert_eq!(problem(&e, &tiled).unwrap(), "is still tiled");
        let pinned: Value = serde_json::from_str(
            r#"{"address": "0xa", "workspace": {"id": 3, "name": "3"}, "floating": true, "pinned": true, "at": [100, 80], "size": [600, 400]}"#,
        )
        .unwrap();
        assert!(place_lua(&e, &pinned, false).is_empty());
        assert_eq!(problem(&e, &pinned), None);
        // The same window under an entry that is not pinned: unpinned first.
        let e = entry(
            r#"{"name": "a", "run": ["x"], "workspace": 3, "size": "600x400", "at": "100,80"}"#,
        );
        let l = place_lua(&e, &pinned, false);
        assert_eq!(l.len(), 1, "{l:?}");
        assert!(l[0].contains("pin({ action = 'disable'"));
        assert_eq!(problem(&e, &pinned).unwrap(), "is still pinned");
        // A size the app did not take, a place it is not at.
        let e = entry(
            r#"{"name": "a", "run": ["x"], "workspace": 3, "floating": true, "size": "5x5"}"#,
        );
        assert!(problem(&e, &pinned).unwrap().starts_with("is still pinned"));
        let floating: Value = serde_json::from_str(
            r#"{"address": "0xa", "workspace": {"id": 3, "name": "3"}, "floating": true, "pinned": false, "at": [100, 80], "size": [9, 20]}"#,
        )
        .unwrap();
        assert!(
            problem(&e, &floating)
                .unwrap()
                .starts_with("took 9x20, not 5x5")
        );
        let e = entry(r#"{"name": "a", "run": ["x"], "workspace": 3, "at": "0,0"}"#);
        assert_eq!(problem(&e, &floating).unwrap(), "is at 100,80, not 0,0");
        let e = entry(r#"{"name": "a", "run": ["x"], "workspace": 3, "floating": false}"#);
        assert!(place_lua(&e, &floating, false)[0].contains("float({ action = 'disable'"));
        assert_eq!(problem(&e, &floating).unwrap(), "is still floating");
    }
}
