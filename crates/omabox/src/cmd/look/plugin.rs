//! `look plugin DIR`: the plugin in the box's bar, its state, the bar's shot and the widget's text;
//! with --open its panel summoned, shot and read.

use super::*;

/// The problem a look plugin --open reports when the shell's summon of ID failed, SAID its words.
/// `no live bar widget` is Omarchy's shell finding no widget of the plugin on the bar with an
/// `open()`, `close()` and `opened` (Bar.qml's findPanelWidget), which alone says nothing to do
/// (#478): ON_BAR, the widget has a slot in the bar, so it lacks those: no panel for --open to show;
/// else it has no slot.
/// OWN: the box that was up, whose shell log can still be read.
pub(super) fn summon_problem(id: &[u8], said: &[u8], on_bar: bool, own: Option<&[u8]>) -> Vec<u8> {
    let said = said.strip_suffix(b"\n").unwrap_or(said);
    // (the shell log's warning already starts `summon: `)
    let head = if said.starts_with(b"summon: ") {
        said.to_vec()
    } else {
        cat!("summon ", id, ": ", said)
    };
    if !crate::exit::contains(said, b"no live bar widget") {
        return head;
    }
    if on_bar {
        return cat!(
            head,
            " (the widget is on the bar but is no panel: it lacks open(), close() and opened, which qs.Ui's Panel has; --open shows nothing)"
        );
    }
    let log = match own {
        Some(n) => cat!("omabox log -b ", shell::quote_q(n), " shell"),
        None => b"look --keep-box, then omabox log -b BOX shell".to_vec(),
    };
    cat!(
        head,
        " (no widget of it is on the bar for summon to open: see its plugin: line; the shell's log: ",
        log,
        ")"
    )
}

/// The bar's strip of the first monitor, from what Hyprland reserves for it (`monitors`' reserved:
/// left, top, right, bottom): the largest edge, as `X,Y WxH`; None when nothing is reserved.
pub fn bar_strip(monitors: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(monitors).ok()?;
    let m = v.as_array()?.first()?;
    let n = |k: &str| m.get(k).and_then(Value::as_i64).unwrap_or(0);
    let (w, h) = (n("width"), n("height"));
    let r: Vec<i64> = m
        .get("reserved")?
        .as_array()?
        .iter()
        .map(|x| x.as_i64().unwrap_or(0))
        .collect();
    let (l, t, rt, b) = (*r.first()?, *r.get(1)?, *r.get(2)?, *r.get(3)?);
    let max = l.max(t).max(rt).max(b);
    if max <= 0 {
        return None;
    }
    Some(if max == t {
        format!("0,0 {w}x{t}")
    } else if max == b {
        format!("0,{} {w}x{b}", h - b)
    } else if max == l {
        format!("0,0 {l}x{h}")
    } else {
        format!("{},0 {rt}x{h}", w - rt)
    })
}

/// The lines of the shell's WARNINGS (shell_warnings_of's) that name plugin ID.
pub fn plugin_warnings(warnings: &[u8], id: &[u8]) -> Vec<Vec<u8>> {
    warnings
        .split(|&c| c == b'\n')
        .filter(|l| !id.is_empty() && l.windows(id.len()).any(|w| w == id))
        .map(<[u8]>::to_vec)
        .collect()
}

/// The bar's layer in an Omarchy box (4.5 and 4.0 alike).
pub(super) const BAR_NS: &[u8] = b"omarchy-bar";
/// A panel's text items shown at most (the rest counted).
pub(super) const PANEL_TEXT_MAX: usize = 60;

/// `tree --layer NS --json`'s text (#467): each node under the frame that has a name, as
/// (name, x, y, w, h, cut) in the screen's coordinates, in the tree's order (cut: its `cut` state,
/// only part of it shows); and the frame's x, y.
pub fn tree_texts(json: &[u8]) -> (Vec<TextItem>, (i64, i64)) {
    let v: Value = serde_json::from_slice(json).unwrap_or(Value::Null);
    let n = |e: &Value, k: &str| e.get(k).and_then(Value::as_i64).unwrap_or(0);
    let mut origin = (0, 0);
    let mut items = Vec::new();
    for e in v.as_array().map(Vec::as_slice).unwrap_or(&[]) {
        if n(e, "depth") == 0 {
            origin = (n(e, "x"), n(e, "y"));
            continue;
        }
        let name = e.get("name").and_then(Value::as_str).unwrap_or("");
        let text = e.get("text").and_then(Value::as_str).unwrap_or("");
        let said = if name.is_empty() { text } else { name };
        if !said.is_empty() {
            let cut = e
                .get("states")
                .and_then(Value::as_array)
                .is_some_and(|a| a.iter().any(|s| s == "cut"));
            items.push((
                said.to_string(),
                n(e, "x"),
                n(e, "y"),
                n(e, "w"),
                n(e, "h"),
                cut,
            ));
        }
    }
    (items, origin)
}

/// The text a bar widget shows (#467): the items of the bar's tree (TREE, `tree --json`) whose
/// middle is in widget ID's slot, from the shell's `debugBarGeometry` (GEOM: window coordinates, the
/// bar's frame giving the layer's place).
pub fn widget_text(tree: &[u8], geom: &[u8], id: &str) -> WidgetText {
    let g: Value = serde_json::from_slice(geom).unwrap_or(Value::Null);
    let Some(slot) = g.as_array().and_then(|a| {
        a.iter()
            .find(|s| s.get("id").and_then(Value::as_str) == Some(id))
    }) else {
        return WidgetText::Absent;
    };
    if slot.get("visible").and_then(Value::as_bool) != Some(true) {
        return WidgetText::Hidden;
    }
    let n = |k: &str| slot.get(k).and_then(Value::as_i64).unwrap_or(0);
    let (items, (ox, oy)) = tree_texts(tree);
    let (x0, y0) = (n("x") + ox, n("y") + oy);
    let (x1, y1) = (x0 + n("width"), y0 + n("height"));
    let mine: Vec<TextItem> = items
        .into_iter()
        .filter(|(_, x, y, w, h, _)| {
            let (mx, my) = (x + w / 2, y + h / 2);
            mx >= x0 && mx < x1 && my >= y0 && my < y1
        })
        .collect();
    let cut = mine.iter().any(|t| t.5);
    WidgetText::Shown(mine.into_iter().map(|(s, ..)| s).collect(), cut)
}

/// widget_text's answer.
#[derive(Debug, PartialEq)]
pub enum WidgetText {
    /// Its slot is shown: the text in it (none: icons or pictures only), and whether any of it is
    /// cut (only part of it shows: a label wider than a vertical bar).
    Shown(Vec<String>, bool),
    /// Its slot is in the bar's layout with nothing shown (the widget hides itself, or is empty).
    Hidden,
    /// No slot of it in the bar's layout (off the bar, or placed by another plugin).
    Absent,
}

/// The namespaces of LAYERS (`hyprctl -j layers`), each once, in order.
pub fn layer_names(layers: &[u8]) -> Vec<String> {
    fn walk(v: &Value, into: &mut Vec<String>) {
        match v {
            Value::Object(m) => {
                if let Some(ns) = m.get("namespace").and_then(Value::as_str) {
                    if !into.iter().any(|n| n == ns) {
                        into.push(ns.to_string());
                    }
                }
                m.values().for_each(|c| walk(c, into));
            }
            Value::Array(a) => a.iter().for_each(|c| walk(c, into)),
            _ => {}
        }
    }
    let mut into = Vec::new();
    walk(
        &serde_json::from_slice(layers).unwrap_or(Value::Null),
        &mut into,
    );
    into
}

/// Text items as one line: `a | b | c`, MAX at most and the rest counted; "none" for none.
pub fn text_line(items: &[String], max: usize) -> Vec<u8> {
    if items.is_empty() {
        return b"none".to_vec();
    }
    let mut s = items
        .iter()
        .take(max)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" | ");
    if items.len() > max {
        s.push_str(&format!(" | ... and {} more", items.len() - max));
    }
    s.into_bytes()
}

/// One text item of a layer: what it says and where (tree_texts').
pub(super) type TextItem = (String, i64, i64, i64, i64, bool);

/// `tree --layer NS --json` in box NAME, as it printed it, or why it read nothing.
pub(super) fn layer_tree(name: &[u8], ns: &[u8]) -> Result<Vec<u8>, Vec<u8>> {
    let r = ob(&[b"tree", b"-b", name, b"--layer", ns, b"--json"]);
    if !r.ok {
        let why = shell::subst(&r.err);
        let why = why.strip_prefix(b"omabox: ").unwrap_or(&why).to_vec();
        return Err(why);
    }
    Ok(r.out)
}

/// layer_tree's text items.
pub(super) fn layer_texts(name: &[u8], ns: &[u8]) -> Result<Vec<TextItem>, Vec<u8>> {
    layer_tree(name, ns).map(|j| tree_texts(&j).0)
}

/// A plugin's id, from DIR/manifest.json.
pub(super) fn plugin_id(dir: &[u8]) -> Option<Vec<u8>> {
    let m: Value =
        serde_json::from_slice(&std::fs::read(shell::path(&cat!(dir, "/manifest.json"))).ok()?)
            .ok()?;
    m.get("id")
        .and_then(Value::as_str)
        .map(|s| s.as_bytes().to_vec())
}

pub(super) fn plugin(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let mut a = parse(ctx, args, "plugin")?;
    let dir = a.dir.clone().unwrap_or_default();
    let Some(id) = plugin_id(&dir) else {
        let subs = subfolders_with(&dir, &|d| plugin_id(d).is_some());
        return Err(ctx.die(cat!(
            "look plugin: ",
            dir,
            " has no manifest.json with an id: not a plugin's folder",
            subfolder_hint("plugin", args, &dir, &subs)
        )));
    };
    let me = std::process::id();
    let (name, guard) = pick(ctx, &mut a, me)?;
    let r = plugin_in(ctx, &a, &dir, &id, &name, me);
    if let Some(g) = guard {
        g.finish();
    }
    r
}

pub(super) fn plugin_in(
    ctx: &Ctx,
    a: &App,
    dir: &[u8],
    id: &[u8],
    name: &[u8],
    me: u32,
) -> Res<()> {
    let mut l = Look::new(a.keep.then_some(name));
    start(ctx, a, name, me, &[b"--plugin", dir], &mut l)?;
    // Its state, as ls --json has it.
    let status = || -> Value {
        let ls: Value = serde_json::from_slice(&ob(&[b"ls", b"--json"]).out).unwrap_or(Value::Null);
        ls.as_array()
            .and_then(|v| {
                v.iter().find(|b| {
                    b.get("name").and_then(Value::as_str).map(str::as_bytes) == Some(name)
                })
            })
            .and_then(|b| {
                b.get("plugin_status")?
                    .get(std::str::from_utf8(id).ok()?)
                    .cloned()
            })
            .unwrap_or(Value::Null)
    };
    // In a box that was up: the shell again, so it shows the plugin as its files are now (and its
    // log holds this shell's lines only: no warning of an edit ago); a box without the plugin gets
    // it first, as `plugin add` puts one in.
    if a.own {
        let r = if status().is_null() {
            ob(&[b"plugin", b"-b", name, b"add", dir])
        } else {
            ob(&[b"restart-shell", b"-b", name])
        };
        if !r.ok {
            let _ = std::io::Write::write_all(&mut std::io::stderr(), &r.err);
            return Err(ctx.die(cat!(
                "look plugin: the shell of box '",
                name,
                "' could not be given the plugin (look --fresh for a box of its own)"
            )));
        }
        warnings(&r.err, &mut l.problems);
    }
    let st = status();
    let f = |k: &str| st.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let state = if f("state").is_empty() {
        "unknown".to_string()
    } else {
        f("state")
    };
    let mut line = cat!(id, " ", state.as_str());
    if !f("via").is_empty() {
        line = cat!(line, " (", f("via"), ")");
    }
    if !f("host").is_empty() {
        line = cat!(line, ", in ", f("host"), "'s layout");
    }
    if state != "loaded" && state != "hosted" {
        l.problem(cat!("plugin ", id, ": ", state.as_str()));
    }
    let still: [&[u8]; 6] = [b"wait", b"-b", name, b"--timeout", b"5s", b"still"];
    if ob(&still).code == 124 {
        l.note(b"still changing after 5s: the shot is of a moving screen".to_vec());
    }
    // The bar's strip, from what Hyprland reserves for it; the whole screen when nothing is.
    let g = bar_strip(&ob(&[b"hyprctl", b"-b", name, b"-j", b"monitors"]).out);
    let mut sh: Vec<&[u8]> = vec![b"shot", b"-b", name];
    if let Some(g) = &g {
        sh.extend([b"-g".as_slice(), g.as_bytes()]);
    } else {
        l.note(b"no bar reserves an edge: the shot is the whole screen".to_vec());
    }
    if let Some(o) = &a.out {
        sh.extend([b"-o".as_slice(), o]);
    }
    let r = ob(&sh);
    let shot = if r.ok {
        last_line(&r.out)
    } else {
        l.problem(cat!("shot: ", shell::subst(&r.err)));
        None
    };
    // What the widget shows (#467): the bar's text items in its slot, from the shell's own geometry
    // of the bar and `tree`, which reads the shell's text (tools/qtext).
    let id_s = String::from_utf8_lossy(id).into_owned();
    let geom = ob(&[
        b"run",
        b"-b",
        name,
        b"--",
        b"omarchy-shell",
        b"shell",
        b"debugBarGeometry",
    ]);
    let bar_json = layer_tree(name, BAR_NS);
    let bar = bar_json.as_ref().map(|j| tree_texts(j).0);
    let wt = bar_json.as_ref().map(|j| widget_text(j, &geom.out, &id_s));
    let on_bar = matches!(wt, Ok(WidgetText::Shown(..) | WidgetText::Hidden));
    let text = match wt {
        Err(why) => {
            l.note(cat!("the bar's text could not be read: ", why));
            b"unknown".to_vec()
        }
        Ok(WidgetText::Shown(t, cut)) => {
            if cut {
                l.note(
                    b"part of the widget's text is cut off (outside the bar, or clipped): the shot shows only some of it"
                        .to_vec(),
                );
            }
            text_line(&t, PANEL_TEXT_MAX)
        }
        Ok(WidgetText::Hidden) => {
            l.note(cat!(
                id,
                " is in the bar's layout but shows nothing now (hidden, or no width)"
            ));
            b"none".to_vec()
        }
        Ok(WidgetText::Absent) => {
            l.note(cat!(
                id,
                " has no slot in the bar's layout (off the bar, or placed by another plugin): its text is not read"
            ));
            b"unknown".to_vec()
        }
    };
    // --open: its panel summoned, then the whole screen.
    let mut panel = None;
    let mut panel_text = None;
    if a.open {
        let layers = |n: &[u8]| layer_names(&ob(&[b"hyprctl", b"-b", n, b"-j", b"layers"]).out);
        let before = layers(name);
        let r = ob(&[
            b"run",
            b"-b",
            name,
            b"--",
            b"omarchy-shell",
            b"shell",
            b"summon",
            id,
        ]);
        if !r.ok {
            let said = cat!(shell::subst(&r.err), shell::subst(&r.out));
            l.problem(summon_problem(id, &said, on_bar, a.own.then_some(name)));
        }
        ob(&still);
        // Its text: the layers the summon mapped (Omarchy's panels are layers of their own), else
        // what is new in the bar's (a popup drawn in the bar's layer, #315).
        let mut items: Vec<String> = Vec::new();
        let new: Vec<String> = layers(name)
            .into_iter()
            .filter(|n| !before.contains(n))
            .collect();
        let mut unread = Vec::new();
        for ns in &new {
            match layer_texts(name, ns.as_bytes()) {
                Ok(t) => items.extend(t.into_iter().map(|(s, ..)| s)),
                Err(why) => unread.push(why),
            }
        }
        if new.is_empty() {
            if let (Ok(old), Ok(now)) = (&bar, layer_texts(name, BAR_NS)) {
                items.extend(
                    now.into_iter()
                        .filter(|t| !old.contains(t))
                        .map(|(s, ..)| s),
                );
            }
        }
        if items.is_empty() {
            for why in unread {
                l.note(cat!("the panel's text could not be read: ", why));
            }
        }
        panel_text = Some(text_line(&items, PANEL_TEXT_MAX));
        if a.own {
            l.note(b"its panel is left open in the box".to_vec());
        }
        let out_p = a.out.as_ref().map(|o| suffixed(o, "panel"));
        let mut sh: Vec<&[u8]> = vec![b"shot", b"-b", name];
        if let Some(o) = &out_p {
            sh.extend([b"-o".as_slice(), o]);
        }
        let r = ob(&sh);
        panel = if r.ok {
            last_line(&r.out)
        } else {
            l.problem(cat!("shot: ", shell::subst(&r.err)));
            None
        };
    }
    // The shell's warnings that name the plugin (its QML's path holds its id): the box's own (no
    // portal, no system bus) are no problem of the plugin's.
    let log = ob(&[b"log", b"-b", name, b"shell", b"-n", b"all", b"--full"]);
    // (summon's failure is a warning in the shell's log, its exit 0: said what it means, #478)
    for w in plugin_warnings(&crate::plugins::shell_warnings_of(&log.out), id) {
        l.problem(if crate::exit::contains(&w, b"no live bar widget") {
            summon_problem(id, &w, on_bar, a.own.then_some(name))
        } else {
            w
        });
    }
    l.head("screenshot", shot.as_deref().unwrap_or(b"none"));
    if let Some(p) = &panel {
        l.head("panel", p);
    }
    l.head("plugin", &line);
    l.head("text", &text);
    if let Some(t) = &panel_text {
        l.head("panel text", t);
    }
    l.own_box(ctx, a.own, name);
    l.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #478: summon's `no live bar widget` says what it means and what to do.
    #[test]
    fn summon_no_live_widget_explained() {
        let said = b"summon: no live bar widget for: example.cups\n";
        let p = |on_bar, own: Option<&[u8]>| {
            String::from_utf8(summon_problem(b"example.cups", said, on_bar, own)).unwrap()
        };
        assert_eq!(
            p(true, None),
            "summon: no live bar widget for: example.cups (the widget is on the bar but is no panel: it lacks open(), close() and opened, which qs.Ui's Panel has; --open shows nothing)"
        );
        assert!(
            p(false, None).ends_with("(no widget of it is on the bar for summon to open: see its plugin: line; the shell's log: look --keep-box, then omabox log -b BOX shell)"),
            "{}",
            p(false, None)
        );
        assert!(p(false, Some(b"dev")).ends_with("the shell's log: omabox log -b dev shell)"));
        assert!(
            String::from_utf8(summon_problem(
                b"x.y",
                b"summon: no live bar widget for: x.y",
                true,
                None
            ))
            .unwrap()
            .starts_with("summon: no live bar widget for: x.y (the widget is on the bar"),
            "...the shell log's warning, as it came, then what it means"
        );
        assert_eq!(
            summon_problem(b"x.y", b"exit 1\n", true, None),
            b"summon x.y: exit 1",
            "...another failure: as it came"
        );
    }

    #[test]
    fn the_bar_strip() {
        let m =
            |r: &str| format!(r#"[{{"width":1920,"height":1080,"reserved":{r}}}]"#).into_bytes();
        assert_eq!(bar_strip(&m("[0,26,0,0]")).as_deref(), Some("0,0 1920x26"));
        assert_eq!(
            bar_strip(&m("[0,0,0,40]")).as_deref(),
            Some("0,1040 1920x40")
        );
        assert_eq!(bar_strip(&m("[48,0,0,0]")).as_deref(), Some("0,0 48x1080"));
        assert_eq!(
            bar_strip(&m("[0,0,30,0]")).as_deref(),
            Some("1890,0 30x1080")
        );
        assert_eq!(bar_strip(&m("[0,0,0,0]")), None);
        assert_eq!(suffixed(b"/x/a.png", "panel"), b"/x/a-panel.png");
        assert_eq!(suffixed(b"/x.d/a", "panel"), b"/x.d/a-panel");
        let w = b"Could not connect to DBus. UPower service will not work.\nqs:///p/plugins/me.x/Panel.qml[15:-1]: ReferenceError: y";
        assert_eq!(
            plugin_warnings(w, b"me.x"),
            [b"qs:///p/plugins/me.x/Panel.qml[15:-1]: ReferenceError: y".to_vec()]
        );
    }

    // #467: what a widget shows, from the bar's tree (`tree --layer omarchy-bar --json`, its text
    // items) and the shell's debugBarGeometry; a panel's from the layers its summon mapped.
    #[test]
    fn a_widgets_text() {
        let tree = br#"[{"depth": 0, "role": "frame", "name": "omarchy-bar", "states": [], "x": 0, "y": 30, "w": 1920, "h": 26},
 {"depth": 1, "role": "text", "name": "Thursday 21:25", "states": [], "x": 910, "y": 35, "w": 101, "h": 16},
 {"depth": 1, "role": "text", "name": "cups", "states": [], "x": 1845, "y": 35, "w": 30, "h": 16},
 {"depth": 1, "role": "label", "name": "", "text": "391", "states": ["cut"], "x": 1880, "y": 35, "w": 20, "h": 16}]"#;
        let geom =
            br#"[{"id":"omarchy.clock","x":901,"y":0,"width":118,"height":26,"visible":true},
 {"id":"example.cups","x":1837,"y":0,"width":75,"height":26,"visible":true},
 {"id":"omarchy.tray","x":1729,"y":0,"width":0,"height":0,"visible":false}]"#;
        assert_eq!(
            widget_text(tree, geom, "example.cups"),
            WidgetText::Shown(vec!["cups".into(), "391".into()], true),
            "the items in its slot, the bar's place added (a name, else the text), one cut"
        );
        assert_eq!(
            widget_text(tree, geom, "omarchy.clock"),
            WidgetText::Shown(vec!["Thursday 21:25".into()], false)
        );
        assert_eq!(widget_text(tree, geom, "omarchy.tray"), WidgetText::Hidden);
        assert_eq!(widget_text(tree, geom, "nope"), WidgetText::Absent);
        assert_eq!(widget_text(tree, b"", "example.cups"), WidgetText::Absent);
        assert_eq!(
            widget_text(b"[]", geom, "example.cups"),
            WidgetText::Shown(vec![], false)
        );
        assert_eq!(tree_texts(tree).1, (0, 30));
        assert_eq!(text_line(&["a".into(), "b c".into()], 60), b"a | b c");
        assert_eq!(
            text_line(&["a".into(), "b".into(), "c".into()], 2),
            b"a | b | ... and 1 more"
        );
        assert_eq!(text_line(&[], 60), b"none");
        let layers = br#"{"DP-1": {"levels": {"0": [{"namespace": "omarchy-background"}],
 "2": [{"namespace": "omarchy-bar"}, {"namespace": "omarchy-bar"}], "3": [{"namespace": "omarchy-keyboard-panel"}]}}}"#;
        assert_eq!(
            layer_names(layers),
            [
                "omarchy-background",
                "omarchy-bar",
                "omarchy-keyboard-panel"
            ]
        );
    }
}
