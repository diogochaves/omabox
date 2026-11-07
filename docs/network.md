# Network and mounts

How a box's network, ports and mounts work, in detail. Back to the [README](../README.md).

## Networking and mount details

- Across the box boundary, use `127.0.0.1:PORT`, with a server that listens on IPv4 (`127.0.0.1`,
  `0.0.0.0` or `::`). `localhost` works from your host into a box, but from a box it is reset when
  the server listens on IPv4 only, as most dev servers do, and from one box to another it never
  works. A server in a box that listens on `::1` only cannot be reached from outside it, though its
  port is still taken on your `127.0.0.1`, where connections to it are reset.
- An isolated box can have more addresses on its loopback, for apps that judge their network by
  where a peer comes from (a BitTorrent client reads "firewalled" while every peer is on
  127.0.0.1): `omabox up --net isolated --addr 203.0.113.10 --addr 203.0.113.11`. A server on .10
  sees a client bound to .11 come from .11. Only documentation ranges (192.0.2.0/24,
  198.51.100.0/24, 203.0.113.0/24, 2001:db8::/32), which no real host has; nothing in the box can
  add one itself.
- A box's ports are forwarded to your host's `127.0.0.1` only (never your LAN address), usually
  within a second of its server listening, the ephemeral range included; other boxes reach them
  there too. A TCP port there also takes the UDP port of the same number. So while a connected box
  runs a server on a port, your own server cannot start on it; an isolated box's ports stay its own
  (two isolated boxes can use the same port). The other way round, a connected box sees your
  host's servers on their ports, another box's forwarded ports among them, so a server a box starts
  on a port your host or another connected box already has fails to start (address in use). Only
  servers started in two boxes within about a second of each other both run, each answering inside
  its own box, and only one of them gets the port on your `127.0.0.1`. A dev server that takes the
  next free port when its own is taken (as Vite does) moves on by itself.
  `omabox ports` lists every box's servers and what holds each port on your
  `127.0.0.1`: that box, any of the boxes sharing it, or one of your own processes.
- To open a box's server from another machine on your tailnet, share its port from your host with
  Tailscale: `tailscale serve --bg --http=3000 3000`, then open `http://<machine-name>:3000` (by
  name: the tailnet IP answers 404). Everyone on your tailnet reaches it, users you share the
  machine with included; your LAN does not. It stays on after the box goes down (answering 502)
  until `tailscale serve --http=3000 off`.
- Inside a box, your machine's LAN address is the box itself. A connected box started inside
  another box has no network.
- Every box needs `passt` (which `install.sh` installs), and a connected one `/dev/net/tun`. Its own
  network also keeps what runs in it from reaching or taking your host's abstract sockets (a box's
  X11 display used to catch X11 apps started on the host).
- `--net host` (only by name: never the default, and no `omabox config` key picks it) is for a tool
  that discovers devices on your LAN, which a connected box cannot test: pasta answers for every
  address on its subnet, so a scan finds a "device" at each. The box shares your host's network
  namespace and has no pasta, but keeps its own seat, session bus, runtime dir and HOME. What changes:
  its servers listen on your host's real ports, so they collide with your services (and other host
  boxes), and nothing is forwarded (`omabox ports` says `host-net`); it sees your real interfaces
  (Tailscale, Docker bridges), and its ARP and ICMP reach your LAN; it reaches your abstract sockets,
  so an X server in it would catch your X11 apps (`--xwayland` is refused; don't start Xvfb there);
  and protected ports cannot hold for it, so it does not start while `protect-ports` names one. It
  is refused with `--allow` and `--addr` (isolated's), with `--desktop desk` (your own programs,
  at your HOME's path, would meet your session's abstract sockets) and inside ai-jail. Not for code you do not
  trust: that stays `--net isolated`, ai-jail or a VM. Inside a box, `--net host` is that box's
  network.
- `--ro-bind DIR:DEST` mounts a dir somewhere else (`--ro-bind ~/nas:/mnt/nas`, to test path
  mapping); DEST cannot be `/`, a system dir, `/run`, `/tmp` itself, `/opt/omabox`, the box HOME, or
  a dir above those. In a desk box (`--desktop desk`) the box HOME is at your HOME's path: a DEST
  inside it is fine, but not omabox's own files there (`.config/omabox`, its logs). A plugin dir inside `~/.config/omarchy/plugins` is fine (a theme in `themes/` too). A throwaway `run`
  outside a repo mounts nothing of the current dir: the command runs in the box HOME (`run` says so
  when a word of it is a path there), `run --overlay DIR` brings it in.
- `--seed SRC:DEST` copies a file or folder into the box HOME before the session
  starts (`--seed ./fixture.json:.config/myplugin/config.json`), for a plugin that reads its config
  once at start; the same sources are refused as for `--ro-bind`.
- `--no-seed fonts` starts the box without your fonts, which are otherwise mounted read-only so a
  theme's or a plugin's own faces draw as on your desktop.
- `--no-seed extensions` starts the box without your `~/.config/omarchy/extensions` (plugins'
  settings files). Without it `up` copies them, and names each file a `--plugin`'s code reads there:
  a plugin under test starts on your desktop's settings, an on/off switch in them too.
- `--theme-dir DIR` puts a theme you are editing in the box live: the box's
  `~/.config/omarchy/themes/NAME` links to DIR, which the box sees at its own path (the repo, or a
  read-only mount). Your other themes are copies made at `up`, a linked one too.
- `--theme NAME` starts the box on that theme (Omarchy's, one of yours or a
  `--theme-dir` one) instead of your desktop's current one, so a test's start does not depend on
  what your desktop is on that day.
