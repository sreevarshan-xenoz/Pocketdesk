//! A KDE Connect device's identity line and link packets
//! (`server/src/kdeconnect_compat/`).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    pocketdesk_server::fuzzing::kdeconnect(data);
});
