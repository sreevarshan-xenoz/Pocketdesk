//! The phone's screen on the computer (T10k): the server's half of handing a
//! cast to the desktop app. One relay serves the process. It listens on
//! loopback; while a cast is admitted it names the port and a fresh token in
//! `cast.json` (owner-only, `core::local::cast`), and the one viewer that
//! sends the token gets the cast's config and frames. The newest cast
//! replaces an older one, and the newest viewer an older viewer.

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use pocketdesk_core::local::cast::{self as wire, CastFile, Message, TOKEN_LEN};
use pocketdesk_core::local::paths::CAST_FILE;
use pocketdesk_core::streaming::cast::{CastChain, CastEvent, CastPayload};
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

struct Cast {
    session_id: u64,
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
    let (joined_tx, mut joined_rx) = mpsc::channel::<(u64, TcpStream)>(4);
    let pending = std::sync::Arc::new(tokio::sync::Semaphore::new(PENDING_VIEWERS));
    let mut heartbeat = tokio::time::interval(HEARTBEAT);
    loop {
        tokio::select! {
            event = events.recv() => {
                let Some(event) = event else { break };
                match event {
                    CastEvent::Start { session_id, device_id, want_keyframe, control } => {
                        end(&mut cast, &mut viewer, &state_dir);
                        let mut token = [0u8; TOKEN_LEN];
                        rand::rngs::OsRng.fill_bytes(&mut token);
                        let file = CastFile {
                            session_id,
                            device_id: device_id.unwrap_or_default(),
                            port,
                            token: wire::token_hex(&token),
                        };
                        if let Err(e) = wire::write(&state_dir, &file) {
                            tracing::warn!(error = %e, "Could not say where the cast is");
                        }
                        tracing::info!(session_id, "A phone's screen can be shown");
                        cast = Some(Cast {
                            session_id,
                            want: want_keyframe,
                            control,
                            control_on: Default::default(),
                            token,
                            config: None,
                            asks: CastChain::default(),
                        });
                    }
                    CastEvent::Payload { session_id, payload } => {
                        let Some(current) = cast.as_mut().filter(|c| c.session_id == session_id) else {
                            continue;
                        };
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
                        if cast.as_ref().is_some_and(|c| c.session_id == session_id) {
                            end(&mut cast, &mut viewer, &state_dir);
                        }
                    }
                }
            }
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                // Nothing to show, or too many waiting to say the token:
                // the connection is simply closed.
                if let Some(c) = &cast
                    && let Ok(slot) = pending.clone().try_acquire_owned()
                {
                    let (token, session_id, joined) = (c.token, c.session_id, joined_tx.clone());
                    tokio::spawn(async move {
                        authenticate(stream, token, session_id, joined).await;
                        drop(slot);
                    });
                }
            }
            Some((session_id, stream)) = joined_rx.recv() => {
                let Some(c) = cast.as_ref().filter(|c| c.session_id == session_id) else {
                    continue;
                };
                let v = start_viewer(
                    stream,
                    session_id,
                    c.want.clone(),
                    c.control.clone(),
                    c.control_on.clone(),
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
                if cast.as_ref().is_some_and(|c| c.want.is_closed()) {
                    end(&mut cast, &mut viewer, &state_dir);
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
    end(&mut cast, &mut viewer, &state_dir);
}

/// The cast is over: the viewer is told, and `cast.json` forgets it.
fn end(cast: &mut Option<Cast>, viewer: &mut Option<Viewer>, state_dir: &Path) {
    if let Some(v) = viewer.take() {
        let _ = v.tx.try_send(Message::End);
    }
    if let Some(c) = cast.take() {
        if let Err(e) = wire::remove(state_dir, c.session_id) {
            tracing::warn!(error = %e, "Could not forget the cast");
        }
        tracing::info!(
            session_id = c.session_id,
            "The phone's screen is no longer shown"
        );
    }
}

/// A connection becomes the viewer once it sends the cast's token.
async fn authenticate(
    mut stream: TcpStream,
    token: [u8; TOKEN_LEN],
    session_id: u64,
    joined: mpsc::Sender<(u64, TcpStream)>,
) {
    let mut given = [0u8; TOKEN_LEN];
    match tokio::time::timeout(TOKEN_WAIT, stream.read_exact(&mut given)).await {
        Ok(Ok(_)) if wire::token_matches(&token, &given) => {
            let _ = joined.send((session_id, stream)).await;
        }
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
                    // next time).
                    if control_on.load(std::sync::atomic::Ordering::Acquire)
                        && pocketdesk_core::streaming::cast_control::ControlBatch::decode(&batch)
                            .is_some()
                    {
                        let _ = control.try_send(batch);
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
}
