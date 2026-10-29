//! What the windows that show another device's screen share: a phone's
//! cast (T10k) and another computer's screen (BK7). An `ffmpeg` sidecar
//! decodes H.264 to RGBA pictures of a size fixed when it starts; a gate
//! feeds it from a keyframe on; a picture is letterboxed in the window, and
//! a point on it is a fraction of it; a recording copies the stream into an
//! MKV, and a screenshot is a PNG.

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use eframe::egui;

/// An access unit delimiter: ffmpeg's parser emits a frame once the next
/// one begins, and this begins one at once.
pub const AUD: [u8; 6] = [0, 0, 0, 1, 0x09, 0xF0];

/// One decoded picture.
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
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

/// The `ffmpeg` sidecar that decodes one encoder's stream.
pub struct Decoder {
    child: Child,
    stdin: ChildStdin,
}

impl Decoder {
    pub fn start(
        width: u16,
        height: u16,
        mut out: Box<dyn FnMut(Picture) + Send>,
    ) -> io::Result<Self> {
        let (w, h) = (usize::from(width), usize::from(height));
        if w == 0 || h == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "an empty screen",
            ));
        }
        let mut command = Command::new("ffmpeg");
        // Each flag here removes a frame of delay ffmpeg would otherwise add
        // (measured with ffmpeg 8): `nobuffer` is left out, as it drops the
        // packets read while probing (the keyframe), and the rawvideo
        // encoder holds a frame unless it runs on one thread.
        command
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
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
            ])
            // Whatever the stream's crop, frames come out the size read.
            .arg(format!("scale={w}:{h}"))
            .args([
                "-vsync",
                "passthrough",
                "-threads",
                "1",
                // A broken stretch is waited out, not given up on.
                "-max_error_rate",
                "1",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "pipe:1",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
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
        std::thread::Builder::new()
            .name("video-decode".into())
            .spawn(move || {
                loop {
                    let mut rgba = vec![0u8; w * h * 4];
                    if stdout.read_exact(&mut rgba).is_err() {
                        break;
                    }
                    out(Picture {
                        width: w,
                        height: h,
                        rgba,
                    });
                }
            })?;
        Ok(Self { child, stdin })
    }

    /// One access unit, decoded at once.
    pub fn feed(&mut self, access_unit: &[u8]) -> io::Result<()> {
        self.stdin.write_all(access_unit)?;
        self.stdin.write_all(&AUD)?;
        self.stdin.flush()
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
                let _ = tx.send((p.width, p.height, p.rgba.len()));
            }),
        )
        .expect("ffmpeg starts");
        decoder.feed(&encoded.stdout).expect("feed");
        for _ in 0..3 {
            let got = rx.recv_timeout(Duration::from_secs(10)).expect("a picture");
            assert_eq!(got, (64, 128, 64 * 128 * 4));
        }
    }
}
