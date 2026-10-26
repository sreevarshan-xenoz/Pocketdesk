//! The computers that told this phone it is not paired with them (BB1): a
//! computer refuses an unpaired phone's packets on the control link, and
//! kept no word of it before, so the phone went on sending its
//! notifications, texts and clipboard there and counting them delivered.
//! Kept by the address the phone dialed, in memory: the next refusal after
//! a restart says it again. Anything else from that computer (it sends a
//! phone it refuses nothing but pairing's answers), or a pairing, clears it.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

pub(crate) use pocketdesk_core::protocol::connection::NOT_PAIRED;

static SAID: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

fn said_set() -> std::sync::MutexGuard<'static, HashSet<String>> {
    SAID.lock().unwrap_or_else(|e| e.into_inner())
}

/// The computer at `address` said this phone is not paired with it.
pub(crate) fn said(address: &str) {
    if said_set().insert(address.to_string()) {
        tracing::info!(peer = %address, "The computer says this phone is not paired with it");
    }
}

/// The computer at `address` let this phone in (it sent something else, or
/// they paired).
pub(crate) fn let_in(address: &str) {
    if said_set().remove(address) {
        tracing::info!(peer = %address, "The computer lets this phone in again");
    }
}

/// Whether the computer at `address` said, when last heard, that this phone
/// is not paired with it.
pub fn by(address: &str) -> bool {
    said_set().contains(address)
}

/// Whether `packet_type` is pairing's own (asked, or answered): the two the
/// computer acts on before pairing, exactly (BB2 review).
fn pairing(packet_type: &str) -> bool {
    matches!(
        packet_type,
        "kdeconnect.pair" | "kdeconnect.pocketdesk.pair"
    )
}

/// What a packet of `packet_type` from the computer at `address` says of
/// this phone's standing there: its word that it is not paired, or, for
/// anything but pairing's own answers, that it is let in.
pub(crate) fn heard(address: &str, packet_type: &str) {
    if packet_type == NOT_PAIRED {
        said(address);
    } else if !pairing(packet_type) {
        let_in(address);
    }
}

/// Why a send goes nowhere: the computer said the phone is not paired.
pub(crate) const REFUSED: &str = "That computer unpaired this phone: pair again, or remove it";

/// Whether a packet of `packet_type` stays on the phone, as the computer at
/// `address` said this phone is not paired with it (BB2: its notifications,
/// texts and clipboard went on, dropped there, and were counted delivered).
/// Pairing still goes.
pub(crate) fn refuses(address: &str, packet_type: &str) -> bool {
    !pairing(packet_type) && by(address)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BB1: the computer's word is kept until it lets the phone in.
    #[test]
    fn a_computer_s_word_is_kept_until_it_lets_the_phone_in() {
        let address = "192.0.2.90";
        assert!(!by(address));
        heard(address, NOT_PAIRED);
        assert!(by(address));
        heard(address, "kdeconnect.pair");
        heard(address, "kdeconnect.pocketdesk.pair");
        assert!(by(address), "pairing's answers say nothing either way");
        heard(address, "kdeconnect.pocketdesk.endpoint");
        assert!(!by(address), "a push is only for a phone let in");
        said(address);
        let_in(address);
        assert!(!by(address), "paired again");
        assert!(!by("192.0.2.91"), "only that computer");
    }

    /// BB2 review: each way the phone sends to a computer refuses at once,
    /// before any connection, when that computer said so.
    #[tokio::test]
    async fn every_send_path_keeps_it_on_the_phone() {
        use pocketdesk_core::protocol::kdeconnect::NetworkPacket;
        let address = "192.0.2.94";
        said(address);
        let packet = NetworkPacket::new("kdeconnect.notification");
        let refused = Err(REFUSED.to_string());
        assert_eq!(
            crate::api::push_packet(address, 1716, &packet).await,
            refused
        );
        assert_eq!(
            crate::api::send_on_live(address, 1716, &packet).await,
            refused
        );
        assert_eq!(
            crate::extra_links::send(address, 1716, "kdeconnect.notification", "{}").await,
            refused
        );
        assert!(matches!(
            crate::download::send_watching_replies(address, 1716, &NetworkPacket::new("kdeconnect.share.request")).await,
            Err(e) if e == REFUSED
        ));
        assert!(crate::api::refused_request(address, &packet).await);
        let_in(address);
    }

    /// BB2: nothing but pairing goes to a computer that said so.
    #[test]
    fn nothing_but_pairing_goes_to_a_computer_that_said_so() {
        let address = "192.0.2.92";
        assert!(!refuses(address, "kdeconnect.notification"));
        said(address);
        assert!(refuses(address, "kdeconnect.notification"));
        assert!(refuses(address, "kdeconnect.pocketdesk.sms"));
        assert!(!refuses(address, "kdeconnect.pair"));
        assert!(!refuses(address, "kdeconnect.pocketdesk.pair"));
        assert!(
            !refuses("192.0.2.93", "kdeconnect.notification"),
            "only that computer"
        );
        let_in(address);
        assert!(!refuses(address, "kdeconnect.notification"));
    }
}
