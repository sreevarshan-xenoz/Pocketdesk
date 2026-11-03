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

/// How often the sender says the whole pointer again (the mode, the shape
/// that is current, whether it shows, where it is) whatever has changed: a
/// message is one stream of its own and may be lost, cut short or late, and
/// the viewer would otherwise hold a wrong or no pointer until the next
/// change. A few bytes a time, and none of it while the pointer never was
/// sent apart.
pub const REFRESH_EVERY: std::time::Duration = std::time::Duration::from_millis(1500);

/// How soon the whole pointer is said again after a write failed.
pub const RETRY_AFTER: std::time::Duration = std::time::Duration::from_millis(250);

#[derive(Default)]
struct State {
    /// The picture has no pointer in it, as the viewer is told.
    separate: bool,
    /// A capture holds the pointer out of its picture now
    /// ([`CursorFeed::apart`]).
    apart: bool,
    /// Which capture's hold that is, so a hold that ended late does not
    /// clear the next one's.
    apart_gen: u64,
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

    /// The picture has the pointer taken out (or not), as the viewer is to
    /// be told. A capture that takes it out for as long as it runs uses
    /// [`CursorFeed::apart`] instead, which cannot be left set.
    pub fn set_separate(&self, separate: bool) {
        let changed = std::mem::replace(&mut self.state().separate, separate) != separate;
        if changed {
            self.inner.wake.notify_one();
        }
    }

    /// A capture takes the pointer out of its picture for as long as the
    /// guard lives: the feed says so now, and says no again when the guard
    /// drops, however the capture ends (its thread stops, its session is
    /// stopped, a start fails), so the viewer never waits for a pointer
    /// that is no longer sent. A capture that can also put the pointer back
    /// in (a window's picture) says which with [`CursorFeed::want_out`].
    pub fn apart(&self) -> ApartGuard {
        let gen_ = {
            let mut state = self.state();
            state.apart_gen += 1;
            state.apart = true;
            state.separate = true;
            state.apart_gen
        };
        self.inner.wake.notify_one();
        ApartGuard {
            feed: self.clone(),
            gen_,
        }
    }

    /// Whether the next picture is asked for without the pointer: `out`
    /// is what this frame would like, and it is so only while the capture
    /// holds the pointer out ([`CursorFeed::apart`]). The viewer is told
    /// what it is, in the same step.
    pub fn want_out(&self, out: bool) -> bool {
        let (effective, changed) = {
            let mut state = self.state();
            let effective = out && state.apart;
            let changed = state.separate != effective;
            state.separate = effective;
            (effective, changed)
        };
        if changed {
            self.inner.wake.notify_one();
        }
        effective
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

    /// A pointer record as a capture reads it: the image when it came,
    /// whether the pointer shows, where it is. The same as the three `set_`
    /// calls, for one lock and one wake.
    pub fn report(&self, shape: Option<CursorShape>, visible: bool, place: Option<(u16, u16)>) {
        let wake = {
            let mut state = self.state();
            let mut wake = false;
            if let Some(shape) = shape
                && state.shape.as_ref().is_none_or(|s| s.id != shape.id)
            {
                state.shape = Some(Arc::new(shape));
                state.shape_gen += 1;
                wake = true;
            }
            if state.visible != visible {
                state.visible = visible;
                state.visible_gen += 1;
                wake = true;
            }
            if let Some(place) = place
                && state.place != Some(place)
            {
                state.place = Some(place);
                state.place_gen += 1;
                wake |= self.tracking();
            }
            wake
        };
        if wake {
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
        // A viewer that asks for places again is told where the pointer is
        // now, though it has not moved since it last said.
        let tracking = self.tracking();
        if tracking && (state.place_gen != marks.place || !marks.tracking) {
            marks.place = state.place_gen;
            pending.place = state.place;
        }
        marks.tracking = tracking;
        pending
    }

    /// Everything as it is now, changed or not, and the marks moved up to
    /// it: what the sender says again at each refresh ([`REFRESH_EVERY`]).
    pub fn refresh(&self, marks: &mut Marks) -> Pending {
        let state = self.state();
        let tracking = self.tracking();
        marks.shape = state.shape_gen;
        marks.visible = state.visible_gen;
        marks.visible_sent = true;
        marks.tracking = tracking;
        let place = if tracking {
            marks.place = state.place_gen;
            state.place
        } else {
            None
        };
        Pending {
            shape: state.shape.clone(),
            visible: Some(state.visible),
            place,
        }
    }
}

/// Held by a capture while it keeps the pointer out of its picture
/// ([`CursorFeed::apart`]).
pub struct ApartGuard {
    feed: CursorFeed,
    gen_: u64,
}

impl Drop for ApartGuard {
    fn drop(&mut self) {
        {
            let mut state = self.feed.state();
            if state.apart_gen != self.gen_ {
                return;
            }
            state.apart = false;
            state.separate = false;
        }
        self.feed.inner.wake.notify_one();
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
    /// Whether places were being asked for at the last look.
    tracking: bool,
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

/// As [`plan`], but the shape always whole: for a refresh, which must heal
/// a viewer that lost the message that carried the shape or never held it.
pub fn plan_full(pending: &Pending, sent: &mut ShapeCache) -> Vec<CursorMessage> {
    let mut out = Vec::new();
    if let Some(shape) = &pending.shape {
        sent.put(shape.clone());
        out.push(CursorMessage::Shape((**shape).clone()));
    }
    out.extend(plan(
        &Pending {
            shape: None,
            visible: pending.visible,
            place: pending.place,
        },
        sent,
    ));
    out
}

/// What a session's sender holds between looks at a feed: what the viewer
/// has been told, and the numbers of the messages. Pure (no connection, no
/// clock) so what it says is tested without either; the session's loop
/// sends the messages and says when a refresh is due.
#[derive(Debug, Default)]
pub struct Sender {
    held: ShapeCache,
    marks: Marks,
    /// The mode the viewer was last told.
    announced: Option<bool>,
    /// The pointer was sent apart at some point, so the viewer holds
    /// something a lost message could leave wrong.
    ever_apart: bool,
    resend: bool,
    sequence: u64,
}

impl Sender {
    /// The messages to send now, each with its stream's number, in order.
    /// The first look says the mode; a change of mode says it and, when
    /// the pointer is apart, everything; so does a `refresh` that is due,
    /// once there is anything a viewer could hold wrong. Otherwise only
    /// what changed.
    pub fn step(&mut self, feed: &CursorFeed, refresh: bool) -> Vec<(u64, CursorMessage)> {
        let separate = feed.separate();
        self.ever_apart |= separate;
        let full = self.announced != Some(separate) || self.resend || (refresh && self.ever_apart);
        self.resend = false;
        let mut messages = Vec::new();
        if full {
            self.announced = Some(separate);
            messages.push(CursorMessage::Mode { separate });
        }
        if separate {
            let pending = if full {
                feed.refresh(&mut self.marks)
            } else {
                feed.take(&mut self.marks)
            };
            messages.extend(if full {
                plan_full(&pending, &mut self.held)
            } else {
                plan(&pending, &mut self.held)
            });
        }
        messages
            .into_iter()
            .map(|message| {
                self.sequence += 1;
                (self.sequence, message)
            })
            .collect()
    }

    /// A message did not go: the viewer may hold less than was believed, so
    /// the next step says everything.
    pub fn lost(&mut self) {
        self.held = ShapeCache::default();
        self.resend = true;
    }
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

    /// What a viewer that was sent `messages` holds, as the window's state.
    fn viewer(messages: &[(u64, CursorMessage)]) -> crate::protocol::cursor::CursorState {
        let mut state = crate::protocol::cursor::CursorState::default();
        for (_, message) in messages {
            state.apply(message.clone());
        }
        state
    }

    #[test]
    fn the_first_look_says_the_mode_and_then_only_what_changes() {
        let feed = CursorFeed::new();
        feed.set_separate(true);
        feed.set_shape(shape(1));
        let mut sender = Sender::default();
        let first = sender.step(&feed, false);
        assert_eq!(first[0], (1, CursorMessage::Mode { separate: true }));
        assert!(matches!(first[1].1, CursorMessage::Shape(_)));
        assert_eq!(first[2].1, CursorMessage::Visible(true));
        assert!(sender.step(&feed, false).is_empty(), "nothing changed");
        feed.set_visible(false);
        let next = sender.step(&feed, false);
        assert_eq!(
            next,
            vec![(first.len() as u64 + 1, CursorMessage::Visible(false))]
        );
    }

    /// A lost mode: the viewer draws nothing until the host says it again,
    /// which the refresh does (with the shape and whether it shows).
    #[test]
    fn a_mode_that_was_lost_is_said_again_by_the_next_refresh() {
        let feed = CursorFeed::new();
        feed.set_separate(true);
        feed.set_shape(shape(1));
        let mut sender = Sender::default();
        // The mode's stream was lost; the rest arrived.
        let arrived: Vec<_> = sender
            .step(&feed, false)
            .into_iter()
            .filter(|(_, m)| !matches!(m, CursorMessage::Mode { .. }))
            .collect();
        let held = viewer(&arrived);
        assert!(!held.separate && held.shape.is_none(), "nothing drawn yet");
        // Nothing changed, so a look without a refresh says nothing...
        assert!(sender.step(&feed, false).is_empty());
        // ...and the refresh says it all again.
        let again = sender.step(&feed, true);
        assert_eq!(again[0].1, CursorMessage::Mode { separate: true });
        let held = viewer(&again);
        assert!(held.separate && held.visible);
        assert_eq!(held.shape.as_ref().map(|s| s.id), Some(shape(1).id));
    }

    /// A shape the viewer never held, named by `Use`: it keeps the one it
    /// had, and the refresh (the shape whole) puts it right.
    #[test]
    fn a_use_of_a_shape_the_viewer_does_not_hold_is_put_right_by_the_refresh() {
        let feed = CursorFeed::new();
        feed.set_separate(true);
        let mut sender = Sender::default();
        feed.set_shape(shape(1));
        let first = sender.step(&feed, false);
        assert!(!first.is_empty());
        feed.set_shape(shape(2));
        let second = sender.step(&feed, false);
        // The viewer lost the first shape's stream, and heard the second.
        let mut late = viewer(&[(1, CursorMessage::Mode { separate: true })]);
        for (_, m) in &second {
            late.apply(m.clone());
        }
        assert_eq!(late.shape.as_ref().map(|s| s.id), Some(shape(2).id));
        // Back to the first: sent as its name, which the viewer cannot
        // read, so it keeps the second.
        feed.set_shape(shape(1));
        let back = sender.step(&feed, false);
        assert_eq!(back[0].1, CursorMessage::Use { id: shape(1).id });
        for (_, m) in &back {
            late.apply(m.clone());
        }
        assert_eq!(late.shape.as_ref().map(|s| s.id), Some(shape(2).id));
        // The refresh sends it whole, and the viewer has it.
        for (_, m) in sender.step(&feed, true) {
            late.apply(m);
        }
        assert_eq!(late.shape.as_ref().map(|s| s.id), Some(shape(1).id));
    }

    #[test]
    fn a_write_that_failed_has_everything_said_again_at_the_next_look() {
        let feed = CursorFeed::new();
        feed.set_separate(true);
        feed.set_shape(shape(1));
        let mut sender = Sender::default();
        sender.step(&feed, false);
        feed.set_shape(shape(2));
        sender.step(&feed, false);
        sender.lost();
        feed.set_shape(shape(1));
        let again = sender.step(&feed, false);
        assert_eq!(again[0].1, CursorMessage::Mode { separate: true });
        assert!(
            matches!(&again[1].1, CursorMessage::Shape(s) if s.id == shape(1).id),
            "whole, as the viewer may not hold it: {again:?}"
        );
    }

    /// A capture that gives the pointer up (its thread ended): the sender
    /// says so, and the viewer goes back to its own pointer.
    #[test]
    fn a_capture_that_ends_has_the_mode_of_no_said_and_the_viewer_forgets_the_pointer() {
        let feed = CursorFeed::new();
        let guard = feed.apart();
        feed.set_shape(shape(1));
        let mut sender = Sender::default();
        let mut sent = sender.step(&feed, false);
        assert_eq!(sent[0].1, CursorMessage::Mode { separate: true });
        let mut held = viewer(&sent);
        assert!(held.separate && held.shape.is_some());
        drop(guard);
        assert!(!feed.separate());
        sent = sender.step(&feed, false);
        assert_eq!(
            sent.iter().map(|(_, m)| m.clone()).collect::<Vec<_>>(),
            vec![CursorMessage::Mode { separate: false }]
        );
        for (_, m) in sent {
            held.apply(m);
        }
        assert!(!held.separate && held.shape.is_none() && !held.visible);
        // Said again by the refresh (a lost one must not stick), and
        // nothing else while the pointer is in the picture.
        let again = sender.step(&feed, true);
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].1, CursorMessage::Mode { separate: false });
    }

    /// A host that never takes the pointer out says so once and then
    /// nothing, refresh or not.
    #[test]
    fn a_pointer_that_was_never_apart_costs_a_single_message() {
        let feed = CursorFeed::new();
        let mut sender = Sender::default();
        let first = sender.step(&feed, false);
        assert_eq!(first, vec![(1, CursorMessage::Mode { separate: false })]);
        assert!(sender.step(&feed, true).is_empty());
        assert!(sender.step(&feed, true).is_empty());
    }

    #[test]
    fn the_guard_holds_the_pointer_out_only_while_it_lives_and_a_late_one_clears_nothing() {
        let feed = CursorFeed::new();
        assert!(!feed.separate());
        let first = feed.apart();
        assert!(feed.separate());
        drop(first);
        assert!(!feed.separate(), "a capture that ended says no");
        // A hold that ends after the next capture's began leaves it be.
        let old = feed.apart();
        let new = feed.apart();
        drop(old);
        assert!(feed.separate());
        drop(new);
        assert!(!feed.separate());
        // And no capture can say yes once it is gone.
        assert!(!feed.want_out(true));
        assert!(!feed.separate());
    }

    /// A capture that can put the pointer back in a window's picture asks
    /// per picture; the viewer is told in step.
    #[test]
    fn a_window_picture_keeps_the_pointer_and_the_output_picture_leaves_it_out() {
        let feed = CursorFeed::new();
        let _hold = feed.apart();
        assert!(feed.want_out(true));
        assert!(feed.separate());
        assert!(!feed.want_out(false), "a window's picture has its pointer");
        assert!(!feed.separate(), "and the viewer is told so");
        assert!(feed.want_out(true), "the output's again");
        assert!(feed.separate());
    }

    #[test]
    fn a_viewer_that_asks_for_places_again_is_told_where_the_pointer_is() {
        let feed = CursorFeed::new();
        let mut marks = Marks::default();
        feed.set_place(10, 20);
        feed.set_tracking(true);
        assert_eq!(feed.take(&mut marks).place, Some((10, 20)));
        assert!(feed.take(&mut marks).place.is_none());
        // Off and on again with the pointer still: the place is said.
        feed.set_tracking(false);
        assert!(feed.take(&mut marks).place.is_none());
        feed.set_tracking(true);
        assert_eq!(feed.take(&mut marks).place, Some((10, 20)));
    }

    #[test]
    fn a_report_is_the_three_changes_in_one() {
        let feed = CursorFeed::new();
        feed.set_tracking(true);
        let mut marks = Marks::default();
        feed.take(&mut marks);
        feed.report(Some(shape(1)), false, Some((3, 4)));
        let got = feed.take(&mut marks);
        assert_eq!(got.shape.as_ref().map(|s| s.id), Some(shape(1).id));
        assert_eq!(got.visible, Some(false));
        assert_eq!(got.place, Some((3, 4)));
        feed.report(Some(shape(1)), false, Some((3, 4)));
        assert!(feed.take(&mut marks).is_empty(), "nothing new");
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
