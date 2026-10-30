//! What the windows that show another device's screen share: a phone's
//! cast (T10k) and another computer's screen (BK7). An `ffmpeg` sidecar
//! decodes H.264 to RGBA pictures of a size fixed when it starts (for a
//! computer's screen, PC5: on the graphics card where it can, and to NV12
//! for a shader to draw, [`crate::nv12`]); a gate
//! feeds it from a keyframe on; a picture is letterboxed in the window, and
//! a point on it is a fraction of it; a recording copies the stream into an
//! MKV, and a screenshot is a PNG.

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use pocketdesk_core::streaming::ffmpeg_flags::fps_mode_flag;

/// An access unit delimiter: ffmpeg's parser emits a frame once the next
/// one begins, and this begins one at once.
pub const AUD: [u8; 6] = [0, 0, 0, 1, 0x09, 0xF0];

/// The largest picture decoded: 8K, 8192 by 4352 (H.264's largest level),
/// either way round.
const MAX_SIDE: usize = 8192;
const MAX_SIDE_SHORT: usize = 4352;

/// What a window says when its decoder would not start, showing `what`
/// ("a phone's screen"): a picture it refuses, or no ffmpeg.
pub fn decoder_words(e: &io::Error, what: &str) -> String {
    if e.kind() == io::ErrorKind::InvalidData {
        format!("Cannot show {what}: {e}.")
    } else {
        format!("Showing {what} needs ffmpeg on this computer ({e}).")
    }
}

/// `unix` as a name's date and time, UTC: "2026-10-21 01-02-03" (L5).
pub fn stamp(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    let secs = unix % 86_400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}-{:02}-{:02}",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

/// The user's own Pictures or Videos folder (`name`), with Pocketdesk in
/// it: where the system says it is (L5 review: OneDrive's moved folders,
/// localized XDG ones), else under the home folder.
pub fn user_folder(name: &str) -> PathBuf {
    let known = match name {
        "Pictures" => dirs::picture_dir(),
        "Videos" => dirs::video_dir(),
        _ => None,
    };
    known
        .or_else(|| dirs::home_dir().map(|home| home.join(name)))
        .unwrap_or_else(std::env::temp_dir)
        .join("Pocketdesk")
}

/// `dir`/`stem`.`ext`, or the next free "`stem` (2)".
pub fn free_name(dir: &std::path::Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}).{ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A recording: the cast's H.264 copied, not encoded again, into an MKV by
/// ffmpeg, with the wall clock's timestamps (the cast's frame rate varies),
/// from a keyframe on (L5).
pub struct Recorder {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    /// Nothing is written before a keyframe: the file would open grey.
    started: bool,
}

impl Recorder {
    pub fn start(path: &std::path::Path) -> io::Result<Self> {
        let mut command = Command::new("ffmpeg");
        command
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-use_wallclock_as_timestamps",
                "1",
                "-fflags",
                "+genpts",
                "-f",
                "h264",
                "-i",
                "pipe:0",
                "-c",
                "copy",
                "-y",
            ])
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn()?;
        let stdin = child.stdin.take();
        Ok(Self {
            child,
            stdin,
            started: false,
        })
    }

    /// Write one access unit, once a keyframe has come.
    pub fn feed(&mut self, keyframe: bool, access_unit: &[u8]) -> io::Result<()> {
        self.started |= keyframe;
        if !self.started {
            return Ok(());
        }
        match self.stdin.as_mut() {
            Some(stdin) => stdin.write_all(access_unit),
            None => Ok(()),
        }
    }
}

impl Drop for Recorder {
    /// The file is closed: ffmpeg finishes it once its input ends, given
    /// three seconds, then it is stopped (L5 review: a stalled disk must not
    /// hold the window's worker for good).
    fn drop(&mut self) {
        drop(self.stdin.take());
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if !matches!(self.child.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The point `pos` as fractions of `picture` (C8), 0 to 65535 on each
/// axis; `None` outside it, unless `clamp` (a drag that left the picture
/// keeps to its edge).
pub fn fraction(picture: egui::Rect, pos: egui::Pos2, clamp: bool) -> Option<(u16, u16)> {
    if picture.width() <= 0.0 || picture.height() <= 0.0 {
        return None;
    }
    if !clamp && !picture.contains(pos) {
        return None;
    }
    let fx = ((pos.x - picture.left()) / picture.width()).clamp(0.0, 1.0);
    let fy = ((pos.y - picture.top()) / picture.height()).clamp(0.0, 1.0);
    Some(((fx * 65535.0).round() as u16, (fy * 65535.0).round() as u16))
}

/// Where a `w`×`h` picture goes in `area`: as large as fits, centred.
pub fn letterbox(area: egui::Rect, w: usize, h: usize) -> egui::Rect {
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let scale = (area.width() / w).min(area.height() / h);
    egui::Rect::from_center_size(area.center(), egui::vec2(w * scale, h * scale))
}

/// What to do with a frame after a decoder (re)started, or after a gap.
#[derive(Debug, PartialEq, Eq)]
pub enum Admit {
    Feed,
    Skip,
    /// Skip it, and ask the phone for a keyframe (once).
    SkipAndAsk,
}

/// A decoder starts from a keyframe: deltas before one would only draw
/// garbage. So does one after a gap in the sequence (a frame dropped on
/// the way, by the phone or the relay), until the next keyframe.
#[derive(Debug, Default)]
pub struct KeyGate {
    fed: bool,
    asked: bool,
    last_seq: Option<u64>,
}

impl KeyGate {
    pub fn admit(&mut self, keyframe: bool, seq: u64) -> Admit {
        let gap = self
            .last_seq
            .is_some_and(|last| seq > last.saturating_add(1));
        self.last_seq = Some(seq);
        if gap && !keyframe {
            self.fed = false;
            self.asked = false;
        }
        if keyframe || self.fed {
            self.fed = true;
            return Admit::Feed;
        }
        if self.asked {
            Admit::Skip
        } else {
            self.asked = true;
            Admit::SkipAndAsk
        }
    }
}

/// The codec a stream comes in (PC5: HEVC beside H.264).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
}

impl Codec {
    /// ffmpeg's demuxer for the raw stream.
    fn demuxer(self) -> &'static str {
        match self {
            Codec::H264 => "h264",
            Codec::Hevc => "hevc",
        }
    }

    /// An access unit delimiter of this codec: ffmpeg's parser emits a
    /// frame once the next one begins, and this begins one at once.
    fn delimiter(self) -> &'static [u8] {
        match self {
            Codec::H264 => &AUD,
            Codec::Hevc => &AUD_HEVC,
        }
    }
}

/// HEVC's access unit delimiter (type 35, any slice type).
const AUD_HEVC: [u8; 7] = [0, 0, 0, 1, 0x46, 0x01, 0x50];

/// How the sidecar decodes (PC5): on the graphics card through Direct3D
/// 11 or VA-API (at a render node), whichever ffmpeg finds, or on the
/// processor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Accel {
    D3d11va,
    Vaapi(String),
    Auto,
    Software,
}

impl Accel {
    /// The overlay's word for it ("d3d11va", "vaapi (renderD129)").
    pub fn words(&self) -> String {
        match self {
            Accel::D3d11va => "d3d11va".to_string(),
            Accel::Vaapi(node) => {
                let name = node.rsplit('/').next().unwrap_or(node);
                format!("vaapi ({name})")
            }
            Accel::Auto => "auto".to_string(),
            Accel::Software => "software".to_string(),
        }
    }
}

/// The ways to try in order (PC5): Direct3D 11 on Windows; VA-API on Linux
/// at each render node present (`present`, in the order the encoder tries
/// them), or only at `pinned` (`POCKETDESK_VAAPI_DEVICE`); then whatever
/// ffmpeg finds; then the processor. Only the processor with `hardware`
/// off.
pub fn accel_chain(
    hardware: bool,
    windows: bool,
    linux: bool,
    pinned: Option<&str>,
    present: &[String],
) -> Vec<Accel> {
    let mut chain = Vec::new();
    if hardware {
        if windows {
            chain.push(Accel::D3d11va);
        }
        if linux {
            match pinned.map(str::trim).filter(|p| !p.is_empty()) {
                Some(node) => chain.push(Accel::Vaapi(node.to_string())),
                None => chain.extend(render_nodes(present).into_iter().map(Accel::Vaapi)),
            }
        }
        chain.push(Accel::Auto);
    }
    chain.push(Accel::Software);
    chain
}

/// The render nodes in `present` (paths under `/dev/dri`), in the
/// encoder's order: `renderD128` up, an odd name after.
fn render_nodes(present: &[String]) -> Vec<String> {
    let name = |p: &String| p.rsplit('/').next().unwrap_or("").to_string();
    let number = |p: &String| {
        name(p)
            .strip_prefix("renderD")
            .and_then(|n| n.parse::<u32>().ok())
    };
    let mut nodes: Vec<String> = present
        .iter()
        .filter(|p| name(p).starts_with("renderD"))
        .cloned()
        .collect();
    nodes.sort_by_key(|p| (number(p).unwrap_or(u32::MAX), p.clone()));
    nodes.dedup();
    nodes
}

/// The render nodes on this computer, as paths (none off Linux).
pub fn present_render_nodes() -> Vec<String> {
    std::fs::read_dir("/dev/dri")
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// This computer's ways to decode, in order, `hardware` or not (PC5).
pub fn local_accel_chain(hardware: bool) -> Vec<Accel> {
    let pinned = std::env::var("POCKETDESK_VAAPI_DEVICE").ok();
    accel_chain(
        hardware,
        cfg!(windows),
        cfg!(target_os = "linux"),
        pinned.as_deref(),
        &present_render_nodes(),
    )
}

/// How a decoded picture's samples are laid out (PC5): RGBA, four bytes a
/// pixel, or NV12, a byte of luma a pixel then a half-size plane of
/// interleaved chroma (12 bits a pixel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Planes {
    Rgba,
    Nv12,
}

impl Planes {
    fn pix_fmt(self) -> &'static str {
        match self {
            Planes::Rgba => "rgba",
            Planes::Nv12 => "nv12",
        }
    }
}

/// What a YUV picture's samples mean (PC5): BT.709's matrix or BT.601's,
/// and the full range or the limited one (16 to 235). A stream that says
/// nothing is BT.601 and limited, as ffmpeg reads it, and as the encoders
/// here make it (ffmpeg's BGRA to YUV conversion, signalling nothing).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Colour {
    pub bt709: bool,
    pub full: bool,
}

/// One decoded picture.
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub planes: Planes,
    pub data: Vec<u8>,
    pub colour: Colour,
}

impl Picture {
    /// The picture as RGBA, for a screenshot or a window that cannot draw
    /// NV12.
    pub fn to_rgba(&self) -> Vec<u8> {
        match self.planes {
            Planes::Rgba => self.data.clone(),
            Planes::Nv12 => crate::nv12::to_rgba(
                &crate::nv12::Layout::new(self.width, self.height),
                &self.data,
                self.colour,
            ),
        }
    }
}

/// The bytes one picture takes, `width` by `height`, laid out as `planes`.
pub fn picture_len(planes: Planes, width: usize, height: usize) -> usize {
    match planes {
        Planes::Rgba => width * height * 4,
        Planes::Nv12 => crate::nv12::Layout::new(width, height).len(),
    }
}

/// What a decoder is asked for (PC5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeWanted {
    pub codec: Codec,
    /// The picture's size when the stream's SPS said it; `None` takes it
    /// from the first decoded picture (HEVC).
    pub size: Option<(u16, u16)>,
    pub accel: Accel,
    pub planes: Planes,
}

/// Why a size is not one to decode: empty, or past H.264's largest (BK7
/// review I1: each picture's memory is asked for before ffmpeg says a
/// word).
fn size_problem(w: usize, h: usize) -> Option<io::Error> {
    if w == 0 || h == 0 {
        return Some(io::Error::new(
            io::ErrorKind::InvalidData,
            "an empty screen",
        ));
    }
    if w > MAX_SIDE || h > MAX_SIDE || w * h > MAX_SIDE * MAX_SIDE_SHORT {
        return Some(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a picture too large to show ({w}x{h})"),
        ));
    }
    None
}

/// The sidecar's arguments for `wanted` (PC5). Each flag here removes a
/// frame of delay ffmpeg would otherwise add (measured with ffmpeg 8):
/// `nobuffer` is left out, as it drops the packets read while probing (the
/// keyframe), and the rawvideo encoder holds a frame unless it runs on one
/// thread. A card's way asks for the card's own frames and downloads them,
/// so a card that cannot decode stops ffmpeg rather than leaving it to
/// decode on the processor unsaid; `auto` cannot, so its log says which it
/// took. The log is verbose for the lines read: the size, the colours and
/// `auto`'s way.
pub fn decoder_args(wanted: &DecodeWanted, fps_mode: &str) -> Vec<String> {
    let mut args: Vec<String> = ["-hide_banner", "-loglevel", "verbose", "-nostats"]
        .map(String::from)
        .to_vec();
    let mut filters: Vec<String> = Vec::new();
    match &wanted.accel {
        Accel::D3d11va => {
            args.extend(
                ["-hwaccel", "d3d11va", "-hwaccel_output_format", "d3d11"].map(String::from),
            );
            filters.push("hwdownload,format=nv12".to_string());
        }
        Accel::Vaapi(node) => {
            args.extend(["-hwaccel", "vaapi", "-hwaccel_device"].map(String::from));
            args.push(node.clone());
            args.extend(["-hwaccel_output_format", "vaapi"].map(String::from));
            filters.push("hwdownload,format=nv12".to_string());
        }
        Accel::Auto => args.extend(["-hwaccel", "auto"].map(String::from)),
        Accel::Software => {}
    }
    args.extend(
        [
            "-flags",
            "low_delay",
            "-probesize",
            "32",
            "-analyzeduration",
            "0",
            "-f",
            wanted.codec.demuxer(),
            "-i",
            "pipe:0",
        ]
        .map(String::from),
    );
    // Whatever the stream's crop, frames come out the size read.
    if let Some((w, h)) = wanted.size {
        filters.push(format!("scale={w}:{h}"));
    }
    if !filters.is_empty() {
        args.push("-vf".to_string());
        args.push(filters.join(","));
    }
    args.extend(
        [
            // FFmpeg 9 refuses `-vsync`, 4.4 knows nothing else.
            fps_mode,
            "passthrough",
            "-threads",
            "1",
            // A broken stretch is waited out, not given up on.
            "-max_error_rate",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            wanted.planes.pix_fmt(),
            "pipe:1",
        ]
        .map(String::from),
    );
    args
}

/// The size and colours of the pictures ffmpeg writes, from its output
/// stream's line ("Stream #0:0: Video: rawvideo (NV12 / …), nv12(tv,
/// bt709, progressive), 1920x1080 …"): said once the first picture is
/// decoded, before it is written (PC5).
pub fn output_line(line: &str) -> Option<((usize, usize), Colour)> {
    let video = line.split_once("Video: rawvideo")?.1;
    let all_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let size = video.split([',', ' ']).find_map(|word| {
        let (w, h) = word.split_once('x')?;
        if !(all_digits(w) && all_digits(h)) {
            return None;
        }
        Some((w.parse().ok()?, h.parse().ok()?))
    })?;
    let mut colour = Colour::default();
    for format in ["nv12(", "rgba("] {
        if let Some((_, rest)) = video.split_once(format) {
            let inside = rest.split(')').next().unwrap_or("");
            for word in inside.split([',', ' ', '/']) {
                match word {
                    "pc" => colour.full = true,
                    "bt709" => colour.bt709 = true,
                    _ => {}
                }
            }
        }
    }
    Some((size, colour))
}

/// The way `-hwaccel auto` took, from its line ("Using auto hwaccel type
/// dxva2 with new default device.").
pub fn auto_line(line: &str) -> Option<String> {
    let rest = line.split_once("Using auto hwaccel type ")?.1;
    rest.split_whitespace().next().map(str::to_string)
}

/// What a running sidecar has done, for the window's choice of way (PC5).
#[derive(Debug, Default)]
struct DecoderInfo {
    pictures: AtomicU64,
    ended: AtomicBool,
    auto: Mutex<Option<String>>,
}

/// The `ffmpeg` sidecar that decodes one encoder's stream.
pub struct Decoder {
    child: Child,
    stdin: ChildStdin,
    codec: Codec,
    planes: Planes,
    info: Arc<DecoderInfo>,
}

impl Decoder {
    /// A phone's cast: H.264 of a size known, decoded on the processor to
    /// RGBA, as before PC5.
    pub fn start(width: u16, height: u16, out: Box<dyn FnMut(Picture) + Send>) -> io::Result<Self> {
        Self::open(
            &DecodeWanted {
                codec: Codec::H264,
                size: Some((width, height)),
                accel: Accel::Software,
                planes: Planes::Rgba,
            },
            out,
        )
    }

    /// A sidecar for `wanted`, its pictures to `out`.
    pub fn open(
        wanted: &DecodeWanted,
        mut out: Box<dyn FnMut(Picture) + Send>,
    ) -> io::Result<Self> {
        if let Some((w, h)) = wanted.size
            && let Some(problem) = size_problem(usize::from(w), usize::from(h))
        {
            return Err(problem);
        }
        let mut command = Command::new("ffmpeg");
        command
            .args(decoder_args(wanted, fps_mode_flag("ffmpeg")))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("no stdin"))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("no stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("no stderr"))?;
        let info = Arc::new(DecoderInfo::default());
        let colour = Arc::new(Mutex::new(Colour::default()));
        let (size_tx, size_rx) = std::sync::mpsc::channel::<(usize, usize)>();
        {
            let (info, colour) = (info.clone(), colour.clone());
            std::thread::Builder::new()
                .name("video-decode-log".into())
                .spawn(move || read_log(stderr, &info, &colour, &size_tx))?;
        }
        let planes = wanted.planes;
        let known = wanted.size.map(|(w, h)| (usize::from(w), usize::from(h)));
        let thread_info = info.clone();
        std::thread::Builder::new()
            .name("video-decode".into())
            .spawn(move || {
                // The size is the stream's, or else the first picture's.
                let size = known.or_else(|| size_rx.recv().ok());
                if let Some((w, h)) = size.filter(|(w, h)| size_problem(*w, *h).is_none()) {
                    let len = picture_len(planes, w, h);
                    loop {
                        let mut data = vec![0u8; len];
                        if stdout.read_exact(&mut data).is_err() {
                            break;
                        }
                        thread_info.pictures.fetch_add(1, Ordering::Relaxed);
                        let colour = *colour.lock().unwrap_or_else(|e| e.into_inner());
                        out(Picture {
                            width: w,
                            height: h,
                            planes,
                            data,
                            colour,
                        });
                    }
                }
                thread_info.ended.store(true, Ordering::Release);
            })?;
        Ok(Self {
            child,
            stdin,
            codec: wanted.codec,
            planes: wanted.planes,
            info,
        })
    }

    /// One access unit, decoded at once.
    pub fn feed(&mut self, access_unit: &[u8]) -> io::Result<()> {
        self.stdin.write_all(access_unit)?;
        self.stdin.write_all(self.codec.delimiter())?;
        self.stdin.flush()
    }

    /// How its pictures are laid out.
    pub fn planes(&self) -> Planes {
        self.planes
    }

    /// The pictures it has given.
    pub fn pictures(&self) -> u64 {
        self.info.pictures.load(Ordering::Relaxed)
    }

    /// Whether it has stopped giving pictures (ffmpeg ended).
    pub fn ended(&self) -> bool {
        self.info.ended.load(Ordering::Acquire)
    }

    /// The way `auto` took, once ffmpeg said.
    pub fn auto_took(&self) -> Option<String> {
        self.info
            .auto
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

/// Read ffmpeg's log to its end: the output's size (sent once), its
/// colours and `auto`'s way.
fn read_log(
    stderr: std::process::ChildStderr,
    info: &DecoderInfo,
    colour: &Mutex<Colour>,
    size: &std::sync::mpsc::Sender<(usize, usize)>,
) {
    let mut reader = io::BufReader::new(stderr);
    let mut line = Vec::new();
    let mut sized = false;
    loop {
        line.clear();
        match io::BufRead::read_until(&mut reader, b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&line);
        if let Some((read, said)) = output_line(&text) {
            *colour.lock().unwrap_or_else(|e| e.into_inner()) = said;
            if !sized {
                sized = true;
                let _ = size.send(read);
            }
        } else if let Some(way) = auto_line(&text) {
            *info.auto.lock().unwrap_or_else(|e| e.into_inner()) = Some(way);
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Save `rgba`, `width` by `height`, as a PNG named `stem` (or the next free
/// name) in the Pictures folder's Pocketdesk: what the window then says.
/// One at a time picks its name, so two in a second do not take the same
/// one (L5 review).
pub fn save_png(stem: &str, width: usize, height: usize, rgba: Vec<u8>) -> String {
    static SAVING: Mutex<()> = Mutex::new(());
    let _one = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    let dir = user_folder("Pictures");
    let path = free_name(&dir, stem, "png");
    let saved = std::fs::create_dir_all(&dir)
        .map_err(|e| e.to_string())
        .and_then(|()| {
            image::RgbaImage::from_raw(width as u32, height as u32, rgba)
                .ok_or_else(|| "a picture of the wrong size".to_string())?
                .save(&path)
                .map_err(|e| e.to_string())
        });
    match saved {
        Ok(()) => format!("Saved {} and copied it.", path.display()),
        Err(e) => format!("Copied, not saved: {e}."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nv12::Layout;

    fn picture() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(100.0, 0.0), egui::vec2(200.0, 400.0))
    }

    /// L5: a recording copies the stream into an MKV of the wall clock's
    /// length, from its first keyframe. Needs ffmpeg and ffprobe; skipped
    /// without them.
    #[test]
    fn a_recording_lasts_as_long_as_the_cast() {
        let dir = std::env::temp_dir().join(format!("pd-l5-rec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("t.h264");
        let made = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
            ])
            .arg("testsrc=size=320x240:rate=10")
            .args([
                "-t",
                "2",
                "-c:v",
                "libx264",
                "-x264-params",
                "aud=1:keyint=10",
            ])
            .args(["-bsf:v", "h264_mp4toannexb", "-f", "h264"])
            .arg(&source)
            .status();
        if !made.is_ok_and(|s| s.success()) {
            eprintln!("no ffmpeg with libx264 here: skipped");
            return;
        }
        let data = std::fs::read(&source).unwrap();
        let aud: &[u8] = &[0, 0, 0, 1, 9];
        let mut starts: Vec<usize> = data
            .windows(aud.len())
            .enumerate()
            .filter(|(_, w)| *w == aud)
            .map(|(i, _)| i)
            .collect();
        starts.push(data.len());
        let units: Vec<&[u8]> = starts.windows(2).map(|w| &data[w[0]..w[1]]).collect();
        assert_eq!(units.len(), 20);
        let out = dir.join("out.mkv");
        let mut recorder = Recorder::start(&out).unwrap();
        // A delta frame before the first keyframe is left out.
        recorder.feed(false, units[1]).unwrap();
        for (i, unit) in units.iter().enumerate() {
            recorder.feed(i % 10 == 0, unit).unwrap();
            std::thread::sleep(Duration::from_millis(100));
        }
        drop(recorder);
        let probe = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration:stream=codec_name,width",
            ])
            .args(["-of", "default=nw=1"])
            .arg(&out)
            .output();
        let Ok(probe) = probe else {
            eprintln!("no ffprobe here: skipped");
            return;
        };
        let text = String::from_utf8_lossy(&probe.stdout).to_string();
        assert!(
            text.contains("codec_name=h264") && text.contains("width=320"),
            "{text}"
        );
        let duration: f64 = text
            .lines()
            .find_map(|l| l.strip_prefix("duration="))
            .and_then(|d| d.parse().ok())
            .unwrap();
        assert!(
            (1.6..2.4).contains(&duration),
            "{duration} s for 2 s of frames"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L5: names carry the date and time, UTC.
    #[test]
    fn a_name_carries_the_date() {
        assert_eq!(super::stamp(0), "1970-01-01 00-00-00");
        assert_eq!(super::stamp(1_792_070_523), "2026-10-15 13-22-03");
        assert_eq!(
            super::stamp(951_782_400),
            "2000-02-29 00-00-00",
            "a leap day"
        );
    }

    /// L5: a name taken gets the next free one.
    #[test]
    fn a_name_taken_gets_the_next() {
        let dir = std::env::temp_dir().join(format!("pd-l5-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            super::free_name(&dir, "Phone", "png"),
            dir.join("Phone.png")
        );
        std::fs::write(dir.join("Phone.png"), b"x").unwrap();
        assert_eq!(
            super::free_name(&dir, "Phone", "png"),
            dir.join("Phone (2).png")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_point_is_a_fraction_of_the_picture_and_outside_it_is_nothing() {
        assert_eq!(
            fraction(picture(), egui::pos2(100.0, 0.0), false),
            Some((0, 0))
        );
        assert_eq!(
            fraction(picture(), egui::pos2(300.0, 400.0), false),
            Some((65535, 65535))
        );
        assert_eq!(
            fraction(picture(), egui::pos2(200.0, 200.0), false),
            Some((32768, 32768))
        );
        assert_eq!(
            fraction(picture(), egui::pos2(50.0, 200.0), false),
            None,
            "the letterbox's bar"
        );
        assert_eq!(
            fraction(picture(), egui::pos2(50.0, 500.0), true),
            Some((0, 65535)),
            "a drag that left keeps to the edge"
        );
    }

    #[test]
    fn a_picture_is_letterboxed_in_the_window() {
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 400.0));
        let placed = letterbox(area, 100, 200);
        assert_eq!(placed.size(), egui::vec2(200.0, 400.0));
        assert_eq!(placed.center(), area.center());
    }

    #[test]
    fn a_decoder_starts_from_a_keyframe_and_asks_for_one_once() {
        let mut gate = KeyGate::default();
        assert_eq!(gate.admit(false, 3), Admit::SkipAndAsk);
        assert_eq!(gate.admit(false, 4), Admit::Skip);
        assert_eq!(gate.admit(true, 5), Admit::Feed);
        assert_eq!(gate.admit(false, 6), Admit::Feed, "deltas after it are fed");
    }

    #[test]
    fn a_gap_in_the_frames_waits_for_the_next_keyframe() {
        let mut gate = KeyGate::default();
        assert_eq!(gate.admit(true, 0), Admit::Feed);
        assert_eq!(gate.admit(false, 1), Admit::Feed);
        assert_eq!(
            gate.admit(false, 3),
            Admit::SkipAndAsk,
            "frame 2 was dropped"
        );
        assert_eq!(gate.admit(false, 4), Admit::Skip);
        assert_eq!(
            gate.admit(true, 9),
            Admit::Feed,
            "a keyframe after a gap is fed"
        );
        assert_eq!(gate.admit(false, 10), Admit::Feed);
    }

    /// BK7 review I1: a size past H.264's largest is refused before ffmpeg
    /// starts or a picture's memory is asked for, and says why.
    #[test]
    fn a_decoder_refuses_a_picture_too_large() {
        for (w, h) in [(8193, 100), (100, 8193), (8192, 8192), (u16::MAX, u16::MAX)] {
            let refused = Decoder::start(w, h, Box::new(|_| {}))
                .err()
                .expect("refused");
            assert_eq!(refused.kind(), io::ErrorKind::InvalidData);
            assert!(refused.to_string().contains("too large"), "{refused}");
        }
    }

    #[test]
    fn a_refused_picture_is_not_said_to_need_ffmpeg() {
        let large = io::Error::new(
            io::ErrorKind::InvalidData,
            "a picture too large to show (9000x9000)",
        );
        assert_eq!(
            decoder_words(&large, "a phone's screen"),
            "Cannot show a phone's screen: a picture too large to show (9000x9000)."
        );
        let missing = io::Error::new(io::ErrorKind::NotFound, "program not found");
        assert!(decoder_words(&missing, "a phone's screen").contains("needs ffmpeg"));
    }

    /// The sidecar turns an H.264 stream into pictures of the size read.
    /// Skipped where ffmpeg is not installed.
    #[test]
    fn the_sidecar_decodes_what_ffmpeg_encodes() {
        let encoded = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=64x128:rate=10",
                "-frames:v",
                "3",
                "-c:v",
                "libx264",
                "-bf",
                "0",
                "-f",
                "h264",
                "pipe:1",
            ])
            .stderr(Stdio::null())
            .output();
        let Ok(encoded) = encoded else {
            eprintln!("no ffmpeg here: skipped");
            return;
        };
        if !encoded.status.success() || encoded.stdout.is_empty() {
            eprintln!("this ffmpeg cannot encode H.264: skipped");
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let mut decoder = Decoder::start(
            64,
            128,
            Box::new(move |p| {
                let _ = tx.send((p.width, p.height, p.data.len()));
            }),
        )
        .expect("ffmpeg starts");
        decoder.feed(&encoded.stdout).expect("feed");
        for _ in 0..3 {
            let got = rx.recv_timeout(Duration::from_secs(10)).expect("a picture");
            assert_eq!(got, (64, 128, 64 * 128 * 4));
        }
    }

    /// An H.264 stream ffmpeg encodes from `source` (a lavfi graph), or
    /// `None` without an ffmpeg that can.
    fn encoded(source: &str, frames: u32) -> Option<Vec<u8>> {
        let out = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg(source)
            .args(["-frames:v", &frames.to_string()])
            .args(["-pix_fmt", "yuv420p", "-c:v", "libx264", "-bf", "0"])
            .args(["-f", "h264", "pipe:1"])
            .stderr(Stdio::null())
            .output()
            .ok()?;
        (out.status.success() && !out.stdout.is_empty()).then_some(out.stdout)
    }

    /// Decode `stream` as `wanted` asks: each picture's size, planes and
    /// bytes, as many as `count`, or what came in 10 s.
    fn decode(wanted: &DecodeWanted, stream: &[u8], count: usize) -> Vec<Picture> {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut decoder = Decoder::open(
            wanted,
            Box::new(move |p| {
                let _ = tx.send(p);
            }),
        )
        .expect("ffmpeg starts");
        let _ = decoder.feed(stream);
        (0..count)
            .map_while(|_| rx.recv_timeout(Duration::from_secs(10)).ok())
            .collect()
    }

    /// PC5: the arguments for each way: a card's own frames downloaded as
    /// NV12, `auto` left to ffmpeg, the size scaled to when the stream said
    /// it, and the planes asked for.
    #[test]
    fn each_way_asks_ffmpeg_for_its_own() {
        let mut wanted = DecodeWanted {
            codec: Codec::H264,
            size: Some((1920, 1080)),
            accel: Accel::Software,
            planes: Planes::Rgba,
        };
        let joined = |w: &DecodeWanted| decoder_args(w, "-fps_mode").join(" ");
        let software = joined(&wanted);
        assert!(!software.contains("-hwaccel"), "{software}");
        assert!(software.contains("-f h264 -i pipe:0 -vf scale=1920:1080 -fps_mode passthrough"));
        assert!(
            software.ends_with("-f rawvideo -pix_fmt rgba pipe:1"),
            "{software}"
        );
        wanted.accel = Accel::D3d11va;
        wanted.planes = Planes::Nv12;
        let d3d = joined(&wanted);
        assert!(
            d3d.contains("-hwaccel d3d11va -hwaccel_output_format d3d11 -flags low_delay"),
            "{d3d}"
        );
        assert!(
            d3d.contains("-vf hwdownload,format=nv12,scale=1920:1080"),
            "{d3d}"
        );
        assert!(d3d.ends_with("-pix_fmt nv12 pipe:1"), "{d3d}");
        wanted.accel = Accel::Vaapi("/dev/dri/renderD129".into());
        let vaapi = joined(&wanted);
        assert!(
            vaapi.contains(
                "-hwaccel vaapi -hwaccel_device /dev/dri/renderD129 -hwaccel_output_format vaapi"
            ),
            "{vaapi}"
        );
        assert!(vaapi.contains("-vf hwdownload,format=nv12,"), "{vaapi}");
        wanted.accel = Accel::Auto;
        wanted.size = None;
        let auto = joined(&wanted);
        assert!(auto.contains("-hwaccel auto -flags"), "{auto}");
        assert!(!auto.contains("-vf"), "no size, no scale: {auto}");
        assert!(
            auto.contains("-loglevel verbose"),
            "the lines read are verbose"
        );
    }

    /// PC5: the ways in order on each system, the VA-API nodes as the
    /// encoder tries them, the pinned node alone, and the processor alone
    /// with the setting off.
    #[test]
    fn the_ways_to_decode_go_card_first_and_end_on_the_processor() {
        let nodes: Vec<String> = [
            "/dev/dri/renderD129",
            "/dev/dri/card0",
            "/dev/dri/renderD128",
            "/dev/dri/renderD9999x",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            accel_chain(true, true, false, None, &nodes),
            [Accel::D3d11va, Accel::Auto, Accel::Software]
        );
        assert_eq!(
            accel_chain(true, false, true, None, &nodes),
            [
                Accel::Vaapi("/dev/dri/renderD128".into()),
                Accel::Vaapi("/dev/dri/renderD129".into()),
                Accel::Vaapi("/dev/dri/renderD9999x".into()),
                Accel::Auto,
                Accel::Software
            ]
        );
        assert_eq!(
            accel_chain(true, false, true, Some("/dev/dri/renderD130"), &nodes),
            [
                Accel::Vaapi("/dev/dri/renderD130".into()),
                Accel::Auto,
                Accel::Software
            ],
            "POCKETDESK_VAAPI_DEVICE pins one"
        );
        assert_eq!(
            accel_chain(true, false, true, Some(" "), &[]),
            [Accel::Auto, Accel::Software],
            "an empty pin and no node"
        );
        assert_eq!(
            accel_chain(true, false, false, None, &nodes),
            [Accel::Auto, Accel::Software]
        );
        for (windows, linux) in [(true, false), (false, true)] {
            assert_eq!(
                accel_chain(false, windows, linux, None, &nodes),
                [Accel::Software],
                "the setting off"
            );
        }
        assert_eq!(
            Accel::Vaapi("/dev/dri/renderD129".into()).words(),
            "vaapi (renderD129)"
        );
    }

    /// PC5: the output's size and colours from ffmpeg 8's own lines, and
    /// `auto`'s way.
    #[test]
    fn ffmpegs_lines_say_the_size_the_colours_and_the_way() {
        let plain = "  Stream #0:0: Video: rawvideo (NV12 / 0x3231564E), nv12(progressive), 318x181 [SAR 1:1 DAR 318:181], q=2-31, 20720 kb/s, 30 fps, 30 tbn";
        assert_eq!(output_line(plain), Some(((318, 181), Colour::default())));
        let hd = "  Stream #0:0: Video: rawvideo, 1 reference frame (NV12 / 0x3231564E), nv12(tv, bt709, progressive, left), 1920x1080 [SAR 1:1 DAR 16:9], q=2-31";
        assert_eq!(
            output_line(hd),
            Some((
                (1920, 1080),
                Colour {
                    bt709: true,
                    full: false
                }
            ))
        );
        let full = "Stream #0:0: Video: rawvideo (RGBA / 0x41424752), rgba(pc, gbr/bt709/bt709, progressive), 64x128, q=2-31";
        assert_eq!(
            output_line(full),
            Some((
                (64, 128),
                Colour {
                    bt709: true,
                    full: true
                }
            ))
        );
        assert_eq!(
            output_line("  Stream #0:0: Video: h264 (Main), yuv420p(progressive), 1920x1080"),
            None,
            "the input's line"
        );
        assert_eq!(output_line("Output #0, rawvideo, to 'pipe:1':"), None);
        assert_eq!(
            auto_line(
                "[vist#0:0/h264 @ 0000022f03885040] [dec:h264 @ 0000022f02e8db80] Using auto hwaccel type dxva2 with new default device."
            ),
            Some("dxva2".to_string())
        );
        assert_eq!(auto_line("Stream mapping:"), None);
    }

    /// PC5: NV12 out of the sidecar is a third of RGBA's bytes, and its
    /// colours turned to RGB here match ffmpeg's own RGBA within a step or
    /// two. Skipped without an ffmpeg that encodes H.264.
    #[test]
    fn nv12_from_the_sidecar_matches_its_rgba() {
        let Some(stream) = encoded("testsrc2=size=66x34:rate=10", 3) else {
            eprintln!("no ffmpeg with libx264 here: skipped");
            return;
        };
        let mut wanted = DecodeWanted {
            codec: Codec::H264,
            size: Some((66, 34)),
            accel: Accel::Software,
            planes: Planes::Rgba,
        };
        let rgba = decode(&wanted, &stream, 3);
        wanted.planes = Planes::Nv12;
        let nv12 = decode(&wanted, &stream, 3);
        assert_eq!((rgba.len(), nv12.len()), (3, 3));
        assert_eq!(rgba[0].data.len(), 66 * 34 * 4);
        assert_eq!(nv12[0].data.len(), 66 * 34 + 66 * 17, "12 bits a pixel");
        assert_eq!(nv12[0].planes, Planes::Nv12);
        let mine = nv12[2].to_rgba();
        let theirs = &rgba[2].data;
        assert_eq!(mine.len(), theirs.len());
        let mean = mine
            .iter()
            .zip(theirs)
            .map(|(a, b)| u64::from(a.abs_diff(*b)))
            .sum::<u64>() as f64
            / mine.len() as f64;
        assert!(mean < 3.0, "a mean step of {mean:.2}");
    }

    /// PC5: with no size given, the size is the first picture's, from
    /// ffmpeg's line, even an odd one.
    #[test]
    fn with_no_size_the_first_picture_says_it() {
        let Some(stream) = encoded("testsrc2=size=318x182:rate=10", 2) else {
            eprintln!("no ffmpeg with libx264 here: skipped");
            return;
        };
        let wanted = DecodeWanted {
            codec: Codec::H264,
            size: None,
            accel: Accel::Software,
            planes: Planes::Nv12,
        };
        let pictures = decode(&wanted, &stream, 2);
        assert_eq!(pictures.len(), 2);
        assert_eq!((pictures[0].width, pictures[0].height), (318, 182));
        assert_eq!(pictures[0].data.len(), Layout::new(318, 182).len());
    }

    /// PC5: Direct3D 11 decodes where the card can (a machine without one
    /// stops ffmpeg, the window's cue for the next way); `auto` decodes on
    /// whatever it finds. Skipped without ffmpeg and libx264.
    #[test]
    fn a_card_decodes_or_stops_and_auto_decodes() {
        let Some(stream) = encoded("testsrc2=size=128x72:rate=10", 3) else {
            eprintln!("no ffmpeg with libx264 here: skipped");
            return;
        };
        let mut wanted = DecodeWanted {
            codec: Codec::H264,
            size: Some((128, 72)),
            accel: Accel::Auto,
            planes: Planes::Nv12,
        };
        let auto = decode(&wanted, &stream, 3);
        assert_eq!(auto.len(), 3, "auto decodes");
        if cfg!(windows) {
            wanted.accel = Accel::D3d11va;
            let card = decode(&wanted, &stream, 3);
            if card.is_empty() {
                eprintln!("no Direct3D 11 decoder here: the window would go on");
            } else {
                assert_eq!(card.len(), 3);
                assert_eq!((card[0].width, card[0].height), (128, 72));
            }
        }
    }
}
