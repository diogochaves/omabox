//! `omabox lua [-b NAME] [--json] (EXPR | -)`: Lua in the box's Hyprland, and what it returns (issue
//! #40, finding 106). share/lua.lua runs the source and raises its answer as an error on purpose, the
//! one thing `hyprctl eval` hands back. One line per value: strings and numbers as they are, nil as
//! nil, tables as JSON; --json prints strings quoted too. A dispatch Hyprland refused is exit 1
//! (#427, finding 686).

use crate::args::{optarg, unknown_opt_hint};
use crate::boxes::{box_alive, need_box};
use crate::cat;
use crate::exec::{ErrTo, Out, on_box};
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::hypr::{HYPR_LIMIT, hung_text, hypr_answers, hypr_down_text};
use crate::json;
use crate::select::select_box;
use crate::shell;
use serde_json::Value;
use std::io::Read;

const NOTHING: &str =
    "lua: nothing to evaluate (omabox lua EXPR, or omabox lua - with the source on stdin)";

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut as_json, mut src) = (Vec::new(), false, Vec::<Vec<u8>>::new());
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, args.get(i + 1))?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"--json" => as_json = true,
            b"-h" | b"--help" => return crate::help::usage("lua"),
            b"--" => {
                src.extend(args[i + 1..].iter().cloned());
                break;
            }
            b"-" => src.push(a.clone()),
            // After the source has begun, a word like -1 is part of it (finding 176).
            _ if a.starts_with(b"-") => {
                if src.is_empty() {
                    return Err(unknown_opt_hint(
                        ctx,
                        "lua",
                        a,
                        "(a Lua source starting with - goes after --)",
                    ));
                }
                src.push(a.clone());
            }
            _ => src.push(a.clone()),
        }
        i += 1;
    }
    // Stdin only when asked for with -: an agent's `omabox lua` would otherwise wait on an inherited
    // stdin forever (finding 106).
    if src.is_empty() {
        return Err(ctx.die(NOTHING));
    }
    let code = if src == [b"-".to_vec()] {
        let mut v = Vec::new();
        if std::io::stdin().read_to_end(&mut v).is_err() {
            ctx.parsed();
            return Err(ctx.die("lua: cannot read the source from stdin"));
        }
        shell::subst(&v)
    } else {
        src.join(&b' ')
    };
    if shell::all_space(&code) {
        return Err(ctx.die(NOTHING));
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    // A long string whose brackets the source does not contain, so nothing in it is quoting.
    let mut lvl = Vec::new();
    while crate::exit::contains(&code, &cat!("]", lvl, "]")) {
        lvl.push(b'=');
    }
    let body = std::fs::read(crate::root().join("share/lua.lua")).unwrap_or_default();
    let eval = cat!(
        "(function(...)\n",
        shell::strip_nl(&body),
        "\nend)([",
        lvl,
        "[\n",
        code,
        "\n]",
        lvl,
        "])"
    );
    let argv = vec![
        b"timeout".to_vec(),
        HYPR_LIMIT.as_bytes().to_vec(),
        b"hyprctl".to_vec(),
        b"eval".to_vec(),
        eval,
    ];
    let r = on_box(ctx, &argv, false, Out::Capture(None), ErrTo::Out);
    let got = shell::subst(&r.out);
    let vals = if let Some(v) = got.strip_prefix(b"error: omabox-lua-ok:") {
        v.to_vec()
    } else if let Some(e) = got.strip_prefix(b"error: omabox-lua-error:") {
        let _ = std::io::Write::write_all(&mut std::io::stderr(), &cat!("lua: ", e, "\n"));
        return Err(Exit(1));
    } else {
        if !box_alive(ctx)? {
            return Err(ctx.die(hypr_down_text(ctx)));
        }
        // No answer in time (#139), or hyprctl's own "didn't respond": a hung Hyprland.
        if r.rc == 124 {
            let t = hung_text(ctx, &format!("{HYPR_LIMIT} s"))?;
            return Err(ctx.die(t));
        }
        if !hypr_answers(ctx)? {
            let t = hung_text(ctx, "")?;
            return Err(ctx.die(t));
        }
        return Err(ctx.die(cat!(
            "lua: the box's Hyprland did not answer as expected: ",
            got
        )));
    };
    // A dispatcher or function returned is an answer, but agents read it as "ran" (#130).
    let colon = vals.iter().position(|&c| c == b':');
    let kinds = colon.map_or(&vals[..], |i| &vals[..i]);
    let vals = colon.map_or(&vals[..], |i| &vals[i + 1..]);
    if kinds.contains(&b'd') {
        say(
            "lua: a dispatcher, returned and not run: omabox lua 'hl.dispatch(EXPR)' (or omabox hyprctl dispatch 'EXPR')",
        );
    }
    if kinds.contains(&b'f') {
        say("lua: a function, returned and not called");
    }
    // A dispatch Hyprland refused (#427, finding 686): x its answer returned, `{"ok":false,…}`,
    // printed as it came; z one that returned nothing, whose words this channel loses.
    if kinds.contains(&b'z') {
        say(
            "lua: hl.dispatch was refused: what it was given is not a dispatcher (an hl.dsp call given arguments it does not take is nil); omabox hyprctl dispatch 'EXPR' prints Hyprland's error",
        );
    }
    print_values(vals, as_json)?;
    if kinds.iter().any(|k| matches!(k, b'x' | b'z')) {
        return Err(Exit(1));
    }
    Ok(())
}

/// `jq -c '.[]'` (--json) or `jq -r '.[] | if type == "string" then . elif type == "null" then
/// "nil" else tojson end'` of VALS: here for the array lua.lua writes, by jq itself for anything
/// else (its errors and its exit code as they are).
fn print_values(vals: &[u8], as_json: bool) -> Res<()> {
    let (v, ok) = json::values(vals);
    if let ([Value::Array(a)], true) = (v.as_slice(), ok) {
        let mut text = String::new();
        for x in a {
            match x {
                Value::String(s) if !as_json => text.push_str(s),
                Value::Null if !as_json => text.push_str("nil"),
                _ => text.push_str(&json::compact(x)),
            }
            text.push('\n');
        }
        return out(text);
    }
    let prog = if as_json {
        ".[]"
    } else {
        r#".[] | if type == "string" then . elif type == "null" then "nil" else tojson end"#
    };
    let mut c = crate::sig::command("jq");
    c.arg(if as_json { "-c" } else { "-r" }).arg(prog);
    c.stdin(std::process::Stdio::piped());
    let Ok(mut child) = c.spawn() else {
        return Err(Exit(127));
    };
    if let Some(si) = child.stdin.take() {
        let t = crate::exec::feed(si.into(), cat!(vals, "\n"));
        let _ = t.join();
    }
    let (rc, _) = child.wait().map(crate::exec::status).unwrap_or((1, None));
    crate::cmd::hyprctl::code(rc)
}
