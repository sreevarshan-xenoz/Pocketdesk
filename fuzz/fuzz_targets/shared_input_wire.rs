//! Shared input's messages and frames (`server/src/shared_input/wire.rs`).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    pocketdesk_server::fuzzing::shared_input(data);
});
