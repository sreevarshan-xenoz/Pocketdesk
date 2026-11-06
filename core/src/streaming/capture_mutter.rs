//! "Unattended on GNOME": the screen and, where asked, the keyboard and
//! pointer through mutter's own D-Bus API, with no dialog. The same calls
//! gnome-remote-desktop's headless mode makes:
//!
//! - `org.gnome.Mutter.ScreenCast` makes a session, `RecordMonitor` a stream
//!   (a PipeWire node, announced by `PipeWireStreamAdded` after `Start`);
//! - `org.gnome.Mutter.RemoteDesktop` makes the session that takes input;
//!   a ScreenCast session made with its `SessionId` is linked to it, and
//!   starting the RemoteDesktop session starts both.
//!
//! Only used with the owner's switch on, on a GNOME session
//! ([`super::gnome_unattended`]); any failure here sends the capture back to
//! the portal. The events are the portal's own ([`PortalEvent`]), so the
//! injector does not know which of the two it feeds.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use ashpd::zbus::zvariant::{OwnedValue, Value};
use ashpd::zbus::{Connection, Message};
use futures_lite::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::capture::{CaptureSession, monitor_rect};
use super::portal_input::{PortalEvent, WheelStyle, absolute_position, fine_wheel_call};
use super::window_target::Lease;
use super::{StreamingConfig, VideoFrame};

const SCREENCAST: &str = "org.gnome.Mutter.ScreenCast";
const SCREENCAST_PATH: &str = "/org/gnome/Mutter/ScreenCast";
const SCREENCAST_SESSION: &str = "org.gnome.Mutter.ScreenCast.Session";
const SCREENCAST_STREAM: &str = "org.gnome.Mutter.ScreenCast.Stream";
const REMOTE_DESKTOP: &str = "org.gnome.Mutter.RemoteDesktop";
const REMOTE_DESKTOP_PATH: &str = "/org/gnome/Mutter/RemoteDesktop";
const REMOTE_DESKTOP_SESSION: &str = "org.gnome.Mutter.RemoteDesktop.Session";
const DISPLAY_CONFIG: &str = "org.gnome.Mutter.DisplayConfig";
const DISPLAY_CONFIG_PATH: &str = "/org/gnome/Mutter/DisplayConfig";

/// How long any one call may take: mutter answers at once or not at all.
const CALL_WAIT: Duration = Duration::from_secs(10);

/// The options for a ScreenCast session: linked to the RemoteDesktop
/// session `remote_session_id` when the capture also takes input.
pub fn session_options(remote_session_id: Option<&str>) -> HashMap<&'static str, Value<'_>> {
    let mut options = HashMap::new();
    if let Some(id) = remote_session_id {
        options.insert("remote-desktop-session-id", Value::from(id));
    }
    options
}

/// The options for `RecordMonitor`: the cursor stays out of the picture (as
/// through the portal, where it is asked for hidden), and the stream says
/// it is a recording, which makes GNOME show its screen-sharing indicator
/// for as long as it runs. With `metadata` (a viewer that draws the
/// pointer itself, local cursor) the cursor is still out of the picture and
/// comes as the stream's cursor metadata (mutter's mode 2).
pub fn record_options(metadata: bool) -> HashMap<&'static str, Value<'static>> {
    HashMap::from([
        (
            "cursor-mode",
            Value::from(if metadata { 2u32 } else { 0u32 }),
        ),
        ("is-recording", Value::from(true)),
    ])
}

/// The options for `RecordWindow`: the window by mutter's id, and the same
/// two that `RecordMonitor` takes ([`record_options`]). The pointer is kept
/// out of the picture (`metadata` false: a window lent to look at is shown
/// with no trace of the owner's pointer).
pub fn window_record_options(id: u64) -> HashMap<&'static str, Value<'static>> {
    let mut options = record_options(false);
    options.insert("window-id", Value::from(id));
    options
}

/// The mutter window a session's lease holds: the id in its native name, and
/// only if the lease still stands. `Err` says why a window is not recorded.
pub fn leased_window(lease: &Lease) -> Result<u64> {
    let id = super::window_target::gnome_id(&lease.native)
        .context("that lease is not for a GNOME window")?;
    if !super::window_target::lease_alive(lease) {
        anyhow::bail!("That window is not lent to this session any more");
    }
    Ok(id)
}

/// One logical monitor as `DisplayConfig` lists it, reduced to what picks
/// a screen: where it is, whether it is the primary, its connectors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Logical {
    pub x: i32,
    pub y: i32,
    pub primary: bool,
    pub connectors: Vec<String>,
}

/// The connector to record for a capture of the monitor at `want` (its
/// top-left corner, as the portal path matches streams): the logical
/// monitor there, else the primary, else the first; `None` when there are
/// none (mutter is then asked for its primary with an empty connector).
pub fn pick_connector(logicals: &[Logical], want: Option<(i32, i32)>) -> Option<String> {
    let chosen = want
        .and_then(|pos| logicals.iter().find(|l| (l.x, l.y) == pos))
        .or_else(|| logicals.iter().find(|l| l.primary))
        .or_else(|| logicals.first())?;
    chosen.connectors.first().cloned()
}

/// The `NotifyPointerAxisDiscrete` calls that make `steps` wheel notches on
/// `axis` (0 vertical, 1 horizontal; positive is down, or right): mutter is
/// told one notch a call, each a wheel click of 120 in `axis_value120`.
pub fn axis_calls(axis: u32, steps: i32) -> Vec<(u32, i32)> {
    let one = steps.signum();
    (0..steps.unsigned_abs()).map(|_| (axis, one)).collect()
}

/// A monitor of `GetCurrentState`'s answer: `(connector, vendor, product,
/// serial)`.
type MonitorSpec = (String, String, String, String);

/// A logical monitor of `GetCurrentState`'s answer: `(x, y, scale,
/// transform, primary, monitors, properties)`.
type LogicalRow = (
    i32,
    i32,
    f64,
    u32,
    bool,
    Vec<MonitorSpec>,
    HashMap<String, OwnedValue>,
);

/// A mode of a physical monitor: `(id, width, height, refresh rate,
/// preferred scale, supported scales, properties)`.
type ModeRow = (
    String,
    i32,
    i32,
    f64,
    f64,
    Vec<f64>,
    HashMap<String, OwnedValue>,
);

/// A physical monitor of `GetCurrentState`'s answer: `(spec, modes,
/// properties)`. Only read so the answer's whole signature matches.
type PhysicalRow = (MonitorSpec, Vec<ModeRow>, HashMap<String, OwnedValue>);

/// The whole answer of `GetCurrentState`: `(serial, monitors, logical
/// monitors, properties)`.
type CurrentState = (
    u32,
    Vec<PhysicalRow>,
    Vec<LogicalRow>,
    HashMap<String, OwnedValue>,
);

/// The logical monitors mutter lists now.
async fn logical_monitors(conn: &Connection) -> Result<Vec<Logical>> {
    let reply = call(
        conn,
        DISPLAY_CONFIG,
        DISPLAY_CONFIG_PATH,
        DISPLAY_CONFIG,
        "GetCurrentState",
        &(),
    )
    .await?;
    let state: CurrentState = reply
        .body()
        .deserialize()
        .context("could not read the monitors' state")?;
    Ok(state
        .2
        .into_iter()
        .map(|(x, y, _, _, primary, monitors, _)| Logical {
            x,
            y,
            primary,
            connectors: monitors.into_iter().map(|m| m.0).collect(),
        })
        .collect())
}

/// One method call on the session bus, bounded.
async fn call<B>(
    conn: &Connection,
    destination: &str,
    path: &str,
    interface: &str,
    method: &str,
    body: &B,
) -> Result<Message>
where
    B: serde::ser::Serialize + ashpd::zbus::zvariant::DynamicType,
{
    let what = format!("{interface}.{method}");
    tokio::time::timeout(
        CALL_WAIT,
        conn.call_method(Some(destination), path, Some(interface), method, body),
    )
    .await
    .map_err(|_| anyhow::anyhow!("{what} timed out"))?
    .with_context(|| what)
}

/// An object path from a call's answer.
fn object_path(reply: &Message) -> Result<String> {
    let path: ashpd::zbus::zvariant::OwnedObjectPath = reply.body().deserialize()?;
    Ok(path.as_str().to_string())
}

/// A property of an object, as the value it holds.
async fn property(
    conn: &Connection,
    destination: &str,
    path: &str,
    interface: &str,
    name: &str,
) -> Result<OwnedValue> {
    let reply = call(
        conn,
        destination,
        path,
        "org.freedesktop.DBus.Properties",
        "Get",
        &(interface, name),
    )
    .await?;
    let value: OwnedValue = reply.body().deserialize()?;
    Ok(value)
}

/// What a started session gives the injector: where input goes.
struct Input {
    conn: Connection,
    session: String,
    stream: String,
    size: (u32, u32),
}

impl Input {
    /// Tell mutter one input event.
    async fn apply(
        &self,
        rest: &mut crate::streaming::wheel::Notches,
        event: PortalEvent,
    ) -> Result<()> {
        match event {
            PortalEvent::Motion { dx, dy } => {
                self.notify("NotifyPointerMotionRelative", &(dx, dy)).await
            }
            PortalEvent::Absolute { x_norm, y_norm } => {
                let (x, y) = absolute_position(x_norm, y_norm, self.size);
                self.notify("NotifyPointerMotionAbsolute", &(self.stream.as_str(), x, y))
                    .await
            }
            PortalEvent::Button { code, pressed } => {
                self.notify("NotifyPointerButton", &(code, pressed)).await
            }
            PortalEvent::Scroll { axis, steps } => self.turn(axis, steps).await,
            PortalEvent::FineScroll { axis, v120 } => {
                // Whole steps once the turns make one, as on the portal.
                let steps = match fine_wheel_call(WheelStyle::Discrete, axis, v120, rest) {
                    Some(super::portal_input::WheelCall::Discrete { steps, .. }) => steps,
                    _ => 0,
                };
                self.turn(axis, steps).await
            }
            PortalEvent::Key { code, pressed } => {
                self.notify("NotifyKeyboardKeycode", &(code as u32, pressed))
                    .await
            }
            PortalEvent::Keysym { sym, pressed } => {
                self.notify("NotifyKeyboardKeysym", &(sym as u32, pressed))
                    .await
            }
        }
    }

    /// The pointer to `(x, y)` in the window the stream is of (window-relative
    /// pixels; mutter clamps a position into the window's rectangle).
    async fn move_to(&self, x: f64, y: f64) -> Result<()> {
        self.notify("NotifyPointerMotionAbsolute", &(self.stream.as_str(), x, y))
            .await
    }

    /// `steps` wheel notches on `axis`, one call each.
    async fn turn(&self, axis: u32, steps: i32) -> Result<()> {
        for (axis, one) in axis_calls(axis, steps) {
            self.notify("NotifyPointerAxisDiscrete", &(axis, one))
                .await?;
        }
        Ok(())
    }

    async fn notify<B>(&self, method: &str, body: &B) -> Result<()>
    where
        B: serde::ser::Serialize + ashpd::zbus::zvariant::DynamicType,
    {
        call(
            &self.conn,
            REMOTE_DESKTOP,
            &self.session,
            REMOTE_DESKTOP_SESSION,
            method,
            body,
        )
        .await
        .map(|_| ())
    }
}

/// Start the capture through mutter: the screen alone, or (`input`) the
/// screen with the keyboard and pointer in one linked pair of sessions.
/// Fails, with the portal as the way on, if mutter's D-Bus is missing or
/// refuses.
pub async fn start_capture(
    config: StreamingConfig,
    frame_tx: crate::streaming::latest_frame::FrameSender<VideoFrame>,
    cancel: CancellationToken,
    input: bool,
    cursor: Option<super::cursor_feed::CursorFeed>,
) -> Result<CaptureSession> {
    // The pointer as metadata for a viewer that draws it (`cursor`): the
    // feed is told the picture has none in it only once the capture thread
    // runs (`spawn_pipewire_capture`), so a start that fails says nothing.
    let conn = tokio::time::timeout(CALL_WAIT, Connection::session())
        .await
        .map_err(|_| anyhow::anyhow!("the session bus did not answer"))?
        .context("no session bus")?;

    // Made in order: the RemoteDesktop session (when input is wanted), the
    // ScreenCast session linked to it, one monitor's stream.
    let remote = if input {
        let reply = call(
            &conn,
            REMOTE_DESKTOP,
            REMOTE_DESKTOP_PATH,
            REMOTE_DESKTOP,
            "CreateSession",
            &(),
        )
        .await?;
        Some(object_path(&reply)?)
    } else {
        None
    };
    // The ScreenCast session, linked to the RemoteDesktop one when there is
    // one; that one is stopped if this fails, not left to mutter (review).
    let made = async {
        let remote_id = match &remote {
            Some(path) => {
                let id = property(
                    &conn,
                    REMOTE_DESKTOP,
                    path,
                    REMOTE_DESKTOP_SESSION,
                    "SessionId",
                )
                .await?;
                Some(String::try_from(id).context("the session id is not a string")?)
            }
            None => None,
        };
        let reply = call(
            &conn,
            SCREENCAST,
            SCREENCAST_PATH,
            SCREENCAST,
            "CreateSession",
            &(session_options(remote_id.as_deref()),),
        )
        .await?;
        object_path(&reply)
    }
    .await;
    let cast = match made {
        Ok(cast) => cast,
        Err(e) => {
            if let Some(path) = &remote {
                let _ = call(
                    &conn,
                    REMOTE_DESKTOP,
                    path,
                    REMOTE_DESKTOP_SESSION,
                    "Stop",
                    &(),
                )
                .await;
            }
            return Err(e);
        }
    };
    // Closed when anything after this fails, so no session outlives it.
    let stop = |conn: Connection, remote: Option<String>, cast: String| async move {
        let (path, iface) = match &remote {
            Some(path) => (path.clone(), REMOTE_DESKTOP_SESSION),
            None => (cast, SCREENCAST_SESSION),
        };
        let _ = call(&conn, destination_of(iface), &path, iface, "Stop", &()).await;
    };

    let started = async {
        let connector = match logical_monitors(&conn).await {
            Ok(logicals) => {
                let want = monitor_rect(config.monitor_index).map(|(x, y, _, _)| (x, y));
                pick_connector(&logicals, want).unwrap_or_default()
            }
            // mutter's primary, asked for with no connector.
            Err(e) => {
                warn!(error = %e, "Mutter's monitors could not be listed: recording its primary");
                String::new()
            }
        };
        let reply = call(
            &conn,
            SCREENCAST,
            &cast,
            SCREENCAST_SESSION,
            "RecordMonitor",
            &(connector.as_str(), record_options(cursor.is_some())),
        )
        .await?;
        let stream = object_path(&reply)?;

        // The stream's node is announced once the session starts: listen
        // before starting it.
        let proxy = ashpd::zbus::Proxy::new(&conn, SCREENCAST, stream.as_str(), SCREENCAST_STREAM)
            .await
            .context("no stream object")?;
        let mut added = proxy
            .receive_signal("PipeWireStreamAdded")
            .await
            .context("could not listen for the stream")?;
        match &remote {
            Some(path) => {
                call(
                    &conn,
                    REMOTE_DESKTOP,
                    path,
                    REMOTE_DESKTOP_SESSION,
                    "Start",
                    &(),
                )
                .await?
            }
            None => call(&conn, SCREENCAST, &cast, SCREENCAST_SESSION, "Start", &()).await?,
        };
        let signal = tokio::time::timeout(CALL_WAIT, added.next())
            .await
            .map_err(|_| anyhow::anyhow!("mutter announced no stream"))?
            .context("the stream signal ended")?;
        let node_id: u32 = signal.body().deserialize().context("no node id")?;

        // The stream's size, for absolute moves; the capture's own when
        // mutter does not say.
        let size = match property(&conn, SCREENCAST, &stream, SCREENCAST_STREAM, "Parameters").await
        {
            Ok(parameters) => stream_size(parameters).unwrap_or((config.width, config.height)),
            Err(_) => (config.width, config.height),
        };
        Ok::<(u32, String, (u32, u32)), anyhow::Error>((node_id, stream, size))
    }
    .await;
    let (node_id, stream, size) = match started {
        Ok(started) => started,
        Err(e) => {
            stop(conn, remote, cast).await;
            return Err(e);
        }
    };
    info!(
        "Unattended GNOME capture through mutter: node_id={node_id}, {}x{}{}",
        size.0,
        size.1,
        if remote.is_some() {
            ", input through mutter"
        } else {
            ""
        }
    );

    // The session ends with the capture, or at once if the capture below
    // does not start: the portal then takes over under the same `cancel`,
    // and this session must not stay up beside it (review).
    let closer = cancel.child_token();
    let abandon = closer.clone();
    let closing = (conn.clone(), remote.clone(), cast.clone());
    let worker_input = remote.clone().map(|session| Input {
        conn: conn.clone(),
        session,
        stream,
        size,
    });
    let (refused_tx, refused_rx) = tokio::sync::watch::channel(false);
    let registered = worker_input.as_ref().map(|_| {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (super::portal_input::register(tx), rx)
    });
    tokio::spawn(async move {
        let mut said = false;
        if let (Some(input), Some((registered, mut rx))) = (worker_input, registered) {
            let _registered = registered;
            let mut rest = crate::streaming::wheel::Notches::default();
            loop {
                tokio::select! {
                    _ = closer.cancelled() => break,
                    event = rx.recv() => match event {
                        Some(event) => {
                            if let Err(e) = input.apply(&mut rest, event).await {
                                let text = format!("{e:#}");
                                if !std::mem::replace(&mut said, true) {
                                    warn!(error = %text, "Mutter input not taken");
                                }
                                if super::portal_input::input_refused_text(&text) {
                                    let _ = refused_tx.send(true);
                                }
                            }
                        }
                        None => {
                            closer.cancelled().await;
                            break;
                        }
                    },
                }
            }
        } else {
            closer.cancelled().await;
        }
        stop(closing.0, closing.1, closing.2).await;
    });

    let zero_copy = match super::capture_linux::spawn_pipewire_capture(
        node_id,
        config.clone(),
        frame_tx,
        cancel.clone(),
        cursor,
    ) {
        Ok(switch) => switch,
        Err(e) => {
            abandon.cancel();
            return Err(e);
        }
    };
    let session = CaptureSession::new(config, cancel).with_zero_copy(zero_copy);
    Ok(if remote.is_some() {
        session.with_input_refused(refused_rx)
    } else {
        session
    })
}

/// Capture the one window a session holds `lease` on (window handoff, phase
/// 6: GNOME as the source) through mutter's `RecordWindow`, which records a
/// window by id whatever is in front of it, with no dialog: the same
/// no-dialog ScreenCast calls [`start_capture`] makes, so the same owner's
/// switch and the same trust (SECURITY limit 75) apply.
///
/// Without `input` this is screen only: no RemoteDesktop session is made, so
/// nothing this capture does can send input, and the pointer is not in the
/// picture. With `input` (the Pocketdesk GNOME Shell extension answers for the
/// window, [`super::window_target::takes_input`]) the ScreenCast session is
/// linked to a RemoteDesktop session of its own, made for this window's
/// stream alone, and the session's guard-vetted packets come to
/// [`CaptureSession::window_input`]: each is placed *in the window* (a pixel
/// the window no longer holds is dropped, never clamped to its edge) and
/// sent to this session only, never through the shared injector, so no other
/// session's input can reach this window's stream or the other way round.
///
/// The capture ends, and its session is stopped, when the lease does not
/// stand any more (the window was given back or closed) as well as when
/// `cancel` is cancelled; it cancels `cancel` itself in that case, which ends
/// the streaming session. Failing to start leaves `cancel` alone.
pub async fn start_window_capture(
    config: StreamingConfig,
    frame_tx: crate::streaming::latest_frame::FrameSender<VideoFrame>,
    cancel: CancellationToken,
    lease: Lease,
    input: bool,
) -> Result<CaptureSession> {
    let id = leased_window(&lease)?;
    let conn = tokio::time::timeout(CALL_WAIT, Connection::session())
        .await
        .map_err(|_| anyhow::anyhow!("the session bus did not answer"))?
        .context("no session bus")?;
    // Made in order, as `start_capture` does: the RemoteDesktop session (when
    // input is wanted), the ScreenCast session linked to it.
    let remote = if input {
        let reply = call(
            &conn,
            REMOTE_DESKTOP,
            REMOTE_DESKTOP_PATH,
            REMOTE_DESKTOP,
            "CreateSession",
            &(),
        )
        .await?;
        Some(object_path(&reply)?)
    } else {
        None
    };
    // Closed when anything after this fails, so no session outlives it.
    let stop = |conn: Connection, remote: Option<String>, cast: String| async move {
        let (path, iface) = match &remote {
            Some(path) => (path.clone(), REMOTE_DESKTOP_SESSION),
            None => (cast, SCREENCAST_SESSION),
        };
        let _ = call(&conn, destination_of(iface), &path, iface, "Stop", &()).await;
    };
    let made = async {
        let remote_id = match &remote {
            Some(path) => {
                let id = property(
                    &conn,
                    REMOTE_DESKTOP,
                    path,
                    REMOTE_DESKTOP_SESSION,
                    "SessionId",
                )
                .await?;
                Some(String::try_from(id).context("the session id is not a string")?)
            }
            None => None,
        };
        let reply = call(
            &conn,
            SCREENCAST,
            SCREENCAST_PATH,
            SCREENCAST,
            "CreateSession",
            &(session_options(remote_id.as_deref()),),
        )
        .await?;
        object_path(&reply)
    }
    .await;
    let cast = match made {
        Ok(cast) => cast,
        Err(e) => {
            if let Some(path) = &remote {
                let _ = call(
                    &conn,
                    REMOTE_DESKTOP,
                    path,
                    REMOTE_DESKTOP_SESSION,
                    "Stop",
                    &(),
                )
                .await;
            }
            return Err(e);
        }
    };

    let started = async {
        let reply = call(
            &conn,
            SCREENCAST,
            &cast,
            SCREENCAST_SESSION,
            "RecordWindow",
            &(window_record_options(id),),
        )
        .await?;
        let stream = object_path(&reply)?;
        // The stream's node is announced once the session starts: listen
        // before starting it.
        let proxy = ashpd::zbus::Proxy::new(&conn, SCREENCAST, stream.as_str(), SCREENCAST_STREAM)
            .await
            .context("no stream object")?;
        let mut added = proxy
            .receive_signal("PipeWireStreamAdded")
            .await
            .context("could not listen for the stream")?;
        match &remote {
            Some(path) => {
                call(
                    &conn,
                    REMOTE_DESKTOP,
                    path,
                    REMOTE_DESKTOP_SESSION,
                    "Start",
                    &(),
                )
                .await?
            }
            None => call(&conn, SCREENCAST, &cast, SCREENCAST_SESSION, "Start", &()).await?,
        };
        let signal = tokio::time::timeout(CALL_WAIT, added.next())
            .await
            .map_err(|_| anyhow::anyhow!("mutter announced no stream"))?
            .context("the stream signal ended")?;
        let node_id: u32 = signal.body().deserialize().context("no node id")?;
        Ok::<(u32, String), anyhow::Error>((node_id, stream))
    }
    .await;
    let (node_id, stream) = match started {
        Ok(started) => started,
        Err(e) => {
            stop(conn, remote, cast).await;
            return Err(e);
        }
    };
    info!(
        "Lent GNOME window capture through mutter: node_id={node_id}{}",
        if remote.is_some() {
            ", input through mutter into the window"
        } else {
            ""
        }
    );

    // The session is stopped when the capture ends, whichever way, and at
    // once if the capture below does not start. It watches the lease too:
    // the registry says as it changes, and a slow look backs it up.
    let closer = cancel.child_token();
    let abandon = closer.clone();
    let session_end = cancel.clone();
    let watched = lease.clone();
    let closing = (conn.clone(), remote.clone(), cast.clone());
    tokio::spawn(async move {
        let mut changes = super::window_target::changes();
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                _ = closer.cancelled() => break,
                _ = changes.changed() => {}
                _ = tick.tick() => {}
            }
            if !super::window_target::lease_alive(&watched) {
                info!("The lent window is not lent any more: ending its capture");
                session_end.cancel();
                break;
            }
        }
        stop(closing.0, closing.1, closing.2).await;
    });

    // The window's own input worker: this session's packets only.
    let mut window_input = None;
    let mut refused = None;
    if let Some(session) = remote.clone() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let (refused_tx, refused_rx) = tokio::sync::watch::channel(false);
        tokio::spawn(window_input_worker(
            Input {
                conn: conn.clone(),
                session,
                stream,
                size: (0, 0),
            },
            id,
            rx,
            refused_tx,
            cancel.child_token(),
        ));
        window_input = Some(tx);
        refused = Some(refused_rx);
    }

    // The pointer is out of the picture (the capture is told there is no
    // cursor feed, so it never reports one). The stream is as big as a screen
    // with the window in a corner of it: it is asked which part is the window,
    // and each frame is cut to that. That part is the window's buffer, a
    // client-side decorated window's shadow with it; each picture is cut
    // again to the frame, which is the size the lend said.
    let (cut_tx, cut_rx) = crate::streaming::latest_frame::channel();
    tokio::spawn(cut_to_frame(
        conn.clone(),
        id,
        cut_rx,
        frame_tx,
        cancel.child_token(),
    ));
    let zero_copy = match super::capture_linux::spawn_pipewire_capture_with(
        node_id,
        config.clone(),
        cut_tx,
        cancel.clone(),
        None,
        true,
    ) {
        Ok(switch) => switch,
        Err(e) => {
            abandon.cancel();
            return Err(e);
        }
    };
    // A lent window's frames are cut on the processor: always copies.
    let mut session = CaptureSession::new(config, cancel).with_zero_copy(zero_copy);
    if let (Some(tx), Some(refused)) = (window_input, refused) {
        session = session.with_window_input(tx).with_input_refused(refused);
    }
    Ok(session)
}

/// Each picture of a lent window's stream, cut from its buffer to its frame
/// ([`gnome_windows::frame_in_picture`](super::gnome_windows::frame_in_picture)),
/// on to `out`. The window's place is asked of the extension now and then;
/// without it (a Shell with no extension) pictures go on as they are.
async fn cut_to_frame(
    conn: Connection,
    id: u64,
    mut pictures: crate::streaming::latest_frame::FrameReceiver<VideoFrame>,
    out: crate::streaming::latest_frame::FrameSender<VideoFrame>,
    end: CancellationToken,
) {
    use super::gnome_windows::{bus, frame_in_picture};
    let mut window = None;
    let mut asked: Option<std::time::Instant> = None;
    loop {
        let mut picture = tokio::select! {
            picture = pictures.recv() => match picture {
                Some(picture) => picture,
                None => break,
            },
            _ = end.cancelled() => break,
        };
        if asked.is_none_or(|at| at.elapsed() >= Duration::from_millis(500)) {
            asked = Some(std::time::Instant::now());
            window = bus::list(&conn)
                .await
                .ok()
                .and_then(|all| all.into_iter().find(|w| w.id == id));
        }
        if let Some((x, y, w, h)) = window
            .as_ref()
            .and_then(|w| frame_in_picture(w, (picture.width, picture.height)))
            && !picture.crop_region(x, y, w, h)
        {
            continue;
        }
        if out.try_send(picture).is_err() {
            break;
        }
    }
}

/// What the packets a lent window's guard let through do: into the window's
/// own RemoteDesktop session, each placed in the window by its frame as it is
/// when the packet is applied. Ends with `closer` or when the channel closes.
///
/// A button press or a scroll acts at the seat's pointer, so it is sent only
/// while the move before it put the pointer in the window
/// ([`Placement`]): a move that was dropped or failed leaves them dropped.
/// Packets queued before the end are all applied first, and what is still
/// queued as it ends is applied if it lets go of something, so a button or key
/// the session held is not left down.
async fn window_input_worker(
    input: Input,
    id: u64,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<super::input_packet::InputPacket>,
    refused_tx: tokio::sync::watch::Sender<bool>,
    closer: CancellationToken,
) {
    let mut rest = crate::streaming::wheel::Notches::default();
    let mut placement = super::gnome_windows::Placement::default();
    let mut said = false;
    loop {
        // The queue first: what the guard sent before the end (its releases
        // included) is not overtaken by the end.
        let packet = tokio::select! {
            biased;
            packet = rx.recv() => match packet {
                Some(packet) => packet,
                None => break,
            },
            _ = closer.cancelled() => break,
        };
        let result =
            apply_window_packet(&input, id, &mut placement, &mut rest, packet, false).await;
        note_input_failure(result, &mut said, &refused_tx);
    }
    // Whatever is still queued: only what lets go.
    while let Ok(packet) = rx.try_recv() {
        let _ = apply_window_packet(&input, id, &mut placement, &mut rest, packet, true).await;
    }
}

/// Tell mutter one packet of a lent window. With `releases_only` (the
/// session is ending) only what lets go of a button or key is told.
async fn apply_window_packet(
    input: &Input,
    id: u64,
    placement: &mut super::gnome_windows::Placement,
    rest: &mut crate::streaming::wheel::Notches,
    packet: super::input_packet::InputPacket,
    releases_only: bool,
) -> Result<()> {
    use super::gnome_windows::{
        bus, frame_offset, is_release, local_pointer, visible, window_events,
    };
    use super::input_packet::InputPacket;

    match packet {
        // The guard said a pixel of its own space: the window holds it
        // now, or nothing is sent and the pointer is not placed.
        InputPacket::MouseMoveAbs { x_norm, y_norm } => {
            if releases_only {
                return Ok(());
            }
            let window = bus::list(&input.conn)
                .await
                .ok()
                .and_then(|all| all.into_iter().find(|w| w.id == id))
                .filter(visible);
            // In the frame, then moved by the shadow mutter's stream has
            // round a client-side decorated window.
            let local = window.and_then(|w| {
                let (dx, dy) = frame_offset(&w);
                local_pointer((x_norm, y_norm), w.frame).map(|(x, y)| (x + dx, y + dy))
            });
            match placement.moved(local) {
                Some((x, y)) => {
                    let moved = input.move_to(f64::from(x), f64::from(y)).await;
                    if moved.is_err() {
                        placement.failed();
                    }
                    moved
                }
                None => Ok(()),
            }
        }
        other => {
            let mut result = Ok(());
            for event in window_events(&other) {
                if !placement.allows(&event) || (releases_only && !is_release(&event)) {
                    continue;
                }
                result = input.apply(rest, event).await;
                if result.is_err() {
                    break;
                }
            }
            result
        }
    }
}

/// Say, once, that mutter did not take the window's input, and tell the
/// phone when it was refused.
fn note_input_failure(
    result: Result<()>,
    said: &mut bool,
    refused_tx: &tokio::sync::watch::Sender<bool>,
) {
    if let Err(e) = result {
        let text = format!("{e:#}");
        if !std::mem::replace(said, true) {
            warn!(error = %text, "Mutter input not taken for the lent window");
        }
        if super::portal_input::input_refused_text(&text) {
            let _ = refused_tx.send(true);
        }
    }
}

/// The bus name an interface of the two session kinds is served by.
fn destination_of(interface: &str) -> &'static str {
    if interface == REMOTE_DESKTOP_SESSION {
        REMOTE_DESKTOP
    } else {
        SCREENCAST
    }
}

/// A stream's size from its `Parameters` (`size` is `(ii)`).
pub fn stream_size(parameters: OwnedValue) -> Option<(u32, u32)> {
    let parameters: HashMap<String, OwnedValue> = parameters.try_into().ok()?;
    let Value::Structure(size) = &**parameters.get("size")? else {
        return None;
    };
    let [w, h] = size.fields() else {
        return None;
    };
    let (w, h) = (i32::try_from(w).ok()?, i32::try_from(h).ok()?);
    Some((w.max(1) as u32, h.max(1) as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logical(x: i32, y: i32, primary: bool, connector: &str) -> Logical {
        Logical {
            x,
            y,
            primary,
            connectors: vec![connector.to_string()],
        }
    }

    #[test]
    fn a_session_is_linked_to_its_remote_desktop_session_only_when_there_is_one() {
        assert!(session_options(None).is_empty());
        let linked = session_options(Some("abc123"));
        assert_eq!(linked.len(), 1);
        assert_eq!(
            linked["remote-desktop-session-id"],
            Value::from("abc123"),
            "the id mutter gave the RemoteDesktop session"
        );
    }

    #[test]
    fn a_recording_hides_the_cursor_and_shows_gnomes_indicator() {
        let options = record_options(false);
        assert_eq!(options["cursor-mode"], Value::from(0u32), "hidden");
        assert_eq!(options["is-recording"], Value::from(true));
        // A viewer that draws the pointer gets it as metadata, still out of
        // the picture.
        let options = record_options(true);
        assert_eq!(options["cursor-mode"], Value::from(2u32), "metadata");
        assert_eq!(options["is-recording"], Value::from(true));
    }

    #[test]
    fn the_monitor_at_the_asked_corner_else_the_primary_else_the_first() {
        let monitors = [
            logical(0, 0, false, "DP-1"),
            logical(1920, 0, true, "HDMI-1"),
        ];
        assert_eq!(
            pick_connector(&monitors, Some((0, 0))).as_deref(),
            Some("DP-1")
        );
        assert_eq!(
            pick_connector(&monitors, Some((1920, 0))).as_deref(),
            Some("HDMI-1")
        );
        assert_eq!(
            pick_connector(&monitors, Some((5, 5))).as_deref(),
            Some("HDMI-1"),
            "no match: the primary"
        );
        assert_eq!(
            pick_connector(&monitors, None).as_deref(),
            Some("HDMI-1"),
            "none asked: the primary"
        );
        assert_eq!(
            pick_connector(&[logical(0, 0, false, "eDP-1")], None).as_deref(),
            Some("eDP-1"),
            "no primary: the first"
        );
        assert_eq!(pick_connector(&[], Some((0, 0))), None, "none at all");
    }

    #[test]
    fn a_window_is_recorded_by_its_id_with_the_pointer_out_of_the_picture() {
        let options = window_record_options(2609479513);
        assert_eq!(options["window-id"], Value::from(2609479513u64), "a t");
        assert_eq!(options["cursor-mode"], Value::from(0u32), "hidden");
        assert_eq!(options["is-recording"], Value::from(true));
        assert_eq!(options.len(), 3, "nothing else is asked of mutter");
    }

    #[test]
    fn only_a_standing_gnome_lease_names_a_window_to_record() {
        use super::super::window_target::{self, Lease};
        let device = "capture-mutter-lease-device";
        let (lease, _) = window_target::register_native("gnome:77", device, None).unwrap();
        assert_eq!(leased_window(&lease).unwrap(), 77);
        // A Windows lease, or a name that is not a GNOME window's, is none.
        let handle = Lease {
            native: String::new(),
            ..lease.clone()
        };
        assert!(leased_window(&handle).is_err());
        let odd = Lease {
            native: "gnome:0x4d".into(),
            ..lease.clone()
        };
        assert!(leased_window(&odd).is_err());
        // Given back, it records nothing.
        window_target::unregister(&lease);
        let gone = leased_window(&lease).unwrap_err().to_string();
        assert!(gone.contains("not lent"), "{gone}");
    }

    #[test]
    fn a_wheel_turn_is_one_notch_a_call_in_its_direction() {
        assert_eq!(axis_calls(0, 3), [(0, 1), (0, 1), (0, 1)]);
        assert_eq!(axis_calls(1, -2), [(1, -1), (1, -1)]);
        assert_eq!(axis_calls(0, 0), []);
    }

    #[test]
    fn a_streams_size_is_read_from_its_parameters() {
        let mut parameters: HashMap<String, Value> = HashMap::new();
        parameters.insert("size".into(), Value::from((1280i32, 720i32)));
        parameters.insert("position".into(), Value::from((0i32, 0i32)));
        let owned = OwnedValue::try_from(Value::from(parameters)).unwrap();
        assert_eq!(stream_size(owned), Some((1280, 720)));
        let none: HashMap<String, Value> = HashMap::new();
        assert_eq!(
            stream_size(OwnedValue::try_from(Value::from(none)).unwrap()),
            None
        );
    }
}
