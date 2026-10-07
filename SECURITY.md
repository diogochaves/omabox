# Security

omabox promises one thing: a box never reaches your real desktop. Its Hyprland does not get your
screen, keyboard or mouse; its apps do not see your HOME, your secrets or your session; and the
commands that drive a box do not land on your desktop instead. This page lists each rule behind that
promise, where `bin/omabox` (or a tool) enforces it, and the test in `test/run.sh` that proves it.

## Reporting a problem

Report a way around any rule below privately, through GitHub's
[private vulnerability reporting](https://github.com/diogochaves/omabox/security/advisories/new),
not in a public issue. Say what you ran, what the box reached and your versions (`omabox --version`,
`Hyprland --version`, `omarchy version`). Only the latest release gets fixes.

## What omabox is not

A box is **not a security boundary against a hostile program**. It shares your kernel and your GPU
(a runaway app can load or hang either), and by default your network: it reaches the internet, your
LAN and the servers on your host's loopback. It reads what you mount into it. The rules below keep a
box from reaching your desktop, by mistake or through an app that misbehaves. They are not a defence
against code written to escape. For code you do not trust, use `--net isolated` at least,
[ai-jail](https://github.com/akitaonrails/ai-jail), or a VM.

Commands you run on the host are not boxed either: a project's `sudo ./setup`, or `omabox host --
CMD`, which runs on your real desktop on purpose.

## The rules

| Rule | Enforced in | Proven by |
|---|---|---|
| **Hyprland never gets your seat.** Hyprland tries DRM first, so a box gets a GPU render node (`renderD*`, plus NVIDIA's `/dev/nvidia*` nodes for it) and never `/dev/dri/card*`, `/dev/input`, `/run/seatd.sock`, the system bus or your `$XDG_RUNTIME_DIR`. Its runtime dir is one of its own, at the same path. | `cmd_up`'s bwrap arguments (a fresh `/dev`, no host `/run`); `render_node`/`render_nodes` accept only `/dev/dri/renderD[0-9]+` | `t_main`: "no /dev/input", "no DRM card node", "no seatd socket", "no system bus", "the runtime dir is the box's own, not yours" |
| **The box HOME is fake and holds no secrets.** It starts from `/etc/skel` and gets your Omarchy look (theme, terminal configs, branding, menu extensions) and git's `user.name`/`user.email`. Never `api-keys.env`, keyrings, tokens or the rest of your git config. Links in what it copies stay links, so a link to a secret brings nothing in. Your real HOME is not mounted. | `seed_home`, `seed_copy` | `t_main`: "real HOME invisible", "no api-keys.env in the box HOME"; `t_unit_seed_copy` |
| **Secrets and live sockets are never mounted or copied in.** `--ro-bind`, `--overlay`, `--plugin`, `--theme-dir` and `--seed` refuse anything that is or contains HOME, `~/.config/omarchy` or `/tmp`, anything inside `~/.config/omarchy` but `plugins/` and `themes/` (`api-keys.env`, your hooks), and anything inside or containing `~/.ssh`, `~/.gnupg`, keyrings, `~/.password-store`, `~/.aws`, `~/.kube`, `~/.docker`, `~/.netrc`, git's credential files, gh's, gcloud's, azure's and 1Password's `op` config, omabox's own saves and box HOMEs, the runtime dir, `/run`, `/dev`, `/proc`, `/sys` or `/tmp`'s socket dirs. A path is checked as named and where its links lead, so a link to a secret, or a secret's link to a file elsewhere, is refused too. `--seed` never writes through a link already in the box HOME. Nothing is mounted over the box's own system dirs, runtime dir, `/tmp` or HOME. A refused mount fails before a box dir is made. | `refuse_src`, `refuse_dest` (`seed_copy` also skips what `refuse_src` refuses) | `t_unit_mount_rules`, `t_unit_refusals` |
| **Host code is read-only.** `/usr`, `/etc` and omabox's own files are read-only binds. A repo is mounted read-only at its own path, or, for `omabox run` with no box up, as an overlay whose writes are thrown away. | `cmd_up`'s `--ro-bind` and `--tmp-overlay` arguments | `t_main`: "repo read-only"; `t_throwaway`: "host checkout unchanged" |
| **Each box has its own namespaces and network.** pid, IPC, UTS and network namespaces of its own, so its Xwayland cannot take your X11 socket. A connected box's servers are forwarded to your host's 127.0.0.1 only, never the LAN. An isolated box has a loopback and the `--allow` ports, nothing else. | `cmd_up`: bwrap's `--unshare-*`, pasta's arguments | `t_isolated`, `t_connected`, `t_no_new_privs` |
| **Teardown kills the box and nothing else.** `down` SIGKILLs the box's PID 1, and the kernel ends everything in its pid namespace. A recorded pid counts only while it is still in the box's own pid namespace, and a pasta only while its name and command line are this box's, so a reused pid of yours is never killed. A box ends with its pasta and with its Hyprland. | `box_pid`, `box_pasta`, `pasta_pid`, `kill_box` | `t_main` (down leaves no dir, HOME or reaper), `t_stale_pid`, `t_isolated_no_pidfile`, `t_pasta_dies`, `t_hyprland_dies`, `t_other_userns` |
| **The tools that drive a box never follow it out.** A box can write its runtime dir and HOME, so omabox runs grim, hyprctl, gdb and its input tools inside the box's namespaces with a clean environment, writes into a box HOME from inside the box (`gdb --watch`'s log, `run -d`'s), and only takes plain names from what the box wrote (no paths). A box with no pasta in front of it (a nested one) gets none of its caller's open files. | `on_box`, `box_env`, `cmd_gdb`; `cmd_up`'s launch script (the fds) | `t_hostile`, `t_gdb`, `t_replace`, `t_leak_control` (the nested box) |
| **Input never reaches your desktop.** omabox's keyboard, pointer, still and events tools refuse to run outside a box. While the suite runs, a watcher listens to your Hyprland's event socket (read-only): any window, focus change or virtual keyboard of the suite's that shows up there fails the test that was running. At the end it checks your focused workspace and window are what they were. | the tools' own check: `/opt/omabox/share` a mount point and no system bus, as in every box and never on your desktop; the suite's watcher | `t_unit_pointer`, `t_unit_wait`, `t_unit_inspect` (the refusals); `t_unit_leak_scan`, and `t_leak_control`, which leaks on purpose into a box standing in for your desktop and must see each leak |
| **An agent in ai-jail drives only its own boxes, and gets no more than its jail.** The broker reads the jail's policy from its bwrap command line, which nothing in the jail can change, and refuses what it cannot read. It refuses boxes another jail or you started, folders and a network the jail was not given, a path outside the jail to write a shot to, and `omabox host`. | `broker_init`, `jail_policy`, `select_box` | `t_jail`, `t_unit_jail_policy`, `t_unit_broker_units` |

The agent guard (`omabox guard`) gives agents' shells a display that does not exist, so a GUI app
an agent starts by mistake fails instead of opening on your desktop (`t_guard`,
`t_unit_guard_settings`, `t_unit_guard_exec_host`). It catches mistakes. It is not a boundary: an
agent can still run anything you could.
