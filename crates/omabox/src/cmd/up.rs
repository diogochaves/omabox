//! `omabox up [NAME] [OPTIONS]` (#324 wave 5a): start a box. Here the words are read and checked,
//! the name locked, and every refusal said before anything is made; cmd/up_box.rs makes the box. A
//! box that is up already is said as such (up_already), or refused when it lacks what was asked for.
//!
//! Rules held by construction (#355, SECURITY.md): the bwrap command is built by bwrap.rs (S1, S3,
//! S4), the network by net.rs (S5, S10), the HOME by seed.rs (S2), the host's Hyprland is reached
//! only through host::HostSilent (S8), and the started box's mounts are checked for the seat's paths
//! (up_box.rs's seat_check, S1).

use crate::args::{optarg, unknown_opt};
use crate::boxes::{self, box_alive, meta};
use crate::cat;
use crate::exit::{self, Ctx, Exit, Res, say};
use crate::install::{self, Aq};
use crate::jail::in_jail;
use crate::jailpath::{self, jail_path, jail_refuse};
use crate::json;
use crate::net::Ports;
use crate::procfs;
use crate::render::{self, RenderNode};
use crate::repo::{jail_root, norepo_note, readlink_f, refuse_src, repo_top};
use crate::shell;
use crate::upspec::{self, SeedRefused, refuse_dest};
use std::time::Duration;

const W_WHOLE: &str = "a folder this jail was given whole (its project, or one of its maps)";
const W_PLUGIN: &str = "a plugin's folder (with a manifest.json) this jail was given whole (its project, or one of its maps), nor inside one the box has";
const W_THEME: &str = "a folder in this jail's project nor one it was given whole";
const W_HYPR: &str = "in this jail's project nor in a folder it was given whole (or is not a Hyprland build's binary there)";
const W_QS: &str = "in this jail's project nor in a folder it was given whole (or is not a Quickshell build's binary there)";
const W_OMARCHY: &str =
    "in this jail's project nor a folder it was given whole (or is not an Omarchy tree there)";

/// The options up_already compares when given (finding 134).
const GIVEN: &[&str] = &[
    "size",
    "interactive",
    "plugin",
    "net",
    "allow",
    "addr",
    "from",
    "hyprland",
    "quickshell",
    "omarchy",
    "stock-bar",
    "desk-shell",
    "systemd",
    "audio",
    "polkit",
    "xwayland",
    "autoreload",
    "no-shell",
    "no-pointer",
    "dnd",
    "no-dock",
    "seed",
    "no-seed",
    "monitor",
    "theme-dir",
    "theme",
    "clean",
    "stock",
];

/// cmd_up's state: what the options set, then what up worked out from them.
#[derive(Default)]
pub struct Up {
    pub t0: Vec<u8>,
    pub name: Vec<u8>,
    pub json: bool,
    pub size: Vec<u8>,
    pub scale: Vec<u8>,
    pub ws: Vec<u8>,
    pub new: bool,
    pub confirm: Vec<u8>,
    pub confirm_from: &'static str,
    pub interactive: bool,
    pub plugin_args: Vec<Vec<u8>>,
    pub plugin_given: Vec<Vec<u8>>,
    pub overlays: Vec<Vec<u8>>,
    /// (src, dest) of each read-only mount, as robind_spec has them.
    pub robinds: Vec<(Vec<u8>, Vec<u8>)>,
    /// (src, rel) of each --seed.
    pub seeds: Vec<(Vec<u8>, Vec<u8>)>,
    pub no_seed: Vec<u8>,
    pub monitors: Vec<Vec<u8>>,
    pub theme_dirs: Vec<Vec<u8>>,
    pub theme_given: Vec<Vec<u8>>,
    pub theme: Vec<u8>,
    pub net: Vec<u8>,
    pub net_set: bool,
    pub allow: Vec<u8>,
    pub allow_ports: Ports,
    /// protect-ports (S10), read once.
    pub protect: Ports,
    pub net_addrs: Vec<Vec<u8>>,
    /// --env KEY=VAL: (KEY, VAL), as bwrap's --setenv takes them.
    pub envs: Vec<(Vec<u8>, Vec<u8>)>,
    pub idle: Vec<u8>,
    pub idle_how: &'static str,
    pub idle_s: i64,
    pub from: Vec<u8>,
    pub from_dir: Vec<u8>,
    pub hyprland: Vec<u8>,
    pub hypr_given: Vec<u8>,
    pub hypr_bind: bool,
    pub quickshell: Vec<u8>,
    pub qs_given: Vec<u8>,
    pub qs_bind: bool,
    pub omarchy: Vec<u8>,
    pub omarchy_given: Vec<u8>,
    pub omarchy_bind: bool,
    pub owner: Vec<u8>,
    pub agent: Vec<u8>,
    pub stock_bar: bool,
    /// --stock (#265): a new Omarchy user's desktop; --clean sets it too (a stock box with freezes).
    pub stock: bool,
    pub clean: bool,
    pub clean_keep: Vec<u8>,
    pub clean_lang: Vec<u8>,
    pub clean_zone: Vec<u8>,
    pub desk_shell: bool,
    pub desk: Vec<u8>,
    pub desk_shown: Vec<u8>,
    pub desk_ids: Vec<u8>,
    pub systemd: bool,
    pub audio: bool,
    pub polkit: bool,
    pub xwayland: bool,
    pub autoreload: bool,
    pub shell: bool,
    pub nopointer: bool,
    /// --dnd, --no-dock (#309): Do Not Disturb on, the dock off, in the box's shell once it is up.
    pub dnd: bool,
    pub nodock: bool,
    /// The options given, for up_already.
    pub given: Vec<&'static str>,
    /// The host paths --ro-bind, --seed, --overlay and --theme-dir name, as named: (verb, path, prep).
    pub named: Vec<(&'static str, Vec<u8>, &'static str)>,
    /// Each plugin the box mounts or links.
    pub plugins: Vec<crate::plugins::Plugin>,
    /// The plugins linked to at their own path (finding 241), by id.
    pub linked: Vec<Vec<u8>>,
    /// What up mounts by itself (the repo, its git dirs): left out, said, where a named one is refused.
    pub auto: Vec<Vec<u8>>,
    /// --ro-bind's words for bwrap, checked: (src, dest).
    pub ro_args: Vec<(Vec<u8>, Vec<u8>)>,
    /// A jailed caller's covers (#210): what goes over a hidden path (from the box dir), and the
    /// folders jail_verify checks.
    pub covers: Vec<(jailpath::CoverKind, Vec<u8>)>,
    pub cover_srcs: Vec<Vec<u8>>,
    pub render: Option<RenderNode>,
    pub aq: Aq,
    /// The lock on the name (one up or down at a time) and up --new's on the choice of a name.
    pub lock: Option<std::fs::File>,
    pub newlock: Option<std::fs::File>,
}

impl Up {
    pub fn has(&self, opt: &str) -> bool {
        self.given.contains(&opt)
    }

    pub fn clean_has(&self, k: &str) -> bool {
        upspec::clean_has(&self.clean_keep, k)
    }

    /// `${omarchy:-/usr/share/omarchy}`.
    pub fn tree(&self) -> Vec<u8> {
        if self.omarchy.is_empty() {
            b"/usr/share/omarchy".to_vec()
        } else {
            self.omarchy.clone()
        }
    }
}

/// `${G/#\~/$HOME}`.
fn tilde(g: &[u8]) -> Vec<u8> {
    match g.strip_prefix(b"~") {
        Some(rest) => cat!(shell::env_or_empty("HOME"), rest),
        None => g.to_vec(),
    }
}

/// `${V%%PAT*}`: what comes before PAT's first occurrence.
pub fn before<'a>(v: &'a [u8], pat: &[u8]) -> &'a [u8] {
    v.windows(pat.len())
        .position(|w| w == pat)
        .map_or(v, |i| &v[..i])
}

/// `${V%/*}`: the folder a path is in.
pub fn dir_of(v: &[u8]) -> Vec<u8> {
    match v.iter().rposition(|&c| c == b'/') {
        Some(i) => v[..i].to_vec(),
        None => v.to_vec(),
    }
}

pub fn pid_self() -> Vec<u8> {
    std::process::id().to_string().into_bytes()
}

/// Who this up is, as a down that came while it was starting is told by: its pid and start. A
/// broker's is the jailed caller's (#404, finding 579): its call began before the broker made this
/// process, and a user's down between the two came while it was starting.
fn up_origin() -> (Vec<u8>, Vec<u8>) {
    match crate::broker_side::caller() {
        Some((pid, start)) => (pid.clone(), start.clone()),
        None => (pid_self(), procfs::self_start().unwrap_or_default()),
    }
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let mut u = Up {
        // (a down started after this asks for the box gone, even before it has a lock)
        t0: up_origin().1,
        size: b"1920x1080".to_vec(),
        net: b"connected".to_vec(),
        idle: shell::env("OMABOX_IDLE")
            .filter(|v| !v.is_empty())
            .unwrap_or(b"2h".to_vec()),
        idle_how: "OMABOX_IDLE=",
        hypr_bind: true,
        qs_bind: true,
        omarchy_bind: true,
        shell: true,
        confirm_from: "flag",
        ..Up::default()
    };
    if parse(ctx, args, &mut u)? {
        return Ok(());
    }
    if !checks(ctx, &mut u)? {
        return Ok(());
    }
    crate::cmd::up_box::make(ctx, u)
}

/// Reads the words into U: true for --help (said).
fn parse(ctx: &mut Ctx, args: &[Vec<u8>], u: &mut Up) -> Res<bool> {
    let jailed = in_jail();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let v = args.get(i + 1);
        let val = v.cloned().unwrap_or_default();
        if let Some(o) = a.strip_prefix(b"--")
            && let Some(g) = GIVEN.iter().find(|g| g.as_bytes() == o)
            && !u.given.contains(g)
        {
            u.given.push(g);
        }
        let mut takes = true;
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, v)?;
                u.name = val;
            }
            b"--size" => {
                optarg(ctx, a, v)?;
                u.size = val;
            }
            b"--workspace" => {
                optarg(ctx, a, v)?;
                u.ws = crate::deskwin::ws_check(&val).ok_or_else(|| {
                    ctx.die(cat!(
                        "--workspace: 1-99, special (the scratchpad, SUPER+S) or special:NAME, got ",
                        val
                    ))
                })?;
            }
            b"--plugin" => {
                optarg(ctx, a, v)?;
                let g = if jailed {
                    jail_path(&val).ok_or_else(|| jail_refuse(ctx, "--plugin", &val, W_PLUGIN))?
                } else {
                    val.clone()
                };
                u.plugin_args.push(g);
                u.plugin_given.push(val);
            }
            b"--overlay" => {
                optarg(ctx, a, v)?;
                let g = if jailed {
                    jail_path(&val)
                        .filter(|g| shell::path(g).is_dir())
                        .and_then(|g| jail_root(&g))
                        .ok_or_else(|| jail_refuse(ctx, "--overlay", &val, W_WHOLE))?
                } else {
                    if !shell::path(&val).is_dir() {
                        return Err(ctx.die(cat!("--overlay: no such directory: ", val)));
                    }
                    readlink_f(&val).unwrap_or_default()
                };
                u.overlays.push(g);
                u.named.push(("overlay", val, "in"));
            }
            b"--ro-bind" => {
                optarg(ctx, a, v)?;
                let src_word = before(&val, b":/").to_vec();
                let Some(spec) = upspec::robind_spec(&val) else {
                    if jailed {
                        return Err(jail_refuse(ctx, "--ro-bind", &src_word, W_WHOLE));
                    }
                    return Err(ctx.die(cat!("--ro-bind: no such path: ", src_word)));
                };
                u.robinds.push(spec);
                u.named.push(("mount", tilde(&src_word), "into"));
            }
            b"--seed" => {
                optarg(ctx, a, v)?;
                match upspec::seed_spec(&val) {
                    Ok(spec) => u.seeds.push(spec),
                    Err(e) => {
                        if e == SeedRefused::Jail {
                            ctx.parsed();
                        }
                        return Err(ctx.refused());
                    }
                }
                u.named.push(("seed", tilde(before(&val, b":")), "into"));
            }
            b"--no-seed" => {
                optarg(ctx, a, v)?;
                u.no_seed = upspec::no_seed_spec(&val, &u.no_seed).map_err(|()| ctx.refused())?;
            }
            b"--monitor" => {
                optarg(ctx, a, v)?;
                if crate::mon::monitor_spec(&val).is_err() {
                    return Err(ctx.refused());
                }
                u.monitors.push(val);
            }
            b"--theme-dir" => {
                optarg(ctx, a, v)?;
                let g = if jailed {
                    jail_path(&val)
                        .filter(|g| shell::path(g).is_dir())
                        .and_then(|g| jailpath::jail_sees(&g))
                        .ok_or_else(|| jail_refuse(ctx, "--theme-dir", &val, W_THEME))?
                } else {
                    if !shell::path(&val).is_dir() {
                        return Err(ctx.die(cat!("--theme-dir: no such directory: ", val)));
                    }
                    readlink_f(&val).unwrap_or_default()
                };
                let base = base_of(&g);
                if base.starts_with(b".") {
                    return Err(ctx.die(cat!(
                        "--theme-dir: a theme's name (its dir's) cannot start with a dot: ",
                        val
                    )));
                }
                if u.theme_dirs.iter().any(|t| base_of(t) == base) {
                    return Err(ctx.die(cat!("--theme-dir: two themes named ", base)));
                }
                u.theme_dirs.push(g);
                u.named.push(("mount", tilde(&val), "into"));
                u.theme_given.push(val);
            }
            b"--theme" => {
                optarg(ctx, a, v)?;
                u.theme = val;
            }
            b"--net" => {
                optarg(ctx, a, v)?;
                u.net = val;
                u.net_set = true;
            }
            b"--allow" => {
                optarg(ctx, a, v)?;
                u.allow = val;
            }
            b"--addr" => {
                optarg(ctx, a, v)?;
                let g = upspec::box_addr(&val).ok_or_else(|| {
                    ctx.die(cat!(
                        "up: --addr takes an address in a documentation range, which no real host has (192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24 or 2001:db8::/32; one address, no /prefix), got ",
                        val
                    ))
                })?;
                u.net_addrs.push(g);
            }
            b"--env" => {
                optarg(ctx, a, v)?;
                if !shell::re_match("^[A-Za-z_][A-Za-z0-9_]*=", &val) {
                    return Err(ctx.die(cat!("up: --env wants KEY=VAL, got ", val)));
                }
                let k = before(&val, b"=").to_vec();
                if crate::cmd::run::env_reserved(&k) {
                    return Err(ctx.die(cat!(
                        "up: --env cannot set ",
                        k,
                        ": it is the box session's own"
                    )));
                }
                let vv = val[k.len() + 1..].to_vec();
                u.envs.push((k, vv));
            }
            b"--idle" => {
                optarg(ctx, a, v)?;
                u.idle = val;
                u.idle_how = "--idle ";
            }
            b"--from" => {
                optarg(ctx, a, v)?;
                u.from = val;
            }
            b"--hyprland" => {
                optarg(ctx, a, v)?;
                u.hyprland = if jailed {
                    jail_path(&val)
                        .and_then(|g| exit::muted(|| install::hypr_bin(ctx, &g).ok()))
                        .ok_or_else(|| jail_refuse(ctx, "--hyprland", &val, W_HYPR))?
                } else {
                    install::hypr_bin(ctx, &val)?
                };
                u.hypr_given = val;
            }
            b"--quickshell" => {
                optarg(ctx, a, v)?;
                u.quickshell = if jailed {
                    jail_path(&val)
                        .and_then(|g| exit::muted(|| install::qs_bin(ctx, &g).ok()))
                        .ok_or_else(|| jail_refuse(ctx, "--quickshell", &val, W_QS))?
                } else {
                    install::qs_bin(ctx, &val)?
                };
                u.qs_given = val;
            }
            b"--omarchy" => {
                optarg(ctx, a, v)?;
                u.omarchy = if jailed {
                    let view = jailpath::view();
                    jail_path(&val)
                        .and_then(|g| exit::muted(|| install::omarchy_tree(ctx, &g, view).ok()))
                        .ok_or_else(|| jail_refuse(ctx, "--omarchy", &val, W_OMARCHY))?
                } else {
                    install::omarchy_tree(ctx, &val, None)?
                };
                u.omarchy_given = val;
            }
            // internal: `run`'s throwaway box goes when this pid does
            b"--owner" => {
                optarg(ctx, a, v)?;
                if !shell::re_match("^[1-9][0-9]*$", &val) {
                    return Err(ctx.die(cat!("up: --owner is a pid, got ", val)));
                }
                u.owner = val;
            }
            b"--keep" => {
                optarg(ctx, a, v)?;
                u.clean_keep = upspec::clean_keep(&val).map_err(|()| ctx.refused())?;
            }
            _ => {
                takes = false;
                match a.as_slice() {
                    b"--json" => u.json = true,
                    b"--new" => u.new = true,
                    b"--confirm-close" => u.confirm = b"on".to_vec(),
                    b"--no-confirm-close" => u.confirm = b"off".to_vec(),
                    b"--interactive" => u.interactive = true,
                    // (#273, #287) -i is ignore-case in log and events: refused since 1.0.
                    b"-i" => {
                        return Err(ctx.usage_die(
                            "up -i is refused since 1.0: write up --interactive (-i is ignore-case in log and events)",
                        ));
                    }
                    b"--stock-bar" => u.stock_bar = true,
                    b"--clean" => u.clean = true,
                    b"--stock" => u.stock = true,
                    // The desk's config is outside any jail, and its files are the user's (#206).
                    b"--desk-shell" => {
                        if jailed {
                            ctx.parsed();
                            return Err(ctx.die(
                                "up: --desk-shell mirrors your desktop's shell config, not for an agent inside ai-jail",
                            ));
                        }
                        u.desk_shell = true;
                    }
                    b"--systemd" => u.systemd = true,
                    b"--audio" => u.audio = true,
                    b"--polkit" => u.polkit = true,
                    b"--xwayland" => u.xwayland = true,
                    b"--autoreload" => u.autoreload = true,
                    b"--no-shell" => u.shell = false,
                    b"--no-pointer" => u.nopointer = true,
                    b"--dnd" => u.dnd = true,
                    b"--no-dock" => u.nodock = true,
                    b"-h" | b"--help" => {
                        crate::args::usage("up")?;
                        return Ok(true);
                    }
                    _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "up", a)),
                    _ => {
                        if !u.name.is_empty() {
                            return Err(ctx.die(cat!(
                                "up: one box name, got ",
                                u.name,
                                " and ",
                                a
                            )));
                        }
                        u.name = a.clone();
                    }
                }
            }
        }
        i += if takes { 2 } else { 1 };
    }
    Ok(false)
}

/// `${G##*/}`.
fn base_of(g: &[u8]) -> Vec<u8> {
    match g.iter().rposition(|&c| c == b'/') {
        Some(i) => g[i + 1..].to_vec(),
        None => g.to_vec(),
    }
}

/// Everything cmd_up checks before it makes anything: false when the box is up already (said).
fn checks(ctx: &mut Ctx, u: &mut Up) -> Res<bool> {
    let jailed = in_jail();
    // --stock (#265, finding 613): Omarchy as a new user sees it. Omarchy's own bar on CLEAN_THEME
    // with its own wallpaper, English in UTC (finding 363), none of the desk's look (seed_home); the
    // clock, pointer, network and animations as in any box. --clean (#252, finding 357) is --stock
    // plus the freezes, a desktop where only what is tested moves: a sentinel wallpaper, the clock
    // frozen, no pointer, no network, animations off; --keep takes any of them back (and the bar, the
    // locale, the fonts), --theme and --net choose their own.
    if u.clean {
        u.stock = true;
    }
    if u.stock {
        if u.desk_shell {
            return Err(ctx.die(if u.clean {
                "up: --clean or --desk-shell, not both (--clean starts from Omarchy's bar; --keep bar keeps yours)"
            } else {
                "up: --stock or --desk-shell, not both (--stock is a new user's desktop, --desk-shell yours)"
            }));
        }
        if !u.clean_has("bar") {
            u.stock_bar = true;
        }
        if u.theme.is_empty() {
            u.theme = upspec::CLEAN_THEME.as_bytes().to_vec();
        }
        if !u.clean_has("locale") {
            u.clean_lang = upspec::clean_locale().as_bytes().to_vec();
            u.clean_zone = upspec::CLEAN_TZ.as_bytes().to_vec();
        }
    }
    if u.clean {
        if !u.clean_has("pointer") {
            u.nopointer = true;
        }
        if !u.net_set {
            u.net = b"isolated".to_vec();
        }
    } else if !u.clean_keep.is_empty() {
        return Err(
            ctx.die("up: --keep goes with --clean (what of the clean desktop to keep as it is)")
        );
    }
    // A plugin that cannot be mounted (not there, or a manifest id that is not a name: finding 175)
    // is refused before any lock or box dir is made. A jailed caller's (finding 99): a folder of the
    // jail's, whole, or one inside a folder the box has (finding 241), its manifest.json followed
    // only inside the jail's folders (#198); refused as written, however it fails.
    for (i, p) in u.plugin_args.clone().iter().enumerate() {
        if jailed {
            let ok = jail_path(&cat!(p, "/manifest.json"))
                .is_some_and(|m| shell::path(&m).is_file())
                && (jail_root(p).is_some() || upspec::own_path_visible(p, &u.robinds, &u.overlays));
            let r = if ok {
                crate::plugins::resolve_plugin(ctx, p, &jailpath::jail_read, true).ok()
            } else {
                None
            };
            match r {
                Some(pl) => u.plugins.push(pl),
                None => {
                    let given = u.plugin_given.get(i).cloned().unwrap_or_default();
                    return Err(jail_refuse(ctx, "--plugin", &given, W_PLUGIN));
                }
            }
        } else {
            u.plugins.push(crate::plugins::resolve_plugin(
                ctx,
                p,
                &jailpath::jail_read,
                false,
            )?);
        }
    }
    // --desk-shell (#206): every plugin of the desk's too, after the --plugin ones (which win for an
    // id both have); its shell.json and other files go in with seed_home.
    if u.desk_shell {
        if u.stock_bar {
            return Err(ctx.die("up: --desk-shell or --stock-bar, not both"));
        }
        // (the desk's config: not the command line's)
        u.desk = crate::plugins::desk_config(ctx)?;
        u.desk_shown = if shell::env("OMABOX_DESK_CONFIG").is_none_or(|v| v.is_empty()) {
            b"~/.config/omarchy".to_vec()
        } else {
            u.desk.clone()
        };
        for p in crate::plugins::desk_plugins(&u.desk, &u.plugins) {
            u.desk_ids.extend_from_slice(&cat!(p.id, " "));
            u.plugins.push(p);
        }
    }
    // A jailed caller's --hyprland and --omarchy, checked before anything reads in them: mounted as
    // --ro-bind's are, one of the jail's folders whole; a build inside the jail's project needs no
    // mount of its own (the project is in the box, and the path resolves there).
    if jailed {
        let in_top = |p: &[u8]| {
            repo_top().is_some_and(|top| {
                jail_root(&top).is_some()
                    && jailpath::jail_sees(p).is_some()
                    && p.starts_with(&cat!(top, "/"))
            })
        };
        if !u.hyprland.is_empty() && jail_root(&dir_of(&u.hyprland)).is_none() {
            if !in_top(&u.hyprland) {
                return Err(jail_refuse(ctx, "--hyprland", &u.hypr_given, W_HYPR));
            }
            u.hypr_bind = false;
        }
        if !u.quickshell.is_empty() && jail_root(&dir_of(&u.quickshell)).is_none() {
            if !in_top(&u.quickshell) {
                return Err(jail_refuse(ctx, "--quickshell", &u.qs_given, W_QS));
            }
            u.qs_bind = false;
        }
        if !u.omarchy.is_empty() && jail_root(&u.omarchy).is_none() {
            if !in_top(&u.omarchy) {
                return Err(jail_refuse(ctx, "--omarchy", &u.omarchy_given, W_OMARCHY));
            }
            u.omarchy_bind = false;
        }
    }
    // --theme NAME (finding 245): a theme there is, named as omarchy-theme-set names it.
    if !u.theme.is_empty() {
        u.theme = crate::plugins::theme_named(ctx, &u.theme, &u.tree(), &u.theme_dirs, &jail_path)?;
    }
    if u.new {
        // A name no box has (finding 73): box-1, box-2, ... The choice holds a lock until the box's
        // dir exists, so two at once never pick the same one. Printed on stdout.
        if !u.name.is_empty() {
            return Err(ctx.die(cat!(
                "up: --new picks the name itself, got ",
                u.name,
                " too"
            )));
        }
        let _ = std::fs::create_dir_all(shell::path(&ctx.boxes));
        u.newlock = boxes::lock_file(&ctx.in_boxes(".lock-new"), Duration::from_secs(60));
        if u.newlock.is_none() {
            ctx.parsed();
            return Err(ctx.die("another up --new is still choosing a name; try again"));
        }
        let mut n = 1u64;
        while std::fs::symlink_metadata(ctx.in_boxes(cat!("box-", n.to_string()))).is_ok() {
            n += 1;
        }
        u.name = cat!("box-", n.to_string());
    }
    crate::select::select_box(ctx, &u.name)?;
    if u.name.is_empty() {
        upspec::up_session_note(ctx);
    }
    // The box an agent session gets by default goes when the agent does (finding 93), or when idle.
    // Not an interactive box, a throwaway, or any box while OMABOX is set.
    if !u.interactive && u.owner.is_empty() && crate::select::session_box(ctx) {
        u.agent = crate::select::agent_proc()
            .map(|(p, s)| cat!(p, " ", s))
            .unwrap_or_default();
    }
    // A jailed agent's box goes with its jail, and sees no more than the jail does (finding 99).
    if jailed {
        if u.interactive {
            return Err(ctx.die(
                "up: a window on the user's desktop is not for an agent inside ai-jail (headless boxes only)",
            ));
        }
        if u.owner.is_empty() {
            u.agent = crate::jail::jail_id();
        }
        // Saves are the user's (finding 100).
        if !u.from.is_empty() {
            return Err(ctx.die(
                "up: saves are the user's (keyring secrets, apps signed in), not for an agent inside ai-jail",
            ));
        }
        if crate::jail::get_raw("net") != b"true" {
            if u.net_set && u.net != b"isolated" {
                return Err(ctx.die(
                    "up: this jail has no network, so its boxes have none either (--net isolated, the default here)",
                ));
            }
            if !u.allow.is_empty() {
                return Err(ctx.die(
                    "up: this jail has no network, so its boxes reach no host port (no --allow)",
                ));
            }
            u.net = b"isolated".to_vec();
        }
    }
    seeds_onto(ctx, u)?;
    // --dnd, --no-dock (#309): asked of the box's shell once it is up, so refused before it starts
    // when there is none to ask, or its Omarchy has none of it.
    for (on, opt) in [(u.dnd, "--dnd"), (u.nodock, "--no-dock")] {
        if on && !u.shell {
            return Err(ctx.usage_die(cat!(
                "up: ",
                opt,
                " asks the box's shell, and --no-shell starts none"
            )));
        }
    }
    if u.dnd && !crate::omshell::tree_has_dnd(&u.tree()) {
        return Err(ctx.die(cat!(
            "up: --dnd: the Omarchy at ",
            u.tree(),
            " has no Do Not Disturb to set (its notifications answer no setDnd)"
        )));
    }
    if u.nodock && !crate::omshell::tree_has_dock(&u.tree()) {
        return Err(ctx.die(cat!(
            "up: --no-dock: the Omarchy at ",
            u.tree(),
            " has no dock (Omarchy 4.5 brought it)"
        )));
    }
    // --hyprland (finding 116): the build's folder, read-only at its own path, refused as --ro-bind's
    // are. Not one under /usr: the box has it.
    for (bin, bind, opt) in [
        (u.hyprland.clone(), u.hypr_bind, "--hyprland"),
        (u.quickshell.clone(), u.qs_bind, "--quickshell"),
    ] {
        if opt == "--quickshell" && !u.quickshell.is_empty() && !u.shell {
            return Err(
                ctx.usage_die("up: --quickshell runs the box's shell, and --no-shell starts none")
            );
        }
        if bin.is_empty() || !bind || bin.starts_with(b"/usr/") {
            continue;
        }
        let d = dir_of(&bin);
        if let Some(why) = refuse_src(&d) {
            return Err(ctx.die(cat!(
                "up: ",
                opt,
                ": refusing to mount ",
                d,
                " (the binary's folder) into a box: ",
                why,
                "; put the build in a folder of its own"
            )));
        }
        if let Some(why) = refuse_dest(&d) {
            return Err(ctx.die(cat!(
                "up: ",
                opt,
                ": refusing to mount ",
                d,
                " (the binary's folder) at its own path in a box: ",
                why
            )));
        }
        u.robinds.push((d.clone(), d));
    }
    // --omarchy (finding 135): the tree, read-only at its own path.
    if !u.omarchy.is_empty() && u.omarchy_bind {
        u.robinds.push((u.omarchy.clone(), u.omarchy.clone()));
    }
    // (bash's `read -r size scale < <(split_scale ...)` said the refusal and went on with no size,
    // which parse_mode refused again: one refusal here, finding 456)
    (u.size, u.scale) = crate::mon::split_scale(ctx, "--size", &u.size)?;
    if u.interactive && u.scale != b"1" {
        return Err(ctx.die(
            "up: an interactive box's screen is its window, at your desktop's scale: --size takes no scale there",
        ));
    }
    u.size = crate::mon::parse_mode(ctx, &u.size)?;
    u.idle_s = match upspec::idle_duration(&u.idle, u.idle_how) {
        Ok(s) => s,
        Err(2) => return Err(Exit(ctx.die_rc.unwrap_or(2))),
        Err(_) => {
            return Err(ctx.die(cat!(
                "up: ",
                u.idle_how.trim_end(),
                " takes a duration like 30m, 2h or 90s, or 0 (never), got ",
                u.idle
            )));
        }
    };
    if u.interactive {
        u.idle_s = 0;
    }
    // (a bad $OMABOX_READY_TIMEOUT is said now, before any box starts)
    crate::ready::ready_timeout(ctx)?;
    if !u.interactive && !(u.ws.is_empty() && u.confirm.is_empty()) {
        return Err(ctx.die(
            "up: --workspace and --confirm-close are for --interactive (peek takes --workspace)",
        ));
    }
    // Settings (finding 70): the flags, else ~/.config/omabox/config.
    u.confirm_from = "flag";
    if u.ws.is_empty() {
        u.ws = crate::conf::workspace();
    }
    if u.interactive {
        crate::cmd::peek::ws_note(&u.ws);
    }
    if u.confirm.is_empty() {
        u.confirm = confirm_conf();
        u.confirm_from = "config";
    }
    // host: connected's name from before every box had a network of its own (finding 89).
    match u.net.as_slice() {
        b"host" | b"connected" => u.net = b"connected".to_vec(),
        b"isolated" => {}
        _ => return Err(ctx.die("up: --net is connected (the default) or isolated")),
    }
    if !u.allow.is_empty() && u.net != b"isolated" {
        return Err(ctx.die("up: --allow needs --net isolated"));
    }
    // --addr (#229): only on an isolated box's loopback, which no other network shares.
    if !u.net_addrs.is_empty() && u.net != b"isolated" {
        return Err(ctx.die("up: --addr needs --net isolated"));
    }
    u.net_addrs.sort();
    u.net_addrs.dedup();
    if u.net == b"connected" && std::path::Path::new("/opt/omabox/share/session.sh").is_file() {
        u.net = b"none".to_vec();
    }
    if !u.allow.is_empty() && !shell::re_match("^[0-9]+(,[0-9]+)*$", &u.allow) {
        return Err(ctx.die("up: --allow takes ports: 8081,8082"));
    }
    u.allow_ports = Ports::parse(&u.allow).map_err(|()| {
        ctx.die(cat!(
            "up: --allow takes ports from 1 to 65535, got ",
            u.allow
        ))
    })?;
    u.allow = u.allow_ports.csv();
    // Protected ports (#355, S10): never an isolated box's --allow either.
    u.protect = crate::net::conf_protect(ctx)?;
    let protect = u.protect.clone();
    if let Some(p) = u
        .allow_ports
        .ports()
        .iter()
        .find(|p| protect.ports().contains(p))
    {
        return Err(ctx.die(cat!(
            "up: --allow ",
            p.to_string(),
            ": a protected port (protect-ports in ~/.config/omabox/config), which no box reaches"
        )));
    }
    // The save's dir, resolved once (finding 163).
    if !u.from.is_empty() {
        u.from_dir = upspec::save_dir(ctx, &u.from)?;
        let fd = cat!(u.from_dir, "/save.json");
        for (key, what, theirs) in [
            ("hyprland", "was made in a box on ", "this one runs "),
            (
                "quickshell",
                "was made in a box whose shell ran ",
                "this one's runs ",
            ),
        ] {
            let saved_on = json::field(&shell::path(&fd), key).0;
            let (ours, none) = if key == "hyprland" {
                (&u.hyprland, "the installed Hyprland")
            } else {
                (&u.quickshell, "the installed Quickshell")
            };
            if saved_on != *ours {
                let s = |v: &[u8]| {
                    if v.is_empty() {
                        none.as_bytes().to_vec()
                    } else {
                        v.to_vec()
                    }
                };
                say(cat!(
                    "note: save '",
                    u.from,
                    "' ",
                    what,
                    s(&saved_on),
                    ", ",
                    theirs,
                    s(ours)
                ));
            }
        }
    }
    // Nor does a box behind pasta start from a process with no_new_privs: bwrap cannot write its uid
    // map there (finding 89). A connected box falls back to none; an isolated one is refused. A box
    // that is up already is still found up (without the lock below).
    if u.net != b"none" && !u.interactive && no_new_privs() {
        if box_alive(ctx)? {
            up_already(ctx, u)?;
            return Ok(false);
        }
        if u.net != b"connected" {
            return Err(ctx.die(
                "an isolated box cannot start from a no_new_privs process like this one (an agent's sandbox, a systemd unit with NoNewPrivileges=, a box's own session): its bwrap cannot write the uid map behind pasta",
            ));
        }
        u.net = b"none".to_vec();
        say(cat!(
            "box '",
            ctx.name,
            "' has no network: pasta's cannot start from a no_new_privs process like this one (an agent's sandbox, a systemd unit with NoNewPrivileges=); it has only a loopback of its own"
        ));
    }
    // Every top-level box's network is pasta's (finding 89). Before check_install.
    if !crate::exec::executable_on_path(b"pasta") {
        return Err(ctx.die("a box's network needs pasta: sudo pacman -S passt"));
    }
    if !u.net_addrs.is_empty() && !crate::exec::executable_on_path(b"ip") {
        return Err(ctx.die("up: --addr needs ip: sudo pacman -S iproute2"));
    }
    // A protected port's rule is nft's (#355), which the box's start runs with its own PATH, not the
    // caller's (#364): one only on the caller's would leave it "bwrap did not start" after 10 s.
    if u.net == b"connected"
        && !protect.is_empty()
        && !crate::exec::executable_in(crate::cmd::up_box::LAUNCH_PATH, b"nft")
    {
        return Err(ctx.die(cat!(
            "up: protect-ports (~/.config/omabox/config) needs nft in ",
            crate::cmd::up_box::LAUNCH_PATH,
            " (where a box's start looks for it) to keep a box off them: sudo pacman -S nftables"
        )));
    }
    // A sandbox that refuses new namespaces (ai-jail's seccomp filter) cannot start a box.
    if !jailed && !unshare_ok() {
        return Err(ctx.die(
            "cannot start a box here: this sandbox refuses new namespaces. Inside ai-jail, omabox goes through the broker: run 'omabox broker on' outside the jail and add the lines it prints to ~/.ai-jail",
        ));
    }
    if u.net == b"connected" && !is_char("/dev/net/tun") {
        return Err(ctx.die(
            "a box's network needs /dev/net/tun (the tun kernel module); --net isolated does without, and has no internet",
        ));
    }
    // The render node, resolved once, before anything is made (finding 163).
    let render = render::render_node(ctx)?;
    u.aq = install::check_install(
        ctx,
        &render,
        (!u.hyprland.is_empty()).then_some(u.hyprland.as_slice()),
    )?;
    if !u.quickshell.is_empty() {
        install::qs_check(ctx, &u.quickshell)?;
    }
    // A box starts from the user's Omarchy look, which Omarchy makes at their first login (finding
    // 130). Said before anything is made. With --theme the box starts on that one instead.
    let home = shell::env_or_empty("HOME");
    if u.theme.is_empty()
        && !shell::path(&cat!(home, "/.local/state/omarchy/current/theme")).is_dir()
    {
        return Err(ctx.die(cat!(
            "Omarchy has not set a theme for you yet (",
            home,
            "/.local/state/omarchy/current/theme), and a box starts from your Omarchy look: log into Omarchy once, or set one: omarchy-theme-set \"Tokyo Night\" (or up --theme NAME)"
        )));
    }
    // More monitors (#122): an interactive box's are windows (#123), which need aquamarine's fix and
    // omabox's build (each window a class of its own, kept off your workspace: finding 429).
    if !u.monitors.is_empty() && u.interactive {
        if !u.aq.fixed {
            return Err(ctx.die(cat!(
                "up: --monitor on an interactive box opens a window per monitor, which needs ",
                u.aq.lacks(),
                ": ",
                install::AQ_HOWTO
            )));
        }
        if !crate::exit::contains(&u.aq.ver, b"+appid") {
            return Err(ctx.die(cat!(
                "up: --monitor on an interactive box needs omabox's aquamarine build, which gives each monitor's window a class of its own to keep it off your workspace (boxes use ",
                u.aq.desc(),
                "): omabox setup --aquamarine"
            )));
        }
    }
    // What needs aquamarine's fix (finding 125): a headless box on an NVIDIA render node and
    // confirm-close asked for on `up` (from the settings it is turned off for the box, below).
    if !u.aq.fixed {
        if !u.interactive {
            install::nvidia_needs_fix_in(ctx, &u.aq, &render, b"/")?;
        }
        if u.confirm_from != "config" && u.confirm == b"on" {
            return Err(ctx.die(cat!(
                "--confirm-close (the window it opens again) needs ",
                u.aq.lacks(),
                ": ",
                install::AQ_HOWTO
            )));
        }
    }
    u.render = Some(render);
    // One `up` or `down` per name at a time.
    let _ = std::fs::create_dir_all(shell::path(&ctx.boxes));
    u.lock = boxes::lock_file(
        &ctx.in_boxes(cat!(".lock-", ctx.name)),
        Duration::from_secs(60),
    );
    if u.lock.is_none() {
        return Err(ctx.die(cat!(
            "box '",
            ctx.name,
            "' is busy (another up or down); try again"
        )));
    }
    // A jailed caller's box is checked again under the lock: select_box passed a name whose up (the
    // user's, or another jail's) had not written its box.json yet; that up has finished since, and
    // its box is not this caller's to be told about.
    if crate::jail::in_jail()
        && ctx.at("box.json").is_file()
        && meta(&ctx.d, "jail").0 != crate::jail::jail_id()
    {
        return Err(ctx.die(cat!(
            "box '",
            ctx.name,
            "' is not this jail's: an agent inside ai-jail drives only the boxes it started"
        )));
    }
    // A down started since this up was, before it held the lock, found no box and left its start
    // (finding 147): the box it asked to have gone is this one. A jailed down's (its jail on a
    // second line) only for an up of that jail: a jail does not cancel the user's up.
    let mpath = ctx.in_boxes(cat!(".down-", ctx.name));
    let dmark = std::fs::read(&mpath).unwrap_or_default();
    let (dline, djail) = down_marker(&dmark);
    let mine = djail.is_none_or(|j| crate::jail::in_jail() && j == crate::jail::jail_id());
    // (a broker's caller's pid is one of this process's pid namespace: SO_PEERCRED's, as the kernel
    // hands it to the listener; it was made before this process, so its made_us holds for it too)
    let me = UpSelf {
        pid: num(&up_origin().0).unwrap_or_else(|| i64::from(std::process::id())),
        pidns: procfs::pidns(b"self"),
        call_us: num(&crate::call_us()),
        pid_max: procfs::pid_max(),
        made_us: crate::made_us(),
        timens: procfs::timens(),
    };
    if let (true, Some(d), Some(t0)) = (mine, DownLine::read(&dline), num(&u.t0))
        && down_came(&d, t0, &me, marker_us(&mpath))
    {
        return Err(ctx.die(cat!(
            "up: cancelled: omabox down ",
            ctx.name,
            " came while it was starting"
        )));
    }
    // No down can come first here (this holds the lock): only a box that just died starts again.
    if box_alive(ctx)? && boxes::take_over(ctx, true)? {
        up_already(ctx, u)?;
        return Ok(false);
    }
    if u.systemd && !user_manager_ok() {
        return Err(ctx.die(
            "--systemd needs your systemd user manager (systemctl --user) to delegate the box a cgroup",
        ));
    }
    if u.audio
        && let Err(missing) = install::audio_packages()
    {
        return Err(ctx.die(cat!(
            "up --audio needs ",
            missing,
            " (sudo pacman -S pipewire wireplumber pipewire-pulse libpulse)"
        )));
    }
    if u.polkit
        && let Err(missing) = install::polkit_ready()
    {
        return Err(ctx.die(cat!("up --polkit needs ", missing)));
    }
    for p in &u.plugins {
        if let Some(why) = refuse_src(&p.dir) {
            return Err(ctx.die(cat!(
                ctx.label(),
                "refusing to mount plugin ",
                p.dir,
                " into a box: ",
                why
            )));
        }
    }
    mounts(ctx, u)?;
    Ok(true)
}

/// A down marker's first line (down.rs's down_mark): its start in clock ticks, then, from a Rust
/// down (finding 513), the pid its pid namespace made last as it wrote the marker, that namespace
/// and its CALL_US, and (#404) CLOCK_BOOTTIME in µs as it wrote the marker and the time namespace
/// that was read in. Bash's down writes the start alone.
#[derive(Debug, Default, PartialEq)]
struct DownLine {
    dt: i64,
    last_pid: Option<i64>,
    pidns: Option<Vec<u8>>,
    call_us: Option<i64>,
    boot_us: Option<i64>,
    timens: Option<Vec<u8>>,
}

impl DownLine {
    /// None when its start is not a number (`[[ $dt =~ ^[0-9]+$ ]]`).
    fn read(line: &[u8]) -> Option<DownLine> {
        let w = shell::words(line);
        let dt = num(w.first()?)?;
        let field = |i: usize| w.get(i).filter(|v| v.as_slice() != b"-");
        Some(DownLine {
            dt,
            last_pid: field(1).and_then(|v| num(v)),
            pidns: field(2).cloned(),
            call_us: field(3).and_then(|v| num(v)),
            boot_us: field(4).and_then(|v| num(v)),
            timens: field(5).cloned(),
        })
    }
}

/// This up, as down_came compares it with a down: its pid, pid namespace and CALL_US, where its
/// namespace's pids wrap, and when it was made at the latest (CLOCK_BOOTTIME, µs, procfs::made_us)
/// in which time namespace.
struct UpSelf {
    pid: i64,
    pidns: Option<Vec<u8>>,
    call_us: Option<i64>,
    pid_max: i64,
    made_us: Option<i64>,
    timens: Option<Vec<u8>>,
}

/// Whether the down whose marker is D came while an up started at tick T0 was starting (finding
/// 147). Starts are clock ticks (10 ms): a later one came, an earlier one did not. In a tie, the
/// down came only if it was not over when this up began (finding 506): this up's pid was made by the
/// time the down wrote its marker, at or below the last pid made then, in the same pid namespace
/// (pids are made in order; taken round pid_max, where they wrap, as the two are 10 ms apart at most).
/// That is the order itself, where the times compared before were each taken a while after a
/// process began: an up started first but slower to its first line than a whole down (a few ms) was
/// taken for one started after it, and `up B & down B` left the box up 14 times in 15 (finding
/// 513, #380). A down of another pid namespace (one run in a container of its own: a jailed
/// caller's commands run in the broker's, the user's) has pids of another order: this up came
/// first if it was made, at the latest (procfs::made_us), by the time on CLOCK_BOOTTIME the down
/// wrote its marker at, read in the same time namespace (#404; the calls' times compared before
/// were the fault above, and a wall-clock step could flip them). Without those, the calls' times;
/// one from bash (its start alone) by when its marker was written against this call's time; with
/// none of them, it came (bash's `dt >= t0`).
fn down_came(d: &DownLine, t0: i64, me: &UpSelf, marker_us: Option<i64>) -> bool {
    match d.dt.cmp(&t0) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => {
            if let (Some(last), Some(dns), Some(ns)) = (d.last_pid, &d.pidns, &me.pidns)
                && dns == ns
            {
                let max = me.pid_max.max(2);
                return (last - me.pid).rem_euclid(max) < max / 2;
            }
            if let (Some(db), Some(dtn), Some(made), Some(tn)) =
                (d.boot_us, &d.timens, me.made_us, &me.timens)
                && dtn == tn
            {
                return made <= db;
            }
            match (d.call_us, marker_us, me.call_us) {
                (Some(dc), _, Some(c)) => dc >= c,
                (None, Some(m), Some(c)) => m >= c,
                _ => true,
            }
        }
    }
}

/// When a marker was written, in µs of the wall clock (as CALL_US is).
fn marker_us(p: &std::path::Path) -> Option<i64> {
    let t = std::fs::metadata(p).ok()?.modified().ok()?;
    let d = t.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(d.as_micros()).ok()
}

/// A `.down-NAME` marker: the down's start, and the jail of a jailed caller's down (its second line).
fn down_marker(v: &[u8]) -> (Vec<u8>, Option<Vec<u8>>) {
    let t = shell::strip_nl(v);
    match t.iter().position(|&c| c == b'\n') {
        Some(i) => (t[..i].to_vec(), Some(t[i + 1..].to_vec())),
        None => (t, None),
    }
}

/// `[[ $dt =~ ^[0-9]+$ ]]` and its value.
fn num(v: &[u8]) -> Option<i64> {
    if v.is_empty() || !v.iter().all(u8::is_ascii_digit) {
        return None;
    }
    shell::dec(v)
}

/// conf_get confirm-close: on or off (on|true|yes|1, off|false|no|0), off by default.
pub fn confirm_conf() -> Vec<u8> {
    crate::conf::conf_get(
        "confirm-close",
        |v| match v {
            b"on" | b"true" | b"yes" | b"1" => Some(b"on".to_vec()),
            b"off" | b"false" | b"no" | b"0" => Some(b"off".to_vec()),
            _ => None,
        },
        b"off",
    )
}

/// `grep -q '^NoNewPrivs:[[:space:]]*1$' /proc/self/status`.
pub(crate) fn no_new_privs() -> bool {
    procfs::status(b"self", "NoNewPrivs", 2).is_some_and(|v| v == b"1")
}

/// `unshare -U true`: whether this process may make a user namespace (a child tries; nothing of
/// this process changes).
pub(crate) fn unshare_ok() -> bool {
    let mut c = crate::sig::command("unshare");
    c.args(["-U", "true"])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    crate::sig::run_status(c).is_ok_and(|rc| rc == 0)
}

pub(crate) fn is_char(p: &str) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(p).is_ok_and(|m| m.file_type().is_char_device())
}

/// `systemctl --user is-system-running` exits 0, or says degraded.
fn user_manager_ok() -> bool {
    let mut c = crate::sig::command("systemctl");
    c.args(["--user", "is-system-running"])
        .stdin(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::null());
    match crate::sig::run_output(c) {
        Ok((0, _)) => true,
        Ok((_, out)) => shell::strip_nl(&out) == b"degraded",
        Err(_) => false,
    }
}

/// A --seed onto a --plugin's or --theme-dir's folder in the box HOME (#199): refused before
/// anything is made, naming the plugin or theme and DEST as the box HOME has it.
fn seeds_onto(ctx: &Ctx, u: &Up) -> Res<()> {
    if u.seeds.is_empty() {
        return Ok(());
    }
    let mut onto: Vec<crate::seed::Place> = Vec::new();
    for p in &u.plugins {
        onto.push(crate::seed::Place {
            at: cat!(".config/omarchy/plugins/", p.id),
            kind: "plugin",
            name: p.id.clone(),
        });
    }
    for g in &u.theme_dirs {
        let b = base_of(g);
        onto.push(crate::seed::Place {
            at: cat!(".config/omarchy/themes/", b),
            kind: "theme",
            name: b,
        });
    }
    for (src, rel) in &u.seeds {
        let Some((place, how)) = crate::seed::seed_blocked(src, rel, &onto, &jail_path) else {
            continue;
        };
        let (at, kind, what, inside) = (&place.at, place.kind, &place.name, how == "in");
        let opt = if kind == "theme" {
            "--theme-dir"
        } else {
            "--plugin"
        };
        let how = if inside {
            cat!("is inside ", kind, " ", what, " (", opt, ")")
        } else {
            cat!(
                "would write over ",
                kind,
                " ",
                what,
                " (",
                opt,
                ", ~/",
                at,
                ")"
            )
        };
        let tail = if kind == "theme" {
            ""
        } else {
            " (or go under its mount, unseen)"
        };
        return Err(ctx.die(cat!(
            "--seed ~/",
            rel,
            " ",
            how,
            ": the box has the ",
            kind,
            " from its folder, and the seed would replace it with a folder holding only what is seeded",
            tail,
            ". Write the file in the ",
            kind,
            "'s folder instead, or seed outside it"
        )));
    }
    Ok(())
}

/// Host code, read-only (at the same path unless DIR:DEST): the repo `up` runs from, its git dirs,
/// mise's toolchains, ~/.config/omabox/ro-bind, --ro-bind, --theme-dir. Checked before anything is
/// created: what the user named is refused loudly, what omabox adds by itself is skipped, said.
fn mounts(ctx: &Ctx, u: &mut Up) -> Res<()> {
    let jailed = in_jail();
    let home = shell::env_or_empty("HOME");
    if let Some(top) = repo_top() {
        u.auto.push(top.clone());
        if let Some(why) = refuse_src(&top) {
            say(cat!("not mounting the repo ", top, ": ", why));
        } else {
            u.robinds.insert(0, (top.clone(), top.clone()));
            // A linked worktree's git dir (finding 250), before the repo: mounted after, one that held
            // it would hide it.
            for gd in install::repo_git_dirs(&top, jailpath::view()) {
                if let Some(why) = refuse_src(&gd).or_else(|| refuse_dest(&gd)) {
                    say(cat!(
                        "not mounting ",
                        gd,
                        ", the git dir of ",
                        top,
                        ": ",
                        why
                    ));
                } else {
                    u.robinds.insert(0, (gd.clone(), gd.clone()));
                    u.auto.push(gd);
                }
            }
        }
    } else if let Some(n) = norepo_note("up") {
        say(n);
    }
    // Language toolchains mise installed: interpreters and compilers, no secrets (finding 47). A
    // jailed caller's: holding nothing the jail hides, where nothing hidden can move to (#210).
    let mise = cat!(
        shell::env("MISE_DATA_DIR")
            .filter(|v| !v.is_empty())
            .unwrap_or(cat!(home, "/.local/share/mise")),
        "/installs"
    );
    if shell::path(&mise).is_dir()
        && let Some(mi) = readlink_f(&mise)
    {
        let hidden = jailed
            && !(jailpath::jail_sees(&mi).is_some()
                && !jailpath::jail_masked(&mi)
                && !jailpath::jail_deep(&mi));
        if !hidden {
            u.robinds.push((mi.clone(), mi));
        }
    }
    let conf = cat!(home, "/.config/omabox/ro-bind");
    if shell::path(&conf).is_file() && !jailed {
        let text = std::fs::read(shell::path(&conf)).unwrap_or_default();
        for raw in shell_lines(&text) {
            let Some(line) = robind_line(&raw) else {
                continue;
            };
            if let Some(spec) = upspec::robind_spec(&line) {
                u.robinds.push(spec);
                u.named.push(("mount", tilde(before(&line, b":/")), "into"));
            } else {
                say(cat!(conf, ": skipping ", line, " (no such path)"));
            }
        }
    }
    // --theme-dir (#170): the dir, at its own path in the box, which seed_home links into the box
    // HOME's themes/: the repo has it there already, or a same-path mount.
    for (i, g) in u.theme_dirs.clone().iter().enumerate() {
        if upspec::own_path_visible(g, &u.robinds, &u.overlays) {
            continue;
        }
        if jailed && jail_root(g).is_none() {
            let given = u.theme_given.get(i).cloned().unwrap_or_default();
            return Err(jail_refuse(ctx, "--theme-dir", &given, W_THEME));
        }
        u.robinds.push((g.clone(), g.clone()));
    }
    // A path as named, too (finding 228): one inside ~/.config/omarchy or a secret store is refused
    // even when it is a link to somewhere that would be fine.
    for (verb, g, prep) in &u.named {
        if let Some(why) = refuse_src(g) {
            return Err(ctx.die(cat!(
                "refusing to ",
                verb,
                " ",
                g,
                " ",
                prep,
                " a box: ",
                why
            )));
        }
    }
    // A jailed caller's folders go in with what the jail hides in them hidden again (#210).
    for (src, dest) in u.robinds.clone() {
        if let Some(why) = refuse_src(&src) {
            return Err(ctx.die(cat!(
                ctx.label(),
                "refusing to mount ",
                src,
                " into a box: ",
                why
            )));
        }
        if let Some(why) = refuse_dest(&dest) {
            return Err(ctx.die(cat!(
                ctx.label(),
                "refusing to mount ",
                src,
                " at ",
                dest,
                " in a box: ",
                why
            )));
        }
        if jailed {
            match jailpath::jail_cover(&src, &dest) {
                Some(c) => {
                    if let Some(s) = c.src {
                        u.cover_srcs.push(s);
                    }
                    u.covers.extend(c.binds);
                }
                None => {
                    let why = "this jail hides paths in it, and the box hides them too only with /usr/bin/python3 (package python) to check it does";
                    // What up mounts by itself is left out, said; what the caller named is refused.
                    if u.auto.contains(&src) {
                        say(cat!("not mounting ", src, ": ", why));
                        continue;
                    }
                    return Err(ctx.die(cat!(
                        ctx.label(),
                        "refusing to mount ",
                        src,
                        " into a box: ",
                        why
                    )));
                }
            }
        }
        u.ro_args.push((src, dest));
    }
    // A plugin the box sees at its own path anyway is linked to it from the box HOME (finding 241).
    for p in &u.plugins {
        if upspec::own_path_visible(&p.dir, &u.robinds, &u.overlays) {
            u.linked.push(p.id.clone());
        }
    }
    for (src, _) in &u.seeds {
        if let Some(why) = refuse_src(src) {
            return Err(ctx.die(cat!("refusing to seed ", src, " into a box: ", why)));
        }
    }
    for p in &u.overlays {
        if let Some(why) = refuse_src(p).or_else(|| refuse_dest(p)) {
            return Err(ctx.die(cat!("refusing to overlay ", p, " in a box: ", why)));
        }
    }
    Ok(())
}

/// `while IFS= read -r line || [ -n "$line" ]`: the lines of TEXT, a last one without a newline too.
fn shell_lines(text: &[u8]) -> Vec<Vec<u8>> {
    let mut v: Vec<Vec<u8>> = text.split(|&c| c == b'\n').map(<[u8]>::to_vec).collect();
    if text.ends_with(b"\n") || text.is_empty() {
        v.pop();
    }
    v
}

/// One line of ~/.config/omabox/ro-bind: comments (the whole line, or after a blank) off, blanks
/// around it trimmed; None for none left.
fn robind_line(raw: &[u8]) -> Option<Vec<u8>> {
    let blank =
        |c: u8| c == b' ' || c == b'\t' || c == b'\n' || c == 0x0b || c == 0x0c || c == b'\r';
    // `${line%%[[:space:]]#*}`: from the first blank followed by a # on
    let mut line = raw.to_vec();
    if let Some(i) = line.windows(2).position(|w| blank(w[0]) && w[1] == b'#') {
        line.truncate(i);
    }
    if line.starts_with(b"#") {
        return None;
    }
    let s = line.iter().position(|&c| !blank(c)).unwrap_or(line.len());
    let e = line.iter().rposition(|&c| !blank(c)).map_or(s, |i| i + 1);
    let t = line[s..e.max(s)].to_vec();
    (!t.is_empty()).then_some(t)
}

/// What `up` says of a box that is up already, which counts as use. The options given that the box
/// does not have are refused (finding 134); only those given. Mounts, --env and --idle are not
/// compared.
pub fn up_already(ctx: &mut Ctx, u: &Up) -> Res<()> {
    crate::whose::mark_used(ctx);
    // (#309: asked of its shell; a box with none, or a shell that does not answer, is not on)
    let dnd_off = u.has("dnd")
        && crate::omshell::ipc(ctx, &[b"notifications", b"isDnd"]).as_deref()
            != Ok(b"on".as_slice());
    let dock_on =
        u.has("no-dock") && crate::omshell::dock_enabled(ctx) && meta(&ctx.d, "shell").0 == b"true";
    let ctx: &Ctx = ctx;
    let d = &ctx.d;
    let m = |k: &str| meta(d, k).0;
    let or = |v: Vec<u8>, none: &str| {
        if v.is_empty() {
            none.as_bytes().to_vec()
        } else {
            v
        }
    };
    let jqr = |prog: &str| -> Vec<u8> {
        crate::jq::Args::new(&["-r", prog])
            .arg(cat!(d, "/box.json"))
            .run_file(crate::jq::Err::Null)
            .text()
    };
    let jqc = |prog: &str| -> Vec<u8> {
        crate::jq::Args::new(&["-c", prog])
            .arg(cat!(d, "/box.json"))
            .run_file(crate::jq::Err::Null)
            .text()
    };
    let list_json = |v: &[Vec<u8>]| -> Vec<u8> {
        // (`printf '%s\n' ... | jq -R . | jq -sc .`: an empty list is [""])
        let items: Vec<String> = if v.is_empty() {
            vec![json::quote("")]
        } else {
            v.iter()
                .map(|x| json::quote(&String::from_utf8_lossy(x)))
                .collect()
        };
        format!("[{}]", items.join(",")).into_bytes()
    };
    let meta_scale = || or(m("scale"), "1");
    let mut diff: Vec<Vec<u8>> = Vec::new();
    if u.has("interactive") && m("mode") != b"interactive" {
        diff.push(cat!("--interactive (it is ", m("mode"), ")"));
    }
    if u.has("size") && !u.interactive && !(m("size") == u.size && meta_scale() == u.scale) {
        diff.push(cat!(
            "--size ",
            crate::mon::size_text(&u.size, &u.scale),
            " (it is ",
            crate::mon::size_text(&m("size"), &meta_scale()),
            ")"
        ));
    }
    if u.has("net") {
        if u.net == b"isolated" {
            if m("net") != b"isolated" {
                diff.push(cat!("--net isolated (it is ", m("net"), ")"));
            }
        } else if m("net") == b"isolated" {
            diff.push(b"--net connected (it is isolated)".to_vec());
        }
    }
    if u.has("allow") {
        let theirs = Ports::parse(&m("allow"))
            .map(|p| p.csv())
            .unwrap_or_default();
        if theirs != u.allow {
            diff.push(cat!(
                "--allow ",
                u.allow,
                " (it allows ",
                or(m("allow"), "none"),
                ")"
            ));
        }
    }
    if u.has("addr") && list_json(&u.net_addrs) != jqc(".addrs // []") {
        diff.push(cat!(
            "--addr ",
            u.net_addrs.join(&b' '),
            " (it has ",
            jqr(r#".addrs // [] | if length == 0 then "none" else join(" ") end"#),
            ")"
        ));
    }
    if u.has("systemd") && m("systemd") != b"true" {
        diff.push(b"--systemd (it has no user manager)".to_vec());
    }
    if u.has("audio") && m("audio") != b"true" {
        diff.push(b"--audio (it has no audio)".to_vec());
    }
    if u.has("polkit") && m("polkit") != b"true" {
        diff.push(b"--polkit (it has no polkit)".to_vec());
    }
    if u.has("stock-bar") && m("bar") != b"stock" {
        diff.push(b"--stock-bar (it has your bar)".to_vec());
    }
    // (a clean box is a stock one too)
    if u.has("stock") && jqr(".stock // (.clean != null)") != b"true" {
        diff.push(b"--stock (it has your look)".to_vec());
    }
    if u.has("clean") {
        if jqr(".clean != null") != b"true" {
            diff.push(b"--clean (it is not clean)".to_vec());
        } else if jqr(r#".clean.keep | join(",")"#) != u.clean_keep {
            diff.push(cat!(
                "--keep ",
                or(u.clean_keep.clone(), "nothing"),
                " (it keeps ",
                jqr(r#".clean.keep | if length == 0 then "nothing" else join(",") end"#),
                ")"
            ));
        }
    }
    if u.has("desk-shell") && m("bar") != b"desk" {
        let has = if m("bar") == b"stock" {
            "the stock bar"
        } else {
            "your bar with only its own plugins"
        };
        diff.push(cat!("--desk-shell (it has ", has, ")"));
    }
    if u.has("xwayland") && m("xwayland") != b"true" {
        diff.push(b"--xwayland (it has none)".to_vec());
    }
    if u.has("autoreload") && m("autoreload") != b"true" {
        diff.push(b"--autoreload (its autoreload is off)".to_vec());
    }
    if u.has("no-pointer") {
        let hidden = std::fs::read(ctx.at("pointer"))
            .is_ok_and(|t| t.split(|&c| c == b'\n').any(|l| l == b"hidden"));
        if !hidden {
            diff.push(b"--no-pointer (its pointer shows: omabox pointer hide)".to_vec());
        }
    }
    if dnd_off {
        diff.push(b"--dnd (its Do Not Disturb is off: omabox dnd on)".to_vec());
    }
    if dock_on {
        diff.push(b"--no-dock (its dock is on: omabox dock off)".to_vec());
    }
    if u.has("no-shell") && jqr(".shell | tostring") != b"false" {
        diff.push(b"--no-shell (it has the shell)".to_vec());
    }
    if u.has("omarchy") && m("omarchy") != u.omarchy {
        diff.push(cat!(
            "--omarchy ",
            u.tree(),
            " (it runs ",
            or(m("omarchy"), "/usr/share/omarchy"),
            ")"
        ));
    }
    if u.has("hyprland") && m("hyprland") != u.hyprland {
        diff.push(cat!(
            "--hyprland ",
            u.hyprland,
            " (it runs ",
            or(m("hyprland"), "the installed one"),
            ")"
        ));
    }
    if u.has("quickshell") && m("quickshell") != u.quickshell {
        diff.push(cat!(
            "--quickshell ",
            u.quickshell,
            " (its shell runs ",
            or(m("quickshell"), "the installed one"),
            ")"
        ));
    }
    if u.has("from") && m("from") != u.from {
        diff.push(cat!(
            "--from ",
            u.from,
            " (it started from ",
            or(m("from"), "no save"),
            ")"
        ));
    }
    if u.has("monitor") && list_json(&u.monitors) != jqc(".monitor_specs // []") {
        diff.push(cat!(
            "--monitor (it has ",
            jqr(r#".monitor_specs // [] | if length == 0 then "none" else join(", ") end"#),
            ")"
        ));
    }
    if u.has("seed") {
        let seeds: Vec<Vec<u8>> = u.seeds.iter().map(|(s, r)| cat!(s, "\t", r)).collect();
        if list_json(&seeds) != jqc(".seeds // []") {
            diff.push(b"--seed (files are seeded only when a box starts)".to_vec());
        }
    }
    if u.has("no-seed") && jqr(r#".no_seed // [] | join(",")"#) != u.no_seed {
        diff.push(cat!(
            "--no-seed ",
            u.no_seed,
            " (it started with ",
            jqr(
                r#".no_seed // [] | if length == 0 then "all of it" else "all but " + join(",") end"#
            ),
            "; files are seeded only when a box starts)"
        ));
    }
    if u.has("theme") && m("theme") != u.theme {
        diff.push(cat!(
            "--theme ",
            u.theme,
            " (it started on ",
            or(m("theme"), "your desktop's theme"),
            ")"
        ));
    }
    if u.has("theme-dir") && list_json(&u.theme_dirs) != jqc(".theme_dirs // []") {
        diff.push(cat!(
            "--theme-dir (it has ",
            jqr(r#".theme_dirs // [] | if length == 0 then "none" else join(", ") end"#),
            ")"
        ));
    }
    let ids = jqr(".plugins[]?");
    for p in &u.plugin_args {
        let id = crate::plugins::resolve_plugin(ctx, p, &jailpath::jail_read, false)
            .map(|p| p.id)
            .unwrap_or_default();
        if !ids.split(|&c| c == b'\n').any(|l| l == id.as_slice()) {
            // (#345: one can go into the box as it is)
            diff.push(cat!(
                "--plugin ",
                p,
                " (not mounted: omabox plugin add ",
                p,
                " -b ",
                ctx.name,
                " adds it)"
            ));
        }
    }
    if !diff.is_empty() {
        let list: Vec<Vec<u8>> = diff.iter().map(|x| cat!(" ", x)).collect();
        return Err(ctx.die(cat!(
            "box '",
            ctx.name,
            "' is already up, without what you asked for:",
            list.join(&b';'),
            ". omabox down ",
            ctx.name,
            " first, or omabox up --new for a box of your own"
        )));
    }
    say(cat!("box '", ctx.name, "' is already up"));
    if u.json {
        crate::exit::out(crate::ready::box_json(ctx, false, "", ""))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_down_over_before_the_up_is_not_one_that_came() {
        let bash = |dt| DownLine {
            dt,
            ..DownLine::default()
        };
        let me = |pid, call| UpSelf {
            pid,
            pidns: Some(b"pid:[1]".to_vec()),
            call_us: call,
            pid_max: 4_194_304,
            made_us: Some(5),
            timens: Some(b"time:[1]".to_vec()),
        };
        assert!(down_came(&bash(11), 10, &me(500, None), None));
        assert!(!down_came(&bash(9), 10, &me(500, Some(1)), Some(5)));
        // (a tie, a bash down's: the marker written after the up was called, or no times to tell)
        assert!(down_came(&bash(10), 10, &me(500, Some(1000)), Some(2000)));
        assert!(!down_came(&bash(10), 10, &me(500, Some(1000)), Some(999)));
        assert!(down_came(&bash(10), 10, &me(500, Some(1000)), None));
    }

    /// Finding 513 (#380): in a tie, the order the pids were made in, whatever the clocks say.
    #[test]
    fn a_down_still_going_when_the_up_began_came() {
        let ours = |last| DownLine {
            dt: 10,
            last_pid: Some(last),
            pidns: Some(b"pid:[1]".to_vec()),
            call_us: Some(900),
            ..DownLine::default()
        };
        let me = |pid, ns: &[u8]| UpSelf {
            pid,
            pidns: Some(ns.to_vec()),
            call_us: Some(1000),
            pid_max: 4_194_304,
            made_us: Some(5),
            timens: Some(b"time:[1]".to_vec()),
        };
        // `up B & down B`: the up made first, slower to its first line than the whole down: it came
        // (its call and its marker both before the up's call, which said otherwise)
        assert!(down_came(&ours(510), 10, &me(500, b"pid:[1]"), Some(950)));
        assert!(down_came(&ours(500), 10, &me(500, b"pid:[1]"), Some(950)));
        // `down B; up B`: the up made after the down's marker: it did not
        assert!(!down_came(&ours(499), 10, &me(500, b"pid:[1]"), Some(1100)));
        // pids that wrapped between the two
        assert!(down_came(&ours(3), 10, &me(4_194_300, b"pid:[1]"), None));
        assert!(!down_came(&ours(4_194_300), 10, &me(3, b"pid:[1]"), None));
        // another pid namespace's, from a down with no boot time: the calls' times, like with like
        assert!(!down_came(&ours(510), 10, &me(500, b"pid:[2]"), Some(2000)));
        let later = DownLine {
            call_us: Some(1000),
            ..ours(510)
        };
        assert!(down_came(&later, 10, &me(500, b"pid:[2]"), None));
        // ticks still decide when they differ
        assert!(!down_came(
            &DownLine { dt: 9, ..ours(510) },
            10,
            &me(500, b"pid:[1]"),
            None
        ));
    }

    /// #404: a down of another pid namespace, in a tie, by CLOCK_BOOTTIME: when this up was made at
    /// the latest against when the down wrote its marker, whatever the calls' times say.
    #[test]
    fn a_down_of_another_pid_namespace_by_boot_time() {
        let theirs = |boot, tns: &[u8]| DownLine {
            dt: 10,
            last_pid: Some(3),
            pidns: Some(b"pid:[2]".to_vec()),
            call_us: Some(900),
            boot_us: Some(boot),
            timens: Some(tns.to_vec()),
        };
        let me = |made| UpSelf {
            pid: 500,
            pidns: Some(b"pid:[1]".to_vec()),
            call_us: Some(1000),
            pid_max: 4_194_304,
            made_us: made,
            timens: Some(b"time:[1]".to_vec()),
        };
        // an up made first, slower to its first line than the down to its marker: it came (the
        // calls' times, 900 before 1000, said it was over)
        assert!(down_came(
            &theirs(7000, b"time:[1]"),
            10,
            &me(Some(6000)),
            None
        ));
        assert!(down_came(
            &theirs(7000, b"time:[1]"),
            10,
            &me(Some(7000)),
            None
        ));
        // `down B; up B`: the up made after the marker: it did not
        assert!(!down_came(
            &theirs(7000, b"time:[1]"),
            10,
            &me(Some(7001)),
            None
        ));
        // another time namespace (another offset), or no made time: the calls' times
        assert!(!down_came(
            &theirs(7000, b"time:[2]"),
            10,
            &me(Some(6000)),
            None
        ));
        assert!(!down_came(&theirs(7000, b"time:[1]"), 10, &me(None), None));
        // the same pid namespace still goes by the pids' order
        let same = DownLine {
            pidns: Some(b"pid:[1]".to_vec()),
            last_pid: Some(499),
            ..theirs(7000, b"time:[1]")
        };
        assert!(!down_came(&same, 10, &me(Some(6000)), None));
    }

    #[test]
    fn made_us_is_before_now_and_after_boot() {
        let made = crate::procfs::made_us().expect("CLOCK_BOOTTIME");
        let now = crate::procfs::boot_us().expect("CLOCK_BOOTTIME");
        assert!(made > 0 && made <= now, "{made} {now}");
    }

    #[test]
    fn down_lines() {
        assert_eq!(
            DownLine::read(b"123"),
            Some(DownLine {
                dt: 123,
                ..DownLine::default()
            })
        );
        assert_eq!(
            DownLine::read(b"123 456 pid:[4026531836] 1792884909460285"),
            Some(DownLine {
                dt: 123,
                last_pid: Some(456),
                pidns: Some(b"pid:[4026531836]".to_vec()),
                call_us: Some(1_792_884_909_460_285),
                ..DownLine::default()
            })
        );
        assert_eq!(
            DownLine::read(
                b"123 456 pid:[4026531836] 1792884909460285 640123456789 time:[4026531834]"
            ),
            Some(DownLine {
                dt: 123,
                last_pid: Some(456),
                pidns: Some(b"pid:[4026531836]".to_vec()),
                call_us: Some(1_792_884_909_460_285),
                boot_us: Some(640_123_456_789),
                timens: Some(b"time:[4026531834]".to_vec()),
            })
        );
        assert_eq!(
            DownLine::read(b"123 - - 77"),
            Some(DownLine {
                dt: 123,
                call_us: Some(77),
                ..DownLine::default()
            })
        );
        assert_eq!(DownLine::read(b""), None);
        assert_eq!(DownLine::read(b"x 1"), None);
    }

    #[test]
    fn down_markers() {
        assert_eq!(down_marker(b"123\n"), (b"123".to_vec(), None));
        assert_eq!(
            down_marker(b"123\nj 1\n"),
            (b"123".to_vec(), Some(b"j 1".to_vec()))
        );
        assert_eq!(down_marker(b""), (Vec::new(), None));
    }

    #[test]
    fn ro_bind_lines() {
        assert_eq!(
            robind_line(b"  ~/nas:/mnt/nas  # mine"),
            Some(b"~/nas:/mnt/nas".to_vec())
        );
        assert_eq!(robind_line(b"# all of it"), None);
        assert_eq!(robind_line(b"/a b"), Some(b"/a b".to_vec()));
        assert_eq!(robind_line(b"/a#b"), Some(b"/a#b".to_vec()));
        assert_eq!(robind_line(b"   "), None);
        assert_eq!(shell_lines(b"a\nb"), vec![b"a".to_vec(), b"b".to_vec()]);
        assert_eq!(shell_lines(b"a\n"), vec![b"a".to_vec()]);
    }

    #[test]
    fn paths() {
        assert_eq!(before(b"~/x:/y", b":/"), b"~/x");
        assert_eq!(dir_of(b"/a/b/Hyprland"), b"/a/b");
        assert_eq!(base_of(b"/a/theme"), b"theme");
    }
}
