//! `omabox ls` (`list`): the boxes, a line each with what goes under it, or `--json` for the bar
//! widget (plugin/): one array, a single pass, with peek state. Anything else is refused (#236): a
//! -b or an unknown flag dropped here gave a script the table where it asked for JSON. Boxes in byte
//! order, the same in every locale (finding 446; bash's glob followed the locale's collation).

use crate::args::{unexpected, unknown_opt_hint};
use crate::boxes::{box_state, list_names, meta, stray_names};
use crate::cat;
use crate::cmd::save::pad;
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::jq::{self, Args};
use crate::mon::size_text;
use crate::select::select_box;
use crate::shell;

/// ls's per-box programs on box.json, each as bash's omabox had it.
const MONITORS_JQ: &str = ".monitors // [] | length";
const PLUGINS_JQ: &str = r#".plugins | join(",")"#;
const ADDRS_JQ: &str = r#".addrs // [] | join(" ")"#;
const RENDER_JQ: &str = ".render // empty | select($many or .fallback)";
const SOFT_APPS_JQ: &str = r#".client_render // empty | select(.software == true) | "  apps: \(.renderer | sub(" \\(.*"; "")), in software (cost measured here is software rendering'"'"'s)""#;

/// What hung_boxes found of each box up: its Hyprland not answering within 1 s (#125, finding
/// 182), its shell hung (shell_hung, #209), or its Hyprland gone (crashed, exited, going: finding
/// 350).
#[derive(Default)]
struct Hung {
    hypr: Vec<Vec<u8>>,
    shell: Vec<Vec<u8>>,
    gone: Vec<(Vec<u8>, String)>,
}

impl Hung {
    fn hypr(&self, n: &[u8]) -> bool {
        self.hypr.iter().any(|x| x == n)
    }
    fn shell_word(&self, n: &[u8]) -> &'static str {
        if self.shell.iter().any(|x| x == n) {
            "hung"
        } else {
            "ok"
        }
    }
    fn gone(&self, n: &[u8]) -> &str {
        self.gone
            .iter()
            .find(|(x, _)| x == n)
            .map_or("", |(_, w)| w.as_str())
    }
}

/// hung_boxes: every box up (with its omabox.env: one still starting is not asked) asked at once, so
/// `ls` takes 1 s at most for them (3 s with a hung shell).
fn hung_boxes(ctx: &Ctx) -> Hung {
    let mut asks = Vec::new();
    for n in list_names(ctx, false) {
        let mut c = ctx.clone();
        c.name = n.clone();
        c.d = cat!(ctx.boxes, "/", n);
        if box_state(&c.d) != "up" || !c.at("run/omabox.env").metadata().is_ok_and(|m| m.len() > 0)
        {
            continue;
        }
        asks.push(std::thread::spawn(move || ask_one(&c)));
    }
    let mut h = Hung::default();
    for a in asks {
        match a.join() {
            Ok((n, Some(Kind::Hypr))) => h.hypr.push(n),
            Ok((n, Some(Kind::Shell))) => h.shell.push(n),
            Ok((n, Some(Kind::Gone(w)))) => h.gone.push((n, w)),
            _ => {}
        }
    }
    h
}

enum Kind {
    Hypr,
    Shell,
    Gone(String),
}

fn ask_one(c: &Ctx) -> (Vec<u8>, Option<Kind>) {
    use crate::shellnote::{shell_hung, shell_pid, shell_running};
    let k = crate::exit::muted(|| {
        if crate::hypr::hypr_answers_in(c, "1").unwrap_or(false) {
            let hung = meta(&c.d, "shell").0 == b"true"
                && shell_pid(c).is_some_and(|p| shell_running(c, &p) && shell_hung(c, &p));
            hung.then_some(Kind::Shell)
        } else if let Some(w) = crate::hypr::hypr_gone(c) {
            Some(Kind::Gone(w))
        } else {
            Some(Kind::Hypr)
        }
    });
    (c.name.clone(), k)
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let cmd = String::from_utf8_lossy(&ctx.main_cmd).into_owned();
    let hint = format!("(omabox {cmd} [--json])");
    let mut as_json = false;
    for a in args {
        match a.as_slice() {
            b"--json" => as_json = true,
            b"-b" | b"--box" => {
                say(cat!(cmd, " takes no -b: it is not about one box"));
                return Err(Exit(2));
            }
            _ if a.starts_with(b"-") => return Err(unknown_opt_hint(ctx, &cmd, a, &hint)),
            _ => return Err(unexpected(ctx, cmd.as_bytes(), a, &hint)),
        }
    }
    ctx.parsed();
    // (a dir no box can be named is left out, said: #371; bash's hung_boxes ended `ls` at it, exit 2)
    for n in stray_names(ctx) {
        say(cat!(
            cmd,
            ": '",
            n,
            "' in ",
            ctx.boxes,
            " is no box's folder (no box can be named so): left out"
        ));
    }
    let hung = hung_boxes(ctx);
    if as_json {
        return json(ctx, &hung);
    }
    table(ctx, &hung)
}

/// --json: box_json of each box, as one array (`| jq -s -c .`: box_json's lines are jq's own, so
/// they are joined as they are).
fn json(ctx: &mut Ctx, hung: &Hung) -> Res<()> {
    let mut lines: Vec<Vec<u8>> = Vec::new();
    for n in list_names(ctx, false) {
        select_box(ctx, &n)?;
        let j = crate::ready::box_json(ctx, hung.hypr(&n), hung.shell_word(&n), hung.gone(&n));
        lines.extend(
            j.split(|&c| c == b'\n')
                .filter(|l| !l.is_empty())
                .map(<[u8]>::to_vec),
        );
    }
    out(cat!("[", lines.join(&b','), "]\n"))
}

fn row(cols: [&[u8]; 8]) -> Vec<u8> {
    let w = [24, 11, 16, 7, 14, 9, 9];
    let mut o = Vec::new();
    for (c, n) in cols.iter().zip(w) {
        o.extend(pad(c, n));
        o.push(b' ');
    }
    o.extend_from_slice(cols[7]);
    o.push(b'\n');
    o
}

/// `jq -r PROG box.json 2>/dev/null` as `$(...)`, and whether jq succeeded.
fn jq_box(ctx: &Ctx, prog: &str) -> (Vec<u8>, bool) {
    let r = Args::new(&["-r", prog])
        .arg(cat!(ctx.d, "/box.json"))
        .run_file(jq::Err::Null);
    (r.text(), r.ok())
}

/// The table. IDLE: minutes since the box was last used, of its limit (it goes down when they
/// meet). SHELL (#147): running, hung, restarting or gone, and its crashes since up (+N); - for none
/// or a box not up.
fn table(ctx: &mut Ctx, hung: &Hung) -> Res<()> {
    out(row([
        b"NAME", b"MODE", b"SIZE", b"STATE", b"SHELL", b"NET", b"IDLE", b"PLUGINS",
    ]))?;
    let many = crate::render::render_nodes().len() >= 2;
    let mut orphans: Vec<Vec<u8>> = Vec::new();
    for n in list_names(ctx, false) {
        select_box(ctx, &n)?;
        // A box going down while this lists them has no box.json any more: left out, not the end
        // of the list.
        let mut state = box_state(&ctx.d).to_string();
        if state == "up" && hung.hypr(&n) {
            state = "hung".into();
        }
        let gw = hung.gone(&n);
        if state == "up" && !gw.is_empty() {
            state = gw.to_string();
        }
        let (idle, ok) = meta(&ctx.d, "idle");
        if !ok {
            continue;
        }
        let limit = if idle.is_empty() {
            Some(0)
        } else {
            shell::test_int(&idle)
        };
        // (stat -c %Y: the file's own time, a link not followed)
        let used = ctx
            .at("used")
            .symlink_metadata()
            .ok()
            .and_then(|m| m.modified().ok());
        let idle_col: Vec<u8> = match (limit, used) {
            (Some(l), Some(u)) if l > 0 => {
                let now = std::time::SystemTime::now();
                let secs = |t: std::time::SystemTime| match t.duration_since(std::time::UNIX_EPOCH)
                {
                    Ok(d) => i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
                    Err(e) => -i64::try_from(e.duration().as_secs()).unwrap_or(i64::MAX),
                };
                let m = secs(now).saturating_sub(secs(u)) / 60;
                format!("{m}m/{}", crate::num::human_duration(l)).into_bytes()
            }
            _ if (state == "up" || state == "hung") && crate::ready::box_orphan(ctx) => {
                orphans.push(ctx.name.clone());
                b"orphan".to_vec()
            }
            _ => b"never".to_vec(),
        };
        let mut sh = b"-".to_vec();
        if state == "up" && meta(&ctx.d, "shell").0 == b"true" {
            sh = crate::shellnote::shell_state_given(ctx, hung.shell_word(&n))
                .as_bytes()
                .to_vec();
            let c = crate::shellnote::shell_crashes(ctx);
            if c != 0 {
                sh.extend(format!("+{c}").bytes());
            }
        }
        let nm = match jq_box(ctx, MONITORS_JQ) {
            (v, true) => v,
            _ => b"0".to_vec(),
        };
        let size =
            if shell::test_int(if nm.is_empty() { b"0" } else { &nm }).is_some_and(|m| m <= 1) {
                let sc = meta(&ctx.d, "scale").0;
                size_text(
                    &meta(&ctx.d, "size").0,
                    if sc.is_empty() { b"1" } else { &sc },
                )
            } else {
                cat!(nm, " monitors")
            };
        out(row([
            &ctx.name,
            &meta(&ctx.d, "mode").0,
            &size,
            state.as_bytes(),
            &sh,
            &meta(&ctx.d, "net").0,
            &idle_col,
            &jq_box(ctx, PLUGINS_JQ).0,
        ]))?;
        // Its --addr addresses (#229), under its line: an isolated box with more than a loopback.
        let (al, ok) = jq_box(ctx, ADDRS_JQ);
        if ok && !al.is_empty() {
            out(cat!("  addresses: ", al, " (on its loopback)\n"))?;
        }
        // A box on a Hyprland build of its own (finding 116), and one whose shell runs a Quickshell
        // build (finding 358), under its line: never taken for a stock one.
        for (key, label) in [("hyprland", "Hyprland"), ("quickshell", "Quickshell")] {
            let b = meta(&ctx.d, key).0;
            if !b.is_empty() {
                let v = meta(&ctx.d, &format!("{key}_version")).0;
                let v = if v.is_empty() {
                    Vec::new()
                } else {
                    cat!(" (", v, ")")
                };
                out(cat!("  ", label, ": ", b, v, "\n"))?;
            }
        }
        // keys-to-box (finding 117), when on: SUPER keys follow focus into it.
        if ctx.at("keys-to-box").is_file() {
            out(
                "  keys-to-box: on (SUPER keys go to it while its window has focus and the pointer)\n",
            )?;
        }
        // Its GPU (#118), on a machine with more than one, or when it is a fallback (finding 237).
        let r = Args::new(&["-c"])
            .kv("--argjson", "many", if many { "true" } else { "false" })
            .arg(RENDER_JQ)
            .arg(cat!(ctx.d, "/box.json"))
            .run_file(jq::Err::Null);
        let rt = if r.ok() { r.text() } else { Vec::new() };
        if !rt.is_empty() {
            let l = jq::jq(
                &["-r", crate::jqprog::RENDER_LINE_JQ],
                Some(&rt),
                jq::Err::Inherit,
            );
            out(cat!("  ", l.out))?;
            if !l.ok() {
                out("\n")?;
            }
        }
        // Apps on a software renderer (#204, up's probe): their cost is not the GPU's.
        let soft = soft_apps_prog();
        let s = Args::new(&["-r", &soft])
            .arg(cat!(ctx.d, "/box.json"))
            .run_file(jq::Err::Null);
        out(&s.out)?;
    }
    // Orphans (#220), on stderr: what the IDLE column's word means, and what takes them down.
    if !orphans.is_empty() {
        say(cat!(
            "orphan: ",
            orphans.join(&b' '),
            " (no idle limit, and what started it is gone; nothing takes it down by itself: omabox down --orphans does)"
        ));
    }
    Ok(())
}

/// SOFT_APPS_JQ as jq gets it: the shell's `'"'"'` is a quote.
fn soft_apps_prog() -> String {
    SOFT_APPS_JQ.replace(r#"'"'"'"#, "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_apps_prog_whole() {
        assert!(soft_apps_prog().ends_with("rendering's)\""));
    }

    #[test]
    fn rows_padded_as_printf() {
        let r = row([
            b"b",
            b"headless",
            b"1x1",
            b"up",
            b"-",
            b"host",
            b"never",
            b"p,q",
        ]);
        assert_eq!(
            r,
            b"b                        headless    1x1              up      -              host      never     p,q\n"
        );
    }
}
