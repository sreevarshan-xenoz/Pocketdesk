//! The input-capture portal's session task (BK8), behind a seam over the
//! portal's calls ([`Calls`]) and what it and libei say ([`Signal`]), so
//! the order of its calls is tested on every system.
//!
//! [`PortalBackend`] is what the glue asks ([`Backend`]); each ask goes to
//! the task ([`Ask`]). [`serve`] reads the zones and tells the glue, then
//! answers the asks: arming is disable, set the barriers for the zone set,
//! enable (GNOME's portal sets barriers only on a disabled session);
//! disarming is disable; a release is a release with a position. What the
//! portal says (activations, their ends, the session disabled, new zones)
//! and libei's input go on to the glue ([`Portal`]). It ends when the glue
//! is gone or the stream closes, and then closes the session. The real
//! calls and signals are `capture_linux`'s (ashpd and reis).

use std::sync::mpsc::Sender;

use futures_lite::{FutureExt, Stream, StreamExt};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::barriers::Barrier;
use super::portal_glue::{Backend, Portal};
use super::wire::Screen;

/// What the glue asks of the session's task.
#[derive(Debug, Clone, PartialEq)]
pub enum Ask {
    Arm(Vec<Barrier>),
    Disarm,
    Release(Option<u32>, (i32, i32)),
}

/// The portal, as the glue asks it: each ask goes to the session's task.
pub struct PortalBackend {
    asks: UnboundedSender<Ask>,
}

impl PortalBackend {
    /// A backend and the asks it sends, for the session's task.
    pub fn new() -> (PortalBackend, UnboundedReceiver<Ask>) {
        let (asks, asked) = unbounded_channel();
        (PortalBackend { asks }, asked)
    }
}

impl Backend for PortalBackend {
    fn arm(&mut self, barriers: &[Barrier]) {
        let _ = self.asks.send(Ask::Arm(barriers.to_vec()));
    }

    fn disarm(&mut self) {
        let _ = self.asks.send(Ask::Disarm);
    }

    fn release(&mut self, activation: Option<u32>, at: (i32, i32)) {
        let _ = self.asks.send(Ask::Release(activation, at));
    }
}

/// What the portal and libei say to the task.
#[derive(Debug, Clone, PartialEq)]
pub enum Signal {
    Activated {
        activation: Option<u32>,
        barrier: Option<u32>,
        at: Option<(f64, f64)>,
    },
    Deactivated {
        activation: Option<u32>,
    },
    Disabled,
    /// The zones changed: read them again.
    ZonesChanged,
    /// libei's input, for the glue.
    Input(Vec<Portal>),
    /// The libei stream is gone (why).
    Closed(String),
}

/// The portal's calls on the session. Its futures run on the session's
/// own thread, so none needs to be `Send`.
#[allow(async_fn_in_trait)]
pub trait Calls {
    /// The zones and the zone set they belong to.
    async fn zones(&self) -> Result<(Vec<Screen>, u32), String>;
    async fn disable(&self) -> Result<(), String>;
    /// Set `barriers` for `zone_set`: the ids the desktop refused.
    async fn set_barriers(&self, barriers: &[Barrier], zone_set: u32) -> Result<Vec<u32>, String>;
    async fn enable(&self) -> Result<(), String>;
    async fn release(&self, activation: Option<u32>, at: (i32, i32)) -> Result<(), String>;
    async fn close(&self);
}

/// Tell the glue `event`; an error once it is gone.
pub fn say(events: &Sender<Portal>, event: Portal) -> Result<(), String> {
    events
        .send(event)
        .map_err(|_| "the driver stopped".to_string())
}

/// Read the zones and tell the glue; the zone set they belong to.
async fn zones<C: Calls>(calls: &C, events: &Sender<Portal>) -> Result<u32, String> {
    let (screens, zone_set) = calls.zones().await?;
    say(events, Portal::Zones(screens))?;
    Ok(zone_set)
}

/// A call the desktop keeps refusing: said at warn once a spell, then at
/// debug until it works again (the glue asks again every few seconds).
#[derive(Debug, Default)]
pub struct Spell {
    failing: bool,
}

impl Spell {
    /// It failed: whether to say so at warn (the first of a spell).
    pub fn failed(&mut self) -> bool {
        !std::mem::replace(&mut self.failing, true)
    }

    /// It worked: the next failure starts a new spell.
    pub fn worked(&mut self) {
        self.failing = false;
    }
}

/// Say `what` failed, at warn the first time of a spell.
fn refused(spell: &mut Spell, what: &str) {
    if spell.failed() {
        tracing::warn!("Shared input: {what}");
    } else {
        tracing::debug!("Shared input: {what}");
    }
}

/// Set `barriers` for `zone_set` and enable the capture. One the desktop
/// refused (not on the zones' outer edge, as it sees them) is said; the
/// others still work. Whether they were set: if not, the capture is left
/// disabled.
async fn arm<C: Calls>(calls: &C, barriers: &[Barrier], zone_set: u32, spell: &mut Spell) -> bool {
    // GNOME's portal sets barriers only on a disabled session.
    let _ = calls.disable().await;
    let mut whole = true;
    match calls.set_barriers(barriers, zone_set).await {
        Ok(failed) if !failed.is_empty() => {
            whole = false;
            refused(
                spell,
                &format!("the desktop refused some of this computer's edges ({failed:?})"),
            );
        }
        Ok(_) => {}
        Err(e) => {
            refused(spell, &format!("the edges were not set: {e}"));
            return false;
        }
    }
    if let Err(e) = calls.enable().await {
        refused(spell, &format!("input capture not enabled: {e}"));
        return false;
    }
    if whole {
        spell.worked();
    }
    true
}

/// The session, from its zones to its end: the glue's asks answered and
/// what the portal and libei say passed on, until the glue is gone or the
/// stream closes. However it ends, a panic too, the capture is then
/// disabled and the session closed, so this computer's keyboard and mouse
/// are never left taken.
pub async fn serve<C: Calls, S: Stream<Item = Signal> + Unpin>(
    calls: &C,
    mut signals: S,
    mut asked: UnboundedReceiver<Ask>,
    events: &Sender<Portal>,
) -> Result<(), String> {
    let ended = std::panic::AssertUnwindSafe(async {
        let mut zone_set = zones(calls, events).await?;
        let (mut arming, mut releasing) = (Spell::default(), Spell::default());
        loop {
            tokio::select! {
                ask = asked.recv() => match ask {
                    // The glue is gone: the session ends with it.
                    None => return Ok(()),
                    Some(Ask::Arm(barriers)) => {
                        // Not set: the glue is told the capture is off, and
                        // sets them again at its next pass.
                        if !arm(calls, &barriers, zone_set, &mut arming).await {
                            say(events, Portal::Disabled)?;
                        }
                    }
                    Some(Ask::Disarm) => {
                        if let Err(e) = calls.disable().await {
                            tracing::debug!("Shared input: input capture not disabled: {e}");
                        }
                    }
                    Some(Ask::Release(activation, at)) => {
                        // The glue lets go of it again if input keeps coming.
                        match calls.release(activation, at).await {
                            Ok(()) => releasing.worked(),
                            Err(e) => refused(
                                &mut releasing,
                                &format!("input capture not released: {e}"),
                            ),
                        }
                    }
                },
                signal = signals.next() => match signal {
                    None => return Err("the input-capture signals ended".to_string()),
                    Some(Signal::Activated { activation, barrier, at }) => {
                        say(events, Portal::Activated { activation, barrier, at })?;
                    }
                    Some(Signal::Deactivated { activation }) => {
                        say(events, Portal::Deactivated { activation })?;
                    }
                    Some(Signal::Disabled) => say(events, Portal::Disabled)?,
                    // Said to the glue, which sets the new zone set's
                    // barriers once nothing is captured (setting them here
                    // would end a capture behind its back).
                    Some(Signal::ZonesChanged) => zone_set = zones(calls, events).await?,
                    Some(Signal::Input(input)) => {
                        for event in input {
                            say(events, event)?;
                        }
                    }
                    Some(Signal::Closed(why)) => return Err(why),
                },
            }
        }
    })
    .catch_unwind()
    .await
    .unwrap_or_else(|_| {
        Err("the input-capture task failed (a panic); its session is closed".to_string())
    });
    if let Err(e) = calls.disable().await {
        tracing::debug!("Shared input: input capture not disabled as it ended: {e}");
    }
    calls.close().await;
    ended
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::mpsc::{self, Receiver};

    /// A stand-in portal: its calls written down in order.
    #[derive(Default)]
    pub(crate) struct FakeCalls {
        pub log: RefCell<Vec<String>>,
        /// The zone set each zones call says, from 1 up.
        pub zone_sets: RefCell<u32>,
        /// Setting barriers fails.
        pub set_fails: bool,
    }

    impl Calls for FakeCalls {
        async fn zones(&self) -> Result<(Vec<Screen>, u32), String> {
            let set = {
                let mut n = self.zone_sets.borrow_mut();
                *n += 1;
                *n
            };
            self.log.borrow_mut().push(format!("zones -> {set}"));
            let screen = Screen {
                x: 0,
                y: 0,
                width: 1600,
                height: 1000,
            };
            Ok((vec![screen], set))
        }
        async fn disable(&self) -> Result<(), String> {
            self.log.borrow_mut().push("disable".into());
            Ok(())
        }
        async fn set_barriers(
            &self,
            barriers: &[Barrier],
            zone_set: u32,
        ) -> Result<Vec<u32>, String> {
            let ids: Vec<u32> = barriers.iter().map(|b| b.id).collect();
            self.log
                .borrow_mut()
                .push(format!("set {ids:?} for {zone_set}"));
            if self.set_fails {
                Err("refused".into())
            } else {
                Ok(Vec::new())
            }
        }
        async fn enable(&self) -> Result<(), String> {
            self.log.borrow_mut().push("enable".into());
            Ok(())
        }
        async fn release(&self, activation: Option<u32>, at: (i32, i32)) -> Result<(), String> {
            self.log
                .borrow_mut()
                .push(format!("release {activation:?} at {at:?}"));
            Ok(())
        }
        async fn close(&self) {
            self.log.borrow_mut().push("close".into());
        }
    }

    pub(crate) fn barrier(id: u32) -> Barrier {
        Barrier {
            id,
            line: (1600, 0, 1600, 999),
            from: ((1599, 0), (1599, 999)),
            way: (1, 0),
            to: "W".to_string(),
        }
    }

    /// A signal stream fed from a channel.
    pub(crate) fn signals(
        rx: UnboundedReceiver<Signal>,
    ) -> std::pin::Pin<Box<dyn Stream<Item = Signal>>> {
        Box::pin(futures_lite::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|s| (s, rx))
        }))
    }

    /// Run `serve` over `steps` (an ask or a signal each, in order) with
    /// `calls`; the asks then end. What it returned and what it said.
    pub(crate) async fn run(
        calls: &FakeCalls,
        steps: Vec<Result<Ask, Signal>>,
        end_with_signal: Option<Signal>,
    ) -> (Result<(), String>, Vec<Portal>) {
        let (asks, asked) = unbounded_channel();
        let (sig_tx, sig_rx) = unbounded_channel();
        let (said, heard): (Sender<Portal>, Receiver<Portal>) = mpsc::channel();
        // Both kept until the end, so it ends the way the test says.
        let asks = RefCell::new(Some(asks));
        let feed = async {
            for step in steps {
                match step {
                    Ok(ask) => asks.borrow().as_ref().unwrap().send(ask).unwrap(),
                    Err(signal) => sig_tx.send(signal).unwrap(),
                }
                // One step at a time, as they would come.
                for _ in 0..8 {
                    tokio::task::yield_now().await;
                }
            }
            match end_with_signal {
                Some(signal) => sig_tx.send(signal).unwrap(),
                None => drop(asks.borrow_mut().take()),
            }
        };
        let (ended, ()) = tokio::join!(serve(calls, signals(sig_rx), asked, &said), feed);
        (ended, heard.try_iter().collect())
    }

    #[tokio::test]
    async fn the_portals_calls_go_in_order_and_the_session_closes() {
        let calls = FakeCalls::default();
        let (ended, said) = run(
            &calls,
            vec![
                Ok(Ask::Arm(vec![barrier(1)])),
                Err(Signal::Activated {
                    activation: Some(4),
                    barrier: Some(1),
                    at: Some((1600.0, 500.0)),
                }),
                Err(Signal::Input(vec![Portal::Motion { dx: 1.0, dy: 0.0 }])),
                Ok(Ask::Release(Some(4), (1595, 500))),
                Ok(Ask::Disarm),
            ],
            None,
        )
        .await;
        assert_eq!(ended, Ok(()));
        assert_eq!(
            *calls.log.borrow(),
            [
                "zones -> 1",
                "disable",
                "set [1] for 1",
                "enable",
                "release Some(4) at (1595, 500)",
                "disable",
                "disable",
                "close",
            ]
        );
        assert_eq!(
            said,
            vec![
                Portal::Zones(vec![Screen {
                    x: 0,
                    y: 0,
                    width: 1600,
                    height: 1000
                }]),
                Portal::Activated {
                    activation: Some(4),
                    barrier: Some(1),
                    at: Some((1600.0, 500.0)),
                },
                Portal::Motion { dx: 1.0, dy: 0.0 },
            ]
        );
    }

    #[tokio::test]
    async fn a_closed_stream_ends_the_session() {
        let calls = FakeCalls::default();
        let (ended, _) = run(
            &calls,
            vec![Ok(Ask::Arm(vec![barrier(1)]))],
            Some(Signal::Closed("the libei stream closed".into())),
        )
        .await;
        assert_eq!(ended, Err("the libei stream closed".to_string()));
        assert_eq!(calls.log.borrow().last().map(String::as_str), Some("close"));
    }

    /// BK8 review C1: a panic in the session task (reis's converter has
    /// `expect`s) must never leave this computer's keyboard and mouse
    /// taken: the capture is disabled and the session closed all the same.
    #[tokio::test]
    async fn a_panic_in_the_session_task_still_disables_and_closes_it() {
        let calls = FakeCalls::default();
        let (_asks, asked) = unbounded_channel();
        let (sig_tx, sig_rx) = unbounded_channel();
        let (said, _heard): (Sender<Portal>, Receiver<Portal>) = mpsc::channel();
        let boom = signals(sig_rx).map(|s| {
            if s == Signal::Closed("boom".into()) {
                panic!("a stand-in for a panic in libei's converter");
            }
            s
        });
        sig_tx
            .send(Signal::Activated {
                activation: Some(2),
                barrier: Some(1),
                at: None,
            })
            .unwrap();
        sig_tx.send(Signal::Closed("boom".into())).unwrap();
        let ended = serve(&calls, Box::pin(boom), asked, &said).await;
        assert!(
            ended.as_ref().is_err_and(|e| e.contains("panic")),
            "{ended:?}"
        );
        assert_eq!(*calls.log.borrow(), ["zones -> 1", "disable", "close"]);
    }

    /// BK8 review I2: new zones never set the old barriers again behind
    /// the glue (that would disable a capture going on): the zones are
    /// read and said, and the glue arms the new ones once nothing is
    /// captured.
    #[tokio::test]
    async fn new_zones_are_said_and_never_set_the_old_barriers_again() {
        let calls = FakeCalls::default();
        let (ended, said) = run(
            &calls,
            vec![
                Ok(Ask::Arm(vec![barrier(1)])),
                Err(Signal::ZonesChanged),
                Ok(Ask::Arm(vec![barrier(2)])),
            ],
            None,
        )
        .await;
        assert_eq!(ended, Ok(()));
        assert_eq!(
            *calls.log.borrow(),
            [
                "zones -> 1",
                "disable",
                "set [1] for 1",
                "enable",
                "zones -> 2",
                "disable",
                "set [2] for 2",
                "enable",
                "disable",
                "close",
            ]
        );
        assert_eq!(
            said.iter()
                .filter(|p| matches!(p, Portal::Zones(_)))
                .count(),
            2
        );
    }

    /// BK8 review M3: barriers the desktop would not set leave the capture
    /// disabled; the glue is told so, and sets them again at its next pass
    /// instead of taking them as set.
    #[tokio::test]
    async fn barriers_that_were_not_set_are_said_as_disabled() {
        let calls = FakeCalls {
            set_fails: true,
            ..FakeCalls::default()
        };
        let (_, said) = run(&calls, vec![Ok(Ask::Arm(vec![barrier(1)]))], None).await;
        assert_eq!(said.last(), Some(&Portal::Disabled), "{said:?}");
        assert!(!calls.log.borrow().iter().any(|c| c == "enable"));
    }

    /// BK8 re-review N2: a call the desktop keeps refusing (a Release, the
    /// barriers) is said at warn once a spell, then quietly until it works.
    #[test]
    fn a_refusal_is_warned_of_once_a_spell() {
        let mut spell = Spell::default();
        assert!(spell.failed(), "the first: warned");
        assert!(!spell.failed(), "again: quiet");
        assert!(!spell.failed());
        spell.worked();
        assert!(spell.failed(), "a new spell: warned again");
    }
}
