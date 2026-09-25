// omabox-peek: a live, view-only window of a box's screen on the host desktop.
//
//   omabox-peek --box /path/to/box/wayland-1 [--output NAME] [--fps N] [--title TEXT]
//
// Two Wayland connections: to the box's compositor, where it only ever captures the screen
// (wlr-screencopy, the same way `omabox shot` does; no input, nothing in the box changes), and to the
// host compositor ($WAYLAND_DISPLAY), where it shows the frames in a window, scaled to fit.
// A plain copy (not copy_with_damage) returns the current frame even when the box is idle, which is
// what left the old VNC view black (NOTES finding 34). It captures only when the host is ready for the
// next frame (frame callbacks), so a peek hidden on workspace 9 costs next to nothing.
// The box is what is being contained, and it can answer on its own socket: every frame's size, stride
// and format are checked before peek (a host process) reads from the buffer.
#define _GNU_SOURCE
#include <errno.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>
#include <wayland-client.h>

#include "wlr-screencopy-unstable-v1-client-protocol.h"
#include "xdg-shell-client-protocol.h"

#define DRM_FORMAT_XBGR8888 0x34324258
#define DRM_FORMAT_ABGR8888 0x34324241
#define MAX_SIDE 16384

// ---- shared memory buffers ------------------------------------------------------------------------

struct shmbuf {
    struct wl_buffer *buffer;
    uint32_t *data;
    int width, height, stride;
    size_t size;
    int busy;
};

static int shmbuf_create(struct shmbuf *b, struct wl_shm *shm, int width, int height, int stride, uint32_t format) {
    b->size = (size_t)stride * (size_t)height;
    int fd = memfd_create("omabox-peek", MFD_CLOEXEC);
    if (fd < 0) return 0;
    if (ftruncate(fd, (off_t)b->size) < 0) { close(fd); return 0; }
    b->data = mmap(NULL, b->size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (b->data == MAP_FAILED) { close(fd); return 0; }
    struct wl_shm_pool *pool = wl_shm_create_pool(shm, fd, (int32_t)b->size);
    b->buffer = wl_shm_pool_create_buffer(pool, 0, width, height, stride, format);
    wl_shm_pool_destroy(pool);
    close(fd);
    b->width = width; b->height = height; b->stride = stride; b->busy = 0;
    return 1;
}

static void shmbuf_destroy(struct shmbuf *b) {
    if (b->buffer) wl_buffer_destroy(b->buffer);
    if (b->data && b->data != MAP_FAILED) munmap(b->data, b->size);
    memset(b, 0, sizeof(*b));
}

// ---- the box side: capture ------------------------------------------------------------------------

static struct {
    struct wl_display *display;
    struct wl_shm *shm;
    struct zwlr_screencopy_manager_v1 *manager;
    uint32_t manager_version;
    struct wl_output *output;
    const char *want_output;
    struct zwlr_screencopy_frame_v1 *frame;
    struct shmbuf buf;
    uint32_t format;          // of the frame being captured
    int width, height, stride;
    int y_invert;
    uint32_t buf_format;      // of the complete frame in buf (what draw() reads)
    int buf_y_invert;
    int have_frame;  // buf holds a complete frame not yet drawn
} box;

static void output_name(void *data, struct wl_output *o, const char *name) {
    (void)data;
    if (box.want_output && !strcmp(name, box.want_output)) box.output = o;
}
static void output_geometry(void *d, struct wl_output *o, int32_t x, int32_t y, int32_t pw, int32_t ph, int32_t sp,
                            const char *make, const char *model, int32_t t) {
    (void)d; (void)o; (void)x; (void)y; (void)pw; (void)ph; (void)sp; (void)make; (void)model; (void)t;
}
static void output_mode(void *d, struct wl_output *o, uint32_t f, int32_t w, int32_t h, int32_t r) {
    (void)d; (void)o; (void)f; (void)w; (void)h; (void)r;
}
static void output_done(void *d, struct wl_output *o) { (void)d; (void)o; }
static void output_scale(void *d, struct wl_output *o, int32_t s) { (void)d; (void)o; (void)s; }
static void output_description(void *d, struct wl_output *o, const char *s) { (void)d; (void)o; (void)s; }
static const struct wl_output_listener output_listener = {
    output_geometry, output_mode, output_done, output_scale, output_name, output_description};

static void box_global(void *data, struct wl_registry *reg, uint32_t name, const char *iface, uint32_t version) {
    (void)data;
    if (!strcmp(iface, wl_shm_interface.name)) {
        box.shm = wl_registry_bind(reg, name, &wl_shm_interface, 1);
    } else if (!strcmp(iface, zwlr_screencopy_manager_v1_interface.name)) {
        box.manager_version = version < 3 ? version : 3;
        box.manager = wl_registry_bind(reg, name, &zwlr_screencopy_manager_v1_interface, box.manager_version);
    } else if (!strcmp(iface, wl_output_interface.name)) {
        struct wl_output *o = wl_registry_bind(reg, name, &wl_output_interface, version < 4 ? version : 4);
        if (version >= 4) wl_output_add_listener(o, &output_listener, NULL);
        if (!box.output && !box.want_output) box.output = o;
    }
}
static void global_remove(void *data, struct wl_registry *reg, uint32_t name) { (void)data; (void)reg; (void)name; }
static const struct wl_registry_listener box_registry = {box_global, global_remove};

static void frame_copy(void) {
    if (box.buf.buffer && (box.buf.width != box.width || box.buf.height != box.height || box.buf.stride != box.stride))
        shmbuf_destroy(&box.buf);
    if (!box.buf.buffer && !shmbuf_create(&box.buf, box.shm, box.width, box.height, box.stride, box.format)) {
        fprintf(stderr, "omabox-peek: cannot allocate a capture buffer\n");
        exit(1);
    }
    zwlr_screencopy_frame_v1_copy(box.frame, box.buf.buffer);
}

static void frame_buffer(void *data, struct zwlr_screencopy_frame_v1 *f, uint32_t format, uint32_t w, uint32_t h, uint32_t stride) {
    (void)data; (void)f;
    // 32-bit pixels only, rows at least as long as the width, all of it well inside an int.
    int fmt_ok = format == WL_SHM_FORMAT_XRGB8888 || format == WL_SHM_FORMAT_ARGB8888 ||
                 format == DRM_FORMAT_XBGR8888 || format == DRM_FORMAT_ABGR8888;
    if (!fmt_ok || !w || !h || w > MAX_SIDE || h > MAX_SIDE || stride % 4 || stride < w * 4 ||
        (uint64_t)stride * h > INT32_MAX) {
        fprintf(stderr, "omabox-peek: the box sent a frame it cannot have (format %#x, %ux%u, stride %u)\n", format, w, h, stride);
        exit(1);
    }
    box.format = format; box.width = (int)w; box.height = (int)h; box.stride = (int)stride;
    if (box.manager_version < 3) frame_copy();  // v3 announces all buffer types, then buffer_done
}
static void frame_flags(void *data, struct zwlr_screencopy_frame_v1 *f, uint32_t flags) {
    (void)data; (void)f;
    box.y_invert = flags & ZWLR_SCREENCOPY_FRAME_V1_FLAGS_Y_INVERT;
}
static void frame_ready(void *data, struct zwlr_screencopy_frame_v1 *f, uint32_t sec_hi, uint32_t sec_lo, uint32_t nsec) {
    (void)data; (void)sec_hi; (void)sec_lo; (void)nsec;
    zwlr_screencopy_frame_v1_destroy(f);
    box.frame = NULL;
    box.buf_format = box.format; box.buf_y_invert = box.y_invert;
    box.have_frame = 1;
}
static void frame_failed(void *data, struct zwlr_screencopy_frame_v1 *f) {
    (void)data;
    zwlr_screencopy_frame_v1_destroy(f);
    box.frame = NULL;
}
static void frame_damage(void *d, struct zwlr_screencopy_frame_v1 *f, uint32_t x, uint32_t y, uint32_t w, uint32_t h) {
    (void)d; (void)f; (void)x; (void)y; (void)w; (void)h;
}
static void frame_dmabuf(void *d, struct zwlr_screencopy_frame_v1 *f, uint32_t fmt, uint32_t w, uint32_t h) {
    (void)d; (void)f; (void)fmt; (void)w; (void)h;
}
static void frame_buffer_done(void *data, struct zwlr_screencopy_frame_v1 *f) { (void)data; (void)f; frame_copy(); }
static const struct zwlr_screencopy_frame_v1_listener frame_listener = {
    frame_buffer, frame_flags, frame_ready, frame_failed, frame_damage, frame_dmabuf, frame_buffer_done};

static void capture(void) {
    box.frame = zwlr_screencopy_manager_v1_capture_output(box.manager, 1, box.output);
    zwlr_screencopy_frame_v1_add_listener(box.frame, &frame_listener, NULL);
}

// ---- the host side: the window --------------------------------------------------------------------

static struct {
    struct wl_display *display;
    struct wl_compositor *compositor;
    struct wl_shm *shm;
    struct xdg_wm_base *wm;
    struct wl_surface *surface;
    struct xdg_surface *xdg_surface;
    struct xdg_toplevel *toplevel;
    int width, height, pending_w, pending_h;
    int configured, closed;
    int waiting;   // a frame callback is pending: the host has not shown the last one yet
    struct shmbuf bufs[2];
} host;

static void wm_ping(void *data, struct xdg_wm_base *wm, uint32_t serial) { (void)data; xdg_wm_base_pong(wm, serial); }
static const struct xdg_wm_base_listener wm_listener = {wm_ping};

static void host_global(void *data, struct wl_registry *reg, uint32_t name, const char *iface, uint32_t version) {
    (void)data; (void)version;
    if (!strcmp(iface, wl_compositor_interface.name))
        host.compositor = wl_registry_bind(reg, name, &wl_compositor_interface, 4);
    else if (!strcmp(iface, wl_shm_interface.name))
        host.shm = wl_registry_bind(reg, name, &wl_shm_interface, 1);
    else if (!strcmp(iface, xdg_wm_base_interface.name)) {
        host.wm = wl_registry_bind(reg, name, &xdg_wm_base_interface, 1);
        xdg_wm_base_add_listener(host.wm, &wm_listener, NULL);
    }
}
static const struct wl_registry_listener host_registry = {host_global, global_remove};

static void toplevel_configure(void *data, struct xdg_toplevel *t, int32_t w, int32_t h, struct wl_array *states) {
    (void)data; (void)t; (void)states;
    host.pending_w = w; host.pending_h = h;
}
static void toplevel_close(void *data, struct xdg_toplevel *t) { (void)data; (void)t; host.closed = 1; }
static void toplevel_bounds(void *d, struct xdg_toplevel *t, int32_t w, int32_t h) { (void)d; (void)t; (void)w; (void)h; }
static void toplevel_caps(void *d, struct xdg_toplevel *t, struct wl_array *a) { (void)d; (void)t; (void)a; }
static const struct xdg_toplevel_listener toplevel_listener = {toplevel_configure, toplevel_close, toplevel_bounds, toplevel_caps};

static void xdg_surface_configure(void *data, struct xdg_surface *s, uint32_t serial) {
    (void)data;
    xdg_surface_ack_configure(s, serial);
    host.width = host.pending_w > 0 ? host.pending_w : (box.width > 0 ? box.width / 2 : 960);
    host.height = host.pending_h > 0 ? host.pending_h : (box.height > 0 ? box.height / 2 : 540);
    host.configured = 1;
}
static const struct xdg_surface_listener xdg_surface_listener = {xdg_surface_configure};

static void buffer_release(void *data, struct wl_buffer *b) { (void)b; ((struct shmbuf *)data)->busy = 0; }
static const struct wl_buffer_listener buffer_listener = {buffer_release};

// The host stops frame callbacks while the window is not shown (another workspace), so no capture.
static void frame_done(void *data, struct wl_callback *cb, uint32_t t) { (void)data; (void)t; wl_callback_destroy(cb); host.waiting = 0; }
static const struct wl_callback_listener frame_done_listener = {frame_done};

static void *xmalloc(size_t n) {
    void *p = malloc(n);
    if (!p) { fprintf(stderr, "omabox-peek: out of memory\n"); exit(1); }
    return p;
}

// Scale the captured frame into the window, letterboxed, bilinear. Box pixels are XRGB/ARGB8888 or
// XBGR/ABGR8888 (swapped to XRGB).
static void draw(void) {
    struct shmbuf *b = NULL;
    for (int i = 0; i < 2; i++)
        if (!host.bufs[i].busy) { b = &host.bufs[i]; break; }
    if (!b) return;  // both with the compositor: skip this frame
    if (b->buffer && (b->width != host.width || b->height != host.height)) shmbuf_destroy(b);
    if (!b->buffer) {
        if (!shmbuf_create(b, host.shm, host.width, host.height, host.width * 4, WL_SHM_FORMAT_XRGB8888)) exit(1);
        wl_buffer_add_listener(b->buffer, &buffer_listener, b);
    }
    // The completed frame's own size and format: box.width etc. may already describe the next one.
    const int W = host.width, H = host.height, sw = box.buf.width, sh = box.buf.height;
    const int swap = box.buf_format == DRM_FORMAT_XBGR8888 || box.buf_format == DRM_FORMAT_ABGR8888;
    // fit: the largest sw:sh rectangle inside W x H
    int dw = W, dh = (int)((int64_t)W * sh / sw);
    if (dh > H) { dh = H; dw = (int)((int64_t)H * sw / sh); }
    const int ox = (W - dw) / 2, oy = (H - dh) / 2;
    const uint32_t bg = 0xff111111;
    const uint32_t *src = box.buf.data;
    const int sstride = box.buf.stride / 4;
    // Column mapping (source pixels + weight) depends only on the sizes: compute once per change.
    static int *cx0, *cx1, cw_for = -1, cs_for = -1, cd_for = -1;
    static uint32_t *cwx;
    if (cw_for != W || cs_for != sw || cd_for != dw) {
        free(cx0); free(cx1); free(cwx);
        cx0 = xmalloc(sizeof(int) * (size_t)dw); cx1 = xmalloc(sizeof(int) * (size_t)dw); cwx = xmalloc(sizeof(uint32_t) * (size_t)dw);
        for (int x = 0; x < dw; x++) {
            int64_t fx = ((int64_t)x * 2 + 1) * sw * 32768 / dw - 32768;  // 16.16, pixel centres
            if (fx < 0) fx = 0;
            cx0[x] = (int)(fx >> 16);
            cx1[x] = cx0[x] + 1 < sw ? cx0[x] + 1 : cx0[x];
            cwx[x] = (uint32_t)(fx & 0xffff) >> 8;
        }
        cw_for = W; cs_for = sw; cd_for = dw;
    }
    for (int y = 0; y < H; y++) {
        uint32_t *row = b->data + (size_t)y * (size_t)W;
        if (y < oy || y >= oy + dh) { for (int x = 0; x < W; x++) row[x] = bg; continue; }
        int64_t fy = ((int64_t)(y - oy) * 2 + 1) * sh * 32768 / dh - 32768;
        if (fy < 0) fy = 0;
        int y0 = (int)(fy >> 16), y1 = y0 + 1 < sh ? y0 + 1 : y0;
        const uint32_t wy = (uint32_t)(fy & 0xffff) >> 8;
        if (box.buf_y_invert) { y0 = sh - 1 - y0; y1 = sh - 1 - y1; }
        const uint32_t *r0 = src + (size_t)y0 * (size_t)sstride, *r1 = src + (size_t)y1 * (size_t)sstride;
        for (int x = 0; x < ox; x++) row[x] = bg;
        for (int x = ox + dw; x < W; x++) row[x] = bg;
        uint32_t *out = row + ox;
        for (int x = 0; x < dw; x++) {
            const uint32_t a = r0[cx0[x]], bb = r0[cx1[x]], c = r1[cx0[x]], d = r1[cx1[x]], wx = cwx[x];
            // red+blue and green channels two at a time (8-bit weights, no overflow in 32 bits per pair)
            uint32_t rb_t = ((a & 0xff00ff) * (256 - wx) + (bb & 0xff00ff) * wx) >> 8 & 0xff00ff;
            uint32_t rb_b = ((c & 0xff00ff) * (256 - wx) + (d & 0xff00ff) * wx) >> 8 & 0xff00ff;
            uint32_t g_t = ((a & 0xff00) * (256 - wx) + (bb & 0xff00) * wx) >> 8 & 0xff00;
            uint32_t g_b = ((c & 0xff00) * (256 - wx) + (d & 0xff00) * wx) >> 8 & 0xff00;
            uint32_t rb = (rb_t * (256 - wy) + rb_b * wy) >> 8 & 0xff00ff;
            uint32_t g = (g_t * (256 - wy) + g_b * wy) >> 8 & 0xff00;
            uint32_t px = 0xff000000 | rb | g;
            if (swap) px = (px & 0xff00ff00) | ((px >> 16) & 0xff) | ((px & 0xff) << 16);
            out[x] = px;
        }
    }
    wl_surface_attach(host.surface, b->buffer, 0, 0);
    wl_surface_damage_buffer(host.surface, 0, 0, W, H);
    wl_callback_add_listener(wl_surface_frame(host.surface), &frame_done_listener, NULL);
    host.waiting = 1;
    wl_surface_commit(host.surface);
    b->busy = 1;
}

// ---- main loop -------------------------------------------------------------------------------------

static int64_t now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

// Dispatch whatever is readable on the two connections, waiting up to timeout ms.
// Returns 0 when a connection is gone.
static int pump(int timeout) {
    struct wl_display *d[2] = {box.display, host.display};
    struct pollfd fds[2];
    for (int i = 0; i < 2; i++) {
        while (wl_display_prepare_read(d[i]) != 0)
            if (wl_display_dispatch_pending(d[i]) < 0) {
                if (i) wl_display_cancel_read(d[0]);
                return 0;
            }
        if (wl_display_flush(d[i]) < 0 && errno != EAGAIN) {
            for (int j = 0; j <= i; j++) wl_display_cancel_read(d[j]);  // both prepared so far
            return 0;
        }
        fds[i] = (struct pollfd){.fd = wl_display_get_fd(d[i]), .events = POLLIN};
    }
    int n = poll(fds, 2, timeout);
    for (int i = 0; i < 2; i++) {
        if (n > 0 && (fds[i].revents & POLLIN)) {
            if (wl_display_read_events(d[i]) < 0) return 0;
        } else {
            wl_display_cancel_read(d[i]);
        }
        if (fds[i].revents & (POLLERR | POLLHUP)) return 0;
        if (wl_display_dispatch_pending(d[i]) < 0) return 0;
    }
    return 1;
}

int main(int argc, char **argv) {
    const char *box_socket = NULL, *title = "omabox peek";
    int fps = 10;
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--box") && i + 1 < argc) box_socket = argv[++i];
        else if (!strcmp(argv[i], "--output") && i + 1 < argc) box.want_output = argv[++i];
        else if (!strcmp(argv[i], "--fps") && i + 1 < argc) {
            char *end;
            long v = strtol(argv[++i], &end, 10);
            if (!*argv[i] || *end || v < 1 || v > 99) { fprintf(stderr, "omabox-peek: --fps is 1 to 99\n"); return 2; }
            fps = v > 60 ? 60 : (int)v;
        }
        else if (!strcmp(argv[i], "--title") && i + 1 < argc) title = argv[++i];
        else { fprintf(stderr, "usage: omabox-peek --box SOCKET [--output NAME] [--fps N] [--title TEXT]\n"); return 2; }
    }
    if (!box_socket) { fprintf(stderr, "omabox-peek: --box is required\n"); return 2; }
    // libwayland prefers an inherited WAYLAND_SOCKET over both the box's socket and WAYLAND_DISPLAY.
    unsetenv("WAYLAND_SOCKET");

    box.display = wl_display_connect(box_socket);
    if (!box.display) { fprintf(stderr, "omabox-peek: cannot connect to the box at %s\n", box_socket); return 1; }
    wl_registry_add_listener(wl_display_get_registry(box.display), &box_registry, NULL);
    if (wl_display_roundtrip(box.display) < 0 || wl_display_roundtrip(box.display) < 0) {  // globals, output names
        fprintf(stderr, "omabox-peek: lost the box\n");
        return 1;
    }
    if (!box.shm || !box.manager || !box.output) {
        fprintf(stderr, "omabox-peek: the box offers no screencopy, shm or output%s%s\n",
                box.want_output ? " named " : "", box.want_output ? box.want_output : "");
        return 1;
    }

    host.display = wl_display_connect(NULL);
    if (!host.display) { fprintf(stderr, "omabox-peek: cannot connect to the host display\n"); return 1; }
    wl_registry_add_listener(wl_display_get_registry(host.display), &host_registry, NULL);
    if (wl_display_roundtrip(host.display) < 0) { fprintf(stderr, "omabox-peek: lost the host display\n"); return 1; }
    if (!host.compositor || !host.shm || !host.wm) { fprintf(stderr, "omabox-peek: host lacks compositor, shm or xdg_wm_base\n"); return 1; }

    // First frame before the window, so the window can open at a sensible size.
    capture();
    while (!box.have_frame && box.frame) if (wl_display_dispatch(box.display) < 0) return 1;
    if (!box.have_frame) { fprintf(stderr, "omabox-peek: the box refused a screen capture\n"); return 1; }

    host.surface = wl_compositor_create_surface(host.compositor);
    host.xdg_surface = xdg_wm_base_get_xdg_surface(host.wm, host.surface);
    xdg_surface_add_listener(host.xdg_surface, &xdg_surface_listener, NULL);
    host.toplevel = xdg_surface_get_toplevel(host.xdg_surface);
    xdg_toplevel_add_listener(host.toplevel, &toplevel_listener, NULL);
    xdg_toplevel_set_title(host.toplevel, title);
    xdg_toplevel_set_app_id(host.toplevel, "omabox-peek");
    wl_surface_commit(host.surface);

    const int64_t interval = 1000 / fps;
    int64_t next = 0;
    while (!host.closed) {
        if (host.configured && box.have_frame) { draw(); box.have_frame = 0; }
        int64_t t = now_ms();
        if (!box.frame && !host.waiting && t >= next) { capture(); next = t + interval; }
        int timeout = box.frame || host.waiting ? 100 : (int)(next - t > 0 ? next - t : 0);
        if (!pump(timeout)) break;
    }
    return 0;
}
