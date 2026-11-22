//! `omabox env`: exports for host-side Wayland tools.

use crate::args::name_arg;
use crate::boxes::{box_env, need_box};
use crate::cat;
use crate::exit::{Ctx, Res, out};
use crate::select::select_box;
use crate::shell::{self, quote_q};
use std::os::unix::fs::FileTypeExt;

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    // (name_arg says why; its words are still being read: a usage error.)
    let name = name_arg(ctx, "env", args).map_err(|_| ctx.refused())?;
    select_box(ctx, &name)?;
    need_box(ctx)?;
    // The box bus is private and its Hyprland socket path too long to reach directly: for those use
    // `omabox run` and `omabox hyprctl`. Plain Wayland clients (wl-paste, wlr-randr, ...) work with
    // this.
    let Some(wd) = box_env(&ctx.d, "WAYLAND_DISPLAY") else {
        return Err(ctx.die(cat!("box '", ctx.name, "' has no WAYLAND_DISPLAY")));
    };
    let path = match socket_link(&ctx.d, &wd) {
        Ok(p) => p,
        Err(why) => {
            return Err(ctx.die(cat!(
                "env: box '",
                ctx.name,
                "': its Wayland socket (run/",
                wd,
                ") ",
                why,
                ": refused (a host tool pointed there could reach another display)"
            )));
        }
    };
    out(cat!(
        "export WAYLAND_DISPLAY=",
        quote_q(&path),
        "\nunset DISPLAY WAYLAND_SOCKET HYPRLAND_INSTANCE_SIGNATURE\n"
    ))
}

/// The box's Wayland socket for host tools, as a hard link in the box dir, which only the host
/// writes (finding 493). Its runtime dir is the box's to write: a link put there in the socket's
/// place, resolved on the host, would lead a host tool to the real desktop's compositor (bash
/// printed that path). A hard link names the socket's inode itself, which only a process in the box
/// can have bound; link(2) never follows a link, and what it made is checked again, so a swap
/// between the check and the link is caught. peek hands its window this path too (finding 751).
pub fn socket_link(d: &[u8], wd: &[u8]) -> Result<Vec<u8>, &'static str> {
    let is_socket = |p: &[u8]| {
        std::fs::symlink_metadata(shell::path(p)).is_ok_and(|m| m.file_type().is_socket())
    };
    let src = cat!(d, "/run/", wd);
    if !is_socket(&src) {
        return Err("is not a socket");
    }
    let dst = cat!(d, "/wayland");
    let tmp = cat!(d, "/.wayland.", std::process::id().to_string());
    let _ = std::fs::remove_file(shell::path(&tmp));
    if std::fs::hard_link(shell::path(&src), shell::path(&tmp)).is_err() {
        return Err("cannot be linked");
    }
    if !is_socket(&tmp) {
        let _ = std::fs::remove_file(shell::path(&tmp));
        return Err("is not a socket");
    }
    let moved = std::fs::rename(shell::path(&tmp), shell::path(&dst)).is_ok();
    // (rename between two links to one inode does nothing and succeeds: the link of an earlier env
    // to this same socket leaves TMP where it was)
    let _ = std::fs::remove_file(shell::path(&tmp));
    if !moved {
        return Err("cannot be linked");
    }
    Ok(dst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::net::UnixListener;

    #[test]
    fn a_link_in_the_sockets_place_is_refused() {
        let d = std::env::temp_dir().join(format!("omabox-env-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("run")).expect("dirs");
        let db = shell::bytes(&d).to_vec();
        let _host = UnixListener::bind(d.join("host.sock")).expect("a socket");
        std::os::unix::fs::symlink(d.join("host.sock"), d.join("run/wayland-1")).expect("a link");
        assert_eq!(socket_link(&db, b"wayland-1"), Err("is not a socket"));
        assert!(!d.join("wayland").exists());
        std::fs::remove_file(d.join("run/wayland-1")).expect("rm");
        let _own = UnixListener::bind(d.join("run/wayland-1")).expect("a socket");
        assert_eq!(socket_link(&db, b"wayland-1"), Ok(cat!(&db, "/wayland")));
        let ino = |p: &str| std::fs::metadata(d.join(p)).expect(p).ino();
        assert_eq!(ino("run/wayland-1"), ino("wayland"));
        // Again, the link there already: the same, and nothing else left in the dir.
        assert_eq!(socket_link(&db, b"wayland-1"), Ok(cat!(&db, "/wayland")));
        let mut names: Vec<String> = std::fs::read_dir(&d)
            .expect("the dir")
            .map(|e| {
                e.expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        assert_eq!(names, ["host.sock", "run", "wayland"]);
        let _ = std::fs::remove_dir_all(&d);
    }
    /// peek's case (finding 751): the host's peek connects some time after the check. It is handed
    /// the link in the box dir, so a box that swaps its socket for a link to the host's own socket
    /// after the check leaves the path peek has on the box's socket.
    #[test]
    fn a_swap_after_the_check_leaves_the_link_on_the_box() {
        use std::os::unix::net::UnixStream;
        let d = std::env::temp_dir().join(format!("omabox-envswap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("run")).expect("dirs");
        let db = shell::bytes(&d).to_vec();
        let host = UnixListener::bind(d.join("host.sock")).expect("a socket");
        let own = UnixListener::bind(d.join("run/wayland-1")).expect("a socket");
        let p = socket_link(&db, b"wayland-1").expect("the link");
        std::fs::remove_file(d.join("run/wayland-1")).expect("rm");
        std::os::unix::fs::symlink(d.join("host.sock"), d.join("run/wayland-1")).expect("a link");
        host.set_nonblocking(true).expect("nonblocking");
        own.set_nonblocking(true).expect("nonblocking");
        let _c = UnixStream::connect(shell::path(&p)).expect("connects");
        assert!(own.accept().is_ok(), "the box's socket gets the connection");
        assert!(host.accept().is_err(), "the host's never does");
        let _ = std::fs::remove_dir_all(&d);
    }
}
