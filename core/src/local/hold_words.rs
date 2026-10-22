//! Notifications held by their words (P4): desktop Settings "Hold
//! notifications with these words" lists phrases, one a line, each with an
//! app optional (`Signal: lunch`). A phone's notification whose title or
//! text holds one, from that app when one is named, is held as a muted
//! app's (K2b): logged and kept to answer, never shown.

/// At most this many rules are read; the rest are left out.
pub const MAX_RULES: usize = 100;

/// A rule as written: the words, and the app it is for, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub app: Option<String>,
    pub words: String,
}

impl Rule {
    /// The rule as the Settings box has it, for the log and Activity.
    pub fn line(&self) -> String {
        match &self.app {
            Some(app) => format!("{app}: {}", self.words),
            None => self.words.clone(),
        }
    }

    /// Whether a notification from `app` with `title` and `text` holds it,
    /// ignoring case.
    pub fn holds(&self, app: &str, title: &str, text: &str) -> bool {
        if let Some(only) = &self.app
            && only.to_lowercase() != app.trim().to_lowercase()
        {
            return false;
        }
        let words = self.words.to_lowercase();
        title.to_lowercase().contains(&words) || text.to_lowercase().contains(&words)
    }
}

/// One line as a rule: `words`, or `App: words` (an app before the first
/// `": "`). Blank lines, and lines with no words, are none.
pub fn parse_line(line: &str) -> Option<Rule> {
    // Split before trimming the end: `Signal: ` is an app with no words.
    let line = line.trim_start();
    let (app, words) = match split_app(line) {
        Some((app, words)) if !app.trim().is_empty() => {
            (Some(app.trim().to_string()), words.trim())
        }
        // `: words` names no app: for words with ": " in them.
        Some((_, words)) => (None, words.trim()),
        // An app and a colon, the space trimmed away: no words either.
        _ if line.trim_end().ends_with(':') => return None,
        _ => (None, line.trim_end()),
    };
    (!words.is_empty()).then(|| Rule {
        app,
        words: words.to_string(),
    })
}

/// A line split at its first colon followed by a space or tab.
fn split_app(line: &str) -> Option<(&str, &str)> {
    line.char_indices()
        .find(|&(i, c)| c == ':' && line[i + 1..].starts_with([' ', '\t']))
        .map(|(i, _)| (&line[..i], &line[i + 1..]))
}

/// What keeps the lines from holding as written, for Settings (P4
/// review): lines that make no rule, and lines past [`MAX_RULES`].
pub fn problem(lines: &[String]) -> Option<String> {
    let written: Vec<&String> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
    let empty = written.iter().filter(|l| parse_line(l).is_none()).count();
    if empty > 0 {
        return Some(format!(
            "{empty} line{} end{} with a colon and hold nothing: an app needs words after \": \".",
            if empty == 1 { "" } else { "s" },
            if empty == 1 { "s" } else { "" },
        ));
    }
    (written.len() > MAX_RULES).then(|| format!("Only the first {MAX_RULES} lines are read."))
}

/// The rules the config's lines make, at most [`MAX_RULES`].
pub fn rules(lines: &[String]) -> Vec<Rule> {
    lines
        .iter()
        .filter_map(|l| parse_line(l))
        .take(MAX_RULES)
        .collect()
}

/// The first rule that holds a notification, if any.
pub fn held_by<'a>(rules: &'a [Rule], app: &str, title: &str, text: &str) -> Option<&'a Rule> {
    rules.iter().find(|rule| rule.holds(app, title, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn lines_are_words_with_an_app_optional() {
        assert_eq!(
            parse_line("  lunch order "),
            Some(Rule {
                app: None,
                words: "lunch order".into()
            })
        );
        assert_eq!(
            parse_line("Signal: Mum"),
            Some(Rule {
                app: Some("Signal".into()),
                words: "Mum".into()
            })
        );
        assert_eq!(parse_line(""), None);
        assert_eq!(
            parse_line("Signal: "),
            None,
            "an app with no words is no rule"
        );
        assert_eq!(
            parse_line("10:30 meeting"),
            Some(Rule {
                app: None,
                words: "10:30 meeting".into()
            }),
            "a colon without a space is words"
        );
        assert_eq!(
            parse_line(": Re: sale"),
            Some(Rule {
                app: None,
                words: "Re: sale".into()
            }),
            "a leading colon names no app"
        );
    }

    #[test]
    fn a_rule_holds_its_words_in_the_title_or_text_ignoring_case() {
        let rules = rules(&lines(&["sale", "Slack: standup"]));
        assert_eq!(
            held_by(&rules, "Shop", "Big SALE today", "").map(Rule::line),
            Some("sale".into())
        );
        assert_eq!(
            held_by(&rules, "Shop", "Hi", "the sale ends").map(Rule::line),
            Some("sale".into())
        );
        assert_eq!(
            held_by(&rules, "Slack", "Standup in 5", "").map(Rule::line),
            Some("Slack: standup".into())
        );
        assert_eq!(
            held_by(&rules, "Signal", "standup", ""),
            None,
            "another app's"
        );
        assert_eq!(held_by(&rules, "Signal", "Lunch?", "see you"), None);
    }

    #[test]
    fn app_names_match_ignoring_case_in_any_script() {
        let rules = rules(&lines(&["ÉCOLE: absence"]));
        assert!(held_by(&rules, "école", "Absence de Léa", "").is_some());
    }

    #[test]
    fn a_tab_after_the_colon_names_an_app_too() {
        assert_eq!(
            parse_line("Slack:\tstandup").and_then(|r| r.app),
            Some("Slack".into())
        );
    }

    #[test]
    fn lines_that_hold_nothing_are_said() {
        assert_eq!(problem(&lines(&["sale", ""])), None);
        assert!(
            problem(&lines(&["Note to self:"]))
                .unwrap()
                .contains("1 line ends")
        );
        let many: Vec<String> = (0..MAX_RULES + 1).map(|i| format!("w{i}")).collect();
        assert!(problem(&many).unwrap().contains("first 100"));
    }

    #[test]
    fn rules_are_bounded() {
        let many: Vec<String> = (0..MAX_RULES + 5).map(|i| format!("w{i}")).collect();
        assert_eq!(rules(&many).len(), MAX_RULES);
    }
}
