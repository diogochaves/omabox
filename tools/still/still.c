// omabox-still: wait until the screen of the Wayland display in $WAYLAND_DISPLAY holds still, or changes.
//
//   omabox-still (still | change | settle) [--region X,Y,W,H] [--ignore X,Y,W,H]... [--mask X,Y,W,H]... [--quiet MS]
//                [--start MS] [--timeout MS] [--first MS] [--strict] [--tied]
//
// still:  satisfied once nothing significant has changed for --quiet ms (default 300).
// change: satisfied at the first significant change.
// settle: a significant change within --start ms (default 2000), then --quiet ms without one: what
//         follows an action (omabox keys/click/run --wait). No change at all is not settled.
//
// The screen is every output (monitor) the compositor has, in layout coordinates: each output's place
// and logical size come from xdg-output, so a monitor at 1920,0 or at scale 1.6 is where `hyprctl
// monitors`, grim -g and omabox's clicks put it (finding 231). It watches the outputs --region touches
// (default: all of them), and every rectangle, in and out, is in layout coordinates.
//
// It asks the compositor for frames with wlr-screencopy's copy_with_damage, which only completes when
// something was drawn: an idle screen costs nothing while it waits. Every frame is compared with the
// one before of its output, pixel by pixel, inside --region. A change is significant
// unless it is 4 pixels or less in width or height (a text caret blinking) or lies inside an --ignore
// rectangle (where the software cursor is: Hyprland hides it on a key press) or a --mask one (the
// caller's: an animation that never stops, #131); --strict counts all but a --mask.
// Before it answers "nothing changed" or "still", one plain copy (which never waits) of each output is
// compared too, so a change drawn between two requests is not missed.
//
// Prints `ready WxH area=N` once it has the first frame of each output (the caller acts after that):
// the layout box around the outputs watched and how much of it they cover, then one line:
//   satisfied|unsatisfied|unknown REASON t=MS first=MS last=MS change=X,Y,W,H ignored=X,Y,W,H why=WHY
//   late=X,Y,W,H frames=N
// (times from `ready`, - when there is none; late: all the significant changes in the second half of
// --timeout, one box around them: what kept a wait from its answer). Exit 0 satisfied, 124 unsatisfied, 1 unknown, 2 usage.
// unknown: no first frame within --first ms (default 2000: an interactive box's hidden window is not
// rendered), the compositor went away, it did not answer its first round trip within 10 s (`hung`: alive
// but stopped or deadlocked, finding 181), or with --tied, stdin closed (the caller is gone).
#define _GNU_SOURCE
#include <errno.h>
#include <math.h>
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
#include "xdg-output-unstable-v1-client-protocol.h"
#include "../common/box.h"
#include "../common/roundtrip.h"

#define DRM_FORMAT_XBGR8888 0x34324258
#define DRM_FORMAT_ABGR8888 0x34324241
#define MAX_SIDE 16384
#define MAX_IGNORE 48   // drag --wait: the cursor along its path (omabox path_rects: 30), and 16 --mask
#define MAX_OUTPUTS 16
#define THIN 4   // a change this thin or thinner is a caret, not a change

struct rect { int x, y, w, h; };

static void usage(void) {
    fprintf(stderr, "usage: omabox-still (still|change|settle) [--region X,Y,W,H] [--ignore X,Y,W,H]... [--mask X,Y,W,H]...\n"
                    "                    [--quiet MS]\n"
                    "                    [--start MS] [--timeout MS] [--first MS] [--strict] [--tied]\n");
    exit(2);
}

static long num(const char *s, long lo, long hi) {
    char *end;
    if (!s || !*s) usage();
    long v = strtol(s, &end, 10);
    if (*end || v < lo || v > hi) { fprintf(stderr, "omabox-still: bad number '%s'\n", s); usage(); }
    return v;
}

static struct rect parse_rect(const char *s) {
    struct rect r;
    char tail;
    if (!s || sscanf(s, "%d,%d,%d,%d%c", &r.x, &r.y, &r.w, &r.h, &tail) != 4 || r.w <= 0 || r.h <= 0 ||
        r.w > MAX_SIDE || r.h > MAX_SIDE || r.x < -MAX_SIDE || r.y < -MAX_SIDE || r.x > MAX_SIDE || r.y > MAX_SIDE) {
        fprintf(stderr, "omabox-still: a rectangle is X,Y,W,H, got '%s'\n", s ? s : "");
        usage();
    }
    return r;
}

static int64_t now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

static int meets(const struct rect *a, const struct rect *b) {
    return a->x < b->x + b->w && b->x < a->x + a->w && a->y < b->y + b->h && b->y < a->y + a->h;
}

// ---- options and state ------------------------------------------------------------------------------

enum mode { STILL, CHANGE, SETTLE };
static enum mode mode;
static long quiet = 300, start = 2000, timeout = 10000, first = 2000;
static int strict, tied, have_region;
static struct rect region, ignore[MAX_IGNORE];   // layout coordinates
static int nignore, masked[MAX_IGNORE];   // masked: a --mask, which --strict keeps

static int64_t t0 = -1;               // when `ready` was printed
static int64_t first_sig = -1, last_sig = -1;   // ms since t0
static struct rect sig_box, ign_box;  // the last significant change, the last ignored one (layout)
static struct rect late_box;          // around every significant change after timeout/2
static int nlate;
static const char *ign_why;
static long frames;

// ---- outputs and capture ---------------------------------------------------------------------------

struct shmbuf {
    struct wl_buffer *buffer;
    uint32_t *data;
    size_t size;
};

// One output: where it is in the layout, its two buffers (the frame before, prev, and the one being
// captured), and its frame in flight. lw/lh 0: no xdg-output, the output's pixels are the layout's.
struct output {
    struct wl_output *wl;
    struct zxdg_output_v1 *xdg;
    uint32_t name;
    int lx, ly, lw, lh;
    int watched, gone;
    struct shmbuf bufs[2];
    int prev, have_prev;
    int width, height, stride, y_invert;   // of the allocated buffers
    uint32_t format;
    int fw, fh, fstride, finvert;          // of the frame being captured
    uint32_t fformat;
    struct zwlr_screencopy_frame_v1 *frame;
    int frame_done, frame_failed, frame_plain;
    struct rect pregion, pignore[MAX_IGNORE];   // region and ignores in this output's pixels
};

static struct wl_display *display;
static struct wl_shm *shm;
static struct zwlr_screencopy_manager_v1 *manager;
static uint32_t manager_version;
static struct zxdg_output_manager_v1 *xdg_manager;
static struct output outs[MAX_OUTPUTS];
static int nouts, started;

// The output's rectangle in the layout.
static struct rect out_rect(const struct output *o) {
    return (struct rect){o->lx, o->ly, o->lw ? o->lw : o->width, o->lh ? o->lh : o->height};
}

// Layout → this output's pixels (rounded outwards, clipped to it), and back.
static double scale_x(const struct output *o) { return o->lw ? (double)o->width / o->lw : 1; }
static double scale_y(const struct output *o) { return o->lh ? (double)o->height / o->lh : 1; }
static struct rect to_pixels(const struct output *o, struct rect r) {
    double sx = scale_x(o), sy = scale_y(o);
    int x0 = (int)floor((r.x - o->lx) * sx), y0 = (int)floor((r.y - o->ly) * sy);
    int x1 = (int)ceil((r.x + r.w - o->lx) * sx), y1 = (int)ceil((r.y + r.h - o->ly) * sy);
    if (x0 < 0) x0 = 0;
    if (y0 < 0) y0 = 0;
    if (x1 > o->width) x1 = o->width;
    if (y1 > o->height) y1 = o->height;
    if (x1 < x0) x1 = x0;
    if (y1 < y0) y1 = y0;
    return (struct rect){x0, y0, x1 - x0, y1 - y0};
}
static struct rect to_layout(const struct output *o, struct rect p) {
    double sx = scale_x(o), sy = scale_y(o);
    int x0 = o->lx + (int)floor(p.x / sx), y0 = o->ly + (int)floor(p.y / sy);
    int x1 = o->lx + (int)ceil((p.x + p.w) / sx), y1 = o->ly + (int)ceil((p.y + p.h) / sy);
    return (struct rect){x0, y0, x1 - x0, y1 - y0};
}
// The region and ignores in pixels: when the buffers are (re)made, and when the output moves.
static void map_rects(struct output *o) {
    o->pregion = to_pixels(o, have_region ? region : out_rect(o));
    for (int i = 0; i < nignore; i++) o->pignore[i] = to_pixels(o, ignore[i]);
}

static int shmbuf_create(struct output *o, struct shmbuf *b) {
    b->size = (size_t)o->stride * (size_t)o->height;
    int fd = memfd_create("omabox-still", MFD_CLOEXEC);
    if (fd < 0) return 0;
    if (ftruncate(fd, (off_t)b->size) < 0) { close(fd); return 0; }
    b->data = mmap(NULL, b->size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (b->data == MAP_FAILED) { b->data = NULL; close(fd); return 0; }
    struct wl_shm_pool *pool = wl_shm_create_pool(shm, fd, (int32_t)b->size);
    b->buffer = wl_shm_pool_create_buffer(pool, 0, o->width, o->height, o->stride, o->format);
    wl_shm_pool_destroy(pool);
    close(fd);
    return 1;
}

static void shmbuf_destroy(struct shmbuf *b) {
    if (b->buffer) wl_buffer_destroy(b->buffer);
    if (b->data) munmap(b->data, b->size);
    memset(b, 0, sizeof(*b));
}

// xdg-output: the output's place and logical size. One that moves once watching has begun is a change
// of all of it (its next frame is compared with nothing).
static void xdg_position(void *data, struct zxdg_output_v1 *x, int32_t lx, int32_t ly) {
    (void)x;
    struct output *o = data;
    if (started && (o->lx != lx || o->ly != ly)) o->have_prev = 0;
    o->lx = lx; o->ly = ly;
    if (o->width) map_rects(o);
}
static void xdg_size(void *data, struct zxdg_output_v1 *x, int32_t w, int32_t h) {
    (void)x;
    struct output *o = data;
    if (w <= 0 || h <= 0) return;
    if (started && (o->lw != w || o->lh != h)) o->have_prev = 0;
    o->lw = w; o->lh = h;
    if (o->width) map_rects(o);
}
static void xdg_done(void *d, struct zxdg_output_v1 *x) { (void)d; (void)x; }
static void xdg_name(void *d, struct zxdg_output_v1 *x, const char *n) { (void)d; (void)x; (void)n; }
static void xdg_description(void *d, struct zxdg_output_v1 *x, const char *s) { (void)d; (void)x; (void)s; }
static const struct zxdg_output_v1_listener xdg_listener = {xdg_position, xdg_size, xdg_done, xdg_name, xdg_description};

static void xdg_bind(struct output *o) {
    if (!xdg_manager || o->xdg) return;
    o->xdg = zxdg_output_manager_v1_get_xdg_output(xdg_manager, o->wl);
    zxdg_output_v1_add_listener(o->xdg, &xdg_listener, o);
}

static void output_gone(struct output *o);

static void global(void *data, struct wl_registry *reg, uint32_t name, const char *iface, uint32_t version) {
    (void)data;
    if (!strcmp(iface, wl_shm_interface.name)) {
        shm = wl_registry_bind(reg, name, &wl_shm_interface, 1);
    } else if (!strcmp(iface, zwlr_screencopy_manager_v1_interface.name)) {
        manager_version = version < 3 ? version : 3;
        manager = wl_registry_bind(reg, name, &zwlr_screencopy_manager_v1_interface, manager_version);
    } else if (!strcmp(iface, zxdg_output_manager_v1_interface.name)) {
        xdg_manager = wl_registry_bind(reg, name, &zxdg_output_manager_v1_interface, version < 2 ? version : 2);
        for (int i = 0; i < nouts; i++) xdg_bind(&outs[i]);
    } else if (!strcmp(iface, wl_output_interface.name) && !started && nouts < MAX_OUTPUTS) {
        // The outputs there when it starts are the screen it watches; one plugged in later is not.
        struct output *o = &outs[nouts++];
        o->name = name;
        o->wl = wl_registry_bind(reg, name, &wl_output_interface, 1);
        xdg_bind(o);
    }
}
static void global_remove(void *data, struct wl_registry *reg, uint32_t name) {
    (void)data; (void)reg;
    for (int i = 0; i < nouts; i++)
        if (outs[i].name == name && !outs[i].gone) output_gone(&outs[i]);
}
static const struct wl_registry_listener registry_listener = {global, global_remove};

// The frame's buffer is only known once the compositor describes it: a new size (a mode change) means
// new buffers, and the next frame is a change of the whole output.
static void frame_copy(struct output *o) {
    if (!o->bufs[0].buffer || o->fw != o->width || o->fh != o->height || o->fstride != o->stride || o->fformat != o->format) {
        shmbuf_destroy(&o->bufs[0]); shmbuf_destroy(&o->bufs[1]);
        o->width = o->fw; o->height = o->fh; o->stride = o->fstride; o->format = o->fformat;
        if (!shmbuf_create(o, &o->bufs[0]) || !shmbuf_create(o, &o->bufs[1])) {
            fprintf(stderr, "omabox-still: cannot allocate a capture buffer\n");
            exit(1);
        }
        o->have_prev = 0;
        map_rects(o);
    }
    struct wl_buffer *b = o->bufs[!o->prev].buffer;
    if (o->frame_plain) zwlr_screencopy_frame_v1_copy(o->frame, b);
    else zwlr_screencopy_frame_v1_copy_with_damage(o->frame, b);
}

static void frame_buffer(void *data, struct zwlr_screencopy_frame_v1 *f, uint32_t fmt, uint32_t w, uint32_t h, uint32_t s) {
    (void)f;
    struct output *o = data;
    int ok = fmt == WL_SHM_FORMAT_XRGB8888 || fmt == WL_SHM_FORMAT_ARGB8888 ||
             fmt == DRM_FORMAT_XBGR8888 || fmt == DRM_FORMAT_ABGR8888;
    if (!ok || !w || !h || w > MAX_SIDE || h > MAX_SIDE || s % 4 || s < w * 4 || (uint64_t)s * h > INT32_MAX) {
        fprintf(stderr, "omabox-still: a frame it cannot read (format %#x, %ux%u, stride %u; it reads XRGB8888, "
                        "ARGB8888, XBGR8888 and ABGR8888 only, not a 10-bit output's)\n", fmt, w, h, s);
        exit(1);
    }
    o->fformat = fmt; o->fw = (int)w; o->fh = (int)h; o->fstride = (int)s;
    if (manager_version < 3) frame_copy(o);   // v3 lists every buffer type, then buffer_done
}
static void frame_flags(void *data, struct zwlr_screencopy_frame_v1 *f, uint32_t flags) {
    (void)f;
    struct output *o = data;
    o->finvert = flags & ZWLR_SCREENCOPY_FRAME_V1_FLAGS_Y_INVERT;
}
static void frame_ready(void *data, struct zwlr_screencopy_frame_v1 *f, uint32_t a, uint32_t b, uint32_t c) {
    (void)a; (void)b; (void)c;
    struct output *o = data;
    zwlr_screencopy_frame_v1_destroy(f);
    o->frame = NULL;
    o->y_invert = o->finvert;
    o->frame_done = 1;
}
static void frame_failed(void *data, struct zwlr_screencopy_frame_v1 *f) {
    struct output *o = data;
    zwlr_screencopy_frame_v1_destroy(f);
    o->frame = NULL;
    o->frame_failed = 1;
}
static void frame_damage(void *d, struct zwlr_screencopy_frame_v1 *f, uint32_t x, uint32_t y, uint32_t w, uint32_t h) {
    (void)d; (void)f; (void)x; (void)y; (void)w; (void)h;   // always the whole output in Hyprland: the pixels tell
}
static void frame_dmabuf(void *d, struct zwlr_screencopy_frame_v1 *f, uint32_t fmt, uint32_t w, uint32_t h) {
    (void)d; (void)f; (void)fmt; (void)w; (void)h;
}
static void frame_buffer_done(void *data, struct zwlr_screencopy_frame_v1 *f) { (void)f; frame_copy(data); }
static const struct zwlr_screencopy_frame_v1_listener frame_listener = {
    frame_buffer, frame_flags, frame_ready, frame_failed, frame_damage, frame_dmabuf, frame_buffer_done};

static void cancel_capture(struct output *o) {
    if (o->frame) zwlr_screencopy_frame_v1_destroy(o->frame);
    o->frame = NULL;
}

// plain: the current frame at once; else the next one drawn with damage (none while nothing changes).
static void capture(struct output *o, int plain) {
    cancel_capture(o);
    o->frame_done = o->frame_failed = 0;
    o->frame_plain = plain;
    o->frame = zwlr_screencopy_manager_v1_capture_output(manager, 1, o->wl);   // the cursor is in the frame anyway
    zwlr_screencopy_frame_v1_add_listener(o->frame, &frame_listener, o);
}

// ---- comparing frames ------------------------------------------------------------------------------

static void grow(struct rect *r, int *n, int x, int y) {
    if (!*n) { *r = (struct rect){x, y, 1, 1}; *n = 1; return; }
    if (x < r->x) { r->w += r->x - x; r->x = x; } else if (x >= r->x + r->w) r->w = x - r->x + 1;
    if (y < r->y) { r->h += r->y - y; r->y = y; } else if (y >= r->y + r->h) r->h = y - r->y + 1;
}

// The --ignore rectangle a pixel of this output is in (its index), or -1.
static int ignored(const struct output *o, int x, int y) {
    for (int i = 0; i < nignore; i++) {
        const struct rect *g = &o->pignore[i];
        if (x >= g->x && x < g->x + g->w && y >= g->y && y < g->y + g->h) return i;
    }
    return -1;
}

// Compare the output's new frame (cur) with its prev inside the region. Returns 1 for a significant
// change (its box in sig_box, in the layout), 0 otherwise (a smaller one goes to ign_box/ign_why).
static int compare(struct output *o) {
    int cur = !o->prev;
    if (!o->have_prev) {
        sig_box = out_rect(o);
        // Moved out of the region watched (another monitor's add or remove re-laid it out): nothing of
        // the region changed, so not a change of it.
        if (have_region && !meets(&sig_box, &region)) return 0;
        if (have_region) {
            struct rect r = region, s = sig_box;
            int x1 = s.x + s.w < r.x + r.w ? s.x + s.w : r.x + r.w, y1 = s.y + s.h < r.y + r.h ? s.y + s.h : r.y + r.h;
            sig_box.x = s.x > r.x ? s.x : r.x; sig_box.y = s.y > r.y ? s.y : r.y;
            sig_box.w = x1 - sig_box.x; sig_box.h = y1 - sig_box.y;
        }
        return 1;
    }
    const struct rect r = o->pregion;
    const int x0 = r.x, y0 = r.y, x1 = r.x + r.w, y1 = r.y + r.h;
    if (x0 >= x1 || y0 >= y1) return 0;
    // Each ignored rectangle on its own: a box around two of them would name the screen between.
    struct rect s = {0}, ig[MAX_IGNORE] = {{0}};
    int ns = 0, ni[MAX_IGNORE] = {0};
    const int words = o->stride / 4;
    for (int y = y0; y < y1; y++) {
        int row = o->y_invert ? o->height - 1 - y : y;
        const uint32_t *a = o->bufs[o->prev].data + (size_t)row * (size_t)words, *b = o->bufs[cur].data + (size_t)row * (size_t)words;
        if (!memcmp(a + x0, b + x0, (size_t)(x1 - x0) * 4)) continue;
        for (int x = x0; x < x1; x++) {
            if (!((a[x] ^ b[x]) & 0xffffff)) continue;   // colour only: X/alpha bytes may be anything
            int i = ignored(o, x, y);
            if (strict && i >= 0 && !masked[i]) i = -1;
            if (i >= 0) grow(&ig[i], &ni[i], x, y);
            else grow(&s, &ns, x, y);
        }
    }
    for (int i = 0; i < nignore; i++)
        if (ni[i]) { ign_box = to_layout(o, ig[i]); ign_why = masked[i] ? "mask" : "cursor"; }
    if (!ns) return 0;
    // Thin in the layout's pixels: a caret is as thin at scale 2 as at 1.
    s = to_layout(o, s);
    if (!strict && (s.w <= THIN || s.h <= THIN)) { ign_box = s; ign_why = "caret"; return 0; }
    sig_box = s;
    return 1;
}

// ---- main loop -------------------------------------------------------------------------------------

static void rect_str(char *out, size_t n, const struct rect *r, int have) {
    if (have) snprintf(out, n, "%d,%d,%d,%d", r->x, r->y, r->w, r->h);
    else snprintf(out, n, "-");
}

static void finish(const char *result, const char *reason, int code) {
    char t[32] = "-", f[32] = "-", l[32] = "-", s[64], ig[64], la[64];
    if (t0 >= 0) snprintf(t, sizeof(t), "%lld", (long long)(now_ms() - t0));
    if (first_sig >= 0) snprintf(f, sizeof(f), "%lld", (long long)first_sig);
    if (last_sig >= 0) snprintf(l, sizeof(l), "%lld", (long long)last_sig);
    rect_str(s, sizeof(s), &sig_box, last_sig >= 0);
    rect_str(ig, sizeof(ig), &ign_box, ign_why != NULL);
    rect_str(la, sizeof(la), &late_box, nlate);
    printf("%s %s t=%s first=%s last=%s change=%s ignored=%s why=%s late=%s frames=%ld\n",
           result, reason, t, f, l, s, ig, ign_why ? ign_why : "-", la, frames);
    fflush(stdout);
    exit(code);
}

// Dispatch what arrives on the display (and notice stdin closing with --tied), waiting up to ms.
static void pump(int ms) {
    struct pollfd fds[2] = {{.fd = wl_display_get_fd(display), .events = POLLIN}, {.fd = 0, .events = POLLIN}};
    while (wl_display_prepare_read(display) != 0)
        if (wl_display_dispatch_pending(display) < 0) finish("unknown", "lost", 1);
    if (wl_display_flush(display) < 0 && errno != EAGAIN) { wl_display_cancel_read(display); finish("unknown", "lost", 1); }
    int n = poll(fds, tied ? 2 : 1, ms < 0 ? 0 : ms);
    if (n > 0 && (fds[0].revents & POLLIN)) {
        if (wl_display_read_events(display) < 0) finish("unknown", "lost", 1);
    } else {
        wl_display_cancel_read(display);
    }
    if (fds[0].revents & (POLLERR | POLLHUP)) finish("unknown", "lost", 1);
    if (wl_display_dispatch_pending(display) < 0) finish("unknown", "lost", 1);
    if (tied && n > 0 && fds[1].revents) {
        char c[64];
        if (read(0, c, sizeof(c)) <= 0) finish("unknown", "cancelled", 1);
    }
}

// The condition as it stands at time t (ms since ready): 1 satisfied, 0 not (yet).
static int satisfied(int64_t t) {
    switch (mode) {
    case CHANGE: return last_sig >= 0;
    case STILL: return t - (last_sig >= 0 ? last_sig : 0) >= quiet;
    case SETTLE: return last_sig >= 0 && t - last_sig >= quiet;
    }
    return 0;
}

// When the answer can next change without a new frame arriving.
static int64_t next_deadline(void) {
    int64_t d = timeout;
    if (mode == STILL || (mode == SETTLE && last_sig >= 0)) {
        int64_t q = (last_sig >= 0 ? last_sig : 0) + quiet;
        if (q < d) d = q;
    } else if (mode == SETTLE && start < d) {
        d = start;
    }
    return d;
}

static void unsatisfied(void) {
    if (mode == STILL || (mode == SETTLE && last_sig >= 0)) finish("unsatisfied", "changing", 124);
    finish("unsatisfied", "nothing", 124);
}

static void significant(int64_t t) {
    if (first_sig < 0) first_sig = t;
    last_sig = t;
    if (2 * t >= timeout) {
        grow(&late_box, &nlate, sig_box.x, sig_box.y);
        grow(&late_box, &nlate, sig_box.x + sig_box.w - 1, sig_box.y + sig_box.h - 1);
    }
}

// A watched output unplugged mid-wait: what it showed is gone, a change of all of it; the rest are
// still watched, and with none left there is nothing to watch.
static void output_gone(struct output *o) {
    o->gone = 1;
    cancel_capture(o);
    if (!started || !o->watched) return;
    o->watched = 0;
    if (t0 >= 0) { sig_box = out_rect(o); significant(now_ms() - t0); }
    int left = 0;
    for (int i = 0; i < nouts; i++) left += outs[i].watched;
    if (!left) finish("unknown", "lost", 1);
}

int main(int argc, char **argv) {
    // Only ever inside a box (omabox runs it there): from a host shell, WAYLAND_DISPLAY is the user's
    // real desktop, and there is nothing of theirs to wait on.
    if (!omabox_inside_box()) {
        fprintf(stderr, "%s: only runs inside an omabox box (use omabox wait)\n", argv[0]);
        return 2;
    }
    unsetenv("WAYLAND_SOCKET");   // only the socket in WAYLAND_DISPLAY, never an inherited connection
    if (argc < 2) usage();
    if (!strcmp(argv[1], "still")) mode = STILL;
    else if (!strcmp(argv[1], "change")) mode = CHANGE;
    else if (!strcmp(argv[1], "settle")) mode = SETTLE;
    else usage();
    for (int i = 2; i < argc; i++) {
        const char *a = argv[i], *v = i + 1 < argc ? argv[i + 1] : NULL;
        if (!strcmp(a, "--strict")) strict = 1;
        else if (!strcmp(a, "--tied")) tied = 1;
        else if (!v) usage();
        else if (!strcmp(a, "--region")) { region = parse_rect(v); have_region = 1; i++; }
        else if (!strcmp(a, "--ignore") || !strcmp(a, "--mask")) {
            if (nignore == MAX_IGNORE) usage();
            masked[nignore] = !strcmp(a, "--mask");
            ignore[nignore++] = parse_rect(v); i++;
        }
        else if (!strcmp(a, "--quiet")) { quiet = num(v, 1, 600000); i++; }
        else if (!strcmp(a, "--start")) { start = num(v, 1, 600000); i++; }
        else if (!strcmp(a, "--timeout")) { timeout = num(v, 1, 600000); i++; }
        else if (!strcmp(a, "--first")) { first = num(v, 1, 600000); i++; }
        else usage();
    }

    display = wl_display_connect(NULL);
    if (!display) { fprintf(stderr, "omabox-still: cannot connect to $WAYLAND_DISPLAY\n"); return 1; }
    wl_registry_add_listener(wl_display_get_registry(display), &registry_listener, NULL);
    // A stopped compositor still accepts the connection (its listen backlog), and a plain roundtrip then
    // waited forever, --timeout or not (#124, finding 181). Everything after this has a deadline.
    int r = omabox_roundtrip(display, 10000);
    if (r == -2) finish("unknown", "hung", 1);
    if (r < 0) finish("unknown", "lost", 1);
    if (!shm || !manager || !nouts) { fprintf(stderr, "omabox-still: the compositor offers no screencopy, shm or output\n"); return 1; }
    // The outputs' places (xdg-output). Without it, only the first output, its pixels the layout's.
    if (xdg_manager) {
        r = omabox_roundtrip(display, 10000);
        if (r == -2) finish("unknown", "hung", 1);
        if (r < 0) finish("unknown", "lost", 1);
    } else {
        nouts = 1;
    }
    started = 1;

    // The outputs watched: those the region touches (all without one). One with no size yet is not on
    // the screen. Without xdg-output the one output's size is known with its first frame.
    int nwatched = 0;
    for (int i = 0; i < nouts; i++) {
        struct output *o = &outs[i];
        if (o->gone || (xdg_manager && (o->lw <= 0 || o->lh <= 0))) continue;
        struct rect orc = out_rect(o);
        o->watched = !xdg_manager || !have_region || meets(&orc, &region);
        nwatched += o->watched;
    }
    // A region that misses the screen would watch nothing and read as still: unknown, never satisfied.
    if (!nwatched) finish("unknown", "off-screen", 1);

    // The first frame of each: the baseline. None in time is an unrendered screen (a hidden interactive box).
    const int64_t begin = now_ms();
    for (int i = 0; i < nouts; i++) if (outs[i].watched) capture(&outs[i], 1);
    for (;;) {
        int pending = 0;
        for (int i = 0; i < nouts; i++) {
            struct output *o = &outs[i];
            if (!o->watched) continue;
            if (o->frame_failed) finish("unknown", "refused", 1);
            pending += !o->frame_done;
        }
        if (!pending) break;
        int64_t left = begin + first - now_ms();
        if (left <= 0) finish("unknown", "not-rendered", 1);
        pump((int)left);
    }
    frames = 1;
    int bx0 = MAX_SIDE * 4, by0 = MAX_SIDE * 4, bx1 = -MAX_SIDE * 4, by1 = -MAX_SIDE * 4;
    long long area = 0;
    for (int i = 0; i < nouts; i++) {
        struct output *o = &outs[i];
        if (!o->watched) continue;
        o->have_prev = 1;
        o->prev = !o->prev;
        struct rect orc = out_rect(o);
        if (orc.x < bx0) bx0 = orc.x;
        if (orc.y < by0) by0 = orc.y;
        if (orc.x + orc.w > bx1) bx1 = orc.x + orc.w;
        if (orc.y + orc.h > by1) by1 = orc.y + orc.h;
        area += (long long)orc.w * orc.h;
    }
    if (have_region && !xdg_manager) {
        struct rect orc = out_rect(&outs[0]);
        if (!meets(&orc, &region)) finish("unknown", "off-screen", 1);
    }
    printf("ready %dx%d area=%lld\n", bx1 - bx0, by1 - by0, area);
    fflush(stdout);
    t0 = now_ms();

    for (int i = 0; i < nouts; i++) if (outs[i].watched) capture(&outs[i], 0);
    int verifying = 0, failures = 0;
    int64_t verify_since = 0;   // when the plain copies were asked for
    for (;;) {
        int64_t t = now_ms() - t0;
        int any = 0, pending = 0;
        for (int i = 0; i < nouts; i++) {
            struct output *o = &outs[i];
            if (!o->watched) continue;
            if (o->frame_failed) {
                // A frame the compositor could not fill (the screen changed under it): ask again, plainly.
                if (++failures > 3) finish("unknown", "refused", 1);
                capture(o, 1);
                any = 1;
            } else if (o->frame_done) {
                failures = 0;
                frames++;
                if (compare(o)) significant(t);
                o->have_prev = 1;
                o->prev = !o->prev;
                o->frame_done = 0;
                if (mode == CHANGE && last_sig >= 0) finish("satisfied", "changed", 0);
                // While verifying, each output's plain copy is its last until all are in.
                if (!verifying) capture(o, 0);
                any = 1;
            }
            if (verifying && o->frame) pending++;
        }
        if (verifying && !pending) {
            verifying = 0;
            if (satisfied(t)) finish("satisfied", mode == STILL ? "still" : "settled", 0);
            if (t >= timeout) unsatisfied();
            if (mode == SETTLE && last_sig < 0 && t >= start) finish("unsatisfied", "nothing", 124);
            for (int i = 0; i < nouts; i++) if (outs[i].watched) capture(&outs[i], 0);
            continue;
        }
        if (any) continue;
        int64_t d = next_deadline();
        if (!verifying && t >= d) {
            // The answer would be "no change": first make sure with a plain copy of each, which never waits.
            for (int i = 0; i < nouts; i++) if (outs[i].watched) capture(&outs[i], 1);
            verifying = 1;
            verify_since = now_ms();
            continue;
        }
        pump(verifying ? 100 : (int)(d - t));
        // A plain copy is answered at the next frame the output renders: none within --first ms is a
        // screen nobody renders (an interactive box's window was hidden after its first frame).
        if (verifying && now_ms() - verify_since > first) {
            int waiting = 0;
            for (int i = 0; i < nouts; i++) waiting |= outs[i].watched && !outs[i].frame_done && outs[i].frame;
            if (waiting) finish("unknown", "not-rendered", 1);
        }
    }
}
