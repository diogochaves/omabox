//! A box's logs (issue #41, finding 107): which there are and where. Most are in the box HOME and its
//! runtime dir, which the box can write: a live box's are read inside its mount namespace (on_box),
//! where a path resolves as the box sees it; a dead box's from the host, only as regular files inside
//! its own dirs. box.log and reap.log are the box dir's, which the box never sees. The box HOME is
//! box_home's: /home/sbx, or in a desk box your HOME's path (#321), where the host has your real
//! files: a path there is only ever read on the host through the box's own folder.

use crate::boxes::{box_env, box_home};
use crate::cat;
use crate::exit::Ctx;
use crate::procfs;
use crate::repo::readlink_e;
use crate::shell;

/// A box's logs (issue #41, finding 107), by name.
pub const LOG_NAMES: [&str; 16] = [
    "hyprland",
    "shell",
    "apps",
    // (a desk box's autostart policy, #321: ~/autostart.log)
    "autostart",
    "run",
    "events",
    "keyring",
    "labwc",
    "systemd",
    "audio",
    "polkit",
    "gdb",
    "journal",
    "notifications",
    "box",
    "reap",
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
            Some(cat!(box_home(&ctx.d), "/", r))
        }
        "box" | "reap" => Some(cat!(ctx.d, "/", s, ".log")),
        _ => Some(cat!(box_home(&ctx.d), "/", s, ".log")),
    }
}

/// A log path log_where gave (or a run -d job's, in the box HOME), from the host: one in the box
/// HOME or runtime dir as the box dir's own; box.log and reap.log as they are (the box dir's
/// already). Anything else is no path of the box dir's: empty, never the path itself, which on the
/// host is another file (a desk box's HOME is at your HOME's path, #321: there it is yours).
pub fn log_host_path(ctx: &Ctx, p: &[u8]) -> Vec<u8> {
    log_host_path_in(&ctx.d, &box_home(&ctx.d), procfs::uid(), p)
}

/// log_host_path for box folder D, its HOME HOME in the box, the user UID.
pub fn log_host_path_in(d: &[u8], home: &[u8], uid: u32, p: &[u8]) -> Vec<u8> {
    if ["box", "reap"].iter().any(|s| p == cat!(d, "/", s, ".log")) {
        return p.to_vec();
    }
    let run = format!("/run/user/{uid}/");
    if let Some(r) = p.strip_prefix(cat!(home, "/").as_slice()) {
        cat!(d, "/home/", r)
    } else if let Some(r) = p.strip_prefix(run.as_bytes()) {
        cat!(d, "/run/", r)
    } else {
        Vec::new()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell;

    /// A desk box (#321) in a scratch dir: HOME H (a real folder here, standing in for yours, with
    /// files of "yours"), its box folder under it as ~/.cache/omabox/N is, box.json's home H.
    fn desk_box(tag: &str) -> (std::path::PathBuf, Vec<u8>, Ctx) {
        let h = std::env::temp_dir().join(format!("omabox-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&h);
        let d = h.join(".cache/omabox/3");
        for x in ["home", "run/hypr/sig"] {
            std::fs::create_dir_all(d.join(x)).unwrap();
        }
        let hb = shell::bytes(&h).to_vec();
        let j = serde_json::json!({"desktop": "desk", "home": String::from_utf8_lossy(&hb)});
        std::fs::write(d.join("box.json"), j.to_string()).unwrap();
        let mut ctx = Ctx::new(b"log", Vec::new());
        ctx.d = shell::bytes(&d).to_vec();
        (h, hb, ctx)
    }

    // #321, S7: a box path under a desk box's HOME is read on the host only through the box's own
    // folder, never at that path, which on the host is the user's real HOME.
    #[test]
    fn log_host_path_never_real_home() {
        let (h, hb, ctx) = desk_box("lhp");
        let d = ctx.d.clone();
        // "Yours" at the box paths, the box's own in its folder.
        for f in ["events.log", "gdb.log", "run-5.log", "secret.log"] {
            std::fs::write(h.join(f), "yours\n").unwrap();
        }
        std::fs::write(h.join(".cache/omabox/3/home/events.log"), "box's\n").unwrap();
        std::fs::write(h.join(".cache/omabox/3/home/run-7.log"), "box's\n").unwrap();
        std::fs::write(h.join(".cache/omabox/3/box.log"), "box dir's\n").unwrap();
        let ev = log_where(&ctx, "events").unwrap();
        assert_eq!(
            ev,
            cat!(hb, "/events.log"),
            "as the box sees it: its HOME's path"
        );
        assert_eq!(log_host_path(&ctx, &ev), cat!(d, "/home/events.log"));
        let dead = log_dead_path(&ctx, &ev).unwrap();
        assert_eq!(std::fs::read(shell::path(&dead)).unwrap(), b"box's\n");
        // gdb.log: the box has none; yours is never taken for it.
        let g = log_where(&ctx, "gdb").unwrap();
        assert_eq!(log_host_path(&ctx, &g), cat!(d, "/home/gdb.log"));
        assert_eq!(log_dead_path(&ctx, &g), None);
        // The latest run -d log is the box's (run-7), not your run-5.
        assert_eq!(log_where(&ctx, "run").unwrap(), cat!(hb, "/run-7.log"));
        // box.log is the box folder's, a host path already, though it is under that HOME.
        let b = log_where(&ctx, "box").unwrap();
        assert_eq!(log_host_path(&ctx, &b), b);
        assert_eq!(
            log_host_path(&ctx, &cat!(d, "/reap.log")),
            cat!(d, "/reap.log")
        );
        // Anything else of the box dir's is still mapped as the box sees it (its HOME's .cache)...
        assert_eq!(
            log_host_path(&ctx, &cat!(d, "/home/x")),
            cat!(d, "/home/.cache/omabox/3/home/x")
        );
        // ...and a path that is neither the box's HOME nor its runtime dir is no path at all.
        for p in [&b"/etc/passwd"[..], b"", &hb, b"relative.log"] {
            assert_eq!(
                log_host_path(&ctx, p),
                b"",
                "{}",
                String::from_utf8_lossy(p)
            );
            assert_eq!(log_dead_path(&ctx, p), None);
        }
        // A link in the box's HOME to yours is not followed out of its folder.
        std::os::unix::fs::symlink(h.join("secret.log"), d_path(&d).join("home/link.log")).unwrap();
        assert_eq!(log_dead_path(&ctx, &cat!(hb, "/link.log")), None);
        // The runtime dir as before.
        let uid = procfs::uid();
        assert_eq!(
            log_host_path(
                &ctx,
                format!("/run/user/{uid}/hypr/sig/hyprland.log").as_bytes()
            ),
            cat!(d, "/run/hypr/sig/hyprland.log")
        );
        // A box of before #321 (no `home`): /home/sbx, and a desk HOME's path is nothing there.
        assert_eq!(
            log_host_path_in(&d, b"/home/sbx", 1000, b"/home/sbx/events.log"),
            cat!(d, "/home/events.log")
        );
        assert_eq!(
            log_host_path_in(&d, b"/home/sbx", 1000, &cat!(hb, "/events.log")),
            b""
        );
        let _ = std::fs::remove_dir_all(&h);
    }

    fn d_path(d: &[u8]) -> std::path::PathBuf {
        shell::path(d).to_path_buf()
    }

    // #321: every box-side path omabox builds is under the box's own HOME.
    #[test]
    fn desk_box_paths() {
        let (h, hb, ctx) = desk_box("dbp");
        assert_eq!(crate::desk::events_log(&ctx.d), cat!(hb, "/events.log"));
        assert_eq!(
            crate::cmd::record::rec_dir(&ctx.d).into_bytes(),
            cat!(hb, "/.cache/omabox-record")
        );
        assert_eq!(log_where(&ctx, "apps").unwrap(), cat!(hb, "/apps.log"));
        assert_eq!(
            log_where(&ctx, "hyprland"),
            None,
            "no Hyprland signature yet"
        );
        // Not desk: /home/sbx, as before.
        std::fs::write(
            shell::path(&cat!(ctx.d, "/box.json")),
            r#"{"desktop":"omarchy"}"#,
        )
        .unwrap();
        assert_eq!(crate::desk::events_log(&ctx.d), b"/home/sbx/events.log");
        assert_eq!(
            crate::cmd::record::rec_dir(&ctx.d),
            "/home/sbx/.cache/omabox-record"
        );
        assert_eq!(log_where(&ctx, "apps").unwrap(), b"/home/sbx/apps.log");
        let _ = std::fs::remove_dir_all(&h);
    }
}
