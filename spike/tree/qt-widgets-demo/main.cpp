// Spike #144: a Qt widgets window with what a shot is usually read for (the GTK4 demo's twin).
// Build on the host: qmake6 -o BUILD/Makefile spike/tree/qt-widgets-demo/qt-widgets-demo.pro &&
// make -C BUILD; copy the binary into the box HOME and run it there.
#include <QApplication>
#include <QCheckBox>
#include <QComboBox>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QPushButton>
#include <QSlider>
#include <QSpinBox>
#include <QVBoxLayout>
#include <QWidget>

int main(int argc, char **argv) {
  QApplication app(argc, argv);
  QWidget win;
  win.setWindowTitle("Tree demo Qt");
  win.resize(480, 420);
  auto *box = new QVBoxLayout(&win);
  auto *status = new QLabel("Clicked 0 times");
  auto *btn = new QPushButton("Click me");
  int count = 0;
  QObject::connect(btn, &QPushButton::clicked, [&] {
    status->setText(QString("Clicked %1 times").arg(++count));
  });
  auto *entry = new QLineEdit("hello box");
  auto *check = new QCheckBox("Enable sync");
  check->setChecked(true);
  auto *check2 = new QCheckBox("Dark mode");
  auto *spin = new QSpinBox();
  spin->setRange(0, 100);
  spin->setValue(42);
  auto *slider = new QSlider(Qt::Horizontal);
  slider->setRange(0, 100);
  slider->setValue(70);
  auto *combo = new QComboBox();
  combo->addItems({"Small", "Medium", "Large"});
  combo->setCurrentIndex(1);
  auto *list = new QListWidget();
  list->addItems({"Alpha", "Beta", "Gamma"});
  list->setCurrentRow(1);
  for (QWidget *w : {(QWidget *)status, (QWidget *)btn, (QWidget *)entry, (QWidget *)check,
                     (QWidget *)check2, (QWidget *)spin, (QWidget *)slider, (QWidget *)combo,
                     (QWidget *)list})
    box->addWidget(w);
  win.show();
  return app.exec();
}
