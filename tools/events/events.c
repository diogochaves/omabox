// omabox-events: record the box's Hyprland events (its .socket2.sock) in a file, one line each, with
// the time it came.
//
//   omabox-events FILE
//
// Started with the box by its Hyprland (share/hyprland.lua); `omabox events` reads FILE (NOTES
// finding 108). Each line is "SECONDS.MILLIS EVENT>>DATA", written with one write() to a file opened
// with O_APPEND: a reader never sees half a line, and nothing is ever truncated or padded, so a byte
// offset into the file stays a place in it (`omabox events --mark`). Lines of its own are "omabox>>...".
// It stops writing at MAX_BYTES (the log is on the disk the box HOME is on: an app that retitles its
// window every frame should not fill it). Ends when Hyprland closes the socket; dies with the box.
// Refuses outside a box: on the host it would record the real session's events.
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>

#define MAX_BYTES (256L << 20)
#define MAX_LINE (64 << 10)   // longer lines (a huge title) are cut here

static int out = -1;
static long written;
static int stopped;

// One line, stamped, in one write.
static void put(const char *s, size_t n) {
    static char buf[MAX_LINE + 64];
    if (stopped) return;
    if (n > MAX_LINE) n = MAX_LINE;
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    int h = snprintf(buf, 64, "%lld.%03ld ", (long long)ts.tv_sec, ts.tv_nsec / 1000000);
    memcpy(buf + h, s, n);
    buf[h + n] = '\n';
    size_t len = h + n + 1;
    if (written + (long)len > MAX_BYTES) {   // the last line, then nothing
        stopped = 1;
        len = h + snprintf(buf + h, sizeof(buf) - h, "omabox>>stopped: the log reached %ld MB\n", MAX_BYTES >> 20);
    }
    ssize_t w = write(out, buf, len);
    if (w > 0) written += w;
}

static void say(const char *s) { put(s, strlen(s)); }

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: omabox-events FILE\n");
        return 2;
    }
    if (access("/opt/omabox/share", F_OK) != 0) {
        fprintf(stderr, "omabox-events: only runs inside an omabox box\n");
        return 2;
    }
    const char *rt = getenv("XDG_RUNTIME_DIR"), *sig = getenv("HYPRLAND_INSTANCE_SIGNATURE");
    if (!rt || !sig || strchr(sig, '/')) {
        fprintf(stderr, "omabox-events: no XDG_RUNTIME_DIR or HYPRLAND_INSTANCE_SIGNATURE\n");
        return 1;
    }
    out = open(argv[1], O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC | O_NOFOLLOW, 0644);
    if (out < 0) { fprintf(stderr, "omabox-events: %s: %s\n", argv[1], strerror(errno)); return 1; }
    off_t end = lseek(out, 0, SEEK_END);
    written = end > 0 ? end : 0;
    // The socket's path, from its directory: the whole path can pass the 108 bytes of an address.
    char dir[4096];
    snprintf(dir, sizeof(dir), "%s/hypr/%s", rt, sig);
    struct sockaddr_un addr = {.sun_family = AF_UNIX};
    strcpy(addr.sun_path, ".socket2.sock");
    int fd = -1;
    for (int i = 0; i < 200; i++) {   // Hyprland makes it as it starts: 10 s at most
        if (chdir(dir) == 0) {
            fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
            if (fd >= 0 && connect(fd, (struct sockaddr *)&addr, sizeof(addr)) == 0) break;
            if (fd >= 0) close(fd);
            fd = -1;
        }
        usleep(50000);
    }
    if (fd < 0) { say("omabox>>stopped: no event socket"); return 1; }
    if (chdir("/") != 0) { /* nothing depends on it */ }
    say("omabox>>listening");
    static char buf[MAX_LINE];
    size_t have = 0;
    for (;;) {
        ssize_t n = read(fd, buf + have, sizeof(buf) - have);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) break;
        have += n;
        size_t start = 0;
        for (size_t i = 0; i < have; i++) {
            if (buf[i] != '\n') continue;
            if (i > start) put(buf + start, i - start);
            start = i + 1;
        }
        if (start == 0 && have == sizeof(buf)) { put(buf, have); have = 0; continue; }   // a line too long
        memmove(buf, buf + start, have - start);
        have -= start;
    }
    if (have) put(buf, have);
    say("omabox>>stopped: the event socket closed");
    return 0;
}
