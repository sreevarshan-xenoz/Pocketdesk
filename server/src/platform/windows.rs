//! Windows implementations of [`super`]'s helpers.

use windows::Data::Xml::Dom::XmlDocument;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegSetValueExW,
};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::core::{HSTRING, PCWSTR, w};

/// The identity toasts are shown under. An unpackaged app needs one registered
/// under `HKCU\Software\Classes\AppUserModelId` for Windows to display them.
const AUMID: &str = "LinuxLink.Server";

pub(super) fn init_process() {
    // Fails harmlessly when awareness was already set (e.g. by a manifest),
    // which is the only way it can fail on a supported Windows.
    if let Err(e) =
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
    {
        tracing::debug!("DPI awareness not changed: {e}");
    }
}

/// Register [`AUMID`] for the current user with a display name. Idempotent.
fn register_aumid() -> windows::core::Result<()> {
    let subkey = HSTRING::from(format!(r"Software\Classes\AppUserModelId\{AUMID}"));
    let mut key = HKEY::default();
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &subkey,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
        .ok()?;
        let name: Vec<u8> = "Linux Link\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let set = RegSetValueExW(key, w!("DisplayName"), None, REG_SZ, Some(&name));
        let _ = RegCloseKey(key);
        set.ok()
    }
}

/// Escape text for an XML text node.
fn xml_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The toast payload: a title line and a body line.
fn toast_xml(summary: &str, body: &str) -> String {
    format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
        xml_text(summary),
        xml_text(body)
    )
}

fn show_toast(summary: &str, body: &str) -> windows::core::Result<()> {
    // The calling thread may never have touched WinRT; an apartment that is
    // already initialized makes this a no-op error worth ignoring.
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    register_aumid()?;
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(toast_xml(summary, body)))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?.Show(&toast)
}

pub(super) fn notify(summary: &str, body: &str) -> bool {
    match show_toast(summary, body) {
        Ok(()) => true,
        Err(e) => {
            tracing::debug!("toast notification failed: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::HiDpi::{AreDpiAwarenessContextsEqual, GetThreadDpiAwarenessContext};

    #[test]
    fn the_process_ends_up_per_monitor_v2_aware() {
        init_process();
        let current = unsafe { GetThreadDpiAwarenessContext() };
        assert!(
            unsafe {
                AreDpiAwarenessContextsEqual(current, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
            }
            .as_bool()
        );
    }

    #[test]
    fn toast_text_is_escaped_and_loads_as_xml() {
        let xml = toast_xml("PIN <1234> & go", "a > b");
        assert!(xml.contains("PIN &lt;1234&gt; &amp; go"));
        let doc = XmlDocument::new().unwrap();
        doc.LoadXml(&HSTRING::from(xml))
            .expect("well-formed toast XML");
    }

    /// Live: a toast actually reaches the Action Center. Run on a desktop:
    /// `cargo test -p linux-link-server --lib a_toast_is_shown -- --ignored`.
    #[test]
    #[ignore = "shows a real toast on an interactive Windows desktop"]
    fn a_toast_is_shown() {
        assert!(notify(
            "Linux Link test",
            "If you can read this, toasts work."
        ));
    }
}
