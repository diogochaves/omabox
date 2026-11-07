// omabox-notices: record what reaches the box's notification server (#430, NOTES finding 680), one
// JSON line each, in a file of the box HOME (`omabox log notifications`, `omabox wait notification`).
//
//   omabox-notices FILE READY
//
// share/session.sh starts it as soon as the box's session bus is up, before anything that could send
// a notice, and waits for READY, which is made once the bus has taken it as a monitor
// (org.freedesktop.DBus.Monitoring.BecomeMonitor, as dbus-monitor and busctl monitor do). Its match
// rules are the notification interface's calls and signals, the replies and errors of whoever owns
// org.freedesktop.Notifications at the time, and the bus's own errors (what a Notify with no server
// gets). So it works whatever server the box has (the shell's, a plain box's, none yet), and it reads
// nothing of the host's bus: it only ever connects to the box's own.
//
// A line each (SECONDS.MILLIS: when the bus passed it on):
//   {"time":S,"event":"notify","id":N|null,"error":null|"NAME: TEXT","serial":N,"sender":":1.9","app_name":..,
//    "replaces_id":N,"app_icon":..,"summary":..,"body":..,"actions":[{"key":..,"label":..}],
//    "hints":{"urgency":2,"category":"..",...},"expire_timeout":-1}
//   {"time":S,"event":"close","id":N,"sender":..}         CloseNotification, an app's
//   {"time":S,"event":"closed","id":N,"reason":R}         NotificationClosed, the server's (1 expired,
//                                                         2 dismissed, 3 closed by CloseNotification)
//   {"time":S,"event":"action","id":N,"action":KEY}       ActionInvoked, the server's
//   {"time":S,"event":"reply","serial":N,"sender":..,"call_time":S,"id":N|null,"error":null|..}
//                                                         a late reply: see below
//   {"time":S,"event":"omabox","note":..}                 its own: listening, stopped
// A Notify is written when its reply (the id) or error comes, or REPLY_MS after it with neither (id
// null, error "no reply in 5 s"); one sent wanting no reply at once (id null, no serial). One written
// without its reply is still waited for until LATE_MS after it (D-Bus's default reply timeout): a
// reply that comes by then is a "reply" line naming the call by its sender and serial (and its time),
// which `omabox log notifications` joins to the notify line (#430, finding 688). Hints keep their basic
// values; a structured one (image-data) is its D-Bus signature. A string past its cap is cut at a
// character and ends in "…". Each line is one write() to a file opened O_APPEND, never truncated, so
// a byte offset stays a place in it (`wait notification --since`); it stops at MAX_BYTES. Idle it
// sleeps on the bus's socket: no timer runs unless a Notify waits for its reply.
//
// Refuses outside a box (tools/common/box.h): on the host it would watch the real session's bus.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <math.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <systemd/sd-bus.h>
#include <time.h>
#include <unistd.h>

#include "../common/box.h"

#define IFACE "org.freedesktop.Notifications"
#define MAX_BYTES (16L << 20)
#define MAX_PENDING 64
#define REPLY_MS 5000
#define LATE_MS 25000         // D-Bus's default reply timeout (libdbus's, GDBus's, sd-bus's)
#define CAP_TEXT (16 << 10)   // summary, body
#define CAP_WORD 1024         // a name, a key, a label, a hint's string
#define MAX_ITEMS 64          // actions (pairs), hints

struct buf {
    char *p;
    size_t n, cap;
};

static void put(struct buf *b, const char *s, size_t n) {
    if (b->n + n + 1 > b->cap) {
        size_t c = b->cap ? b->cap : 256;
        while (b->n + n + 1 > c) c *= 2;
        char *p = realloc(b->p, c);
        if (!p) { fprintf(stderr, "omabox-notices: out of memory\n"); exit(1); }
        b->p = p, b->cap = c;
    }
    memcpy(b->p + b->n, s, n);
    b->n += n;
    b->p[b->n] = 0;
}

static void say(struct buf *b, const char *s) { put(b, s, strlen(s)); }

static void sayf(struct buf *b, const char *fmt, ...) {
    char t[128];
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(t, sizeof(t), fmt, ap);
    va_end(ap);
    if (n > 0) put(b, t, (size_t)n < sizeof(t) ? (size_t)n : sizeof(t) - 1);
}

// S as a JSON string, cut at CAP bytes (at a character: sd-bus has checked it is UTF-8).
static void jstr(struct buf *b, const char *s, size_t cap) {
    if (!s) { say(b, "null"); return; }
    size_t n = strlen(s);
    int cut = 0;
    if (n > cap) {
        n = cap;
        while (n > 0 && ((unsigned char)s[n] & 0xC0) == 0x80) n--;
        cut = 1;
    }
    put(b, "\"", 1);
    for (size_t i = 0; i < n; i++) {
        unsigned char c = (unsigned char)s[i];
        if (c == '"' || c == '\\') { char e[2] = {'\\', (char)c}; put(b, e, 2); }
        else if (c == '\n') say(b, "\\n");
        else if (c == '\t') say(b, "\\t");
        else if (c == '\r') say(b, "\\r");
        else if (c < 0x20 || c == 0x7f) sayf(b, "\\u%04x", c);
        else put(b, (const char *)&c, 1);
    }
    if (cut) say(b, "\xe2\x80\xa6");
    put(b, "\"", 1);
}

static int out = -1;
static long written;
static int stopped;

static void stamp(struct buf *b, const struct timespec *ts, const char *event) {
    sayf(b, "{\"time\":%lld.%03ld,\"event\":\"%s\"", (long long)ts->tv_sec, ts->tv_nsec / 1000000, event);
}

static struct timespec now_real(void) {
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return ts;
}

static uint64_t now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000 + (uint64_t)ts.tv_nsec / 1000000;
}

// One line (B, which ends in its closing brace), in one write; past MAX_BYTES a last line, then none.
static void emit(struct buf *b) {
    if (stopped) return;
    put(b, "\n", 1);
    if (written + (long)b->n > MAX_BYTES) {
        stopped = 1;
        struct timespec ts = now_real();
        b->n = 0;
        stamp(b, &ts, "omabox");
        sayf(b, ",\"note\":\"stopped: the log reached %ld MB\"}\n", MAX_BYTES >> 20);
    }
    size_t off = 0;
    while (off < b->n) {
        ssize_t w = write(out, b->p + off, b->n - off);
        if (w < 0 && errno == EINTR) continue;
        if (w <= 0) break;
        off += (size_t)w;
    }
    written += (long)off;
}

static void note(const char *what) {
    struct buf b = {0};
    struct timespec ts = now_real();
    stamp(&b, &ts, "omabox");
    say(&b, ",\"note\":");
    jstr(&b, what, CAP_WORD);
    say(&b, "}");
    emit(&b);
    free(b.p);
}

// A Notify waiting for its reply: who sent it, its serial, when, and its line from "sender" on; once
// written (late), only who, which and when, until LATE_MS.
struct pending {
    int used, written;
    char *sender;
    uint64_t cookie, at_ms;
    struct timespec ts;
    struct buf rest;
};
static struct pending pend[MAX_PENDING];

// ",\"id\":N|null,\"error\":null|\"..\""
static void id_error(struct buf *b, const uint32_t *id, const char *error) {
    if (id) sayf(b, ",\"id\":%" PRIu32, *id);
    else say(b, ",\"id\":null");
    say(b, ",\"error\":");
    jstr(b, error, CAP_WORD);
}

// SERIAL is 0 for a call that wants no reply (then the line has none).
static void write_notify(const struct timespec *ts, const uint32_t *id, const char *error, uint64_t serial,
                         const struct buf *rest) {
    struct buf b = {0};
    stamp(&b, ts, "notify");
    id_error(&b, id, error);
    if (serial) sayf(&b, ",\"serial\":%" PRIu64, serial);
    put(&b, rest->p, rest->n);
    say(&b, "}");
    emit(&b);
    free(b.p);
}

static void drop(struct pending *p) {
    free(p->sender);
    free(p->rest.p);
    memset(p, 0, sizeof(*p));
}

// Its reply (ID or ERROR) came: the notify line, or, when that is out already, a reply line.
static void settle(struct pending *p, const uint32_t *id, const char *error) {
    if (!p->written) {
        write_notify(&p->ts, id, error, p->cookie, &p->rest);
        drop(p);
        return;
    }
    struct buf b = {0};
    struct timespec ts = now_real();
    stamp(&b, &ts, "reply");
    sayf(&b, ",\"serial\":%" PRIu64 ",\"sender\":", p->cookie);
    jstr(&b, p->sender, CAP_WORD);
    sayf(&b, ",\"call_time\":%lld.%03ld", (long long)p->ts.tv_sec, p->ts.tv_nsec / 1000000);
    id_error(&b, id, error);
    say(&b, "}");
    emit(&b);
    free(b.p);
    drop(p);
}

// Its notify line goes out without an id (WHY); the reply is still waited for, until LATE_MS.
static void give_up(struct pending *p, const char *why) {
    write_notify(&p->ts, NULL, why, p->cookie, &p->rest);
    free(p->rest.p);
    p->rest = (struct buf){0};
    p->written = 1;
}

static uint64_t due(const struct pending *p) { return p->at_ms + (p->written ? LATE_MS : REPLY_MS); }

// The ones whose reply did not come in REPLY_MS (written without an id), and those written so whose
// reply did not come in LATE_MS (forgotten), oldest first; and how long until the next one is due.
static uint64_t expire(void) {
    uint64_t t = now_ms(), next = UINT64_MAX;
    for (;;) {
        struct pending *old = NULL;
        for (int i = 0; i < MAX_PENDING; i++)
            if (pend[i].used && (!old || due(&pend[i]) < due(old))) old = &pend[i];
        if (!old) break;
        if (due(old) > t) { next = due(old) - t; break; }
        if (old->written) drop(old);
        else give_up(old, "no reply in 5 s");
    }
    return next;
}

static int is_basic(const char *sig) {
    return sig && sig[0] && !sig[1] && strchr("ybnqiuxtdsogh", sig[0]);
}

// The variant the message is at, as JSON: a basic value as it is, anything else as its signature.
static int variant(sd_bus_message *m, struct buf *b) {
    char t;
    const char *contents = NULL;
    int r = sd_bus_message_peek_type(m, &t, &contents);
    if (r <= 0 || t != 'v') return r < 0 ? r : -EBADMSG;
    if (!is_basic(contents)) {
        jstr(b, contents, 64);
        return sd_bus_message_skip(m, "v");
    }
    if ((r = sd_bus_message_enter_container(m, 'v', contents)) < 0) return r;
    union {
        uint8_t y; int bo; int16_t n; uint16_t q; int32_t i; uint32_t u;
        int64_t x; uint64_t t; double d; const char *s; int h;
    } v;
    if ((r = sd_bus_message_read_basic(m, contents[0], &v)) < 0) return r;
    switch (contents[0]) {
    case 'y': sayf(b, "%u", v.y); break;
    case 'b': say(b, v.bo ? "true" : "false"); break;
    case 'n': sayf(b, "%d", v.n); break;
    case 'q': sayf(b, "%u", v.q); break;
    case 'i': sayf(b, "%" PRId32, v.i); break;
    case 'u': sayf(b, "%" PRIu32, v.u); break;
    case 'x': sayf(b, "%" PRId64, v.x); break;
    case 't': sayf(b, "%" PRIu64, v.t); break;
    case 'd': if (isfinite(v.d)) sayf(b, "%.17g", v.d); else say(b, "null"); break;
    case 'h': say(b, "\"h\""); break;
    default: jstr(b, v.s, CAP_WORD); break;
    }
    return sd_bus_message_exit_container(m);
}

// Notify's arguments (susssasa{sv}i) as the line's rest, from "sender" on.
static int notify_rest(sd_bus_message *m, const char *sender, struct buf *b) {
    const char *app, *icon, *summary, *body;
    uint32_t rid;
    int32_t tmo;
    int r = sd_bus_message_read(m, "susss", &app, &rid, &icon, &summary, &body);
    if (r < 0) return r;
    say(b, ",\"sender\":");
    jstr(b, sender, CAP_WORD);
    say(b, ",\"app_name\":");
    jstr(b, app, CAP_WORD);
    sayf(b, ",\"replaces_id\":%" PRIu32 ",\"app_icon\":", rid);
    jstr(b, icon, CAP_WORD);
    say(b, ",\"summary\":");
    jstr(b, summary, CAP_TEXT);
    say(b, ",\"body\":");
    jstr(b, body, CAP_TEXT);
    say(b, ",\"actions\":[");
    if ((r = sd_bus_message_enter_container(m, 'a', "s")) < 0) return r;
    const char *k;
    int n = 0;
    for (;;) {
        if ((r = sd_bus_message_read(m, "s", &k)) < 0) return r;
        if (r == 0) break;
        if (n / 2 >= MAX_ITEMS) { n++; continue; }
        if (n % 2 == 0) {
            say(b, n ? "},{\"key\":" : "{\"key\":");
            jstr(b, k, CAP_WORD);
        } else {
            say(b, ",\"label\":");
            jstr(b, k, CAP_WORD);
        }
        n++;
    }
    if (n % 2 == 1 && n / 2 < MAX_ITEMS) say(b, ",\"label\":null");
    if (n) say(b, "}");
    say(b, "],\"hints\":{");
    if ((r = sd_bus_message_exit_container(m)) < 0) return r;
    if ((r = sd_bus_message_enter_container(m, 'a', "{sv}")) < 0) return r;
    int h = 0;
    for (;;) {
        if ((r = sd_bus_message_enter_container(m, 'e', "sv")) < 0) return r;
        if (r == 0) break;
        if ((r = sd_bus_message_read(m, "s", &k)) < 0) return r;
        if (h < MAX_ITEMS) {
            if (h) say(b, ",");
            jstr(b, k, CAP_WORD);
            say(b, ":");
            if ((r = variant(m, b)) < 0) return r;
        } else if ((r = sd_bus_message_skip(m, "v")) < 0) {
            return r;
        }
        h++;
        if ((r = sd_bus_message_exit_container(m)) < 0) return r;
    }
    if ((r = sd_bus_message_exit_container(m)) < 0) return r;
    if ((r = sd_bus_message_read(m, "i", &tmo)) < 0) return r;
    sayf(b, "},\"expire_timeout\":%" PRId32, tmo);
    return 0;
}

static int is(const char *a, const char *b) { return a && b && strcmp(a, b) == 0; }

static void on_call(sd_bus_message *m, const struct timespec *ts) {
    const char *member = sd_bus_message_get_member(m), *sender = sd_bus_message_get_sender(m);
    if (is(member, "Notify")) {
        struct buf rest = {0};
        int r = notify_rest(m, sender, &rest);
        if (r < 0) {
            rest.n = 0;
            say(&rest, "a Notify call that could not be read (");
            say(&rest, strerror(-r));
            say(&rest, "): its arguments are not susssasa{sv}i?");
            note(rest.p);
            free(rest.p);
            return;
        }
        uint64_t cookie = 0;
        if (sd_bus_message_get_expect_reply(m) <= 0 || !sender || sd_bus_message_get_cookie(m, &cookie) < 0) {
            write_notify(ts, NULL, NULL, 0, &rest);
            free(rest.p);
            return;
        }
        struct pending *slot = NULL;
        for (int i = 0; i < MAX_PENDING && !slot; i++)
            if (!pend[i].used) slot = &pend[i];
        if (!slot) {   // full: the oldest one written already is forgotten, else the oldest goes out as it is
            for (int i = 0; i < MAX_PENDING; i++)
                if (!slot || pend[i].written > slot->written ||
                    (pend[i].written == slot->written && pend[i].at_ms < slot->at_ms))
                    slot = &pend[i];
            if (slot->written) drop(slot);
            else {
                give_up(slot, "no reply yet (too many waiting)");
                drop(slot);
            }
        }
        *slot = (struct pending){.used = 1, .sender = strdup(sender), .cookie = cookie, .at_ms = now_ms(),
                                 .ts = *ts, .rest = rest};
        if (!slot->sender) {
            write_notify(ts, NULL, NULL, 0, &rest);
            drop(slot);
        }
    } else if (is(member, "CloseNotification")) {
        uint32_t id;
        if (sd_bus_message_read(m, "u", &id) < 0) return;
        struct buf b = {0};
        stamp(&b, ts, "close");
        sayf(&b, ",\"id\":%" PRIu32 ",\"sender\":", id);
        jstr(&b, sender, CAP_WORD);
        say(&b, "}");
        emit(&b);
        free(b.p);
    }
}

static void on_signal(sd_bus_message *m, const struct timespec *ts) {
    const char *member = sd_bus_message_get_member(m);
    uint32_t id, reason;
    const char *key;
    struct buf b = {0};
    if (is(member, "NotificationClosed") && sd_bus_message_read(m, "uu", &id, &reason) >= 0) {
        stamp(&b, ts, "closed");
        sayf(&b, ",\"id\":%" PRIu32 ",\"reason\":%" PRIu32 "}", id, reason);
    } else if (is(member, "ActionInvoked") && sd_bus_message_read(m, "us", &id, &key) >= 0) {
        stamp(&b, ts, "action");
        sayf(&b, ",\"id\":%" PRIu32 ",\"action\":", id);
        jstr(&b, key, CAP_WORD);
        say(&b, "}");
    }
    if (b.n) emit(&b);
    free(b.p);
}

static void on_reply(sd_bus_message *m, uint8_t type) {
    uint64_t rc;
    const char *dest = sd_bus_message_get_destination(m);
    if (!dest || sd_bus_message_get_reply_cookie(m, &rc) < 0) return;
    struct pending *p = NULL;
    for (int i = 0; i < MAX_PENDING && !p; i++)
        if (pend[i].used && pend[i].cookie == rc && is(pend[i].sender, dest)) p = &pend[i];
    if (!p) return;
    if (type == SD_BUS_MESSAGE_METHOD_RETURN) {
        uint32_t id;
        if (sd_bus_message_read(m, "u", &id) > 0) settle(p, &id, NULL);
        else settle(p, NULL, "a reply that is not an id");
        return;
    }
    const sd_bus_error *e = sd_bus_message_get_error(m);
    struct buf t = {0};
    say(&t, e && e->name ? e->name : "an error");
    if (e && e->message) { say(&t, ": "); say(&t, e->message); }
    settle(p, NULL, t.p);
    free(t.p);
}

int main(int argc, char **argv) {
    if (argc != 3) {
        fprintf(stderr, "usage: omabox-notices FILE READY\n");
        return 2;
    }
    if (!omabox_inside_box()) {
        fprintf(stderr, "omabox-notices: only runs inside an omabox box\n");
        return 2;
    }
    const char *addr = getenv("DBUS_SESSION_BUS_ADDRESS");
    if (!addr || !*addr) {
        fprintf(stderr, "omabox-notices: no DBUS_SESSION_BUS_ADDRESS\n");
        return 1;
    }
    out = open(argv[1], O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC | O_NOFOLLOW, 0644);
    if (out < 0) { fprintf(stderr, "omabox-notices: %s: %s\n", argv[1], strerror(errno)); return 1; }
    off_t end = lseek(out, 0, SEEK_END);
    written = end > 0 ? end : 0;

    sd_bus *bus = NULL;
    sd_bus_message *call = NULL;
    sd_bus_error err = SD_BUS_ERROR_NULL;
    int r;
    if ((r = sd_bus_new(&bus)) < 0 || (r = sd_bus_set_monitor(bus, 1)) < 0 ||
        (r = sd_bus_set_bus_client(bus, 1)) < 0 || (r = sd_bus_set_address(bus, addr)) < 0 ||
        (r = sd_bus_start(bus)) < 0) {
        fprintf(stderr, "omabox-notices: the session bus: %s\n", strerror(-r));
        return 1;
    }
    static const char *const rules[] = {
        "type='method_call',interface='" IFACE "'",
        "type='signal',interface='" IFACE "'",
        "type='method_return',sender='" IFACE "'",
        "type='error',sender='" IFACE "'",
        "type='error',sender='org.freedesktop.DBus'",
    };
    if ((r = sd_bus_message_new_method_call(bus, &call, "org.freedesktop.DBus", "/org/freedesktop/DBus",
                                            "org.freedesktop.DBus.Monitoring", "BecomeMonitor")) < 0 ||
        (r = sd_bus_message_open_container(call, 'a', "s")) < 0) {
        fprintf(stderr, "omabox-notices: BecomeMonitor: %s\n", strerror(-r));
        return 1;
    }
    for (size_t i = 0; i < sizeof(rules) / sizeof(rules[0]); i++)
        if ((r = sd_bus_message_append_basic(call, 's', rules[i])) < 0) break;
    uint32_t flags = 0;
    if (r >= 0) r = sd_bus_message_close_container(call);
    if (r >= 0) r = sd_bus_message_append_basic(call, 'u', &flags);
    if (r >= 0) r = sd_bus_call(bus, call, 10 * 1000000ULL, &err, NULL);
    if (r < 0) {
        fprintf(stderr, "omabox-notices: BecomeMonitor: %s\n", err.message ? err.message : strerror(-r));
        return 1;
    }
    sd_bus_message_unref(call);
    note("listening");
    int fd = open(argv[2], O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC | O_NOFOLLOW, 0644);
    if (fd >= 0) close(fd);

    for (;;) {
        sd_bus_message *m = NULL;
        r = sd_bus_process(bus, &m);
        if (r < 0) break;
        if (m) {
            struct timespec ts = now_real();
            uint8_t type = 0;
            sd_bus_message_get_type(m, &type);
            if (sd_bus_message_is_signal(m, "org.freedesktop.DBus.Local", "Disconnected") > 0) {
                sd_bus_message_unref(m);
                break;
            }
            int ours = is(sd_bus_message_get_interface(m), IFACE);
            if (type == SD_BUS_MESSAGE_METHOD_CALL && ours) on_call(m, &ts);
            else if (type == SD_BUS_MESSAGE_SIGNAL && ours) on_signal(m, &ts);
            else if (type == SD_BUS_MESSAGE_METHOD_RETURN || type == SD_BUS_MESSAGE_METHOD_ERROR) on_reply(m, type);
            sd_bus_message_unref(m);
            continue;
        }
        if (r > 0) continue;
        uint64_t next = expire();
        r = sd_bus_wait(bus, next == UINT64_MAX ? UINT64_MAX : next * 1000);
        if (r < 0 && r != -EINTR) break;
    }
    for (int i = 0; i < MAX_PENDING; i++)
        if (pend[i].used && !pend[i].written) settle(&pend[i], NULL, "no reply: the bus closed");
    note("stopped: the bus closed");
    return 0;
}
