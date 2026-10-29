//! Driving another computer from Linux (BK8): the input-capture portal
//! (`org.freedesktop.portal.InputCapture`, through ashpd) and its libei
//! stream (through reis, as a receiver), behind the [`Backend`] seam that
//! [`PortalGlue`](super::portal_glue::PortalGlue) asks.
//!
//! The session is created (version 2's `CreateSession2` and `Start`, with
//! the restore token kept from the last start so the desktop's permission
//! dialog shows once; version 1's `CreateSession` where that is all there
//! is), its signals subscribed, the libei stream connected and greeted as a
//! receiver, and the zones read. Then one task answers the glue's asks (set
//! the barriers for the zone set and enable, disable, release with a
//! position) and passes on what the portal says ([`Portal`]): activations,
//! their ends, the session disabled, new zones (the barriers set again for
//! the new zone set at once), and libei's pointer, buttons, scroll (one
//! [`Portal::Scroll`] a frame) and keys. The stream closing, or the session
//! failing, ends it ([`Portal::Closed`]). Desktops: GNOME 45 and later,
//! KDE Plasma 6.1 and later, Hyprland with xdg-desktop-portal-hyprland 1.4
//! and later.
//!
//! Nothing here runs in the tests but the libei half, against a stand-in
//! EIS server on a socket pair: no portal, no desktop, no input device.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use ashpd::desktop::PersistMode;
use ashpd::desktop::input_capture::{
    ActivatedBarrier, Barrier as PortalBarrier, BarrierID, Capabilities, ConnectToEISOptions,
    CreateSession2Options, CreateSessionOptions, InputCapture, ReleaseOptions, StartOptions,
};
use futures_lite::StreamExt;
use reis::ei;
use reis::event::{DeviceCapability, EiEvent};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::barriers::Barrier;
use super::capture_restore::{self, RestoreToken};
use super::portal_glue::{Backend, Portal, Standing};
use super::wire::Screen;

/// What the glue asks of the portal's task.
#[derive(Debug)]
enum Ask {
    Arm(Vec<Barrier>),
    Disarm,
    Release(Option<u32>, (i32, i32)),
}

/// The portal, as the glue asks it: each ask goes to the session's task.
pub struct PortalBackend {
    asks: UnboundedSender<Ask>,
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

/// Start the session on a thread of its own (libei's stream is not `Send`,
/// so it runs on a single-threaded runtime there): what it says goes to
/// `events`, the restore token is kept in `state_dir`. Dropping the
/// backend ends it; `None` if the thread did not start.
pub fn start(state_dir: Option<PathBuf>, events: Sender<Portal>) -> Option<PortalBackend> {
    let (asks, asked) = unbounded_channel();
    let thread = std::thread::Builder::new()
        .name("pd-input-capture".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(e) => {
                    let _ = events.send(Portal::Closed(format!("no runtime for the portal: {e}")));
                    return;
                }
            };
            if let Err(why) = runtime.block_on(run(state_dir, &events, asked)) {
                let _ = events.send(Portal::Closed(why));
            }
        });
    match thread {
        Ok(_) => Some(PortalBackend { asks }),
        Err(e) => {
            tracing::warn!("Shared input: the input-capture thread did not start: {e}");
            None
        }
    }
}

/// Tell the glue `event`; an error once it is gone.
fn say(events: &Sender<Portal>, event: Portal) -> Result<(), String> {
    events
        .send(event)
        .map_err(|_| "the driver stopped".to_string())
}

/// The session, from its start to its end.
async fn run(
    state_dir: Option<PathBuf>,
    events: &Sender<Portal>,
    mut asked: UnboundedReceiver<Ask>,
) -> Result<(), String> {
    let portal = match InputCapture::new().await {
        Ok(portal) => portal,
        Err(e) => {
            say(events, Portal::Standing(Standing::Unsupported))?;
            return Err(format!("this desktop has no input-capture portal ({e})"));
        }
    };
    let session = match portal
        .create_session2(CreateSession2Options::default())
        .await
    {
        Ok(session) => {
            let token = state_dir.as_deref().and_then(capture_restore::read);
            if token.is_none() {
                // The desktop's dialog asks the owner now.
                say(events, Portal::Standing(Standing::NeedsPermission))?;
            }
            let options = StartOptions::default()
                .set_capabilities(Capabilities::Keyboard | Capabilities::Pointer)
                .set_persist_mode(PersistMode::ExplicitlyRevoked)
                .set_restore_token(token.map(|t| t.as_str().to_string()));
            let started = match portal.start(&session, None, options).await {
                Ok(request) => request.response(),
                Err(e) => Err(e),
            };
            match started {
                Ok(response) => {
                    // A new token at each start, good for the next one.
                    if let (Some(dir), Some(token)) = (
                        state_dir.as_deref(),
                        response.restore_token().and_then(RestoreToken::new),
                    ) && let Err(e) = capture_restore::write(dir, &token)
                    {
                        tracing::debug!("Shared input: the input-capture grant was not kept: {e}");
                    }
                }
                Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => {
                    if let Some(dir) = state_dir.as_deref() {
                        capture_restore::forget(dir);
                    }
                    say(events, Portal::Standing(Standing::NeedsPermission))?;
                    return Err("the owner did not allow input capture".to_string());
                }
                Err(e) => return Err(format!("input capture did not start: {e}")),
            }
            session
        }
        // A version 1 portal starts the session as it creates it.
        Err(ashpd::Error::RequiresVersion(_, _)) => {
            let options = CreateSessionOptions::default()
                .set_capabilities(Capabilities::Keyboard | Capabilities::Pointer);
            portal
                .create_session(None, options)
                .await
                .map_err(|e| format!("no input-capture session: {e}"))?
                .0
        }
        Err(e) => {
            if matches!(e, ashpd::Error::PortalNotFound(_)) {
                say(events, Portal::Standing(Standing::Unsupported))?;
            }
            return Err(format!("no input-capture session: {e}"));
        }
    };
    let signals = |e: ashpd::Error| format!("the input-capture signals: {e}");
    let activated = portal.receive_activated().await.map_err(signals)?;
    let deactivated = portal.receive_deactivated().await.map_err(signals)?;
    let disabled = portal.receive_disabled().await.map_err(signals)?;
    let zones_changed = portal.receive_zones_changed().await.map_err(signals)?;
    let (mut activated, mut deactivated, mut disabled, mut zones_changed) = (
        std::pin::pin!(activated),
        std::pin::pin!(deactivated),
        std::pin::pin!(disabled),
        std::pin::pin!(zones_changed),
    );
    let fd = portal
        .connect_to_eis(&session, ConnectToEISOptions::default())
        .await
        .map_err(|e| format!("no libei stream: {e}"))?;
    let context = ei::Context::new(UnixStream::from(fd)).map_err(|e| format!("libei: {e}"))?;
    let (_connection, ei_events) = context
        .handshake_tokio("pocketdesk", ei::handshake::ContextType::Receiver)
        .await
        .map_err(|e| format!("the libei greeting: {e}"))?;
    let mut ei_events = std::pin::pin!(ei_events);
    let mut zone_set = zones(&portal, &session, events).await?;
    let mut scroll = ScrollFrame::default();
    // The barriers last asked for, set again for each new zone set.
    let mut armed: Option<Vec<Barrier>> = None;
    let ended = loop {
        tokio::select! {
            ask = asked.recv() => match ask {
                // The glue is gone: the session ends with it.
                None => break Ok(()),
                Some(Ask::Arm(barriers)) => {
                    arm(&portal, &session, &barriers, zone_set).await;
                    armed = Some(barriers);
                }
                Some(Ask::Disarm) => {
                    armed = None;
                    if let Err(e) = portal.disable(&session, Default::default()).await {
                        tracing::debug!("Shared input: input capture not disabled: {e}");
                    }
                }
                Some(Ask::Release(activation, (x, y))) => {
                    let options = ReleaseOptions::default()
                        .set_activation_id(activation)
                        .set_cursor_position((f64::from(x), f64::from(y)));
                    if let Err(e) = portal.release(&session, options).await {
                        tracing::debug!("Shared input: input capture not released: {e}");
                    }
                }
            },
            Some(a) = activated.next() => say(events, Portal::Activated {
                activation: a.activation_id(),
                barrier: match a.barrier_id() {
                    Some(ActivatedBarrier::Barrier(id)) => Some(id.get()),
                    _ => None,
                },
                at: a.cursor_position().map(|(x, y)| (f64::from(x), f64::from(y))),
            })?,
            Some(d) = deactivated.next() => say(events, Portal::Deactivated {
                activation: d.activation_id(),
            })?,
            Some(_) = disabled.next() => {
                armed = None;
                say(events, Portal::Disabled)?;
            }
            Some(_) = zones_changed.next() => {
                zone_set = zones(&portal, &session, events).await?;
                // The barriers belong to a zone set: set again for this one
                // (the glue sets new ones once the layout follows).
                if let Some(barriers) = &armed {
                    arm(&portal, &session, barriers, zone_set).await;
                }
            }
            event = ei_events.next() => match event {
                None => break Err("the libei stream closed".to_string()),
                Some(Err(e)) => break Err(format!("the libei stream failed: {e}")),
                Some(Ok(event)) => {
                    for event in translate(event, &mut scroll) {
                        say(events, event)?;
                    }
                    let _ = context.flush();
                }
            },
        }
    };
    let _ = session.close().await;
    ended
}

/// Read the zones and tell the glue; the zone set they belong to.
async fn zones(
    portal: &InputCapture,
    session: &ashpd::desktop::Session<InputCapture>,
    events: &Sender<Portal>,
) -> Result<u32, String> {
    let zones = portal
        .zones(session, Default::default())
        .await
        .and_then(|request| request.response())
        .map_err(|e| format!("the input-capture zones: {e}"))?;
    let screens = zones
        .regions()
        .iter()
        .map(|r| Screen {
            x: r.x_offset(),
            y: r.y_offset(),
            width: r.width(),
            height: r.height(),
        })
        .filter(Screen::is_sound)
        .collect();
    say(events, Portal::Zones(screens))?;
    Ok(zones.zone_set())
}

/// Set `barriers` for `zone_set` and enable the capture. One the
/// compositor refused (not on the zones' outer edge, as it sees them) is
/// logged; the others still work.
async fn arm(
    portal: &InputCapture,
    session: &ashpd::desktop::Session<InputCapture>,
    barriers: &[Barrier],
    zone_set: u32,
) {
    let barriers: Vec<PortalBarrier> = barriers
        .iter()
        .filter_map(|b| Some(PortalBarrier::new(BarrierID::new(b.id)?, b.line)))
        .collect();
    // GNOME's portal sets barriers only on a disabled session.
    let _ = portal.disable(session, Default::default()).await;
    match portal
        .set_pointer_barriers(session, &barriers, zone_set, Default::default())
        .await
        .and_then(|request| request.response())
    {
        Ok(response) if !response.failed_barriers().is_empty() => tracing::warn!(
            failed = ?response.failed_barriers(),
            "Shared input: the desktop refused some of this computer's edges"
        ),
        Ok(_) => {}
        Err(e) => {
            tracing::warn!("Shared input: the edges were not set: {e}");
            return;
        }
    }
    if let Err(e) = portal.enable(session, Default::default()).await {
        tracing::warn!("Shared input: input capture not enabled: {e}");
    }
}

/// One libei frame's scroll, said once at its end.
#[derive(Debug, Default)]
struct ScrollFrame {
    smooth: (f64, f64),
    discrete: Option<(i32, i32)>,
}

/// What a libei event says, for the glue: the seat's capabilities are
/// bound as it comes (the pointer, its buttons and scroll, the keyboard).
fn translate(event: EiEvent, scroll: &mut ScrollFrame) -> Vec<Portal> {
    match event {
        EiEvent::SeatAdded(added) => {
            added.seat.bind_capabilities(
                DeviceCapability::Pointer
                    | DeviceCapability::Button
                    | DeviceCapability::Scroll
                    | DeviceCapability::Keyboard,
            );
            Vec::new()
        }
        EiEvent::PointerMotion(m) => vec![Portal::Motion {
            dx: f64::from(m.dx),
            dy: f64::from(m.dy),
        }],
        EiEvent::Button(b) => vec![Portal::Button {
            code: b.button,
            down: b.state == ei::button::ButtonState::Press,
        }],
        EiEvent::ScrollDelta(s) => {
            scroll.smooth.0 += f64::from(s.dx);
            scroll.smooth.1 += f64::from(s.dy);
            Vec::new()
        }
        EiEvent::ScrollDiscrete(s) => {
            let (x, y) = scroll.discrete.unwrap_or_default();
            scroll.discrete = Some((
                x.saturating_add(s.discrete_dx),
                y.saturating_add(s.discrete_dy),
            ));
            Vec::new()
        }
        EiEvent::Frame(_) => {
            let frame = std::mem::take(scroll);
            if frame.discrete.is_none() && frame.smooth == (0.0, 0.0) {
                return Vec::new();
            }
            vec![Portal::Scroll {
                dx: frame.smooth.0,
                dy: frame.smooth.1,
                discrete: frame.discrete,
            }]
        }
        EiEvent::KeyboardKey(k) => vec![Portal::Key {
            code: k.key,
            down: k.state == ei::keyboard::KeyState::Press,
        }],
        EiEvent::Disconnected(d) => vec![Portal::Closed(format!(
            "the desktop closed the libei stream ({:?})",
            d.reason
        ))],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_input::drive::{Action, Driver};
    use crate::shared_input::layout::{Layout, Placed};
    use crate::shared_input::portal_glue::PortalGlue;
    use crate::shared_input::wire::Msg;
    use reis::eis;
    use reis::handshake::EisHandshaker;
    use reis::request::{DeviceCapability as EisCapability, EisRequest, EisRequestConverter};
    use std::time::{Duration, Instant};

    /// One frame of the stand-in compositor's input.
    type Frame = Box<dyn Fn(&reis::request::Device) + Send>;

    /// A stand-in compositor's EIS side on `socket`: greets the receiver,
    /// offers a seat, adds a pointer-and-keyboard device once it is bound,
    /// and sends `script` (each a frame), then closes.
    fn fake_eis(socket: UnixStream, script: Vec<Frame>) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let context = eis::Context::new(socket).unwrap();
        let mut handshaker = EisHandshaker::new(&context, 1);
        let mut converter: Option<EisRequestConverter> = None;
        let mut seat: Option<reis::request::Seat> = None;
        let mut device: Option<reis::request::Device> = None;
        while Instant::now() < deadline {
            match context.read() {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return,
                Err(e) => panic!("fake EIS read: {e}"),
            }
            while let Some(pending) = context.pending_request() {
                let reis::PendingRequestResult::Request(request) = pending else {
                    panic!("fake EIS: a bad request");
                };
                match converter.as_mut() {
                    None => {
                        if let Some(resp) = handshaker.handle_request(request).unwrap() {
                            let c = EisRequestConverter::new(&context, resp, 1);
                            seat = Some(c.handle().add_seat(
                                Some("default"),
                                EisCapability::Pointer
                                    | EisCapability::Button
                                    | EisCapability::Scroll
                                    | EisCapability::Keyboard,
                            ));
                            converter = Some(c);
                        }
                    }
                    Some(c) => c.handle_request(request).unwrap(),
                }
            }
            if let Some(c) = converter.as_mut() {
                while let Some(request) = c.next_request() {
                    if let EisRequest::Bind(bind) = request
                        && device.is_none()
                    {
                        let d = bind.seat.add_device(
                            Some("stand-in"),
                            eis::device::DeviceType::Virtual,
                            bind.capabilities,
                            |_| {},
                        );
                        d.resumed();
                        d.start_emulating(1);
                        device = Some(d);
                    }
                }
                let _ = c.handle().flush();
            }
            if let Some(d) = device.take() {
                for (n, frame) in script.iter().enumerate() {
                    frame(&d);
                    d.frame(n as u64 + 1);
                }
                let _ = context.flush();
                // Give the receiver a moment, then close: the stream ends.
                std::thread::sleep(Duration::from_millis(200));
                drop(seat);
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("fake EIS: no bind within the deadline");
    }

    /// What the receiver half says from `socket`, until the stream ends.
    async fn receive(socket: UnixStream) -> Vec<Portal> {
        let context = ei::Context::new(socket).unwrap();
        let (_connection, events) = context
            .handshake_tokio("pocketdesk-test", ei::handshake::ContextType::Receiver)
            .await
            .unwrap();
        let mut events = std::pin::pin!(events);
        let mut scroll = ScrollFrame::default();
        let mut said = Vec::new();
        while let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_secs(10), events.next()).await
        {
            match event {
                Ok(event) => said.extend(translate(event, &mut scroll)),
                Err(e) => {
                    said.push(Portal::Closed(e.to_string()));
                    break;
                }
            }
            let _ = context.flush();
        }
        said
    }

    #[derive(Default)]
    struct Fake;

    impl Backend for Fake {
        fn arm(&mut self, _: &[Barrier]) {}
        fn disarm(&mut self) {}
        fn release(&mut self, _: Option<u32>, _: (i32, i32)) {}
    }

    /// A stand-in EIS server sends pointer, button, scroll and key events
    /// to the receiver; what it says, through the glue and the driver,
    /// comes out as the messages Windows would get.
    #[tokio::test]
    async fn libei_input_from_a_stand_in_compositor_reaches_the_computer_driven() {
        let (server, client) = UnixStream::pair().unwrap();
        let script: Vec<Frame> = vec![
            Box::new(|d| {
                d.interface::<eis::Pointer>()
                    .unwrap()
                    .motion_relative(12.5, -3.0);
            }),
            Box::new(|d| {
                d.interface::<eis::Button>()
                    .unwrap()
                    .button(0x110, eis::button::ButtonState::Press);
            }),
            Box::new(|d| {
                d.interface::<eis::Button>()
                    .unwrap()
                    .button(0x110, eis::button::ButtonState::Released);
            }),
            Box::new(|d| {
                let s = d.interface::<eis::Scroll>().unwrap();
                s.scroll(0.0, 30.0);
                s.scroll_discrete(0, 240);
            }),
            Box::new(|d| {
                d.interface::<eis::Keyboard>()
                    .unwrap()
                    .key(30, eis::keyboard::KeyState::Press);
            }),
            Box::new(|d| {
                d.interface::<eis::Keyboard>()
                    .unwrap()
                    .key(30, eis::keyboard::KeyState::Released);
            }),
        ];
        let eis = std::thread::spawn(move || fake_eis(server, script));
        let said = receive(client).await;
        eis.join().unwrap();
        let input: Vec<Portal> = said
            .iter()
            .filter(|p| !matches!(p, Portal::Closed(_)))
            .cloned()
            .collect();
        assert_eq!(
            input,
            vec![
                Portal::Motion { dx: 12.5, dy: -3.0 },
                Portal::Button {
                    code: 0x110,
                    down: true
                },
                Portal::Button {
                    code: 0x110,
                    down: false
                },
                Portal::Scroll {
                    dx: 0.0,
                    dy: 30.0,
                    discrete: Some((0, 240))
                },
                Portal::Key {
                    code: 30,
                    down: true
                },
                Portal::Key {
                    code: 30,
                    down: false
                },
            ],
            "{said:?}"
        );

        // Through the glue onto Windows ("W"), to the right of this laptop.
        let placed = |id: &str, origin, w, h| Placed {
            device_id: id.to_string(),
            name: id.to_string(),
            origin,
            screens: vec![Screen {
                x: 0,
                y: 0,
                width: w,
                height: h,
            }],
        };
        let layout = Layout {
            computers: vec![
                placed("L", (0, 0), 1600, 1000),
                placed("W", (1600, 0), 1920, 1080),
            ],
        };
        let mut driver = Driver::new("L", layout.clone());
        driver.set_reachable(vec!["W".to_string()]);
        let mut glue = PortalGlue::new("L", Fake, None);
        let now = Instant::now();
        glue.portal(
            Portal::Zones(layout.computers[0].screens.clone()),
            &mut driver,
            now,
        );
        glue.set_targets(&layout, &["W".to_string()]);
        let start = driver.start();
        glue.act(start);
        glue.portal(
            Portal::Activated {
                activation: Some(1),
                barrier: Some(1),
                at: Some((1600.0, 500.0)),
            },
            &mut driver,
            now,
        );
        assert_eq!(driver.away_on(), Some("W"));
        let msgs: Vec<Msg> = input
            .into_iter()
            .flat_map(|p| glue.portal(p, &mut driver, now))
            .filter_map(|a| match a {
                Action::Send(to, msg) if to == "W" => Some(msg),
                _ => None,
            })
            .collect();
        let (x, y) = layout.normalized("W", (12, 497));
        assert_eq!(
            msgs,
            vec![
                Msg::Move { seq: 1, x, y },
                Msg::Button {
                    button: 0,
                    pressed: true
                },
                Msg::Button {
                    button: 0,
                    pressed: false
                },
                Msg::Wheel { dx: 0, dy: 2 },
                Msg::Key {
                    code: 30,
                    pressed: true
                },
                Msg::Key {
                    code: 30,
                    pressed: false
                },
            ]
        );
    }
}
