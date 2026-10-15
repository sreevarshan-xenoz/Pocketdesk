//! The Windows clipboard as text (`CF_UNICODETEXT`) and as an image (the
//! registered "PNG" format browsers and the Snipping Tool use, and `CF_DIB`
//! for everyone else; roadmap N4).
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
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::{CF_DIB, CF_UNICODETEXT};
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
                w!("Pocketdesk clipboard"),
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

/// Global memory holding a copy of `bytes`, for `SetClipboardData`.
fn global_copy(bytes: &[u8]) -> io::Result<HGLOBAL> {
    let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }.map_err(os_err)?;
    let target = unsafe { GlobalLock(memory) } as *mut u8;
    if target.is_null() {
        let _ = unsafe { GlobalFree(Some(memory)) };
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `memory` holds `bytes.len()` bytes.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
        let _ = GlobalUnlock(memory);
    }
    Ok(memory)
}

/// A copy of the clipboard's data in `format`, if it holds that format.
/// The clipboard must be open.
fn read_format(format: u32) -> io::Result<Option<Vec<u8>>> {
    let Ok(handle) = (unsafe { GetClipboardData(format) }) else {
        return Ok(None);
    };
    let memory = HGLOBAL(handle.0);
    let size = unsafe { GlobalSize(memory) };
    let source = unsafe { GlobalLock(memory) } as *const u8;
    if source.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the clipboard's global holds `size` bytes while locked.
    let bytes = unsafe { std::slice::from_raw_parts(source, size) }.to_vec();
    let _ = unsafe { GlobalUnlock(memory) };
    Ok(Some(bytes))
}

/// The registered "PNG" clipboard format.
fn png_format() -> io::Result<u32> {
    match unsafe { RegisterClipboardFormatW(w!("PNG")) } {
        0 => Err(io::Error::last_os_error()),
        format => Ok(format),
    }
}

/// A number that changes whenever anything puts data on the clipboard, so a
/// poll can tell an unchanged clipboard without reading it.
pub(crate) fn sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

/// The marks that say a copy is private (T10b): its presence for the two
/// flag formats, the DWORD of the two `Can...` formats.
pub(crate) fn privacy_marks() -> crate::clipboard_push::WindowsMarks {
    let format = |name: windows::core::PCWSTR| unsafe { RegisterClipboardFormatW(name) };
    let present = |id: u32| id != 0 && unsafe { IsClipboardFormatAvailable(id) }.is_ok();
    let exclude = format(w!("ExcludeClipboardContentFromMonitorProcessing"));
    let viewer = format(w!("Clipboard Viewer Ignore"));
    let history = format(w!("CanIncludeInClipboardHistory"));
    let cloud = format(w!("CanUploadToCloudClipboard"));
    let mut marks = crate::clipboard_push::WindowsMarks {
        exclude_from_monitors: present(exclude),
        viewer_ignore: present(viewer),
        ..Default::default()
    };
    if (present(history) || present(cloud))
        && let Ok(_open) = Open::new(None)
    {
        marks.history = present(history).then(|| dword(history)).flatten();
        marks.cloud = present(cloud).then(|| dword(cloud)).flatten();
    }
    marks
}

/// The DWORD in clipboard format `id`; the clipboard must be open.
fn dword(id: u32) -> Option<u32> {
    let handle = unsafe { GetClipboardData(id) }.ok()?;
    let memory = HGLOBAL(handle.0);
    if unsafe { GlobalSize(memory) } < 4 {
        return None;
    }
    let source = unsafe { GlobalLock(memory) } as *const u32;
    if source.is_null() {
        return None;
    }
    // SAFETY: at least 4 bytes, checked above; unaligned reads are allowed.
    let value = unsafe { source.read_unaligned() };
    let _ = unsafe { GlobalUnlock(memory) };
    Some(value)
}

/// Whether the clipboard holds text, which wins over an image.
pub(crate) fn has_text() -> bool {
    unsafe { IsClipboardFormatAvailable(u32::from(CF_UNICODETEXT.0)) }.is_ok()
}

/// The clipboard's image as PNG: the "PNG" format as it is when an app put
/// one there (so a PNG this wrote reads back byte for byte), else `CF_DIB`
/// converted. `None` when it holds no image. The bytes are copied out and the
/// clipboard closed before any conversion, so other apps are not kept
/// waiting while a large bitmap is encoded.
pub(crate) fn get_image_png() -> io::Result<Option<Vec<u8>>> {
    let png = png_format()?;
    let dib = {
        let _open = Open::new(None)?;
        if let Some(bytes) = read_format(png)?
            && super::clip_image::is_png(&bytes)
        {
            return Ok(Some(bytes));
        }
        read_format(u32::from(CF_DIB.0))?
    };
    match dib {
        Some(dib) => super::clip_image::png_from_dib(&dib)
            .map(Some)
            .map_err(io::Error::other),
        None => Ok(None),
    }
}

/// Global memory not yet handed to the clipboard, freed if it never is.
struct Pending(Option<HGLOBAL>);

impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(memory) = self.0.take() {
            let _ = unsafe { GlobalFree(Some(memory)) };
        }
    }
}

/// Replace the clipboard with the image in `png_bytes`, as "PNG" and as
/// `CF_DIB` (Paint and older apps read only the bitmap), and return the
/// clipboard sequence number this write left. Windows moves the number when
/// the clipboard closes, so it is read just after; another app copying in
/// that instant would be taken for this write, a race accepted here.
///
/// Both blocks are allocated before the clipboard is emptied: a failure to
/// allocate leaves what was there.
pub(crate) fn set_image_png(png_bytes: &[u8]) -> io::Result<u32> {
    let dib = super::clip_image::dib_from_png(png_bytes).map_err(io::Error::other)?;
    let png = png_format()?;
    let mut blocks = [
        (png, Pending(Some(global_copy(png_bytes)?))),
        (u32::from(CF_DIB.0), Pending(Some(global_copy(&dib)?))),
    ];
    let owner = Owner::new()?;
    {
        let _open = Open::new(Some(owner.0))?;
        unsafe { EmptyClipboard() }.map_err(os_err)?;
        for (format, block) in &mut blocks {
            let memory = block.0.expect("set once, taken below");
            unsafe { SetClipboardData(*format, Some(HANDLE(memory.0))) }.map_err(os_err)?;
            // The clipboard owns the memory now.
            block.0 = None;
        }
    }
    Ok(sequence())
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
    /// `cargo test -p pocketdesk-server --lib clipboard_round_trips -- --ignored`.
    #[test]
    #[ignore = "uses the real clipboard on an interactive Windows desktop"]
    fn clipboard_round_trips_unicode_text() {
        let previous = get_text().unwrap_or_default();
        let sample = "Pocketdesk ✓ — naïve café 😀\nline two";
        set_text(sample).unwrap();
        assert_eq!(get_text().unwrap(), sample);
        set_text(&previous).unwrap();
    }

    /// Live: an image put on the real clipboard reads back as the same PNG,
    /// and as a DIB other apps can read. Leaves the image there (a test
    /// cannot put back an image it did not read).
    /// `cargo test -p pocketdesk-server --lib clipboard_holds_an_image -- --ignored`.
    #[test]
    #[ignore = "uses the real clipboard on an interactive Windows desktop"]
    fn clipboard_holds_an_image() {
        let mut png_bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_bytes, 2, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&[255, 0, 0, 255, 0, 0, 255, 128])
                .unwrap();
        }
        let before = sequence();
        let written = set_image_png(&png_bytes).unwrap();
        assert_ne!(written, before, "the sequence number moves");
        assert_eq!(sequence(), written, "and is the one the write returned");
        assert!(!has_text());
        assert_eq!(get_image_png().unwrap().unwrap(), png_bytes);
        let _open = Open::new(None).unwrap();
        let dib = read_format(u32::from(CF_DIB.0)).unwrap().unwrap();
        drop(_open);
        assert_eq!(
            super::super::clip_image::dib_from_png(
                &super::super::clip_image::png_from_dib(&dib).unwrap()
            )
            .unwrap(),
            dib
        );
    }
}
