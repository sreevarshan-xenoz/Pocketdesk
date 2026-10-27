//! Notes from the phone (BF4), appended to the Markdown file the owner
//! picked (`notes_file`): one list item a note, its time first, its later
//! lines indented under it.

use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// This computer's local date and time now, as `2026-10-27 06:30`; `None`
/// where it cannot be read.
pub fn local_stamp() -> Option<String> {
    #[cfg(windows)]
    {
        // SAFETY: GetLocalTime takes nothing and returns the time.
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        Some(format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute
        ))
    }
    #[cfg(unix)]
    {
        // SAFETY: `time` with a null pointer only returns; `localtime_r`
        // fills the struct it is given and returns null on failure.
        unsafe {
            let now = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&now, &mut tm).is_null() {
                return None;
            }
            Some(format!(
                "{:04}-{:02}-{:02} {:02}:{:02}",
                tm.tm_year + 1900,
                tm.tm_mon + 1,
                tm.tm_mday,
                tm.tm_hour,
                tm.tm_min
            ))
        }
    }
    #[cfg(not(any(windows, unix)))]
    {
        None
    }
}

/// The note as it is appended: `- **<stamp>** <first line>`, its other
/// lines indented two spaces, so a Markdown viewer keeps them in the item.
pub fn entry(note: &str, stamp: &str) -> String {
    let mut lines = note.lines();
    let mut out = format!("- **{stamp}** {}\n", lines.next().unwrap_or(""));
    for line in lines {
        if line.is_empty() {
            out.push('\n');
        } else {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Append `entry` to the file at `path`, made if it is not there (its
/// folder must be). Written in one call, and on a line of its own: only the
/// file's last byte is read for that (BF4 review: a long notes file was
/// read whole for each note).
pub fn append(path: &Path, entry: &str) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .create(true)
        .append(true)
        .open(path)?;
    let ends_open = if file.metadata()?.len() > 0 {
        let mut last = [0u8; 1];
        file.seek(SeekFrom::End(-1))?;
        file.read_exact(&mut last)?;
        last[0] != b'\n'
    } else {
        false
    };
    let mut text = String::new();
    if ends_open {
        text.push('\n');
    }
    text.push_str(entry);
    file.write_all(text.as_bytes())
}

/// The notes file `setting` names: trimmed, and a path pasted with its
/// quotes (Windows' "Copy as path") without them. `None` when empty.
pub fn notes_path(setting: &str) -> Option<&str> {
    let trimmed = setting.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(trimmed)
        .trim();
    (!unquoted.is_empty()).then_some(unquoted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_is_one_list_item_its_lines_kept_inside() {
        assert_eq!(
            entry("Buy milk", "2026-10-27 06:30"),
            "- **2026-10-27 06:30** Buy milk\n"
        );
        assert_eq!(
            entry(
                "Call back\nabout the invoice\n\nhttps://example.com/x",
                "2026-10-27 06:30"
            ),
            "- **2026-10-27 06:30** Call back\n  about the invoice\n\n  https://example.com/x\n"
        );
    }

    #[test]
    fn notes_are_appended_each_on_a_line_of_its_own() {
        let dir = std::env::temp_dir().join(format!("pd-notes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("inbox.md");
        std::fs::write(&file, "# Inbox").unwrap();
        append(&file, &entry("one", "t1")).unwrap();
        append(&file, &entry("two", "t2")).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "# Inbox\n- **t1** one\n- **t2** two\n"
        );
        let fresh = dir.join("new.md");
        append(&fresh, &entry("first", "t3")).unwrap();
        assert_eq!(std::fs::read_to_string(&fresh).unwrap(), "- **t3** first\n");
        assert!(append(&dir.join("gone").join("x.md"), "x").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_pasted_with_its_quotes_is_taken_without_them() {
        assert_eq!(
            notes_path(r#" "C:\Notes\inbox.md" "#),
            Some(r"C:\Notes\inbox.md")
        );
        assert_eq!(notes_path("/home/me/inbox.md"), Some("/home/me/inbox.md"));
        assert_eq!(notes_path("  "), None);
        assert_eq!(notes_path(&'"'.to_string().repeat(2)), None);
    }

    #[test]
    fn the_stamp_is_a_date_and_a_time() {
        let stamp = local_stamp().expect("this system's clock");
        assert_eq!(stamp.len(), 16, "{stamp}");
        assert_eq!(&stamp[4..5], "-");
        assert_eq!(&stamp[10..11], " ");
    }
}
