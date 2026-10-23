//! Files on the clipboard (P7): those copied in Explorer (`CF_HDROP`) or a
//! Linux file manager (`text/uri-list`), so "Send the clipboard" and the
//! desktop app's Ctrl+V send them as Send file… does. A test seam,
//! `POCKETDESK_CLIPBOARD_FILES_FILE`, stands in for the clipboard: a file of
//! paths, one a line; the owner's own clipboard is never read then.

use std::path::PathBuf;

/// At most this many files are taken from one copy.
pub const MAX_FILES: usize = 100;

/// The files the clipboard holds, if it holds files: those that are there,
/// at most [`MAX_FILES`]. `None` when it holds none, or cannot be read.
pub fn read() -> Option<Vec<PathBuf>> {
    let paths = match std::env::var_os("POCKETDESK_CLIPBOARD_FILES_FILE") {
        Some(seam) => std::fs::read_to_string(seam)
            .ok()?
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .collect(),
        None => system()?,
    };
    let paths: Vec<PathBuf> = paths
        .into_iter()
        .filter(|p| p.exists())
        .take(MAX_FILES)
        .collect();
    (!paths.is_empty()).then_some(paths)
}

/// The paths a `text/uri-list` names: its `file://` lines, decoded; comments
/// and other schemes left out.
pub fn parse_uri_list(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("file://"))
        // `file://host/path`: only this computer's (no host, or localhost).
        .filter_map(|rest| {
            if rest.starts_with('/') {
                Some(rest)
            } else {
                rest.strip_prefix("localhost")
                    .filter(|r| r.starts_with('/'))
            }
        })
        .filter_map(percent_decode)
        .map(PathBuf::from)
        .collect()
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(windows)]
fn system() -> Option<Vec<PathBuf>> {
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    };
    use windows::Win32::System::Ole::CF_HDROP;
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

    let format = u32::from(CF_HDROP.0);
    // SAFETY: a query of the clipboard's formats.
    if unsafe { IsClipboardFormatAvailable(format) }.is_err() {
        return None;
    }
    // Another process may hold it open a moment.
    let mut opened = false;
    for _ in 0..10 {
        // SAFETY: opened with no owner, for reading only.
        if unsafe { OpenClipboard(None) }.is_ok() {
            opened = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    if !opened {
        return None;
    }
    // SAFETY: the clipboard is open; the handle stays valid until it closes,
    // and DragQueryFileW reads it within that.
    unsafe {
        let paths = GetClipboardData(format).ok().map(|handle| {
            let drop = HDROP(handle.0);
            let count = DragQueryFileW(drop, u32::MAX, None);
            (0..count.min(MAX_FILES as u32))
                .filter_map(|i| {
                    let len = DragQueryFileW(drop, i, None) as usize;
                    let mut name = vec![0u16; len + 1];
                    let got = DragQueryFileW(drop, i, Some(&mut name)) as usize;
                    (got > 0).then(|| PathBuf::from(String::from_utf16_lossy(&name[..got])))
                })
                .collect::<Vec<_>>()
        });
        let _ = CloseClipboard();
        paths
    }
}

#[cfg(target_os = "linux")]
fn system() -> Option<Vec<PathBuf>> {
    // Only when the copy offers a URI list: wl-paste would otherwise print
    // a text copy as one.
    let types = std::process::Command::new("wl-paste")
        .arg("--list-types")
        .output()
        .ok()?;
    if !String::from_utf8_lossy(&types.stdout)
        .lines()
        .any(|t| t.trim() == "text/uri-list")
    {
        return None;
    }
    let out = std::process::Command::new("wl-paste")
        .args(["--no-newline", "--type", "text/uri-list"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| parse_uri_list(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn system() -> Option<Vec<PathBuf>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uri_list_names_this_computers_files() {
        let list = "# copied\r\nfile:///home/ann/My%20Report.pdf\r\nfile://localhost/tmp/a.txt\nhttps://example.org/x\nfile://other-host/etc/passwd\n";
        assert_eq!(
            parse_uri_list(list),
            vec![
                PathBuf::from("/home/ann/My Report.pdf"),
                PathBuf::from("/tmp/a.txt")
            ]
        );
        assert!(
            parse_uri_list("file:///bad%zz").is_empty(),
            "a bad escape is no path"
        );
        assert_eq!(
            parse_uri_list("file:///caf%C3%A9"),
            vec![PathBuf::from("/café")]
        );
    }
}
