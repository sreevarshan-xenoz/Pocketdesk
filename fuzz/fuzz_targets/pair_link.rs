//! The `pocketdesk://pair` link (`core/src/local/pair_link.rs`): a link
//! that reads reads back the same from the link it makes.
#![no_main]

use libfuzzer_sys::fuzz_target;
use pocketdesk_core::local::pair_link::{MAX_LINK_BYTES, PairLink};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(link) = PairLink::from_uri(text) {
        let again = link.to_uri();
        // Raw characters it encodes can make the link it writes longer
        // than the limit; only a link within it must read back.
        if again.len() <= MAX_LINK_BYTES {
            assert_eq!(PairLink::from_uri(&again).as_ref(), Ok(&link), "{again}");
        }
    }
});
