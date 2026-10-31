//! A box's logs (issue #41, finding 107): which there are and where. Most are in the box HOME and its
//! runtime dir, which the box can write: a live box's are read inside its mount namespace (on_box),
//! where a path resolves as the box sees it; a dead box's from the host, only as regular files inside
//! its own dirs. box.log and reap.log are the box dir's, which the box never sees.

use crate::boxes::box_env;
use crate::cat;
use crate::exit::Ctx;
use crate::procfs;
use crate::repo::readlink_e;
use crate::shell;

/// A box's logs (issue #41, finding 107), by name.
pub const LOG_NAMES: [&str; 14] = [
    "hyprland", "shell", "apps", "run", "events", "keyring", "labwc", "systemd", "audio", "polkit",
    "gdb", "journal", "box", "reap",
];

/// Where log NAME of the selected box is, as the box sees it (`box PATH`) or the host does (`host
/// PATH`, the box dir's own); None when the box has none.
pub fn log_where(ctx: &Ctx, s: &str) -> Option<Vec<u8>> {
    match s {
        "hyprland" => {
            let sig = box_env(&ctx.d, "HYPRLAND_INSTANCE_SIGNATURE")?;
            Some(cat!(
                format!("/run/user/{}/hypr/", procfs::uid()),
                sig,
                "/hyprland.log"
            ))
        }
        // The latest `run -d` (their names are run-<ns since 1970>.log: in order as text).
        "run" => {
            let home = ctx.at("home");
            std::fs::read_dir(&home).ok()?;
            let mut runs: Vec<Vec<u8>> = shell::glob(&home, false)
                .into_iter()
                .filter(|n| {
                    n.strip_prefix(b"run-")
                        .and_then(|r| r.strip_suffix(b".log"))
                        .is_some_and(|d| !d.is_empty() && d.iter().all(u8::is_ascii_digit))
                })
                .collect();
            runs.sort();
            let r = runs.pop()?;
            Some(cat!("/home/sbx/", r))
        }
        "box" | "reap" => Some(cat!(ctx.d, "/", s, ".log")),
        _ => Some(cat!("/home/sbx/", s, ".log")),
    }
}

/// A path as the box sees it, from the host (the box dir's HOME and runtime dir).
pub fn log_host_path(ctx: &Ctx, p: &[u8]) -> Vec<u8> {
    let run = format!("/run/user/{}/", procfs::uid());
    if let Some(r) = p.strip_prefix(b"/home/sbx/") {
        cat!(ctx.d, "/home/", r)
    } else if let Some(r) = p.strip_prefix(run.as_bytes()) {
        cat!(ctx.d, "/run/", r)
    } else {
        p.to_vec()
    }
}

/// log_dead_path: a dead box's log on the host, when it is a regular file inside the box's own dirs.
pub fn log_dead_path(ctx: &Ctx, p: &[u8]) -> Option<Vec<u8>> {
    let r = readlink_e(&log_host_path(ctx, p))?;
    if !shell::path(&r).is_file() {
        return None;
    }
    // (`"$(readlink -f DIR)"/*`: a dir that cannot be resolved leaves "/*")
    let inside = |dir: &str| {
        let d = crate::repo::readlink_f(&cat!(ctx.d, "/", dir)).unwrap_or_default();
        r.starts_with(&cat!(d, "/"))
    };
    (inside("run") || inside("home")).then_some(r)
}
