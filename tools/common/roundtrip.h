// omabox_roundtrip(): wl_display_roundtrip with a deadline (finding 177), for the tools (keyboard,
// pointer; still's first one, finding 181). A compositor that is alive but stopped (SIGSTOP, a
// deadlock) never answers, and a plain roundtrip waited forever, holding the caller's `omabox
// keys/click` (and with -m, its modifiers) or `wait`. After a failure the tool exits: the sync
// callback may still be pending, with this function's frame as its data.
#ifndef OMABOX_ROUNDTRIP_H
#define OMABOX_ROUNDTRIP_H

#include <errno.h>
#include <poll.h>
#include <stdint.h>
#include <time.h>
#include <wayland-client.h>

static inline int64_t omabox_now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

static void omabox_synced(void *data, struct wl_callback *cb, uint32_t t) {
    (void)t;
    *(int *)data = 1;
    wl_callback_destroy(cb);
}
static const struct wl_callback_listener omabox_synced_listener = {omabox_synced};

// 0 answered, -1 the connection failed, -2 no answer within ms.
static inline int omabox_roundtrip(struct wl_display *d, int ms) {
    int done = 0;
    struct wl_callback *cb = wl_display_sync(d);
    if (!cb) return -1;
    wl_callback_add_listener(cb, &omabox_synced_listener, &done);
    int64_t end = omabox_now_ms() + ms;
    while (!done) {
        while (wl_display_prepare_read(d) != 0)
            if (wl_display_dispatch_pending(d) < 0) return -1;
        if (done) { wl_display_cancel_read(d); break; }
        int blocked = 0;
        if (wl_display_flush(d) < 0) {
            if (errno != EAGAIN) { wl_display_cancel_read(d); return -1; }
            blocked = 1;   // the compositor is not reading: wait to write as well
        }
        int left = (int)(end - omabox_now_ms());
        if (left <= 0) { wl_display_cancel_read(d); return -2; }
        struct pollfd p = {.fd = wl_display_get_fd(d), .events = POLLIN | (blocked ? POLLOUT : 0)};
        int n = poll(&p, 1, left);
        if (n > 0 && (p.revents & POLLIN)) {
            if (wl_display_read_events(d) < 0) return -1;
        } else {
            wl_display_cancel_read(d);
            if (n < 0 && errno != EINTR) return -1;
            if (n > 0 && (p.revents & (POLLERR | POLLHUP))) return -1;
        }
        if (wl_display_dispatch_pending(d) < 0) return -1;
    }
    return 0;
}

#endif
