// omabox-tree: one window's accessibility (AT-SPI) tree as compact text, for `omabox tree` (#178).
//
//   omabox-tree --pid PID [--title TITLE] [--size WxH] [--twins N] [--find TEXT] [--json] [--all]
//   omabox-tree --pid PID --layer --size WxH [--offset X,Y] [--active yes|no] [--find TEXT] [--json] [--all]
//
// Run by `omabox tree` inside the box (box_exec: the box's session bus and pid numbering), never on the
// host, where it would read the real desktop's apps: it refuses outside a box. PID is the window's
// process as the box's Hyprland has it, which is the AT-SPI application's (NOTES finding 244); TITLE
// picks that application's window (its frame's name) when it has several; WxH is the window's size,
// and what lies wholly outside it (a long page scrolled out) is left out. An application's only frame
// stands for a window whose title it does not name only when it is the window's size and N (the
// application's other windows of that size, from `omabox tree`) is 0 (#302): an app can publish one of
// its windows (egui's root viewport, not its child ones), and that one's tree is not this window's.
//
// A line is `role "name" 'text' =value [states] @x,y wxh`, indented two spaces a level; the bounds are
// in window coordinates, what `omabox click --window` takes. Anonymous containers, hidden nodes, a
// control's own label and wrappers repeating their parent's name are left out (--all keeps them).
// --find TEXT prints only the lines holding TEXT (any case; not in the bounds), each after its
// printed ancestors: `list "Options" > check box "Dark mode" [checked] @12,40 140x24`.
//
// Before reading, it sets org.a11y.Status.IsEnabled and ScreenReaderEnabled (what a screen reader
// sets): Qt apps export their tree only then, and follow it live; AccessKit apps (egui, winit) before
// accesskit_unix 0.22 watch only ScreenReaderEnabled (#237); GTK4 exports always. at-spi2-core 2.61
// and later have no ScreenReaderEnabled: left alone there, said only when PID is not on the bus. One
// that just turned either on waits up to 3 s for PID's application to register (again).
//
// --layer (#254, NOTES finding 358): a layer surface's tree instead of a window's. Its frame has no
// name (Quickshell's), so it is the one of PID's applications' frames that is WxH (the empty `gtk`
// application Omarchy's QT_QPA_PLATFORMTHEME adds beside Quickshell's `Qt` one has none); --active
// tells frames of one size apart when `omabox tree` knows that the layer is (yes) or is not (no) the
// one of them with the keyboard: applied only when exactly one of them is active. Bounds are the
// surface's own plus --offset (the layer's place on the screen: what `omabox click X Y` takes).
//
// Exit: 0 printed; 1 --find matched nothing; 3 no tree for that window (said why); 4 the box has no
// accessibility bus; 5 the layer's frame has nothing under it; 6 several frames are the layer's size,
// or the app's only frame is the size of several of its windows; 7 the app's only frame is not the
// window's size (that window is not on the bus); 2 usage, or outside a box.
#define _GNU_SOURCE
#include <atspi/atspi.h>
#include <gio/gio.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <time.h>

#include "../common/box.h"

#define BUDGET 4000      // nodes visited at most (a long page is tens of thousands)
#define FULL 4000        // characters of a name, text or value read at most (#314: --json and --find
                         // see them whole; the text view cuts them, NAME_W and VALUE_W)
#define NAME_W 80        // a name's or text's characters in the text view
#define VALUE_W 40       // a value's
#define MAX_CHILDREN 500 // children of one node read at most
#define MAX_DEPTH 64

static int opt_all, opt_json, win_w = -1, win_h = -1, opt_layer, layer_ox, layer_oy, active_hint = -1, twins;
static const char *find;
static int nodes, lines, matched, truncated;
static GString *out;

// The printed ancestors of the node being read, for --find's path: as the text view says them, and
// whole for --json.
static char *path[MAX_DEPTH], *path_full[MAX_DEPTH];
static int path_n;

static const struct { AtspiStateType st; const char *word; } SHOWN[] = {
    {ATSPI_STATE_FOCUSED, "focused"},   {ATSPI_STATE_CHECKED, "checked"},
    {ATSPI_STATE_SELECTED, "selected"}, {ATSPI_STATE_PRESSED, "pressed"},
    {ATSPI_STATE_EXPANDED, "expanded"}, {ATSPI_STATE_INDETERMINATE, "mixed"},
    {ATSPI_STATE_EDITABLE, "editable"}, {ATSPI_STATE_READ_ONLY, "readonly"},
    {ATSPI_STATE_MODAL, "modal"},       {ATSPI_STATE_ACTIVE, "active"},
    {ATSPI_STATE_BUSY, "busy"},         {ATSPI_STATE_INVALID_ENTRY, "invalid"},
};
static const char *CONTAINERS[] = {"filler", "panel", "section", "unknown", "grouping", "scroll pane", "viewport",
    "layered pane", "split pane", "redundant object", "invalid", "internal frame", "block quote", "static",
    "landmark", "form", "paragraph", NULL};
static const char *TEXT_ROLES[] = {"text", "entry", "password text", "terminal", "label", "static", "paragraph",
    "heading", "document text", "editbar", NULL};
static const char *CHECKABLE[] = {"check box", "radio button", "toggle button", "check menu item",
    "radio menu item", "switch", NULL};

static int in(const char *s, const char **set) {
    for (; *set; set++)
        if (strcmp(s, *set) == 0) return 1;
    return 0;
}

// g cut to n characters, its last one an ellipsis, when it is longer.
static void cut_to(GString *g, glong n) {
    if (g_utf8_strlen(g->str, -1) > n) {
        char *end = g_utf8_offset_to_pointer(g->str, n - 1);
        g_string_truncate(g, end - g->str);
        g_string_append(g, "…");
    }
}

// s as the text view says it: at most n characters.
static char *cut(const char *s, glong n) {
    GString *g = g_string_new(s);
    cut_to(g, n);
    return g_string_free(g, FALSE);
}

// One line of text: U+FFFC (an embedded child) and private-use glyphs (icon fonts) out, runs of
// white space as one space, at most n characters.
static char *clean(const char *s, glong n) {
    GString *g = g_string_new(NULL);
    int space = 0;
    for (const char *p = s ? s : ""; *p; p = g_utf8_next_char(p)) {
        gunichar c = g_utf8_get_char_validated(p, -1);
        if (c == (gunichar)-1 || c == (gunichar)-2) break;
        if (c == 0xFFFC || (c >= 0xE000 && c <= 0xF8FF)) continue;
        if (g_unichar_isspace(c)) { space = g->len > 0; continue; }
        if (space) g_string_append_c(g, ' ');
        space = 0;
        g_string_append_unichar(g, c);
    }
    cut_to(g, n);
    return g_string_free(g, FALSE);
}

static void json_str(GString *g, const char *s) {
    g_string_append_c(g, '"');
    for (const unsigned char *p = (const unsigned char *)s; *p; p++) {
        if (*p == '"' || *p == '\\') g_string_append_printf(g, "\\%c", *p);
        else if (*p < 0x20) g_string_append_printf(g, "\\u%04x", *p);
        else g_string_append_c(g, *p);
    }
    g_string_append_c(g, '"');
}

static char *role_of(AtspiAccessible *a) {
    char *r = atspi_accessible_get_role_name(a, NULL);
    return r ? r : g_strdup("?");
}
static char *name_of(AtspiAccessible *a, glong n) {
    char *raw = atspi_accessible_get_name(a, NULL), *s = clean(raw, n);
    g_free(raw);
    return s;
}

// The text a shot would be read for, when the name does not carry it.
static char *text_of(AtspiAccessible *a, const char *role) {
    if (!in(role, TEXT_ROLES)) return g_strdup("");
    AtspiText *t = atspi_accessible_get_text_iface(a);
    if (!t) return g_strdup("");
    gint n = atspi_text_get_character_count(t, NULL);
    char *raw = n > 0 ? atspi_text_get_text(t, 0, MIN(n, FULL), NULL) : NULL, *s = clean(raw, FULL);
    g_free(raw);
    g_object_unref(t);
    return s;
}

static char *value_of(AtspiAccessible *a) {
    AtspiValue *v = atspi_accessible_get_value_iface(a);
    if (!v) return NULL;
    GError *e = NULL;
    char *txt = atspi_value_get_text(v, NULL), *r = NULL;
    if (txt && *txt) r = clean(txt, FULL);
    else {
        gdouble cur = atspi_value_get_current_value(v, &e);
        if (!e) r = g_strdup_printf("%g", cur);
    }
    g_clear_error(&e);
    g_free(txt);
    g_object_unref(v);
    return r;
}

// A closed combo box's choice: its (hidden) menu's selected item, as Chromium has it.
static char *combo_choice(AtspiAccessible *a) {
    gint n = atspi_accessible_get_child_count(a, NULL);
    for (int i = 0; i < MIN(n, 4); i++) {
        AtspiAccessible *menu = atspi_accessible_get_child_at_index(a, i, NULL);
        if (!menu) continue;
        gint m = atspi_accessible_get_child_count(menu, NULL);
        for (int j = 0; j < MIN(m, 200); j++) {
            AtspiAccessible *it = atspi_accessible_get_child_at_index(menu, j, NULL);
            if (!it) continue;
            AtspiStateSet *ss = atspi_accessible_get_state_set(it);
            int sel = ss && atspi_state_set_contains(ss, ATSPI_STATE_SELECTED);
            if (ss) g_object_unref(ss);
            if (sel) {
                char *s = name_of(it, FULL);
                g_object_unref(it);
                g_object_unref(menu);
                if (*s) return s;
                g_free(s);
                return NULL;
            }
            g_object_unref(it);
        }
        g_object_unref(menu);
    }
    return NULL;
}

static int extents_in(AtspiAccessible *a, AtspiCoordType type, int b[4]) {
    AtspiComponent *c = atspi_accessible_get_component_iface(a);
    if (!c) return 0;
    GError *e = NULL;
    AtspiRect *r = atspi_component_get_extents(c, type, &e);
    g_object_unref(c);
    if (e || !r) { g_clear_error(&e); g_free(r); return 0; }
    b[0] = r->x, b[1] = r->y, b[2] = r->width, b[3] = r->height;
    g_free(r);
    return 1;
}

// Qt measures an element's window coordinates from its nearest dialog or window, and counts a Qt
// Quick popup with that role inside the window as one: the popup says 0,0 and its contents are
// relative to it. Its screen coordinates are right relative to the window's (a Wayland window's
// screen origin is 0,0 to Qt), so the popup's offset is the difference, and its contents take it.
static int frame_sx, frame_sy, have_frame_screen, off_x, off_y;

static int extents(AtspiAccessible *a, int depth, const char *role, int b[4]) {
    if (!extents_in(a, ATSPI_COORD_TYPE_WINDOW, b)) return 0;
    int s[4];
    if (depth == 0) {
        have_frame_screen = extents_in(a, ATSPI_COORD_TYPE_SCREEN, s);
        if (have_frame_screen) frame_sx = s[0], frame_sy = s[1];
        off_x = off_y = 0;
    } else if (b[0] == 0 && b[1] == 0 && have_frame_screen && (!strcmp(role, "dialog") || !strcmp(role, "window")) &&
               extents_in(a, ATSPI_COORD_TYPE_SCREEN, s)) {
        off_x = s[0] - frame_sx, off_y = s[1] - frame_sy;
    }
    b[0] += off_x, b[1] += off_y;
    return 1;
}

// Whether TEXT is in s, any case.
static int holds(const char *s, const char *text) {
    char *a = g_utf8_casefold(s, -1), *b = g_utf8_casefold(text, -1);
    int r = strstr(a, b) != NULL;
    g_free(a);
    g_free(b);
    return r;
}

// A node's line without its bounds; names, text and values cut as the text view has them when cut.
static GString *line_of(const char *role, const char *name, const char *text, const char *val, GPtrArray *states,
                        int cut_them) {
    char *n = cut_them ? cut(name, NAME_W) : g_strdup(name), *t = cut_them ? cut(text, NAME_W) : g_strdup(text),
         *v = val && cut_them ? cut(val, VALUE_W) : g_strdup(val);
    GString *body = g_string_new(role);
    if (*n) g_string_append_printf(body, " \"%s\"", n);
    if (*t) g_string_append_printf(body, " '%s'", t);
    if (v) g_string_append_printf(body, " =%s", v);
    if (states->len) {
        g_string_append(body, " [");
        for (guint i = 0; i < states->len; i++) g_string_append_printf(body, "%s%s", i ? " " : "", (char *)states->pdata[i]);
        g_string_append_c(body, ']');
    }
    g_free(n);
    g_free(t);
    g_free(v);
    return body;
}

static void emit(int depth, const char *role, const char *name, const char *text, const char *val,
                 GPtrArray *states, int *b) {
    // --find matches the whole strings (#314), whatever the text view shows of them.
    GString *body = line_of(role, name, text, val, states, 0);
    lines++;
    int hit = !find || holds(body->str, find);
    if (hit) {
        matched++;
        if (opt_json) {
            g_string_append(out, out->len ? ",\n " : "[");
            g_string_append_printf(out, "{\"depth\": %d, \"role\": ", depth);
            json_str(out, role);
            g_string_append(out, ", \"name\": ");
            json_str(out, name);
            if (*text) { g_string_append(out, ", \"text\": "); json_str(out, text); }
            if (val) { g_string_append(out, ", \"value\": "); json_str(out, val); }
            g_string_append(out, ", \"states\": [");
            for (guint i = 0; i < states->len; i++) {
                if (i) g_string_append(out, ", ");
                json_str(out, states->pdata[i]);
            }
            g_string_append_c(out, ']');
            if (b) g_string_append_printf(out, ", \"x\": %d, \"y\": %d, \"w\": %d, \"h\": %d", b[0] + layer_ox, b[1] + layer_oy, b[2], b[3]);
            if (find) {
                g_string_append(out, ", \"path\": [");
                for (int i = 1; i < path_n; i++) {
                    if (i > 1) g_string_append(out, ", ");
                    json_str(out, path_full[i]);
                }
                g_string_append_c(out, ']');
            }
            g_string_append_c(out, '}');
        } else {
            if (find) {
                // The window itself (path[0]) is the one asked for: not repeated on every line.
                for (int i = 1; i < path_n; i++) g_string_append_printf(out, "%s > ", path[i]);
            } else {
                for (int i = 0; i < depth; i++) g_string_append(out, "  ");
            }
            GString *shown = line_of(role, name, text, val, states, 1);
            g_string_append(out, shown->str);
            g_string_free(shown, TRUE);
            if (b) g_string_append_printf(out, " @%d,%d %dx%d", b[0] + layer_ox, b[1] + layer_oy, b[2], b[3]);
            g_string_append_c(out, '\n');
        }
    }
    g_string_free(body, TRUE);
}

static void walk(AtspiAccessible *a, int depth, const char *parent_name) {
    if (nodes >= BUDGET) { truncated = 1; return; }
    nodes++;
    char *role = role_of(a), *name = name_of(a, FULL);
    AtspiStateSet *ss = atspi_accessible_get_state_set(a);
#define HAS(st) (ss && atspi_state_set_contains(ss, (st)))
    // A popup's offset (extents) holds for what is under it, and no further.
    int ox = off_x, oy = off_y;
    int b[4], hasb = extents(a, depth, role, b) && b[2] > 0 && b[3] > 0;
    char *text = NULL, *val = NULL;
    GPtrArray *states = g_ptr_array_new();
    gint n = 0;
    int printed = 0, skip = 0;

    if (ss && !HAS(ATSPI_STATE_SHOWING) && !HAS(ATSPI_STATE_VISIBLE) && depth > 0 && !opt_all) skip = 1;
    // Wholly outside the window (scrolled out of a long page): a shot would not show it either.
    else if (depth > 0 && hasb && win_w > 0 && !opt_all &&
             (b[0] >= win_w || b[1] >= win_h || b[0] + b[2] <= 0 || b[1] + b[3] <= 0)) {
        skip = 1;
    }
    if (skip) goto done;

    val = value_of(a);
    if (!val && strcmp(role, "combo box") == 0) val = combo_choice(a);
    if (val && strcmp(val, name) == 0) g_clear_pointer(&val, g_free);
    text = text_of(a, role);
    if (strcmp(text, name) == 0) *text = 0;
    for (size_t i = 0; i < G_N_ELEMENTS(SHOWN); i++)
        if (HAS(SHOWN[i].st)) g_ptr_array_add(states, (gpointer)SHOWN[i].word);
    if (HAS(ATSPI_STATE_CHECKABLE) && !HAS(ATSPI_STATE_CHECKED) && in(role, CHECKABLE))
        g_ptr_array_add(states, "unchecked");
    if (ss && !HAS(ATSPI_STATE_SENSITIVE) && !HAS(ATSPI_STATE_ENABLED)) g_ptr_array_add(states, "disabled");
    n = atspi_accessible_get_child_count(a, NULL);
    if (n < 0) n = 0;

    if (!opt_all && !*name && !*text && n == 1 &&
        (!strcmp(role, "list item") || !strcmp(role, "table cell") || !strcmp(role, "menu item"))) {
        // An unnamed row whose one child is its label: named after it.
        AtspiAccessible *ch = atspi_accessible_get_child_at_index(a, 0, NULL);
        if (ch) {
            char *cr = role_of(ch);
            if (!strcmp(cr, "label") && atspi_accessible_get_child_count(ch, NULL) == 0) {
                g_free(name);
                name = name_of(ch, FULL);
                n = 0;
            }
            g_free(cr);
            g_object_unref(ch);
        }
    }
    int anon = !*name && !val && !*text && states->len == 0;
    printed = opt_all || depth == 0 || !(anon && in(role, CONTAINERS));
    if (printed && !opt_all && depth > 0 && *name && !strcmp(name, parent_name) && !val && !*text &&
        states->len == 0 && n)
        printed = 0; // a wrapper repeating its parent's name (an icon's section, a button's)
    if (!opt_all && (!strcmp(role, "label") || !strcmp(role, "static")) && *name && !n) {
        // A button's own label, a paragraph's runs of text: the parent already says it.
        char *bare = g_strdup(name);
        size_t l = strlen(bare);
        if (l >= 3 && !strcmp(bare + l - 3, "…")) bare[l - 3] = 0;
        int dup = !strcmp(name, parent_name) || strstr(parent_name, bare) != NULL;
        g_free(bare);
        if (dup && *parent_name) goto done;
    }

    int pushed = 0;
    if (printed) {
        emit(path_n, role, name, text, val, states, hasb ? b : NULL);
        // Popped below only when pushed here: past MAX_DEPTH a pop would free an ancestor's slot.
        if (path_n < MAX_DEPTH) {
            char *shown = cut(name, NAME_W);
            path[path_n] = *name ? g_strdup_printf("%s \"%s\"", role, shown) : g_strdup(role);
            path_full[path_n++] = *name ? g_strdup_printf("%s \"%s\"", role, name) : g_strdup(role);
            g_free(shown);
            pushed = 1;
        }
    }
    for (int i = 0; i < MIN(n, MAX_CHILDREN) && nodes < BUDGET; i++) {
        AtspiAccessible *ch = atspi_accessible_get_child_at_index(a, i, NULL);
        if (!ch) continue;
        walk(ch, depth + 1, printed ? (*name ? name : text) : parent_name);
        g_object_unref(ch);
    }
    if (n > MAX_CHILDREN) truncated = 1;
    if (pushed) {
        path_n--;
        g_free(path[path_n]);
        g_free(path_full[path_n]);
    }
done:
#undef HAS
    off_x = ox, off_y = oy;
    g_ptr_array_free(states, TRUE);
    if (ss) g_object_unref(ss);
    g_free(role);
    g_free(name);
    g_free(text);
    g_free(val);
}

// at-spi2-core 2.61 and later have no org.a11y.Status.ScreenReaderEnabled (its bus launcher's
// IsEnabled rework): then AccessKit before 0.22 never registers, whatever tree does.
static int no_sre;

// The box's a11y bus answers (at-spi-bus-launcher on its session bus); and IsEnabled set true.
// Returns -1 without a bus, else whether IsEnabled or ScreenReaderEnabled was off (and is on now).
static int a11y_bus(void) {
    GError *e = NULL;
    GDBusConnection *c = g_bus_get_sync(G_BUS_TYPE_SESSION, NULL, &e);
    if (!c) {
        fprintf(stderr, "omabox-tree: no session bus (%s)\n", e->message);
        g_clear_error(&e);
        return -1;
    }
    GVariant *r = g_dbus_connection_call_sync(c, "org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus", "GetAddress", NULL,
                                              G_VARIANT_TYPE("(s)"), G_DBUS_CALL_FLAGS_NONE, 5000, NULL, &e);
    if (!r) {
        fprintf(stderr, "omabox-tree: the box has no accessibility bus (%s)\n", e->message);
        g_clear_error(&e);
        g_object_unref(c);
        return -1;
    }
    const char *addr;
    g_variant_get(r, "(&s)", &addr);
    int none = !*addr;
    g_variant_unref(r);
    if (none) {
        fprintf(stderr, "omabox-tree: the box has no accessibility bus (an empty address)\n");
        g_object_unref(c);
        return -1;
    }
    // IsEnabled for Qt (and AccessKit from 0.22 on); ScreenReaderEnabled for AccessKit before 0.22
    // (eframe 0.36 and older egui apps), which watches only that (#237).
    static const char *const PROPS[] = {"IsEnabled", "ScreenReaderEnabled"};
    static const char *const WHO[] = {"Qt apps", "older egui/winit (AccessKit) apps"};
    int was_off = 0;
    for (size_t k = 0; k < G_N_ELEMENTS(PROPS); k++) {
        int off = 1;
        r = g_dbus_connection_call_sync(c, "org.a11y.Bus", "/org/a11y/bus", "org.freedesktop.DBus.Properties", "Get",
                                        g_variant_new("(ss)", "org.a11y.Status", PROPS[k]), G_VARIANT_TYPE("(v)"),
                                        G_DBUS_CALL_FLAGS_NONE, 5000, NULL, &e);
        // A property the launcher does not have (GDBus answers InvalidArgs "No such property"): not one
        // to set. Any other error: try to set it, and say so if that fails too.
        if (!r && k == 1 && g_error_matches(e, G_DBUS_ERROR, G_DBUS_ERROR_INVALID_ARGS) && strstr(e->message, "No such property")) {
            g_clear_error(&e);
            no_sre = 1;
            continue;
        }
        g_clear_error(&e);
        if (r) {
            GVariant *v;
            g_variant_get(r, "(v)", &v);
            if (g_variant_is_of_type(v, G_VARIANT_TYPE_BOOLEAN) && g_variant_get_boolean(v)) off = 0;
            g_variant_unref(v);
            g_variant_unref(r);
        }
        if (!off) continue;
        r = g_dbus_connection_call_sync(c, "org.a11y.Bus", "/org/a11y/bus", "org.freedesktop.DBus.Properties", "Set",
                                        g_variant_new("(ssv)", "org.a11y.Status", PROPS[k], g_variant_new_boolean(TRUE)),
                                        NULL, G_DBUS_CALL_FLAGS_NONE, 5000, NULL, &e);
        if (!r) {
            fprintf(stderr, "omabox-tree: could not set org.a11y.Status.%s (%s): %s will have no tree\n", PROPS[k], e->message, WHO[k]);
            g_clear_error(&e);
        } else {
            g_variant_unref(r);
            was_off = 1;
        }
    }
    g_object_unref(c);
    return was_off;
}

// PID's applications on the a11y bus: those with something under them (Qt leaves the application it
// registered before IsEnabled, empty, beside the one it registers after). *any: one with PID at all.
static GPtrArray *apps_of(guint pid, int *any, char **toolkit) {
    GPtrArray *found = g_ptr_array_new_with_free_func(g_object_unref);
    AtspiAccessible *desk = atspi_get_desktop(0);
    gint n = atspi_accessible_get_child_count(desk, NULL);
    for (int i = 0; i < n; i++) {
        AtspiAccessible *app = atspi_accessible_get_child_at_index(desk, i, NULL);
        if (!app) continue;
        GError *e = NULL;
        guint p = atspi_accessible_get_process_id(app, &e);
        if (!e && p == pid) {
            *any = 1;
            if (toolkit && !*toolkit) *toolkit = atspi_accessible_get_toolkit_name(app, NULL);
            if (atspi_accessible_get_child_count(app, NULL) > 0) { g_ptr_array_add(found, app); app = NULL; }
        }
        g_clear_error(&e);
        if (app) g_object_unref(app);
    }
    g_object_unref(desk);
    return found;
}

// libatspi's "AT-SPI: Error in GetItems ... Unknown object '/org/a11y/atspi/cache'" on every read of an
// app without the cache object (AccessKit before 0.22), whose tree reads fine without it: dropped. Any
// other dbind message passes on. Its log domain is "atspi" from at-spi2-core 2.61, "dbind" before.
static void dbind_log(const gchar *d, GLogLevelFlags l, const gchar *m, gpointer u) {
    if (m && strstr(m, "Unknown object '/org/a11y/atspi/cache'")) return;
    g_log_default_handler(d, l, m, u);
}

// What libatspi, GLib and GDBus log (warnings about one app's objects, while the window read fine)
// is held here and printed only when the run fails: on a tree that was read it was a stray line in
// omabox tree's output (finding 349). An error (it aborts) goes out at once.
struct held_log { GLogLevelFlags level; gchar *domain, *message; };
static GArray *glog;
static GLogWriterOutput glog_writer(GLogLevelFlags l, const GLogField *f, gsize n, gpointer u) {
    (void)u;
    if (l & G_LOG_LEVEL_ERROR) return g_log_writer_standard_streams(l, f, n, NULL);
    const char *domain = NULL, *message = NULL;
    for (gsize i = 0; i < n; i++) {
        if (!strcmp(f[i].key, "GLIB_DOMAIN")) domain = f[i].value;
        else if (!strcmp(f[i].key, "MESSAGE")) message = f[i].value;
    }
    if (g_log_writer_default_would_drop(l, domain)) return G_LOG_WRITER_HANDLED;   // debug, unless G_MESSAGES_DEBUG
    if (!glog) glog = g_array_new(FALSE, FALSE, sizeof(struct held_log));
    struct held_log h = {l, g_strdup(domain), g_strdup(message)};
    g_array_append_val(glog, h);
    return G_LOG_WRITER_HANDLED;
}
// The held messages, as GLib would have printed them.
static void glog_flush(void) {
    for (guint i = 0; glog && i < glog->len; i++) {
        struct held_log *h = &g_array_index(glog, struct held_log, i);
        GLogField f[2] = {{"MESSAGE", h->message ? h->message : "", -1}, {"GLIB_DOMAIN", h->domain, -1}};
        g_log_writer_standard_streams(h->level, f, h->domain ? 2 : 1, NULL);
    }
}

static double now(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec + ts.tv_nsec / 1e9;
}

// A frame's size, for --layer and the only frame of a window's application.
static int frame_size(AtspiAccessible *f, int *w, int *h) {
    int b[4];
    if (!extents_in(f, ATSPI_COORD_TYPE_WINDOW, b)) return 0;
    *w = b[2], *h = b[3];
    return 1;
}

// The window: the frame named TITLE; else one whose name and TITLE hold each other; else the only one,
// when it is the window's size (2 px either way, for rounding) and no other window of the app is.
// Into *win, else says why and returns the exit code.
static int window_frame(GPtrArray *apps, const char *title, AtspiAccessible **win_out) {
    AtspiAccessible *win = NULL, *loose = NULL;
    int frames = 0, loose_n = 0;
    GString *names = g_string_new(NULL);
    for (guint i = 0; i < apps->len && !win; i++) {
        AtspiAccessible *app = apps->pdata[i];
        gint n = atspi_accessible_get_child_count(app, NULL);
        for (int j = 0; j < MIN(n, 200) && !win; j++) {
            AtspiAccessible *f = atspi_accessible_get_child_at_index(app, j, NULL);
            if (!f) continue;
            char *raw = atspi_accessible_get_name(f, NULL), *nm = raw ? raw : g_strdup("");
            frames++;
            g_string_append_printf(names, "%s\"%s\"", names->len ? ", " : "", nm);
            if (title && !strcmp(nm, title)) win = g_object_ref(f);
            else if (title && *nm && (strstr(title, nm) || strstr(nm, title))) {
                if (!loose) loose = g_object_ref(f);
                loose_n++;
            }
            if (!loose && !title) loose = g_object_ref(f), loose_n = 1;
            g_free(nm);
            g_object_unref(f);
        }
    }
    if (!win && loose_n == 1) win = g_object_ref(loose);
    if (!win && frames == 1 && apps->len == 1) {
        AtspiAccessible *f = atspi_accessible_get_child_at_index(apps->pdata[0], 0, NULL);
        int w, h, rc = 0;
        if (f && win_w > 0 && frame_size(f, &w, &h) && (abs(w - win_w) > 2 || abs(h - win_h) > 2)) {
            fprintf(stderr, "no accessibility tree for this window: its application publishes one window, %s %dx%d, and this "
                    "one is %dx%d (its other windows are not on the accessibility bus)\n", names->str, w, h, win_w, win_h);
            rc = 7;
        } else if (f && twins > 0) {
            fprintf(stderr, "its application publishes one window, %s, and %d of its windows are this one's size, %dx%d: "
                    "which one it is cannot be told\n", names->str, twins + 1, win_w, win_h);
            rc = 6;
        }
        if (rc) {
            if (loose) g_object_unref(loose);
            g_object_unref(f);
            g_string_free(names, TRUE);
            return rc;
        }
        win = f;
    }
    if (loose) g_object_unref(loose);
    if (!win) {
        fprintf(stderr, "no accessibility tree for the window \"%s\": its application's windows are %s\n", title ? title : "",
                names->str);
        g_string_free(names, TRUE);
        return 3;
    }
    g_string_free(names, TRUE);
    *win_out = win;
    return 0;
}

// Whether a frame is active (has the keyboard), for --layer.
static int is_active(AtspiAccessible *f) {
    AtspiStateSet *ss = atspi_accessible_get_state_set(f);
    int r = ss && atspi_state_set_contains(ss, ATSPI_STATE_ACTIVE);
    if (ss) g_object_unref(ss);
    return r;
}

// --layer: the frame of APPS that is the layer's size (win_w x win_h) into *win, else says why and
// returns the exit code. Its frame with nothing under it is said apart: that is a layer whose items
// set no Accessible role, not a build that publishes no tree (Quickshell before 0.3.2: no frame).
static int layer_frame(GPtrArray *apps, long pid, AtspiAccessible **win) {
    GPtrArray *same = g_ptr_array_new_with_free_func(g_object_unref);
    GString *sizes = g_string_new(NULL);
    for (guint i = 0; i < apps->len; i++) {
        AtspiAccessible *app = apps->pdata[i];
        gint n = atspi_accessible_get_child_count(app, NULL);
        for (int j = 0; j < MIN(n, 200); j++) {
            AtspiAccessible *f = atspi_accessible_get_child_at_index(app, j, NULL);
            if (!f) continue;
            int w, h;
            if (frame_size(f, &w, &h)) {
                g_string_append_printf(sizes, "%s%dx%d", sizes->len ? ", " : "", w, h);
                if (w == win_w && h == win_h) { g_ptr_array_add(same, f); f = NULL; }
            }
            if (f) g_object_unref(f);
        }
    }
    if (same->len > 1 && active_hint >= 0) {
        int act = 0, at = -1;
        for (guint i = 0; i < same->len; i++)
            if (is_active(same->pdata[i])) act++, at = (int)i;
        if (act == 1) {
            if (active_hint) {
                AtspiAccessible *f = g_object_ref(same->pdata[at]);
                g_ptr_array_set_size(same, 0);
                g_ptr_array_add(same, f);
            } else {
                g_ptr_array_remove_index(same, at);
            }
        }
    }
    int rc = 0;
    if (same->len == 0) {
        fprintf(stderr, "no accessibility tree for the layer: none of its application's (pid %ld) windows is %dx%d (they are %s)\n",
                pid, win_w, win_h, sizes->len ? sizes->str : "none");
        rc = 3;
    } else if (same->len > 1) {
        fprintf(stderr, "%u of its application's (pid %ld) windows are %dx%d, and they have no names: which one is the layer's cannot be told\n",
                same->len, pid, win_w, win_h);
        rc = 6;
    } else if (atspi_accessible_get_child_count(same->pdata[0], NULL) <= 0) {
        fprintf(stderr, "the layer publishes no accessible items (nothing in it sets an Accessible role)\n");
        rc = 5;
    } else {
        *win = g_object_ref(same->pdata[0]);
    }
    g_ptr_array_unref(same);
    g_string_free(sizes, TRUE);
    return rc;
}

static int tree_main(int argc, char **argv);
int main(int argc, char **argv) {
    g_log_set_writer_func(glog_writer, NULL, NULL);
    int rc = tree_main(argc, argv);
    if (rc) glog_flush();
    return rc;
}

static int tree_main(int argc, char **argv) {
    if (!omabox_inside_box()) {
        fprintf(stderr, "omabox-tree: only runs inside an omabox box\n");
        return 2;
    }
    long pid = -1;
    const char *title = NULL;
    for (int i = 1; i < argc; i++) {
        const char *a = argv[i], *v = i + 1 < argc ? argv[i + 1] : NULL;
        if (!strcmp(a, "--all")) opt_all = 1;
        else if (!strcmp(a, "--json")) opt_json = 1;
        else if (!strcmp(a, "--pid") && v) pid = strtol(argv[++i], NULL, 10);
        else if (!strcmp(a, "--title") && v) title = argv[++i];
        else if (!strcmp(a, "--find") && v) find = argv[++i];
        else if (!strcmp(a, "--twins") && v) twins = atoi(argv[++i]);
        else if (!strcmp(a, "--size") && v) {
            if (sscanf(argv[++i], "%dx%d", &win_w, &win_h) != 2) win_w = win_h = -1;
        } else if (!strcmp(a, "--layer")) opt_layer = 1;
        else if (!strcmp(a, "--offset") && v) {
            if (sscanf(argv[++i], "%d,%d", &layer_ox, &layer_oy) != 2) layer_ox = layer_oy = 0;
        } else if (!strcmp(a, "--active") && v) {
            v = argv[++i];
            active_hint = !strcmp(v, "yes") ? 1 : !strcmp(v, "no") ? 0 : -1;
        } else {
            fprintf(stderr, "usage: omabox-tree --pid PID [--title TITLE | --layer] [--size WxH] [--twins N] [--offset X,Y] [--active yes|no] "
                            "[--find TEXT] [--json] [--all]\n");
            return 2;
        }
    }
    if (opt_layer && win_w <= 0) {
        fprintf(stderr, "omabox-tree: --layer needs the layer's --size WxH\n");
        return 2;
    }
    if (pid <= 0) {
        fprintf(stderr, "omabox-tree: --pid PID is needed\n");
        return 2;
    }
    if (find && !*find) find = NULL;
    g_log_set_handler("dbind", G_LOG_LEVEL_WARNING, dbind_log, NULL);
    g_log_set_handler("atspi", G_LOG_LEVEL_WARNING, dbind_log, NULL);

    int was_off = a11y_bus();
    if (was_off < 0) return 4;
    if (atspi_init() < 0) {
        fprintf(stderr, "omabox-tree: could not reach the accessibility bus\n");
        return 4;
    }
    atspi_set_timeout(2000, 4000);

    int any = 0;
    char *toolkit = NULL;
    GPtrArray *apps = apps_of(pid, &any, &toolkit);
    // Turned on just now: a Qt app registers again (with its tree) a moment later, an AccessKit one for
    // the first time.
    for (double end = now() + 3; was_off && apps->len == 0 && now() < end;) {
        g_ptr_array_unref(apps);
        for (double t = now() + 0.1; now() < t;) {
            if (!g_main_context_iteration(NULL, FALSE)) g_usleep(10000);
        }
        apps = apps_of(pid, &any, toolkit ? NULL : &toolkit);
    }
    if (apps->len == 0) {
        if (any) fprintf(stderr, "no accessibility tree: its application (pid %ld) is on the bus with no window under it\n", pid);
        else if (no_sre) fprintf(stderr, "no accessibility tree: pid %ld is not on the box's accessibility bus (an egui/winit app on "
                                 "AccessKit before accesskit_unix 0.22 never joins it here: it waits for ScreenReaderEnabled, which "
                                 "at-spi2-core 2.61 and later do not have)\n", pid);
        else fprintf(stderr, "no accessibility tree: pid %ld is not on the box's accessibility bus\n", pid);
        return 3;
    }

    AtspiAccessible *win = NULL;
    int rc = opt_layer ? layer_frame(apps, pid, &win) : window_frame(apps, title, &win);
    if (rc) return rc;

    out = g_string_new(NULL);
    walk(win, 0, "");
    if (opt_json) g_string_append(out, out->len ? "]\n" : "[]\n");
    fputs(out->str, stdout);
    // Chromium and Electron export the page only when started with --force-renderer-accessibility.
    if (toolkit && !strcasecmp(toolkit, "Chromium") && lines <= 3)
        fprintf(stderr, "omabox-tree: a Chromium or Electron window shows its page in the tree only when "
                        "started with --force-renderer-accessibility\n");
    if (truncated) fprintf(stderr, "omabox-tree: stopped at %d nodes (or %d children of one)\n", BUDGET, MAX_CHILDREN);
    if (find && !matched) {
        fprintf(stderr, "omabox-tree: nothing in the %s holds \"%s\" (%d lines)\n", opt_layer ? "layer" : "window", find, lines);
        return 1;
    }
    return 0;
}
