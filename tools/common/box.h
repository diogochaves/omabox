// Shared by the tools that run inside a box (keyboard, pointer, still, events): header-only.
//
// omabox_inside_box(): whether this process is in a box's mount namespace (SECURITY.md, finding 177).
// Run from a host shell these tools would drive, or watch, the real desktop (it happened once). A box
// has /opt/omabox/share as a mount point of its own (`up` binds it) and never the system bus (the box
// safety invariant: /run/dbus is never bound in); a host has the bus, and a plain /opt/omabox/share
// directory there (an install, a stray mkdir) is not a mount point.
#ifndef OMABOX_BOX_H
#define OMABOX_BOX_H

// (statx needs _GNU_SOURCE, defined by each tool before its first include.)
#include <fcntl.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>
#include <unistd.h>

static inline int omabox_inside_box(void) {
    if (access("/run/dbus/system_bus_socket", F_OK) == 0) return 0;
    // Not /proc/self/mountinfo: a tool `omabox` starts with nsenter is in the box's mount namespace
    // but not its pid namespace, where the box's /proc has no self.
    struct statx sx;
    if (statx(AT_FDCWD, "/opt/omabox/share", AT_NO_AUTOMOUNT, STATX_BASIC_STATS, &sx) != 0) return 0;
    if (sx.stx_attributes_mask & STATX_ATTR_MOUNT_ROOT) return (sx.stx_attributes & STATX_ATTR_MOUNT_ROOT) != 0;
    // A kernel before 5.8: a mount point is on another device than the dir it is in.
    struct stat dir;
    if (stat("/opt/omabox", &dir) != 0) return 0;
    return dir.st_dev != makedev(sx.stx_dev_major, sx.stx_dev_minor);
}

#endif
