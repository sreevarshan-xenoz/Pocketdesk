//! The newest captured frame, for the encoder (BG2).
//!
//! Capture had pushed into a channel of two and, when it was full, dropped
//! the frame it had just made: the encoder then worked through older ones,
//! and the last change on the screen could wait for the next repeat (up to
//! a second on a still screen). This holds one frame, and a new one replaces
//! the one waiting, so the encoder always takes the newest. The senders keep
//! the channel's `try_send`, so a backend's code does not change; `Full` is
//! never returned. How many frames were replaced is counted for the
//! session's record.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;
use tokio::sync::mpsc::error::TrySendError;

struct Slot<T> {
    waiting: Mutex<Option<T>>,
    wake: Notify,
    senders: AtomicUsize,
    receiver_gone: AtomicBool,
    replaced: AtomicU64,
}

/// A slot for the newest frame: its sender (clone it for each producer) and
/// its one receiver.
pub fn channel<T>() -> (FrameSender<T>, FrameReceiver<T>) {
    let slot = Arc::new(Slot {
        waiting: Mutex::new(None),
        wake: Notify::new(),
        senders: AtomicUsize::new(1),
        receiver_gone: AtomicBool::new(false),
        replaced: AtomicU64::new(0),
    });
    (FrameSender(slot.clone()), FrameReceiver(slot))
}

/// Puts frames in the slot.
pub struct FrameSender<T>(Arc<Slot<T>>);

impl<T> FrameSender<T> {
    /// Put `frame` in the slot, replacing one still waiting. `Closed` once
    /// the receiver is gone; never `Full`.
    pub fn try_send(&self, frame: T) -> Result<(), TrySendError<T>> {
        if self.0.receiver_gone.load(Ordering::Acquire) {
            return Err(TrySendError::Closed(frame));
        }
        let old = self
            .0
            .waiting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace(frame);
        if old.is_some() {
            self.0.replaced.fetch_add(1, Ordering::Relaxed);
        }
        self.0.wake.notify_one();
        Ok(())
    }

    /// Whether the receiver is gone.
    pub fn is_closed(&self) -> bool {
        self.0.receiver_gone.load(Ordering::Acquire)
    }
}

impl<T> Clone for FrameSender<T> {
    fn clone(&self) -> Self {
        self.0.senders.fetch_add(1, Ordering::AcqRel);
        FrameSender(self.0.clone())
    }
}

impl<T> Drop for FrameSender<T> {
    fn drop(&mut self) {
        if self.0.senders.fetch_sub(1, Ordering::AcqRel) == 1 {
            // The last one: a receiver waiting hears the slot is closed.
            self.0.wake.notify_one();
        }
    }
}

/// Takes frames from the slot.
pub struct FrameReceiver<T>(Arc<Slot<T>>);

impl<T> FrameReceiver<T> {
    /// The newest frame, waiting for one; `None` once every sender is gone
    /// and nothing waits.
    pub async fn recv(&mut self) -> Option<T> {
        loop {
            // Registered before looking, so a frame put in between wakes it.
            let woken = self.0.wake.notified();
            if let Some(frame) = self.take() {
                return Some(frame);
            }
            if self.0.senders.load(Ordering::Acquire) == 0 {
                return self.take();
            }
            woken.await;
        }
    }

    /// The frame waiting, if one is, without waiting.
    pub fn try_recv(&mut self) -> Option<T> {
        self.take()
    }

    fn take(&self) -> Option<T> {
        self.0
            .waiting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    /// How many frames were replaced by a newer one before the encoder
    /// took them, since the slot was made.
    pub fn replaced(&self) -> u64 {
        self.0.replaced.load(Ordering::Relaxed)
    }
}

impl<T> Drop for FrameReceiver<T> {
    fn drop(&mut self) {
        self.0.receiver_gone.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn the_newest_frame_wins_and_the_ones_it_replaced_are_counted() {
        let (tx, mut rx) = channel();
        tx.try_send(1).unwrap();
        tx.try_send(2).unwrap();
        tx.try_send(3).unwrap();
        assert_eq!(rx.recv().await, Some(3));
        assert_eq!(rx.replaced(), 2);
        tx.try_send(4).unwrap();
        assert_eq!(rx.recv().await, Some(4));
        assert_eq!(rx.replaced(), 2, "a frame taken was not replaced");
    }

    #[tokio::test]
    async fn the_receiver_waits_for_a_frame_put_later() {
        let (tx, mut rx) = channel();
        let put = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            tx.try_send(7).unwrap();
            tx
        });
        let got = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        assert_eq!(got.unwrap(), Some(7));
        drop(put.await.unwrap());
    }

    #[tokio::test]
    async fn the_slot_closes_once_every_sender_is_gone_after_its_last_frame() {
        let (tx, mut rx) = channel();
        let other = tx.clone();
        other.try_send(5).unwrap();
        drop(other);
        drop(tx);
        assert_eq!(rx.recv().await, Some(5), "the frame left is still taken");
        let closed = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        assert_eq!(closed.unwrap(), None);
    }

    #[tokio::test]
    async fn a_receiver_waiting_hears_the_last_sender_go() {
        let (tx, mut rx) = channel::<u8>();
        let waiting = tokio::spawn(async move { rx.recv().await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(tx);
        let got = tokio::time::timeout(Duration::from_secs(5), waiting).await;
        assert_eq!(got.unwrap().unwrap(), None);
    }

    #[test]
    fn a_frame_sent_with_the_receiver_gone_is_refused_as_closed() {
        let (tx, rx) = channel();
        drop(rx);
        assert!(tx.is_closed());
        assert!(matches!(tx.try_send(9), Err(TrySendError::Closed(9))));
    }
}
