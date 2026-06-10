//! Pane-title summarization — pure logic with no zellij dependency.
//!
//! A pane's raw title is whatever zellij reports — usually the running
//! command line (`/usr/bin/cargo watch`, `nvim main.rs`) but sometimes a
//! user-chosen rename (`deploy notes`). The minimap has only a few cells to
//! spend per pane, so this module distills a title down to either a single
//! glyph (when the leading command is recognized and icons are enabled) or a
//! width-budgeted text label.
//!
//! All width accounting is in *display columns* via `unicode-width`, so a CJK
//! or wide glyph that occupies two cells is counted as two, never one. The
//! module has no zellij dependency and is unit-tested natively.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Map a known command name to its Nerd Font glyph, or `None` if unrecognized.
///
/// `cmd` is expected to already be a bare command (path prefix stripped). The
/// glyphs are written as `\u{...}` escapes so the source is self-contained and
/// readable without the Private Use Area font installed; the trailing comment
/// names each one. `md-*` glyphs come from Material Design Icons (widest
/// coverage); the editor pair has no `md-` variant so the `custom-*` set is
/// used.
pub fn icon_for(cmd: &str) -> Option<&'static str> {
    let glyph = match cmd {
        "nvim" | "neovim" => "\u{e6ae}",             // custom-neovim
        "vim" | "vi" => "\u{e62b}",                  // custom-vim
        "cargo" | "rustc" | "rustup" => "\u{f1617}", // md-language_rust
        "node" | "npm" | "npx" | "pnpm" | "yarn" | "bun" | "deno" => "\u{f0399}", // md-nodejs
        "git" => "\u{f062c}",                        // md-source_branch
        "python" | "python3" | "pip" | "pip3" | "uv" => "\u{f0320}", // md-language_python
        "docker" | "docker-compose" => "\u{f0868}",  // md-docker
        _ => return None,
    };
    Some(glyph)
}

/// Summarize `title` to fit within `available` display columns.
///
/// Priority (see `docs/design.md` §4.2):
/// 1. empty title → empty string
/// 2. a user rename (multi-token title whose leading word is not a known
///    command) → the whole title, truncated to width
/// 3. otherwise the leading command token's basename, optionally replaced by
///    its glyph when `icons` is true and the glyph fits, else truncated.
pub fn summarize(title: &str, available: usize, icons: bool) -> String {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // A rename (free-form text) is kept whole and merely width-truncated; only
    // command-shaped titles get reduced to their leading token / glyph.
    if !is_command_title(trimmed) {
        return truncated_to_width(trimmed, available);
    }
    let token = basename(leading_token(trimmed));
    if icons {
        if let Some(glyph) = icon_for(token) {
            if UnicodeWidthStr::width(glyph) <= available {
                return glyph.to_string();
            }
        }
    }
    truncated_to_width(token, available)
}

/// The first whitespace-separated token, or the whole string if it has none.
fn leading_token(title: &str) -> &str {
    title.split_whitespace().next().unwrap_or(title)
}

/// The path basename of `token` (text after the final `/`), or `token` itself
/// when that would be empty (e.g. a trailing slash).
fn basename(token: &str) -> &str {
    token
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(token)
}

/// Whether `title` should be treated as a command invocation (vs a rename).
///
/// A single token is always command-shaped (a bare command or a path). A
/// multi-token title is a command only when its leading token's basename is a
/// recognized command — the icon table is the discriminator. This is a
/// deliberate seam: `cargo build` and `deploy notes` are structurally
/// identical ("word space words"), and "is the head a known command" is the
/// only signal separating them. An unknown leading word is read as prose.
fn is_command_title(title: &str) -> bool {
    let mut tokens = title.split_whitespace();
    let Some(first) = tokens.next() else {
        return false;
    };
    if tokens.next().is_none() {
        return true;
    }
    icon_for(basename(first)).is_some()
}

/// Truncate `s` to at most `available` display columns.
///
/// When the full string fits, it is returned unchanged. Otherwise, with
/// `available >= 4` columns we reserve one column for a trailing `…` and fill
/// the remaining `available - 1` columns with whole characters; with fewer
/// columns there is no room for a meaningful ellipsis, so content is
/// hard-truncated without one. Width is accumulated per character and a wide
/// (2-column) glyph is never split, so the kept content can fall a column short
/// of the budget — e.g. `"あ…"` uses only 3 of a 4-column budget.
fn truncated_to_width(s: &str, available: usize) -> String {
    if UnicodeWidthStr::width(s) <= available {
        return s.to_string();
    }
    let (budget, ellipsis) = if available >= 4 {
        (available - 1, "…")
    } else {
        (available, "")
    };
    let kept = s
        .chars()
        .scan(0usize, |used, c| {
            let next = *used + UnicodeWidthChar::width(c).unwrap_or(0);
            (next <= budget).then(|| {
                *used = next;
                c
            })
        })
        .collect::<String>();
    format!("{kept}{ellipsis}")
}

/// Whether `label` is safe for a char-indexed (one-cell-per-char) overlay —
/// every character occupies exactly one display column.
///
/// The minimap places labels by character index (one terminal cell per char),
/// which is only correct when each char is a single column. A summarized label
/// can still be wider than one column per char (a CJK rename, or an icon glyph
/// once `icons` is enabled), so the renderer uses this to drop such labels
/// rather than corrupt the row until width-aware placement lands.
pub fn is_single_column(label: &str) -> bool {
    label.chars().all(|c| UnicodeWidthChar::width(c) == Some(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_for_maps_known_commands_and_rejects_unknown() {
        // Known commands resolve to a glyph; distinct families are distinct.
        assert!(icon_for("cargo").is_some());
        assert!(icon_for("nvim").is_some());
        assert!(icon_for("node").is_some());
        assert_ne!(icon_for("cargo"), icon_for("node"));
        assert_ne!(icon_for("nvim"), icon_for("cargo"));
        // Unknown / empty → no glyph.
        assert_eq!(icon_for("ls"), None);
        assert_eq!(icon_for(""), None);
        // Pin one mapping so an accidental table edit is caught.
        assert_eq!(icon_for("cargo"), Some("\u{f1617}")); // md-language_rust
    }

    #[test]
    fn summarize_keeps_short_titles() {
        assert_eq!(summarize("nvim", 10, false), "nvim");
        assert_eq!(summarize("nvim main.rs", 10, false), "nvim");
    }

    #[test]
    fn summarize_strips_path_prefix() {
        assert_eq!(summarize("/usr/bin/cargo watch", 10, false), "cargo");
    }

    #[test]
    fn summarize_caps_with_ellipsis() {
        // available >= 4 → keep (available - 1) content columns, then '…'.
        assert_eq!(summarize("verylongcommand", 5, false), "very…");
        assert_eq!(summarize("cargo", 4, false), "car…");
        // available < 4 → no ellipsis (floor: keep >= 3 content cols before '…').
        assert_eq!(summarize("cargo", 3, false), "car");
        assert_eq!(summarize("cargo", 5, false), "cargo");
        assert_eq!(summarize("verylongcommand", 0, false), "");
    }

    #[test]
    fn summarize_handles_empty() {
        assert_eq!(summarize("", 5, false), "");
        assert_eq!(summarize("   ", 5, false), "");
    }

    #[test]
    fn summarize_accounts_for_wide_characters() {
        // "あいう" is 6 display columns (2 each). With no ellipsis room
        // (available < 4) we fit whole cells only, never splitting a column.
        assert_eq!(summarize("あいう", 2, false), "あ");
        // available >= 4 reserves one column for '…'; only "あ" (2 cols) fits
        // in the remaining 3-column budget.
        assert_eq!(summarize("あいう", 4, false), "あ…");
    }

    #[test]
    fn summarize_passes_renames_through_untouched() {
        // Multi-token title whose leading word is not a known command is a
        // user rename: kept whole (only width-truncated), not reduced to its
        // first word.
        assert_eq!(summarize("deploy notes", 20, false), "deploy notes");
    }

    #[test]
    fn summarize_uses_icon_when_enabled_and_fitting() {
        // Leading command recognized + icons on + glyph fits → just the glyph.
        assert_eq!(summarize("cargo build", 4, true), "\u{f1617}");
        // Same input with icons off falls back to width-truncated text.
        assert_eq!(summarize("cargo build", 4, false), "car…");
    }

    #[test]
    fn summarize_falls_back_to_text_when_the_icon_does_not_fit() {
        // Leading command recognized + icons on, but no column to spend: the
        // 1-column glyph cannot fit a 0-column budget, so the icon branch falls
        // through to the width-truncated text path (which also yields nothing).
        assert_eq!(summarize("cargo build", 0, true), "");
    }

    #[test]
    fn summarize_keeps_text_when_no_icon_is_known() {
        // Icons on, but the command has no glyph in the table: the icon branch
        // yields nothing and the leading token passes through as text.
        assert_eq!(summarize("ls", 5, true), "ls");
    }

    #[test]
    fn is_command_title_rejects_a_blank_title() {
        // `summarize` trims and returns early on empty input, but the helper
        // itself must stay total: no leading token → not a command.
        assert!(!is_command_title(""));
        assert!(!is_command_title("   "));
    }

    #[test]
    fn is_single_column_flags_wide_glyphs() {
        // ASCII and the 1-column ellipsis are safe for char-indexed placement.
        assert!(is_single_column("cargo"));
        assert!(is_single_column("car…"));
        assert!(is_single_column(""));
        // A CJK title occupies two columns per char — unsafe for that path.
        assert!(!is_single_column("実装中"));
    }
}
