//! A mount into a box that is up (#345, finding 594): `omabox plugin add`'s read-only bind of a
//! plugin's folder, and `plugin remove`'s unmount. bwrap made the box's mounts in the user namespace
//! that owns its mount namespace (bwrap's first one: the box's PID 1 runs in a second one inside it,
//! for --uid), and that namespace is ours, so a child of ours that joins it has every capability
//! there and none outside. The child makes a mount namespace of its own there (a copy of the host's),
//! clones the folder in it as a detached tree (open_tree), makes the tree read-only, nosuid and nodev,
//! as bwrap's --ro-bind is (S4), then joins the box's mount namespace through a pidfd of its PID 1 and
//! attaches the tree (move_mount). The box's processes have no capability over that namespace: they
//! can neither unmount it nor make it writable, as with up's mounts. The box HOME is the box's to
//! write, so where the tree goes is opened with no link followed and no mount crossed (openat2): a
//! link there never takes the mount elsewhere in the box, and nothing is mounted over a mount.
//!
//! Everything run in the child is a plain syscall on values made before the fork (the parent may
//! have threads); it says which step failed, and its errno, through a pipe.

use crate::boxes::{BoxPid, box_pid};
use crate::cat;
use crate::enter::errno_text;
use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

/// A box whose namespaces a mount goes into: a pidfd of its PID 1 and the user namespace that owns
/// its mount namespace.
pub struct BoxNs {
    pidfd: OwnedFd,
    userns: OwnedFd,
}

fn errno() -> i32 {
    // SAFETY: this thread's errno.
    unsafe { *libc::__errno_location() }
}

fn owned(fd: libc::c_long) -> Option<OwnedFd> {
    let fd = RawFd::try_from(fd).ok().filter(|f| *f >= 0)?;
    // SAFETY: a new fd of ours, just returned by the kernel.
    Some(unsafe { OwnedFd::from_raw_fd(fd) })
}

impl BoxNs {
    /// The box in D, pinned: its PID 1 (box_pid) by a pidfd, and its mount namespace's owner read
    /// while that pidfd shows it alive (a pid reused meanwhile is never entered).
    pub fn open(d: &[u8]) -> Result<BoxNs, Vec<u8>> {
        let gone = || b"the box is not up".to_vec();
        let BoxPid::Pid(pid) = box_pid(d) else {
            return Err(gone());
        };
        let n: libc::pid_t = std::str::from_utf8(&pid)
            .ok()
            .and_then(|p| p.parse().ok())
            .ok_or_else(gone)?;
        // SAFETY: pidfd_open takes a pid and flags; the result is a new fd (close-on-exec) or -1.
        let pidfd = owned(unsafe { libc::syscall(libc::SYS_pidfd_open, n, 0) }).ok_or_else(gone)?;
        let mnt = std::fs::File::open(format!("/proc/{n}/ns/mnt")).map_err(|e| {
            cat!(
                "cannot read the box's mount namespace: ",
                errno_text(e.raw_os_error().unwrap_or(0))
            )
        })?;
        // SAFETY: NS_GET_USERNS on a namespace fd of ours returns a new fd (close-on-exec) or -1.
        let userns = owned(libc::c_long::from(unsafe {
            libc::ioctl(mnt.as_raw_fd(), libc::NS_GET_USERNS)
        }))
        .ok_or_else(|| {
            cat!(
                "cannot read who owns the box's mount namespace: ",
                errno_text(errno())
            )
        })?;
        // (alive now, so the /proc entry read above was its)
        // SAFETY: signal 0 through a pidfd of ours: only whether the process is there.
        let alive = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                pidfd.as_raw_fd(),
                0,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        } == 0;
        if !alive || !matches!(box_pid(d), BoxPid::Pid(p) if p == pid) {
            return Err(gone());
        }
        Ok(BoxNs { pidfd, userns })
    }

    /// SRC (a folder, its real path) read-only at PARENT/NAME in the box, an empty folder there that
    /// is no mount yet. Err: the words of the step that failed.
    pub fn mount_ro(&self, src: &[u8], parent: &[u8], name: &[u8]) -> Result<(), Vec<u8>> {
        self.run(Some(src), parent, name)
    }

    /// The mount at PARENT/NAME in the box gone (detached: a process of the box still in it keeps
    /// its view until it leaves). Err: the words of the step that failed.
    pub fn umount(&self, parent: &[u8], name: &[u8]) -> Result<(), Vec<u8>> {
        self.run(None, parent, name)
    }

    fn run(&self, src: Option<&[u8]>, parent: &[u8], name: &[u8]) -> Result<(), Vec<u8>> {
        let c = |b: &[u8]| CString::new(b).map_err(|_| cat!(b, ": a path with a NUL byte"));
        let src_c = src.map(c).transpose()?;
        let parent_c = c(parent)?;
        let name_c = c(name)?;
        let mut fds = [-1; 2];
        // SAFETY: pipe2 fills two fds of ours.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
            return Err(cat!("cannot make a pipe: ", errno_text(errno())));
        }
        // SAFETY: just made, ours alone.
        let (r, w) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        let job = Job {
            userns: self.userns.as_raw_fd(),
            pidfd: self.pidfd.as_raw_fd(),
            src: src_c.as_ref().map(|s| s.as_ptr()),
            parent: parent_c.as_ptr(),
            name: name_c.as_ptr(),
            report: w.as_raw_fd(),
        };
        // SAFETY: the child runs only plain syscalls on what Job holds, then _exit.
        let child = unsafe { libc::fork() };
        if child < 0 {
            return Err(cat!("cannot fork: ", errno_text(errno())));
        }
        if child == 0 {
            // SAFETY: in the child, single-threaded: see Job::run.
            unsafe { job.run() }
        }
        drop(w);
        let mut st = 0;
        // SAFETY: our child, waited on once.
        while unsafe { libc::waitpid(child, &mut st, 0) } < 0 && errno() == libc::EINTR {}
        let mut b = [0u8; 5];
        let got = std::io::Read::read(&mut std::fs::File::from(r), &mut b).unwrap_or(0);
        if libc::WIFEXITED(st) && libc::WEXITSTATUS(st) == 0 {
            return Ok(());
        }
        if got != 5 {
            return Err(b"the mount's child ended without a word".to_vec());
        }
        let e = i32::from_le_bytes([b[1], b[2], b[3], b[4]]);
        Err(step_words(b[0], e, src, parent, name))
    }
}

/// What failed at STEP (Job::run's numbers), with ERRNO.
fn step_words(step: u8, e: i32, src: Option<&[u8]>, parent: &[u8], name: &[u8]) -> Vec<u8> {
    let src = src.unwrap_or_default();
    let dest = cat!(parent, "/", name);
    // (here, in the parent: the system's own words for it)
    let why = std::io::Error::from_raw_os_error(e).to_string();
    match step {
        1 => cat!("cannot join the box's user namespace: ", why),
        2 => cat!("cannot make a mount namespace: ", why),
        3 if e == libc::ELOOP => cat!(
            src,
            " goes through a link now (it moved since it was checked)"
        ),
        3 => cat!("cannot open ", src, ": ", why),
        4 => cat!("cannot clone ", src, ": ", why),
        5 => cat!("cannot make ", src, " read-only: ", why),
        6 => cat!("cannot enter the box's mount namespace: ", why),
        7 | 8 if e == libc::ELOOP => cat!(dest, " in the box goes through a link"),
        8 if e == libc::EXDEV => cat!(dest, " in the box is a mount already"),
        7 | 8 => cat!("cannot open ", dest, " in the box: ", why),
        9 if src.is_empty() && e == libc::EINVAL => cat!(dest, " in the box is not a mount"),
        9 if src.is_empty() => cat!("cannot unmount ", dest, " in the box: ", why),
        9 => cat!("cannot mount at ", dest, " in the box: ", why),
        _ => cat!("the mount failed: ", why),
    }
}

/// openat2's `struct open_how` (libc's is non-exhaustive: it cannot be made here).
#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

/// What the child does, raw: fds and C strings made before the fork.
struct Job {
    userns: RawFd,
    pidfd: RawFd,
    /// The folder to mount (None: unmount).
    src: Option<*const libc::c_char>,
    parent: *const libc::c_char,
    name: *const libc::c_char,
    report: RawFd,
}

impl Job {
    /// Runs in the child after fork: syscalls only, then _exit (0 when done, 1 with the step and
    /// errno written to REPORT).
    unsafe fn run(&self) -> ! {
        let fail = |step: u8| -> ! {
            let e = errno().to_le_bytes();
            let b = [step, e[0], e[1], e[2], e[3]];
            // SAFETY: a write of 5 bytes of ours, then the child's end.
            unsafe {
                libc::write(self.report, b.as_ptr().cast(), b.len());
                libc::_exit(1)
            }
        };
        // SAFETY: each call below is a plain syscall on fds and NUL-terminated strings this Job
        // holds, alive in the parent's memory, which the child shares as a copy.
        unsafe {
            if libc::setns(self.userns, libc::CLONE_NEWUSER) != 0 {
                fail(1);
            }
            let open = |dir: RawFd, p: *const libc::c_char, extra: u64| -> libc::c_long {
                // (no O_NOFOLLOW: with O_PATH it would open a link itself, where
                // RESOLVE_NO_SYMLINKS refuses one, ELOOP)
                let how = OpenHow {
                    flags: (libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC) as u64,
                    mode: 0,
                    resolve: libc::RESOLVE_NO_SYMLINKS | libc::RESOLVE_NO_MAGICLINKS | extra,
                };
                // (openat2 on a NUL-terminated path and an open_how of ours)
                libc::syscall(
                    libc::SYS_openat2,
                    dir,
                    p,
                    &how as *const OpenHow,
                    std::mem::size_of::<OpenHow>(),
                )
            };
            let tree = match self.src {
                Some(src) => {
                    if libc::unshare(libc::CLONE_NEWNS) != 0 {
                        fail(2);
                    }
                    // SRC is its real path: opened with no link followed, nothing swapped in on its
                    // way since it was checked is cloned.
                    let at = open(libc::AT_FDCWD, src, 0);
                    if at < 0 {
                        fail(3);
                    }
                    let flags = libc::OPEN_TREE_CLONE
                        | libc::OPEN_TREE_CLOEXEC
                        | libc::AT_RECURSIVE as libc::c_uint
                        | libc::AT_EMPTY_PATH as libc::c_uint;
                    let t = libc::syscall(libc::SYS_open_tree, at as RawFd, c"".as_ptr(), flags);
                    if t < 0 {
                        fail(4);
                    }
                    let t = t as RawFd;
                    let attr = libc::mount_attr {
                        attr_set: libc::MOUNT_ATTR_RDONLY
                            | libc::MOUNT_ATTR_NOSUID
                            | libc::MOUNT_ATTR_NODEV,
                        attr_clr: 0,
                        propagation: 0,
                        userns_fd: 0,
                    };
                    if libc::syscall(
                        libc::SYS_mount_setattr,
                        t,
                        c"".as_ptr(),
                        libc::AT_EMPTY_PATH | libc::AT_RECURSIVE,
                        &attr as *const libc::mount_attr,
                        std::mem::size_of::<libc::mount_attr>(),
                    ) != 0
                    {
                        fail(5);
                    }
                    Some(t)
                }
                None => None,
            };
            if libc::setns(self.pidfd, libc::CLONE_NEWNS) != 0 {
                fail(6);
            }
            let pfd = open(libc::AT_FDCWD, self.parent, 0);
            if pfd < 0 {
                fail(7);
            }
            let pfd = pfd as RawFd;
            match tree {
                Some(t) => {
                    let dest = open(
                        pfd,
                        self.name,
                        libc::RESOLVE_BENEATH | libc::RESOLVE_NO_XDEV,
                    );
                    if dest < 0 {
                        fail(8);
                    }
                    if libc::syscall(
                        libc::SYS_move_mount,
                        t,
                        c"".as_ptr(),
                        dest as RawFd,
                        c"".as_ptr(),
                        libc::MOVE_MOUNT_F_EMPTY_PATH | libc::MOVE_MOUNT_T_EMPTY_PATH,
                    ) != 0
                    {
                        fail(9);
                    }
                }
                None => {
                    if libc::fchdir(pfd) != 0 {
                        fail(8);
                    }
                    if libc::umount2(self.name, libc::UMOUNT_NOFOLLOW | libc::MNT_DETACH) != 0 {
                        fail(9);
                    }
                }
            }
            libc::_exit(0)
        }
    }
}
