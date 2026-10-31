#!/usr/bin/env python3
"""omabox check's helper (#252, finding 357): what `bin/omabox` asks of a diff and of two trees.

  check.py regions SPECK GAP < VERBOSE
      VERBOSE: magick's -connected-components verbose listing of a difference mask (white where
      two shots differ). Parts under SPECK pixels are specks (distance-field text varies by a few
      pixels a glyph from one start of an app to the next: never a part over 13 px measured), the
      rest are merged when GAP px apart or less. Prints "X,Y WxH PIXELS" a region, then
      "specks N PIXELS LARGEST" (LARGEST: the biggest speck's pixels, 0 for none).
  check.py holes screen|window W H CORNER GAP [X Y RW RH] < VERBOSE
      VERBOSE: the listing of a W x H mask, white where a shot is see-through (the sentinel
      wallpaper in a screen shot, alpha 0 in a window's own pixels). A part touching a screen
      shot's edge is the open wallpaper (around and between the surfaces); one whose box is at most
      CORNER px a side and that fills under half of it is a rounded corner (outside an arc: 21% of
      its square). The rest are holes, merged when GAP px apart or less. X Y RW RH: only what is
      inside that region, in its own pixels (a -g checkpoint). Prints "X,Y WxH PIXELS" a hole,
      then "corners N".
  check.py tree BASE.json NEW.json [IGNORE] [--shot W H [--mask X Y W H]...]
      Two `omabox tree --json` dumps of one window, element by element (by place: see load):
      moved, resized, name, text, value or state changed, appeared, vanished. IGNORE: a file of element keys to leave out
      (what differed between the baseline's two runs; "" for none). --shot: the checkpoint's shot is
      W x H, and each --mask a region of it the checkpoint --ignores: an element wholly inside one,
      in both trees, is left out as its pixels are (when the tree's pixels are the shot's: not on a
      scaled monitor). Prints a line a change; exit 1 when any.
  check.py tree-keys BASE.json NEW.json [IGNORE] [--shot W H [--mask X Y W H]...]
      The keys of the elements that differ, "KEY  (role "name")" a line (for IGNORE); exit 0.
  check.py unexplained BASE.json NEW.json [IGNORE] --shot W H [--mask X Y W H]... [--region X Y W H]...
      The regions (of a check's difference, in the shot's pixels) that no changed element's box
      touches, "X,Y WxH" a line: what the tree's changes do not account for (#257: two checkpoints
      with the same tree changes and none of these are the same change). Every region when the tree's
      pixels are not the shot's.
  check.py sheet PLAN OUT.png [FONT [SCALE]]
      The report of a check (finding 361): one image of every change, each region cropped with some
      context from the baseline and the new shot side by side, inside what a model sees in one look.
      PLAN: tab-separated lines, "title TEXT", then per changed checkpoint "point ID BASE.png NEW.png
      BASE.tree.json NEW.tree.json UNSTABLE" (empty fields: none) and its "region X Y W H" or "size
      TEXT" lines; "also FIRST ID": ID's change was FIRST's, named on FIRST's card. Prints magick's arguments NUL-separated, the first being what was left out ("" when
      nothing). SCALE (2 to 4, #402): the same sheet, laid out as at 1, drawn SCALE times its size
      (text, marks and crops; a crop at its shot's own pixels where the box's scale is SCALE), for
      people (a README, a site, a video); the default 1 is what an agent reads.
  check.py overview PLAN OUT.png [FONT [SCALE]]
      The baseline in one image (--approve): PLAN's "title TEXT", then per checkpoint "point ID PNG
      TEXT" and its "unstable X Y W H" (amber: left out) and "hole X Y W H" (red) lines; every shot
      scaled to fit one look, but none under OV_MIN_PX in area: then more pages, OUT-2.png...
      (OUT less a .part suffix). Prints magick's arguments as sheet does, the first being the pages
      after OUT, a line each ("" for one page). SCALE as sheet's; an OUT ending @SCALEx.png has its
      pages OUT-2@SCALEx.png...
  check.py crop-tree TREE.json X Y W H
      A window's tree for a crop of it (in the window's coordinates): the elements that touch the
      crop, their ancestors, their keys from the whole tree. JSON on stdout.
"""
import json
import sys


def parts(lines):
    """The white parts of a -connected-components verbose listing, as [x0, y0, x1, y1, area]."""
    for line in lines:
        f = line.split()
        # "  12: 5x3+100+40 102.0,41.0 9 gray(255)": id, geometry, centroid, area, colour (white:
        # gray(255), graya(255,...), srgb(255,...), white; the background is 0).
        if len(f) < 5 or not f[0].endswith(":"):
            continue
        c = f[-1]
        if not (c == "white" or c.split("(", 1)[-1].split(",")[0].rstrip(")") in ("255", "100%")):
            continue
        w, rest = f[1].split("x", 1)
        h, x, y = rest.split("+")
        yield [int(x), int(y), int(x) + int(w), int(y) + int(h), int(float(f[3]))]


def merge(boxes, gap):
    """Boxes GAP px apart or less as one, their areas added."""
    merged = True
    while merged:
        merged = False
        out = []
        for b in boxes:
            for o in out:
                if b[0] <= o[2] + gap and o[0] <= b[2] + gap and b[1] <= o[3] + gap and o[1] <= b[3] + gap:
                    o[0], o[1] = min(o[0], b[0]), min(o[1], b[1])
                    o[2], o[3] = max(o[2], b[2]), max(o[3], b[3])
                    o[4] += b[4]
                    merged = True
                    break
            else:
                out.append(b)
        boxes = out
    return sorted(boxes, key=lambda b: (b[1], b[0]))


def show(b):
    print(f"{b[0]},{b[1]} {b[2] - b[0]}x{b[3] - b[1]} {b[4]}")


def regions(speck, gap):
    boxes, specks, speck_px, largest = [], 0, 0, 0
    for b in parts(sys.stdin):
        if b[4] < speck:
            specks += 1
            speck_px += b[4]
            largest = max(largest, b[4])
            continue
        boxes.append(b)
    for b in merge(boxes, gap):
        show(b)
    print(f"specks {specks} {speck_px} {largest}")


def holes(kind, width, height, corner, gap, region):
    boxes, corners = [], 0
    for b in parts(sys.stdin):
        w, h = b[2] - b[0], b[3] - b[1]
        # The wallpaper around the surfaces and in the gaps between them reaches the screen's edge.
        if kind == "screen" and (b[0] == 0 or b[1] == 0 or b[2] == width or b[3] == height):
            continue
        if w <= corner and h <= corner and 2 * b[4] < w * h:
            corners += 1
            continue
        boxes.append(b)
    if region:
        rx, ry, rw, rh = region
        inside = []
        for b in boxes:
            x0, y0 = max(b[0], rx), max(b[1], ry)
            x1, y1 = min(b[2], rx + rw), min(b[3], ry + rh)
            if x0 >= x1 or y0 >= y1:
                continue
            # A hole the region cuts: its pixels counted as its share of its box.
            share = (x1 - x0) * (y1 - y0) / ((b[2] - b[0]) * (b[3] - b[1]))
            inside.append([x0 - rx, y0 - ry, x1 - rx, y1 - ry, max(1, round(b[4] * share))])
        boxes = inside
    for b in merge(boxes, gap):
        show(b)
    print(f"corners {corners}")


def load(path):
    """The tree as {key: node}, a key being the node's place: role#N from the window down, N its
    count among its parent's children of that role. A name is not in it, so a label whose text
    changes is that label changed, not one gone and another come; an element put in before others of
    its role shifts theirs."""
    with open(path, encoding="utf-8") as f:
        nodes = json.load(f)
    out, stack, counts = {}, [], []
    for n in nodes:
        if "key" in n:
            # A crop's tree (crop-tree): its keys are the whole window's, so an element outside the
            # crop does not shift the places of those inside.
            out[n["key"]] = n
            continue
        d = n["depth"]
        del stack[d:]
        while len(stack) < d:
            stack.append("?")
        del counts[d + 1:]
        while len(counts) <= d:
            counts.append({})
        i = counts[d].get(n["role"], 0) + 1
        counts[d][n["role"]] = i
        stack.append(f"{n['role']}#{i}")
        out["/".join(stack)] = n
    return out


def label(n):
    return n["role"] + (f' "{n["name"]}"' if n.get("name") else "")


def at(n):
    return f"@{n['x']},{n['y']} {n['w']}x{n['h']}" if "x" in n else ""


def place(key, nodes):
    """Where an element without a name is: from its nearest ancestor with one (the window at the
    latest) down to its own place, 'frame "W" > panel#1 > button#3' (the 3rd button there)."""
    parts = key.split("/")
    for i in range(len(parts) - 1, 0, -1):
        a = nodes.get("/".join(parts[:i]))
        if a and a.get("name"):
            return " > ".join([label(a)] + parts[i:])
    root = nodes.get(parts[0])
    return " > ".join(([label(root)] if root else [parts[0]]) + parts[1:])


def changes(base, new):
    """(key, line) for each difference, in the base's order, then what appeared. An element with no
    name that vanished or appeared says where it is (in what, which of its kind there)."""
    for key, b in base.items():
        n = new.get(key)
        if n is None:
            where = "" if b.get("name") else f"  (in {place(key, base)})"
            yield key, f"vanished  {label(b)}" + (f"  (was {at(b)})" if "x" in b else "") + where
            continue
        if b.get("name", "") != n.get("name", ""):
            yield key, f"name  {b['role']}  {json.dumps(b.get('name', ''))} -> {json.dumps(n.get('name', ''))}"
        short = label(n)
        if "x" in b and "x" in n:
            dx, dy = n["x"] - b["x"], n["y"] - b["y"]
            dw, dh = n["w"] - b["w"], n["h"] - b["h"]
            what = []
            if dx or dy:
                what.append(f"moved {dx:+d},{dy:+d}")
            if dw or dh:
                what.append(f"resized {dw:+d}x{dh:+d}")
            if what:
                yield key, f"{' '.join(what)}  {short}  {at(n)}"
        for field in ("text", "value"):
            if b.get(field, "") != n.get(field, ""):
                yield key, f"{field}  {short}  {json.dumps(b.get(field, ''))} -> {json.dumps(n.get(field, ''))}"
        if b.get("states", []) != n.get("states", []):
            yield key, f"state  {short}  [{','.join(b.get('states', []))}] -> [{','.join(n.get('states', []))}]"
    for key, n in new.items():
        if key not in base:
            where = "" if n.get("name") else f"  (in {place(key, new)})"
            yield key, f"appeared  {label(n)}  {at(n)}".rstrip() + where


def masked_by(base, new, shot, masks):
    """Whether an element is left out by the checkpoint's --ignore regions (in its shot's pixels):
    wholly inside one in each tree that has it. Only when the tree's pixels are the shot's (its
    window's or crop's size is the shot's); else nothing is."""
    root = next(iter(new.values()), None)
    if not masks or not shot or not root:
        return lambda key: False
    crop = root.get("crop")
    ox, oy = (crop["x"], crop["y"]) if crop else (0, 0)
    if ((crop["w"], crop["h"]) if crop else (root.get("w"), root.get("h"))) != shot:
        return lambda key: False

    def inside(n):
        if not n or "x" not in n:
            return None
        x, y = n["x"] - ox, n["y"] - oy
        return any(mx <= x and my <= y and x + n["w"] <= mx + mw and y + n["h"] <= my + mh for mx, my, mw, mh in masks)

    def masked(key):
        held = [inside(t.get(key)) for t in (base, new) if key in t]
        return bool(held) and all(held)
    return masked


def crop_tree(path, x, y, w, h):
    """A window's tree for a checkpoint of a crop of it (check point --window SEL -g): the window,
    the elements that touch the crop and their ancestors, each with its key in the whole tree, so
    one outside the crop that moves or goes changes nothing inside; an element without bounds goes
    with its nearest ancestor that has some. The window's node says the crop ("crop"), in its own
    coordinates, for the report's marks. JSON on stdout."""
    nodes = load(path)
    crop = (x, y, w, h)
    keep, inside = [], {}
    for key, n in nodes.items():
        if "/" not in key:
            inside[key] = True
            continue
        parent = key.rsplit("/", 1)[0]
        b = bounds(n)
        inside[key] = overlaps(b, crop) if b else inside.get(parent, False)
    for key in list(inside):
        if inside[key]:
            # its ancestors too
            parts = key.split("/")
            for i in range(1, len(parts)):
                inside["/".join(parts[:i])] = True
    for key, n in nodes.items():
        if inside[key]:
            m = dict(n, key=key)
            if "/" not in key:
                m["crop"] = {"x": x, "y": y, "w": w, "h": h}
            keep.append(m)
    json.dump(keep, sys.stdout)
    sys.stdout.write("\n")


# The report (finding 361): what a model sees in one look is 1568 px a side and ~1.15 MP (finding
# 243), so the sheet is 1456 px wide at most and its area under that; each side of a pair 706 px wide
# and 360 high at most, a region cropped with CTX px around it, crops of one checkpoint merged while
# they are near, a small crop shown 2x (a 2 px move is 4 there). More than CROPS_MAX crops in one
# checkpoint: the whole shots instead (a change of everything reads better as one picture).
SHEET_W, SHEET_PX, SHEET_H = 1456, 1150000, 1568
HALF_W, HALF_H = 706, 360
CTX, CROPS_MAX = 24, 6
ELEMENT_W, ELEMENT_H = 400, 120   # a region's element in its crop up to this
PAD, GAP, SEP = 6, 10, 8
PT, LH, CW = 14, 18, 8.5          # pointsize, line height, a monospace character's width at it
TEXT_MAX = 560                    # a card wider than its pair for its text, up to this
RED, BLUE, INK = "#ff2020", "#1a6fe0", "#111111"
# The scale a sheet is drawn at (#402): laid out at 1, every length drawn F times (an integer, so
# each one is exact); 1 is the agent's report.
F = 1


def z(v):
    """A length of the 1x layout as drawn."""
    return round(v * F)


def png_size(path):
    with open(path, "rb") as f:
        head = f.read(24)
    if head[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"not a PNG: {path}")
    return int.from_bytes(head[16:20], "big"), int.from_bytes(head[20:24], "big")


def overlaps(a, b, slack=0):
    return a[0] < b[0] + b[2] + slack and b[0] < a[0] + a[2] + slack and a[1] < b[1] + b[3] + slack and b[1] < a[1] + a[3] + slack


def bounds(n):
    return (n["x"], n["y"], n["w"], n["h"]) if n and "x" in n else None


def union(a, b):
    x0, y0 = min(a[0], b[0]), min(a[1], b[1])
    return (x0, y0, max(a[0] + a[2], b[0] + b[2]) - x0, max(a[1] + a[3], b[1] + b[3]) - y0)


def node_line(n):
    st = f" [{' '.join(n['states'])}]" if n.get("states") else ""
    return f"{label(n)}{st} {at(n)}"


def clean(text):
    """One printable line (names come from the app)."""
    return "".join(c if c.isprintable() else " " for c in text)


def fit(text, width):
    n = max(4, int(width // CW))
    return text if len(text) <= n else text[: n - 1] + "…"


def changed_boxes(base, new, size, ignore, masks):
    """Each changed element's box in the shot's pixels, {key: (box, tree line)}, the keys in ignore
    and the elements the masks hold left out; None when the tree's pixels are not the shot's (a
    scale)."""
    root = next(iter(new.values()), None)
    # A crop of the window (crop-tree): its elements' bounds less the crop's corner.
    crop = root.get("crop") if root else None
    ox, oy = (crop["x"], crop["y"]) if crop else (0, 0)
    if not root or ((crop["w"], crop["h"]) if crop else (root.get("w"), root.get("h"))) != size:
        return None

    def at_shot(n):
        return (n["x"] - ox, n["y"] - oy, n["w"], n["h"]) if n and "x" in n else None
    masked = masked_by(base, new, size, masks)
    changed = {}
    for key, line in changes(base, new):
        if key in ignore or key in changed or masked(key):
            continue
        b, n = at_shot(base.get(key)), at_shot(new.get(key))
        if b or n:
            changed[key] = (union(b, n) if b and n else (b or n), line)
    return changed


def marks_of(pt):
    """The checkpoint's marks: [box, colour, tree line or "", the element's box or None] for each
    region, then for each tree change with bounds no region covers (a move under a speck's size
    shows only there)."""
    marks = [[r, RED, "", None] for r in pt["regions"]]
    bt, nt = pt["trees"]
    if not (bt and nt):
        return marks
    base, new = load(bt), load(nt)
    ignore = set()
    if pt["unstable"]:
        with open(pt["unstable"], encoding="utf-8") as f:
            ignore = {l[5:].rstrip("\n").split("  (", 1)[0] for l in f if l.startswith("tree ")}
    # The checkpoint's --ignore regions, from its .point beside the new shot (none for check diff).
    masks = []
    try:
        with open(pt["new"].removesuffix(".png") + ".point", encoding="utf-8") as f:
            masks = [tuple(int(v) for v in l.split()[1:5]) for l in f if l.startswith("ignore ")]
    except OSError:
        pass
    changed = changed_boxes(base, new, pt["size"], ignore, masks)
    if changed is None:
        return marks   # the tree's pixels are not the shot's (a scale): no marks from it
    root = next(iter(new.values()))
    crop = root.get("crop")
    ox, oy = (crop["x"], crop["y"]) if crop else (0, 0)

    def bounds(n):
        return (n["x"] - ox, n["y"] - oy, n["w"], n["h"]) if n and "x" in n else None
    for m in marks:
        hits = [c for c in changed.values() if overlaps(c[0], m[0])]
        if hits:
            m[3], m[2] = min(hits, key=lambda c: c[0][2] * c[0][3])
        else:
            els = [n for n in new.values() if n.get("depth", 0) > 0 and bounds(n) and overlaps(bounds(n), m[0])]
            if els:
                n = min(els, key=lambda n: n["w"] * n["h"])
                m[2], m[3] = node_line(n), bounds(n)
    for box, line in changed.values():
        if box[2] > 0 and box[3] > 0 and not any(overlaps(box, r, 2) for r in pt["regions"]):
            marks.append([box, BLUE, line, box])
    return marks


def crops_of(pt, marks):
    """Boxes around the marks, CTX px of context, merged while they are under CTX apart and fit one
    side (a card's lines cost more than the pixels between two near crops). A mark's element is in
    its crop when that stays small (the whole label whose last word changed)."""
    W, H = pt["size"]
    boxes = []
    for box, _, _, el in marks:
        if el:
            u = union(box, el)
            if u[2] <= ELEMENT_W and u[3] <= ELEMENT_H:
                box = u
        x, y, w, h = box
        x0, y0 = max(0, x - CTX), max(0, y - CTX)
        boxes.append((x0, y0, min(W, x + w + CTX) - x0, min(H, y + h + CTX) - y0))
    merged = True
    while merged:
        merged = False
        out = []
        for b in boxes:
            for i, o in enumerate(out):
                u = union(o, b)
                if overlaps(o, b, CTX) and u[2] <= HALF_W and u[3] <= HALF_H:
                    out[i] = u
                    merged = True
                    break
            else:
                out.append(b)
        boxes = out
    return sorted(boxes, key=lambda b: (b[1], b[0]))


def card(pt, box, marks, whole=False, lim=1):
    """A card: its lines, its pair's box and scale, its size."""
    cx, cy, cw, ch = box
    s = min(HALF_W * lim / cw, HALF_H * lim / ch)
    s = 2 if s >= 2 and cw <= 160 and ch <= 120 else (1 if s >= 1 else s)
    sw, sh = max(1, round(cw * s)), max(1, round(ch * s))
    inside = [m for m in marks if overlaps(m[0], box)]
    regs = [m for m in inside if m[1] == RED]
    if pt["sizechange"]:
        head = f"{pt['id']}: size {pt['sizechange']} (both whole, at {round(s * 100)}%)"
    elif whole:
        head = f"{pt['id']}: {len(regs)} regions (the whole shots, at {round(s * 100)}%)"
    elif regs:
        head = f"{pt['id']}: {f'{len(regs)} regions, ' if len(regs) > 1 else ''}" + ", ".join(
            f"at {x},{y} {w}x{h}" for (x, y, w, h), *_ in regs)
    else:
        head = f"{pt['id']}: in the tree only (under a speck's size in pixels)"
    lines = [(clean(head), INK)]
    if pt.get("also"):
        # The checkpoints whose change was this one's (#257): named here, not a card each.
        lines.append((clean(f"the same in {len(pt['also'])} more: {', '.join(pt['also'])}"), INK))
    seen = []
    for _, colour, line, _ in inside:
        if line and line not in seen:
            seen.append(line)
            if len(seen) <= 3:
                lines.append(("tree: " + clean(line), colour))
    if len(seen) > 3:
        lines[-1] = (f"tree: +{len(seen) - 2} more (check run's output lists them)", INK)
    pair = 2 * sw + SEP
    textw = max(len(t) for t, _ in lines) * CW
    w = max(pair, min(textw, TEXT_MAX)) + 2 * PAD
    h = PAD + len(lines) * LH + 4 + sh + PAD
    return {"pt": pt, "box": box, "s": s, "sw": sw, "sh": sh, "marks": inside, "lines": lines, "w": int(w), "h": int(h)}


def wrap(text, width):
    """Lines of at most width px, broken at spaces."""
    n, lines = max(8, int(width // CW)), [""]
    for word in text.split(" "):
        if lines[-1] and len(lines[-1]) + 1 + len(word) > n:
            lines.append("")
        lines[-1] += (" " if lines[-1] else "") + word
    return lines


def layout(cards, footer, title):
    """Rows, left to right: [(card, x, y)], the sheet's width and height, and the title's lines
    (wrapped to the cards' width, 360 px at least: a legend wider than a small report costs more
    than its lines)."""
    x, y, rowh, out, W = GAP, 0, 0, [], 360
    for c in cards:
        if x > GAP and x + c["w"] + GAP > SHEET_W:
            x, y, rowh = GAP, y + rowh + GAP, 0
        out.append([c, x, y])
        x += c["w"] + GAP
        rowh = max(rowh, c["h"])
        W = max(W, x)
    lines = wrap(clean(title), W - 2 * GAP)
    top = GAP + len(lines) * LH + 4
    for o in out:
        o[2] += top
    H = top + y + rowh + GAP + footer * LH
    return out, W, H, lines


def corners(x0, y0, x1, y1):
    """A mark's four corners, not its whole box: its edges would hide what is beside the change (the
    digit before a time's changed ones)."""
    ax, ay = max(3 * F, min(12 * F, (x1 - x0) // 4)), max(3 * F, min(12 * F, (y1 - y0) // 4))
    return " ".join(f"line {x},{y} {x + dx * ax},{y} line {x},{y} {x},{y + dy * ay}"
                    for x, y, dx, dy in ((x0, y0, 1, 1), (x1, y0, -1, 1), (x0, y1, 1, -1), (x1, y1, -1, -1)))


def esc(text):
    """-annotate reads % and \\ as escapes, and a text starting with @ as a file's name."""
    text = text.replace("\\", "\\\\").replace("%", "%%")
    return " " + text if text.startswith("@") else text


def cards_of(points, marks, lim):
    """Each checkpoint's cards, in order, each side of a pair at most lim of HALF_W x HALF_H."""
    per = []
    for pt, mk in zip(points, marks):
        if pt["sizechange"]:
            bw, bh = png_size(pt["base"])
            nw, nh = pt["size"]
            # Both at one scale, so their sizes compare: the box is the bigger of the two.
            per.append([card(pt, (0, 0, max(bw, nw), max(bh, nh)), [], lim=lim)])
            continue
        boxes = crops_of(pt, mk)
        if len(boxes) > CROPS_MAX:
            per.append([card(pt, (0, 0) + pt["size"], mk, whole=True, lim=lim)])
        else:
            per.append([card(pt, b, mk, lim=lim) for b in boxes])
    return per


def choose(per, title):
    """Round by round (each checkpoint's first card, then its second...), the cards that fit, as
    {(checkpoint, card)}; laid out in order."""
    keep = set()
    for r in range(max((len(c) for c in per), default=0)):
        for i, cs in enumerate(per):
            if r >= len(cs):
                continue
            trial = keep | {(i, r)}
            _, W, H, _ = layout([c for j, cs2 in enumerate(per) for k, c in enumerate(cs2) if (j, k) in trial], 2, title)
            if W <= SHEET_W and H <= SHEET_H and W * H <= SHEET_PX:
                keep = trial
    return keep


def sheet(plan, out_png, font):
    title, points = "", []
    with open(plan, encoding="utf-8") as f:
        for line in f:
            p = line.rstrip("\n").split("\t")
            if p[0] == "title":
                title = p[1]
            elif p[0] == "point":
                points.append({"id": p[1], "base": p[2], "new": p[3], "trees": (p[4], p[5]), "unstable": p[6],
                               "regions": [], "sizechange": ""})
                points[-1]["size"] = png_size(p[3])
            elif p[0] == "region":
                points[-1]["regions"].append(tuple(int(v) for v in p[1:5]))
            elif p[0] == "size":
                points[-1]["sizechange"] = p[1]
            elif p[0] == "also":
                for pt in points:
                    if pt["id"] == p[1]:
                        pt.setdefault("also", []).append(p[2])
    marks = [marks_of(pt) if not pt["sizechange"] else [] for pt in points]
    # At full size first; when that leaves some out, each side at half the size, if that shows more.
    best = None
    for lim in (1, 0.5):
        per = cards_of(points, marks, lim)
        keep = choose(per, title)
        if best is None or len(keep) > len(best[1]):
            best = (per, keep)
        if len(keep) == sum(len(cs) for cs in per):
            break
    per, keep = best
    shown = [c for j, cs in enumerate(per) for k, c in enumerate(cs) if (j, k) in keep]
    left = []
    for j, cs in enumerate(per):
        n = sum(1 for k in range(len(cs)) if (j, k) not in keep)
        if n == len(cs):
            left.append(cs[0]["pt"]["id"])
        elif n:
            left.append(f"{cs[0]['pt']['id']} ({n} of {len(cs)} crops)")
    left_text = ", ".join(left)
    placed, W, H, tlines = layout(shown, 2 if left else 0, title)
    a = ["-size", f"{z(W)}x{z(H)}", "xc:white"] + (["-font", font] if font else []) + ["-pointsize", str(PT * F)]

    def text(x, y, t, colour):
        a.extend(["-stroke", "none", "-fill", colour, "-annotate", f"+{z(x)}+{z(y + LH - 5)}", esc(t)])

    for i, t in enumerate(tlines):
        text(GAP, GAP + i * LH, t, INK)
    for c, x, y in placed:
        a.extend(["-stroke", "none", "-fill", "#e6e6e6", "-draw", f"rectangle {z(x)},{z(y)} {z(x + c['w']) - 1},{z(y + c['h']) - 1}"])
        for i, (t, colour) in enumerate(c["lines"]):
            text(x + PAD, y + PAD + i * LH, fit(t, c["w"] - 2 * PAD), colour)
        iy = y + PAD + len(c["lines"]) * LH + 4
        cx, cy, cw, ch = c["box"]
        s = c["s"] * F   # (a shot's pixel drawn s times)
        for k, path in enumerate((c["pt"]["base"], c["pt"]["new"])):
            ix = x + PAD + k * (c["sw"] + SEP)
            if c["pt"]["sizechange"]:
                iw, ih = png_size(path)
                geo, crop = f"{max(1, round(iw * s))}x{max(1, round(ih * s))}!", []
            else:
                geo, crop = f"{z(c['sw'])}x{z(c['sh'])}!", ["-crop", f"{cw}x{ch}+{cx}+{cy}", "+repage"]
            a.extend(["(", "png:" + path, "-alpha", "off"] + crop + (["-scale", geo] if s >= 1 else ["-resize", geo]))
            a.extend(["-fill", "none", "-strokewidth", str(2 * F)])
            for (mx, my, mw, mh), colour, *_ in c["marks"]:
                x0, y0 = int((mx - cx) * s) - 3 * F, int((my - cy) * s) - 3 * F
                x1, y1 = int((mx + mw - cx) * s + 0.999) + 2 * F, int((my + mh - cy) * s + 0.999) + 2 * F
                a.extend(["-stroke", colour, "-draw", corners(x0, y0, x1, y1)])
            a.extend([")", "-geometry", f"+{z(ix)}+{z(iy)}", "-composite"])
    if left:
        fy = H - GAP - 2 * LH + 4
        text(GAP, fy, fit("not shown, no room (what changed there is in check's output):", W - 2 * GAP), INK)
        text(GAP, fy + LH, fit(left_text, W - 2 * GAP), INK)
    a.extend(["-alpha", "off", "-depth", "8", f"PNG:{out_png}"])
    sys.stdout.write("\0".join([left_text] + a) + "\0")


# The baseline's overview (--approve): every checkpoint as it was kept, in one look, what a check
# will leave out (unstable regions, amber) and its holes (red) marked on it. A person or an agent
# looks at it before keeping the baseline: when most runs showed the same wrong screen (an error, a
# page not loaded yet), that is what the baseline is.
AMBER = "#e09000"


# Each shot at least OV_MIN_PX in area (400x225 for a 16:9 screen; a tall sheet narrower, a bar's
# strip at the sheet's width), or as it is when smaller, so a wrong screen can be told from a right
# one; what does not fit one look then goes on more pages (#256: 28 checkpoints in one look were
# 40-70 px each). A card is its shot's width, its title cut to it, and OV_CARD_MIN at least.
OV_MIN_PX, OV_CARD_MIN = 90000, 200


def fits(W, H):
    return W <= SHEET_W and H <= SHEET_H and W * H <= SHEET_PX


def overview(plan, out_png, font):
    title, points = "", []
    with open(plan, encoding="utf-8") as f:
        for line in f:
            p = line.rstrip("\n").split("\t")
            if p[0] == "title":
                title = p[1]
            elif p[0] == "point":
                points.append({"id": p[1], "png": p[2], "text": p[3], "marks": []})
                points[-1]["size"] = png_size(p[2])
            elif p[0] in ("unstable", "hole"):
                points[-1]["marks"].append((AMBER if p[0] == "unstable" else RED, tuple(int(v) for v in p[1:5])))
    room = SHEET_W - 2 * GAP - 2 * PAD

    def cards_at(f):
        cards = []
        for pt in points:
            w, h = pt["size"]
            top = min(1.0, room / w)
            smin = min(top, (OV_MIN_PX / (w * h)) ** 0.5)
            sc = max(top * f, smin)
            sw, sh = max(1, round(w * sc)), max(1, round(h * sc))
            cards.append({"pt": pt, "s": sc, "floor": sc <= smin, "sw": sw, "sh": sh,
                          "lines": [(clean(f"{pt['id']}: {pt['text']}"), INK)],
                          "w": max(sw, OV_CARD_MIN) + 2 * PAD, "h": PAD + LH + 4 + sh + PAD})
        return cards

    # All of them smaller together until they fit one look, each no smaller than its floor; then pages.
    paged = title + " (page 99 of 99)"
    f = 1.0
    while True:
        cards = cards_at(f)
        _, W, H, _ = layout(cards, 0, title)
        if fits(W, H):
            pages = [cards]
            break
        if all(c["floor"] for c in cards) or f < 0.05:
            pages, page = [], []
            for c in cards:
                _, W, H, _ = layout(page + [c], 0, paged)
                if page and not fits(W, H):
                    pages.append(page)
                    page = []
                page.append(c)
            pages.append(page)
            break
        f *= 0.9
    n = len(pages)
    stem, at = out_png.removesuffix(".part").removesuffix(".png"), ""
    if F > 1 and stem.endswith(f"@{F}x"):
        stem, at = stem[: -len(f"@{F}x")], f"@{F}x"
    paths = [out_png] + [f"{stem}-{k}{at}.png" for k in range(2, n + 1)]
    a = (["-font", font] if font else []) + ["-pointsize", str(PT * F)]

    def text(x, y, t, colour):
        a.extend(["-stroke", "none", "-fill", colour, "-annotate", f"+{z(x)}+{z(y + LH - 5)}", esc(t)])

    for k, page in enumerate(pages):
        placed, W, H, tlines = layout(page, 0, title if n == 1 else f"{title} (page {k + 1} of {n})")
        a.extend(["(", "-size", f"{z(W)}x{z(H)}", "xc:white"])
        for i, t in enumerate(tlines):
            text(GAP, GAP + i * LH, t, INK)
        for c, x, y in placed:
            a.extend(["-stroke", "none", "-fill", "#e6e6e6", "-draw", f"rectangle {z(x)},{z(y)} {z(x + c['w']) - 1},{z(y + c['h']) - 1}"])
            text(x + PAD, y + PAD, fit(c["lines"][0][0], c["w"] - 2 * PAD), INK)
            iy, sc = y + PAD + LH + 4, c["s"] * F
            a.extend(["(", "png:" + c["pt"]["png"], "-alpha", "off"] + (["-scale"] if sc >= 1 else ["-resize"]) + [f"{z(c['sw'])}x{z(c['sh'])}!"])
            for colour, (mx, my, mw, mh) in c["pt"]["marks"]:
                x0, y0 = int(mx * sc), int(my * sc)
                x1, y1 = max(x0 + 1, int((mx + mw) * sc + 0.999) - 1), max(y0 + 1, int((my + mh) * sc + 0.999) - 1)
                if colour == AMBER:
                    # Left out: a see-through fill over it and its edge, so what is under it still reads.
                    a.extend(["-stroke", colour, "-strokewidth", str(F), "-fill", colour + "40", "-draw", f"rectangle {x0},{y0} {x1},{y1}"])
                else:
                    a.extend(["-stroke", colour, "-strokewidth", str(2 * F), "-fill", "none",
                              "-draw", corners(x0 - 3 * F, y0 - 3 * F, x1 + 3 * F, y1 + 3 * F)])
            a.extend([")", "-geometry", f"+{z(x + PAD)}+{z(iy)}", "-composite"])
        a.extend(["-alpha", "off", "-depth", "8"])
        # Every page but the last written on the way (and dropped); the last is magick's output.
        a.extend(["-write", f"PNG:{paths[k]}", "+delete", ")"] if k < n - 1 else [")"])
    a.append(f"PNG:{paths[-1]}")
    # The first field: the pages after the first, a line each (the caller says them).
    sys.stdout.write("\0".join(["\n".join(paths[1:])] + a) + "\0")


def main():
    global F
    if len(sys.argv) >= 4 and sys.argv[1] == "regions":
        regions(int(sys.argv[2]), int(sys.argv[3]))
        return 0
    if len(sys.argv) in (7, 11) and sys.argv[1] == "holes" and sys.argv[2] in ("screen", "window"):
        n = [int(a) for a in sys.argv[3:]]
        holes(sys.argv[2], n[0], n[1], n[2], n[3], n[4:] or None)
        return 0
    if len(sys.argv) >= 4 and sys.argv[1] in ("tree", "tree-keys", "unexplained"):
        base, new = load(sys.argv[2]), load(sys.argv[3])
        rest = sys.argv[4:]
        ignore = set()
        if rest and not rest[0].startswith("--"):
            if rest[0]:
                with open(rest[0], encoding="utf-8") as f:
                    # "KEY  (what it is)" lines: the key is before the two spaces.
                    ignore = {line.rstrip("\n").split("  (", 1)[0] for line in f if line.strip()}
            rest = rest[1:]
        shot, masks, regs = None, [], []
        while rest:
            if rest[0] == "--shot" and len(rest) >= 3:
                shot, rest = (int(rest[1]), int(rest[2])), rest[3:]
            elif rest[0] in ("--mask", "--region") and len(rest) >= 5:
                (masks if rest[0] == "--mask" else regs).append(tuple(int(v) for v in rest[1:5]))
                rest = rest[5:]
            else:
                print(__doc__.strip(), file=sys.stderr)
                return 2
        if sys.argv[1] == "unexplained":
            boxes = changed_boxes(base, new, shot, ignore, masks) if shot else None
            for r in regs:
                if boxes is None or not any(overlaps(b, r, 2) for b, _ in boxes.values()):
                    print(f"{r[0]},{r[1]} {r[2]}x{r[3]}")
            return 0
        masked = masked_by(base, new, shot, masks)
        found = 0
        keys = []
        what = {}
        for key, line in changes(base, new):
            # Its box too: an --ignore meant for it must hold all of it, not only its text.
            n = base.get(key) or new[key]
            what.setdefault(key, f"{label(n)} {at(n)}".rstrip())
            if key in ignore or masked(key):
                continue
            found += 1
            if sys.argv[1] == "tree":
                print(line)
            elif key not in keys:
                keys.append(key)
        for key in keys:
            print(f"{key}  ({what[key]})")
        return 1 if found and sys.argv[1] == "tree" else 0
    if len(sys.argv) == 7 and sys.argv[1] == "crop-tree":
        crop_tree(sys.argv[2], *(int(a) for a in sys.argv[3:7]))
        return 0
    if len(sys.argv) in (4, 5, 6) and sys.argv[1] in ("overview", "sheet") and sys.argv[5:] in ([], ["1"], ["2"], ["3"], ["4"]):
        F = int(sys.argv[5]) if len(sys.argv) == 6 else 1
        (overview if sys.argv[1] == "overview" else sheet)(sys.argv[2], sys.argv[3], sys.argv[4] if len(sys.argv) >= 5 else "")
        return 0
    print(__doc__.strip(), file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
