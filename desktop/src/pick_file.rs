//! "Choose file…" and "Choose folder…" for sending to a phone (F7b, T6b).
//! Windows has the common file dialog; elsewhere the app offers drag and
//! drop and a path field instead, so no GTK comes into the Linux build.

use std::path::PathBuf;
use std::thread::JoinHandle;

/// Whether this system has a file dialog to offer.
pub const AVAILABLE: bool = cfg!(windows);

/// Open the dialog on a thread of its own, so the window keeps painting;
/// the handle yields the chosen file (or folder, with `folder`), or `None`
/// when it was cancelled.
pub fn spawn(folder: bool) -> JoinHandle<Option<PathBuf>> {
    std::thread::spawn(move || pick(folder))
}

#[cfg(windows)]
fn pick(folder: bool) -> Option<PathBuf> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoTaskMemFree, CoUninitialize,
    };
    use windows::Win32::UI::Shell::{
        FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog,
        SIGDN_FILESYSPATH,
    };
    // SAFETY: COM is initialised for this thread for the dialog's lifetime
    // and released after; the display name is freed with CoTaskMemFree as
    // the API requires.
    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let picked = (|| -> windows::core::Result<Option<PathBuf>> {
            let dialog: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            let mut options = dialog.GetOptions()? | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM;
            if folder {
                options |= FOS_PICKFOLDERS;
            }
            dialog.SetOptions(options)?;
            if dialog.Show(None).is_err() {
                return Ok(None); // cancelled
            }
            let name = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
            let path = name.to_string().ok().map(PathBuf::from);
            CoTaskMemFree(Some(name.0 as *const _));
            Ok(path)
        })();
        if init.is_ok() {
            CoUninitialize();
        }
        picked.ok().flatten()
    }
}

#[cfg(not(windows))]
fn pick(_folder: bool) -> Option<PathBuf> {
    None
}
