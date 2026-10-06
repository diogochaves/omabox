// omabox-pointer: drive a virtual pointer on the Wayland display in $WAYLAND_DISPLAY.
//
//   omabox-pointer --extent WxH  move X Y  click [BTN]  down [BTN]  up [BTN]  scroll DY  hscroll DX
//                  source wheel|finger|continuous|tilt  sleep MS  pause
//   omabox-pointer --hold
//
// scroll DY / hscroll DX: the vertical / horizontal axis, in wl_pointer's units (a wheel notch is 15;
// positive is down / right). With no source they go as one axis event, as before. `source` sets it for
// the scrolls after it (#134): wheel and tilt send a notch (15, discrete 1) per frame, as a mouse wheel
// does, so an app counting notches sees each; finger and continuous send the distance in 10 frames,
// then axis_stop (a touchpad lifting off: kinetic scrolling starts there).
// BTN is left (the default), right or middle. pause prints "paused" on stdout and waits for a line (or
// the end) on stdin: `omabox drag --shot` takes its shot there, with the button still down.
//
// Commands run left to right in one connection, all checked before it connects. Coordinates are
// absolute in the compositor's layout, scaled against --extent (the layout size; default 1920x1080).
// It only ever talks to the socket in WAYLAND_DISPLAY (never an inherited WAYLAND_SOCKET), so aimed at
// a sandbox it cannot move the real desktop's cursor. Exit 1 if the compositor goes away mid-run.
// --hold keeps an idle pointer on the seat until the compositor goes away (NOTES finding 41): omabox's
// own, left out of the usage line (it never returns; agents took it for "hold the button").
#define _GNU_SOURCE
#include <linux/input-event-codes.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <wayland-client.h>

#include "wlr-virtual-pointer-unstable-v1-client-protocol.h"
#include "../common/box.h"
#include "../common/roundtrip.h"

static struct wl_seat *seat;
static struct zwlr_virtual_pointer_manager_v1 *manager;
static uint32_t manager_version;

static void global(void *data, struct wl_registry *reg, uint32_t name, const char *iface, uint32_t version) {
    (void)data;
    if (!strcmp(iface, wl_seat_interface.name) && !seat)
        seat = wl_registry_bind(reg, name, &wl_seat_interface, 1);
    else if (!strcmp(iface, zwlr_virtual_pointer_manager_v1_interface.name))
        manager = wl_registry_bind(reg, name, &zwlr_virtual_pointer_manager_v1_interface, manager_version = version < 2 ? version : 2);
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
    fprintf(stderr, "usage: omabox-pointer [--extent WxH] (move X Y | click [BTN] | down [BTN] | up [BTN] | scroll DY | hscroll DX |\n"
                    "                      source wheel|finger|continuous|tilt | sleep MS | pause)...  BTN: left (default), right, middle\n");
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

static int source_code(const char *s) {
    if (!s) return -1;
    if (!strcmp(s, "wheel")) return WL_POINTER_AXIS_SOURCE_WHEEL;
    if (!strcmp(s, "finger")) return WL_POINTER_AXIS_SOURCE_FINGER;
    if (!strcmp(s, "continuous")) return WL_POINTER_AXIS_SOURCE_CONTINUOUS;
    if (!strcmp(s, "tilt")) return WL_POINTER_AXIS_SOURCE_WHEEL_TILT;
    return -1;
}

static struct wl_display *display;
// A lost compositor (box gone, protocol error) is a failure, not "clicked" (finding 63).
static void sync_or_die(void) {
    int r = omabox_roundtrip(display, 10000);   // a stopped compositor never answers (finding 177)
    if (r == -2) { fprintf(stderr, "omabox-pointer: lost the compositor (no answer in 10 s)\n"); exit(3); }
    if (r < 0) { fprintf(stderr, "omabox-pointer: lost the compositor\n"); exit(1); }
}
static void sleep_ms(long ms) {
    struct timespec ts = {ms / 1000, (ms % 1000) * 1000000L};
    nanosleep(&ts, NULL);
}

// One scroll of D on AXIS, as SOURCE sends it (-1: one axis event, as before #134).
static void scroll(struct zwlr_virtual_pointer_v1 *ptr, uint32_t axis, double d, int source) {
    if (source < 0) {
        zwlr_virtual_pointer_v1_axis(ptr, now_ms(), axis, wl_fixed_from_double(d));
        zwlr_virtual_pointer_v1_frame(ptr);
        return;
    }
    if (source == WL_POINTER_AXIS_SOURCE_WHEEL || source == WL_POINTER_AXIS_SOURCE_WHEEL_TILT) {
        long notches = (long)(fabs(d) / 15 + 0.5);
        if (notches < 1) notches = 1;
        int dir = d < 0 ? -1 : 1;
        for (long k = 0; k < notches; k++) {
            zwlr_virtual_pointer_v1_axis_source(ptr, (uint32_t)source);
            zwlr_virtual_pointer_v1_axis_discrete(ptr, now_ms(), axis, wl_fixed_from_int(15 * dir), dir);
            zwlr_virtual_pointer_v1_frame(ptr);
            sync_or_die();
            sleep_ms(15);
        }
        return;
    }
    for (int k = 0; k < 10; k++) {
        zwlr_virtual_pointer_v1_axis_source(ptr, (uint32_t)source);
        zwlr_virtual_pointer_v1_axis(ptr, now_ms(), axis, wl_fixed_from_double(d / 10));
        zwlr_virtual_pointer_v1_frame(ptr);
        sync_or_die();
        sleep_ms(8);
    }
    zwlr_virtual_pointer_v1_axis_source(ptr, (uint32_t)source);
    zwlr_virtual_pointer_v1_axis_stop(ptr, now_ms(), axis);
    zwlr_virtual_pointer_v1_frame(ptr);
}

// One pass over the commands: with run = 0 it only checks them (before connecting, so a bad token
// never leaves half a sequence done, or a button held down); with run = 1 it sends them.
static void commands(struct zwlr_virtual_pointer_v1 *ptr, int argc, char **argv, int i, uint32_t ew, uint32_t eh, int run) {
    int src = -1, *source = &src;   // `source`: for the scrolls after it
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
        } else if (!strcmp(cmd, "down") || !strcmp(cmd, "up")) {
            const char *b = is_button(i + 1 < argc ? argv[i + 1] : NULL) ? argv[++i] : NULL;
            if (!run) continue;
            zwlr_virtual_pointer_v1_button(ptr, now_ms(), button_code(b), !strcmp(cmd, "down") ? WL_POINTER_BUTTON_STATE_PRESSED : WL_POINTER_BUTTON_STATE_RELEASED);
            zwlr_virtual_pointer_v1_frame(ptr);
        } else if (!strcmp(cmd, "source") && i + 1 < argc) {
            *source = source_code(argv[++i]);
            if (*source < 0) { fprintf(stderr, "omabox-pointer: source is wheel, finger, continuous or tilt, got '%s'\n", argv[i]); usage(); }
            if (!run) continue;
            if (manager_version < 2) { fprintf(stderr, "omabox-pointer: this compositor's virtual pointer (v1) takes no scroll source\n"); exit(1); }
        } else if ((!strcmp(cmd, "scroll") || !strcmp(cmd, "hscroll")) && i + 1 < argc) {
            double d = numd(argv[++i], -10000, 10000);
            if (!run) continue;
            uint32_t axis = cmd[0] == 'h' ? WL_POINTER_AXIS_HORIZONTAL_SCROLL : WL_POINTER_AXIS_VERTICAL_SCROLL;
            scroll(ptr, axis, d, *source);
        } else if (!strcmp(cmd, "sleep") && i + 1 < argc) {
            long ms = num(argv[++i], 0, 600000);
            if (!run) continue;
            sync_or_die();
            sleep_ms(ms);
        } else if (!strcmp(cmd, "pause")) {
            if (!run) continue;
            sync_or_die();
            printf("paused\n");
            fflush(stdout);
            int c;
            while ((c = getchar()) != EOF && c != '\n') {}
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
    if (!omabox_inside_box()) {
        fprintf(stderr, "%s: only runs inside an omabox box (use omabox keys/click/pointer)\n", argv[0]);
        return 2;
    }
    unsetenv("WAYLAND_SOCKET");
    uint32_t ew = 1920, eh = 1080;
    int i = 1, hold = 0;
    for (; i < argc; i++) {
        if (!strcmp(argv[i], "--hold")) hold = 1;
        else if (!strcmp(argv[i], "--extent") && i + 1 < argc) {
            // WxH, digits only: sscanf took 1920x1080abc (#109).
            const char *s = argv[++i], *x = strchr(s, 'x');
            if (!x || x == s || !x[1] || strspn(s, "0123456789") != (size_t)(x - s) || strspn(x + 1, "0123456789") != strlen(x + 1)) usage();
            unsigned long w = strtoul(s, NULL, 10), h = strtoul(x + 1, NULL, 10);
            if (!w || !h || w > 65536 || h > 65536) usage();
            ew = (unsigned)w, eh = (unsigned)h;
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
