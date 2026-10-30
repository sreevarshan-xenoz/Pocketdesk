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
//!
//! The clipboard follows the window (PC3): as the window gains the focus
//! while it controls the other computer, this computer's clipboard goes
//! there; as it loses it, that computer's comes back
//! ([`crate::clipboard_push::push_for_window`] and `ask_back`), with
//! `computer_view_clipboard` on.
//!
//! Privacy follows the window too (PC8): its bar's "Hide what I do there"
//! asks the other computer for its own privacy mode (its keyboard and
//! mouse blocked, its ring shown), with the packet a phone asks it with
//! and judged there by the same Power switch. It is renewed while the
//! window shows that computer and controls it, and let go as the window
//! closes, the view ends or drops, or control is taken away; that computer
//! ends it too as the connection it was asked on closes. With "Lock it when
//! I close the window" the window's last word asks that computer's lock.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pocketdesk_core::local::cast::{TOKEN_LEN, token_hex, token_matches};
use pocketdesk_core::local::computers::{self, PeerComputer};
use pocketdesk_core::local::paths::VIEW_FILE;
use pocketdesk_core::local::view::{
    self as wire, Message, Phase, ViewFile, ViewPrivacy, ViewStatus,
};
use pocketdesk_core::protocol::kdeconnect::NetworkPacket;
use pocketdesk_core::streaming::cast::CastChain;
use pocketdesk_core::streaming::transport::{
    CertManager, StreamClient, StreamTransportConfig, peer_label_for_address,
};
use pocketdesk_core::streaming::{
    AudioPacket, EncodedPacket, InputPacket, QuinnConnection, SessionMode, SharedConnection,
    StreamingClient,
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
/// The internet road, up first, waits this long for a home address still
/// trying (PC2): the home network is preferred when both come up about
/// together.
pub const PREFER_HOME: Duration = Duration::from_millis(300);
/// One input packet's way to the other computer, at most.
const INPUT_SEND: Duration = Duration::from_secs(2);
/// A view nobody watches (the app never came, or went without a word) ends.
const UNWATCHED: Duration = Duration::from_secs(30);
/// An ended view is kept this long for a window that comes late to hear why.
const ENDED_KEPT: Duration = Duration::from_secs(60);
/// How often the session looks at what the other computer said.
const LOOK: Duration = Duration::from_millis(250);
/// Why a view of a computer not paired here ends.
const NOT_PAIRED: &str = "That computer is no longer paired with this one.";
/// How often the window hears how the picture comes (PC2).
const STATS_EVERY: Duration = Duration::from_secs(1);
/// The codecs a view offers the other computer (PC5): HEVC beside H.264
/// when the app's ask said its ffmpeg decodes it (`asked`, PR #562 review
/// I3: the app's sidecar decodes, not this process) and
/// `computer_view_hevc` and `computer_view_hw_decode` are on here
/// (`wanted`; review M10: HEVC on the processor is slower than H.264);
/// else H.264 alone, said by sending none. An app before PC5 never asks,
/// so it never hears HEVC.
pub fn codec_caps(asked: bool, wanted: bool) -> Option<u8> {
    (asked && wanted).then_some(pocketdesk_core::streaming::client::CODEC_CAP_HEVC)
}

/// The codecs a view the app asked for (`asked` HEVC or not) offers now.
fn view_codec_caps(asked: bool) -> Option<u8> {
    let config = crate::events::current_config();
    codec_caps(
        asked,
        config.computer_view_hevc && config.computer_view_hw_decode,
    )
}

/// What a keyframe says the stream is (PC5): H.264 of the size its SPS
/// says, or HEVC, known by its parameter sets (its size is the window's
/// first picture's); `None` for neither.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    H264(u16, u16),
    Hevc(Vec<u8>),
}

impl Shape {
    /// The codec as the window's stats name it.
    pub fn codec(&self) -> &'static str {
        match self {
            Shape::H264(..) => "H.264",
            Shape::Hevc(_) => "HEVC",
        }
    }

    /// What the window is told before the frames of this shape.
    pub fn message(&self) -> Message {
        match self {
            Shape::H264(width, height) => Message::Config {
                width: *width,
                height: *height,
            },
            Shape::Hevc(_) => Message::HevcConfig,
        }
    }
}

/// The NAL units of an Annex-B access unit, start codes taken off.
fn nal_units(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(n, &start)| {
            let mut end = starts.get(n + 1).map_or(data.len(), |&next| next - 3);
            // A four-byte start code's leading zero.
            if starts.get(n + 1).is_some() && end > start && data[end - 1] == 0 {
                end -= 1;
            }
            &data[start..end.max(start)]
        })
        .collect()
}

/// The shape of a keyframe's stream (PC5): HEVC when it carries a video
/// parameter set (type 32, layer 0: `0x40 0x01`, which no H.264 unit
/// begins with), its parameter sets (VPS, SPS, PPS) together as its mark;
/// else H.264 when its SPS gives a size.
pub fn stream_shape(data: &[u8]) -> Option<Shape> {
    let units = nal_units(data);
    let hevc_type =
        |unit: &[u8]| (unit.len() >= 2 && unit[1] == 0x01).then(|| (unit[0] >> 1) & 0x3F);
    if units.iter().any(|u| hevc_type(u) == Some(32)) {
        let sets: Vec<u8> = units
            .iter()
            .filter(|u| matches!(hevc_type(u), Some(32..=34)))
            .flat_map(|u| u.iter().copied())
            .collect();
        return Some(Shape::Hevc(sets));
    }
    pocketdesk_core::streaming::h264_sps::picture_size(data).map(|(w, h)| Shape::H264(w, h))
}

/// The window's privacy ask waits this long for the other computer's
/// answer (PC8), the first and each renewal: past it, privacy is given up.
pub const PRIVACY_WAIT: Duration = Duration::from_secs(8);
/// How often a privacy mode held there is renewed (PC8): it would lapse by
/// itself after 10 minutes, and a renewal refused says it ended there.
pub const PRIVACY_RENEW: Duration = Duration::from_secs(10);

/// Why privacy ended when no window watched the view any more (PR #561
/// review I2).
const WINDOW_GONE: &str = "Privacy ended: the window closed.";

/// What the Computers page asks of the view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Show the computer `device_id`'s screen `monitor`, replacing any view.
    /// `hevc`: the app decodes HEVC and asks for it (PC5).
    Start {
        device_id: String,
        monitor: u32,
        hevc: bool,
    },
    /// Stop showing it: the view `session_id` when given (one since
    /// replaced is left alone, BK7 review M1), else the one shown now.
    Stop { session_id: Option<u64> },
    /// The computer `device_id` was forgotten here (PR #551 review I1): its
    /// view, if shown, ends.
    Forgotten { device_id: String },
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
    async fn dial(
        &self,
        computer: &PeerComputer,
        monitor: u32,
        caps: Option<u8>,
    ) -> Result<Dialed, String> {
        match self {
            Dialer::Real { certs, wan } => dial(computer, certs, wan.as_ref(), monitor, caps).await,
            Dialer::Direct { addr, certs, id } => {
                let (client, frames, audio) = StreamingClient::connect(
                    &addr.to_string(),
                    certs.clone(),
                    Some(monitor),
                    Some(id),
                    None,
                    caps,
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

/// Dial the computer's screen session by every one of [`plan`]'s ways at
/// once (PC2), rather than the home network first and the internet after
/// it gave up: the first road to come up wins, the home network if it comes
/// within [`PREFER_HOME`] of the internet road, and the others are cancelled
/// or closed before this computer says who it is on any, so the other
/// computer starts one session only. Why none did, when none did.
async fn dial(
    computer: &PeerComputer,
    certs: &Arc<CertManager>,
    wan: Option<&iroh::Endpoint>,
    monitor: u32,
    caps: Option<u8>,
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
    // The internet road only at the key the computer is held to (PC1
    // review M3): a computer paired both ways is dialled at its bound key.
    let internet = computer
        .endpoint
        .as_ref()
        .and_then(crate::peer_computers::WanTarget::parse)
        .map(|target| {
            let bound = crate::state::state_dir()
                .is_ok_and(|dir| crate::peer_computers::endpoint_bound(&dir, computer, &target));
            (target, bound)
        });
    let dials: Vec<(bool, RoadDial<SharedConnection>)> = routes
        .iter()
        .map(|route| match route {
            Route::Home(addr) => (
                true,
                Box::pin(dial_home(*addr, certs.clone())) as RoadDial<SharedConnection>,
            ),
            Route::Internet => (
                false,
                Box::pin(dial_internet(wan.cloned(), internet.clone()))
                    as RoadDial<SharedConnection>,
            ),
        })
        .collect();
    let (won, conn) = race(dials, PREFER_HOME, |lost: SharedConnection| {
        lost.close(0, b"another road won")
    })
    .await
    .map_err(|why| why.join("; "))?;
    let (client, frames, audio) = StreamingClient::attach(
        conn,
        Some(monitor),
        Some(&me.device_id),
        iroh_id.as_deref(),
        caps,
        SessionMode::Screen,
    )
    .await;
    Ok(Dialed {
        client,
        frames,
        audio,
        route: routes[won].words(),
    })
}

/// The home network's road at `addr`: a connection to the certificate
/// pinned there, or why not.
async fn dial_home(addr: SocketAddr, certs: Arc<CertManager>) -> Result<SharedConnection, String> {
    let transport = StreamClient::new(StreamTransportConfig::default(), &certs)
        .map_err(|e| format!("{addr}: {e:#}"))?;
    match tokio::time::timeout(HOME_TRY, transport.connect(addr, &addr.ip().to_string())).await {
        Ok(Ok(conn)) => Ok(QuinnConnection::shared(conn)),
        Ok(Err(e)) => Err(format!("{addr}: {e:#}")),
        Err(_) => Err(format!("{addr}: no answer")),
    }
}

/// The internet road, from this computer's WAN endpoint to the other's
/// `target`, when it is the key that computer is held to (`bound`).
async fn dial_internet(
    wan: Option<iroh::Endpoint>,
    target: Option<(crate::peer_computers::WanTarget, bool)>,
) -> Result<SharedConnection, String> {
    let (Some(wan), Some((target, bound))) = (wan, target) else {
        return Err("internet: no endpoint".to_string());
    };
    if !bound {
        return Err("internet: its endpoint is not the key it paired with".to_string());
    }
    pocketdesk_core::streaming::iroh_connection::dial_stream_from(
        &wan,
        &target.endpoint_id,
        &target.relay_urls,
        &target.direct_addrs,
        INTERNET_TRY,
    )
    .await
    .map_err(|e| format!("internet: {e:#}"))
}

/// What the race between the roads says now (PC2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Keep waiting: until this instant at most, when a road that came up
    /// is held for the home network.
    Wait(Option<Instant>),
    /// The road at this index wins; the rest are cancelled.
    Take(usize),
    /// Every road failed.
    Lost,
}

/// The race between the roads to the other computer, dialled at once
/// (PC2): the home network wins as soon as it comes up; the internet road,
/// up first, is held [`PREFER_HOME`] for a home address still trying, then
/// wins.
#[derive(Debug)]
pub struct Race {
    home: Vec<bool>,
    pending: Vec<bool>,
    /// The internet road, up and held, and until when.
    held: Option<(usize, Instant)>,
    /// A home address that came up: the winner.
    won: Option<usize>,
    grace: Duration,
}

impl Race {
    /// A race between roads, `home[i]` saying whether road `i` is on the
    /// home network.
    pub fn new(home: Vec<bool>, grace: Duration) -> Self {
        let pending = vec![true; home.len()];
        Self {
            home,
            pending,
            held: None,
            won: None,
            grace,
        }
    }

    /// Road `i` came up at `now`.
    pub fn up(&mut self, i: usize, now: Instant) -> Verdict {
        self.pending[i] = false;
        if self.home[i] && self.won.is_none() {
            self.won = Some(i);
        }
        if !self.home[i] && self.held.is_none() {
            self.held = Some((i, now + self.grace));
        }
        self.verdict(now)
    }

    /// Road `i` failed at `now`.
    pub fn failed(&mut self, i: usize, now: Instant) -> Verdict {
        self.pending[i] = false;
        self.verdict(now)
    }

    /// No road is still trying (one ended without saying).
    pub fn all_done(&mut self, now: Instant) -> Verdict {
        self.pending.iter_mut().for_each(|p| *p = false);
        self.verdict(now)
    }

    /// The verdict at `now`, with nothing new.
    pub fn verdict(&self, now: Instant) -> Verdict {
        if let Some(i) = self.won {
            return Verdict::Take(i);
        }
        let home_trying = self.pending.iter().zip(&self.home).any(|(p, h)| *p && *h);
        match self.held {
            Some((i, until)) if now >= until || !home_trying => Verdict::Take(i),
            Some((_, until)) => Verdict::Wait(Some(until)),
            None if self.pending.iter().any(|p| *p) => Verdict::Wait(None),
            None => Verdict::Lost,
        }
    }
}

/// One road's dial: a connection, or why not.
pub type RoadDial<T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, String>> + Send>>;

/// Dial every road at once and take the one [`Race`] picks (PC2): its index
/// and connection, or why each failed. A road that came up and lost is
/// handed to `close`; one still dialling is cancelled.
pub async fn race<T: Send + 'static>(
    dials: Vec<(bool, RoadDial<T>)>,
    grace: Duration,
    close: impl Fn(T),
) -> Result<(usize, T), Vec<String>> {
    let mut race = Race::new(dials.iter().map(|(home, _)| *home).collect(), grace);
    let mut ups: Vec<Option<T>> = dials.iter().map(|_| None).collect();
    let mut why = Vec::new();
    let mut set = tokio::task::JoinSet::new();
    for (i, (_, dial)) in dials.into_iter().enumerate() {
        set.spawn(async move { (i, dial.await) });
    }
    let verdict = loop {
        let wake = match race.verdict(Instant::now()) {
            Verdict::Wait(until) => until,
            done => break done,
        };
        let sleep = async move {
            match wake {
                Some(at) => tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await,
                None => std::future::pending().await,
            }
        };
        // Results first (PR #551 review M4): a home road that came up as
        // the grace ran out still wins.
        tokio::select! {
            biased;
            joined = set.join_next() => match joined {
                Some(Ok((i, Ok(conn)))) => {
                    ups[i] = Some(conn);
                    race.up(i, Instant::now());
                }
                Some(Ok((i, Err(e)))) => {
                    why.push(e);
                    race.failed(i, Instant::now());
                }
                // A dial that panicked: the others decide.
                Some(Err(_)) => {}
                None => {
                    race.all_done(Instant::now());
                }
            },
            () = sleep => {}
        }
    };
    // The losers: cancelled while dialling, closed if they came up.
    set.abort_all();
    while let Some(joined) = set.join_next().await {
        if let Ok((_, Ok(conn))) = joined {
            close(conn);
        }
    }
    match verdict {
        Verdict::Take(won) => {
            for (i, up) in ups.iter_mut().enumerate() {
                if i != won
                    && let Some(conn) = up.take()
                {
                    close(conn);
                }
            }
            match ups[won].take() {
                Some(conn) => Ok((won, conn)),
                None => Err(why),
            }
        }
        _ => Err(why),
    }
}

/// What a session tells the relay.
#[derive(Debug)]
enum Event {
    Status(ViewStatus),
    /// The stream's codec and size, as the window is told it.
    Config(Message),
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

/// What a session counts of its frames, for the window's stats (PC2).
#[derive(Debug, Default)]
pub struct FrameCounts {
    /// This second's frames and bytes.
    frames: u32,
    bytes: u64,
    /// The numbering run now (a rebuilt encoder counts from 0 again): its
    /// lowest and highest number, and the frames that came in it.
    lowest: u64,
    highest: u64,
    arrived: u64,
    /// Frames that never came, in the runs before this one.
    lost_before: u64,
    /// Frames that came after a newer one, dropped.
    pub late: u64,
    /// Frames dropped here: too large for the window.
    pub dropped: u64,
    /// Whether the run now followed another (the encoder was rebuilt).
    rebuilt: bool,
}

impl FrameCounts {
    /// Frames a run has before a 0 starts another, and how far past a
    /// young run's newest a frame of the old run is told apart.
    const NEW_RUN: u64 = 32;

    /// A frame numbered `seq`, of `len` bytes, came.
    pub fn came(&mut self, seq: u64, len: usize) {
        self.frames = self.frames.saturating_add(1);
        self.bytes = self.bytes.saturating_add(len as u64);
        // A rebuilt encoder counts from 0 again: a new run, once the old
        // one is long enough that a 0 is not just the first frame come late
        // (PR #551 review M6).
        let new_run = self.arrived == 0 || (seq == 0 && self.highest >= Self::NEW_RUN);
        if new_run {
            self.lost_before += pocketdesk_core::streaming::client::missed_frames_in_window(
                self.lowest,
                self.highest,
                self.arrived,
            );
            self.rebuilt = self.arrived > 0;
            (self.lowest, self.highest, self.arrived) = (seq, seq, 1);
            return;
        }
        // A frame of the old run, come after the new one started: late.
        if self.rebuilt
            && self.arrived < Self::NEW_RUN
            && seq > self.highest.saturating_add(Self::NEW_RUN)
        {
            self.late += 1;
            return;
        }
        self.lowest = self.lowest.min(seq);
        self.highest = self.highest.max(seq);
        self.arrived += 1;
    }

    /// Frames that never came since the session started.
    pub fn lost(&self) -> u64 {
        self.lost_before
            + pocketdesk_core::streaming::client::missed_frames_in_window(
                self.lowest,
                self.highest,
                self.arrived,
            )
    }

    /// The frames a second and kilobits a second over the `elapsed` since
    /// the last call; the next second starts.
    pub fn second(&mut self, elapsed: Duration) -> (u32, u32) {
        let (frames, bytes) = (
            std::mem::take(&mut self.frames),
            std::mem::take(&mut self.bytes),
        );
        let ms = elapsed.as_millis();
        if ms == 0 {
            return (0, 0);
        }
        let fps = u128::from(frames) * 1000 / ms;
        let kbps = u128::from(bytes) * 8 / ms;
        (
            u32::try_from(fps).unwrap_or(u32::MAX),
            u32::try_from(kbps).unwrap_or(u32::MAX),
        )
    }
}

/// `d` in whole milliseconds, as the stats say it.
fn millis(d: Duration) -> u32 {
    u32::try_from(d.as_millis()).unwrap_or(u32::MAX)
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

/// The words a computer closes a session with when it ends or refuses it on
/// purpose: its End (a kick), a refusal or its owner's no, an unpairing, a
/// permission it does not give.
const ENDED_ON_PURPOSE: [&str; 4] = ["kicked", "refused", "not paired", "not allowed"];

/// Whether a view the other computer closed with `reason` is dialled again
/// by itself (PC2): not when that computer ended it on purpose. A server
/// stopping or restarting, a link gone, the encoder rebuilt or permissions
/// changed are worth another dial.
pub fn may_reconnect(reason: Option<&str>) -> bool {
    let reason = reason.unwrap_or("").to_ascii_lowercase();
    !ENDED_ON_PURPOSE.iter().any(|word| reason.contains(word))
}

/// The words for a session that ended for good: before the other computer
/// let this one in (it refused, or its owner said no), or after (that
/// computer ended it).
fn ended_words(name: &str, admitted: bool, reason: Option<&str>) -> String {
    if admitted {
        // A link gone is dialled again, never said here.
        return match reason.filter(|r| !r.is_empty()) {
            Some(reason) => format!("{name} ended the view ({reason})."),
            None => format!("{name} ended the view."),
        };
    }
    let reason = reason.filter(|r| !r.is_empty()).unwrap_or("it closed");
    format!(
        "{name} did not let this computer in ({reason}). On {name}, this computer must still be paired (its Devices page), and the session allowed if it asks first."
    )
}

/// The words once the tries after a drop gave up.
fn gave_up_words(name: &str) -> String {
    format!("Lost the connection to {name}, and it did not come back within two minutes.")
}

/// A dropped view is dialled again for about this long (PC2).
const RECONNECT_FOR: Duration = Duration::from_secs(120);
/// The longest wait between two tries.
const RECONNECT_MAX_WAIT: Duration = Duration::from_secs(15);
/// A view shown again for this long starts a later drop's tries afresh;
/// one that drops sooner goes on with the tries it was on, so a computer
/// that lets the view in and drops it each time is not dialled for ever.
const STABLE: Duration = Duration::from_secs(30);

/// The wait before try `n` after a drop, from 1: 1, 2, 4 and 8 s, then
/// 15 s each.
pub fn backoff(n: u32) -> Duration {
    let secs = 1u64.checked_shl(n.saturating_sub(1)).unwrap_or(u64::MAX);
    Duration::from_secs(secs).min(RECONNECT_MAX_WAIT)
}

/// The tries after a view dropped (PC2).
#[derive(Debug, Default)]
pub struct Reconnect {
    /// When the drop these tries are for came.
    since: Option<Instant>,
    /// The tries made since.
    tries: u32,
    /// When the view was last shown again.
    shown: Option<Instant>,
}

impl Reconnect {
    /// The view dropped at `now`: the try to make and the wait before it,
    /// or `None` once the time is up.
    pub fn dropped(&mut self, now: Instant) -> Option<(u32, Duration)> {
        let afresh = self.since.is_none()
            || self
                .shown
                .is_some_and(|at| now.saturating_duration_since(at) >= STABLE);
        if afresh {
            self.since = Some(now);
            self.tries = 0;
        }
        self.shown = None;
        self.next(now)
    }

    /// A try failed at `now`: the next and the wait before it, or `None`
    /// once the time is up (or when nothing dropped).
    pub fn failed(&mut self, now: Instant) -> Option<(u32, Duration)> {
        self.since?;
        self.next(now)
    }

    /// The view was shown again at `now`.
    pub fn shown(&mut self, now: Instant) {
        self.shown = Some(now);
    }

    fn next(&mut self, now: Instant) -> Option<(u32, Duration)> {
        let since = self.since?;
        let n = self.tries.saturating_add(1);
        let wait = backoff(n);
        if now.saturating_duration_since(since) + wait > RECONNECT_FOR {
            return None;
        }
        self.tries = n;
        Some((n, wait))
    }
}

/// The window's choices a new session there must hear again after a
/// reconnect (PC2): the last quality and view-only it sent.
#[derive(Debug, Default)]
pub struct Sticky {
    quality: Option<u8>,
    view_only: Option<bool>,
}

impl Sticky {
    /// The window sent `packet`.
    pub fn note(&mut self, packet: &InputPacket) {
        match packet {
            InputPacket::QualityPreset { preset } => self.quality = Some(*preset),
            InputPacket::ViewOnly { enabled } => self.view_only = Some(*enabled),
            _ => {}
        }
    }

    /// What a new session hears again.
    pub fn replay(&self) -> Vec<InputPacket> {
        let mut packets = Vec::new();
        if let Some(preset) = self.quality {
            packets.push(InputPacket::QualityPreset { preset });
        }
        if let Some(enabled) = self.view_only {
            packets.push(InputPacket::ViewOnly { enabled });
        }
        packets
    }
}

/// Aborts its task when dropped: the view's input goes when the view does.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Says a view's events to the relay.
struct Say {
    session_id: u64,
    events: mpsc::Sender<(u64, Event)>,
}

impl Say {
    async fn say(&self, event: Event) {
        let _ = self.events.send((self.session_id, event)).await;
    }
}

/// How one session with the other computer ended.
enum Outcome {
    /// It never let this one in: its close reason.
    NotLetIn(Option<String>),
    /// Shown, then gone: its close reason.
    Lost(Option<String>),
}

/// One view, until it ends: dialled, shown, and after a drop dialled again
/// by itself with [`backoff`] for about two minutes (PC2). A Stop or a
/// window closed aborts it, so it never dials again after one.
#[allow(clippy::too_many_arguments)]
async fn session(
    session_id: u64,
    state_dir: PathBuf,
    mut computer: PeerComputer,
    monitor: u32,
    hevc: bool,
    dialer: Arc<Dialer>,
    events: mpsc::Sender<(u64, Event)>,
    input: mpsc::Receiver<InputPacket>,
) {
    let say = Say { session_id, events };
    let name = computer.name.clone();
    let mut status = ViewStatus {
        monitor,
        ..ViewStatus::default()
    };
    crate::peer_computers::ask_screens(&computer.device_id);
    say.say(Event::Status(status.clone())).await;
    // The window's input goes to the session of the moment, for the
    // view's whole life.
    let (conn_now, conn_rx) = tokio::sync::watch::channel(None);
    let _forward = AbortOnDrop(tokio::spawn(forward_input(input, conn_rx)));
    let mut reconnect = Reconnect::default();
    let mut shown_once = false;
    loop {
        let dial_started = Instant::now();
        // HEVC too when the window's decoder reads it (PC5), as this
        // computer's settings say at each dial.
        let next = match dialer.dial(&computer, monitor, view_codec_caps(hevc)).await {
            Err(why) if !shown_once => {
                // Its addresses stay out of the log at info (BK7 review M8):
                // the window says them to the owner.
                tracing::info!(device_id = %computer.device_id, "Another computer's screen not reached");
                tracing::debug!(device_id = %computer.device_id, "Another computer's screen not reached: {why}");
                say.say(Event::Ended(format!("Could not reach {name}: {why}")))
                    .await;
                return;
            }
            Err(why) => {
                tracing::debug!(device_id = %computer.device_id, attempt = status.reconnecting, "Another computer's screen not reached again: {why}");
                reconnect.failed(Instant::now())
            }
            Ok(dialed) => {
                let dial_ms = millis(dial_started.elapsed());
                let outcome = attempt(
                    &computer,
                    dialed,
                    dial_ms,
                    &mut status,
                    &say,
                    &conn_now,
                    &mut reconnect,
                )
                .await;
                match outcome {
                    Outcome::Lost(reason) => {
                        shown_once = true;
                        if !may_reconnect(reason.as_deref()) {
                            tracing::info!(device_id = %computer.device_id, "Another computer ended the view of its screen");
                            say.say(Event::Ended(ended_words(&name, true, reason.as_deref())))
                                .await;
                            return;
                        }
                        tracing::info!(device_id = %computer.device_id, "Another computer's screen dropped; dialling it again");
                        reconnect.dropped(Instant::now())
                    }
                    // Its server still starting, during the tries: another try.
                    Outcome::NotLetIn(reason) if shown_once && may_reconnect(reason.as_deref()) => {
                        reconnect.failed(Instant::now())
                    }
                    // A refusal, or its owner's no, is honoured.
                    Outcome::NotLetIn(reason) => {
                        tracing::info!(device_id = %computer.device_id, "Another computer did not let this one see its screen");
                        say.say(Event::Ended(ended_words(&name, false, reason.as_deref())))
                            .await;
                        return;
                    }
                }
            }
        };
        let Some((attempt_no, wait)) = next else {
            tracing::info!(device_id = %computer.device_id, "Another computer's screen did not come back");
            say.say(Event::Ended(gave_up_words(&name))).await;
            return;
        };
        status = ViewStatus {
            phase: Phase::Connecting,
            monitor: status.monitor,
            monitors: std::mem::take(&mut status.monitors),
            reconnecting: attempt_no,
            ..ViewStatus::default()
        };
        say.say(Event::Status(status.clone())).await;
        tokio::time::sleep(wait).await;
        // Its record as it is now (PR #551 review I1): new addresses are
        // tried, and a computer forgotten here meanwhile is not dialled.
        match computers::read(&state_dir)
            .computers
            .into_iter()
            .find(|c| c.device_id == computer.device_id)
        {
            Some(fresh) => computer = fresh,
            None => {
                tracing::info!(device_id = %computer.device_id, "Another computer's screen not dialled again: it is no longer paired");
                say.say(Event::Ended(format!("{name} is no longer paired here.")))
                    .await;
                return;
            }
        }
    }
}

/// The window's input, in order, one stream each as a phone sends it, to
/// the session that let this computer in; before one does, and between two
/// (a reconnect), it goes nowhere (PR #551 review M7: no burst of what was
/// typed meanwhile). Each session that lets it in hears the window's last
/// quality and view-only first, from this same task, so a choice made as it
/// is let in comes after them (review M5).
async fn forward_input(
    mut input: mpsc::Receiver<InputPacket>,
    mut conn: tokio::sync::watch::Receiver<Option<SharedConnection>>,
) {
    let mut sticky = Sticky::default();
    let mut current: Option<SharedConnection> = None;
    loop {
        tokio::select! {
            biased;
            changed = conn.changed() => {
                if changed.is_err() {
                    return;
                }
                current = conn.borrow_and_update().clone();
                if let Some(conn) = &current {
                    for packet in sticky.replay() {
                        deliver(conn, &packet).await;
                    }
                }
            }
            packet = input.recv() => {
                let Some(packet) = packet else { return };
                sticky.note(&packet);
                if let Some(conn) = &current {
                    deliver(conn, &packet).await;
                }
            }
        }
    }
}

/// One input packet to the other computer, a release tried again on a
/// stalled link (BK7 review M7): left undone, the key stays held there.
async fn deliver(conn: &SharedConnection, packet: &InputPacket) {
    let tries = if is_release(packet) { RELEASE_TRIES } else { 1 };
    for _ in 0..tries {
        let sent = tokio::time::timeout(INPUT_SEND, send_input(conn, packet)).await;
        if matches!(sent, Ok(Ok(()))) {
            break;
        }
        tracing::debug!("An input packet did not reach the other computer");
    }
}

/// One session with the other computer, dialled, until it ends.
#[allow(clippy::too_many_arguments)]
async fn attempt(
    computer: &PeerComputer,
    dialed: Dialed,
    dial_ms: u32,
    status: &mut ViewStatus,
    say: &Say,
    conn_now: &tokio::sync::watch::Sender<Option<SharedConnection>>,
    reconnect: &mut Reconnect,
) -> Outcome {
    let name = &computer.name;
    let Dialed {
        mut client,
        mut frames,
        audio,
        route,
    } = dialed;
    tracing::info!(device_id = %computer.device_id, route, dial_ms, "Showing another computer's screen");
    let conn = client.connection().cloned();
    let remote = client.remote();
    let _hangup = Hangup {
        cancel: client.cancel_token(),
        conn: conn.clone(),
    };
    tokio::spawn(async move { client.start().await });
    tokio::spawn(play_sound(audio));
    status.phase = Phase::Waiting;
    status.route = route.to_string();
    say.say(Event::Status(status.clone())).await;
    let mut shape: Option<Shape> = None;
    let mut last_seq: Option<u64> = None;
    let mut admitted = false;
    let mut shown = false;
    let mut look = tokio::time::interval(LOOK);
    let mut said_too_large = false;
    let mut too_large_ask: Option<Instant> = None;
    let mut counts = FrameCounts::default();
    let mut stats_tick = tokio::time::interval(STATS_EVERY);
    let mut second = Instant::now();
    loop {
        tokio::select! {
            packet = frames.recv() => {
                let Some(packet) = packet else { break };
                admitted = true;
                counts.came(packet.sequence, packet.data.len());
                if !in_order(packet.is_keyframe, packet.sequence, last_seq) {
                    counts.late += 1;
                    continue;
                }
                last_seq = Some(packet.sequence);
                // A new size (another screen, its resolution changed) or
                // codec comes with the keyframe that starts it; HEVC's by
                // new parameter sets (PC5).
                if packet.is_keyframe
                    && let Some(read) = stream_shape(&packet.data)
                    && shape.as_ref() != Some(&read)
                {
                    say.say(Event::Config(read.message())).await;
                    shape = Some(read);
                }
                if shape.is_none() {
                    continue;
                }
                // A frame the relay cannot carry (BK7 review M6): said once,
                // and the chain it breaks starts again from a keyframe.
                if !fits(packet.data.len()) {
                    counts.dropped += 1;
                    // The window says so (BK7 review M6).
                    if status.note.is_none() {
                        status.note = Some(format!(
                            "{name}'s screen comes in frames too large to show here. A lower Quality, or a smaller screen there, may help."
                        ));
                        say.say(Event::Status(status.clone())).await;
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
                    say.say(Event::Status(status.clone())).await;
                }
                say.say(Event::Frame {
                    keyframe: packet.is_keyframe,
                    seq: packet.sequence,
                    data: packet.data,
                }).await;
            }
            _ = look.tick() => {}
            _ = stats_tick.tick() => {
                // How the picture comes, for the window's stats (PC2).
                let (fps, kbps) = counts.second(second.elapsed());
                second = Instant::now();
                let link = conn.as_ref().map(|c| c.stats());
                status.stats = Some(pocketdesk_core::local::view::ViewStats {
                    fps,
                    kbps,
                    rtt_ms: link.as_ref().filter(|l| !l.rtt.is_zero()).map(|l| millis(l.rtt)),
                    relayed: link.as_ref().is_some_and(|l| l.relayed),
                    dial_ms: Some(dial_ms),
                    lost: counts.lost(),
                    late: counts.late,
                    dropped: counts.dropped,
                    codec: shape.as_ref().map_or("H.264", Shape::codec).to_string(),
                });
                say.say(Event::Status(status.clone())).await;
            }
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
        // Which system it runs, for the window's Send keys menu (PC4).
        if let Some(windows) = pocketdesk_core::streaming::screen_runs_windows(features) {
            next.windows = Some(windows);
        }
        if admitted {
            next.phase = Phase::Showing;
            if !shown {
                // Let in (again, after a drop): the tries are over, and
                // the window's input (its choices first) goes to this
                // session.
                shown = true;
                next.reconnecting = 0;
                reconnect.shown(Instant::now());
                conn_now.send_replace(conn.clone());
            }
        }
        next.monitors = crate::peer_computers::screens(&computer.device_id);
        if next != *status {
            *status = next;
            say.say(Event::Status(status.clone())).await;
        }
    }
    conn_now.send_replace(None);
    let reason = conn.as_ref().and_then(|c| c.close_reason());
    tracing::info!(device_id = %computer.device_id, "Another computer's screen ended");
    if admitted {
        Outcome::Lost(reason)
    } else {
        Outcome::NotLetIn(reason)
    }
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

/// Sends a privacy packet to the computer named (PC8), without waiting.
pub type PrivacySend = Arc<dyn Fn(&str, NetworkPacket) + Send + Sync>;

/// Where the window's privacy asks go and their answers come from (PC8):
/// the other computer, or a stand-in for it in tests.
pub struct PrivacyHost {
    pub send: PrivacySend,
    pub answers: mpsc::Receiver<(String, serde_json::Value)>,
    /// Tells the owner here what went wrong (PR #561 review I3: a lock on
    /// close that failed): a notification, or a stand-in in tests.
    pub notice: PrivacyNotice,
}

/// Shows the owner a notice, in words (PR #561 review I3).
pub type PrivacyNotice = Arc<dyn Fn(String) + Send + Sync>;

/// How long a lock asked as the window closes waits for its answer before
/// the owner is told it did not come (PR #561 review I3).
pub const LOCK_WAIT: Duration = Duration::from_secs(10);

/// A lock asked of the computer `device_id` (`name`), as ask `id`, at
/// `since`, whose answer is awaited (PR #561 review I3).
struct LockPending {
    device_id: String,
    name: String,
    id: u64,
    since: Instant,
}

/// What an answer says of a lock: done (`Ok`), not done and why (`Err`),
/// or nothing of one.
pub fn lock_heard(body: &serde_json::Value) -> Option<Result<(), String>> {
    if body.get("locked").is_some() {
        return Some(Ok(()));
    }
    if let Some(why) = body.get("lockError") {
        return Some(Err(why.as_str().unwrap_or("its lock failed").to_string()));
    }
    if body.get("ok").and_then(|v| v.as_bool()) == Some(false) {
        let why = body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("it refused");
        return Some(Err(why.to_string()));
    }
    None
}

/// What the owner is told when the computer `name` was not locked.
pub fn lock_failed_words(name: &str, why: &str) -> String {
    format!("Couldn't lock {name}: {why}")
}

/// Where the answers that come on a link or a connection go: the relay
/// running now.
type AnswerSender = mpsc::Sender<(String, serde_json::Value)>;
static ANSWERS: std::sync::Mutex<Option<AnswerSender>> = std::sync::Mutex::new(None);

impl PrivacyHost {
    /// The other computer itself: asked on its connection here or on the
    /// link this one keeps to it, as the window's clipboard asks; its
    /// answers come back through [`privacy_answer`].
    pub fn real() -> Self {
        let (tx, answers) = mpsc::channel(16);
        *ANSWERS.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
        // One ordered sender per computer (PR #561 re-review N3): a Hide
        // then a Show go in that order, and a lock goes only while its
        // answer is still awaited.
        let queues: Arc<std::sync::Mutex<HashMap<String, PrivacyQueue>>> = Arc::default();
        let send: PrivacySend = Arc::new(move |device_id: &str, packet: NetworkPacket| {
            remember_asked(device_id);
            let mut queues = queues.lock().unwrap_or_else(|e| e.into_inner());
            let queue = queues.entry(device_id.to_string()).or_insert_with(|| {
                privacy_queue(
                    device_id,
                    |device_id: String, packet: NetworkPacket| async move {
                        crate::clipboard_push::send_to_computer(&device_id, &packet).await
                    },
                )
            });
            let _ = queue.send((packet, Instant::now()));
        });
        let notice: PrivacyNotice = Arc::new(|words: String| {
            tracing::info!("{words}");
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn_blocking(move || {
                    crate::platform::notify("Pocketdesk", &words, Duration::from_secs(10))
                });
            }
        });
        Self {
            send,
            answers,
            notice,
        }
    }
}

/// A computer's privacy asks, waiting their turn (PR #561 re-review N3).
type PrivacyQueue = mpsc::UnboundedSender<(NetworkPacket, Instant)>;

/// A lock still goes this long after it was asked, and no longer: past it
/// the owner may already have been told it never came ([`LOCK_WAIT`]), so a
/// lock never lands after that word.
pub const LOCK_SEND_BY: Duration = Duration::from_secs(8);

/// Whether `packet`, asked at `asked`, still goes at `now`: a lock only
/// within [`LOCK_SEND_BY`], anything else always (a release late is still
/// a release).
pub fn still_goes(packet: &NetworkPacket, asked: Instant, now: Instant) -> bool {
    let lock = packet.body.get("lock").and_then(|v| v.as_bool()) == Some(true);
    !lock || now.saturating_duration_since(asked) < LOCK_SEND_BY
}

/// The ordered sender to the computer `device_id`: one task sends its asks
/// one after the other, as [`crate::clipboard_push::send_to_computer`]
/// does (its connection here, else the link this one keeps).
fn privacy_queue<F, Fut>(device_id: &str, deliver: F) -> PrivacyQueue
where
    F: Fn(String, NetworkPacket) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<(), String>> + Send,
{
    let (tx, mut rx) = mpsc::unbounded_channel::<(NetworkPacket, Instant)>();
    let device_id = device_id.to_string();
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(async move {
            while let Some((packet, asked)) = rx.recv().await {
                if !still_goes(&packet, asked, Instant::now()) {
                    tracing::info!(device_id, "A lock asked too long ago was not sent");
                    continue;
                }
                if let Err(e) = deliver(device_id.clone(), packet).await {
                    tracing::info!("Privacy not asked of {device_id}: {e}");
                }
            }
        });
    }
    tx
}

/// The computers a privacy ask went to (PR #561 review M4): only their
/// answers enter the relay's channel, so a paired phone sending answers
/// cannot crowd out the computer's.
static ASKED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Note that a privacy ask went to `device_id`.
pub(crate) fn remember_asked(device_id: &str) {
    let mut asked = ASKED.lock().unwrap_or_else(|e| e.into_inner());
    if !asked.iter().any(|d| d == device_id) {
        asked.push(device_id.to_string());
    }
}

/// Whether a privacy ask ever went to `device_id` from here.
pub(crate) fn was_asked(device_id: &str) -> bool {
    ASKED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|d| d == device_id)
}

/// An answer from the computer `device_id` to a privacy ask (PC8), for the
/// view that asked it, if any; from a device never asked, dropped here.
pub(crate) fn privacy_answer(device_id: &str, body: &serde_json::Value) {
    if !was_asked(device_id) {
        tracing::debug!(
            device_id,
            "A privacy answer from a device not asked; dropped"
        );
        return;
    }
    if let Some(tx) = ANSWERS.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = tx.try_send((device_id.to_string(), body.clone()));
    }
}

/// What the window's privacy ask does (PC8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivacyStep {
    /// Ask the other computer for its privacy mode.
    Grab,
    /// Let it go there, locking that computer too when `lock`.
    Release {
        lock: bool,
    },
    /// Lock that computer, with no privacy held.
    Lock,
    /// Not asked, and why.
    Refuse(String),
    Nothing,
}

/// What the window's privacy `ask` ([`wire::PRIVACY_HIDE`], `_SHOW` or
/// `_LOCK`) does, with privacy `holding` or not, for the view of the
/// computer `name` that stands as `status`: hidden only while shown and
/// controlled (the window's input taken there), never while it reconnects.
pub fn privacy_step(ask: u8, holding: bool, status: &ViewStatus, name: &str) -> PrivacyStep {
    match ask {
        wire::PRIVACY_HIDE if holding => PrivacyStep::Nothing,
        wire::PRIVACY_HIDE if status.phase != Phase::Showing || status.reconnecting > 0 => {
            PrivacyStep::Refuse(format!("Not connected to {name} now."))
        }
        wire::PRIVACY_HIDE if status.control != Some(true) => PrivacyStep::Refuse(format!(
            "{name} does not take this computer's clicks and keys, so there is nothing to hide."
        )),
        wire::PRIVACY_HIDE => PrivacyStep::Grab,
        wire::PRIVACY_LOCK if holding => PrivacyStep::Release { lock: true },
        wire::PRIVACY_LOCK => PrivacyStep::Lock,
        _ if holding => PrivacyStep::Release { lock: false },
        _ => PrivacyStep::Nothing,
    }
}

/// The packet a step sends the other computer, or none.
pub fn privacy_packet(step: &PrivacyStep) -> Option<NetworkPacket> {
    let body = match step {
        PrivacyStep::Grab => serde_json::json!({ "action": "grab" }),
        // The lock goes as its own status (review I3), never refused for
        // a grab another device holds.
        PrivacyStep::Release { .. } => serde_json::json!({ "action": "release" }),
        PrivacyStep::Lock => serde_json::json!({ "action": "status", "lock": true }),
        PrivacyStep::Refuse(_) | PrivacyStep::Nothing => return None,
    };
    Some(NetworkPacket::new(crate::plugins::privacy::PRIVACY).with_body(body))
}

/// A renewal of a privacy mode held there: only one still held is renewed.
pub fn renewal() -> NetworkPacket {
    NetworkPacket::new(crate::plugins::privacy::PRIVACY)
        .with_body(serde_json::json!({ "action": "grab", "renew": true }))
}

/// What an answer from the other computer says of a grab or a renewal:
/// held (`Ok`), refused and why (`Err`), or nothing about one (a status, a
/// release, a lock).
pub fn privacy_heard(body: &serde_json::Value) -> Option<Result<(), String>> {
    let ok = body.get("ok").and_then(|v| v.as_bool());
    if ok == Some(false) {
        let why = body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("it refused");
        return Some(Err(why.to_string()));
    }
    let grab = body.get("grabbed").is_some() && body.get("ttlLeft").is_none();
    (ok == Some(true) && grab).then_some(Ok(()))
}

/// What a held privacy mode needs now (PC8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivacyDue {
    Nothing,
    /// Renew it there.
    Renew,
    /// The last ask went unanswered: give it up.
    GiveUp,
}

/// What a privacy mode held needs at `now`: given up when an ask sent at
/// `sent` is still `waiting` after [`PRIVACY_WAIT`], renewed once `on` and
/// [`PRIVACY_RENEW`] after the last.
pub fn privacy_due(on: bool, waiting: bool, sent: Instant, now: Instant) -> PrivacyDue {
    let since = now.saturating_duration_since(sent);
    if waiting {
        if since >= PRIVACY_WAIT {
            PrivacyDue::GiveUp
        } else {
            PrivacyDue::Nothing
        }
    } else if on && since >= PRIVACY_RENEW {
        PrivacyDue::Renew
    } else {
        PrivacyDue::Nothing
    }
}

/// Why privacy held there ends as the view comes to stand as `status`, or
/// `None` while it may stay: the view dropped (it reconnects) or that
/// computer stopped taking this one's input.
pub fn privacy_ends(status: &ViewStatus, name: &str) -> Option<String> {
    if status.reconnecting > 0 || status.phase != Phase::Showing {
        Some(format!(
            "Privacy ended: the view of {name} dropped. Turn it on again once it is back."
        ))
    } else if status.control != Some(true) {
        Some(format!(
            "Privacy ended: {name} no longer takes this computer's clicks and keys."
        ))
    } else {
        None
    }
}

/// A new id for a privacy ask (PR #561 review M3): the other computer says
/// it again in its answer, so an answer to an older ask is not taken for
/// the newest.
fn next_ask_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// `packet` with the ask's `id`.
pub fn tagged(mut packet: NetworkPacket, id: u64) -> NetworkPacket {
    packet.body["id"] = serde_json::json!(id);
    packet
}

/// Whether the answer `body` answers the ask `id`: it names it, or names
/// none (a computer before the review, which says no id).
pub fn answers_ask(body: &serde_json::Value, id: u64) -> bool {
    body.get("id").is_none_or(|said| said.as_u64() == Some(id))
}

/// The window's privacy mode held on the other computer (PC8): asked, then
/// renewed, and let go there however it ends here, as the hold drops.
struct PrivacyHold {
    device_id: String,
    send: PrivacySend,
    /// That computer said it holds it.
    on: bool,
    /// When the last grab or renewal went, its id, and whether it is
    /// unanswered.
    sent: Instant,
    asked: u64,
    waiting: bool,
    /// Let go already, or never held there (refused).
    done: bool,
}

impl PrivacyHold {
    fn ask(device_id: &str, send: PrivacySend, now: Instant) -> Self {
        let asked = next_ask_id();
        if let Some(packet) = privacy_packet(&PrivacyStep::Grab) {
            send(device_id, tagged(packet, asked));
        }
        Self {
            device_id: device_id.to_string(),
            send,
            on: false,
            sent: now,
            asked,
            waiting: true,
            done: false,
        }
    }

    fn renew(&mut self, now: Instant) {
        self.asked = next_ask_id();
        (self.send)(&self.device_id, tagged(renewal(), self.asked));
        self.sent = now;
        self.waiting = true;
    }

    /// Let it go there.
    fn let_go(mut self) {
        self.done = true;
        if let Some(packet) = privacy_packet(&PrivacyStep::Release { lock: false }) {
            (self.send)(&self.device_id, tagged(packet, next_ask_id()));
        }
    }
}

impl Drop for PrivacyHold {
    fn drop(&mut self) {
        if !self.done
            && let Some(packet) = privacy_packet(&PrivacyStep::Release { lock: false })
        {
            (self.send)(&self.device_id, tagged(packet, next_ask_id()));
        }
    }
}

struct View {
    session_id: u64,
    device_id: String,
    /// The computer's name, for the window's words.
    name: String,
    /// Its privacy mode, while the window holds it (PC8).
    privacy: Option<PrivacyHold>,
    token: [u8; TOKEN_LEN],
    input: mpsc::Sender<InputPacket>,
    task: Option<tokio::task::JoinHandle<()>>,
    /// The stream's codec and size as last told, for a window that joins.
    config: Option<Message>,
    status: ViewStatus,
    /// Why it ended, and when.
    ended: Option<(String, Instant)>,
    /// Since when nobody watches it.
    unwatched_since: Option<Instant>,
    asks: CastChain,
    /// Frames the window was too slow to take, for its stats (PC2).
    window_dropped: u64,
    /// The window's focus last passed on, and when (PC3).
    focus_said: Option<(bool, Instant)>,
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
    commands: mpsc::Receiver<Command>,
    listener: TcpListener,
    state_dir: PathBuf,
    dialer: Dialer,
) {
    run_with(commands, listener, state_dir, dialer, PrivacyHost::real()).await
}

/// [`run`], with the window's privacy asked of `privacy` (PC8): the other
/// computer itself, or a stand-in in tests.
pub async fn run_with(
    mut commands: mpsc::Receiver<Command>,
    listener: TcpListener,
    state_dir: PathBuf,
    dialer: Dialer,
    privacy: PrivacyHost,
) {
    let PrivacyHost {
        send: privacy_send,
        answers: mut privacy_answers,
        notice: privacy_notice,
    } = privacy;
    let mut lock_pending: Option<LockPending> = None;
    // Ask the computer `device_id` (`name`) to lock, its answer awaited.
    let ask_lock = |device_id: &str, name: &str| {
        let id = next_ask_id();
        if let Some(packet) = privacy_packet(&PrivacyStep::Lock) {
            privacy_send(device_id, tagged(packet, id));
        }
        LockPending {
            device_id: device_id.to_string(),
            name: name.to_string(),
            id,
            since: Instant::now(),
        }
    };
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
    let (focus_tx, mut focus_rx) = mpsc::channel::<(u64, bool)>(8);
    let (privacy_tx, mut privacy_rx) = mpsc::channel::<(u64, String, u8)>(8);
    let pending = Arc::new(tokio::sync::Semaphore::new(PENDING_VIEWERS));
    let mut heartbeat = tokio::time::interval(HEARTBEAT);
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                let Command::Start { device_id, monitor, hevc } = command else {
                    match command {
                        Command::Stop { session_id }
                            if stops(session_id, view.as_ref().map(|v| v.session_id)) =>
                        {
                            end(&mut view, &mut viewer, &state_dir, None);
                        }
                        Command::Forgotten { device_id }
                            if view.as_ref().is_some_and(|v| v.device_id == device_id) =>
                        {
                            end(&mut view, &mut viewer, &state_dir, Some(NOT_PAIRED));
                        }
                        _ => {}
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
                            state_dir.clone(),
                            computer,
                            monitor,
                            hevc,
                            dialer.clone(),
                            events_tx.clone(),
                            input_rx,
                        ))),
                        None,
                    ),
                    None => (
                        None,
                        Some((NOT_PAIRED.to_string(), Instant::now())),
                    ),
                };
                view = Some(View {
                    session_id,
                    device_id,
                    name,
                    privacy: None,
                    token,
                    input: input_tx,
                    task,
                    config: None,
                    status: ViewStatus { monitor, ..ViewStatus::default() },
                    ended,
                    unwatched_since: Some(Instant::now()),
                    asks: CastChain::default(),
                    window_dropped: 0,
                    focus_said: None,
                });
            }
            Some((session_id, event)) = events.recv() => {
                let Some(current) = view.as_mut().filter(|v| v.session_id == session_id) else {
                    continue;
                };
                let watching = viewer.as_ref().filter(|v| v.session_id == session_id);
                match event {
                    Event::Status(mut status) => {
                        match status.stats.as_mut() {
                            Some(stats) => stats.dropped += current.window_dropped,
                            // A new session's counts start again, the
                            // window's too (PR #551 review M6).
                            None => current.window_dropped = 0,
                        }
                        // Privacy is the relay's to say (PC8): it ends
                        // there as the view drops or control goes.
                        status.privacy = current.status.privacy.clone();
                        if current.privacy.is_some()
                            && let Some(why) = privacy_ends(&status, &current.name)
                        {
                            current.privacy = None;
                            status.privacy = ViewPrivacy::Refused(why);
                        }
                        current.status = status.clone();
                        if let Some(v) = watching {
                            let _ = v.tx.try_send(Message::Status(status));
                        }
                    }
                    Event::Config(config) => {
                        current.config = Some(config.clone());
                        // A viewer that missed a new size would decode at the
                        // old one: it is dropped, and rejoins with the size.
                        if let Some(v) = watching
                            && v.tx.try_send(config).is_err()
                        {
                            viewer = None;
                        }
                    }
                    Event::Frame { keyframe, seq, data } => {
                        if let Some(v) = watching
                            && v.tx.try_send(Message::Frame { keyframe, seq, data }).is_err()
                        {
                            current.window_dropped += 1;
                            if current.asks.lost(Instant::now()) {
                                let _ = current.input.try_send(InputPacket::RequestKeyframe);
                            }
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
                let joined = start_viewer(
                    stream,
                    session_id,
                    v.device_id.clone(),
                    v.input.clone(),
                    focus_tx.clone(),
                    privacy_tx.clone(),
                );
                let _ = joined.tx.try_send(Message::Status(v.status.clone()));
                if let Some(config) = v.config.clone() {
                    let _ = joined.tx.try_send(config);
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
            Some((session_id, focused)) = focus_rx.recv() => {
                // The window watching the view now, of a view still going.
                let Some(v) = view
                    .as_mut()
                    .filter(|v| v.session_id == session_id && v.ended.is_none())
                else {
                    continue;
                };
                if viewer.as_ref().is_none_or(|w| w.session_id != session_id) {
                    continue;
                }
                // The app settles the focus first; a client that does not
                // is held to it here too (PR #554 review M2).
                let now = Instant::now();
                if !focus_passes(v.focus_said, focused, now) {
                    continue;
                }
                v.focus_said = Some((focused, now));
                window_focus(v.device_id.clone(), focused, v.status.control);
            }
            Some((session_id, device_id, ask)) = privacy_rx.recv() => {
                let watching = viewer.as_ref().is_some_and(|w| w.session_id == session_id);
                let Some(v) = view
                    .as_mut()
                    .filter(|v| v.session_id == session_id && v.ended.is_none() && watching)
                else {
                    // The view went before its window's last word: its
                    // privacy was let go as it ended; a lock is still asked.
                    if ask == wire::PRIVACY_LOCK {
                        // computers.json is a file: read off the runtime
                        // (PR #561 re-review N5).
                        let (dir, id) = (state_dir.clone(), device_id.clone());
                        let name = tokio::task::spawn_blocking(move || {
                            computers::read(&dir)
                                .computers
                                .into_iter()
                                .find(|c| c.device_id == id)
                                .map(|c| c.name)
                        })
                        .await
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| "that computer".to_string());
                        lock_pending = Some(ask_lock(&device_id, &name));
                    }
                    continue;
                };
                let said = match privacy_step(ask, v.privacy.is_some(), &v.status, &v.name) {
                    PrivacyStep::Grab => {
                        v.privacy = Some(PrivacyHold::ask(
                            &v.device_id,
                            privacy_send.clone(),
                            Instant::now(),
                        ));
                        ViewPrivacy::Asking
                    }
                    PrivacyStep::Release { lock } => {
                        if let Some(hold) = v.privacy.take() {
                            hold.let_go();
                        }
                        if lock {
                            lock_pending = Some(ask_lock(&v.device_id, &v.name));
                        }
                        ViewPrivacy::Off
                    }
                    PrivacyStep::Lock => {
                        lock_pending = Some(ask_lock(&v.device_id, &v.name));
                        v.status.privacy.clone()
                    }
                    PrivacyStep::Refuse(why) => ViewPrivacy::Refused(why),
                    PrivacyStep::Nothing if ask == wire::PRIVACY_SHOW => ViewPrivacy::Off,
                    PrivacyStep::Nothing => v.status.privacy.clone(),
                };
                answer_privacy(v, viewer.as_ref(), said);
            }
            Some((device_id, body)) = privacy_answers.recv() => {
                // The answer to a lock asked as the window closed (review
                // I3): a lock not done is said to the owner here.
                if let Some(pending) = lock_pending
                    .as_ref()
                    .filter(|p| p.device_id == device_id && answers_ask(&body, p.id))
                    && let Some(heard) = lock_heard(&body)
                {
                    match heard {
                        Ok(()) => tracing::info!(device_id = %pending.device_id, "Locked another computer as its window closed"),
                        Err(why) => privacy_notice(lock_failed_words(&pending.name, &why)),
                    }
                    lock_pending = None;
                    continue;
                }
                // Only an answer to the hold's newest ask (review M3): one to
                // an older ask, refused late, leaves a newer grab alone.
                let Some(v) = view.as_mut().filter(|v| {
                    v.device_id == device_id
                        && v.privacy.as_ref().is_some_and(|h| answers_ask(&body, h.asked))
                }) else {
                    continue;
                };
                let watching = viewer.as_ref().filter(|w| w.session_id == v.session_id);
                match privacy_heard(&body) {
                    Some(Ok(())) => {
                        if let Some(hold) = v.privacy.as_mut() {
                            hold.on = true;
                            hold.waiting = false;
                        }
                        say_privacy(v, watching, ViewPrivacy::On);
                    }
                    Some(Err(why)) => {
                        // Let go there anyway, as the hold drops: a release
                        // with nothing held is nothing, and one that crossed
                        // a grant is not left held.
                        v.privacy = None;
                        tracing::info!(device_id = %v.device_id, "Privacy there refused: {why}");
                        say_privacy(v, watching, ViewPrivacy::Refused(why));
                    }
                    None => {}
                }
            }
            _ = heartbeat.tick() => {
                // A lock whose answer never came (the link down, that
                // computer gone) is said too (review I3).
                if let Some(pending) = lock_pending
                    .take_if(|p| p.since.elapsed() >= LOCK_WAIT)
                {
                    privacy_notice(lock_failed_words(&pending.name, "it did not answer."));
                }
                // A window gone (closed, crashed): its socket closed, so its
                // reader ended, or its writer can no longer write.
                if viewer.as_ref().is_some_and(|v| {
                    v.reader.is_finished()
                        || (v.tx.try_send(Message::Heartbeat).is_err() && v.tx.is_closed())
                }) {
                    viewer = None;
                }
                // A privacy mode held there is renewed only while a window
                // watches (PR #561 review I2): with none, it is let go at
                // once; else given up when that computer stopped answering
                // (PC8).
                if let Some(v) = view.as_mut()
                    && v.privacy.is_some()
                {
                    let watching = viewer.as_ref().filter(|w| w.session_id == v.session_id);
                    let now = Instant::now();
                    let due = v
                        .privacy
                        .as_ref()
                        .map(|hold| privacy_due(hold.on, hold.waiting, hold.sent, now));
                    if watching.is_none() {
                        v.privacy = None;
                        tracing::info!(device_id = %v.device_id, "Privacy there let go: no window watches it");
                        say_privacy(v, None, ViewPrivacy::Refused(WINDOW_GONE.to_string()));
                    } else {
                        match due {
                            Some(PrivacyDue::Renew) => {
                                if let Some(hold) = v.privacy.as_mut() {
                                    hold.renew(now);
                                }
                            }
                            Some(PrivacyDue::GiveUp) => {
                                v.privacy = None;
                                let why = format!("{} did not answer; privacy is off here.", v.name);
                                say_privacy(v, watching, ViewPrivacy::Refused(why));
                            }
                            Some(PrivacyDue::Nothing) | None => {}
                        }
                    }
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

/// A focus passed on at most this often (PR #554 review M2): the app
/// says one only after it held for 300 ms.
pub const FOCUS_GAP: Duration = Duration::from_millis(250);

/// Whether the window's focus `focused` at `now` is passed on, after `last`
/// (what was passed on, and when): only a change, and not within
/// [`FOCUS_GAP`] of the last.
pub fn focus_passes(last: Option<(bool, Instant)>, focused: bool, now: Instant) -> bool {
    last.is_none_or(|(said, at)| said != focused && now.saturating_duration_since(at) >= FOCUS_GAP)
}

/// The window showing the computer `device_id` gained or lost the focus
/// (PC3), while that computer said `control` of this one's input: the
/// clipboard follows, as [`crate::clipboard_push::window_focus_does`] says.
fn window_focus(device_id: String, focused: bool, control: Option<bool>) {
    tokio::spawn(async move {
        // config.toml is a file: read off the loop.
        let on =
            tokio::task::spawn_blocking(|| crate::events::current_config().computer_view_clipboard)
                .await
                .unwrap_or(false);
        let sync = crate::clipboard_push::with_computers();
        use crate::clipboard_push::WindowFocus;
        match crate::clipboard_push::window_focus_does(focused, on, sync, control) {
            Some(WindowFocus::Push) => crate::clipboard_push::push_for_window(&device_id).await,
            Some(WindowFocus::AskBack) => crate::clipboard_push::ask_back(&device_id).await,
            None => {}
        }
    });
}

/// The view's privacy now stands as `privacy` (PC8): its window is told,
/// when it changed.
fn say_privacy(view: &mut View, viewer: Option<&Viewer>, privacy: ViewPrivacy) {
    if view.status.privacy == privacy {
        return;
    }
    answer_privacy(view, viewer, privacy);
}

/// The view's privacy now stands as `privacy`, said to its window whether
/// or not it changed (PR #561 review M5): the window's own ask showed it
/// asked or off at once, and hears how it really stands.
fn answer_privacy(view: &mut View, viewer: Option<&Viewer>, privacy: ViewPrivacy) {
    view.status.privacy = privacy;
    if let Some(w) = viewer.filter(|w| w.session_id == view.session_id) {
        let _ = w.tx.try_send(Message::Status(view.status.clone()));
    }
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
/// the input it may send reach the other computer, and its focus the
/// relay's loop (`focus`, PC3).
fn start_viewer(
    stream: TcpStream,
    session_id: u64,
    device_id: String,
    input: mpsc::Sender<InputPacket>,
    focus: mpsc::Sender<(u64, bool)>,
    privacy: mpsc::Sender<(u64, String, u8)>,
) -> Viewer {
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
                wire::FOCUS if len == 1 => {
                    let mut focused = [0u8; 1];
                    if read.read_exact(&mut focused).await.is_err() || focused[0] > 1 {
                        break;
                    }
                    let _ = focus.try_send((session_id, focused[0] == 1));
                }
                // PC8: one byte, show, hide or lock; anything else ends it.
                wire::PRIVACY if len == 1 => {
                    let mut ask = [0u8; 1];
                    if read.read_exact(&mut ask).await.is_err() || ask[0] > wire::PRIVACY_LOCK {
                        break;
                    }
                    // Waited for, not dropped: a lock as the window closes
                    // must reach the relay.
                    if privacy
                        .send((session_id, device_id.clone(), ask[0]))
                        .await
                        .is_err()
                    {
                        break;
                    }
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

    /// PC5: a keyframe's stream is HEVC by its video parameter set, with
    /// its parameter sets as its mark (new ones: a new config); H.264 by
    /// its SPS's size; neither, nothing.
    #[test]
    fn a_keyframe_says_its_codec_and_shape() {
        let Some(Shape::Hevc(sets)) = stream_shape(&HEVC_SAMPLE) else {
            panic!("HEVC");
        };
        assert_eq!(&sets[..2], [0x40, 0x01], "the VPS first");
        assert!(sets.len() > 40 && sets.len() < HEVC_SAMPLE.len());
        // The same sets with another picture: the same shape.
        let mut again = HEVC_SAMPLE.to_vec();
        *again.last_mut().unwrap() ^= 0x01;
        assert_eq!(stream_shape(&again), Some(Shape::Hevc(sets.clone())));
        // Another SPS: another shape.
        let mut other = HEVC_SAMPLE.to_vec();
        other[45] ^= 0x10;
        assert_ne!(stream_shape(&other), Some(Shape::Hevc(sets)));
        // H.264: a 64x32 keyframe from libx264, SPS, PPS and slice.
        let h264 = [
            0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0xc0, 0x0a, 0xdc, 0x45, 0xb0, 0x11, 0x00, 0x00,
            0x03, 0x00, 0x01, 0x00, 0x00, 0x03, 0x00, 0x02, 0x0f, 0x12, 0x27, 0x80, 0x00, 0x00,
            0x00, 0x01, 0x68, 0xce, 0x0f, 0x2c, 0x80, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x04,
            0xbc, 0x98, 0xa0, 0x00, 0x20, 0x2f, 0x27, 0x27, 0x27, 0x5d, 0x75, 0xd7, 0x80,
        ];
        let shape = stream_shape(&h264);
        assert_eq!(shape, Some(Shape::H264(64, 32)));
        assert_eq!(shape.as_ref().map(Shape::codec), Some("H.264"));
        assert_eq!(stream_shape(&[0, 0, 0, 1, 0x65, 0x88]), None);
        assert_eq!(Shape::Hevc(Vec::new()).codec(), "HEVC");
        assert_eq!(Shape::Hevc(Vec::new()).message(), Message::HevcConfig);
        assert_eq!(
            Shape::H264(640, 360).message(),
            Message::Config {
                width: 640,
                height: 360
            }
        );
    }

    /// A one-picture HEVC stream, 16 by 16 grey (libx265, 91 bytes).
    const HEVC_SAMPLE: [u8; 91] = [
        0x00, 0x00, 0x00, 0x01, 0x40, 0x01, 0x0c, 0x01, 0xff, 0xff, 0x04, 0x08, 0x00, 0x00, 0x03,
        0x00, 0x9f, 0xa8, 0x00, 0x00, 0x03, 0x00, 0x00, 0x1e, 0xba, 0x02, 0x40, 0x00, 0x00, 0x00,
        0x01, 0x42, 0x01, 0x01, 0x04, 0x08, 0x00, 0x00, 0x03, 0x00, 0x9f, 0xa8, 0x00, 0x00, 0x03,
        0x00, 0x00, 0x1e, 0xa0, 0x88, 0x45, 0x96, 0xea, 0xaf, 0x2b, 0xc0, 0x5a, 0x02, 0x00, 0x00,
        0x03, 0x00, 0x02, 0x00, 0x00, 0x03, 0x00, 0x02, 0x10, 0x00, 0x00, 0x00, 0x01, 0x44, 0x01,
        0xc1, 0x73, 0xc0, 0x89, 0x00, 0x00, 0x01, 0x28, 0x01, 0xaf, 0x78, 0xf8, 0x5d, 0xf7, 0xe1,
        0x70,
    ];

    /// PC5, PR #562 review I3: HEVC is offered only when the app asked for
    /// it (its own ffmpeg decodes it) and the setting here is on; an app
    /// before PC5 never asks.
    #[test]
    fn hevc_is_offered_only_when_the_app_asks_and_the_setting_lets_it() {
        let hevc = pocketdesk_core::streaming::client::CODEC_CAP_HEVC;
        assert_eq!(codec_caps(true, true), Some(hevc));
        assert_eq!(codec_caps(true, false), None, "turned off here");
        assert_eq!(codec_caps(false, true), None, "the app did not ask");
        let older: pocketdesk_core::local::computers::ComputerRequest = serde_json::from_str(
            r#"{"id":"a","ask":"view","device_id":"desk","monitor":0,"created_unix":1}"#,
        )
        .unwrap();
        let pocketdesk_core::local::computers::ComputerAsk::View { hevc: asked, .. } = older.ask
        else {
            panic!("a view");
        };
        assert_eq!(codec_caps(asked, true), None, "an app before PC5");
    }

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
            mine: false,
            wake: None,
            viewed_unix: 0,
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
        let (focus, _) = mpsc::channel(1);
        let (privacy, _) = mpsc::channel(1);
        let _viewer = start_viewer(relay_end, 1, "desk".into(), input, focus, privacy);
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

    /// PC3: the window's focus reaches the relay's loop, named with its
    /// view; a focus byte that is neither 0 nor 1 ends the window's reader.
    #[tokio::test]
    async fn the_windows_focus_reaches_the_relay_with_its_view() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let mut window = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (relay_end, _) = listener.accept().await.unwrap();
        let (input, _queued) = mpsc::channel(4);
        let (focus, mut focused) = mpsc::channel(4);
        let (privacy, _) = mpsc::channel(4);
        let mut viewer = start_viewer(relay_end, 7, "desk".into(), input, focus, privacy);
        for message in [Message::Focus(true), Message::Focus(false)] {
            window.write_all(&message.encode()).await.unwrap();
        }
        let wait = Duration::from_secs(2);
        let first = tokio::time::timeout(wait, focused.recv()).await.unwrap();
        let second = tokio::time::timeout(wait, focused.recv()).await.unwrap();
        assert_eq!((first, second), (Some((7, true)), Some((7, false))));
        window
            .write_all(&[wire::FOCUS, 1, 0, 0, 0, 2])
            .await
            .unwrap();
        tokio::time::timeout(wait, &mut viewer.reader)
            .await
            .unwrap()
            .unwrap();
    }

    /// PC8: the window's privacy asks reach the relay's loop, named with
    /// their view and computer; a byte past lock ends the window's reader.
    #[tokio::test]
    async fn the_windows_privacy_asks_reach_the_relay_with_its_view() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let mut window = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (relay_end, _) = listener.accept().await.unwrap();
        let (input, _queued) = mpsc::channel(4);
        let (focus, _focused) = mpsc::channel(4);
        let (privacy, mut asked) = mpsc::channel(4);
        let mut viewer = start_viewer(relay_end, 9, "desk".into(), input, focus, privacy);
        for ask in [wire::PRIVACY_HIDE, wire::PRIVACY_SHOW, wire::PRIVACY_LOCK] {
            window
                .write_all(&Message::Privacy(ask).encode())
                .await
                .unwrap();
        }
        let wait = Duration::from_secs(2);
        let mut got = Vec::new();
        for _ in 0..3 {
            got.push(tokio::time::timeout(wait, asked.recv()).await.unwrap());
        }
        let desk = || "desk".to_string();
        assert_eq!(
            got,
            [
                Some((9, desk(), wire::PRIVACY_HIDE)),
                Some((9, desk(), wire::PRIVACY_SHOW)),
                Some((9, desk(), wire::PRIVACY_LOCK)),
            ]
        );
        window
            .write_all(&[wire::PRIVACY, 1, 0, 0, 0, 3])
            .await
            .unwrap();
        tokio::time::timeout(wait, &mut viewer.reader)
            .await
            .unwrap()
            .unwrap();
    }

    fn controlled() -> ViewStatus {
        ViewStatus {
            phase: Phase::Showing,
            control: Some(true),
            ..ViewStatus::default()
        }
    }

    /// PC8: privacy is asked only of a computer shown and controlled; the
    /// window's off lets it go, and a lock goes with a release or alone.
    #[test]
    fn privacy_is_asked_only_while_the_window_controls() {
        let shown = controlled();
        assert_eq!(
            privacy_step(wire::PRIVACY_HIDE, false, &shown, "Desk"),
            PrivacyStep::Grab
        );
        assert_eq!(
            privacy_step(wire::PRIVACY_HIDE, true, &shown, "Desk"),
            PrivacyStep::Nothing,
            "held already"
        );
        let view_only = ViewStatus {
            control: Some(false),
            ..controlled()
        };
        let PrivacyStep::Refuse(why) = privacy_step(wire::PRIVACY_HIDE, false, &view_only, "Desk")
        else {
            panic!("refused without control");
        };
        assert!(why.contains("clicks and keys"), "{why}");
        for not_shown in [
            ViewStatus {
                phase: Phase::Waiting,
                ..controlled()
            },
            ViewStatus {
                reconnecting: 2,
                ..controlled()
            },
        ] {
            assert!(matches!(
                privacy_step(wire::PRIVACY_HIDE, false, &not_shown, "Desk"),
                PrivacyStep::Refuse(_)
            ));
        }
        assert_eq!(
            privacy_step(wire::PRIVACY_SHOW, true, &shown, "Desk"),
            PrivacyStep::Release { lock: false }
        );
        assert_eq!(
            privacy_step(wire::PRIVACY_SHOW, false, &shown, "Desk"),
            PrivacyStep::Nothing
        );
        assert_eq!(
            privacy_step(wire::PRIVACY_LOCK, true, &view_only, "Desk"),
            PrivacyStep::Release { lock: true }
        );
        assert_eq!(
            privacy_step(wire::PRIVACY_LOCK, false, &view_only, "Desk"),
            PrivacyStep::Lock,
            "a lock as the window closes, with or without control"
        );
    }

    /// PC8: the packets are the phone's own privacy packet, a renewal the
    /// only addition.
    #[test]
    fn privacy_asks_the_phones_own_packet() {
        let body = |step| privacy_packet(&step).map(|p| p.body);
        assert_eq!(
            body(PrivacyStep::Grab),
            Some(serde_json::json!({"action": "grab"}))
        );
        assert_eq!(
            body(PrivacyStep::Release { lock: false }),
            Some(serde_json::json!({"action": "release"}))
        );
        assert_eq!(
            body(PrivacyStep::Release { lock: true }),
            Some(serde_json::json!({"action": "release"})),
            "the lock goes as its own status (review I3)"
        );
        assert_eq!(
            body(PrivacyStep::Lock),
            Some(serde_json::json!({"action": "status", "lock": true}))
        );
        assert_eq!(body(PrivacyStep::Nothing), None);
        assert_eq!(body(PrivacyStep::Refuse("no".into())), None);
        let packet = privacy_packet(&PrivacyStep::Grab).unwrap();
        assert_eq!(packet.packet_type, crate::plugins::privacy::PRIVACY);
        assert_eq!(
            renewal().body,
            serde_json::json!({"action": "grab", "renew": true})
        );
    }

    /// PR #561 review I3: a lock's answer says whether it was done, and
    /// why not; an answer about something else says nothing of it.
    #[test]
    fn a_locks_answer_says_whether_it_was_done() {
        use serde_json::json;
        assert_eq!(
            lock_heard(&json!({"ok": true, "locked": "loginctl"})),
            Some(Ok(()))
        );
        assert_eq!(
            lock_heard(&json!({"ok": true, "lockError": "no screen locker found"})),
            Some(Err("no screen locker found".into()))
        );
        assert_eq!(
            lock_heard(&json!({"ok": false, "error": "it is off there"})),
            Some(Err("it is off there".into()))
        );
        assert_eq!(lock_heard(&json!({"ok": true, "grabbed": 2})), None);
        assert_eq!(
            lock_failed_words("Desk", "it did not answer."),
            "Couldn't lock Desk: it did not answer."
        );
    }

    /// PR #561 review M4: only a computer asked has its answers taken; a
    /// phone (never asked) is dropped before the relay's channel.
    #[test]
    fn only_a_computer_asked_is_heard() {
        assert!(!was_asked("pc8-test-phone"));
        remember_asked("pc8-test-desk");
        remember_asked("pc8-test-desk");
        assert!(was_asked("pc8-test-desk"));
        assert!(!was_asked("pc8-test-phone"));
        assert_eq!(
            ASKED
                .lock()
                .unwrap()
                .iter()
                .filter(|d| *d == "pc8-test-desk")
                .count(),
            1,
            "kept once"
        );
    }

    /// PR #561 re-review N3: a lock goes only while its answer is awaited;
    /// any other ask goes however late.
    #[test]
    fn a_lock_late_is_not_sent() {
        let at = Instant::now();
        let lock = privacy_packet(&PrivacyStep::Lock).unwrap();
        let release = privacy_packet(&PrivacyStep::Release { lock: false }).unwrap();
        assert!(still_goes(&lock, at, at));
        assert!(!still_goes(&lock, at, at + LOCK_SEND_BY));
        assert!(still_goes(&release, at, at + LOCK_SEND_BY * 4));
        assert!(LOCK_SEND_BY < LOCK_WAIT, "never after the owner was told");
    }

    /// PR #561 re-review N3: the asks to one computer go in order, from one
    /// queue, even when one takes longer to send than the next; a lock
    /// asked too long ago does not go.
    #[tokio::test]
    async fn a_computers_asks_keep_their_order() {
        let sent = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let to = sent.clone();
        let queue = privacy_queue("desk", move |_device: String, packet: NetworkPacket| {
            let to = to.clone();
            async move {
                let action = packet.body["action"].as_str().unwrap().to_string();
                // The first is slow to send: a later one must not pass it.
                if action == "grab" {
                    tokio::time::sleep(Duration::from_millis(80)).await;
                }
                to.lock().unwrap().push(action);
                Ok(())
            }
        });
        let now = Instant::now();
        for step in [PrivacyStep::Grab, PrivacyStep::Release { lock: false }] {
            queue.send((privacy_packet(&step).unwrap(), now)).unwrap();
        }
        let stale = now
            .checked_sub(LOCK_SEND_BY)
            .expect("a clock that far back");
        queue
            .send((privacy_packet(&PrivacyStep::Lock).unwrap(), stale))
            .unwrap();
        queue
            .send((privacy_packet(&PrivacyStep::Lock).unwrap(), now))
            .unwrap();
        for _ in 0..50 {
            if sent.lock().unwrap().len() >= 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(*sent.lock().unwrap(), ["grab", "release", "status"]);
    }

    /// PR #561 review M3: an answer is matched to its ask by the id the
    /// other computer says again; one before the review says none.
    #[test]
    fn an_answer_is_matched_to_its_ask() {
        use serde_json::json;
        assert!(answers_ask(&json!({"ok": true, "id": 7}), 7));
        assert!(
            !answers_ask(&json!({"ok": false, "id": 6}), 7),
            "an older ask's"
        );
        assert!(
            answers_ask(&json!({"ok": true}), 7),
            "a host that says none"
        );
        assert!(!answers_ask(&json!({"ok": true, "id": "7"}), 7));
        let packet = tagged(privacy_packet(&PrivacyStep::Grab).unwrap(), 9);
        assert_eq!(packet.body, json!({"action": "grab", "id": 9}));
        assert_ne!(next_ask_id(), next_ask_id());
    }

    /// PC8: a grab's answer holds it, a refusal says why, and a status, a
    /// release or a lock says nothing of it.
    #[test]
    fn an_answer_says_whether_privacy_is_held() {
        use serde_json::json;
        assert_eq!(
            privacy_heard(&json!({"ok": true, "grabbed": 2})),
            Some(Ok(()))
        );
        assert_eq!(
            privacy_heard(&json!({"ok": false, "error": "privacy mode ended there"})),
            Some(Err("privacy mode ended there".into()))
        );
        assert_eq!(
            privacy_heard(&json!({"ok": false})),
            Some(Err("it refused".into()))
        );
        assert_eq!(
            privacy_heard(&json!({"ok": true, "grabbed": 0, "ttlLeft": null})),
            None,
            "a status"
        );
        assert_eq!(privacy_heard(&json!({"ok": true, "released": 2})), None);
        assert_eq!(
            privacy_heard(&json!({"ok": true, "released": 2, "locked": "LockWorkStation"})),
            None
        );
        assert_eq!(privacy_heard(&json!({"action": "grab"})), None);
    }

    /// PC8: an ask unanswered past the wait is given up; one held is
    /// renewed each PRIVACY_RENEW.
    #[test]
    fn privacy_held_is_renewed_and_given_up_unanswered() {
        let at = Instant::now();
        let after = |d| at + d;
        assert_eq!(
            privacy_due(false, true, at, after(Duration::from_secs(1))),
            PrivacyDue::Nothing
        );
        assert_eq!(
            privacy_due(false, true, at, after(PRIVACY_WAIT)),
            PrivacyDue::GiveUp
        );
        assert_eq!(
            privacy_due(true, true, at, after(PRIVACY_WAIT)),
            PrivacyDue::GiveUp,
            "a renewal unanswered"
        );
        assert_eq!(
            privacy_due(
                true,
                false,
                at,
                after(PRIVACY_RENEW - Duration::from_millis(1))
            ),
            PrivacyDue::Nothing
        );
        assert_eq!(
            privacy_due(true, false, at, after(PRIVACY_RENEW)),
            PrivacyDue::Renew
        );
        assert!(PRIVACY_RENEW < crate::privacy::GRAB_TTL);
    }

    /// PC8: privacy ends as the view drops or control goes, never left on
    /// there with nobody driving.
    #[test]
    fn privacy_ends_as_the_view_drops_or_control_goes() {
        assert_eq!(privacy_ends(&controlled(), "Desk"), None);
        let dropped = ViewStatus {
            reconnecting: 1,
            ..controlled()
        };
        assert!(privacy_ends(&dropped, "Desk").unwrap().contains("dropped"));
        let connecting = ViewStatus {
            phase: Phase::Connecting,
            ..controlled()
        };
        assert!(privacy_ends(&connecting, "Desk").is_some());
        let taken = ViewStatus {
            control: Some(false),
            ..controlled()
        };
        assert!(privacy_ends(&taken, "Desk").unwrap().contains("no longer"));
    }

    /// PC8: a hold let go, dropped or refused sends one release at most,
    /// and a lock only when asked.
    #[test]
    fn a_hold_lets_go_once_however_it_ends() {
        let sent = Arc::new(std::sync::Mutex::new(
            Vec::<(String, serde_json::Value)>::new(),
        ));
        let to = sent.clone();
        // Each ask names itself; the id is checked apart.
        let send: PrivacySend = Arc::new(move |id: &str, p: NetworkPacket| {
            let mut body = p.body;
            assert!(body["id"].as_u64().is_some(), "{body}");
            body.as_object_mut().unwrap().remove("id");
            to.lock().unwrap().push((id.to_string(), body));
        });
        let taken = || std::mem::take(&mut *sent.lock().unwrap());
        let now = Instant::now();
        drop(PrivacyHold::ask("desk", send.clone(), now));
        assert_eq!(
            taken(),
            [
                ("desk".into(), serde_json::json!({"action": "grab"})),
                ("desk".into(), serde_json::json!({"action": "release"})),
            ],
            "dropped: let go there"
        );
        PrivacyHold::ask("desk", send.clone(), now).let_go();
        assert_eq!(
            taken()[1],
            ("desk".into(), serde_json::json!({"action": "release"})),
            "let go once"
        );
        let mut refused = PrivacyHold::ask("desk", send.clone(), now);
        refused.done = true;
        drop(refused);
        assert_eq!(taken().len(), 1, "refused there: nothing to let go");
        let mut held = PrivacyHold::ask("desk", send, now);
        held.renew(now);
        assert!(held.waiting);
        held.done = true;
        assert_eq!(taken()[1].1, renewal().body);
    }

    /// PR #554 review M2: the relay passes a focus on only when it changed,
    /// and not sooner than FOCUS_GAP after the last it passed on.
    #[test]
    fn a_focus_goes_on_only_as_it_changes_and_not_too_often() {
        let t = Instant::now();
        assert!(focus_passes(None, true, t), "the first");
        assert!(
            !focus_passes(Some((true, t)), true, t + FOCUS_GAP * 4),
            "the same again"
        );
        assert!(
            !focus_passes(Some((true, t)), false, t + FOCUS_GAP / 2),
            "too soon"
        );
        assert!(focus_passes(Some((true, t)), false, t + FOCUS_GAP));
        assert!(focus_passes(Some((false, t)), true, t + FOCUS_GAP));
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

    /// PC2: the home network wins as soon as it comes up; the internet road
    /// up first is held for a home address still trying, which wins if it
    /// comes within the grace, else the internet road does; a road alone
    /// wins as it comes; every road failing is a loss.
    #[test]
    fn the_race_prefers_the_home_network_within_the_grace() {
        let t = Instant::now();
        let ms = |n: u64| t + Duration::from_millis(n);
        let grace = Duration::from_millis(300);
        // Home first: at once.
        let mut race = Race::new(vec![true, false], grace);
        assert_eq!(race.verdict(t), Verdict::Wait(None));
        assert_eq!(race.up(0, ms(50)), Verdict::Take(0));
        assert_eq!(race.up(1, ms(60)), Verdict::Take(0), "the first stays won");
        // The internet first, home within the grace: home.
        let mut race = Race::new(vec![true, false], grace);
        assert_eq!(race.up(1, ms(0)), Verdict::Wait(Some(ms(300))));
        assert_eq!(race.up(0, ms(200)), Verdict::Take(0));
        // The internet first, home too late: the internet.
        let mut race = Race::new(vec![true, false], grace);
        race.up(1, ms(0));
        assert_eq!(race.verdict(ms(299)), Verdict::Wait(Some(ms(300))));
        assert_eq!(race.verdict(ms(300)), Verdict::Take(1));
        // The internet first, the home addresses failing meanwhile: the
        // internet once the last has.
        let mut race = Race::new(vec![true, true, false], grace);
        race.up(2, ms(0));
        assert_eq!(
            race.failed(0, ms(10)),
            Verdict::Wait(Some(ms(300))),
            "another home address still tries"
        );
        assert_eq!(race.failed(1, ms(20)), Verdict::Take(2));
        // Home failing first: the internet as it comes, however late.
        let mut race = Race::new(vec![true, false], grace);
        assert_eq!(race.failed(0, ms(0)), Verdict::Wait(None));
        assert_eq!(race.up(1, ms(4000)), Verdict::Take(1));
        // Every road failing, or none to dial.
        let mut race = Race::new(vec![true, false], grace);
        race.failed(1, ms(0));
        assert_eq!(race.failed(0, ms(1)), Verdict::Lost);
        assert_eq!(Race::new(vec![], grace).verdict(t), Verdict::Lost);
        // Dials that ended without a word: what came up wins, else lost.
        let mut race = Race::new(vec![true, false], grace);
        race.up(1, ms(0));
        assert_eq!(race.all_done(ms(5)), Verdict::Take(1));
        assert_eq!(Race::new(vec![true], grace).all_done(ms(0)), Verdict::Lost);
    }

    /// PC2: the race over real tasks. The home network within the grace
    /// wins and the internet road that came up first is closed; the
    /// internet road wins at the grace's end over a home dial still trying,
    /// which is cancelled; every road failing says why each did.
    #[tokio::test]
    async fn the_race_closes_and_cancels_the_roads_that_lost() {
        fn after(ms: u64, road: &'static str) -> RoadDial<&'static str> {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                Ok(road)
            })
        }
        let grace = Duration::from_millis(300);
        let closed = std::sync::Mutex::new(Vec::new());
        let won = race(
            vec![(true, after(100, "home")), (false, after(0, "internet"))],
            grace,
            |lost| closed.lock().unwrap().push(lost),
        )
        .await
        .unwrap();
        assert_eq!(won, (0, "home"));
        assert_eq!(*closed.lock().unwrap(), ["internet"]);

        struct Cancelled(Arc<std::sync::atomic::AtomicBool>);
        impl Drop for Cancelled {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let guard = Cancelled(cancelled.clone());
        let slow_home: RoadDial<&'static str> = Box::pin(async move {
            let _guard = guard;
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok("home")
        });
        let started = Instant::now();
        let won = race(
            vec![(true, slow_home), (false, after(0, "internet"))],
            grace,
            |_| panic!("nothing came up to lose"),
        )
        .await
        .unwrap();
        assert_eq!(won, (1, "internet"));
        let took = started.elapsed();
        assert!(
            took >= Duration::from_millis(250) && took < Duration::from_secs(5),
            "{took:?}"
        );
        assert!(
            cancelled.load(std::sync::atomic::Ordering::SeqCst),
            "the home dial was cancelled"
        );

        let fail = |why: &'static str| -> RoadDial<&'static str> {
            Box::pin(async move { Err(why.to_string()) })
        };
        let mut why = race(
            vec![
                (true, fail("192.0.2.10: no answer")),
                (false, fail("internet: no endpoint")),
            ],
            grace,
            |_| {},
        )
        .await
        .unwrap_err();
        why.sort();
        assert_eq!(why, ["192.0.2.10: no answer", "internet: no endpoint"]);
    }

    /// PC2: after a drop the tries wait 1, 2, 4 and 8 s, then 15 s each.
    #[test]
    fn the_tries_after_a_drop_back_off_to_fifteen_seconds() {
        let waits: Vec<u64> = (1..=8).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(waits, [1, 2, 4, 8, 15, 15, 15, 15]);
        assert_eq!(backoff(0), Duration::from_secs(1));
        assert_eq!(backoff(u32::MAX), Duration::from_secs(15));
    }

    /// PC2: the tries go on for about two minutes from the drop, then stop;
    /// a view shown again for a while starts a later drop's tries afresh,
    /// one that drops at once goes on with the tries it was on.
    #[test]
    fn the_tries_stop_after_two_minutes_and_start_afresh_once_shown() {
        let start = Instant::now();
        let at = |secs: u64| start + Duration::from_secs(secs);
        let mut tries = Reconnect::default();
        assert_eq!(
            tries.failed(at(0)),
            None,
            "a failure with no drop is not a try"
        );
        assert_eq!(tries.dropped(at(0)), Some((1, Duration::from_secs(1))));
        assert_eq!(tries.failed(at(1)), Some((2, Duration::from_secs(2))));
        assert_eq!(tries.failed(at(3)), Some((3, Duration::from_secs(4))));
        assert_eq!(tries.failed(at(7)), Some((4, Duration::from_secs(8))));
        assert_eq!(tries.failed(at(15)), Some((5, Duration::from_secs(15))));
        assert_eq!(tries.failed(at(104)), Some((6, Duration::from_secs(15))));
        assert_eq!(tries.failed(at(106)), None, "past two minutes");

        // Shown again, then dropped at once: the same tries go on.
        let mut tries = Reconnect::default();
        tries.dropped(at(0));
        tries.failed(at(1));
        tries.shown(at(3));
        assert_eq!(tries.dropped(at(5)), Some((3, Duration::from_secs(4))));
        // Shown for long enough: a later drop starts afresh.
        tries.shown(at(10));
        assert_eq!(tries.dropped(at(200)), Some((1, Duration::from_secs(1))));
    }

    /// PC2: a view the other computer ended on purpose (its End, a refusal,
    /// an unpairing, a permission it does not give) is not dialled again;
    /// a link gone, a server stopping or restarting, the encoder rebuilt or
    /// permissions changed are.
    #[test]
    fn only_a_view_ended_on_purpose_is_left_ended() {
        for kept in [
            Some("kicked by user"),
            Some("session refused"),
            Some("not paired"),
            Some("input not allowed"),
            Some("Kicked by user"),
        ] {
            assert!(!may_reconnect(kept), "{kept:?}");
        }
        for again in [
            None,
            Some(""),
            Some("session ended"),
            Some("encoder failed"),
            Some("permissions changed"),
            Some("timed out"),
            Some("done"),
        ] {
            assert!(may_reconnect(again), "{again:?}");
        }
        assert_eq!(
            ended_words("Desk", true, Some("kicked by user")),
            "Desk ended the view (kicked by user)."
        );
    }

    /// PC2: a new session after a reconnect hears the window's quality and
    /// view-only again, the last of each.
    #[test]
    fn the_windows_choices_are_said_again_to_a_new_session() {
        let mut sticky = Sticky::default();
        assert!(sticky.replay().is_empty());
        for packet in [
            InputPacket::QualityPreset { preset: 1 },
            InputPacket::KeyEvent {
                key: 30,
                pressed: true,
            },
            InputPacket::ViewOnly { enabled: true },
            InputPacket::QualityPreset { preset: 3 },
        ] {
            sticky.note(&packet);
        }
        assert_eq!(
            sticky
                .replay()
                .iter()
                .map(|p| format!("{p:?}"))
                .collect::<Vec<_>>(),
            ["QualityPreset { preset: 3 }", "ViewOnly { enabled: true }"]
        );
    }

    /// A connection that keeps the input packets written to it.
    struct Kept(Arc<std::sync::Mutex<Vec<InputPacket>>>);

    struct KeptStream(Arc<std::sync::Mutex<Vec<InputPacket>>>, Vec<u8>);

    #[async_trait::async_trait]
    impl pocketdesk_core::streaming::OutStream for KeptStream {
        async fn write_all(
            &mut self,
            data: &[u8],
        ) -> Result<(), pocketdesk_core::streaming::ConnectionError> {
            self.1.extend_from_slice(data);
            Ok(())
        }
        fn finish(&mut self) -> Result<(), pocketdesk_core::streaming::ConnectionError> {
            let packet = InputPacket::decode(&self.1).expect("an input packet");
            self.0.lock().unwrap().push(packet);
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl pocketdesk_core::streaming::Connection for Kept {
        async fn open_uni(
            &self,
        ) -> Result<
            Box<dyn pocketdesk_core::streaming::OutStream>,
            pocketdesk_core::streaming::ConnectionError,
        > {
            Ok(Box::new(KeptStream(self.0.clone(), Vec::new())))
        }
        async fn accept_uni(
            &self,
        ) -> Result<
            Box<dyn pocketdesk_core::streaming::InStream>,
            pocketdesk_core::streaming::ConnectionError,
        > {
            Err(pocketdesk_core::streaming::ConnectionError::Closed)
        }
        fn remote_address(&self) -> SocketAddr {
            "192.0.2.10:4716".parse().unwrap()
        }
        fn stats(&self) -> pocketdesk_core::streaming::ConnectionStats {
            pocketdesk_core::streaming::ConnectionStats::default()
        }
        fn transport_family(&self) -> pocketdesk_core::streaming::TransportFamily {
            pocketdesk_core::streaming::TransportFamily::Quinn
        }
        fn close(&self, _code: u32, _reason: &[u8]) {}
    }

    /// PR #551 review M7 and M5: what the window sends before a session
    /// lets it in goes nowhere (no burst once it is let in), except its
    /// choices, which are said first to the session that lets it in; and a
    /// choice made as it is let in comes after them, never overtaken.
    #[tokio::test]
    async fn input_waits_for_a_session_that_let_it_in_and_its_choices_go_first() {
        let kept = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (input, rx) = mpsc::channel(16);
        let (conn_now, conn_rx) = tokio::sync::watch::channel(None);
        let _forward = AbortOnDrop(tokio::spawn(forward_input(rx, conn_rx)));
        for packet in [
            InputPacket::QualityPreset { preset: 3 },
            InputPacket::KeyEvent {
                key: 30,
                pressed: true,
            },
            InputPacket::ViewOnly { enabled: true },
        ] {
            input.send(packet).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            kept.lock().unwrap().is_empty(),
            "nothing before it is let in"
        );
        let conn: SharedConnection = Arc::new(Kept(kept.clone()));
        conn_now.send_replace(Some(conn));
        input
            .send(InputPacket::ViewOnly { enabled: false })
            .await
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while kept.lock().unwrap().len() < 3 {
            assert!(Instant::now() < deadline, "{:?}", kept.lock().unwrap());
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            kept.lock()
                .unwrap()
                .iter()
                .map(|p| format!("{p:?}"))
                .collect::<Vec<_>>(),
            [
                "QualityPreset { preset: 3 }",
                "ViewOnly { enabled: true }",
                "ViewOnly { enabled: false }"
            ]
        );
    }

    /// PC2: the window's stats from the frames that came: per second, and
    /// the frames lost across a rebuilt encoder's new run.
    #[test]
    fn frames_are_counted_for_the_stats() {
        let mut counts = FrameCounts::default();
        assert_eq!(counts.lost(), 0, "nothing came yet");
        for seq in [0, 1, 2, 4, 5] {
            counts.came(seq, 1_000);
        }
        assert_eq!(counts.lost(), 1, "frame 3 never came");
        // 5 frames of 1000 bytes in half a second: 10 a second, 80 kb/s.
        assert_eq!(counts.second(Duration::from_millis(500)), (10, 80));
        assert_eq!(
            counts.second(Duration::from_secs(1)),
            (0, 0),
            "a new second"
        );
        // A run long enough, then a rebuilt encoder counting from 0: the
        // old run's loss is kept.
        for seq in 6..=40 {
            counts.came(seq, 10);
        }
        for seq in [0, 1, 3] {
            counts.came(seq, 10);
        }
        assert_eq!(counts.lost(), 2);
        assert_eq!(counts.second(Duration::ZERO), (0, 0), "no time, no rate");
    }

    /// PR #551 review M6: frame 1 before frame 0 at the start is no loss;
    /// a frame of the old run that comes after a rebuilt encoder's new one
    /// is late, not a gap the size of the old numbers.
    #[test]
    fn frames_out_of_order_at_a_run_s_start_or_end_are_not_lost() {
        let mut counts = FrameCounts::default();
        for seq in [1, 0, 2, 3] {
            counts.came(seq, 10);
        }
        assert_eq!(counts.lost(), 0, "0 came after 1");
        for seq in 4..=100 {
            counts.came(seq, 10);
        }
        // Rebuilt: 0, 1, then a straggler of the old run.
        for seq in [0, 1, 99, 2] {
            counts.came(seq, 10);
        }
        assert_eq!(counts.lost(), 0);
        assert_eq!(counts.late, 1, "the straggler");
    }

    #[test]
    fn an_end_says_whether_the_computer_let_this_one_in() {
        // Said only for an end the other computer chose (a link gone is
        // dialled again, PR #551 review M9).
        assert_eq!(
            ended_words("Desk", true, Some("kicked by user")),
            "Desk ended the view (kicked by user)."
        );
        assert_eq!(ended_words("Desk", true, None), "Desk ended the view.");
        let refused = ended_words("Desk", false, Some("session refused"));
        assert!(refused.starts_with("Desk did not let this computer in (session refused)"));
        assert!(ended_words("Desk", false, None).contains("(it closed)"));
    }
}
