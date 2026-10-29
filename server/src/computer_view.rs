//! Another of the owner's computers' screens in a window here (BK7). This
//! server dials the other computer's screen session as a phone would, on a
//! stream connection of its own (never the link's: the link reader stops any
//! stream that is not shared input's): on the home network first, to an
//! address whose certificate it pinned at pairing, then by the internet
//! road from its own WAN endpoint (a second endpoint with the same key could
//! not use a relay). The other computer's gate judges it as one of its
//! paired devices: it may ask its owner first, and without its Control
//! (Input) switch for this computer the session is view-only.
//!
//! The picture goes on to the desktop app as a phone's cast does
//! ([`crate::cast_relay`]): over loopback, named with a token in
//! `view.json` (`core::local::view`), the newest viewer winning. The
//! window's input comes back as input packets, checked before they go on.
//! Its sound, when the other computer sends it, plays here through the
//! shared player ([`crate::remote_sound`]).

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pocketdesk_core::local::cast::{TOKEN_LEN, token_hex, token_matches};
use pocketdesk_core::local::computers::{self, PeerComputer};
use pocketdesk_core::local::paths::VIEW_FILE;
use pocketdesk_core::local::view::{self as wire, Message, Phase, ViewFile, ViewStatus};
use pocketdesk_core::streaming::cast::CastChain;
use pocketdesk_core::streaming::transport::{CertManager, peer_label_for_address};
use pocketdesk_core::streaming::{
    AudioPacket, EncodedPacket, InputPacket, SessionMode, SharedConnection, StreamingClient,
};
use rand::RngCore;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

/// Where the other computer's sound goes instead of the speakers, for tests.
pub const SOUND_SEAM: &str = "POCKETDESK_VIEW_SOUND_FILE";

/// The viewer hears from the relay this often.
const HEARTBEAT: Duration = Duration::from_secs(2);
/// A viewer sends its token within this.
const TOKEN_WAIT: Duration = Duration::from_secs(2);
/// Messages waiting for a slow viewer; past this, frames are dropped and a
/// keyframe asked for.
const VIEWER_QUEUE: usize = 32;
/// Connections that have not sent the token yet, at most.
const PENDING_VIEWERS: usize = 4;
/// Input waiting to go to the other computer.
const INPUT_QUEUE: usize = 512;
/// A try on the home network, as the phone's: a computer that is not there
/// is given up on soon, for the internet road.
const HOME_TRY: Duration = Duration::from_secs(3);
/// A try by the internet road.
const INTERNET_TRY: Duration = Duration::from_secs(20);
/// One input packet's way to the other computer, at most.
const INPUT_SEND: Duration = Duration::from_secs(2);
/// A view nobody watches (the app never came, or went without a word) ends.
const UNWATCHED: Duration = Duration::from_secs(30);
/// An ended view is kept this long for a window that comes late to hear why.
const ENDED_KEPT: Duration = Duration::from_secs(60);
/// How often the session looks at what the other computer said.
const LOOK: Duration = Duration::from_millis(250);

/// What the Computers page asks of the view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Show the computer `device_id`'s screen `monitor`, replacing any view.
    Start { device_id: String, monitor: u32 },
    /// Stop showing it: the view `session_id` when given (one since
    /// replaced is left alone, BK7 review M1), else the one shown now.
    Stop { session_id: Option<u64> },
}

/// Whether a Stop for `asked` ends the view `current`.
pub fn stops(asked: Option<u64>, current: Option<u64>) -> bool {
    asked.is_none_or(|asked| current == Some(asked))
}

/// Start the view's relay; the requests the app leaves go to it.
pub fn spawn(
    state_dir: PathBuf,
    certs: Arc<CertManager>,
    wan: Option<iroh::Endpoint>,
) -> mpsc::Sender<Command> {
    let (tx, rx) = mpsc::channel(8);
    tokio::spawn(async move {
        match TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await {
            Ok(listener) => {
                let dial = Dialer::Real { certs, wan };
                run(rx, listener, state_dir, dial).await
            }
            Err(e) => tracing::warn!(error = %e, "The view relay could not listen"),
        }
    });
    tx
}

/// One way to the other computer's screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// On the home network, at this address and its streaming port.
    Home(SocketAddr),
    /// By the internet road.
    Internet,
}

impl Route {
    fn words(&self) -> &'static str {
        match self {
            Route::Home(_) => "home network",
            Route::Internet => "internet",
        }
    }
}

/// The ways to try, in order: the home network first, at each address of
/// the computer that is an IP address pinned to its certificate (a host name
/// or an address no longer pinned to it is not tried: a first-use pin there
/// would trust whoever answers), then the internet road when this computer
/// has one and the other's endpoint is known.
pub fn plan(
    computer: &PeerComputer,
    pins: &HashMap<String, [u8; 32]>,
    internet: bool,
) -> Vec<Route> {
    let pinned = crate::peer_computers::parse_fingerprint(&computer.fingerprint);
    let mut routes: Vec<Route> = computer
        .addresses
        .iter()
        .filter_map(|address| {
            let ip: IpAddr = address.parse().ok()?;
            let label = peer_label_for_address(address)?;
            (pinned.is_some() && pins.get(&label) == pinned.as_ref())
                .then(|| Route::Home(SocketAddr::new(ip, computer.stream_port)))
        })
        .collect();
    routes.dedup();
    if internet
        && computer
            .endpoint
            .as_ref()
            .and_then(crate::peer_computers::WanTarget::parse)
            .is_some()
    {
        routes.push(Route::Internet);
    }
    routes
}

/// A session reached: its client (not yet started), picture, sound and road.
pub struct Dialed {
    pub client: StreamingClient,
    pub frames: mpsc::Receiver<EncodedPacket>,
    pub audio: mpsc::Receiver<AudioPacket>,
    pub route: &'static str,
}

/// How a view reaches the other computer.
pub enum Dialer {
    /// This server's certificate and WAN endpoint, as the other knows them.
    Real {
        certs: Arc<CertManager>,
        wan: Option<iroh::Endpoint>,
    },
    /// Tests: straight to a streaming server at this address, as `id`.
    #[doc(hidden)]
    Direct {
        addr: SocketAddr,
        certs: Arc<CertManager>,
        id: String,
    },
}

impl Dialer {
    async fn dial(&self, computer: &PeerComputer, monitor: u32) -> Result<Dialed, String> {
        match self {
            Dialer::Real { certs, wan } => dial(computer, certs, wan.as_ref(), monitor).await,
            Dialer::Direct { addr, certs, id } => {
                let (client, frames, audio) = StreamingClient::connect(
                    &addr.to_string(),
                    certs.clone(),
                    Some(monitor),
                    Some(id),
                    None,
                    None,
                    SessionMode::Screen,
                )
                .await
                .map_err(|e| format!("{e:#}"))?;
                Ok(Dialed {
                    client,
                    frames,
                    audio,
                    route: "home network",
                })
            }
        }
    }
}

/// Dial the computer's screen session by [`plan`]'s ways, the first that
/// answers; why none did.
async fn dial(
    computer: &PeerComputer,
    certs: &Arc<CertManager>,
    wan: Option<&iroh::Endpoint>,
    monitor: u32,
) -> Result<Dialed, String> {
    let me = crate::kde::host_identity();
    let iroh_id = wan.map(|e| e.id().to_string());
    let pins: HashMap<String, [u8; 32]> = certs.known_peers().into_iter().collect();
    let routes = plan(computer, &pins, wan.is_some());
    if routes.is_empty() {
        return Err(
            "no way to it: none of its addresses is known here, and the internet road is off or it has no endpoint"
                .to_string(),
        );
    }
    let mut why = Vec::new();
    for route in routes {
        match route {
            Route::Home(addr) => {
                let tried = tokio::time::timeout(
                    HOME_TRY,
                    StreamingClient::connect(
                        &addr.to_string(),
                        certs.clone(),
                        Some(monitor),
                        Some(&me.device_id),
                        iroh_id.as_deref(),
                        // H.264 only: the window's decoder reads no HEVC.
                        None,
                        SessionMode::Screen,
                    ),
                )
                .await;
                match tried {
                    Ok(Ok((client, frames, audio))) => {
                        return Ok(Dialed {
                            client,
                            frames,
                            audio,
                            route: route.words(),
                        });
                    }
                    Ok(Err(e)) => why.push(format!("{addr}: {e:#}")),
                    Err(_) => why.push(format!("{addr}: no answer")),
                }
            }
            Route::Internet => {
                let (Some(wan), Some(target)) = (
                    wan,
                    computer
                        .endpoint
                        .as_ref()
                        .and_then(crate::peer_computers::WanTarget::parse),
                ) else {
                    continue;
                };
                let dialed = pocketdesk_core::streaming::iroh_connection::dial_stream_from(
                    wan,
                    &target.endpoint_id,
                    &target.relay_urls,
                    &target.direct_addrs,
                    INTERNET_TRY,
                )
                .await;
                match dialed {
                    Ok(conn) => {
                        let (client, frames, audio) = StreamingClient::attach(
                            conn,
                            Some(monitor),
                            Some(&me.device_id),
                            iroh_id.as_deref(),
                            None,
                            SessionMode::Screen,
                        )
                        .await;
                        return Ok(Dialed {
                            client,
                            frames,
                            audio,
                            route: route.words(),
                        });
                    }
                    Err(e) => why.push(format!("internet: {e:#}")),
                }
            }
        }
    }
    Err(why.join("; "))
}

/// What a session tells the relay.
#[derive(Debug)]
enum Event {
    Status(ViewStatus),
    Config(u16, u16),
    Frame {
        keyframe: bool,
        seq: u64,
        data: Vec<u8>,
    },
    Ended(String),
}

/// Ends the other computer's session however the view's task ends (a Stop
/// aborts it).
struct Hangup {
    cancel: tokio_util::sync::CancellationToken,
    conn: Option<SharedConnection>,
}

impl Drop for Hangup {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(conn) = &self.conn {
            conn.close(0, b"view closed");
        }
    }
}

/// Whether a frame goes on: a keyframe always, a delta only after the
/// newest one sent (frames ride streams of their own and can arrive out of
/// order; a late delta fed to the decoder draws garbage).
pub fn in_order(keyframe: bool, seq: u64, last: Option<u64>) -> bool {
    keyframe || last.is_none_or(|last| seq > last)
}

/// How often a release is tried on a link that does not take it.
const RELEASE_TRIES: usize = 3;

/// Whether `packet` lets go of a key or a button.
pub fn is_release(packet: &InputPacket) -> bool {
    matches!(
        packet,
        InputPacket::KeyEvent { pressed: false, .. }
            | InputPacket::MouseClick { pressed: false, .. }
    )
}

/// Whether a frame of `len` bytes fits in one relay message, its header
/// and all.
pub fn fits(len: usize) -> bool {
    len.saturating_add(16) <= pocketdesk_core::local::cast::MAX_MESSAGE
}

/// The words for a session that ended: before the other computer let this
/// one in (it refused, or its owner said no), or after.
fn ended_words(name: &str, admitted: bool, reason: Option<&str>) -> String {
    if admitted {
        return format!("Lost the connection to {name}.");
    }
    let reason = reason.filter(|r| !r.is_empty()).unwrap_or("it closed");
    format!(
        "{name} did not let this computer in ({reason}). On {name}, this computer must still be paired (its Devices page), and the session allowed if it asks first."
    )
}

/// One view's session with the other computer, until it ends.
async fn session(
    session_id: u64,
    computer: PeerComputer,
    monitor: u32,
    dialer: Arc<Dialer>,
    events: mpsc::Sender<(u64, Event)>,
    mut input: mpsc::Receiver<InputPacket>,
) {
    let say = |event| {
        let events = events.clone();
        async move {
            let _ = events.send((session_id, event)).await;
        }
    };
    let name = computer.name.clone();
    let mut status = ViewStatus {
        monitor,
        ..ViewStatus::default()
    };
    crate::peer_computers::ask_screens(&computer.device_id);
    say(Event::Status(status.clone())).await;
    let dialed = match dialer.dial(&computer, monitor).await {
        Ok(dialed) => dialed,
        Err(why) => {
            // Its addresses stay out of the log at info (BK7 review M8):
            // the window says them to the owner.
            tracing::info!(device_id = %computer.device_id, "Another computer's screen not reached");
            tracing::debug!(device_id = %computer.device_id, "Another computer's screen not reached: {why}");
            say(Event::Ended(format!("Could not reach {name}: {why}"))).await;
            return;
        }
    };
    let Dialed {
        mut client,
        mut frames,
        audio,
        route,
    } = dialed;
    tracing::info!(device_id = %computer.device_id, route, "Showing another computer's screen");
    let conn = client.connection().cloned();
    let remote = client.remote();
    let _hangup = Hangup {
        cancel: client.cancel_token(),
        conn: conn.clone(),
    };
    tokio::spawn(async move { client.start().await });
    tokio::spawn(play_sound(audio));
    // Input in order, one stream each, as a phone sends it.
    if let Some(conn) = conn.clone() {
        tokio::spawn(async move {
            while let Some(packet) = input.recv().await {
                // A release is tried again on a stalled link (BK7 review M7):
                // left undone, the key stays held there.
                let tries = if is_release(&packet) {
                    RELEASE_TRIES
                } else {
                    1
                };
                for _ in 0..tries {
                    let sent = tokio::time::timeout(INPUT_SEND, send_input(&conn, &packet)).await;
                    if matches!(sent, Ok(Ok(()))) {
                        break;
                    }
                    tracing::debug!("An input packet did not reach the other computer");
                }
            }
        });
    }
    status.phase = Phase::Waiting;
    status.route = route.to_string();
    say(Event::Status(status.clone())).await;
    let mut size: Option<(u16, u16)> = None;
    let mut last_seq: Option<u64> = None;
    let mut admitted = false;
    let mut look = tokio::time::interval(LOOK);
    let mut said_too_large = false;
    let mut too_large_ask: Option<Instant> = None;
    loop {
        tokio::select! {
            packet = frames.recv() => {
                let Some(packet) = packet else { break };
                admitted = true;
                if !in_order(packet.is_keyframe, packet.sequence, last_seq) {
                    continue;
                }
                last_seq = Some(packet.sequence);
                // A new size (another screen, its resolution changed) comes
                // with the keyframe that starts it.
                if packet.is_keyframe
                    && let Some(read) = pocketdesk_core::streaming::h264_sps::picture_size(&packet.data)
                    && size != Some(read)
                {
                    size = Some(read);
                    say(Event::Config(read.0, read.1)).await;
                }
                if size.is_none() {
                    continue;
                }
                // A frame the relay cannot carry (BK7 review M6): said once,
                // and the chain it breaks starts again from a keyframe.
                if !fits(packet.data.len()) {
                    // The window says so (BK7 review M6).
                    if status.note.is_none() {
                        status.note = Some(format!(
                            "{name}'s screen comes in frames too large to show here. A lower Quality, or a smaller screen there, may help."
                        ));
                        say(Event::Status(status.clone())).await;
                    }
                    if !said_too_large {
                        said_too_large = true;
                        tracing::warn!(
                            device_id = %computer.device_id,
                            bytes = packet.data.len(),
                            "A frame of another computer's screen too large for the window; dropped"
                        );
                    }
                    if too_large_ask.is_none_or(|at: Instant| at.elapsed() >= Duration::from_secs(1))
                        && let Some(conn) = conn.clone()
                    {
                        too_large_ask = Some(Instant::now());
                        tokio::spawn(async move {
                            let _ = tokio::time::timeout(
                                INPUT_SEND,
                                send_input(&conn, &InputPacket::RequestKeyframe),
                            )
                            .await;
                        });
                    }
                    continue;
                }
                if status.note.take().is_some() {
                    say(Event::Status(status.clone())).await;
                }
                say(Event::Frame {
                    keyframe: packet.is_keyframe,
                    seq: packet.sequence,
                    data: packet.data,
                }).await;
            }
            _ = look.tick() => {}
        }
        // What the other computer said: let in, with or without control,
        // and its screens.
        let features = remote
            .screen_features
            .load(std::sync::atomic::Ordering::Acquire);
        let mut next = status.clone();
        if let Some(control) = pocketdesk_core::streaming::screen_takes_input(features) {
            admitted = true;
            next.control = Some(control);
        }
        if admitted {
            next.phase = Phase::Showing;
        }
        next.monitors = crate::peer_computers::screens(&computer.device_id);
        if next != status {
            status = next;
            say(Event::Status(status.clone())).await;
        }
    }
    let reason = conn.as_ref().and_then(|c| c.close_reason());
    tracing::info!(device_id = %computer.device_id, "Another computer's screen ended");
    say(Event::Ended(ended_words(
        &name,
        admitted,
        reason.as_deref(),
    )))
    .await;
}

/// One input packet on its own stream.
async fn send_input(conn: &SharedConnection, packet: &InputPacket) -> Result<(), String> {
    let mut stream = conn.open_uni().await.map_err(|e| e.to_string())?;
    stream
        .write_all(&packet.encode())
        .await
        .map_err(|e| e.to_string())?;
    stream.finish().map_err(|e| e.to_string())
}

/// The other computer's sound, played here until the session ends (none
/// comes unless its Audio permission for this computer is on).
async fn play_sound(mut rx: mpsc::Receiver<AudioPacket>) {
    let Some(first) = rx.recv().await else {
        return;
    };
    let seam = std::env::var_os(SOUND_SEAM).map(PathBuf::from);
    let _ = tokio::task::spawn_blocking(move || {
        let mut player = match crate::remote_sound::Player::new("another computer's sound", seam) {
            Ok(player) => player,
            Err(e) => {
                tracing::warn!("Another computer's sound cannot be decoded here: {e:#}");
                return;
            }
        };
        player.play(&first.data);
        while let Some(packet) = rx.blocking_recv() {
            player.play(&packet.data);
        }
    })
    .await;
}

struct View {
    session_id: u64,
    device_id: String,
    token: [u8; TOKEN_LEN],
    input: mpsc::Sender<InputPacket>,
    task: Option<tokio::task::JoinHandle<()>>,
    config: Option<(u16, u16)>,
    status: ViewStatus,
    /// Why it ended, and when.
    ended: Option<(String, Instant)>,
    /// Since when nobody watches it.
    unwatched_since: Option<Instant>,
    asks: CastChain,
}

impl Drop for View {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

struct Viewer {
    session_id: u64,
    tx: mpsc::Sender<Message>,
    reader: tokio::task::JoinHandle<()>,
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// The relay's loop, over `listener` (bound to loopback).
pub async fn run(
    mut commands: mpsc::Receiver<Command>,
    listener: TcpListener,
    state_dir: PathBuf,
    dialer: Dialer,
) {
    let port = match listener.local_addr() {
        Ok(addr) => addr.port(),
        Err(e) => {
            tracing::warn!(error = %e, "The view relay has no port");
            return;
        }
    };
    let dialer = Arc::new(dialer);
    // A file left by a server that stopped mid-view names nothing now.
    let _ = std::fs::remove_file(state_dir.join(VIEW_FILE));
    let mut next_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_millis() as u64);
    let mut view: Option<View> = None;
    let mut viewer: Option<Viewer> = None;
    let (events_tx, mut events) = mpsc::channel::<(u64, Event)>(64);
    let (joined_tx, mut joined_rx) = mpsc::channel::<(u64, TcpStream)>(4);
    let pending = Arc::new(tokio::sync::Semaphore::new(PENDING_VIEWERS));
    let mut heartbeat = tokio::time::interval(HEARTBEAT);
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                let Command::Start { device_id, monitor } = command else {
                    if let Command::Stop { session_id } = command
                        && stops(session_id, view.as_ref().map(|v| v.session_id))
                    {
                        end(&mut view, &mut viewer, &state_dir, None);
                    }
                    continue;
                };
                end(&mut view, &mut viewer, &state_dir, None);
                next_id += 1;
                let session_id = next_id;
                let computer = computers::read(&state_dir)
                    .computers
                    .into_iter()
                    .find(|c| c.device_id == device_id);
                let mut token = [0u8; TOKEN_LEN];
                rand::rngs::OsRng.fill_bytes(&mut token);
                let name = computer.as_ref().map_or_else(|| "That computer".to_string(), |c| c.name.clone());
                let file = ViewFile {
                    session_id,
                    device_id: device_id.clone(),
                    name: name.clone(),
                    port,
                    token: token_hex(&token),
                };
                if let Err(e) = wire::write(&state_dir, &file) {
                    tracing::warn!(error = %e, "Could not say where the view is");
                }
                let (input_tx, input_rx) = mpsc::channel(INPUT_QUEUE);
                let (task, ended) = match computer {
                    Some(computer) => (
                        Some(tokio::spawn(session(
                            session_id,
                            computer,
                            monitor,
                            dialer.clone(),
                            events_tx.clone(),
                            input_rx,
                        ))),
                        None,
                    ),
                    None => (
                        None,
                        Some(("That computer is no longer paired with this one.".to_string(), Instant::now())),
                    ),
                };
                view = Some(View {
                    session_id,
                    device_id,
                    token,
                    input: input_tx,
                    task,
                    config: None,
                    status: ViewStatus { monitor, ..ViewStatus::default() },
                    ended,
                    unwatched_since: Some(Instant::now()),
                    asks: CastChain::default(),
                });
            }
            Some((session_id, event)) = events.recv() => {
                let Some(current) = view.as_mut().filter(|v| v.session_id == session_id) else {
                    continue;
                };
                let watching = viewer.as_ref().filter(|v| v.session_id == session_id);
                match event {
                    Event::Status(status) => {
                        current.status = status.clone();
                        if let Some(v) = watching {
                            let _ = v.tx.try_send(Message::Status(status));
                        }
                    }
                    Event::Config(width, height) => {
                        current.config = Some((width, height));
                        // A viewer that missed a new size would decode at the
                        // old one: it is dropped, and rejoins with the size.
                        if let Some(v) = watching
                            && v.tx.try_send(Message::Config { width, height }).is_err()
                        {
                            viewer = None;
                        }
                    }
                    Event::Frame { keyframe, seq, data } => {
                        if let Some(v) = watching
                            && v.tx.try_send(Message::Frame { keyframe, seq, data }).is_err()
                            && current.asks.lost(Instant::now())
                        {
                            let _ = current.input.try_send(InputPacket::RequestKeyframe);
                        }
                    }
                    Event::Ended(why) => {
                        current.task = None;
                        if let Some(v) = watching {
                            let _ = v.tx.try_send(Message::End { why: why.clone() });
                        }
                        current.ended = Some((why, Instant::now()));
                        // Said to the window watching, the view is over; one
                        // that has not come yet finds it still named, and
                        // hears why as it joins (or it goes in a minute).
                        if watching.is_some() {
                            viewer = None;
                            end(&mut view, &mut None, &state_dir, None);
                        }
                    }
                }
            }
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                if let Some(v) = &view
                    && let Ok(slot) = pending.clone().try_acquire_owned()
                {
                    let (token, session_id, joined) = (v.token, v.session_id, joined_tx.clone());
                    tokio::spawn(async move {
                        authenticate(stream, token, session_id, joined).await;
                        drop(slot);
                    });
                }
            }
            Some((session_id, stream)) = joined_rx.recv() => {
                let Some(v) = view.as_mut().filter(|v| v.session_id == session_id) else {
                    continue;
                };
                let joined = start_viewer(stream, session_id, v.input.clone());
                let _ = joined.tx.try_send(Message::Status(v.status.clone()));
                if let Some((width, height)) = v.config {
                    let _ = joined.tx.try_send(Message::Config { width, height });
                }
                if let Some((why, _)) = &v.ended {
                    let _ = joined.tx.try_send(Message::End { why: why.clone() });
                    end(&mut view, &mut None, &state_dir, None);
                    viewer = None;
                    continue;
                }
                // A new viewer starts from a keyframe.
                let _ = v.input.try_send(InputPacket::RequestKeyframe);
                v.unwatched_since = None;
                tracing::info!(device_id = %v.device_id, "Another computer's screen is being shown");
                viewer = Some(joined);
            }
            _ = heartbeat.tick() => {
                if viewer.as_ref().is_some_and(|v| v.tx.try_send(Message::Heartbeat).is_err() && v.tx.is_closed()) {
                    viewer = None;
                }
                let now = Instant::now();
                if let Some(v) = view.as_mut() {
                    let watched = viewer.as_ref().is_some_and(|w| w.session_id == v.session_id);
                    if watched {
                        v.unwatched_since = None;
                    } else if v.unwatched_since.is_none() {
                        v.unwatched_since = Some(now);
                    }
                    let unwatched = v.unwatched_since.is_some_and(|t| now.duration_since(t) >= UNWATCHED);
                    let stale = v.ended.as_ref().is_some_and(|(_, at)| now.duration_since(*at) >= ENDED_KEPT);
                    if stale || (unwatched && v.ended.is_none()) {
                        if unwatched && v.ended.is_none() {
                            tracing::info!(device_id = %v.device_id, "Another computer's screen ended: no window shows it");
                        }
                        end(&mut view, &mut viewer, &state_dir, None);
                    }
                }
            }
        }
    }
    end(&mut view, &mut viewer, &state_dir, None);
}

/// The view is over: its window is told, and `view.json` forgets it.
fn end(view: &mut Option<View>, viewer: &mut Option<Viewer>, state_dir: &Path, why: Option<&str>) {
    if let Some(v) = viewer.take() {
        let _ = v.tx.try_send(Message::End {
            why: why.unwrap_or("").to_string(),
        });
    }
    if let Some(v) = view.take()
        && let Err(e) = wire::remove(state_dir, v.session_id)
    {
        tracing::warn!(error = %e, "Could not forget the view");
    }
}

/// A connection becomes the viewer once it sends the view's token.
async fn authenticate(
    mut stream: TcpStream,
    token: [u8; TOKEN_LEN],
    session_id: u64,
    joined: mpsc::Sender<(u64, TcpStream)>,
) {
    let mut given = [0u8; TOKEN_LEN];
    match tokio::time::timeout(TOKEN_WAIT, stream.read_exact(&mut given)).await {
        Ok(Ok(_)) if token_matches(&token, &given) => {
            let _ = joined.send((session_id, stream)).await;
        }
        _ => tracing::warn!("Refused a view's window without the token"),
    }
}

/// The viewer's two halves: messages go out in order; its keyframe asks and
/// the input it may send reach the other computer.
fn start_viewer(stream: TcpStream, session_id: u64, input: mpsc::Sender<InputPacket>) -> Viewer {
    let _ = stream.set_nodelay(true);
    let (mut read, mut write) = stream.into_split();
    let (tx, mut rx) = mpsc::channel::<Message>(VIEWER_QUEUE);
    tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            let end = matches!(message, Message::End { .. });
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
                    let _ = input.try_send(InputPacket::RequestKeyframe);
                }
                wire::INPUT if len > 0 && len <= wire::MAX_INPUT => {
                    let mut packet = vec![0u8; len];
                    if read.read_exact(&mut packet).await.is_err() {
                        break;
                    }
                    // Only what the window may send; the rest is dropped.
                    match InputPacket::decode(&packet) {
                        Ok(packet) if wire::input_allowed(&packet) => {
                            // A release waits for room (BK7 review M7): a
                            // key or button let go of here must be let go of
                            // there. The rest may be dropped.
                            if is_release(&packet) {
                                if input.send(packet).await.is_err() {
                                    break;
                                }
                            } else {
                                let _ = input.try_send(packet);
                            }
                        }
                        _ => tracing::debug!("Dropped input the view's window may not send"),
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

    fn computer(addresses: &[&str], endpoint: Option<serde_json::Value>) -> PeerComputer {
        PeerComputer {
            device_id: "desk".into(),
            name: "Desk".into(),
            addresses: addresses.iter().map(|a| a.to_string()).collect(),
            control_port: 1716,
            stream_port: 4800,
            fingerprint: "ab".repeat(32),
            endpoint,
            paired_unix: 0,
            link: String::new(),
            link_unix: 0,
            lets_drive: false,
        }
    }

    fn pins(pairs: &[(&str, u8)]) -> HashMap<String, [u8; 32]> {
        pairs
            .iter()
            .map(|(address, byte)| (peer_label_for_address(address).unwrap(), [*byte; 32]))
            .collect()
    }

    fn endpoint() -> serde_json::Value {
        let key = iroh::SecretKey::generate();
        serde_json::json!({"endpointId": key.public().to_string(), "relayUrls": [], "directAddrs": []})
    }

    /// The home network first, at each pinned address and the computer's
    /// streaming port, then the internet road.
    #[test]
    fn the_home_network_is_tried_first_then_the_internet() {
        let desk = computer(&["192.0.2.10", "192.0.2.11"], Some(endpoint()));
        let held = pins(&[("192.0.2.10", 0xab), ("192.0.2.11", 0xab)]);
        assert_eq!(
            plan(&desk, &held, true),
            [
                Route::Home("192.0.2.10:4800".parse().unwrap()),
                Route::Home("192.0.2.11:4800".parse().unwrap()),
                Route::Internet,
            ]
        );
        assert_eq!(
            plan(&desk, &held, false),
            [
                Route::Home("192.0.2.10:4800".parse().unwrap()),
                Route::Home("192.0.2.11:4800".parse().unwrap()),
            ],
            "no internet road here"
        );
    }

    /// An address not pinned to this computer's certificate (another's now,
    /// or none) or a host name is never dialled: a first-use pin would trust
    /// whoever answers. No endpoint, no internet road.
    #[test]
    fn only_addresses_pinned_to_the_computer_are_dialled() {
        let desk = computer(
            &["192.0.2.10", "192.0.2.12", "desk.example.com", "192.0.2.13"],
            None,
        );
        let held = pins(&[
            ("192.0.2.10", 0xcd),
            ("192.0.2.12", 0xab),
            ("desk.example.com", 0xab),
        ]);
        assert_eq!(
            plan(&desk, &held, true),
            [Route::Home("192.0.2.12:4800".parse().unwrap())]
        );
        let unreadable = PeerComputer {
            fingerprint: "zz".into(),
            ..desk
        };
        assert!(plan(&unreadable, &held, true).is_empty());
    }

    /// BK7 review M1: a window's Stop names its view; one for a view since
    /// replaced (the window opened again at once) stops nothing.
    #[test]
    fn a_stop_ends_only_the_view_it_names() {
        assert!(stops(Some(5), Some(5)));
        assert!(!stops(Some(5), Some(6)), "replaced since");
        assert!(stops(None, Some(6)), "the window never knew its view");
        assert!(!stops(Some(5), None));
    }

    /// BK7 review M7: with the queue to the other computer full, a press
    /// or a move may be dropped, a release never (a key left held there).
    #[tokio::test]
    async fn a_release_waits_for_room_where_a_press_is_dropped() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let mut window = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (relay_end, _) = listener.accept().await.unwrap();
        let (input, mut queued) = mpsc::channel(1);
        let _viewer = start_viewer(relay_end, 1, input);
        let press = |key| InputPacket::KeyEvent { key, pressed: true };
        for packet in [
            press(30),
            press(31),
            InputPacket::KeyEvent {
                key: 30,
                pressed: false,
            },
        ] {
            window
                .write_all(&Message::Input(packet.encode()).encode())
                .await
                .unwrap();
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut got = Vec::new();
        while let Ok(Some(packet)) =
            tokio::time::timeout(Duration::from_millis(500), queued.recv()).await
        {
            got.push(format!("{packet:?}"));
        }
        assert_eq!(
            got,
            [
                "KeyEvent { key: 30, pressed: true }",
                "KeyEvent { key: 30, pressed: false }"
            ],
            "the second press found no room; the release waited for it"
        );
        assert!(is_release(&InputPacket::MouseClick {
            button: 0,
            pressed: false
        }));
        assert!(!is_release(&press(1)));
    }

    #[test]
    fn a_frame_fits_the_relay_or_is_too_large() {
        let max = pocketdesk_core::local::cast::MAX_MESSAGE;
        assert!(fits(0));
        assert!(fits(max - 16));
        assert!(!fits(max - 15));
        assert!(!fits(usize::MAX));
    }

    #[test]
    fn a_late_delta_is_dropped_and_a_keyframe_always_goes() {
        assert!(in_order(false, 0, None));
        assert!(in_order(false, 5, Some(4)));
        assert!(!in_order(false, 4, Some(5)), "came after a newer one");
        assert!(!in_order(false, 5, Some(5)));
        assert!(
            in_order(true, 0, Some(9)),
            "a rebuilt encoder counts from 0"
        );
    }

    #[test]
    fn an_end_says_whether_the_computer_let_this_one_in() {
        assert_eq!(
            ended_words("Desk", true, Some("closed")),
            "Lost the connection to Desk."
        );
        let refused = ended_words("Desk", false, Some("session refused"));
        assert!(refused.starts_with("Desk did not let this computer in (session refused)"));
        assert!(ended_words("Desk", false, None).contains("(it closed)"));
    }
}
