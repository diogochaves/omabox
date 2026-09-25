// omabox-pointer: drive a virtual pointer on the Wayland display in $WAYLAND_DISPLAY.
//
//   omabox-pointer --extent WxH  move X Y  click [left|right|middle]  scroll DY  down BTN  up BTN
//   omabox-pointer --hold
//
// Commands run left to right in one connection, all checked before it connects. Coordinates are
// absolute in the compositor's layout, scaled against --extent (the layout size; default 1920x1080).
// It only ever talks to the socket in WAYLAND_DISPLAY (never an inherited WAYLAND_SOCKET), so aimed at
// a sandbox it cannot move the real desktop's cursor. Exit 1 if the compositor goes away mid-run.
// --hold keeps an idle pointer on the seat until the compositor goes away (NOTES finding 41).
#include <linux/input-event-codes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <wayland-client.h>

#include "wlr-virtual-pointer-unstable-v1-client-protocol.h"

static struct wl_seat *seat;
static struct zwlr_virtual_pointer_manager_v1 *manager;

static void global(void *data, struct wl_registry *reg, uint32_t name, const char *iface, uint32_t version) {
    (void)data;
    if (!strcmp(iface, wl_seat_interface.name) && !seat)
        seat = wl_registry_bind(reg, name, &wl_seat_interface, 1);
    else if (!strcmp(iface, zwlr_virtual_pointer_manager_v1_interface.name))
        manager = wl_registry_bind(reg, name, &zwlr_virtual_pointer_manager_v1_interface, version < 2 ? version : 2);
}
static void global_remove(void *data, struct wl_registry *reg, uint32_t name) { (void)data; (void)reg; (void)name; }
static const struct wl_registry_listener registry_listener = {global, global_remove};

static uint32_t now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint32_t)(ts.tv_sec * 1000 + ts.tv_nsec / 1000000);
}

static int is_button(const char *s) { return s && (!strcmp(s, "left") || !strcmp(s, "right") || !strcmp(s, "middle")); }
static uint32_t button_code(const char *name) {
    if (!name || !strcmp(name, "left")) return BTN_LEFT;
    return !strcmp(name, "right") ? BTN_RIGHT : BTN_MIDDLE;   // validated before we connect
}

static void usage(void) {
    fprintf(stderr, "usage: omabox-pointer [--hold] [--extent WxH] (move X Y | click [BTN] | down BTN | up BTN | scroll DY | sleep MS)...\n");
    exit(2);
}

// A whole decimal number in [lo, hi], or usage(): no junk ("1O0"), no negatives wrapped into huge values.
static long num(const char *s, long lo, long hi) {
    char *end;
    if (!s || !*s) usage();
    long v = strtol(s, &end, 10);
    if (*end || v < lo || v > hi) { fprintf(stderr, "omabox-pointer: bad number '%s'\n", s); usage(); }
    return v;
}

static double numd(const char *s, double lo, double hi) {
    char *end;
    if (!s || !*s) usage();
    double v = strtod(s, &end);
    if (*end || !(v >= lo && v <= hi)) { fprintf(stderr, "omabox-pointer: bad number '%s'\n", s); usage(); }
    return v;
}

static struct wl_display *display;
// A lost compositor (box gone, protocol error) is a failure, not "clicked" (finding 63).
static void sync_or_die(void) {
    if (wl_display_roundtrip(display) < 0) { fprintf(stderr, "omabox-pointer: lost the compositor\n"); exit(1); }
}
static void sleep_ms(long ms) {
    struct timespec ts = {ms / 1000, (ms % 1000) * 1000000L};
    nanosleep(&ts, NULL);
}

// One pass over the commands: with run = 0 it only checks them (before connecting, so a bad token
// never leaves half a sequence done, or a button held down); with run = 1 it sends them.
static void commands(struct zwlr_virtual_pointer_v1 *ptr, int argc, char **argv, int i, uint32_t ew, uint32_t eh, int run) {
    for (; i < argc; i++) {
        const char *cmd = argv[i];
        if (!strcmp(cmd, "move") && i + 2 < argc) {
            long x = num(argv[++i], 0, ew - 1), y = num(argv[++i], 0, eh - 1);
            if (!run) continue;
            zwlr_virtual_pointer_v1_motion_absolute(ptr, now_ms(), (uint32_t)x, (uint32_t)y, ew, eh);
            zwlr_virtual_pointer_v1_frame(ptr);
            // Let the client see the hover (enter, motion, a repaint) before any button: a QML button
            // pressed in the same instant as the motion that reached it ignores the click.
            sync_or_die();
            sleep_ms(40);
        } else if (!strcmp(cmd, "click")) {
            const char *b = is_button(i + 1 < argc ? argv[i + 1] : NULL) ? argv[++i] : NULL;
            if (!run) continue;
            uint32_t code = button_code(b);
            zwlr_virtual_pointer_v1_button(ptr, now_ms(), code, WL_POINTER_BUTTON_STATE_PRESSED);
            zwlr_virtual_pointer_v1_frame(ptr);
            sync_or_die();
            sleep_ms(30);
            zwlr_virtual_pointer_v1_button(ptr, now_ms(), code, WL_POINTER_BUTTON_STATE_RELEASED);
            zwlr_virtual_pointer_v1_frame(ptr);
        } else if ((!strcmp(cmd, "down") || !strcmp(cmd, "up")) && i + 1 < argc) {
            if (!is_button(argv[++i])) { fprintf(stderr, "omabox-pointer: unknown button '%s'\n", argv[i]); usage(); }
            if (!run) continue;
            zwlr_virtual_pointer_v1_button(ptr, now_ms(), button_code(argv[i]), !strcmp(cmd, "down") ? WL_POINTER_BUTTON_STATE_PRESSED : WL_POINTER_BUTTON_STATE_RELEASED);
            zwlr_virtual_pointer_v1_frame(ptr);
        } else if (!strcmp(cmd, "scroll") && i + 1 < argc) {
            double dy = numd(argv[++i], -10000, 10000);
            if (!run) continue;
            zwlr_virtual_pointer_v1_axis(ptr, now_ms(), WL_POINTER_AXIS_VERTICAL_SCROLL, wl_fixed_from_double(dy));
            zwlr_virtual_pointer_v1_frame(ptr);
        } else if (!strcmp(cmd, "sleep") && i + 1 < argc) {
            long ms = num(argv[++i], 0, 600000);
            if (!run) continue;
            sync_or_die();
            sleep_ms(ms);
        } else {
            usage();
        }
        if (run) sync_or_die();
    }
}

int main(int argc, char **argv) {
    // Only ever the socket in WAYLAND_DISPLAY: libwayland would prefer an inherited WAYLAND_SOCKET,
    // which could be a connection to another compositor, the user's real one (finding 63).
    // Only ever inside a box (omabox runs it there, in the box's mount namespace): run from a host
    // shell, WAYLAND_DISPLAY is the user's real desktop (it happened once: a parse check clicked it).
    if (access("/opt/omabox/share", F_OK) != 0) {
        fprintf(stderr, "%s: only runs inside an omabox box (use omabox keys/click/pointer)\n", argv[0]);
        return 2;
    }
    unsetenv("WAYLAND_SOCKET");
    uint32_t ew = 1920, eh = 1080;
    int i = 1, hold = 0;
    for (; i < argc; i++) {
        if (!strcmp(argv[i], "--hold")) hold = 1;
        else if (!strcmp(argv[i], "--extent") && i + 1 < argc) {
            char x;
            if (sscanf(argv[++i], "%u%c%u", &ew, &x, &eh) != 3 || x != 'x' || !ew || !eh || ew > 65536 || eh > 65536) usage();
        } else break;
    }
    if (i >= argc && !hold) usage();
    commands(NULL, argc, argv, i, ew, eh, 0);

    display = wl_display_connect(NULL);
    if (!display) { fprintf(stderr, "omabox-pointer: cannot connect to $WAYLAND_DISPLAY\n"); return 1; }
    struct wl_registry *registry = wl_display_get_registry(display);
    wl_registry_add_listener(registry, &registry_listener, NULL);
    sync_or_die();
    if (!seat || !manager) { fprintf(stderr, "omabox-pointer: compositor lacks wl_seat or zwlr_virtual_pointer_manager_v1\n"); return 1; }

    struct zwlr_virtual_pointer_v1 *ptr = zwlr_virtual_pointer_manager_v1_create_virtual_pointer(manager, seat);
    // Like the keyboard's: with no pointer on the seat Hyprland drops pointer focus changes, and a
    // click where the last device left the cursor never reaches the surface under it.
    if (hold) { sync_or_die(); while (wl_display_dispatch(display) != -1) {} return 0; }
    // A new device's first real motion moves the cursor but never reaches the client as hover (the
    // same device switch that loses the keyboard's first key; a zero-delta motion does not count).
    // Spend it on a 1px nudge, then step back.
    for (int dx = 1; dx >= -1; dx -= 2) {
        zwlr_virtual_pointer_v1_motion(ptr, now_ms(), wl_fixed_from_int(dx), 0);
        zwlr_virtual_pointer_v1_frame(ptr);
        sync_or_die();
    }
    commands(ptr, argc, argv, i, ew, eh, 1);

    zwlr_virtual_pointer_v1_destroy(ptr);
    sync_or_die();
    wl_display_disconnect(display);
    return 0;
}
