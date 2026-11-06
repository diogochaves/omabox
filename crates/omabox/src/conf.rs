//! The user's settings (~/.config/omabox/config), as bin/omabox's conf_get reads them.

use crate::cat;
use crate::exit::say;
use crate::shell;

/// `sed -n "s/^[[:space:]]*KEY[[:space:]]*=[[:space:]]*//p" CONFIG | tail -n 1 | sed 's/[[:space:]]*$//'`:
/// the config's path and its last KEY's value, blanks around taken off (empty when none).
pub fn conf_raw(key: &str) -> (Vec<u8>, Vec<u8>) {
    let config = cat!(shell::env_or_empty("HOME"), "/.config/omabox/config");
    let p = shell::path(&config);
    let mut v = Vec::new();
    if p.is_file() {
        // (sed's own words for a config it cannot read)
        let data = match std::fs::read(&p) {
            Ok(d) => d,
            Err(e) => {
                let _ = std::io::Write::write_all(
                    &mut std::io::stderr(),
                    &cat!(
                        "sed: can't read ",
                        config,
                        ": ",
                        crate::shots::why(&e),
                        "\n"
                    ),
                );
                Vec::new()
            }
        };
        let blank = |c: u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
        for l in data.split(|&c| c == b'\n') {
            let s = &l[l.iter().position(|&c| !blank(c)).unwrap_or(l.len())..];
            let Some(rest) = s.strip_prefix(key.as_bytes()) else {
                continue;
            };
            let rest = &rest[rest.iter().position(|&c| !blank(c)).unwrap_or(rest.len())..];
            let Some(rest) = rest.strip_prefix(b"=".as_slice()) else {
                continue;
            };
            v = rest[rest.iter().position(|&c| !blank(c)).unwrap_or(rest.len())..].to_vec();
        }
        while v.last().is_some_and(|&c| blank(c)) {
            v.pop();
        }
    }
    (config, v)
}

/// The config's path and why, when it is there (a file, or a link to one) but cannot be read: a
/// mode that does not let this user read it, say (finding 482).
pub fn unreadable() -> Option<(Vec<u8>, String)> {
    let config = cat!(shell::env_or_empty("HOME"), "/.config/omabox/config");
    let p = shell::path(&config);
    if !p.is_file() {
        return None;
    }
    match std::fs::File::open(&p) {
        Ok(_) => None,
        Err(e) => Some((config, crate::shots::why(&e))),
    }
}

/// conf_get KEY with KEY's check (conf_check) and default (conf_default): a value that does not
/// check out is said and ignored.
pub fn conf_get(key: &str, check: fn(&[u8]) -> Option<Vec<u8>>, default: &[u8]) -> Vec<u8> {
    let (config, v) = conf_raw(key);
    if !v.is_empty() {
        if let Some(c) = check(&v) {
            return c;
        }
        say(cat!(config, ": ignoring ", key, "=", v));
    }
    default.to_vec()
}

/// conf_get workspace: where interactive and peek windows open (finding 70), 9 by default.
pub fn workspace() -> Vec<u8> {
    conf_get("workspace", crate::deskwin::ws_check, b"9")
}

/// The settings `omabox config` shows and changes (CONF_KEYS), in its order. protect-ports (#355,
/// S10, finding 480) is the Rust half's alone: bash's CONF_KEYS lacks it.
pub const KEYS: &[&str] = &[
    "workspace",
    "confirm-close",
    "bar-icon",
    "gpu",
    "shot-fit",
    "protect-ports",
    "desktop",
];

/// conf_default KEY: None for a key there is not. protect-ports' is empty: no port protected.
pub fn default(key: &[u8]) -> Option<&'static [u8]> {
    Some(match key {
        b"workspace" => b"9",
        b"confirm-close" => b"off",
        b"bar-icon" => b"always",
        b"gpu" => b"auto",
        b"shot-fit" => b"off",
        b"protect-ports" => b"",
        b"desktop" => b"auto",
        _ => return None,
    })
}

/// conf_check KEY VALUE: VALUE in its normal form, None when it does not check out. protect-ports
/// as `up` reads it (net::protect_parse): a value written here is one every `up` takes, sorted.
pub fn check(key: &[u8], v: &[u8]) -> Option<Vec<u8>> {
    match key {
        b"workspace" => crate::deskwin::ws_check(v),
        b"confirm-close" => confirm_check(v),
        b"bar-icon" => matches!(v, b"auto" | b"always").then(|| v.to_vec()),
        b"gpu" => crate::gpu::conf_gpu(v),
        b"shot-fit" => crate::shots::fit_check(v),
        b"protect-ports" => crate::net::protect_parse(v).map(|p| p.csv()),
        b"desktop" => matches!(v, b"auto" | b"omarchy" | b"plain").then(|| v.to_vec()),
        _ => None,
    }
}

/// conf_help KEY: what KEY takes, for a refusal.
pub fn help(key: &[u8]) -> &'static str {
    match key {
        b"workspace" => {
            "1-99, special (the scratchpad, SUPER+S) or special:NAME (special:omabox: a special workspace of omabox's own)"
        }
        b"confirm-close" => "on or off",
        b"bar-icon" => "always (the default) or auto (only while boxes exist)",
        b"gpu" => {
            "auto (the default: the first GPU), nvidia, amd, intel (or a driver: nouveau, amdgpu, radeon, i915, xe) or a PCI slot (0000:03:00.0); headless boxes only; omabox config gpu lists them"
        }
        b"shot-fit" => {
            "off (the default: full size) or the longest side in pixels, 10-99999, every shot is scaled down to, as with --fit: 1456 for 16:9 as Claude sees it"
        }
        b"protect-ports" => {
            "host ports no connected box reaches, PORT[,PORT...] each 1-65535, like 8080,5432; default: none"
        }
        b"desktop" => {
            "auto (the default: Omarchy's where Omarchy is installed, plain where it is not), omarchy or plain; up --desktop wins"
        }
        _ => "",
    }
}

/// confirm-close's values: on (on, true, yes, 1) or off (off, false, no, 0).
pub fn confirm_check(v: &[u8]) -> Option<Vec<u8>> {
    match v {
        b"on" | b"true" | b"yes" | b"1" => Some(b"on".to_vec()),
        b"off" | b"false" | b"no" | b"0" => Some(b"off".to_vec()),
        _ => None,
    }
}

/// conf_get KEY for any key of KEYS. protect-ports' bad value is not ignored as the others' are
/// (every `up` refuses it, S10 fails closed): said as up says it, and shown as it is.
pub fn get(key: &[u8]) -> Vec<u8> {
    let k = String::from_utf8_lossy(key);
    if key == b"protect-ports" {
        if let Some((config, why)) = unreadable() {
            say(crate::net::protect_unread(&config, &why));
            return Vec::new();
        }
        let (config, v) = conf_raw("protect-ports");
        if v.is_empty() {
            return v;
        }
        return match crate::net::protect_parse(&v) {
            Some(p) => p.csv(),
            None => {
                say(crate::net::protect_bad(&config, &v));
                v
            }
        };
    }
    let check = |v: &[u8]| check(key, v);
    let (config, v) = conf_raw(&k);
    if !v.is_empty() {
        if let Some(c) = check(&v) {
            return c;
        }
        say(cat!(config, ": ignoring ", key, "=", v));
    }
    default(key).unwrap_or_default().to_vec()
}

/// FILE through a link to its target (`[ ! -L FILE ] || FILE=$(readlink -f FILE)`), FILE itself
/// when it is no link. A link that cannot be followed (into a folder not there yet, a loop) is
/// refused, its words naming it (finding 482): bash's readlink failed there and set -e ended the
/// change; writing at the link's own path would replace the user's link with a plain file.
pub fn link_target(file: &[u8]) -> Result<Vec<u8>, Vec<u8>> {
    if !std::fs::symlink_metadata(shell::path(file)).is_ok_and(|m| m.file_type().is_symlink()) {
        return Ok(file.to_vec());
    }
    crate::repo::readlink_f(file).ok_or_else(|| {
        let to = std::fs::read_link(shell::path(file))
            .map(|t| shell::bytes(&t).to_vec())
            .unwrap_or_default();
        cat!(
            "cannot follow the link ",
            file,
            " (to ",
            to,
            "): a folder on its way is not there, or it loops; not touched"
        )
    })
}

/// conf_set KEY [VALUE]: KEY's line set to VALUE (none: removed, back to the default), the rest of
/// the file kept, comments included; through a link to its target (a config kept in a dotfiles repo
/// stays a link), one change at a time (the widget and the CLI may set at once). False, with its
/// words said, when it could not be written.
pub fn set(key: &[u8], value: Option<&[u8]>) -> bool {
    let config = cat!(shell::env_or_empty("HOME"), "/.config/omabox/config");
    let dir = cat!(shell::env_or_empty("HOME"), "/.config/omabox");
    let _ = std::fs::create_dir_all(shell::path(&dir));
    let file = match link_target(&config) {
        Ok(f) => f,
        Err(m) => {
            say(cat!("config: ", m));
            return false;
        }
    };
    // (flock -w 10 || true: a lock not had in 10 s does not stop the change)
    let _lock = crate::boxes::lock_file(
        &shell::path(&cat!(dir, "/.config.lock")),
        std::time::Duration::from_secs(10),
    );
    let Ok((mut tmp, tmp_path)) = shell::mktemp_in(&cat!(file, ".XXXXXX")) else {
        return false;
    };
    // (gone however this ends, INT, TERM or HUP too: bash left it beside the config)
    let t = tmp_path.clone();
    let _trap = crate::trap::on_exit(move || {
        let _ = std::fs::remove_file(shell::path(&t));
    });
    let mut body: Vec<u8> = Vec::new();
    if shell::path(&file).is_file() {
        // (grep -v "^[[:space:]]*KEY[[:space:]]*=": every other line, each ending in a newline)
        let blank = |c: u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
        let old = std::fs::read(shell::path(&file)).unwrap_or_default();
        let mut lines: Vec<&[u8]> = old.split(|&c| c == b'\n').collect();
        if old.last() == Some(&b'\n') {
            lines.pop();
        }
        for l in lines {
            let s = &l[l.iter().position(|&c| !blank(c)).unwrap_or(l.len())..];
            let ours = s.strip_prefix(key).is_some_and(|r| {
                r[r.iter().position(|&c| !blank(c)).unwrap_or(r.len())..].starts_with(b"=")
            });
            if !ours {
                body.extend_from_slice(l);
                body.push(b'\n');
            }
        }
    }
    if let Some(v) = value.filter(|v| !v.is_empty()) {
        body.extend_from_slice(&cat!(key, "=", v, "\n"));
    }
    let ok = std::io::Write::write_all(&mut tmp, &body).is_ok()
        && std::fs::rename(shell::path(&tmp_path), shell::path(&file)).is_ok();
    if !ok {
        let _ = std::fs::remove_file(shell::path(&tmp_path));
        say(cat!("config: could not write ", file));
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_checked_as_their_readers_read_them() {
        for k in KEYS {
            assert!(default(k.as_bytes()).is_some(), "{k}");
            assert!(!help(k.as_bytes()).is_empty(), "{k}");
        }
        assert_eq!(default(b"nope"), None);
        assert_eq!(
            check(b"protect-ports", b"8080, 5432,8080"),
            Some(b"5432,8080".to_vec())
        );
        for bad in ["", "0", "70000", "80,,81", "80,", "8080;rm", "80 81"] {
            assert_eq!(check(b"protect-ports", bad.as_bytes()), None, "{bad}");
        }
        assert_eq!(check(b"confirm-close", b"yes"), Some(b"on".to_vec()));
        assert_eq!(check(b"bar-icon", b"sometimes"), None);
        assert_eq!(check(b"shot-fit", b"0"), Some(b"off".to_vec()));
        assert_eq!(check(b"gpu", b"03:00.0"), Some(b"0000:03:00.0".to_vec()));
        assert_eq!(
            check(b"workspace", b"special"),
            Some(b"special:scratchpad".to_vec())
        );
    }
}
