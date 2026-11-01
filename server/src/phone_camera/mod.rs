//! The phone's camera as this computer's camera (F6 part 2). A camera cast
//! (a cast that said [`pocketdesk_core::streaming::client::CAST_WEBCAM_FLAG`],
//! admitted with the phone's Webcam permission and the owner's
//! `phone_camera` setting) is decoded here by `ffmpeg`, as the desktop
//! app's window decodes a cast, and published on Linux as a PipeWire
//! `Video/Source` node with `media.role = Camera`, "Pocketdesk camera
//! (<phone>)": apps that take cameras through PipeWire or the camera portal
//! (Firefox, Chromium, OBS, GNOME Snapshot) open it, and V4L2-only apps
//! through `pw-v4l2`. No kernel module.
//!
//! The node is this process's: it goes when the cast ends (the camera is
//! dropped), and PipeWire takes it away with the connection if the server
//! dies, so none is ever left behind.

use std::io::{self, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use pocketdesk_core::protocol::phone_camera::CameraAsk;
use pocketdesk_core::streaming::cast::CastChain;

#[cfg(target_os = "linux")]
mod node;

/// Whether this computer offers phones as a camera now: the owner's
/// setting, and only where a camera can be published (Linux).
pub fn offered() -> bool {
    cfg!(target_os = "linux") && crate::events::current_config().phone_camera
}

/// What a camera ask sent now carries, from the owner's settings; `None`
/// while phones are not offered as a camera.
pub fn ask_now() -> Option<CameraAsk> {
    if !offered() {
        return None;
    }
    let config = crate::events::current_config();
    Some(CameraAsk::new(
        &config.phone_camera_lens,
        config.phone_camera_height,
        config.phone_camera_fps,
    ))
}

/// The longest phone name a camera's name carries.
const NAME_CHARS: usize = 40;

/// The camera's name in apps: "Pocketdesk camera (<phone>)", the phone's
/// name cleaned of control and format characters and cut short.
pub fn camera_name(phone: Option<&str>) -> String {
    let phone: String = phone
        .unwrap_or("")
        .chars()
        .filter(|c| {
            !c.is_control() && !pocketdesk_core::protocol::kdeconnect::is_invisible_format(*c)
        })
        .take(NAME_CHARS)
        .collect();
    let phone = phone.trim();
    if phone.is_empty() {
        "Pocketdesk camera (phone)".to_string()
    } else {
        format!("Pocketdesk camera ({phone})")
    }
}

/// The pixel layouts the camera offers apps: NV12 first (what the decoder
/// gives), and YUY2 for apps that refuse NV12.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Nv12,
    Yuy2,
}

/// What the camera offers, in order of preference.
pub const OFFERED: [PixelFormat; 2] = [PixelFormat::Nv12, PixelFormat::Yuy2];

impl PixelFormat {
    /// One row's bytes and one picture's bytes at `width` by `height` (both
    /// even): NV12's luma rows then half as many chroma rows; YUY2's two
    /// bytes a pixel.
    pub fn layout(self, width: usize, height: usize) -> (usize, usize) {
        match self {
            PixelFormat::Nv12 => (width, width * height * 3 / 2),
            PixelFormat::Yuy2 => (width * 2, width * height * 2),
        }
    }

    /// Write `nv12`, a picture of `width` by `height`, into `out` in this
    /// layout; `false` (nothing written) when either is the wrong size.
    pub fn write(self, nv12: &[u8], width: usize, height: usize, out: &mut [u8]) -> bool {
        let (_, in_len) = PixelFormat::Nv12.layout(width, height);
        let (_, out_len) = self.layout(width, height);
        if nv12.len() != in_len || out.len() < out_len {
            return false;
        }
        match self {
            PixelFormat::Nv12 => out[..out_len].copy_from_slice(nv12),
            PixelFormat::Yuy2 => nv12_to_yuy2(nv12, width, height, &mut out[..out_len]),
        }
        true
    }
}

/// NV12 to YUY2 (Y0 U Y1 V a pixel pair): each chroma pair serves the two
/// rows it was sampled for.
fn nv12_to_yuy2(nv12: &[u8], width: usize, height: usize, out: &mut [u8]) {
    let (luma, chroma) = nv12.split_at(width * height);
    for row in 0..height {
        let y = &luma[row * width..(row + 1) * width];
        let uv = &chroma[(row / 2) * width..(row / 2 + 1) * width];
        let line = &mut out[row * width * 2..(row + 1) * width * 2];
        for pair in 0..width / 2 {
            line[pair * 4] = y[pair * 2];
            line[pair * 4 + 1] = uv[pair * 2];
            line[pair * 4 + 2] = y[pair * 2 + 1];
            line[pair * 4 + 3] = uv[pair * 2 + 1];
        }
    }
}

/// The longest and the shortest side a camera may have: 1080p, either way
/// round, and a little over for an encoder that rounds up (PR #604
/// review N1: only 720p and 1080p are asked for).
pub const MAX_SIDES: (usize, usize) = (1920 + 16, 1080 + 16);

/// The picture a camera publishes: the cast's size, made even (both
/// layouts take pixel pairs), within what a camera may be.
pub fn picture_size(width: u16, height: u16) -> Option<(usize, usize)> {
    let (w, h) = (usize::from(width) & !1, usize::from(height) & !1);
    let (long, short) = (w.max(h), w.min(h));
    (short >= 16 && long <= MAX_SIDES.0 && short <= MAX_SIDES.1).then_some((w, h))
}

/// The newest decoded picture, for the node to send when it next runs.
#[derive(Default)]
pub(crate) struct Latest {
    picture: Mutex<Option<Arc<Vec<u8>>>>,
    /// Pictures decoded so far; the node sends one each time this moves.
    count: AtomicU64,
    /// Pictures the node has handed to PipeWire.
    sent: AtomicU64,
}

impl Latest {
    fn put(&self, picture: Vec<u8>) {
        *self.picture.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(picture));
        self.count.fetch_add(1, Ordering::Release);
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) fn take(&self) -> Option<Arc<Vec<u8>>> {
        self.picture
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(crate) fn count(&self) -> u64 {
        self.count.load(Ordering::Acquire)
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) fn note_sent(&self) {
        self.sent.fetch_add(1, Ordering::Relaxed);
    }

    fn sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }

    /// Forget the picture held: a new decoder starts.
    fn clear(&self) {
        *self.picture.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// Access units waiting for `ffmpeg`, at most; more are dropped and a
/// keyframe asked for.
const FEED_QUEUE: usize = 8;

/// H.264's access unit delimiter: `ffmpeg`'s parser emits a frame once the
/// next begins, and this begins one at once (as the desktop app's window).
const AUD: [u8; 6] = [0, 0, 0, 1, 0x09, 0xF0];

/// The `ffmpeg` arguments that decode a cast's H.264 to NV12 at `width` by
/// `height`, a picture out for each in, with no frame of delay it can help.
pub fn decoder_args(width: usize, height: usize, fps_mode: &str) -> Vec<String> {
    let mut args: Vec<String> = [
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostats",
        "-flags",
        "low_delay",
        "-probesize",
        "32",
        "-analyzeduration",
        "0",
        "-f",
        "h264",
        "-i",
        "pipe:0",
        "-vf",
    ]
    .map(String::from)
    .to_vec();
    args.push(format!("scale={width}:{height}"));
    args.extend(
        [
            fps_mode,
            "passthrough",
            "-threads",
            "1",
            "-max_error_rate",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "nv12",
            "pipe:1",
        ]
        .map(String::from),
    );
    args
}

/// What became of an access unit handed to the decoder.
#[derive(Debug, PartialEq, Eq)]
enum Fed {
    Queued,
    /// The queue is full: `ffmpeg` is behind.
    Full,
    /// `ffmpeg` has stopped.
    Dead,
}

/// A cast's H.264 decoded by `ffmpeg` into [`Latest`].
struct Decoder {
    child: Child,
    feed: Option<mpsc::SyncSender<Vec<u8>>>,
}

impl Decoder {
    fn start(width: usize, height: usize, latest: Arc<Latest>) -> io::Result<Self> {
        let program = "ffmpeg";
        let args = decoder_args(
            width,
            height,
            pocketdesk_core::streaming::ffmpeg_flags::fps_mode_flag(program),
        );
        Self::spawn(program, &args, width, height, latest)
    }

    /// `program` with `args` as the decoder: H.264 in, NV12 out.
    fn spawn(
        program: &str,
        args: &[String],
        width: usize,
        height: usize,
        latest: Arc<Latest>,
    ) -> io::Result<Self> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let (stdin, stdout) = match (child.stdin.take(), child.stdout.take()) {
            (Some(stdin), Some(stdout)) => (stdin, stdout),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::other("ffmpeg has no pipes"));
            }
        };
        let (feed, units) = mpsc::sync_channel::<Vec<u8>>(FEED_QUEUE);
        let spawned = std::thread::Builder::new()
            .name("camera-feed".into())
            .spawn(move || feed_units(stdin, units))
            .and_then(|_| {
                std::thread::Builder::new()
                    .name("camera-decode".into())
                    .spawn(move || read_pictures(stdout, width, height, &latest))
            });
        if let Err(e) = spawned {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        Ok(Self {
            child,
            feed: Some(feed),
        })
    }

    /// Whether `ffmpeg` has stopped (the exit is collected as it is seen).
    fn dead(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    /// Queue an access unit.
    fn feed(&mut self, unit: &[u8]) -> Fed {
        if self.dead() {
            return Fed::Dead;
        }
        match self.feed.as_ref().map(|f| f.try_send(unit.to_vec())) {
            Some(Ok(())) => Fed::Queued,
            Some(Err(mpsc::TrySendError::Full(_))) => Fed::Full,
            _ => Fed::Dead,
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // The feeder ends with its channel; ffmpeg is stopped either way,
        // and its reader then reads the end. `wait` reaps it: never a
        // zombie.
        self.feed = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn feed_units(mut stdin: ChildStdin, units: mpsc::Receiver<Vec<u8>>) {
    for unit in units {
        if stdin
            .write_all(&unit)
            .and_then(|()| stdin.write_all(&AUD))
            .and_then(|()| stdin.flush())
            .is_err()
        {
            return;
        }
    }
}

fn read_pictures(mut stdout: impl Read, width: usize, height: usize, latest: &Latest) {
    let (_, len) = PixelFormat::Nv12.layout(width, height);
    loop {
        let mut picture = vec![0u8; len];
        if stdout.read_exact(&mut picture).is_err() {
            return;
        }
        latest.put(picture);
    }
}

/// Restarts of a decoder that stopped, at most, within [`RESTART_WINDOW`]
/// (PR #604 review I2): past them the camera ends, so a stream that kills
/// `ffmpeg` cannot have the server start it in a loop.
pub const RESTARTS: usize = 2;

/// The window [`RESTARTS`] counts in.
pub const RESTART_WINDOW: Duration = Duration::from_secs(60);

/// How long a stopped decoder waits before each restart.
pub const RESTART_BACKOFF: [Duration; RESTARTS] =
    [Duration::from_millis(500), Duration::from_secs(2)];

/// What to do about a decoder that stopped.
#[derive(Debug, PartialEq, Eq)]
pub enum Restart {
    /// Start a new one now.
    Now,
    /// Not yet: its backoff runs.
    Wait,
    /// It stopped too often: end the camera.
    GiveUp,
}

/// The decoder's restarts, kept pure for tests.
#[derive(Debug, Default)]
pub struct Restarts {
    done: Vec<Instant>,
}

impl Restarts {
    /// A decoder found stopped at `died`; what to do at `now`.
    pub fn ask(&mut self, died: Instant, now: Instant) -> Restart {
        self.done
            .retain(|t| now.saturating_duration_since(*t) < RESTART_WINDOW);
        let Some(backoff) = RESTART_BACKOFF.get(self.done.len()) else {
            return Restart::GiveUp;
        };
        if now.saturating_duration_since(died) < *backoff {
            return Restart::Wait;
        }
        self.done.push(now);
        Restart::Now
    }
}

/// Sizes said at most, within [`CONFIG_WINDOW`] (PR #604 review N1): each
/// opens the camera and the decoder again, so a phone that says more ends
/// its camera. A camera says one, and once more if its encoder restarts.
pub const CONFIGS: usize = 3;

/// The window [`CONFIGS`] counts in.
pub const CONFIG_WINDOW: Duration = Duration::from_secs(5);

/// What a camera does with what its cast sends: the plan, kept pure for
/// tests.
#[derive(Debug, Default)]
pub struct Lifecycle {
    size: Option<(usize, usize)>,
    /// A keyframe is waited for: none since the decoder started, or one
    /// was lost on the way.
    waiting: bool,
    chain: CastChain,
    /// When the last sizes were said, within [`CONFIG_WINDOW`].
    configs: Vec<Instant>,
}

/// What [`Lifecycle`] says to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Publish the camera (again) at this size, a new decoder with it, and
    /// ask the phone for a keyframe.
    Open(usize, usize),
    /// The size cannot be a camera, or sizes come too often: end it.
    Close,
    /// Decode this frame.
    Decode,
    /// Drop this frame, and ask the phone for a keyframe.
    DropAndAsk,
    /// Drop this frame (a keyframe is already asked for).
    Drop,
}

impl Lifecycle {
    /// The phone said its encoder's size at `now`.
    pub fn config(&mut self, width: u16, height: u16, now: Instant) -> Step {
        self.configs
            .retain(|t| now.saturating_duration_since(*t) < CONFIG_WINDOW);
        let size = picture_size(width, height).filter(|_| self.configs.len() < CONFIGS);
        let Some(size) = size else {
            self.size = None;
            return Step::Close;
        };
        self.configs.push(now);
        self.chain.restart();
        self.waiting = true;
        // The same size too: a new encoder, the same camera; the decoder
        // starts again from its keyframe.
        self.size = Some(size);
        Step::Open(size.0, size.1)
    }

    /// The camera's size, while it has one.
    pub fn size(&self) -> Option<(usize, usize)> {
        self.size
    }

    /// A frame arrived.
    pub fn frame(&mut self, keyframe: bool, seq: u64, now: Instant) -> Step {
        if self.size.is_none() {
            return Step::Drop;
        }
        let broke = self.chain.on_frame(keyframe, seq, now);
        if keyframe {
            self.waiting = false;
            return Step::Decode;
        }
        if broke {
            self.waiting = true;
            return Step::DropAndAsk;
        }
        if self.waiting {
            return Step::Drop;
        }
        Step::Decode
    }

    /// A frame could not be decoded: wait for a keyframe; whether to ask
    /// for one now.
    pub fn lost(&mut self, now: Instant) -> bool {
        self.waiting = true;
        self.chain.lost(now)
    }
}

/// How a camera's decoder is started: `ffmpeg`, or a stand-in in tests.
type StartDecoder = fn(usize, usize, Arc<Latest>) -> io::Result<Decoder>;

/// One phone's camera, published while its camera cast runs.
pub struct PhoneCamera {
    name: String,
    fps: u32,
    lifecycle: Lifecycle,
    latest: Arc<Latest>,
    decoder: Option<Decoder>,
    start_decoder: StartDecoder,
    /// When its decoder was found stopped, while it waits to restart.
    died: Option<Instant>,
    restarts: Restarts,
    #[cfg(target_os = "linux")]
    node: Option<node::Node>,
}

impl PhoneCamera {
    /// A camera named `name` at `fps`, published once the cast says its
    /// size.
    pub fn new(name: String, fps: u32) -> Self {
        Self {
            name,
            fps: fps.clamp(1, 60),
            lifecycle: Lifecycle::default(),
            latest: Arc::default(),
            decoder: None,
            start_decoder: Decoder::start,
            died: None,
            restarts: Restarts::default(),
            #[cfg(target_os = "linux")]
            node: None,
        }
    }

    /// The cast's size: the camera published at it (again). `Ok(true)`
    /// asks the phone for a keyframe; an error takes the camera away, and
    /// the cast should end.
    pub fn config(&mut self, width: u16, height: u16) -> io::Result<bool> {
        let Step::Open(w, h) = self.lifecycle.config(width, height, Instant::now()) else {
            self.close();
            return Err(io::Error::other(format!(
                "a camera of {width}x{height}, or sizes said too often"
            )));
        };
        self.decoder = None;
        self.died = None;
        self.latest.clear();
        if let Err(e) = self.publish(w, h) {
            self.close();
            return Err(e);
        }
        match (self.start_decoder)(w, h, self.latest.clone()) {
            Ok(decoder) => {
                self.decoder = Some(decoder);
                Ok(true)
            }
            Err(e) => {
                self.close();
                Err(e)
            }
        }
    }

    /// The node at `width` by `height`: the one there if it is that size,
    /// else a new one, the old gone first.
    #[cfg(target_os = "linux")]
    fn publish(&mut self, width: usize, height: usize) -> io::Result<()> {
        if self
            .node
            .as_ref()
            .is_some_and(|n| n.size() == (width, height))
        {
            return Ok(());
        }
        self.node = None;
        self.node = Some(node::Node::open(
            &self.name,
            width,
            height,
            self.fps,
            self.latest.clone(),
        )?);
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    fn publish(&mut self, _width: usize, _height: usize) -> io::Result<()> {
        let _ = (&self.name, self.fps);
        Err(io::Error::other(
            "a phone can be a camera on Linux only (PipeWire)",
        ))
    }

    /// One of the cast's frames; `Ok(true)` asks the phone for a keyframe,
    /// an error says the decoder kept stopping and the cast should end.
    pub fn frame(&mut self, keyframe: bool, seq: u64, data: &[u8]) -> io::Result<bool> {
        self.frame_at(keyframe, seq, data, Instant::now())
    }

    fn frame_at(
        &mut self,
        keyframe: bool,
        seq: u64,
        data: &[u8],
        now: Instant,
    ) -> io::Result<bool> {
        match self.lifecycle.frame(keyframe, seq, now) {
            Step::Decode => match self.decoder.as_mut().map(|d| d.feed(data)) {
                Some(Fed::Queued) => Ok(false),
                Some(Fed::Full) => Ok(self.lifecycle.lost(now)),
                Some(Fed::Dead) | None => self.revive(now),
            },
            Step::DropAndAsk => Ok(true),
            _ => {
                // Dropped frames still notice a decoder that stopped.
                if self.decoder.as_mut().is_some_and(Decoder::dead) {
                    return self.revive(now);
                }
                Ok(false)
            }
        }
    }

    /// The decoder stopped: collect it, and start another after its
    /// backoff, from the next keyframe; past [`RESTARTS`], end.
    fn revive(&mut self, now: Instant) -> io::Result<bool> {
        let Some((width, height)) = self.lifecycle.size() else {
            return Ok(false);
        };
        if self.decoder.take().is_some() {
            tracing::warn!("The phone's camera decoder stopped");
        }
        let died = *self.died.get_or_insert(now);
        match self.restarts.ask(died, now) {
            Restart::Wait => {
                self.lifecycle.lost(now);
                Ok(false)
            }
            Restart::GiveUp => {
                self.close();
                Err(io::Error::other(
                    "the camera's decoder (ffmpeg) kept stopping",
                ))
            }
            Restart::Now => {
                self.died = None;
                let decoder = (self.start_decoder)(width, height, self.latest.clone());
                match decoder {
                    Ok(decoder) => {
                        tracing::info!("The phone's camera decoder started again");
                        self.decoder = Some(decoder);
                        self.lifecycle.lost(now);
                        Ok(true)
                    }
                    Err(e) => {
                        self.close();
                        Err(e)
                    }
                }
            }
        }
    }

    /// Pictures decoded, and pictures handed to PipeWire, so far.
    pub fn counts(&self) -> (u64, u64) {
        (self.latest.count(), self.latest.sent())
    }

    /// The camera's PipeWire node id, once published.
    pub fn node_id(&self) -> Option<u32> {
        #[cfg(target_os = "linux")]
        {
            self.node.as_ref().and_then(node::Node::id)
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }

    fn close(&mut self) {
        self.decoder = None;
        #[cfg(target_os = "linux")]
        {
            self.node = None;
        }
    }
}

impl Drop for PhoneCamera {
    fn drop(&mut self) {
        self.close();
    }
}

/// What a camera cast sends its camera.
#[derive(Debug)]
pub enum Feed {
    /// The phone's encoder size.
    Config { width: u16, height: u16 },
    /// One of its frames.
    Frame {
        keyframe: bool,
        seq: u64,
        data: Vec<u8>,
    },
}

/// What waits for the camera's thread, at most; past this a frame is
/// dropped, and the chain's gap asks for a keyframe.
const WORKER_QUEUE: usize = 16;

/// What a camera cast is closed with when its camera fails here.
pub const CAMERA_FAILED: &[u8] = b"phone camera failed";

/// A camera cast's camera, on a thread of its own (opening PipeWire and
/// `ffmpeg` blocks): fed while the cast runs, and taken away, node and
/// all, once this is dropped.
pub struct Worker {
    feed: mpsc::SyncSender<Feed>,
}

impl Worker {
    /// The camera for the phone `device_id`'s camera cast, the live session
    /// `session_id`; `want` asks the phone for a keyframe. A camera that
    /// fails ends its session, and says so.
    pub fn start(
        session_id: u64,
        device_id: Option<String>,
        want: tokio::sync::mpsc::Sender<()>,
    ) -> io::Result<Self> {
        let (feed, fed) = mpsc::sync_channel::<Feed>(WORKER_QUEUE);
        std::thread::Builder::new()
            .name("phone-camera".into())
            .spawn(move || {
                let name = camera_name(device_id.as_deref().and_then(phone_name).as_deref());
                let fps = ask_now().unwrap_or_default().fps;
                let mut camera = PhoneCamera::new(name, u32::from(fps));
                for item in fed {
                    let done = match item {
                        Feed::Config { width, height } => camera.config(width, height),
                        Feed::Frame {
                            keyframe,
                            seq,
                            data,
                        } => camera.frame(keyframe, seq, &data),
                    };
                    match done {
                        Ok(true) => {
                            let _ = want.try_send(());
                        }
                        Ok(false) => {}
                        Err(e) => {
                            tracing::warn!(error = %e, "The phone's camera stopped");
                            fail(session_id, &e);
                            break;
                        }
                    }
                }
                let (decoded, sent) = camera.counts();
                tracing::info!(decoded, sent, "The phone's camera cast ended");
            })?;
        Ok(Self { feed })
    }

    /// Hand the camera what the cast sent; `false` when it was dropped.
    pub fn send(&self, item: Feed) -> bool {
        self.feed.try_send(item).is_ok()
    }
}

/// The camera of session `session_id` failed: its session ends, telling
/// the phone (which closes its camera), and the owner is told here.
fn fail(session_id: u64, why: &io::Error) {
    let _ = pocketdesk_core::streaming::sessions::end_session(session_id, CAMERA_FAILED);
    let body = format!("The phone's camera was taken away: {why}.");
    if !crate::platform::notify(
        "Pocketdesk: the phone's camera stopped",
        &body,
        Duration::from_secs(10),
    ) {
        tracing::info!("{body}");
    }
}

/// The name the owner knows the phone `device_id` by.
fn phone_name(device_id: &str) -> Option<String> {
    let path = crate::state::trust_store_path().ok()?;
    let store = pocketdesk_core::protocol::kdeconnect::TrustStore::read(path).ok()?;
    store.name(device_id).map(str::to_string)
}

#[cfg(test)]
mod tests;
