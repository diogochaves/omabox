// omabox-idlehold: hold a box's idle off (#321 step 5, NOTES finding 704), for a desk box, whose
// shell is the user's own and may lock, blank or suspend after its idle timeouts: an agent drives a
// box with no input between its steps, so the box would go idle under it.
//
//   omabox-idlehold
//
// One idle inhibitor (zwp_idle_inhibit_manager_v1) on one bare wl_surface that never gets a role (no
// layer, no window, no buffer, on no output). Hyprland finds no view behind such a surface and counts
// its inhibitor as a non-desktop element's, "assuming it's visible" (IdleInhibitor.cpp): it holds on
// every output, under a fullscreen window too, from the moment it is made, with nothing to map or
// fade in. (A mapped surface's inhibitor counts only while Hyprland judges the surface visible, and
// 0.57 judged a new layer before its fade-in and never again, finding 704.) Clients asking
// ext-idle-notify for idle that respects inhibitors (Quickshell's IdleMonitor by default, hypridle,
// swayidle) are never told idle while it runs; one asking for input idle (respectInhibitors: false)
// still is. Prints `omabox-idlehold: holding idle off on every output` once the compositor has the
// inhibitor (what `up` waits for), then runs until the compositor goes away (exit 0) or is killed.
// Exit 1 when the compositor lacks idle inhibit.
//
// Refuses outside a box (tools/common/box.h): on the host it would hold the real desktop's idle off.
// It only ever talks to the socket in WAYLAND_DISPLAY (never an inherited WAYLAND_SOCKET).
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <wayland-client.h>

#include "idle-inhibit-unstable-v1-client-protocol.h"
#include "../common/box.h"
#include "../common/roundtrip.h"

static struct wl_compositor *compositor;
static struct zwp_idle_inhibit_manager_v1 *inhibit_manager;

static void global(void *data, struct wl_registry *reg, uint32_t name, const char *iface, uint32_t version) {
    (void)data, (void)version;
    if (!strcmp(iface, wl_compositor_interface.name) && !compositor)
        compositor = wl_registry_bind(reg, name, &wl_compositor_interface, 1);
    else if (!strcmp(iface, zwp_idle_inhibit_manager_v1_interface.name) && !inhibit_manager)
        inhibit_manager = wl_registry_bind(reg, name, &zwp_idle_inhibit_manager_v1_interface, 1);
}
static void global_remove(void *data, struct wl_registry *reg, uint32_t name) {
    (void)data, (void)reg, (void)name;
}
static const struct wl_registry_listener registry_listener = {global, global_remove};

static void roundtrip_or_die(struct wl_display *display) {
    int r = omabox_roundtrip(display, 5000);
    if (r == -2) { fprintf(stderr, "omabox-idlehold: the compositor did not answer in 5 s\n"); exit(1); }
    if (r < 0) { fprintf(stderr, "omabox-idlehold: lost the compositor\n"); exit(1); }
}

int main(int argc, char **argv) {
    (void)argv;
    if (!omabox_inside_box()) {
        fprintf(stderr, "omabox-idlehold: only runs inside an omabox box\n");
        return 2;
    }
    if (argc > 1) {
        fprintf(stderr, "usage: omabox-idlehold (omabox starts it in a desk box)\n");
        return 2;
    }
    unsetenv("WAYLAND_SOCKET");
    struct wl_display *display = wl_display_connect(NULL);
    if (!display) { fprintf(stderr, "omabox-idlehold: cannot connect to $WAYLAND_DISPLAY\n"); return 1; }
    struct wl_registry *registry = wl_display_get_registry(display);
    wl_registry_add_listener(registry, &registry_listener, NULL);
    roundtrip_or_die(display);
    if (!compositor) { fprintf(stderr, "omabox-idlehold: the compositor has no wl_compositor\n"); return 1; }
    if (!inhibit_manager) { fprintf(stderr, "omabox-idlehold: the compositor has no idle inhibit (zwp_idle_inhibit_manager_v1)\n"); return 1; }
    // Never committed: a role, a buffer or an output would have Hyprland judge whether it is seen.
    struct wl_surface *surface = wl_compositor_create_surface(compositor);
    zwp_idle_inhibit_manager_v1_create_inhibitor(inhibit_manager, surface);
    roundtrip_or_die(display);
    printf("omabox-idlehold: holding idle off on every output\n");
    fflush(stdout);
    while (wl_display_dispatch(display) >= 0) {}
    return 0;
}
