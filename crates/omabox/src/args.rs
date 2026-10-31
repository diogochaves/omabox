//! Reading a command's words: its options, a box name, and what a usage error says about them (the
//! nearest option, the form help gives a value in).

use crate::cat;
use crate::exit::{Ctx, Exit, Res};
use crate::shell::strip_nl;

/// Each command's options, as bash's CMD_OPTS had them (t_unit_cli_opts checks each against the
/// command's help, `_rust-opts CMD`): `=` takes a value, `?` may take one. One table for eq_split, unknown_opt and its nearest. None
/// for a command that takes none (help, guard, host, broker, version as such).
pub fn cmd_opts(cmd: &[u8]) -> Option<&'static str> {
    match cmd {
        b"up" => Some(concat!(
            "-b= --box= --json --workspace= --new --confirm-close --no-confirm-close --interactive ",
            "--size= --plugin= --overlay= --ro-bind= --seed= --no-seed= --monitor= --theme-dir= ",
            "--theme= --net= --allow= --addr= --env= --idle= --from= --hyprland= --quickshell= ",
            "--omarchy= --keep= --stock-bar --clean --desk-shell --systemd --audio --polkit --xwayland ",
            "--autoreload --no-shell --no-pointer"
        )),
        b"path" => Some("-b= --box= --logs --run --home --json"),
        b"env" | b"brief" | b"reload" | b"restart-shell" => Some("-b= --box="),
        b"which" => Some("--json"),
        b"pid" => Some("-b= --box= --in-box --json"),
        b"down" => Some("-b= --box= --all --orphans"),
        b"ls" | b"list" | b"saves" | b"--version" | b"config" => Some("--json"),
        b"save" => Some("-b= --box= --force"),
        b"setup" => Some("--aquamarine --force --remove --feedback-skill --no-feedback-skill"),
        b"run" => Some(concat!(
            "-b= --box= -d --detach -q --print-log --replace --pass= --env-file= ",
            "--wait --start= --quiet= --timeout= --strict --json --ignore= -g= ",
            "--size= --plugin= --overlay= --ro-bind= --seed= --no-seed= --monitor= --theme-dir= ",
            "--theme= --net= --allow= --addr= --env= --idle= --from= --hyprland= --quickshell= ",
            "--omarchy= --keep= --stock-bar --clean --desk-shell --systemd --audio --polkit --xwayland ",
            "--autoreload --no-shell --no-pointer"
        )),
        b"hyprctl" => Some("-b= --box="),
        b"lua" => Some("-b= --box= --json"),
        b"log" | b"logs" => Some("-b= --box= -f --follow --crash --grep= -i -n="),
        b"events" => Some(
            "-b= --box= --since= --mark? --grep= -i --json -f --follow --until= --timeout= -n=",
        ),
        b"wait" => Some(
            "-b= --box= --json --ignore= --timeout= --quiet= --strict -g= -w= --window= --active --gone --focused -i --monitor= -l= --layer= --shot= --since= --like= --grep=",
        ),
        b"windows" | b"jobs" => Some("-b= --box= --json"),
        b"stop" => Some("-b= --box= --all"),
        b"window" => Some("-b= --box= -w= --window= --active --size= --float --tile --json"),
        b"tree" => Some("-b= --box= -w= --window= --active -l= --layer= --find= --json --all"),
        b"pixel" => Some("-b= --box= -w= --window= --active --in= --json"),
        b"keys" => Some(
            "-b= --box= -w= --window= --active -t= -s= --pass= --wait --start= --quiet= --timeout= --strict --json --ignore= -g= --expect=",
        ),
        b"keys-to-box" => Some("-b= --box="),
        b"click" => Some(
            "-b= --box= -w= --window= --active --in= --no-raise --double --steps= --glide= --mod= -l= --layer= --find= --index= --wait --start= --quiet= --timeout= --strict --json --ignore= -g= --expect=",
        ),
        b"scroll" => Some(
            "-b= --box= -w= --window= --active --in= --no-raise --mod= --source= --wait --start= --quiet= --timeout= --strict --json --ignore= -g= --expect=",
        ),
        b"pointer" => Some("-b= --box= -w= --window= --active --in= --steps= --glide= --mod="),
        b"drag" => Some(
            "-b= --box= -w= --window= --active --in= --no-raise --steps= --glide= --hold= --shot= --shot-fit= --shot-g= --shot-monitor= --mod= --wait --start= --quiet= --timeout= --strict --json --ignore= -g= --expect=",
        ),
        b"shot" | b"screenshot" => Some(
            "-b= --box= -o= --output= --no-pointer -g= -w= --window= --active --monitor= --fit= --zoom= --burst= --every= --sheet --diff --changed --since= --ignore= --after --json",
        ),
        b"calibrate" => Some("-b= --box= -o= --output= --size= --answer= --json"),
        b"mode" => Some("-b= --box= --json"),
        b"output" => Some("-b= --box= --for= --cycles="),
        b"monitor" => Some("-b= --box= --name= --json"),
        b"plugin" => Some("-b= --box="),
        b"gpu" => Some("-b= --box= --json"),
        b"cpu" => Some("-b= --box= --settle= --pid= --app= --threads --json"),
        b"audio" => Some("-b= --box= --json"),
        b"polkit" => Some(
            "-b= --box= --action= --message= --icon= --prompt= --accept= --detail= --timeout= --delay= --json",
        ),
        b"ports" => Some("-b= --box= --json"),
        b"clip" => Some("-b= --box= --from-box"),
        b"peek" => Some("-b= --box= --fps= --workspace= --focus --monitor="),
        b"gdb" => Some("-b= --box= --shell --pid= --watch"),
        b"record" => Some(concat!(
            "-b= --box= -o= --output= --monitor= --no-pointer --audio --glide= --show-input --captions --cut-idle? --fps= ",
            "--hold= --for= --sheet --frames= --every= --fit= --json"
        )),
        b"check" => Some(concat!(
            "-b= --box= --approve --runs= --base= -o= --output= --speck= --ignore= --report= --report-scale= --only= -w= --window= --active -g= ",
            "--timeout= --json --size= --plugin= --overlay= --ro-bind= --seed= --no-seed= --monitor= --theme-dir= --theme= --net= --allow= --addr= --env= --idle= --from= --hyprland= --quickshell= --omarchy= --keep= --stock-bar --clean --desk-shell --systemd --audio --polkit --xwayland --autoreload --no-shell --no-pointer"
        )),
        _ => None,
    }
}

/// bash's UP_BOX_OPTS: what makes a box, the end of up's, run's and check's tables (check's help lists
/// them as UP OPTIONS).
pub const UP_BOX_OPTS: &str = concat!(
    "--size= --plugin= --overlay= --ro-bind= --seed= --no-seed= --monitor= --theme-dir= ",
    "--theme= --net= --allow= --addr= --env= --idle= --from= --hyprland= --quickshell= ",
    "--omarchy= --keep= --stock-bar --clean --desk-shell --systemd --audio --polkit --xwayland ",
    "--autoreload --no-shell --no-pointer"
);

/// The names cmd_opts has a table for, as bash's CMD_OPTS keys (no aliases: screenshot, logs, list).
pub fn cmd_opts_keys() -> Vec<&'static str> {
    crate::help::COMMANDS
        .iter()
        .copied()
        .chain(["--version"])
        .filter(|c| cmd_opts(c.as_bytes()).is_some())
        .collect()
}

/// The form help gives OPT's value in, as ": OPT VALUE" (": -g \"X,Y WxH\""), from the command's
/// lines in `omabox help CMD`; nothing when help shows none.
pub fn opt_form(main_cmd: &[u8], opt: &str) -> String {
    if main_cmd.is_empty() {
        return String::new();
    }
    let count = |s: &str, c: char| s.matches(c).count();
    let head = crate::help::head(main_cmd);
    for line in String::from_utf8_lossy(&head).lines() {
        if line.is_empty() {
            break;
        }
        let f: Vec<&str> = line.split_ascii_whitespace().collect();
        let mut i = 0;
        while i + 1 < f.len() {
            if f[i].trim_start_matches(['[', '(']) != opt {
                i += 1;
                continue;
            }
            let mut v = f[i + 1].to_string();
            if v.starts_with(['-', '[', '|', '(']) {
                i += 1;
                continue;
            }
            if v.starts_with('"') {
                while !(v.len() > 1 && v[1..].contains('"')) && i + 2 < f.len() {
                    i += 1;
                    v = format!("{v} {}", f[i + 1]);
                }
            }
            if let Some(s) = v.strip_suffix("...") {
                v = s.to_string();
            }
            while v.ends_with([']', ')', '|']) {
                if v.ends_with(']') && count(&v, '[') >= count(&v, ']') {
                    break;
                }
                if v.ends_with(')') && count(&v, '(') >= count(&v, ')') {
                    break;
                }
                v.pop();
            }
            return format!(": {opt} {v}");
        }
    }
    String::new()
}

/// Levenshtein distance with a swap of two neighbours as one edit.
fn dist(a: &[char], b: &[char]) -> usize {
    let (la, lb) = (a.len(), b.len());
    let mut d = vec![vec![0usize; lb + 1]; la + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, x) in d[0].iter_mut().enumerate() {
        *x = j;
    }
    for i in 1..=la {
        for j in 1..=lb {
            let mut x = (d[i - 1][j] + 1).min(d[i][j - 1] + 1);
            x = x.min(d[i - 1][j - 1] + usize::from(a[i - 1] != b[j - 1]));
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                x = x.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = x;
        }
    }
    d[la][lb]
}

/// " (did you mean X?)" for the CANDIDATE nearest WORD: the only one it is the start of, or the
/// only one nearest and a typo away (a letter wrong, missing or extra, or two swapped; two typos past
/// 4 letters, three past 10). Nothing otherwise.
pub fn did_you_mean(word: &str, candidates: &[&str]) -> String {
    if word.is_empty() || candidates.is_empty() {
        return String::new();
    }
    let mut c: Vec<&str> = Vec::new();
    for k in candidates {
        if !k.is_empty() && !c.contains(k) {
            c.push(k);
        }
    }
    let lw = word.to_lowercase();
    let core = lw.trim_start_matches('-');
    if core.chars().count() >= 2 {
        let starts: Vec<&&str> = c
            .iter()
            .filter(|k| k.starts_with(&lw) && **k != lw)
            .collect();
        if let [one] = starts.as_slice() {
            return format!(" (did you mean {one}?)");
        }
    }
    let n = lw.chars().count();
    let t = if n <= 4 {
        1
    } else if n <= 10 {
        2
    } else {
        3
    };
    let w: Vec<char> = lw.chars().collect();
    let (mut best, mut bk, mut ties) = (t + 1, "", 0);
    for k in &c {
        let d = dist(&w, &k.chars().collect::<Vec<_>>());
        if d < best {
            best = d;
            bk = k;
            ties = 0;
        } else if d == best {
            ties += 1;
        }
    }
    if best <= t && best > 0 && ties == 0 {
        format!(" (did you mean {bk}?)")
    } else {
        String::new()
    }
}

/// An option the command does not take: a usage error, with the nearest one it does when one is
/// close. LABEL is what the message starts with, its first word the command.
pub fn unknown_opt(ctx: &Ctx, label: &str, opt: &[u8]) -> Exit {
    unknown_opt_hint(ctx, label, opt, "")
}

/// unknown_opt, the message ending in HINT (when there is one).
pub fn unknown_opt_hint(ctx: &Ctx, label: &str, opt: &[u8], hint: &str) -> Exit {
    let cmd = label.split(' ').next().unwrap_or_default();
    let mut known: Vec<&str> = cmd_opts(cmd.as_bytes())
        .map(|o| {
            o.split_ascii_whitespace()
                .map(|w| w.strip_suffix(['=', '?']).unwrap_or(w))
                .collect()
        })
        .unwrap_or_default();
    known.extend(["-h", "--help"]);
    let word = String::from_utf8_lossy(opt.split(|&c| c == b'=').next().unwrap_or_default());
    let hint = if hint.is_empty() {
        String::new()
    } else {
        format!(" {hint}")
    };
    ctx.usage_die(cat!(
        label,
        ": unknown option ",
        opt,
        did_you_mean(&word, &known),
        hint
    ))
}

/// A word that is not one of CHOICES where one of them goes: "LABEL: unknown WHAT WORD (did you
/// mean X?) TEXT", a usage error.
pub fn unknown_word(
    ctx: &Ctx,
    label: &str,
    what: &str,
    w: &[u8],
    text: &str,
    choices: &[&str],
) -> Exit {
    let near = did_you_mean(&String::from_utf8_lossy(w), choices);
    let text = if text.is_empty() {
        String::new()
    } else {
        format!(" {text}")
    };
    ctx.usage_die(cat!(label, ": unknown ", what, " ", w, near, text))
}

/// A word with no place in LABEL's command line: a usage error. One that names a box says the box
/// goes with -b (audit E4).
pub fn unexpected(ctx: &Ctx, label: impl AsRef<[u8]>, w: &[u8], hint: &str) -> Exit {
    let mut why = hint.as_bytes().to_vec();
    if crate::shell::re_match("^[A-Za-z0-9][A-Za-z0-9_.-]{0,39}$", w) && ctx.in_boxes(w).is_dir() {
        why = cat!("(box '", w, "'? a box goes with -b: -b ", w, ")");
    }
    let sep: &[u8] = if why.is_empty() { b"" } else { b" " };
    ctx.usage_die(cat!(label.as_ref(), ": unexpected ", w, sep, why))
}

/// `optarg "$@"`: an option with no value is a usage error naming the form help gives it.
pub fn optarg(ctx: &Ctx, opt: &[u8], value: Option<&Vec<u8>>) -> Res<()> {
    if value.is_some_and(|v| !v.is_empty()) {
        return Ok(());
    }
    let o = String::from_utf8_lossy(opt);
    Err(ctx.usage_die(cat!(
        ctx.label(),
        opt,
        " needs a value",
        opt_form(&ctx.main_cmd, &o)
    )))
}

/// NAME, -b NAME, or nothing (the default box): for commands that take nothing else.
pub fn name_arg(ctx: &Ctx, cmd: &str, args: &[Vec<u8>]) -> Res<Vec<u8>> {
    let mut name: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == b"-b" || a == b"--box" {
            optarg(ctx, a, args.get(i + 1))?;
            name = args[i + 1].clone();
            i += 1;
        } else if a.starts_with(b"-") {
            return Err(unknown_opt(ctx, cmd, a));
        } else {
            if !name.is_empty() {
                return Err(ctx.die(cat!(cmd, ": one box name, got ", name, " and ", a)));
            }
            name = a.clone();
        }
        i += 1;
    }
    Ok(strip_nl(&name))
}

/// `usage CMD` (a command's --help after its other words): its help as `omabox help CMD` prints it.
pub use crate::help::usage;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms_from_help() {
        assert_eq!(opt_form(b"path", "-b"), ": -b NAME");
        assert_eq!(opt_form(b"env", "-b"), ": -b NAME");
        assert_eq!(opt_form(b"path", "--box"), "");
        assert_eq!(opt_form(b"", "-b"), "");
        assert_eq!(opt_form(b"which", "--json"), "");
    }

    /// t_unit_cli_contract's UP OPTIONS: up's, run's and check's tables end with UP_BOX_OPTS.
    #[test]
    fn up_box_opts_end_the_box_makers() {
        for c in [&b"up"[..], b"run", b"check"] {
            let t = cmd_opts(c).unwrap();
            assert!(t.ends_with(&format!(" {UP_BOX_OPTS}")), "{t}");
        }
        let k = cmd_opts_keys();
        for c in ["up", "--version", "check", "shot", "clip", "setup"] {
            assert!(k.contains(&c), "{c}");
        }
        for c in [
            "help",
            "guard",
            "host",
            "broker",
            "version",
            "screenshot",
            "logs",
            "list",
        ] {
            assert!(!k.contains(&c), "{c}");
        }
    }

    #[test]
    fn nearest() {
        let o = [
            "-b", "--box", "--logs", "--run", "--home", "--json", "-h", "--help",
        ];
        assert_eq!(did_you_mean("--lgs", &o), " (did you mean --logs?)");
        assert_eq!(did_you_mean("--lo", &o), " (did you mean --logs?)");
        assert_eq!(did_you_mean("--h", &o), " (did you mean -h?)");
        assert_eq!(did_you_mean("--hme", &o), " (did you mean --home?)");
        assert_eq!(did_you_mean("--Logs", &o), "");
        assert_eq!(did_you_mean("--jsno", &o), " (did you mean --json?)");
        assert_eq!(did_you_mean("--zzzzzz", &o), "");
        assert_eq!(did_you_mean("-x", &o), "");
        assert_eq!(did_you_mean("", &o), "");
        // (t_unit_exit_codes's: a swap is one typo, far is nothing, a tie is nothing)
        assert_eq!(
            did_you_mean("--windwo", &["--window", "--wait"]),
            " (did you mean --window?)"
        );
        assert_eq!(did_you_mean("--zzzz", &["--window", "--wait"]), "");
        assert_eq!(did_you_mean("-x", &["-b", "-g", "-o"]), "");
    }
}
