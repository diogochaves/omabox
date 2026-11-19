---
name: omabox-feedback
description: "Report what omabox got wrong in this session, only when the user asks. Reads back through the session (in Claude Code, every subagent it ran too) for each omabox bug, rough edge, missing feature or misleading doc that got in the way, checks each against omabox's GitHub issues, drafts each as a public issue or comment with anything sensitive taken out, and files them after one yes from the user. Use only when the user types /omabox-feedback (/omabox-feedback auto files without asking) or asks for it (\"anything for omabox?\", \"report this to omabox\"); never on your own initiative, never at the end of a session unasked."
---

# omabox feedback: what got in the way, filed

For anyone who used omabox in a session and wants to report what got in the way, the maintainer
included. It finds where omabox failed, got in the way, lacked something or misled, checks what is
already reported, and files the rest on [diogochaves/omabox](https://github.com/diogochaves/omabox),
a **public** repo. The user opted in to this skill (`omabox setup --feedback-skill`).

**Run it only when the user asks** (`/omabox-feedback`, or in words): never on your own initiative,
never as an end-of-session habit, never suggested unprompted. **Nothing is posted before the user
says yes** (step 6), unless they ran `/omabox-feedback auto`. Every draft is scrubbed of anything
sensitive (step 5) before the user sees it.

## 1. Gather

Go back through the whole conversation, compacted summaries included. List every moment, yours or a
subagent's, where omabox:

- **failed or misbehaved:** an error, a crash, a box that died, would not start or would not come
  down, a hang, a wrong exit code, output that did not match what happened;
- **got in the way:** a refusal or the guard (`omabox-guard`, `omabox host`) blocking something
  legitimate, or letting through something it should have stopped; a workaround (retries, sleeps,
  hand-written polling loops, `pgrep` dances, reading logs by hand);
- **was missing something:** steps done by hand (several commands, a script, a loop) that one
  omabox command could have done;
- **misled:** the `omabox` skill (`SKILL.md`, its `ref/` pages), `omabox help`, the README or an error
  message said something wrong or unclear, or left out what was needed;
- **was slow or expensive** in a way that mattered (a cold start, a render, a whole suite run to
  check one thing).

For each, note the command, what it printed (exact if short), what was expected, and the
workaround. Leave out mistakes that were the agent's alone (a wrong flag the docs had right) and
problems in the project under test rather than in omabox.

### 1a. Subagents (Claude Code)

A subagent's work never reaches the conversation, only its final report. Claude Code keeps each
whole transcript on disk, and `digest.py` (next to this file) cuts them down to the omabox moments.
From the directory the session started in:

```bash
python3 -I ~/.claude/skills/omabox-feedback/digest.py --out <scratch dir>/omabox-digest
```

It takes the newest session there (`--session ID` for another; `--help`) and writes one digest per
transcript, `session.txt` the conversation itself: each command that ran omabox or a desktop tool,
with its result (`FAILED?` when it looks failed, `GUARD` when the guard answered), and the agent's
own words about the box, the guard or a workaround. It prints a table with each digest's size.
`FAILED?` is a guess: many are the project's own test failures, an exit the docs promise, or an
agent's slip. Read the context.

- **Under about 100 K in all:** read them yourself.
- **More:** if your agent can run subagents, split the digests by size into at most four groups, one
  subagent each, in parallel. Give each the paths, step 1's list of what counts and what to leave
  out, and: "Report each finding as: transcript and `[n]`; the command; what it printed (exact,
  short); what was expected; the workaround; whether it cost time. Read the `.jsonl` named in the
  digest's header only to settle a doubtful one. No findings: say so. Don't file or search
  anything." Otherwise read them yourself, largest first.

Merge: the same problem met by several agents is one finding, with every place it showed up.

Other agents (Codex, OpenCode, pi, Hermes): skip 1a, unless you can read your own subagents' logs;
the conversation is the sweep.

Found nothing? Say so in one line and stop. Never make up findings to fill a list.

## 2. Safety first

Did a box reach the real desktop, the real session, real files, the real seat or the user's real
services, or did the guard fail to stop something that touched them? **Never file that publicly.**
Tell the user first, with the evidence, and point them at SECURITY.md: GitHub's private
vulnerability reporting, https://github.com/diogochaves/omabox/security/advisories/new. Leave it out
of everything below.

## 3. Check what is already there

For each finding:

```bash
gh issue list -R diogochaves/omabox --state all --search "<two or three key words>" --limit 10
```

Also grep omabox's `NOTES.md` (findings, the Open list) and `UPSTREAM.md`. They sit in the tree of its
binary: `readlink -f "$(command -v omabox)"` is `<dir>/build/omabox` in a checkout or `<dir>/bin/omabox`
in a package (`/usr/lib/omabox`), so `<dir>/NOTES.md` either way. Not there: skip the grep.

- **An open issue matches:** propose a comment only if this session adds something (another trigger,
  a version, a smaller repro). Otherwise `known`, with its link.
- **A closed issue matches:** came back? Propose a new issue that links the old one. Closed as
  won't-fix or by design: `known`, say so.
- **A known gap in NOTES.md:** `known`, unless this session shows it costs more than the note says.

## 4. Draft

Write each proposed issue or comment to its own files in a scratch dir: `N.title` (issues only),
`N.md` (the body). Each must be fit to post as it stands:

- Title: one plain sentence naming the behaviour, as the existing issues do ("wait still: a long
  thin change is read as a caret and ignored").
- Body: what happens; why, if known, and how you know; what was expected; the workaround; and
  **Wanted:** what would fix it. Short, no headers unless it is long. One issue per problem; two
  symptoms with one cause are one issue. Say how many times or agents it hit, if more than once.
- CONTRIBUTING's version lines: `omabox --version` (both lines), `Hyprland --version | head -1`,
  `omarchy version`, the GPU driver (`ls -l /sys/class/drm/renderD*/device/driver`), the command and
  what it printed. Box log lines only the ones that matter.
- Label, one of: `bug`, `enhancement`, `documentation` (the skill or docs), `accessibility`.

## 5. Scrub (every draft, before anything is shown)

Abstract away anything that may be sensitive or private:

- home paths become `~` (`omabox --version`'s second line has one); the user's name, username,
  hostname and email go;
- tokens, keys, passwords, cookies, IPs, private URLs and hostnames go;
- the names of the user's own projects, repos (`gh repo list --visibility private` lists the private
  ones), plugins and apps under test become what they are: "a Qt Quick app's add dialog", "a shell
  plugin's bar widget", "a project's test script";
- the *content* the session handled goes too: file names, window titles, document text, media or
  download names, chat text. Describe the scenario, not the data.

Then check mechanically: grep the drafts for every value below that is not empty, and fix each real
hit (omabox's own public names, `diogochaves/omabox` and `chaves.omabox`, stay):

```bash
for p in "$HOME" "$USER" "$(uname -n)" "$(git config user.email)" "$(basename "$PWD")" \
  $(gh repo list --visibility private --limit 200 --json name -q '.[].name' 2>/dev/null); do
  [ -n "$p" ] && grep -nIiF -- "$p" <scratch dir>/*.title <scratch dir>/*.md
done
```

(`basename "$PWD"` is the project's name; skip it when the project is omabox.)

## 6. Show, ask once

A short numbered table, not the drafts:

| # | Proposal | Finding | Label | Why |
|---|---|---|---|---|
| 1 | new issue | `up --theme` ignores a linked theme's edits | bug | cost a box restart and 10 min |
| 2 | comment on #180 | `wait still` misses a thin bar | | a second, smaller repro |
| 3 | known (#212) | IgnoreAction on drag-out | | already says it all |

Then ask once: "File all N? **y** files them all · **show 2** / **show all** prints a draft as it
would be posted · **1 3** files only those · or just say what to change." In Claude Code, the
AskUserQuestion tool fits: "File all (Recommended)", "Show me the drafts first", "Only some", "Don't
file". The user can always answer in their own words instead.

- **show:** print those drafts exactly as they would be posted (title, label, body), then ask again.
- **a change** (reword, drop an item, merge two, a different label): update the drafts, scrub again
  (step 5), and ask once more.
- **no:** file nothing; say where the drafts are.

`/omabox-feedback auto` (only when the user passed it): skip the question, file every `new issue`
and `comment`, and report.

## 7. File

```bash
gh issue create -R diogochaves/omabox --title "$(cat N.title)" --label bug --body-file N.md
gh issue comment -R diogochaves/omabox 180 --body-file N.md
```

Without triage rights a label can be refused: if `gh` errors on the label, file again without it.

## 8. Report

Each issue and comment URL, numbered as in the table; the `known` ones with their links; and the
safety item from step 2, if any, again at the top.
