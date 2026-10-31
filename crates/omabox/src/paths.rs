//! Where omabox keeps things, as bin/omabox's globals name them (#324 wave 5a): box HOMEs, saves,
//! the tools in the checkout and where a box sees them.

use crate::cat;
use crate::shell;

/// `${VAR:-DEFAULT}`.
fn env_or(name: &str, default: Vec<u8>) -> Vec<u8> {
    shell::env(name)
        .filter(|v| !v.is_empty())
        .unwrap_or(default)
}

/// HOMES: box HOMEs, on disk rather than the runtime dir's tmpfs (finding 50).
pub fn homes() -> Vec<u8> {
    let h = shell::env_or_empty("HOME");
    cat!(env_or("XDG_CACHE_HOME", cat!(h, "/.cache")), "/omabox")
}

/// SAVES: box HOMEs kept on purpose (`omabox save`).
pub fn saves() -> Vec<u8> {
    let h = shell::env_or_empty("HOME");
    cat!(
        env_or("XDG_DATA_HOME", cat!(h, "/.local/share")),
        "/omabox/saves"
    )
}

/// AQ_USER: the user's private aquamarine build.
pub fn aq_user() -> Vec<u8> {
    let h = shell::env_or_empty("HOME");
    cat!(
        env_or("XDG_DATA_HOME", cat!(h, "/.local/share")),
        "/omabox/aquamarine"
    )
}

/// A tool of the checkout's: `$ROOT/tools/NAME/omabox-NAME` (KEYBOARD, POINTER, STILL, ...).
pub fn tool(name: &str) -> Vec<u8> {
    let root = crate::root();
    cat!(shell::bytes(&root), "/tools/", name, "/omabox-", name)
}

/// `up --polkit`'s session shim (#258), preloaded in the box: `$ROOT/tools/polkit/omabox-polkit-session.so`.
pub fn polkit_session() -> Vec<u8> {
    cat!(tool("polkit"), "-session.so")
}

/// The checkout's root as bytes ($ROOT).
pub fn root() -> Vec<u8> {
    shell::bytes(&crate::root()).to_vec()
}

/// READY_TIMEOUT as given: `${OMABOX_READY_TIMEOUT:-30}`.
pub fn ready_timeout_raw() -> Vec<u8> {
    env_or("OMABOX_READY_TIMEOUT", b"30".to_vec())
}
