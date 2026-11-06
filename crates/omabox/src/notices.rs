//! The box's notifications log (#430, finding 680): what tools/notices recorded in the box HOME's
//! notifications.log, a JSON object a line (each Notify with the id its server gave, each close,
//! closed and action), read by `log notifications` as a line of text each (or --json) and matched by
//! `wait notification RE`. What the box wrote is the box's: each line is read as JSON, written again
//! by omabox (--json), its text made plain (term.rs), never passed on as it came. A reply that came
//! after the recorder wrote its Notify without an id is a line of its own (`reply`, finding 688),
//! which `log notifications` joins to the Notify.

use crate::json;
use serde_json::Value;
use std::os::unix::ffi::OsStrExt;
use std::process::Stdio;

/// One record of the log: the line read as JSON (an object), or None for a line that is not one.
pub fn record(line: &[u8]) -> Option<Value> {
    let v: Value = serde_json::from_slice(line).ok()?;
    v.is_object().then_some(v)
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or_default()
}

/// A field's value as a word of a text line: a string as it is (quoted when it has a space, a
/// quote or a newline, or is empty), anything else as JSON writes it.
fn word(v: &Value) -> String {
    match v {
        Value::String(t)
            if !t.is_empty() && !t.contains([' ', '"', '\\', '\n', '\t', '\r', '=', ',']) =>
        {
            t.clone()
        }
        Value::String(t) => json::quote(t),
        _ => json::compact(v),
    }
}

/// "HH:MM:SS.mmm" of a record's time (SECONDS.MILLIS), local time.
fn when(v: &Value) -> String {
    clock(v.get("time"))
}

/// "HH:MM:SS.mmm" of a time (SECONDS.MILLIS), local time.
fn clock(t: Option<&Value>) -> String {
    let t = t.map(ToString::to_string).unwrap_or_default();
    let (int, frac) = t.split_once('.').unwrap_or((&t, "000"));
    let secs: i64 = int.parse().unwrap_or(0);
    format!("{}.{frac}", crate::cmd::events::hms(secs))
}

/// NotificationClosed's reason, as the spec names it.
fn reason(r: &Value) -> String {
    match r.as_u64() {
        Some(1) => "expired".into(),
        Some(2) => "dismissed".into(),
        Some(3) => "closed by CloseNotification".into(),
        Some(4) => "closed (reason undefined)".into(),
        _ => format!("closed (reason {})", json::compact(r)),
    }
}

/// "6.0 s": how long after time A time B is (None when either is not a number).
fn after(a: &Value, b: &Value) -> Option<String> {
    let d = b.as_f64()? - a.as_f64()?;
    Some(format!("{d:.1} s"))
}

/// Urgency's byte, named.
fn urgency(u: &Value) -> String {
    match u.as_u64() {
        Some(0) => "low".into(),
        Some(1) => "normal".into(),
        Some(2) => "critical".into(),
        _ => word(u),
    }
}

/// A record as one line of text (no newline), its strings escaped as JSON escapes them and the line
/// made plain: `HH:MM:SS.mmm notify #3 APP: "SUMMARY" "BODY" urgency=critical ...`.
pub fn text(v: &Value) -> Vec<u8> {
    let id = |v: &Value| match v.get("id") {
        Some(Value::Number(n)) => format!("#{n}"),
        _ => "(no id)".into(),
    };
    if s(v, "event") == "omabox" {
        return crate::term::plain(format!("{} omabox: {}", when(v), s(v, "note")).as_bytes());
    }
    let mut l = format!("{} {} ", when(v), s(v, "event"));
    match s(v, "event") {
        "notify" => {
            l.push_str(&id(v));
            if let Some(t) = v.get("reply_time").and_then(|t| after(&v["time"], t)) {
                l.push_str(&format!(" (replied after {t})"));
            }
            if v.get("replaces_id").and_then(Value::as_u64).unwrap_or(0) != 0 {
                l.push_str(&format!(" (replaces #{})", v["replaces_id"]));
            }
            l.push_str(&format!(
                " {}: {}",
                word(v.get("app_name").unwrap_or(&Value::String(String::new()))),
                json::quote(s(v, "summary"))
            ));
            if !s(v, "body").is_empty() {
                l.push_str(&format!(" {}", json::quote(s(v, "body"))));
            }
            if let Some(Value::Object(h)) = v.get("hints") {
                for (k, x) in h {
                    // (the sender's pid is libnotify's own; image data says only that there is one)
                    if k == "sender-pid" {
                        continue;
                    }
                    let x = if k == "urgency" { urgency(x) } else { word(x) };
                    l.push_str(&format!(" {}={x}", word(&Value::String(k.clone()))));
                }
            }
            if let Some(Value::Array(a)) = v.get("actions")
                && !a.is_empty()
            {
                let each: Vec<String> = a
                    .iter()
                    .map(|x| {
                        let k = word(x.get("key").unwrap_or(&Value::Null));
                        match x.get("label") {
                            Some(Value::String(t)) => format!("{k}:{}", json::quote(t)),
                            _ => k,
                        }
                    })
                    .collect();
                l.push_str(&format!(" actions={}", each.join(",")));
            }
            match v.get("expire_timeout").and_then(Value::as_i64) {
                Some(0) => l.push_str(" timeout=never"),
                Some(t) if t > 0 => l.push_str(&format!(" timeout={t}ms")),
                _ => {}
            }
            if let Some(Value::String(e)) = v.get("error") {
                l.push_str(&format!(" -- not shown: {e}"));
            }
        }
        "close" => l.push_str(&format!("{} (asked by {})", id(v), s(v, "sender"))),
        "closed" => l.push_str(&format!(
            "{} {}",
            id(v),
            reason(v.get("reason").unwrap_or(&Value::Null))
        )),
        "action" => l.push_str(&format!("{} {}", id(v), word(&v["action"]))),
        "reply" => {
            l.push_str(&format!(
                "{} to the notify of {} ({}'s call {}",
                id(v),
                clock(v.get("call_time")),
                s(v, "sender"),
                word(v.get("serial").unwrap_or(&Value::Null))
            ));
            if let Some(t) = v.get("call_time").and_then(|c| after(c, &v["time"])) {
                l.push_str(&format!(", {t} after it"));
            }
            l.push(')');
            if let Some(Value::String(e)) = v.get("error") {
                l.push_str(&format!(" -- not shown: {e}"));
            }
        }
        _ => l.push_str(&json::compact(v)),
    }
    crate::term::plain(l.as_bytes())
}

/// The log's records in order: each line that is one, with the byte just past its newline.
pub fn records(data: &[u8]) -> Vec<(usize, Value)> {
    let mut out = Vec::new();
    let mut at = 0;
    for l in data.split_inclusive(|&c| c == b'\n') {
        at += l.len();
        if !l.ends_with(b"\n") {
            break;
        }
        if let Some(v) = record(&l[..l.len() - 1]) {
            out.push((at, v));
        }
    }
    out
}

/// RECORDS with each late reply joined to its Notify (finding 688): a Notify written without an id
/// takes the id (or the error) of the `reply` that names its sender and serial, and the reply's time
/// as "reply_time"; that reply is then no record of its own. One whose Notify is not there stays.
pub fn joined(recs: Vec<(usize, Value)>) -> Vec<(usize, Value)> {
    let key = |v: &Value| Some((s(v, "sender").to_string(), v.get("serial")?.as_u64()?));
    let mut out: Vec<(usize, Value)> = Vec::with_capacity(recs.len());
    let mut waiting = std::collections::HashMap::new();
    for (at, v) in recs {
        match s(&v, "event") {
            "notify" if v.get("id").is_some_and(Value::is_null) => {
                if let Some(k) = key(&v) {
                    waiting.insert(k, out.len());
                }
            }
            "reply" => {
                if let Some(i) = key(&v).and_then(|k| waiting.remove(&k)) {
                    let n = &mut out[i].1;
                    n["id"] = v.get("id").cloned().unwrap_or(Value::Null);
                    n["error"] = v.get("error").cloned().unwrap_or(Value::Null);
                    n["reply_time"] = v.get("time").cloned().unwrap_or(Value::Null);
                    continue;
                }
            }
            _ => {}
        }
        out.push((at, v));
    }
    out
}

/// The indexes of LINES (one line each, no newline in them) that RE matches, as `grep -E` (-i:
/// ICASE) matches a line: `log --grep`'s dialect.
pub fn grep_lines(re: &[u8], icase: bool, lines: &[Vec<u8>]) -> Vec<usize> {
    if lines.is_empty() {
        return Vec::new();
    }
    let mut data = Vec::new();
    for l in lines {
        data.extend_from_slice(l);
        data.push(b'\n');
    }
    let mut c = crate::sig::command("grep");
    c.args(["-a", "-n", "-E"]);
    if icase {
        c.arg("-i");
    }
    c.arg("--").arg(std::ffi::OsStr::from_bytes(re));
    c.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let Ok(mut child) = c.spawn() else {
        return Vec::new();
    };
    let feeder = child
        .stdin
        .take()
        .map(|si| crate::exec::feed(si.into(), data));
    let got = child
        .stdout
        .take()
        .map(|so| crate::exec::read_all(so.into(), None))
        .unwrap_or_default();
    if let Some(f) = feeder {
        let _ = f.join();
    }
    let _ = child.wait();
    got.split(|&c| c == b'\n')
        .filter_map(|l| {
            let colon = l.iter().position(|&c| c == b':')?;
            let n: usize = std::str::from_utf8(&l[..colon]).ok()?.parse().ok()?;
            n.checked_sub(1)
        })
        .collect()
}

/// A Notify's summary and body as `wait notification` matches them: each one line (its newlines as
/// spaces), as it came otherwise.
fn texts(v: &Value) -> [Vec<u8>; 2] {
    let one = |k: &str| s(v, k).replace(['\n', '\r'], " ").into_bytes();
    [one("summary"), one("body")]
}

/// `wait notification`: the first Notify in DATA (whole lines) whose summary or body RE matches
/// (grep -E, ICASE: -i), with the byte just past its line.
pub fn first_notify(re: &[u8], icase: bool, data: &[u8]) -> Option<(usize, Value)> {
    let recs: Vec<(usize, Value)> = records(data)
        .into_iter()
        .filter(|(_, v)| s(v, "event") == "notify")
        .collect();
    let lines: Vec<Vec<u8>> = recs.iter().flat_map(|(_, v)| texts(v)).collect();
    let i = *grep_lines(re, icase, &lines).first()?;
    recs.into_iter().nth(i / 2)
}

/// `log notifications`: the records of DATA, late replies joined, as text lines (with JSON, as JSON
/// lines written again), only those whose text line RE matches when there is one, the last N (None:
/// all), each line ending in a newline.
pub fn show(data: &[u8], json: bool, re: &[u8], icase: bool, n: Option<usize>) -> Vec<u8> {
    let recs = joined(records(data));
    let lines: Vec<Vec<u8>> = recs.iter().map(|(_, v)| text(v)).collect();
    let keep: Vec<usize> = if re.is_empty() {
        (0..recs.len()).collect()
    } else {
        grep_lines(re, icase, &lines)
    };
    let from = n.map_or(0, |n| keep.len().saturating_sub(n));
    let mut out = Vec::new();
    for &i in &keep[from..] {
        if json {
            out.extend_from_slice(json::compact(&recs[i].1).as_bytes());
        } else {
            out.extend_from_slice(&lines[i]);
        }
        out.push(b'\n');
    }
    out
}

/// One line of a followed notifications log, as `show` prints it (an incomplete or foreign line as
/// it is, made plain).
pub fn follow_line(line: &[u8], json: bool) -> Vec<u8> {
    match record(line) {
        Some(v) if json => json::compact(&v).into_bytes(),
        Some(v) => text(&v),
        None => crate::term::plain(line),
    }
}

/// A record as `wait notification`'s line has it: its text from the event on (no time).
pub fn detail(v: &Value) -> Vec<u8> {
    let t = text(v);
    match t.iter().position(|&c| c == b' ') {
        Some(i) => t[i + 1..].to_vec(),
        None => t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &[u8] = br#"{"time":1794005255.930,"event":"omabox","note":"listening"}
{"time":1794005261.655,"event":"notify","id":1,"error":null,"sender":":1.29","app_name":"myapp","replaces_id":0,"app_icon":"","summary":"Hello there","body":"line one\nline \"two\"","actions":[{"key":"default","label":"Open"},{"key":"later","label":"Remind me"}],"hints":{"category":"transfer.complete","urgency":2,"sender-pid":1961},"expire_timeout":-1}
{"time":1794005262.000,"event":"notify","id":null,"error":"org.freedesktop.DBus.Error.ServiceUnknown: no server","sender":":1.30","app_name":"two words","replaces_id":3,"app_icon":"","summary":"Second\u001b[31m","body":"","actions":[],"hints":{"image-data":"(iiibiiay)"},"expire_timeout":0}
{"time":1794005263.100,"event":"action","id":1,"action":"default"}
{"time":1794005264.100,"event":"closed","id":1,"reason":2}
{"time":1794005265.100,"event":"close","id":1,"sender":":1.29"}
{"time":1794005266.1"#;

    fn body(t: &[u8]) -> String {
        // (the time is local: what follows it)
        let t = String::from_utf8_lossy(t).into_owned();
        t.split_once(' ').map(|(_, r)| r.to_string()).unwrap_or(t)
    }

    #[test]
    fn records_and_text() {
        let r = records(LOG);
        assert_eq!(r.len(), 6, "the half line at the end is not one yet");
        assert_eq!(r[0].0, LOG.iter().position(|&c| c == b'\n').unwrap() + 1);
        let t: Vec<String> = r.iter().map(|(_, v)| body(&text(v))).collect();
        assert_eq!(t[0], "omabox: listening");
        assert_eq!(
            t[1],
            r#"notify #1 myapp: "Hello there" "line one\nline \"two\"" category=transfer.complete urgency=critical actions=default:"Open",later:"Remind me""#
        );
        // (an escape the app sent is shown as its JSON escape, never sent to the terminal)
        assert_eq!(
            t[2],
            r#"notify (no id) (replaces #3) "two words": "Second\u001b[31m" image-data=(iiibiiay) timeout=never -- not shown: org.freedesktop.DBus.Error.ServiceUnknown: no server"#
        );
        assert_eq!(t[3], "action #1 default");
        assert_eq!(t[4], "closed #1 dismissed");
        assert_eq!(t[5], "close #1 (asked by :1.29)");
        assert!(String::from_utf8_lossy(&text(&r[1].1)).starts_with(|c: char| c.is_ascii_digit()));
    }

    #[test]
    fn plain_on_the_terminal() {
        let v = record(br#"{"time":1.000,"event":"omabox","note":"a\u0085b\u001bc"}"#).unwrap();
        assert!(body(&text(&v)).ends_with("a?b?c"));
        assert_eq!(
            follow_line(b"not json \x1b[2J", false),
            b"not json ?[2J".to_vec()
        );
    }

    #[test]
    fn shown() {
        let all = show(LOG, false, b"", false, None);
        assert_eq!(all.iter().filter(|&&c| c == b'\n').count(), 6);
        let g = show(LOG, false, b"dismissed|Remind", false, None);
        let g = String::from_utf8_lossy(&g);
        assert_eq!(g.lines().count(), 2, "{g}");
        assert!(g.lines().next().unwrap().contains("notify #1"));
        let j = show(LOG, true, b"HELLO", true, Some(5));
        let v: Value = serde_json::from_slice(j.strip_suffix(b"\n").unwrap()).unwrap();
        assert_eq!(v["id"], 1);
        assert_eq!(v["hints"]["urgency"], 2);
        assert_eq!(
            show(LOG, false, b"", false, Some(1))
                .iter()
                .filter(|&&c| c == b'\n')
                .count(),
            1
        );
        assert_eq!(show(LOG, false, b"", false, Some(0)), b"");
        assert_eq!(show(b"", false, b"x", false, None), b"");
    }

    #[test]
    fn late_reply_joined() {
        const LATE: &[u8] = br#"{"time":100.000,"event":"notify","id":null,"error":"no reply in 5 s","serial":7,"sender":":1.5","app_name":"a","replaces_id":0,"app_icon":"","summary":"slow","body":"","actions":[],"hints":{},"expire_timeout":-1}
{"time":101.000,"event":"notify","id":null,"error":"no reply in 5 s","serial":8,"sender":":1.5","app_name":"a","replaces_id":0,"app_icon":"","summary":"never","body":"","actions":[],"hints":{},"expire_timeout":-1}
{"time":106.012,"event":"reply","serial":7,"sender":":1.5","call_time":100.000,"id":45,"error":null}
{"time":107.000,"event":"reply","serial":9,"sender":":1.5","call_time":102.000,"id":null,"error":"E: no"}
"#;
        // (before its reply: no id; the wait reads the notify as it was written)
        let two: usize = LATE
            .split_inclusive(|&c| c == b'\n')
            .take(2)
            .map(<[u8]>::len)
            .sum();
        let t = show(&LATE[..two], false, b"", false, None);
        assert!(String::from_utf8_lossy(&t).contains("notify (no id) a: \"slow\""));
        let t = String::from_utf8_lossy(&show(LATE, false, b"", false, None)).into_owned();
        let l: Vec<String> = t.lines().map(|l| body(l.as_bytes())).collect();
        assert_eq!(l.len(), 3, "{t}");
        assert_eq!(l[0], r#"notify #45 (replied after 6.0 s) a: "slow""#);
        assert_eq!(
            l[1],
            r#"notify (no id) a: "never" -- not shown: no reply in 5 s"#
        );
        // (a reply whose notify is not in the log stays a line)
        assert!(
            l[2].starts_with("reply (no id) to the notify of ")
                && l[2].ends_with("(:1.5's call 9, 5.0 s after it) -- not shown: E: no"),
            "{}",
            l[2]
        );
        let j = show(LATE, true, b"slow", false, None);
        let v: Value = serde_json::from_slice(j.strip_suffix(b"\n").unwrap()).unwrap();
        assert_eq!(
            (&v["id"], &v["error"], &v["reply_time"]),
            (&Value::from(45), &Value::Null, &serde_json::json!(106.012))
        );
        assert_eq!(
            first_notify(b"slow", false, LATE).unwrap().1["id"],
            Value::Null
        );
    }

    #[test]
    fn first_notify_matches_summary_or_body() {
        let (end, v) = first_notify(b"^line one line", false, LOG).unwrap();
        assert_eq!(v["id"], 1);
        assert_eq!(&LOG[end - 2..end], b"}\n");
        assert_eq!(
            first_notify(b"^Second", false, LOG).unwrap().1["replaces_id"],
            3
        );
        assert_eq!(first_notify(b"hello", true, LOG).unwrap().1["id"], 1);
        assert!(first_notify(b"hello", false, LOG).is_none());
        // (only a notify's summary and body: not its app, an action or a note)
        assert!(first_notify(b"myapp|default|listening", false, LOG).is_none());
        assert_eq!(
            detail(&first_notify(b"Hello", false, LOG).unwrap().1)[..9],
            *b"notify #1"
        );
    }
}
