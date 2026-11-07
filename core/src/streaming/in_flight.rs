//! The frames a sender keeps on the wire, sent and not yet delivered: the
//! computer's video (R2) and the phone's cast (T10k).

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::connection::{Delivered, OutStream};

/// The fewest video frames that may be on the wire, sent and not yet
/// delivered, before the next waits. Without a bound, a slow link took every
/// frame into the connection's send buffer (megabytes), where nothing could
/// trim the stale ones and the phone waited behind them.
pub const MAX_VIDEO_IN_FLIGHT: usize = 8;
/// The most, whatever the round trip measures.
pub const MAX_IN_FLIGHT_CEILING: usize = 64;

/// How many frames to keep on the wire at `fps` over a path whose round
/// trip is `rtt`: a frame counts as delivered about one round trip (plus
/// the peer's ack delay) after it is sent, so a window of a round trip's
/// frames does not throttle a healthy far link, and a slow one still fills
/// it and waits.
pub fn in_flight_window(fps: u32, rtt: Duration) -> usize {
    let per_trip = (rtt + Duration::from_millis(50)).as_secs_f64() * f64::from(fps);
    (per_trip.ceil() as usize).clamp(MAX_VIDEO_IN_FLIGHT, MAX_IN_FLIGHT_CEILING)
}

/// The video frames on the wire ([`in_flight_window`]).
pub struct InFlight {
    max: usize,
    /// Each frame's delivery, and its stream when it may be reset
    /// ([`InFlight::moot`]).
    pending: Vec<(Delivered, Option<Box<dyn OutStream>>)>,
}

impl InFlight {
    pub fn new(max: usize) -> Self {
        Self {
            max: max.max(1),
            pending: Vec::new(),
        }
    }

    pub fn set_max(&mut self, max: usize) {
        self.max = max.max(1);
    }

    /// Wait for room for one more frame; false when `cancel` ended the wait.
    /// Any frame delivered makes room, not only the oldest: one repairing a
    /// lost packet must not hold up those behind it already delivered.
    pub async fn admit(&mut self, cancel: &CancellationToken) -> bool {
        let max = self.max;
        let pending = &mut self.pending;
        let room = std::future::poll_fn(|cx| {
            pending.retain_mut(|(delivered, _)| delivered.as_mut().poll(cx).is_pending());
            if pending.len() < max {
                std::task::Poll::Ready(())
            } else {
                std::task::Poll::Pending
            }
        });
        tokio::select! {
            biased;
            _ = cancel.cancelled() => false,
            _ = room => true,
        }
    }

    /// A frame just sent, delivered when `delivered` completes.
    pub fn push(&mut self, delivered: Delivered) {
        self.pending.push((delivered, None));
    }

    /// A frame just sent on `stream` (finished), kept so a keyframe after
    /// it can reset it ([`InFlight::moot`]).
    pub fn push_stream(&mut self, stream: Box<dyn OutStream>) {
        self.pending.push((stream.delivered(), Some(stream)));
    }

    /// A keyframe is about to go (BG3): every frame still on the wire is
    /// moot, so each stream kept is reset with `code` and none waits for
    /// room any more. How many were reset.
    pub fn moot(&mut self, code: u32) -> usize {
        let mut reset = 0;
        for (_, stream) in self.pending.drain(..) {
            if let Some(mut stream) = stream {
                stream.reset(code);
                reset += 1;
            }
        }
        reset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The transport keeps at most so many video frames on the wire: on a
    /// slow link the next waits for the oldest to be delivered, so frames
    /// queue where they can be trimmed, not inside the connection.
    #[tokio::test]
    async fn a_later_frame_delivered_makes_room_while_the_oldest_is_repaired() {
        let cancel = CancellationToken::new();
        let mut in_flight = InFlight::new(2);
        let (_oldest_done, oldest) = tokio::sync::oneshot::channel::<()>();
        let (later_done, later) = tokio::sync::oneshot::channel::<()>();
        in_flight.push(Box::pin(async move {
            let _ = oldest.await;
        }));
        in_flight.push(Box::pin(async move {
            let _ = later.await;
        }));
        later_done.send(()).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), in_flight.admit(&cancel))
                .await
                .expect("room once any frame is delivered, not only the oldest")
        );
    }

    /// BG3: a keyframe makes the frames on the wire moot; their streams are
    /// reset and the window is free at once.
    #[tokio::test]
    async fn a_keyframe_resets_the_frames_still_on_the_wire() {
        use std::sync::{Arc, Mutex};
        struct Kept(Arc<Mutex<Vec<u32>>>);
        #[async_trait::async_trait]
        impl OutStream for Kept {
            async fn write_all(
                &mut self,
                _: &[u8],
            ) -> Result<(), super::super::connection::ConnectionError> {
                Ok(())
            }
            fn finish(&mut self) -> Result<(), super::super::connection::ConnectionError> {
                Ok(())
            }
            fn delivered(&self) -> Delivered {
                Box::pin(std::future::pending())
            }
            fn reset(&mut self, code: u32) {
                self.0.lock().unwrap().push(code);
            }
        }
        let resets = Arc::new(Mutex::new(Vec::new()));
        let cancel = CancellationToken::new();
        let mut in_flight = InFlight::new(2);
        in_flight.push_stream(Box::new(Kept(resets.clone())));
        in_flight.push_stream(Box::new(Kept(resets.clone())));
        in_flight.push(Box::pin(std::future::pending()));
        assert_eq!(in_flight.moot(7), 2, "only the streams kept are reset");
        assert_eq!(*resets.lock().unwrap(), [7, 7]);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), in_flight.admit(&cancel))
                .await
                .expect("the window is free")
        );
    }

    #[test]
    fn the_window_covers_a_round_trip_of_frames() {
        // A LAN round trip: the floor.
        assert_eq!(
            in_flight_window(60, Duration::from_millis(5)),
            MAX_VIDEO_IN_FLIGHT
        );
        // A relayed one: enough frames for the round trip plus the ack delay.
        assert_eq!(in_flight_window(60, Duration::from_millis(250)), 18);
        assert!(in_flight_window(30, Duration::from_millis(150)) >= 6);
        // Never unbounded, whatever the measurement says.
        assert_eq!(
            in_flight_window(240, Duration::from_secs(10)),
            MAX_IN_FLIGHT_CEILING
        );
    }

    #[tokio::test]
    async fn a_frame_waits_while_the_link_still_carries_the_last_ones() {
        let cancel = CancellationToken::new();
        let mut in_flight = InFlight::new(2);
        let (done_a, a) = tokio::sync::oneshot::channel::<()>();
        let (_done_b, b) = tokio::sync::oneshot::channel::<()>();
        assert!(in_flight.admit(&cancel).await, "room for the first");
        in_flight.push(Box::pin(async move {
            let _ = a.await;
        }));
        assert!(in_flight.admit(&cancel).await, "room for the second");
        in_flight.push(Box::pin(async move {
            let _ = b.await;
        }));
        // Full: the third waits until one of the two is delivered.
        let third =
            tokio::time::timeout(Duration::from_millis(100), in_flight.admit(&cancel)).await;
        assert!(third.is_err(), "admitted while two were on the wire");
        done_a.send(()).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), in_flight.admit(&cancel))
                .await
                .expect("admitted once the oldest was delivered")
        );
        // A session ending stops the wait.
        in_flight.push(Box::pin(std::future::pending()));
        cancel.cancel();
        assert!(!in_flight.admit(&cancel).await, "cancelled, not admitted");
    }
}
