//! --wait --tree (#288, finding 610): what changed in a window's accessibility tree between two
//! `tree --json` reads, one before the input and one once the screen settled (treewait::snap).
//!
//! A node is known by its ancestry (the printed ancestors' `role "name"`, the window's frame left
//! out), its role and its name, never by its place: a place renumbers when one node appears before
//! it, and a diff by place then pairs the wrong nodes. Nodes with the same key pair in order. What
//! is left after that pairs only when as many are left on both sides under the same roles (a label
//! whose name is its text: "Idle" then "Saving" is that label renamed, not one gone and one come).
//! The bounds are not compared: the screen's watch says where it changed. A password field's text
//! and value are never printed, nor any text or value when the input held a secret (keys --pass).

use serde_json::{Map, Value, json};
use std::collections::{HashMap, VecDeque};

/// The lines a --wait line is followed by at most; the rest is "and N more".
pub const CAP: usize = 20;
/// A name's or text's characters in a line, a value's: as `tree`'s text view cuts them.
const NAME_W: usize = 80;
const VALUE_W: usize = 40;

/// One node of `tree --json`.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub depth: usize,
    pub role: String,
    pub name: String,
    pub text: Option<String>,
    pub value: Option<String>,
    pub states: Vec<String>,
    /// The printed ancestors below the window's frame, `role "name"` each (`tree --find --json`'s
    /// `path`), and the parent's index.
    pub path: Vec<String>,
    pub parent: Option<usize>,
    /// The ancestors' roles alone, its own last: the second pass's key.
    roles: String,
}

/// `tree --json`'s array as nodes, their ancestry from their depths; None when it is not one.
pub fn nodes(js: &[u8]) -> Option<Vec<Node>> {
    let v: Value = serde_json::from_slice(js).ok()?;
    let mut out: Vec<Node> = Vec::new();
    // (the ancestors of the node being read: its label, its index, its roles)
    let mut stack: Vec<(String, usize, String)> = Vec::new();
    for n in v.as_array()? {
        let depth = usize::try_from(n.get("depth")?.as_u64()?).ok()?;
        let s = |k: &str| n.get(k).and_then(Value::as_str).map(str::to_owned);
        let role = s("role").unwrap_or_default();
        let name = s("name").unwrap_or_default();
        let states = n
            .get("states")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        stack.truncate(depth);
        let path = stack.iter().skip(1).map(|a| a.0.clone()).collect();
        let parent = stack.last().map(|a| a.1);
        let roles = match stack.last() {
            Some(a) => format!("{}/{role}", a.2),
            None => role.clone(),
        };
        out.push(Node {
            depth,
            role: role.clone(),
            name: name.clone(),
            text: s("text").filter(|t| !t.is_empty()),
            value: s("value"),
            states,
            path,
            parent,
            roles: roles.clone(),
        });
        stack.push((label(&role, &name), out.len() - 1, roles));
    }
    Some(out)
}

fn label(role: &str, name: &str) -> String {
    if name.is_empty() {
        role.to_owned()
    } else {
        format!("{role} \"{}\"", cut(name, NAME_W))
    }
}

/// S at most N characters, its last one an ellipsis when it is longer.
fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_owned();
    }
    let mut t: String = s.chars().take(n - 1).collect();
    t.push('…');
    t
}

/// The key a node pairs by: its ancestry, role and name (the frame's own name, the window's title,
/// left out: a title is said by the desktop's facts).
fn key(n: &Node) -> (Vec<String>, String, String) {
    let name = if n.depth == 0 {
        String::new()
    } else {
        n.name.clone()
    };
    (n.path.clone(), n.role.clone(), name)
}

/// What is left of a group after the first pass: its nodes in BASE, in NEW.
type Sides = (Vec<usize>, Vec<usize>);

/// The pairs: for each node of NEW, the node of BASE it is (None: it appeared).
fn pair(base: &[Node], new: &[Node]) -> Vec<Option<usize>> {
    let mut by: HashMap<(Vec<String>, String, String), VecDeque<usize>> = HashMap::new();
    for (i, b) in base.iter().enumerate() {
        by.entry(key(b)).or_default().push_back(i);
    }
    let mut to: Vec<Option<usize>> = new
        .iter()
        .map(|n| by.get_mut(&key(n)).and_then(VecDeque::pop_front))
        .collect();
    // What is left: by roles alone, paired in order where as many are left on both sides.
    let taken: std::collections::HashSet<usize> = to.iter().flatten().copied().collect();
    let mut left: HashMap<(usize, &str), Sides> = HashMap::new();
    for (i, b) in base.iter().enumerate() {
        if !taken.contains(&i) {
            left.entry((b.depth, &b.roles)).or_default().0.push(i);
        }
    }
    for (i, n) in new.iter().enumerate() {
        if to[i].is_none() {
            left.entry((n.depth, &n.roles)).or_default().1.push(i);
        }
    }
    for (b, n) in left.values() {
        if b.len() == n.len() {
            for (bi, ni) in b.iter().zip(n) {
                to[*ni] = Some(*bi);
            }
        }
    }
    to
}

/// What changed, a line and an object each: the nodes of NEW in order (changed, appeared), then
/// those of BASE gone. A node that appeared or went with its parent counts in the parent's
/// `inside`. SECRET: no text or value shown at all.
pub fn diff(base: &[Node], new: &[Node], secret: bool) -> Vec<(String, Value)> {
    let to = pair(base, new);
    let mut out = Vec::new();
    let hidden = |n: &Node| secret || n.role == "password text";
    // (appeared: the topmost of each new subtree, and how many under it)
    let mut top: Vec<Option<usize>> = vec![None; new.len()];
    let mut inside: Vec<usize> = vec![0; new.len()];
    let mut order: Vec<usize> = Vec::new();
    for (i, n) in new.iter().enumerate() {
        if to[i].is_some() {
            order.push(i);
            continue;
        }
        match n.parent.and_then(|p| top[p]) {
            Some(t) => {
                top[i] = Some(t);
                inside[t] += 1;
            }
            None => {
                top[i] = Some(i);
                order.push(i);
            }
        }
    }
    for &i in &order {
        let n = &new[i];
        match to[i] {
            Some(b) => {
                if let Some(c) = changed(&base[b], n, hidden(n) || hidden(&base[b])) {
                    out.push(c);
                }
            }
            None => out.push(came(n, "appeared", inside[i])),
        }
    }
    // (gone: the same, in BASE)
    let mut kept = vec![false; base.len()];
    for b in to.iter().flatten() {
        kept[*b] = true;
    }
    let mut btop: Vec<Option<usize>> = vec![None; base.len()];
    let mut binside: Vec<usize> = vec![0; base.len()];
    let mut border = Vec::new();
    for (i, b) in base.iter().enumerate() {
        if kept[i] {
            continue;
        }
        match b.parent.and_then(|p| btop[p]) {
            Some(t) => {
                btop[i] = Some(t);
                binside[t] += 1;
            }
            None => {
                btop[i] = Some(i);
                border.push(i);
            }
        }
    }
    for i in border {
        out.push(came(&base[i], "vanished", binside[i]));
    }
    out
}

/// Where a node without a name is: in its nearest named ancestor.
fn whereabouts(n: &Node) -> String {
    if !n.name.is_empty() {
        return String::new();
    }
    match n.path.iter().rev().find(|p| p.contains('"')) {
        Some(a) => format!(" (in {a})"),
        None => String::new(),
    }
}

fn came(n: &Node, kind: &str, inside: usize) -> (String, Value) {
    let sign = if kind == "appeared" { "+" } else { "-" };
    let more = if inside == 0 {
        String::new()
    } else {
        format!(" (and {inside} inside)")
    };
    let line = format!("{sign} {}{}{more}", label(&n.role, &n.name), whereabouts(n));
    let mut o = base_obj(n, kind);
    o.insert("inside".into(), json!(inside));
    o.insert("line".into(), json!(line));
    (line, Value::Object(o))
}

fn base_obj(n: &Node, kind: &str) -> Map<String, Value> {
    let mut o = Map::new();
    o.insert("kind".into(), json!(kind));
    o.insert("role".into(), json!(n.role));
    o.insert("name".into(), json!(n.name));
    o.insert("path".into(), json!(n.path));
    o
}

/// One node's change, B before and N after, or None when nothing that is said differs. HIDE: its text
/// and value never shown (a password field's, a secret's).
fn changed(b: &Node, n: &Node, hide: bool) -> Option<(String, Value)> {
    let mut o = base_obj(n, "changed");
    let mut parts: Vec<String> = Vec::new();
    let renamed = n.depth > 0 && b.name != n.name;
    let head = if renamed {
        o.insert("name_was".into(), json!(b.name));
        format!(
            "{} \"{}\" → \"{}\"{}",
            n.role,
            cut(&b.name, NAME_W),
            cut(&n.name, NAME_W),
            whereabouts(b)
        )
    } else {
        format!("{}{}", label(&n.role, &n.name), whereabouts(n))
    };
    let (tb, tn) = (b.text.as_deref(), n.text.as_deref());
    let (vb, vn) = (b.value.as_deref(), n.value.as_deref());
    if hide && (tb != tn || vb != vn) {
        parts.push("its text changed (not shown)".into());
        o.insert("hidden".into(), json!(true));
    } else {
        if tb != tn {
            let q = |t: Option<&str>| format!("'{}'", cut(t.unwrap_or(""), NAME_W));
            parts.push(format!("{} → {}", q(tb), q(tn)));
            o.insert(
                "text".into(),
                json!({"from": tb.unwrap_or(""), "to": tn.unwrap_or("")}),
            );
        }
        if vb != vn {
            let q = |v: Option<&str>| match v {
                Some(v) => format!("={}", cut(v, VALUE_W)),
                None => "no value".into(),
            };
            parts.push(format!("{} → {}", q(vb), q(vn)));
            o.insert("value".into(), json!({"from": vb, "to": vn}));
        }
    }
    let added: Vec<&String> = n.states.iter().filter(|s| !b.states.contains(s)).collect();
    let removed: Vec<&String> = b.states.iter().filter(|s| !n.states.contains(s)).collect();
    if !added.is_empty() || !removed.is_empty() {
        let mut s: Vec<String> = added.iter().map(|a| format!("+{a}")).collect();
        s.extend(removed.iter().map(|r| format!("-{r}")));
        parts.push(s.join(" "));
        o.insert("states_added".into(), json!(added));
        o.insert("states_removed".into(), json!(removed));
    }
    if parts.is_empty() && !renamed {
        return None;
    }
    let line = if parts.is_empty() {
        head
    } else {
        format!("{head}: {}", parts.join(", "))
    };
    o.insert("line".into(), json!(line));
    Some((line, Value::Object(o)))
}

/// The --wait line's part for N changes in WHAT's tree ("the app's", "the layer's").
pub fn summary(n: usize, what: &str) -> String {
    match n {
        0 => format!("no change in {what} tree"),
        1 => format!("{what} tree: 1 change"),
        n => format!("{what} tree: {n} changes"),
    }
}

/// The lines under the --wait line: CAP at most, two spaces in, then "and N more".
pub fn shown(lines: &[String]) -> Vec<u8> {
    let mut out = String::new();
    for l in lines.iter().take(CAP) {
        out.push_str("  ");
        out.push_str(l);
        out.push('\n');
    }
    if lines.len() > CAP {
        out.push_str(&format!("  and {} more\n", lines.len() - CAP));
    }
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(js: &str) -> Vec<Node> {
        nodes(js.as_bytes()).expect("a tree")
    }

    fn lines(a: &str, b: &str, secret: bool) -> Vec<String> {
        diff(&tree(a), &tree(b), secret)
            .into_iter()
            .map(|c| c.0)
            .collect()
    }

    const FORM: &str = r#"[{"depth":0,"role":"frame","name":"Form","states":["active"]},
      {"depth":1,"role":"entry","name":"Name","states":["editable"]},
      {"depth":1,"role":"check box","name":"Beta","states":["unchecked"]},
      {"depth":1,"role":"list","name":"Fruit","states":[]},
      {"depth":2,"role":"list item","name":"Apple","states":["selected"]},
      {"depth":2,"role":"list item","name":"Pear","states":[]},
      {"depth":1,"role":"label","name":"Idle","states":[]},
      {"depth":1,"role":"password text","name":"Password","states":["editable"]}]"#;

    #[test]
    fn ancestry_from_depths() {
        let t = tree(FORM);
        assert_eq!(t[4].path, vec![r#"list "Fruit""#.to_string()]);
        assert_eq!(t[4].parent, Some(3));
        assert!(t[1].path.is_empty());
        assert_eq!(t[0].parent, None);
        assert!(nodes(b"{}").is_none());
        assert!(nodes(b"not json").is_none());
    }

    #[test]
    fn no_change_is_none() {
        assert!(lines(FORM, FORM, false).is_empty());
        assert_eq!(summary(0, "the app's"), "no change in the app's tree");
        assert_eq!(summary(1, "the app's"), "the app's tree: 1 change");
        assert_eq!(summary(3, "the layer's"), "the layer's tree: 3 changes");
    }

    #[test]
    fn text_states_selection_and_a_rename() {
        let after = FORM
            .replace(
                r#""name":"Name","states":["editable"]"#,
                r#""name":"Name","text":"hello","states":["editable","focused"]"#,
            )
            .replace(
                r#""Beta","states":["unchecked"]"#,
                r#""Beta","states":["checked"]"#,
            )
            .replace(r#""Apple","states":["selected"]"#, r#""Apple","states":[]"#)
            .replace(r#""Pear","states":[]"#, r#""Pear","states":["selected"]"#)
            .replace(r#""name":"Idle""#, r#""name":"Saving""#);
        assert_eq!(
            lines(FORM, &after, false),
            vec![
                r#"entry "Name": '' → 'hello', +focused"#,
                r#"check box "Beta": +checked -unchecked"#,
                r#"list item "Apple": -selected"#,
                r#"list item "Pear": +selected"#,
                r#"label "Idle" → "Saving""#,
            ]
        );
    }

    #[test]
    fn keyed_by_name_not_place() {
        // One item put first: the others are the same items, not each renamed (a diff by place).
        let after = FORM.replace(
            r#"{"depth":2,"role":"list item","name":"Apple""#,
            r#"{"depth":2,"role":"list item","name":"Fig","states":[]},
      {"depth":2,"role":"list item","name":"Apple""#,
        );
        assert_eq!(lines(FORM, &after, false), vec![r#"+ list item "Fig""#]);
        // Two gone, one come: not paired as a rename.
        let after = FORM
            .replace(
                r#"{"depth":2,"role":"list item","name":"Apple","states":["selected"]},"#,
                "",
            )
            .replace(r#""name":"Pear""#, r#""name":"Plum""#);
        let mut l = lines(FORM, &after, false);
        l.sort();
        let want = vec![
            r#"+ list item "Plum""#,
            r#"- list item "Apple""#,
            r#"- list item "Pear""#,
        ];
        let mut want: Vec<String> = want.into_iter().map(String::from).collect();
        want.sort();
        assert_eq!(l, want);
    }

    #[test]
    fn a_subtree_counts_once() {
        let dlg = r#"[{"depth":0,"role":"frame","name":"Form","states":["active"]},
          {"depth":1,"role":"dialog","name":"Confirm","states":[]},
          {"depth":2,"role":"label","name":"Sure?","states":[]},
          {"depth":2,"role":"push button","name":"OK","states":[]},
          {"depth":2,"role":"entry","states":[]}]"#;
        let base = r#"[{"depth":0,"role":"frame","name":"Form","states":["active"]}]"#;
        assert_eq!(
            lines(base, dlg, false),
            vec![r#"+ dialog "Confirm" (and 3 inside)"#]
        );
        assert_eq!(
            lines(dlg, base, false),
            vec![r#"- dialog "Confirm" (and 3 inside)"#]
        );
        // An unnamed one says where it is.
        let one = r#"[{"depth":0,"role":"frame","name":"Form","states":[]},
          {"depth":1,"role":"dialog","name":"Confirm","states":[]},
          {"depth":2,"role":"entry","states":[]}]"#;
        let two = one.replace(
            r#""role":"entry","states":[]"#,
            r#""role":"entry","text":"x","states":[]"#,
        );
        assert_eq!(
            lines(one, &two, false),
            vec![r#"entry (in dialog "Confirm"): '' → 'x'"#]
        );
    }

    #[test]
    fn never_a_password() {
        let after = FORM.replace(
            r#""name":"Password","states":["editable"]"#,
            r#""name":"Password","text":"hunter2","states":["editable"]"#,
        );
        let d = diff(&tree(FORM), &tree(&after), false);
        assert_eq!(d.len(), 1);
        assert_eq!(
            d[0].0,
            r#"password text "Password": its text changed (not shown)"#
        );
        let js = serde_json::to_string(&d[0].1).unwrap();
        assert!(!js.contains("hunter2"), "{js}");
        assert!(js.contains(r#""hidden":true"#), "{js}");
        // A secret typed (keys --pass): no text anywhere.
        let after = FORM.replace(
            r#""name":"Name","states":["editable"]"#,
            r#""name":"Name","text":"s3cret","states":["editable"]"#,
        );
        let d = diff(&tree(FORM), &tree(&after), true);
        assert_eq!(d[0].0, r#"entry "Name": its text changed (not shown)"#);
        assert!(!serde_json::to_string(&d[0].1).unwrap().contains("s3cret"));
    }

    #[test]
    fn values_and_the_cap() {
        let a = r#"[{"depth":0,"role":"frame","name":"F","states":[]},
          {"depth":1,"role":"slider","name":"Volume","value":"40","states":[]}]"#;
        let b = a.replace(r#""value":"40""#, r#""value":"60""#);
        assert_eq!(lines(a, &b, false), vec![r#"slider "Volume": =40 → =60"#]);
        // The frame's own name (the title) is the desktop's to say.
        let c = a.replace(r#""name":"F""#, r#""name":"G""#);
        assert!(lines(a, &c, false).is_empty());
        let many: Vec<String> = (0..25).map(|i| format!("c{i}")).collect();
        let s = String::from_utf8(shown(&many)).unwrap();
        assert_eq!(s.lines().count(), CAP + 1);
        assert!(s.starts_with("  c0\n"));
        assert!(s.ends_with("  and 5 more\n"));
        assert_eq!(shown(&many[..2]), b"  c0\n  c1\n".to_vec());
        assert_eq!(cut("abcdef", 4), "abc…");
    }
}
