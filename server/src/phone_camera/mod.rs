//! The phone's camera as this computer's camera (F6 part 2). A camera cast
//! (a cast that said [`pocketdesk_core::streaming::client::CAST_WEBCAM_MARKER`],
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
use std::time::Instant;

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

/// The picture a camera publishes: the cast's size, made even (both
/// layouts take pixel pairs), within what a camera may be.
pub fn picture_size(width: u16, height: u16) -> Option<(usize, usize)> {
    let (w, h) = (usize::from(width) & !1, usize::from(height) & !1);
    (w >= 16 && h >= 16 && w <= 4096 && h <= 4096).then_some((w, h))
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

/// A cast's H.264 decoded by `ffmpeg` into [`Latest`].
struct Decoder {
    child: Child,
    feed: Option<mpsc::SyncSender<Vec<u8>>>,
}

impl Decoder {
    fn start(width: usize, height: usize, latest: Arc<Latest>) -> io::Result<Self> {
        let program = "ffmpeg";
        let mut child = Command::new(program)
            .args(decoder_args(
                width,
                height,
                pocketdesk_core::streaming::ffmpeg_flags::fps_mode_flag(program),
            ))
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

    /// Queue an access unit; `false` when it was dropped (the queue full,
    /// or `ffmpeg` gone).
    fn feed(&self, unit: &[u8]) -> bool {
        self.feed
            .as_ref()
            .is_some_and(|feed| feed.try_send(unit.to_vec()).is_ok())
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // The feeder ends with its channel; ffmpeg is stopped either way,
        // and its reader then reads the end.
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

/// What a camera does with what its cast sends: the plan, kept pure for
/// tests.
#[derive(Debug, Default)]
pub struct Lifecycle {
    size: Option<(usize, usize)>,
    /// A keyframe is waited for: none since the decoder started, or one
    /// was lost on the way.
    waiting: bool,
    chain: CastChain,
}

/// What [`Lifecycle`] says to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing changes.
    Keep,
    /// Publish the camera (again) at this size, a new decoder with it, and
    /// ask the phone for a keyframe.
    Open(usize, usize),
    /// The size cannot be a camera: take it away.
    Close,
    /// Decode this frame.
    Decode,
    /// Drop this frame, and ask the phone for a keyframe.
    DropAndAsk,
    /// Drop this frame (a keyframe is already asked for).
    Drop,
}

impl Lifecycle {
    /// The phone said its encoder's size.
    pub fn config(&mut self, width: u16, height: u16) -> Step {
        let Some(size) = picture_size(width, height) else {
            self.size = None;
            return Step::Close;
        };
        self.chain.restart();
        self.waiting = true;
        if self.size == Some(size) {
            // The same size: a new encoder, the same camera; the decoder
            // starts again from its keyframe.
            return Step::Open(size.0, size.1);
        }
        self.size = Some(size);
        Step::Open(size.0, size.1)
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

    /// A frame could not be queued for the decoder: wait for a keyframe;
    /// whether to ask for one now.
    pub fn lost(&mut self, now: Instant) -> bool {
        self.waiting = true;
        self.chain.lost(now)
    }
}

/// One phone's camera, published while its camera cast runs.
pub struct PhoneCamera {
    name: String,
    fps: u32,
    lifecycle: Lifecycle,
    latest: Arc<Latest>,
    decoder: Option<Decoder>,
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
            #[cfg(target_os = "linux")]
            node: None,
        }
    }

    /// The cast's size: the camera published at it (again). `Ok(true)`
    /// asks the phone for a keyframe; an error takes the camera away.
    pub fn config(&mut self, width: u16, height: u16) -> io::Result<bool> {
        let Step::Open(w, h) = self.lifecycle.config(width, height) else {
            self.close();
            return Ok(false);
        };
        self.decoder = None;
        self.latest.clear();
        if let Err(e) = self.publish(w, h) {
            self.close();
            return Err(e);
        }
        match Decoder::start(w, h, self.latest.clone()) {
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

    /// One of the cast's frames; `true` asks the phone for a keyframe.
    pub fn frame(&mut self, keyframe: bool, seq: u64, data: &[u8]) -> bool {
        let now = Instant::now();
        match self.lifecycle.frame(keyframe, seq, now) {
            Step::Decode => {
                let fed = self.decoder.as_ref().is_some_and(|d| d.feed(data));
                !fed && self.lifecycle.lost(now)
            }
            Step::DropAndAsk => true,
            _ => false,
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

/// A camera cast's camera, on a thread of its own (opening PipeWire and
/// `ffmpeg` blocks): fed while the cast runs, and taken away, node and
/// all, once this is dropped.
pub struct Worker {
    feed: mpsc::SyncSender<Feed>,
}

impl Worker {
    /// The camera for the phone `device_id`'s camera cast; `want` asks the
    /// phone for a keyframe.
    pub fn start(
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
                    let ask = match item {
                        Feed::Config { width, height } => match camera.config(width, height) {
                            Ok(ask) => ask,
                            Err(e) => {
                                tracing::warn!(error = %e, "The phone's camera could not be published");
                                false
                            }
                        },
                        Feed::Frame {
                            keyframe,
                            seq,
                            data,
                        } => camera.frame(keyframe, seq, &data),
                    };
                    if ask {
                        let _ = want.try_send(());
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

/// The name the owner knows the phone `device_id` by.
fn phone_name(device_id: &str) -> Option<String> {
    let path = crate::state::trust_store_path().ok()?;
    let store = pocketdesk_core::protocol::kdeconnect::TrustStore::read(path).ok()?;
    store.name(device_id).map(str::to_string)
}

#[cfg(test)]
mod tests;
