#!/usr/bin/env bash
# Runs in a delegated scope: bind our own cgroup writable, new cgroup ns, then systemd --user inside.
cg=/sys/fs/cgroup$(sed -n 's/^0:://p' /proc/self/cgroup)
echo "scope cgroup: $cg"
exec bwrap --unshare-user --uid "$(id -u)" --gid "$(id -g)" --unshare-pid --unshare-ipc --unshare-uts --unshare-cgroup \
  --ro-bind /usr /usr --symlink usr/bin /bin --symlink usr/lib /lib --symlink usr/lib /lib64 --ro-bind /etc /etc \
  --proc /proc --dev /dev --ro-bind /sys /sys --bind "$cg" /sys/fs/cgroup \
  --tmpfs /run --dir /run/systemd/system --dir /run/user/$(id -u) --tmpfs /tmp --tmpfs /home --dir /home/sbx --bind "$OUT" /out \
  --setenv HOME /home/sbx --unsetenv XDG_CONFIG_HOME --unsetenv XDG_DATA_HOME --unsetenv XDG_STATE_HOME --unsetenv XDG_CACHE_HOME --setenv XDG_RUNTIME_DIR /run/user/$(id -u) --unsetenv DBUS_SESSION_BUS_ADDRESS \
  --unsetenv WAYLAND_DISPLAY --unsetenv HYPRLAND_INSTANCE_SIGNATURE --unsetenv DISPLAY \
  timeout 20 bash -c 'exec > /out/spike.txt 2>&1
    /usr/lib/systemd/systemd --user --log-target=console --log-level=notice > /tmp/sd.log 2>&1 &
    for i in $(seq 50); do systemctl --user is-system-running >/dev/null 2>&1 && break; sleep 0.1; done
    echo "state: $(systemctl --user is-system-running 2>&1)"
    systemd-run --user --on-active=1 --timer-property=AccuracySec=100ms --unit spike-timer touch /tmp/fired 2>&1
    mkdir -p ~/.config/systemd/user; printf "[Unit]\nDescription=demo\n[Service]\nExecStart=/usr/bin/sleep 300\n[Install]\nWantedBy=default.target\n" > ~/.config/systemd/user/demo.service
    systemd-analyze --user unit-paths | head -4; systemctl --user show-environment | grep -E "HOME|XDG"; systemctl --user daemon-reload; systemctl --user enable --now demo.service 2>&1; systemctl --user is-active demo.service; systemctl --user show -p ControlGroup demo.service
    systemd-run --user --unit spike-svc sleep 30 </dev/null >/dev/null 2>&1
    sleep 2; ls /tmp/fired 2>&1
    systemctl --user status spike-svc 2>&1 | head -4
    systemctl --user --failed --no-legend 2>&1 | head
    echo "--- log"; head -20 /tmp/sd.log
    cat /proc/self/cgroup; echo; cat /tmp/sd.log | tail -15; kill -KILL -1'
