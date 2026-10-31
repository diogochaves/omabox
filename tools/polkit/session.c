// omabox-polkit-session.so: the box's session for polkit agents in an `up --polkit` box (#258,
// NOTES finding 627), preloaded into the box's session (share/session.sh).
//
// An agent registers for its session, and finds it as polkit's library does, with logind's
// sd_pid_get_session: a box has no logind session (its processes are in a user unit's cgroup, or the
// box's own), so it fails ("No session for pid N") and the agent never registers. Here it answers
// OMABOX_POLKIT_SESSION, the session the box's stand-in authority (omabox-polkit serve) serves, for
// any process; with that unset, logind's own answer.
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>

int sd_pid_get_session(pid_t pid, char **session) {
    const char *s = getenv("OMABOX_POLKIT_SESSION");
    if (s && *s) {
        if (!session) return -EINVAL;
        *session = strdup(s);
        return *session ? 0 : -ENOMEM;
    }
    int (*real)(pid_t, char **) = (int (*)(pid_t, char **))dlsym(RTLD_NEXT, "sd_pid_get_session");
    return real ? real(pid, session) : -ENODATA;
}
