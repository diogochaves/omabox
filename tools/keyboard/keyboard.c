// omabox-keyboard: type on the Wayland display in $WAYLAND_DISPLAY through a virtual keyboard.
//
//   omabox-keyboard [--layout us] [--variant V] [--model M] [--options O] [--delay MS]  COMBO | -t TEXT | -s MS ...
//   omabox-keyboard [--layout ...] --hold
//
// COMBO is super+space, ctrl+shift+t, Return, a, F5, ctrl++, super+/ ... (modifiers: super ctrl shift
// alt altgr; a key is a keysym name or a single character). With modifiers a letter is the key, not
// the shifted symbol: SUPER+W is super+w, as in Hyprland's binds; shift+w for SUPER+SHIFT+W. On its own
// an uppercase letter types it (A is shift+a). -t types TEXT; -s sleeps.
// Tokens run left to right in one connection, all checked before it connects. Exit 1 if the
// compositor goes away mid-run.
// --hold keeps an idle keyboard on the seat until the compositor goes away (NOTES finding 41).
//
// Why not wtype: wtype uploads a keymap of its own and numbers keys from keycode 9. Hyprland replaces
// a virtual keyboard's keymap with its configured layout, so those keycodes are read as other keys
// (the first one as Escape). This tool uses real evdev keycodes looked up in the same XKB layout the
// compositor applies, so binds and clients see what a physical keyboard would send. A character the
// layout lacks (Ü on us) is bound to a spare keycode in the keymap this keyboard uploads (finding 55).
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>
#include <wayland-client.h>
#include <xkbcommon/xkbcommon.h>

#include "virtual-keyboard-unstable-v1-client-protocol.h"

static struct wl_seat *seat;
static struct zwp_virtual_keyboard_manager_v1 *manager;
static struct wl_display *display;
static struct zwp_virtual_keyboard_v1 *kbd;
static struct xkb_keymap *keymap;
static unsigned delay_ms = 12;

static void global(void *data, struct wl_registry *reg, uint32_t name, const char *iface, uint32_t version) {
    (void)data; (void)version;
    if (!strcmp(iface, wl_seat_interface.name) && !seat)
        seat = wl_registry_bind(reg, name, &wl_seat_interface, 1);
    else if (!strcmp(iface, zwp_virtual_keyboard_manager_v1_interface.name))
        manager = wl_registry_bind(reg, name, &zwp_virtual_keyboard_manager_v1_interface, 1);
}
static void global_remove(void *data, struct wl_registry *reg, uint32_t name) { (void)data; (void)reg; (void)name; }
static const struct wl_registry_listener registry_listener = {global, global_remove};

static uint32_t now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint32_t)(ts.tv_sec * 1000 + ts.tv_nsec / 1000000);
}
static void sleep_ms(long ms) {
    struct timespec ts = {ms / 1000, (ms % 1000) * 1000000L};
    nanosleep(&ts, NULL);
}

static void usage(void) {
    fprintf(stderr, "usage: omabox-keyboard [--layout L] [--variant V] [--model M] [--options O] [--delay MS] [--hold | (COMBO | -t TEXT | -s MS)...]\n");
    exit(2);
}

// A whole decimal number in [lo, hi], or usage(): no junk ("abc" was 0), no negatives wrapped into
// huge sleeps (-s -1 slept 49 days).
static long num(const char *s, long lo, long hi) {
    char *end;
    long v = strtol(s, &end, 10);
    if (!*s || *end || v < lo || v > hi) { fprintf(stderr, "omabox-keyboard: bad number '%s'\n", s); usage(); }
    return v;
}

// A lost compositor (box gone, protocol error) is a failure, not "keys sent" (finding 64).
static void sync_or_die(void) {
    if (wl_display_roundtrip(display) < 0) { fprintf(stderr, "omabox-keyboard: lost the compositor\n"); exit(1); }
}

// A keysym's first (keycode, modifiers) in layout 0 of the keymap, lowest level first. The modifiers
// that reach a level come from the key's type: Shift for level 2 on most keys, AltGr (Mod5) for levels
// 3 and 4 on layouts that have them. Levels needing a modifier this tool cannot press are skipped.
// Keycodes up to 255 first: X11 keycodes are 8 bits, so Xwayland cannot give an X client a key above
// that (evdev keymaps run to 709; the us layout has € only on keycode 443).
struct hit { xkb_keycode_t code; xkb_mod_mask_t mask; };
static xkb_mod_mask_t pressable;
#define X11_MAX_KEYCODE 255
static int find_keysym_in(xkb_keysym_t sym, struct hit *out, xkb_keycode_t min, xkb_keycode_t max) {
    for (xkb_level_index_t level = 0; level < 4; level++)
        for (xkb_keycode_t kc = min; kc <= max; kc++) {
            if (level >= xkb_keymap_num_levels_for_key(keymap, kc, 0)) continue;
            const xkb_keysym_t *syms;
            int n = xkb_keymap_key_get_syms_by_level(keymap, kc, 0, level, &syms);
            if (n != 1 || syms[0] != sym) continue;
            xkb_mod_mask_t masks[8];
            size_t nm = xkb_keymap_key_get_mods_for_level(keymap, kc, 0, level, masks, 8);
            for (size_t i = 0; i < nm; i++)
                if (!(masks[i] & ~pressable)) { out->code = kc; out->mask = masks[i]; return 1; }
        }
    return 0;
}
static int find_keysym_x11(xkb_keysym_t sym, struct hit *out) {
    xkb_keycode_t min = xkb_keymap_min_keycode(keymap), max = xkb_keymap_max_keycode(keymap);
    return find_keysym_in(sym, out, min, max < X11_MAX_KEYCODE ? max : X11_MAX_KEYCODE);
}
static int find_keysym(xkb_keysym_t sym, struct hit *out) {
    return find_keysym_x11(sym, out) ||
           find_keysym_in(sym, out, X11_MAX_KEYCODE + 1, xkb_keymap_max_keycode(keymap));
}

struct mod { const char *names[4]; const char *xkb; const char *keysym; };
static const struct mod mods[] = {
    {{"super", "logo", "win", "mod"}, XKB_MOD_NAME_LOGO, "Super_L"},   // mod: Hyprland's name for SUPER
    {{"ctrl", "control"}, XKB_MOD_NAME_CTRL, "Control_L"},
    {{"shift"}, XKB_MOD_NAME_SHIFT, "Shift_L"},
    {{"alt"}, XKB_MOD_NAME_ALT, "Alt_L"},
    {{"altgr"}, "Mod5", "ISO_Level3_Shift"},
};
#define NMODS (sizeof(mods) / sizeof(mods[0]))

static int mod_index(const char *name) {
    for (size_t m = 0; m < NMODS; m++)
        for (int i = 0; i < 4 && mods[m].names[i]; i++)
            if (!strcasecmp(name, mods[m].names[i])) return (int)m;
    return -1;
}
static uint32_t mod_mask(int m) { return 1u << xkb_keymap_mod_get_index(keymap, mods[m].xkb); }
static unsigned held_for(xkb_mod_mask_t mask) {
    unsigned held = 0;
    for (size_t m = 0; m < NMODS; m++) if (mask & mod_mask((int)m)) held |= 1u << m;
    return held;
}

// evdev codes are XKB keycodes minus 8.
static void key(xkb_keycode_t code, int down) {
    zwp_virtual_keyboard_v1_key(kbd, now_ms(), code - 8, down ? WL_KEYBOARD_KEY_STATE_PRESSED : WL_KEYBOARD_KEY_STATE_RELEASED);
}
static void set_mods(uint32_t depressed) { zwp_virtual_keyboard_v1_modifiers(kbd, depressed, 0, 0, 0); }

// Press the modifiers in `held` (bitmask over mods[]), tap `code`, release in reverse. With run = 0 it
// only checks that every modifier has a key in the layout.
static int chord(unsigned held, xkb_keycode_t code, int run) {
    uint32_t mask = 0;
    struct hit mk[NMODS] = {0};
    for (size_t m = 0; m < NMODS; m++)
        if ((held & (1u << m)) && !find_keysym(xkb_keysym_from_name(mods[m].keysym, 0), &mk[m])) {
            fprintf(stderr, "omabox-keyboard: no key for modifier %s in this layout\n", mods[m].names[0]);
            return 0;
        }
    if (!run) return 1;
    for (size_t m = 0; m < NMODS; m++) {
        if (!(held & (1u << m))) continue;
        key(mk[m].code, 1);
        set_mods(mask |= mod_mask((int)m));
    }
    key(code, 1);
    sync_or_die();
    key(code, 0);
    for (int m = (int)NMODS - 1; m >= 0; m--) {
        if (!(held & (1u << m))) continue;
        key(mk[m].code, 0);
        set_mods(mask &= ~mod_mask(m));
    }
    sync_or_die();
    sleep_ms(delay_ms);
    return 1;
}

static const struct { const char *alias, *name; } aliases[] = {
    {"enter", "Return"}, {"esc", "Escape"}, {"del", "Delete"}, {"pgup", "Prior"}, {"pageup", "Prior"},
    {"pgdn", "Next"}, {"pagedown", "Next"}, {"backspace", "BackSpace"}, {"bs", "BackSpace"},
    // a modifier pressed on its own
    {"shift", "Shift_L"}, {"ctrl", "Control_L"}, {"control", "Control_L"}, {"alt", "Alt_L"},
    {"super", "Super_L"}, {"logo", "Super_L"}, {"win", "Super_L"}, {"mod", "Super_L"}, {"altgr", "ISO_Level3_Shift"},
};

// One UTF-8 character, strictly: no stray continuation bytes, overlongs, surrogates or values past
// U+10FFFF (Latin-1 bytes are "bad UTF-8", not garbage code points).
static int decode(const unsigned char **pp, uint32_t *cp) {
    const unsigned char *p = *pp;
    int len;
    uint32_t min;
    if (*p < 0x80) { *cp = *p; len = 1; min = 0; }
    else if (*p >= 0xC2 && *p <= 0xDF) { *cp = *p & 0x1f; len = 2; min = 0x80; }
    else if ((*p >> 4) == 14) { *cp = *p & 0x0f; len = 3; min = 0x800; }
    else if (*p >= 0xF0 && *p <= 0xF4) { *cp = *p & 0x07; len = 4; min = 0x10000; }
    else return 0;
    for (int i = 1; i < len; i++) {
        if ((p[i] & 0xC0) != 0x80) return 0;
        *cp = (*cp << 6) | (p[i] & 0x3f);
    }
    if (*cp < min || *cp > 0x10FFFF || (*cp >= 0xD800 && *cp <= 0xDFFF)) return 0;
    *pp = p + len;
    return 1;
}

static int do_combo(const char *tok, int run) {
    char buf[256];
    if (snprintf(buf, sizeof(buf), "%s", tok) >= (int)sizeof(buf)) { fprintf(stderr, "omabox-keyboard: token too long\n"); return 0; }
    unsigned held = 0;
    char *keyname = buf, *plus;
    while ((plus = strchr(keyname, '+')) && plus[1]) {  // "ctrl++" means ctrl and '+'
        *plus = 0;
        int m = mod_index(keyname);
        if (m < 0) { fprintf(stderr, "omabox-keyboard: unknown modifier '%s' in %s\n", keyname, tok); return 0; }
        held |= 1u << m;
        keyname = plus + 1;
    }
    for (size_t i = 0; i < sizeof(aliases) / sizeof(aliases[0]); i++)
        if (!strcasecmp(keyname, aliases[i].alias)) keyname = (char *)aliases[i].name;
    xkb_keysym_t sym = xkb_keysym_from_name(keyname, XKB_KEYSYM_NO_FLAGS);
    if (sym == XKB_KEY_NoSymbol) sym = xkb_keysym_from_name(keyname, XKB_KEYSYM_CASE_INSENSITIVE);
    // A single character is its own keysym: + - / are not keysym names (plus, minus, slash are).
    const unsigned char *p = (const unsigned char *)keyname;
    uint32_t cp;
    if (sym == XKB_KEY_NoSymbol && decode(&p, &cp) && !*p) sym = xkb_utf32_to_keysym(cp);
    // With modifiers a letter names the key, as Hyprland's binds do (SUPER+W is not SUPER+SHIFT+W).
    if (held) sym = xkb_keysym_to_lower(sym);
    struct hit h;
    if (sym == XKB_KEY_NoSymbol || !find_keysym(sym, &h)) {
        fprintf(stderr, "omabox-keyboard: no key for '%s' in this layout\n", keyname);
        return 0;
    }
    return chord(held | held_for(h.mask), h.code, run); // A, question, ...: the shifted symbol
}

// Characters in -t TEXT that the layout cannot produce get a keycode of their own: one the layout
// leaves empty, bound to that keysym in an extra `key` line of the symbols section. The keyboard then
// uploads that keymap, and a client decodes the spare keycode as the character.
static int text_keysym(uint32_t cp) { return cp == '\n' ? XKB_KEY_Return : cp == '\t' ? XKB_KEY_Tab : (int)xkb_utf32_to_keysym(cp); }
// A character on no key up to 255 gets a spare there, and spares up to 255 come first (X11, above);
// only when those run out, higher ones.
#define MAXWANT 128
static struct xkb_keymap *with_spares(struct xkb_context *ctx, int argc, char **argv, int first) {
    xkb_keysym_t want[MAXWANT];
    size_t nwant = 0, dropped = 0;
    for (int a = first; a < argc; a++) {
        if (strcmp(argv[a], "-t") || a + 1 >= argc) continue;
        const unsigned char *p = (const unsigned char *)argv[++a];
        uint32_t cp;
        while (*p && decode(&p, &cp)) {
            xkb_keysym_t sym = (xkb_keysym_t)text_keysym(cp);
            struct hit h;
            if (sym == XKB_KEY_NoSymbol || find_keysym_x11(sym, &h)) continue;
            size_t k = 0;
            while (k < nwant && want[k] != sym) k++;
            if (k == nwant) { if (nwant < MAXWANT) want[nwant++] = sym; else dropped++; }
        }
    }
    if (dropped) fprintf(stderr, "omabox-keyboard: more than %d characters outside the layout; the rest cannot be typed\n", MAXWANT);
    if (!nwant) return NULL;
    char *text = xkb_keymap_get_as_string(keymap, XKB_KEYMAP_FORMAT_TEXT_V1);
    char *sec = text ? strstr(text, "xkb_symbols") : NULL, *end = sec ? strstr(sec, "\n};") : NULL;
    if (!end) { free(text); return NULL; }
    size_t used = (size_t)(end - text), cap = strlen(text) + nwant * 128 + 1;
    char *out = malloc(cap);
    if (!out) { perror("omabox-keyboard: malloc"); exit(1); }
    memcpy(out, text, used);
    size_t k = 0, high = 0;
    xkb_keycode_t min = xkb_keymap_min_keycode(keymap), max = xkb_keymap_max_keycode(keymap);
    for (int pass = 0; pass < 2; pass++)
        for (xkb_keycode_t kc = pass ? X11_MAX_KEYCODE + 1 : min; kc <= (pass ? max : (max < X11_MAX_KEYCODE ? max : X11_MAX_KEYCODE)) && k < nwant; kc++) {
            const char *kname = xkb_keymap_key_get_name(keymap, kc);
            if (!kname || xkb_keymap_num_layouts_for_key(keymap, kc) != 0) continue;
            struct hit h;   // out of low spares, a character the layout has above 255 keeps its key
            while (pass && k < nwant && find_keysym(want[k], &h)) { k++; high++; }
            if (k == nwant) break;
            char sname[64];
            if (xkb_keysym_get_name(want[k], sname, sizeof(sname)) <= 0) { k++; continue; }
            int n = snprintf(out + used, cap - used, "\n\tkey <%s> { [ %s ] };", kname, sname);
            if (n < 0 || (size_t)n >= cap - used) break;
            used += (size_t)n;
            k++;
            high += pass;
        }
    snprintf(out + used, cap - used, "%s", end);
    free(text);
    struct xkb_keymap *km = xkb_keymap_new_from_string(ctx, out, XKB_KEYMAP_FORMAT_TEXT_V1, XKB_KEYMAP_COMPILE_NO_FLAGS);
    free(out);
    if (high) fprintf(stderr, "omabox-keyboard: %zu character(s) on keycodes above 255: X11 (Xwayland) apps will not see them\n", high);
    if (k < nwant) fprintf(stderr, "omabox-keyboard: no spare keycode for %zu character(s)\n", nwant - k);
    return km;
}

static int do_text(const char *s, int run) {
    const unsigned char *p = (const unsigned char *)s;
    uint32_t cp;
    while (*p) {
        if (!decode(&p, &cp)) { fprintf(stderr, "omabox-keyboard: bad UTF-8 in text\n"); return 0; }
        struct hit h;
        if (!find_keysym((xkb_keysym_t)text_keysym(cp), &h)) {
            fprintf(stderr, "omabox-keyboard: cannot type U+%04X with this layout (no key and no spare keycode)\n", cp);
            return 0;
        }
        if (!chord(held_for(h.mask), h.code, run)) return 0;
    }
    return 1;
}

// One pass over the tokens: with run = 0 it only checks them (before connecting, so a bad token never
// leaves half a sequence typed); with run = 1 it sends them.
static int tokens(int argc, char **argv, int i, int run) {
    for (; i < argc; i++) {
        if (!strcmp(argv[i], "-t")) {
            if (i + 1 >= argc) usage();
            if (!do_text(argv[++i], run)) return 0;
        } else if (!strcmp(argv[i], "-s")) {
            if (i + 1 >= argc) usage();
            long ms = num(argv[++i], 0, 600000);
            if (run) { sync_or_die(); sleep_ms(ms); }
        } else if (!do_combo(argv[i], run)) return 0;
    }
    return 1;
}

int main(int argc, char **argv) {
    // Only ever inside a box (omabox runs it there, in the box's mount namespace): run from a host
    // shell, WAYLAND_DISPLAY is the user's real desktop (it happened once: a parse check clicked it).
    if (access("/opt/omabox/share", F_OK) != 0) {
        fprintf(stderr, "%s: only runs inside an omabox box (use omabox keys/click/pointer)\n", argv[0]);
        return 2;
    }
    unsetenv("WAYLAND_SOCKET");   // only the socket in WAYLAND_DISPLAY, never an inherited connection
    // The box's keyboard settings come as options (omabox keys reads them from its Hyprland); the
    // caller's XKB_DEFAULT_* must not change what the keycodes mean.
    struct xkb_rule_names names = {.rules = "evdev", .layout = "us", .variant = "", .model = "", .options = ""};
    int i = 1, hold = 0;
    for (; i < argc; i++) {
        const char **opt = !strcmp(argv[i], "--layout") ? &names.layout : !strcmp(argv[i], "--variant") ? &names.variant
                         : !strcmp(argv[i], "--model") ? &names.model : !strcmp(argv[i], "--options") ? &names.options : NULL;
        if (!strcmp(argv[i], "--hold")) hold = 1;
        else if (opt) { if (i + 1 >= argc) usage(); *opt = argv[++i]; }
        else if (!strcmp(argv[i], "--delay")) { if (i + 1 >= argc) usage(); delay_ms = (unsigned)num(argv[++i], 0, 10000); }
        else break;
    }
    if (hold ? i < argc : i >= argc) usage();

    struct xkb_context *ctx = xkb_context_new(XKB_CONTEXT_NO_ENVIRONMENT_NAMES);
    keymap = ctx ? xkb_keymap_new_from_names(ctx, &names, XKB_KEYMAP_COMPILE_NO_FLAGS) : NULL;
    if (!keymap) { fprintf(stderr, "omabox-keyboard: cannot build keymap for layout '%s'\n", names.layout); return 1; }
    for (size_t m = 0; m < NMODS; m++) pressable |= mod_mask((int)m);
    struct xkb_keymap *spares = hold ? NULL : with_spares(ctx, argc, argv, i);
    if (spares) { xkb_keymap_unref(keymap); keymap = spares; }
    if (!tokens(argc, argv, i, 0)) return 1;

    display = wl_display_connect(NULL);
    if (!display) { fprintf(stderr, "omabox-keyboard: cannot connect to Wayland display\n"); return 1; }
    struct wl_registry *reg = wl_display_get_registry(display);
    wl_registry_add_listener(reg, &registry_listener, NULL);
    sync_or_die();
    if (!seat || !manager) { fprintf(stderr, "omabox-keyboard: compositor lacks wl_seat or zwp_virtual_keyboard_manager_v1\n"); return 1; }

    kbd = zwp_virtual_keyboard_manager_v1_create_virtual_keyboard(manager, seat);
    char *km = xkb_keymap_get_as_string(keymap, XKB_KEYMAP_FORMAT_TEXT_V1);
    if (!km) { fprintf(stderr, "omabox-keyboard: cannot serialise the keymap\n"); return 1; }
    size_t size = strlen(km) + 1;
    int fd = memfd_create("omabox-keymap", MFD_CLOEXEC);
    if (fd < 0 || ftruncate(fd, (off_t)size) < 0) { perror("omabox-keyboard: memfd"); return 1; }
    char *map = mmap(NULL, size, PROT_WRITE, MAP_SHARED, fd, 0);
    if (map == MAP_FAILED) { perror("omabox-keyboard: mmap"); return 1; }
    memcpy(map, km, size);
    munmap(map, size);
    free(km);
    zwp_virtual_keyboard_v1_keymap(kbd, WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1, fd, (uint32_t)size);
    sync_or_die();
    close(fd);
    // A box has no keyboard of its own. With none on the seat, Hyprland drops keyboard focus changes
    // ("setKeyboardFocus without a valid keyboard set"), so a device that comes later finds a shell
    // panel without focus and its keys go nowhere. An idle one held for the box's life prevents it.
    if (hold) { while (wl_display_dispatch(display) != -1) {} return 0; }
    // Hyprland makes a new keyboard the active one on its first event, re-sending keymap and enter to
    // the focused client, and the key that caused the switch is lost. Spend that on an empty
    // modifiers event so the first real key arrives.
    set_mods(0);
    sync_or_die();
    sleep_ms(20);

    int ok = tokens(argc, argv, i, 1);
    sync_or_die();
    sleep_ms(30); // let the compositor handle the last release before the device goes away
    zwp_virtual_keyboard_v1_destroy(kbd);
    sync_or_die();
    wl_display_disconnect(display);
    return ok ? 0 : 1;
}
