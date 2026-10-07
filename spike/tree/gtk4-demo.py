#!/usr/bin/python3
"""Spike #144: a GTK4 window with what a shot is usually read for (run inside a box)."""
import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gtk  # noqa: E402


def build(app):
    win = Gtk.ApplicationWindow(application=app, title="Tree demo GTK4")
    win.set_default_size(480, 420)
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6, margin_top=12,
                  margin_bottom=12, margin_start=12, margin_end=12)
    status = Gtk.Label(label="Clicked 0 times")
    count = [0]
    btn = Gtk.Button(label="Click me")

    def clicked(_b):
        count[0] += 1
        status.set_label("Clicked %d times" % count[0])
    btn.connect("clicked", clicked)
    entry = Gtk.Entry(text="hello box")
    check = Gtk.CheckButton(label="Enable sync", active=True)
    check2 = Gtk.CheckButton(label="Dark mode", active=False)
    sw = Gtk.Switch(active=True)
    swrow = Gtk.Box(spacing=6)
    swrow.append(Gtk.Label(label="Wi-Fi"))
    swrow.append(sw)
    spin = Gtk.SpinButton.new_with_range(0, 100, 1)
    spin.set_value(42)
    scale = Gtk.Scale.new_with_range(Gtk.Orientation.HORIZONTAL, 0, 100, 1)
    scale.set_value(70)
    scale.set_hexpand(True)
    drop = Gtk.DropDown.new_from_strings(["Small", "Medium", "Large"])
    drop.set_selected(1)
    lb = Gtk.ListBox()
    for t in ("Alpha", "Beta", "Gamma"):
        lb.append(Gtk.Label(label=t, xalign=0))
    lb.select_row(lb.get_row_at_index(1))
    for w in (status, btn, entry, check, check2, swrow, spin, scale, drop, lb):
        box.append(w)
    win.set_child(box)
    win.present()


app = Gtk.Application(application_id="org.omabox.TreeDemo")
app.connect("activate", build)
app.run(None)
