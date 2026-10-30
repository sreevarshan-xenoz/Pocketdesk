//! "Send my mic" in the window that shows another computer's screen
//! (PC10): this computer's microphone goes to that computer, where it
//! becomes its "Pocketdesk Mic", as a phone's microphone does (R4 E2).
//!
//! While the window controls that computer and the switch on its bar is
//! on, a thread reads the microphone ([`MicSource`]: WASAPI on Windows,
//! `pw-record`, else `parec`, on Linux), cuts what it reads into 20 ms
//! frames of 48 kHz mono ([`Framer`]), encodes each with Opus (core's
//! encoder at the phone's 32 kb/s) and sends it as an [`InputPacket::Mic`]
//! through the view relay, after the start packet a phone sends. The
//! window tells the relay first (view message `MIC`); the relay passes the
//! frames on only while the window's last word on that connection is "on",
//! and as it says off, or the connection ends, the relay sends the other
//! computer the stop itself. That computer plays them only where its
//! Devices page lets this one use its microphone, as for a phone.
//!
//! It stops as the window closes, turns View only, stops controlling (the
//! view ending, dropping into reconnecting, or Control turned off there),
//! and as the microphone fails. Not as the window loses the focus: a call on
//! the other computer goes on while this one is used for something else,
//! and the bar keeps saying "Mic on".
//!
//! The decisions ([`MicSwitch`], [`Framer`], [`pump`]) are pure and tested
//! here with a fake source; no test opens a real microphone (one ignored
//! test does, behind `POCKETDESK_MIC_TEST_MACHINE=1`).

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pocketdesk_core::streaming::audio::{MIC_CHANNELS, MIC_SAMPLE_RATE};
use pocketdesk_core::streaming::{AudioConfig, AudioEncoder, InputPacket};

/// The frames' length: the phone's, which the other computer's relay takes.
pub const FRAME_MS: u32 = 20;
/// Samples in a frame: 960 at 48 kHz mono.
pub const FRAME_SAMPLES: usize = (MIC_SAMPLE_RATE * FRAME_MS / 1000) as usize;
/// Opus's bitrate, the phone's.
pub const BITRATE: u32 = 32_000;

/// What the bar says while the microphone goes to the other computer.
pub const ON_AIR: &str = "Mic on";

/// Whether this system has a way to read the microphone here.
pub fn available() -> bool {
    cfg!(any(windows, target_os = "linux"))
}

/// The switch on the bar across the window's passes: whether it is wanted,
/// and whether the microphone goes now.
#[derive(Debug, Default)]
pub struct MicSwitch {
    wanted: bool,
    on: bool,
}

/// What the window does with the microphone this pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Start,
    Stop,
}

impl MicSwitch {
    /// The bar's switch.
    pub fn toggle(&mut self) {
        self.wanted = !self.wanted;
    }

    /// Off now, and not wanted (PR #569 review I1): the window closing, or
    /// the microphone stopping by itself; the caller stops the capture.
    pub fn stopped(&mut self) {
        self.wanted = false;
        self.on = false;
    }

    /// Whether the bar's switch shows on.
    pub fn wanted(&self) -> bool {
        self.wanted || self.on
    }

    /// Whether the microphone goes to the other computer now.
    pub fn on(&self) -> bool {
        self.on
    }

    /// The switch while the window `controls` the other computer (Control
    /// on there, View only off, connected, the view neither ended nor
    /// reconnecting): on as it is wanted and the window controls, off as it
    /// is not wanted or the window no longer controls, and then not wanted
    /// any more (control coming back never turns the microphone on again
    /// by itself).
    pub fn step(&mut self, controls: bool) -> Option<Change> {
        if !controls {
            self.wanted = false;
        }
        match (self.wanted, self.on) {
            (true, false) => {
                self.on = true;
                Some(Change::Start)
            }
            (false, true) => {
                self.on = false;
                Some(Change::Stop)
            }
            _ => None,
        }
    }
}

/// Cuts the microphone's samples, as they come, into whole frames.
#[derive(Debug, Default)]
pub struct Framer {
    pending: Vec<i16>,
}

impl Framer {
    /// `pcm` added; each whole frame of [`FRAME_SAMPLES`] now complete, the
    /// rest kept for the next.
    pub fn push(&mut self, pcm: &[i16]) -> Vec<Vec<i16>> {
        self.pending.extend_from_slice(pcm);
        let whole = self.pending.len() / FRAME_SAMPLES * FRAME_SAMPLES;
        self.pending
            .drain(..whole)
            .collect::<Vec<_>>()
            .chunks(FRAME_SAMPLES)
            .map(<[i16]>::to_vec)
            .collect()
    }

    /// Samples kept for the next frame.
    #[cfg(test)]
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
}

/// The start a phone sends first: the other computer makes its
/// "Pocketdesk Mic".
pub fn start_packet() -> InputPacket {
    InputPacket::Mic {
        enabled: true,
        opus: Vec::new(),
    }
}

/// One encoded frame.
pub fn frame_packet(opus: Vec<u8>) -> InputPacket {
    InputPacket::Mic {
        enabled: true,
        opus,
    }
}

/// Where the microphone's samples come from: 48 kHz mono, as they come.
pub trait MicSource {
    /// The next samples (none after a short wait is fine); an error once
    /// the microphone is gone.
    fn read(&mut self) -> io::Result<Vec<i16>>;

    /// How to end it at once from another thread, so a read blocked on it
    /// returns (PR #569 review M4: a stalled recorder never keeps the
    /// thread or its process). None: a stop waits for the next read.
    fn stopper(&self) -> Option<Stopper> {
        None
    }
}

/// Ends a [`MicSource`] from another thread.
pub type Stopper = Box<dyn FnOnce() + Send>;

/// How the microphone stopped going.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    /// The window stopped it.
    Stopped,
    /// The relay could not take a packet (the window's connection went, or
    /// the switch went off meanwhile).
    Refused,
    /// The microphone could not be opened, failed, or its frames could not
    /// be encoded; why, in words.
    Failed(String),
}

/// An Opus encoder for one frame: its bytes, or why not.
pub type Encode<'a> = dyn FnMut(&[i16]) -> Result<Vec<u8>, String> + 'a;

/// Read `source` until `stop`, sending the start and then each frame
/// `encode`d, until `send` takes no more or the source fails.
pub fn pump(
    source: &mut dyn MicSource,
    encode: &mut Encode<'_>,
    send: &mut dyn FnMut(InputPacket) -> bool,
    stop: &AtomicBool,
) -> Ended {
    if stop.load(Ordering::Acquire) {
        return Ended::Stopped;
    }
    if !send(start_packet()) {
        return Ended::Refused;
    }
    let mut framer = Framer::default();
    while !stop.load(Ordering::Acquire) {
        let pcm = match source.read() {
            Ok(pcm) => pcm,
            Err(e) => return Ended::Failed(e.to_string()),
        };
        for frame in framer.push(&pcm) {
            if stop.load(Ordering::Acquire) {
                return Ended::Stopped;
            }
            let opus = match encode(&frame) {
                Ok(opus) => opus,
                Err(why) => return Ended::Failed(why),
            };
            if !send(frame_packet(opus)) {
                return Ended::Refused;
            }
        }
    }
    Ended::Stopped
}

/// Core's Opus encoder as the phone's: 48 kHz mono, 20 ms, 32 kb/s.
pub fn opus_encoder() -> Result<impl FnMut(&[i16]) -> Result<Vec<u8>, String>, String> {
    let mut encoder = AudioEncoder::new(AudioConfig {
        sample_rate: MIC_SAMPLE_RATE,
        channels: MIC_CHANNELS,
        bitrate_bps: BITRATE,
        frame_duration_ms: FRAME_MS,
    })
    .map_err(|e| format!("{e:#}"))?;
    Ok(move |pcm: &[i16]| match encoder.encode(pcm) {
        Ok(Some(packet)) => Ok(packet.data),
        Ok(None) => Ok(Vec::new()),
        Err(e) => Err(format!("{e:#}")),
    })
}

/// The microphone going, on its own thread; dropped, it stops (the thread
/// ends at its next read, sending nothing more).
pub struct MicRun {
    stop: Arc<AtomicBool>,
    /// The source's [`Stopper`], once it is open (taken and called as the
    /// run is dropped).
    stopper: Arc<std::sync::Mutex<Option<Stopper>>>,
}

fn slot(stopper: &std::sync::Mutex<Option<Stopper>>) -> std::sync::MutexGuard<'_, Option<Stopper>> {
    stopper
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl MicRun {
    /// Open the microphone with `open` on a thread of its own, and pump
    /// it into `send`; `ended` hears how it stopped. An error when no
    /// thread could be started.
    pub fn start(
        open: impl FnOnce() -> io::Result<Box<dyn MicSource>> + Send + 'static,
        mut send: impl FnMut(InputPacket) -> bool + Send + 'static,
        ended: impl FnOnce(Ended) + Send + 'static,
    ) -> io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let stopper = Arc::new(std::sync::Mutex::new(None));
        let (stopping, stopper_slot) = (stop.clone(), stopper.clone());
        std::thread::Builder::new()
            .name("view-mic".into())
            .spawn(move || {
                let how = (|| {
                    let mut encode = opus_encoder().map_err(Ended::Failed)?;
                    let mut source = open().map_err(|e| Ended::Failed(e.to_string()))?;
                    // Its stopper is kept for the drop; a drop that came
                    // while it opened ends it now. The slot's lock orders
                    // the two, so neither misses the other.
                    if let Some(stop_it) = source.stopper() {
                        let mut kept = slot(&stopper_slot);
                        if stopping.load(Ordering::Acquire) {
                            drop(kept);
                            stop_it();
                        } else {
                            *kept = Some(stop_it);
                        }
                    }
                    Ok::<_, Ended>(pump(&mut *source, &mut encode, &mut send, &stopping))
                })();
                ended(how.unwrap_or_else(|how| how));
            })?;
        Ok(Self { stop, stopper })
    }
}

impl Drop for MicRun {
    fn drop(&mut self) {
        let mut kept = slot(&self.stopper);
        self.stop.store(true, Ordering::Release);
        let stop_it = kept.take();
        drop(kept);
        if let Some(stop_it) = stop_it {
            stop_it();
        }
    }
}

/// What the bar says as the microphone stopped because the window's
/// connection to the view went (PR #569 review I1): it is not heard there.
pub const CUT_WORDS: &str =
    "Mic stopped: the connection to the view was cut. Turn Send my mic on again.";

/// What the bar says as the microphone stopped because the other computer
/// stopped taking this one's input (PR #569 re-review N4).
pub const TAKEN_WORDS: &str = "Mic stopped: the other computer took control back. Turn Send my mic on again once it gives it.";

/// Why the microphone could not be used, as the bar says it.
pub fn failed_words(why: &str) -> String {
    format!("Couldn't use this computer's microphone: {why}")
}

/// Open the microphone: `device`, part of its name, or the system's
/// default with none.
pub fn open(device: Option<String>) -> io::Result<Box<dyn MicSource>> {
    let device = device.filter(|d| !d.trim().is_empty());
    #[cfg(windows)]
    {
        wasapi::open(device.as_deref())
    }
    #[cfg(target_os = "linux")]
    {
        recorder::open(device.as_deref())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        drop(device);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this system has no way to read the microphone here",
        ))
    }
}

/// The index of the device whose name holds `wanted`, case aside (the
/// first, if several do).
#[cfg(any(test, windows))]
pub fn named(names: &[String], wanted: &str) -> Option<usize> {
    let wanted = wanted.trim().to_lowercase();
    names
        .iter()
        .position(|name| name.to_lowercase().contains(&wanted))
}

/// `why`, and the last thing the recorder said on its error output, if
/// anything (PR #569 review M4).
#[cfg(any(test, target_os = "linux"))]
pub fn with_said(why: &str, said: &str) -> String {
    let said = said.trim();
    if said.is_empty() {
        why.to_string()
    } else {
        format!("{why}: {said}")
    }
}

/// Little-endian 16-bit samples from `bytes`, a byte left over from the
/// last read first and one left over now kept in `carry`.
#[cfg(any(test, target_os = "linux"))]
pub fn samples_le(bytes: &[u8], carry: &mut Option<u8>) -> Vec<i16> {
    let mut all = Vec::with_capacity(bytes.len() + 1);
    all.extend(carry.take());
    all.extend_from_slice(bytes);
    if all.len() % 2 == 1 {
        *carry = all.pop();
    }
    (0..all.len() / 2)
        .map(|i| i16::from_le_bytes([all[2 * i], all[2 * i + 1]]))
        .collect()
}

/// The recorders tried on Linux, in order, each with its arguments: raw
/// 16-bit little-endian, 48 kHz mono on its standard output.
#[cfg(any(test, target_os = "linux"))]
pub fn recorders(device: Option<&str>) -> Vec<(&'static str, Vec<String>)> {
    let rate = MIC_SAMPLE_RATE.to_string();
    let channels = MIC_CHANNELS.to_string();
    let mut pw = vec![
        "--rate".to_string(),
        rate.clone(),
        "--channels".to_string(),
        channels.clone(),
        "--format".to_string(),
        "s16".to_string(),
        // A frame's worth (PR #569 review M5): its default buffers more.
        "--latency".to_string(),
        format!("{FRAME_MS}ms"),
        "--raw".to_string(),
    ];
    let mut pa = vec![
        "--raw".to_string(),
        "--format=s16le".to_string(),
        format!("--rate={rate}"),
        format!("--channels={channels}"),
        format!("--latency-msec={FRAME_MS}"),
    ];
    if let Some(device) = device {
        pw.extend(["--target".to_string(), device.to_string()]);
        pa.push(format!("--device={device}"));
    }
    pw.push("-".to_string());
    vec![("pw-record", pw), ("parec", pa)]
}

#[cfg(target_os = "linux")]
mod recorder {
    //! The microphone on Linux, through PipeWire's own recorder (or the
    //! PulseAudio one PipeWire also answers): no audio library in the app.

    use std::io::{self, BufRead, Read};
    use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{MicSource, Stopper};

    /// A recorder that gives up this soon (unknown option, no server) is
    /// taken not to work, and the next is tried.
    const SETTLE: Duration = Duration::from_millis(300);

    pub struct Recorder {
        child: Arc<Mutex<Child>>,
        out: ChildStdout,
        carry: Option<u8>,
        said: Arc<Mutex<String>>,
    }

    fn held<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The recorder's error output, a line at a time, to this app's (as
    /// its debug output), the last line kept for the bar's reason.
    fn follow_errors(
        program: &'static str,
        errors: ChildStderr,
        said: Arc<Mutex<String>>,
    ) -> Option<std::thread::JoinHandle<()>> {
        std::thread::Builder::new()
            .name("view-mic-errors".into())
            .spawn(move || {
                for line in io::BufReader::new(errors).lines() {
                    let Ok(line) = line else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    eprintln!("{program}: {line}");
                    *held(&said) = line;
                }
            })
            .ok()
    }

    pub fn open(device: Option<&str>) -> io::Result<Box<dyn MicSource>> {
        let mut why = Vec::new();
        for (program, args) in super::recorders(device) {
            let spawned = Command::new(program)
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn();
            let mut child = match spawned {
                Ok(child) => child,
                Err(e) => {
                    why.push(format!("{program}: {e}"));
                    continue;
                }
            };
            let said = Arc::new(Mutex::new(String::new()));
            let errors = child
                .stderr
                .take()
                .and_then(|errors| follow_errors(program, errors, said.clone()));
            // A recorder that works sends its first samples meanwhile:
            // they are read as one burst of about 15 frames as it starts,
            // which the relay's short backlog trims to its newest (PR #569
            // review M5). Harmless: a start, not a steady delay.
            std::thread::sleep(SETTLE);
            match child.try_wait() {
                Ok(None) => {
                    let Some(out) = child.stdout.take() else {
                        let _ = child.kill();
                        let _ = child.wait();
                        why.push(format!("{program}: no output"));
                        continue;
                    };
                    return Ok(Box::new(Recorder {
                        child: Arc::new(Mutex::new(child)),
                        out,
                        carry: None,
                        said,
                    }));
                }
                Ok(Some(status)) => {
                    // Its error output ends with it: what it said is read.
                    if let Some(errors) = errors {
                        let _ = errors.join();
                    }
                    why.push(super::with_said(
                        &format!("{program} stopped ({status})"),
                        &held(&said),
                    ));
                }
                Err(e) => why.push(format!("{program}: {e}")),
            }
        }
        Err(io::Error::other(format!(
            "no recorder could read it ({})",
            why.join("; ")
        )))
    }

    impl MicSource for Recorder {
        fn read(&mut self) -> io::Result<Vec<i16>> {
            let mut bytes = [0u8; super::FRAME_SAMPLES * 2];
            let n = self.out.read(&mut bytes)?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    super::with_said("the recorder stopped", &held(&self.said)),
                ));
            }
            Ok(super::samples_le(&bytes[..n], &mut self.carry))
        }

        fn stopper(&self) -> Option<Stopper> {
            let child = self.child.clone();
            Some(Box::new(move || {
                let _ = held(&child).kill();
            }))
        }
    }

    impl Drop for Recorder {
        fn drop(&mut self) {
            let mut child = held(&self.child);
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(windows)]
mod wasapi {
    //! The microphone on Windows, through WASAPI in shared mode, asked for
    //! 48 kHz mono 16-bit and converted to it by Windows.

    use std::io;
    use std::time::Duration;

    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Foundation::E_ACCESSDENIED;
    use windows::Win32::Media::Audio::{
        AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
        AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, DEVICE_STATE_ACTIVE, IAudioCaptureClient,
        IAudioClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX, eCapture,
        eConsole,
    };
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
        CoUninitialize, STGM_READ,
    };

    use super::MicSource;

    /// How long a read waits for the device to gather more.
    const POLL: Duration = Duration::from_millis(10);
    const WAVE_FORMAT_PCM: u16 = 1;

    pub struct Wasapi {
        client: IAudioClient,
        capture: IAudioCaptureClient,
        /// Last, so COM ends on this thread after its objects go (PR #569
        /// review M8).
        _com: Com,
    }

    /// COM on the capture's thread, ended as it goes when it was begun
    /// here (an apartment of another kind is left as it was).
    struct Com(bool);

    impl Drop for Com {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    fn words(e: &windows::core::Error) -> String {
        if e.code() == E_ACCESSDENIED {
            "Windows keeps the microphone from desktop apps (Settings, Privacy & security, Microphone)".to_string()
        } else {
            e.message()
        }
    }

    fn failed(e: windows::core::Error) -> io::Error {
        io::Error::other(words(&e))
    }

    fn friendly_name(device: &IMMDevice) -> String {
        let named = (|| -> windows::core::Result<String> {
            let store = unsafe { device.OpenPropertyStore(STGM_READ) }?;
            let value = unsafe { store.GetValue(&PKEY_Device_FriendlyName) }?;
            let text = unsafe { PropVariantToStringAlloc(&value) }?;
            let name = unsafe { text.to_string() }.unwrap_or_default();
            unsafe { CoTaskMemFree(Some(text.0 as *const std::ffi::c_void)) };
            Ok(name)
        })();
        named.unwrap_or_default()
    }

    fn device(enumerator: &IMMDeviceEnumerator, wanted: Option<&str>) -> io::Result<IMMDevice> {
        let Some(wanted) = wanted else {
            return unsafe { enumerator.GetDefaultAudioEndpoint(eCapture, eConsole) }
                .map_err(|_| io::Error::other("this computer has no microphone"));
        };
        let collection = unsafe { enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) }
            .map_err(failed)?;
        let count = unsafe { collection.GetCount() }.map_err(failed)?;
        let mut devices = Vec::new();
        for index in 0..count {
            let device = unsafe { collection.Item(index) }.map_err(failed)?;
            devices.push((friendly_name(&device), device));
        }
        let names: Vec<String> = devices.iter().map(|(name, _)| name.clone()).collect();
        super::named(&names, wanted)
            .map(|index| devices.swap_remove(index).1)
            .ok_or_else(|| {
                io::Error::other(format!(
                    "no microphone here is named like \u{201c}{wanted}\u{201d}"
                ))
            })
    }

    pub fn open(wanted: Option<&str>) -> io::Result<Box<dyn MicSource>> {
        let com = Com(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok());
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.map_err(failed)?;
        let device = device(&enumerator, wanted)?;
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(failed)?;
        let channels = super::MIC_CHANNELS;
        let rate = super::MIC_SAMPLE_RATE;
        let align = channels * 2;
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM,
            nChannels: channels,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: rate * u32::from(align),
            nBlockAlign: align,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                2_000_000, // 200 ms, in 100 ns units
                0,
                &format,
                None,
            )
        }
        .map_err(failed)?;
        let capture: IAudioCaptureClient = unsafe { client.GetService() }.map_err(failed)?;
        unsafe { client.Start() }.map_err(failed)?;
        Ok(Box::new(Wasapi {
            client,
            capture,
            _com: com,
        }))
    }

    impl MicSource for Wasapi {
        fn read(&mut self) -> io::Result<Vec<i16>> {
            std::thread::sleep(POLL);
            let mut out = Vec::new();
            loop {
                let available = unsafe { self.capture.GetNextPacketSize() }.map_err(failed)?;
                if available == 0 {
                    return Ok(out);
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                unsafe {
                    self.capture
                        .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                }
                .map_err(failed)?;
                let count = frames as usize * usize::from(super::MIC_CHANNELS);
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    out.extend(std::iter::repeat_n(0i16, count));
                } else {
                    // SAFETY: GetBuffer handed out `frames` frames of the
                    // 16-bit mono format asked for, two bytes each.
                    let bytes = unsafe { std::slice::from_raw_parts(data, count * 2) };
                    out.extend(
                        (0..count).map(|i| i16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]])),
                    );
                }
                unsafe { self.capture.ReleaseBuffer(frames) }.map_err(failed)?;
            }
        }
    }

    impl Drop for Wasapi {
        fn drop(&mut self) {
            let _ = unsafe { self.client.Stop() };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Known samples, a read at a time, then the microphone gone.
    struct Fake {
        reads: VecDeque<Vec<i16>>,
    }

    impl MicSource for Fake {
        fn read(&mut self) -> io::Result<Vec<i16>> {
            self.reads
                .pop_front()
                .ok_or_else(|| io::Error::other("unplugged"))
        }
    }

    fn tone(samples: usize) -> Vec<i16> {
        (0..samples)
            .map(|i| ((i as f32 * 0.05).sin() * 8_000.0) as i16)
            .collect()
    }

    #[test]
    fn a_frame_is_twenty_ms_of_48_khz_mono() {
        assert_eq!(FRAME_SAMPLES, 960);
        assert_eq!(MIC_SAMPLE_RATE, 48_000);
        assert_eq!(MIC_CHANNELS, 1);
    }

    #[test]
    fn the_framer_cuts_whole_frames_and_keeps_the_rest() {
        let mut framer = Framer::default();
        assert!(framer.push(&tone(500)).is_empty());
        assert_eq!(framer.pending(), 500);
        let frames = framer.push(&tone(2_000));
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|f| f.len() == FRAME_SAMPLES));
        assert_eq!(framer.pending(), 2_500 - 2 * FRAME_SAMPLES);
        // The samples keep their order across reads.
        let mut framer = Framer::default();
        let all: Vec<i16> = (0..FRAME_SAMPLES as i16 * 3).collect();
        let mut out = Vec::new();
        for piece in all.chunks(333) {
            out.extend(framer.push(piece).into_iter().flatten());
        }
        assert_eq!(out, all);
        assert!(framer.push(&[]).is_empty());
    }

    #[test]
    fn the_packets_are_a_phones() {
        let start = start_packet();
        assert!(matches!(&start, InputPacket::Mic { enabled: true, opus } if opus.is_empty()));
        let frame = frame_packet(vec![0xF8, 0xFF, 0xFE]);
        // As the wire has it: the mic's tag, on, the length, the frame.
        let wire = frame.encode();
        assert_eq!(&wire[1..6], &[1, 3, 0, 0, 0]);
        assert_eq!(&wire[6..], &[0xF8, 0xFF, 0xFE]);
        assert!(matches!(
            InputPacket::decode(&wire).unwrap(),
            InputPacket::Mic { enabled: true, opus } if opus == [0xF8, 0xFF, 0xFE]
        ));
        assert!(matches!(
            InputPacket::decode(&start.encode()).unwrap(),
            InputPacket::Mic { enabled: true, opus } if opus.is_empty()
        ));
    }

    /// Known samples go as the start and then one Opus frame each 20 ms of
    /// them, which the other computer's decoder reads back as 960 samples.
    #[test]
    fn the_pump_sends_the_start_then_each_frame_encoded() {
        let mut source = Fake {
            reads: VecDeque::from([tone(700), Vec::new(), tone(1_500), tone(681)]),
        };
        let mut encode = opus_encoder().unwrap();
        let mut sent = Vec::new();
        let stop = AtomicBool::new(false);
        let ended = pump(
            &mut source,
            &mut encode,
            &mut |packet| {
                sent.push(packet);
                true
            },
            &stop,
        );
        assert_eq!(ended, Ended::Failed("unplugged".into()));
        // 700 + 1500 + 681 = 2881 samples: three whole frames, one left.
        assert_eq!(sent.len(), 4, "{sent:?}");
        assert!(matches!(&sent[0], InputPacket::Mic { enabled: true, opus } if opus.is_empty()));
        let mut decoder =
            pocketdesk_core::streaming::AudioDecoder::new(MIC_SAMPLE_RATE, MIC_CHANNELS).unwrap();
        for packet in &sent[1..] {
            let InputPacket::Mic { enabled, opus } = packet else {
                panic!("a mic frame: {packet:?}");
            };
            assert!(*enabled && !opus.is_empty() && opus.len() < 400);
            assert_eq!(decoder.decode(opus).unwrap().len(), FRAME_SAMPLES);
        }
    }

    #[test]
    fn the_pump_stops_when_told_or_refused() {
        let reads = || VecDeque::from(vec![tone(FRAME_SAMPLES); 50]);
        let mut encode = opus_encoder().unwrap();
        // Stopped before it began: nothing goes, not even the start.
        let stop = AtomicBool::new(true);
        let mut sent = 0;
        let ended = pump(
            &mut Fake { reads: reads() },
            &mut encode,
            &mut |_| {
                sent += 1;
                true
            },
            &stop,
        );
        assert_eq!((ended, sent), (Ended::Stopped, 0));
        // Stopped by the window after the third packet.
        let stop = AtomicBool::new(false);
        let mut sent = 0;
        let ended = pump(
            &mut Fake { reads: reads() },
            &mut encode,
            &mut |_| {
                sent += 1;
                if sent == 3 {
                    stop.store(true, Ordering::Release);
                }
                true
            },
            &stop,
        );
        assert_eq!((ended, sent), (Ended::Stopped, 3));
        // The relay took the start, then no more.
        let stop = AtomicBool::new(false);
        let mut sent = 0;
        let ended = pump(
            &mut Fake { reads: reads() },
            &mut encode,
            &mut |_| {
                sent += 1;
                sent < 2
            },
            &stop,
        );
        assert_eq!((ended, sent), (Ended::Refused, 2));
        // An encoder that fails says why.
        let stop = AtomicBool::new(false);
        let ended = pump(
            &mut Fake { reads: reads() },
            &mut |_| Err("no encoder".to_string()),
            &mut |_| true,
            &stop,
        );
        assert_eq!(ended, Ended::Failed("no encoder".into()));
    }

    /// The thread with a fake source: the packets reach the sink, and how
    /// it ended is heard; a source that cannot open says why.
    #[test]
    fn the_run_pumps_on_its_own_thread_and_says_how_it_ended() {
        let (tx, rx) = std::sync::mpsc::channel();
        let (ended_tx, ended_rx) = std::sync::mpsc::channel();
        let run = MicRun::start(
            || {
                Ok(Box::new(Fake {
                    reads: VecDeque::from([tone(FRAME_SAMPLES * 2)]),
                }) as Box<dyn MicSource>)
            },
            move |packet| tx.send(packet).is_ok(),
            move |how| {
                let _ = ended_tx.send(how);
            },
        )
        .unwrap();
        let wait = std::time::Duration::from_secs(10);
        assert_eq!(
            ended_rx.recv_timeout(wait).unwrap(),
            Ended::Failed("unplugged".into())
        );
        assert_eq!(rx.try_iter().count(), 3, "the start and two frames");
        drop(run);

        let (ended_tx, ended_rx) = std::sync::mpsc::channel();
        let _run = MicRun::start(
            || Err(io::Error::other("no microphone")),
            |_| true,
            move |how| {
                let _ = ended_tx.send(how);
            },
        )
        .unwrap();
        assert_eq!(
            ended_rx.recv_timeout(wait).unwrap(),
            Ended::Failed("no microphone".into())
        );
        assert_eq!(
            failed_words("no microphone"),
            "Couldn't use this computer's microphone: no microphone"
        );
    }

    /// Dropping the run stops the thread at its next read: the sink hears
    /// nothing more, and the end is Stopped.
    #[test]
    fn dropping_the_run_stops_it() {
        struct Endless;
        impl MicSource for Endless {
            fn read(&mut self) -> io::Result<Vec<i16>> {
                std::thread::sleep(std::time::Duration::from_millis(5));
                Ok(vec![0; FRAME_SAMPLES / 4])
            }
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let (ended_tx, ended_rx) = std::sync::mpsc::channel();
        let run = MicRun::start(
            || Ok(Box::new(Endless) as Box<dyn MicSource>),
            move |packet| tx.send(packet).is_ok(),
            move |how| {
                let _ = ended_tx.send(how);
            },
        )
        .unwrap();
        let wait = std::time::Duration::from_secs(10);
        rx.recv_timeout(wait).expect("the start");
        drop(run);
        assert_eq!(ended_rx.recv_timeout(wait).unwrap(), Ended::Stopped);
    }

    /// PR #569 review M4: dropping the run ends a source whose read is
    /// blocked (a stalled recorder) through its stopper, at once.
    #[test]
    fn dropping_the_run_ends_a_blocked_read() {
        use std::sync::mpsc;
        struct Stalled {
            woken: mpsc::Receiver<()>,
            wake: mpsc::Sender<()>,
        }
        impl MicSource for Stalled {
            fn read(&mut self) -> io::Result<Vec<i16>> {
                // Blocks until stopped, as a recorder that sends nothing.
                self.woken.recv().map_err(|_| io::Error::other("closed"))?;
                Err(io::Error::other("killed"))
            }
            fn stopper(&self) -> Option<Stopper> {
                let wake = self.wake.clone();
                Some(Box::new(move || {
                    let _ = wake.send(());
                }))
            }
        }
        let (opened_tx, opened_rx) = mpsc::channel();
        let (ended_tx, ended_rx) = mpsc::channel();
        let run = MicRun::start(
            move || {
                let (wake, woken) = mpsc::channel();
                let _ = opened_tx.send(());
                Ok(Box::new(Stalled { woken, wake }) as Box<dyn MicSource>)
            },
            |_| true,
            move |how| {
                let _ = ended_tx.send(how);
            },
        )
        .unwrap();
        let wait = std::time::Duration::from_secs(10);
        opened_rx.recv_timeout(wait).unwrap();
        // The start went; the read is blocked now (or about to be).
        std::thread::sleep(std::time::Duration::from_millis(50));
        drop(run);
        assert_eq!(
            ended_rx.recv_timeout(wait).unwrap(),
            Ended::Failed("killed".into()),
            "the blocked read returned"
        );
    }

    #[test]
    fn a_recorders_last_words_join_the_reason() {
        assert_eq!(with_said("pw-record stopped", "  "), "pw-record stopped");
        assert_eq!(
            with_said("pw-record stopped", "error: no such target\n"),
            "pw-record stopped: error: no such target"
        );
    }

    #[test]
    fn the_switch_goes_on_only_while_the_window_controls() {
        let mut mic = MicSwitch::default();
        assert!(!mic.wanted() && !mic.on());
        assert_eq!(mic.step(true), None, "off by default");
        mic.toggle();
        assert!(mic.wanted());
        assert_eq!(mic.step(true), Some(Change::Start));
        assert!(mic.on());
        assert_eq!(mic.step(true), None);
        // Control lost (View only, the view ending or reconnecting): off,
        // and it stays off as control comes back.
        assert_eq!(mic.step(false), Some(Change::Stop));
        assert!(!mic.on() && !mic.wanted());
        assert_eq!(mic.step(true), None);
        // Wanted while the window cannot: it never starts.
        mic.toggle();
        assert_eq!(mic.step(false), None);
        assert_eq!(mic.step(true), None);
        // The switch, then off from the bar.
        mic.toggle();
        assert_eq!(mic.step(true), Some(Change::Start));
        mic.toggle();
        assert_eq!(mic.step(true), Some(Change::Stop));
        // Turned off from the bar again.
        mic.toggle();
        assert_eq!(mic.step(true), Some(Change::Start));
        mic.toggle();
        assert_eq!(mic.step(true), Some(Change::Stop));
        assert!(!mic.wanted());
        // Stopped at once (the window closing, the capture ending): no
        // change left for the next step, and it stays off.
        mic.toggle();
        assert_eq!(mic.step(true), Some(Change::Start));
        mic.stopped();
        assert!(!mic.on() && !mic.wanted());
        assert_eq!(mic.step(true), None);
    }

    #[test]
    fn a_device_is_found_by_part_of_its_name() {
        let names = vec![
            "Microphone (Realtek(R) Audio)".to_string(),
            "Headset Microphone (USB Audio)".to_string(),
            "CABLE Output (VB-Audio Virtual Cable)".to_string(),
        ];
        assert_eq!(named(&names, "usb"), Some(1));
        assert_eq!(named(&names, " Realtek "), Some(0));
        assert_eq!(named(&names, "microphone"), Some(0), "the first");
        assert_eq!(named(&names, "Webcam"), None);
    }

    #[test]
    fn samples_are_read_little_endian_across_reads() {
        let mut carry = None;
        assert_eq!(samples_le(&[0x01, 0x00, 0xFF], &mut carry), [1]);
        assert_eq!(carry, Some(0xFF));
        assert_eq!(
            samples_le(&[0x7F, 0x00, 0x80], &mut carry),
            [0x7FFF, i16::MIN]
        );
        assert_eq!(carry, None);
        assert!(samples_le(&[], &mut carry).is_empty());
    }

    #[test]
    fn the_recorders_ask_for_raw_48_khz_mono() {
        let tried = recorders(None);
        assert_eq!(tried[0].0, "pw-record");
        assert_eq!(
            tried[0].1,
            [
                "--rate",
                "48000",
                "--channels",
                "1",
                "--format",
                "s16",
                "--latency",
                "20ms",
                "--raw",
                "-"
            ]
        );
        assert_eq!(tried[1].0, "parec");
        assert_eq!(
            tried[1].1,
            [
                "--raw",
                "--format=s16le",
                "--rate=48000",
                "--channels=1",
                "--latency-msec=20"
            ]
        );
        let named = recorders(Some("alsa_input.usb-headset"));
        assert!(named[0].1.ends_with(&[
            "--target".into(),
            "alsa_input.usb-headset".into(),
            "-".into()
        ]));
        assert_eq!(
            named[1].1.last().map(String::as_str),
            Some("--device=alsa_input.usb-headset")
        );
    }

    /// The real microphone, for a test machine only (never the owner's
    /// box): open the default one for a second and count what came. Counts
    /// only; nothing is played or kept.
    #[test]
    #[ignore = "opens a real microphone"]
    fn the_real_microphone_gives_frames() {
        if std::env::var("POCKETDESK_MIC_TEST_MACHINE").as_deref() != Ok("1") {
            eprintln!("skipped: set POCKETDESK_MIC_TEST_MACHINE=1 on a test machine");
            return;
        }
        let device = std::env::var("POCKETDESK_MIC_TEST_DEVICE").ok();
        let mut source = open(device).expect("a microphone");
        let mut framer = Framer::default();
        let mut frames = 0;
        let until = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while std::time::Instant::now() < until {
            frames += framer.push(&source.read().unwrap()).len();
        }
        eprintln!("frames in a second: {frames}");
        assert!((35..=65).contains(&frames), "{frames} frames");
    }
}
