//! `omabox tray [-b NAME] [--json] [--all]`, `tray menu ID`, `tray activate ID [--secondary]` and
//! `tray click ID LABEL [--index N]` (#343, finding 623): the box's tray items (StatusNotifierItem)
//! and their menus (com.canonical.dbusmenu), read and driven over the box's own session bus: busctl
//! run in the box (box_sys), never the host's bus. The shell's bar publishes no accessible items
//! (`tree --layer omarchy-bar` has none) and no place for an icon, so this is what an agent has of
//! a tray menu without pixels: its rows as `tree` words them, and a row clicked as the host does.

use crate::args::{optarg, unexpected, unknown_opt};
use crate::boxes::need_box;
use crate::cat;
use crate::exec::{ExecOpts, Redir, box_exec, pipe, read_all};
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::select::select_box;
use crate::shell;
use serde_json::{Map, Value, json};
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const SNI: &str = "org.kde.StatusNotifierItem";
const MENU: &str = "com.canonical.dbusmenu";

/// A tray item as its StatusNotifierItem properties say.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Item {
    /// The watcher's entry, `SERVICE/PATH`.
    pub entry: String,
    pub service: String,
    pub path: String,
    pub id: String,
    pub title: String,
    pub status: String,
    pub category: String,
    /// Its dbusmenu's object path, when it has one.
    pub menu: Option<String>,
    pub item_is_menu: bool,
    pub rows: Vec<Row>,
    /// Why its menu could not be read.
    pub menu_err: Option<String>,
}

/// One row of a dbusmenu, as GetLayout gives it.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: i64,
    /// Its label, the mnemonic's underscore off.
    pub label: String,
    pub separator: bool,
    /// `checkmark`, `radio` or empty.
    pub toggle: String,
    /// toggle-state: 0 off, 1 on, anything else indeterminate.
    pub state: i64,
    pub enabled: bool,
    pub visible: bool,
    pub submenu: bool,
    pub children: Vec<Row>,
}

/// A dbusmenu label as shown: `_` marks the next character as the mnemonic, `__` is one `_`.
pub fn strip_mnemonic(l: &str) -> String {
    let mut out = String::new();
    let mut it = l.chars().peekable();
    while let Some(c) = it.next() {
        if c == '_' {
            if it.peek() == Some(&'_') {
                out.push('_');
                it.next();
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// A watcher entry's service and object path: `:1.42/StatusNotifierItem`, or a bare service name
/// (an item registered by name), whose path is the spec's `/StatusNotifierItem`.
pub fn parse_entry(e: &str) -> (String, String) {
    match e.find('/') {
        Some(i) if i > 0 => (e[..i].to_string(), e[i..].to_string()),
        _ => (e.to_string(), "/StatusNotifierItem".to_string()),
    }
}

/// busctl's `{"type": T, "data": D}`: D.
fn data(v: &Value) -> &Value {
    v.get("data").unwrap_or(&Value::Null)
}

fn prop_str(p: &Map<String, Value>, k: &str) -> Option<String> {
    p.get(k).and_then(|v| data(v).as_str()).map(str::to_string)
}

/// An item from its GetAll answer (busctl's JSON of `a{sv}`).
pub fn item_from_props(entry: &str, all: &Value) -> Item {
    let (service, path) = parse_entry(entry);
    let empty = Map::new();
    let p = data(all)
        .get(0)
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let menu = prop_str(p, "Menu").filter(|m| !m.is_empty() && m != "/");
    Item {
        entry: entry.to_string(),
        service,
        path,
        id: prop_str(p, "Id").unwrap_or_default(),
        title: prop_str(p, "Title").unwrap_or_default(),
        status: prop_str(p, "Status").unwrap_or_default(),
        category: prop_str(p, "Category").unwrap_or_default(),
        menu,
        item_is_menu: p
            .get("ItemIsMenu")
            .and_then(|v| data(v).as_bool())
            .unwrap_or(false),
        rows: Vec::new(),
        menu_err: None,
    }
}

/// One `(ia{sv}av)` node as busctl writes it: `[id, {props}, [{"type", "data": node}, ...]]`.
fn row_from(node: &Value) -> Option<Row> {
    let id = node.get(0)?.as_i64()?;
    let empty = Map::new();
    let p = node.get(1).and_then(Value::as_object).unwrap_or(&empty);
    let children: Vec<Row> = node
        .get(2)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|c| row_from(data(c))).collect())
        .unwrap_or_default();
    let b = |k: &str| p.get(k).and_then(|v| data(v).as_bool()).unwrap_or(true);
    Some(Row {
        id,
        label: strip_mnemonic(&prop_str(p, "label").unwrap_or_default()),
        separator: prop_str(p, "type").as_deref() == Some("separator"),
        toggle: prop_str(p, "toggle-type").unwrap_or_default(),
        state: p
            .get("toggle-state")
            .and_then(|v| data(v).as_i64())
            .unwrap_or(-1),
        enabled: b("enabled"),
        visible: b("visible"),
        submenu: prop_str(p, "children-display").as_deref() == Some("submenu")
            || !children.is_empty(),
        children,
    })
}

/// GetLayout's answer (busctl's JSON of `u(ia{sv}av)`): its revision and the root's rows.
pub fn layout(v: &Value) -> Option<(u64, Vec<Row>)> {
    let d = data(v);
    let rev = d.get(0)?.as_u64()?;
    Some((rev, row_from(d.get(1)?)?.children))
}

/// A row's role, in `tree`'s words for a menu's accessible nodes.
fn role(r: &Row) -> &'static str {
    if r.separator {
        "separator"
    } else if r.submenu {
        "menu"
    } else if r.toggle == "checkmark" {
        "check menu item"
    } else if r.toggle == "radio" {
        "radio menu item"
    } else {
        "menu item"
    }
}

/// A row as a line: `check menu item "Sync" [checked]`, as `tree` words a node.
pub fn row_line(r: &Row) -> String {
    if r.separator {
        return "separator".to_string();
    }
    let mut states: Vec<&str> = Vec::new();
    if !r.visible {
        states.push("hidden");
    }
    if !r.enabled {
        states.push("disabled");
    }
    if !r.submenu && (r.toggle == "checkmark" || r.toggle == "radio") {
        states.push(match r.state {
            1 => "checked",
            0 => "unchecked",
            _ => "mixed",
        });
    }
    let mut s = format!("{} \"{}\"", role(r), r.label);
    if !states.is_empty() {
        s.push_str(&format!(" [{}]", states.join(" ")));
    }
    s
}

fn status_word(s: &str) -> String {
    match s {
        "NeedsAttention" => "needs attention".to_string(),
        _ => s.to_lowercase(),
    }
}

/// An item's own line: `ID "TITLE" [active]`, `(no menu)` when it has none on the bus.
pub fn item_line(it: &Item) -> String {
    let mut s = format!("{} \"{}\" [{}]", it.id, it.title, status_word(&it.status));
    if it.menu.is_none() {
        s.push_str(" (no menu)");
    }
    s
}

fn rows_text(rows: &[Row], depth: usize, all: bool, s: &mut String) {
    for r in rows {
        if !r.visible && !all {
            continue;
        }
        s.push_str(&"  ".repeat(depth));
        s.push_str(&row_line(r));
        s.push('\n');
        rows_text(&r.children, depth + 1, all, s);
    }
}

/// An item and its menu as text: its line, then its rows indented two spaces a level; hidden rows
/// only with ALL.
pub fn item_text(it: &Item, all: bool) -> String {
    let mut s = item_line(it);
    s.push('\n');
    rows_text(&it.rows, 1, all, &mut s);
    s
}

fn row_json(r: &Row) -> Value {
    let checked = match (r.toggle.as_str(), r.state) {
        ("checkmark" | "radio", 1) => json!(true),
        ("checkmark" | "radio", 0) => json!(false),
        _ => Value::Null,
    };
    json!({
        "id": r.id,
        "label": r.label,
        "role": role(r),
        "enabled": r.enabled,
        "visible": r.visible,
        "toggle": if r.toggle.is_empty() { Value::Null } else { json!(r.toggle) },
        "checked": checked,
        "rows": r.children.iter().map(row_json).collect::<Vec<_>>(),
    })
}

/// An item as `--json` gives it: every row, hidden ones too (`visible`).
pub fn item_json(it: &Item) -> Value {
    let mut v = json!({
        "id": it.id,
        "title": it.title,
        "status": it.status,
        "category": it.category,
        "service": it.service,
        "path": it.path,
        "menu": it.menu,
        "item_is_menu": it.item_is_menu,
        "rows": it.rows.iter().map(row_json).collect::<Vec<_>>(),
    });
    if let Some(e) = &it.menu_err {
        v["menu_error"] = json!(e);
    }
    v
}

/// The item WANT names: its Id exactly, else its Title (any case), else its watcher entry or
/// service, else a part of its Id or Title (any case). Err: the ones a tier matched several of
/// (empty: none matched).
pub fn pick_item(items: &[Item], want: &str) -> Result<usize, Vec<usize>> {
    let w = want.to_lowercase();
    let tiers: [&dyn Fn(&Item) -> bool; 4] = [
        &|i| i.id == want,
        &|i| i.title.to_lowercase() == w,
        &|i| i.entry == want || i.service == want,
        &|i| i.id.to_lowercase().contains(&w) || i.title.to_lowercase().contains(&w),
    ];
    for t in tiers {
        let hits: Vec<usize> = (0..items.len()).filter(|&n| t(&items[n])).collect();
        match hits.len() {
            0 => continue,
            1 => return Ok(hits[0]),
            _ => return Err(hits),
        }
    }
    Err(Vec::new())
}

/// A row with where it sits: its line after its submenus', the submenus' ids (outermost first), and
/// whether it or one of them is hidden, or disabled.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub row: Row,
    pub line: String,
    pub parents: Vec<i64>,
    pub hidden: bool,
    pub disabled: bool,
}

fn flatten(rows: &[Row], up: &Found, all: &mut Vec<Found>) {
    for r in rows {
        let line = if up.line.is_empty() {
            row_line(r)
        } else {
            format!("{} > {}", up.line, row_line(r))
        };
        let f = Found {
            row: Row {
                children: Vec::new(),
                ..r.clone()
            },
            line,
            parents: up.parents.clone(),
            hidden: up.hidden || !r.visible,
            disabled: up.disabled || !r.enabled,
        };
        all.push(f.clone());
        let mut down = f;
        down.parents.push(r.id);
        flatten(&r.children, &down, all);
    }
}

/// Every row of ROWS (separators left out), each with where it sits.
pub fn found_rows(rows: &[Row]) -> Vec<Found> {
    let top = Found {
        row: Row {
            id: 0,
            label: String::new(),
            separator: false,
            toggle: String::new(),
            state: -1,
            enabled: true,
            visible: true,
            submenu: true,
            children: Vec::new(),
        },
        line: String::new(),
        parents: Vec::new(),
        hidden: false,
        disabled: false,
    };
    let mut all = Vec::new();
    flatten(rows, &top, &mut all);
    all.retain(|f| !f.row.separator);
    all
}

/// What `tray click` finds for LABEL in ROWS.
#[derive(Debug, PartialEq)]
pub enum Pick {
    One(Found),
    None,
    /// Several shown: pick one with --index.
    Several(Vec<Found>),
    /// --index past the N there are.
    Past(usize),
}

/// The row LABEL names (any case; an exact label wins over a part of one; of several, the shown
/// ones), or the INDEXth (from 1) of several.
pub fn pick_row(rows: &[Row], label: &str, index: Option<usize>) -> Pick {
    let w = label.to_lowercase();
    let all = found_rows(rows);
    let mut hits: Vec<Found> = all
        .iter()
        .filter(|f| f.row.label.to_lowercase() == w)
        .cloned()
        .collect();
    if hits.is_empty() {
        hits = all
            .into_iter()
            .filter(|f| f.row.label.to_lowercase().contains(&w))
            .collect();
    }
    if hits.len() > 1 && hits.iter().any(|f| !f.hidden) {
        hits.retain(|f| !f.hidden);
    }
    match (hits.len(), index) {
        (0, _) => Pick::None,
        (n, Some(i)) if i > n => Pick::Past(n),
        (_, Some(i)) => Pick::One(hits.swap_remove(i - 1)),
        (1, None) => Pick::One(hits.swap_remove(0)),
        _ => Pick::Several(hits),
    }
}

/// The row of ROWS with that id.
fn by_id(rows: &[Row], id: i64) -> Option<&Row> {
    for r in rows {
        if r.id == id {
            return Some(r);
        }
        if let Some(c) = by_id(&r.children, id) {
            return Some(c);
        }
    }
    None
}

/// busctl in the box, on its own session bus: its JSON answer (Null for none), or its error words.
fn bus(ctx: &mut Ctx, words: &[&str]) -> Res<Result<Value, String>> {
    let argv: Vec<Vec<u8>> = [
        "/usr/bin/busctl",
        "--user",
        "--json=short",
        "--timeout=5",
        "--",
    ]
    .iter()
    .chain(words)
    .map(|w| w.as_bytes().to_vec())
    .collect();
    let (r1, w1) = pipe().map_err(|_| ctx.die("tray: no pipe"))?;
    let (r2, w2) = pipe().map_err(|_| ctx.die("tray: no pipe"))?;
    let o = std::thread::spawn(move || read_all(r1, Some(16 << 20)));
    let e = std::thread::spawn(move || read_all(r2, Some(64 << 10)));
    let rc = {
        let _e = Redir::to(2, w2.as_raw_fd());
        box_exec(
            ctx,
            &argv,
            ExecOpts {
                sys: true,
                stdin_null: true,
                stdout: Some(w1.as_raw_fd()),
                ..ExecOpts::default()
            },
        )
    };
    drop((w1, w2));
    let (o, e) = (o.join().unwrap_or_default(), e.join().unwrap_or_default());
    // What box_exec said of a box it could not enter went into the pipe with busctl's words.
    let rc = rc.inspect_err(|_| {
        let _ = std::io::Write::write_all(&mut std::io::stderr(), &e);
    })?;
    if rc != 0 {
        let e = String::from_utf8_lossy(&shell::subst(&e)).into_owned();
        return Ok(Err(if e.is_empty() {
            format!("busctl exited {rc}")
        } else {
            e
        }));
    }
    let o = shell::subst(&o);
    if o.is_empty() {
        return Ok(Ok(Value::Null));
    }
    Ok(serde_json::from_slice(&o).map_err(|e| format!("busctl's answer did not parse: {e}")))
}

/// The watcher's entries, or why there are none to read (no watcher: the box's tray is off).
fn entries(ctx: &mut Ctx) -> Res<Vec<String>> {
    let has = bus(
        ctx,
        &[
            "call",
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            "s",
            WATCHER,
        ],
    )?;
    if !has
        .as_ref()
        .is_ok_and(|v| data(v).get(0) == Some(&json!(true)))
    {
        if let Err(e) = has {
            return Err(ctx.die(cat!("tray: box '", ctx.name, "''s bus did not answer: ", e)));
        }
        return Err(ctx.die(cat!(
            "tray: box '",
            ctx.name,
            "' has no tray: nothing owns ",
            WATCHER,
            " on its bus (a box up with --no-shell, or a bar with no omarchy.tray: up --stock-bar has one)"
        )));
    }
    let v = bus(
        ctx,
        &[
            "get-property",
            WATCHER,
            "/StatusNotifierWatcher",
            WATCHER,
            "RegisteredStatusNotifierItems",
        ],
    )?
    .map_err(|e| ctx.die(cat!("tray: the box's tray watcher did not answer: ", e)))?;
    Ok(data(&v)
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| e.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default())
}

/// Every item on the box's tray, those that answer (one that does not is said and left out).
fn items(ctx: &mut Ctx) -> Res<Vec<Item>> {
    let mut list = Vec::new();
    for e in entries(ctx)? {
        let (svc, path) = parse_entry(&e);
        match bus(
            ctx,
            &[
                "call",
                &svc,
                &path,
                "org.freedesktop.DBus.Properties",
                "GetAll",
                "s",
                SNI,
            ],
        )? {
            Ok(v) => list.push(item_from_props(&e, &v)),
            Err(why) => say(cat!("tray: ", e, " did not answer, left out: ", why)),
        }
    }
    Ok(list)
}

/// IT's menu read into its rows (its revision); AboutToShow on the root first when OPEN, as a host
/// does when it opens the menu (an app may fill it then).
fn read_menu(ctx: &mut Ctx, it: &mut Item, open: bool) -> Res<Option<u64>> {
    let Some(m) = it.menu.clone() else {
        return Ok(None);
    };
    if open {
        let _ = bus(
            ctx,
            &["call", &it.service, &m, MENU, "AboutToShow", "i", "0"],
        )?;
    }
    match bus(
        ctx,
        &[
            "call",
            &it.service,
            &m,
            MENU,
            "GetLayout",
            "iias",
            "0",
            "-1",
            "0",
        ],
    )? {
        Ok(v) => match layout(&v) {
            Some((rev, rows)) => {
                it.rows = rows;
                it.menu_err = None;
                Ok(Some(rev))
            }
            None => {
                it.menu_err = Some("its layout did not parse".to_string());
                Ok(None)
            }
        },
        Err(why) => {
            it.menu_err = Some(why);
            Ok(None)
        }
    }
}

/// The item ID names, its menu not read yet.
fn the_item(ctx: &mut Ctx, want: &str) -> Res<Item> {
    let mut list = items(ctx)?;
    match pick_item(&list, want) {
        Ok(n) => Ok(list.swap_remove(n)),
        Err(hits) if hits.is_empty() => {
            if list.is_empty() {
                say(cat!("tray: box '", ctx.name, "' has no tray items"));
            } else {
                say(cat!("tray: no tray item is \"", want, "\"; the box's:"));
                for i in &list {
                    eprintln!("  {}", item_line(i));
                }
            }
            Err(Exit(1))
        }
        Err(hits) => {
            say(cat!(
                "tray: \"",
                want,
                "\" names ",
                hits.len().to_string(),
                " tray items: name one by its id, or by its bus name (each line's last word)"
            ));
            for n in hits {
                eprintln!("  {} {}", item_line(&list[n]), list[n].entry);
            }
            Err(Exit(2))
        }
    }
}

/// Whether `tray WORDS` (the words after `tray`) acts, for `shot --after` and `record -- ACTION`:
/// menu, activate and click do; a listing only reads.
pub fn acts(words: &[Vec<u8>]) -> bool {
    words
        .iter()
        .find(|w| !w.starts_with(b"-"))
        .is_some_and(|w| matches!(w.as_slice(), b"menu" | b"activate" | b"click"))
}

/// What an action check says of a `tray` that does not act.
pub const LISTING_NOTE: &str = " (a listing: tray menu, activate or click act)";

enum Sub {
    List,
    Menu(String),
    Activate(String),
    Click(String, String),
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let mut name = Vec::new();
    let (mut json, mut all, mut secondary) = (false, false, false);
    let mut index: Option<usize> = None;
    let mut words: Vec<String> = Vec::new();
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
            b"--json" => json = true,
            b"--all" => all = true,
            b"--secondary" => secondary = true,
            b"--index" => {
                optarg(ctx, a, v)?;
                let n = std::str::from_utf8(&args[i + 1])
                    .ok()
                    .and_then(|s| s.parse::<usize>().ok())
                    .filter(|&n| n >= 1);
                let Some(n) = n else {
                    return Err(
                        ctx.usage_die(cat!("tray: --index is a number from 1, got ", args[i + 1]))
                    );
                };
                index = Some(n);
                i += 1;
            }
            b"-h" | b"--help" => return crate::args::usage("tray"),
            _ if a.starts_with(b"-") && words.len() < 2 => {
                return Err(unknown_opt(ctx, "tray", a));
            }
            _ => words.push(String::from_utf8_lossy(a).into_owned()),
        }
        i += 1;
    }
    let mut w = words.into_iter();
    let first = w.next();
    let sub = match first.as_deref() {
        None => Sub::List,
        Some(s @ ("menu" | "activate" | "click")) => {
            let Some(id) = w.next().filter(|x| !x.is_empty()) else {
                return Err(ctx.usage_die(cat!(
                    "tray ",
                    s,
                    ": which item: its id (omabox tray lists them)"
                )));
            };
            match s {
                "menu" => Sub::Menu(id),
                "activate" => Sub::Activate(id),
                _ => {
                    let Some(l) = w.next().filter(|x| !x.is_empty()) else {
                        return Err(ctx.usage_die(
                            "tray click ID LABEL: the row's label (omabox tray menu ID lists them)",
                        ));
                    };
                    Sub::Click(id, l)
                }
            }
        }
        Some(x) => {
            return Err(unexpected(
                ctx,
                "tray",
                x.as_bytes(),
                "(tray, tray menu ID, tray activate ID, tray click ID LABEL)",
            ));
        }
    };
    if let Some(x) = w.next() {
        return Err(unexpected(ctx, "tray", x.as_bytes(), ""));
    }
    let only = |ok: bool, opt: &str, ctx: &Ctx| -> Res<()> {
        if ok {
            Ok(())
        } else {
            Err(ctx.usage_die(cat!("tray: ", opt, " is not for this form")))
        }
    };
    only(
        index.is_none() || matches!(sub, Sub::Click(..)),
        "--index",
        ctx,
    )?;
    only(
        !secondary || matches!(sub, Sub::Activate(_)),
        "--secondary",
        ctx,
    )?;
    only(
        !(json || all) || matches!(sub, Sub::List | Sub::Menu(_)),
        "--json/--all",
        ctx,
    )?;
    select_box(ctx, &name)?;
    need_box(ctx)?;
    match sub {
        Sub::List => list(ctx, json, all),
        Sub::Menu(id) => menu(ctx, &id, json, all),
        Sub::Activate(id) => activate(ctx, &id, secondary),
        Sub::Click(id, l) => click(ctx, &id, &l, index),
    }
}

fn list(ctx: &mut Ctx, json: bool, all: bool) -> Res<()> {
    let mut list = items(ctx)?;
    for it in list.iter_mut() {
        read_menu(ctx, it, false)?;
        if let Some(e) = &it.menu_err {
            say(cat!("tray: ", it.id, "'s menu did not answer: ", e));
        }
    }
    if json {
        let v = Value::Array(list.iter().map(item_json).collect());
        return out(cat!(crate::json::compact(&v), "\n"));
    }
    if list.is_empty() {
        say(cat!("tray: box '", ctx.name, "' has no tray items"));
        return Ok(());
    }
    let mut s = String::new();
    for it in &list {
        s.push_str(&item_text(it, all));
    }
    out(s)
}

fn menu(ctx: &mut Ctx, id: &str, json: bool, all: bool) -> Res<()> {
    let mut it = the_item(ctx, id)?;
    let Some(_) = it.menu.clone() else {
        // What a host does for an item with no menu of its own on the bus: asks it to show one.
        let r = bus(
            ctx,
            &[
                "call",
                &it.service,
                &it.path,
                SNI,
                "ContextMenu",
                "ii",
                "0",
                "0",
            ],
        )?;
        if let Err(e) = r {
            return Err(ctx.die(cat!(
                "tray: ",
                it.id,
                " has no menu on the bus, and did not take ContextMenu: ",
                e
            )));
        }
        say(cat!(
            "tray: ",
            it.id,
            " has no menu on the bus: asked it to show its own (ContextMenu); it opens as a window or popup of the app's (omabox windows, omabox tree)"
        ));
        return Ok(());
    };
    read_menu(ctx, &mut it, true)?;
    if let Some(e) = &it.menu_err {
        return Err(ctx.die(cat!("tray: ", it.id, "'s menu did not answer: ", e)));
    }
    if json {
        return out(cat!(crate::json::compact(&item_json(&it)), "\n"));
    }
    out(item_text(&it, all))
}

fn activate(ctx: &mut Ctx, id: &str, secondary: bool) -> Res<()> {
    let it = the_item(ctx, id)?;
    if it.item_is_menu && !secondary {
        return Err(ctx.die(cat!(
            "tray: ",
            it.id,
            " is a menu only (ItemIsMenu): a click on it opens its menu, not Activate (omabox tray menu ",
            it.id,
            ")"
        )));
    }
    let (m, what) = if secondary {
        ("SecondaryActivate", "a middle click")
    } else {
        ("Activate", "a left click")
    };
    if let Err(e) = bus(
        ctx,
        &["call", &it.service, &it.path, SNI, m, "ii", "0", "0"],
    )? {
        return Err(ctx.die(cat!("tray: ", it.id, " did not take ", m, ": ", e)));
    }
    say(cat!("tray: ", it.id, ": ", m, " (", what, " on its icon)"));
    Ok(())
}

fn click(ctx: &mut Ctx, id: &str, label: &str, index: Option<usize>) -> Res<()> {
    let mut it = the_item(ctx, id)?;
    let Some(m) = it.menu.clone() else {
        return Err(ctx.die(cat!(
            "tray: ",
            it.id,
            " has no menu on the bus to click in (omabox tray menu ",
            it.id,
            " asks it to show its own)"
        )));
    };
    let rev = read_menu(ctx, &mut it, true)?;
    if let Some(e) = &it.menu_err {
        return Err(ctx.die(cat!("tray: ", it.id, "'s menu did not answer: ", e)));
    }
    let f = match pick_row(&it.rows, label, index) {
        Pick::One(f) => f,
        Pick::None => {
            return Err(ctx.die(cat!(
                "tray: no row of ",
                it.id,
                "'s menu is named \"",
                label,
                "\" (omabox tray menu ",
                it.id,
                " lists them)"
            )));
        }
        Pick::Past(n) => {
            return Err(ctx.usage_die(cat!(
                "tray: --index past the ",
                n.to_string(),
                " rows named \"",
                label,
                "\""
            )));
        }
        Pick::Several(hits) => {
            say(cat!(
                "tray: ",
                hits.len().to_string(),
                " rows of ",
                it.id,
                "'s menu are named \"",
                label.to_lowercase(),
                "\": pick one with --index N"
            ));
            for (n, h) in hits.iter().enumerate() {
                eprintln!("  {}: {}", n + 1, h.line);
            }
            return Err(Exit(2));
        }
    };
    let why = if f.hidden {
        Some("hidden (itself or its submenu): the menu does not show it")
    } else if f.disabled {
        Some("disabled (itself or its submenu)")
    } else if f.row.submenu {
        Some("it opens a submenu, nothing to click: click a row in it")
    } else {
        None
    };
    if let Some(w) = why {
        return Err(ctx.die(cat!("tray: ", it.id, ": ", f.line, ": ", w)));
    }
    // Each submenu opened on the way, as a host does before it shows one.
    for p in &f.parents {
        let _ = bus(
            ctx,
            &[
                "call",
                &it.service,
                &m,
                MENU,
                "AboutToShow",
                "i",
                &p.to_string(),
            ],
        )?;
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0);
    let r = bus(
        ctx,
        &[
            "call",
            &it.service,
            &m,
            MENU,
            "Event",
            "isvu",
            &f.row.id.to_string(),
            "clicked",
            "i",
            "0",
            &ts.to_string(),
        ],
    )?;
    if let Err(e) = r {
        return Err(ctx.die(cat!("tray: ", it.id, " did not take the click: ", e)));
    }
    say(cat!("tray: ", it.id, ": clicked ", f.line));
    // The row as the app has it after: its menu's revision moves when the click changed it (1 s at
    // most); an app that quit on it has no menu to read.
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let now = read_menu(ctx, &mut it, false)?;
        if now.is_none() {
            return Ok(());
        }
        if now != rev || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    match by_id(&it.rows, f.row.id) {
        Some(r) => out(cat!(row_line(r), "\n")),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // GetLayout as busctl --json=short gives it, from the suite's fixture (trimmed).
    const LAYOUT: &str = r#"{"type":"u(ia{sv}av)","data":[2,[0,{"children-display":{"type":"s","data":"submenu"}},[
      {"type":"(ia{sv}av)","data":[1,{"label":{"type":"s","data":"_Show window"}},[]]},
      {"type":"(ia{sv}av)","data":[2,{"label":{"type":"s","data":"Sync"},"toggle-type":{"type":"s","data":"checkmark"},"toggle-state":{"type":"i","data":1}},[]]},
      {"type":"(ia{sv}av)","data":[3,{"type":{"type":"s","data":"separator"}},[]]},
      {"type":"(ia{sv}av)","data":[4,{"label":{"type":"s","data":"Delete all"},"enabled":{"type":"b","data":false}},[]]},
      {"type":"(ia{sv}av)","data":[5,{"label":{"type":"s","data":"Stations"},"children-display":{"type":"s","data":"submenu"}},[
        {"type":"(ia{sv}av)","data":[7,{"label":{"type":"s","data":"Jazz"},"toggle-type":{"type":"s","data":"radio"},"toggle-state":{"type":"i","data":1}},[]]},
        {"type":"(ia{sv}av)","data":[8,{"label":{"type":"s","data":"Open"},"toggle-type":{"type":"s","data":"radio"},"toggle-state":{"type":"i","data":0}},[]]}]]},
      {"type":"(ia{sv}av)","data":[6,{"label":{"type":"s","data":"Hidden"},"visible":{"type":"b","data":false}},[]]},
      {"type":"(ia{sv}av)","data":[9,{"label":{"type":"s","data":"Open"}},[]]}]]]}"#;

    fn rows() -> Vec<Row> {
        layout(&serde_json::from_str(LAYOUT).unwrap()).unwrap().1
    }

    #[test]
    fn which_act() {
        let w = |s: &str| -> Vec<Vec<u8>> { s.split(' ').map(|x| x.as_bytes().to_vec()).collect() };
        assert!(acts(&w("click x y")) && acts(&w("--json menu x")) && acts(&w("activate x")));
        assert!(!acts(&w("--json")) && !acts(&[]) && !acts(&w("bogus")));
    }

    #[test]
    fn mnemonics_off() {
        assert_eq!(strip_mnemonic("_Show window"), "Show window");
        assert_eq!(strip_mnemonic("Save __as"), "Save _as");
        assert_eq!(strip_mnemonic("plain"), "plain");
    }

    #[test]
    fn entries_split() {
        assert_eq!(
            parse_entry(":1.42/StatusNotifierItem"),
            (":1.42".into(), "/StatusNotifierItem".into())
        );
        assert_eq!(
            parse_entry(":1.9/org/ayatana/NotificationItem/x"),
            (":1.9".into(), "/org/ayatana/NotificationItem/x".into())
        );
        assert_eq!(
            parse_entry("org.kde.StatusNotifierItem-7-1"),
            (
                "org.kde.StatusNotifierItem-7-1".into(),
                "/StatusNotifierItem".into()
            )
        );
    }

    #[test]
    fn layout_as_text() {
        let it = Item {
            id: "demo".into(),
            title: "Demo".into(),
            status: "Active".into(),
            menu: Some("/MenuBar".into()),
            rows: rows(),
            ..Item::default()
        };
        assert_eq!(
            item_text(&it, false),
            "demo \"Demo\" [active]\n  menu item \"Show window\"\n  check menu item \"Sync\" [checked]\n  separator\n  menu item \"Delete all\" [disabled]\n  menu \"Stations\"\n    radio menu item \"Jazz\" [checked]\n    radio menu item \"Open\" [unchecked]\n  menu item \"Open\"\n"
        );
        assert!(item_text(&it, true).contains("\n  menu item \"Hidden\" [hidden]\n"));
        let j = item_json(&it);
        assert_eq!(j["rows"][1]["checked"], json!(true));
        assert_eq!(j["rows"][0]["checked"], Value::Null);
        assert_eq!(j["rows"][4]["rows"][1]["role"], json!("radio menu item"));
        let none = Item {
            status: "NeedsAttention".into(),
            ..Item::default()
        };
        assert_eq!(item_line(&none), " \"\" [needs attention] (no menu)");
    }

    #[test]
    fn props_read() {
        let v: Value = serde_json::from_str(r#"{"type":"a{sv}","data":[{"Id":{"type":"s","data":"x"},"Title":{"type":"s","data":"X"},"Status":{"type":"s","data":"Passive"},"Menu":{"type":"o","data":"/"},"ItemIsMenu":{"type":"b","data":true}}]}"#).unwrap();
        let it = item_from_props(":1.5/StatusNotifierItem", &v);
        assert_eq!(
            (it.id.as_str(), it.status.as_str(), it.menu, it.item_is_menu),
            ("x", "Passive", None, true)
        );
        assert_eq!(it.service, ":1.5");
    }

    #[test]
    fn items_picked() {
        let mk = |id: &str, title: &str, e: &str| Item {
            id: id.into(),
            title: title.into(),
            entry: e.into(),
            service: parse_entry(e).0,
            ..Item::default()
        };
        let l = vec![
            mk("app-one", "Tray demo", ":1.1/S"),
            mk("app-two", "Plain demo", ":1.2/S"),
            mk("app-one", "Again", ":1.3/S"),
        ];
        assert_eq!(pick_item(&l, "app-two"), Ok(1));
        assert_eq!(pick_item(&l, "app-one"), Err(vec![0, 2]));
        assert_eq!(pick_item(&l, "plain DEMO"), Ok(1));
        assert_eq!(pick_item(&l, ":1.3"), Ok(2));
        assert_eq!(pick_item(&l, "demo"), Err(vec![0, 1]));
        assert_eq!(pick_item(&l, "two"), Ok(1));
        assert_eq!(pick_item(&l, "nothing"), Err(vec![]));
    }

    #[test]
    fn rows_picked() {
        let r = rows();
        let Pick::One(f) = pick_row(&r, "JAZZ", None) else {
            panic!()
        };
        assert_eq!(
            (f.row.id, f.parents.clone(), f.line.as_str()),
            (
                7,
                vec![5],
                "menu \"Stations\" > radio menu item \"Jazz\" [checked]"
            )
        );
        let Pick::One(f) = pick_row(&r, "show", None) else {
            panic!()
        };
        assert_eq!(f.row.id, 1);
        // Exact over a part: "sync" is one; "open" two exact ones, numbered for --index.
        let Pick::Several(h) = pick_row(&r, "open", None) else {
            panic!()
        };
        assert_eq!(h.iter().map(|f| f.row.id).collect::<Vec<_>>(), vec![8, 9]);
        assert!(matches!(pick_row(&r, "open", Some(2)), Pick::One(f) if f.row.id == 9));
        assert_eq!(pick_row(&r, "open", Some(3)), Pick::Past(2));
        assert_eq!(pick_row(&r, "nothing", None), Pick::None);
        let Pick::One(f) = pick_row(&r, "delete", None) else {
            panic!()
        };
        assert!(f.disabled && !f.hidden);
        let Pick::One(f) = pick_row(&r, "hidden", None) else {
            panic!()
        };
        assert!(f.hidden);
        let Pick::One(f) = pick_row(&r, "stations", None) else {
            panic!()
        };
        assert!(f.row.submenu);
        // Separators are not rows to click.
        assert!(found_rows(&r).iter().all(|f| !f.row.separator));
        assert_eq!(found_rows(&r).len(), 8);
    }
}
