//! A key as Windows' low-level keyboard hook names it, as the evdev code
//! the wire carries: the table lives in core
//! ([`pocketdesk_core::input::windows_scan`]), shared with the desktop
//! app's keyboard grab (PC4); its round trip through `stroke_for` is
//! tested here, beside `stroke_for`.

pub use pocketdesk_core::input::windows_scan::evdev_from_scan;

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// Every key the phone contract names comes back as itself from the
    /// stroke Windows presses it with: scancodes directly, and the keys
    /// pressed by virtual key from the scancodes a keyboard sends for them.
    #[test]
    fn every_contract_key_round_trips_through_stroke_for() {
        use crate::input_sendinput::{Stroke, stroke_for};
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK, VK_NUMLOCK, VK_PAUSE,
            VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP,
        };
        // What a keyboard's hook reports for the keys pressed by virtual key.
        let scan_of_vk = |vk| match vk {
            v if v == VK_NUMLOCK => (0x45, true),
            v if v == VK_PAUSE => (0x45, false),
            v if v == VK_VOLUME_MUTE => (0x20, true),
            v if v == VK_VOLUME_DOWN => (0x2E, true),
            v if v == VK_VOLUME_UP => (0x30, true),
            v if v == VK_MEDIA_NEXT_TRACK => (0x19, true),
            v if v == VK_MEDIA_PLAY_PAUSE => (0x22, true),
            v if v == VK_MEDIA_PREV_TRACK => (0x10, true),
            other => panic!("no scancode known for {other:?}"),
        };
        for code in pocketdesk_core::input::keys::phone_emittable_codes() {
            let stroke = stroke_for(code).unwrap_or_else(|| panic!("{code} has no stroke"));
            let (scan, extended) = match stroke {
                Stroke::Scan(scan, extended) => (scan, extended),
                Stroke::Vk(vk) => scan_of_vk(vk),
            };
            assert_eq!(
                evdev_from_scan(scan, extended),
                Some(code),
                "{code} as {scan:#x}/{extended}"
            );
        }
    }
}
