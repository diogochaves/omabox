#!/usr/bin/python3
"""Spike #144: dump the box's AT-SPI tree as compact text (role, name, states, bounds).

Run INSIDE a box (`omabox run -- /usr/bin/python3 spike/tree/atspi-tree.py ...`): it talks to the
session bus it is given, which in a box is the box's own. Never run it on the host. The box needs
`up --env ATSPI_DBUS_IMPLEMENTATION=dbus-daemon` for its a11y bus to start (NOTES finding 244),
and Qt apps `org.a11y.Status.IsEnabled` true (`measure.sh` sets both).

  atspi-tree.py                    list the applications on the a11y bus, with their windows
  atspi-tree.py APP_RE             the tree of every application whose name matches APP_RE
  options: --pid N (by process instead), --all (keep hidden, scrolled-out and anonymous nodes),
           --raw (one line per node, nothing pruned: the size before compaction),
           --screen (print screen coordinates next to window ones), --stats (counts on stderr)

A line: `role "name" =value [states] @x,y wxh` (window coordinates: what `click --window` takes).
Anonymous containers (no name, value or text) are not printed; their children move up a level.
"""
import re
import sys
import time

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, GLib  # noqa: E402

S = Atspi.StateType
# States worth a word; the rest (enabled, sensitive, visible, showing, focusable...) are the default.
SHOWN = [
    (S.FOCUSED, "focused"), (S.CHECKED, "checked"), (S.SELECTED, "selected"),
    (S.PRESSED, "pressed"), (S.EXPANDED, "expanded"), (S.INDETERMINATE, "mixed"),
    (S.EDITABLE, "editable"), (S.READ_ONLY, "readonly"), (S.MODAL, "modal"),
    (S.ACTIVE, "active"), (S.BUSY, "busy"), (S.INVALID_ENTRY, "invalid"),
]
CONTAINERS = {"filler", "panel", "section", "unknown", "grouping", "scroll pane", "viewport",
              "layered pane", "split pane", "redundant object", "invalid", "internal frame",
              "block quote", "static", "landmark", "form", "paragraph"}


def safe(f, default=None):
    try:
        return f()
    except GLib.Error:
        return default
    except Exception:  # noqa: BLE001  (a dying app raises anything)
        return default


def clean(s, n=80):
    # U+FFFC stands for an embedded child; private-use characters are icon-font glyphs.
    s = re.sub(r"[\ufffc\ue000-\uf8ff]", "", s or "")
    s = re.sub(r"\s+", " ", s).strip()
    return s if len(s) <= n else s[: n - 1] + "…"


def node_text(acc, role):
    """The text a shot would be read for, when the name does not carry it."""
    if role in ("text", "entry", "password text", "terminal", "label", "static", "paragraph",
                "heading", "document text", "editbar"):
        t = safe(lambda: acc.get_text_iface())
        if t is not None:
            n = safe(lambda: Atspi.Text.get_character_count(acc), 0)
            if n:
                return safe(lambda: Atspi.Text.get_text(acc, 0, min(n, 200)), "")
    return ""


def value_of(acc):
    v = safe(lambda: acc.get_value_iface())
    if v is None:
        return None
    cur = safe(lambda: Atspi.Value.get_current_value(acc))
    txt = safe(lambda: Atspi.Value.get_text(acc), "")
    if txt:
        return txt
    return None if cur is None else ("%g" % cur)


def combo_choice(acc):
    """A closed combo box's choice: its (hidden) menu's selected item, as Chromium has it."""
    for i in range(min(safe(acc.get_child_count, 0) or 0, 4)):
        menu = safe(lambda: acc.get_child_at_index(i))
        for j in range(min(safe(menu.get_child_count, 0) or 0, 200) if menu else 0):
            it = safe(lambda: menu.get_child_at_index(j))
            ss = safe(it.get_state_set) if it else None
            if ss is not None and ss.contains(S.SELECTED):
                return clean(safe(it.get_name, ""), 40) or None
    return None


def extents(acc, coord):
    c = safe(lambda: acc.get_component_iface())
    if c is None:
        return None
    e = safe(lambda: Atspi.Component.get_extents(acc, coord))
    return None if e is None else (e.x, e.y, e.width, e.height)


class Dump:
    def __init__(self, opts):
        self.o = opts
        self.lines = []
        self.nodes = 0
        self.nobounds = 0
        self.clipped = 0
        self.view = None

    def walk(self, acc, depth, budget=4000, parent_name=""):
        if self.nodes >= budget:
            return
        self.nodes += 1
        role = safe(acc.get_role_name, "?")
        name = clean(safe(acc.get_name, ""))
        sset = safe(acc.get_state_set)
        has = (lambda st: sset.contains(st)) if sset else (lambda st: False)
        hidden = sset is not None and not (has(S.SHOWING) or has(S.VISIBLE))
        if hidden and role != "application" and not self.o["all"] and not self.o["raw"]:
            return
        b = extents(acc, Atspi.CoordType.WINDOW)
        if depth == 1 and b:
            self.view = b  # a top-level window: what it shows is inside it
        elif (b and self.view and b[2] > 0 and b[3] > 0 and not self.o["all"]
              and not self.o["raw"] and (b[0] >= self.view[2] or b[1] >= self.view[3]
                                         or b[0] + b[2] <= 0 or b[1] + b[3] <= 0)):
            self.clipped += 1
            return  # scrolled out of the window (a long page): a shot would not show it either
        val = value_of(acc)
        if val is None and role == "combo box":
            val = combo_choice(acc)
        if val == name:
            val = None
        text = clean(node_text(acc, role))
        if text == name:
            text = ""
        states = [w for st, w in SHOWN if has(st)]
        if has(S.CHECKABLE) and not has(S.CHECKED) and role in ("check box", "radio button",
                                                                 "toggle button", "check menu item",
                                                                 "radio menu item", "switch"):
            states.append("unchecked")
        if sset is not None and not has(S.SENSITIVE) and not has(S.ENABLED):
            states.append("disabled")
        n = safe(acc.get_child_count, 0) or 0
        if role == "application" and not n and not self.o["raw"]:
            return  # Qt registers again once enabled: the first registration stays, empty
        if not self.o["raw"] and not name and not text and n == 1 and role in ("list item",
                                                                              "table cell",
                                                                              "menu item"):
            # An unnamed row whose one child is its label: name the row after it.
            ch = safe(lambda: acc.get_child_at_index(0))
            if ch is not None and safe(ch.get_role_name) == "label" and not safe(ch.get_child_count):
                name, n = clean(safe(ch.get_name, "")), 0
        anon = not name and not val and not text and not states
        printed = self.o["raw"] or self.o["all"] or not (anon and role in CONTAINERS)
        if (printed and not self.o["raw"] and not self.o["all"] and name and name == parent_name
                and not val and not text and not states and n):
            printed = False  # a wrapper repeating its parent's name (an icon's section, a button's)
        if (not self.o["raw"] and role in ("label", "static") and name and not n
                and (name == parent_name or name.rstrip("…") in parent_name)):
            # A button's own label, a paragraph's runs of text: the parent already says it.
            return
        if printed:
            parts = ["  " * depth + role]
            if name:
                parts.append('"%s"' % name)
            if text:
                parts.append("'%s'" % text)
            if val is not None:
                parts.append("=" + val)
            if states:
                parts.append("[" + " ".join(states) + "]")
            if b and b[2] > 0 and b[3] > 0:
                parts.append("@%d,%d %dx%d" % b)
            elif role not in ("application",):
                self.nobounds += 1
            if self.o["screen"]:
                sb = extents(acc, Atspi.CoordType.SCREEN)
                if sb:
                    parts.append("screen@%d,%d %dx%d" % sb)
            self.lines.append(" ".join(parts))
        for i in range(min(n, 500)):
            ch = safe(lambda: acc.get_child_at_index(i))
            if ch is not None:
                self.walk(ch, depth + 1 if printed else depth, budget,
                          (name or text) if printed else parent_name)


def main(argv):
    opts = {"all": False, "raw": False, "screen": False, "stats": False}
    app_re, pid = None, None
    args = list(argv)
    while args:
        a = args.pop(0)
        if a == "--pid":
            pid = int(args.pop(0))
        elif a.startswith("--") and a[2:] in opts:
            opts[a[2:]] = True
        else:
            app_re = re.compile(a, re.I)
    Atspi.init()
    Atspi.set_timeout(2000, 4000)
    desktop = Atspi.get_desktop(0)
    apps = [desktop.get_child_at_index(i) for i in range(desktop.get_child_count())]
    if app_re is None and pid is None:
        for a in apps:
            if a is None:
                continue
            wins = [safe(lambda: clean(a.get_child_at_index(j).get_name()), "?")
                    for j in range(safe(a.get_child_count, 0) or 0)]
            print('%s pid=%s windows=%s' % (safe(a.get_name, "?"), safe(a.get_process_id, "?"),
                                            wins))
        return 0
    t0 = time.monotonic()
    d = Dump(opts)
    found = 0
    for a in apps:
        if a is None:
            continue
        if pid is not None and safe(a.get_process_id) != pid:
            continue
        if app_re is not None and not app_re.search(safe(a.get_name, "") or ""):
            continue
        found += 1
        d.walk(a, 0)
    if not found:
        print("no such application on the a11y bus", file=sys.stderr)
        return 1
    print("\n".join(d.lines))
    if opts["stats"]:
        out = "\n".join(d.lines)
        print("stats: nodes=%d clipped=%d lines=%d bytes=%d ~tokens=%d nobounds=%d ms=%d" % (
            d.nodes, d.clipped, len(d.lines), len(out.encode()), len(out.encode()) // 3, d.nobounds,
            (time.monotonic() - t0) * 1000), file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
