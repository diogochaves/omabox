//! The box's layer surfaces (the shell's bar, panels, menus) as bin/omabox reads them: one by a
//! selector (layer_select, finding 146: a 1x1 one is a closed menu), their words (layer_lines,
//! layer_desc), and the accessibility tree of one (layer_read, #254, finding 358), read by
//! omabox-tree in the box.

use crate::boxes::meta;
use crate::cat;
use crate::desk::kb_takers;
use crate::exec::{ExecOpts, Redir, box_exec, pipe, read_all};
use crate::exit::{Ctx, Exit, Res, say};
use crate::jq::{self, Args};
use crate::jqprog::LAYER_LIST_JQ;
use crate::shell;
use crate::win::jq_r;
use std::io::Write;
use std::os::fd::{AsRawFd, RawFd};

pub const BOX_TREE: &str = "/opt/omabox/bin/omabox-tree";

/// layer_select LAYERS SEL: one layer of LAYERS (`hyprctl -j layers`) by its namespace,
/// address:0x... or pid:PID; a 1x1 one only to say it is not drawn (exit 1); none or several listed,
/// exit 2.
pub fn layer_select(ctx: &Ctx, lys: &[u8], sel: &[u8]) -> Res<Vec<u8>> {
    let a = jq::jq(&["-c", LAYER_LIST_JQ], Some(lys), jq::Err::Null);
    if !a.ok() {
        return Err(ctx.die(cat!("box '", ctx.name, "' did not list its layers")));
    }
    let all = a.text();
    let hits = Args::new(&["-c"])
        .kv("--arg", "s", sel)
        .arg(crate::jqprog::LAYER_HITS_JQ)
        .run(Some(&all), jq::Err::Inherit)
        .text();
    let big = jq::jq(
        &["-c", crate::jqprog::LAYER_BIG_JQ],
        Some(&hits),
        jq::Err::Inherit,
    )
    .text();
    let n = crate::win::jq_length(&big);
    match n.as_slice() {
        b"1" => return Ok(jq::jq(&["-c", ".[0]"], Some(&big), jq::Err::Inherit).text()),
        b"0" => {
            if crate::win::jq_length(&hits) != b"0" {
                say(cat!(
                    "tree: layer ",
                    sel,
                    " is mapped at 1x1, not drawn (a closed menu: open it first)"
                ));
                return Err(Exit(1));
            }
            say(cat!(
                "no layer matches ",
                sel,
                " (a namespace, address:0x... or pid:PID); the box's layers:"
            ));
            layer_lines(&all);
        }
        _ => {
            say(cat!(
                sel,
                " matches ",
                n,
                " layers: name one by its address (address:0x...):"
            ));
            layer_lines(&big);
        }
    }
    Err(Exit(2))
}

/// layer_lines LIST >&2: a layer a line, for a refusal.
pub fn layer_lines(list: &[u8]) {
    let r = jq::jq(
        &["-r", crate::jqprog::LAYER_LINES_JQ],
        Some(list),
        jq::Err::Inherit,
    );
    let _ = std::io::stderr().write_all(&r.out);
}

/// layer_desc LAYER: `"omarchy-bar" (pid 142, 42x1080 at 0,0)`, for messages.
pub fn layer_desc(l: &[u8]) -> Vec<u8> {
    jq_r(crate::jqprog::LAYER_DESC_JQ, l)
}

/// The layers of LAYERS of LAYER's pid and size (its frame is the one of its pid that is its size).
pub fn layer_tied(l: &[u8], lys: &[u8]) -> Vec<u8> {
    Args::new(&["-c"])
        .arg(format!("{LAYER_LIST_JQ}{}", crate::jqprog::LAYER_TIED_JQ))
        .kv("--argjson", "l", l)
        .run(Some(lys), jq::Err::Inherit)
        .text()
}

/// box_sys ARGV... 2>"$ERR": one of omabox's own helpers in the box (box_exec's BOX_SYS), its
/// stderr, and every word said on the way, in the file ERR is open on.
pub fn box_sys_err(ctx: &mut Ctx, argv: &[Vec<u8>], err: RawFd) -> Res<i32> {
    let _e = Redir::to(2, err);
    box_exec(
        ctx,
        argv,
        ExecOpts {
            sys: true,
            ..ExecOpts::default()
        },
    )
}

/// `$(box_sys ARGV... 2>/dev/null)`: its output, or None where it failed to start.
pub fn box_sys_out(ctx: &mut Ctx, argv: &[Vec<u8>]) -> Option<(i32, Vec<u8>)> {
    let (r, w) = pipe().ok()?;
    let reader = std::thread::spawn(move || read_all(r, None));
    let rc = {
        let _e = Redir::null(2);
        box_exec(
            ctx,
            argv,
            ExecOpts {
                sys: true,
                stdout: Some(w.as_raw_fd()),
                ..ExecOpts::default()
            },
        )
    };
    drop(w);
    let out = reader.join().unwrap_or_default();
    match rc {
        Ok(rc) => Some((rc, shell::subst(&out))),
        Err(_) => None,
    }
}

/// layer_read LAYER LAYERS ERR [omabox-tree ARGS]: LAYER's accessibility tree on stdout, in the
/// screen's coordinates, the tool's words in ERR; its exit code (3: no pid to find its app by). Two
/// layers of one pid and size are told apart only when exactly one can hold the keyboard (kb_hint).
/// SECS: how long the read may take (`timeout`'s, 30 for `tree`; a wait's time left, #319).
pub fn layer_read(
    ctx: &mut Ctx,
    l: &[u8],
    lys: &[u8],
    err: &std::fs::File,
    args: &[Vec<u8>],
    secs: &[u8],
) -> Res<i32> {
    let mut argv = match layer_argv(l, lys, args, || kb_takers(ctx)) {
        Ok(a) => a,
        Err(words) => {
            let mut e = err;
            let _ = e.write_all(&words);
            return Ok(3);
        }
    };
    argv[1] = secs.to_vec();
    box_sys_err(ctx, &argv, err.as_raw_fd())
}

/// What layer_read runs in the box for LAYER of LAYERS (omabox-tree with its pid, size, place and,
/// for one of several alike, whether it is the one that can hold the keyboard as KBT_OF says:
/// kb_takers, kb_hint), ARGS after; Err: the words for a layer with no pid.
fn layer_argv(
    l: &[u8],
    lys: &[u8],
    args: &[Vec<u8>],
    kbt_of: impl FnOnce() -> Vec<u8>,
) -> Result<Vec<Vec<u8>>, Vec<u8>> {
    let f = shell::read_fields(&jq_r(crate::jqprog::LAYER_FIELDS_JQ, l), 5);
    let (pid, w, h, x, y) = (&f[0], &f[1], &f[2], &f[3], &f[4]);
    if !shell::re_match("^[1-9][0-9]*$", pid) {
        return Err(cat!(
            "the layer has no pid to find its application by (",
            pid,
            ")\n"
        ));
    }
    let tied = layer_tied(l, lys);
    let mut hint: Vec<Vec<u8>> = Vec::new();
    let more = shell::test_int(&crate::win::jq_length(&tied)).is_some_and(|n| n > 1);
    if more {
        let addrs = jq_r(".[].address", &tied);
        let addrs: Vec<&[u8]> = addrs.split(|&c| c == b'\n').collect();
        if let Some(it) = kb_hint(&jq_r(".address", l), &addrs, &kbt_of()) {
            let yes: &[u8] = if it { b"yes" } else { b"no" };
            hint = vec![b"--active".to_vec(), yes.to_vec()];
        }
    }
    let mut argv: Vec<Vec<u8>> = ["/usr/bin/timeout", "30", BOX_TREE, "--pid"]
        .iter()
        .map(|w| w.as_bytes().to_vec())
        .collect();
    argv.extend([
        pid.clone(),
        b"--layer".to_vec(),
        b"--size".to_vec(),
        cat!(w, "x", h),
        b"--offset".to_vec(),
        cat!(x, ",", y),
    ]);
    argv.extend(hint);
    argv.extend(args.iter().cloned());
    Ok(argv)
}

/// Whether the layer at ADDR is the one of TIED (the addresses of the layers of its pid and size)
/// that can hold the keyboard, by TAKERS (kb_takers): the one exclusive, else the one that takes it
/// at all (on demand: Omarchy 4.5's bar popups, #315), as only a layer that can take the keyboard
/// has an active frame; None when not one of them is that one.
fn kb_hint(addr: &[u8], tied: &[&[u8]], takers: &[u8]) -> Option<bool> {
    let modes: Vec<(&[u8], &[u8])> = takers
        .split(|&c| c == b',')
        .filter_map(|t| {
            let t = t.trim_ascii();
            let i = t.iter().rposition(|&c| c == b'=')?;
            Some((&t[..i], &t[i + 1..]))
        })
        .collect();
    let of = |pick: &dyn Fn(&[u8]) -> bool| -> Vec<&[u8]> {
        tied.iter()
            .copied()
            .filter(|a| modes.iter().any(|(m, i)| m == a && pick(i)))
            .collect()
    };
    let ex = of(&|i| i == b"1");
    let one = match ex.len() {
        1 => ex[0],
        0 => match of(&|i| i != b"0").as_slice() {
            [a] => a,
            _ => return None,
        },
        _ => return None,
    };
    Some(one == addr)
}

/// layer_qs_note LAYER: after "no accessibility tree" for a layer of a Quickshell, which releases
/// publish one.
pub fn layer_qs_note(ctx: &mut Ctx, l: &[u8]) {
    let pid = jq_r(".pid", l);
    let argv = vec![b"/usr/bin/cat".to_vec(), cat!("/proc/", pid, "/comm")];
    let comm = box_sys_out(ctx, &argv)
        .filter(|(rc, _)| *rc == 0)
        .map(|(_, o)| o)
        .unwrap_or_default();
    if comm != b"quickshell" && comm != b"qs" {
        return;
    }
    let v = meta(&ctx.d, "quickshell_version").0;
    let v = if v.is_empty() {
        Vec::new()
    } else {
        cat!("; this box runs ", v)
    };
    say(cat!(
        "Quickshell publishes its windows' accessibility trees from 0.3.2 on (quickshell-mirror/quickshell 916a0dd)",
        v,
        ": up --quickshell PATH runs a build of your own"
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    // layer_read's layer of several alike (#315): the one that can take the keyboard, given the
    // layers that may take it (kb_takers: ADDRESS=1 exclusive, =2 on demand).
    #[test]
    fn kb_hint_picks_the_one_that_can_hold_the_keyboard() {
        let tied: &[&[u8]] = &[b"0xa", b"0xb", b"0xc"];
        assert_eq!(kb_hint(b"0xc", tied, b""), None, "none takes it: no hint");
        assert_eq!(
            kb_hint(b"0xc", tied, b"0xc=2"),
            Some(true),
            "the one on demand (an Omarchy 4.5 popup beside its bar and wallpaper, #315): it"
        );
        assert_eq!(
            kb_hint(b"0xa", tied, b"0xc=2"),
            Some(false),
            "...the others: not it"
        );
        assert_eq!(
            kb_hint(b"0xb", tied, b"0xb=1, 0xc=2"),
            Some(true),
            "one exclusive, one on demand: the exclusive one"
        );
        assert_eq!(
            kb_hint(b"0xb", tied, b"0xb=2, 0xc=2"),
            None,
            "two on demand: cannot tell"
        );
        assert_eq!(
            kb_hint(b"0xb", tied, b"0xa=1, 0xb=1"),
            None,
            "two exclusive: cannot tell"
        );
        assert_eq!(
            kb_hint(b"0xa", tied, b"0xd=1, 0xc=2"),
            Some(false),
            "a layer not of the size holding it is not counted"
        );
    }

    // t_unit_tree_layer's layer_read (#254): what it asks the box's tools/tree, the layers that may
    // take the keyboard given.
    #[test]
    fn layer_argv_as_bash() {
        let j = br#"{"DP-1": {"levels": {"0": [{"address": "0xa", "namespace": "omarchy-background", "pid": 7, "x": 0, "y": 0, "w": 1920, "h": 1080}],
      "1": [], "2": [{"address": "0xb", "namespace": "omarchy-bar", "pid": 7, "x": 0, "y": 0, "w": 42, "h": 1080},
                     {"address": "0xm", "namespace": "omarchy-menu", "pid": 7, "x": 0, "y": 0, "w": 1, "h": 1}],
      "3": [{"address": "0xc", "namespace": "omarchy-launcher", "pid": 7, "x": 0, "y": 0, "w": 1920, "h": 1080}]}},
    "DP-2": {"levels": {"2": [{"address": "0xd", "namespace": "omarchy-bar", "pid": 7, "x": 1920, "y": 0, "w": 42, "h": 1080},
      {"address": "0xe", "namespace": "solo", "pid": 9, "x": 2100, "y": 300, "w": 200, "h": 60}]}}}"#;
        let mut ctx = Ctx::new(b"tree", b"/nonexistent".to_vec());
        ctx.parsed();
        let read = |sel: &[u8], kb: &str| {
            let l = layer_select(&ctx, j, sel).unwrap();
            let a = layer_argv(&l, j, &[b"--json".to_vec()], || kb.as_bytes().to_vec()).unwrap();
            String::from_utf8_lossy(&a.join(&b' ')).into_owned()
        };
        assert_eq!(
            read(b"solo", ""),
            "/usr/bin/timeout 30 /opt/omabox/bin/omabox-tree --pid 9 --layer --size 200x60 --offset 2100,300 --json",
            "a layer alone at its size: its pid, size and place, no hint"
        );
        assert!(
            read(b"omarchy-launcher", "0xc=1").ends_with("--offset 0,0 --active yes --json"),
            "two of its size, it with the keyboard: its frame is the active one"
        );
        assert!(
            read(b"omarchy-background", "0xc=1").ends_with("--active no --json"),
            "...the other: not the active one"
        );
        assert!(
            !read(b"omarchy-background", "").contains("--active"),
            "...neither taking the keyboard: no hint (tree says it cannot tell)"
        );
        assert!(
            read(b"omarchy-launcher", "0xc=2").ends_with("--active yes --json")
                && read(b"omarchy-background", "0xc=2").ends_with("--active no --json"),
            "...it taking the keyboard on demand, the other not at all (#315): the same"
        );
        assert!(
            read(b"omarchy-launcher", "0xb=1, 0xc=2").ends_with("--active yes --json"),
            "...another layer's exclusive keyboard does not hide it (not of its size)"
        );
        assert_eq!(
            layer_argv(br#"{"pid": null}"#, j, &[], Vec::new),
            Err(b"the layer has no pid to find its application by (null)\n".to_vec()),
            "a layer with no pid: said, nothing run"
        );
    }
}
