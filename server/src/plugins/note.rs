//! Notes from the phone (BF4): `note {text}` is appended, with the time, to
//! the Markdown file picked in Settings (`notes_file`), and answered
//! `note_saved`. With no file picked, the phone hears so. Under the Files
//! permission; a note's words are never logged.

use std::path::Path;

use pocketdesk_core::error::Result;
use pocketdesk_core::local::notes;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use pocketdesk_core::protocol::note::{NOTE, NOTE_SAVED, clean_note, saved_body};

/// Keep the note in `body` in the file `notes_file` names; why not, if not.
fn keep(
    body: &serde_json::Value,
    notes_file: &str,
    stamp: &str,
) -> std::result::Result<(), &'static str> {
    let notes_file = notes_file.trim();
    if notes_file.is_empty() {
        return Err("no_file");
    }
    let Some(note) = body
        .get("text")
        .and_then(|v| v.as_str())
        .and_then(clean_note)
    else {
        return Err("empty");
    };
    notes::append(Path::new(notes_file), &notes::entry(&note, stamp)).map_err(|e| {
        tracing::warn!("A note from a phone was not written: {e}");
        "not_written"
    })
}

/// The plugin keeping the phones' notes.
pub struct NotePlugin;

#[async_trait::async_trait]
impl Plugin for NotePlugin {
    fn name(&self) -> &'static str {
        "note"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[NOTE]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[NOTE_SAVED]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type != NOTE {
            return Ok(());
        }
        let config = crate::events::current_config();
        let stamp = notes::local_stamp().unwrap_or_else(|| "?".to_string());
        let body = packet.body.clone();
        let file = config.notes_file.clone();
        let kept = tokio::task::spawn_blocking(move || keep(&body, &file, &stamp))
            .await
            .unwrap_or(Err("not_written"));
        match kept {
            Ok(()) => tracing::info!("A note from a phone was saved"),
            Err(reason) => tracing::info!("A note from a phone was not saved: {reason}"),
        }
        sender
            .send_packet(&NetworkPacket::new(NOTE_SAVED).with_body(saved_body(kept.err())))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_note_is_kept_in_the_picked_file_or_told_why_not() {
        let dir = std::env::temp_dir().join(format!("pd-note-plugin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("inbox.md");
        let path = file.display().to_string();
        assert_eq!(keep(&json!({"text": "Buy milk"}), &path, "t1"), Ok(()));
        assert_eq!(
            keep(&json!({"text": "Buy milk"}), "  ", "t1"),
            Err("no_file")
        );
        assert_eq!(keep(&json!({"text": " \n "}), &path, "t1"), Err("empty"));
        assert_eq!(keep(&json!({}), &path, "t1"), Err("empty"));
        let gone = dir.join("gone").join("inbox.md").display().to_string();
        assert_eq!(keep(&json!({"text": "x"}), &gone, "t1"), Err("not_written"));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "- **t1** Buy milk\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
