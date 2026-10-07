import QtQuick
import QtQuick.Controls
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui

// omabox in the bar: an icon while any box exists, and a panel listing them.
// The panel only displays `omabox ls --json` and runs omabox for actions; the
// CLI owns every rule (peek opens on workspace 9, down kills the namespace).
// Its settings are the CLI's too (`omabox config`, ~/.config/omabox/config), so
// the panel and the command line never disagree.
Panel {
  id: root
  moduleName: "chaves.omabox"
  ipcTarget: "chaves.omabox"

  // Hand-edited settings: an empty command, or an interval like "5s" (NaN made a 0 ms timer that
  // forked omabox back to back), fall back to the defaults.
  readonly property string command: String(setting("command", "omabox") || "omabox")
  readonly property int refreshSec: {
    var n = Number(setting("refreshIntervalSec", 5))
    return isFinite(n) && n >= 1 ? Math.min(Math.round(n), 300) : 5
  }
  // How long one `omabox ls --json` may take before it is stopped and counted as a failed poll
  // (finding 170). It answers in well under a second (65 ms with two boxes); 15 s is far past an
  // answer that is merely slow (a loaded machine, many boxes), and short enough that a hang shows in
  // the panel while the user still looks. Not in the manifest's schema: a hand-edited
  // "listTimeoutSec" in the widget's shell.json entry (the suite sets 2).
  readonly property int listTimeoutSec: {
    var n = Number(setting("listTimeoutSec", 15))
    return isFinite(n) && n >= 1 ? Math.min(Math.round(n), 300) : 15
  }

  // The bar's colours and font, or the theme's while the bar is null: a `bar.layout` change rebuilds
  // every widget, the new before the old are deleted, and the old ones' bindings read a bar already gone.
  readonly property color foreground: bar ? bar.foreground : Color.foreground
  readonly property color urgent: bar ? bar.urgent : Color.urgent
  readonly property string fontFamily: bar ? bar.fontFamily : Style.font.family

  property var boxes: []               // the last `omabox ls --json`
  // What the list shows (#117): the boxes' names in order, and each one's data. While the pointer is
  // over the panel nothing moves under it: rows update in place, a box that went stays in its slot
  // (marked gone, its actions off), and a new one is only counted in the header until the pointer
  // leaves (or the panel closes). Rows are built again only when the names change.
  property var rows: []
  property var rowData: ({})
  property int newCount: 0
  readonly property bool held: opened && holdHover.hovered
  onHeldChanged: syncRows(false)
  property int selectedIndex: 0
  property string selectedName: ""     // the selection follows the box, not its place in the list
  property bool cursorActive: false
  property string armedDown: ""        // box whose Down was pressed once; a second press confirms
  property bool armedNew: false        // n pressed once: a second n starts a box (a stray key does not)
  // When Down or n was armed: a second press sooner than confirmGapMs is the same gesture (a
  // double-click, a key bounced or typed twice), not a confirmation (#120). It stays armed.
  property real armedAt: 0
  readonly property int confirmGapMs: 400
  property var pendingSet: null        // a setting picked while the last one was still being written
  property string busy: ""             // box an action is running for
  property string lastError: ""        // kept until the next action succeeds
  property string listError: ""        // `omabox ls` itself failing: the list shown may be stale
  property int listFailures: 0
  property string action: ""           // what actionProc is doing: peek, shot, clip, clip out, down, keys-to-box
  property real now: Date.now()
  // The box SUPER keys go to right now (the host's "omabox" submap, one-shot or keys-to-box), or "":
  // what the CLI's Lua in your Hyprland writes to $XDG_RUNTIME_DIR/omabox/.keys on each change.
  property string keysFileBox: ""
  // ...when that box is up in the list: a file left by a Hyprland that went away never lights the icon.
  readonly property string keysBox:
    boxes.some(function(b) { return b.name === keysFileBox && b.state === "up" }) ? keysFileBox : ""
  // `omabox config --json`; the defaults until it answers. Not named `settings`: that is the base
  // Panel's, which the bar sets to this widget's shell.json entry for setting() (finding 169).
  property var cliConfig: ({ "workspace": "9", "confirm-close": "off", "bar-icon": "always" })
  // bar-icon: always (the default) keeps the icon, and the panel's settings, in the bar with no box
  // up, dimmed; auto shows it only while any box exists.
  readonly property bool alwaysShown: cliConfig["bar-icon"] !== "auto"
  readonly property bool shown: hasBoxes || alwaysShown
  readonly property string workspaceLabel: {
    var w = String(cliConfig.workspace || "9")
    return w === "special:scratchpad" ? "the scratchpad" : w.indexOf("special:") === 0 ? "special:" + w.slice(8) : "workspace " + w
  }
  readonly property var workspaceOptions: {
    var o = []
    for (var i = 1; i <= 10; i++) o.push({ value: String(i), label: "Workspace " + i })
    o.push({ value: "special:scratchpad", label: "Scratchpad (SUPER+S)" })
    var cur = String(cliConfig.workspace || "9")
    if (!o.some(function(x) { return x.value === cur }))   // set by hand: shown as it is
      o.push({ value: cur, label: cur.indexOf("special:") === 0 ? "Special: " + cur.slice(8) : "Workspace " + cur })
    return o
  }

  // The GPUs, for the picker in Settings and the rows' captions (#118): `config --json`'s list of every
  // GPU on the bus, the one `auto` takes and the one a headless box renders on now, by PCI slot.
  readonly property var gpus: Array.isArray(cliConfig.gpus) ? cliConfig.gpus : []
  readonly property string gpuSetting: String(cliConfig.gpu || "auto")
  readonly property bool gpuShown: gpus.length > 1 || gpuSetting !== "auto"
  function gpuBySlot(slot) { return gpus.find(function(g) { return g.pci === slot }) }
  function gpuName(g) { return g ? (g.name || (g.driver + " " + g.pci)) : "" }
  readonly property string gpuNowName: gpuName(gpuBySlot(cliConfig["gpu-now"])) || "no GPU"
  readonly property var gpuOptions: {
    var o = [{ value: "auto", label: "Auto (now: " + (gpuName(gpuBySlot(cliConfig["gpu-auto"])) || "none") + ")" }]
    gpus.forEach(function(g) {
      o.push({ value: g.pci, label: gpuName(g) + " · " + (g.available ? g.driver : "unavailable" + (g.why ? " (" + g.why + ")" : "")) })
    })
    var cur = gpuSetting
    if (!o.some(function(x) { return x.value === cur }))   // a kind, or a slot set by hand: shown as it is
      o.push({ value: cur, label: cur === "nvidia" ? "Any NVIDIA GPU" : cur === "amd" ? "Any AMD GPU" : cur === "intel" ? "Any Intel GPU" : cur })
    return o
  }
  // Why the setting's GPU is not there: the one at its slot, or (a kind, as `gpu nvidia`) each GPU of
  // that kind that is not usable, as `config --json` gives each one's kind (finding 237).
  readonly property string gpuWhy: {
    var s = gpuSetting
    var off = gpus.filter(function(g) { return (g.pci === s || g.kind === s) && !g.available && g.why })
    if (off.length === 1) return off[0].why
    return off.map(function(g) { return g.pci + ": " + g.why }).join("; ")
  }
  // More than one GPU a box can render on (a render node each), as `ls` counts them for its GPU line.
  readonly property bool gpuNodes: gpus.filter(function(g) { return !!g.node }).length > 1
  // The setting names a GPU and new boxes render elsewhere: it is not there now (vfio-pci, gone).
  readonly property bool gpuFallback: {
    if (gpuSetting === "auto") return false
    var now = gpuBySlot(cliConfig["gpu-now"])
    if (!now) return true
    var kinds = { nvidia: ["nvidia", "nouveau"], amd: ["amdgpu", "radeon"], intel: ["i915", "xe"] }
    return kinds[gpuSetting] ? kinds[gpuSetting].indexOf(now.driver) < 0 : now.pci !== gpuSetting
  }

  // The card's two faces, as omawin's: the box list, and Settings behind the gear at the top right,
  // which Back (or Esc) leaves.
  property string face: "list"
  readonly property string pluginVersion: "0.4.8"   // manifest.json's and VERSION
  // A shell does not reload a plugin when its files change (NOTES finding 41): after an upgrade the
  // bar runs this widget as it was, against the new CLI, until the shell restarts (finding 133).
  // `config --json` names the CLI's version; a difference is said once per version, dismissable.
  property string staleDismissed: ""
  readonly property string staleNote: {
    var v = cliConfig.version
    return v && v !== pluginVersion && v !== staleDismissed
      ? "omabox " + v + " is installed; this widget is " + pluginVersion + ". Restart the shell to load the new one: omarchy restart shell"
      : ""
  }

  readonly property var icons: ({
    gear: String.fromCodePoint(0xF013),     // fa-cog, omawin's
    back: String.fromCodePoint(0xF053),     // fa-chevron_left, omawin's
    add: String.fromCodePoint(0xF067),      // fa-plus
    peek: String.fromCodePoint(0xF0208),    // md-eye
    show: String.fromCodePoint(0xF0379),    // md-monitor
    shot: String.fromCodePoint(0xF0100),    // md-camera
    clipIn: String.fromCodePoint(0xF014A),  // md-clipboard_arrow_down
    clipOut: String.fromCodePoint(0xF0C57), // md-clipboard_arrow_up
    keys: String.fromCodePoint(0xF030C),    // md-keyboard
    down: String.fromCodePoint(0xF0159),    // md-close_circle
    alert: String.fromCodePoint(0xF0026),   // md-alert
    dismiss: String.fromCodePoint(0xF0156)  // md-close
  })

  readonly property bool hasBoxes: boxes.length > 0
  // The list's height when the screen is short (#121): the card's room less everything else on the
  // list face, and never under two rows.
  readonly property real listRoom: panel.availableCardHeight > 0
    ? Math.max(Style.space(80), panel.availableCardHeight - panel.verticalContentInset - hero.height
      - (alertStrip.visible ? alertStrip.height + column.spacing : 0) - column.spacing
      - listSep.height - newButton.height - hints.height - listFace.spacing * 3)
    : 100000
  readonly property int upCount: boxes.filter(function(b) { return b.state === "up" }).length
  readonly property int deadCount: boxes.length - upCount
  // The console convention (omarchy-console DESIGN.md §6), read by its rail: running while a box is up.
  readonly property bool consoleAwake: upCount > 0
  readonly property string consoleState: consoleAwake ? "running" : ""
  // One wording for the tooltip and the panel: the icon shows while any box exists, dead ones too.
  readonly property string countText: (upCount === 1 ? "1 box up" : upCount + " boxes up") + (deadCount ? " · " + deadCount + " dead" : "")
    + (newCount ? " · " + newCount + " new" : "")

  visible: shown
  implicitWidth: shown ? button.implicitWidth : 0
  implicitHeight: shown ? button.implicitHeight : 0

  onShownChanged: if (!shown) close()
  onOpenedChanged: {
    if (!opened) return
    // Opened (IPC, a keybinding) with nothing to show: stay closed, or the panel would pop up and take
    // the keyboard later, whenever an agent starts a box. Later: closing inside this handler is a
    // binding loop on `opened`, which then stays true.
    if (!shown) { Qt.callLater(close); return }
    armedDown = ""
    armedNew = false
    cursorActive = false
    face = "list"
    now = Date.now()
    refresh()
    configProc.running = true
  }

  function openFace(name) {
    face = name
    armedDown = ""
    armedNew = false
    cursorActive = false
    if (name === "settings") configProc.running = true
    keyCatcher.forceActiveFocus()
  }

  // Esc: back to the list from Settings, closed from the list.
  function goBack() {
    if (face === "list") close()
    else openFace("list")
  }

  // A new interactive box under a name the CLI picks (`up --new`: box-1, box-2, ...); once it is up,
  // its window is brought forward: the user asked for it.
  function newBox() {
    run("new box", "new", [command, "up", "--interactive", "--new"])
  }

  // A setting goes through the CLI, which checks it; the answer is read back either way.
  function setSetting(key, value) {
    if (setProc.running) { pendingSet = [key, value]; return }
    setProc.command = [command, "config", key, value]
    setProc.running = true
  }

  function refresh() {
    if (listProc.running) return
    listProc.exited = false
    listProc.timedOut = false
    listProc.running = true
  }

  function parseList(text) {
    var parsed
    try { parsed = JSON.parse(text) } catch (e) { parsed = null }
    if (!Array.isArray(parsed)) { listFailed("omabox ls --json: not a list"); return }
    listFailures = 0
    listError = ""
    boxes = parsed
    syncRows(false)
  }

  // The rows from the list (#117). Held (the pointer over the panel), the rows stay: their data is
  // the list's, a box gone from it keeps its last data marked gone, and new ones are counted. Not
  // held, or forced (the list emptied after failures), the rows are the list. The selection follows
  // its box by name, scrolled into view when the rows are not held.
  function syncRows(force) {
    var live = {}, names = []
    boxes.forEach(function(b) { live[b.name] = b; names.push(b.name) })
    var data = {}, fresh = 0
    if (held && !force) {
      rows.forEach(function(n) { data[n] = live[n] || Object.assign({}, rowData[n], { gone: true }) })
      fresh = names.filter(function(n) { return rows.indexOf(n) < 0 }).length
      if (armedDown !== "" && data[armedDown] && data[armedDown].gone) armedDown = ""
    } else {
      data = live
      if (JSON.stringify(names) !== JSON.stringify(rows)) rows = names
    }
    if (JSON.stringify(data) !== JSON.stringify(rowData)) rowData = data
    newCount = fresh
    var i = rows.indexOf(selectedName)
    select(i >= 0 ? i : Math.min(selectedIndex, Math.max(0, rows.length - 1)))
    if (!held) showSelected()
  }

  function showSelected() {
    if (rows.length > 0) boxList.positionViewAtIndex(selectedIndex, ListView.Contain)
  }

  // Keep the last list through a hiccup, but say so; after three failures in a row it is gone, and
  // with it the icon and the alert strip, so that one goes out as a notification.
  function listFailed(why) {
    listError = why
    if (++listFailures === 3) Quickshell.execDetached(["notify-send", "-a", "omabox", "omabox: cannot list boxes", why])
    if (listFailures >= 3) { boxes = []; syncRows(true) }
  }

  function select(i) {
    selectedIndex = i
    selectedName = rows[i] !== undefined ? rows[i] : ""
  }

  function age(created) {
    var t = Date.parse(created || "")
    if (isNaN(t)) return ""
    var m = Math.max(0, Math.floor((now - t) / 60000))
    if (m < 1) return "just now"
    if (m < 60) return m + "m"
    var h = Math.floor(m / 60)
    if (h < 24) return h + "h " + (m % 60) + "m"
    return Math.floor(h / 24) + "d " + (h % 24) + "h"
  }

  function caption(b) {
    if (b.gone) return "gone"
    if (armedDown === b.name) return "Press again to shut it down"
    if (b.state !== "up") return "dead · Down cleans it up"
    var parts = [b.mode || "?"]
    // Its GPU (#118), next to the mode, before what elides: as `ls` has it (finding 237), with more than
    // one render node or when it is a fallback; its driver and slot, two GPUs of one kind told apart.
    if (b.render && (gpuNodes || b.render.fallback))
      parts.push(b.render.driver + " " + String(b.render.pci || "").replace(/^0000:/, "") + (b.render.fallback ? " (fallback)" : ""))
    if (b.mode !== "interactive" && b.size) parts.push(b.size)
    var a = age(b.created); if (a) parts.push(a)
    if (b.plugins && b.plugins.length) parts.push(b.plugins.join(", "))
    if (b.net === "isolated") parts.push("isolated")
    if (b.peeking) parts.push("peeking")
    if (keysBox === b.name) parts.push("SUPER keys here")
    else if (b.keys_to_box) parts.push("keys follow focus")
    return parts.join(" · ")
  }

  // keys-to-box (finding 117): an interactive box's SUPER keys follow focus into its window, or not.
  function keysToBox(b) {
    if (!b || b.gone || b.state !== "up" || b.mode !== "interactive") return
    run(b.name, "keys-to-box", [command, "keys-to-box", "-b", b.name, b.keys_to_box ? "off" : "on"])
  }

  // One action at a time; a second one while it runs says so rather than vanish.
  function run(name, what, args) {
    if (actionProc.running) {
      lastError = "still busy: " + action + " " + busy
      return false
    }
    busy = name
    action = what
    actionProc.exited = false
    actionProc.command = args
    actionProc.running = true
    return true
  }

  // Peek at a headless box (or bring its peek forward); show an interactive one. On a dead box the
  // only thing to do is Down: arm it.
  function peek(b) {
    if (!b || b.gone) return
    if (b.state !== "up") { down(b); return }
    if (run(b.name, "peek", [command, "peek", "-b", b.name, "--focus"])) close()
  }

  // omabox shot only: the viewer opens detached, so an image left open does not hold up every later
  // action (xdg-open waits for the viewer on Hyprland).
  function shot(b) {
    if (!b || b.gone) return
    if (b.state !== "up") { down(b); return }
    if (run(b.name, "shot", [command, "shot", "-b", b.name])) close()
  }

  // Your clipboard into an interactive box, or the box's out (omabox clip, issue #23): one item, once.
  // Headless boxes are agents': the CLI refuses them, and the panel offers them nothing.
  function clip(b, out) {
    if (!b || b.gone || b.state !== "up" || b.mode !== "interactive") return
    var args = [command, "clip", "-b", b.name]
    if (out) args.push("--from-box")
    if (run(b.name, out ? "clip out" : "clip", args)) close()
  }

  function down(b) {
    if (!b || b.gone) return
    if (armedDown !== b.name) {
      armedDown = b.name
      armedAt = Date.now()
      disarm.restart()
      return
    }
    if (Date.now() - armedAt < confirmGapMs) return
    if (run(b.name, "down", [command, "down", b.name])) armedDown = ""
  }

  // n twice, with the same gap as Down's.
  function armNew() {
    if (!armedNew) { armedNew = true; armedAt = Date.now(); disarm.restart(); return }
    if (Date.now() - armedAt < confirmGapMs) return
    armedNew = false
    newBox()
  }

  function selected() { return rowData[rows[selectedIndex]] }

  Process {
    id: listProc
    // A command that cannot start (not on PATH) never emits exited: running just drops back.
    property bool exited: false
    // Stopped by listLimit: its exit (SIGTERM or SIGKILL) is that failure, not its exit code.
    property bool timedOut: false
    command: [root.command, "ls", "--json"]
    stdout: StdioCollector { id: listOut; waitForEnd: true }
    stderr: StdioCollector { id: listErr; waitForEnd: true }
    onStarted: listLimit.restart()
    onExited: function(code) {
      exited = true
      if (timedOut) root.listFailed("omabox ls --json did not answer in " + root.listTimeoutSec + " s")
      else if (code === 0) root.parseList(listOut.text)
      else root.listFailed((listErr.text.trim().split("\n").pop() || root.command + " ls: exit " + code))
    }
    onRunningChanged: if (!running) {
      listLimit.stop()
      listKill.stop()
      Qt.callLater(function() {
        if (!listProc.exited && !listProc.running) root.listFailed("cannot run " + root.command)
      })
    }
  }

  // refresh() skips a poll while one runs, so one `ls --json` that never answers froze the list for
  // good (finding 170: a FIFO in a box HOME, #95, from 15:33 until it was killed by hand). Past the
  // limit it is stopped with all it started. `running = false` alone is Quickshell's SIGTERM to that
  // one process (running stays true until it is gone; then exited, then runningChanged): its children,
  // such as the `head` blocked on that FIFO, would be left behind, one more each poll. So the tree
  // under it is collected first, then all of it gets SIGTERM and, 2 s later, SIGKILL; SIGKILL from
  // here 3 s on in case that could not run. Its exit is then this failure, not "cannot run".
  readonly property string treeKill: 't=$0 all=; while [ -n "$t" ]; do all="$all $t" n=; for p in $t; do n="$n $(pgrep -P "$p")"; done; t=$(echo $n); done; kill -TERM $all 2>/dev/null; sleep 2; kill -KILL $all 2>/dev/null'
  Timer {
    id: listLimit
    interval: root.listTimeoutSec * 1000
    onTriggered: {
      var pid = Number(listProc.processId)
      if (!listProc.running || !(pid > 1)) return
      listProc.timedOut = true
      Quickshell.execDetached(["sh", "-c", root.treeKill, String(pid)])
      listKill.restart()
    }
  }
  Timer { id: listKill; interval: 3000; onTriggered: if (listProc.running) listProc.signal(9) }

  Process {
    id: actionProc
    property bool exited: false
    stdout: StdioCollector { id: actionOut; waitForEnd: true }
    stderr: StdioCollector { id: actionErr; waitForEnd: true }
    onExited: function(code) {
      exited = true
      root.actionDone(code === 0 ? "" : (actionErr.text.trim().split("\n").pop() || ("exit " + code)), actionOut.text.trim(),
        actionErr.text.trim().split("\n").pop().replace(/^omabox: /, ""))
    }
    onRunningChanged: if (!running) Qt.callLater(function() {
      if (!actionProc.exited && !actionProc.running && root.busy !== "") root.actionDone("cannot run " + root.command, "", "")
    })
  }

  // Peek, shot and clip close the panel, so a failure also goes out as a notification; so does what a
  // clip handed over (its type and size, never the content).
  function actionDone(error, out, said) {
    lastError = error
    if (error !== "") Quickshell.execDetached(["notify-send", "-a", "omabox", "omabox " + (action === "new" ? "new box" : action + " " + busy), error])
    else if (action === "shot" && out !== "") Quickshell.execDetached(["xdg-open", out])
    else if ((action === "clip" || action === "clip out") && said) Quickshell.execDetached(["notify-send", "-a", "omabox", "omabox " + action + " " + busy, said])
    else if (action === "new" && /^[A-Za-z0-9][A-Za-z0-9_.-]*$/.test(out)) {
      // Brought forward only while the panel is still open: once it is closed the user has moved on,
      // and focusing the box would switch their workspace under them.
      if (opened) Quickshell.execDetached([command, "peek", "-b", out, "--focus"])
      else Quickshell.execDetached(["notify-send", "-a", "omabox", "omabox: " + out + " is up", "Its window is on " + workspaceLabel + "."])
      close()
    }
    busy = ""
    action = ""
    refresh()
  }

  Process {
    id: configProc
    command: [root.command, "config", "--json"]
    stdout: StdioCollector { id: configOut; waitForEnd: true }
    onExited: function(code) {
      if (code !== 0) return
      try {
        var c = JSON.parse(configOut.text)
        if (c && typeof c === "object") {
          root.cliConfig = c
          // A pick sets the Dropdown's own value, which ends its binding: show the setting again (a
          // refused pick, or `omabox config workspace N` since).
          wsDropdown.value = String(c.workspace || "9")
          if (gpuDropdown) gpuDropdown.value = String(c.gpu || "auto")
        }
      } catch (e) {}
    }
  }

  Process {
    id: setProc
    stderr: StdioCollector { id: setErr; waitForEnd: true }
    onExited: function(code) {
      root.lastError = code === 0 ? "" : (setErr.text.trim().split("\n").pop() || ("omabox config: exit " + code))
      configProc.running = true
      if (root.pendingSet) {
        var next = root.pendingSet
        root.pendingSet = null
        root.setSetting(next[0], next[1])
      }
    }
  }

  // bar-icon matters while the panel is closed: the settings are read at start and whenever the file
  // changes (`omabox config` renames a new one into place), not on every poll.
  Component.onCompleted: configProc.running = true
  FileView {
    path: Quickshell.env("HOME") + "/.config/omabox/config"
    watchChanges: true
    printErrors: false
    onFileChanged: if (!configProc.running) configProc.running = true
  }

  // Where SUPER keys go: watched, so the icon lights the moment the host enters the submap (the file is
  // renamed into place: text() is stale in the change signal, so through reload → onLoaded), and
  // read again at each poll too, in case a change was missed. Missing (no interactive box yet): none.
  FileView {
    id: keysFile
    path: Quickshell.env("XDG_RUNTIME_DIR") + "/omabox/.keys"
    watchChanges: true
    printErrors: false
    onFileChanged: reload()
    onLoaded: root.keysFileBox = text().trim()
    onLoadFailed: root.keysFileBox = ""
  }

  Timer {
    interval: root.opened ? 2000 : root.refreshSec * 1000
    running: true
    repeat: true
    triggeredOnStart: true
    onTriggered: { root.now = Date.now(); root.refresh(); keysFile.reload() }
  }

  Timer { id: disarm; interval: 3000; onTriggered: { root.armedDown = ""; root.armedNew = false } }

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    slotSize: Style.bar.iconSlot * (root.boxes.length > 1 && !vertical ? 2 : 1)
    // The mark in the 16 px icon canvas; the count, when there is one, hangs off
    // it inside the doubled slot, the whole row centred like the glyph was.
    iconComponent: Component {
      Item {
        Row {
          anchors.centerIn: parent
          spacing: Style.space(4)

          Mark {
            small: true
            width: Style.bar.iconCanvas
            height: Style.bar.iconCanvas
            color: button.active && button.useActiveColor ? button.activeColor : button.foreground
          }

          Text {   // every box, dead ones too: they are what keeps the icon in the bar
            visible: root.boxes.length > 1 && !button.vertical
            anchors.verticalCenter: parent.verticalCenter
            textFormat: Text.PlainText
            text: root.boxes.length
            color: button.active && button.useActiveColor ? button.activeColor : button.foreground
            font.family: button.fontFamily
            font.pixelSize: button.fontSize
          }
        }
      }
    }
    opacity: root.hasBoxes ? 1 : 0.5
    // Lit (the bar's attention colour, as the box window's border) while SUPER keys go to a box.
    active: root.keysBox !== ""
    tooltipText: root.opened ? "" : root.keysBox !== "" ? "SUPER keys go to " + root.keysBox
      : (root.hasBoxes ? root.countText : "No boxes")
    onPressed: function(b) { root.toggle() }
  }

  KeyboardPanel {
    id: panel
    anchorItem: button
    owner: root
    bar: root.bar
    open: root.opened && root.shown
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(400))
    contentHeight: panel.fittedContentHeight(column.implicitHeight)

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent

      // The pointer anywhere over the card holds the list still (#117).
      HoverHandler { id: holdHover }
      onMoveRequested: function(dx, dy) {
        if (root.face !== "list") return
        if (!root.cursorActive) { root.cursorActive = true; return }
        if (dy !== 0 && root.rows.length > 0) {
          root.select((root.selectedIndex + dy + root.rows.length) % root.rows.length)
          root.showSelected()
          root.armedDown = ""
        }
      }
      onActivateRequested: if (root.face === "list" && root.cursorActive) root.peek(root.selected())
      onDeleteRequested: if (root.face === "list" && root.cursorActive) root.down(root.selected())
      onCloseRequested: root.goBack()
      onTabRequested: function(direction) { root.switchPanel(direction) }
      onTextKey: function(t) {
        if (root.face !== "list") return
        if (t === "r") { root.refresh(); return }
        if (t === "n") {
          // Twice, like Down: a stray n (typed as if into a search) must not open a window.
          root.armNew()
          return
        }
        if (!root.cursorActive) return
        if (t === "p") root.peek(root.selected())
        else if (t === "s") root.shot(root.selected())
        else if (t === "v") root.clip(root.selected(), false)
        else if (t === "c") root.clip(root.selected(), true)
        else if (t === "d") root.down(root.selected())
        else if (t === "f") root.keysToBox(root.selected())
      }

      Column {
        id: column
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        spacing: Style.space(12)

        PanelHero {
          width: parent.width
          iconComponent: Component {
            Item {   // whole units of the 15-unit mark, near the display size (30 px at 24)
              width: 15 * Math.max(1, Math.round(Style.font.display / 15))
              height: width

              Mark {
                anchors.fill: parent
                color: root.foreground
              }
            }
          }
          id: hero
          title: root.face === "settings" ? "Settings" : "Boxes"
          meta: root.face === "settings" ? "omabox " + root.pluginVersion
            : (root.hasBoxes ? root.countText : "no boxes").toUpperCase()
          foreground: root.foreground
          fontFamily: root.fontFamily
          // omawin's: the gear into Settings, and Back out of it, share the top right.
          trailingControl: Component {
            Row {
              Button {
                visible: root.face === "list"
                anchors.verticalCenter: parent.verticalCenter
                iconText: root.icons.gear
                iconSize: Style.font.bodySmall
                fontSize: Style.font.caption
                verticalPadding: Style.spacing.xs
                horizontalPadding: Style.spacing.sm
                foreground: root.foreground
                fontFamily: root.fontFamily
                tooltipText: "Settings"
                opacity: 0.7
                onClicked: root.openFace("settings")
              }
              Button {
                visible: root.face !== "list"
                anchors.verticalCenter: parent.verticalCenter
                bordered: true
                iconText: root.icons.back
                text: "Back"
                iconSize: Style.font.bodySmall
                fontSize: Style.font.caption
                verticalPadding: Style.spacing.controlPaddingY
                horizontalPadding: Style.spacing.sm
                foreground: root.foreground
                fontFamily: root.fontFamily
                tooltipText: "Back · Esc"
                onClicked: root.goBack()
              }
            }
          }
        }

        // The last failure, full width in the urgent colour: a pill in the hero was too small to read.
        // A failing list comes first: everything below it may be stale.
        Rectangle {
          id: alertStrip
          visible: root.lastError !== "" || root.listError !== "" || root.staleNote !== ""
          width: parent.width
          implicitHeight: alertRow.implicitHeight + Style.space(12)
          radius: Style.cornerRadius
          color: Util.alpha(root.urgent, 0.14)
          border.width: 1
          border.color: Util.alpha(root.urgent, 0.55)

          Row {
            id: alertRow
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            anchors.leftMargin: Style.space(10)
            anchors.rightMargin: Style.space(4)
            spacing: Style.space(8)

            Text {
              id: alertIcon
              anchors.verticalCenter: parent.verticalCenter
              textFormat: Text.PlainText
              text: root.icons.alert
              color: root.urgent
              font.family: root.fontFamily
              font.pixelSize: Style.font.body
            }
            Text {
              width: parent.width - alertIcon.width - alertDismiss.width - parent.spacing * 2
              anchors.verticalCenter: parent.verticalCenter
              textFormat: Text.PlainText
              text: root.listError !== "" ? "list: " + root.listError : root.lastError !== "" ? root.lastError : root.staleNote
              color: root.urgent
              font.family: root.fontFamily
              font.pixelSize: Style.font.bodySmall
              wrapMode: Text.Wrap
            }
            PanelActionButton {
              id: alertDismiss
              anchors.verticalCenter: parent.verticalCenter
              iconText: root.icons.dismiss
              tooltipText: "Dismiss"
              foreground: root.urgent
              hoverColor: root.urgent
              fontFamily: root.fontFamily
              onClicked: {
                if (root.lastError === "" && root.listError === "") root.staleDismissed = root.cliConfig.version || ""
                root.lastError = ""; root.listError = ""
              }
            }
          }
        }

        // ============================= the list =============================
        Column {
          id: listFace
          visible: root.face === "list"
          width: parent.width
          spacing: Style.space(12)

          PanelSeparator { id: listSep; foreground: root.foreground }

          Text {
            visible: root.rows.length === 0
            width: parent.width
            textFormat: Text.PlainText
            text: "No boxes up"
            color: Qt.darker(root.foreground, 1.4)
            font.family: root.fontFamily
            font.pixelSize: Style.font.body
            horizontalAlignment: Text.AlignHCenter
          }

          // Only the rows scroll (#121): the list takes the height the screen leaves once the hero, an
          // alert, the New button and the hints are in, so those stay in view with any number of boxes.
          // A ListView, as Omarchy's long lists: the wheel scrolls it, and keys keep the selection in
          // view (showSelected); a row the pointer selects is not scrolled to, so nothing moves under it.
          ListView {
            id: boxList
            visible: root.rows.length > 0
            width: parent.width
            height: Math.min(contentHeight, root.listRoom)
            spacing: Style.space(4)
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            interactive: contentHeight > height
            ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }
            model: root.rows
            delegate: BoxRow {
              required property var modelData
              required property int index
              width: ListView.view.width
              box: root.rowData[modelData] || ({})
              rowIndex: index
            }
          }

          Button {
            id: newButton
            width: parent.width
            bordered: true
            iconText: root.icons.add
            text: root.action === "new" ? "Starting a box…" : root.armedNew ? "Press n again to start one" : "New interactive box"
            iconSize: Style.font.bodySmall
            fontSize: Style.font.body
            foreground: root.foreground
            fontFamily: root.fontFamily
            tooltipText: "A window on " + root.workspaceLabel + ", named box-1, box-2, …"
            enabled: !actionProc.running
            opacity: enabled ? 1.0 : 0.6
            onClicked: root.newBox()
          }

          Text {
            id: hints
            width: parent.width
            textFormat: Text.PlainText
            // No-break spaces keep each key with its action when the line wraps.
            text: ["↑↓ move", "p peek/show", "s shot", "f keys", "v paste in", "c copy out", "d down", "n new", "r refresh"]
              .map(function(h) { return h.replace(/ /g, "\u00a0") }).join("\u00a0· ")
            color: root.foreground
            opacity: 0.5
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
          }
        }

        // ============================= Settings =============================
        // The CLI's settings (`omabox config`), in omawin's rows: a bold line and a dim caption on
        // the left, the control on the right. Every change goes through the CLI, which checks it.
        Column {
          visible: root.face === "settings"
          width: parent.width
          spacing: Style.space(14)

          PanelSeparator { foreground: root.foreground }

          Column {
            width: parent.width
            spacing: Style.space(6)

            SettingText { text: "Where windows open"; bold: true }
            SettingText { text: "Interactive boxes and peek windows, never focused when they open. The scratchpad is SUPER+S."; dim: true }
            Dropdown {
              id: wsDropdown
              width: parent.width
              showLabel: false
              fontFamily: root.fontFamily
              options: root.workspaceOptions
              value: String(root.cliConfig.workspace || "9")
              onChanged: function(v) { root.setSetting("workspace", v) }
            }
          }

          // The GPU headless boxes render on (#118), on a machine with more than one (or with one named
          // in the settings, so it can be set back to Auto).
          Column {
            visible: root.gpuShown
            width: parent.width
            spacing: Style.space(6)

            SettingText { text: "GPU for agent boxes"; bold: true }
            SettingText { text: "Headless boxes render here. Interactive boxes always use the desktop's GPU."; dim: true }
            Dropdown {
              id: gpuDropdown
              width: parent.width
              showLabel: false
              fontFamily: root.fontFamily
              options: root.gpuOptions
              value: root.gpuSetting
              onChanged: function(v) { root.setSetting("gpu", v) }
            }
            SettingText {
              id: gpuLine
              text: root.gpuFallback ? "Not there now" + (root.gpuWhy ? " (" + root.gpuWhy + ")" : "") + ": new boxes render on "
                + root.gpuNowName + " until it is."
                : "New boxes render on " + root.gpuNowName + "."
              dim: !root.gpuFallback
              color: root.gpuFallback ? root.urgent : Qt.darker(root.foreground, 1.4)
            }
          }

          // Off and greyed while boxes cannot open their window again (an aquamarine without the fix,
          // NOTES finding 125): `config --json` says so, read again each time Settings opens.
          SettingSwitch {
            readonly property bool available: root.cliConfig["confirm-close-available"] !== false
            title: "Confirm before closing"
            caption: available ? "Closing an interactive box's window asks first; closing it again shuts the box down."
              : "Needs aquamarine's fix for nested Wayland outputs: run omabox setup --aquamarine, then open Settings again."
            checked: available && root.cliConfig["confirm-close"] === "on"
            enabled: available
            onToggled: if (available) root.setSetting("confirm-close", checked ? "off" : "on")
          }

          SettingSwitch {
            title: "Always show in the bar"
            caption: "Off: the icon only while a box is up (then back here, or: omabox config bar-icon always)."
            checked: root.alwaysShown
            onToggled: root.setSetting("bar-icon", checked ? "auto" : "always")
          }

          PanelSeparator { foreground: root.foreground }

          Column {
            width: parent.width
            spacing: Style.spacing.labelGap
            SettingPair { label: "Settings file"; value: "~/.config/omabox/config" }
            SettingPair { label: "Command"; value: "omabox config" }
          }
        }
      }
    }
  }

  // ------------------------------------------------------------- the pieces

  component SettingText: Text {
    property bool bold: false
    property bool dim: false
    width: parent ? parent.width : 0
    textFormat: Text.PlainText
    wrapMode: Text.WordWrap
    color: dim ? Qt.darker(root.foreground, 1.4) : root.foreground
    font.family: root.fontFamily
    font.pixelSize: Style.font.caption
    font.bold: bold
  }

  component SettingSwitch: Row {
    id: sw
    property string title: ""
    property string caption: ""
    property bool checked: false
    signal toggled()
    width: parent ? parent.width : 0
    spacing: Style.space(12)

    Column {
      width: sw.width - swSwitch.implicitWidth - sw.spacing
      anchors.verticalCenter: parent.verticalCenter
      spacing: Style.space(2)
      SettingText { text: sw.title + (sw.checked ? " · ON" : ""); bold: true }
      SettingText { text: sw.caption; dim: true }
    }
    ToggleSwitch {
      id: swSwitch
      anchors.verticalCenter: parent.verticalCenter
      checked: sw.checked
      interactive: sw.enabled
      opacity: sw.enabled ? 1 : 0.4
      busy: setProc.running
      foreground: root.foreground
      onToggled: sw.toggled()
    }
  }

  component SettingPair: Row {
    property string label: ""
    property string value: ""
    width: parent ? parent.width : 0
    spacing: Style.space(8)
    Text {
      textFormat: Text.PlainText
      text: parent.label
      color: root.foreground
      opacity: 0.6
      font.family: root.fontFamily
      font.pixelSize: Style.font.caption
    }
    Text {
      textFormat: Text.PlainText
      text: parent.value
      color: Qt.darker(root.foreground, 1.4)
      font.family: root.fontFamily
      font.pixelSize: Style.font.caption
      font.bold: true
    }
  }

  component BoxRow: CursorSurface {
    id: row
    property var box: ({})
    property int rowIndex: 0
    readonly property bool gone: box.gone === true   // gone from the list while it was held (#117)
    readonly property bool up: box.state === "up" && !gone
    readonly property bool interactive: box.mode === "interactive"
    readonly property bool rowSelected: root.cursorActive && root.selectedIndex === rowIndex
    readonly property bool armed: root.armedDown === box.name

    hasCursor: rowSelected
    foreground: root.foreground
    opacity: gone ? 0.45 : 1
    implicitHeight: content.implicitHeight + Style.spacing.rowPaddingX

    MouseArea {
      id: rowMouse
      anchors.fill: parent
      hoverEnabled: true
      cursorShape: row.up ? Qt.PointingHandCursor : Qt.ArrowCursor
      onContainsMouseChanged: if (containsMouse) {
        root.cursorActive = true
        root.select(row.rowIndex)
      }
      onClicked: root.peek(row.box)
    }

    Item {
      id: content
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.verticalCenter: parent.verticalCenter
      anchors.leftMargin: Style.space(10)
      anchors.rightMargin: Style.space(6)
      implicitHeight: Math.max(info.implicitHeight, actions.implicitHeight)

      Column {
        id: info
        anchors.left: parent.left
        anchors.right: actions.left
        anchors.rightMargin: Style.space(8)
        anchors.verticalCenter: parent.verticalCenter
        spacing: Style.space(1)

        Text {
          width: parent.width
          textFormat: Text.PlainText
          text: row.box.name + (root.busy === row.box.name ? " …" : "")
          color: root.foreground
          opacity: row.up ? 1 : 0.6
          font.family: root.fontFamily
          font.pixelSize: Style.font.body
          elide: Text.ElideRight
        }
        Text {
          width: parent.width
          textFormat: Text.PlainText
          text: root.caption(row.box)
          color: row.armed ? root.urgent : Qt.darker(root.foreground, 1.4)
          font.family: root.fontFamily
          font.pixelSize: Style.font.caption
          elide: Text.ElideRight
        }
      }

      // Every row has the same six slots in the same order (#120): Peek/Show, Keys, Shot, Paste in,
      // Copy out, Down. One that does not apply to the row is empty, never removed, so a click from
      // habit lands on the same action on every row, and never on an interactive box's clipboard. A
      // click in an empty slot (or between buttons) does nothing: it does not fall through to the row.
      Item {
        id: actions
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        implicitWidth: slots.implicitWidth
        implicitHeight: slots.implicitHeight
        width: implicitWidth
        height: implicitHeight

        MouseArea { anchors.fill: parent }

        Row {
          id: slots
          spacing: Style.space(2)

          SlotButton {
            slot: "peek"
            applies: row.up
            iconText: row.interactive ? root.icons.show : root.icons.peek
            tooltipText: row.interactive ? "Show" : "Peek"
            foreground: root.foreground
            fontFamily: root.fontFamily
            onClicked: root.peek(row.box)
          }
          SlotButton {   // keys-to-box: lit while on
            slot: "keys"
            applies: row.up && row.interactive
            iconText: root.icons.keys
            tooltipText: row.box.keys_to_box ? "SUPER keys follow focus and the pointer into it: on (f)" : "SUPER keys to the box while it has focus and the pointer: off (f)"
            foreground: row.box.keys_to_box ? root.urgent : root.foreground
            hoverColor: root.urgent
            fontFamily: root.fontFamily
            onClicked: root.keysToBox(row.box)
          }
          SlotButton {
            slot: "shot"
            applies: row.up
            iconText: root.icons.shot
            tooltipText: "Screenshot"
            foreground: root.foreground
            fontFamily: root.fontFamily
            onClicked: root.shot(row.box)
          }
          SlotButton {
            slot: "clipIn"
            applies: row.up && row.interactive
            iconText: root.icons.clipIn
            tooltipText: "Paste your clipboard into the box"
            foreground: root.foreground
            fontFamily: root.fontFamily
            onClicked: root.clip(row.box, false)
          }
          SlotButton {
            slot: "clipOut"
            applies: row.up && row.interactive
            iconText: root.icons.clipOut
            tooltipText: "Copy the box's clipboard out"
            foreground: root.foreground
            fontFamily: root.fontFamily
            onClicked: root.clip(row.box, true)
          }
          SlotButton {
            slot: "down"
            applies: !row.gone
            iconText: root.icons.down
            tooltipText: row.armed ? "Click again to shut it down" : "Down"
            foreground: row.armed ? root.urgent : root.foreground
            hoverColor: root.urgent
            fontFamily: root.fontFamily
            onClicked: root.down(row.box)
          }
        }
      }
    }
  }

  // A row's action slot: there on every row, empty where the action does not apply (#120).
  component SlotButton: PanelActionButton {
    property string slot: ""
    property bool applies: true
    objectName: "slot-" + slot
    enabled: applies
    opacity: applies ? 1 : 0
  }

  // What the panel shows, as JSON, for tests and agents (#117, #120, #121): the rows in order, each
  // with its place on screen and its action slots' centres, and the list's scroll. Read only.
  function inspect() {
    var out = { held: held, newCount: newCount, selected: selectedName, face: face,
      list: { contentY: boxList.contentY, height: boxList.height, contentHeight: boxList.contentHeight },
      newButton: Math.round(newButton.mapToItem(null, 0, 0).y), hintsBottom: Math.round(hints.mapToItem(null, 0, hints.height).y),
      screen: panel.screenH, rows: [],
      gpu: { shown: gpuShown, value: gpuDropdown.value, options: gpuOptions.map(function(o) { return o.label }),
        line: gpuLine.text, at: (function() { var q = gpuDropdown.mapToItem(null, gpuDropdown.width / 2, gpuDropdown.height / 2)
          return { x: Math.round(q.x), y: Math.round(q.y) } })() } }
    var vis = boxList.mapToItem(null, 0, 0)
    for (var i = 0; i < boxList.contentItem.children.length; i++) {
      var r = boxList.contentItem.children[i]
      if (r.rowIndex === undefined || !r.box || !r.box.name) continue
      var p = r.mapToItem(null, 0, 0)
      var slotsOut = {}
      collect(r, slotsOut)
      out.rows.push({ name: r.box.name, index: r.rowIndex, gone: r.gone, caption: caption(r.box), x: p.x, y: p.y, w: r.width, h: r.height,
        visible: p.y >= vis.y - 1 && p.y + r.height <= vis.y + boxList.height + 1, slots: slotsOut })
    }
    out.rows.sort(function(a, b) { return a.index - b.index })
    return JSON.stringify(out)
  }
  function collect(item, into) {
    for (var i = 0; i < item.children.length; i++) {
      var c = item.children[i]
      if (typeof c.objectName === "string" && c.objectName.indexOf("slot-") === 0) {
        var q = c.mapToItem(null, c.width / 2, c.height / 2)
        into[c.objectName.slice(5)] = { x: Math.round(q.x), y: Math.round(q.y), applies: c.applies }
      } else collect(item.children[i], into)
    }
  }
  IpcHandler {
    target: "chaves.omabox.panel"
    function inspect(): string { return root.inspect() }
    // The panel open on a face: list or settings (the gear).
    function face(name: string): void { if (name === "list" || name === "settings") { root.open(); root.openFace(name) } }
  }
}
