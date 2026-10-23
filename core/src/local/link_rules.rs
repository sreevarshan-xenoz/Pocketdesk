//! Links by site (P8): desktop Settings "Links from phones" gains rules, one
//! a line, `site = browser`, `site = show` or `site = <program>`: a link
//! whose host is the site, or ends with `.site`, opens in the browser, is
//! only shown, or is handed to that program. The longest site that matches
//! wins; with none, "Links from phones" decides as before.

use std::path::PathBuf;

/// At most this many rules are read.
pub const MAX_RULES: usize = 50;

/// What a rule does with a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// The default browser.
    Browser,
    /// Shown in a notification, never opened.
    Show,
    /// Handed to this program as its one argument, never through a shell.
    Program(PathBuf),
}

/// One rule: a site, and what to do with its links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub site: String,
    pub action: Action,
}

/// A line as a rule: `site = browser | show | <program>`. A site is a host
/// name: lower case, no scheme, path or port; a line that is not one is
/// none, as is a blank or `#` line.
pub fn parse_line(line: &str) -> Option<Rule> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (site, action) = line.split_once('=')?;
    let site = site
        .trim()
        .trim_start_matches("*.")
        .trim_matches('.')
        .to_ascii_lowercase();
    let valid = !site.is_empty()
        && site.len() <= 253
        && site
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.');
    if !valid {
        return None;
    }
    let action = match action.trim() {
        "" => return None,
        a if a.eq_ignore_ascii_case("browser") => Action::Browser,
        a if a.eq_ignore_ascii_case("show") => Action::Show,
        program => Action::Program(PathBuf::from(program.trim_matches('"'))),
    };
    Some(Rule { site, action })
}

/// The rules the config's lines make, at most [`MAX_RULES`].
pub fn rules(lines: &[String]) -> Vec<Rule> {
    lines
        .iter()
        .filter_map(|l| parse_line(l))
        .take(MAX_RULES)
        .collect()
}

/// The lines that make no rule, for Settings to say.
pub fn bad_lines(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#') && parse_line(l).is_none())
        .map(str::to_string)
        .collect()
}

/// The rule for a link to `host`: the longest site it is, or ends with as
/// `.site`.
pub fn pick<'a>(rules: &'a [Rule], host: &str) -> Option<&'a Rule> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    rules
        .iter()
        .filter(|r| host == r.site || host.ends_with(&format!(".{}", r.site)))
        .max_by_key(|r| r.site.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn lines_are_a_site_and_what_to_do() {
        assert_eq!(
            parse_line(" YouTube.com = show "),
            Some(Rule {
                site: "youtube.com".into(),
                action: Action::Show
            })
        );
        assert_eq!(
            parse_line("*.zoom.us = C:\\Program Files\\Zoom\\Zoom.exe"),
            Some(Rule {
                site: "zoom.us".into(),
                action: Action::Program(PathBuf::from("C:\\Program Files\\Zoom\\Zoom.exe"))
            })
        );
        assert_eq!(
            parse_line("docs.example.org = Browser").map(|r| r.action),
            Some(Action::Browser)
        );
        for bad in [
            "",
            "# a note",
            "example.com",
            "https://example.com = show",
            "exa mple.com = show",
            "a.com =",
            "a.com/x = show",
        ] {
            assert_eq!(parse_line(bad), None, "{bad:?}");
        }
        assert_eq!(
            bad_lines(&lines(&["a.com = show", "nope", "", "# x"])),
            vec!["nope".to_string()]
        );
    }

    #[test]
    fn the_longest_site_a_host_is_or_ends_with_wins() {
        let rules = rules(&lines(&[
            "example.com = show",
            "docs.example.com = browser",
            "ample.com = show",
        ]));
        assert_eq!(
            pick(&rules, "docs.example.com").map(|r| &r.action),
            Some(&Action::Browser)
        );
        assert_eq!(
            pick(&rules, "a.docs.example.com").map(|r| &r.action),
            Some(&Action::Browser)
        );
        assert_eq!(
            pick(&rules, "WWW.Example.com.").map(|r| r.site.as_str()),
            Some("example.com")
        );
        assert_eq!(
            pick(&rules, "badexample.com"),
            None,
            "a site's end only at a dot"
        );
        assert_eq!(pick(&rules, "example.org"), None);
    }

    #[test]
    fn rules_are_bounded() {
        let many: Vec<String> = (0..MAX_RULES + 3)
            .map(|i| format!("s{i}.com = show"))
            .collect();
        assert_eq!(rules(&many).len(), MAX_RULES);
    }
}
