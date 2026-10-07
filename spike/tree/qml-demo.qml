// Spike #144: a Qt Quick Controls window, the GTK4 and Qt widgets demos' twin.
// Run inside a box: QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1 qml6 spike/tree/qml-demo.qml
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ApplicationWindow {
    width: 480; height: 420; visible: true
    title: "Tree demo QML"
    property int count: 0
    ColumnLayout {
        anchors.fill: parent; anchors.margins: 12
        Label { text: "Clicked " + count + " times" }
        Button { text: "Click me"; Layout.fillWidth: true; onClicked: count++ }
        TextField { text: "hello box"; Layout.fillWidth: true }
        CheckBox { text: "Enable sync"; checked: true }
        CheckBox { text: "Dark mode" }
        Switch { text: "Wi-Fi"; checked: true }
        SpinBox { from: 0; to: 100; value: 42 }
        Slider { from: 0; to: 100; value: 70; Layout.fillWidth: true }
        ComboBox { model: ["Small", "Medium", "Large"]; currentIndex: 1; Layout.fillWidth: true }
        ListView {
            id: lv
            Layout.fillWidth: true; Layout.preferredHeight: 66
            model: ["Alpha", "Beta", "Gamma"]; currentIndex: 1
            delegate: ItemDelegate {
                text: modelData; width: lv.width; height: 22
                highlighted: ListView.isCurrentItem
                onClicked: lv.currentIndex = index
            }
        }
    }
}
