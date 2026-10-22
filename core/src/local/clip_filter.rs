//! Copies kept on this computer by the program that made them or by how
//! they look (O4): desktop Settings "Never send copies from" (programs, by
//! their file name) and "Never send copies that look like" (patterns,
//! regular expressions). A copy either names stays here whatever road would
//! have carried it (T10b's push, a session's sync, the tray's Send the
//! clipboard, D3's history), on top of the private marks a password manager
//! sets, which not every one does.

use regex::Regex;

/// The longest pattern taken: a pattern is a line, not a program.
pub const MAX_PATTERN: usize = 512;

/// Programs offered in Settings: password managers that do not always mark
/// their copies private.
pub const USUAL_PROGRAMS: &[&str] = &[
    "KeePass.exe",
    "KeePassXC.exe",
    "1Password.exe",
    "Bitwarden.exe",
];

/// Patterns offered in Settings: a private key's first line, and the shapes
/// of common access tokens.
pub const USUAL_PATTERNS: &[&str] = &[
    "-----BEGIN [A-Z ]*PRIVATE KEY-----",
    "\\bAKIA[0-9A-Z]{16}\\b",
    "\\bgh[pousr]_[A-Za-z0-9]{36,}\\b",
    "\\bxox[abpr]-[A-Za-z0-9-]{10,}",
];

/// Why `pattern` cannot be one, or `None` when it can.
pub fn pattern_problem(pattern: &str) -> Option<String> {
    if pattern.len() > MAX_PATTERN {
        return Some(format!("A pattern is at most {MAX_PATTERN} characters."));
    }
    Regex::new(pattern).err().map(|e| {
        format!(
            "\"{pattern}\" is not a pattern: {}",
            first_line(&e.to_string())
        )
    })
}

fn first_line(text: &str) -> &str {
    text.lines().last().unwrap_or(text).trim()
}

/// A program as copies name it: its file name, in lower case, without
/// `.exe`, whatever folder it was given with.
pub fn program_name(program: &str) -> String {
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program).trim();
    let lower = name.to_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
}

/// The rules, read from the two settings.
#[derive(Debug, Default)]
pub struct ClipFilter {
    programs: Vec<String>,
    patterns: Vec<Regex>,
}

impl ClipFilter {
    /// The rules for `programs` and `patterns`; a blank entry or a pattern
    /// that does not read is left out (Settings will not save one).
    pub fn new(programs: &[String], patterns: &[String]) -> Self {
        Self {
            programs: programs
                .iter()
                .map(|p| program_name(p))
                .filter(|p| !p.is_empty())
                .collect(),
            patterns: patterns
                .iter()
                .map(|p| p.trim())
                .filter(|p| !p.is_empty() && pattern_problem(p).is_none())
                .filter_map(|p| Regex::new(p).ok())
                .collect(),
        }
    }

    /// Whether nothing is kept back.
    pub fn is_empty(&self) -> bool {
        self.programs.is_empty() && self.patterns.is_empty()
    }

    /// Whether a copy, its `text` when it is text (an image has none, so
    /// only its program can hold it back), copied by the program at `owner`
    /// (its path or file name, when the system says), may leave this
    /// computer.
    pub fn lets_out(&self, text: Option<&str>, owner: Option<&str>) -> bool {
        let from = owner.map(program_name);
        !(from.is_some_and(|from| self.programs.contains(&from))
            || text.is_some_and(|text| self.patterns.iter().any(|p| p.is_match(text))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// O4: a program is named by its file name, any case, with or without `.exe`.
    #[test]
    fn a_program_is_known_by_its_file_name() {
        assert_eq!(
            program_name("C:\\Program Files\\KeePassXC\\KeePassXC.exe"),
            "keepassxc"
        );
        assert_eq!(program_name("keepassxc"), "keepassxc");
        assert_eq!(program_name("/usr/bin/keepassxc"), "keepassxc");
        let filter = ClipFilter::new(&strings(&["KeePassXC.exe"]), &[]);
        assert!(!filter.lets_out(Some("hunter2"), Some("C:\\Apps\\keepassxc.EXE")));
        assert!(filter.lets_out(Some("hunter2"), Some("C:\\Windows\\notepad.exe")));
        assert!(filter.lets_out(Some("hunter2"), None), "no owner said");
    }

    /// O4: a copy matching a pattern stays, wherever in it the match is.
    #[test]
    fn a_copy_that_looks_like_a_secret_stays() {
        let filter = ClipFilter::new(&[], &strings(USUAL_PATTERNS));
        assert!(!filter.lets_out(Some("-----BEGIN OPENSSH PRIVATE KEY-----\nb3Bl"), None));
        assert!(!filter.lets_out(Some("key: AKIA0123456789ABCDEF"), None));
        assert!(!filter.lets_out(Some(&format!("token ghp_{}", "a".repeat(36))), None));
        assert!(filter.lets_out(Some("an ordinary line"), None));
        // An image is held only by its program.
        assert!(filter.lets_out(None, None));
        assert!(ClipFilter::new(&[], &[]).is_empty());
    }

    /// O4: Settings refuses a pattern that does not read; the filter leaves one out.
    #[test]
    fn a_bad_pattern_is_said_and_left_out() {
        assert!(pattern_problem("(unclosed").is_some());
        assert!(pattern_problem(&"a".repeat(MAX_PATTERN + 1)).is_some());
        assert!(pattern_problem("^ok$").is_none());
        for usual in USUAL_PATTERNS {
            assert!(pattern_problem(usual).is_none(), "{usual}");
        }
        let filter = ClipFilter::new(&strings(&["", "  "]), &strings(&["(unclosed", " "]));
        assert!(filter.is_empty());
    }
}
