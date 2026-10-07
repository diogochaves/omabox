#!/usr/bin/python3
# Phase 1 of #145 (finding 243): a GTK4 window with the widgets a look-after-action checks (a menu, a
# toggle, a text field, a fill, a drag source), around content that does not change. Run in a box:
#   omabox run -b BOX -d --wait -- /usr/bin/python3 spike/changed/probe-app.py
import gi
gi.require_version("Gtk", "4.0")
from gi.repository import Gtk, Gio, Gdk  # noqa: E402

LOREM = ("Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt "
         "ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation. ") * 3


def on_activate(app):
    win = Gtk.ApplicationWindow(application=app, title="Probe")
    win.set_default_size(1200, 800)
    hb = Gtk.HeaderBar()
    menu = Gio.Menu()
    for label in ("New Window", "Open…", "Preferences", "Keyboard Shortcuts", "About Probe", "Quit"):
        menu.append(label, "app.noop")
    mb = Gtk.MenuButton(icon_name="open-menu-symbolic", menu_model=menu)
    hb.pack_end(mb)
    win.set_titlebar(hb)
    act = Gio.SimpleAction.new("noop", None)
    app.add_action(act)

    root = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=12)
    side = Gtk.ListBox()
    for name in ("Inbox", "Starred", "Sent", "Drafts", "Archive", "Spam", "Trash", "Labels", "Settings"):
        side.append(Gtk.Label(label=name, xalign=0, margin_start=12, margin_top=8, margin_bottom=8))
    side.set_size_request(220, -1)
    root.append(side)

    main = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=14, margin_top=16, margin_start=16,
                   margin_end=16, margin_bottom=16, hexpand=True)
    row = Gtk.Box(spacing=12)
    row.append(Gtk.Label(label="Wi-Fi"))
    row.append(Gtk.Switch(name="sw"))
    row.append(Gtk.CheckButton(label="Notify me", name="chk"))
    main.append(row)
    entry = Gtk.Entry(placeholder_text="Search messages", name="entry")
    main.append(entry)
    pb = Gtk.ProgressBar(fraction=0.2, show_text=True, name="pb")
    step = Gtk.Button(label="Step")
    step.connect("clicked", lambda *_: pb.set_fraction(min(1.0, pb.get_fraction() + 0.2)))
    prow = Gtk.Box(spacing=12)
    pb.set_hexpand(True)
    prow.append(pb)
    prow.append(step)
    main.append(prow)
    # A drag source and a drop target: the ghost is a drag icon surface, not the window's.
    drow = Gtk.Box(spacing=40)
    src = Gtk.Label(label="  Drag me: report.pdf  ", name="src")
    src.add_css_class("card")
    src.set_size_request(240, 60)
    ds = Gtk.DragSource()
    ds.connect("prepare", lambda *_: Gdk.ContentProvider.new_for_value("report.pdf"))
    src.add_controller(ds)
    dst = Gtk.Label(label="Drop here", name="dst")
    dst.add_css_class("card")
    dst.set_size_request(300, 120)
    dt = Gtk.DropTarget.new(str, Gdk.DragAction.COPY)
    dt.connect("drop", lambda _t, v, *_: (dst.set_label(f"Dropped {v}"), True)[1])
    dst.add_controller(dt)
    drow.append(src)
    drow.append(dst)
    main.append(drow)
    tv = Gtk.TextView(wrap_mode=Gtk.WrapMode.WORD, editable=False, vexpand=True)
    tv.get_buffer().set_text(LOREM * 6)
    main.append(tv)
    root.append(main)
    win.set_child(root)
    win.present()


app = Gtk.Application(application_id="dev.omabox.Probe", flags=Gio.ApplicationFlags.NON_UNIQUE)
app.connect("activate", on_activate)
app.run()
