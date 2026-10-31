// omabox-polkit: a stand-in polkit authority for `omabox up --polkit` (#258, NOTES finding 627), so
// a polkit agent in a box (a shell plugin's PolkitAgent, a GTK or Qt agent) registers and is asked,
// and what ends a request is the agent's own code: polkit's agent library, its helper protocol, the
// agent's flow. It never reaches the host's polkit: it is the only authority on the box's own system
// bus, a dbus-daemon in the box (share/session.sh), and it decides nothing for any service.
//
//   omabox-polkit serve SESSION     the authority (org.freedesktop.PolicyKit1) and the agent helper
//   omabox-polkit ask [--action ID] [--message TEXT] [--icon NAME] [--prompt TEXT] [--accept PASSWORD]
//                     [--detail KEY=VALUE]... [--timeout SECONDS] [--delay MS] [--json]
//   omabox-polkit agents [--json]
//
// serve: owns org.freedesktop.PolicyKit1 on the box's bus at /run/user/UID/omabox-polkit/bus, as
// polkitd does on the system bus, and keeps its agents as polkitd does: one per subject (a session,
// SESSION being the box's own, or a process), an agent registering for another session refused,
// dropped when it leaves the bus. It listens on /run/polkit/agent-helper.socket, where polkit's agent
// library (polkit 121 and later) looks first for the helper that runs PAM: in a box that socket is
// this one, which says polkit-agent-helper-1's lines (a password prompt, SUCCESS or FAILURE) and
// takes the password `ask` was given in place of PAM. No PAM, no setuid, no root: nothing in a box
// could get them anyway (bwrap's no_new_privs), and nothing here can authorize anything outside it.
//
// ask: one request, as polkitd makes one: BeginAuthentication on the agent (the box session's, else
// the last one registered), with a cookie, the box user as the identity, and the action, message,
// icon and details given. Says how it ended, as the agent ended it: authenticated (the helper took the
// accepted password and the agent returned), not-authenticated (the agent returned without it),
// cancelled (the agent cancelled: its user dismissed the dialog), timeout (no end in SECONDS, 60 by
// default: the authority cancelled it, CancelAuthentication, as polkitd does when its caller gives
// up), agent-gone (it left the bus) or no-agent; and how many passwords the helper was given, how many
// wrong. --delay MS: the helper answers each password MS later (a "checking" state to see).
// Exit 0 authenticated, 124 timeout, 1 any other end, 2 the authority is not there or a usage error.
// `omabox polkit` runs it in the box and checks its words first.
//
// agents: who is registered: subject, bus name, object path, fallback.
//
// Refuses outside a box (tools/common/box.h): on the host it would ask the real system bus.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <getopt.h>
#include <inttypes.h>
#include <pwd.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/random.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <systemd/sd-bus.h>
#include <systemd/sd-event.h>
#include <unistd.h>

#include "../common/box.h"

#define AUTH_PATH "/org/freedesktop/PolicyKit1/Authority"
#define AUTH_IFACE "org.freedesktop.PolicyKit1.Authority"
#define AUTH_NAME "org.freedesktop.PolicyKit1"
#define AGENT_IFACE "org.freedesktop.PolicyKit1.AuthenticationAgent"
#define TEST_PATH "/org/omabox/Polkit"
#define TEST_IFACE "org.omabox.Polkit"
#define ERR_FAILED "org.freedesktop.PolicyKit1.Error.Failed"
#define ERR_CANCELLED "org.freedesktop.PolicyKit1.Error.Cancelled"
#define ERR_NOT_AUTHORIZED "org.freedesktop.PolicyKit1.Error.NotAuthorized"
#define ERR_NOT_SUPPORTED "org.freedesktop.PolicyKit1.Error.NotSupported"
#define HELPER_DIR "/run/polkit"
#define HELPER_SOCKET HELPER_DIR "/agent-helper.socket"
#define MAX_LINE 4096
#define USEC_PER_SEC 1000000ULL

static sd_bus *bus;
static sd_event *ev;
static char *session;   // the box's session id
static char *user;      // the box user's name, the identity a request offers
static uint32_t uid;

static char dir_path[128], bus_path[160], ready_path[160];

static void paths(void) {
    uid = getuid();
    snprintf(dir_path, sizeof dir_path, "/run/user/%u/omabox-polkit", uid);
    snprintf(bus_path, sizeof bus_path, "%s/bus", dir_path);
    snprintf(ready_path, sizeof ready_path, "%s/ready", dir_path);
}

static int connect_bus(void) {
    char addr[200];
    snprintf(addr, sizeof addr, "unix:path=%s", bus_path);
    int r = sd_bus_new(&bus);
    if (r >= 0) r = sd_bus_set_address(bus, addr);
    if (r >= 0) r = sd_bus_set_bus_client(bus, 1);
    if (r >= 0) r = sd_bus_start(bus);
    return r;
}

// --- serve: agents ----------------------------------------------------------------------------

struct agent {
    struct agent *next;
    char *scope;   // "unix-session:ID" or "unix-process:PID"
    char *owner;   // its unique bus name
    char *path;
    int fallback;
};
static struct agent *agents;

static void agent_free(struct agent *a) {
    free(a->scope);
    free(a->owner);
    free(a->path);
    free(a);
}

// The scope a subject (sa{sv}) names, as polkitd keys its agents; NULL and ERR set when it is not
// one an agent may register for.
static char *read_subject(sd_bus_message *m, sd_bus_error *err) {
    const char *kind;
    char *sid = NULL;
    uint32_t pid = 0;
    int pidfd = -1;
    int r = sd_bus_message_enter_container(m, 'r', "sa{sv}");
    if (r < 0) goto bad;
    if (sd_bus_message_read(m, "s", &kind) < 0) goto bad;
    if (sd_bus_message_enter_container(m, 'a', "{sv}") < 0) goto bad;
    while ((r = sd_bus_message_enter_container(m, 'e', "sv")) > 0) {
        const char *k, *contents;
        if (sd_bus_message_read(m, "s", &k) < 0) goto bad;
        if (sd_bus_message_peek_type(m, NULL, &contents) < 0) goto bad;
        if (strcmp(k, "session-id") == 0 && strcmp(contents, "s") == 0) {
            const char *s;
            if (sd_bus_message_read(m, "v", "s", &s) < 0) goto bad;
            free(sid);
            sid = strdup(s);
        } else if (strcmp(k, "pid") == 0 && strcmp(contents, "u") == 0) {
            if (sd_bus_message_read(m, "v", "u", &pid) < 0) goto bad;
        } else if (strcmp(k, "pidfd") == 0 && strcmp(contents, "h") == 0) {
            if (sd_bus_message_read(m, "v", "h", &pidfd) < 0) goto bad;
            pidfd = fcntl(pidfd, F_DUPFD_CLOEXEC, 3);
        } else if (sd_bus_message_skip(m, "v") < 0) {
            goto bad;
        }
        if (sd_bus_message_exit_container(m) < 0) goto bad;
    }
    if (r < 0 || sd_bus_message_exit_container(m) < 0 || sd_bus_message_exit_container(m) < 0) goto bad;
    char *scope = NULL;
    if (strcmp(kind, "unix-session") == 0) {
        if (!sid || strcmp(sid, session) != 0) {
            // polkitd's own words for a session that is not the caller's
            sd_bus_error_set(err, ERR_FAILED,
                             "Passed session and the session the caller is in differs. They must be equal for now.");
        } else if (asprintf(&scope, "unix-session:%s", sid) < 0) {
            scope = NULL;
        }
    } else if (strcmp(kind, "unix-process") == 0) {
        if (pid == 0 && pidfd >= 0) {
            // A pidfd alone: its pid from fdinfo.
            char p[64], line[128];
            snprintf(p, sizeof p, "/proc/self/fdinfo/%d", pidfd);
            FILE *f = fopen(p, "re");
            while (f && fgets(line, sizeof line, f))
                if (sscanf(line, "Pid: %" SCNu32, &pid) == 1) break;
            if (f) fclose(f);
        }
        if (pid == 0) sd_bus_error_set(err, ERR_FAILED, "Cannot determine the process of the subject");
        else if (asprintf(&scope, "unix-process:%" PRIu32, pid) < 0) scope = NULL;
    } else {
        sd_bus_error_set(err, ERR_FAILED, "Only unix-process and unix-session subjects can be used for authentication agents.");
    }
    if (pidfd >= 0) close(pidfd);
    free(sid);
    return scope;
bad:
    if (pidfd >= 0) close(pidfd);
    free(sid);
    sd_bus_error_set(err, SD_BUS_ERROR_INVALID_ARGS, "Malformed subject");
    return NULL;
}

static struct agent *agent_for(const char *scope) {
    for (struct agent *a = agents; a; a = a->next)
        if (strcmp(a->scope, scope) == 0) return a;
    return NULL;
}

static void say(const char *fmt, const char *a, const char *b, const char *c) {
    fprintf(stderr, "omabox-polkit: ");
    fprintf(stderr, fmt, a, b, c);
    fputc('\n', stderr);
}

static int do_register(sd_bus_message *m, sd_bus_error *err, int with_options) {
    char *scope = read_subject(m, err);
    if (!scope) return -EINVAL;
    const char *locale, *path;
    int fallback = 0, r;
    if ((r = sd_bus_message_read(m, "ss", &locale, &path)) < 0) goto out;
    if (with_options) {
        if ((r = sd_bus_message_enter_container(m, 'a', "{sv}")) < 0) goto out;
        while ((r = sd_bus_message_enter_container(m, 'e', "sv")) > 0) {
            const char *k, *contents;
            if ((r = sd_bus_message_read(m, "s", &k)) < 0) goto out;
            if ((r = sd_bus_message_peek_type(m, NULL, &contents)) < 0) goto out;
            if (strcmp(k, "fallback") == 0 && strcmp(contents, "b") == 0) r = sd_bus_message_read(m, "v", "b", &fallback);
            else r = sd_bus_message_skip(m, "v");
            if (r < 0 || (r = sd_bus_message_exit_container(m)) < 0) goto out;
        }
        if (r < 0 || (r = sd_bus_message_exit_container(m)) < 0) goto out;
    }
    if (!sd_bus_object_path_is_valid(path)) {
        r = sd_bus_error_set(err, SD_BUS_ERROR_INVALID_ARGS, "Not an object path");
        goto out;
    }
    if (agent_for(scope)) {
        say("refused an agent for %s at %s %s: one is registered", scope, sd_bus_message_get_sender(m), path);
        r = sd_bus_error_set(err, ERR_FAILED, "An authentication agent already exists for the given subject");
        goto out;
    }
    struct agent *a = calloc(1, sizeof *a);
    if (!a) { r = -ENOMEM; goto out; }
    a->scope = scope;
    scope = NULL;
    a->owner = strdup(sd_bus_message_get_sender(m));
    a->path = strdup(path);
    a->fallback = fallback;
    a->next = agents;
    agents = a;
    say("registered an agent for %s at %s %s", a->scope, a->owner, a->path);
    r = sd_bus_reply_method_return(m, NULL);
out:
    free(scope);
    return r;
}

static int m_register(sd_bus_message *m, void *u, sd_bus_error *err) { (void)u; return do_register(m, err, 0); }
static int m_register_opts(sd_bus_message *m, void *u, sd_bus_error *err) { (void)u; return do_register(m, err, 1); }

static int m_unregister(sd_bus_message *m, void *u, sd_bus_error *err) {
    (void)u;
    char *scope = read_subject(m, err);
    if (!scope) return -EINVAL;
    const char *path;
    int r = sd_bus_message_read(m, "s", &path);
    if (r < 0) goto out;
    struct agent **pp = &agents;
    while (*pp && strcmp((*pp)->scope, scope) != 0) pp = &(*pp)->next;
    if (!*pp) r = sd_bus_error_set(err, ERR_FAILED, "No such agent registered");
    else if (strcmp((*pp)->owner, sd_bus_message_get_sender(m)) != 0) r = sd_bus_error_set(err, ERR_FAILED, "System bus names do not match");
    else if (strcmp((*pp)->path, path) != 0) r = sd_bus_error_set(err, ERR_FAILED, "Object paths do not match");
    else {
        struct agent *a = *pp;
        *pp = a->next;
        say("unregistered the agent for %s at %s %s", a->scope, a->owner, a->path);
        agent_free(a);
        r = sd_bus_reply_method_return(m, NULL);
    }
out:
    free(scope);
    return r;
}

// The helper's answer comes from this process; polkitd takes one only from uid 0, which nothing in a
// box is.
static int m_response(sd_bus_message *m, void *u, sd_bus_error *err) {
    (void)m; (void)u;
    return sd_bus_error_set(err, ERR_NOT_AUTHORIZED, "Only uid 0 may invoke this method.");
}

static int m_check(sd_bus_message *m, void *u, sd_bus_error *err) {
    (void)m; (void)u;
    return sd_bus_error_set(err, ERR_NOT_SUPPORTED,
                            "omabox's stand-in authority decides nothing: a request comes from `omabox polkit ask`");
}

static int m_enumerate(sd_bus_message *m, void *u, sd_bus_error *err) {
    (void)u; (void)err;
    sd_bus_message *reply = NULL;
    int r = sd_bus_message_new_method_return(m, &reply);
    if (r >= 0) r = sd_bus_message_append(reply, "a(ssssssuuua{ss})", 0);
    if (r >= 0) r = sd_bus_send(NULL, reply, NULL);
    sd_bus_message_unref(reply);
    return r;
}

static int p_name(sd_bus *b, const char *p, const char *i, const char *prop, sd_bus_message *reply, void *u, sd_bus_error *e) {
    (void)b; (void)p; (void)i; (void)prop; (void)u; (void)e;
    return sd_bus_message_append(reply, "s", strcmp(prop, "BackendName") == 0 ? "omabox" : "1");
}

static int p_features(sd_bus *b, const char *p, const char *i, const char *prop, sd_bus_message *reply, void *u, sd_bus_error *e) {
    (void)b; (void)p; (void)i; (void)prop; (void)u; (void)e;
    return sd_bus_message_append(reply, "u", (uint32_t)0);
}

static const sd_bus_vtable auth_vtable[] = {
    SD_BUS_VTABLE_START(0),
    SD_BUS_PROPERTY("BackendName", "s", p_name, 0, SD_BUS_VTABLE_PROPERTY_CONST),
    SD_BUS_PROPERTY("BackendVersion", "s", p_name, 0, SD_BUS_VTABLE_PROPERTY_CONST),
    SD_BUS_PROPERTY("BackendFeatures", "u", p_features, 0, SD_BUS_VTABLE_PROPERTY_CONST),
    SD_BUS_METHOD("RegisterAuthenticationAgent", "(sa{sv})ss", "", m_register, 0),
    SD_BUS_METHOD("RegisterAuthenticationAgentWithOptions", "(sa{sv})ssa{sv}", "", m_register_opts, 0),
    SD_BUS_METHOD("UnregisterAuthenticationAgent", "(sa{sv})s", "", m_unregister, 0),
    SD_BUS_METHOD("AuthenticationAgentResponse", "s(sa{sv})", "", m_response, 0),
    SD_BUS_METHOD("AuthenticationAgentResponse2", "us(sa{sv})", "", m_response, 0),
    SD_BUS_METHOD("CheckAuthorization", "(sa{sv})sa{ss}us", "(bba{ss})", m_check, 0),
    SD_BUS_METHOD("EnumerateActions", "s", "a(ssssssuuua{ss})", m_enumerate, 0),
    SD_BUS_SIGNAL("Changed", "", 0),
    SD_BUS_VTABLE_END,
};

// An agent that left the bus is dropped, as polkitd drops it (its requests end with the call's own
// NoReply).
static int on_owner_changed(sd_bus_message *m, void *u, sd_bus_error *err) {
    (void)u; (void)err;
    const char *name, *old, *new;
    if (sd_bus_message_read(m, "sss", &name, &old, &new) < 0 || name[0] != ':' || new[0]) return 0;
    for (struct agent **pp = &agents; *pp;) {
        if (strcmp((*pp)->owner, name) == 0) {
            struct agent *a = *pp;
            *pp = a->next;
            say("dropped the agent for %s at %s %s: it left the bus", a->scope, a->owner, a->path);
            agent_free(a);
        } else {
            pp = &(*pp)->next;
        }
    }
    return 0;
}

// --- serve: requests --------------------------------------------------------------------------

struct req {
    struct req *next;
    char cookie[64];
    char *accept;      // the password the helper takes; NULL: none
    char *prompt;
    char *owner, *path;
    sd_bus_message *ask;   // the Ask call, answered when the request ends
    sd_event_source *timer;
    uint64_t delay;
    unsigned tries, wrong;
    int authenticated, timed_out;
};
static struct req *reqs;
static unsigned serial;

static struct req *req_for(const char *cookie) {
    for (struct req *q = reqs; q; q = q->next)
        if (strcmp(q->cookie, cookie) == 0) return q;
    return NULL;
}

static void req_end(struct req *q, const char *result, const char *why) {
    say("request %s ended: %s %s", q->cookie, result, why ? why : "");
    sd_bus_reply_method_return(q->ask, "suusss", result, q->tries, q->wrong, q->owner, q->path, why ? why : "");
    for (struct req **pp = &reqs; *pp; pp = &(*pp)->next)
        if (*pp == q) { *pp = q->next; break; }
    sd_bus_message_unref(q->ask);
    sd_event_source_disable_unref(q->timer);
    if (q->accept) explicit_bzero(q->accept, strlen(q->accept));
    free(q->accept);
    free(q->prompt);
    free(q->owner);
    free(q->path);
    free(q);
}

static int on_begin_reply(sd_bus_message *m, void *u, sd_bus_error *err) {
    (void)err;
    struct req *q = u;
    const sd_bus_error *e = sd_bus_message_get_error(m);
    if (!e) {
        req_end(q, q->authenticated ? "authenticated" : "not-authenticated", NULL);
    } else if (q->timed_out) {
        req_end(q, "timeout", e->message);
    } else if (sd_bus_error_has_name(e, ERR_CANCELLED)) {
        req_end(q, "cancelled", e->message);
    } else if (sd_bus_error_has_names(e, SD_BUS_ERROR_NO_REPLY, SD_BUS_ERROR_SERVICE_UNKNOWN, SD_BUS_ERROR_NAME_HAS_NO_OWNER)) {
        req_end(q, "agent-gone", e->message);
    } else {
        char why[512];
        snprintf(why, sizeof why, "%s: %s", e->name, e->message ? e->message : "");
        req_end(q, "error", why);
    }
    return 0;
}

static int on_timeout(sd_event_source *s, uint64_t usec, void *u) {
    (void)s; (void)usec;
    struct req *q = u;
    q->timed_out = 1;
    say("request %s: no end in time, cancelling it at %s %s", q->cookie, q->owner, q->path);
    sd_bus_call_method_async(bus, NULL, q->owner, q->path, AGENT_IFACE, "CancelAuthentication", NULL, NULL, "s", q->cookie);
    return 0;
}

// Ask(action, message, icon, prompt, accept, details, timeout µs, delay µs) -> (result, tries, wrong,
// agent's name, its path, why)
static int m_ask(sd_bus_message *m, void *u, sd_bus_error *err) {
    (void)u;
    const char *action, *message, *icon, *prompt, *accept;
    uint64_t timeout, delay;
    int r = sd_bus_message_read(m, "sssss", &action, &message, &icon, &prompt, &accept);
    if (r < 0) return r;
    sd_bus_message *call = NULL;
    struct agent *a = NULL;
    for (struct agent *x = agents; x; x = x->next) {
        char want[256];
        snprintf(want, sizeof want, "unix-session:%s", session);
        if (strcmp(x->scope, want) == 0 && !x->fallback) { a = x; break; }
    }
    for (struct agent *x = agents; !a && x; x = x->next)
        if (!x->fallback) a = x;
    if (!a) a = agents;
    struct req *q = calloc(1, sizeof *q);
    if (!q) return -ENOMEM;
    uint64_t rnd = 0;
    (void)!getrandom(&rnd, sizeof rnd, 0);
    snprintf(q->cookie, sizeof q->cookie, "omabox-%u-%016" PRIx64, ++serial, rnd);
    q->accept = accept[0] ? strdup(accept) : NULL;
    q->prompt = strdup(prompt[0] ? prompt : "Password: ");
    q->ask = sd_bus_message_ref(m);
    q->next = reqs;
    reqs = q;
    if (!a) {
        q->owner = strdup("");
        q->path = strdup("");
        // the details and times not read: the call's answer needs none of them
        req_end(q, "no-agent", "no polkit agent is registered in the box");
        return 1;
    }
    q->owner = strdup(a->owner);
    q->path = strdup(a->path);
    r = sd_bus_message_new_method_call(bus, &call, a->owner, a->path, AGENT_IFACE, "BeginAuthentication");
    if (r >= 0) r = sd_bus_message_append(call, "sss", action, message, icon);
    // the details, copied through
    if (r >= 0) r = sd_bus_message_open_container(call, 'a', "{ss}");
    if (r >= 0) r = sd_bus_message_enter_container(m, 'a', "{ss}");
    while (r >= 0) {
        const char *k, *v;
        r = sd_bus_message_read(m, "{ss}", &k, &v);
        if (r <= 0) break;
        r = sd_bus_message_append(call, "{ss}", k, v);
    }
    if (r >= 0) r = sd_bus_message_exit_container(m);
    if (r >= 0) r = sd_bus_message_close_container(call);
    if (r >= 0) r = sd_bus_message_append(call, "s", q->cookie);
    if (r >= 0) r = sd_bus_message_append(call, "a(sa{sv})", 1, "unix-user", 1, "uid", "u", uid);
    if (r >= 0) r = sd_bus_message_read(m, "tt", &timeout, &delay);
    if (r < 0) {
        sd_bus_message_unref(call);
        req_end(q, "error", "could not make the request");
        return 1;
    }
    q->delay = delay;
    // The agent's call waits as long as polkitd's does (no timeout of the bus's); the request's
    // own TIMEOUT cancels it.
    r = sd_bus_call_async(bus, NULL, call, on_begin_reply, q, timeout + 3600 * USEC_PER_SEC);
    sd_bus_message_unref(call);
    if (r < 0) {
        req_end(q, "error", "could not call the agent");
        return 1;
    }
    uint64_t now;
    sd_event_now(ev, CLOCK_MONOTONIC, &now);
    sd_event_add_time(ev, &q->timer, CLOCK_MONOTONIC, now + timeout, 0, on_timeout, q);
    say("request %s: BeginAuthentication on %s %s", q->cookie, q->owner, q->path);
    (void)err;
    return 1;
}

static int m_agents(sd_bus_message *m, void *u, sd_bus_error *err) {
    (void)u; (void)err;
    sd_bus_message *reply = NULL;
    int r = sd_bus_message_new_method_return(m, &reply);
    if (r >= 0) r = sd_bus_message_open_container(reply, 'a', "(sssb)");
    for (struct agent *a = agents; a && r >= 0; a = a->next)
        r = sd_bus_message_append(reply, "(sssb)", a->scope, a->owner, a->path, a->fallback);
    if (r >= 0) r = sd_bus_message_close_container(reply);
    if (r >= 0) r = sd_bus_send(NULL, reply, NULL);
    sd_bus_message_unref(reply);
    return r;
}

static const sd_bus_vtable test_vtable[] = {
    SD_BUS_VTABLE_START(0),
    SD_BUS_METHOD("Ask", "sssssa{ss}tt", "suusss", m_ask, 0),
    SD_BUS_METHOD("Agents", "", "a(sssb)", m_agents, 0),
    SD_BUS_VTABLE_END,
};

// --- serve: the helper socket -----------------------------------------------------------------

struct conn {
    int fd;
    sd_event_source *io, *timer;
    char buf[MAX_LINE + 2];
    size_t len;
    int stage;   // 0 user, 1 cookie, 2 the response, 3 checking it
    char user[256], cookie[64];
};

static void conn_close(struct conn *c) {
    sd_event_source_disable_unref(c->io);
    sd_event_source_disable_unref(c->timer);
    close(c->fd);
    explicit_bzero(c->buf, sizeof c->buf);
    free(c);
}

// A line as polkit-agent-helper-1 sends one: g_strescape'd.
static void send_line(struct conn *c, const char *kind, const char *text) {
    char out[MAX_LINE * 4 + 64];
    size_t n = (size_t)snprintf(out, sizeof out, "%s", kind);
    if (text) {
        out[n++] = ' ';
        for (const unsigned char *p = (const unsigned char *)text; *p && n < sizeof out - 8; p++) {
            switch (*p) {
            case '\\': case '"': out[n++] = '\\'; out[n++] = (char)*p; break;
            case '\n': out[n++] = '\\'; out[n++] = 'n'; break;
            case '\t': out[n++] = '\\'; out[n++] = 't'; break;
            case '\r': out[n++] = '\\'; out[n++] = 'r'; break;
            default:
                if (*p < 0x20 || *p >= 0x7f) n += (size_t)snprintf(out + n, 5, "\\%03o", *p);
                else out[n++] = (char)*p;
            }
        }
    }
    out[n++] = '\n';
    for (size_t off = 0; off < n;) {
        ssize_t w = write(c->fd, out + off, n - off);
        if (w < 0 && errno == EINTR) continue;
        if (w <= 0) return;
        off += (size_t)w;
    }
}

static int on_checked(sd_event_source *s, uint64_t usec, void *u) {
    (void)s; (void)usec;
    struct conn *c = u;
    struct req *q = req_for(c->cookie);
    int ok = q && q->accept && strcmp(c->buf, q->accept) == 0 && strcmp(c->user, user) == 0;
    explicit_bzero(c->buf, sizeof c->buf);
    if (q) {
        if (ok) q->authenticated = 1;
        else q->wrong++;
    }
    say("helper: %s for %s", ok ? "the accepted password" : "a wrong password", c->cookie, "");
    send_line(c, ok ? "SUCCESS" : "FAILURE", NULL);
    conn_close(c);
    return 0;
}

// One line from the agent; 1 when nothing more is read from it (closed, or its response taken).
static int conn_line(struct conn *c, char *line) {
    switch (c->stage) {
    case 0: {
        size_t n = strlen(line);
        if (n < sizeof c->user) memcpy(c->user, line, n + 1);   // longer: no user's, so a wrong one
        c->stage = 1;
        return 0;
    }
    case 1: {
        size_t n = strlen(line);
        if (n < sizeof c->cookie) memcpy(c->cookie, line, n + 1);   // longer: none of ours
        struct req *q = n < sizeof c->cookie ? req_for(c->cookie) : NULL;
        if (!q) {
            say("helper: no request for cookie %s", c->cookie, "", "");
            send_line(c, "FAILURE", NULL);
            conn_close(c);
            return 1;
        }
        c->stage = 2;
        send_line(c, "PAM_PROMPT_ECHO_OFF", q->prompt);
        return 0;
    }
    case 2: {
        // The response stays in buf for on_checked; nothing more is read.
        size_t n = strlen(line);
        memmove(c->buf, line, n + 1);
        c->len = 0;
        c->stage = 3;
        sd_event_source_set_enabled(c->io, SD_EVENT_OFF);
        struct req *q = req_for(c->cookie);
        if (q) q->tries++;
        uint64_t now;
        sd_event_now(ev, CLOCK_MONOTONIC, &now);
        sd_event_add_time(ev, &c->timer, CLOCK_MONOTONIC, now + (q ? q->delay : 0), 0, on_checked, c);
        return 1;
    }
    }
    return 1;
}

static int on_conn(sd_event_source *s, int fd, uint32_t events, void *u) {
    (void)s; (void)events;
    struct conn *c = u;
    ssize_t n = read(fd, c->buf + c->len, MAX_LINE + 1 - c->len);
    if (n < 0 && (errno == EINTR || errno == EAGAIN)) return 0;
    if (n <= 0) { conn_close(c); return 0; }
    c->len += (size_t)n;
    c->buf[c->len] = 0;
    char *nl;
    while (c->stage < 3 && (nl = memchr(c->buf, '\n', c->len))) {
        *nl = 0;
        size_t used = (size_t)(nl - c->buf) + 1;
        char line[MAX_LINE + 2];
        memcpy(line, c->buf, used);
        memmove(c->buf, c->buf + used, c->len - used);
        c->len -= used;
        c->buf[c->len] = 0;
        int done = conn_line(c, line);
        explicit_bzero(line, sizeof line);
        if (done) return 0;
    }
    if (c->len > MAX_LINE) { send_line(c, "FAILURE", NULL); conn_close(c); }
    return 0;
}

static int on_accept(sd_event_source *s, int fd, uint32_t events, void *u) {
    (void)s; (void)events; (void)u;
    int cfd = accept4(fd, NULL, NULL, SOCK_CLOEXEC | SOCK_NONBLOCK);
    if (cfd < 0) return 0;
    struct conn *c = calloc(1, sizeof *c);
    if (!c) { close(cfd); return 0; }
    c->fd = cfd;
    if (sd_event_add_io(ev, &c->io, cfd, EPOLLIN, on_conn, c) < 0) {
        close(cfd);
        free(c);
    }
    return 0;
}

static int serve(const char *sess) {
    session = strdup(sess);
    struct passwd *pw = getpwuid(uid);
    if (!pw) { fprintf(stderr, "omabox-polkit: no passwd entry for uid %u\n", uid); return 2; }
    user = strdup(pw->pw_name);
    signal(SIGPIPE, SIG_IGN);
    int r = connect_bus();
    if (r < 0) { fprintf(stderr, "omabox-polkit: the box's bus at %s: %s\n", bus_path, strerror(-r)); return 2; }
    sd_event_default(&ev);
    sd_bus_attach_event(bus, ev, 0);
    sd_bus_add_object_vtable(bus, NULL, AUTH_PATH, AUTH_IFACE, auth_vtable, NULL);
    sd_bus_add_object_vtable(bus, NULL, TEST_PATH, TEST_IFACE, test_vtable, NULL);
    sd_bus_match_signal(bus, NULL, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                        "NameOwnerChanged", on_owner_changed, NULL);
    // The helper's socket, at the path polkit's agent library tries first.
    if (mkdir(HELPER_DIR, 0755) < 0 && errno != EEXIST) {
        fprintf(stderr, "omabox-polkit: %s: %s\n", HELPER_DIR, strerror(errno));
        return 2;
    }
    int lfd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
    struct sockaddr_un sa = {.sun_family = AF_UNIX};
    snprintf(sa.sun_path, sizeof sa.sun_path, "%s", HELPER_SOCKET);
    unlink(HELPER_SOCKET);
    if (lfd < 0 || bind(lfd, (struct sockaddr *)&sa, sizeof sa) < 0 || listen(lfd, 16) < 0) {
        fprintf(stderr, "omabox-polkit: %s: %s\n", HELPER_SOCKET, strerror(errno));
        return 2;
    }
    sd_event_add_io(ev, NULL, lfd, EPOLLIN, on_accept, NULL);
    r = sd_bus_request_name(bus, AUTH_NAME, 0);
    if (r < 0) { fprintf(stderr, "omabox-polkit: could not own %s: %s\n", AUTH_NAME, strerror(-r)); return 2; }
    int rf = open(ready_path, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
    if (rf >= 0) close(rf);
    say("the authority is up for session %s, user %s%s", session, user, "");
    r = sd_event_loop(ev);
    return r < 0 ? 2 : 0;
}

// --- the clients ------------------------------------------------------------------------------

static void json_str(const char *s) {
    putchar('"');
    for (const unsigned char *p = (const unsigned char *)s; *p; p++) {
        if (*p == '"' || *p == '\\') printf("\\%c", *p);
        else if (*p < 0x20) printf("\\u%04x", *p);
        else putchar(*p);
    }
    putchar('"');
}

static int client(void) {
    int r = connect_bus();
    if (r < 0) {
        fprintf(stderr, "omabox-polkit: the box has no polkit bus (%s: %s)\n", bus_path, strerror(-r));
        return 2;
    }
    return 0;
}

static int agents_cmd(int json) {
    if (client()) return 2;
    sd_bus_error e = SD_BUS_ERROR_NULL;
    sd_bus_message *reply = NULL;
    int r = sd_bus_call_method(bus, AUTH_NAME, TEST_PATH, TEST_IFACE, "Agents", &e, &reply, "");
    if (r < 0) {
        fprintf(stderr, "omabox-polkit: the box's authority did not answer: %s\n", e.message ? e.message : strerror(-r));
        return 2;
    }
    sd_bus_message_enter_container(reply, 'a', "(sssb)");
    const char *scope, *owner, *path;
    int fb, n = 0;
    if (json) putchar('[');
    while (sd_bus_message_read(reply, "(sssb)", &scope, &owner, &path, &fb) > 0) {
        if (json) {
            printf("%s{\"subject\":", n ? "," : "");
            json_str(scope);
            printf(",\"name\":");
            json_str(owner);
            printf(",\"path\":");
            json_str(path);
            printf(",\"fallback\":%s}", fb ? "true" : "false");
        } else {
            printf("%s\t%s\t%s%s\n", scope, owner, path, fb ? "\tfallback" : "");
        }
        n++;
    }
    if (json) puts("]");
    return 0;
}

static int ask_cmd(int argc, char **argv) {
    const char *action = "org.omabox.test", *message = NULL, *icon = "", *prompt = "", *accept = "";
    double timeout = 60, delay = 0;
    int json = 0;
    char **details = calloc((size_t)argc + 1, sizeof *details);
    int nd = 0;
    static const struct option opts[] = {
        {"action", 1, 0, 'a'}, {"message", 1, 0, 'm'}, {"icon", 1, 0, 'i'}, {"prompt", 1, 0, 'p'},
        {"accept", 1, 0, 'A'}, {"detail", 1, 0, 'd'}, {"timeout", 1, 0, 't'}, {"delay", 1, 0, 'D'},
        {"json", 0, 0, 'j'}, {0, 0, 0, 0},
    };
    int o;
    char *end;
    while ((o = getopt_long(argc, argv, "", opts, NULL)) != -1) {
        switch (o) {
        case 'a': action = optarg; break;
        case 'm': message = optarg; break;
        case 'i': icon = optarg; break;
        case 'p': prompt = optarg; break;
        case 'A': accept = optarg; break;
        case 'd':
            if (!strchr(optarg, '=') || optarg[0] == '=') {
                fprintf(stderr, "omabox-polkit: --detail is KEY=VALUE, got %s\n", optarg);
                return 2;
            }
            details[nd++] = optarg;
            break;
        case 't':
            timeout = strtod(optarg, &end);
            if (*end || !(timeout > 0 && timeout <= 3600)) {
                fprintf(stderr, "omabox-polkit: --timeout is seconds, more than 0 and up to 3600, got %s\n", optarg);
                return 2;
            }
            break;
        case 'D':
            delay = strtod(optarg, &end);
            if (*end || !(delay >= 0 && delay <= 60000)) {
                fprintf(stderr, "omabox-polkit: --delay is milliseconds, 0 to 60000, got %s\n", optarg);
                return 2;
            }
            break;
        case 'j': json = 1; break;
        default: return 2;
        }
    }
    if (optind < argc) {
        fprintf(stderr, "omabox-polkit: ask takes no words, got %s\n", argv[optind]);
        return 2;
    }
    char dflt[512];
    if (!message) {
        snprintf(dflt, sizeof dflt, "Authentication is required for %s", action);
        message = dflt;
    }
    if (client()) return 2;
    sd_bus_message *call = NULL, *reply = NULL;
    sd_bus_error e = SD_BUS_ERROR_NULL;
    int r = sd_bus_message_new_method_call(bus, &call, AUTH_NAME, TEST_PATH, TEST_IFACE, "Ask");
    if (r >= 0) r = sd_bus_message_append(call, "sssss", action, message, icon, prompt, accept);
    if (r >= 0) r = sd_bus_message_open_container(call, 'a', "{ss}");
    for (int i = 0; i < nd && r >= 0; i++) {
        char *eq = strchr(details[i], '=');
        *eq = 0;
        r = sd_bus_message_append(call, "{ss}", details[i], eq + 1);
    }
    if (r >= 0) r = sd_bus_message_close_container(call);
    uint64_t tus = (uint64_t)(timeout * USEC_PER_SEC), dus = (uint64_t)(delay * 1000);
    if (r >= 0) r = sd_bus_message_append(call, "tt", tus, dus);
    if (r >= 0) r = sd_bus_call(bus, call, tus + dus + 30 * USEC_PER_SEC, &e, &reply);
    if (r < 0) {
        fprintf(stderr, "omabox-polkit: the box's authority did not answer: %s\n", e.message ? e.message : strerror(-r));
        return 2;
    }
    const char *result, *owner, *path, *why;
    uint32_t tries, wrong;
    if (sd_bus_message_read(reply, "suusss", &result, &tries, &wrong, &owner, &path, &why) < 0) return 2;
    if (json) {
        printf("{\"result\":");
        json_str(result);
        printf(",\"tries\":%u,\"wrong\":%u,\"agent\":", tries, wrong);
        json_str(owner);
        printf(",\"path\":");
        json_str(path);
        printf(",\"action\":");
        json_str(action);
        printf(",\"why\":");
        json_str(why);
        puts("}");
    } else {
        printf("%s", result);
        if (owner[0]) printf(": the agent at %s %s", owner, path);
        printf(", %u password%s (%u wrong)", tries, tries == 1 ? "" : "s", wrong);
        if (why[0]) printf(": %s", why);
        putchar('\n');
    }
    return strcmp(result, "authenticated") == 0 ? 0 : strcmp(result, "timeout") == 0 ? 124 : 1;
}

int main(int argc, char **argv) {
    if (!omabox_inside_box()) {
        fprintf(stderr, "omabox-polkit: refusing to run outside an omabox box (it would use your real system bus)\n");
        return 2;
    }
    paths();
    if (argc == 3 && strcmp(argv[1], "serve") == 0) return serve(argv[2]);
    if (argc >= 2 && strcmp(argv[1], "ask") == 0) return ask_cmd(argc - 1, argv + 1);
    if (argc >= 2 && strcmp(argv[1], "agents") == 0) {
        int json = argc == 3 && strcmp(argv[2], "--json") == 0;
        if (argc > 3 || (argc == 3 && !json)) {
            fprintf(stderr, "omabox-polkit: agents takes only --json\n");
            return 2;
        }
        return agents_cmd(json);
    }
    fprintf(stderr, "usage: omabox-polkit serve SESSION | ask [OPTIONS] | agents [--json]\n");
    return 2;
}
