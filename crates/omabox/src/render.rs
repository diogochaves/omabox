//! Where a box renders (#324 wave 5a): its render node, an interactive box's, and NVIDIA's device
//! nodes for it (S1: render nodes only).
//!
//! S1 by construction (#355): a box's GPU goes in only as one of the types here, each made only from a
//! path of its kind. A RenderNode is `/dev/dri/renderD<digits>` and nothing else, so the seat's card
//! nodes (`/dev/dri/card*`, which Hyprland would take the seat through: finding 5) can never be one;
//! an NvidiaDev is `/dev/nvidia<digits>`, `/dev/nvidiactl` or `/dev/nvidia-uvm`; NVIDIA's EGL wants the
//! card's path to exist (finding 294), which a CardPlaceholder gives as an empty file of the box dir's,
//! read-only at `/dev/dri/cardN`, never the device.

use crate::cat;
use crate::exit::{Ctx, Res, say};
use crate::gpu::{at, conf_gpu, gpu_is_in, gpu_kind, render_driver_in, render_slot_in};
use crate::shell;
use std::os::unix::ffi::OsStrExt;

/// Where render nodes and their devices are (bash's DRI, SYSDRM, SYSPCI). The helpers that read them
/// have `_in` forms that take a ROOT they are under: "/" (what the binary always gives), or a
/// stand-in tree's for the unit tests (#324 wave 6-suite). A render node keeps its own path, the
/// box's; only the reads are under ROOT.
pub const DRI: &str = "/dev/dri";
pub const SYSDRM: &str = "/sys/class/drm";
pub const SYSPCI: &str = "/sys/bus/pci/devices";

/// A GPU render node, `/dev/dri/renderD<digits>`: the only DRM node a box gets (S1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderNode(Vec<u8>);

impl RenderNode {
    /// P when it is a render node's path, as written (no `..`, no trailing slash): else None.
    pub fn new(p: &[u8]) -> Option<RenderNode> {
        shell::re_match("^/dev/dri/renderD[0-9]+$", p).then(|| RenderNode(p.to_vec()))
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    /// Its driver (render_driver).
    pub fn driver(&self) -> Vec<u8> {
        render_driver_in(&self.0, b"/")
    }
    /// Its driver, SYSDRM under ROOT.
    pub fn driver_in(&self, root: &[u8]) -> Vec<u8> {
        render_driver_in(&self.0, root)
    }
    /// Its GPU's PCI slot (render_slot).
    pub fn slot(&self) -> Vec<u8> {
        render_slot_in(&self.0, b"/")
    }
    /// The GPU is NVIDIA's (its driver is nvidia).
    pub fn is_nvidia(&self) -> bool {
        self.driver() == b"nvidia"
    }
}

/// One of NVIDIA's userspace device nodes a box's GPU needs: `/dev/nvidia<digits>`, `/dev/nvidiactl`
/// or `/dev/nvidia-uvm` (CUDA's, #149). Never a DRM node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NvidiaDev(Vec<u8>);

impl NvidiaDev {
    pub fn new(p: &[u8]) -> Option<NvidiaDev> {
        (p == b"/dev/nvidiactl"
            || p == b"/dev/nvidia-uvm"
            || shell::re_match("^/dev/nvidia[0-9]+$", p))
        .then(|| NvidiaDev(p.to_vec()))
    }
    pub fn ctl() -> NvidiaDev {
        NvidiaDev(b"/dev/nvidiactl".to_vec())
    }
    pub fn uvm() -> NvidiaDev {
        NvidiaDev(b"/dev/nvidia-uvm".to_vec())
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// An empty file of the box dir's (`$D/card-placeholder`), to be bound read-only at `/dev/dri/cardN`:
/// NVIDIA's Wayland EGL stats the GPU's primary node before its render node and gives up without it
/// (#204, finding 294); it never opens it. Never the device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardPlaceholder {
    src: Vec<u8>,
    dest: Vec<u8>,
}

impl CardPlaceholder {
    /// `: > "$D/card-placeholder"`, for card CARD (`card<digits>`, nvidia_card's): the file made (or
    /// emptied) and checked to be a regular, empty file of D's, then the bind's two paths.
    pub fn make(d: &[u8], card: &[u8]) -> std::io::Result<CardPlaceholder> {
        let bad = || std::io::Error::from(std::io::ErrorKind::InvalidInput);
        if !shell::re_match("^card[0-9]+$", card) {
            return Err(bad());
        }
        let src = cat!(d, "/card-placeholder");
        if !src.starts_with(b"/")
            || [b"/dev/".as_slice(), b"/sys/", b"/proc/"]
                .iter()
                .any(|x| src.starts_with(x))
        {
            return Err(bad());
        }
        std::fs::File::create(shell::path(&src))?;
        let m = std::fs::symlink_metadata(shell::path(&src))?;
        if !m.file_type().is_file() || m.len() != 0 {
            return Err(bad());
        }
        Ok(CardPlaceholder {
            src,
            dest: cat!(DRI, "/", card),
        })
    }
    /// The empty file, on the host.
    pub fn src(&self) -> &[u8] {
        &self.src
    }
    /// Where the box sees it: `/dev/dri/cardN`.
    pub fn dest(&self) -> &[u8] {
        &self.dest
    }
}

/// `access(P, MODE) == 0`: `[ -r P ]`, `[ -w P ]` as test(1) asks them.
fn access(p: &[u8], mode: libc::c_int) -> bool {
    std::ffi::CString::new(p.to_vec()).is_ok_and(|c| {
        // SAFETY: c is a valid C string for the call.
        unsafe { libc::access(c.as_ptr(), mode) == 0 }
    })
}

/// `[ -r P ] && [ -w P ]`, P under ROOT.
fn rw(root: &[u8], p: &[u8]) -> bool {
    let p = at(root, p);
    access(&p, libc::R_OK) && access(&p, libc::W_OK)
}

/// `for n in "$DRI"/renderD*`: the render nodes' paths, in byte order (finding 446); DRI under ROOT,
/// the paths as the system names them.
fn render_globbed(root: &[u8]) -> Vec<Vec<u8>> {
    let Ok(rd) = std::fs::read_dir(shell::path(&at(root, DRI.as_bytes()))) else {
        return Vec::new();
    };
    let mut v: Vec<Vec<u8>> = rd
        .flatten()
        .map(|e| std::os::unix::ffi::OsStringExt::into_vec(e.file_name()))
        .filter(|n| n.starts_with(b"renderD"))
        .map(|n| cat!(DRI, "/", n))
        .collect();
    v.sort();
    v
}

/// Every render node's path under ROOT, usable or not (`omabox doctor` lists them all).
pub fn render_all_in(root: &[u8]) -> Vec<Vec<u8>> {
    render_globbed(root)
}

/// Whether node N (its path, read under ROOT) is one a box can use: readable and writable.
pub fn render_rw_in(n: &[u8], root: &[u8]) -> bool {
    rw(root, n)
}

/// Whether render node N (read under ROOT) renders in software only (#395, install.sh's test, kept
/// in step): virtio-gpu (a VM's) with neither virgl nor context types, as its virtio device's
/// features say (one 0/1 per bit: 0 virgl, 4 context init). That device is the node's device's child
/// virtioN on PCI (device/ is virtio-pci's), or device/ itself (virtio-mmio).
pub fn software_only_in(n: &[u8], root: &[u8]) -> bool {
    let node = n.rsplit(|&c| c == b'/').next().unwrap_or_default();
    let dev = at(root, &cat!(SYSDRM, "/", node, "/device"));
    let mut devs = vec![dev.clone()];
    devs.extend(
        shell::glob(&shell::path(&dev), true)
            .into_iter()
            .filter(|c| c.starts_with(b"virtio"))
            .map(|c| cat!(dev, "/", c)),
    );
    devs.iter().any(|d| {
        let drv = crate::repo::readlink_f(&cat!(d, "/driver"))
            .map(|p| shell::basename(&p))
            .unwrap_or_default();
        let f = std::fs::read(shell::path(&cat!(d, "/features"))).unwrap_or_default();
        drv == b"virtio_gpu" && f.first() == Some(&b'0') && f.get(4) == Some(&b'0')
    })
}

/// conf_get gpu: the `gpu` setting (auto by default).
fn conf_want() -> Vec<u8> {
    crate::conf::conf_get("gpu", conf_gpu, b"auto")
}

/// render_node: the GPU render node a headless box renders with: $OMABOX_RENDER_NODE; else the first
/// usable one the `gpu` setting names (#156, finding 214); else, and when that GPU is not there (an
/// NVIDIA card handed to a VM), the first usable one, said with why (finding 237). Only ever a render
/// node, never a card node: that is the seat (finding 5).
pub fn render_node(ctx: &Ctx) -> Res<RenderNode> {
    render_node_with(ctx, shell::env_or_empty("OMABOX_RENDER_NODE"))
}

/// render_node with OMABOX_RENDER_NODE as N (`OMABOX_RENDER_NODE="" render_node`: the setting's).
pub fn render_node_with(ctx: &Ctx, n: Vec<u8>) -> Res<RenderNode> {
    render_node_in(ctx, n, b"/")
}

/// render_node_with, the devices under ROOT.
pub fn render_node_in(ctx: &Ctx, n: Vec<u8>, root: &[u8]) -> Res<RenderNode> {
    let mut n = n;
    if n.is_empty() {
        let want = conf_want();
        let mut fell = false;
        if want != b"auto" {
            n = render_globbed(root)
                .into_iter()
                .find(|x| rw(root, x) && gpu_is_in(x, &want, root))
                .unwrap_or_default();
            fell = n.is_empty();
        }
        if n.is_empty() {
            n = render_globbed(root)
                .into_iter()
                .find(|x| rw(root, x))
                .unwrap_or_default();
        }
        if fell && !n.is_empty() {
            let node = n.rsplit(|&c| c == b'/').next().unwrap_or_default();
            say(cat!(
                "gpu ",
                want,
                " (omabox config gpu): none usable here (",
                gpu_why_not_in(&want, root),
                "), so the first GPU: ",
                render_driver_in(&n, root),
                " ",
                render_slot_in(&n, root),
                " (",
                node,
                ")"
            ));
        }
    }
    match RenderNode::new(&n) {
        Some(r) if rw(root, &n) => Ok(r),
        _ => Err(ctx.die(cat!(
            "no usable GPU render node (",
            DRI,
            "/renderD*); set OMABOX_RENDER_NODE"
        ))),
    }
}

/// render_nodes: every usable render node, for an interactive box, whose Hyprland renders on the GPU
/// the desktop's compositor names, not necessarily render_node's (finding 95). Render nodes only.
pub fn render_nodes() -> Vec<RenderNode> {
    render_nodes_in(b"/")
}

/// render_nodes, the devices under ROOT.
pub fn render_nodes_in(root: &[u8]) -> Vec<RenderNode> {
    render_globbed(root)
        .into_iter()
        .filter(|n| rw(root, n))
        .filter_map(|n| RenderNode::new(&n))
        .collect()
}

/// render_fallback N: the `gpu` setting names a GPU and N is not it, and OMABOX_RENDER_NODE did not
/// choose.
pub fn render_fallback(n: &RenderNode) -> bool {
    render_fallback_in(n, b"/")
}

/// render_fallback, the devices under ROOT.
pub fn render_fallback_in(n: &RenderNode, root: &[u8]) -> bool {
    if !shell::env_or_empty("OMABOX_RENDER_NODE").is_empty() {
        return false;
    }
    let want = conf_want();
    want != b"auto" && !gpu_is_in(n.as_bytes(), &want, root)
}

/// The `gpu` setting's note test (config gpu VALUE): a usable render node is the GPU WANT names.
pub fn gpu_usable_in(want: &[u8], root: &[u8]) -> bool {
    render_nodes_in(root)
        .iter()
        .any(|n| gpu_is_in(n.as_bytes(), want, root))
}

/// config --json's gpu_auto and gpu_now: the slots of the first usable GPU and of the one a headless
/// box renders on now (render_node's choice without OMABOX_RENDER_NODE), empty for none.
pub fn gpu_auto_now_in(ctx: &Ctx, root: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let now = crate::exit::muted(|| render_node_in(ctx, Vec::new(), root))
        .map(|n| render_slot_in(n.as_bytes(), root))
        .unwrap_or_default();
    let auto = render_nodes_in(root)
        .first()
        .map(|n| render_slot_in(n.as_bytes(), root))
        .unwrap_or_default();
    (auto, now)
}

/// pci_driver DEV: the driver a PCI device (its dir in SYSPCI) is bound to, or nothing.
fn pci_driver(dev: &[u8]) -> Vec<u8> {
    let d = cat!(dev, "/driver");
    if !shell::path(&d).exists() {
        return Vec::new();
    }
    crate::repo::readlink_f(&d)
        .map(|p| shell::basename(&p))
        .unwrap_or_default()
}

/// `$(cat FILE)`.
fn read_subst(p: &[u8]) -> Vec<u8> {
    std::fs::read(shell::path(p))
        .map(|v| shell::subst(&v))
        .unwrap_or_default()
}

/// pci_kind DEV: by its driver, else (vfio-pci, or none) by its vendor, so an NVIDIA card handed to a
/// VM is still one (finding 237).
fn pci_kind(dev: &[u8]) -> Vec<u8> {
    let k = gpu_kind(&pci_driver(dev));
    if !k.is_empty() {
        return k.to_vec();
    }
    match read_subst(&cat!(dev, "/vendor")).as_slice() {
        b"0x10de" => b"nvidia".to_vec(),
        b"0x1002" => b"amd".to_vec(),
        b"0x8086" => b"intel".to_vec(),
        _ => Vec::new(),
    }
}

/// slot_node SLOT: the render node of the GPU at PCI SLOT, or nothing.
fn slot_node(slot: &[u8], root: &[u8]) -> Vec<u8> {
    render_globbed(root)
        .into_iter()
        .find(|n| shell::path(&at(root, n)).exists() && render_slot_in(n, root) == slot)
        .unwrap_or_default()
}

/// pci_why DEV: why a box cannot render on the GPU at DEV, or nothing when it can.
fn pci_why(dev: &[u8], root: &[u8]) -> Vec<u8> {
    let drv = pci_driver(dev);
    let node = slot_node(&shell::basename(dev), root);
    if drv.is_empty() {
        b"no driver".to_vec()
    } else if node.is_empty() {
        cat!("bound to ", drv)
    } else if !rw(root, &node) {
        cat!("no access to ", node)
    } else {
        Vec::new()
    }
}

/// The PCI devices' names (`"$SYSPCI"/*`), in byte order; SYSPCI under ROOT.
fn pci_devs(root: &[u8]) -> Vec<Vec<u8>> {
    let mut devs: Vec<Vec<u8>> = std::fs::read_dir(shell::path(&at(root, SYSPCI.as_bytes())))
        .map(|rd| {
            rd.flatten()
                .map(|e| std::os::unix::ffi::OsStringExt::into_vec(e.file_name()))
                .filter(|n| n.first() != Some(&b'.'))
                .collect()
        })
        .unwrap_or_default();
    devs.sort();
    devs
}

/// lspci's name of the GPU at SLOT: the model's bracketed name (`GB203 [GeForce RTX 5070 Ti]`), or
/// the whole model; nothing without lspci.
fn lspci_name(slot: &[u8]) -> Vec<u8> {
    if !crate::exec::executable_on_path(b"lspci") {
        return Vec::new();
    }
    let mut c = crate::sig::command("lspci");
    c.args(["-mm", "-s"])
        .arg(std::ffi::OsStr::from_bytes(slot))
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let o = c.output().map(|o| o.stdout).unwrap_or_default();
    // (awk -F'"' 'NR == 1 { print $6 }')
    let first = o.split(|&c| c == b'\n').next().unwrap_or_default();
    let name = first
        .split(|&c| c == b'"')
        .nth(5)
        .map(<[u8]>::to_vec)
        .unwrap_or_default();
    shell::re_group(r"\[(.+)\]", &name, 1).unwrap_or(name)
}

/// gpu_json: the GPUs on the PCI bus (display class 0x03) for the bar widget's picker (#118): each
/// one's slot, driver, kind, name, render node and whether a box can render on it, with why not.
/// From the bus, not /dev/dri, so a GPU handed to a VM (vfio-pci) is listed too.
pub fn gpu_json() -> serde_json::Value {
    gpu_json_in(b"/")
}

/// gpu_json, the devices under ROOT.
pub fn gpu_json_in(root: &[u8]) -> serde_json::Value {
    use serde_json::{Map, Value};
    let s = |v: &[u8]| Value::String(String::from_utf8_lossy(v).into_owned());
    let or_null = |v: &[u8]| if v.is_empty() { Value::Null } else { s(v) };
    let mut all = Vec::new();
    for slot in pci_devs(root) {
        let dev = at(root, &cat!(SYSPCI, "/", slot));
        if !read_subst(&cat!(dev, "/class")).starts_with(b"0x03") {
            continue;
        }
        let why = pci_why(&dev, root);
        let mut m = Map::new();
        m.insert("pci".into(), s(&slot));
        m.insert("driver".into(), s(&pci_driver(&dev)));
        m.insert("kind".into(), or_null(&pci_kind(&dev)));
        m.insert("name".into(), s(&lspci_name(&slot)));
        m.insert("node".into(), or_null(&slot_node(&slot, root)));
        m.insert("available".into(), Value::Bool(why.is_empty()));
        if !why.is_empty() {
            m.insert("why".into(), s(&why));
        }
        all.push(Value::Object(m));
    }
    Value::Array(all)
}

/// gpu_list: every render node, for `omabox config gpu` (#156): its driver, its slot, the values
/// that name it, and `*` on the one a headless box would render on now (render_node's choice, said
/// without its note).
pub fn gpu_list(ctx: &Ctx) -> Vec<u8> {
    gpu_list_in(ctx, b"/")
}

/// gpu_list, the devices under ROOT.
pub fn gpu_list_in(ctx: &Ctx, root: &[u8]) -> Vec<u8> {
    let usen =
        crate::exit::muted(|| render_node_in(ctx, shell::env_or_empty("OMABOX_RENDER_NODE"), root))
            .map(|n| n.as_bytes().to_vec())
            .unwrap_or_default();
    let mut o = Vec::new();
    let env = shell::env_or_empty("OMABOX_RENDER_NODE");
    if !env.is_empty() {
        o.extend_from_slice(&cat!(
            "OMABOX_RENDER_NODE=",
            env,
            " overrides the setting here\n"
        ));
    }
    o.extend_from_slice(
        b"GPUs (* = where a headless box renders now; set one with: omabox config gpu VALUE):\n",
    );
    let pad = |v: &[u8], w: usize| {
        let mut v = v.to_vec();
        while v.len() < w {
            v.push(b' ');
        }
        v
    };
    for n in render_globbed(root) {
        if !shell::path(&at(root, &n)).exists() {
            continue;
        }
        let d = render_driver_in(&n, root);
        let slot = render_slot_in(&n, root);
        let k = gpu_kind(&d);
        let mark: &[u8] = if n == usen { b"*" } else { b" " };
        let node = n.rsplit(|&c| c == b'/').next().unwrap_or_default();
        let names = if k.is_empty() {
            cat!("gpu ", slot)
        } else {
            cat!("gpu ", k, " or gpu ", slot)
        };
        let usable: &[u8] = if rw(root, &n) {
            b""
        } else {
            b" (not usable: no access)"
        };
        o.extend_from_slice(&cat!(
            mark,
            " ",
            pad(node, 11),
            " ",
            pad(&d, 8),
            " ",
            pad(&slot, 13),
            " ",
            names,
            usable,
            "\n"
        ));
    }
    o
}

/// gpu_why_not WANT: why no usable GPU is the one WANT names (a kind or a slot), each such GPU with
/// its reason ("0000:01:00.0: bound to vfio-pci"), or that there is none.
pub fn gpu_why_not(want: &[u8]) -> Vec<u8> {
    gpu_why_not_in(want, b"/")
}

/// gpu_why_not, the devices under ROOT.
pub fn gpu_why_not_in(want: &[u8], root: &[u8]) -> Vec<u8> {
    let mut why: Vec<u8> = Vec::new();
    for name in pci_devs(root) {
        let dev = at(root, &cat!(SYSPCI, "/", name));
        if !read_subst(&cat!(dev, "/class")).starts_with(b"0x03") {
            continue;
        }
        let ok = match want {
            b"nvidia" | b"amd" | b"intel" => pci_kind(&dev) == want,
            _ => name == want,
        };
        if !ok {
            continue;
        }
        let w = pci_why(&dev, root);
        if !why.is_empty() {
            why.extend_from_slice(b"; ");
        }
        let w = if w.is_empty() {
            b"usable now".to_vec()
        } else {
            w
        };
        why.extend_from_slice(&cat!(name, ": ", w));
    }
    if !why.is_empty() {
        return why;
    }
    match want {
        b"nvidia" | b"amd" | b"intel" => cat!("no ", want, " GPU on this machine"),
        _ => cat!("no GPU at ", want),
    }
}

/// nvidia_device RENDER [CREATE]: the /dev/nvidiaN of RENDER's GPU (nvidia_node); CREATE false
/// leaves a missing node missing. Never a DRM card.
pub fn nvidia_device(ctx: &Ctx, render: &RenderNode, create: bool) -> Res<NvidiaDev> {
    let slot = render.slot();
    let p = nvidia_node(ctx, &slot, create, b"/proc/driver/nvidia", b"/dev")?;
    NvidiaDev::new(&p).ok_or_else(|| {
        ctx.die(cat!(
            "NVIDIA GPU ",
            slot,
            ": ",
            p,
            " is not one of NVIDIA's device nodes"
        ))
    })
}

/// nvidia_node SLOT CREATE PROC DEV: the DEV/nvidiaN of the GPU at PCI SLOT (PROC and DEV stand in for
/// /proc/driver/nvidia and /dev in the unit tests). A GPU switched to the nvidia driver at runtime can
/// lack its nodes until something creates them (finding 122): NVIDIA's setuid helper, nvidia-modprobe,
/// is asked once for each missing one, unless CREATE is false. /dev/nvidiactl is minor 255: `-c MINOR`
/// makes only /dev/nvidiaMINOR (finding 254).
pub fn nvidia_node(ctx: &Ctx, slot: &[u8], create: bool, proc: &[u8], dev: &[u8]) -> Res<Vec<u8>> {
    let run = |m: &[u8]| {
        let mut c = crate::sig::command("nvidia-modprobe");
        c.arg("-c")
            .arg(shell::path(m))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let _ = crate::sig::run_status(c);
    };
    let helper: Option<&Helper> =
        crate::exec::executable_on_path(b"nvidia-modprobe").then_some(&run);
    nvidia_node_with(ctx, slot, create, proc, dev, helper)
}

/// `nvidia-modprobe -c MINOR`, as nvidia_node_with is given it.
pub type Helper<'a> = dyn Fn(&[u8]) + 'a;

/// nvidia_node with HELPER as `nvidia-modprobe -c MINOR` (None: not on PATH), so a #[test] can be
/// the helper, never the real one, which is setuid root and makes /dev nodes.
pub fn nvidia_node_with(
    ctx: &Ctx,
    slot: &[u8],
    create: bool,
    proc: &[u8],
    dev: &[u8],
    helper: Option<&Helper>,
) -> Res<Vec<u8>> {
    let info = cat!(proc, "/gpus/", slot, "/information");
    let text = match std::fs::read(shell::path(&info)) {
        Ok(t) if access(&info, libc::R_OK) => t,
        _ => return Err(ctx.die(cat!("NVIDIA GPU ", slot, " has no ", info))),
    };
    let minor = device_minor(&text);
    if minor.is_empty() || !minor.iter().all(u8::is_ascii_digit) {
        return Err(ctx.die(cat!("NVIDIA GPU ", slot, ": no device minor in ", info)));
    }
    let mut miss = nvidia_missing(dev, &minor);
    if let Some(run) = helper.filter(|_| create && !miss.is_empty()) {
        for m in &miss {
            run(m);
        }
        miss = nvidia_missing(dev, &minor);
    }
    if !miss.is_empty() {
        let (mut what, mut run) = (Vec::new(), Vec::new());
        for m in &miss {
            if m == b"255" {
                what.extend_from_slice(&cat!(" ", dev, "/nvidiactl"));
            } else {
                what.extend_from_slice(&cat!(" ", dev, "/nvidia", m));
            }
            run.extend_from_slice(&cat!(" `nvidia-modprobe -c ", m, "`"));
        }
        return Err(ctx.die(cat!(
            "NVIDIA GPU ",
            slot,
            " has no usable",
            what,
            " (run",
            run,
            ", NVIDIA's helper from nvidia-utils, no root needed), then try again"
        )));
    }
    Ok(cat!(dev, "/nvidia", minor))
}

/// `awk -F: '/^Device Minor:/ {gsub(/[[:space:]]/, "", $2); print $2}'`: each such line's second
/// field, white space out, one a line.
fn device_minor(text: &[u8]) -> Vec<u8> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    for l in text.split(|&c| c == b'\n') {
        if !l.starts_with(b"Device Minor:") {
            continue;
        }
        let f = l.split(|&c| c == b':').nth(1).unwrap_or_default();
        out.push(
            f.iter()
                .copied()
                .filter(|c| !c.is_ascii_whitespace())
                .collect(),
        );
    }
    out.join(&b'\n')
}

/// nvidia_missing DEV MINOR: the minors of the nodes nvidia_node needs that DEV lacks, the GPU's and
/// 255 (nvidiactl).
fn nvidia_missing(dev: &[u8], minor: &[u8]) -> Vec<Vec<u8>> {
    use std::os::unix::fs::FileTypeExt;
    let chr = |p: Vec<u8>| {
        std::fs::metadata(shell::path(&p)).is_ok_and(|m| m.file_type().is_char_device())
    };
    let mut v = Vec::new();
    if !chr(cat!(dev, "/nvidia", minor)) {
        v.push(minor.to_vec());
    }
    if !chr(cat!(dev, "/nvidiactl")) {
        v.push(b"255".to_vec());
    }
    v
}

/// nvidia_card RENDER: the name (cardN) of the primary node of RENDER's GPU, the path NVIDIA's EGL
/// gives as its device file (EGL_DRM_DEVICE_FILE_EXT); None when sysfs names none.
pub fn nvidia_card(render: &RenderNode) -> Option<Vec<u8>> {
    nvidia_card_in(render.as_bytes(), SYSDRM.as_bytes())
}

/// nvidia_card RENDER SYS (SYS stands in for /sys/class/drm in the unit tests). The glob's names in
/// byte order (finding 446).
pub fn nvidia_card_in(render: &[u8], sys: &[u8]) -> Option<Vec<u8>> {
    let node = render.rsplit(|&c| c == b'/').next().unwrap_or_default();
    let dir = cat!(sys, "/", node, "/device/drm");
    shell::glob(&shell::path(&dir), false)
        .into_iter()
        .find(|n| shell::re_match("^card[0-9]+$", n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn render_nodes_only() {
        for ok in [&b"/dev/dri/renderD128"[..], b"/dev/dri/renderD0"] {
            assert!(RenderNode::new(ok).is_some(), "{ok:?}");
        }
        for bad in [
            &b"/dev/dri/card0"[..],
            b"/dev/dri/card1",
            b"/dev/input/event3",
            b"/run/seatd.sock",
            b"/dev/dri/renderD",
            b"/dev/dri/renderD12a",
            b"/dev/dri/renderD128/../card0",
            b"/dev/dri/renderD128/",
            b"/dev/dri/renderD128\n",
            b"/dev/dri//renderD128",
            b"dev/dri/renderD128",
            b"/dev/dri/renderD\xd9\xa1",
        ] {
            assert!(RenderNode::new(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn nvidia_nodes_only() {
        for ok in [
            &b"/dev/nvidia0"[..],
            b"/dev/nvidia12",
            b"/dev/nvidiactl",
            b"/dev/nvidia-uvm",
        ] {
            assert!(NvidiaDev::new(ok).is_some(), "{ok:?}");
        }
        for bad in [
            &b"/dev/nvidia"[..],
            b"/dev/nvidia-modeset",
            b"/dev/nvidia-uvm-tools",
            b"/dev/dri/card0",
            b"/dev/nvidia0/../input",
            b"/dev/nvidiactl\n",
        ] {
            assert!(NvidiaDev::new(bad).is_none(), "{bad:?}");
        }
    }

    proptest! {
        // No path either type accepts is the seat's: a card node, an input device, seatd's socket,
        // the system bus, a runtime dir, or one that climbs out with `..`.
        #[test]
        fn no_seat_path_is_a_box_gpu_node(tail in "[a-zA-Z0-9/._-]{0,24}", pre in prop::sample::select(vec![
            "/dev/dri/renderD", "/dev/dri/card", "/dev/input/", "/run/seatd", "/run/dbus/", "/run/user/",
            "/dev/nvidia", "/dev/", "",
        ])) {
            let p = format!("{pre}{tail}");
            let r = RenderNode::new(p.as_bytes()).map(|r| r.0);
            let n = NvidiaDev::new(p.as_bytes()).map(|n| n.0);
            for ok in [r, n].into_iter().flatten() {
                let s = String::from_utf8_lossy(&ok).into_owned();
                prop_assert!(!s.contains("card") && !s.contains("input") && !s.contains("seatd"));
                prop_assert!(!s.contains("..") && !s.starts_with("/run") && !s.contains("dbus"));
                prop_assert!(s.starts_with("/dev/dri/renderD") || s.starts_with("/dev/nvidia"));
            }
        }
    }

    #[test]
    fn card_placeholder_is_an_empty_file() {
        let d = std::env::temp_dir().join(format!("omabox-cp-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let db = shell::bytes(&d).to_vec();
        std::fs::write(d.join("card-placeholder"), b"x").unwrap();
        let c = CardPlaceholder::make(&db, b"card1").unwrap();
        assert_eq!(c.dest(), b"/dev/dri/card1");
        assert_eq!(std::fs::metadata(shell::path(c.src())).unwrap().len(), 0);
        assert!(CardPlaceholder::make(&db, b"renderD128").is_err());
        assert!(CardPlaceholder::make(&db, b"card1/../x").is_err());
        assert!(CardPlaceholder::make(b"/dev/dri", b"card1").is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn device_minor_as_awk() {
        assert_eq!(
            device_minor(b"Model: x\nDevice Minor: \t 1 \nIRQ: 2\n"),
            b"1"
        );
        assert_eq!(device_minor(b"Device Minor:"), b"");
        assert_eq!(device_minor(b"Device Minor: 0\nDevice Minor: 3"), b"0\n3");
        assert_eq!(device_minor(b" Device Minor: 0"), b"");
    }

    #[test]
    fn nvidia_node_and_card_in_a_fake_tree() {
        let t = std::env::temp_dir().join(format!("omabox-nv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let tb = shell::bytes(&t).to_vec();
        std::fs::create_dir_all(t.join("proc/gpus/0000:01:00.0")).unwrap();
        std::fs::create_dir_all(t.join("dev")).unwrap();
        std::fs::write(
            t.join("proc/gpus/0000:01:00.0/information"),
            "Model: X\nDevice Minor: 0\n",
        )
        .unwrap();
        let mut ctx = Ctx::new(b"up", b"/x".to_vec());
        ctx.parsed();
        let (p, d) = (cat!(tb, "/proc"), cat!(tb, "/dev"));
        // (no char devices in a temp dir: both missing, said; nothing created)
        let r = crate::exit::muted(|| nvidia_node(&ctx, b"0000:01:00.0", false, &p, &d));
        assert!(r.is_err());
        assert_eq!(
            nvidia_missing(&d, b"0"),
            vec![b"0".to_vec(), b"255".to_vec()]
        );
        let r = crate::exit::muted(|| nvidia_node(&ctx, b"0000:02:00.0", false, &p, &d));
        assert!(r.is_err());
        std::fs::create_dir_all(t.join("sys/renderD128/device/drm/card1")).unwrap();
        std::fs::create_dir_all(t.join("sys/renderD128/device/drm/renderD128")).unwrap();
        let s = cat!(tb, "/sys");
        assert_eq!(
            nvidia_card_in(b"/dev/dri/renderD128", &s),
            Some(b"card1".to_vec())
        );
        assert_eq!(nvidia_card_in(b"/dev/dri/renderD129", &s), None);
        std::fs::remove_dir_all(&t).unwrap();
    }

    /// t_unit_nvidia: nvidia_node with a stand-in nvidia-modprobe (never the real one: it is setuid
    /// root and makes /dev nodes) that logs its words and makes the node it is asked for, or fails;
    /// the nodes it makes links to /dev/null, a character device.
    #[test]
    fn nvidia_node_asks_the_helper_as_bash() {
        use std::cell::RefCell;
        let t = std::env::temp_dir().join(format!("omabox-nvh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let tb = shell::bytes(&t).to_vec();
        let slot = b"0000:01:00.0";
        std::fs::create_dir_all(t.join("proc/gpus/0000:01:00.0")).unwrap();
        std::fs::create_dir_all(t.join("dev")).unwrap();
        let info = t.join("proc/gpus/0000:01:00.0/information");
        std::fs::write(
            &info,
            "Model: \t\t NVIDIA GeForce RTX 5070 Ti\nIRQ:   \t\t 180\nDevice Minor: \t 3\n",
        )
        .unwrap();
        let ctl = t.join("dev/nvidiactl");
        let gpu = t.join("dev/nvidia3");
        std::os::unix::fs::symlink("/dev/null", &ctl).unwrap();
        let (p, d) = (cat!(tb, "/proc"), cat!(tb, "/dev"));
        let mut ctx = Ctx::new(b"up", b"/x".to_vec());
        ctx.parsed();
        let calls: RefCell<Vec<String>> = RefCell::default();
        let make = RefCell::new(false);
        let helper = |m: &[u8]| {
            calls
                .borrow_mut()
                .push(format!("-c {}", String::from_utf8_lossy(m)));
            if *make.borrow() {
                let n = if m == b"255" {
                    "nvidiactl".to_string()
                } else {
                    format!("nvidia{}", String::from_utf8_lossy(m))
                };
                let _ = std::os::unix::fs::symlink("/dev/null", t.join("dev").join(n));
            }
        };
        let nv = |create: bool| {
            crate::exit::heard(|| nvidia_node_with(&ctx, slot, create, &p, &d, Some(&helper)))
        };
        let take = || std::mem::take(&mut *calls.borrow_mut()).join(" ");
        // a missing node the helper does not create: what to run, after asking it once
        let (r, said) = nv(true);
        assert!(r.is_err());
        assert!(
            said.contains(&format!(
                "no usable {tb}/dev/nvidia3 (run `nvidia-modprobe -c 3`,",
                tb = t.display()
            )),
            "{said}"
        );
        assert_eq!(take(), "-c 3");
        // CREATE false (an interactive box's other GPUs): not asked
        let (r, said) = nv(false);
        assert!(r.is_err() && said.contains("no usable"), "{said}");
        assert_eq!(take(), "");
        // a missing node the helper creates; then one there: used, not asked
        *make.borrow_mut() = true;
        assert_eq!(nv(true).0.unwrap(), cat!(tb, "/dev/nvidia3"));
        assert_eq!(take(), "-c 3");
        assert_eq!(nv(true).0.unwrap(), cat!(tb, "/dev/nvidia3"));
        assert_eq!(take(), "");
        // a missing control node asks too, for minor 255, and says what makes it
        std::fs::remove_file(&ctl).unwrap();
        *make.borrow_mut() = false;
        let (r, said) = nv(true);
        assert!(r.is_err());
        assert!(
            said.contains(&format!(
                "no usable {}/dev/nvidiactl (run `nvidia-modprobe -c 255`,",
                t.display()
            )),
            "{said}"
        );
        assert_eq!(take(), "-c 255");
        *make.borrow_mut() = true;
        assert_eq!(nv(true).0.unwrap(), cat!(tb, "/dev/nvidia3"));
        assert_eq!(take(), "-c 255");
        // both missing: both named, both asked for, one call each
        std::fs::remove_file(&ctl).unwrap();
        std::fs::remove_file(&gpu).unwrap();
        *make.borrow_mut() = false;
        let (r, said) = nv(true);
        assert!(r.is_err());
        let want = format!(
            "no usable {d}/dev/nvidia3 {d}/dev/nvidiactl (run `nvidia-modprobe -c 3` `nvidia-modprobe -c 255`,",
            d = t.display()
        );
        assert!(said.contains(&want), "{said}");
        assert_eq!(take(), "-c 3 -c 255");
        // no helper on PATH: not asked, the same words
        let (r, said) = crate::exit::heard(|| nvidia_node_with(&ctx, slot, true, &p, &d, None));
        assert!(r.is_err() && said.contains("no usable"), "{said}");
        assert_eq!(take(), "");
        // no device minor in the driver's file; a GPU the driver does not list
        std::fs::write(&info, "Model: x\n").unwrap();
        let (r, said) = nv(true);
        assert!(r.is_err() && said.contains("no device minor"), "{said}");
        let (r, said) = crate::exit::heard(|| {
            nvidia_node_with(&ctx, b"0000:02:00.0", true, &p, &d, Some(&helper))
        });
        assert!(r.is_err());
        assert!(
            said.contains(&format!(
                "has no {}/proc/gpus/0000:02:00.0/information",
                t.display()
            )),
            "{said}"
        );
        // nvidia_card (finding 294): the card, not its control or render entries; none; a node sysfs lacks
        std::fs::create_dir_all(t.join("drm/renderD129/device/drm/card0")).unwrap();
        std::fs::create_dir_all(t.join("drm/renderD129/device/drm/renderD129")).unwrap();
        std::fs::create_dir_all(t.join("drm/renderD130/device/drm/renderD130")).unwrap();
        std::os::unix::fs::symlink("card0", t.join("drm/renderD129/device/drm/controlD64"))
            .unwrap();
        let s = cat!(tb, "/drm");
        assert_eq!(
            nvidia_card_in(b"/dev/dri/renderD129", &s),
            Some(b"card0".to_vec())
        );
        assert_eq!(nvidia_card_in(b"/dev/dri/renderD130", &s), None);
        assert_eq!(nvidia_card_in(b"/dev/dri/renderD131", &s), None);
        std::fs::remove_dir_all(&t).unwrap();
    }
}
