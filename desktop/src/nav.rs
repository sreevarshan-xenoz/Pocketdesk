//! The pages and how they are grouped down the left of the window (U1):
//! each page's name, the line saying what it is for, whether it scrolls
//! its own lists, and the key `--page` opens it by.

use crate::app::Tab;

/// A group of pages down the left, with its title if it has one.
pub struct Group {
    pub title: Option<&'static str>,
    pub pages: &'static [Tab],
}

/// The pages, in the order shown.
pub const GROUPS: &[Group] = &[
    Group {
        title: None,
        pages: &[Tab::Status],
    },
    Group {
        title: Some("Phones and computers"),
        pages: &[Tab::Pair, Tab::Devices, Tab::Sessions],
    },
    Group {
        title: Some("Sharing"),
        pages: &[Tab::Files, Tab::Messages, Tab::Notifications, Tab::Phone],
    },
    Group {
        title: None,
        pages: &[Tab::Activity, Tab::Settings],
    },
    Group {
        title: Some("Help"),
        pages: &[Tab::Diagnostics, Tab::Logs],
    },
];

impl Tab {
    /// Every page.
    pub const ALL: [Tab; 12] = [
        Tab::Status,
        Tab::Pair,
        Tab::Devices,
        Tab::Sessions,
        Tab::Files,
        Tab::Messages,
        Tab::Notifications,
        Tab::Phone,
        Tab::Activity,
        Tab::Settings,
        Tab::Diagnostics,
        Tab::Logs,
    ];

    /// Its name down the left and as its heading.
    pub fn name(self) -> &'static str {
        match self {
            Tab::Status => "Overview",
            Tab::Pair => "Pair",
            Tab::Devices => "Devices",
            Tab::Sessions => "Sessions",
            Tab::Files => "Files",
            Tab::Messages => "Messages",
            Tab::Notifications => "Notifications",
            Tab::Phone => "Phone files",
            Tab::Activity => "Activity",
            Tab::Settings => "Settings",
            Tab::Diagnostics => "Diagnostics",
            Tab::Logs => "Logs",
        }
    }

    /// One line under its heading: what it is for.
    pub fn purpose(self) -> &'static str {
        match self {
            Tab::Status => "Whether the server runs, and how phones reach this computer.",
            Tab::Pair => "Pair a phone with a code it scans, or a PIN.",
            Tab::Devices => "The phones and computers paired with this one, and what each may do.",
            Tab::Sessions => "Who is viewing or driving this computer now.",
            Tab::Files => "Send files to a phone or computer, and the files received.",
            Tab::Messages => "Texts from the phone, and replies to them.",
            Tab::Notifications => "The phone's notifications, and answers to them.",
            Tab::Phone => "Folders the phone shares with this computer.",
            Tab::Activity => "What has happened, newest first.",
            Tab::Settings => "How this computer works with phones and other computers.",
            Tab::Diagnostics => "Checks for when a phone cannot connect.",
            Tab::Logs => "The server's log.",
        }
    }

    /// Whether the page scrolls its own lists; the others are put in one
    /// scroll area as a whole (a scroll area inside another would have no
    /// height to fill).
    pub fn scrolls_itself(self) -> bool {
        matches!(
            self,
            Tab::Files
                | Tab::Messages
                | Tab::Notifications
                | Tab::Phone
                | Tab::Activity
                | Tab::Logs
        )
    }

    /// The key `--page` takes.
    pub fn key(self) -> &'static str {
        match self {
            Tab::Status => "overview",
            Tab::Pair => "pair",
            Tab::Devices => "devices",
            Tab::Sessions => "sessions",
            Tab::Files => "files",
            Tab::Messages => "messages",
            Tab::Notifications => "notifications",
            Tab::Phone => "phone-files",
            Tab::Activity => "activity",
            Tab::Settings => "settings",
            Tab::Diagnostics => "diagnostics",
            Tab::Logs => "logs",
        }
    }

    /// The page a key names.
    pub fn from_key(key: &str) -> Option<Tab> {
        Tab::ALL.into_iter().find(|t| t.key() == key)
    }
}

/// The page `--page <key>` (or `--page=<key>`) in `args` names, if any.
/// Read as the OS gives them: an argument that is not Unicode (a file
/// name, say) is passed over, where `std::env::args` would panic (U1
/// review).
pub fn page_from_args(args: impl IntoIterator<Item = std::ffi::OsString>) -> Option<Tab> {
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let Some(arg) = arg.to_str() else {
            continue;
        };
        if arg == "--page" {
            return args
                .next()
                .and_then(|key| key.to_str().and_then(Tab::from_key));
        }
        if let Some(key) = arg.strip_prefix("--page=") {
            return Tab::from_key(key);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_is_in_exactly_one_group() {
        for tab in Tab::ALL {
            let found = GROUPS
                .iter()
                .flat_map(|g| g.pages.iter())
                .filter(|t| **t == tab)
                .count();
            assert_eq!(found, 1, "{tab:?}");
        }
        let shown: usize = GROUPS.iter().map(|g| g.pages.len()).sum();
        assert_eq!(shown, Tab::ALL.len());
    }

    #[test]
    fn names_and_keys_are_unique_and_keys_round_trip() {
        for a in Tab::ALL {
            for b in Tab::ALL {
                if a != b {
                    assert_ne!(a.name(), b.name());
                    assert_ne!(a.key(), b.key());
                }
            }
            assert_eq!(Tab::from_key(a.key()), Some(a));
            assert!(!a.purpose().is_empty());
        }
    }

    #[test]
    fn page_names_a_page_to_open_on() {
        let args = |list: &[&str]| {
            list.iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            page_from_args(args(&["app", "--page", "settings"])),
            Some(Tab::Settings)
        );
        assert_eq!(
            page_from_args(args(&["app", "--page=phone-files"])),
            Some(Tab::Phone)
        );
        assert_eq!(page_from_args(args(&["app", "--page", "nope"])), None);
        assert_eq!(page_from_args(args(&["app"])), None);
    }

    #[test]
    fn an_argument_that_is_not_unicode_is_passed_over() {
        #[cfg(windows)]
        let odd = std::os::windows::ffi::OsStringExt::from_wide(&[0xD800]);
        #[cfg(unix)]
        let odd = std::os::unix::ffi::OsStringExt::from_vec(vec![0xff]);
        let args: Vec<std::ffi::OsString> = vec!["app".into(), odd, "--page=settings".into()];
        assert_eq!(page_from_args(args), Some(Tab::Settings));
    }
}
