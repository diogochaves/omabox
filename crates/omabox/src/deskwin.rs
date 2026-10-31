//! An interactive box's monitors (#123, #174): each a window on the user's desktop, made by the box's
//! Hyprland (`output create wayland NAME`), and the box's windows there laid out as its layout scaled
//! as a whole, a display-settings picture (desk_relay, desk_picture). What is done on the host is
//! done on purpose, by address (no focus moves), and a host rule catching the new window, by a class
//! of its own, keeps it off the user's workspace (findings 70, 429).
//!
//! As in mon.rs: a step that fails ends the command the same way in every caller (finding 442).

use crate::boxes::{BoxPid, box_pid, meta};
use crate::cat;
use crate::exec::{ErrTo, Out, on_box};
use crate::exit::{Ctx, Exit, Res, say};
use crate::host::{Addr, HostSilent, Query, host_query};
use crate::hypr::{HYPR_LIMIT, hypr_json};
use crate::jq::{self, Args};
use crate::jqprog::JQ_MON;
use crate::mon::{
    box_layout, field1, line_starts, monitor_place, monitor_spec, monitors_file, monitors_place,
    monitors_record, monitors_write, records, w, with_line,
};
use crate::screen::main_screen;
use crate::settle::{after, past};
use crate::shell::{self, dec, strip_nl};

/// ws_check WS: where interactive and peek windows open (a workspace 1-99, `special`, or
/// special:NAME: #292, finding 423), in its normal form; None for anything else.
pub fn ws_check(v: &[u8]) -> Option<Vec<u8>> {
    if v == b"special" {
        return Some(b"special:scratchpad".to_vec());
    }
    if let Some(n) = v.strip_prefix(b"special:") {
        return shell::re_match("^[A-Za-z0-9_-]{1,32}$", n).then(|| v.to_vec());
    }
    shell::re_match("^[1-9][0-9]?$", v).then(|| v.to_vec())
}

/// The box's windows' pid on the host: the outer bwrap's (omabox-wlfd connects, then execs it), the
/// parent of the box's PID 1 (`ps -o ppid= -p "$(box_pid)"`).
fn win_pid(ctx: &Ctx) -> Vec<u8> {
    match box_pid(&ctx.d) {
        BoxPid::Pid(p) => crate::procfs::proc_ppid(&p).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// `$(meta workspace)` through ws_check, 9 when box.json has none.
fn box_ws(ctx: &Ctx) -> Option<Vec<u8>> {
    let ws = meta(&ctx.d, "workspace").0;
    ws_check(if ws.is_empty() { b"9" } else { &ws })
}

/// `jq ARGS <<<"$INPUT"` as `$(...)` keeps it, its words on stderr; its status too.
fn jqt(a: Args, input: &[u8]) -> (i32, Vec<u8>) {
    let r = a.run(Some(input), jq::Err::Inherit);
    (r.rc, r.text())
}

/// `$(desk_windows [CLIENTS])`: the box's windows on the host, "NAME ADDRESS" a line, the main one as
/// `main` first (none when it is closed), then its monitors' ($D/monitor-windows, in the order
/// made) still open. None where bash's returns 1.
pub fn desk_windows(ctx: &Ctx, clients: Option<&[u8]>) -> Option<Vec<u8>> {
    let clients = match clients {
        Some(c) if !c.is_empty() => c.to_vec(),
        _ => {
            let (rc, c) = host_query(ctx, Query::Clients, false);
            if rc != 0 {
                return None;
            }
            strip_nl(&c)
        }
    };
    let pid = win_pid(ctx);
    if !shell::is_num(&pid) {
        return None;
    }
    let ws = box_ws(ctx)?;
    let mw = std::fs::read(ctx.at("monitor-windows")).unwrap_or_default();
    let mut known: Vec<Vec<u8>> = Vec::new();
    for l in records(&mw) {
        let f = shell::read_fields(l, 2);
        let ok = Args::new(&["-e"])
            .kv("--arg", "a", &f[1])
            .arg(crate::jqprog::ANY_ADDR_JQ)
            .run(Some(&clients), jq::Err::Null)
            .ok();
        if ok {
            known.push(cat!(f[0], " ", f[1]));
        }
    }
    // (`printf '%s\n' "${known[@]}" | jq -R 'select(. != "") | split(" ")[1]' | jq -s .`)
    let lines = cat!(known.join(&b'\n'), "\n");
    let k = jq::tool(
        "jq",
        &["-R", r#"select(. != "") | split(" ")[1]"#],
        Some(&lines),
        jq::Err::Inherit,
    );
    let k = jq::tool("jq", &["-s", "."], Some(&k.out), jq::Err::Inherit).text();
    let r = Args::new(&["-r"])
        .kv("--argjson", "p", &pid)
        .kv("--arg", "ws", &ws)
        .kv("--argjson", "k", &k)
        .arg(crate::jqprog::DESK_MAIN_JQ)
        .run(Some(&clients), jq::Err::Inherit);
    let mut out = r.out;
    for l in &known {
        out.extend_from_slice(&cat!(l, "\n"));
    }
    Some(strip_nl(&out))
}

/// desk_prune: a window closed on the desktop unplugged its monitor: its line and its address go
/// before the picture is laid out again. The windows' record that cannot be rewritten is said, and
/// the command goes on (its temporary file gone).
pub fn desk_prune(ctx: &Ctx) -> Res<()> {
    let Ok(m) = hypr_json(ctx, &["monitors"]) else {
        return Ok(());
    };
    let m = strip_nl(&m);
    let mut keep: Vec<Vec<u8>> = Vec::new();
    let f = monitors_file(ctx);
    for l in records(&f) {
        // (`read -r line`: its blanks at either end off; `${line%% *}` its first word)
        let line = shell::read_fields(l, 1).remove(0);
        let n = line
            .split(|&c| c == b' ')
            .next()
            .unwrap_or_default()
            .to_vec();
        if n.is_empty() {
            continue;
        }
        if crate::cmd::output::listed(&m, &n) {
            keep.push(line);
        }
    }
    let kept = shell::grep_lines(&keep.join(&b'\n'));
    if kept != shell::grep_lines(&strip_nl(&monitors_file(ctx))) {
        monitors_write(ctx, &kept);
    }
    let mw = ctx.at("monitor-windows");
    if mw.is_file() {
        let names = jq::jq(
            &["-r", r#"map(.name) | join("|")"#],
            Some(&m),
            jq::Err::Inherit,
        )
        .text();
        let r = jq::tool(
            "grep",
            &[
                b"-E".to_vec(),
                cat!("^(", names, ") "),
                shell::bytes(&mw).to_vec(),
            ],
            None,
            jq::Err::Inherit,
        );
        let new = cat!(ctx.d, "/monitor-windows.new");
        let _new = shell::Rm(new.clone());
        if let Err(e) = std::fs::write(shell::path(&new), &r.out) {
            say(cat!(ctx.label(), crate::shots::cant_write(&new, &e)));
        } else if !crate::shots::mv_f(&new, shell::bytes(&mw)) {
            say(cat!(
                ctx.label(),
                "box '",
                ctx.name,
                "''s record of its monitor windows was not updated"
            ));
        }
    }
    Ok(())
}

/// A step whose status is RC: its failure said (WHAT could not be done) and the command ended, 1, in
/// every caller (bash's set -e ended it with the step's status, unsaid, where it held: finding 442).
fn chk(ctx: &Ctx, rc: i32, what: &str) -> Res<()> {
    if rc != 0 {
        return Err(ctx.die(cat!("monitor: ", what)));
    }
    Ok(())
}

/// The words desk_relay adds up (jq's and the picture's integers), decimal, 0 for an empty one; one
/// that is no number ends the command, said (finding 448: bash's arithmetic, in its own words).
fn ints(ctx: &Ctx, v: &[&[u8]]) -> Res<Vec<i64>> {
    let n: Option<Vec<i64>> = v
        .iter()
        .map(|x| if x.is_empty() { Some(0) } else { dec(x) })
        .collect();
    n.ok_or_else(|| {
        ctx.die(cat!(
            ctx.label(),
            "cannot read the layout's numbers: ",
            v.join(&b' ')
        ))
    })
}

/// A window address read from the host's clients, for HostSilent; anything else ends the command.
fn addr(ctx: &Ctx, a: &[u8]) -> Res<Addr> {
    Addr::new(a).ok_or_else(|| {
        ctx.die(cat!(
            ctx.label(),
            "not a window's address on your desktop: ",
            a
        ))
    })
}

/// desk_relay [LINE]: lay an interactive box's windows out on the host as its layout, scaled as a
/// whole (#174), on the host monitor its main window is on, in that monitor's free area, the
/// picture centred, one scale for all (desk_picture). The main window is floated for it and made the
/// size of its view; with no monitors left it goes back to tiling, if it tiled before. With LINE (a
/// monitor about to be made), its window's size and place relative to the host monitor, "W H X Y".
/// Moves by address: no focus moves.
pub fn desk_relay(ctx: &Ctx, new: &[u8]) -> Res<Vec<u8>> {
    let host = HostSilent::new(ctx);
    let (rc, clients) = host_query(ctx, Query::Clients, false);
    if rc != 0 {
        return Err(ctx.die("monitor: cannot read your desktop's windows"));
    }
    let clients = strip_nl(&clients);
    let (rc, mons) = host_query(ctx, Query::Monitors, false);
    if rc != 0 {
        return Err(ctx.die("monitor: cannot read your desktop's monitors"));
    }
    let mons = strip_nl(&mons);
    let Some(wins) = desk_windows(ctx, Some(&clients)) else {
        return Err(ctx.die(cat!(
            "monitor: cannot find box '",
            ctx.name,
            "''s windows on your desktop"
        )));
    };
    let main = records(&wins)
        .into_iter()
        .find(|l| field1(l) == b"main")
        .map(|l| {
            crate::mon::awk_fields(l)
                .get(1)
                .copied()
                .unwrap_or_default()
                .to_vec()
        })
        .unwrap_or_default();
    let mut mon = Vec::new();
    if !main.is_empty() {
        let (_, id) = jqt(
            Args::new(&[])
                .kv("--arg", "a", &main)
                .arg(crate::jqprog::WIN_MONITOR_JQ),
            &clients,
        );
        let (rc, m) = jqt(
            Args::new(&["-c"])
                .kv("--argjson", "id", &id)
                .arg(crate::jqprog::MON_BY_ID_JQ),
            &mons,
        );
        chk(ctx, rc, "cannot read your desktop's monitors")?;
        mon = m;
    }
    if mon.is_empty() {
        let (rc, m) = jqt(Args::new(&["-c", crate::jqprog::MON_FOCUSED_JQ]), &mons);
        chk(ctx, rc, "cannot read your desktop's monitors")?;
        mon = m;
    }
    if mon.is_empty() {
        return Err(ctx.die("monitor: cannot read your monitor's size"));
    }
    // No monitors left: the main window back as it was.
    if new.is_empty() && shell::grep_lines(&monitors_file(ctx)).is_empty() {
        let mf = std::fs::read(ctx.at("main-window")).map(|v| strip_nl(&v));
        if !main.is_empty() && mf.is_ok_and(|v| v == cat!(main, " tiled")) {
            host.float(&addr(ctx, &main)?, false)?;
        }
        let _ = std::fs::remove_file(ctx.at("main-window"));
        return Ok(Vec::new());
    }
    let opt = |o: &'static str, prog: &str| -> Vec<u8> {
        let (rc, v) = host_query(ctx, Query::Option(o), false);
        let r = jq::tool("jq", &["-r", prog], Some(&v), jq::Err::Null);
        if rc != 0 || !r.ok() {
            return Vec::new();
        }
        r.text()
    };
    let mut gap = opt("general:gaps_out", crate::jqprog::GAPS_JQ);
    let mut bw = opt("general:border_size", ".int // empty");
    if !shell::is_num(&gap) {
        gap = b"10".to_vec();
    }
    if !shell::is_num(&bw) {
        bw = b"2".to_vec();
    }
    let g = ints(ctx, &[&gap, &bw]).map(|v| v[0].saturating_add(v[1]))?;
    let g = g.to_string();
    let r = Args::new(&["-r"])
        .kv("--argjson", "g", &g)
        .arg(format!("{JQ_MON}{}", crate::jqprog::DESK_AREA_JQ))
        .run(Some(&mon), jq::Err::Inherit);
    // (`read -r ... < <(jq ...)`: nothing read is read's failure)
    if r.out.is_empty() {
        chk(ctx, 1, "cannot read your monitor's free area")?;
    }
    let a = shell::read_fields(&r.out, 6);
    let (hrc, hw) = jqt(Args::new(&["-r", &format!("{JQ_MON}lw | floor")]), &mon);
    chk(ctx, hrc, "cannot read your monitor's size")?;
    let (hrc, hh) = jqt(Args::new(&["-r", &format!("{JQ_MON}lh | floor")]), &mon);
    chk(ctx, hrc, "cannot read your monitor's size")?;
    let Some(lay) = box_layout(ctx, if new.is_empty() { None } else { Some(new) }) else {
        return Err(ctx.die(cat!(
            "monitor: box '",
            ctx.name,
            "' was started by an omabox before its windows were laid out as its layout (#174): start it again"
        )));
    };
    let r = jq::tool(
        "awk",
        &[crate::jqprog::DESK_PICTURE_AWK],
        Some(&cat!(a[4], " ", a[5], " ", g, "\n", lay, "\n")),
        jq::Err::Inherit,
    );
    if !r.ok() {
        return Err(ctx.die(cat!(
            "monitor: cannot lay out box '",
            ctx.name,
            "''s windows"
        )));
    }
    let pic = strip_nl(&r.out);
    let mut pl = pic.split(|&c| c == b'\n');
    let head = shell::read_fields(pl.next().unwrap_or_default(), 2);
    let (k, fitted) = (&head[0], &head[1]);
    if fitted == b"0" {
        say(cat!(
            "monitor: box '",
            ctx.name,
            "''s layout does not fit on your ",
            hw,
            "x",
            hh,
            " monitor even at a quarter of its size (Hyprland's smallest scale): its windows go past its edge"
        ));
    } else {
        let pct = jq::tool(
            "awk",
            &[
                b"-v".to_vec(),
                cat!("k=", k),
                crate::jqprog::DESK_PCT_AWK.as_bytes().to_vec(),
            ],
            None,
            jq::Err::Inherit,
        );
        say(cat!(
            "box '",
            ctx.name,
            "''s windows: its layout at ",
            pct.out,
            " of its size"
        ));
    }
    // The main window floated (it tiled: kept, to tile again when the monitors are gone), then each
    // to its place; the new one's printed.
    if !main.is_empty() {
        let (_, fl) = jqt(
            Args::new(&["-r"])
                .kv("--arg", "a", &main)
                .arg(crate::jqprog::WIN_FLOATING_JQ),
            &clients,
        );
        if fl != b"true" {
            if let Err(e) = std::fs::write(ctx.at("main-window"), cat!(main, " tiled\n")) {
                return Err(ctx.die(cat!(
                    "monitor: ",
                    crate::shots::cant_write(&cat!(ctx.d, "/main-window"), &e)
                )));
            }
            host.float(&addr(ctx, &main)?, true)?;
        }
    }
    let mut said = Vec::new();
    let rows: Vec<&[u8]> = pl.collect();
    let new_name = new.split(|&c| c == b' ').next().unwrap_or_default();
    for row in &rows {
        let f = shell::read_fields(row, 5);
        let (name, x, y, wd, ht) = (&f[0], &f[1], &f[2], &f[3], &f[4]);
        if !new.is_empty() && name == new_name {
            let v = ints(ctx, &[&a[2], &a[0], x, &a[3], &a[1], y])?;
            said.extend_from_slice(&cat!(
                wd,
                " ",
                ht,
                " ",
                (v[0] - v[1] + v[2]).to_string(),
                " ",
                (v[3] - v[4] + v[5]).to_string(),
                "\n"
            ));
            continue;
        }
        let at = records(&wins)
            .into_iter()
            .find(|l| field1(l) == name.as_slice())
            .map(|l| {
                crate::mon::awk_fields(l)
                    .get(1)
                    .copied()
                    .unwrap_or_default()
                    .to_vec()
            })
            .unwrap_or_default();
        if at.is_empty() {
            continue;
        }
        let at = addr(ctx, &at)?;
        let s = ints(ctx, &[wd, ht])?;
        host.resize(&at, s[0], s[1])?;
        let v = ints(ctx, &[&a[2], x, &a[3], y])?;
        host.move_to(&at, v[0] + v[1], v[2] + v[3])?;
    }
    // The box's monitors follow their windows' new sizes: wait for the main one, at most 3 s.
    if !main.is_empty() {
        let end = after(3);
        let want = rows
            .iter()
            .filter(|l| field1(l) == b"main")
            .map(|l| {
                let f = crate::mon::awk_fields(l);
                let g = |i: usize| f.get(i).copied().unwrap_or_default();
                cat!(g(3), "x", g(4))
            })
            .collect::<Vec<_>>()
            .join(&b'\n');
        loop {
            let now = match hypr_json(ctx, &["monitors"]) {
                Ok(m) => jq::tool(
                    "jq",
                    &[
                        "-r",
                        &format!("{}{}", main_screen(ctx), crate::jqprog::WH_JQ),
                    ],
                    Some(&m),
                    jq::Err::Inherit,
                )
                .text(),
                Err(_) => Vec::new(),
            };
            if now == want || past(end) {
                break;
            }
            crate::trap::sleep(b"0.05");
        }
    }
    Ok(strip_nl(&said))
}

/// monitor_add_window's EXIT trap once the box was told to make the output (MON_CREATING, mon_undo,
/// finding 429): it is removed, so an add cut short (a refusal; INT, TERM or HUP) leaves no
/// monitor. The box handles its IPC in order, so a remove sent after the create reaches it comes after
/// it, however late. A signal's trap runs once main's foreground child, the create's capture, is over
/// (finding 453), so the remove is sent once, after the create (bash's mon_undo sent it at once, and
/// again half a second on for a create still on its way, "output not found" the first time). Nothing
/// here keeps the window off the user's workspace (its own rule does): a remove that fails leaves at
/// worst a monitor on the box's workspace. Its token file
/// goes once the remove is answered `ok` (the box made the output, so read it), and stays otherwise,
/// for a box that has yet to read it.
struct MonUndo {
    trap: Option<crate::trap::Trap>,
    creating: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl MonUndo {
    fn arm(ctx: &Ctx, name: &[u8], appid: std::path::PathBuf) -> MonUndo {
        let c = ctx.clone();
        let creating = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (cr, name) = (creating.clone(), name.to_vec());
        let trap = crate::trap::on_exit(move || {
            if !cr.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            crate::exit::muted(|| {
                let remove = || {
                    let r = on_box(
                        &c,
                        &w(&[
                            b"timeout",
                            HYPR_LIMIT.as_bytes(),
                            b"hyprctl",
                            b"output",
                            b"remove",
                            &name,
                        ]),
                        false,
                        Out::Capture(None),
                        ErrTo::Null,
                    );
                    shell::subst(&r.out) == b"ok"
                };
                if remove() {
                    let _ = std::fs::remove_file(&appid);
                }
            });
        });
        MonUndo {
            trap: Some(trap),
            creating,
        }
    }

    /// `MON_CREATING=1` / `MON_CREATING=""`.
    fn creating(&self, on: bool) {
        self.creating.store(on, std::sync::atomic::Ordering::SeqCst);
    }

    /// The window is there: nothing left to undo.
    fn done(&mut self) {
        self.creating(false);
        if let Some(t) = self.trap.take() {
            t.forget();
        }
    }
}

/// mon_answer RC OUT: what the box's answer to `output create` says (finding 429).
#[derive(PartialEq, Eq, Debug)]
enum Answer {
    /// `ok`.
    Ok,
    /// It made none: Hyprland's own words, hyprctl could not reach it, the box gone.
    No,
    /// Anything else: hyprctl gave up waiting (5 s: "Couldn't read (6)"), was cut short or said
    /// nothing, and the box may still handle the create later.
    Unclear,
}

fn mon_answer(ctx: &Ctx, rc: i32, out: &[u8]) -> Answer {
    if out == b"ok" {
        return Answer::Ok;
    }
    answer_of(crate::boxes::box_state(&ctx.d), rc, out)
}

/// mon_answer's table, once the answer is not `ok`: from the box's state (box_state), hyprctl's
/// status and its words.
fn answer_of(state: &str, rc: i32, out: &[u8]) -> Answer {
    if out == b"ok" {
        return Answer::Ok;
    }
    match state {
        "dead" => return Answer::No,
        "unknown" => return Answer::Unclear,
        _ => {}
    }
    if crate::exit::contains(out, b"Couldn't connect")
        || crate::exit::contains(out, b"Couldn't open a socket")
        || ((rc == 0 || rc == 7) && !out.is_empty())
    {
        Answer::No
    } else {
        Answer::Unclear
    }
}

/// A class token for a monitor's window: 16 hex digits from /dev/urandom (`od -An -N8 -tx1`).
fn mon_token() -> Option<Vec<u8>> {
    use std::io::Read;
    let mut b = [0u8; 8];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .ok()?;
    Some(
        b.iter()
            .map(|x| format!("{x:02x}"))
            .collect::<String>()
            .into_bytes(),
    )
}

/// `$(monitor_add_window SPEC)`: an interactive box's monitor (#123), a window on the user's
/// desktop as its first one is. It has a class of its own (aquamarine-omabox-TOKEN, from the token
/// file omabox.appid.NAME in the box's runtime dir: omabox's aquamarine build, +appid), and a host
/// rule matching that class alone (workspace, silent, no focus, floating at its size and place) is
/// on before the token is written, and stays on (finding 429). In the box it is placed as a
/// headless box's, kept in omabox.monitors; on the host the picture is laid out again with it
/// (#174). Its line as Hyprland made it.
pub fn monitor_add_window(ctx: &mut Ctx, spec: &[u8]) -> Res<Vec<u8>> {
    let Ok([mode, scale, place]) = monitor_spec(spec) else {
        return Err(Exit(1));
    };
    let Some(ws) = box_ws(ctx) else {
        return Err(ctx.die(cat!(
            "monitor add: box '",
            ctx.name,
            "' has no usable workspace in box.json"
        )));
    };
    let pid = win_pid(ctx);
    if !shell::is_num(&pid) {
        return Err(ctx.die(cat!(
            "monitor add: cannot find box '",
            ctx.name,
            "''s windows on your desktop"
        )));
    }
    desk_prune(ctx)?;
    // Named here (the next free WAYLAND-N), so its line, and so its place and scale, are there
    // before it is.
    let m = hypr_json(ctx, &["monitors"])
        .map(|m| strip_nl(&m))
        .unwrap_or_default();
    let mut i = 2;
    loop {
        let n = format!("WAYLAND-{i}");
        if !(crate::cmd::output::listed(&m, n.as_bytes())
            || line_starts(&monitors_file(ctx), n.as_bytes()))
        {
            break;
        }
        i += 1;
    }
    let name = format!("WAYLAND-{i}").into_bytes();
    let line = cat!(name, " ", mode, " ", monitor_place(&place), " ", scale);
    // The picture with it: the windows there moved (and the main one floated) first, then its line.
    let rect = desk_relay(ctx, &line).map_err(|_| Exit(1))?;
    let rest = strip_nl(&monitors_file(ctx));
    if !monitors_write(ctx, &with_line(&rest, &line)) {
        return Err(ctx.die(cat!(
            "monitor add: could not record ",
            name,
            " for the box's config reloads"
        )));
    }
    if monitors_place(ctx).is_err() {
        monitors_write(ctx, &rest);
        return Err(ctx.die(cat!(
            "monitor add: placing box '",
            ctx.name,
            "''s monitors failed"
        )));
    }
    let r4 = shell::read_fields(&rect, 4);
    let Some(tok) = mon_token() else {
        monitors_write(ctx, &rest);
        return Err(ctx.die(cat!(
            "monitor add: could not make a class for ",
            name,
            "'s window"
        )));
    };
    let cls = cat!("aquamarine-omabox-", tok);
    let appid = ctx.at(&format!(
        "run/omabox.appid.{}",
        String::from_utf8_lossy(&name)
    ));
    // (its window's size and place, as numbers for HostSilent's rule)
    let Ok(r4) = ints(ctx, &[&r4[0], &r4[1], &r4[2], &r4[3]]) else {
        monitors_write(ctx, &rest);
        return Err(Exit(1));
    };
    let r4 = [r4[0], r4[1], r4[2], r4[3]];
    // A half-made monitor removed however this ends (MonUndo).
    let mut undo = MonUndo::arm(ctx, &name, appid.clone());
    // The rule (aquamarine-omabox-TOK's, cls): its `move` is relative to its monitor.
    let host = HostSilent::new(ctx);
    host.mon_rule(&tok, &ws, r4)?;
    // The token once its rule is on: no window of that class is ever without one.
    if !crate::boxes::box_file(ctx, &appid, &cat!("omabox-", tok)) {
        monitors_write(ctx, &rest);
        return Err(ctx.die(cat!(
            "monitor add: could not give ",
            name,
            "'s window a class of its own"
        )));
    }
    // Sent again right before the window is asked for: a config reload of the host's drops every
    // rule.
    host.mon_rule(&tok, &ws, r4)?;
    undo.creating(true);
    let r = on_box(
        ctx,
        &w(&[
            b"timeout",
            HYPR_LIMIT.as_bytes(),
            b"hyprctl",
            b"output",
            b"create",
            b"wayland",
            &name,
        ]),
        false,
        Out::Capture(None),
        ErrTo::Out,
    );
    let res = shell::subst(&r.out);
    if res != b"ok" {
        // Removed by the trap unless the box clearly made none: one that answered late may make it
        // yet.
        if mon_answer(ctx, r.rc, &res) == Answer::No {
            undo.creating(false);
            let _ = std::fs::remove_file(&appid);
        }
        monitors_write(ctx, &rest);
        return Err(ctx.die(cat!(
            "monitor add: the box could not make another window: ",
            res
        )));
    }
    // Its window, by its class: this add's alone, whatever else opens meanwhile.
    let end = after(5);
    let addr;
    loop {
        let (rc, c) = host_query(ctx, Query::Clients, false);
        if rc == 0 {
            // (its words to /dev/null, as bash's `2>/dev/null`: a reply not JSON, a host mid-reload)
            let j = Args::new(&["-er"])
                .kv("--arg", "c", &cls)
                .arg(crate::jqprog::CLASS_ADDR_JQ)
                .run(Some(&strip_nl(&c)), jq::Err::Null);
            if j.rc == 0 {
                addr = j.text();
                break;
            }
        }
        if past(end) {
            // (MON_CREATING kept: the trap removes the output, whose window may still come; and
            // its line)
            monitors_write(ctx, &rest);
            return Err(ctx.die(cat!(
                "monitor add: no new window of box '",
                ctx.name,
                "' on your desktop in 5 s"
            )));
        }
        crate::trap::sleep(b"0.05");
    }
    // (made: the box read its token)
    undo.done();
    let _ = std::fs::remove_file(&appid);
    let end = after(5);
    loop {
        if let Ok(m) = hypr_json(ctx, &["monitors"]) {
            if crate::cmd::output::listed(&strip_nl(&m), &name) {
                break;
            }
        }
        if past(end) {
            return Err(ctx.die(cat!("monitor add: the box lists no monitor ", name)));
        }
        crate::trap::sleep(b"0.05");
    }
    if !addr.is_empty() {
        use std::io::Write;
        let f = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(ctx.at("monitor-windows"));
        match f {
            Ok(mut f) => {
                let _ = f.write_all(&cat!(name, " ", addr, "\n"));
            }
            Err(e) => say(cat!(ctx.d, "/monitor-windows: ", crate::shots::why(&e))),
        }
    }
    // Its own view's scale, from its window's size (the views' until then).
    if monitors_place(ctx).is_err() {
        return Err(ctx.die(cat!(
            "monitor add: ",
            name,
            " is there, but setting its place and scale failed"
        )));
    }
    crate::trap::sleep(b"0.3");
    monitors_record(ctx)?;
    // (`hypr_json monitors | jq ...` last: its status the subshell's, pipefail's)
    let m = hypr_json(ctx, &["monitors"])?;
    let r = jq::tool(
        "jq",
        &[
            b"-r".to_vec(),
            b"--arg".to_vec(),
            b"n".to_vec(),
            name.clone(),
            crate::jqprog::MON_LINE_JQ.as_bytes().to_vec(),
        ],
        Some(&m),
        jq::Err::Inherit,
    );
    crate::cmd::hyprctl::code(r.rc)?;
    Ok(r.text())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspaces_as_ws_check_takes_them() {
        assert_eq!(ws_check(b"special"), Some(b"special:scratchpad".to_vec()));
        assert_eq!(
            ws_check(b"special:omabox"),
            Some(b"special:omabox".to_vec())
        );
        assert_eq!(ws_check(b"special:"), None);
        assert_eq!(ws_check(b"9"), Some(b"9".to_vec()));
        assert_eq!(ws_check(b"99"), Some(b"99".to_vec()));
        assert_eq!(ws_check(b"0"), None);
        assert_eq!(ws_check(b"100"), None);
    }

    #[test]
    fn mon_answer_table() {
        use Answer::{No, Ok, Unclear};
        let cases: &[(&str, i32, &[u8], Answer)] = &[
            ("up", 0, b"ok", Ok),
            ("dead", 0, b"ok", Ok),
            // Hyprland's own words: rc 0 ("Name already taken"), or 7 for an `error:` answer
            ("up", 0, b"Name already taken", No),
            ("up", 7, b"error: nope", No),
            // hyprctl could not reach it
            (
                "up",
                4,
                b"Couldn't connect to /run/user/1000/hypr/s/.socket.sock. (4)",
                No,
            ),
            ("up", 1, b"Couldn't open a socket (1)", No),
            // the box gone, whatever hyprctl said
            ("dead", 6, b"Couldn't read (6)", No),
            ("dead", 124, b"", No),
            // hyprctl's own give-up at 5 s, timeout(1)'s at 10, cut short, nothing said
            (
                "up",
                6,
                b"Hyprland IPC didn't respond in time\n\nCouldn't read (6)",
                Unclear,
            ),
            ("up", 124, b"", Unclear),
            ("up", 0, b"", Unclear),
            ("up", 143, b"", Unclear),
            ("up", 5, b"Couldn't write (5)", Unclear),
            // a box whose state cannot be read: never taken for one that made none
            ("unknown", 0, b"Name already taken", Unclear),
            ("unknown", 4, b"Couldn't connect to x. (4)", Unclear),
        ];
        for (st, rc, out, want) in cases {
            assert_eq!(
                answer_of(st, *rc, out),
                *want,
                "{st} {rc} {}",
                String::from_utf8_lossy(out)
            );
        }
    }
}
