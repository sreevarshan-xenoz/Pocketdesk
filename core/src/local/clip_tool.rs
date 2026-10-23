//! The program that reads and writes the clipboard on Linux (T2): Wayland's
//! `wl-paste`/`wl-copy`, else X11's `xclip` (Mint and other desktops still
//! start in X11).
//!
//! Only the command lines live here, so they are tested on every platform;
//! the server runs them.

use std::ffi::OsStr;

/// Which clipboard the desktop has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// `wl-clipboard`: `WAYLAND_DISPLAY` is set.
    Wayland,
    /// `xclip`: any other session (X11's `DISPLAY`).
    X11,
}

/// A program and its arguments.
pub type Command = (&'static str, Vec<String>);

/// The text targets an X11 copy offers, best first: UTF-8 before Latin-1.
const X11_TEXT: [&str; 5] = [
    "UTF8_STRING",
    "text/plain;charset=utf-8",
    "text/plain",
    "STRING",
    "TEXT",
];

impl Tool {
    /// From `WAYLAND_DISPLAY`'s and `DISPLAY`'s values: X11 only when there
    /// is an X display and no Wayland one. With neither (a server started
    /// from a unit or over ssh), Wayland, as before T2: `wl-paste` finds
    /// `wayland-0` by itself (T2 review).
    pub fn from_env(wayland_display: Option<&OsStr>, display: Option<&OsStr>) -> Self {
        let set = |v: Option<&OsStr>| v.is_some_and(|v| !v.is_empty());
        if !set(wayland_display) && set(display) {
            Self::X11
        } else {
            Self::Wayland
        }
    }

    /// This session's.
    pub fn current() -> Self {
        Self::from_env(
            std::env::var_os("WAYLAND_DISPLAY").as_deref(),
            std::env::var_os("DISPLAY").as_deref(),
        )
    }

    /// The types the clipboard's copy offers, one a line (X11's targets).
    pub fn list_types(self) -> Command {
        match self {
            Self::Wayland => ("wl-paste", args(&["--list-types"])),
            Self::X11 => (
                "xclip",
                args(&["-selection", "clipboard", "-o", "-t", "TARGETS"]),
            ),
        }
    }

    /// The copy's text, as offered among `types` ([`Self::list_types`]'s
    /// output), or `None` when it offers none. `wl-paste` picks the text
    /// type itself and adds no newline; `xclip` is told which target.
    pub fn read_text(self, types: &str) -> Option<Command> {
        let offered: Vec<&str> = types.lines().map(str::trim).collect();
        match self {
            // The types the server read as text before T2.
            Self::Wayland => offered
                .iter()
                .any(|t| {
                    ["text/plain", "UTF8_STRING", "TEXT", "STRING"]
                        .iter()
                        .any(|p| t.starts_with(p))
                })
                .then(|| ("wl-paste", args(&["--no-newline", "--type", "text"]))),
            Self::X11 => X11_TEXT
                .iter()
                .find(|t| offered.contains(t))
                .map(|t| self.read(t)),
        }
    }

    /// The copy as `mime` (an image's `image/png`).
    pub fn read(self, mime: &str) -> Command {
        match self {
            Self::Wayland => ("wl-paste", args(&["--type", mime])),
            Self::X11 => (
                "xclip",
                args(&["-selection", "clipboard", "-o", "-t", mime]),
            ),
        }
    }

    /// Put what is on the program's input on the clipboard, as `mime` when
    /// given (text otherwise: UTF-8, which an older `xclip` does not assume;
    /// T2 review). Both stay in the background to serve it.
    pub fn write(self, mime: Option<&str>) -> Command {
        let mut command = match self {
            Self::Wayland => ("wl-copy", Vec::new()),
            Self::X11 => ("xclip", args(&["-selection", "clipboard", "-i"])),
        };
        let mime = match (self, mime) {
            (Self::X11, None) => Some("UTF8_STRING"),
            (_, mime) => mime,
        };
        if let Some(mime) = mime {
            let flag = match self {
                Self::Wayland => "--type",
                Self::X11 => "-t",
            };
            command.1.extend(args(&[flag, mime]));
        }
        command
    }
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|a| a.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x11_only_with_an_x_display_and_no_wayland_one() {
        let os = |v: &'static str| Some(OsStr::new(v));
        assert_eq!(Tool::from_env(os("wayland-0"), os(":0")), Tool::Wayland);
        assert_eq!(Tool::from_env(os("wayland-0"), None), Tool::Wayland);
        assert_eq!(Tool::from_env(None, os(":0")), Tool::X11);
        assert_eq!(Tool::from_env(os(""), os(":1")), Tool::X11);
        // Neither (a unit, ssh): as before, wl-paste finds wayland-0 itself.
        assert_eq!(Tool::from_env(None, None), Tool::Wayland);
        assert_eq!(Tool::from_env(None, os("")), Tool::Wayland);
    }

    #[test]
    fn x11_reads_the_best_text_target_offered() {
        let utf8 = "TIMESTAMP\nTARGETS\nSTRING\nUTF8_STRING\ntext/plain\n";
        assert_eq!(
            Tool::X11.read_text(utf8),
            Some((
                "xclip",
                args(&["-selection", "clipboard", "-o", "-t", "UTF8_STRING"])
            ))
        );
        let latin = "TARGETS\nSTRING\n";
        assert_eq!(
            Tool::X11.read_text(latin).unwrap().1.last().unwrap(),
            "STRING"
        );
        assert_eq!(
            Tool::X11.read_text("TARGETS\nimage/png\n"),
            None,
            "an image"
        );
        assert_eq!(Tool::X11.read_text(""), None, "empty");
    }

    #[test]
    fn wayland_reads_text_as_it_did() {
        assert_eq!(
            Tool::Wayland.read_text("text/plain;charset=utf-8\n"),
            Some(("wl-paste", args(&["--no-newline", "--type", "text"])))
        );
        assert_eq!(Tool::Wayland.read_text("image/png\n"), None);
    }

    #[test]
    fn types_images_and_writes() {
        assert_eq!(Tool::Wayland.list_types().1, args(&["--list-types"]));
        assert_eq!(
            Tool::X11.list_types().1,
            args(&["-selection", "clipboard", "-o", "-t", "TARGETS"])
        );
        assert_eq!(
            Tool::X11.read("image/png").1,
            args(&["-selection", "clipboard", "-o", "-t", "image/png"])
        );
        assert_eq!(Tool::Wayland.write(None), ("wl-copy", Vec::new()));
        assert_eq!(
            Tool::Wayland.write(Some("image/png")).1,
            args(&["--type", "image/png"])
        );
        assert_eq!(
            Tool::X11.write(None),
            (
                "xclip",
                args(&["-selection", "clipboard", "-i", "-t", "UTF8_STRING"])
            )
        );
        assert_eq!(
            Tool::X11.write(Some("image/png")).1,
            args(&["-selection", "clipboard", "-i", "-t", "image/png"])
        );
    }
}
