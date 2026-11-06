//! `omabox brief [-b NAME]` (#271): what a subagent needs to drive one box, in ~15 lines its parent
//! pastes into its prompt, so it does not load the whole skill. The box's name, size and windows,
//! each with a selector that finds it alone, then the rules. With no box up: the same rules for a
//! box the subagent starts itself (up --new), said on stderr.

use crate::args::{optarg, unexpected, unknown_opt};
use crate::boxes::{box_alive, need_box};
use crate::cat;
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::jq::{self, Args};
use crate::select::select_box;

/// The windows, each by the word that finds it alone (its class's last part, as the selector reads
/// a word: the class, its last dot part, or part of a title), else its address; 6 at most. As in
/// bin/omabox, byte for byte.
pub const WINDOWS_JQ: &str = r#". as $all
      | def word: (.class // "" | ascii_downcase | split(".") | last // "");
        def finds($w): [$all[] | (.class // "" | ascii_downcase) as $c | select($c == $w or ($c | split(".") | last) == $w
          or (.title // "" | ascii_downcase | contains($w)))] | length == 1;
        def sel: word as $w | if ($w | test("^[a-z0-9][a-z0-9_.-]*$")) and $w != "active" and ($w | test("^0x") | not) and finds($w) then $w else .address end;
      if length == 0 then "  none yet (omabox -b \($b) run -d --wait -- APP opens one)"
      else (.[:6][] | "  \(sel)  \(.title // "" | if length > 40 then .[:39] + "…" else . end | tojson)  \(.size[0])x\(.size[1]) at \(.at[0]),\(.at[1])  "
          + (if .onscreen | not then "off-screen (workspace \(.workspace))" elif .cover != [] then "covered" else "on-screen" end)
          + (if .active then ", active" else "" end)),
        (if length > 6 then "  and \(length - 6) more: omabox -b \($b) windows" else empty end) end"#;

/// The first line's program, as bash expands its double-quoted one around main_screen's.
fn first_jq(main: &str) -> String {
    format!(
        r#"({main}) as $s | (map(select(.disabled | not)) | length - 1) as $n
      | "You drive omabox box '\($b)' (\(if $s then "\($s.width)x\($s.height), scale \($s.scale * 100 | round / 100)" else "its main screen away" end)"
        + (if $n > 0 then ", \($n) more monitor\(if $n > 1 then "s" else "" end): monitor list" else "" end)
        + "): a private desktop, not the user's. Its windows (--window SEL, size at x,y):""#
    )
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let mut name = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, args.get(i + 1))?;
                name = args[i + 1].clone();
                i += 1;
            }
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "brief", a)),
            _ => return Err(unexpected(ctx, "brief", a, "")),
        }
        i += 1;
    }
    select_box(ctx, &name)?;
    let (mut b, mut home) = (
        b"NAME".to_vec(),
        b"the path omabox -b NAME path --home prints".to_vec(),
    );
    let (mut mons, mut wins) = (Vec::new(), Vec::new());
    if box_alive(ctx)? {
        need_box(ctx)?;
        b = ctx.name.clone();
        home = cat!(ctx.d, "/home");
        mons = crate::hypr::hypr_json(ctx, &["monitors"])?;
        wins = crate::win::box_windows(ctx, None)?;
    } else {
        say(cat!(
            "brief: no box '",
            ctx.name,
            "' is up: this brief is for a box the subagent starts itself (omabox up --new); with one up, omabox brief -b NAME names it and its windows"
        ));
    }
    if mons.is_empty() {
        out(
            "You drive an omabox box: a private desktop, not the user's. Start yours: omabox up --new [--plugin PATH]\n(all its options there); it prints its name (box-3): that is NAME below. It has no windows yet.\n",
        )?;
    } else {
        let main = crate::screen::main_screen(ctx);
        let first = run_jq(&first_jq(main), &b, &mons)?;
        let last = run_jq(WINDOWS_JQ, &b, &wins)?;
        out(cat!(first, "\n", last, "\n"))?;
    }
    let own: &[u8] = if b == b"NAME" {
        b"The box is yours: omabox down -b NAME when done; never down a box you did not start (your session's other agents use theirs)."
    } else {
        b"The box is your parent's: leave it up, and never down another box you did not start."
    };
    out(cat!(
        "Every call: omabox -b ",
        b,
        " CMD ..., one literal command per call: no $VAR, $(...), pipes, && or ;. Look a value up, then type it.\n",
        "The loop: look, act once, look again. Text first (omabox -b ",
        b,
        " windows, tree --find TEXT --window SEL); pixels: shot --window SEL prints a PNG path: Read it.\n",
        "Act: run -d --wait -- APP; keys --window SEL --wait -t TEXT Return; click --window SEL X Y (window coordinates, as in its shot); after a -g or --fit shot, click --in THAT.png X Y.\n",
        "Exit 0 from keys or click means sent, not landed: never resend input you have not seen land (124 after --wait: it WAS sent; shot first).\n",
        "No sleep: the action's --wait, wait still, wait change, wait window SEL [--gone], wait until -- CMD (never wait cmd); in a video, record hold 2s.\n",
        "Never the real desktop: no hyprctl, grim, wtype, ydotool or GUI app of your own; omabox -b ",
        b,
        " hyprctl ..., omabox -b ",
        b,
        " run -- CMD instead.\n",
        "Box files: the box's /home/sbx is ",
        home,
        " on the host: Read, Write and Edit there, never $(omabox path) in a command.\n",
        "Several shell steps in the box: write them to a script there with your file tool, then omabox -b ",
        b,
        " run -- sh /home/sbx/F.sh.\n",
        "Stay in your worktree: no cd out of it, no git on another checkout (absolute paths instead). Scroll with scroll X Y DY --source wheel.\n",
        own,
        " Report what you saw, and what a box cannot show (real hardware, the installed package).\n",
        "One command's options: omabox help CMD. If this brief is not enough, load the omabox skill.\n"
    ))
}

/// `$(jq -r --arg b "$b" PROGRAM <<<"$INPUT")`: its lines, or jq's failure as bash's set -e ends on
/// it (jq's words said, its status).
fn run_jq(prog: &str, b: &[u8], input: &[u8]) -> Res<Vec<u8>> {
    let r = Args::new(&["-r"])
        .kv("--arg", "b", b)
        .arg(prog)
        .run(Some(&crate::shell::strip_nl(input)), jq::Err::Inherit);
    if !r.ok() {
        return Err(Exit(u8::try_from(r.rc).unwrap_or(1)));
    }
    Ok(r.text())
}
