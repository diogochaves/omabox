import QtQuick
import qs.Ui

// How many cups of coffee today: a count worked out when the bar loads it.
BarWidget {
  id: root
  property int cups: [17, 23].reduce((a, b) => a * b, 1)
  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight
  WidgetButton { id: button; bar: root.bar; text: "cups " + root.cups }
}
