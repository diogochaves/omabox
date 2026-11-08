# omabox with ai-jail

Running agents in ai-jail that drive boxes of their own, through omabox's broker. Back to the
[README](../README.md).

[ai-jail](https://github.com/akitaonrails/ai-jail) ([aijail.io](https://aijail.io)), by Fabio
Akita, fences the agent in: bubblewrap, Landlock and seccomp on Linux, `sandbox-exec` on macOS, so
your home, keys and cloud credentials are out of reach. ai-jail answers what the agent can touch,
omabox where it draws, and they stack. To run a build you do not trust yet with the box's screen as
its only display (tested with ai-jail 3.3.0 and omabox 1.4):

```bash
omabox up
eval "$(omabox env)"   # Wayland clients now draw in the box
ai-jail --gpu --rw-map "$WAYLAND_DISPLAY" --env WAYLAND_DISPLAY -- ./build/app
```

An agent inside ai-jail can drive boxes of its own through omabox's broker
([#16](https://github.com/diogochaves/omabox/issues/16)), with no change to ai-jail. The jail
cannot enter a box itself (its seccomp filter refuses new namespaces), so its `omabox` hands each
command to the broker outside:

```bash
omabox broker on   # a systemd user socket; prints the ro_maps to add to ai-jail's config.toml
```

Add those `ro_maps` (they map omabox, its relay, the skill and the broker's socket into the jail,
read-only) to ai-jail's global config, `~/.config/ai-jail/config.toml` (`$XDG_CONFIG_HOME/ai-jail/`
when that is set; ai-jail before 3.0 read `~/.ai-jail`, and 3.0 moves that file there on its first
launch). That file is ai-jail's trusted policy, so the maps need no approval per project: ai-jail
3's first-run screen in a project asks about the rest. Then run `ai-jail claude` as usual: `omabox up`, `shot`, `keys`, `click` and the rest
work inside the jail as outside. The broker keeps a jail's boxes to what the jail has: no network
when the jail has none (`--net isolated`), only the jail's own project mounted (when it is a git
repo: `up` says so of one without a `.git`, with the `--ro-bind` that brings it; what the jail hides
in it, its `.ai-jail` included, hidden in the box too), and only the boxes
that jail started (never yours, nor another jail's), which go down when the jail exits. Driving a
box is running code in it, so that is what stops a box from being a way out of the jail. Not for a
jailed agent: `omabox host`, `peek`, interactive boxes, `guard`, `config` changes. `omabox ports`
lists the jail's boxes and says a host process, another user or a box outside the jail holds a port
without naming it. A shot or a recording (`record`, its marks, captions and contact sheet) is written into
the jail by its own `omabox`; the broker never opens a path the jail names.
Since ai-jail 2.8, a bare `ai-jail claude` reaches only the hosts its proxy lets through (its
model's API, and in 3.x what the project's onboarding allowed, such as package registries); its
boxes still get no network. `omabox broker off` turns it off.

On Omarchy 4.5, `claude`, `codex` and `grok` are mise command wrappers (Omarchy runs each as the
subscription account you picked), and mise takes a lock in its data folder every time a wrapper or
one of its shims runs. ai-jail 3.3 maps that folder read-only, so `ai-jail claude` stops at `mise
ERROR Read-only file system`, and `python3` or `node` from mise fail inside the jail too. Until
ai-jail lets mise take that lock, give the jail a copy-on-write mise data folder (ai-jail keeps its
writes in the project's `.ai-jail-overlays`, which it git-ignores):

```bash
ai-jail --mise --overlay-map ~/.local/share/mise claude
```

Tested with ai-jail 3.3.0 and omabox 1.4 on Linux, and before that with ai-jail 2.2.1, 2.6.2,
2.8.1, 2.10.0 and 2.12.0.
