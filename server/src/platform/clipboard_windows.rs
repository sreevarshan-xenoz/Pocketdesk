//! The Windows clipboard as text (`CF_UNICODETEXT`).
//!
//! Another process can hold the clipboard open for a moment (a clipboard
//! manager reading what was just copied), so opening it retries briefly
//! before reporting the failure. Writing needs an owner window: opened with
//! no owner, `EmptyClipboard` leaves the clipboard ownerless and Windows
//! drops the data, so each write borrows a message-only window.

use std::io;
use std::time::Duration;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE,
};
use windows::core::w;

/// A message-only window to own the clipboard while writing.
struct Owner(HWND);

impl Owner {
    fn new() -> io::Result<Self> {
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("Linux Link clipboard"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
        }
        .map_err(os_err)?;
        Ok(Owner(hwnd))
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

fn os_err(e: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(e.code().0 & 0xFFFF)
}

/// The clipboard, open for the lifetime of the guard.
struct Open;

impl Open {
    fn new(owner: Option<HWND>) -> io::Result<Self> {
        let mut last = None;
        for _ in 0..10 {
            match unsafe { OpenClipboard(owner) } {
                Ok(()) => return Ok(Open),
                Err(e) => last = Some(e),
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Err(os_err(last.expect("ten attempts were made")))
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        let _ = unsafe { CloseClipboard() };
    }
}

/// Replace the clipboard with `text`.
pub(crate) fn set_text(text: &str) -> io::Result<()> {
    let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = units.len() * 2;
    let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) }.map_err(os_err)?;
    let target = unsafe { GlobalLock(memory) } as *mut u16;
    if target.is_null() {
        let _ = unsafe { GlobalFree(Some(memory)) };
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `memory` holds `bytes` bytes, exactly `units.len()` u16s.
    unsafe {
        std::ptr::copy_nonoverlapping(units.as_ptr(), target, units.len());
        let _ = GlobalUnlock(memory);
    }

    let owner = Owner::new().inspect_err(|_| {
        let _ = unsafe { GlobalFree(Some(memory)) };
    })?;
    let _open = Open::new(Some(owner.0)).inspect_err(|_| {
        let _ = unsafe { GlobalFree(Some(memory)) };
    })?;
    if let Err(e) = unsafe { EmptyClipboard() } {
        let _ = unsafe { GlobalFree(Some(memory)) };
        return Err(os_err(e));
    }
    match unsafe { SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(memory.0))) } {
        // The clipboard owns the memory now.
        Ok(_) => Ok(()),
        Err(e) => {
            let _ = unsafe { GlobalFree(Some(memory)) };
            Err(os_err(e))
        }
    }
}

/// The clipboard's text, or an empty string when it holds no text.
pub(crate) fn get_text() -> io::Result<String> {
    let _open = Open::new(None)?;
    let Ok(handle) = (unsafe { GetClipboardData(u32::from(CF_UNICODETEXT.0)) }) else {
        return Ok(String::new());
    };
    let memory = HGLOBAL(handle.0);
    let source = unsafe { GlobalLock(memory) } as *const u16;
    if source.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CF_UNICODETEXT is a NUL-terminated UTF-16 string.
    let text = unsafe {
        let mut len = 0;
        while *source.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(source, len))
    };
    let _ = unsafe { GlobalUnlock(memory) };
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live: round-trips text through the real clipboard, then puts back what
    /// was there. Run on a desktop:
    /// `cargo test -p linux-link-server --lib clipboard_round_trips -- --ignored`.
    #[test]
    #[ignore = "uses the real clipboard on an interactive Windows desktop"]
    fn clipboard_round_trips_unicode_text() {
        let previous = get_text().unwrap_or_default();
        let sample = "Linux Link ✓ — naïve café 😀\nline two";
        set_text(sample).unwrap();
        assert_eq!(get_text().unwrap(), sample);
        set_text(&previous).unwrap();
    }
}
