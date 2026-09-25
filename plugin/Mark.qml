import QtQuick

// The omabox mark: offset arms around a window, drawn as whole-pixel rects so it
// stays crisp and takes the theme's colour. `small` is the 8-unit bar glyph (2 px
// per unit in the bar's 16 px icon canvas); otherwise the 15-unit mark.
// The same shapes as assets/omabox-glyph.svg and assets/omabox-mark.svg.
Item {
  id: root

  property bool small: false
  property color color: "white"

  // [x, y, width, height] in units
  readonly property var rects: small
    ? [[0, 0, 5, 1], [7, 0, 1, 7], [0, 1, 1, 7], [2, 2, 4, 2], [2, 4, 1, 1], [5, 4, 1, 1], [2, 5, 4, 1], [3, 7, 5, 1]]
    : [[0, 0, 8, 1], [13, 0, 2, 1], [0, 1, 1, 13], [14, 1, 1, 13], [4, 4, 7, 2], [4, 6, 1, 4], [10, 6, 1, 4], [4, 10, 7, 1], [0, 14, 2, 1], [7, 14, 8, 1]]
  readonly property int units: small ? 8 : 15
  // Rounded down: rounded up it drew 30 px in the hero's 24 px item (24/15 = 1.6 → 2).
  readonly property int unit: Math.max(1, Math.floor(Math.min(width, height) / units))
  readonly property int originX: Math.round((width - units * unit) / 2)
  readonly property int originY: Math.round((height - units * unit) / 2)

  implicitWidth: units * 2
  implicitHeight: units * 2

  Repeater {
    model: root.rects
    Rectangle {
      required property var modelData
      x: root.originX + modelData[0] * root.unit
      y: root.originY + modelData[1] * root.unit
      width: modelData[2] * root.unit
      height: modelData[3] * root.unit
      color: root.color
      antialiasing: false
    }
  }
}
