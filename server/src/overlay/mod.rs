//! A few words flashed on this computer's screens for a moment (the shared
//! keyboard and mouse's badge: which computer has them now, and Identify),
//! never in the way: it takes no click, no key and no focus.
//!
//! Windows: a topmost, click-through, layered window centred on each
//! monitor, shown without activation, its text drawn with GDI
//! ([`windows`]). Linux: on a compositor with the layer shell (Hyprland,
//! Sway, KDE Plasma), a surface on its overlay layer on each output with an
//! empty input region and no keyboard, its text drawn with `ab_glyph` from
//! the font `fc-match sans` names; elsewhere (GNOME, X11, no font) a
//! desktop notification instead ([`wayland`]).
//!
//! One thread draws them and takes them down: a flash replaces the one
//! showing, never stacks on it.

use std::time::Duration;

#[cfg(target_os = "linux")]
mod wayland;
#[cfg(windows)]
mod windows;

/// The longest line a badge has, in characters, before it wraps.
pub const LINE_CHARS: usize = 56;

/// The most lines a badge has.
pub const MOST_LINES: usize = 6;

/// What a flash becomes where it cannot be drawn (Linux without the layer
/// shell or a font).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    /// A desktop notification says it.
    Notify,
    /// Nothing: it comes too often to be worth a notification.
    Quiet,
}

/// Show `text` on this computer's screens for `how_long`, in place of
/// what shows now; where it cannot be drawn, `fallback` says whether a
/// notification says it. Returns at once: the drawing is another
/// thread's.
pub fn flash(text: &str, how_long: Duration, fallback: Fallback) {
    let lines = lines(text);
    if lines.is_empty() {
        return;
    }
    #[cfg(windows)]
    {
        let _ = fallback;
        windows::flash(lines, how_long);
    }
    #[cfg(target_os = "linux")]
    wayland::flash(lines, how_long, fallback);
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = (how_long, fallback);
}

/// `text`'s lines as a badge shows them: each of its own lines wrapped at
/// a space to at most [`LINE_CHARS`] (a word longer than that is cut), at
/// most [`MOST_LINES`] of them (an ellipsis marking the cut), blank ones
/// left out.
pub fn lines(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for given in text.lines() {
        let mut line = String::new();
        for word in given.split_whitespace() {
            let word: String = word.chars().take(LINE_CHARS).collect();
            let fits =
                line.is_empty() || line.chars().count() + 1 + word.chars().count() <= LINE_CHARS;
            if !fits {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&word);
        }
        if !line.is_empty() {
            out.push(line);
        }
    }
    if out.len() > MOST_LINES {
        out.truncate(MOST_LINES);
        if let Some(last) = out.last_mut() {
            let kept: String = last.chars().take(LINE_CHARS - 1).collect();
            *last = format!("{kept}…");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_badge_is_wrapped_and_kept_short() {
        assert_eq!(
            lines("Keyboard and mouse: Desk"),
            ["Keyboard and mouse: Desk"]
        );
        assert_eq!(
            lines("Keyboard and mouse: Desk\n\nThis computer locked"),
            ["Keyboard and mouse: Desk", "This computer locked"]
        );
        let long = "The program in front runs as administrator (or won't let Pocketdesk \
                    look at it), so this computer's keyboard and mouse cannot be held back";
        let wrapped = lines(long);
        assert!(wrapped.len() > 1, "{wrapped:?}");
        assert!(wrapped.iter().all(|l| l.chars().count() <= LINE_CHARS));
        assert_eq!(wrapped.join(" "), long);
        // A word longer than a line is cut to it.
        let word = "x".repeat(LINE_CHARS + 10);
        assert_eq!(lines(&word), ["x".repeat(LINE_CHARS)]);
        // Too many lines: the last says there was more.
        let many = ["line"; MOST_LINES + 3].join("\n");
        let kept = lines(&many);
        assert_eq!(kept.len(), MOST_LINES);
        assert_eq!(kept[MOST_LINES - 1], "line…");
        assert!(lines(" \n ").is_empty());
    }
}
