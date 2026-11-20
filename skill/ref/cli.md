# omabox: exit codes and JSON

The safety rules are in `SKILL.md` ("Never"), some spelled out here; nothing here relaxes them.

Read it for what an exit code means, or the fields of a command's `--json`.

Sections:

- Exit codes
- JSON

## Exit codes

The same in every command (`omabox help exit-codes`):

| Code | Means |
|---|---|
| 0 | Done, or the condition held (`wait`, `events --until`; `check run`: nothing changed). |
| 1 | It ran and failed, or the answer is no: no such box up, a refusal, `wait`'s `unknown` (the box went down), `tree` finding nothing, `which`'s `not in a box`; `check`: a checkpoint changed. |
| 2 | Called wrong: an unknown option or command (the nearest one is named: "did you mean --window?"), a value missing or not of its form, a word too many or too few. The message says which, and `omabox help CMD`: fix the call, do not retry it. Also `--window SEL` matching no window or several (they are listed), `which PID` for a process gone, every error of `check`'s. |
| 3, 4 | `check run`: the scenario itself failed / no baseline to compare with. |
| 124 | Not in time: `wait`, `--wait` (keys, click, scroll, drag, `run -d`), `events --until`. |
| 125 | `run`: omabox's own failure (no box, a bad option, a throwaway that did not start, the box going down while the command ran), never the command's. |
| 126, 127 | `run`: the command could not be run / was not found in the box. |
| N | `run`, `hyprctl`: the command's own status, passed through; `run -d`: the job's, when it ended at once (`--wait`: before the screen settled). |
| 128+N | Ended by signal N (130: Ctrl-C). |

An option that takes a value takes it as `--opt VALUE` or `--opt=VALUE`, in every command.
A form on its way out still works for one minor release, with a warning on stderr (`omabox:
deprecated: OLD: write NEW`): write NEW from then on. It is refused (2) after that; `OMABOX_STRICT=1`
refuses it at once (2), for CI.

## JSON

What `--json` gives, by its top-level fields. These are kept from 1.0: a field may be added, never
renamed, dropped or given another type; read the ones you need by name. Field order, whitespace and
the words inside `message` are not part of it. Snake_case everywhere; rectangles are `x`, `y`, `w`,
`h`; durations seconds; sizes in kB (`_kb`). One object or array on stdout unless said "a line each".

| Command | Shape |
|---|---|
| `up --json`, `ls --json` | a box (an array of them for `ls`): `name, state, mode, size, scale, created, dir, net, allowed, addrs, idle, plugins, plugin_status, bar, theme, render, monitors, hyprland_version, quickshell_version, omarchy_version, aquamarine, clean, starter, hung, peeking, keys_to_box, shell_state, shell_crashes, orphan` and more |
| `jobs --json` | an array, oldest first: `pid, age` (seconds), `processes, log, argv` |
| `windows --json` | an array: `address, stableId, class, title, initialClass, initialTitle, pid, workspace, x, y, w, h, at, size, floating, fullscreen, fullscreenMode, xwayland, active, onscreen, cover, layers` (Hyprland's own names kept: selectors use them; `fullscreen` is true when fullscreen or maximized, `fullscreenMode` says which: `none`, `maximized`, `fullscreen`); each of `layers`: `namespace, pid, layer, x, y, w, h, at, size, monitor, certain, span` (`certain` false: a layer spanning the screen, which may take input only where its items are, so it may not cover the window at all; `span` then says how: `screen`, or the edge a full-height or full-width strip is at, `right`, `left`, `top`, `bottom`, else `height` or `width`; null on a certain one) |
| `tree --json` | an array of nodes: `depth, role, name, states`, and `text, value, x, y, w, h, path` when known |
| `tray --json` | an array of items: `id, title, status, category, service, path, menu` (null: none), `item_is_menu, rows` (each `id, label, role, enabled, visible, toggle, checked, rows`); `tray menu --json`: one item |
| `pixel --json` | a line each point: `x, y, hex, r, g, b` |
| `find --json` | `image, similar, matches` (an array in reading order: `x, y, w, h, middle` ([x, y]), `score`, and `window` (`address, x, y`: the window's own coordinates) with `--window`), `more` (true: past 1000, not all listed), `best` (the best place seen, a match's shape, null for none), `near_tie` (a scaled template's one match with another place within 0.05 of it: that place, a match's shape; null for none) |
| `lua --json` | a line each value Lua returned (any JSON value) |
| `wait --json`, `--wait --json` | `result, condition, message, elapsed`; still/change/settle add `first_change, last_change, change, late_changes, ignored, frames, blinking`; window/layer/job/log/tree/until add `detail`; `log LOG` also `log, offset` (where the line found ends: the next wait's `--since`; null for hyprland); `notification` also `log, offset, notification` (the record, as `log notifications --json` has it; null when none came); `for` has `detail` null; `--expect` adds `expect` (`tree, met, detail`: `met` null when the screen did not settle); `--tree` adds `tree` (`window, changes, why`: each change `kind` changed, appeared or vanished, `role, name, path, line`, and `name_was, text, value` (`from, to`), `states_added, states_removed, hidden, inside` when they apply; `changes` null and `why` when there was no tree to compare); an action's `--wait` adds `changes` (an array, `[]` none, null not read: each `kind`, more kinds may come, ignore unknown ones), `target` (`kind` window, layer or none, `window, namespace, at, certain`: false when a layer spanning the screen over the point may have taken it), `events_since` (an `events --since` offset); `run -d --wait --json` adds `job` (the job's pid, printed alone otherwise) |
| `events --json` | a line each event: `time, event, data`; `--mark --json`: `mark, offset` |
| `log notifications --json` | a line each record: `time, event` (notify, close, closed, action, omabox); notify: `id` (null: none came), `error` (null, or why there is no id), `serial` (the call's, when it wanted a reply), `reply_time` (only when the id came after the 5 s the recorder waits: joined from the late reply's own line), `sender, app_name, replaces_id, app_icon, summary, body, actions` (each `key, label`), `hints` (a basic value each; a structured one, image-data, as its D-Bus signature), `expire_timeout`; close: `id, sender`; closed: `id, reason` (1 expired, 2 dismissed, 3 CloseNotification); action: `id, action`; omabox: `note` |
| `log commands --json` | a line each command run against the box: `time, box, command, args` (the words, `<hidden: N bytes>` or `KEY=<hidden>` for what is never logged), `read` (it only looked), `exit, secs, pid, ppid, jailed`; `session` (the agent's, when known), `jail` (when jailed), `error` (its last line, when it failed) |
| `shot --json` | `path, kind` (screen, region, monitor, window), `region` (the part taken: layout coordinates, a window's own for a window), `image` (`w, h`), `scale` (image px per layout px), `window, monitor, fit, zoom, grid` (`--grid`: `n, margin, file` (`w, h`), `columns` (`name, x, w`), `rows` (`name, y, h`)), `changed` (`--changed`: `result` changed, nothing or whole, `since, why`) |
| `check run --json` | `result` (same, changed), `base, shots, report, report_scaled, setup, only, counts` (`skipped`: not named by `--only`), `checkpoints` (each `id, result, as, size, regions, holes, tree, specks, marked, approved_holes, still`); exit 3 or 4: `result` failed or no_baseline |
| `check diff --json` | `result, size, regions, specks, marked, report, report_scaled` |
| `record status --json` | `recording, stopped, monitors, fps, file, audio, started` |
| `mode --json` | `name, mode, scale` |
| `window --json` | the window, as an element of `windows --json` |
| `dock --json` | `enabled, position, autohide, monitors` (each `name, workspace, layout, fullscreen, layer` (`at, size`, or null), `state` (shown, tucked, hidden, off, none), `input` (icons, edge, none)) |
| `monitor list --json` | an array: `name, mode, scale, x, y, workspace` (`dropped` for one `output drop` took) |
| `gpu --json` | `box, seconds, source, render, percent, engines` and more |
| `cpu --json` | `box, seconds, settle, cores, percent, processes, total` and more |
| `audio --json` | `box, seconds, sound, sound_s, peak_db, rms_db, spans, levels` and more |
| `polkit ask --json` | `result` (authenticated, not-authenticated, cancelled, timeout, agent-gone, no-agent), `tries, wrong, agent, path, action, why`; `polkit agents --json`: an array, `subject, name, path, fallback` |
| `ports --json` | an array: `box, port, addresses, process, host` |
| `path --logs --json` | `{NAME: PATH}`, each log there is |
| `saves --json` | an array: `name, box, saved, size, size_kb, keyring` |
| `config --json` | `workspace, confirm_close, bar_icon, gpu, shot_fit, protect_ports, desktop, confirm_close_available, version, gpus, gpu_auto, gpu_now, hyprland` (OMABOX_HYPRLAND's path, or null) |
| `calibrate --answer --json` | `result` (one, fit, up, none), `card, marks, scale, fit, fit_size, error_px, worst, off, shot_fit` |
| `which --json` | `pid, box, by` (pidns or env; `box` null: not in one) |
| `pid --json` | `box, process` (hyprland or shell), `pid` (the host's), `box_pid` (the box's own) |
| `--version --json` | `version, aquamarine` (`kind, version, fixed, message`), `hyprland` (null, or with OMABOX_HYPRLAND set `given, binary, message`) |
| `doctor --json` | `ok, os` (`id, id_like, family`: null for one omabox has no package names for), `checks` (an array: `id, status` ok, info (what this machine is: no Omarchy), warn or fail, `found, fix` an array of lines), `smoke` (null without `--smoke`; else `ok, box, shot, error`) |
