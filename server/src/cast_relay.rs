//! The phone's screen on the computer (T10k): the server's half of handing a
//! cast to the desktop app. One relay serves the process. It listens on
//! loopback; while a cast is admitted it names the port and a fresh token in
//! `cast.json` (owner-only, `core::local::cast`), and the one viewer that
//! sends the token gets the cast's config and frames. The newest cast
//! replaces an older one, and the newest viewer an older viewer.
//!
//! Phone apps tier 2: the same cast also carries up to
//! [`MAX_WINDOWS`] app windows, each with a file and a token of its own
//! (`cast-window-<n>.json`) and a viewer of its own; a phone that shows only
//! app windows has no `cast.json`.

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use pocketdesk_core::local::cast::{self as wire, CastFile, Message, TOKEN_LEN};
use pocketdesk_core::local::paths::CAST_FILE;
use pocketdesk_core::streaming::cast::{CastChain, CastEvent, CastPayload, MAX_WINDOWS};
use rand::RngCore;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

/// The viewer is told the relay is alive this often (a still phone sends
/// no frames), and an ended cast whose `End` was lost is noticed.
const HEARTBEAT: Duration = Duration::from_secs(2);
/// A viewer sends its token within this.
const TOKEN_WAIT: Duration = Duration::from_secs(2);
/// Messages waiting for a slow viewer; past this, frames are dropped and the
/// phone is asked for a keyframe.
const VIEWER_QUEUE: usize = 32;
/// An app window nobody shows is closed on the phone after this (the
/// viewer's own close message normally comes first; this is for a desktop
/// app that quit or crashed).
const ORPHAN_WINDOW: Duration = Duration::from_secs(30);
/// Connections that have not sent the token yet, at most; more are closed.
const PENDING_VIEWERS: usize = 4;

/// Start the relay; the server's cast sessions send it what they receive.
pub fn spawn(state_dir: PathBuf) -> mpsc::Sender<CastEvent> {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(async move {
        match TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await {
            Ok(listener) => run(rx, listener, state_dir).await,
            // The receiver drops, and each cast says the relay is gone.
            Err(e) => tracing::warn!(error = %e, "The cast relay could not listen"),
        }
    });
    tx
}

/// The windows of a cast, by id less one.
type Windows = [Option<WindowCast>; MAX_WINDOWS as usize];

/// One app window of the cast (Phone apps tier 2).
struct WindowCast {
    token: [u8; TOKEN_LEN],
    config: Option<(u16, u16)>,
    /// Whether the phone takes touch and typing in this window, as it last said.
    input: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Paces the keyframe asks a slow viewer of this window makes.
    asks: CastChain,
    viewer: Option<Viewer>,
    /// Since when no viewer has been showing this window, if none is: a
    /// window nobody shows is closed on the phone after [`ORPHAN_WINDOW`].
    unviewed_since: Option<std::time::Instant>,
}

struct Cast {
    session_id: u64,
    device_id: String,
    /// The phone shows its screen too: `cast.json` names it. False for a
    /// phone that shows only app windows.
    screen: bool,
    want: mpsc::Sender<()>,
    /// Control batches for the phone (C8).
    control: mpsc::Sender<Vec<u8>>,
    /// Whether the phone takes control now, as it last said (C8).
    control_on: std::sync::Arc<std::sync::atomic::AtomicBool>,
    token: [u8; TOKEN_LEN],
    config: Option<(u16, u16)>,
    /// Paces the keyframe asks a slow viewer makes.
    asks: CastChain,
}

/// What an older camera cast is closed with when another phone's camera
/// replaces it (F6 part 2).
pub const REPLACED_CAMERA: &[u8] = b"another phone is the camera now";

/// A camera cast (F6 part 2) and the camera it feeds.
struct Webcam {
    session_id: u64,
    want: mpsc::Sender<()>,
    worker: crate::phone_camera::Worker,
}

struct Viewer {
    session_id: u64,
    tx: mpsc::Sender<Message>,
    /// What it sends, read for as long as it is the viewer and no longer:
    /// a viewer replaced (another joined, or the cast changed) can no
    /// longer reach the phone (C8 review).
    reader: tokio::task::JoinHandle<()>,
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// The relay's loop, over `listener` (bound to loopback).
pub async fn run(mut events: mpsc::Receiver<CastEvent>, listener: TcpListener, state_dir: PathBuf) {
    let port = match listener.local_addr() {
        Ok(addr) => addr.port(),
        Err(e) => {
            tracing::warn!(error = %e, "The cast relay has no port");
            return;
        }
    };
    // A file left by a server that stopped mid-cast names nothing now.
    let _ = std::fs::remove_file(state_dir.join(CAST_FILE));
    let mut cast: Option<Cast> = None;
    let mut viewer: Option<Viewer> = None;
    let mut windows: Windows = Default::default();
    // F6 part 2: a camera cast goes to a camera, not a viewer; beside a
    // screen cast, never replacing it.
    let mut webcam: Option<Webcam> = None;
    let (joined_tx, mut joined_rx) = mpsc::channel::<(u64, Option<u8>, TcpStream)>(4);
    let pending = std::sync::Arc::new(tokio::sync::Semaphore::new(PENDING_VIEWERS));
    let mut heartbeat = tokio::time::interval(HEARTBEAT);
    loop {
        tokio::select! {
            event = events.recv() => {
                let Some(event) = event else { break };
                match event {
                    CastEvent::Start { session_id, device_id, want_keyframe, control: _, webcam: true, windows_only: _ } => {
                        // The newest camera cast replaces an older one, and
                        // the older ends, its phone told why, so its camera
                        // does not stay open for nothing (PR #604 review N2).
                        if let Some(old) = webcam.take() {
                            let _ = pocketdesk_core::streaming::sessions::end_session(
                                old.session_id,
                                REPLACED_CAMERA,
                            );
                            tracing::info!(session_id = old.session_id, "A phone's camera was replaced by another's");
                        }
                        match crate::phone_camera::Worker::start(session_id, device_id, want_keyframe.clone()) {
                            Ok(worker) => {
                                tracing::info!(session_id, "A phone's camera can be this computer's");
                                webcam = Some(Webcam { session_id, want: want_keyframe, worker });
                            }
                            Err(e) => tracing::warn!(error = %e, "The phone's camera could not start"),
                        }
                    }
                    CastEvent::Start { session_id, device_id, want_keyframe, control, webcam: false, windows_only } => {
                        end(&mut cast, &mut viewer, &mut windows, &state_dir);
                        let mut token = [0u8; TOKEN_LEN];
                        rand::rngs::OsRng.fill_bytes(&mut token);
                        let device_id = device_id.unwrap_or_default();
                        if windows_only {
                            // No screen of the phone's: its app windows are
                            // announced as each one starts.
                            tracing::info!(session_id, "A phone's app windows can be shown");
                        } else {
                            let file = CastFile {
                                session_id,
                                device_id: device_id.clone(),
                                port,
                                token: wire::token_hex(&token),
                                window: None,
                            };
                            if let Err(e) = wire::write(&state_dir, &file) {
                                tracing::warn!(error = %e, "Could not say where the cast is");
                            }
                            tracing::info!(session_id, "A phone's screen can be shown");
                        }
                        cast = Some(Cast {
                            session_id,
                            device_id,
                            screen: !windows_only,
                            want: want_keyframe,
                            control,
                            control_on: Default::default(),
                            token,
                            config: None,
                            asks: CastChain::default(),
                        });
                    }
                    CastEvent::Payload { session_id, payload } if webcam.as_ref().is_some_and(|w| w.session_id == session_id) => {
                        let Some(w) = webcam.as_ref() else { continue };
                        let feed = match payload {
                            CastPayload::Config { width, height } => {
                                crate::phone_camera::Feed::Config { width, height }
                            }
                            CastPayload::Frame { keyframe, seq, data } => {
                                crate::phone_camera::Feed::Frame { keyframe, seq, data }
                            }
                            // No control, and its sound is not the camera's.
                            CastPayload::ControlState { .. }
                            | CastPayload::Audio { .. }
                            | CastPayload::WindowConfig { .. }
                            | CastPayload::WindowFrame { .. }
                            | CastPayload::WindowClosed { .. } => continue,
                        };
                        let config = matches!(feed, crate::phone_camera::Feed::Config { .. });
                        if !w.worker.send(feed) && config {
                            // A size the camera missed would decode wrong:
                            // the camera starts again with the next keyframe.
                            let _ = w.want.try_send(());
                        }
                    }
                    CastEvent::Payload { session_id, payload } => {
                        let Some(current) = cast.as_mut().filter(|c| c.session_id == session_id) else {
                            continue;
                        };
                        // An app window of the phone (Phone apps tier 2).
                        if relay_window(&payload, current, &mut windows, port, &state_dir) {
                            continue;
                        }
                        let is_config = matches!(payload, CastPayload::Config { .. });
                        let message = match payload {
                            CastPayload::Config { width, height } => {
                                current.config = Some((width, height));
                                Message::Config { width, height }
                            }
                            CastPayload::Frame { keyframe, seq, data } => {
                                Message::Frame { keyframe, seq, data }
                            }
                            CastPayload::ControlState { on } => {
                                current
                                    .control_on
                                    .store(on, std::sync::atomic::Ordering::Release);
                                Message::ControlState { on }
                            }
                            // Its sound goes to the player, never the
                            // viewer (K5): the streamer keeps it from here.
                            CastPayload::Audio { .. } => continue,
                            // Handled above, a window's own.
                            CastPayload::WindowConfig { .. }
                            | CastPayload::WindowFrame { .. }
                            | CastPayload::WindowClosed { .. } => continue,
                        };
                        if let Some(v) = viewer.as_ref().filter(|v| v.session_id == session_id)
                            && v.tx.try_send(message).is_err()
                        {
                            if is_config {
                                // A viewer that missed a new size would decode
                                // at the old one: it is dropped, and rejoins
                                // with the config.
                                viewer = None;
                            } else if current.asks.lost(std::time::Instant::now()) {
                                // A frame lost to a slow viewer breaks its chain.
                                let _ = current.want.try_send(());
                            }
                        }
                    }
                    CastEvent::End { session_id } => {
                        if webcam.as_ref().is_some_and(|w| w.session_id == session_id) {
                            // The node goes with the camera.
                            webcam = None;
                        }
                        if cast.as_ref().is_some_and(|c| c.session_id == session_id) {
                            end(&mut cast, &mut viewer, &mut windows, &state_dir);
                        }
                    }
                }
            }
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                // Nothing to show, or too many waiting to say the token:
                // the connection is simply closed.
                if let Some(c) = &cast {
                    let mut tokens: Vec<(Option<u8>, [u8; TOKEN_LEN])> = Vec::new();
                    if c.screen {
                        tokens.push((None, c.token));
                    }
                    for (slot, w) in windows.iter().enumerate() {
                        if let Some(w) = w {
                            tokens.push((Some(slot as u8 + 1), w.token));
                        }
                    }
                    if !tokens.is_empty()
                        && let Ok(slot) = pending.clone().try_acquire_owned()
                    {
                        let (session_id, joined) = (c.session_id, joined_tx.clone());
                        tokio::spawn(async move {
                            authenticate(stream, tokens, session_id, joined).await;
                            drop(slot);
                        });
                    }
                }
            }
            Some((session_id, window, stream)) = joined_rx.recv() => {
                let Some(c) = cast.as_ref().filter(|c| c.session_id == session_id) else {
                    continue;
                };
                // An app window's viewer (Phone apps tier 2): the same
                // messages, its own picture.
                if let Some(window) = window {
                    let Some(w) = windows[usize::from(window) - 1].as_mut() else {
                        continue;
                    };
                    let v = start_viewer(
                        stream,
                        session_id,
                        c.want.clone(),
                        c.control.clone(),
                        w.input.clone(),
                        Some(window),
                    );
                    if let Some((width, height)) = w.config {
                        let _ = v.tx.try_send(Message::Config { width, height });
                    }
                    if w.input.load(std::sync::atomic::Ordering::Acquire) {
                        let _ = v.tx.try_send(Message::ControlState { on: true });
                    }
                    let _ = c.want.try_send(());
                    tracing::info!(session_id, window, "A phone's app window is being shown");
                    w.viewer = Some(v);
                    w.unviewed_since = None;
                    continue;
                }
                let v = start_viewer(
                    stream,
                    session_id,
                    c.want.clone(),
                    c.control.clone(),
                    c.control_on.clone(),
                    None,
                );
                if let Some((width, height)) = c.config {
                    let _ = v.tx.try_send(Message::Config { width, height });
                }
                // It learns at once that it may control the phone (C8). Only a
                // yes is said: a desktop app from before C8 cannot read the
                // message and would leave, so a cast without control never
                // sends it one.
                if c.control_on.load(std::sync::atomic::Ordering::Acquire) {
                    let _ = v.tx.try_send(Message::ControlState { on: true });
                }
                // A new viewer starts from a keyframe.
                let _ = c.want.try_send(());
                tracing::info!(session_id, "The phone's screen is being shown");
                viewer = Some(v);
            }
            _ = heartbeat.tick() => {
                // Its session ended without a word (the `End` was lost).
                if webcam.as_ref().is_some_and(|w| w.want.is_closed()) {
                    webcam = None;
                }
                if cast.as_ref().is_some_and(|c| c.want.is_closed()) {
                    end(&mut cast, &mut viewer, &mut windows, &state_dir);
                }
                for (slot, w) in windows.iter_mut().enumerate() {
                    let Some(w) = w else { continue };
                    if let Some(v) = &w.viewer {
                        if v.tx.try_send(Message::Heartbeat).is_err() && v.tx.is_closed() {
                            w.viewer = None;
                        } else if w.input.load(std::sync::atomic::Ordering::Acquire) {
                            let _ = v.tx.try_send(Message::ControlState { on: true });
                        }
                    }
                    if w.viewer.is_some() {
                        w.unviewed_since = None;
                        continue;
                    }
                    // Nobody shows it: the phone is asked to close it after a
                    // while, and again until it says it did.
                    let since = *w.unviewed_since.get_or_insert_with(std::time::Instant::now);
                    if since.elapsed() >= ORPHAN_WINDOW
                        && let Some(c) = cast.as_ref()
                    {
                        close_on_phone(&c.control, slot as u8 + 1);
                        w.unviewed_since = Some(std::time::Instant::now());
                    }
                }
                if viewer.as_ref().is_some_and(|v| v.tx.try_send(Message::Heartbeat).is_err() && v.tx.is_closed()) {
                    viewer = None;
                }
                // The phone says its control state once: a viewer whose queue
                // was full then hears it again (C8 review).
                if let (Some(c), Some(v)) = (&cast, &viewer)
                    && v.session_id == c.session_id
                    && c.control_on.load(std::sync::atomic::Ordering::Acquire)
                {
                    let _ = v.tx.try_send(Message::ControlState { on: true });
                }
            }
        }
    }
    end(&mut cast, &mut viewer, &mut windows, &state_dir);
}

/// One of the cast's app windows (Phone apps tier 2) changed: its config
/// starts its file and viewer socket, a frame goes to its viewer, and its
/// end tells the viewer and forgets the file. Whether `payload` was a
/// window's.
fn relay_window(
    payload: &CastPayload,
    cast: &mut Cast,
    windows: &mut Windows,
    port: u16,
    state_dir: &Path,
) -> bool {
    match payload {
        CastPayload::WindowConfig {
            window,
            width,
            height,
            input,
        } => {
            let slot = usize::from(*window) - 1;
            if windows[slot].is_none() {
                let mut token = [0u8; TOKEN_LEN];
                rand::rngs::OsRng.fill_bytes(&mut token);
                let file = CastFile {
                    session_id: cast.session_id,
                    device_id: cast.device_id.clone(),
                    port,
                    token: wire::token_hex(&token),
                    window: Some(*window),
                };
                if let Err(e) = wire::write_window(state_dir, &file) {
                    tracing::warn!(error = %e, window, "Could not say where the app window is");
                }
                tracing::info!(
                    session_id = cast.session_id,
                    window,
                    "A phone's app window can be shown"
                );
                windows[slot] = Some(WindowCast {
                    token,
                    config: None,
                    input: Default::default(),
                    asks: CastChain::default(),
                    viewer: None,
                    unviewed_since: Some(std::time::Instant::now()),
                });
            }
            if let Some(w) = windows[slot].as_mut() {
                w.config = Some((*width, *height));
                w.input.store(*input, std::sync::atomic::Ordering::Release);
                if let Some(v) = &w.viewer {
                    // A viewer that missed a new size would decode at the
                    // old one: it is dropped, and rejoins with the config.
                    if v.tx
                        .try_send(Message::Config {
                            width: *width,
                            height: *height,
                        })
                        .is_err()
                    {
                        w.viewer = None;
                    } else {
                        let _ = v.tx.try_send(Message::ControlState { on: *input });
                    }
                }
            }
            true
        }
        CastPayload::WindowFrame {
            window,
            keyframe,
            seq,
            data,
        } => {
            if let Some(w) = windows[usize::from(*window) - 1].as_mut()
                && let Some(v) = &w.viewer
                && v.tx
                    .try_send(Message::Frame {
                        keyframe: *keyframe,
                        seq: *seq,
                        data: data.clone(),
                    })
                    .is_err()
                && w.asks.lost(std::time::Instant::now())
            {
                let _ = cast.want.try_send(());
            }
            true
        }
        CastPayload::WindowClosed { window } => {
            end_window(windows, *window, cast.session_id, state_dir);
            true
        }
        _ => false,
    }
}

/// Ask the phone to close app window `window` and give its display back.
fn close_on_phone(control: &mpsc::Sender<Vec<u8>>, window: u8) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    if let Some(batch) =
        pocketdesk_core::streaming::cast_control::ControlBatch::close_window(now, window)
            .and_then(|b| b.encode())
    {
        let _ = control.try_send(batch);
    }
}

/// An app window is over: its viewer is told and its file forgotten.
fn end_window(windows: &mut Windows, window: u8, session_id: u64, state_dir: &Path) {
    if let Some(w) = windows[usize::from(window) - 1].take() {
        if let Some(v) = &w.viewer {
            let _ = v.tx.try_send(Message::End);
        }
        if let Err(e) = wire::remove_window(state_dir, window, session_id) {
            tracing::warn!(error = %e, window, "Could not forget the app window");
        }
        tracing::info!(
            session_id,
            window,
            "A phone's app window is no longer shown"
        );
    }
}

/// The cast is over: the viewer is told, and `cast.json` forgets it, and
/// so do the app windows.
fn end(
    cast: &mut Option<Cast>,
    viewer: &mut Option<Viewer>,
    windows: &mut Windows,
    state_dir: &Path,
) {
    if let Some(v) = viewer.take() {
        let _ = v.tx.try_send(Message::End);
    }
    if let Some(c) = cast.take() {
        for window in 1..=MAX_WINDOWS {
            end_window(windows, window, c.session_id, state_dir);
        }
        if let Err(e) = wire::remove(state_dir, c.session_id) {
            tracing::warn!(error = %e, "Could not forget the cast");
        }
        tracing::info!(
            session_id = c.session_id,
            "The phone's screen is no longer shown"
        );
    }
}

/// Which of `tokens` (the screen's, `None`, and each app window's) a token
/// sent is, comparing it with all of them in the same time.
fn token_owner(tokens: &[(Option<u8>, [u8; TOKEN_LEN])], given: &[u8]) -> Option<Option<u8>> {
    let mut owner = None;
    for (window, token) in tokens {
        if wire::token_matches(token, given) {
            owner = Some(*window);
        }
    }
    owner
}

/// A connection becomes the viewer of the screen or of an app window once it
/// sends that one's token.
async fn authenticate(
    mut stream: TcpStream,
    tokens: Vec<(Option<u8>, [u8; TOKEN_LEN])>,
    session_id: u64,
    joined: mpsc::Sender<(u64, Option<u8>, TcpStream)>,
) {
    let mut given = [0u8; TOKEN_LEN];
    match tokio::time::timeout(TOKEN_WAIT, stream.read_exact(&mut given)).await {
        Ok(Ok(_)) => match token_owner(&tokens, &given) {
            Some(window) => {
                let _ = joined.send((session_id, window, stream)).await;
            }
            None => tracing::warn!("Refused a cast viewer without the token"),
        },
        _ => tracing::warn!("Refused a cast viewer without the token"),
    }
}

/// The viewer's two halves: messages go out in order, and what it may
/// send, a keyframe ask and control while the phone takes it (C8), reaches
/// the phone.
fn start_viewer(
    stream: TcpStream,
    session_id: u64,
    want: mpsc::Sender<()>,
    control: mpsc::Sender<Vec<u8>>,
    control_on: std::sync::Arc<std::sync::atomic::AtomicBool>,
    window: Option<u8>,
) -> Viewer {
    let _ = stream.set_nodelay(true);
    let (mut read, mut write) = stream.into_split();
    let (tx, mut rx) = mpsc::channel::<Message>(VIEWER_QUEUE);
    tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            let end = message == Message::End;
            if write.write_all(&message.encode()).await.is_err() || end {
                break;
            }
        }
        let _ = write.shutdown().await;
    });
    let reader = tokio::spawn(async move {
        let mut head = [0u8; 5];
        while read.read_exact(&mut head).await.is_ok() {
            let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
            match head[0] {
                wire::WANT_KEYFRAME if len == 0 => {
                    let _ = want.try_send(());
                }
                wire::CONTROL if len > 0 && len <= wire::MAX_CONTROL => {
                    let mut batch = vec![0u8; len];
                    if read.read_exact(&mut batch).await.is_err() {
                        break;
                    }
                    // Only what the phone can read, and only while it takes
                    // control: a full queue drops (the newest input wins
                    // next time). The screen's viewer may name no window; an
                    // app window's viewer reaches its own window only
                    // (Phone apps tier 2).
                    if control_on.load(std::sync::atomic::Ordering::Acquire)
                        && let Some(batch) = control_batch(&batch, window)
                    {
                        let _ = control.try_send(batch);
                    }
                }
                // The owner closed an app window's window: the phone gives
                // the display back. Only an app window's viewer may say so.
                wire::CLOSE_WINDOW if len == 0 && window.is_some() => {
                    if let Some(window) = window {
                        close_on_phone(&control, window);
                    }
                }
                _ => break,
            }
        }
    });
    Viewer {
        session_id,
        tx,
        reader,
    }
}

/// What a viewer's control batch becomes on its way to the phone: the batch
/// as sent for the screen's viewer (`window` is `None`) unless it names a
/// window, and for an app window's viewer the batch made for that window;
/// `None` for anything the phone could not read.
fn control_batch(batch: &[u8], window: Option<u8>) -> Option<Vec<u8>> {
    let decoded = pocketdesk_core::streaming::cast_control::ControlBatch::decode(batch)?;
    match window {
        None => decoded.window().is_none().then(|| batch.to_vec()),
        Some(window) => decoded.for_window(window)?.encode(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn read_message(stream: &mut TcpStream) -> Message {
        let mut head = [0u8; 5];
        stream.read_exact(&mut head).await.expect("head");
        let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
        let mut whole = head.to_vec();
        whole.resize(5 + len, 0);
        stream.read_exact(&mut whole[5..]).await.expect("payload");
        Message::read(&mut std::io::Cursor::new(whole)).expect("message")
    }

    async fn cast_file(dir: &Path) -> CastFile {
        for _ in 0..100 {
            if let Some(file) = wire::read(dir) {
                return file;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no cast.json");
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-cast-relay-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn a_viewer_with_the_token_sees_the_cast_and_asks_for_keyframes() {
        let dir = temp_dir("viewer");
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let (events, rx) = mpsc::channel(64);
        let relay = tokio::spawn(run(rx, listener, dir.clone()));

        let (want_tx, mut want_rx) = mpsc::channel(4);
        events
            .send(CastEvent::Start {
                session_id: 7,
                device_id: Some("phone".into()),
                want_keyframe: want_tx,
                control: mpsc::channel(4).0,
                webcam: false,
                windows_only: false,
            })
            .await
            .unwrap();
        events
            .send(CastEvent::Payload {
                session_id: 7,
                payload: CastPayload::Config {
                    width: 864,
                    height: 1920,
                },
            })
            .await
            .unwrap();
        let file = cast_file(&dir).await;
        assert_eq!(file.session_id, 7);
        assert_eq!(file.device_id, "phone");

        let mut view = TcpStream::connect((Ipv4Addr::LOCALHOST, file.port))
            .await
            .unwrap();
        view.write_all(&file.token_bytes().unwrap()).await.unwrap();
        assert_eq!(
            read_message(&mut view).await,
            Message::Config {
                width: 864,
                height: 1920
            }
        );
        assert_eq!(
            want_rx.recv().await,
            Some(()),
            "a new viewer starts from a keyframe"
        );

        // Another session's frame is not this cast's.
        for session_id in [8, 7] {
            events
                .send(CastEvent::Payload {
                    session_id,
                    payload: CastPayload::Frame {
                        keyframe: true,
                        seq: session_id,
                        data: vec![0, 0, 0, 1, 0x65],
                    },
                })
                .await
                .unwrap();
        }
        assert_eq!(
            read_message(&mut view).await,
            Message::Frame {
                keyframe: true,
                seq: 7,
                data: vec![0, 0, 0, 1, 0x65]
            }
        );

        view.write_all(&Message::WantKeyframe.encode())
            .await
            .unwrap();
        assert_eq!(
            want_rx.recv().await,
            Some(()),
            "the viewer's ask reaches the phone"
        );

        events.send(CastEvent::End { session_id: 7 }).await.unwrap();
        assert_eq!(read_message(&mut view).await, Message::End);
        for _ in 0..100 {
            if wire::read(&dir).is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(wire::read(&dir), None, "cast.json forgets the cast");

        drop(events);
        relay.await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_viewers_control_reaches_the_phone_only_while_it_takes_it() {
        let dir = temp_dir("control");
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let (events, rx) = mpsc::channel(64);
        let relay = tokio::spawn(run(rx, listener, dir.clone()));
        let (want_tx, _want_rx) = mpsc::channel(4);
        let (control_tx, mut control_rx) = mpsc::channel(4);
        events
            .send(CastEvent::Start {
                session_id: 3,
                device_id: None,
                want_keyframe: want_tx,
                control: control_tx,
                webcam: false,
                windows_only: false,
            })
            .await
            .unwrap();
        let file = cast_file(&dir).await;
        let mut view = TcpStream::connect((Ipv4Addr::LOCALHOST, file.port))
            .await
            .unwrap();
        view.write_all(&file.token_bytes().unwrap()).await.unwrap();
        let batch = pocketdesk_core::streaming::cast_control::ControlBatch {
            sent_ms: 1,
            events: vec![pocketdesk_core::streaming::cast_control::ControlEvent::Backspace],
        }
        .encode()
        .unwrap();
        view.write_all(&Message::Control(batch.clone()).encode())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            control_rx.try_recv().is_err(),
            "the phone had not said it takes control"
        );

        events
            .send(CastEvent::Payload {
                session_id: 3,
                payload: CastPayload::ControlState { on: true },
            })
            .await
            .unwrap();
        assert_eq!(
            read_message(&mut view).await,
            Message::ControlState { on: true }
        );
        // What the phone could not read is dropped; what it can goes.
        view.write_all(&Message::Control(vec![1, 2, 3]).encode())
            .await
            .unwrap();
        view.write_all(&Message::Control(batch.clone()).encode())
            .await
            .unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), control_rx.recv())
                .await
                .unwrap(),
            Some(batch.clone())
        );
        assert!(
            control_rx.try_recv().is_err(),
            "the malformed batch went nowhere"
        );
        // Another viewer joins: the first can no longer reach the phone.
        let mut second = TcpStream::connect((Ipv4Addr::LOCALHOST, file.port))
            .await
            .unwrap();
        second
            .write_all(&file.token_bytes().unwrap())
            .await
            .unwrap();
        assert_eq!(
            read_message(&mut second).await,
            Message::ControlState { on: true }
        );
        let _ = view
            .write_all(&Message::Control(batch.clone()).encode())
            .await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            control_rx.try_recv().is_err(),
            "the replaced viewer's control went nowhere"
        );
        second
            .write_all(&Message::Control(batch.clone()).encode())
            .await
            .unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), control_rx.recv())
                .await
                .unwrap(),
            Some(batch)
        );
        drop(events);
        let _ = relay.await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_viewer_without_the_token_sees_nothing() {
        let dir = temp_dir("token");
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let (events, rx) = mpsc::channel(64);
        let relay = tokio::spawn(run(rx, listener, dir.clone()));
        let (want_tx, _want_rx) = mpsc::channel(4);
        events
            .send(CastEvent::Start {
                session_id: 1,
                device_id: None,
                want_keyframe: want_tx,
                control: mpsc::channel(4).0,
                webcam: false,
                windows_only: false,
            })
            .await
            .unwrap();
        events
            .send(CastEvent::Payload {
                session_id: 1,
                payload: CastPayload::Config {
                    width: 10,
                    height: 20,
                },
            })
            .await
            .unwrap();
        let file = cast_file(&dir).await;

        let mut view = TcpStream::connect((Ipv4Addr::LOCALHOST, file.port))
            .await
            .unwrap();
        view.write_all(&[0u8; TOKEN_LEN]).await.unwrap();
        let mut byte = [0u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(5), view.read(&mut byte))
            .await
            .expect("closed, not held");
        assert!(matches!(read, Ok(0) | Err(_)), "closed without a byte");

        drop(events);
        relay.await.unwrap();
        assert_eq!(wire::read(&dir), None, "the relay's end forgets the cast");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_cast_whose_session_ended_silently_is_forgotten() {
        let dir = temp_dir("silent");
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let (events, rx) = mpsc::channel(64);
        let relay = tokio::spawn(run(rx, listener, dir.clone()));
        let (want_tx, want_rx) = mpsc::channel(4);
        events
            .send(CastEvent::Start {
                session_id: 3,
                device_id: None,
                want_keyframe: want_tx,
                control: mpsc::channel(4).0,
                webcam: false,
                windows_only: false,
            })
            .await
            .unwrap();
        cast_file(&dir).await;
        // The session's half of the keyframe channel goes with it.
        drop(want_rx);
        let mut gone = false;
        for _ in 0..60 {
            if wire::read(&dir).is_none() {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(gone, "noticed within a heartbeat or two");
        drop(events);
        relay.await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    async fn window_file(dir: &Path, window: u8) -> CastFile {
        for _ in 0..100 {
            if let Some(file) = wire::read_window(dir, window) {
                return file;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no file for window {window}");
    }

    async fn join(file: &CastFile) -> TcpStream {
        let mut view = TcpStream::connect((Ipv4Addr::LOCALHOST, file.port))
            .await
            .unwrap();
        view.write_all(&file.token_bytes().unwrap()).await.unwrap();
        view
    }

    /// Phone apps tier 2: a phone that shows only app windows has no
    /// `cast.json`; each window has its own file and token, and its viewer
    /// sees that window's picture and reaches that window only.
    #[tokio::test]
    async fn each_app_window_has_its_own_file_viewer_and_input() {
        use pocketdesk_core::streaming::cast_control::{ControlBatch, ControlEvent};
        let dir = temp_dir("windows");
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let (events, rx) = mpsc::channel(64);
        let relay = tokio::spawn(run(rx, listener, dir.clone()));
        let (want_tx, mut want_rx) = mpsc::channel(8);
        let (control_tx, mut control_rx) = mpsc::channel(8);
        events
            .send(CastEvent::Start {
                session_id: 5,
                device_id: Some("phone".into()),
                want_keyframe: want_tx,
                control: control_tx,
                webcam: false,
                windows_only: true,
            })
            .await
            .unwrap();
        for (window, input) in [(1u8, true), (2, false)] {
            events
                .send(CastEvent::Payload {
                    session_id: 5,
                    payload: CastPayload::WindowConfig {
                        window,
                        width: 720,
                        height: 1280,
                        input,
                    },
                })
                .await
                .unwrap();
        }
        let one = window_file(&dir, 1).await;
        let two = window_file(&dir, 2).await;
        assert_eq!((one.window, two.window), (Some(1), Some(2)));
        assert_eq!(one.session_id, 5);
        assert_ne!(one.token, two.token, "a token for each");
        assert_eq!(wire::read(&dir), None, "no screen, no cast.json");

        let mut v1 = join(&one).await;
        let mut v2 = join(&two).await;
        assert_eq!(
            read_message(&mut v1).await,
            Message::Config {
                width: 720,
                height: 1280
            }
        );
        assert_eq!(
            read_message(&mut v1).await,
            Message::ControlState { on: true }
        );
        assert_eq!(
            read_message(&mut v2).await,
            Message::Config {
                width: 720,
                height: 1280
            }
        );
        assert_eq!(
            want_rx.recv().await,
            Some(()),
            "a viewer starts from a keyframe"
        );

        // A frame of window 2 goes to window 2's viewer only.
        events
            .send(CastEvent::Payload {
                session_id: 5,
                payload: CastPayload::WindowFrame {
                    window: 2,
                    keyframe: true,
                    seq: 0,
                    data: vec![0, 0, 0, 1, 0x65],
                },
            })
            .await
            .unwrap();
        assert_eq!(
            read_message(&mut v2).await,
            Message::Frame {
                keyframe: true,
                seq: 0,
                data: vec![0, 0, 0, 1, 0x65]
            }
        );

        // Window 1's viewer, which may take input, reaches window 1 only,
        // whatever window it names; window 2 does not take input.
        let batch =
            |events: Vec<ControlEvent>| ControlBatch { sent_ms: 1, events }.encode().unwrap();
        v1.write_all(
            &Message::Control(batch(vec![
                ControlEvent::Window(4),
                ControlEvent::Backspace,
            ]))
            .encode(),
        )
        .await
        .unwrap();
        let sent = tokio::time::timeout(Duration::from_secs(5), control_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let sent = ControlBatch::decode(&sent).unwrap();
        assert_eq!(sent.window(), Some(1), "its own window, whatever it named");
        assert_eq!(sent.events.len(), 2);
        v2.write_all(&Message::Control(batch(vec![ControlEvent::Backspace])).encode())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            control_rx.try_recv().is_err(),
            "window 2 does not take input"
        );

        // The owner closing window 2's window asks the phone to close that
        // window, input or not; the screen's viewer has no such message.
        v2.write_all(&Message::CloseWindow.encode()).await.unwrap();
        let closed = tokio::time::timeout(Duration::from_secs(5), control_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let closed = ControlBatch::decode(&closed).unwrap();
        assert!(closed.is_close());
        assert_eq!(closed.window(), Some(2));

        // Closing a window ends its viewer and forgets its file.
        events
            .send(CastEvent::Payload {
                session_id: 5,
                payload: CastPayload::WindowClosed { window: 1 },
            })
            .await
            .unwrap();
        assert_eq!(read_message(&mut v1).await, Message::End);
        for _ in 0..100 {
            if wire::read_window(&dir, 1).is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(wire::read_window(&dir, 1), None);
        assert!(wire::read_window(&dir, 2).is_some(), "window 2 stays");

        // The session's end ends the rest.
        events.send(CastEvent::End { session_id: 5 }).await.unwrap();
        assert_eq!(read_message(&mut v2).await, Message::End);
        for _ in 0..100 {
            if wire::windows(&dir).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(wire::windows(&dir).is_empty());
        drop(events);
        relay.await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_screen_viewer_names_no_window_and_a_window_viewer_only_its_own() {
        use pocketdesk_core::streaming::cast_control::{ControlBatch, ControlEvent};
        let plain = ControlBatch {
            sent_ms: 1,
            events: vec![ControlEvent::Enter],
        }
        .encode()
        .unwrap();
        let named = ControlBatch {
            sent_ms: 1,
            events: vec![ControlEvent::Window(2), ControlEvent::Enter],
        }
        .encode()
        .unwrap();
        assert_eq!(control_batch(&plain, None), Some(plain.clone()));
        assert_eq!(
            control_batch(&named, None),
            None,
            "the screen's viewer names no window"
        );
        assert_eq!(control_batch(&[1, 2], None), None);
        assert_eq!(control_batch(&named, Some(2)), Some(named.clone()));
        let made = ControlBatch::decode(&control_batch(&plain, Some(3)).unwrap()).unwrap();
        assert_eq!(made.window(), Some(3));
        let other = ControlBatch::decode(&control_batch(&named, Some(3)).unwrap()).unwrap();
        assert_eq!(other.window(), Some(3), "not the window it named");
        assert_eq!(control_batch(&plain, Some(0)), None);
    }
}
