// omabox-peek: a live, view-only window of a box's screen on the host desktop.
//
//   omabox-peek --box /path/to/box/wayland-1 [--output NAME] [--fps N] [--title TEXT] [--marks FILE]
//
// One window shows one of the box's outputs (--output, else the first advertised); a box with several
// monitors gets a peek window each.
//
// Two Wayland connections: to the box's compositor, where it only ever captures the screen
// (wlr-screencopy, the same way `omabox shot` does; no input, nothing in the box changes), and to the
// host compositor ($WAYLAND_DISPLAY), where it shows the frames in a window, scaled to fit.
// A plain copy (not copy_with_damage) returns the current frame even when the box is idle, which is
// what left the old VNC view black (NOTES finding 34). It captures only when the host is ready for the
// next frame (frame callbacks), so a peek hidden on workspace 9 costs next to nothing.
// The box is what is being contained, and it can answer on its own socket: every frame's size, stride
// and format are checked before peek (a host process) reads from the buffer.
// --marks FILE: what omabox click/pointer/keys did, drawn over the view (see "marks" below). Every
// peek window of a box reads the same file; each draws only what happened on its own output.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/inotify.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>
#include <wayland-client.h>

#include "../common/roundtrip.h"
#include "font8x8.h"
#include "wlr-screencopy-unstable-v1-client-protocol.h"
#include "xdg-output-unstable-v1-client-protocol.h"
#include "xdg-shell-client-protocol.h"

#define DRM_FORMAT_XBGR8888 0x34324258
#define DRM_FORMAT_ABGR8888 0x34324241
#define MAX_SIDE 16384

// ---- shared memory buffers ------------------------------------------------------------------------

struct shmbuf {
    struct wl_buffer *buffer;
    uint32_t *data;
    int width, height, stride;
    uint32_t format;
    size_t size;
    int busy;
};

static int shmbuf_create(struct shmbuf *b, struct wl_shm *shm, int width, int height, int stride, uint32_t format) {
    b->size = (size_t)stride * (size_t)height;
    b->format = format;
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
    // Where the output shown is in the box's layout, in logical pixels (xdg-output; it follows a move
    // or a new mode). Marks are in layout coordinates: this is what maps them onto the view.
    struct zxdg_output_manager_v1 *xdg_manager;
    int lx, ly, lw, lh;
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

static void xdg_position(void *d, struct zxdg_output_v1 *x, int32_t lx, int32_t ly) { (void)d; (void)x; box.lx = lx; box.ly = ly; }
static void xdg_size(void *d, struct zxdg_output_v1 *x, int32_t w, int32_t h) {
    (void)d; (void)x;
    if (w > 0 && h > 0) { box.lw = w; box.lh = h; }
}
static void xdg_done(void *d, struct zxdg_output_v1 *x) { (void)d; (void)x; }
static void xdg_name(void *d, struct zxdg_output_v1 *x, const char *n) { (void)d; (void)x; (void)n; }
static void xdg_description(void *d, struct zxdg_output_v1 *x, const char *s) { (void)d; (void)x; (void)s; }
static const struct zxdg_output_v1_listener xdg_listener = {xdg_position, xdg_size, xdg_done, xdg_name, xdg_description};

static void box_global(void *data, struct wl_registry *reg, uint32_t name, const char *iface, uint32_t version) {
    (void)data;
    if (!strcmp(iface, zxdg_output_manager_v1_interface.name)) {
        box.xdg_manager = wl_registry_bind(reg, name, &zxdg_output_manager_v1_interface, version < 2 ? version : 2);
    } else if (!strcmp(iface, wl_shm_interface.name)) {
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
    // A new size or format (#109: a buffer of the old format failed every copy, silently): a new buffer.
    if (box.buf.buffer && (box.buf.width != box.width || box.buf.height != box.height || box.buf.stride != box.stride ||
                           box.buf.format != box.format))
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
        fprintf(stderr, "omabox-peek: the box sent a frame it cannot have (format %#x, %ux%u, stride %u; it reads "
                        "XRGB8888, ARGB8888, XBGR8888 and ABGR8888 only, not a 10-bit output's)\n", format, w, h, stride);
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
    struct wl_subcompositor *subcompositor;
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
    (void)data;
    if (!strcmp(iface, wl_compositor_interface.name))
        host.compositor = wl_registry_bind(reg, name, &wl_compositor_interface, version < 4 ? version : 4);
    else if (!strcmp(iface, wl_subcompositor_interface.name))
        host.subcompositor = wl_registry_bind(reg, name, &wl_subcompositor_interface, 1);
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

// Where a sw x sh frame goes in a W x H window: the largest sw:sh rectangle inside, centred.
static void fit(int W, int H, int sw, int sh, int *dw, int *dh, int *ox, int *oy) {
    *dw = W; *dh = (int)((int64_t)W * sh / sw);
    if (*dh > H) { *dh = H; *dw = (int)((int64_t)H * sw / sh); }
    *ox = (W - *dw) / 2; *oy = (H - *dh) / 2;
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
    int dw, dh, ox, oy;
    fit(W, H, sw, sh, &dw, &dh, &ox, &oy);
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

// ---- marks: what the agent does, drawn over the view ------------------------------------------------
//
// `omabox click/pointer/keys` append one line per command to <box dir>/marks while a peek window is
// open (NOTES finding 85). The box cannot reach that file, so it can neither forge nor read marks.
//   ptr WxH (move X Y | click [BTN] | down BTN | up BTN | scroll DY | sleep MS)...
//   combo KEY  |  text TEXT  |  secret N
// WxH is the box's layout (all its monitors), X Y a point in it. A window shows the pointer and the
// clicks that fall on its own output only, at that output's place and scale (finding 233); key
// captions show in every window of the box. Without xdg-output the view stands for all of WxH.
// Any other line is ignored whole. Marks are drawn here only, never into the box's frames, in a
// subsurface of the window with frame callbacks of its own, and only while one is on show: a ring
// that glides to the pointer, a ripple per click, key captions at the bottom; all gone ~3 s after
// the last action. With nothing on show, or the window hidden, they cost nothing.

#define MARK_FADE 2500    // ms: fully shown until then,
#define MARK_GONE 3000    // faded out by then
#define GLIDE_MS 120
#define RIPPLE_MS 600
#define MERGE_MS 1500     // keys this close together share a caption
#define PILL_CHARS 40     // shown at most (the end of it)
#define PILL_BUF 200
#define NPILLS 3
#define NRIPPLES 8
#define ACCENT 0xff3cc8   // rgb; the suite looks for it

struct pill { char text[PILL_BUF + 1]; int len, apart; int64_t t; };
struct area { int x, y, w, h; };
struct areas { int n, whole; struct area a[32]; };   // what a buffer has drawn in it
struct ripple { double x, y; int64_t t; const char *tag; };

static struct {
    int fd, ifd;
    off_t off;
    char line[1024];
    size_t len;
    int skip;                     // in a line too long to be one: dropped up to its newline
    double fx, fy, tx, ty;        // the pointer glides from f to t (layout pixels)
    int64_t moved, active;        // when that glide started; the last pointer command
    int have_ptr;
    int ew, eh;                   // the layout's size, as the last ptr line gave it
    struct ripple ripples[NRIPPLES];
    int next_ripple;
    struct pill pills[NPILLS];
    int npills;
    struct wl_surface *surface;
    struct shmbuf bufs[2];
    struct areas drawn[2];
    int last;                     // the buffer shown now
    int waiting, mapped, dirty;
} marks = {.fd = -1, .ifd = -1};

// A whole decimal number in [lo, hi].
static int mark_num(const char *s, long lo, long hi, long *out) {
    char *end;
    if (!s || !((*s >= '0' && *s <= '9') || *s == '-')) return 0;
    errno = 0;
    long v = strtol(s, &end, 10);
    if (errno || *end || v < lo || v > hi) return 0;
    *out = v;
    return 1;
}
static int is_btn(const char *s) { return !strcmp(s, "left") || !strcmp(s, "right") || !strcmp(s, "middle"); }

static void ptr_pos(int64_t now, double *x, double *y) {
    double k = (double)(now - marks.moved) / GLIDE_MS;
    k = k < 0 ? 1 : k > 1 ? 0 : 1 - k;
    k = 1 - k * k * k;   // eased out
    *x = marks.fx + (marks.tx - marks.fx) * k;
    *y = marks.fy + (marks.ty - marks.fy) * k;
}

// ptr WxH COMMANDS (tools/pointer's), checked whole before any of it shows.
static int mark_ptr(char *rest, int64_t now) {
    char *save = NULL, *tok = strtok_r(rest, " ", &save), *x = tok ? strchr(tok, 'x') : NULL;
    long w, h, v;
    if (!x) return 0;
    *x = 0;
    if (!mark_num(tok, 1, 65536, &w) || !mark_num(x + 1, 1, 65536, &h)) return 0;
    char *t[64];
    int n = 0;
    while ((tok = strtok_r(NULL, " ", &save)) && n < 64) t[n++] = tok;
    if (!n || tok) return 0;
    for (int i = 0; i < n; i++) {
        if (!strcmp(t[i], "move")) {
            if (i + 2 >= n || !mark_num(t[i + 1], 0, w - 1, &v) || !mark_num(t[i + 2], 0, h - 1, &v)) return 0;
            i += 2;
        } else if (!strcmp(t[i], "click")) {
            if (i + 1 < n && is_btn(t[i + 1])) i++;
        } else if (!strcmp(t[i], "down") || !strcmp(t[i], "up")) {
            if (i + 1 < n && is_btn(t[i + 1])) i++;   // the button is optional: left
        } else if (!strcmp(t[i], "pause")) {
        } else if (!strcmp(t[i], "scroll")) {
            char *end;
            if (i + 1 >= n || !*t[i + 1]) return 0;
            double dy = strtod(t[++i], &end);
            if (*end || !(dy >= -10000 && dy <= 10000)) return 0;
        } else if (!strcmp(t[i], "sleep")) {
            if (i + 1 >= n || !mark_num(t[++i], 0, 600000, &v)) return 0;
        } else return 0;
    }
    // A box's cursor starts centred: before the first pointer line, the centre of its layout.
    if (!marks.ew) { marks.fx = marks.tx = (double)w / 2; marks.fy = marks.ty = (double)h / 2; }
    marks.ew = (int)w; marks.eh = (int)h;
    double px, py;
    ptr_pos(now, &px, &py);
    double cx = marks.tx, cy = marks.ty;   // where the box's pointer is, as far as we know
    int moved = 0;
    for (int i = 0; i < n; i++) {
        if (!strcmp(t[i], "move")) {
            mark_num(t[i + 1], 0, w - 1, &v); cx = (double)v + 0.5;
            mark_num(t[i + 2], 0, h - 1, &v); cy = (double)v + 0.5;
            moved = 1;
            i += 2;
        } else if (!strcmp(t[i], "click") || !strcmp(t[i], "down")) {
            const char *b = i + 1 < n && is_btn(t[i + 1]) ? t[++i] : "left";
            marks.ripples[marks.next_ripple] = (struct ripple){cx, cy, now, !strcmp(b, "left") ? NULL : !strcmp(b, "right") ? "right" : "middle"};
            marks.next_ripple = (marks.next_ripple + 1) % NRIPPLES;
        } else if (!strcmp(t[i], "up")) {
            if (i + 1 < n && is_btn(t[i + 1])) i++;   // nothing to show
        } else if (strcmp(t[i], "pause")) {
            i++;   // scroll DY, sleep MS: nothing to show
        }
    }
    if (moved) { marks.fx = px; marks.fy = py; marks.tx = cx; marks.ty = cy; marks.moved = now; }
    marks.have_ptr = 1;
    marks.active = now;
    return 1;
}

// Captions are ASCII (all the font has): any other character shows as one '?'.
static void pill_add(struct pill *p, const char *s) {
    for (const unsigned char *c = (const unsigned char *)s; *c; c++) {
        char out = *c >= 0x20 && *c < 0x7f ? (char)*c : *c >= 0xc0 ? '?' : 0;
        if (!out) continue;
        if (p->len == PILL_BUF) { memmove(p->text, p->text + 1, PILL_BUF - 1); p->len--; }
        p->text[p->len++] = out;
    }
    p->text[p->len] = 0;
}
// Text runs on; a combo or a secret stands apart from what is next to it.
static void caption(const char *s, int apart, int64_t now) {
    struct pill *p = marks.npills ? &marks.pills[marks.npills - 1] : NULL;
    if (!p || now - p->t >= MERGE_MS) {
        if (marks.npills == NPILLS) memmove(marks.pills, marks.pills + 1, sizeof(marks.pills[0]) * --marks.npills);
        p = &marks.pills[marks.npills++];
        p->len = 0;
        p->text[0] = 0;
    } else if (apart || p->apart) {
        pill_add(p, " ");
    }
    pill_add(p, s);
    p->apart = apart;
    p->t = now;
}

static void mark_line(char *l, size_t len) {
    if (strlen(l) != len) return;   // a NUL in it
    for (size_t i = 0; i < len; i++)
        if ((unsigned char)l[i] < 0x20 || l[i] == 0x7f) return;
    int64_t now = now_ms();
    long v;
    if (!strncmp(l, "ptr ", 4)) {
        if (!mark_ptr(l + 4, now)) return;
    } else if (!strncmp(l, "combo ", 6)) {
        if (!l[6] || strchr(l + 6, ' ') || len - 6 > 64) return;
        caption(l + 6, 1, now);
    } else if (!strncmp(l, "text ", 5)) {
        size_t chars = 0;
        for (const unsigned char *c = (const unsigned char *)l + 5; *c; c++) chars += (*c & 0xc0) != 0x80;
        if (!chars || chars > PILL_BUF) return;
        caption(l + 5, 0, now);
    } else if (!strncmp(l, "secret ", 7)) {
        if (!mark_num(l + 7, 1, 100000, &v)) return;
        char stars[17];
        const size_t n = v < 16 ? (size_t)v : 16;   // what a password field shows, at most 16
        memset(stars, '*', n);
        stars[n] = 0;
        caption(stars, 1, now);
    } else return;
    marks.dirty = 1;
}

static void read_marks(void) {
    struct stat st;
    if (fstat(marks.fd, &st) < 0) return;
    if (st.st_size < marks.off) { marks.off = 0; marks.len = 0; marks.skip = 0; }   // emptied at its size cap
    char chunk[4096];
    ssize_t n;
    while ((n = pread(marks.fd, chunk, sizeof(chunk), marks.off)) > 0) {
        marks.off += n;
        for (ssize_t i = 0; i < n; i++) {
            if (chunk[i] == '\n') {
                if (!marks.skip) { marks.line[marks.len] = 0; mark_line(marks.line, marks.len); }
                marks.len = 0; marks.skip = 0;
            } else if (!marks.skip) {
                if (marks.len + 1 < sizeof(marks.line)) marks.line[marks.len++] = chunk[i];
                else { marks.skip = 1; marks.len = 0; }
            }
        }
    }
}

// Only what is written from now on: the file is read from its current end.
static void open_marks(const char *path) {
    marks.fd = open(path, O_RDONLY | O_CREAT | O_NOFOLLOW | O_CLOEXEC, 0600);
    struct stat st;
    if (marks.fd < 0 || fstat(marks.fd, &st) < 0 || !S_ISREG(st.st_mode)) {
        fprintf(stderr, "omabox-peek: no marks: %s is not a file\n", path);
        if (marks.fd >= 0) close(marks.fd);
        marks.fd = -1;
        return;
    }
    marks.off = st.st_size;
    char self[64];
    snprintf(self, sizeof(self), "/proc/self/fd/%d", marks.fd);   // the file opened, whatever has its name later
    marks.ifd = inotify_init1(IN_NONBLOCK | IN_CLOEXEC);
    if (marks.ifd < 0 || inotify_add_watch(marks.ifd, self, IN_MODIFY) < 0) {
        fprintf(stderr, "omabox-peek: no marks: cannot watch %s\n", path);
        close(marks.fd);
        if (marks.ifd >= 0) close(marks.ifd);
        marks.fd = marks.ifd = -1;
    }
}

// Drawing, into a premultiplied ARGB buffer. Only the areas drawn are cleared and damaged, so a
// large window does not cost a full-size upload a frame.
static struct areas *drawing;
static void touched(int x0, int y0, int x1, int y1) {
    if (x1 <= x0 || y1 <= y0) return;
    if (drawing->n == (int)(sizeof(drawing->a) / sizeof(drawing->a[0]))) { drawing->whole = 1; return; }
    drawing->a[drawing->n++] = (struct area){x0, y0, x1 - x0, y1 - y0};
}
static float root_of(float x) {   // no libm
    if (x <= 0) return 0;
    union { float f; uint32_t i; } u = {x};
    u.i = (u.i >> 1) + 0x1fc00000;   // a first guess, then Newton
    for (int k = 0; k < 3; k++) u.f = 0.5f * (u.f + x / u.f);
    return u.f;
}
static void blend(uint32_t *p, uint32_t rgb, int a) {
    if (a <= 0) return;
    if (a > 255) a = 255;
    const uint32_t d = *p, inv = 255 - (uint32_t)a;
    uint32_t out = ((uint32_t)a + (d >> 24) * inv / 255) << 24;
    for (int s = 0; s < 24; s += 8) out |= (((rgb >> s & 255) * (uint32_t)a + (d >> s & 255) * inv) / 255) << s;
    *p = out;
}
// A ring of radius r and half width hw around (cx, cy), antialiased; r = 0 is a disc.
static void ring(struct shmbuf *b, float cx, float cy, float r, float hw, uint32_t rgb, int alpha) {
    int x0 = (int)(cx - r - hw) - 1, x1 = (int)(cx + r + hw) + 2, y0 = (int)(cy - r - hw) - 1, y1 = (int)(cy + r + hw) + 2;
    if (x0 < 0) x0 = 0;
    if (y0 < 0) y0 = 0;
    if (x1 > b->width) x1 = b->width;
    if (y1 > b->height) y1 = b->height;
    touched(x0, y0, x1, y1);
    for (int y = y0; y < y1; y++)
        for (int x = x0; x < x1; x++) {
            float dx = (float)x + 0.5f - cx, dy = (float)y + 0.5f - cy, d = root_of(dx * dx + dy * dy) - r;
            float cov = hw + 0.5f - (d < 0 ? -d : d);
            if (cov > 0) blend(&b->data[(size_t)y * (size_t)b->width + (size_t)x], rgb, (int)((float)alpha * (cov > 1 ? 1 : cov)));
        }
}
static void rect(struct shmbuf *b, int x0, int y0, int w, int h, uint32_t rgb, int alpha) {
    for (int y = y0 < 0 ? 0 : y0; y < y0 + h && y < b->height; y++)
        for (int x = x0 < 0 ? 0 : x0; x < x0 + w && x < b->width; x++)
            blend(&b->data[(size_t)y * (size_t)b->width + (size_t)x], rgb, alpha);
}
// n characters of s on a dark plate with an accent edge, font8x8 at scale.
static void plate(struct shmbuf *b, int x, int y, const char *s, int n, int scale, int alpha) {
    const int px = 5 * scale, py = 3 * scale, w = n * 8 * scale + 2 * px, h = 8 * scale + 2 * py;
    touched(x < 0 ? 0 : x, y < 0 ? 0 : y, x + w < b->width ? x + w : b->width, y + h < b->height ? y + h : b->height);
    rect(b, x, y, w, h, ACCENT, alpha);
    rect(b, x + 2, y + 2, w - 4, h - 4, 0x16161e, alpha * 9 / 10);
    for (int i = 0; i < n; i++) {
        if (s[i] < 0x20 || s[i] > 0x7e) continue;
        const unsigned char *g = font8x8[s[i] - 0x20];
        for (int row = 0; row < 8; row++)
            for (int col = 0; col < 8; col++)
                if (g[row] >> col & 1) rect(b, x + px + (i * 8 + col) * scale, y + py + row * scale, scale, scale, 0xffffff, alpha);
    }
}
static int fade(int64_t age) {
    return age < MARK_FADE ? 255 : age < MARK_GONE ? (int)(255 * (MARK_GONE - age) / (MARK_GONE - MARK_FADE)) : 0;
}

// A layout point on this window's output: 1, and where it is in the window (the view is dw x dh at
// ox, oy); 0 when it is on another monitor.
static int on_view(double x, double y, int dw, int dh, int ox, int oy, float *wx, float *wy) {
    double rx = 0, ry = 0, rw = marks.ew, rh = marks.eh;
    if (box.lw > 0) { rx = box.lx; ry = box.ly; rw = box.lw; rh = box.lh; }
    if (rw <= 0 || rh <= 0 || x < rx || y < ry || x >= rx + rw || y >= ry + rh) return 0;
    *wx = (float)(ox + (x - rx) / rw * dw);
    *wy = (float)(oy + (y - ry) / rh * dh);
    return 1;
}

static void marks_frame_done(void *data, struct wl_callback *cb, uint32_t t) { (void)data; (void)t; wl_callback_destroy(cb); marks.waiting = 0; }
static const struct wl_callback_listener marks_frame_listener = {marks_frame_done};

// Draw what is on show, or unmap the subsurface once nothing is. 0: no free buffer, try again soon.
static int draw_marks(int64_t now) {
    int dw, dh, ox, oy;
    float x, y;
    fit(host.width, host.height, box.buf.width, box.buf.height, &dw, &dh, &ox, &oy);
    // On show here: the pointer while it is on this output (or gliding, onto it maybe), the clicks
    // made on it, the captions.
    double px, py;
    ptr_pos(now, &px, &py);
    const int ptr_on = marks.have_ptr && fade(now - marks.active) > 0;
    int on = ptr_on && (now - marks.moved < GLIDE_MS || on_view(px, py, dw, dh, ox, oy, &x, &y));
    for (int i = 0; i < NRIPPLES; i++) {
        const struct ripple *r = &marks.ripples[i];
        if (r->t && fade(now - r->t) > 0 && on_view(r->x, r->y, dw, dh, ox, oy, &x, &y)) on = 1;
    }
    for (int i = 0; i < marks.npills; i++)
        if (fade(now - marks.pills[i].t) > 0) on = 1;
    if (!on) {
        if (marks.mapped) { wl_surface_attach(marks.surface, NULL, 0, 0); wl_surface_commit(marks.surface); marks.mapped = 0; }
        marks.npills = 0;
        return 1;
    }
    int bi = !marks.bufs[0].busy ? 0 : !marks.bufs[1].busy ? 1 : -1;
    if (bi < 0) return 0;
    struct shmbuf *b = &marks.bufs[bi];
    int whole = !marks.mapped;   // damage all of it: nothing of ours on screen to compare with
    if (b->buffer && (b->width != host.width || b->height != host.height)) shmbuf_destroy(b);
    if (!b->buffer) {
        if (!shmbuf_create(b, host.shm, host.width, host.height, host.width * 4, WL_SHM_FORMAT_ARGB8888)) exit(1);
        wl_buffer_add_listener(b->buffer, &buffer_listener, b);
        marks.drawn[bi] = (struct areas){0};   // new memory is clear
        whole = 1;
    }
    const struct areas shown = marks.drawn[marks.last];
    drawing = &marks.drawn[bi];
    if (drawing->whole) memset(b->data, 0, b->size);
    else
        for (int i = 0; i < drawing->n; i++)
            for (int y = drawing->a[i].y; y < drawing->a[i].y + drawing->a[i].h; y++)
                memset(b->data + (size_t)y * (size_t)b->width + (size_t)drawing->a[i].x, 0, (size_t)drawing->a[i].w * 4);
    *drawing = (struct areas){0};
    for (int i = 0; i < NRIPPLES; i++) {
        const struct ripple *r = &marks.ripples[i];
        const int64_t age = now - r->t;
        if (!r->t || fade(age) <= 0 || !on_view(r->x, r->y, dw, dh, ox, oy, &x, &y)) continue;
        if (age < RIPPLE_MS) ring(b, x, y, 12.0f + 26.0f * (float)age / RIPPLE_MS, 1.5f, ACCENT, (int)(255 * (RIPPLE_MS - age) / RIPPLE_MS));
        if (r->tag) plate(b, (int)x + 16, (int)y + 14, r->tag, (int)strlen(r->tag), 1, fade(age));
    }
    if (ptr_on && on_view(px, py, dw, dh, ox, oy, &x, &y)) {
        const int a = fade(now - marks.active);
        ring(b, x, y, 12, 3.5f, 0x000000, a / 2);   // a dark edge, for light screens
        ring(b, x, y, 12, 2, ACCENT, a);
        ring(b, x, y, 0, 2, ACCENT, a);
    }
    const int scale = host.width >= 640 ? 2 : 1, ph = 14 * scale;
    int room = (host.width - 32 - 10 * scale) / (8 * scale);
    if (room > PILL_CHARS) room = PILL_CHARS;
    for (int i = marks.npills - 1, k = 0; i >= 0 && room > 0; i--, k++) {   // newest at the bottom
        const struct pill *p = &marks.pills[i];
        const int a = fade(now - p->t), n = p->len < room ? p->len : room;
        if (a <= 0 || !n) continue;
        const int w = n * 8 * scale + 10 * scale;
        plate(b, (host.width - w) / 2, host.height - 20 - (k + 1) * ph - k * 6, p->text + p->len - n, n, scale, a);
    }
    wl_surface_attach(marks.surface, b->buffer, 0, 0);
    if (whole || shown.whole || drawing->whole) wl_surface_damage_buffer(marks.surface, 0, 0, b->width, b->height);
    else {
        for (int i = 0; i < shown.n; i++) wl_surface_damage_buffer(marks.surface, shown.a[i].x, shown.a[i].y, shown.a[i].w, shown.a[i].h);
        for (int i = 0; i < drawing->n; i++) wl_surface_damage_buffer(marks.surface, drawing->a[i].x, drawing->a[i].y, drawing->a[i].w, drawing->a[i].h);
    }
    marks.last = bi;
    wl_callback_add_listener(wl_surface_frame(marks.surface), &marks_frame_listener, NULL);
    marks.waiting = 1;
    wl_surface_commit(marks.surface);
    b->busy = 1;
    marks.mapped = 1;
    return 1;
}

// A subsurface over the whole window that takes no input (peek takes none anyway), desynchronized so
// its frames never wait for the view's.
static void marks_surface(void) {
    if (marks.fd < 0) return;
    if (!host.subcompositor) {
        fprintf(stderr, "omabox-peek: no marks: the host has no wl_subcompositor\n");
        return;
    }
    marks.surface = wl_compositor_create_surface(host.compositor);
    struct wl_subsurface *sub = wl_subcompositor_get_subsurface(host.subcompositor, marks.surface, host.surface);
    wl_subsurface_set_position(sub, 0, 0);
    wl_subsurface_set_desync(sub);
    struct wl_region *none = wl_compositor_create_region(host.compositor);
    wl_surface_set_input_region(marks.surface, none);
    wl_region_destroy(none);
    wl_surface_commit(marks.surface);
}

// Dispatch whatever is readable on the two connections (and the marks file), waiting up to timeout ms.
// Returns 0 when a connection is gone.
static int pump(int timeout) {
    struct wl_display *d[2] = {box.display, host.display};
    struct pollfd fds[3];
    fds[2] = (struct pollfd){.fd = marks.ifd, .events = POLLIN};   // -1 (no marks): poll skips it
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
    int n = poll(fds, 3, timeout);
    if (n > 0 && (fds[2].revents & POLLIN)) {
        char ev[4096];
        while (read(marks.ifd, ev, sizeof(ev)) > 0) {}
        read_marks();
    }
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
    const char *box_socket = NULL, *title = "omabox peek", *marks_path = NULL;
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
        else if (!strcmp(argv[i], "--marks") && i + 1 < argc) marks_path = argv[++i];
        else { fprintf(stderr, "usage: omabox-peek --box SOCKET [--output NAME] [--fps N] [--title TEXT] [--marks FILE]\n"); return 2; }
    }
    if (!box_socket) { fprintf(stderr, "omabox-peek: --box is required\n"); return 2; }
    if (marks_path) open_marks(marks_path);
    // libwayland prefers an inherited WAYLAND_SOCKET over both the box's socket and WAYLAND_DISPLAY.
    unsetenv("WAYLAND_SOCKET");

    box.display = wl_display_connect(box_socket);
    if (!box.display) { fprintf(stderr, "omabox-peek: cannot connect to the box at %s\n", box_socket); return 1; }
    wl_registry_add_listener(wl_display_get_registry(box.display), &box_registry, NULL);
    // With a deadline (finding 181): a box whose compositor is stopped accepts the connection and never
    // answers, and a peek hung here with no window, which the next `omabox peek` took for the window.
    int rt = omabox_roundtrip(box.display, 10000);   // globals
    if (rt == 0) rt = omabox_roundtrip(box.display, 10000);   // output names
    if (rt < 0) {
        fprintf(stderr, rt == -2 ? "omabox-peek: the box did not answer in 10 s (its Hyprland stopped?)\n" : "omabox-peek: lost the box\n");
        return 1;
    }
    if (!box.shm || !box.manager || !box.output) {
        fprintf(stderr, "omabox-peek: the box offers no screencopy, shm or output%s%s\n",
                box.want_output ? " named " : "", box.want_output ? box.want_output : "");
        return 1;
    }
    if (box.xdg_manager) {   // the output's place in the layout, for the marks
        struct zxdg_output_v1 *x = zxdg_output_manager_v1_get_xdg_output(box.xdg_manager, box.output);
        zxdg_output_v1_add_listener(x, &xdg_listener, NULL);
        if (omabox_roundtrip(box.display, 10000) < 0) { fprintf(stderr, "omabox-peek: the box did not answer\n"); return 1; }
    }

    host.display = wl_display_connect(NULL);
    if (!host.display) { fprintf(stderr, "omabox-peek: cannot connect to the host display\n"); return 1; }
    wl_registry_add_listener(wl_display_get_registry(host.display), &host_registry, NULL);
    if (wl_display_roundtrip(host.display) < 0) { fprintf(stderr, "omabox-peek: lost the host display\n"); return 1; }
    if (!host.compositor || !host.shm || !host.wm) { fprintf(stderr, "omabox-peek: host lacks compositor, shm or xdg_wm_base\n"); return 1; }

    // First frame before the window, so the window can open at a sensible size.
    // Within 10 s (#109): a box that never renders left a peek with no window, which omabox then took
    // for the box's peek window.
    capture();
    const int64_t first_until = now_ms() + 10000;
    while (!box.have_frame && box.frame) {
        while (wl_display_prepare_read(box.display) != 0)
            if (wl_display_dispatch_pending(box.display) < 0) return 1;
        if (box.have_frame || !box.frame) { wl_display_cancel_read(box.display); break; }
        int64_t left = first_until - now_ms();
        if (left <= 0) {
            wl_display_cancel_read(box.display);
            fprintf(stderr, "omabox-peek: the box sent no frame in 10 s\n");
            return 1;
        }
        if (wl_display_flush(box.display) < 0 && errno != EAGAIN) { wl_display_cancel_read(box.display); return 1; }
        struct pollfd pf = {.fd = wl_display_get_fd(box.display), .events = POLLIN};
        int n = poll(&pf, 1, (int)left);
        if (n > 0 && (pf.revents & POLLIN)) { if (wl_display_read_events(box.display) < 0) return 1; }
        else wl_display_cancel_read(box.display);
        if (n > 0 && (pf.revents & (POLLERR | POLLHUP))) return 1;
        if (wl_display_dispatch_pending(box.display) < 0) return 1;
    }
    if (!box.have_frame) { fprintf(stderr, "omabox-peek: the box refused a screen capture\n"); return 1; }

    host.surface = wl_compositor_create_surface(host.compositor);
    host.xdg_surface = xdg_wm_base_get_xdg_surface(host.wm, host.surface);
    xdg_surface_add_listener(host.xdg_surface, &xdg_surface_listener, NULL);
    host.toplevel = xdg_surface_get_toplevel(host.xdg_surface);
    xdg_toplevel_add_listener(host.toplevel, &toplevel_listener, NULL);
    xdg_toplevel_set_title(host.toplevel, title);
    xdg_toplevel_set_app_id(host.toplevel, "omabox-peek");
    marks_surface();
    wl_surface_commit(host.surface);

    const int64_t interval = 1000 / fps;
    int64_t next = 0;
    while (!host.closed) {
        if (host.configured && box.have_frame) { draw(); box.have_frame = 0; }
        int64_t t = now_ms();
        if (!box.frame && !host.waiting && t >= next) { capture(); next = t + interval; }
        int timeout = box.frame || host.waiting ? 100 : (int)(next - t > 0 ? next - t : 0);
        // Marks: a frame whenever the last one is shown, while any is on show; then nothing at all.
        if (host.configured && marks.surface && !marks.waiting && (marks.dirty || marks.mapped)) {
            marks.dirty = 0;
            if (!draw_marks(t)) { marks.dirty = 1; if (timeout > 10) timeout = 10; }   // both buffers still shown
        }
        if (!pump(timeout)) break;
    }
    return 0;
}
