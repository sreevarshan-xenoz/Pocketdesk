//! The words a computer flashes on its screens as the keyboard and mouse
//! move ([`crate::overlay::flash`]): each switch shows the name of the
//! computer that has them now, on the computer the cursor arrives at (and
//! on the one it left), and Identify shows a computer's own name. Pure: no
//! I/O.

use std::time::Duration;

/// How long a switch's badge shows.
pub const SWITCH_FOR: Duration = Duration::from_millis(1200);

/// How long Identify's shows: long enough to look at each screen.
pub const IDENTIFY_FOR: Duration = Duration::from_secs(3);

/// The most of a computer's name shown, in characters (a peer's name is
/// already at most 255 bytes on the wire).
pub const NAME_CHARS: usize = 64;

/// What a computer flashes a badge for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event<'a> {
    /// Another computer's keyboard and mouse came onto this one (its
    /// `Enter`): `here` is this computer's name.
    Arrived { here: &'a str },
    /// This computer's keyboard and mouse went to the computer named `to`.
    Went { to: &'a str },
    /// They came home to `here`; `why`, when it was not the owner moving
    /// them (a lock, a lost link, a refusal).
    Home { here: &'a str, why: Option<&'a str> },
    /// Identify: `here` shows its name.
    Identify { here: &'a str },
    /// A word from the driver that is no switch (the gaming lock, a
    /// refused switch key, nowhere to go).
    Note(&'a str),
}

/// The badge's text for `event`: a line, and for a return the owner did
/// not make, why on a second.
pub fn text(event: &Event) -> String {
    let name = |name: &str| {
        let name = clip(name, NAME_CHARS);
        if name.is_empty() {
            "this computer".to_string()
        } else {
            name
        }
    };
    match *event {
        Event::Arrived { here } | Event::Home { here, why: None } => {
            format!("Keyboard and mouse: {}", name(here))
        }
        Event::Went { to } => format!("Keyboard and mouse: {}", name(to)),
        Event::Home {
            here,
            why: Some(why),
        } => format!("Keyboard and mouse: {}\n{why}", name(here)),
        Event::Identify { here } => format!("This is {}", name(here)),
        Event::Note(note) => note.to_string(),
    }
}

/// `text` cut to `most` characters (an ellipsis marking the cut), on one
/// line: a control character (a newline in a peer's name) is a space.
pub fn clip(text: &str, most: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let flat = flat.trim();
    if flat.chars().count() <= most {
        return flat.to_string();
    }
    let mut cut: String = flat.chars().take(most.saturating_sub(1)).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_text_for_each_event() {
        assert_eq!(
            text(&Event::Arrived { here: "Laptop" }),
            "Keyboard and mouse: Laptop"
        );
        assert_eq!(
            text(&Event::Went { to: "Laptop" }),
            "Keyboard and mouse: Laptop"
        );
        assert_eq!(
            text(&Event::Home {
                here: "Desk",
                why: None
            }),
            "Keyboard and mouse: Desk"
        );
        assert_eq!(
            text(&Event::Home {
                here: "Desk",
                why: Some("This computer locked")
            }),
            "Keyboard and mouse: Desk\nThis computer locked"
        );
        assert_eq!(text(&Event::Identify { here: "Desk" }), "This is Desk");
        assert_eq!(
            text(&Event::Note(
                "No other computer to move the keyboard and mouse to"
            )),
            "No other computer to move the keyboard and mouse to"
        );
        // A computer with no name yet is still said.
        assert_eq!(
            text(&Event::Arrived { here: " " }),
            "Keyboard and mouse: this computer"
        );
        assert_eq!(text(&Event::Identify { here: "" }), "This is this computer");
    }

    #[test]
    fn a_name_is_clipped_and_kept_to_one_line() {
        let long = "n".repeat(300);
        let shown = text(&Event::Arrived { here: &long });
        let name = shown.trim_start_matches("Keyboard and mouse: ");
        assert_eq!(name.chars().count(), NAME_CHARS);
        assert!(name.ends_with('…'), "{name}");
        // A peer's name cannot add a line of its own.
        assert_eq!(
            text(&Event::Went {
                to: "Lap\ntop\u{7}"
            }),
            "Keyboard and mouse: Lap top"
        );
        assert_eq!(clip("Ünïcödé", 4), "Ünï…");
        assert_eq!(clip("short", 64), "short");
        assert_eq!(clip("  padded  ", 64), "padded");
    }
}
