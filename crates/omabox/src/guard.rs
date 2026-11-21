//! The agent guard (findings 65-67): agents' shell commands get a display that does not exist, so a
//! window, hyprctl or grim outside a box fails instead of reaching the desktop. What `guard on`
//! writes, byte for byte as bin/omabox's GUARD_* expand (share/guard/hook-version hashes it, and the
//! suite checks the stamps): Claude Code's SessionStart hook, Codex's marked config block and its hook.
//! See bin/omabox's comments on GUARD_ENV and GUARD_HOOK for what each part is for.

use crate::cat;
use crate::shell;

/// The guard's display and signature: a name with no socket behind it.
pub const GUARD: &str = "omabox-guard";

/// What `guard on` adds to and takes from a JSON hooks file (Claude Code's settings, Codex's
/// hooks.json), as bin/omabox's GUARD_JQ; each use appends its own program.
pub const GUARD_JQ: &str = r#"def ours: (.command? // "") | contains($g);
  def strip: if any(.hooks.SessionStart[]?.hooks[]?; ours) then
      .hooks.SessionStart |= map(if any(.hooks[]?; ours) then (.hooks |= map(select(ours | not))) | select(.hooks != []) else . end)
      | if .hooks.SessionStart == [] then del(.hooks.SessionStart) else . end
      | if .hooks == {} then del(.hooks) else . end
    else . end;"#;
/// guard_hooks' state, on and off, after GUARD_JQ.
pub const STATE_JQ: &str = r#"
        [.hooks.SessionStart[]?.hooks[]? | select(ours)] | if . == [] then "off" elif all(.command == $c) then "on" else "outdated" end"#;
pub const ON_JQ: &str =
    r#" strip | .hooks.SessionStart += [{hooks: [{type: "command", command: $c}]}]"#;
pub const OFF_JQ: &str = " strip";
/// guard_claude's: the commands of our hooks, for the guard dirs they name.
pub const COMMANDS_JQ: &str = ".hooks.SessionStart[]?.hooks[]? | select(ours) | .command";

/// guard_codex's TOML editing, run by the system's python (#152): verbatim from bin/omabox.
pub const CODEX_PY: &str = include_str!("guard_codex.py");

/// The guard's paths and texts for the omabox at ROOT.
pub struct Guard {
    pub root: Vec<u8>,
    /// GUARD_PATH: share/guard, its xdg-open, quickshell, qs, omarchy-shell and hyprctl stand-ins.
    pub path: Vec<u8>,
}

impl Guard {
    pub fn new(root: &[u8]) -> Guard {
        Guard {
            root: root.to_vec(),
            path: cat!(root, "/share/guard"),
        }
    }

    /// GUARD_ENV: NAME=VALUE each.
    pub fn env(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        let xo = cat!(self.path, "/xdg-open");
        vec![
            (b"WAYLAND_DISPLAY".to_vec(), GUARD.as_bytes().to_vec()),
            (
                b"HYPRLAND_INSTANCE_SIGNATURE".to_vec(),
                GUARD.as_bytes().to_vec(),
            ),
            (b"DISPLAY".to_vec(), Vec::new()),
            (b"QT_QPA_PLATFORMTHEME".to_vec(), Vec::new()),
            (b"QT_FORCE_STDERR_LOGGING".to_vec(), b"1".to_vec()),
            (b"BROWSER".to_vec(), xo.clone()),
            (b"GH_BROWSER".to_vec(), xo),
        ]
    }

    /// `${GUARD_ENV[*]}`.
    pub fn env_words(&self) -> Vec<u8> {
        let w: Vec<Vec<u8>> = self.env().iter().map(|(k, v)| cat!(k, "=", v)).collect();
        w.join(&b' ')
    }

    /// GUARD_NOTE: what the hooks tell the agent.
    pub fn note() -> String {
        format!(
            "omabox guard: shell commands here have no display (WAYLAND_DISPLAY={GUARD}). Anything that may open a window (apps, test binaries run outside ctest), hyprctl, grim, quickshell/qs and keyring checks go through omabox (load the omabox skill). Opening a link or file there (xdg-open, gh --web) is refused too: give the user the link. Never point WAYLAND_DISPLAY, DISPLAY or HYPRLAND_INSTANCE_SIGNATURE back at the real session. The ! commands the user runs are under this guard too: never suggest one that touches their desktop; tell them to run it in their own terminal. Only when the user asked for their real desktop in this task, run that command with: omabox host -- CMD"
        )
    }

    /// GUARD_HERE: whether omabox is still there (this checkout's, or one on PATH: ai-jail's, #203).
    fn here(&self) -> Vec<u8> {
        cat!(
            "{ [ -x '",
            self.root,
            "/bin/omabox' ] || command -v omabox >/dev/null 2>&1; }"
        )
    }

    /// GUARD_MARK (#290): the empty file named after the guard's display in $XDG_RUNTIME_DIR.
    pub fn mark() -> String {
        format!(
            r#"{{ [ -z "${{XDG_RUNTIME_DIR:-}}" ] || : >> "$XDG_RUNTIME_DIR/{GUARD}"; }} 2>/dev/null"#
        )
    }

    fn gone(&self) -> Vec<u8> {
        cat!("omabox guard: omabox is gone (", self.root, "/bin/omabox)")
    }

    /// GUARD_STAMP's field N (1 Claude Code, 2 Codex), from share/guard/hook-version.
    fn stamp(&self, n: usize) -> Vec<u8> {
        let v = std::fs::read(shell::path(&cat!(self.path, "/hook-version"))).unwrap_or_default();
        let line = v.split(|&c| c == b'\n').next().unwrap_or_default();
        shell::words(line).get(n - 1).cloned().unwrap_or_default()
    }

    /// guard_stale N: the hook's check of its stamp.
    fn stale(&self, n: usize) -> Vec<u8> {
        let vars = if n == 1 { "s t" } else { "t s" };
        cat!(
            "s=; read -r ",
            vars,
            " 2>/dev/null < '",
            self.path,
            "/hook-version'; [ -z \"$s\" ] || [ \"$s\" = '",
            self.stamp(n),
            "' ] || echo 'omabox guard: this hook is outdated (omabox changed it): tell the user to run omabox guard on to renew it'"
        )
    }

    /// GUARD_HOOK: Claude Code's SessionStart hook.
    pub fn hook(&self) -> Vec<u8> {
        let gp = &self.path;
        cat!(
            "if ! ",
            self.here(),
            "; then echo '",
            self.gone(),
            ", so the guard is not applied: shell commands here reach the real desktop. Its SessionStart hook (the one naming ",
            GUARD,
            ") can go from settings.json'; else ",
            Self::mark(),
            "; if [ -n \"$CLAUDE_ENV_FILE\" ] && l='case :$PATH: in *:",
            gp,
            ":*) ;; *) PATH=\"",
            gp,
            ":$PATH\" ;; esac; export ",
            self.env_words(),
            " PATH; prlimit --pid $$ --core=1: 2>/dev/null || true' && { grep -qxF \"$l\" \"$CLAUDE_ENV_FILE\" 2>/dev/null || { o=$(grep -vF 'WAYLAND_DISPLAY=",
            GUARD,
            " ' \"$CLAUDE_ENV_FILE\" 2>/dev/null); { [ -z \"$o\" ] || printf '%s\\n' \"$o\"; printf '%s\\n' \"$l\"; } > \"$CLAUDE_ENV_FILE\"; }; }; then echo '",
            Self::note(),
            "'; else echo 'omabox guard: NOT applied (no CLAUDE_ENV_FILE): shell commands here reach the real desktop; do GUI work in omabox anyway'; fi; ",
            self.stale(1),
            "; fi"
        )
    }

    /// GUARD_CODEX_HOOK: Codex's SessionStart hook.
    pub fn codex_hook(&self) -> Vec<u8> {
        cat!(
            "if ",
            self.here(),
            "; then ",
            Self::mark(),
            "; echo '",
            Self::note(),
            "'; ",
            self.stale(2),
            "; else echo '",
            self.gone(),
            ", but Codex still applies its guard: shell commands here have no display (WAYLAND_DISPLAY=",
            GUARD,
            "). To end it, delete the lines from # >>> omabox guard to # <<< omabox guard in Codex config.toml, and this hook from its hooks.json'; fi"
        )
    }

    /// GUARD_CODEX: the marked block of Codex's config.toml.
    pub fn codex_block(&self) -> Vec<u8> {
        let mut b = b"# >>> omabox guard (`omabox guard off` removes this block)\n[shell_environment_policy.set]\n".to_vec();
        for (k, v) in self.env() {
            b.extend_from_slice(&cat!(k, " = \"", v, "\"\n"));
        }
        b.extend_from_slice(b"# <<< omabox guard");
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_as_bash_expands_them() {
        let g = Guard::new(b"/co");
        assert_eq!(
            Guard::mark(),
            r#"{ [ -z "${XDG_RUNTIME_DIR:-}" ] || : >> "$XDG_RUNTIME_DIR/omabox-guard"; } 2>/dev/null"#
        );
        let block = String::from_utf8(g.codex_block()).unwrap_or_default();
        assert!(block.starts_with("# >>> omabox guard (`omabox guard off` removes this block)\n"));
        assert!(block.contains("\nDISPLAY = \"\"\n"));
        assert!(block.ends_with("GH_BROWSER = \"/co/share/guard/xdg-open\"\n# <<< omabox guard"));
        let hook = String::from_utf8(g.hook()).unwrap_or_default();
        assert!(hook.starts_with("if ! { [ -x '/co/bin/omabox' ] || command -v omabox"));
        assert!(hook.contains("export WAYLAND_DISPLAY=omabox-guard HYPRLAND_INSTANCE_SIGNATURE=omabox-guard DISPLAY= QT_QPA_PLATFORMTHEME= QT_FORCE_STDERR_LOGGING=1 BROWSER=/co/share/guard/xdg-open GH_BROWSER=/co/share/guard/xdg-open PATH;"));
        assert!(hook.ends_with("tell the user to run omabox guard on to renew it'; fi"));
        assert!(!hook.contains('\n'));
    }
}
