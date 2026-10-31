//! Processes, as /proc has them.

use crate::shell::{self, strip_nl};
use std::os::unix::fs::MetadataExt;

fn proc(pid: &[u8], rel: &str) -> std::path::PathBuf {
    shell::path(&crate::cat!("/proc/", pid, "/", rel))
}

/// `$(cat /proc/PID/FILE)`.
pub fn read(pid: &[u8], rel: &str) -> Option<Vec<u8>> {
    std::fs::read(proc(pid, rel)).ok().map(|v| strip_nl(&v))
}

/// The fields of a stat line after the command name, which may hold spaces and parentheses:
/// `${s##*) }`, what follows its last ") ".
pub fn after_comm(s: &[u8]) -> &[u8] {
    match s.windows(2).rposition(|w| w == b") ") {
        Some(i) => &s[i + 2..],
        None => s,
    }
}

/// Field I of /proc/PID/stat after the command name (0 its state, 1 its parent, 19 its start), empty
/// when it has none; None for no such process. A pid or `self` only: an empty one would read
/// /proc//stat, which is the system-wide /proc/stat.
pub fn proc_stat(pid: &[u8], i: usize) -> Option<Vec<u8>> {
    if !shell::is_num(pid) && pid != b"self" {
        return None;
    }
    let s = read(pid, "stat")?;
    Some(
        shell::words(after_comm(&s))
            .into_iter()
            .nth(i)
            .unwrap_or_default(),
    )
}

pub fn proc_ppid(pid: &[u8]) -> Option<Vec<u8>> {
    proc_stat(pid, 1)
}

pub fn proc_start(pid: &[u8]) -> Option<Vec<u8>> {
    proc_stat(pid, 19)
}

/// This process's start, in clock ticks since boot: /proc/self, never /proc/<its pid>, which in a pid
/// namespace of its own with the host's /proc mounted is another process (its pid 1 the host's init).
pub fn self_start() -> Option<Vec<u8>> {
    proc_start(b"self")
}

/// PID's exit is under way: stat's flags have the kernel's PF_EXITING (0x4), from its exit's
/// start, while it tears down its memory and files, through to the zombie (finding 606).
pub fn exiting(pid: &[u8]) -> bool {
    proc_stat(pid, 6)
        .and_then(|f| String::from_utf8(f).ok())
        .and_then(|f| f.parse::<u64>().ok())
        .is_some_and(|f| f & 0x4 != 0)
}

/// PID is running: not gone, and not a child that exited and was not waited for yet (a zombie).
pub fn running(pid: &[u8]) -> bool {
    let Some(s) = read(pid, "stat") else {
        return false;
    };
    let rest = after_comm(&s);
    let state = rest.split(|&c| c == b' ').next().unwrap_or_default();
    state != b"Z"
}

/// `pgrep -P PID`: its children, lowest pid first.
pub fn children(pid: &[u8]) -> Vec<Vec<u8>> {
    let Some(want) = std::str::from_utf8(pid)
        .ok()
        .and_then(|p| p.parse::<u64>().ok())
    else {
        return Vec::new();
    };
    let mut kids: Vec<u64> = Vec::new();
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for e in rd.flatten() {
        let Some(p) = e.file_name().to_str().and_then(|n| n.parse::<u64>().ok()) else {
            continue;
        };
        let Ok(s) = std::fs::read(e.path().join("stat")) else {
            continue;
        };
        let ppid = shell::words(after_comm(&s)).into_iter().nth(1);
        if ppid.and_then(|v| String::from_utf8(v).ok()?.parse::<u64>().ok()) == Some(want) {
            kids.push(p);
        }
    }
    kids.sort_unstable();
    kids.into_iter()
        .map(|p| p.to_string().into_bytes())
        .collect()
}

/// `awk '/^KEY:/ {print $N; exit}' /proc/PID/status`: field N (1 the key itself) of KEY's line;
/// None when the file cannot be read, empty when the line or field is not there.
pub fn status(pid: &[u8], key: &str, n: usize) -> Option<Vec<u8>> {
    let s = std::fs::read(proc(pid, "status")).ok()?;
    for line in s.split(|&c| c == b'\n') {
        if line.starts_with(key.as_bytes()) && line.get(key.len()) == Some(&b':') {
            return Some(
                shell::words(line)
                    .into_iter()
                    .nth(n - 1)
                    .unwrap_or_default(),
            );
        }
    }
    Some(Vec::new())
}

/// The last field of KEY's line in /proc/PID/status (awk's $NF), empty when it is not there.
pub fn status_last(s: &[u8], key: &str) -> Vec<u8> {
    let mut v = Vec::new();
    for line in s.split(|&c| c == b'\n') {
        if line.starts_with(key.as_bytes()) && line.get(key.len()) == Some(&b':') {
            v = shell::words(line).pop().unwrap_or_default();
        }
    }
    v
}

/// `readlink /proc/PID/ns/pid`.
pub fn pidns(pid: &[u8]) -> Option<Vec<u8>> {
    std::fs::read_link(proc(pid, "ns/pid"))
        .ok()
        .map(|p| shell::bytes(&p).to_vec())
}

/// The pid this pid namespace (the caller's) made last: /proc/loadavg's last field, which the kernel
/// reads from the caller's own namespace whatever /proc is mounted. Pids are made in order (until
/// they wrap at pid_max): one at or below it was made before it was read (finding 513).
pub fn last_pid() -> Option<i64> {
    let s = std::fs::read("/proc/loadavg").ok()?;
    let w = shell::words(&s);
    let last = w.last()?;
    if last.is_empty() || !last.iter().all(u8::is_ascii_digit) {
        return None;
    }
    shell::dec(last)
}

/// `readlink /proc/self/ns/time`: the time namespace CLOCK_BOOTTIME is read in (#404).
pub fn timens() -> Option<Vec<u8>> {
    std::fs::read_link("/proc/self/ns/time")
        .ok()
        .map(|p| shell::bytes(&p).to_vec())
        .filter(|n| !n.is_empty() && !n.contains(&b' '))
}

fn clock_us(clock: libc::clockid_t) -> Option<i64> {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime writes the timespec it is given.
    if unsafe { libc::clock_gettime(clock, &raw mut t) } != 0 {
        return None;
    }
    Some(t.tv_sec * 1_000_000 + t.tv_nsec / 1000)
}

/// CLOCK_BOOTTIME now, in µs: one clock for every pid namespace (a time namespace's offset aside).
pub fn boot_us() -> Option<i64> {
    clock_us(libc::CLOCK_BOOTTIME)
}

/// When this process was made, in µs of CLOCK_BOOTTIME, at the latest (#404): now less the time its
/// thread has spent on a CPU (CLOCK_THREAD_CPUTIME_ID) and waiting for one (schedstat's run_delay)
/// since its fork. Only a sleep (a read from disk) is left out, which makes it later, never earlier.
/// Its start in /proc/PID/stat is in 10 ms ticks; this tells two starts in one tick apart across pid
/// namespaces, where the pids' order cannot. Read first thing, before anything that may sleep.
pub fn made_us() -> Option<i64> {
    let now = boot_us()?;
    let cpu = clock_us(libc::CLOCK_THREAD_CPUTIME_ID)?;
    let wait = std::fs::read("/proc/thread-self/schedstat")
        .ok()
        .and_then(|s| shell::words(&s).get(1).and_then(|w| shell::dec(w)))
        .map_or(0, |ns| ns / 1000);
    Some(now - cpu - wait)
}

/// /proc/sys/kernel/pid_max: where pids wrap (the most a 64-bit kernel allows when it cannot be read).
pub fn pid_max() -> i64 {
    read(b"sys", "kernel/pid_max")
        .and_then(|v| shell::dec(&v))
        .filter(|&n| n > 0)
        .unwrap_or(4_194_304)
}

/// The processes that entered the box whose PID 1 is PID from outside (finding 490): in its user
/// namespace, not in its pid namespace. An on_box tool, the process outside that waits for a command
/// in the box (enter.rs), and an nsenter of bash's once it is in; never this one. Each pinned by a
/// pidfd once found (#377): a signal sent later reaches it or nothing, never a process that took
/// its pid since.
pub fn entered(pid: &[u8]) -> Vec<crate::pidfd::Pidfd> {
    let ns = |p: &[u8], n: &str| std::fs::read_link(proc(p, n)).ok();
    let (Some(user), Some(pidns)) = (ns(pid, "ns/user"), ns(pid, "ns/pid")) else {
        return Vec::new();
    };
    let me = std::process::id().to_string().into_bytes();
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let inside = |p: &[u8]| {
        ns(p, "ns/user").as_ref() == Some(&user) && ns(p, "ns/pid").is_some_and(|n| n != pidns)
    };
    rd.flatten()
        .map(|e| e.file_name().into_encoded_bytes())
        .filter(|p| shell::is_num(p) && *p != me && inside(p))
        // (looked at again once pinned: the process the first look saw may have gone since, and
        // its pid been reused)
        .filter_map(|p| crate::pidfd::Pidfd::pin(&p, || inside(&p)))
        .collect()
}

/// `stat -c %u /proc/PID`.
pub fn owner(pid: &[u8]) -> Option<u32> {
    std::fs::symlink_metadata(shell::path(&crate::cat!("/proc/", pid)))
        .ok()
        .map(|m| m.uid())
}

/// `id -u`.
pub fn euid() -> u32 {
    // SAFETY: geteuid cannot fail and touches no memory of ours.
    unsafe { libc::geteuid() }
}

/// `$UID`.
pub fn uid() -> u32 {
    // SAFETY: getuid cannot fail and touches no memory of ours.
    unsafe { libc::getuid() }
}

/// `id -nu UID`: the user's name, None for a uid with none.
pub fn user_name(uid: u32) -> Option<String> {
    let mut buf = vec![0u8; 16384];
    // SAFETY: passwd is plain data getpwuid_r fills in.
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut res: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: every pointer is to memory we own, buf's length given.
    let rc =
        unsafe { libc::getpwuid_r(uid, &mut pw, buf.as_mut_ptr().cast(), buf.len(), &mut res) };
    if rc != 0 || res.is_null() || pw.pw_name.is_null() {
        return None;
    }
    // SAFETY: pw_name points into buf, NUL-terminated by getpwuid_r.
    let name = unsafe { std::ffi::CStr::from_ptr(pw.pw_name) };
    Some(String::from_utf8_lossy(name.to_bytes()).into_owned())
}

/// `kill -l N` in bash: the signal's name without SIG; "" for 32 and 33 (glibc's own); None for no
/// signal. Past 128 it is an exit status: N - 128.
pub fn signame(word: &[u8]) -> Option<&'static str> {
    const NAMES: [&str; 32] = [
        "EXIT", "HUP", "INT", "QUIT", "ILL", "TRAP", "ABRT", "BUS", "FPE", "KILL", "USR1", "SEGV",
        "USR2", "PIPE", "ALRM", "TERM", "STKFLT", "CHLD", "CONT", "STOP", "TSTP", "TTIN", "TTOU",
        "URG", "XCPU", "XFSZ", "VTALRM", "PROF", "WINCH", "IO", "PWR", "SYS",
    ];
    const RT: [&str; 31] = [
        "RTMIN", "RTMIN+1", "RTMIN+2", "RTMIN+3", "RTMIN+4", "RTMIN+5", "RTMIN+6", "RTMIN+7",
        "RTMIN+8", "RTMIN+9", "RTMIN+10", "RTMIN+11", "RTMIN+12", "RTMIN+13", "RTMIN+14",
        "RTMIN+15", "RTMAX-14", "RTMAX-13", "RTMAX-12", "RTMAX-11", "RTMAX-10", "RTMAX-9",
        "RTMAX-8", "RTMAX-7", "RTMAX-6", "RTMAX-5", "RTMAX-4", "RTMAX-3", "RTMAX-2", "RTMAX-1",
        "RTMAX",
    ];
    let mut n = shell::dec(word)?;
    if n > 128 {
        n -= 128;
    }
    match n {
        0..=31 => NAMES.get(usize::try_from(n).ok()?).copied(),
        32 | 33 => Some(""),
        34..=64 => RT.get(usize::try_from(n - 34).ok()?).copied(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_fields_after_a_name_with_spaces_and_parens() {
        let s = b"123 (a) b (c)) S 1 123 123 0 -1";
        assert_eq!(after_comm(s), b"S 1 123 123 0 -1");
        let w = shell::words(after_comm(s));
        assert_eq!(w[0], b"S");
        assert_eq!(w[1], b"1");
        assert_eq!(after_comm(b"no parens here"), b"no parens here");
    }

    #[test]
    fn stat_of_this_process() {
        let me = std::process::id().to_string().into_bytes();
        // (its state is the scheduler's: R, or S or D while another thread of the test waits)
        let st = proc_stat(&me, 0).unwrap_or_default();
        assert!(matches!(st.as_slice(), b"R" | b"S" | b"D"), "{st:?}");
        let ppid = std::os::unix::process::parent_id().to_string().into_bytes();
        assert_eq!(proc_ppid(&me), Some(ppid));
        assert!(proc_start(&me).is_some_and(|s| !s.is_empty()));
        assert!(proc_stat(b"", 0).is_none());
        assert!(proc_stat(b"1x", 0).is_none());
        assert!(running(&me));
    }

    #[test]
    fn signal_names_as_bash() {
        let n = |s: &str| signame(s.as_bytes());
        assert_eq!(n("0"), Some("EXIT"));
        assert_eq!(n("11"), Some("SEGV"));
        assert_eq!(n("07"), Some("BUS"));
        assert_eq!(n("32"), Some(""));
        assert_eq!(n("34"), Some("RTMIN"));
        assert_eq!(n("49"), Some("RTMIN+15"));
        assert_eq!(n("50"), Some("RTMAX-14"));
        assert_eq!(n("64"), Some("RTMAX"));
        assert_eq!(n("65"), None);
        assert_eq!(n("128"), None);
        assert_eq!(n("143"), Some("TERM"));
        assert_eq!(n("172"), Some("RTMIN+10"));
        assert_eq!(n("192"), Some("RTMAX"));
        assert_eq!(n("193"), None);
    }

    #[test]
    fn status_fields() {
        let s = b"Name:\tx\nNSpid:\t10\t1\nNSsid:\t9\t1\n";
        assert_eq!(status_last(s, "NSpid"), b"1");
        assert_eq!(status_last(s, "State"), b"");
    }

    // t_unit_box_gone: a PID 1 that exited and is not reaped yet is no box (its namespaces are
    // gone): not running, so not alive, and dead in ls. (Its parent execs a sleep, which never
    // reaps it.)
    #[test]
    #[ignore = "starts processes: run by test/run.sh (cargo test -- --ignored)"]
    fn a_zombie_is_not_running() {
        let mut c = std::process::Command::new("sh")
            .args(["-c", "sleep 0 & echo $!; exec sleep 10"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut out = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(c.stdout.take().unwrap()),
            &mut out,
        )
        .unwrap();
        let z = out.trim().as_bytes().to_vec();
        let mut st = Vec::new();
        for _ in 0..40 {
            st = read(&z, "stat").unwrap_or_default();
            if after_comm(&st).starts_with(b"Z") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let zombie = after_comm(&st).starts_with(b"Z");
        let alive = running(&z);
        let _ = c.kill();
        let _ = c.wait();
        assert!(zombie, "a zombie: {:?}", String::from_utf8_lossy(&st));
        assert!(!alive, "a zombie PID 1 is not alive (nor up in ls)");
    }
}
