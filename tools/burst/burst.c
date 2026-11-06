// omabox-burst: `omabox shot --burst`'s frames, all through one Wayland client (#196, finding 314).
//
//   omabox-burst -n N [-e MS] [--prime MS] [-g "X,Y WxH"] [-T ID] [-s SCALE] [-l LEVEL]
//
// Takes N frames of what grim would take with the same -g, -T, -s and -l (the screen, a region of it
// across monitors at the highest scale they have, or a window's own pixels, its -g in its buffer's
// pixels), frame I when I-1 x -e ms have passed since the first (or as soon as the one before is out),
// and writes each to stdout as one line, `frame I T LEN` (T: ms from the first frame's start), then
// LEN bytes of PNG. Exit 0 once all N are out; 1 with a line on stderr when one fails (the box writes
// no files: the CLI writes them on the host); 2 usage.
//
// Why one client: Hyprland posts four screencast events on its socket2 for each capture client
// (`screencast>>1`, `screencastv2>>1`, and both `>>0` when it stops), and a grim per frame was four
// events a frame. A shell or widget that works on each event does it on its GUI thread, where its
// timers run, so a fast burst stalled the very animation it recorded (finding 249). Hyprland keeps one
// screencast session per client and monitor and posts the events once while its frames come less than
// 500 ms apart: one burst, four events a monitor.
//
// The screen: wlr-screencopy's plain copy of each output the region touches, which redraws the output
// (never waits for damage, as ext-image-copy-capture's later frames do). A window (-T, the identifier
// Hyprland's ext-foreign-toplevel-list gives it: its stableId): one ext-image-copy-capture session for
// the burst, each frame nudged by a plain copy of every output, since a window's frame is only
// filled when its monitor next renders, and an idle box renders nothing. --prime MS takes one frame
// first and drops it, then waits MS: a window off screen draws on being captured, and its first frame
// can be one from long before (finding 81).
//
// The image is made as grim 1.5 makes it (its render and PNG writer below), so a frame is a grim shot
// of the same moment. Those parts are grim's, under its licence:
//   Copyright (c) 2018 emersion. MIT License: permission is hereby granted, free of charge, to any
//   person obtaining a copy of this software and associated documentation files (the "Software"), to
//   deal in the Software without restriction, including without limitation the rights to use, copy,
//   modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit
//   persons to whom the Software is furnished to do so, subject to the following conditions: The above
//   copyright notice and this permission notice shall be included in all copies or substantial
//   portions of the Software. THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
//   OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
//   PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE
//   LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
//   OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
//   THE SOFTWARE.
#define _GNU_SOURCE
#include <errno.h>
#include <limits.h>
#include <math.h>
#include <pixman.h>
#include <png.h>
#include <poll.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>
#include <wayland-client.h>

#include "ext-foreign-toplevel-list-v1-client-protocol.h"
#include "ext-image-capture-source-v1-client-protocol.h"
#include "ext-image-copy-capture-v1-client-protocol.h"
#include "wlr-screencopy-unstable-v1-client-protocol.h"
#include "xdg-output-unstable-v1-client-protocol.h"
#include "../common/box.h"
#include "../common/roundtrip.h"

#define MAX_OUTPUTS 16
#define FRAME_MS 10000   // one frame's deadline, as `timeout 10 grim` had

struct gbox { int32_t x, y, width, height; };

struct buffer {
    struct wl_buffer *wl;
    void *data;
    size_t size;
    int32_t width, height, stride;
    uint32_t format;
};

struct output {
    struct wl_output *wl;
    struct zxdg_output_v1 *xdg;
    uint32_t name;
    int32_t fallback_x, fallback_y, mode_width, mode_height, scale;
    enum wl_output_transform transform;
    struct gbox logical;
    double logical_scale;
    int gone, captured;            // captured: the region touches it
    struct buffer buf, nudge;      // the frame's, and a 1x1 nudge's (a window burst)
    struct zwlr_screencopy_frame_v1 *frame;
    struct buffer *target;
    uint32_t flags;
    int done, failed;
};

struct toplevel {
    struct ext_foreign_toplevel_handle_v1 *handle;
    char *identifier;
};

static struct wl_display *display;
static struct wl_shm *shm;
static struct zxdg_output_manager_v1 *xdg_manager;
static struct zwlr_screencopy_manager_v1 *screencopy;
static struct ext_image_copy_capture_manager_v1 *copy_manager;
static struct ext_foreign_toplevel_image_capture_source_manager_v1 *toplevel_sources;
static struct ext_foreign_toplevel_list_v1 *toplevel_list;
static struct output outs[MAX_OUTPUTS];
static int nouts, started;
static struct toplevel *toplevels;
static int ntoplevels;

// The window's session and frame.
static struct {
    struct ext_image_copy_capture_session_v1 *session;
    struct ext_image_copy_capture_frame_v1 *frame;
    uint32_t width, height, shm_format;
    int has_format, constraints, stopped, done, failed;
    uint32_t reason;
    enum wl_output_transform transform;
    struct buffer buf;
} win;

static void usage(void) {
    fprintf(stderr, "usage: omabox-burst -n N [-e MS] [--prime MS] [-g \"X,Y WxH\"] [-T ID] [-s SCALE] [-l LEVEL]\n");
    exit(2);
}

static void fail(const char *fmt, ...) __attribute__((format(printf, 1, 2), noreturn));
static void fail(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    fputs("omabox-burst: ", stderr);
    vfprintf(stderr, fmt, ap);
    fputc('\n', stderr);
    va_end(ap);
    exit(1);
}

static long num(const char *s, long lo, long hi) {
    char *end;
    if (!s || !*s) usage();
    errno = 0;
    long v = strtol(s, &end, 10);
    if (errno || *end || v < lo || v > hi) { fprintf(stderr, "omabox-burst: bad number '%s'\n", s); usage(); }
    return v;
}

// ---- grim's (render.c, box.c, output-layout.c, write_png.c of grim 1.5.0), little-endian ---------------

static bool parse_box(struct gbox *box, const char *str) {
    char *end = NULL;
    box->x = strtol(str, &end, 10);
    if (end[0] != ',') return false;
    char *next = end + 1;
    box->y = strtol(next, &end, 10);
    if (end[0] != ' ') return false;
    next = end + 1;
    box->width = strtol(next, &end, 10);
    if (end[0] != 'x') return false;
    next = end + 1;
    box->height = strtol(next, &end, 10);
    return end[0] == '\0';
}

static bool is_empty_box(const struct gbox *b) { return b->width <= 0 || b->height <= 0; }

static bool intersect_box(const struct gbox *a, const struct gbox *b) {
    if (is_empty_box(a) || is_empty_box(b)) return false;
    int x1 = fmax(a->x, b->x), y1 = fmax(a->y, b->y);
    int x2 = fmin(a->x + a->width, b->x + b->width), y2 = fmin(a->y + a->height, b->y + b->height);
    struct gbox r = {x1, y1, x2 - x1, y2 - y1};
    return !is_empty_box(&r);
}

static void apply_output_transform(enum wl_output_transform t, int32_t *w, int32_t *h) {
    if (t & WL_OUTPUT_TRANSFORM_90) { int32_t tmp = *w; *w = *h; *h = tmp; }
}

static double get_output_rotation(enum wl_output_transform t) {
    switch (t & ~WL_OUTPUT_TRANSFORM_FLIPPED) {
    case WL_OUTPUT_TRANSFORM_90: return M_PI / 2;
    case WL_OUTPUT_TRANSFORM_180: return M_PI;
    case WL_OUTPUT_TRANSFORM_270: return 3 * M_PI / 2;
    }
    return 0;
}

static int get_output_flipped(enum wl_output_transform t) { return t & WL_OUTPUT_TRANSFORM_FLIPPED ? -1 : 1; }

static pixman_format_code_t get_pixman_format(uint32_t f) {
    switch (f) {
    case WL_SHM_FORMAT_ARGB8888: return PIXMAN_a8r8g8b8;
    case WL_SHM_FORMAT_XRGB8888: return PIXMAN_x8r8g8b8;
    case WL_SHM_FORMAT_ABGR8888: return PIXMAN_a8b8g8r8;
    case WL_SHM_FORMAT_XBGR8888: return PIXMAN_x8b8g8r8;
    case WL_SHM_FORMAT_BGRA8888: return PIXMAN_b8g8r8a8;
    case WL_SHM_FORMAT_BGRX8888: return PIXMAN_b8g8r8x8;
    case WL_SHM_FORMAT_RGBA8888: return PIXMAN_r8g8b8a8;
    case WL_SHM_FORMAT_RGBX8888: return PIXMAN_r8g8b8x8;
    case WL_SHM_FORMAT_ARGB2101010: return PIXMAN_a2r10g10b10;
    case WL_SHM_FORMAT_ABGR2101010: return PIXMAN_a2b10g10r10;
    case WL_SHM_FORMAT_XRGB2101010: return PIXMAN_x2r10g10b10;
    case WL_SHM_FORMAT_XBGR2101010: return PIXMAN_x2b10g10r10;
    case WL_SHM_FORMAT_RGB565: return PIXMAN_r5g6b5;
    case WL_SHM_FORMAT_BGR565: return PIXMAN_b5g6r5;
    case WL_SHM_FORMAT_RGB888: return PIXMAN_r8g8b8;
    case WL_SHM_FORMAT_BGR888: return PIXMAN_b8g8r8;
    default: return 0;
    }
}

static uint32_t get_format_min_stride(uint32_t fmt, uint32_t width) {
    uint32_t bpp = PIXMAN_FORMAT_BPP(get_pixman_format(fmt));
    return ((width * bpp + 0x1f) >> 5) * sizeof(uint32_t);
}

static void compute_composite_region(const struct pixman_f_transform *out2com, int w_, int h_, struct gbox *dest, bool *grid_aligned) {
    struct pixman_transform o2c;
    pixman_transform_from_pixman_f_transform(&o2c, out2com);
    pixman_fixed_t w = pixman_int_to_fixed(w_), h = pixman_int_to_fixed(h_);
    struct pixman_vector corners[4] = {{{0, 0, pixman_fixed_1}}, {{w, 0, pixman_fixed_1}}, {{0, h, pixman_fixed_1}}, {{w, h, pixman_fixed_1}}};
    pixman_fixed_t x_min = INT32_MAX, x_max = INT32_MIN, y_min = INT32_MAX, y_max = INT32_MIN;
    for (int i = 0; i < 4; i++) {
        pixman_transform_point(&o2c, &corners[i]);
        x_min = corners[i].vector[0] < x_min ? corners[i].vector[0] : x_min;
        x_max = corners[i].vector[0] > x_max ? corners[i].vector[0] : x_max;
        y_min = corners[i].vector[1] < y_min ? corners[i].vector[1] : y_min;
        y_max = corners[i].vector[1] > y_max ? corners[i].vector[1] : y_max;
    }
    *grid_aligned = pixman_fixed_frac(x_min) == 0 && pixman_fixed_frac(x_max) == 0 &&
                    pixman_fixed_frac(y_min) == 0 && pixman_fixed_frac(y_max) == 0;
    int32_t x1 = pixman_fixed_to_int(pixman_fixed_floor(x_min)), x2 = pixman_fixed_to_int(pixman_fixed_ceil(x_max));
    int32_t y1 = pixman_fixed_to_int(pixman_fixed_floor(y_min)), y2 = pixman_fixed_to_int(pixman_fixed_ceil(y_max));
    *dest = (struct gbox){x1, y1, x2 - x1, y2 - y1};
}

// One source of the image: its buffer, where it goes in the layout, and how it is turned.
struct capture {
    struct buffer *buf;
    struct gbox logical;
    enum wl_output_transform transform;
    uint32_t flags;
};

static pixman_image_t *render(const struct capture *caps, int ncaps, const struct gbox *geometry, double scale) {
    int cw = geometry->width * scale, ch = geometry->height * scale;
    pixman_image_t *common = pixman_image_create_bits(PIXMAN_a8r8g8b8, cw, ch, NULL, 0);
    if (!common) fail("cannot make a %dx%d image", cw, ch);
    for (int c = 0; c < ncaps; c++) {
        const struct capture *cap = &caps[c];
        const struct buffer *buffer = cap->buf;
        pixman_format_code_t fmt = get_pixman_format(buffer->format);
        if (!fmt) fail("a frame in a format it cannot read (%#x)", buffer->format);
        int32_t output_x = cap->logical.x - geometry->x, output_y = cap->logical.y - geometry->y;
        int32_t output_width = cap->logical.width, output_height = cap->logical.height;
        int32_t raw_w = buffer->width, raw_h = buffer->height;
        apply_output_transform(cap->transform, &raw_w, &raw_h);
        int flipped_x = get_output_flipped(cap->transform);
        int flipped_y = cap->flags & ZWLR_SCREENCOPY_FRAME_V1_FLAGS_Y_INVERT ? -1 : 1;
        pixman_image_t *img = pixman_image_create_bits(fmt, buffer->width, buffer->height, buffer->data, buffer->stride);
        if (!img) fail("cannot make an image of a frame");
        struct pixman_f_transform out2com;
        pixman_f_transform_init_identity(&out2com);
        pixman_f_transform_translate(&out2com, NULL, -(double)buffer->width / 2, -(double)buffer->height / 2);
        pixman_f_transform_scale(&out2com, NULL, (double)output_width / raw_w, (double)output_height * flipped_y / raw_h);
        pixman_f_transform_rotate(&out2com, NULL, round(cos(get_output_rotation(cap->transform))), round(sin(get_output_rotation(cap->transform))));
        pixman_f_transform_scale(&out2com, NULL, flipped_x, 1);
        pixman_f_transform_translate(&out2com, NULL, (double)output_width / 2, (double)output_height / 2);
        pixman_f_transform_translate(&out2com, NULL, output_x, output_y);
        pixman_f_transform_scale(&out2com, NULL, scale, scale);
        struct gbox dest;
        bool grid_aligned;
        compute_composite_region(&out2com, buffer->width, buffer->height, &dest, &grid_aligned);
        pixman_f_transform_translate(&out2com, NULL, -dest.x, -dest.y);
        struct pixman_f_transform com2out;
        pixman_f_transform_invert(&com2out, &out2com);
        struct pixman_transform c2o;
        pixman_transform_from_pixman_f_transform(&c2o, &com2out);
        pixman_image_set_transform(img, &c2o);
        double x_scale = fmax(fabs(out2com.m[0][0]), fabs(out2com.m[0][1]));
        double y_scale = fmax(fabs(out2com.m[1][0]), fabs(out2com.m[1][1]));
        if (x_scale >= 0.75 && y_scale >= 0.75) {
            pixman_image_set_filter(img, PIXMAN_FILTER_BILINEAR, NULL, 0);
        } else {
            int n_values = 0;
            pixman_fixed_t *conv = pixman_filter_create_separable_convolution(&n_values,
                pixman_double_to_fixed(fmax(1., 1. / x_scale)), pixman_double_to_fixed(fmax(1., 1. / y_scale)),
                PIXMAN_KERNEL_IMPULSE, PIXMAN_KERNEL_IMPULSE, PIXMAN_KERNEL_LANCZOS2, PIXMAN_KERNEL_LANCZOS2, 2, 2);
            pixman_image_set_filter(img, PIXMAN_FILTER_SEPARABLE_CONVOLUTION, conv, n_values);
            free(conv);
        }
        bool overlapping = false;
        for (int o = 0; o < ncaps; o++)
            if (o != c && intersect_box(&cap->logical, &caps[o].logical)) overlapping = true;
        pixman_op_t op = (grid_aligned && !overlapping) ? PIXMAN_OP_SRC : PIXMAN_OP_OVER;
        pixman_image_composite32(op, img, NULL, common, 0, 0, 0, 0, dest.x, dest.y, dest.width, dest.height);
        pixman_image_unref(img);
    }
    return common;
}

static void pack_row32(uint8_t *restrict out, const uint32_t *restrict in, size_t width, bool opaque) {
    for (size_t x = 0; x < width; x++) {
        uint8_t b = in[x] & 0xff, g = (in[x] >> 8) & 0xff, r = (in[x] >> 16) & 0xff, a = (in[x] >> 24) & 0xff;
        if (!opaque && a != 0 && a != 255) {
            uint32_t inv = (0xff << 16) / a;
            uint32_t sr = r * inv; r = sr > (0xff << 16) ? 0xff : (sr >> 16);
            uint32_t sg = g * inv; g = sg > (0xff << 16) ? 0xff : (sg >> 16);
            uint32_t sb = b * inv; b = sb > (0xff << 16) ? 0xff : (sb >> 16);
        }
        *out++ = r; *out++ = g; *out++ = b;
        if (!opaque) *out++ = a;
    }
}

static int write_png(pixman_image_t *image, FILE *stream, int level) {
    int width = pixman_image_get_width(image), height = pixman_image_get_height(image);
    int stride = pixman_image_get_stride(image);
    const unsigned char *data = (unsigned char *)pixman_image_get_data(image);
    bool opaque = true;
    for (int y = 0; y < height && opaque; y++) {
        const uint32_t *row = (const uint32_t *)(data + y * stride);
        for (int x = 0; x < width; x++) if ((row[x] >> 24) != 0xff) { opaque = false; break; }
    }
    uint8_t *tmp_row = calloc(width, 4);
    if (!tmp_row) return -1;
    int ret = 0;
    png_struct *png = png_create_write_struct(PNG_LIBPNG_VER_STRING, NULL, NULL, NULL);
    png_info *info = png ? png_create_info_struct(png) : NULL;
    if (!png || !info) { ret = -1; goto cleanup; }
    if (setjmp(png_jmpbuf(png))) { ret = -1; goto cleanup; }
    png_init_io(png, stream);
    png_set_IHDR(png, info, width, height, 8, opaque ? PNG_COLOR_TYPE_RGB : PNG_COLOR_TYPE_RGBA,
                 PNG_INTERLACE_NONE, PNG_COMPRESSION_TYPE_BASE, PNG_FILTER_TYPE_BASE);
    png_write_info(png, info);
    png_set_compression_level(png, level);
    png_set_filter(png, 0, level == 0 ? PNG_NO_FILTERS : PNG_ALL_FILTERS);
    for (int y = 0; y < height; y++) {
        pack_row32(tmp_row, (const uint32_t *)(data + y * stride), width, opaque);
        png_write_row(png, tmp_row);
    }
    png_write_end(png, NULL);
cleanup:
    if (info) png_destroy_info_struct(png, &info);
    if (png) png_destroy_write_struct(&png, NULL);
    free(tmp_row);
    return ret;
}

// ---- buffers ----------------------------------------------------------------------------------------

static void buffer_free(struct buffer *b) {
    if (b->wl) wl_buffer_destroy(b->wl);
    if (b->data) munmap(b->data, b->size);
    memset(b, 0, sizeof(*b));
}

// B as a buffer of that shape: kept when it already is one (frames reuse it), else made anew.
static void buffer_fit(struct buffer *b, uint32_t format, int32_t w, int32_t h, int32_t stride) {
    if (b->wl && b->format == format && b->width == w && b->height == h && b->stride == stride) return;
    buffer_free(b);
    if (w <= 0 || h <= 0 || stride < w || (uint64_t)stride * (uint64_t)h > INT32_MAX) fail("a frame it cannot hold (%dx%d, stride %d)", w, h, stride);
    b->size = (size_t)stride * (size_t)h;
    int fd = memfd_create("omabox-burst", MFD_CLOEXEC);
    if (fd < 0 || ftruncate(fd, (off_t)b->size) < 0) fail("cannot allocate a capture buffer");
    b->data = mmap(NULL, b->size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (b->data == MAP_FAILED) fail("cannot allocate a capture buffer");
    struct wl_shm_pool *pool = wl_shm_create_pool(shm, fd, (int32_t)b->size);
    b->wl = wl_shm_pool_create_buffer(pool, 0, w, h, stride, format);
    wl_shm_pool_destroy(pool);
    close(fd);
    b->width = w; b->height = h; b->stride = stride; b->format = format;
}

// ---- outputs ----------------------------------------------------------------------------------------

static void out_geometry(void *data, struct wl_output *o, int32_t x, int32_t y, int32_t pw, int32_t ph, int32_t sub,
                         const char *make, const char *model, int32_t transform) {
    (void)o; (void)pw; (void)ph; (void)sub; (void)make; (void)model;
    struct output *out = data;
    out->fallback_x = x; out->fallback_y = y; out->transform = transform;
}
static void out_mode(void *data, struct wl_output *o, uint32_t flags, int32_t w, int32_t h, int32_t r) {
    (void)o; (void)r;
    struct output *out = data;
    if (flags & WL_OUTPUT_MODE_CURRENT) { out->mode_width = w; out->mode_height = h; }
}
static void out_done(void *d, struct wl_output *o) { (void)d; (void)o; }
static void out_scale(void *data, struct wl_output *o, int32_t f) { (void)o; ((struct output *)data)->scale = f; }
static void out_name(void *d, struct wl_output *o, const char *n) { (void)d; (void)o; (void)n; }
static void out_description(void *d, struct wl_output *o, const char *s) { (void)d; (void)o; (void)s; }
static const struct wl_output_listener output_listener = {out_geometry, out_mode, out_done, out_scale, out_name, out_description};

static void xdg_position(void *data, struct zxdg_output_v1 *x, int32_t lx, int32_t ly) {
    (void)x;
    struct output *o = data;
    o->logical.x = lx; o->logical.y = ly;
}
static void xdg_size(void *data, struct zxdg_output_v1 *x, int32_t w, int32_t h) {
    (void)x;
    struct output *o = data;
    o->logical.width = w; o->logical.height = h;
}
static void xdg_done(void *data, struct zxdg_output_v1 *x) {
    (void)x;
    struct output *o = data;
    // grim guesses the scale from the logical size, so a fractional one is right
    int32_t w = o->mode_width, h = o->mode_height;
    apply_output_transform(o->transform, &w, &h);
    if (o->logical.width > 0) o->logical_scale = (double)w / o->logical.width;
}
static void xdg_name(void *d, struct zxdg_output_v1 *x, const char *n) { (void)d; (void)x; (void)n; }
static void xdg_description(void *d, struct zxdg_output_v1 *x, const char *s) { (void)d; (void)x; (void)s; }
static const struct zxdg_output_v1_listener xdg_listener = {xdg_position, xdg_size, xdg_done, xdg_name, xdg_description};

// ---- the screen: wlr-screencopy --------------------------------------------------------------------

static void sc_buffer(void *data, struct zwlr_screencopy_frame_v1 *f, uint32_t fmt, uint32_t w, uint32_t h, uint32_t s) {
    struct output *o = data;
    if (w > 16384 || h > 16384 || s > 65536 * 4) fail("a frame it cannot hold (%ux%u)", w, h);
    buffer_fit(o->target, fmt, (int32_t)w, (int32_t)h, (int32_t)s);
    zwlr_screencopy_frame_v1_copy(f, o->target->wl);   // plain: the output is redrawn for it, never waits
}
static void sc_flags(void *data, struct zwlr_screencopy_frame_v1 *f, uint32_t flags) { (void)f; ((struct output *)data)->flags = flags; }
static void sc_ready(void *data, struct zwlr_screencopy_frame_v1 *f, uint32_t a, uint32_t b, uint32_t c) {
    (void)a; (void)b; (void)c;
    struct output *o = data;
    zwlr_screencopy_frame_v1_destroy(f);
    o->frame = NULL;
    o->done = 1;
}
static void sc_failed(void *data, struct zwlr_screencopy_frame_v1 *f) {
    struct output *o = data;
    zwlr_screencopy_frame_v1_destroy(f);
    o->frame = NULL;
    o->failed = 1;
}
static void sc_damage(void *d, struct zwlr_screencopy_frame_v1 *f, uint32_t x, uint32_t y, uint32_t w, uint32_t h) {
    (void)d; (void)f; (void)x; (void)y; (void)w; (void)h;
}
static void sc_dmabuf(void *d, struct zwlr_screencopy_frame_v1 *f, uint32_t fmt, uint32_t w, uint32_t h) {
    (void)d; (void)f; (void)fmt; (void)w; (void)h;
}
static void sc_buffer_done(void *d, struct zwlr_screencopy_frame_v1 *f) { (void)d; (void)f; }
static const struct zwlr_screencopy_frame_v1_listener sc_listener = {sc_buffer, sc_flags, sc_ready, sc_failed, sc_damage, sc_dmabuf, sc_buffer_done};

// A plain copy of output O into its frame buffer, or (nudge) into one of its own, only to make it
// render. A whole output, not a pixel of it: Hyprland 0.56 keeps a region's session for the next frame
// only at scale 1 (it compares the asked box with the one it scaled), so a region on a scaled monitor
// was four events a frame again.
// The turn a frame of O still needs. wlr-screencopy's frames came unturned (the output's buffer as
// scanned out) and a client turned them by wl_output's transform, as grim did; Hyprland 0.57 hands
// them over upright already while wl_output still says the turn (#435, finding 652), and turned
// again they were the picture squeezed into a band. A quarter turn changes the size: a frame of the
// turned size, not the mode's, is upright. (A half turn or a flip keeps it: those still turn.)
static enum wl_output_transform frame_transform(const struct output *o) {
    if ((o->transform & WL_OUTPUT_TRANSFORM_90) && o->mode_width != o->mode_height &&
        o->buf.width == o->mode_height && o->buf.height == o->mode_width)
        return WL_OUTPUT_TRANSFORM_NORMAL;
    return o->transform;
}

static void sc_capture(struct output *o, int nudge) {
    o->done = o->failed = 0;
    o->flags = 0;
    o->target = nudge ? &o->nudge : &o->buf;
    o->frame = zwlr_screencopy_manager_v1_capture_output(screencopy, 0, o->wl);
    zwlr_screencopy_frame_v1_add_listener(o->frame, &sc_listener, o);
}

// ---- a window: ext-image-copy-capture --------------------------------------------------------------

static void fr_transform(void *d, struct ext_image_copy_capture_frame_v1 *f, uint32_t t) { (void)d; (void)f; win.transform = t; }
static void fr_damage(void *d, struct ext_image_copy_capture_frame_v1 *f, int32_t x, int32_t y, int32_t w, int32_t h) {
    (void)d; (void)f; (void)x; (void)y; (void)w; (void)h;
}
static void fr_time(void *d, struct ext_image_copy_capture_frame_v1 *f, uint32_t a, uint32_t b, uint32_t c) {
    (void)d; (void)f; (void)a; (void)b; (void)c;
}
static void fr_ready(void *d, struct ext_image_copy_capture_frame_v1 *f) {
    (void)d;
    ext_image_copy_capture_frame_v1_destroy(f);
    win.frame = NULL;
    win.done = 1;
}
static void fr_failed(void *d, struct ext_image_copy_capture_frame_v1 *f, uint32_t reason) {
    (void)d;
    ext_image_copy_capture_frame_v1_destroy(f);
    win.frame = NULL;
    win.failed = 1;
    win.reason = reason;
}
static const struct ext_image_copy_capture_frame_v1_listener fr_listener = {fr_transform, fr_damage, fr_time, fr_ready, fr_failed};

static void se_size(void *d, struct ext_image_copy_capture_session_v1 *s, uint32_t w, uint32_t h) {
    (void)d; (void)s;
    win.width = w; win.height = h;
}
static void se_shm(void *d, struct ext_image_copy_capture_session_v1 *s, uint32_t f) {
    (void)d; (void)s;
    if (!win.has_format && get_pixman_format(f)) { win.shm_format = f; win.has_format = 1; }
}
static void se_device(void *d, struct ext_image_copy_capture_session_v1 *s, struct wl_array *a) { (void)d; (void)s; (void)a; }
static void se_dmabuf(void *d, struct ext_image_copy_capture_session_v1 *s, uint32_t f, struct wl_array *m) {
    (void)d; (void)s; (void)f; (void)m;
}
static void se_done(void *d, struct ext_image_copy_capture_session_v1 *s) { (void)d; (void)s; win.constraints = 1; }
static void se_stopped(void *d, struct ext_image_copy_capture_session_v1 *s) { (void)d; (void)s; win.stopped = 1; }
static const struct ext_image_copy_capture_session_v1_listener se_listener = {se_size, se_shm, se_device, se_dmabuf, se_done, se_stopped};

static void tl_closed(void *d, struct ext_foreign_toplevel_handle_v1 *h) { (void)d; (void)h; }
static void tl_done(void *d, struct ext_foreign_toplevel_handle_v1 *h) { (void)d; (void)h; }
static void tl_title(void *d, struct ext_foreign_toplevel_handle_v1 *h, const char *t) { (void)d; (void)h; (void)t; }
static void tl_app_id(void *d, struct ext_foreign_toplevel_handle_v1 *h, const char *a) { (void)d; (void)h; (void)a; }
static void tl_identifier(void *data, struct ext_foreign_toplevel_handle_v1 *h, const char *id) {
    (void)h;
    struct toplevel *t = &toplevels[(intptr_t)data];
    free(t->identifier);
    t->identifier = strdup(id);
}
static const struct ext_foreign_toplevel_handle_v1_listener tl_listener = {tl_closed, tl_done, tl_title, tl_app_id, tl_identifier};

static void list_toplevel(void *d, struct ext_foreign_toplevel_list_v1 *l, struct ext_foreign_toplevel_handle_v1 *h) {
    (void)d; (void)l;
    struct toplevel *more = realloc(toplevels, sizeof(*toplevels) * (ntoplevels + 1));
    if (!more) fail("out of memory");
    toplevels = more;
    toplevels[ntoplevels] = (struct toplevel){h, NULL};
    ext_foreign_toplevel_handle_v1_add_listener(h, &tl_listener, (void *)(intptr_t)ntoplevels);
    ntoplevels++;
}
static void list_finished(void *d, struct ext_foreign_toplevel_list_v1 *l) { (void)d; (void)l; }
static const struct ext_foreign_toplevel_list_v1_listener list_listener = {list_toplevel, list_finished};

// ---- registry ---------------------------------------------------------------------------------------

static void global(void *data, struct wl_registry *reg, uint32_t name, const char *iface, uint32_t version) {
    (void)data;
    if (!strcmp(iface, wl_shm_interface.name)) {
        shm = wl_registry_bind(reg, name, &wl_shm_interface, 1);
    } else if (!strcmp(iface, zxdg_output_manager_v1_interface.name)) {
        xdg_manager = wl_registry_bind(reg, name, &zxdg_output_manager_v1_interface, version > 2 ? 2 : version);
    } else if (!strcmp(iface, wl_output_interface.name) && !started && nouts < MAX_OUTPUTS) {
        struct output *o = &outs[nouts++];
        o->name = name;
        o->scale = 1;
        o->wl = wl_registry_bind(reg, name, &wl_output_interface, version >= 4 ? 4 : 3);
        wl_output_add_listener(o->wl, &output_listener, o);
    } else if (!strcmp(iface, zwlr_screencopy_manager_v1_interface.name)) {
        screencopy = wl_registry_bind(reg, name, &zwlr_screencopy_manager_v1_interface, 1);
    } else if (!strcmp(iface, ext_image_copy_capture_manager_v1_interface.name)) {
        copy_manager = wl_registry_bind(reg, name, &ext_image_copy_capture_manager_v1_interface, 1);
    } else if (!strcmp(iface, ext_foreign_toplevel_image_capture_source_manager_v1_interface.name)) {
        toplevel_sources = wl_registry_bind(reg, name, &ext_foreign_toplevel_image_capture_source_manager_v1_interface, 1);
    } else if (!strcmp(iface, ext_foreign_toplevel_list_v1_interface.name)) {
        toplevel_list = wl_registry_bind(reg, name, &ext_foreign_toplevel_list_v1_interface, 1);
        ext_foreign_toplevel_list_v1_add_listener(toplevel_list, &list_listener, NULL);
    }
}
static void global_remove(void *data, struct wl_registry *reg, uint32_t name) {
    (void)data; (void)reg;
    for (int i = 0; i < nouts; i++) if (outs[i].name == name) outs[i].gone = 1;
}
static const struct wl_registry_listener registry_listener = {global, global_remove};

// ---- the loop ---------------------------------------------------------------------------------------

// Dispatch what arrives, waiting up to ms. Ends the tool when the compositor goes, or when nobody reads
// its stdout any more (the CLI was stopped: a pipe's writing end polls POLLERR once its reader is gone).
static void pump(int ms) {
    struct pollfd fds[2] = {{.fd = wl_display_get_fd(display), .events = POLLIN}, {.fd = 1, .events = 0}};
    while (wl_display_prepare_read(display) != 0)
        if (wl_display_dispatch_pending(display) < 0) fail("the compositor went away");
    if (wl_display_flush(display) < 0 && errno != EAGAIN) { wl_display_cancel_read(display); fail("the compositor went away"); }
    int n = poll(fds, 2, ms < 0 ? 0 : ms);
    if (n > 0 && (fds[0].revents & POLLIN)) {
        if (wl_display_read_events(display) < 0) fail("the compositor went away");
    } else {
        wl_display_cancel_read(display);
    }
    if (fds[0].revents & (POLLERR | POLLHUP)) fail("the compositor went away");
    if (fds[1].revents & (POLLERR | POLLHUP)) exit(1);
    if (wl_display_dispatch_pending(display) < 0) fail("the compositor went away");
}

static void wait_until(int64_t when) {
    for (int64_t left; (left = when - omabox_now_ms()) > 0;) pump((int)left);
}

static const char *reason_text(uint32_t r) {
    switch (r) {
    case EXT_IMAGE_COPY_CAPTURE_FRAME_V1_FAILURE_REASON_BUFFER_CONSTRAINTS: return "the window changed size";
    case EXT_IMAGE_COPY_CAPTURE_FRAME_V1_FAILURE_REASON_STOPPED: return "the window is gone";
    default: return "the compositor could not copy it";
    }
}

static const char *toplevel_id;
static struct gbox geometry;
static int have_geometry;
static struct capture caps[MAX_OUTPUTS];
static int ncaps;

// One frame into the buffers (caps point at them). A window's frame that failed on its size is asked
// again with the new one, at most 3 times.
static void take_frame(long index) {
    const int64_t deadline = omabox_now_ms() + FRAME_MS;
    if (toplevel_id) {
        for (int tries = 0;; tries++) {
            if (win.stopped) fail("frame %ld: the window is gone", index);
            while (!win.constraints) {
                if (omabox_now_ms() >= deadline) fail("frame %ld: nothing in %d s", index, FRAME_MS / 1000);
                pump((int)(deadline - omabox_now_ms()));
                if (win.stopped) fail("frame %ld: the window is gone", index);
            }
            if (!win.has_format) fail("frame %ld: the window comes in no format it reads", index);
            buffer_fit(&win.buf, win.shm_format, (int32_t)win.width, (int32_t)win.height,
                       (int32_t)get_format_min_stride(win.shm_format, win.width));
            win.done = win.failed = 0;
            win.frame = ext_image_copy_capture_session_v1_create_frame(win.session);
            ext_image_copy_capture_frame_v1_add_listener(win.frame, &fr_listener, NULL);
            ext_image_copy_capture_frame_v1_attach_buffer(win.frame, win.buf.wl);
            ext_image_copy_capture_frame_v1_damage_buffer(win.frame, 0, 0, INT32_MAX, INT32_MAX);
            ext_image_copy_capture_frame_v1_capture(win.frame);
            // A window's frame is filled when its monitor next renders: make every monitor render now.
            for (int i = 0; i < nouts; i++) if (!outs[i].gone && outs[i].logical.width > 0) sc_capture(&outs[i], 1);
            while (!win.done && !win.failed) {
                if (omabox_now_ms() >= deadline) fail("frame %ld: nothing in %d s", index, FRAME_MS / 1000);
                pump((int)(deadline - omabox_now_ms()));
            }
            // The nudges' own copies: done with the frame (or dropped by the compositor), never waited on.
            for (int i = 0; i < nouts; i++)
                if (outs[i].frame && outs[i].target == &outs[i].nudge) { zwlr_screencopy_frame_v1_destroy(outs[i].frame); outs[i].frame = NULL; }
            if (win.done) break;
            if (win.reason != EXT_IMAGE_COPY_CAPTURE_FRAME_V1_FAILURE_REASON_BUFFER_CONSTRAINTS || tries >= 3 || win.stopped)
                fail("frame %ld: %s", index, win.stopped ? "the window is gone" : reason_text(win.reason));
            omabox_roundtrip(display, 1000);   // the new size, sent with or before the failure
        }
        caps[0] = (struct capture){&win.buf, {0, 0, (int32_t)win.width, (int32_t)win.height}, win.transform, 0};
        return;
    }
    for (int i = 0; i < nouts; i++) if (outs[i].captured && !outs[i].gone) sc_capture(&outs[i], 0);
    for (;;) {
        int pending = 0;
        for (int i = 0; i < nouts; i++) {
            struct output *o = &outs[i];
            if (!o->captured) continue;
            if (o->gone) { if (o->frame) { zwlr_screencopy_frame_v1_destroy(o->frame); o->frame = NULL; } continue; }
            if (o->failed) fail("frame %ld: the compositor could not copy an output", index);
            pending += !o->done;
        }
        if (!pending) break;
        if (omabox_now_ms() >= deadline) fail("frame %ld: nothing in %d s", index, FRAME_MS / 1000);
        pump((int)(deadline - omabox_now_ms()));
    }
    // An output unplugged mid-burst is left out (its place stays empty), as grim would leave it.
    ncaps = 0;
    for (int i = 0; i < nouts; i++) {
        struct output *o = &outs[i];
        if (o->captured && !o->gone && o->done) caps[ncaps++] = (struct capture){&o->buf, o->logical, frame_transform(o), o->flags};
    }
}

static void put(const void *p, size_t n) {
    const char *c = p;
    while (n) {
        ssize_t w = write(1, c, n);
        if (w < 0 && errno == EINTR) continue;
        if (w <= 0) exit(1);   // nobody reads: the CLI was stopped
        c += w; n -= (size_t)w;
    }
}

int main(int argc, char **argv) {
    // Only ever inside a box (omabox runs it there): from a host shell it would take the real desktop.
    if (!omabox_inside_box()) {
        fprintf(stderr, "%s: only runs inside an omabox box (use omabox shot --burst)\n", argv[0]);
        return 2;
    }
    unsetenv("WAYLAND_SOCKET");
    long n = 0, every = 0, prime = -1, level = 6;
    double scale = 1.0;
    int greatest = 1;
    for (int i = 1; i < argc; i++) {
        const char *a = argv[i], *v = i + 1 < argc ? argv[i + 1] : NULL;
        if (!v) usage();
        if (!strcmp(a, "-n")) n = num(v, 1, 1000);
        else if (!strcmp(a, "-e")) every = num(v, 0, 600000);
        else if (!strcmp(a, "--prime")) prime = num(v, 0, 10000);
        else if (!strcmp(a, "-l")) level = num(v, 0, 9);
        else if (!strcmp(a, "-T")) toplevel_id = v;
        else if (!strcmp(a, "-s")) {
            char *end;
            scale = strtod(v, &end);
            if (*end || !(scale > 0) || scale > 64) usage();
            greatest = 0;
        } else if (!strcmp(a, "-g")) {
            if (!parse_box(&geometry, v) || is_empty_box(&geometry)) { fprintf(stderr, "omabox-burst: invalid geometry\n"); return 2; }
            have_geometry = 1;
        } else usage();
        i++;
    }
    if (n < 1) usage();

    display = wl_display_connect(NULL);
    if (!display) fail("cannot connect to $WAYLAND_DISPLAY");
    wl_registry_add_listener(wl_display_get_registry(display), &registry_listener, NULL);
    // A stopped compositor still accepts the connection: every wait has a deadline (finding 181).
    for (int round = 0; round < 2; round++) {   // the globals, then what binding them sent
        int r = omabox_roundtrip(display, FRAME_MS);
        if (r == -2) fail("the compositor did not answer in %d s", FRAME_MS / 1000);
        if (r < 0) fail("the compositor went away");
        if (round == 0 && xdg_manager)
            for (int i = 0; i < nouts; i++) {
                outs[i].xdg = zxdg_output_manager_v1_get_xdg_output(xdg_manager, outs[i].wl);
                zxdg_output_v1_add_listener(outs[i].xdg, &xdg_listener, &outs[i]);
            }
    }
    started = 1;
    if (!shm || !screencopy) fail("the compositor offers no wl_shm or wlr-screencopy");
    if (!xdg_manager)
        for (int i = 0; i < nouts; i++) {   // grim's guess without xdg-output
            struct output *o = &outs[i];
            o->logical = (struct gbox){o->fallback_x, o->fallback_y, o->mode_width / o->scale, o->mode_height / o->scale};
            apply_output_transform(o->transform, &o->logical.width, &o->logical.height);
            o->logical_scale = o->scale;
        }

    if (toplevel_id) {
        if (!copy_manager || !toplevel_sources || !toplevel_list) fail("the compositor offers no window capture (ext-image-copy-capture)");
        struct toplevel *t = NULL;
        for (int i = 0; i < ntoplevels; i++)
            if (toplevels[i].identifier && !strcmp(toplevels[i].identifier, toplevel_id)) t = &toplevels[i];
        if (!t) fail("cannot find toplevel");
        struct ext_image_capture_source_v1 *src = ext_foreign_toplevel_image_capture_source_manager_v1_create_source(toplevel_sources, t->handle);
        win.session = ext_image_copy_capture_manager_v1_create_session(copy_manager, src, 0);
        ext_image_copy_capture_session_v1_add_listener(win.session, &se_listener, NULL);
        ext_image_capture_source_v1_destroy(src);
        ncaps = 1;
    } else {
        if (!nouts) fail("no wl_output");
        for (int i = 0; i < nouts; i++) {
            struct output *o = &outs[i];
            if (have_geometry && !intersect_box(&geometry, &o->logical)) continue;
            if (greatest && o->logical_scale > scale) scale = o->logical_scale;
            o->captured = 1;
            ncaps++;
        }
        if (!ncaps) fail("supplied geometry did not intersect with any outputs");
        if (!have_geometry) {   // the layout's extents
            int32_t x1 = INT_MAX, y1 = INT_MAX, x2 = INT_MIN, y2 = INT_MIN;
            for (int i = 0; i < nouts; i++) {
                const struct gbox *g = &outs[i].logical;
                if (!outs[i].captured) continue;
                if (g->x < x1) x1 = g->x;
                if (g->y < y1) y1 = g->y;
                if (g->x + g->width > x2) x2 = g->x + g->width;
                if (g->y + g->height > y2) y2 = g->y + g->height;
            }
            geometry = (struct gbox){x1, y1, x2 - x1, y2 - y1};
            have_geometry = 1;
        }
    }

    if (prime >= 0) { take_frame(0); wait_until(omabox_now_ms() + prime); }
    char *png = NULL;
    size_t len = 0;
    const int64_t t0 = omabox_now_ms();
    for (long i = 1; i <= n; i++) {
        wait_until(t0 + (i - 1) * every);
        const int64_t t = omabox_now_ms() - t0;
        take_frame(i);
        struct gbox g = geometry;
        if (toplevel_id && !have_geometry) g = caps[0].logical;   // a window whole: its buffer
        pixman_image_t *img = render(caps, ncaps, &g, scale);
        FILE *mem = open_memstream(&png, &len);
        if (!mem || write_png(img, mem, (int)level) < 0 || fclose(mem) != 0) fail("frame %ld: cannot write its PNG", i);
        pixman_image_unref(img);
        char head[96];
        int hl = snprintf(head, sizeof(head), "frame %ld %lld %zu\n", i, (long long)t, len);
        put(head, (size_t)hl);
        put(png, len);
        free(png);
        png = NULL;
    }
    return 0;
}
