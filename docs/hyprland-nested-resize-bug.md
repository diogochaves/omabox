# Hyprland bug: a nested Hyprland ignores its window being resized

Found while building omabox (NOTES.md finding 28), 2026-09-23.
Notes for an upstream report: how to reproduce it, and what a fix would need.

## Summary

When Hyprland runs nested (Wayland backend: a window inside another compositor) and that window is
resized, the nested Hyprland takes the new size as its output's mode (`hyprctl monitors` shows it) but
**does not re-arrange anything for it**. Layer surfaces (bars, wallpapers) keep their old size and tiled
windows keep their old geometry. What's on screen is a crop of stale content until something unrelated
triggers a re-layout (clicking a bar item, or `hyprctl reload`).

## Environment

- Hyprland 0.56.2 (Arch `hyprland 0.56.2-2`), Lua config
- aquamarine 0.15.0 (Arch) — **also** reproduced with aquamarine master @ 7bb8bdf4 (#415), so it is not
  the aquamarine configure fix
- Parent compositor: Hyprland 0.56.2 (the same happens with the parent being the user's real session)
- AMD iGPU, Omarchy 4.0.4, kernel 7.2.5

## Reproduce (minimal, no Omarchy, no omabox logic)

`min.lua`:
```lua
hl.monitor({ output = "WAYLAND-1", mode = "preferred", position = "0x0", scale = 1 })
hl.config({ misc = { disable_watchdog_warning = true } })
```

`bar.qml` (any layer-shell client works; this is quickshell):
```qml
import Quickshell
import QtQuick
PanelWindow { anchors { top: true; left: true; right: true } implicitHeight: 30; color: "red" }
```

From a terminal in a running Hyprland session:
```bash
Hyprland --config ./min.lua &            # nested: opens a window, its socket is the next wayland-N
WAYLAND_DISPLAY=wayland-2 quickshell -p ./bar.qml &     # adjust wayland-N
WAYLAND_DISPLAY=wayland-2 foot &
# float the nested window in the parent and resize it (mouse, or from the parent:)
hyprctl dispatch "hl.dsp.window.float({ window = 'class:aquamarine' })"
hyprctl dispatch "hl.dsp.window.resize({ window = 'class:aquamarine', x = 1000, y = 600 })"
# then ask the nested instance:
hyprctl -i 1 -j monitors | jq '.[0] | [.width, .height]'
hyprctl -i 1 -j layers   | jq '[.. | objects | select(.namespace?) | .w]'
hyprctl -i 1 -j clients  | jq '.[].size'
```

Invisible variant (how it was actually measured, nothing on the real desktop): `omabox up fakehost`,
then run the same inside it with `omabox run -b fakehost -- …` and drive the parent with
`omabox hyprctl -b fakehost …` (see omabox NOTES.md finding 26 for the nested-host technique).

## Observed

| Step | nested monitor | layer width | tiled foot |
|---|---|---|---|
| start | 1896x1019 | 1896 | fits |
| resize to 1000x600 | **1000x600** | **1896** (stale) | 958x528 (tiled when it opened at this size) |
| resize to 1500x900 | **1500x900** | **1896** (stale) | **958x528** (stale) |

Identical with system aquamarine 0.15.0 and patched aquamarine 7bb8bdf4.

Things that do **not** fix it: pointer motion into the window, `hl.dsp.force_renderer_reload()`,
re-applying the monitor rule with `hyprctl eval 'hl.monitor{…}'` (even with the explicit new mode),
setting an unrelated config value. `hyprctl reload` does fix it.

## Where it goes wrong (reading v0.56.2 source)

- aquamarine `src/backend/Wayland.cpp`: every `xdg_surface.configure` ends in `applyConfigure()`, which
  emits `events.state` with the new pixel size. That part works (the mode does change).
- Hyprland `src/output/Monitor.cpp` ~line 215, `m_listeners.state`: for `m_createdByUser` outputs
  (Wayland/headless backends) it sets `m_forceSize` and calls `applyMonitorRule(rule)` with the new
  resolution.
- `applyMonitorRule()` (~line 719) applies the mode, `applyMonitorRuleSoft()`, then emits
  `m_events.modeChanged`. **Nothing in that path re-arranges**: the only `modeChanged` listeners are
  protocol code (core/Output, OutputManagement, OutputPower, SessionLock, screenshare, PointerManager,
  ProtocolManager); none are the layout manager or the renderer.
- The re-arrange happens in `onConnect` (~lines 350-351):
  `g_pHyprRenderer->arrangeLayersForMonitor(m_id); g_layoutManager->recalculateMonitor(m_self.lock());`
  and in the config-reload path, which is why `hyprctl reload` "fixes" it.

## Likely fix (hypothesis, to verify on the fork)

After `applyMonitorRule(...)` in the `events.state` listener (both branches), re-arrange the monitor:

```cpp
g_pHyprRenderer->arrangeLayersForMonitor(m_id);
g_layoutManager->recalculateMonitor(m_self.lock());
```

Or, more generally, do it wherever a monitor's size changes (e.g. at the end of `applyMonitorRule`
when the logical size changed), which would also cover `hl.monitor{}` applied at runtime from Lua,
which showed the same staleness here. Check how upstream wants it: a `modeChanged` listener in the
layout manager might be the idiomatic place. Also check whether `m_forceFullFrames`/damage is needed so
the first frame after the resize is not a stale one.

## Before reporting

- Search issues/PRs once more (none found on 2026-09-23 for "nested resize", "wayland backend resize",
  "layers not rearranged"). A candidate origin is the monitor state refactor, PR #14547 (May 2026):
  worth checking whether 0.55 (or pre-#14547) re-arranged on resize.
- Test against Hyprland `main`, not just 0.56.2.
- Upstream's issue template asks for `hyprctl systeminfo` and a log (`$XDG_RUNTIME_DIR/hypr/<sig>/hyprland.log`
  of the nested instance, with `debug.disable_logs = false`).

## omabox's workaround (drop when fixed)

`omabox/share/hyprland.lua`, interactive branch: a 250 ms `hl.timer` watches the `WAYLAND-1` size and runs
`hyprctl reload` when it changes. Works, costs a brief black frame per resize.
