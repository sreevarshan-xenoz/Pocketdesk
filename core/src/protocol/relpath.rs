//! Paths inside a folder sent between the phone and the computer (T6b).
//!
//! A file in a folder travels with its path under the folder's top as a
//! list of components, never as one joined string, and the receiver checks
//! every component before it creates anything. A path that fails is
//! refused, not repaired: two repairs could name the same file, and a
//! sender that sends `..` is not to be helped along.

/// The deepest a file may sit under the folder's top.
pub const MAX_DEPTH: usize = 32;

/// The longest one component may be, in bytes (Linux allows 255).
pub const MAX_COMPONENT_BYTES: usize = 255;

/// The longest a whole path under the top may be, in bytes, its separators
/// counted: an absurd stack of names is refused here, not left to fail at
/// the disk.
pub const MAX_PATH_BYTES: usize = 1024;

/// Why `component` cannot be part of a path, or `None` when it can.
pub fn component_problem(component: &str) -> Option<String> {
    if component.is_empty() {
        return Some("an empty name".to_string());
    }
    if component == "." || component == ".." {
        return Some(format!("{component:?} is not a name"));
    }
    if component.len() > MAX_COMPONENT_BYTES {
        return Some(format!("a name longer than {MAX_COMPONENT_BYTES} bytes"));
    }
    // Separators and the drive colon (which also names an NTFS stream), and
    // what Windows refuses in a name.
    if let Some(c) = component.chars().find(|c| {
        c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
    }) {
        return Some(format!("{component:?} holds {c:?}"));
    }
    if component
        .chars()
        .any(crate::protocol::kdeconnect::is_invisible_format)
    {
        return Some(format!("{component:?} holds an invisible character"));
    }
    // Windows drops these, which could make two names one.
    if component.ends_with([' ', '.']) {
        return Some(format!("{component:?} ends in a space or a dot"));
    }
    if is_device_name(component) {
        return Some(format!("{component:?} is a Windows device name"));
    }
    None
}

/// Whether `name` is a Windows device name, with or without an extension
/// (`NUL`, `com1.txt`, `LPT0`, `LPT¹`): Windows opens the device, not a
/// file.
pub fn is_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end();
    let upper = stem.to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }
    let mut chars = upper.chars();
    let prefix: String = chars.by_ref().take(3).collect();
    let rest: String = chars.collect();
    matches!(prefix.as_str(), "COM" | "LPT")
        && matches!(
            rest.as_str(),
            "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
        )
}

/// Why `path` (components under the folder's top, the file's own name
/// last) cannot be used, or `None` when it can.
pub fn path_problem(path: &[String]) -> Option<String> {
    if path.is_empty() {
        return Some("no name".to_string());
    }
    if path.len() > MAX_DEPTH {
        return Some(format!("deeper than {MAX_DEPTH} folders"));
    }
    if path.iter().map(|c| c.len() + 1).sum::<usize>() > MAX_PATH_BYTES {
        return Some(format!("a path longer than {MAX_PATH_BYTES} bytes"));
    }
    path.iter().find_map(|c| component_problem(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn ordinary_names_and_folders_pass() {
        assert_eq!(path_problem(&path(&["Photos", "2026", "beach.jpg"])), None);
        assert_eq!(path_problem(&path(&["notes", ".hidden-is-fine"])), None);
        assert_eq!(path_problem(&path(&["résumé final.pdf"])), None);
    }

    #[test]
    fn a_path_cannot_climb_out_or_name_a_drive() {
        for bad in [
            path(&["..", "evil.txt"]),
            path(&["a", ".", "b"]),
            path(&["a/../b"]),
            path(&["a\\..\\b"]),
            path(&["C:", "Windows"]),
            path(&["file.txt:stream"]),
            path(&[""]),
            path(&[]),
        ] {
            assert!(path_problem(&bad).is_some(), "{bad:?}");
        }
    }

    #[test]
    fn windows_traps_are_refused() {
        for bad in [
            "NUL",
            "nul.txt",
            "COM1",
            "lpt9.log",
            "COM¹",
            "COM0",
            "lpt0.txt",
            "CON .txt",
            "name.",
            "name ",
            "a?b",
            "a\u{0}b",
            "a\u{202E}b",
        ] {
            assert!(component_problem(bad).is_some(), "{bad:?}");
        }
        for fine in ["COM10", "LPT", "console.log", "nullable.rs", "auxiliary"] {
            assert_eq!(component_problem(fine), None, "{fine:?}");
        }
    }

    #[test]
    fn depth_and_length_are_bounded() {
        let deep: Vec<String> = (0..=MAX_DEPTH).map(|i| format!("d{i}")).collect();
        assert!(path_problem(&deep).is_some());
        assert!(path_problem(&deep[..MAX_DEPTH]).is_none());
        assert!(component_problem(&"a".repeat(MAX_COMPONENT_BYTES + 1)).is_some());
        assert!(component_problem(&"a".repeat(MAX_COMPONENT_BYTES)).is_none());
    }

    #[test]
    fn a_long_path_is_refused_as_a_whole() {
        // Each name is fine and the depth is fine; together they are not.
        let long: Vec<String> = (0..8).map(|i| format!("{i}{}", "a".repeat(199))).collect();
        assert!(path_problem(&long).is_some());
        assert!(path_problem(&long[..4]).is_none());
    }

    #[test]
    fn absolute_and_odd_paths_are_refused_as_a_phone_could_send_them() {
        for bad in [
            r"/etc",
            r"/",
            r"\\server",
            r"\\?\C:",
            r"C:\Windows",
            r"..\..",
            "a\nb",
            "a\tb",
            "f.txt::$DATA",
            "aux.tar.gz",
            "Prn",
            "CONIN$",
            "CONOUT$",
            "a/b",
            "trailing ",
            "trailing.",
            "\u{7f}",
            "...",
        ] {
            assert!(component_problem(bad).is_some(), "{bad:?}");
            assert!(path_problem(&path(&["ok", bad])).is_some(), "{bad:?}");
        }
        // Climbing in the middle of an otherwise fine path.
        assert!(path_problem(&path(&["a", "..", "..", "b"])).is_some());
        assert!(path_problem(&path(&["a", "", "b"])).is_some());
        // A single leading dot is a hidden file, not an escape.
        assert!(path_problem(&path(&[".nomedia"])).is_none());
    }

    #[test]
    fn a_name_that_is_all_dots_or_spaces_is_refused() {
        for bad in [".", "..", "...", "....", " ", "  "] {
            assert!(component_problem(bad).is_some(), "{bad:?}");
        }
    }
}
