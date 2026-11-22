//! A box's GPU as bin/omabox reads it (#118, findings 237, 290, 295): a render node's driver and PCI
//! slot (from /sys/class/drm), the kind a driver counts as, the `gpu` setting's form of a GPU, and
//! what `gpu` and `cpu` say about the box before their figures (cost_context, cost_lines).

use crate::boxmeta::BoxMeta;
use crate::cat;
use crate::exit::{Ctx, Exit, Res, out};
use crate::jq::{self, Args};
use crate::shell;

/// `basename "$(readlink -f "$SYSDRM/${N##*/}/device/driver")"`.
pub fn render_driver(n: &[u8]) -> Vec<u8> {
    render_driver_in(n, b"/")
}

/// render_driver with SYSDRM under ROOT (a stand-in tree's root; "/" the system's own).
pub fn render_driver_in(n: &[u8], root: &[u8]) -> Vec<u8> {
    sys_base(n, "/device/driver", root)
}

/// `basename "$(readlink -f "$SYSDRM/${N##*/}/device")"`: the PCI slot of a render node's GPU.
pub fn render_slot(n: &[u8]) -> Vec<u8> {
    render_slot_in(n, b"/")
}

/// render_slot with SYSDRM under ROOT.
pub fn render_slot_in(n: &[u8], root: &[u8]) -> Vec<u8> {
    sys_base(n, "/device", root)
}

fn sys_base(n: &[u8], rel: &str, root: &[u8]) -> Vec<u8> {
    let node = n.rsplit(|&c| c == b'/').next().unwrap_or_default();
    crate::repo::readlink_f(&at(root, &cat!(crate::render::SYSDRM, "/", node, rel)))
        .map(|p| shell::basename(&p))
        .unwrap_or_default()
}

/// P (an absolute path) under ROOT: a stand-in tree's (the unit tests'), or P itself for "/", the
/// system's own, which is what everything but a test gives.
pub fn at(root: &[u8], p: &[u8]) -> Vec<u8> {
    let r = root.strip_suffix(b"/").unwrap_or(root);
    if r.is_empty() { p.to_vec() } else { cat!(r, p) }
}

/// gpu_kind DRIVER: nvidia, amd, intel, or nothing.
pub fn gpu_kind(d: &[u8]) -> &'static [u8] {
    match d {
        b"nvidia" | b"nouveau" => b"nvidia",
        b"amdgpu" | b"radeon" => b"amd",
        b"i915" | b"xe" => b"intel",
        _ => b"",
    }
}

/// gpu_is NODE WANT: render node NODE is the GPU WANT names (a kind, by its driver, or a slot).
pub fn gpu_is(n: &[u8], want: &[u8]) -> bool {
    gpu_is_in(n, want, b"/")
}

/// gpu_is with SYSDRM under ROOT.
pub fn gpu_is_in(n: &[u8], want: &[u8], root: &[u8]) -> bool {
    match want {
        b"nvidia" => matches!(render_driver_in(n, root).as_slice(), b"nvidia" | b"nouveau"),
        b"amd" => matches!(render_driver_in(n, root).as_slice(), b"amdgpu" | b"radeon"),
        b"intel" => matches!(render_driver_in(n, root).as_slice(), b"i915" | b"xe"),
        _ => render_slot_in(n, root) == want,
    }
}

/// `conf_check gpu V`: a kind of GPU (by its driver) or a PCI slot (03:00.0 taken too), in its
/// normal form; None for anything else.
pub fn conf_gpu(v: &[u8]) -> Option<Vec<u8>> {
    let g = shell::lower(v);
    match g.as_slice() {
        b"auto" | b"nvidia" | b"amd" | b"intel" => Some(g),
        b"nouveau" | b"amdgpu" | b"radeon" | b"i915" | b"xe" => Some(gpu_kind(&g).to_vec()),
        _ => {
            if !shell::re_match(r"^([0-9a-f]{4}:)?[0-9a-f]{2}:[0-9a-f]{2}\.[0-7]$", &g) {
                return None;
            }
            // (`[[ $g == ????:* ]]`: four characters, then a colon)
            let four = shell::head_chars(&g, 4).len();
            if g.get(four) == Some(&b':') && shell::char_count(&g[..four]) == 4 {
                Some(g)
            } else {
                Some(cat!("0000:", g))
            }
        }
    }
}

/// What `gpu` and `cpu` say about the box before their figures: its main screen, the aquamarine it
/// runs, its GPU and its apps' renderer, as box.json keeps them (`null` when it has none).
pub struct Cost {
    pub screen: Vec<u8>,
    pub aq: Vec<u8>,
    pub rj: Vec<u8>,
    pub cr: Vec<u8>,
}

/// cost_context. box_screen's end ends the command.
pub fn cost_context(ctx: &Ctx) -> Res<Cost> {
    let screen = crate::screen::box_screen(ctx)?;
    let f = |prog: &str| -> Vec<u8> {
        let r = Args::new(&["-c", prog])
            .arg(cat!(ctx.d, "/box.json"))
            .run_file(jq::Err::Null);
        if r.ok() { r.text() } else { b"null".to_vec() }
    };
    Ok(Cost {
        screen,
        aq: f(".aquamarine // null"),
        rj: f(".render // null"),
        cr: f(".client_render // null"),
    })
}

/// cost_lines: the aquamarine, the GPU, and the apps' renderer when it is software (#204), under
/// set -e: a jq that fails on the aquamarine's record ends the command.
pub fn cost_lines(ctx: &Ctx, c: &Cost) -> Res<()> {
    if c.aq != b"null" {
        let r = jq::jq(
            &["-r", crate::jqprog::COST_AQ_JQ],
            Some(&c.aq),
            jq::Err::Inherit,
        );
        out(&r.out)?;
        if !r.ok() {
            return Err(Exit(u8::try_from(r.rc & 0xff).unwrap_or(1)));
        }
    }
    if c.rj != b"null" {
        let r = jq::jq(
            &["-r", crate::jqprog::RENDER_LINE_JQ],
            Some(&c.rj),
            jq::Err::Inherit,
        );
        out(&r.out)?;
    } else if BoxMeta::load(&ctx.d).interactive() {
        out("GPU: the desktop's (an interactive box renders where your compositor does)\n")?;
    }
    if !c.cr.is_empty() && c.cr != b"null" {
        let r = jq::jq(
            &["-r", crate::jqprog::COST_APPS_JQ],
            Some(&c.cr),
            jq::Err::Inherit,
        );
        out(&r.out)?;
    }
    Ok(())
}

/// `ps -o ppid=`-free `tr -s ' \t' _ < /proc/PID/comm` as `$(...)` keeps it: blanks to _, runs of _
/// one.
pub fn comm_word(pid: &[u8]) -> Vec<u8> {
    let c = std::fs::read(shell::path(&cat!("/proc/", pid, "/comm"))).unwrap_or_default();
    let mut o: Vec<u8> = Vec::with_capacity(c.len());
    for b in c {
        let b = if b == b' ' || b == b'\t' { b'_' } else { b };
        if b == b'_' && o.last() == Some(&b'_') {
            continue;
        }
        o.push(b);
    }
    shell::strip_nl(&o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpus_as_the_setting_takes_them() {
        assert_eq!(conf_gpu(b"NVIDIA"), Some(b"nvidia".to_vec()));
        assert_eq!(conf_gpu(b"amdgpu"), Some(b"amd".to_vec()));
        assert_eq!(conf_gpu(b"03:00.0"), Some(b"0000:03:00.0".to_vec()));
        assert_eq!(conf_gpu(b"0000:03:00.0"), Some(b"0000:03:00.0".to_vec()));
        assert_eq!(conf_gpu(b"03:00.8"), None);
        assert_eq!(conf_gpu(b"x"), None);
        // (t_unit_gpu: junk refused; a driver's name, as the list prints it, is its kind: finding 237)
        assert_eq!(conf_gpu(b"geforce"), None);
        for (d, k) in [
            (&b"nouveau"[..], &b"nvidia"[..]),
            (b"amdgpu", b"amd"),
            (b"Radeon", b"amd"),
            (b"i915", b"intel"),
            (b"xe", b"intel"),
        ] {
            assert_eq!(conf_gpu(d).as_deref(), Some(k), "{d:?}");
        }
        assert_eq!(at(b"/", b"/dev/dri"), b"/dev/dri");
        assert_eq!(at(b"/t/r/", b"/dev/dri"), b"/t/r/dev/dri");
        assert_eq!(at(b"/t/r", b"/sys"), b"/t/r/sys");
        assert_eq!(gpu_kind(b"xe"), b"intel");
        assert_eq!(gpu_kind(b"vfio-pci"), b"");
    }
}
