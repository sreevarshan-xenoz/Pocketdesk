//! The phone's screen on the computer (T10k): the bridge's half of a cast.
//! The phone's encoder output is queued here and sent in order, one uni
//! stream per config or access unit (`core::streaming::cast`), on the cast
//! session's connection. At most a round trip's frames are on the wire,
//! sent and not yet delivered (as the computer's own video, R2): a link
//! that falls behind fills the queue, a frame that finds it full is
//! dropped, and the phone sends a keyframe (the computer also asks for one
//! when it sees the gap). Without that bound the transport took every frame
//! into its send buffer and the computer showed an ever older picture.

use std::sync::Mutex;
use std::time::Duration;

use pocketdesk_core::streaming::SharedConnection;
use pocketdesk_core::streaming::cast as wire;
use pocketdesk_core::streaming::connection::Delivered;
use pocketdesk_core::streaming::in_flight::{InFlight, MAX_VIDEO_IN_FLIGHT, in_flight_window};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Configs and frames waiting for the link.
const QUEUE: usize = 6;
/// A config waits this long for room: it must not be dropped.
const CONFIG_WAIT: Duration = Duration::from_secs(2);
/// The phone encodes about this many frames a second (`CastPlan.FRAME_RATE`).
const CAST_FPS: u32 = 30;

/// What happened to a queued config or frame.
pub(crate) const QUEUED: i32 = 0;
/// The link is behind: the frame was dropped; send a keyframe.
pub(crate) const FULL: i32 = 1;
/// There is no cast session.
pub(crate) const NO_CAST: i32 = 2;

static QUEUE_TX: Mutex<Option<mpsc::Sender<Vec<u8>>>> = Mutex::new(None);

/// The cast's sound waiting for the link (K5): its own queue and sender,
/// so it never waits behind the picture's frames; a full queue drops a
/// packet, as late sound is worse than a gap.
static AUDIO_TX: Mutex<Option<mpsc::Sender<Vec<u8>>>> = Mutex::new(None);

/// Sound packets waiting: 10, a fifth of a second.
const AUDIO_QUEUE: usize = 10;
/// Sound packets sent and not yet delivered: 5, a tenth of a second.
const AUDIO_IN_FLIGHT: usize = 5;

fn queue_tx() -> Option<mpsc::Sender<Vec<u8>>> {
    QUEUE_TX.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// A cast session was installed: send what is queued on `connection` until
/// `cancel`.
pub(crate) fn start(connection: SharedConnection, cancel: CancellationToken) {
    *AUDIO_TX.lock().unwrap_or_else(|e| e.into_inner()) =
        Some(spawn_audio_sender(connection.clone(), cancel.clone()));
    *QUEUE_TX.lock().unwrap_or_else(|e| e.into_inner()) = Some(spawn_sender(connection, cancel));
}

/// The task that sends the cast's sound, one stream a packet (K5).
fn spawn_audio_sender(
    connection: SharedConnection,
    cancel: CancellationToken,
) -> mpsc::Sender<Vec<u8>> {
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(AUDIO_QUEUE);
    tokio::spawn(async move {
        // At most a tenth of a second sent and not yet delivered (K5 review:
        // a stalled link had piled the sound up in the transport, to arrive
        // late); meanwhile the queue fills, and a full queue drops.
        let mut in_flight = InFlight::new(AUDIO_IN_FLIGHT);
        loop {
            if !in_flight.admit(&cancel).await {
                break;
            }
            let bytes = tokio::select! {
                _ = cancel.cancelled() => break,
                bytes = rx.recv() => match bytes {
                    Some(bytes) => bytes,
                    None => break,
                },
            };
            match send(&connection, &bytes).await {
                Ok(delivered) => in_flight.push(delivered),
                Err(e) => {
                    tracing::debug!("A cast's sound was not sent: {e}");
                    if connection.close_reason().is_some() {
                        break;
                    }
                }
            }
        }
    });
    tx
}

/// One Opus packet of the phone's sound (K5): queued, dropped when the
/// link is behind, or no cast.
pub(crate) fn audio(opus: &[u8]) -> i32 {
    if opus.is_empty() || opus.len() > wire::MAX_CAST_AUDIO {
        return FULL;
    }
    let tx = AUDIO_TX.lock().unwrap_or_else(|e| e.into_inner()).clone();
    offer(tx, wire::audio_bytes(opus))
}

/// The task that sends what is queued, in order, keeping at most a round
/// trip's frames undelivered; the queue it reads from.
fn spawn_sender(connection: SharedConnection, cancel: CancellationToken) -> mpsc::Sender<Vec<u8>> {
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(QUEUE);
    tokio::spawn(async move {
        let mut in_flight = InFlight::new(MAX_VIDEO_IN_FLIGHT);
        loop {
            // Room on the wire first: while there is none, frames wait in
            // the queue, where a full queue drops them.
            in_flight.set_max(in_flight_window(CAST_FPS, connection.stats().rtt));
            if !in_flight.admit(&cancel).await {
                break;
            }
            let bytes = tokio::select! {
                _ = cancel.cancelled() => break,
                bytes = rx.recv() => match bytes {
                    Some(bytes) => bytes,
                    None => break,
                },
            };
            match send(&connection, &bytes).await {
                Ok(delivered) => in_flight.push(delivered),
                Err(e) => {
                    tracing::debug!("A cast stream was not sent: {e}");
                    if connection.close_reason().is_some() {
                        break;
                    }
                }
            }
        }
    });
    tx
}

/// The cast session ended: nothing more is queued.
pub(crate) fn stop() {
    AUDIO_TX.lock().unwrap_or_else(|e| e.into_inner()).take();
    QUEUE_TX.lock().unwrap_or_else(|e| e.into_inner()).take();
}

/// Send one stream; completes its future once the computer has it all.
async fn send(connection: &SharedConnection, bytes: &[u8]) -> Result<Delivered, String> {
    let mut stream = connection.open_uni().await.map_err(|e| e.to_string())?;
    stream.write_all(bytes).await.map_err(|e| e.to_string())?;
    stream.finish().map_err(|e| e.to_string())?;
    Ok(stream.delivered())
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

/// Tell the computer whether this phone takes control now (C8); waits for
/// room, as a config does: a lost "off" would leave the computer sending.
pub(crate) async fn control_state(on: bool) -> i32 {
    let Some(tx) = queue_tx() else {
        return NO_CAST;
    };
    match tokio::time::timeout(CONFIG_WAIT, tx.send(wire::control_state_bytes(on))).await {
        Ok(Ok(())) => QUEUED,
        Ok(Err(_)) => NO_CAST,
        Err(_) => FULL,
    }
}

/// The next control batch the computer sent the cast (C8), as JSON
/// (`ControlBatch::to_json`), waiting up to `timeout`; `None` when none
/// came, there is no cast, or the batch could not be read.
pub(crate) fn take_control(timeout: Duration) -> Option<String> {
    let inbox = crate::session::REMOTE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|r| r.cast)
        .map(|r| r.control.clone());
    let Some(inbox) = inbox else {
        // No cast yet: the caller's loop waits rather than spins.
        std::thread::sleep(timeout.min(Duration::from_millis(100)));
        return None;
    };
    let batch = inbox.take(timeout)?;
    pocketdesk_core::streaming::cast_control::ControlBatch::decode(&batch)
        .map(|b| b.to_json().to_string())
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
    use pocketdesk_core::streaming::connection::{
        Connection, ConnectionError, ConnectionStats, InStream, OutStream, TransportFamily,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A link that takes every stream and delivers none of them.
    struct Stuck(Arc<AtomicUsize>);

    struct StuckStream;

    #[async_trait::async_trait]
    impl OutStream for StuckStream {
        async fn write_all(&mut self, _data: &[u8]) -> Result<(), ConnectionError> {
            Ok(())
        }
        fn finish(&mut self) -> Result<(), ConnectionError> {
            Ok(())
        }
        fn delivered(&self) -> Delivered {
            Box::pin(std::future::pending())
        }
    }

    #[async_trait::async_trait]
    impl Connection for Stuck {
        async fn open_uni(&self) -> Result<Box<dyn OutStream>, ConnectionError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(StuckStream))
        }
        async fn accept_uni(&self) -> Result<Box<dyn InStream>, ConnectionError> {
            std::future::pending().await
        }
        fn remote_address(&self) -> std::net::SocketAddr {
            "192.0.2.10:4716".parse().unwrap()
        }
        fn stats(&self) -> ConnectionStats {
            ConnectionStats::default()
        }
        fn transport_family(&self) -> TransportFamily {
            TransportFamily::Quinn
        }
        fn close(&self, _code: u32, _reason: &[u8]) {}
    }

    /// A link that delivers nothing holds a window of frames on the wire;
    /// the rest wait in the queue until it is full, and then frames are
    /// dropped (the phone sends a keyframe), not buffered without end.
    #[tokio::test]
    async fn a_link_that_falls_behind_fills_the_queue_instead_of_the_transport() {
        let opened = Arc::new(AtomicUsize::new(0));
        let cancel = CancellationToken::new();
        let tx = spawn_sender(Arc::new(Stuck(opened.clone())), cancel.clone());
        let mut queued = 0;
        let full = loop {
            match offer(Some(tx.clone()), vec![0; 10]) {
                QUEUED => queued += 1,
                result => break result,
            }
            assert!(queued <= 100, "never full: the link took everything");
            // The sender takes what it may before the next frame comes.
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(2)).await;
        };
        assert_eq!(full, FULL);
        assert_eq!(
            opened.load(Ordering::SeqCst),
            MAX_VIDEO_IN_FLIGHT,
            "a window on the wire"
        );
        assert_eq!(
            queued,
            MAX_VIDEO_IN_FLIGHT + QUEUE,
            "and the queue behind it"
        );
        cancel.cancel();
    }

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
