import QtQuick
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

  property var boxes: []
  property int selectedIndex: 0
  property string selectedName: ""     // the selection follows the box, not its place in the list
  property bool cursorActive: false
  property string armedDown: ""        // box whose Down was pressed once; a second press confirms
  property bool armedNew: false        // n pressed once: a second n starts a box (a stray key does not)
  property var pendingSet: null        // a setting picked while the last one was still being written
  property string busy: ""             // box an action is running for
  property string lastError: ""        // kept until the next action succeeds
  property string listError: ""        // `omabox ls` itself failing: the list shown may be stale
  property int listFailures: 0
  property string action: ""           // what actionProc is doing: peek, shot, down
  property real now: Date.now()
  // `omabox config --json`; the defaults until it answers.
  property var settings: ({ "workspace": "9", "confirm-close": "off", "bar-icon": "always" })
  // bar-icon: always (the default) keeps the icon, and the panel's settings, in the bar with no box
  // up, dimmed; auto shows it only while any box exists.
  readonly property bool alwaysShown: settings["bar-icon"] !== "auto"
  readonly property bool shown: hasBoxes || alwaysShown
  readonly property string workspaceLabel: {
    var w = String(settings.workspace || "9")
    return w === "special:scratchpad" ? "the scratchpad" : w.indexOf("special:") === 0 ? "special:" + w.slice(8) : "workspace " + w
  }
  readonly property var workspaceOptions: {
    var o = []
    for (var i = 1; i <= 10; i++) o.push({ value: String(i), label: "Workspace " + i })
    o.push({ value: "special:scratchpad", label: "Scratchpad (SUPER+S)" })
    var cur = String(settings.workspace || "9")
    if (!o.some(function(x) { return x.value === cur }))   // set by hand: shown as it is
      o.push({ value: cur, label: cur.indexOf("special:") === 0 ? "Special: " + cur.slice(8) : "Workspace " + cur })
    return o
  }

  // The card's two faces, as omawin's: the box list, and Settings behind the gear at the top right,
  // which Back (or Esc) leaves.
  property string face: "list"
  readonly property string pluginVersion: "0.1.0"   // manifest.json's and VERSION

  readonly property var icons: ({
    gear: String.fromCodePoint(0xF013),     // fa-cog, omawin's
    back: String.fromCodePoint(0xF053),     // fa-chevron_left, omawin's
    add: String.fromCodePoint(0xF067),      // fa-plus
    peek: String.fromCodePoint(0xF0208),    // md-eye
    show: String.fromCodePoint(0xF0379),    // md-monitor
    shot: String.fromCodePoint(0xF0100),    // md-camera
    down: String.fromCodePoint(0xF0159),    // md-close_circle
    alert: String.fromCodePoint(0xF0026),   // md-alert
    dismiss: String.fromCodePoint(0xF0156)  // md-close
  })

  readonly property bool hasBoxes: boxes.length > 0
  readonly property int upCount: boxes.filter(function(b) { return b.state === "up" }).length
  readonly property int deadCount: boxes.length - upCount
  // One wording for the tooltip and the panel: the icon shows while any box exists, dead ones too.
  readonly property string countText: (upCount === 1 ? "1 box up" : upCount + " boxes up") + (deadCount ? " · " + deadCount + " dead" : "")

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
    listProc.running = true
  }

  function parseList(text) {
    var parsed
    try { parsed = JSON.parse(text) } catch (e) { parsed = null }
    if (!Array.isArray(parsed)) { listFailed("omabox ls --json: not a list"); return }
    listFailures = 0
    listError = ""
    boxes = parsed
    var i = boxes.findIndex(function(b) { return b.name === selectedName })
    select(i >= 0 ? i : Math.min(selectedIndex, Math.max(0, boxes.length - 1)))
  }

  // Keep the last list through a hiccup, but say so; after three failures in a row it is gone, and
  // with it the icon and the alert strip, so that one goes out as a notification.
  function listFailed(why) {
    listError = why
    if (++listFailures === 3) Quickshell.execDetached(["notify-send", "-a", "omabox", "omabox: cannot list boxes", why])
    if (listFailures >= 3) boxes = []
  }

  function select(i) {
    selectedIndex = i
    selectedName = boxes[i] ? boxes[i].name : ""
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
    if (armedDown === b.name) return "Press again to shut it down"
    if (b.state !== "up") return "dead · Down cleans it up"
    var parts = [b.mode || "?"]
    if (b.mode !== "interactive" && b.size) parts.push(b.size)
    var a = age(b.created); if (a) parts.push(a)
    if (b.plugins && b.plugins.length) parts.push(b.plugins.join(", "))
    if (b.net === "isolated") parts.push("isolated")
    if (b.peeking) parts.push("peeking")
    return parts.join(" · ")
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
    if (!b) return
    if (b.state !== "up") { down(b); return }
    if (run(b.name, "peek", [command, "peek", "-b", b.name, "--focus"])) close()
  }

  // omabox shot only: the viewer opens detached, so an image left open does not hold up every later
  // action (xdg-open waits for the viewer on Hyprland).
  function shot(b) {
    if (!b) return
    if (b.state !== "up") { down(b); return }
    if (run(b.name, "shot", [command, "shot", "-b", b.name])) close()
  }

  function down(b) {
    if (!b) return
    if (armedDown !== b.name) {
      armedDown = b.name
      disarm.restart()
      return
    }
    if (run(b.name, "down", [command, "down", b.name])) armedDown = ""
  }

  function selected() { return boxes[selectedIndex] }

  Process {
    id: listProc
    // A command that cannot start (not on PATH) never emits exited: running just drops back.
    property bool exited: false
    command: [root.command, "ls", "--json"]
    stdout: StdioCollector { id: listOut; waitForEnd: true }
    stderr: StdioCollector { id: listErr; waitForEnd: true }
    onExited: function(code) {
      exited = true
      if (code === 0) root.parseList(listOut.text)
      else root.listFailed((listErr.text.trim().split("\n").pop() || root.command + " ls: exit " + code))
    }
    onRunningChanged: if (!running) Qt.callLater(function() {
      if (!listProc.exited && !listProc.running) root.listFailed("cannot run " + root.command)
    })
  }

  Process {
    id: actionProc
    property bool exited: false
    stdout: StdioCollector { id: actionOut; waitForEnd: true }
    stderr: StdioCollector { id: actionErr; waitForEnd: true }
    onExited: function(code) {
      exited = true
      root.actionDone(code === 0 ? "" : (actionErr.text.trim().split("\n").pop() || ("exit " + code)), actionOut.text.trim())
    }
    onRunningChanged: if (!running) Qt.callLater(function() {
      if (!actionProc.exited && !actionProc.running && root.busy !== "") root.actionDone("cannot run " + root.command, "")
    })
  }

  // Peek and shot close the panel, so a failure also goes out as a notification.
  function actionDone(error, out) {
    lastError = error
    if (error !== "") Quickshell.execDetached(["notify-send", "-a", "omabox", "omabox " + (action === "new" ? "new box" : action + " " + busy), error])
    else if (action === "shot" && out !== "") Quickshell.execDetached(["xdg-open", out])
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
          root.settings = c
          // A pick sets the Dropdown's own value, which ends its binding: show the setting again (a
          // refused pick, or `omabox config workspace N` since).
          wsDropdown.value = String(c.workspace || "9")
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

  Timer {
    interval: root.opened ? 2000 : root.refreshSec * 1000
    running: true
    repeat: true
    triggeredOnStart: true
    onTriggered: { root.now = Date.now(); root.refresh() }
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
    tooltipText: root.opened ? "" : (root.hasBoxes ? root.countText : "No boxes")
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
      onMoveRequested: function(dx, dy) {
        if (root.face !== "list") return
        if (!root.cursorActive) { root.cursorActive = true; return }
        if (dy !== 0 && root.boxes.length > 0) {
          root.select((root.selectedIndex + dy + root.boxes.length) % root.boxes.length)
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
          if (root.armedNew) { root.armedNew = false; root.newBox() }
          else { root.armedNew = true; disarm.restart() }
          return
        }
        if (!root.cursorActive) return
        if (t === "p") root.peek(root.selected())
        else if (t === "s") root.shot(root.selected())
        else if (t === "d") root.down(root.selected())
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
                color: root.bar.foreground
              }
            }
          }
          id: hero
          title: root.face === "settings" ? "Settings" : "Boxes"
          meta: root.face === "settings" ? "omabox " + root.pluginVersion
            : (root.hasBoxes ? root.countText : "no boxes").toUpperCase()
          foreground: root.bar.foreground
          fontFamily: root.bar.fontFamily
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
                foreground: root.bar.foreground
                fontFamily: root.bar.fontFamily
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
                foreground: root.bar.foreground
                fontFamily: root.bar.fontFamily
                tooltipText: "Back · Esc"
                onClicked: root.goBack()
              }
            }
          }
        }

        // The last failure, full width in the urgent colour: a pill in the hero was too small to read.
        // A failing list comes first: everything below it may be stale.
        Rectangle {
          visible: root.lastError !== "" || root.listError !== ""
          width: parent.width
          implicitHeight: alertRow.implicitHeight + Style.space(12)
          radius: Style.cornerRadius
          color: Util.alpha(root.bar.urgent, 0.14)
          border.width: 1
          border.color: Util.alpha(root.bar.urgent, 0.55)

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
              color: root.bar.urgent
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.body
            }
            Text {
              width: parent.width - alertIcon.width - alertDismiss.width - parent.spacing * 2
              anchors.verticalCenter: parent.verticalCenter
              textFormat: Text.PlainText
              text: root.listError !== "" ? "list: " + root.listError : root.lastError
              color: root.bar.urgent
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.bodySmall
              wrapMode: Text.Wrap
            }
            PanelActionButton {
              id: alertDismiss
              anchors.verticalCenter: parent.verticalCenter
              iconText: root.icons.dismiss
              tooltipText: "Dismiss"
              foreground: root.bar.urgent
              hoverColor: root.bar.urgent
              fontFamily: root.bar.fontFamily
              onClicked: { root.lastError = ""; root.listError = "" }
            }
          }
        }

        // ============================= the list =============================
        Column {
          visible: root.face === "list"
          width: parent.width
          spacing: Style.space(12)

          PanelSeparator { foreground: root.bar.foreground }

          Column {
            width: parent.width
            spacing: Style.space(4)

            Text {
              visible: !root.hasBoxes
              width: parent.width
              textFormat: Text.PlainText
              text: "No boxes up"
              color: Qt.darker(root.bar.foreground, 1.4)
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.body
              horizontalAlignment: Text.AlignHCenter
            }

            Repeater {
              model: root.boxes
              BoxRow {
                required property var modelData
                required property int index
                width: parent.width
                box: modelData
                rowIndex: index
              }
            }
          }

          Button {
            width: parent.width
            bordered: true
            iconText: root.icons.add
            text: root.action === "new" ? "Starting a box…" : root.armedNew ? "Press n again to start one" : "New interactive box"
            iconSize: Style.font.bodySmall
            fontSize: Style.font.body
            foreground: root.bar.foreground
            fontFamily: root.bar.fontFamily
            tooltipText: "A window on " + root.workspaceLabel + ", named box-1, box-2, …"
            enabled: !actionProc.running
            opacity: enabled ? 1.0 : 0.6
            onClicked: root.newBox()
          }

          Text {
            width: parent.width
            textFormat: Text.PlainText
            text: "↑↓ move · p peek/show · s shot · d down · n new · r refresh"
            color: root.bar.foreground
            opacity: 0.5
            font.family: root.bar.fontFamily
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

          PanelSeparator { foreground: root.bar.foreground }

          Column {
            width: parent.width
            spacing: Style.space(6)

            SettingText { text: "Where windows open"; bold: true }
            SettingText { text: "Interactive boxes and peek windows, never focused when they open. The scratchpad is SUPER+S."; dim: true }
            Dropdown {
              id: wsDropdown
              width: parent.width
              showLabel: false
              fontFamily: root.bar.fontFamily
              options: root.workspaceOptions
              value: String(root.settings.workspace || "9")
              onChanged: function(v) { root.setSetting("workspace", v) }
            }
          }

          SettingSwitch {
            title: "Confirm before closing"
            caption: "Closing an interactive box's window asks first; closing it again shuts the box down."
            checked: root.settings["confirm-close"] === "on"
            onToggled: root.setSetting("confirm-close", checked ? "off" : "on")
          }

          SettingSwitch {
            title: "Always show in the bar"
            caption: "Off: the icon only while a box is up (then back here, or: omabox config bar-icon always)."
            checked: root.alwaysShown
            onToggled: root.setSetting("bar-icon", checked ? "auto" : "always")
          }

          PanelSeparator { foreground: root.bar.foreground }

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
    color: dim ? Qt.darker(root.bar.foreground, 1.4) : root.bar.foreground
    font.family: root.bar.fontFamily
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
      busy: setProc.running
      foreground: root.bar.foreground
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
      color: root.bar.foreground
      opacity: 0.6
      font.family: root.bar.fontFamily
      font.pixelSize: Style.font.caption
    }
    Text {
      textFormat: Text.PlainText
      text: parent.value
      color: Qt.darker(root.bar.foreground, 1.4)
      font.family: root.bar.fontFamily
      font.pixelSize: Style.font.caption
      font.bold: true
    }
  }

  component BoxRow: CursorSurface {
    id: row
    property var box: ({})
    property int rowIndex: 0
    readonly property bool up: box.state === "up"
    readonly property bool rowSelected: root.cursorActive && root.selectedIndex === rowIndex
    readonly property bool armed: root.armedDown === box.name

    hasCursor: rowSelected
    foreground: root.bar.foreground
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
          color: root.bar.foreground
          opacity: row.up ? 1 : 0.6
          font.family: root.bar.fontFamily
          font.pixelSize: Style.font.body
          elide: Text.ElideRight
        }
        Text {
          width: parent.width
          textFormat: Text.PlainText
          text: root.caption(row.box)
          color: row.armed ? root.bar.urgent : Qt.darker(root.bar.foreground, 1.4)
          font.family: root.bar.fontFamily
          font.pixelSize: Style.font.caption
          elide: Text.ElideRight
        }
      }

      Row {
        id: actions
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        spacing: Style.space(2)

        PanelActionButton {
          visible: row.up
          iconText: row.box.mode === "interactive" ? root.icons.show : root.icons.peek
          tooltipText: row.box.mode === "interactive" ? "Show" : "Peek"
          foreground: root.bar.foreground
          fontFamily: root.bar.fontFamily
          onClicked: root.peek(row.box)
        }
        PanelActionButton {
          visible: row.up
          iconText: root.icons.shot
          tooltipText: "Screenshot"
          foreground: root.bar.foreground
          fontFamily: root.bar.fontFamily
          onClicked: root.shot(row.box)
        }
        PanelActionButton {
          iconText: root.icons.down
          tooltipText: row.armed ? "Click again to shut it down" : "Down"
          foreground: row.armed ? root.bar.urgent : root.bar.foreground
          hoverColor: root.bar.urgent
          fontFamily: root.bar.fontFamily
          onClicked: root.down(row.box)
        }
      }
    }
  }
}
