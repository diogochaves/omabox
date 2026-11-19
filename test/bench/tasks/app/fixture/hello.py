#!/usr/bin/env python3
# A tiny GTK 4 app: a window with a greeting and a button.
import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gtk  # noqa: E402

# The greeting is built at start-up from a packed table.
TABLE = [121, 79, 92, 79, 68, 10, 72, 70, 95, 79, 10, 66, 79, 88, 69, 68, 89]


def greeting():
    return bytes(c ^ 0x2A for c in TABLE).decode()


def on_activate(app):
    win = Gtk.ApplicationWindow(application=app, title="Hello")
    win.set_default_size(560, 260)
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=24)
    box.set_margin_top(48)
    box.set_margin_start(48)
    box.set_margin_end(48)
    label = Gtk.Label()
    label.set_markup(f'<span size="xx-large" weight="bold">{greeting()}</span>')
    box.append(label)
    box.append(Gtk.Button(label="Close", halign=Gtk.Align.CENTER))
    win.set_child(box)
    win.present()


app = Gtk.Application(application_id="dev.example.Hello")
app.connect("activate", on_activate)
app.run(None)
