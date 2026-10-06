// omabox-relay: carry an omabox command from inside a sandbox (ai-jail) to the broker outside it.
//
//   omabox-relay call SOCKET [--fd N]... [--env NAME=VALUE]... -- ARG...     (in the sandbox)
//   omabox-relay listen [--socket PATH] -- OMABOX [ARG...]                   (the broker, outside)
//
// A sandboxed agent cannot enter a box's namespaces (its seccomp filter refuses setns and unshare),
// so `omabox` there hands its command to the broker: `call` sends its working directory, the
// arguments, the environment entries it was given and its stdin, stdout and stderr (plus any --fd)
// over the socket (SCM_RIGHTS), then waits for the exit status. The broker's `listen` accepts the
// connection, forks, and runs OMABOX ARG... in a session of its own with those fds as 0, 1, 2, 3...,
// so output goes straight to the caller and files the caller opened are the only files the command
// gets from it. The command is told who called (OMABOX_BROKER_PEER, the caller's pid, and
// OMABOX_BROKER_PIDFD, a pidfd for it) and gets nothing else from the caller's environment but the
// --env entries, each renamed OMABOX_RELAY_NAME: a variable the caller chose never reaches the
// broker under its own name (BASH_ENV, LD_PRELOAD, PATH would run the caller's code outside the
// sandbox). What the command may do for that caller is omabox's to decide (bin/omabox, broker mode).
// When the caller goes (killed, Ctrl-C), the command's session is killed too.
//
// `listen` takes the socket from systemd (socket activation, LISTEN_FDS) or makes --socket PATH
// itself. Only callers with its own uid are served.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>

#ifndef SO_PEERPIDFD
#define SO_PEERPIDFD 77
#endif

#define MAGIC "OMBX1\n"
#define MAX_FDS 8
#define MAX_PAYLOAD (1 << 20)
#define MAX_ARGS 4096
#define MAX_ENV 64

struct header {
    char magic[6];
    uint16_t nfds;
    uint32_t len;
};

static void die(const char *what) {
    fprintf(stderr, "omabox-relay: %s: %s\n", what, strerror(errno));
    exit(1);
}

static int write_all(int fd, const void *buf, size_t n) {
    const char *p = buf;
    while (n > 0) {
        ssize_t w = write(fd, p, n);
        if (w < 0 && errno == EINTR) continue;
        if (w <= 0) return -1;
        p += w, n -= (size_t)w;
    }
    return 0;
}

static int read_all(int fd, void *buf, size_t n) {
    char *p = buf;
    while (n > 0) {
        ssize_t r = read(fd, p, n);
        if (r < 0 && errno == EINTR) continue;
        if (r <= 0) return -1;
        p += r, n -= (size_t)r;
    }
    return 0;
}

// A variable name the broker may receive (as OMABOX_RELAY_NAME).
static int good_name(const char *s, size_t n) {
    if (n == 0 || n > 64) return 0;
    for (size_t i = 0; i < n; i++) {
        char c = s[i];
        int ok = c == '_' || (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || (i > 0 && c >= '0' && c <= '9');
        if (!ok) return 0;
    }
    return 1;
}

// --- call (inside the sandbox) -------------------------------------------------------------------

static int cmd_call(int argc, char **argv) {
    if (argc < 2) { fprintf(stderr, "usage: omabox-relay call SOCKET [--fd N]... [--env NAME=VALUE]... -- ARG...\n"); return 2; }
    const char *path = argv[1];
    int fds[MAX_FDS] = {0, 1, 2}, nfds = 3, i = 2;
    char *payload = malloc(MAX_PAYLOAD);
    size_t len = 0;
    if (!payload) die("malloc");
#define PUT(tag, s)                                                                      \
    do {                                                                                 \
        size_t l_ = strlen(s);                                                           \
        if (len + l_ + 3 > MAX_PAYLOAD) { fprintf(stderr, "omabox-relay: too long\n"); return 2; } \
        payload[len++] = (tag), payload[len++] = ':';                                    \
        memcpy(payload + len, (s), l_ + 1), len += l_ + 1;                               \
    } while (0)
    char cwd[4096];
    if (!getcwd(cwd, sizeof(cwd))) die("getcwd");
    PUT('C', cwd);
    for (; i < argc && strcmp(argv[i], "--") != 0; i++) {
        if (strcmp(argv[i], "--fd") == 0 && i + 1 < argc) {
            char *end;
            long fd = strtol(argv[++i], &end, 10);
            if (*end || fd < 3 || fcntl((int)fd, F_GETFD) < 0 || nfds == MAX_FDS) {
                fprintf(stderr, "omabox-relay: bad --fd %s\n", argv[i]);
                return 2;
            }
            fds[nfds++] = (int)fd;
        } else if (strcmp(argv[i], "--env") == 0 && i + 1 < argc) {
            const char *kv = argv[++i], *eq = strchr(kv, '=');
            if (!eq || !good_name(kv, (size_t)(eq - kv))) { fprintf(stderr, "omabox-relay: bad --env %s\n", kv); return 2; }
            PUT('E', kv);
        } else {
            fprintf(stderr, "omabox-relay: unknown option %s\n", argv[i]);
            return 2;
        }
    }
    if (i >= argc) { fprintf(stderr, "omabox-relay: no -- before the command\n"); return 2; }
    for (i++; i < argc; i++) PUT('A', argv[i]);

    struct sockaddr_un addr = {.sun_family = AF_UNIX};
    if (strlen(path) >= sizeof(addr.sun_path)) { fprintf(stderr, "omabox-relay: socket path too long\n"); return 1; }
    strcpy(addr.sun_path, path);
    int s = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (s < 0) die("socket");
    if (connect(s, (struct sockaddr *)&addr, sizeof(addr)) < 0) {
        fprintf(stderr, "omabox: cannot reach the omabox broker at %s: %s (is it on? omabox broker on, outside the sandbox)\n",
                path, strerror(errno));
        return 1;
    }
    struct header h = {.nfds = (uint16_t)nfds, .len = (uint32_t)len};
    memcpy(h.magic, MAGIC, 6);
    union {
        char buf[CMSG_SPACE(sizeof(int) * MAX_FDS)];
        struct cmsghdr align;
    } u;
    memset(&u, 0, sizeof(u));
    struct iovec iov = {.iov_base = &h, .iov_len = sizeof(h)};
    struct msghdr msg = {.msg_iov = &iov, .msg_iovlen = 1, .msg_control = u.buf,
                         .msg_controllen = CMSG_SPACE(sizeof(int) * (size_t)nfds)};
    struct cmsghdr *c = CMSG_FIRSTHDR(&msg);
    c->cmsg_level = SOL_SOCKET, c->cmsg_type = SCM_RIGHTS, c->cmsg_len = CMSG_LEN(sizeof(int) * (size_t)nfds);
    memcpy(CMSG_DATA(c), fds, sizeof(int) * (size_t)nfds);
    ssize_t w;
    do w = sendmsg(s, &msg, 0); while (w < 0 && errno == EINTR);
    if (w != (ssize_t)sizeof(h) || write_all(s, payload, len) < 0) die("send");
    int32_t status;
    if (read_all(s, &status, sizeof(status)) < 0) {
        fprintf(stderr, "omabox: the omabox broker ended the command without an exit status\n");
        return 1;
    }
    return status;
}

// --- listen (the broker, outside) ----------------------------------------------------------------

static char **g_cmd;   // OMABOX [ARG...]

// One connection: read the request, run the command for it, send its exit status.
static void serve(int conn) {
    struct ucred cred;
    socklen_t cl = sizeof(cred);
    if (getsockopt(conn, SOL_SOCKET, SO_PEERCRED, &cred, &cl) < 0 || cred.uid != getuid()) _exit(1);
    int pidfd = -1;
    socklen_t pl = sizeof(pidfd);
    // Without a pidfd the caller's pid could be reused before omabox reads its /proc: no service.
    if (getsockopt(conn, SOL_SOCKET, SO_PEERPIDFD, &pidfd, &pl) < 0 || pidfd < 0) _exit(1);
    struct timeval tv = {.tv_sec = 10};
    setsockopt(conn, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));

    struct header h;
    union {
        char buf[CMSG_SPACE(sizeof(int) * MAX_FDS)];
        struct cmsghdr align;
    } u;
    struct iovec iov = {.iov_base = &h, .iov_len = sizeof(h)};
    struct msghdr msg = {.msg_iov = &iov, .msg_iovlen = 1, .msg_control = u.buf, .msg_controllen = sizeof(u.buf)};
    ssize_t r;
    do r = recvmsg(conn, &msg, MSG_CMSG_CLOEXEC | MSG_WAITALL); while (r < 0 && errno == EINTR);
    if (r != (ssize_t)sizeof(h) || memcmp(h.magic, MAGIC, 6) != 0 || (msg.msg_flags & MSG_CTRUNC)) _exit(1);
    int fds[MAX_FDS], nfds = 0;
    for (struct cmsghdr *c = CMSG_FIRSTHDR(&msg); c; c = CMSG_NXTHDR(&msg, c)) {
        if (c->cmsg_level != SOL_SOCKET || c->cmsg_type != SCM_RIGHTS) _exit(1);
        int n = (int)((c->cmsg_len - CMSG_LEN(0)) / sizeof(int));
        if (nfds + n > MAX_FDS) _exit(1);
        memcpy(fds + nfds, CMSG_DATA(c), sizeof(int) * (size_t)n);
        nfds += n;
    }
    if (nfds != h.nfds || nfds < 3 || h.len == 0 || h.len > MAX_PAYLOAD) _exit(1);
    char *payload = malloc(h.len);
    if (!payload || read_all(conn, payload, h.len) < 0 || payload[h.len - 1] != '\0') _exit(1);

    // The command's environment: this process's own few, then the caller's --env entries renamed.
    static char *env[MAX_ENV + 32], *args[MAX_ARGS + 64];
    int ne = 0, na = 0;
    static const char *keep[] = {"HOME", "USER", "LOGNAME", "LANG", "XDG_RUNTIME_DIR", "XDG_CACHE_HOME",
                                 "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", NULL};
    for (int k = 0; keep[k]; k++) {
        const char *v = getenv(keep[k]);
        if (v && asprintf(&env[ne], "%s=%s", keep[k], v) > 0) ne++;
    }
    env[ne++] = "PATH=/usr/local/bin:/usr/bin";
    // bash sources ~/.bashrc when its stdin is a socket (as under rsh/ssh) and SHLVL is below 2,
    // and the caller's stdin may be one: omabox would run with whatever that sets.
    env[ne++] = "SHLVL=1";
    if (asprintf(&env[ne++], "OMABOX_BROKER_PEER=%d", (int)cred.pid) < 0) _exit(1);
    for (char **a = g_cmd; *a; a++) args[na++] = *a;
    int nuser = 0, have_cwd = 0;
    for (size_t off = 0; off < h.len;) {
        char *s = payload + off;
        size_t l = strlen(s);
        off += l + 1;
        if (l < 2 || s[1] != ':') _exit(1);
        char *v = s + 2;
        switch (s[0]) {
        case 'C':
            if (have_cwd++ || asprintf(&env[ne++], "OMABOX_BROKER_CWD=%s", v) < 0) _exit(1);
            break;
        case 'E': {
            char *eq = strchr(v, '=');
            if (!eq || !good_name(v, (size_t)(eq - v)) || nuser++ == MAX_ENV) _exit(1);
            if (asprintf(&env[ne++], "OMABOX_RELAY_%s", v) < 0) _exit(1);
            break;
        }
        case 'A':
            if (na == MAX_ARGS + 63) _exit(1);
            args[na++] = v;
            break;
        default:
            _exit(1);
        }
    }
    if (!have_cwd) _exit(1);
    int pidfd_at = 3 + (nfds - 3);   // the caller's extra fds are 3.., the pidfd right after them
    if (asprintf(&env[ne++], "OMABOX_BROKER_PIDFD=%d", pidfd_at) < 0) _exit(1);
    env[ne] = NULL, args[na] = NULL;

    signal(SIGCHLD, SIG_DFL);
    // The command's session exists before the caller's hangup is looked for (#109): a kill(-child)
    // before the child's setsid() found no group, and the command ran on. The pipe's write end is
    // the child's alone and closes at its exec (or exit), after setsid().
    int started[2];
    if (pipe2(started, O_CLOEXEC) < 0) _exit(1);
    pid_t child = fork();
    if (child < 0) _exit(1);
    if (child == 0) {
        close(started[0]);
        setsid();
        // Into place: the caller's fds as 0, 1, 2, 3..., the pidfd after them. Moved above the
        // targets first, so no dup2 overwrites one still to be placed.
        int hi[MAX_FDS + 1];
        for (int k = 0; k < nfds; k++) if ((hi[k] = fcntl(fds[k], F_DUPFD_CLOEXEC, 64)) < 0) _exit(126);
        if ((hi[nfds] = fcntl(pidfd, F_DUPFD_CLOEXEC, 64)) < 0) _exit(126);
        for (int k = 0; k <= nfds; k++) if (dup2(hi[k], k == nfds ? pidfd_at : k) < 0) _exit(126);
        if (chdir("/") < 0) _exit(126);
        execve(args[0], args, env);
        fprintf(stderr, "omabox-relay: %s: %s\n", args[0], strerror(errno));
        _exit(127);
    }
    close(started[1]);
    char c;
    while (read(started[0], &c, 1) < 0 && errno == EINTR) {}
    close(started[0]);
    for (int k = 0; k < nfds; k++) close(fds[k]);
    close(pidfd);
    int cfd = (int)syscall(SYS_pidfd_open, child, 0);
    if (cfd < 0) { kill(-child, SIGKILL); _exit(1); }
    // Until the command ends, or the caller goes: anything readable on the connection is its end
    // (a caller sends nothing after the request), and so is a hangup.
    struct pollfd p[2] = {{.fd = cfd, .events = POLLIN}, {.fd = conn, .events = POLLIN | POLLRDHUP}};
    int gone = 0;
    for (;;) {
        if (poll(p, 2, -1) < 0) { if (errno == EINTR) continue; break; }
        if (p[0].revents) break;
        if (p[1].revents) { gone = 1; break; }
    }
    if (gone) {
        kill(-child, SIGTERM);
        struct pollfd q = {.fd = cfd, .events = POLLIN};
        if (poll(&q, 1, 3000) == 0) kill(-child, SIGKILL);
    }
    int st;
    while (waitpid(child, &st, 0) < 0 && errno == EINTR) {}
    int32_t status = WIFEXITED(st) ? WEXITSTATUS(st) : 128 + WTERMSIG(st);
    if (!gone) write_all(conn, &status, sizeof(status));
    _exit(0);
}

#define MAX_HANDLERS 64
static volatile sig_atomic_t handlers;
static void reap_handlers(int sig) {
    (void)sig;
    int e = errno;
    while (waitpid(-1, NULL, WNOHANG) > 0) handlers--;
    errno = e;
}

static int cmd_listen(int argc, char **argv) {
    const char *path = NULL;
    int i = 1;
    for (; i < argc && strcmp(argv[i], "--") != 0; i++) {
        if (strcmp(argv[i], "--socket") == 0 && i + 1 < argc) path = argv[++i];
        else { fprintf(stderr, "omabox-relay: unknown option %s\n", argv[i]); return 2; }
    }
    if (i + 1 >= argc || argv[i + 1][0] != '/') { fprintf(stderr, "omabox-relay: listen needs -- /ABSOLUTE/omabox [ARG...]\n"); return 2; }
    g_cmd = argv + i + 1;
    int ls = -1;
    const char *lp = getenv("LISTEN_PID"), *lf = getenv("LISTEN_FDS");
    if (!path && lp && lf && atoi(lp) == getpid() && atoi(lf) == 1) {
        ls = 3;
        fcntl(ls, F_SETFD, FD_CLOEXEC);
    } else if (path) {
        struct sockaddr_un addr = {.sun_family = AF_UNIX};
        if (strlen(path) >= sizeof(addr.sun_path)) { fprintf(stderr, "omabox-relay: socket path too long\n"); return 1; }
        strcpy(addr.sun_path, path);
        ls = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
        if (ls < 0) die("socket");
        unlink(path);
        mode_t old = umask(0077);
        if (bind(ls, (struct sockaddr *)&addr, sizeof(addr)) < 0) die("bind");
        umask(old);
        if (listen(ls, 16) < 0) die("listen");
    } else {
        fprintf(stderr, "omabox-relay: listen needs --socket PATH or a socket from systemd\n");
        return 2;
    }
    unsetenv("LISTEN_PID"), unsetenv("LISTEN_FDS"), unsetenv("LISTEN_FDNAMES");
    // Connection handlers are counted (#109): a caller of the same uid flooding the socket gets at
    // most MAX_HANDLERS at once, the rest closed at once, instead of a fork each without bound.
    struct sigaction sa = {.sa_handler = reap_handlers, .sa_flags = SA_RESTART | SA_NOCLDSTOP};
    sigemptyset(&sa.sa_mask);
    sigaction(SIGCHLD, &sa, NULL);
    signal(SIGPIPE, SIG_IGN);
    for (;;) {
        int conn = accept4(ls, NULL, NULL, SOCK_CLOEXEC);
        if (conn < 0) { if (errno == EINTR || errno == ECONNABORTED) continue; die("accept"); }
        if (handlers >= MAX_HANDLERS) { close(conn); continue; }
        sigset_t chld, old;   // the count changed by this loop and the handler, never both at once
        sigemptyset(&chld); sigaddset(&chld, SIGCHLD);
        sigprocmask(SIG_BLOCK, &chld, &old);
        pid_t pid = fork();
        if (pid == 0) { sigprocmask(SIG_SETMASK, &old, NULL); close(ls); serve(conn); }
        if (pid > 0) handlers++;
        sigprocmask(SIG_SETMASK, &old, NULL);
        close(conn);
    }
}

int main(int argc, char **argv) {
    if (argc >= 2 && strcmp(argv[1], "call") == 0) return cmd_call(argc - 1, argv + 1);
    if (argc >= 2 && strcmp(argv[1], "listen") == 0) return cmd_listen(argc - 1, argv + 1);
    fprintf(stderr, "usage: omabox-relay call SOCKET [--fd N]... [--env NAME=VALUE]... -- ARG...\n"
                    "       omabox-relay listen [--socket PATH] -- /ABSOLUTE/omabox [ARG...]\n");
    return 2;
}
