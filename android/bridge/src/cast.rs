//! The phone's screen on the computer (T10k): the bridge's half of a cast.
//! The phone's encoder output is queued here and sent in order, one uni
//! stream per config or access unit (`core::streaming::cast`), on the cast
//! session's connection. A link that falls behind fills the queue; a frame
//! that finds it full is dropped, and the phone sends a keyframe (the
//! computer also asks for one when it sees the gap).

use std::sync::Mutex;
use std::time::Duration;

use pocketdesk_core::streaming::SharedConnection;
use pocketdesk_core::streaming::cast as wire;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Configs and frames waiting for the link.
const QUEUE: usize = 6;
/// A config waits this long for room: it must not be dropped.
const CONFIG_WAIT: Duration = Duration::from_secs(2);

/// What happened to a queued config or frame.
pub(crate) const QUEUED: i32 = 0;
/// The link is behind: the frame was dropped; send a keyframe.
pub(crate) const FULL: i32 = 1;
/// There is no cast session.
pub(crate) const NO_CAST: i32 = 2;

static QUEUE_TX: Mutex<Option<mpsc::Sender<Vec<u8>>>> = Mutex::new(None);

fn queue_tx() -> Option<mpsc::Sender<Vec<u8>>> {
    QUEUE_TX.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// A cast session was installed: send what is queued on `connection` until
/// `cancel`.
pub(crate) fn start(connection: SharedConnection, cancel: CancellationToken) {
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(QUEUE);
    *QUEUE_TX.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
    tokio::spawn(async move {
        loop {
            let bytes = tokio::select! {
                _ = cancel.cancelled() => break,
                bytes = rx.recv() => match bytes {
                    Some(bytes) => bytes,
                    None => break,
                },
            };
            if let Err(e) = send(&connection, &bytes).await {
                tracing::debug!("A cast stream was not sent: {e}");
                if connection.close_reason().is_some() {
                    break;
                }
            }
        }
    });
}

/// The cast session ended: nothing more is queued.
pub(crate) fn stop() {
    QUEUE_TX.lock().unwrap_or_else(|e| e.into_inner()).take();
}

async fn send(connection: &SharedConnection, bytes: &[u8]) -> Result<(), String> {
    let mut stream = connection.open_uni().await.map_err(|e| e.to_string())?;
    stream.write_all(bytes).await.map_err(|e| e.to_string())?;
    stream.finish().map_err(|e| e.to_string())
}

fn offer(tx: Option<mpsc::Sender<Vec<u8>>>, bytes: Vec<u8>) -> i32 {
    match tx.map(|tx| tx.try_send(bytes)) {
        None | Some(Err(mpsc::error::TrySendError::Closed(_))) => NO_CAST,
        Some(Err(mpsc::error::TrySendError::Full(_))) => FULL,
        Some(Ok(())) => QUEUED,
    }
}

/// The encoder (re)started at `width`×`height`; waits for room, as the
/// computer cannot show frames of a size it was not told.
pub(crate) async fn config(width: u16, height: u16) -> i32 {
    let Some(tx) = queue_tx() else {
        return NO_CAST;
    };
    match tokio::time::timeout(CONFIG_WAIT, tx.send(wire::config_bytes(width, height))).await {
        Ok(Ok(())) => QUEUED,
        Ok(Err(_)) => NO_CAST,
        Err(_) => FULL,
    }
}

/// One access unit; never waits.
pub(crate) fn frame(keyframe: bool, seq: u64, access_unit: &[u8]) -> i32 {
    offer(queue_tx(), wire::frame_bytes(keyframe, seq, access_unit))
}

/// Whether the computer asked for a keyframe since the last look.
pub(crate) fn keyframe_wanted() -> bool {
    crate::session::REMOTE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|r| {
            r.keyframe_wanted
                .swap(false, std::sync::atomic::Ordering::AcqRel)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_without_a_cast_or_room_says_so() {
        assert_eq!(offer(None, vec![1]), NO_CAST);
        let (tx, rx) = mpsc::channel(1);
        assert_eq!(offer(Some(tx.clone()), vec![1]), QUEUED);
        assert_eq!(offer(Some(tx.clone()), vec![2]), FULL, "the link is behind");
        drop(rx);
        assert_eq!(offer(Some(tx), vec![3]), NO_CAST, "the cast ended");
    }
}
