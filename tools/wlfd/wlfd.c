// omabox-wlfd: connect to a Wayland socket and exec a command with WAYLAND_SOCKET set to the
// connected fd.
//
//   omabox-wlfd /run/user/1000/wayland-1 bwrap ...
//
// Interactive boxes use it so the box holds exactly one connection to the host compositor (the
// nested Hyprland's) and never a socket path that any process inside could reopen. libwayland
// consumes WAYLAND_SOCKET on connect, unsets it and marks the fd close-on-exec, so nothing the
// nested compositor launches inherits it.
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: omabox-wlfd SOCKET COMMAND [ARG...]\n");
        return 2;
    }
    struct sockaddr_un addr = {.sun_family = AF_UNIX};
    if (strlen(argv[1]) >= sizeof(addr.sun_path)) {
        fprintf(stderr, "omabox-wlfd: socket path too long: %s\n", argv[1]);
        return 1;
    }
    strcpy(addr.sun_path, argv[1]);
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0) { perror("omabox-wlfd: socket"); return 1; }
    if (connect(fd, (struct sockaddr *)&addr, sizeof(addr)) < 0) {
        perror("omabox-wlfd: connect");
        close(fd);
        return 1;
    }
    if (fcntl(fd, F_SETFD, 0) < 0) { perror("omabox-wlfd: fcntl"); return 1; } // must survive exec
    char num[16];
    snprintf(num, sizeof(num), "%d", fd);
    setenv("WAYLAND_SOCKET", num, 1);
    execvp(argv[2], argv + 2);
    perror("omabox-wlfd: exec");
    return 127;
}
