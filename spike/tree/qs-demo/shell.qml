// Spike #144: does Quickshell export anything to AT-SPI? A floating window (an xdg toplevel) and a
// panel (a layer surface), each with a label and a Qt Quick Controls button.
// Run inside a box: QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1 quickshell -n -p spike/tree/qs-demo
import QtQuick
import QtQuick.Controls
import Quickshell

ShellRoot {
    FloatingWindow {
        implicitWidth: 300; implicitHeight: 120
        title: "Tree demo Quickshell"
        Column {
            anchors.centerIn: parent
            Label { text: "Floating label" }
            Button { text: "Floating button" }
            CheckBox { text: "Floating check"; checked: true }
        }
    }
    PanelWindow {
        anchors { left: true; right: true; bottom: true }
        implicitHeight: 40
        Row {
            anchors.centerIn: parent; spacing: 12
            Text { text: "Panel text" }
            Button { text: "Panel button" }
            Text { text: "With a role"; Accessible.role: Accessible.StaticText; Accessible.name: text }
        }
    }
}
