//! `omabox config [--json | KEY [VALUE | default]]`: show or change the settings (finding 70). The
//! bar widget uses --json and KEY VALUE. protect-ports (#355, S10) is a key here, and only here
//! (finding 480): bash's CONF_KEYS does not have it.

use crate::args::{unknown_opt, unknown_word};
use crate::cat;
use crate::conf::{self, KEYS};
use crate::exit::{Ctx, Res, out, say};
use crate::install::{AQ_HOWTO, aq_resolve, hypr_aq_soname};
use crate::json;
use crate::render;
use crate::shell;
use serde_json::{Map, Value};

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let mut as_json = false;
    let mut words: Vec<Vec<u8>> = Vec::new();
    for a in args {
        match a.as_slice() {
            b"--json" => as_json = true,
            b"-h" | b"--help" => return crate::help::usage("config"),
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "config", a)),
            _ => words.push(a.clone()),
        }
    }
    let keys = KEYS.join(" ");
    // (audit E6: what goes there, and which keys there are)
    if words.len() > 2 {
        return Err(ctx.usage_die(format!(
            "config: one KEY and a VALUE at most, got {} words (keys: {keys})",
            words.len()
        )));
    }
    let key = words.first().cloned().unwrap_or_default();
    if !words.is_empty() && conf::default(&key).is_none() {
        return Err(unknown_word(
            ctx,
            "config",
            "key",
            &key,
            &format!("(keys: {keys})"),
            KEYS,
        ));
    }
    let mut value = Vec::new();
    if let Some(v) = words.get(1).filter(|v| v.as_slice() != b"default") {
        value = conf::check(&key, v).ok_or_else(|| {
            ctx.die(cat!(
                "config: bad ",
                key,
                ": ",
                v,
                " (",
                conf::help(&key),
                ")"
            ))
        })?;
    }
    ctx.parsed();
    match words.len() {
        0 if as_json => out(cat!(json_line(ctx), "\n")),
        0 => {
            for k in shown() {
                out(cat!(k, "=", conf::get(k.as_bytes()), "\n"))?;
            }
            Ok(())
        }
        1 => {
            out(cat!(conf::get(&key), "\n"))?;
            // The GPUs there are, on stderr: stdout stays the value, for scripts.
            if key == b"gpu" {
                let _ = std::io::Write::write_all(&mut std::io::stderr(), &render::gpu_list(ctx));
            }
            let fit = shell::env_or_empty("OMABOX_SHOT_FIT");
            if key == b"shot-fit" && !fit.is_empty() {
                say(cat!("OMABOX_SHOT_FIT=", fit, " overrides it here"));
            }
            Ok(())
        }
        _ => set(ctx, &key, &words[1], value),
    }
}

/// KEY VALUE (or KEY default): written, then what the change means said.
fn set(ctx: &Ctx, key: &[u8], given: &[u8], value: Vec<u8>) -> Res<()> {
    let before = conf::get(key);
    let v = if given == b"default" {
        if !conf::set(key, None) {
            return Err(ctx.refused());
        }
        conf::default(key).unwrap_or_default().to_vec()
    } else {
        if !conf::set(key, Some(&value)) {
            return Err(ctx.refused());
        }
        value
    };
    if key == b"confirm-close" {
        confirm_live(ctx, &v);
    }
    if key == b"workspace" {
        crate::cmd::peek::ws_note(&v);
    }
    if key == b"protect-ports" {
        protect_notes(ctx, &before, &v);
    }
    if key == b"gpu" && v != b"auto" && !render::gpu_usable_in(&v, b"/") {
        say(cat!(
            "note: no usable GPU here is ",
            v,
            " now: boxes render on the first one until it is (omabox config gpu lists them)"
        ));
    }
    if key == b"confirm-close" && v == b"on" {
        // (bash's `aq_resolve "$(hypr_aq_soname)"`: a soname it cannot tell is said, then nothing)
        let want = hypr_aq_soname(ctx).unwrap_or_default();
        if let Some(aq) = aq_resolve(ctx, &want).filter(|a| !a.fixed) {
            say(cat!(
                "note: confirm-close needs ",
                aq.lacks(),
                ": boxes leave it off until then. ",
                AQ_HOWTO
            ));
        }
    }
    out(cat!(key, "=", v, "\n"))
}

/// protect-ports is read at `up` (#374): a connected box up already keeps what it had, and a box
/// needs nft for the rule. Said, as a user who set it after seeing a box reach a service would take
/// it for covered now.
fn protect_notes(ctx: &Ctx, before: &[u8], v: &[u8]) {
    if !v.is_empty() && !crate::exec::executable_in(crate::cmd::up_box::LAUNCH_PATH, b"nft") {
        say(cat!(
            "note: protect-ports needs nft in ",
            crate::cmd::up_box::LAUNCH_PATH,
            ", where a box's start runs it: until then up refuses a connected box (sudo pacman -S nftables)"
        ));
    }
    if before == v {
        return;
    }
    let up: Vec<Vec<u8>> = crate::boxes::list_names(ctx, true)
        .into_iter()
        .filter(|n| {
            let d = cat!(ctx.boxes, "/", n);
            crate::boxes::box_state(&d) == "up" && crate::boxes::meta(&d, "net").0 == b"connected"
        })
        .collect();
    if !up.is_empty() {
        say(cat!(
            "note: boxes up already keep the ports they started with: ",
            up.join(&b", "[..]),
            " (down and up again to apply it to ",
            if up.len() == 1 { "it" } else { "them" },
            ")"
        ));
    }
}

/// confirm-close is live: running interactive boxes that took it from the config follow a change
/// (--confirm-close / --no-confirm-close on `up` pins a box's own); not one on an aquamarine without
/// the fix, which cannot open its window again (finding 125).
fn confirm_live(ctx: &Ctx, v: &[u8]) {
    for name in crate::shell::glob(&ctx.in_boxes(""), true) {
        let d = cat!(ctx.boxes, "/", name, "/");
        let Ok(text) = std::fs::read(shell::path(&cat!(d, "/box.json"))) else {
            continue;
        };
        let (vals, _) = json::values(&text);
        let from: Vec<u8> = vals
            .iter()
            .filter(|b| {
                b.get("mode").and_then(Value::as_str) == Some("interactive")
                    && b.get("aquamarine")
                        .and_then(|a| a.get("fixed"))
                        .is_none_or(|f| f != &Value::Bool(false))
            })
            .filter_map(|b| match b.get("confirm_close_from") {
                None | Some(Value::Null | Value::Bool(false)) => None,
                Some(x) => Some(json::raw(x)),
            })
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes();
        if from != b"config" {
            continue;
        }
        let mut c = ctx.clone();
        c.d = d.clone();
        crate::upspec::write_confirm_flag(&c, &cat!(d, "/run"), v);
    }
}

/// --json: every key snake_case, as every other --json (#273, audit N1), plus, read-only, whether
/// boxes can confirm a close (finding 125), this omabox's version (finding 133), and the GPUs for the
/// widget's picker (#118): every one on the bus, the one `auto` takes and the one a headless box
/// renders on now, by slot.
/// The keys `config` lists: not protect-ports to a jailed agent (the broker's caller), which would
/// tell it which host ports hold the user's services (#374); broker_check refuses it too.
fn shown() -> impl Iterator<Item = &'static &'static str> {
    let jailed = crate::jail::in_jail();
    KEYS.iter()
        .filter(move |k| !(jailed && **k == "protect-ports"))
}

fn json_line(ctx: &Ctx) -> String {
    let mut m = Map::new();
    for k in shown() {
        m.insert(
            k.replace('-', "_"),
            Value::String(String::from_utf8_lossy(&conf::get(k.as_bytes())).into_owned()),
        );
    }
    let avail = crate::install::confirm_close_available_in(ctx, &crate::paths::root(), b"/");
    let (auto, now) = render::gpu_auto_now_in(ctx, b"/");
    let slot = |v: Vec<u8>| {
        if v.is_empty() {
            Value::Null
        } else {
            Value::String(String::from_utf8_lossy(&v).into_owned())
        }
    };
    m.insert("confirm_close_available".into(), Value::Bool(avail));
    m.insert(
        "version".into(),
        Value::String(String::from_utf8_lossy(&crate::cmd::version::version_text()).into_owned()),
    );
    m.insert("gpus".into(), render::gpu_json());
    m.insert("gpu_auto".into(), slot(auto));
    m.insert("gpu_now".into(), slot(now));
    json::compact(&Value::Object(m))
}
