// omabox-fixture: plain windows that write down every input event they get, for the suite's input,
// focus and scale checks (#336): what the client received, read from its log, instead of what foot
// or zenity seemed to do (findings 14 and 41 needed WAYLAND_DEBUG for it).
//
//   omabox-fixture [--windows N] [--first-frame MS] [--for MS] [--exit CODE] [--title T] [--app-id ID]
//                  [--size WxH] [--log FILE]
//
// N xdg toplevels (default 1), window i titled "T i" (just T when there is one; T defaults to
// omabox-fixture, ID too), each one solid colour of its own (COLOURS[i]) with a corner marker at its
// top left: a white square MARK logical px wide whose own top-left quadrant is black, so a flipped or
// turned window shows it at another corner or turned, and a shot shows it MARK x scale pixels wide; its
// right edge is one buffer pixel of red, one pixel in a shot when the window drew at the screen's
// scale, more when the compositor scaled it up.
// Each is drawn at the buffer scale the compositor prefers for it (wl_surface.preferred_buffer_scale;
// before it says, the highest scale of the outputs the window is on), at the size it configures
// (--size when it leaves that to the client: default 400x300).
//
// One JSON line for each event, to FILE (opened O_APPEND, each line one write, so a reader never sees
// half of one), else stdout; each has "t" (ms since start, monotonic), "ev", and "win" (the window's
// index) when it is about one window:
//   start {windows}                       output {name, scale}
//   configure {win, w, h, states}         scale {win, scale}   transform {win, transform}
//   draw {win, w, h, scale}               ready {win}: its first frame was shown
//   close {win}                           exit {why: close|for|signal, code}
//   pointer: enter {win, x, y}  leave {win}  motion {win, x, y}  button {win, button, state}
//            axis {win, axis, value}      (x, y: surface-local, logical px; button: evdev code)
//   keyboard: keymap {format, size}  enter {win, keys}  leave {win}
//            key {win, code, sym, utf8, state}  modifiers {mods, depressed, latched, locked, group}
//            (code: evdev; sym: xkb's name; mods: the active ones of shift, ctrl, alt, super, joined by +)
//   touch: down {win, id, x, y}  up {id}  motion {id, x, y}  cancel
//
// --first-frame MS: each window draws its first frame MS after its first configure (an app slow to
// show; it maps then). --for MS: ends MS after every window drew its first frame. Otherwise it ends
// when the compositor closes one of its windows, or on SIGTERM / SIGINT. --exit CODE: its status when
// it ends so (default 0). Exit 1: no display, a global it needs missing, or the connection lost;
// 2: usage, or run outside a box (it only runs inside an omabox box: the suite's, never the host's).
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>
#include <wayland-client.h>
#include <xkbcommon/xkbcommon.h>

#include "xdg-shell-client-protocol.h"
#include "../common/box.h"

#define MAX_WINDOWS 16
#define MARK 24
static const uint32_t COLOURS[] = {0xff3070c0, 0xffc04030, 0xff30a050, 0xffc0a020, 0xff8040b0, 0xff20a0a0};
#define NCOLOURS (sizeof(COLOURS) / sizeof(COLOURS[0]))

static int out_fd = 1;
static int64_t t0;
static volatile sig_atomic_t signalled;

static int64_t now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

// One JSON line: `{"t":MS,"ev":"EV"` then FMT's fields (each `,"key":value`), in one write.
static void say(const char *ev, const char *fmt, ...) {
    char line[8192];
    int n = snprintf(line, sizeof(line), "{\"t\":%lld,\"ev\":\"%s\"", (long long)(now_ms() - t0), ev);
    if (fmt) {
        va_list ap;
        va_start(ap, fmt);
        n += vsnprintf(line + n, sizeof(line) - n, fmt, ap);
        va_end(ap);
    }
    if (n > (int)sizeof(line) - 3) n = sizeof(line) - 3;
    line[n++] = '}';
    line[n++] = '\n';
    ssize_t w = write(out_fd, line, n);
    (void)w;
}

// S as a JSON string (quoted) in OUT, cut to fit.
static const char *jstr(char *out, size_t cap, const char *s) {
    size_t o = 0;
    out[o++] = '"';
    for (; s && *s && o + 8 < cap; s++) {
        unsigned char c = (unsigned char)*s;
        if (c == '"' || c == '\\') { out[o++] = '\\'; out[o++] = c; }
        else if (c < 0x20) o += snprintf(out + o, cap - o, "\\u%04x", c);
        else out[o++] = c;
    }
    out[o++] = '"';
    out[o] = 0;
    return out;
}

// ---- windows --------------------------------------------------------------------------------------

struct win {
    int i;
    struct wl_surface *surface;
    struct xdg_surface *xdg;
    struct xdg_toplevel *top;
    int w, h, pending_w, pending_h;
    int scale, told_scale;   // told_scale: the compositor said one (preferred_buffer_scale)
    int64_t configured_at;   // its first configure; 0 before
    int drawn, ready, dirty;
};

static struct {
    struct wl_display *display;
    struct wl_compositor *compositor;
    uint32_t compositor_version;
    struct wl_shm *shm;
    struct xdg_wm_base *wm;
    struct wl_seat *seat;
    struct wl_pointer *pointer;
    struct wl_keyboard *keyboard;
    struct wl_touch *touch;
    struct xkb_context *xkb;
    struct xkb_keymap *keymap;
    struct xkb_state *state;
    struct win wins[MAX_WINDOWS];
    int n;
    int def_w, def_h;
    int first_frame_ms;
} app;

struct out { struct wl_output *o; int scale; char name[64]; struct out *next; };
static struct out *outs;

static struct win *win_of(struct wl_surface *s) {
    if (!s) return NULL;
    for (int i = 0; i < app.n; i++)
        if (app.wins[i].surface == s) return &app.wins[i];
    return NULL;
}
static int win_index(struct wl_surface *s) { struct win *w = win_of(s); return w ? w->i : -1; }

static void buffer_release(void *data, struct wl_buffer *b) { (void)data; wl_buffer_destroy(b); }
static const struct wl_buffer_listener buffer_listener = {buffer_release};

static void frame_done(void *data, struct wl_callback *cb, uint32_t t) {
    (void)t;
    struct win *w = data;
    wl_callback_destroy(cb);
    if (!w->ready) { w->ready = 1; say("ready", ",\"win\":%d", w->i); }
}
static const struct wl_callback_listener frame_listener = {frame_done};

// Its colour and corner marker, at its size and scale, in a buffer of its own.
static int draw(struct win *w) {
    int s = w->scale > 0 ? w->scale : 1, bw = w->w * s, bh = w->h * s, stride = bw * 4;
    size_t size = (size_t)stride * bh;
    int fd = memfd_create("omabox-fixture", MFD_CLOEXEC);
    if (fd < 0) return 0;
    if (ftruncate(fd, (off_t)size) < 0) { close(fd); return 0; }
    uint32_t *px = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (px == MAP_FAILED) { close(fd); return 0; }
    uint32_t bg = COLOURS[w->i % NCOLOURS];
    int m = MARK * s, q = MARK / 2 * s;
    for (int y = 0; y < bh; y++)
        for (int x = 0; x < bw; x++)
            px[y * bw + x] = (x >= m || y >= m) ? bg : (x < q && y < q) ? 0xff000000 : x == m - 1 ? 0xffff0000 : 0xffffffff;
    struct wl_shm_pool *pool = wl_shm_create_pool(app.shm, fd, (int32_t)size);
    struct wl_buffer *b = wl_shm_pool_create_buffer(pool, 0, bw, bh, stride, WL_SHM_FORMAT_ARGB8888);
    wl_shm_pool_destroy(pool);
    munmap(px, size);
    close(fd);
    wl_buffer_add_listener(b, &buffer_listener, NULL);
    wl_surface_set_buffer_scale(w->surface, s);
    wl_surface_attach(w->surface, b, 0, 0);
    wl_surface_damage_buffer(w->surface, 0, 0, bw, bh);
    if (!w->ready) wl_callback_add_listener(wl_surface_frame(w->surface), &frame_listener, w);
    wl_surface_commit(w->surface);
    say("draw", ",\"win\":%d,\"w\":%d,\"h\":%d,\"scale\":%d", w->i, w->w, w->h, s);
    w->drawn = 1;
    w->dirty = 0;
    return 1;
}

// The highest scale of the outputs a window is on, until the compositor says which it prefers.
static void rescale_from_outputs(struct win *w, struct wl_output *entered) {
    if (w->told_scale) return;
    for (struct out *o = outs; o; o = o->next)
        if (o->o == entered && o->scale > w->scale) { w->scale = o->scale; w->dirty = w->drawn; }
}

static void surface_enter(void *data, struct wl_surface *s, struct wl_output *o) { (void)s; rescale_from_outputs(data, o); }
static void surface_leave(void *data, struct wl_surface *s, struct wl_output *o) { (void)data; (void)s; (void)o; }
static void surface_scale(void *data, struct wl_surface *s, int32_t scale) {
    (void)s;
    struct win *w = data;
    say("scale", ",\"win\":%d,\"scale\":%d", w->i, scale);
    w->told_scale = 1;
    if (scale != w->scale) { w->scale = scale; w->dirty = w->drawn; }
}
static void surface_transform(void *data, struct wl_surface *s, uint32_t t) {
    (void)s;
    say("transform", ",\"win\":%d,\"transform\":%u", ((struct win *)data)->i, t);
}
static const struct wl_surface_listener surface_listener = {surface_enter, surface_leave, surface_scale, surface_transform};

static void toplevel_configure(void *data, struct xdg_toplevel *t, int32_t w, int32_t h, struct wl_array *states) {
    (void)t;
    struct win *win = data;
    char st[256] = "";
    size_t o = 0;
    uint32_t *s;
    static const char *names[] = {"?", "maximized", "fullscreen", "resizing", "activated", "tiled_left",
                                  "tiled_right", "tiled_top", "tiled_bottom", "suspended"};
    wl_array_for_each(s, states) {
        const char *nm = *s < sizeof(names) / sizeof(names[0]) ? names[*s] : "?";
        o += snprintf(st + o, sizeof(st) - o, "%s\"%s\"", o ? "," : "", nm);
        if (o >= sizeof(st)) break;
    }
    say("configure", ",\"win\":%d,\"w\":%d,\"h\":%d,\"states\":[%s]", win->i, w, h, st);
    win->pending_w = w;
    win->pending_h = h;
}
static void toplevel_close(void *data, struct xdg_toplevel *t) {
    (void)t;
    struct win *w = data;
    say("close", ",\"win\":%d", w->i);
    signalled = -1;   // (the loop ends: "close")
}
static void toplevel_bounds(void *d, struct xdg_toplevel *t, int32_t w, int32_t h) { (void)d; (void)t; (void)w; (void)h; }
static void toplevel_caps(void *d, struct xdg_toplevel *t, struct wl_array *a) { (void)d; (void)t; (void)a; }
static const struct xdg_toplevel_listener toplevel_listener = {toplevel_configure, toplevel_close, toplevel_bounds, toplevel_caps};

static void xdg_configure(void *data, struct xdg_surface *s, uint32_t serial) {
    struct win *w = data;
    xdg_surface_ack_configure(s, serial);
    int nw = w->pending_w > 0 ? w->pending_w : app.def_w, nh = w->pending_h > 0 ? w->pending_h : app.def_h;
    if (nw != w->w || nh != w->h) { w->w = nw; w->h = nh; w->dirty = 1; }
    if (!w->drawn) w->dirty = 1;
    if (!w->configured_at) w->configured_at = now_ms();
}
static const struct xdg_surface_listener xdg_listener = {xdg_configure};

static void wm_ping(void *data, struct xdg_wm_base *wm, uint32_t serial) { (void)data; xdg_wm_base_pong(wm, serial); }
static const struct xdg_wm_base_listener wm_listener = {wm_ping};

// ---- input ----------------------------------------------------------------------------------------

static int pointer_win = -1;
static void p_enter(void *d, struct wl_pointer *p, uint32_t serial, struct wl_surface *s, wl_fixed_t x, wl_fixed_t y) {
    (void)d; (void)p; (void)serial;
    pointer_win = win_index(s);
    say("enter", ",\"device\":\"pointer\",\"win\":%d,\"x\":%.2f,\"y\":%.2f", pointer_win, wl_fixed_to_double(x), wl_fixed_to_double(y));
}
static void p_leave(void *d, struct wl_pointer *p, uint32_t serial, struct wl_surface *s) {
    (void)d; (void)p; (void)serial;
    say("leave", ",\"device\":\"pointer\",\"win\":%d", win_index(s));
    pointer_win = -1;
}
static void p_motion(void *d, struct wl_pointer *p, uint32_t time, wl_fixed_t x, wl_fixed_t y) {
    (void)d; (void)p; (void)time;
    say("motion", ",\"win\":%d,\"x\":%.2f,\"y\":%.2f", pointer_win, wl_fixed_to_double(x), wl_fixed_to_double(y));
}
static void p_button(void *d, struct wl_pointer *p, uint32_t serial, uint32_t time, uint32_t button, uint32_t state) {
    (void)d; (void)p; (void)serial; (void)time;
    say("button", ",\"win\":%d,\"button\":%u,\"state\":\"%s\"", pointer_win, button,
        state == WL_POINTER_BUTTON_STATE_PRESSED ? "pressed" : "released");
}
static void p_axis(void *d, struct wl_pointer *p, uint32_t time, uint32_t axis, wl_fixed_t v) {
    (void)d; (void)p; (void)time;
    say("axis", ",\"win\":%d,\"axis\":\"%s\",\"value\":%.2f", pointer_win,
        axis == WL_POINTER_AXIS_VERTICAL_SCROLL ? "vertical" : "horizontal", wl_fixed_to_double(v));
}
static void p_frame(void *d, struct wl_pointer *p) { (void)d; (void)p; }
static void p_source(void *d, struct wl_pointer *p, uint32_t s) { (void)d; (void)p; (void)s; }
static void p_stop(void *d, struct wl_pointer *p, uint32_t t, uint32_t a) { (void)d; (void)p; (void)t; (void)a; }
static void p_discrete(void *d, struct wl_pointer *p, uint32_t a, int32_t v) { (void)d; (void)p; (void)a; (void)v; }
// (Named fields: a newer libwayland's listener has more events, which this seat version never sends.)
static const struct wl_pointer_listener pointer_listener = {
    .enter = p_enter, .leave = p_leave, .motion = p_motion, .button = p_button, .axis = p_axis,
    .frame = p_frame, .axis_source = p_source, .axis_stop = p_stop, .axis_discrete = p_discrete};

static int keyboard_win = -1;
static void k_keymap(void *d, struct wl_keyboard *k, uint32_t format, int32_t fd, uint32_t size) {
    (void)d; (void)k;
    say("keymap", ",\"format\":%u,\"size\":%u", format, size);
    if (format != WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1) { close(fd); return; }
    char *map = mmap(NULL, size, PROT_READ, MAP_PRIVATE, fd, 0);
    close(fd);
    if (map == MAP_FAILED) return;
    struct xkb_keymap *km = xkb_keymap_new_from_string(app.xkb, map, XKB_KEYMAP_FORMAT_TEXT_V1, XKB_KEYMAP_COMPILE_NO_FLAGS);
    munmap(map, size);
    if (!km) return;
    if (app.state) xkb_state_unref(app.state);
    if (app.keymap) xkb_keymap_unref(app.keymap);
    app.keymap = km;
    app.state = xkb_state_new(km);
}
static void k_enter(void *d, struct wl_keyboard *k, uint32_t serial, struct wl_surface *s, struct wl_array *keys) {
    (void)d; (void)k; (void)serial;
    keyboard_win = win_index(s);
    char list[1024] = "";
    size_t o = 0;
    uint32_t *key;
    wl_array_for_each(key, keys) {
        o += snprintf(list + o, sizeof(list) - o, "%s%u", o ? "," : "", *key);
        if (o >= sizeof(list)) break;
    }
    say("enter", ",\"device\":\"keyboard\",\"win\":%d,\"keys\":[%s]", keyboard_win, list);
}
static void k_leave(void *d, struct wl_keyboard *k, uint32_t serial, struct wl_surface *s) {
    (void)d; (void)k; (void)serial;
    say("leave", ",\"device\":\"keyboard\",\"win\":%d", win_index(s));
    keyboard_win = -1;
}
static void k_key(void *d, struct wl_keyboard *k, uint32_t serial, uint32_t time, uint32_t key, uint32_t state) {
    (void)d; (void)k; (void)serial; (void)time;
    char name[64] = "", utf8[64] = "", jn[160], ju[160];
    if (app.state) {
        xkb_keysym_t sym = xkb_state_key_get_one_sym(app.state, key + 8);
        xkb_keysym_get_name(sym, name, sizeof(name));
        xkb_state_key_get_utf8(app.state, key + 8, utf8, sizeof(utf8));
    }
    say("key", ",\"win\":%d,\"code\":%u,\"sym\":%s,\"utf8\":%s,\"state\":\"%s\"", keyboard_win, key,
        jstr(jn, sizeof(jn), name), jstr(ju, sizeof(ju), utf8), state == WL_KEYBOARD_KEY_STATE_PRESSED ? "pressed" : "released");
}
static void k_mods(void *d, struct wl_keyboard *k, uint32_t serial, uint32_t dep, uint32_t lat, uint32_t lock, uint32_t group) {
    (void)d; (void)k; (void)serial;
    char mods[64] = "";
    if (app.state) {
        xkb_state_update_mask(app.state, dep, lat, lock, 0, 0, group);
        static const char *names[][2] = {{XKB_MOD_NAME_SHIFT, "shift"}, {XKB_MOD_NAME_CTRL, "ctrl"},
                                         {XKB_MOD_NAME_ALT, "alt"}, {XKB_MOD_NAME_LOGO, "super"}};
        size_t o = 0;
        for (size_t i = 0; i < 4; i++)
            if (xkb_state_mod_name_is_active(app.state, names[i][0], XKB_STATE_MODS_EFFECTIVE) > 0)
                o += snprintf(mods + o, sizeof(mods) - o, "%s%s", o ? "+" : "", names[i][1]);
    }
    say("modifiers", ",\"mods\":\"%s\",\"depressed\":%u,\"latched\":%u,\"locked\":%u,\"group\":%u", mods, dep, lat, lock, group);
}
static void k_repeat(void *d, struct wl_keyboard *k, int32_t r, int32_t delay) { (void)d; (void)k; (void)r; (void)delay; }
static const struct wl_keyboard_listener keyboard_listener = {k_keymap, k_enter, k_leave, k_key, k_mods, k_repeat};

static int touch_win[64];
static void t_down(void *d, struct wl_touch *t, uint32_t serial, uint32_t time, struct wl_surface *s, int32_t id, wl_fixed_t x, wl_fixed_t y) {
    (void)d; (void)t; (void)serial; (void)time;
    if (id >= 0 && id < 64) touch_win[id] = win_index(s);
    say("down", ",\"win\":%d,\"id\":%d,\"x\":%.2f,\"y\":%.2f", win_index(s), id, wl_fixed_to_double(x), wl_fixed_to_double(y));
}
static void t_up(void *d, struct wl_touch *t, uint32_t serial, uint32_t time, int32_t id) {
    (void)d; (void)t; (void)serial; (void)time;
    say("up", ",\"win\":%d,\"id\":%d", id >= 0 && id < 64 ? touch_win[id] : -1, id);
}
static void t_motion(void *d, struct wl_touch *t, uint32_t time, int32_t id, wl_fixed_t x, wl_fixed_t y) {
    (void)d; (void)t; (void)time;
    say("motion", ",\"device\":\"touch\",\"win\":%d,\"id\":%d,\"x\":%.2f,\"y\":%.2f", id >= 0 && id < 64 ? touch_win[id] : -1, id,
        wl_fixed_to_double(x), wl_fixed_to_double(y));
}
static void t_frame(void *d, struct wl_touch *t) { (void)d; (void)t; }
static void t_cancel(void *d, struct wl_touch *t) { (void)d; (void)t; say("cancel", NULL); }
static const struct wl_touch_listener touch_listener = {
    .down = t_down, .up = t_up, .motion = t_motion, .frame = t_frame, .cancel = t_cancel};

static void seat_caps(void *d, struct wl_seat *s, uint32_t caps) {
    (void)d;
    if ((caps & WL_SEAT_CAPABILITY_POINTER) && !app.pointer) {
        app.pointer = wl_seat_get_pointer(s);
        wl_pointer_add_listener(app.pointer, &pointer_listener, NULL);
    }
    if ((caps & WL_SEAT_CAPABILITY_KEYBOARD) && !app.keyboard) {
        app.keyboard = wl_seat_get_keyboard(s);
        wl_keyboard_add_listener(app.keyboard, &keyboard_listener, NULL);
    }
    if ((caps & WL_SEAT_CAPABILITY_TOUCH) && !app.touch) {
        app.touch = wl_seat_get_touch(s);
        wl_touch_add_listener(app.touch, &touch_listener, NULL);
    }
}
static void seat_name(void *d, struct wl_seat *s, const char *n) { (void)d; (void)s; (void)n; }
static const struct wl_seat_listener seat_listener = {seat_caps, seat_name};

// ---- outputs and globals --------------------------------------------------------------------------

static void o_geometry(void *d, struct wl_output *o, int32_t x, int32_t y, int32_t pw, int32_t ph, int32_t sp,
                       const char *make, const char *model, int32_t t) {
    (void)d; (void)o; (void)x; (void)y; (void)pw; (void)ph; (void)sp; (void)make; (void)model; (void)t;
}
static void o_mode(void *d, struct wl_output *o, uint32_t f, int32_t w, int32_t h, int32_t r) {
    (void)d; (void)o; (void)f; (void)w; (void)h; (void)r;
}
static void o_done(void *d, struct wl_output *o) {
    (void)o;
    struct out *x = d;
    char jn[160];
    say("output", ",\"name\":%s,\"scale\":%d", jstr(jn, sizeof(jn), x->name), x->scale);
}
static void o_scale(void *d, struct wl_output *o, int32_t s) { (void)o; ((struct out *)d)->scale = s; }
static void o_name(void *d, struct wl_output *o, const char *n) { (void)o; snprintf(((struct out *)d)->name, 64, "%s", n); }
static void o_desc(void *d, struct wl_output *o, const char *n) { (void)d; (void)o; (void)n; }
static const struct wl_output_listener output_listener = {o_geometry, o_mode, o_done, o_scale, o_name, o_desc};

static uint32_t min_u(uint32_t a, uint32_t b) { return a < b ? a : b; }

static void global(void *data, struct wl_registry *reg, uint32_t name, const char *iface, uint32_t version) {
    (void)data;
    if (!strcmp(iface, wl_compositor_interface.name)) {
        app.compositor_version = min_u(version, 6);
        app.compositor = wl_registry_bind(reg, name, &wl_compositor_interface, app.compositor_version);
    } else if (!strcmp(iface, wl_shm_interface.name)) {
        app.shm = wl_registry_bind(reg, name, &wl_shm_interface, 1);
    } else if (!strcmp(iface, xdg_wm_base_interface.name)) {
        app.wm = wl_registry_bind(reg, name, &xdg_wm_base_interface, min_u(version, 5));
        xdg_wm_base_add_listener(app.wm, &wm_listener, NULL);
    } else if (!strcmp(iface, wl_seat_interface.name) && !app.seat) {
        app.seat = wl_registry_bind(reg, name, &wl_seat_interface, min_u(version, 5));
        wl_seat_add_listener(app.seat, &seat_listener, NULL);
    } else if (!strcmp(iface, wl_output_interface.name) && version >= 2) {
        struct out *o = calloc(1, sizeof(*o));
        if (!o) return;
        o->scale = 1;
        o->o = wl_registry_bind(reg, name, &wl_output_interface, min_u(version, 4));
        wl_output_add_listener(o->o, &output_listener, o);
        o->next = outs;
        outs = o;
    }
}
static void global_remove(void *data, struct wl_registry *reg, uint32_t name) { (void)data; (void)reg; (void)name; }
static const struct wl_registry_listener registry_listener = {global, global_remove};

// ---- main -----------------------------------------------------------------------------------------

static void on_signal(int s) { (void)s; signalled = 1; }

static int usage(void) {
    fprintf(stderr, "usage: omabox-fixture [--windows N] [--first-frame MS] [--for MS] [--exit CODE] [--title T]\n"
                    "                      [--app-id ID] [--size WxH] [--log FILE]\n");
    return 2;
}

// A whole number in [lo, hi], or -1.
static long num(const char *s, long lo, long hi) {
    char *end;
    errno = 0;
    long v = strtol(s, &end, 10);
    return (errno || end == s || *end || v < lo || v > hi) ? -1 : v;
}

int main(int argc, char **argv) {
    int n = 1, exit_code = 0;
    long for_ms = -1;
    const char *title = "omabox-fixture", *app_id = "omabox-fixture", *log = NULL;
    app.def_w = 400;
    app.def_h = 300;
    for (int i = 1; i < argc; i++) {
        const char *a = argv[i], *v = i + 1 < argc ? argv[i + 1] : NULL;
        if (!v) return usage();
        i++;
        if (!strcmp(a, "--windows")) { if ((n = (int)num(v, 1, MAX_WINDOWS)) < 0) return usage(); }
        else if (!strcmp(a, "--first-frame")) { if ((app.first_frame_ms = (int)num(v, 0, 600000)) < 0) return usage(); }
        else if (!strcmp(a, "--for")) { if ((for_ms = num(v, 0, 86400000)) < 0) return usage(); }
        else if (!strcmp(a, "--exit")) { if ((exit_code = (int)num(v, 0, 255)) < 0) return usage(); }
        else if (!strcmp(a, "--title")) title = v;
        else if (!strcmp(a, "--app-id")) app_id = v;
        else if (!strcmp(a, "--log")) log = v;
        else if (!strcmp(a, "--size")) {
            if (sscanf(v, "%dx%d", &app.def_w, &app.def_h) != 2 || app.def_w < 1 || app.def_h < 1 ||
                app.def_w > 8192 || app.def_h > 8192) return usage();
        } else return usage();
    }
    if (!omabox_inside_box()) {
        fprintf(stderr, "omabox-fixture: only runs inside an omabox box\n");
        return 2;
    }
    t0 = now_ms();
    if (log) {
        out_fd = open(log, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC | O_NOFOLLOW, 0644);
        if (out_fd < 0) { fprintf(stderr, "omabox-fixture: %s: %s\n", log, strerror(errno)); return 1; }
    }
    struct sigaction sa = {.sa_handler = on_signal};
    sigaction(SIGTERM, &sa, NULL);
    sigaction(SIGINT, &sa, NULL);

    app.display = wl_display_connect(NULL);
    if (!app.display) { fprintf(stderr, "omabox-fixture: no Wayland display\n"); return 1; }
    app.xkb = xkb_context_new(XKB_CONTEXT_NO_FLAGS);
    struct wl_registry *reg = wl_display_get_registry(app.display);
    wl_registry_add_listener(reg, &registry_listener, NULL);
    if (wl_display_roundtrip(app.display) < 0 || wl_display_roundtrip(app.display) < 0) return 1;
    if (!app.compositor || !app.shm || !app.wm) {
        fprintf(stderr, "omabox-fixture: the compositor lacks wl_compositor, wl_shm or xdg_wm_base\n");
        return 1;
    }
    say("start", ",\"windows\":%d", n);
    app.n = n;
    for (int i = 0; i < n; i++) {
        struct win *w = &app.wins[i];
        w->i = i;
        w->scale = 1;
        w->surface = wl_compositor_create_surface(app.compositor);
        wl_surface_add_listener(w->surface, &surface_listener, w);
        w->xdg = xdg_wm_base_get_xdg_surface(app.wm, w->surface);
        xdg_surface_add_listener(w->xdg, &xdg_listener, w);
        w->top = xdg_surface_get_toplevel(w->xdg);
        xdg_toplevel_add_listener(w->top, &toplevel_listener, w);
        char t[512];
        if (n == 1) snprintf(t, sizeof(t), "%s", title);
        else snprintf(t, sizeof(t), "%s %d", title, i);
        xdg_toplevel_set_title(w->top, t);
        xdg_toplevel_set_app_id(w->top, app_id);
        wl_surface_commit(w->surface);
    }

    int64_t all_drawn = 0;
    const char *why = "close";
    for (;;) {
        if (signalled) { why = signalled < 0 ? "close" : "signal"; break; }
        int64_t now = now_ms(), wake = -1;
        int drawn = 0;
        for (int i = 0; i < n; i++) {
            struct win *w = &app.wins[i];
            if (w->dirty && w->configured_at) {
                int64_t at = w->drawn ? now : w->configured_at + app.first_frame_ms;
                if (at <= now) { if (!draw(w)) { fprintf(stderr, "omabox-fixture: no buffer\n"); return 1; } }
                else if (wake < 0 || at < wake) wake = at;
            }
            drawn += w->drawn;
        }
        if (drawn == n && !all_drawn) all_drawn = now;
        if (all_drawn && for_ms >= 0) {
            if (now >= all_drawn + for_ms) { why = "for"; break; }
            if (wake < 0 || all_drawn + for_ms < wake) wake = all_drawn + for_ms;
        }
        while (wl_display_prepare_read(app.display) != 0)
            if (wl_display_dispatch_pending(app.display) < 0) return 1;
        if (wl_display_flush(app.display) < 0 && errno != EAGAIN) { wl_display_cancel_read(app.display); return 1; }
        struct pollfd p = {.fd = wl_display_get_fd(app.display), .events = POLLIN};
        int r = poll(&p, 1, wake < 0 ? -1 : (int)(wake - now_ms() > 0 ? wake - now_ms() : 0));
        if (r > 0 && (p.revents & POLLIN)) {
            if (wl_display_read_events(app.display) < 0) { fprintf(stderr, "omabox-fixture: the connection is gone\n"); return 1; }
        } else {
            wl_display_cancel_read(app.display);
            if (r > 0 && (p.revents & (POLLERR | POLLHUP))) { fprintf(stderr, "omabox-fixture: the connection is gone\n"); return 1; }
        }
        if (wl_display_dispatch_pending(app.display) < 0) { fprintf(stderr, "omabox-fixture: the connection is gone\n"); return 1; }
    }
    say("exit", ",\"why\":\"%s\",\"code\":%d", why, exit_code);
    wl_display_flush(app.display);
    return exit_code;
}
