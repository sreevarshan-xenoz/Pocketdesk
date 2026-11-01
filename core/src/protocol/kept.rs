//! Phone folders kept on the computer, by themselves: what the phone's
//! packets say, held to the rules by the computer that reads them.
//!
//! A file of a kept folder is an ordinary share (`kdeconnect.share.request`,
//! the transfer path with its resume and SHA-256) with `keep: true` beside
//! the folder fields a folder send already has: `folderId` (the phone's
//! stable id for the kept folder), `folder` (its name on the phone) and
//! `path` (the file's names under it, checked by [`super::relpath`]). A
//! run's end is a [`KEEP_RUN`] packet. Nothing here touches a disk.

use serde_json::Value;

use super::relpath;

/// Phone → computer: a run's deletions and end, `{folderId, folder, mirror,
/// gone, done, sent, failed}`.
pub const KEEP_RUN: &str = "kdeconnect.pocketdesk.keep_run";

/// The most deleted paths one [`KEEP_RUN`] names; a phone sends more in
/// more packets.
pub const MAX_GONE: usize = 200;

/// A share's refusal starts with this when the kept folders hold what the
/// computer's cap allows: the phone stops the run on it, and says so.
pub const FULL: &str = "the kept folders on this computer are full";

/// A share's refusal says this when kept folders are off on the computer.
pub const OFF: &str = "Kept folders are turned off on this computer";

/// A share's or run's refusal says this for a phone that is not one of the
/// owner's own.
pub const NOT_OWNER: &str = "Kept folders take only the owner's own phones";

/// Whether `id` is a kept folder's id: 32 hex digits, as the phone makes it.
pub fn valid_folder_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether a share's `body` is for a kept folder.
pub fn is_keep(body: &Value) -> bool {
    body.get("keep").and_then(Value::as_bool) == Some(true)
}

/// The kept folder a file belongs to, and where it sits under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptFile {
    pub folder_id: String,
    pub label: String,
    /// The file's names under the folder, its own last; each one checked.
    pub path: Vec<String>,
}

/// A name as the phone shows a folder: whatever it is, one safe name for a
/// folder here. Anything that cannot be a name is `Folder`.
pub fn folder_name(label: &str) -> String {
    let cleaned: String = label
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    // Trailing dots and spaces are dropped by Windows, and two names could
    // become one.
    let mut name: String = cleaned.trim().trim_end_matches(['.', ' ']).to_string();
    while name.len() > 100 {
        name.pop();
    }
    let name = name.trim_end_matches(['.', ' ']).to_string();
    if relpath::component_problem(&name).is_some() {
        "Folder".to_string()
    } else {
        name
    }
}

/// The file a kept-folder share names, or why it is refused. `None` when
/// the share is not for a kept folder at all.
pub fn kept_file(body: &Value) -> Option<Result<KeptFile, String>> {
    if !is_keep(body) {
        return None;
    }
    Some(read_kept_file(body))
}

fn read_kept_file(body: &Value) -> Result<KeptFile, String> {
    let folder_id = body
        .get("folderId")
        .and_then(Value::as_str)
        .filter(|id| valid_folder_id(id))
        .ok_or("no usable kept-folder id")?
        .to_string();
    let label = body
        .get("folder")
        .and_then(Value::as_str)
        .ok_or("no kept-folder name")?
        .to_string();
    let path =
        names(body.get("path")).ok_or("the file's place in the folder is not a list of names")?;
    if let Some(problem) = relpath::path_problem(&path) {
        return Err(format!("the file's place in the folder: {problem}"));
    }
    Ok(KeptFile {
        folder_id,
        label,
        path,
    })
}

/// A JSON list of names, or `None` when it is anything else.
fn names(value: Option<&Value>) -> Option<Vec<String>> {
    value?
        .as_array()?
        .iter()
        .map(|v| v.as_str().map(str::to_string))
        .collect()
}

/// A [`KEEP_RUN`] packet's body, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub folder_id: String,
    pub label: String,
    /// The phone's own "Mirror deletions" switch.
    pub mirror: bool,
    /// Paths the phone deleted, each a list of names under the folder.
    pub gone: Vec<Vec<String>>,
    /// The last packet of the run: how many files went and failed.
    pub end: Option<RunEnd>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunEnd {
    pub sent: u64,
    pub failed: u64,
}

/// Read a [`KEEP_RUN`] body. A deleted path that is not safe is dropped
/// from the list, not repaired; the folder's id must be a valid one.
pub fn run_of(body: &Value) -> Result<Run, String> {
    let folder_id = body
        .get("folderId")
        .and_then(Value::as_str)
        .filter(|id| valid_folder_id(id))
        .ok_or("no usable kept-folder id")?
        .to_string();
    let label = body
        .get("folder")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let gone = body
        .get("gone")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .take(MAX_GONE)
                .filter_map(|p| names(Some(p)))
                .filter(|p| relpath::path_problem(p).is_none())
                .collect()
        })
        .unwrap_or_default();
    let count = |key: &str| body.get(key).and_then(Value::as_u64).unwrap_or(0);
    let end = (body.get("done").and_then(Value::as_bool) == Some(true)).then(|| RunEnd {
        sent: count("sent"),
        failed: count("failed"),
    });
    Ok(Run {
        folder_id,
        label,
        mirror: body.get("mirror").and_then(Value::as_bool) == Some(true),
        gone,
        end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ID: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn only_a_share_marked_keep_is_a_kept_file() {
        assert!(kept_file(&json!({"folderId": ID, "folder": "x", "path": ["a"]})).is_none());
        assert!(kept_file(&json!({"keep": false, "folderId": ID})).is_none());
        assert!(kept_file(&json!({"keep": "yes", "folderId": ID})).is_none());
        let file = kept_file(
            &json!({"keep": true, "folderId": ID, "folder": "Notes", "path": ["a", "b.txt"]}),
        )
        .unwrap()
        .unwrap();
        assert_eq!(file.path, ["a", "b.txt"]);
        assert_eq!(file.label, "Notes");
    }

    #[test]
    fn a_kept_file_with_a_bad_id_or_path_is_refused_not_repaired() {
        let share = |id: &str, path: Value| {
            kept_file(&json!({"keep": true, "folderId": id, "folder": "N", "path": path})).unwrap()
        };
        assert!(share("short", json!(["a"])).is_err());
        assert!(share(&"g".repeat(32), json!(["a"])).is_err());
        assert!(share(ID, json!([])).is_err());
        assert!(share(ID, json!("a/b")).is_err());
        assert!(share(ID, json!(["a", 7])).is_err());
        assert!(share(ID, json!(["a", ".."])).is_err());
        assert!(share(ID, json!(["/etc", "passwd"])).is_err());
        assert!(share(ID, json!(["a/../../b"])).is_err());
        assert!(share(ID, json!(["COM1"])).is_err());
        assert!(share(ID, json!([r"C:\Windows"])).is_err());
        let long = "a".repeat(300);
        assert!(share(ID, json!([long])).is_err());
        assert!(
            kept_file(&json!({"keep": true, "folderId": ID, "path": ["a"]}))
                .unwrap()
                .is_err(),
            "no folder name"
        );
    }

    #[test]
    fn a_folders_name_becomes_one_safe_name() {
        assert_eq!(folder_name("WhatsApp Media"), "WhatsApp Media");
        assert_eq!(folder_name("a/b\\c"), "a_b_c");
        assert_eq!(folder_name(".."), "Folder");
        assert_eq!(folder_name("..."), "Folder");
        assert_eq!(folder_name(""), "Folder");
        assert_eq!(folder_name("  "), "Folder");
        assert_eq!(folder_name("NUL"), "Folder");
        assert_eq!(folder_name("Notes. "), "Notes");
        assert_eq!(folder_name("a:b"), "a_b");
        assert!(folder_name(&"x".repeat(500)).len() <= 100);
        // Whatever the phone says, the result passes the path rules.
        for raw in ["../x", "C:\\", "/", "\u{0}", "con.txt", "x\u{202E}y", "\n"] {
            assert_eq!(
                relpath::component_problem(&folder_name(raw)),
                None,
                "{raw:?}"
            );
        }
    }

    #[test]
    fn a_run_drops_unsafe_deletions_and_bounds_the_list() {
        let run = run_of(&json!({
            "folderId": ID, "folder": "Notes", "mirror": true,
            "gone": [["a", "b.txt"], ["..", "x"], ["/etc"], "bad", ["ok"]],
        }))
        .unwrap();
        assert!(run.mirror);
        assert_eq!(run.gone, vec![vec!["a", "b.txt"], vec!["ok"]]);
        assert_eq!(run.end, None);
        let many: Vec<Value> = (0..MAX_GONE + 50)
            .map(|n| json!([format!("f{n}")]))
            .collect();
        assert_eq!(
            run_of(&json!({"folderId": ID, "gone": many}))
                .unwrap()
                .gone
                .len(),
            MAX_GONE
        );
    }

    #[test]
    fn a_run_ends_only_when_it_says_done() {
        let body = json!({"folderId": ID, "sent": 3, "failed": 1});
        assert_eq!(run_of(&body).unwrap().end, None);
        let body = json!({"folderId": ID, "done": true, "sent": 3, "failed": 1});
        assert_eq!(
            run_of(&body).unwrap().end,
            Some(RunEnd { sent: 3, failed: 1 })
        );
        assert!(run_of(&json!({"done": true})).is_err(), "no id");
        assert!(
            !run_of(&json!({"folderId": ID, "mirror": "true"}))
                .unwrap()
                .mirror
        );
    }
}
