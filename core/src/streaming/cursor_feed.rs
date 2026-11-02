//! The pointer of the computer that is shown, as its capture finds it, on
//! its way to the viewer that draws it ([`crate::protocol::cursor`]).
//!
//! A capture backend that can take the pointer out of the picture (the
//! PipeWire stream's cursor metadata, a wlroots compositor's cursor
//! capture session) reports it to a [`CursorFeed`]: a shape when the image
//! changes, whether it shows, and, while the viewer asked for places, where
//! it is. The feed keeps only the latest of each, so a backend can report
//! as often as it likes at the cost of a lock, and a pointer that does not
//! change costs nothing at all. The session's sender takes what changed
//! since its last look ([`CursorFeed::take`]) and turns it into messages
//! ([`plan`]): a shape the viewer already holds goes as its name.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::protocol::cursor::{CursorMessage, CursorShape, ShapeCache};

/// The shortest time between two places sent (about 120 a second).
pub const PLACE_EVERY: std::time::Duration = std::time::Duration::from_millis(8);

#[derive(Default)]
struct State {
    separate: bool,
    shape: Option<Arc<CursorShape>>,
    shape_gen: u64,
    visible: bool,
    visible_gen: u64,
    place: Option<(u16, u16)>,
    place_gen: u64,
}

struct Inner {
    state: Mutex<State>,
    wake: Notify,
    /// The viewer wants places (it locked its pointer).
    tracking: AtomicBool,
}

/// Where a capture reports the pointer and the session's sender reads it.
/// Cheap to clone: the capture holds one, the sender another.
#[derive(Clone)]
pub struct CursorFeed {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for CursorFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CursorFeed").finish_non_exhaustive()
    }
}

impl Default for CursorFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl CursorFeed {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    visible: true,
                    ..State::default()
                }),
                wake: Notify::new(),
                tracking: AtomicBool::new(false),
            }),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The capture takes the pointer out of the picture, and reports it
    /// here. Said once, as the capture starts.
    pub fn set_separate(&self, separate: bool) {
        self.state().separate = separate;
        self.inner.wake.notify_one();
    }

    /// Whether the capture reported that it takes the pointer out.
    pub fn separate(&self) -> bool {
        self.state().separate
    }

    /// The pointer's image changed. The same image as the one held is no
    /// change.
    pub fn set_shape(&self, shape: CursorShape) {
        {
            let mut state = self.state();
            if state.shape.as_ref().is_some_and(|s| s.id == shape.id) {
                return;
            }
            state.shape = Some(Arc::new(shape));
            state.shape_gen += 1;
        }
        self.inner.wake.notify_one();
    }

    /// The pointer shows or hides.
    pub fn set_visible(&self, visible: bool) {
        {
            let mut state = self.state();
            if state.visible == visible {
                return;
            }
            state.visible = visible;
            state.visible_gen += 1;
        }
        self.inner.wake.notify_one();
    }

    /// The pointer's hotspot is at `x`, `y` over the picture (`0..=65535`).
    /// Kept always, woken for only while the viewer asked for places.
    pub fn set_place(&self, x: u16, y: u16) {
        {
            let mut state = self.state();
            if state.place == Some((x, y)) {
                return;
            }
            state.place = Some((x, y));
            state.place_gen += 1;
        }
        if self.tracking() {
            self.inner.wake.notify_one();
        }
    }

    /// The viewer asks for places (or no more).
    pub fn set_tracking(&self, on: bool) {
        self.inner.tracking.store(on, Ordering::Release);
        self.inner.wake.notify_one();
    }

    pub fn tracking(&self) -> bool {
        self.inner.tracking.load(Ordering::Acquire)
    }

    /// Wait until something may have changed.
    pub async fn changed(&self) {
        self.inner.wake.notified().await;
    }

    /// What changed since `marks` last looked, and the marks moved up to
    /// now. A place is taken only while the viewer asked for places.
    pub fn take(&self, marks: &mut Marks) -> Pending {
        let state = self.state();
        let mut pending = Pending::default();
        if state.shape_gen != marks.shape {
            marks.shape = state.shape_gen;
            pending.shape = state.shape.clone();
        }
        if state.visible_gen != marks.visible || !marks.visible_sent {
            marks.visible = state.visible_gen;
            marks.visible_sent = true;
            pending.visible = Some(state.visible);
        }
        if self.tracking() && state.place_gen != marks.place {
            marks.place = state.place_gen;
            pending.place = state.place;
        }
        pending
    }
}

/// The viewer's side: the pointer messages a session received, waiting for
/// the window's relay to take them. Bounded, and a message that arrives
/// after a newer one (each rides a stream of its own) is dropped.
#[derive(Debug)]
pub struct CursorInbox {
    wanted: AtomicBool,
    queue: Mutex<Queue>,
    wake: Notify,
}

#[derive(Debug, Default)]
struct Queue {
    last: Option<u64>,
    messages: std::collections::VecDeque<CursorMessage>,
}

impl CursorInbox {
    /// The most messages kept waiting: a relay that falls behind keeps the
    /// newest.
    pub const MAX: usize = 256;

    /// `wanted`: the session asked the other computer for its pointer
    /// ([`crate::streaming::client::CODEC_CAP_LOCAL_CURSOR`]); a stream of
    /// pointer messages on a session that did not ask is not read.
    pub fn new(wanted: bool) -> Self {
        Self {
            wanted: AtomicBool::new(wanted),
            queue: Mutex::new(Queue::default()),
            wake: Notify::new(),
        }
    }

    pub fn wanted(&self) -> bool {
        self.wanted.load(Ordering::Acquire)
    }

    /// One message, from the stream numbered `sequence`. False when it was
    /// late (a newer one came first) and so dropped.
    pub fn push(&self, sequence: u64, message: CursorMessage) -> bool {
        {
            let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
            if queue.last.is_some_and(|last| sequence <= last) {
                return false;
            }
            queue.last = Some(sequence);
            if queue.messages.len() >= Self::MAX {
                queue.messages.pop_front();
            }
            queue.messages.push_back(message);
        }
        self.wake.notify_one();
        true
    }

    /// Every message waiting, oldest first.
    pub fn drain(&self) -> Vec<CursorMessage> {
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        queue.messages.drain(..).collect()
    }

    /// Wait until a message arrived.
    pub async fn arrived(&self) {
        self.wake.notified().await;
    }
}

impl Default for CursorInbox {
    fn default() -> Self {
        Self::new(false)
    }
}

/// How far a sender has read a feed.
#[derive(Debug, Default)]
pub struct Marks {
    shape: u64,
    visible: u64,
    visible_sent: bool,
    place: u64,
}

/// What changed in a feed since its reader last looked.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pending {
    pub shape: Option<Arc<CursorShape>>,
    pub visible: Option<bool>,
    pub place: Option<(u16, u16)>,
}

impl Pending {
    pub fn is_empty(&self) -> bool {
        self.shape.is_none() && self.visible.is_none() && self.place.is_none()
    }
}

/// The messages for what changed, in the order a viewer needs them: the
/// shape (whole the first time, by its name once `sent` holds it), then
/// whether it shows, then where it is. `sent` is what this viewer holds,
/// kept in step with the viewer's own cache.
pub fn plan(pending: &Pending, sent: &mut ShapeCache) -> Vec<CursorMessage> {
    let mut out = Vec::new();
    if let Some(shape) = &pending.shape {
        if sent.contains(shape.id) {
            out.push(CursorMessage::Use { id: shape.id });
        } else {
            sent.put(shape.clone());
            out.push(CursorMessage::Shape((**shape).clone()));
        }
    }
    if let Some(visible) = pending.visible {
        out.push(CursorMessage::Visible(visible));
    }
    if let Some((x, y)) = pending.place {
        out.push(CursorMessage::Move { x, y });
    }
    out
}

/// A place over a picture of `width` by `height` pixels as the wire says it
/// (`0..=65535`), the hotspot at pixel `x`, `y` (outside the picture, to
/// its edge).
pub fn normalized(x: i32, y: i32, width: u32, height: u32) -> (u16, u16) {
    let norm = |v: i32, size: u32| -> u16 {
        if size <= 1 {
            return 0;
        }
        let v = i64::from(v).clamp(0, i64::from(size) - 1);
        ((v * 65535 + i64::from(size - 1) / 2) / i64::from(size - 1)) as u16
    };
    (norm(x, width), norm(y, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(seed: u8) -> CursorShape {
        CursorShape::new(2, 2, 0, 0, vec![seed; 16]).unwrap()
    }

    #[test]
    fn a_new_feed_says_the_pointer_shows_once_and_nothing_else() {
        let feed = CursorFeed::new();
        let mut marks = Marks::default();
        let first = feed.take(&mut marks);
        assert_eq!(first.visible, Some(true));
        assert!(first.shape.is_none() && first.place.is_none());
        assert!(feed.take(&mut marks).is_empty(), "nothing changed since");
    }

    #[test]
    fn only_what_changed_is_taken_and_the_same_image_is_no_change() {
        let feed = CursorFeed::new();
        let mut marks = Marks::default();
        feed.take(&mut marks);
        feed.set_shape(shape(1));
        let got = feed.take(&mut marks);
        assert_eq!(got.shape.as_ref().map(|s| s.id), Some(shape(1).id));
        assert!(got.visible.is_none());
        feed.set_shape(shape(1));
        assert!(feed.take(&mut marks).is_empty(), "the same image again");
        feed.set_visible(true);
        assert!(feed.take(&mut marks).is_empty(), "still showing");
        feed.set_visible(false);
        assert_eq!(feed.take(&mut marks).visible, Some(false));
    }

    #[test]
    fn places_are_taken_only_while_the_viewer_asked_for_them() {
        let feed = CursorFeed::new();
        let mut marks = Marks::default();
        feed.take(&mut marks);
        feed.set_place(10, 20);
        assert!(feed.take(&mut marks).place.is_none(), "not asked for");
        feed.set_tracking(true);
        assert_eq!(
            feed.take(&mut marks).place,
            Some((10, 20)),
            "the latest one, on asking"
        );
        feed.set_place(11, 20);
        feed.set_place(12, 20);
        assert_eq!(
            feed.take(&mut marks).place,
            Some((12, 20)),
            "only the newest"
        );
        assert!(feed.take(&mut marks).is_empty());
        feed.set_tracking(false);
        feed.set_place(13, 20);
        assert!(feed.take(&mut marks).place.is_none());
    }

    #[test]
    fn a_shape_the_viewer_holds_goes_as_its_name() {
        let mut sent = ShapeCache::default();
        let arrow = Arc::new(shape(1));
        let text = Arc::new(shape(2));
        let pending = |s: &Arc<CursorShape>| Pending {
            shape: Some(s.clone()),
            ..Pending::default()
        };
        assert!(matches!(
            plan(&pending(&arrow), &mut sent)[..],
            [CursorMessage::Shape(_)]
        ));
        assert!(matches!(
            plan(&pending(&text), &mut sent)[..],
            [CursorMessage::Shape(_)]
        ));
        assert_eq!(
            plan(&pending(&arrow), &mut sent),
            vec![CursorMessage::Use { id: arrow.id }],
            "back to the arrow: its name alone"
        );
    }

    #[test]
    fn the_plan_orders_shape_then_visibility_then_place() {
        let mut sent = ShapeCache::default();
        let pending = Pending {
            shape: Some(Arc::new(shape(3))),
            visible: Some(true),
            place: Some((1, 2)),
        };
        let messages = plan(&pending, &mut sent);
        assert!(matches!(messages[0], CursorMessage::Shape(_)));
        assert_eq!(messages[1], CursorMessage::Visible(true));
        assert_eq!(messages[2], CursorMessage::Move { x: 1, y: 2 });
        assert!(plan(&Pending::default(), &mut sent).is_empty());
    }

    #[test]
    fn a_place_is_normalized_over_the_picture_and_kept_inside_it() {
        assert_eq!(normalized(0, 0, 1920, 1080), (0, 0));
        assert_eq!(normalized(1919, 1079, 1920, 1080), (65535, 65535));
        assert_eq!(normalized(-50, 5000, 1920, 1080), (0, 65535));
        let (x, _) = normalized(960, 0, 1921, 1080);
        assert_eq!(x, 32768);
        assert_eq!(normalized(5, 5, 1, 1), (0, 0), "a one-pixel picture");
    }

    #[test]
    fn the_inbox_keeps_messages_in_order_and_drops_a_late_one() {
        let inbox = CursorInbox::new(true);
        assert!(inbox.wanted() && !CursorInbox::default().wanted());
        assert!(inbox.push(5, CursorMessage::Visible(false)));
        assert!(inbox.push(7, CursorMessage::Visible(true)));
        assert!(
            !inbox.push(6, CursorMessage::Visible(false)),
            "late: a newer one came"
        );
        assert!(
            !inbox.push(7, CursorMessage::Visible(false)),
            "the same stream twice"
        );
        assert_eq!(
            inbox.drain(),
            vec![CursorMessage::Visible(false), CursorMessage::Visible(true)]
        );
        assert!(inbox.drain().is_empty());
    }

    #[test]
    fn a_relay_that_falls_behind_keeps_the_newest_messages() {
        let inbox = CursorInbox::new(true);
        for i in 0..(CursorInbox::MAX as u64 + 10) {
            inbox.push(i + 1, CursorMessage::Move { x: i as u16, y: 0 });
        }
        let got = inbox.drain();
        assert_eq!(got.len(), CursorInbox::MAX);
        assert_eq!(got[0], CursorMessage::Move { x: 10, y: 0 });
    }

    #[tokio::test]
    async fn the_sender_is_woken_by_a_change_and_not_by_a_same_one() {
        let feed = CursorFeed::new();
        let waiting = feed.clone();
        feed.set_shape(shape(5));
        tokio::time::timeout(std::time::Duration::from_secs(2), waiting.changed())
            .await
            .expect("woken by the new shape");
        feed.set_shape(shape(5));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), waiting.changed())
                .await
                .is_err(),
            "the same image wakes no one"
        );
    }
}
