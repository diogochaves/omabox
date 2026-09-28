// omabox-still: wait until the screen of the Wayland display in $WAYLAND_DISPLAY holds still, or changes.
//
//   omabox-still (still | change | settle) [--region X,Y,W,H] [--ignore X,Y,W,H]... [--quiet MS]
//                [--start MS] [--timeout MS] [--first MS] [--strict] [--tied]
//
// still:  satisfied once nothing significant has changed for --quiet ms (default 300).
// change: satisfied at the first significant change.
// settle: a significant change within --start ms (default 2000), then --quiet ms without one: what
//         follows an action (omabox keys/click/run --wait). No change at all is not settled.
//
// It asks the compositor for frames with wlr-screencopy's copy_with_damage, which only completes when
// something was drawn: an idle screen costs nothing while it waits. Every frame is compared with the
// one before, pixel by pixel, inside --region (default: the whole output). A change is significant
// unless it is 4 pixels or less in width or height (a text caret blinking) or lies inside an --ignore
// rectangle (where the software cursor is: Hyprland hides it on a key press); --strict counts all.
// Before it answers "nothing changed" or "still", one plain copy (which never waits) is compared too, so
// a change drawn between two requests is not missed.
//
// Prints `ready WxH` once it has the first frame (the caller acts after that), then one line:
//   satisfied|unsatisfied|unknown REASON t=MS first=MS last=MS change=X,Y,W,H ignored=X,Y,W,H why=WHY frames=N
// (times from `ready`, - when there is none). Exit 0 satisfied, 124 unsatisfied, 1 unknown, 2 usage.
// unknown: no first frame within --first ms (default 2000: an interactive box's hidden window is not
// rendered), the compositor went away, or with --tied, stdin closed (the caller is gone).
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

#define DRM_FORMAT_XBGR8888 0x34324258
#define DRM_FORMAT_ABGR8888 0x34324241
#define MAX_SIDE 16384
#define MAX_IGNORE 8
#define THIN 4   // a change this thin or thinner is a caret, not a change

struct rect { int x, y, w, h; };

static void usage(void) {
    fprintf(stderr, "usage: omabox-still (still|change|settle) [--region X,Y,W,H] [--ignore X,Y,W,H]... [--quiet MS]\n"
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

// ---- options and state ------------------------------------------------------------------------------

enum mode { STILL, CHANGE, SETTLE };
static enum mode mode;
static long quiet = 300, start = 2000, timeout = 10000, first = 2000;
static int strict, tied, have_region;
static struct rect region, ignore[MAX_IGNORE];
static int nignore;

static int64_t t0 = -1;               // when `ready` was printed
static int64_t first_sig = -1, last_sig = -1;   // ms since t0
static struct rect sig_box, ign_box;  // the last significant change, the last ignored one
static const char *ign_why;
static long frames;

// ---- capture ---------------------------------------------------------------------------------------

struct shmbuf {
    struct wl_buffer *buffer;
    uint32_t *data;
    size_t size;
};

static struct wl_display *display;
static struct wl_shm *shm;
static struct zwlr_screencopy_manager_v1 *manager;
static uint32_t manager_version;
static struct wl_output *output;

static struct shmbuf bufs[2];  // the frame before (prev) and the one being captured (cur)
static int prev = 0, have_prev;
static int width, height, stride, y_invert;  // of the allocated buffers
static uint32_t format;
static int fw, fh, fstride, finvert;         // of the frame being captured
static uint32_t fformat;
static struct zwlr_screencopy_frame_v1 *frame;
static int frame_done, frame_failed_flag, frame_plain;

static int shmbuf_create(struct shmbuf *b) {
    b->size = (size_t)stride * (size_t)height;
    int fd = memfd_create("omabox-still", MFD_CLOEXEC);
    if (fd < 0) return 0;
    if (ftruncate(fd, (off_t)b->size) < 0) { close(fd); return 0; }
    b->data = mmap(NULL, b->size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (b->data == MAP_FAILED) { b->data = NULL; close(fd); return 0; }
    struct wl_shm_pool *pool = wl_shm_create_pool(shm, fd, (int32_t)b->size);
    b->buffer = wl_shm_pool_create_buffer(pool, 0, width, height, stride, format);
    wl_shm_pool_destroy(pool);
    close(fd);
    return 1;
}

static void shmbuf_destroy(struct shmbuf *b) {
    if (b->buffer) wl_buffer_destroy(b->buffer);
    if (b->data) munmap(b->data, b->size);
    memset(b, 0, sizeof(*b));
}

static void global(void *data, struct wl_registry *reg, uint32_t name, const char *iface, uint32_t version) {
    (void)data;
    if (!strcmp(iface, wl_shm_interface.name)) {
        shm = wl_registry_bind(reg, name, &wl_shm_interface, 1);
    } else if (!strcmp(iface, zwlr_screencopy_manager_v1_interface.name)) {
        manager_version = version < 3 ? version : 3;
        manager = wl_registry_bind(reg, name, &zwlr_screencopy_manager_v1_interface, manager_version);
    } else if (!strcmp(iface, wl_output_interface.name) && !output) {
        output = wl_registry_bind(reg, name, &wl_output_interface, 1);   // a box has one screen
    }
}
static void global_remove(void *data, struct wl_registry *reg, uint32_t name) { (void)data; (void)reg; (void)name; }
static const struct wl_registry_listener registry_listener = {global, global_remove};

// The frame's buffer is only known once the compositor describes it: a new size (a mode change) means
// new buffers, and the next frame is a change of the whole screen.
static void frame_copy(void) {
    if (!bufs[0].buffer || fw != width || fh != height || fstride != stride || fformat != format) {
        shmbuf_destroy(&bufs[0]); shmbuf_destroy(&bufs[1]);
        width = fw; height = fh; stride = fstride; format = fformat;
        if (!shmbuf_create(&bufs[0]) || !shmbuf_create(&bufs[1])) {
            fprintf(stderr, "omabox-still: cannot allocate a capture buffer\n");
            exit(1);
        }
        have_prev = 0;
    }
    struct wl_buffer *b = bufs[!prev].buffer;
    if (frame_plain) zwlr_screencopy_frame_v1_copy(frame, b);
    else zwlr_screencopy_frame_v1_copy_with_damage(frame, b);
}

static void frame_buffer(void *data, struct zwlr_screencopy_frame_v1 *f, uint32_t fmt, uint32_t w, uint32_t h, uint32_t s) {
    (void)data; (void)f;
    int ok = fmt == WL_SHM_FORMAT_XRGB8888 || fmt == WL_SHM_FORMAT_ARGB8888 ||
             fmt == DRM_FORMAT_XBGR8888 || fmt == DRM_FORMAT_ABGR8888;
    if (!ok || !w || !h || w > MAX_SIDE || h > MAX_SIDE || s % 4 || s < w * 4 || (uint64_t)s * h > INT32_MAX) {
        fprintf(stderr, "omabox-still: a frame it cannot read (format %#x, %ux%u, stride %u)\n", fmt, w, h, s);
        exit(1);
    }
    fformat = fmt; fw = (int)w; fh = (int)h; fstride = (int)s;
    if (manager_version < 3) frame_copy();   // v3 lists every buffer type, then buffer_done
}
static void frame_flags(void *data, struct zwlr_screencopy_frame_v1 *f, uint32_t flags) {
    (void)data; (void)f;
    finvert = flags & ZWLR_SCREENCOPY_FRAME_V1_FLAGS_Y_INVERT;
}
static void frame_ready(void *data, struct zwlr_screencopy_frame_v1 *f, uint32_t a, uint32_t b, uint32_t c) {
    (void)data; (void)a; (void)b; (void)c;
    zwlr_screencopy_frame_v1_destroy(f);
    frame = NULL;
    y_invert = finvert;
    frame_done = 1;
}
static void frame_failed(void *data, struct zwlr_screencopy_frame_v1 *f) {
    (void)data;
    zwlr_screencopy_frame_v1_destroy(f);
    frame = NULL;
    frame_failed_flag = 1;
}
static void frame_damage(void *d, struct zwlr_screencopy_frame_v1 *f, uint32_t x, uint32_t y, uint32_t w, uint32_t h) {
    (void)d; (void)f; (void)x; (void)y; (void)w; (void)h;   // always the whole output in Hyprland: the pixels tell
}
static void frame_dmabuf(void *d, struct zwlr_screencopy_frame_v1 *f, uint32_t fmt, uint32_t w, uint32_t h) {
    (void)d; (void)f; (void)fmt; (void)w; (void)h;
}
static void frame_buffer_done(void *data, struct zwlr_screencopy_frame_v1 *f) { (void)data; (void)f; frame_copy(); }
static const struct zwlr_screencopy_frame_v1_listener frame_listener = {
    frame_buffer, frame_flags, frame_ready, frame_failed, frame_damage, frame_dmabuf, frame_buffer_done};

// plain: the current frame at once; else the next one drawn with damage (none while nothing changes).
static void capture(int plain) {
    frame_done = frame_failed_flag = 0;
    frame_plain = plain;
    frame = zwlr_screencopy_manager_v1_capture_output(manager, 1, output);   // the cursor is in the frame anyway
    zwlr_screencopy_frame_v1_add_listener(frame, &frame_listener, NULL);
}

static void cancel_capture(void) {
    if (frame) zwlr_screencopy_frame_v1_destroy(frame);
    frame = NULL;
}

// ---- comparing frames ------------------------------------------------------------------------------

static void grow(struct rect *r, int *n, int x, int y) {
    if (!*n) { *r = (struct rect){x, y, 1, 1}; *n = 1; return; }
    if (x < r->x) { r->w += r->x - x; r->x = x; } else if (x >= r->x + r->w) r->w = x - r->x + 1;
    if (y < r->y) { r->h += r->y - y; r->y = y; } else if (y >= r->y + r->h) r->h = y - r->y + 1;
}

// The --ignore rectangle a pixel is in (its index), or -1.
static int ignored(int x, int y) {
    for (int i = 0; i < nignore; i++)
        if (x >= ignore[i].x && x < ignore[i].x + ignore[i].w && y >= ignore[i].y && y < ignore[i].y + ignore[i].h) return i;
    return -1;
}

// Compare the new frame (cur) with prev inside the region. Returns 1 for a significant change (its box in
// sig_box), 0 otherwise (a smaller one goes to ign_box/ign_why).
static int compare(void) {
    int cur = !prev;
    if (!have_prev) { sig_box = (struct rect){0, 0, width, height}; return 1; }
    struct rect r = have_region ? region : (struct rect){0, 0, width, height};
    int x0 = r.x < 0 ? 0 : r.x, y0 = r.y < 0 ? 0 : r.y;
    int x1 = r.x + r.w > width ? width : r.x + r.w, y1 = r.y + r.h > height ? height : r.y + r.h;
    if (x0 >= x1 || y0 >= y1) return 0;
    // Each ignored rectangle on its own: a box around two of them would name the screen between.
    struct rect s = {0}, ig[MAX_IGNORE] = {{0}};
    int ns = 0, ni[MAX_IGNORE] = {0};
    const int words = stride / 4;
    for (int y = y0; y < y1; y++) {
        int row = y_invert ? height - 1 - y : y;
        const uint32_t *a = bufs[prev].data + (size_t)row * (size_t)words, *b = bufs[cur].data + (size_t)row * (size_t)words;
        if (!memcmp(a + x0, b + x0, (size_t)(x1 - x0) * 4)) continue;
        for (int x = x0; x < x1; x++) {
            if (!((a[x] ^ b[x]) & 0xffffff)) continue;   // colour only: X/alpha bytes may be anything
            int i = strict ? -1 : ignored(x, y);
            if (i >= 0) grow(&ig[i], &ni[i], x, y);
            else grow(&s, &ns, x, y);
        }
    }
    for (int i = 0; i < nignore; i++)
        if (ni[i]) { ign_box = ig[i]; ign_why = "cursor"; }
    if (!ns) return 0;
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
    char t[32] = "-", f[32] = "-", l[32] = "-", s[64], ig[64];
    if (t0 >= 0) snprintf(t, sizeof(t), "%lld", (long long)(now_ms() - t0));
    if (first_sig >= 0) snprintf(f, sizeof(f), "%lld", (long long)first_sig);
    if (last_sig >= 0) snprintf(l, sizeof(l), "%lld", (long long)last_sig);
    rect_str(s, sizeof(s), &sig_box, last_sig >= 0);
    rect_str(ig, sizeof(ig), &ign_box, ign_why != NULL);
    printf("%s %s t=%s first=%s last=%s change=%s ignored=%s why=%s frames=%ld\n",
           result, reason, t, f, l, s, ig, ign_why ? ign_why : "-", frames);
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

int main(int argc, char **argv) {
    // Only ever inside a box (omabox runs it there): from a host shell, WAYLAND_DISPLAY is the user's
    // real desktop, and there is nothing of theirs to wait on.
    if (access("/opt/omabox/share", F_OK) != 0) {
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
        else if (!strcmp(a, "--ignore")) { if (nignore == MAX_IGNORE) usage(); ignore[nignore++] = parse_rect(v); i++; }
        else if (!strcmp(a, "--quiet")) { quiet = num(v, 1, 600000); i++; }
        else if (!strcmp(a, "--start")) { start = num(v, 1, 600000); i++; }
        else if (!strcmp(a, "--timeout")) { timeout = num(v, 1, 600000); i++; }
        else if (!strcmp(a, "--first")) { first = num(v, 1, 600000); i++; }
        else usage();
    }

    display = wl_display_connect(NULL);
    if (!display) { fprintf(stderr, "omabox-still: cannot connect to $WAYLAND_DISPLAY\n"); return 1; }
    wl_registry_add_listener(wl_display_get_registry(display), &registry_listener, NULL);
    if (wl_display_roundtrip(display) < 0) finish("unknown", "lost", 1);
    if (!shm || !manager || !output) { fprintf(stderr, "omabox-still: the compositor offers no screencopy, shm or output\n"); return 1; }

    // The first frame: the baseline. None in time is an unrendered screen (a hidden interactive box).
    const int64_t begin = now_ms();
    capture(1);
    while (!frame_done) {
        if (frame_failed_flag) finish("unknown", "refused", 1);
        int64_t left = begin + first - now_ms();
        if (left <= 0) finish("unknown", "not-rendered", 1);
        pump((int)left);
    }
    frames = 1;
    have_prev = 1;
    prev = !prev;
    // A region that misses the screen would watch nothing and read as still: unknown, never satisfied.
    if (have_region && (region.x >= width || region.y >= height || region.x + region.w <= 0 || region.y + region.h <= 0))
        finish("unknown", "off-screen", 1);
    printf("ready %dx%d\n", width, height);
    fflush(stdout);
    t0 = now_ms();

    capture(0);
    int verifying = 0, failures = 0;
    int64_t verify_since = 0;
    for (;;) {
        int64_t t = now_ms() - t0;
        if (frame_failed_flag) {
            // A frame the compositor could not fill (the screen changed under it): ask again, plainly.
            if (++failures > 3) finish("unknown", "refused", 1);
            capture(1);
            verifying = 1;
            continue;
        }
        if (frame_done) {
            failures = 0;
            frames++;
            if (compare()) {
                if (first_sig < 0) first_sig = t;
                last_sig = t;
            }
            have_prev = 1;
            prev = !prev;
            if (mode == CHANGE && last_sig >= 0) finish("satisfied", "changed", 0);
            if (verifying) {
                verifying = 0;
                if (satisfied(t)) finish("satisfied", mode == STILL ? "still" : "settled", 0);
                if (t >= timeout) unsatisfied();
                if (mode == SETTLE && last_sig < 0 && t >= start) finish("unsatisfied", "nothing", 124);
            }
            capture(0);
            continue;
        }
        int64_t d = next_deadline();
        if (!verifying && t >= d) {
            // The answer would be "no change": first make sure with a plain copy, which never waits.
            cancel_capture();
            capture(1);
            verifying = 1;
            continue;
        }
        if (verifying && !verify_since) verify_since = now_ms();
        if (!verifying) verify_since = 0;
        pump(verifying ? 100 : (int)(d - t));
        // A plain copy is answered at the next frame the output renders: none within --first ms is a
        // screen nobody renders (an interactive box's window was hidden after its first frame).
        if (verifying && !frame_done && now_ms() - verify_since > first) finish("unknown", "not-rendered", 1);
    }
}
