#!/usr/bin/env bash
# test/distros-vm.sh [--keep] [--suite] [DISTRO...]: install.sh, doctor and real boxes on another
# distribution in a throwaway libvirt VM (#247), where test/distros.sh's containers fall short: the
# guest's own kernel and security module (SELinux on Fedora and Tumbleweed), a real login session
# (systemd --user, logind's runtime dir) and a real sudo. DISTRO: arch (Arch without Omarchy, the
# default), fedora, tumbleweed, debian.
#
# Each run: the distribution's official cloud image (downloaded once, its checksum checked) under a
# fresh overlay disk, cloud-init making a user with a throwaway SSH key and password-less sudo, the
# distribution's Hyprland, a terminal and a font installed first (what a user has before omabox),
# then over SSH as that user: this checkout's HEAD as a release tarball has it (git archive, its
# export-ignore honoured), install.sh until it passes (each `sudo CMD` it stops with run with the
# guest's sudo, made non-interactive), omabox setup and doctor, test/distros-boxes.sh's plain boxes,
# and with --suite the whole of test/run.sh, from a Hyprland session autologged in on tty1. The VM
# is destroyed afterwards unless --keep (then `ssh -i DIR/<distro>/key user@IP`, printed).
#
# Boxes in a guest need GPU acceleration: virgl, rendered by QEMU's egl-headless on a host render
# node, an NVIDIA one first (OMABOX_VM_RENDERNODE picks another). Never an AMD node: QEMU (11.1)
# segfaults on exit there. NVIDIA's EGL is forced for QEMU (glvnd would hand it Mesa), and libvirt's
# cgroup_device_acl must list /dev/nvidia0, nvidiactl, nvidia-modeset and nvidia-uvm.
#
# Disks in OMABOX_VM_DIR (default /var/lib/libvirt/images/omabox, writable by you and readable by
# libvirt: `sudo install -d -o $USER -g libvirt -m 2775 DIR` once); logs, shots and the suite's
# evidence in ${XDG_STATE_HOME:-~/.local/state}/omabox/distros-vm/<distro>/. Exit 1 when one failed.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
VMDIR=${OMABOX_VM_DIR:-/var/lib/libvirt/images/omabox}
LOGS=${XDG_STATE_HOME:-$HOME/.local/state}/omabox/distros-vm
CONNECT=qemu:///system
keep=0 suite=0
while [ $# -gt 0 ]; do
  case $1 in
    --keep) keep=1 ;;
    --suite) suite=1 ;;
    -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    -*) echo "distros-vm.sh: unknown option $1" >&2; exit 2 ;;
    *) break ;;
  esac
  shift
done
[ $# -gt 0 ] || set -- arch
for c in virsh virt-install qemu-img ssh ssh-keygen curl git xorriso; do
  command -v "$c" >/dev/null || { echo "distros-vm.sh: needs $c" >&2; exit 2; }
done
[ -d "$VMDIR" ] && [ -w "$VMDIR" ] || {
  echo "distros-vm.sh: $VMDIR is missing or not writable; once: sudo install -d -o $USER -g libvirt -m 2775 $VMDIR" >&2
  exit 2
}

node=${OMABOX_VM_RENDERNODE:-}
if [ -z "$node" ]; then
  for n in /dev/dri/renderD*; do
    [[ $(readlink "/sys/class/drm/${n##*/}/device/driver" 2>/dev/null) == */nvidia ]] && { node=$n; break; }
  done
fi
[ -n "$node" ] || {
  echo "distros-vm.sh: no NVIDIA render node (QEMU segfaults on exit on an AMD one); OMABOX_VM_RENDERNODE=/dev/dri/renderDN to insist" >&2
  exit 2
}

# image DISTRO: sets url (the qcow2), sums (its checksum file), algo, osinfo; prep, the root commands
# that give the guest a Hyprland user's starting point; fix, how a `sudo CMD` install.sh names is
# made non-interactive.
image() {
  case $1 in
    arch)
      url=https://geo.mirror.pkgbuild.com/images/latest/Arch-Linux-x86_64-cloudimg.qcow2
      sums=$url.SHA256 algo=sha256 osinfo=archlinux
      prep='pacman -Syu --noconfirm --needed hyprland foot ttf-dejavu' ;;
    fedora)
      local d=https://download.fedoraproject.org/pub/fedora/linux/releases/45/Cloud/x86_64/images
      url=$d/Fedora-Cloud-Base-Generic-45-1.4.x86_64.qcow2
      sums=$d/Fedora-Cloud-45-1.4-x86_64-CHECKSUM algo=sha256 osinfo=fedora-rawhide
      # (Fedora 45 has no Hyprland but COPR's: pm4rcin/Hyprland builds 0.57 for it, solopasha/hyprland
      # only rawhide; its own Quickshell is 0.2.1, doctor says whether that will do)
      prep='dnf -y -q install dnf-plugins-core && dnf -y -q copr enable pm4rcin/Hyprland &&
        dnf -y -q install hyprland foot dejavu-sans-fonts && { dnf -y -q install quickshell || echo "(no quickshell)"; }' ;;
    tumbleweed)
      url=https://download.opensuse.org/tumbleweed/appliances/openSUSE-Tumbleweed-Minimal-VM.x86_64-Cloud.qcow2
      sums=$url.sha256 algo=sha256 osinfo=opensusetumbleweed
      prep='zypper -n -q refresh && zypper -n -q install hyprland foot dejavu-fonts' ;;
    debian)
      local d=https://cloud.debian.org/images/cloud/sid/daily/latest
      url=$d/debian-sid-generic-amd64-daily.qcow2
      sums=$d/SHA512SUMS algo=sha512 osinfo=debiantesting
      prep='apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -q hyprland foot fonts-dejavu-core' ;;
    *) echo "distros-vm.sh: unknown distribution $1 (arch, fedora, tumbleweed, debian)" >&2; return 1 ;;
  esac
}

# fetch: the base image into VMDIR once, its checksum checked against the distribution's file.
fetch() {
  local base=$VMDIR/${url##*/} want got
  want=$(curl -fsSL --retry 3 "$sums" | grep -F "${url##*/}" | grep -oE '\b[0-9a-f]{64,128}\b' | head -n 1) || want=
  [ -n "$want" ] || { echo "no checksum for ${url##*/} in $sums"; return 1; }
  if [ -f "$base" ]; then
    got=$("${algo}sum" "$base" | cut -d' ' -f1)
    [ "$got" = "$want" ] && { echo "$base"; return 0; }
    echo "   (cached ${url##*/} is not the current one: downloading again)" >&2
  fi
  curl -fL --retry 3 -o "$base.part" "$url" >&2 || return 1
  got=$("${algo}sum" "$base.part" | cut -d' ' -f1)
  [ "$got" = "$want" ] || { echo "checksum mismatch for ${url##*/}"; rm -f "$base.part"; return 1; }
  mv -f "$base.part" "$base" && echo "$base"
}

virsh_() { virsh -c "$CONNECT" "$@"; }
gone_vm() {
  virsh_ destroy "$1" >/dev/null 2>&1 || true
  virsh_ undefine --nvram --remove-all-storage "$1" >/dev/null 2>&1 || virsh_ undefine "$1" >/dev/null 2>&1 || true
}

run_one() {
  local distro=$1 name=omabox-dvm-$1 dir=$VMDIR/$1 out=$LOGS/$1 base ip='' i
  image "$distro" || return 1
  echo "==> image"
  base=$(fetch) || { echo "$base"; return 1; }
  gone_vm "$name"
  rm -rf "$dir" && mkdir -p "$dir"
  qemu-img create -q -f qcow2 -b "$base" -F qcow2 "$dir/disk.qcow2" 40G
  ssh-keygen -q -t ed25519 -N '' -C "$name" -f "$dir/key"
  cat > "$dir/user-data" <<EOF
#cloud-config
users:
  - name: user
    shell: /bin/bash
    lock_passwd: true
    sudo: "ALL=(ALL) NOPASSWD:ALL"
    ssh_authorized_keys: ["$(cat "$dir/key.pub")"]
ssh_pwauth: false
EOF
  local gfx=(--video "virtio,accel3d=yes" --graphics spice --graphics "egl-headless,gl.rendernode=$node")
  [[ $(readlink "/sys/class/drm/${node##*/}/device/driver") != */nvidia ]] ||
    gfx+=(--qemu-commandline "env=__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/10_nvidia.json")
  echo "==> VM $name (virgl on $node)"
  virt-install --connect "$CONNECT" --name "$name" --osinfo "$osinfo" --import \
    --memory "${OMABOX_VM_MEM:-8192}" --vcpus "${OMABOX_VM_CPUS:-8}" --cpu host-passthrough --machine q35 \
    --disk "path=$dir/disk.qcow2,format=qcow2,bus=virtio" --network network=default,model=virtio \
    "${gfx[@]}" --cloud-init "user-data=$dir/user-data" --noautoconsole >/dev/null || return 1

  # (the guest's address and SSH, waited for: at its first boot and after the reboot below)
  ssh_up() {
    for i in $(seq 1 90); do
      ip=$(virsh_ domifaddr "$name" --source lease 2>/dev/null | awk '/ipv4/ { sub(/\/.*/, "", $4); print $4; exit }') || ip=
      [ -n "$ip" ] && ssh "${SSHO[@]}" -i "$dir/key" "user@$ip" true 2>/dev/null && return 0
      ip=''; sleep 2
    done
    echo "no SSH to the guest after 3 min"; return 1
  }
  ssh_up || return 1
  echo "   up at $ip"
  g() { ssh "${SSHO[@]}" -i "$dir/key" "user@$ip" "$@"; }
  g 'cloud-init status --wait >/dev/null 2>&1; true'

  echo "==> the guest"
  # shellcheck disable=SC2016 # expanded in the guest
  g 'grep -h PRETTY_NAME /etc/os-release; uname -r; (getenforce 2>/dev/null || echo "SELinux: none"); ls -l /dev/dri/; echo "XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR"'
  # The user as a desktop user: logged in on the VM's monitor (autologin on tty1 starting Hyprland,
  # from the reboot below), so logind gives them the render node (Tumbleweed's is the render group's
  # only, and an SSH login gets no seat) and the suite has the session it checks boxes leave alone;
  # lingering, so the runtime dir outlives each SSH command.
  echo "==> as root: Hyprland, a terminal, a font; a desktop session on tty1; the user lingers"
  # shellcheck disable=SC2016 # expanded in the guest
  g "sudo sh -c '$prep' && sudo loginctl enable-linger user"' &&
    sudo mkdir -p /etc/systemd/system/getty@tty1.service.d &&
    printf "[Service]\nExecStart=\nExecStart=-/sbin/agetty -o \"-p -f -- \\\\\\\\u\" --noclear --autologin user %%I \$TERM\n" |
      sudo tee /etc/systemd/system/getty@tty1.service.d/autologin.conf >/dev/null &&
    echo "[ \"\$(tty)\" = /dev/tty1 ] && [ -z \"\${WAYLAND_DISPLAY:-}\" ] && exec start-hyprland > ~/hyprland-session.log 2>&1" >> ~/.bash_profile' \
    >"$out/prep.log" 2>&1 || { tail -n 20 "$out/prep.log"; return 1; }
  # A reboot, as after any update: the packages may have replaced the running kernel, whose modules
  # are then gone (nft and a dummy link failed so on Arch's cloud image).
  # virt-install --cloud-init treats the first boot as an install: the guest powers off at its first
  # reboot, and is started again here.
  echo "==> reboot"
  local boot st
  boot=$(g 'cat /proc/sys/kernel/random/boot_id') || boot=
  g 'sudo systemctl reboot' >/dev/null 2>&1 || true
  for i in $(seq 1 60); do
    st=$(virsh_ domstate "$name" 2>/dev/null) || st=
    if [ "$st" = "shut off" ]; then virsh_ start "$name" >/dev/null || return 1; break; fi
    [ "$st" = running ] && [ "$(g 'cat /proc/sys/kernel/random/boot_id' 2>/dev/null)" != "$boot" ] &&
      [ -n "$(g true 2>/dev/null && echo y)" ] && break
    sleep 2
  done
  ssh_up || return 1
  g 'uname -r'
  # shellcheck disable=SC2016 # expanded in the guest
  g 'for i in $(seq 1 30); do ls "$XDG_RUNTIME_DIR"/hypr/*/.socket.sock >/dev/null 2>&1 && exit 0; sleep 1; done; exit 1' ||
    { echo "no Hyprland session on tty1 (~/hyprland-session.log in the guest)"; return 1; }
  g 'Hyprland --version 2>&1 | head -n 1; qs --version 2>&1 | head -n 1 || true'

  echo "==> HEAD, as a release tarball has it"
  git -C "$ROOT" archive --format=tar --prefix=omabox/ HEAD | g 'tar -xf -'

  # shellcheck disable=SC2016 # expanded in the guest
  local P='export PATH=$HOME/.local/bin:$PATH; cd ~/omabox;' cmd ok=0 o
  for i in 1 2 3 4; do
    echo "==> install.sh (run $i)"
    if o=$(g "$P ./install.sh </dev/null" 2>&1); then echo "$o"; ok=1; break; fi
    echo "$o"
    cmd=$(printf '%s\n' "$o" | grep -o 'sudo [a-z-]* .*$' | tail -n 1) || cmd=
    [ -n "$cmd" ] || break
    case $cmd in
      'sudo pacman '*) cmd="$cmd --noconfirm" ;;
      'sudo apt '*) cmd="sudo env DEBIAN_FRONTEND=noninteractive apt-get install -y -q ${cmd#sudo apt install }" ;;
      'sudo zypper '*) cmd="sudo zypper -n ${cmd#sudo zypper }" ;;
      'sudo dnf '*) cmd="sudo dnf -y ${cmd#sudo dnf }" ;;
    esac
    echo "==> $cmd"
    g "$cmd" >"$out/pkg-$i.log" 2>&1 || { tail -n 20 "$out/pkg-$i.log"; break; }
  done
  echo "==> omabox setup"; g "$P omabox setup </dev/null" 2>&1; echo "setup exit $?"
  echo "==> omabox doctor"; g "$P omabox doctor" 2>&1; echo "doctor exit $?"
  g "$P omabox --version" 2>&1 || true
  [ $ok = 1 ] || return 1

  local rc=0
  echo "==> boxes"
  g "$P OMABOX_DISTROS_VM=1 bash test/distros-boxes.sh ~/out" 2>&1 || rc=1
  g 'tar -cf - -C ~/out .' | tar -xf - -C "$out" 2>/dev/null || true
  if [ $suite = 1 ]; then
    # (from a git repo, as a developer runs it: a box mounts the git repo it is started from, and
    # the suite's stand-ins run this omabox inside boxes; a tarball has no .git)
    echo "==> the suite"
    g 'cd ~/omabox && git init -q && git add -A && git -c user.name=t -c user.email=t@t commit -q -m tarball' ||
      { echo "no git repo for the suite"; return 1; }
    # shellcheck disable=SC2016 # expanded in the guest
    g "$P"' export HYPRLAND_INSTANCE_SIGNATURE=$(ls -t "$XDG_RUNTIME_DIR"/hypr/ | head -n 1) WAYLAND_DISPLAY=wayland-1;
      test/run.sh' > "$out/suite.log" 2>&1 || rc=1
    tail -n 30 "$out/suite.log"
    g 'tar -cf - -C ~/.local/state/omabox test 2>/dev/null' | tar -xf - -C "$out" 2>/dev/null || true
  fi
  if [ $keep = 1 ]; then echo "   kept: ssh -i $dir/key user@$ip (virsh -c $CONNECT destroy $name when done)"
  else gone_vm "$name"; rm -rf "$dir"; fi
  return $rc
}

SSHO=(-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o ConnectTimeout=5 -o BatchMode=yes)
rc=0
for d in "$@"; do
  rm -rf "${LOGS:?}/$d" && mkdir -p "$LOGS/$d"
  echo "== $d (log: $LOGS/$d.log)"
  if run_one "$d" > "$LOGS/$d.log" 2>&1; then
    echo "   passed: $(grep -E '^all [0-9]+ checks|^boxes: ' "$LOGS/$d.log" | tr '\n' ' ')"
  else
    rc=1
    echo "   FAILED: $(grep -E 'install.sh: |FAIL|^boxes: |^no |mismatch' "$LOGS/$d.log" | tail -n 2 | tr '\n' ' ')"
    [ $keep = 1 ] || { gone_vm "omabox-dvm-$d"; rm -rf "${VMDIR:?}/$d"; }
  fi
done
exit $rc
