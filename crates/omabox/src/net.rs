//! A box's network (#324 wave 5a, #355): pasta's arguments for each mode (finding 89), the isolated
//! mode carrying its allow list (S5), and the protected ports: host ports no connected box reaches.
//!
//! Protected ports (#355, S10): `protect-ports=PORT[,PORT...]` in ~/.config/omabox/config, none by
//! default. A connected box gets each one excluded (`~PORT`) from both of pasta's `-T` rules and both
//! `-U` rules, so its loopback forwards none of them to the host's, and from both `-t` and both `-u`
//! rules, so a box server on one never takes the host's 127.0.0.1:PORT (#364); and, set up before
//! anything in the box runs by pasta's own shell (root in pasta's namespaces, which the box's user
//! namespace below maps back to the user with no capabilities), an nftables rule in the box's
//! network namespace that rejects a connection to any of the host's addresses on those ports, as a
//! server listening on all of them (0.0.0.0) would otherwise take one through the host's docker
//! bridge or VPN address. The box's reaper keeps that rule's addresses the host's as it gains or
//! loses one (#399). An isolated box reaches only its --allow ports, and a protected one is
//! refused there.

use crate::cat;

/// Ports: sorted, each once, 1-65535 (allow_ports: 8082,8081,8081 is 8081,8082, finding 175).
pub use crate::upspec::AllowedPorts as Ports;

/// How a box is networked: pasta's tap with the host's loopback forwarded both ways (connected),
/// a loopback of its own with only the allowed ports spliced to the host's (isolated), or a network
/// namespace with only a loopback and no pasta (none: a nested connected box, or one started from a
/// no_new_privs process).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Net {
    Connected { protect: Ports },
    Isolated { allow: Ports },
    None,
}

/// pasta's arguments for NET (not None), its pid written to PID_FILE; EPH the ephemeral range
/// (`32768-60999`), which `auto` skips and so has rules of its own (finding 156).
pub fn pasta_args(net: &Net, pid_file: &[u8], eph: &[u8]) -> Vec<Vec<u8>> {
    let mut a: Vec<Vec<u8>> = vec![b"-q".to_vec(), b"-P".to_vec(), pid_file.to_vec()];
    let mut add = |w: &[&[u8]]| a.extend(w.iter().map(|x| x.to_vec()));
    match net {
        Net::Isolated { allow } => {
            let t = if allow.is_empty() {
                b"none".to_vec()
            } else {
                allow.csv()
            };
            add(&[
                b"--splice-only",
                b"-t",
                b"none",
                b"-u",
                b"none",
                b"-U",
                b"none",
                b"-T",
                &t,
            ]);
        }
        Net::Connected { protect } => {
            // (the protected ports out of every host-bound rule, #355: `~P,` in front)
            let ex: Vec<u8> = protect
                .ports()
                .iter()
                .flat_map(|p| format!("~{p},").into_bytes())
                .collect();
            let auto = cat!(ex, "auto");
            let eph_auto = cat!(ex, eph, ",auto");
            // (and out of the box-bound ones, #364: a box server on a protected port the host's
            // service has let go of would take the host's 127.0.0.1:PORT, and its clients)
            let lo_auto = cat!("127.0.0.1/", auto);
            let lo_eph_auto = cat!("127.0.0.1/", eph_auto);
            add(&[
                b"--config-net",
                b"--no-map-gw",
                b"-D",
                b"none",
                b"-S",
                b"none",
                b"--host-lo-to-ns-lo",
                b"-t",
                &lo_auto,
                b"-t",
                &lo_eph_auto,
                b"-u",
                &lo_auto,
                b"-u",
                &lo_eph_auto,
                b"-T",
                &auto,
                b"-T",
                &eph_auto,
                b"-U",
                &auto,
                b"-U",
                &eph_auto,
            ]);
        }
        Net::None => {}
    }
    a
}

/// The ephemeral port range (`/proc/sys/net/ipv4/ip_local_port_range`, as `32768-60999`), that
/// default when it cannot be read.
pub fn ephemeral() -> Vec<u8> {
    let s = std::fs::read("/proc/sys/net/ipv4/ip_local_port_range").unwrap_or_default();
    let w: Vec<&[u8]> = s
        .split(|c| c.is_ascii_whitespace())
        .filter(|x| !x.is_empty())
        .collect();
    match w.as_slice() {
        [a, b, ..] if a.iter().all(u8::is_ascii_digit) && b.iter().all(u8::is_ascii_digit) => {
            cat!(a, "-", b)
        }
        _ => b"32768-60999".to_vec(),
    }
}

/// The host's own addresses, from its routing tables (`/32 host LOCAL` in /proc/net/fib_trie) and
/// its links' IPv4 addresses (RTM_GETADDR), and /proc/net/if_inet6, loopback's left out (a box's
/// loopback is its own), each once, IPv4 then IPv6. The links' are asked as well (#399): the kernel
/// says an IPv4 address came or went before its local route does, so a reaper woken by that event
/// can read fib_trie a moment early; the links' list is already the new one.
pub fn host_addrs() -> (Vec<String>, Vec<String>) {
    let v4 = std::fs::read("/proc/net/fib_trie").unwrap_or_default();
    let v6 = std::fs::read("/proc/net/if_inet6").unwrap_or_default();
    let mut v4 = local_v4(&v4);
    for a in link_v4() {
        if !v4.contains(&a) {
            v4.push(a);
        }
    }
    (v4, local_v6(&v6))
}

/// Every link's IPv4 address (an RTM_GETADDR dump's IFA_LOCAL, else IFA_ADDRESS), loopback's left
/// out; none when netlink does not answer (within a second).
fn link_v4() -> Vec<String> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    // SAFETY: socket takes numbers; the fd it gives is new and ours alone.
    let fd = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC,
            libc::NETLINK_ROUTE,
        )
    };
    if fd < 0 {
        return Vec::new();
    }
    // SAFETY: just made, ours alone.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let tv = libc::timeval {
        tv_sec: 1,
        tv_usec: 0,
    };
    // SAFETY: setsockopt reads a timeval of ours, of its size.
    unsafe {
        libc::setsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            (&raw const tv).cast(),
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        );
    }
    // nlmsghdr (len, type, flags, seq, pid) and an ifaddrmsg asking for AF_INET's.
    let mut req = Vec::with_capacity(24);
    req.extend_from_slice(&24u32.to_ne_bytes());
    req.extend_from_slice(&libc::RTM_GETADDR.to_ne_bytes());
    req.extend_from_slice(&((libc::NLM_F_REQUEST | libc::NLM_F_DUMP) as u16).to_ne_bytes());
    req.extend_from_slice(&1u32.to_ne_bytes());
    req.extend_from_slice(&0u32.to_ne_bytes());
    req.extend_from_slice(&[libc::AF_INET as u8, 0, 0, 0, 0, 0, 0, 0]);
    // SAFETY: send reads REQ, ours, of its length.
    if unsafe { libc::send(fd.as_raw_fd(), req.as_ptr().cast(), req.len(), 0) } < 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut buf = vec![0u8; 32768];
    loop {
        // SAFETY: recv into a buffer of ours, at most its length.
        let n = unsafe { libc::recv(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
        let Ok(n) = usize::try_from(n) else {
            return out;
        };
        if n == 0 {
            return out;
        }
        match addr_dump(&buf[..n], &mut out) {
            Dump::More => continue,
            Dump::Done => return out,
        }
    }
}

enum Dump {
    More,
    Done,
}

/// One datagram of an RTM_GETADDR dump: each RTM_NEWADDR's IPv4 address into OUT (loopback's
/// left out, each once); Done at NLMSG_DONE, an error, or a message that does not parse.
fn addr_dump(b: &[u8], out: &mut Vec<String>) -> Dump {
    let u16_at = |i: usize| b.get(i..i + 2).map(|s| u16::from_ne_bytes([s[0], s[1]]));
    let u32_at = |i: usize| {
        b.get(i..i + 4)
            .map(|s| u32::from_ne_bytes([s[0], s[1], s[2], s[3]]))
    };
    let mut off = 0usize;
    while off + 16 <= b.len() {
        let (Some(len), Some(ty)) = (u32_at(off), u16_at(off + 4)) else {
            return Dump::Done;
        };
        let len = len as usize;
        if len < 16 || off + len > b.len() {
            return Dump::Done;
        }
        if ty == libc::NLMSG_DONE as u16 || ty == libc::NLMSG_ERROR as u16 {
            return Dump::Done;
        }
        if ty == libc::RTM_NEWADDR && len >= 24 && b[off + 16] == libc::AF_INET as u8 {
            let (mut local, mut address) = (None, None);
            let mut a = off + 24;
            while a + 4 <= off + len {
                let (Some(alen), Some(aty)) = (u16_at(a), u16_at(a + 2)) else {
                    break;
                };
                let alen = alen as usize;
                if alen < 4 || a + alen > off + len {
                    break;
                }
                if alen == 8 {
                    let ip = std::net::Ipv4Addr::new(b[a + 4], b[a + 5], b[a + 6], b[a + 7]);
                    if aty == libc::IFA_LOCAL {
                        local = Some(ip);
                    } else if aty == libc::IFA_ADDRESS {
                        address = Some(ip);
                    }
                }
                a += (alen + 3) & !3;
            }
            if let Some(ip) = local.or(address)
                && !ip.is_loopback()
                && !out.contains(&ip.to_string())
            {
                out.push(ip.to_string());
            }
        }
        off += (len + 3) & !3;
    }
    Dump::More
}

fn local_v4(trie: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut last: Option<std::net::Ipv4Addr> = None;
    for l in trie.split(|&c| c == b'\n') {
        let t = String::from_utf8_lossy(l);
        let t = t.trim();
        if let Some(a) = t.strip_prefix("|-- ") {
            last = a.parse().ok();
        } else if t == "/32 host LOCAL"
            && let Some(a) = last.take()
            && !a.is_loopback()
            && !out.contains(&a.to_string())
        {
            out.push(a.to_string());
        }
    }
    out
}

fn local_v6(table: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in table.split(|&c| c == b'\n') {
        let Some(hex) = l.split(|&c| c == b' ').next() else {
            continue;
        };
        if hex.len() != 32 {
            continue;
        }
        let Some(bytes) = (0..16)
            .map(|i| {
                std::str::from_utf8(&hex[2 * i..2 * i + 2])
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
            })
            .collect::<Option<Vec<u8>>>()
        else {
            continue;
        };
        let Ok(arr) = <[u8; 16]>::try_from(bytes) else {
            continue;
        };
        let a = std::net::Ipv6Addr::from(arr);
        if !a.is_loopback() && !out.contains(&a.to_string()) {
            out.push(a.to_string());
        }
    }
    out
}

/// The nft program, for pasta's shell, that rejects connections from the box to any of the host's
/// addresses on a protected port; not to the box's own (oifname lo: its loopback and the address
/// pasta gave it), whose services are its own. The addresses are the sets host4 and host6 (V4 and
/// V6 at up), which the box's reaper keeps the host's as it gains or loses one (#399,
/// protect_sets). None with no port.
pub fn protect_rules(protect: &Ports, v4: &[String], v6: &[String]) -> Option<String> {
    if protect.is_empty() {
        return None;
    }
    let ports = protect
        .ports()
        .iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let mut r = String::from(
        "add table inet omabox; add chain inet omabox out { type filter hook output priority 0; policy accept; }; add set inet omabox host4 { type ipv4_addr; }; add set inet omabox host6 { type ipv6_addr; }",
    );
    r.push_str(&set_elements(v4, v6));
    for (fam, set) in [("ip", "host4"), ("ip6", "host6")] {
        r.push_str(&format!(
            "; add rule inet omabox out oifname != \"lo\" {fam} daddr @{set} meta l4proto {{ tcp, udp }} th dport {{ {ports} }} reject"
        ));
    }
    Some(r)
}

/// The nft program that makes protect_rules' sets host4 and host6 the host's addresses V4 and V6
/// now, in one transaction: no moment with neither the old nor the new ones.
pub fn protect_sets(v4: &[String], v6: &[String]) -> String {
    let mut r = String::from("flush set inet omabox host4; flush set inet omabox host6");
    r.push_str(&set_elements(v4, v6));
    r
}

/// `; add element ...` for each set with an address (nft takes no empty element list).
fn set_elements(v4: &[String], v6: &[String]) -> String {
    let mut r = String::new();
    for (set, addrs) in [("host4", v4), ("host6", v6)] {
        if !addrs.is_empty() {
            r.push_str(&format!(
                "; add element inet omabox {set} {{ {} }}",
                addrs.join(", ")
            ));
        }
    }
    r
}

/// A netlink socket that hears of every address this network namespace gains or loses (IPv4 and
/// IPv6), for the reaper of a box with protected ports (#399): it reads the host's addresses again
/// as soon as one changes, not at its next poll.
pub struct AddrEvents(std::os::fd::OwnedFd);

impl AddrEvents {
    /// None when the socket cannot be had (the reaper then reads the addresses at every look).
    pub fn open() -> Option<AddrEvents> {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        // SAFETY: socket takes numbers; the fd it gives is new and ours alone.
        let fd = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
                libc::NETLINK_ROUTE,
            )
        };
        if fd < 0 {
            return None;
        }
        // SAFETY: just made, ours alone.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: an all-zero sockaddr_nl is a valid one (no pid: the kernel picks it).
        let mut sa: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        sa.nl_family = libc::AF_NETLINK as libc::sa_family_t;
        sa.nl_groups = (libc::RTMGRP_IPV4_IFADDR | libc::RTMGRP_IPV6_IFADDR) as u32;
        let len = std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t;
        // SAFETY: bind reads LEN bytes of SA, ours and filled above.
        if unsafe { libc::bind(fd.as_raw_fd(), (&raw const sa).cast(), len) } != 0 {
            return None;
        }
        Some(AddrEvents(fd))
    }

    /// Waits up to MAX for something to read (an event, at once), and says whether there is.
    pub fn wait(&self, max: std::time::Duration) -> bool {
        use std::os::fd::AsRawFd;
        let mut p = libc::pollfd {
            fd: self.0.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ms = libc::c_int::try_from(max.as_millis()).unwrap_or(libc::c_int::MAX);
        // SAFETY: poll on one pollfd of ours; a signal's EINTR is a wait that ended early.
        unsafe { libc::poll(&raw mut p, 1, ms) > 0 }
    }

    /// Whether anything came since the last call (all of it read): an address event, or the
    /// socket's buffer overrun (ENOBUFS: events were lost, so one may have changed).
    pub fn any(&self) -> bool {
        use std::os::fd::AsRawFd;
        let mut buf = [0u8; 8192];
        let mut any = false;
        loop {
            // SAFETY: recv into a buffer of ours, at most its length, on our fd, without blocking.
            let n = unsafe {
                libc::recv(
                    self.0.as_raw_fd(),
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    libc::MSG_DONTWAIT,
                )
            };
            if n > 0
                || (n < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOBUFS))
            {
                any = true;
                continue;
            }
            return any;
        }
    }
}

/// protect-ports from ~/.config/omabox/config: none when unset. Blanks around each port are fine
/// (`8080, 5432`); anything else that is not a list of ports 1-65535 is refused, exit 2, where every
/// other setting's bad value is said and ignored: ignored, it would let boxes reach the ports the
/// user meant to keep from them (S10 fails closed).
/// A config that is there but cannot be read (finding 482) is refused the same way: what it says of
/// protect-ports cannot be known, and read as none it would let boxes reach those ports.
pub fn conf_protect(ctx: &crate::exit::Ctx) -> crate::exit::Res<Ports> {
    if let Some((config, why)) = crate::conf::unreadable() {
        crate::exit::say(protect_unread(&config, &why));
        return Err(crate::exit::Exit(ctx.die_rc.unwrap_or(2)));
    }
    let (config, v) = crate::conf::conf_raw("protect-ports");
    if v.is_empty() {
        return Ok(Ports::default());
    }
    match protect_parse(&v) {
        Some(p) => Ok(p),
        None => {
            crate::exit::say(protect_bad(&config, &v));
            Err(crate::exit::Exit(ctx.die_rc.unwrap_or(2)))
        }
    }
}

/// What up says of a config it cannot read, for protect-ports (`config` too).
pub fn protect_unread(config: &[u8], why: &str) -> Vec<u8> {
    cat!(
        config,
        " cannot be read (",
        why,
        "): no box starts until it can, as protect-ports there cannot be kept (a box would reach the ports it means to keep)"
    )
}

/// What up says of a protect-ports value V in CONFIG that is not a list of ports (`config` too).
pub fn protect_bad(config: &[u8], v: &[u8]) -> Vec<u8> {
    cat!(
        config,
        ": protect-ports=",
        v,
        " is not a list of ports (PORT[,PORT...], each 1-65535): no box starts until it is one, or the line is gone (a box would reach the ports it means to keep)"
    )
}

/// `8080, 5432` as Ports: each comma-separated item with blanks (space, tab) around it trimmed; an
/// empty item, a trailing comma or anything but digits is no list.
pub fn protect_parse(v: &[u8]) -> Option<Ports> {
    let blank = |c: &u8| *c == b' ' || *c == b'\t';
    let mut items: Vec<&[u8]> = Vec::new();
    for it in v.split(|&c| c == b',') {
        let s = it.iter().position(|c| !blank(c)).unwrap_or(it.len());
        let e = it.iter().rposition(|c| !blank(c)).map_or(s, |i| i + 1);
        let t = &it[s..e.max(s)];
        if t.is_empty() {
            return None;
        }
        items.push(t);
    }
    Ports::parse(&items.join(&b',')).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn ports() {
        assert_eq!(
            Ports::parse(b"8082,8081,8081").map(|p| p.csv()),
            Ok(b"8081,8082".to_vec())
        );
        assert_eq!(Ports::parse(b"080").map(|p| p.csv()), Ok(b"80".to_vec()));
        for bad in ["0", "65536", "1,,2", ",1", "a", "123456", "-1", " 1"] {
            assert!(Ports::parse(bad.as_bytes()).is_err(), "{bad}");
        }
        assert!(Ports::parse(b"").is_ok_and(|p| p.is_empty()));
    }

    #[test]
    fn addresses() {
        let trie = b"Main:\n  +-- 0.0.0.0/0 3 0 5\n     |-- 0.0.0.0\n        /0 universe UNICAST\n     |-- 127.0.0.1\n        /32 host LOCAL\n     |-- 172.17.0.1\n        /32 host LOCAL\n     |-- 172.17.255.255\n        /32 link BROADCAST\nLocal:\n     |-- 172.17.0.1\n        /32 host LOCAL\n";
        assert_eq!(local_v4(trie), vec!["172.17.0.1".to_string()]);
        let t6 = b"00000000000000000000000000000001 01 80 10 80       lo\nfe80000000000000d4e62dfffe8bebda 04 40 20 80 br-1\n20010db8000000000000000000000005 02 40 00 80 enp\n";
        assert_eq!(
            local_v6(t6),
            vec![
                "fe80::d4e6:2dff:fe8b:ebda".to_string(),
                "2001:db8::5".to_string()
            ]
        );
    }

    #[test]
    fn link_addresses() {
        // RTM_NEWADDR (ifaddrmsg AF_INET) with IFA_ADDRESS and IFA_LOCAL (a peer link: LOCAL wins)
        let msg = |local: [u8; 4], peer: Option<[u8; 4]>| {
            let mut attrs = Vec::new();
            if let Some(p) = peer {
                attrs.extend_from_slice(&8u16.to_ne_bytes());
                attrs.extend_from_slice(&libc::IFA_ADDRESS.to_ne_bytes());
                attrs.extend_from_slice(&p);
            }
            attrs.extend_from_slice(&8u16.to_ne_bytes());
            attrs.extend_from_slice(&libc::IFA_LOCAL.to_ne_bytes());
            attrs.extend_from_slice(&local);
            let mut m = Vec::new();
            m.extend_from_slice(&((24 + attrs.len()) as u32).to_ne_bytes());
            m.extend_from_slice(&libc::RTM_NEWADDR.to_ne_bytes());
            m.extend_from_slice(&[0; 10]);
            m.extend_from_slice(&[libc::AF_INET as u8, 24, 0, 0, 1, 0, 0, 0]);
            m.extend_from_slice(&attrs);
            m
        };
        let mut b = msg([127, 0, 0, 1], None);
        b.extend(msg([10, 239, 1, 1], Some([10, 239, 1, 2])));
        b.extend(msg([172, 17, 0, 1], None));
        let mut out = Vec::new();
        assert!(matches!(addr_dump(&b, &mut out), Dump::More));
        assert_eq!(
            out,
            vec!["10.239.1.1".to_string(), "172.17.0.1".to_string()]
        );
        // NLMSG_DONE ends it; a length past the datagram is no message
        let mut done = vec![0u8; 20];
        done[..4].copy_from_slice(&20u32.to_ne_bytes());
        done[4..6].copy_from_slice(&(libc::NLMSG_DONE as u16).to_ne_bytes());
        assert!(matches!(addr_dump(&done, &mut out), Dump::Done));
        let mut bad = msg([10, 0, 0, 9], None);
        bad[..4].copy_from_slice(&999u32.to_ne_bytes());
        assert!(matches!(addr_dump(&bad, &mut out), Dump::Done));
        assert_eq!(out.len(), 2);
        // and this machine's own: what fib_trie has, the links' list has too (no root needed)
        let fib = local_v4(&std::fs::read("/proc/net/fib_trie").unwrap_or_default());
        let links = link_v4();
        for a in &links {
            assert!(fib.contains(a), "{a} of {links:?} not in {fib:?}");
        }
    }

    #[test]
    fn protect_values() {
        let csv = |v: &[u8]| protect_parse(v).map(|p| p.csv());
        assert_eq!(csv(b"8080, 5432"), Some(b"5432,8080".to_vec()));
        assert_eq!(csv(b" 8080 ,\t5432 "), Some(b"5432,8080".to_vec()));
        for bad in ["8080,", "8080,,1", "80 80", "x", "0", "70000", ","] {
            assert!(protect_parse(bad.as_bytes()).is_none(), "{bad}");
        }
    }

    #[test]
    fn rules() {
        assert!(protect_rules(&Ports::default(), &[], &[]).is_none());
        let p = Ports::parse(b"8080,5432").unwrap_or_default();
        let r = protect_rules(&p, &["172.17.0.1".into()], &[]).unwrap_or_default();
        assert!(r.contains("; add element inet omabox host4 { 172.17.0.1 }"));
        // (no host6 elements, which nft refuses empty; its rule is there for one gained later, #399)
        assert!(!r.contains("add element inet omabox host6"));
        for fam in ["ip daddr @host4", "ip6 daddr @host6"] {
            assert!(r.contains(&format!(
                "oifname != \"lo\" {fam} meta l4proto {{ tcp, udp }} th dport {{ 5432, 8080 }} reject"
            )));
        }
        // #399: what the reaper sets as the host's addresses change, in one transaction.
        assert_eq!(
            protect_sets(&["10.0.0.1".into(), "10.0.0.2".into()], &["fd00::1".into()]),
            "flush set inet omabox host4; flush set inet omabox host6; add element inet omabox host4 { 10.0.0.1, 10.0.0.2 }; add element inet omabox host6 { fd00::1 }"
        );
        assert_eq!(
            protect_sets(&[], &[]),
            "flush set inet omabox host4; flush set inet omabox host6"
        );
    }

    proptest! {
        // S5: an isolated box's pasta opens exactly its --allow ports (spliced, TCP only) and nothing
        // else; a connected box's every rule, host-bound and box-bound (#364), leaves out each
        // protected port, and its box-bound ones listen on the host's 127.0.0.1 only.
        #[test]
        fn pasta_opens_what_it_should(ports in prop::collection::btree_set(1u16..=65535, 0..6)) {
            let csv = ports.iter().map(u16::to_string).collect::<Vec<_>>().join(",");
            let set = Ports::parse(csv.as_bytes()).unwrap_or_default();
            let a = pasta_args(&Net::Isolated { allow: set.clone() }, b"/p", b"32768-60999");
            let opt = |o: &[u8]| a.iter().position(|w| w == o).map(|i| a[i + 1].clone());
            prop_assert_eq!(opt(b"-t"), Some(b"none".to_vec()));
            prop_assert_eq!(opt(b"-u"), Some(b"none".to_vec()));
            prop_assert_eq!(opt(b"-U"), Some(b"none".to_vec()));
            let want = if set.is_empty() { b"none".to_vec() } else { set.csv() };
            prop_assert_eq!(opt(b"-T"), Some(want));
            prop_assert!(a.iter().any(|w| w == b"--splice-only"));
            prop_assert_eq!(a.iter().filter(|w| w.starts_with(b"-") && w.len() == 2 && w[1].is_ascii_alphabetic() && w.as_slice() != b"-q" && w.as_slice() != b"-P").count(), 4);
            let c = pasta_args(&Net::Connected { protect: set.clone() }, b"/p", b"32768-60999");
            let (mut host_rules, mut box_rules) = (0, 0);
            for (i, w) in c.iter().enumerate() {
                let to_box = w == b"-t" || w == b"-u";
                if to_box || w == b"-T" || w == b"-U" {
                    if to_box { box_rules += 1 } else { host_rules += 1 }
                    let spec = String::from_utf8_lossy(&c[i + 1]).into_owned();
                    prop_assert!(!to_box || spec.starts_with("127.0.0.1/"), "{}", spec);
                    for p in set.ports() {
                        let ex = format!("~{p},");
                        prop_assert!(spec.contains(&ex), "{} lacks ~{}", spec, p);
                    }
                }
            }
            prop_assert_eq!(host_rules, 4);
            prop_assert_eq!(box_rules, 4);
        }
    }
}
