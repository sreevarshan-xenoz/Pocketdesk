//! Windows implementations of [`super`]'s helpers.

use windows::Data::Xml::Dom::XmlDocument;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegSetValueExW,
};
use windows::Win32::System::Threading::{
    CreateEventW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent, WaitForSingleObject,
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

/// The event a server waits on and `linux-link stop` signals. Session-local,
/// so only the same user's processes can reach it.
fn stop_event_name(pid: u32) -> HSTRING {
    HSTRING::from(format!(r"Local\LinuxLink-{pid}-stop"))
}

/// Create this process's stop event and forward its signal to `tx`.
pub(super) fn listen_for_stop_event(tx: tokio::sync::mpsc::Sender<super::ShutdownReason>) {
    let name = stop_event_name(std::process::id());
    let event = match unsafe { CreateEventW(None, true, false, &name) } {
        Ok(event) => event,
        Err(e) => {
            tracing::warn!("cannot create the stop event; `linux-link stop` will force-kill: {e}");
            return;
        }
    };
    // HANDLE is a raw pointer; the value crosses to the waiting thread.
    let raw = event.0 as isize;
    let spawned = std::thread::Builder::new()
        .name("stop-event".into())
        .spawn(move || {
            let event = windows::Win32::Foundation::HANDLE(raw as *mut _);
            unsafe { WaitForSingleObject(event, INFINITE) };
            let _ = tx.blocking_send("linux-link stop");
            let _ = unsafe { CloseHandle(event) };
        });
    if let Err(e) = spawned {
        tracing::warn!("cannot wait on the stop event: {e}");
    }
}

/// Signal the stop event of the server running as `pid`; `false` when there
/// is none (an older or wedged server).
pub(super) fn signal_stop_event(pid: u32) -> bool {
    let Ok(event) = (unsafe { OpenEventW(EVENT_MODIFY_STATE, false, &stop_event_name(pid)) })
    else {
        return false;
    };
    let set = unsafe { SetEvent(event) }.is_ok();
    let _ = unsafe { CloseHandle(event) };
    set
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

    /// Signals only this test process's own event, but it exercises the
    /// server-shutdown path, so it stays opt-in.
    #[tokio::test]
    #[ignore = "exercises the shutdown path; opt-in only"]
    async fn the_stop_event_reaches_the_shutdown_channel() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        listen_for_stop_event(tx);
        assert!(signal_stop_event(std::process::id()));
        let reason = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("stop request delivered");
        assert_eq!(reason, Some("linux-link stop"));
        assert!(
            !signal_stop_event(u32::MAX),
            "no event for a pid that is not running"
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
