//! The words a computer flashes on its screens as the keyboard and mouse
//! move ([`crate::overlay::flash`]): each switch shows the name of the
//! computer that has them now, on the computer the cursor arrives at (and
//! on the one it left), and Identify shows a computer's own name. Pure: no
//! I/O.

use std::time::Duration;

use crate::overlay::Fallback;

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

/// A badge to flash: its text, and whether it becomes a notification
/// where it cannot be drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Badge {
    pub text: String,
    pub fallback: Fallback,
}

/// The badge for `event`.
pub fn of(event: &Event) -> Badge {
    Badge {
        text: text(event),
        fallback: fallback(event),
    }
}

/// Flash `badge` on this computer's screens for `how_long`.
pub fn flash(badge: &Badge, how_long: Duration) {
    crate::overlay::flash(&badge.text, how_long, badge.fallback);
}

/// Whether `event`'s badge becomes a notification on a desktop that
/// cannot draw it (GNOME, X11, no font): Identify, and what the owner did
/// not do (a return for a reason, the driver's refusals and the gaming
/// lock's words) do; an ordinary switch, which comes at every edge, does
/// not.
pub fn fallback(event: &Event) -> Fallback {
    match event {
        Event::Arrived { .. } | Event::Went { .. } | Event::Home { why: None, .. } => {
            Fallback::Quiet
        }
        Event::Home { why: Some(_), .. } | Event::Identify { .. } | Event::Note(_) => {
            Fallback::Notify
        }
    }
}

/// The badge's text for `event`: a line, and for a return the owner did
/// not make, why on a second. A computer with no name yet is still said:
/// this one as "this computer", another as "another computer" (never
/// "this computer" as the keyboard and mouse leave it).
pub fn text(event: &Event) -> String {
    let name = |name: &str, unnamed: &str| {
        let name = clip(name, NAME_CHARS);
        if name.is_empty() {
            unnamed.to_string()
        } else {
            name
        }
    };
    let here = |here: &str| name(here, "this computer");
    match *event {
        Event::Arrived { here: at }
        | Event::Home {
            here: at,
            why: None,
        } => {
            format!("Keyboard and mouse: {}", here(at))
        }
        Event::Went { to } => format!("Keyboard and mouse: {}", name(to, "another computer")),
        Event::Home {
            here: at,
            why: Some(why),
        } => format!("Keyboard and mouse: {}\n{why}", here(at)),
        Event::Identify { here: at } => format!("This is {}", here(at)),
        Event::Note(note) => note.to_string(),
    }
}

/// `text` cut to `most` characters (an ellipsis marking the cut), on one
/// line: a control character (a newline in a peer's name), or one that
/// turns or hides text (a bidirectional override, a zero-width space), is
/// a space.
pub fn clip(text: &str, most: usize) -> String {
    pocketdesk_core::local::shared_input::display_name(text, most)
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
        // Another computer with no name yet is never "this computer" (BK8
        // live: one that only takes input never says its name).
        assert_eq!(
            text(&Event::Went { to: "" }),
            "Keyboard and mouse: another computer"
        );
        assert_eq!(
            text(&Event::Went { to: " \u{200B} " }),
            "Keyboard and mouse: another computer"
        );
    }

    #[test]
    fn only_identify_and_what_the_owner_did_not_do_notify() {
        let quiet = [
            Event::Arrived { here: "L" },
            Event::Went { to: "L" },
            Event::Home {
                here: "W",
                why: None,
            },
        ];
        for event in quiet {
            assert_eq!(fallback(&event), Fallback::Quiet, "{event:?}");
        }
        let told = [
            Event::Home {
                here: "W",
                why: Some("This computer locked"),
            },
            Event::Identify { here: "W" },
            Event::Note("L won't take input"),
        ];
        for event in told {
            assert_eq!(fallback(&event), Fallback::Notify, "{event:?}");
        }
        assert_eq!(
            of(&Event::Identify { here: "W" }),
            Badge {
                text: "This is W".to_string(),
                fallback: Fallback::Notify
            }
        );
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
        // Nor turn the text around it.
        assert_eq!(
            text(&Event::Arrived {
                here: "Lap\u{202E}pot\u{2066}"
            }),
            "Keyboard and mouse: Lap pot"
        );
    }
}
