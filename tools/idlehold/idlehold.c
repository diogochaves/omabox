// omabox-idlehold: hold a box's idle off (#321 step 5, NOTES finding 704), for a desk box, whose
// shell is the user's own and may lock, blank or suspend after its idle timeouts: an agent drives a
// box with no input between its steps, so the box would go idle under it.
//
//   omabox-idlehold
//
// On each output of the compositor in $WAYLAND_DISPLAY, a 1x1 transparent layer surface on the
// background layer (no input region, no exclusive zone), with an idle inhibitor
// (zwp_idle_inhibit_manager_v1) on it: Waybar's idle-inhibitor shape. Hyprland honours an inhibitor
// only on a mapped surface (IdleInhibitor.cpp: 0.56's a view alive and visible; 0.57's also one whose
// fade alpha is above 0, and a layer fades in from 0 as it maps) and looks again only when one is
// made or destroyed (or on focus and fullscreen changes), never at a map or an animation's end. So
// each is made once its surface's first buffer was committed and the compositor answered, and made
// again (the new one first, then the old one destroyed) at REMAKE_MS after that, by when a fade-in
// has ended. Clients asking ext-idle-notify for idle that respects inhibitors (Quickshell's
// IdleMonitor by default, hypridle, swayidle) are never told idle while it runs; one asking for
// input idle (respectInhibitors: false) still is. An output added later gets its own; one removed
// takes its own away. Prints `omabox-idlehold: holding idle off on N of M outputs` once the first
// ones hold (what `up` waits for), then runs until the compositor goes away (exit 0) or is killed.
// Exit 1 when the compositor lacks layer shell or idle inhibit, or no output held at the start.
//
// Refuses outside a box (tools/common/box.h): on the host it would hold the real desktop's idle off.
// It only ever talks to the socket in WAYLAND_DISPLAY (never an inherited WAYLAND_SOCKET).
#define _GNU_SOURCE
#include <errno.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>
#include <wayland-client.h>

#include "idle-inhibit-unstable-v1-client-protocol.h"
#include "wlr-layer-shell-unstable-v1-client-protocol.h"
#include "../common/box.h"
#include "../common/roundtrip.h"

#define NAMESPACE "omabox-idlehold"
// When an output's inhibitor is made again after its first (ms after that), each look Hyprland takes:
// a layer's fade-in (fadeLayersIn) is over by the last, unless the user's config makes it longer.
static const int REMAKE_MS[] = {250, 750, 1500, 3000, 6000};
#define REMAKES ((int)(sizeof REMAKE_MS / sizeof *REMAKE_MS))

struct hold {
    uint32_t name;   // the wl_output's global
    struct wl_output *out;
    struct wl_surface *surf;
    struct zwlr_layer_surface_v1 *layer;
    struct wl_buffer *buf;
    struct zwp_idle_inhibitor_v1 *inh;
    int64_t made;    // when its first inhibitor was made (omabox_now_ms)
    int remade;      // how many of REMAKE_MS are done
    struct hold *next;
};

static struct wl_display *display;
static struct wl_compositor *compositor;
static struct wl_shm *shm;
static struct wl_shm_pool *pool;
static struct zwlr_layer_shell_v1 *layer_shell;
static struct zwp_idle_inhibit_manager_v1 *inhibit_manager;
static struct hold *holds;
static int started;   // the first outputs are started: one from now on starts at once
static int said;      // the first holds are said: one from now on is said as it comes

static void hold_start(struct hold *h);

static void hold_stop(struct hold *h) {
    if (h->inh) zwp_idle_inhibitor_v1_destroy(h->inh);
    if (h->layer) zwlr_layer_surface_v1_destroy(h->layer);
    if (h->surf) wl_surface_destroy(h->surf);
    if (h->buf) wl_buffer_destroy(h->buf);
    h->inh = NULL, h->layer = NULL, h->surf = NULL, h->buf = NULL;
}

// The surface is mapped once the compositor has answered the commit that gave it its buffer: the
// inhibitor now counts from the moment it is made.
static void mapped(void *data, struct wl_callback *cb, uint32_t t) {
    (void)t;
    struct hold *h = data;
    wl_callback_destroy(cb);
    if (!h->surf || h->inh) return;
    h->inh = zwp_idle_inhibit_manager_v1_create_inhibitor(inhibit_manager, h->surf);
    h->made = omabox_now_ms(), h->remade = 0;
    if (said) printf("omabox-idlehold: holding idle off on output %u\n", h->name), fflush(stdout);
}
static const struct wl_callback_listener mapped_listener = {mapped};

static void configure(void *data, struct zwlr_layer_surface_v1 *l, uint32_t serial, uint32_t w, uint32_t hh) {
    (void)w, (void)hh;
    struct hold *h = data;
    zwlr_layer_surface_v1_ack_configure(l, serial);
    if (h->buf) { wl_surface_commit(h->surf); return; }
    h->buf = wl_shm_pool_create_buffer(pool, 0, 1, 1, 4, WL_SHM_FORMAT_ARGB8888);
    wl_surface_attach(h->surf, h->buf, 0, 0);
    wl_surface_damage(h->surf, 0, 0, 1, 1);
    wl_surface_commit(h->surf);
    wl_callback_add_listener(wl_display_sync(display), &mapped_listener, h);
}

// The compositor took the surface away (its output going, or anything else): this output's hold is
// gone; the output's own global_remove frees the rest.
static void closed(void *data, struct zwlr_layer_surface_v1 *l) {
    (void)l;
    struct hold *h = data;
    hold_stop(h);
    printf("omabox-idlehold: output %u's hold closed by the compositor\n", h->name), fflush(stdout);
}
static const struct zwlr_layer_surface_v1_listener layer_listener = {configure, closed};

static void hold_start(struct hold *h) {
    h->surf = wl_compositor_create_surface(compositor);
    struct wl_region *none = wl_compositor_create_region(compositor);
    wl_surface_set_input_region(h->surf, none);   // clicks go through it
    wl_region_destroy(none);
    h->layer = zwlr_layer_shell_v1_get_layer_surface(layer_shell, h->surf, h->out,
                                                     ZWLR_LAYER_SHELL_V1_LAYER_BACKGROUND, NAMESPACE);
    zwlr_layer_surface_v1_set_size(h->layer, 1, 1);
    zwlr_layer_surface_v1_set_anchor(h->layer, ZWLR_LAYER_SURFACE_V1_ANCHOR_TOP | ZWLR_LAYER_SURFACE_V1_ANCHOR_LEFT);
    zwlr_layer_surface_v1_add_listener(h->layer, &layer_listener, h);
    wl_surface_commit(h->surf);
}

static void global(void *data, struct wl_registry *reg, uint32_t name, const char *iface, uint32_t version) {
    (void)data;
    if (!strcmp(iface, wl_compositor_interface.name) && !compositor)
        compositor = wl_registry_bind(reg, name, &wl_compositor_interface, version < 4 ? version : 4);
    else if (!strcmp(iface, wl_shm_interface.name) && !shm)
        shm = wl_registry_bind(reg, name, &wl_shm_interface, 1);
    else if (!strcmp(iface, zwlr_layer_shell_v1_interface.name) && !layer_shell)
        layer_shell = wl_registry_bind(reg, name, &zwlr_layer_shell_v1_interface, 1);
    else if (!strcmp(iface, zwp_idle_inhibit_manager_v1_interface.name) && !inhibit_manager)
        inhibit_manager = wl_registry_bind(reg, name, &zwp_idle_inhibit_manager_v1_interface, 1);
    else if (!strcmp(iface, wl_output_interface.name)) {
        struct hold *h = calloc(1, sizeof *h);
        if (!h) { fprintf(stderr, "omabox-idlehold: out of memory\n"); exit(1); }
        h->name = name;
        h->out = wl_registry_bind(reg, name, &wl_output_interface, 1);
        h->next = holds, holds = h;
        if (started) hold_start(h);
    }
}

static void global_remove(void *data, struct wl_registry *reg, uint32_t name) {
    (void)data, (void)reg;
    for (struct hold **p = &holds; *p; p = &(*p)->next) {
        if ((*p)->name != name) continue;
        struct hold *h = *p;
        *p = h->next;
        hold_stop(h);
        wl_output_destroy(h->out);
        free(h);
        return;
    }
}
static const struct wl_registry_listener registry_listener = {global, global_remove};

// A 1x1 pool of one transparent ARGB pixel, every output's buffer made from it.
static int make_pool(void) {
    int fd = memfd_create("omabox-idlehold", MFD_CLOEXEC);
    if (fd < 0) return -1;
    if (ftruncate(fd, 4) != 0) { close(fd); return -1; }   // (zero-filled: alpha 0)
    pool = wl_shm_create_pool(shm, fd, 4);
    close(fd);
    return 0;
}

// remake: each inhibitor whose next REMAKE_MS is due made again, the new one before the old one goes
// (each a look Hyprland takes, the hold never off between them); the ms until the next one is due, or
// -1 for none.
static int remake(void) {
    int64_t now = omabox_now_ms(), next = -1;
    for (struct hold *h = holds; h; h = h->next) {
        if (!h->inh) continue;
        while (h->remade < REMAKES && now >= h->made + REMAKE_MS[h->remade]) {
            struct zwp_idle_inhibitor_v1 *old = h->inh;
            h->inh = zwp_idle_inhibit_manager_v1_create_inhibitor(inhibit_manager, h->surf);
            zwp_idle_inhibitor_v1_destroy(old);
            h->remade++;
        }
        if (h->remade < REMAKES) {
            int64_t due = h->made + REMAKE_MS[h->remade] - now;
            if (next < 0 || due < next) next = due;
        }
    }
    return (int)next;
}

// Events until the compositor goes away (0), the inhibitors made again as they fall due.
static int run(void) {
    int fd = wl_display_get_fd(display);
    for (;;) {
        while (wl_display_prepare_read(display) != 0)
            if (wl_display_dispatch_pending(display) < 0) return 0;
        int wait = remake();
        if (wl_display_flush(display) < 0 && errno != EAGAIN) { wl_display_cancel_read(display); return 0; }
        struct pollfd p = {.fd = fd, .events = POLLIN};
        int n = poll(&p, 1, wait);
        if (n > 0) {
            if (wl_display_read_events(display) < 0) return 0;
        } else {
            wl_display_cancel_read(display);
            if (n < 0 && errno != EINTR) { fprintf(stderr, "omabox-idlehold: poll: %s\n", strerror(errno)); return 1; }
        }
        if (wl_display_dispatch_pending(display) < 0) return 0;
    }
}

static int roundtrip_or_die(void) {
    int r = omabox_roundtrip(display, 5000);
    if (r == -2) { fprintf(stderr, "omabox-idlehold: the compositor did not answer in 5 s\n"); exit(1); }
    if (r < 0) { fprintf(stderr, "omabox-idlehold: lost the compositor\n"); exit(1); }
    return 0;
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
    display = wl_display_connect(NULL);
    if (!display) { fprintf(stderr, "omabox-idlehold: cannot connect to $WAYLAND_DISPLAY\n"); return 1; }
    struct wl_registry *registry = wl_display_get_registry(display);
    wl_registry_add_listener(registry, &registry_listener, NULL);
    roundtrip_or_die();
    if (!compositor || !shm) { fprintf(stderr, "omabox-idlehold: the compositor has no wl_compositor or wl_shm\n"); return 1; }
    if (!layer_shell) { fprintf(stderr, "omabox-idlehold: the compositor has no layer shell (zwlr_layer_shell_v1)\n"); return 1; }
    if (!inhibit_manager) { fprintf(stderr, "omabox-idlehold: the compositor has no idle inhibit (zwp_idle_inhibit_manager_v1)\n"); return 1; }
    if (make_pool() != 0) { fprintf(stderr, "omabox-idlehold: cannot make its buffer: %s\n", strerror(errno)); return 1; }
    for (struct hold *h = holds; h; h = h->next) hold_start(h);
    started = 1;
    // configure (its buffer committed), the commit's answer (its inhibitor made), the inhibitor's.
    for (int i = 0; i < 3; i++) roundtrip_or_die();
    int n = 0, m = 0;
    for (struct hold *h = holds; h; h = h->next, m++) n += h->inh != NULL;
    printf("omabox-idlehold: holding idle off on %d of %d outputs\n", n, m);
    fflush(stdout);
    said = 1;
    if (n == 0) { fprintf(stderr, "omabox-idlehold: no output held\n"); return 1; }
    return run();
}
