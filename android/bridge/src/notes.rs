//! A note to the computer (BF4): `note {text}`, answered `note_saved`.

use std::time::Duration;

use pocketdesk_core::protocol::kdeconnect::NetworkPacket;
use pocketdesk_core::protocol::note::{NOTE, NOTE_SAVED, clean_note, saved_of};

/// How long the computer has to say it kept the note.
const ANSWER_WITHIN: Duration = Duration::from_secs(10);

/// The note's packet, or `empty` when nothing is left of `text`.
fn note_packet(text: &str) -> Result<NetworkPacket, String> {
    let text = clean_note(text).ok_or_else(|| "empty".to_string())?;
    Ok(NetworkPacket::new(NOTE).with_body(serde_json::json!({ "text": text })))
}

/// Add `text` to the notes file of the computer at `address`. Its refusal
/// is a reason alone (`no_file`, `empty`, `not_written`), for the app to
/// word; any other error comes as it is.
pub async fn send_note(address: String, port: u16, text: String) -> Result<(), String> {
    let packet = note_packet(&text)?;
    let answer = crate::api::query_peer(&address, port, &packet, NOTE_SAVED, ANSWER_WITHIN).await?;
    saved_of(&answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_goes_cleaned_and_an_empty_one_does_not_go() {
        let packet = note_packet("  Buy milk\r\n").unwrap();
        assert_eq!(packet.packet_type, NOTE);
        assert_eq!(packet.body["text"], "Buy milk");
        assert_eq!(note_packet(" \n ").err().as_deref(), Some("empty"));
    }
}
