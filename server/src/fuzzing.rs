//! Entry points for the fuzz targets in `fuzz/` (built only under
//! `cargo fuzz`, which sets `--cfg fuzzing`). Each runs a parser on bytes
//! as they would arrive, and nothing else: no state is read or written.
//! Behaviour stays where it is; these only reach `pub(crate)` parsers.

use pocketdesk_core::protocol::kdeconnect::NetworkPacket;
use tokio::io::BufReader;

/// A current-thread runtime for the readers, which are async but never
/// wait on bytes already in memory (with a clock: the handshake's line
/// reader has a deadline).
fn block_on<F: std::future::Future>(f: F) -> F::Output {
    thread_local! {
        static RUNTIME: tokio::runtime::Runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("a runtime");
    }
    RUNTIME.with(|rt| rt.block_on(f))
}

/// Shared input's wire: `bytes` as one message, which must encode back to
/// exactly `bytes` when it reads, then as a stream of frames.
pub fn shared_input(bytes: &[u8]) {
    use crate::shared_input::wire;
    if let Ok(msg) = wire::decode(bytes) {
        assert_eq!(wire::encode(&msg), bytes, "{msg:?} reads, but not back");
    }
    block_on(async {
        let mut stream = bytes;
        while let Ok(Some(_)) = wire::read_frame(&mut stream).await {}
    });
}

/// The computer link (`peer_computers`): packets read off `bytes` as its
/// framing reads them, each through the parsers its loop runs on a body.
pub fn computer_link(bytes: &[u8]) {
    block_on(async {
        let mut reader = BufReader::new(bytes);
        while let Ok(Some(packet)) = crate::peer_computers::read_packet(&mut reader).await {
            computer_link_body(&packet);
        }
    });
}

/// What the computer link's loop reads of one packet's body, before any
/// of it touches state.
fn computer_link_body(packet: &NetworkPacket) {
    let body = &packet.body;
    let _ = crate::peer_computers::is_screens_answer(body);
    let _ = crate::peer_computers::WanTarget::parse(body);
    let _ = crate::computer_wake::told(body, 1_700_000_000);
    clipboard_body(body);
    remote_devices_body(body);
    introductions_body(body);
}

/// The clipboard push's body, as the computer link and the control channel
/// read it: its text, its image, its marks.
pub fn clipboard_body(body: &serde_json::Value) {
    if let Some(content) = body.get("content").and_then(|v| v.as_str()) {
        let _ = content.len() > crate::clipboard_push::MAX_PUSH;
    }
    let _ = crate::clipboard_push::told_image(body);
    let _ = crate::clipboard_push::marked(body);
    let _ = crate::clipboard_push::of_window(body);
}

/// Remote revoke's request and answer.
pub fn remote_devices_body(body: &serde_json::Value) {
    use pocketdesk_core::protocol::remote_devices as wire;
    let _ = wire::Request::parse(body);
    let _ = wire::read_answer(body);
}

/// Introductions: the phone's ask and answer, the computer's introduction,
/// cancel and answer.
pub fn introductions_body(body: &serde_json::Value) {
    use pocketdesk_core::protocol::introductions as wire;
    let _ = wire::PhoneRequest::parse(body);
    let _ = wire::read_phone_answer(body);
    let _ = wire::Introduction::parse(body);
    let _ = wire::read_cancel(body);
    let _ = wire::read_computer_answer(body);
}

/// A KDE Connect device's first line (its plaintext identity, at most
/// `MAX_LINE`), as the handshake reads it, then the packets of a link.
pub fn kdeconnect(bytes: &[u8]) {
    use crate::kdeconnect_compat::{handshake, link, pairing, share};
    block_on(async {
        let mut first = bytes;
        let within = std::time::Duration::from_secs(1);
        if let Ok(line) = handshake::read_line(&mut first, handshake::MAX_LINE, within).await
            && let Ok(packet) = NetworkPacket::from_wire(&line)
        {
            let _ = handshake::read_hello(&packet, "00000000000000000000000000000000");
        }
        let mut packets = link::PacketReader::new(bytes);
        while let Some(packet) = packets.next().await {
            let _ = handshake::read_hello(&packet, "00000000000000000000000000000000");
            let _ = pairing::read(&packet);
            let _ = share::offer_of(&packet);
            clipboard_body(&packet.body);
        }
    });
}
