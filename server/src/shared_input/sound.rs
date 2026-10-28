//! Sound follows the mouse (BK6): while this computer's keyboard and mouse
//! drive another of the owner's computers, that computer's sound plays
//! here, with "Hear the computer you're driving here"
//! (`shared_input_sound`) on.
//!
//! The driving side asks for it ([`Asking`]: [`Msg::SoundWanted`] after
//! each arrival, and no longer as the cursor leaves or the setting goes
//! off) and plays what comes ([`Listening`]: a small [`Jitter`] buffer in
//! front of the shared [`Player`]). The computer driven captures its own
//! sound only while the computer driving it now asked for it
//! ([`Sending`]), encodes it as Opus (48 kHz stereo, 20 ms, 96 kbit/s) and
//! sends each packet as one datagram ([`Msg::Sound`]) on their link; one
//! that cannot capture says so once ([`Msg::SoundUnavailable`]). Input
//! never waits behind it: input goes the other way, from the computer
//! driving, and a sound packet is dropped rather than queued while other
//! datagrams wait on the link ([`room_for_sound`]), so a ping's answer is
//! never held up behind sound either.
//!
//! Two test seams keep tests silent: [`TONE_SEAM`] captures a tone instead
//! of this computer's sound, and [`PLAY_SEAM`] writes what would play to a
//! WAV file instead of the speakers.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use pocketdesk_core::streaming::audio_capture::{self, PcmBuffer};
use pocketdesk_core::streaming::{AudioConfig, AudioEncoder};

use super::link;
use super::wire::{self, Msg};
use crate::remote_sound::{self, Player};

/// With this set, the computer driven sends a tone (its value in Hz, 1000
/// when it is no number) instead of capturing its own sound: a test seam,
/// so nothing real is captured.
pub const TONE_SEAM: &str = "POCKETDESK_TEST_AUDIO_TONE";

/// With this set, the driving computer writes the sound it would play to
/// this WAV file instead of its speakers: a test seam, so nothing is heard.
pub const PLAY_SEAM: &str = "POCKETDESK_COMPUTER_AUDIO_FILE";

/// The Opus the computer driven sends: 48 kHz stereo, 20 ms a packet, 96
/// kbit/s (about 240 bytes a packet, well inside a datagram).
pub const BITRATE_BPS: u32 = 96_000;
const FRAME_MS: u32 = 20;
/// Samples in one packet's worth of sound, both channels.
const FRAME_SAMPLES: usize =
    (remote_sound::RATE / 1000 * FRAME_MS) as usize * remote_sound::CHANNELS as usize;

fn opus_config() -> AudioConfig {
    AudioConfig {
        sample_rate: remote_sound::RATE,
        channels: remote_sound::CHANNELS,
        bitrate_bps: BITRATE_BPS,
        frame_duration_ms: FRAME_MS,
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------
// The driving side: asking, and playing what comes.
// ---------------------------------------------------------------------------

/// What the driving side wants now: the sound of `away_on`, the computer
/// its cursor is on, for this visit (`visit`, a new one each arrival),
/// with the setting `on`, and never a phone's (`is_phone`).
pub fn want_for(
    away_on: Option<&str>,
    visit: u64,
    on: bool,
    is_phone: impl Fn(&str) -> bool,
) -> Option<(String, u64)> {
    away_on
        .filter(|to| on && !is_phone(to))
        .map(|to| (to.to_string(), visit))
}

/// A step in asking for sound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Ask this computer for its sound, and play what it sends.
    Ask(String),
    /// No longer: tell it so, and stop playing what it sends.
    Unask(String),
}

/// The driving side's asking for sound: at most one computer at a time,
/// asked once each visit.
#[derive(Debug, Default)]
pub struct Asking {
    asked: Option<(String, u64)>,
}

impl Asking {
    /// What is wanted now ([`want_for`]): the steps to get there from what
    /// was asked. A new visit to the same computer asks again (the
    /// computer driven forgets an ask when a cursor arrives).
    pub fn want(&mut self, want: Option<(&str, u64)>) -> Vec<Step> {
        let now = want.map(|(to, visit)| (to.to_string(), visit));
        if now == self.asked {
            return Vec::new();
        }
        let mut steps = Vec::new();
        if let Some((was, _)) = self.asked.take() {
            steps.push(Step::Unask(was));
        }
        if let Some((to, _)) = &now {
            steps.push(Step::Ask(to.clone()));
        }
        self.asked = now;
        steps
    }

    /// The computer asked now.
    pub fn asked(&self) -> Option<&str> {
        self.asked.as_ref().map(|(to, _)| to.as_str())
    }
}

/// Packets waiting in the jitter buffer before playing starts (or starts
/// again after it ran dry): 40 ms.
pub const PREROLL: usize = 2;

/// The most packets the jitter buffer holds: 60 ms. Past it, the oldest
/// goes, so the sound never falls further behind.
pub const JITTER_CAP: usize = 3;

/// What happened to a packet put in the [`Jitter`] buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pushed {
    Kept,
    /// Kept, and the oldest waiting was dropped to make room.
    KeptOverOldest,
    /// Older than one already played (or dropped): dropped.
    Late,
    /// The same packet again: dropped.
    Twice,
}

/// The driving side's small jitter buffer: packets in order of their count,
/// a late one dropped, at most [`JITTER_CAP`] waiting (the oldest dropped
/// past it), and none played until [`PREROLL`] have come.
#[derive(Debug, Default)]
pub struct Jitter {
    queue: VecDeque<(u32, Vec<u8>)>,
    /// The newest count played or dropped: nothing older plays.
    last: Option<u32>,
    primed: bool,
}

/// Whether `a` comes after `b` by their counts, which may wrap.
fn after(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

impl Jitter {
    pub fn push(&mut self, seq: u32, opus: Vec<u8>) -> Pushed {
        if self.last.is_some_and(|last| !after(seq, last)) {
            return Pushed::Late;
        }
        if self.queue.iter().any(|(s, _)| *s == seq) {
            return Pushed::Twice;
        }
        let at = self
            .queue
            .iter()
            .position(|(s, _)| after(*s, seq))
            .unwrap_or(self.queue.len());
        self.queue.insert(at, (seq, opus));
        if self.queue.len() > JITTER_CAP {
            if let Some((oldest, _)) = self.queue.pop_front() {
                self.last = Some(oldest);
            }
            return Pushed::KeptOverOldest;
        }
        Pushed::Kept
    }

    /// Whether a packet may be played now: once [`PREROLL`] have come, and
    /// until it runs dry.
    pub fn ready(&self) -> bool {
        !self.queue.is_empty() && (self.primed || self.queue.len() >= PREROLL)
    }

    /// The next packet to play, when [`Jitter::ready`].
    pub fn pop(&mut self) -> Option<Vec<u8>> {
        if !self.ready() {
            return None;
        }
        let (seq, opus) = self.queue.pop_front()?;
        self.primed = true;
        self.last = Some(seq);
        Some(opus)
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Nothing waits here or at the output: play only once [`PREROLL`]
    /// have come again.
    pub fn ran_dry(&mut self) {
        if self.queue.is_empty() {
            self.primed = false;
        }
    }
}

/// The most sound kept at the output for the computer driven: 60 ms. With
/// the jitter buffer's preroll, the sound plays about 40 to 60 ms behind.
const OUTPUT_TARGET_FRAMES: u32 = remote_sound::RATE / 1000 * 60;

/// Packets waiting for the player thread; more are dropped.
const FEED_DEPTH: usize = 16;

type Feed = std::sync::mpsc::SyncSender<(u32, Vec<u8>)>;

/// Where the sound each computer sends goes: the [`Listening`] for it,
/// by its generation, and its feed. A computer not listened to is not
/// played, whatever it sends.
static LISTENING: LazyLock<Mutex<HashMap<String, (u64, Feed)>>> = LazyLock::new(Default::default);

/// What each computer listened to said it cannot give ([`Msg::SoundUnavailable`]).
static UNAVAILABLE: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);

static GENERATION: AtomicU64 = AtomicU64::new(0);

/// A sound packet came from `device_id` ([`Msg::Sound`]): played if this
/// computer listens to it, else dropped. Never waits.
pub fn heard(device_id: &str, seq: u32, opus: &[u8]) {
    if let Some((_, feed)) = lock(&LISTENING).get(device_id) {
        let _ = feed.try_send((seq, opus.to_vec()));
    }
}

/// `device_id` said it cannot give its sound (`why`): kept for the driving
/// side to say, while it listens to that computer.
pub fn said_unavailable(device_id: &str, why: &str) {
    if lock(&LISTENING).contains_key(device_id) {
        lock(&UNAVAILABLE).insert(device_id.to_string(), why.to_string());
    }
}

/// What `device_id` said it cannot give, once.
pub fn take_unavailable(device_id: &str) -> Option<String> {
    lock(&UNAVAILABLE).remove(device_id)
}

/// The driving side playing one computer's sound while this is held: on a
/// thread of its own, which opens the output at the first packet and closes
/// it as this is dropped (what is still queued is not played).
#[must_use = "the sound stops when this is dropped"]
pub struct Listening {
    device_id: String,
    generation: u64,
}

impl Listening {
    /// Play what `device_id` sends, on the speakers or the [`PLAY_SEAM`]'s
    /// file.
    pub fn start(device_id: &str) -> Listening {
        Self::start_with(device_id, std::env::var_os(PLAY_SEAM).map(PathBuf::from))
    }

    /// [`Listening::start`], to `seam` when given.
    pub fn start_with(device_id: &str, seam: Option<PathBuf>) -> Listening {
        let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
        let (feed, packets) = std::sync::mpsc::sync_channel(FEED_DEPTH);
        let spawned = std::thread::Builder::new()
            .name("pd-computer-sound".into())
            .spawn(move || play(packets, seam));
        if let Err(e) = spawned {
            tracing::warn!("Shared input: the other computer's sound cannot play: {e}");
        }
        lock(&UNAVAILABLE).remove(device_id);
        lock(&LISTENING).insert(device_id.to_string(), (generation, feed));
        Listening {
            device_id: device_id.to_string(),
            generation,
        }
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        let mut listening = lock(&LISTENING);
        if listening
            .get(&self.device_id)
            .is_some_and(|(generation, _)| *generation == self.generation)
        {
            // Its feed goes, and the thread ends at once.
            listening.remove(&self.device_id);
            drop(listening);
            lock(&UNAVAILABLE).remove(&self.device_id);
        }
    }
}

/// Play the packets that come until their feed closes: through the jitter
/// buffer, at most [`OUTPUT_TARGET_FRAMES`] at the output.
fn play(packets: std::sync::mpsc::Receiver<(u32, Vec<u8>)>, seam: Option<PathBuf>) {
    use std::sync::mpsc::RecvTimeoutError;
    let mut player = match Player::new("the sound of the computer driven", seam) {
        Ok(player) => player,
        Err(e) => {
            tracing::warn!("Shared input: the other computer's sound cannot be decoded: {e:#}");
            return;
        }
    };
    let mut jitter = Jitter::default();
    let mut dropped = 0u64;
    loop {
        match packets.recv_timeout(Duration::from_millis(5)) {
            Ok((seq, opus)) => dropped += u64::from(jitter.push(seq, opus) != Pushed::Kept),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        while let Ok((seq, opus)) = packets.try_recv() {
            dropped += u64::from(jitter.push(seq, opus) != Pushed::Kept);
        }
        while jitter.ready() && player.queued() < OUTPUT_TARGET_FRAMES {
            match jitter.pop() {
                Some(opus) => player.play(&opus),
                None => break,
            }
        }
        if player.queued() == 0 {
            jitter.ran_dry();
        }
    }
    if dropped > 0 {
        tracing::debug!(dropped, "Shared input: late sound packets dropped");
    }
}

// ---------------------------------------------------------------------------
// The computer driven: capturing and sending.
// ---------------------------------------------------------------------------

/// What keeps a capture going; dropped, it stops.
pub type Started = Box<dyn Send>;

/// A capture starting.
pub type Starting = Pin<Box<dyn Future<Output = anyhow::Result<Started>> + Send>>;

/// Where sound comes from: given where to put it (each buffer 16-bit
/// interleaved stereo at 48 kHz) and when to stop, it starts, or says why
/// it cannot. It lets go of its end of the channel once it has stopped
/// altogether (a thread of its own ended).
pub type Source = Arc<dyn Fn(mpsc::Sender<PcmBuffer>, CancellationToken) -> Starting + Send + Sync>;

/// This computer's own sound (PipeWire on Linux, WASAPI on Windows), or
/// the tone with [`TONE_SEAM`] set.
pub fn desktop_source() -> Source {
    if let Some(hz) = tone_from(std::env::var(TONE_SEAM).ok().as_deref()) {
        tracing::info!(
            hz,
            "Shared input: a test tone stands for this computer's sound"
        );
        return tone_source(hz);
    }
    Arc::new(|pcm: mpsc::Sender<PcmBuffer>, cancel: CancellationToken| {
        Box::pin(async move {
            let session = audio_capture::start_audio_capture(
                remote_sound::RATE,
                remote_sound::CHANNELS,
                FRAME_MS,
                pcm,
                cancel,
            )
            .await?;
            Ok(Box::new(session) as Started)
        }) as Starting
    })
}

/// The tone [`TONE_SEAM`]'s value asks for, in Hz: none when it is not
/// set, 1000 when it is no number, kept to 20..=20000.
pub fn tone_from(seam: Option<&str>) -> Option<f32> {
    let value = seam?;
    let hz = value
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|hz| hz.is_finite())
        .unwrap_or(1000.0);
    Some(hz.clamp(20.0, 20_000.0))
}

/// Aborts its task when dropped.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// A sine of `hz` at half scale, 20 ms a buffer, in time: what the
/// computer driven sends with [`TONE_SEAM`] set.
pub fn tone_source(hz: f32) -> Source {
    Arc::new(
        move |pcm: mpsc::Sender<PcmBuffer>, cancel: CancellationToken| {
            Box::pin(async move {
                let task = tokio::spawn(async move {
                    let mut tick =
                        tokio::time::interval(Duration::from_millis(u64::from(FRAME_MS)));
                    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    let rate = f64::from(remote_sound::RATE);
                    let mut n = 0u64;
                    loop {
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            _ = tick.tick() => {}
                        }
                        let frames = FRAME_SAMPLES / 2;
                        let data: Vec<i16> = (0..frames)
                            .flat_map(|i| {
                                let t = (n * frames as u64 + i as u64) as f64 / rate;
                                let s = ((t * f64::from(hz) * std::f64::consts::TAU).sin()
                                    * 16_000.0) as i16;
                                [s, s]
                            })
                            .collect();
                        n += 1;
                        let buffer = PcmBuffer {
                            data,
                            sample_rate: remote_sound::RATE,
                            channels: remote_sound::CHANNELS,
                            timestamp: Instant::now(),
                        };
                        if pcm.send(buffer).await.is_err() {
                            break;
                        }
                    }
                });
                Ok(Box::new(AbortOnDrop(task)) as Started)
            }) as Starting
        },
    )
}

/// Captured sound cut into whole packets' worth ([`FRAME_SAMPLES`]): a
/// capture hands over what it has, whatever its size (PipeWire's quantum),
/// and Opus takes 20 ms at a time.
#[derive(Debug, Default)]
pub struct Frames {
    pending: Vec<i16>,
}

impl Frames {
    /// Take `pcm`; the whole frames there are now, in order.
    pub fn push(&mut self, pcm: &[i16]) -> Vec<Vec<i16>> {
        self.pending.extend_from_slice(pcm);
        let whole = self.pending.len() / FRAME_SAMPLES * FRAME_SAMPLES;
        let rest = self.pending.split_off(whole);
        let frames = std::mem::replace(&mut self.pending, rest);
        frames.chunks(FRAME_SAMPLES).map(<[i16]>::to_vec).collect()
    }
}

/// The most bytes of datagrams that may already wait on the link for a
/// sound packet still to go: past it, the packet is dropped, never queued
/// behind the rest (or ahead of what comes next, such as a ping's answer).
pub const QUEUED_MAX_BYTES: usize = 2048;

/// Whether a sound packet goes now, with `space` bytes free in the link's
/// datagram buffer, `full` the most ever seen free (the buffer empty).
pub fn room_for_sound(full: usize, space: usize) -> bool {
    full.saturating_sub(space) <= QUEUED_MAX_BYTES
}

/// Sends sound packets to one computer on its link.
struct Out {
    to: String,
    full: usize,
    too_large: bool,
}

impl Out {
    /// Send packet `seq`; whether it went.
    fn send(&mut self, seq: u32, opus: Vec<u8>) -> bool {
        if opus.len() > wire::MAX_SOUND_BYTES {
            return false;
        }
        let Some(conn) = link::connection(&self.to) else {
            return false;
        };
        let bytes = wire::encode(&Msg::Sound { seq, opus });
        if conn.max_datagram_size().is_none_or(|max| bytes.len() > max) {
            if !std::mem::replace(&mut self.too_large, true) {
                tracing::warn!(
                    to = self.to,
                    len = bytes.len(),
                    "Shared input: the link takes no datagram this large; sound dropped"
                );
            }
            return false;
        }
        let space = conn.datagram_send_buffer_space();
        self.full = self.full.max(space);
        if !room_for_sound(self.full, space) {
            return false;
        }
        conn.send_datagram(bytes.into()).is_ok()
    }
}

/// Packets of captured sound waiting to be encoded.
const PCM_DEPTH: usize = 8;

/// How long a capture stopped may take to let go before the next one
/// starts anyway (and the log says so).
const CAPTURE_END_WAIT: Duration = Duration::from_secs(2);

/// The computer driven's side: this computer's sound captured and sent to
/// the computer that asked for it, only while one asks, one capture at a
/// time. Wanting it for another computer, or for none, stops the capture
/// first; a capture starts only once the one before has let go of the
/// sound altogether, however fast the asks come and go.
pub struct Sending {
    source: Source,
    /// The computer sent to now, and how to stop that.
    current: Option<(String, CancellationToken)>,
    /// The capture task started last: the next waits for it to end.
    last: Option<JoinHandle<()>>,
    said: mpsc::UnboundedSender<(String, String)>,
    unavailable: mpsc::UnboundedReceiver<(String, String)>,
}

impl Sending {
    pub fn new(source: Source) -> Sending {
        let (said, unavailable) = mpsc::unbounded_channel();
        Sending {
            source,
            current: None,
            last: None,
            said,
            unavailable,
        }
    }

    /// The computer whose ask is current now (`want`: the one driving this
    /// one, when it asked), or none. Called on a runtime, as often as
    /// wanted: only a change does anything.
    pub fn set(&mut self, want: Option<&str>) {
        if self.current.as_ref().map(|(to, _)| to.as_str()) == want {
            return;
        }
        if let Some((to, cancel)) = self.current.take() {
            tracing::info!(
                to,
                "Shared input: this computer's sound no longer goes there"
            );
            cancel.cancel();
        }
        if let Some(to) = want {
            let cancel = CancellationToken::new();
            let task = tokio::spawn(capture_and_send(
                to.to_string(),
                cancel.clone(),
                self.last.take(),
                Arc::clone(&self.source),
                self.said.clone(),
            ));
            self.last = Some(task);
            self.current = Some((to.to_string(), cancel));
        }
    }

    /// The computer this one's sound goes to now.
    pub fn sending_to(&self) -> Option<&str> {
        self.current.as_ref().map(|(to, _)| to.as_str())
    }

    /// A computer whose sound could not be captured for it, and why, once
    /// each ask: to be told, while it is still the one asking.
    pub fn take_unavailable(&mut self) -> Option<(String, String)> {
        self.unavailable.try_recv().ok()
    }
}

impl Drop for Sending {
    fn drop(&mut self) {
        if let Some((_, cancel)) = self.current.take() {
            cancel.cancel();
        }
    }
}

/// Capture this computer's sound from `source` and send it to `to` until
/// `cancel`, once the capture before (`before`) has ended. Why it could
/// not, or why it stopped by itself, goes to `said`.
async fn capture_and_send(
    to: String,
    cancel: CancellationToken,
    before: Option<JoinHandle<()>>,
    source: Source,
    said: mpsc::UnboundedSender<(String, String)>,
) {
    if let Some(before) = before {
        let _ = before.await;
    }
    if cancel.is_cancelled() {
        return;
    }
    let (pcm_tx, mut pcm_rx) = mpsc::channel(PCM_DEPTH);
    let started = match source(pcm_tx, cancel.child_token()).await {
        Ok(started) => started,
        Err(e) => {
            tracing::info!(to, "Shared input: no sound to send: {e:#}");
            let _ = said.send((to, format!("its sound cannot be captured there ({e:#})")));
            return;
        }
    };
    let mut encoder = match AudioEncoder::new(opus_config()) {
        Ok(encoder) => encoder,
        Err(e) => {
            let _ = said.send((to, format!("its sound cannot be encoded there ({e:#})")));
            drop(started);
            let _ = tokio::time::timeout(CAPTURE_END_WAIT, drained(&mut pcm_rx)).await;
            return;
        }
    };
    tracing::info!(
        to,
        "Shared input: this computer's sound goes to the computer driving it"
    );
    let mut out = Out {
        to: to.clone(),
        full: 0,
        too_large: false,
    };
    let mut frames = Frames::default();
    let mut seq = 0u32;
    let (mut sent, mut dropped) = (0u64, 0u64);
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            pcm = pcm_rx.recv() => {
                let Some(pcm) = pcm else {
                    tracing::info!(to, "Shared input: this computer's sound capture stopped");
                    let _ = said.send((to.clone(), "its sound capture stopped there".to_string()));
                    break;
                };
                for frame in frames.push(&pcm.data) {
                    let packet = match encoder.encode(&frame) {
                        Ok(Some(packet)) => packet,
                        Ok(None) => continue,
                        Err(e) => {
                            tracing::debug!("Shared input: a sound frame was not encoded: {e:#}");
                            continue;
                        }
                    };
                    seq = seq.wrapping_add(1);
                    if out.send(seq, packet.data) {
                        sent += 1;
                    } else {
                        dropped += 1;
                    }
                }
            }
        }
    }
    // The capture stops, and lets go of its end of the channel once its
    // thread has ended (a PipeWire stream torn down): waited for, so the
    // next capture never starts beside this one.
    drop(started);
    if tokio::time::timeout(CAPTURE_END_WAIT, drained(&mut pcm_rx))
        .await
        .is_err()
    {
        tracing::warn!(to, "Shared input: the sound capture took long to stop");
    }
    tracing::debug!(to, sent, dropped, "Shared input: sound sent");
}

/// Wait for the capture to let go of its end of `pcm`, taking what it
/// still sends meanwhile (so it is never held up sending).
async fn drained(pcm: &mut mpsc::Receiver<PcmBuffer>) {
    while pcm.recv().await.is_some() {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn the_driver_wants_the_sound_of_the_computer_it_is_on_with_the_setting() {
        let phone = |id: &str| id == "phone";
        assert_eq!(
            want_for(Some("linux"), 3, true, phone),
            Some(("linux".to_string(), 3))
        );
        assert_eq!(want_for(Some("linux"), 3, false, phone), None, "off");
        assert_eq!(want_for(None, 3, true, phone), None, "home");
        assert_eq!(want_for(Some("phone"), 3, true, phone), None, "a phone");
    }

    #[test]
    fn each_visit_asks_once_and_leaving_unasks() {
        let mut asking = Asking::default();
        assert!(asking.want(None).is_empty(), "nothing asked at home");
        // Entering asks, once.
        assert_eq!(asking.want(Some(("linux", 1))), [Step::Ask("linux".into())]);
        assert!(asking.want(Some(("linux", 1))).is_empty());
        assert_eq!(asking.asked(), Some("linux"));
        // Home: no longer.
        assert_eq!(asking.want(None), [Step::Unask("linux".into())]);
        assert_eq!(asking.asked(), None);
        assert!(asking.want(None).is_empty());
        // A new visit to the same computer asks anew (it forgot the ask
        // as the cursor arrived), even when home was never seen between.
        asking.want(Some(("linux", 2)));
        assert_eq!(
            asking.want(Some(("linux", 3))),
            [Step::Unask("linux".into()), Step::Ask("linux".into())]
        );
        // Straight on to another computer: the first is let go first.
        assert_eq!(
            asking.want(Some(("other", 4))),
            [Step::Unask("linux".into()), Step::Ask("other".into())]
        );
    }

    #[test]
    fn the_jitter_buffer_waits_for_its_preroll_then_plays_in_order() {
        let mut jitter = Jitter::default();
        assert_eq!(jitter.push(2, vec![2]), Pushed::Kept);
        assert!(!jitter.ready(), "one packet is not enough to start");
        assert_eq!(jitter.pop(), None);
        // One that comes out of order goes in its place.
        assert_eq!(jitter.push(1, vec![1]), Pushed::Kept);
        assert!(jitter.ready());
        assert_eq!(jitter.pop(), Some(vec![1]));
        // Started: it plays on with one waiting.
        assert_eq!(jitter.pop(), Some(vec![2]));
        assert_eq!(jitter.pop(), None);
        jitter.push(3, vec![3]);
        assert_eq!(jitter.pop(), Some(vec![3]), "still started");
        // Run dry: the preroll again.
        jitter.ran_dry();
        jitter.push(4, vec![4]);
        assert!(!jitter.ready());
        jitter.push(5, vec![5]);
        assert_eq!(jitter.pop(), Some(vec![4]));
    }

    #[test]
    fn a_late_or_repeated_packet_is_dropped() {
        let mut jitter = Jitter::default();
        jitter.push(10, vec![10]);
        jitter.push(11, vec![11]);
        assert_eq!(jitter.pop(), Some(vec![10]));
        assert_eq!(jitter.push(9, vec![9]), Pushed::Late, "older than played");
        assert_eq!(jitter.push(10, vec![10]), Pushed::Late, "played already");
        assert_eq!(jitter.push(11, vec![11]), Pushed::Twice);
        assert_eq!(jitter.len(), 1);
        // The count wraps: one past the top is newer.
        let mut jitter = Jitter::default();
        jitter.push(u32::MAX, vec![1]);
        jitter.push(0, vec![2]);
        assert_eq!(jitter.pop(), Some(vec![1]));
        assert_eq!(jitter.pop(), Some(vec![2]));
        assert_eq!(jitter.push(u32::MAX, vec![1]), Pushed::Late);
    }

    #[test]
    fn past_its_cap_the_oldest_goes_and_never_plays() {
        let mut jitter = Jitter::default();
        for seq in 1..=JITTER_CAP as u32 {
            assert_eq!(jitter.push(seq, vec![seq as u8]), Pushed::Kept);
        }
        assert_eq!(
            jitter.push(JITTER_CAP as u32 + 1, vec![9]),
            Pushed::KeptOverOldest
        );
        assert_eq!(jitter.len(), JITTER_CAP);
        assert_eq!(jitter.pop(), Some(vec![2]), "the oldest went");
        // The one dropped, arriving again, is late.
        assert_eq!(jitter.push(1, vec![1]), Pushed::Late);
        // Never more than the cap behind, however many come at once.
        for seq in 10..100 {
            jitter.push(seq, vec![0]);
            assert!(jitter.len() <= JITTER_CAP);
        }
    }

    #[test]
    fn captured_sound_is_cut_into_whole_20_ms_frames() {
        let mut frames = Frames::default();
        // PipeWire's quantum of 1024 frames, stereo: no whole 960 yet...
        let quantum: Vec<i16> = (0..2048).map(|i| i as i16).collect();
        let out = frames.push(&quantum);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), FRAME_SAMPLES);
        assert_eq!(out[0][..3], [0, 1, 2]);
        // ... the rest waits for the next, in order.
        let out = frames.push(&quantum);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0][0], FRAME_SAMPLES as i16);
        assert!(frames.push(&[]).is_empty());
        // WASAPI's exact 20 ms each.
        let mut frames = Frames::default();
        assert_eq!(frames.push(&vec![0; FRAME_SAMPLES]).len(), 1);
        assert_eq!(frames.push(&vec![0; FRAME_SAMPLES * 3]).len(), 3);
    }

    #[test]
    fn sound_never_queues_behind_other_datagrams() {
        assert!(room_for_sound(16 << 20, 16 << 20), "the buffer empty");
        assert!(room_for_sound(16 << 20, (16 << 20) - QUEUED_MAX_BYTES));
        assert!(!room_for_sound(16 << 20, (16 << 20) - QUEUED_MAX_BYTES - 1));
        assert!(room_for_sound(0, 100), "more free than ever seen");
    }

    #[test]
    fn the_tone_seam_names_its_pitch() {
        assert_eq!(tone_from(None), None);
        assert_eq!(tone_from(Some("440")), Some(440.0));
        assert_eq!(tone_from(Some(" 1000 ")), Some(1000.0));
        assert_eq!(tone_from(Some("yes")), Some(1000.0));
        assert_eq!(tone_from(Some("")), Some(1000.0));
        assert_eq!(tone_from(Some("5")), Some(20.0));
        assert_eq!(tone_from(Some("1e9")), Some(20_000.0));
        assert_eq!(tone_from(Some("NaN")), Some(1000.0));
    }

    /// A capture that, like PipeWire's, lets go of its end of the channel
    /// only a while after it is told to stop (its thread tearing down).
    #[derive(Clone, Default)]
    struct Slow {
        running: Arc<AtomicUsize>,
        most: Arc<AtomicUsize>,
        started: Arc<AtomicUsize>,
    }

    impl Slow {
        fn source(&self, fail: bool) -> Source {
            let me = self.clone();
            Arc::new(
                move |pcm: mpsc::Sender<PcmBuffer>, cancel: CancellationToken| {
                    let me = me.clone();
                    Box::pin(async move {
                        if fail {
                            anyhow::bail!("no backend here");
                        }
                        me.started.fetch_add(1, Ordering::SeqCst);
                        let now = me.running.fetch_add(1, Ordering::SeqCst) + 1;
                        me.most.fetch_max(now, Ordering::SeqCst);
                        let running = Arc::clone(&me.running);
                        std::thread::spawn(move || {
                            while !cancel.is_cancelled() {
                                let buffer = PcmBuffer {
                                    data: vec![0; FRAME_SAMPLES],
                                    sample_rate: 48_000,
                                    channels: 2,
                                    timestamp: Instant::now(),
                                };
                                if pcm.blocking_send(buffer).is_err() {
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(2));
                            }
                            // Tearing down takes a while.
                            std::thread::sleep(Duration::from_millis(30));
                            running.fetch_sub(1, Ordering::SeqCst);
                            drop(pcm);
                        });
                        Ok(Box::new(()) as Started)
                    }) as Starting
                },
            )
        }
    }

    async fn until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "TIMEOUT waiting for {what}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Asks that come and go faster than a capture stops never leave two
    /// captures running, and every capture stops once none is wanted.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rapid_asks_never_run_two_captures_and_all_stop() {
        let slow = Slow::default();
        let mut sending = Sending::new(slow.source(false));
        for i in 0..60 {
            sending.set(if i % 2 == 0 { Some("test-snd-a") } else { None });
            if i % 7 == 0 {
                tokio::time::sleep(Duration::from_millis(3)).await;
            }
            sending.set(if i % 3 == 0 { Some("test-snd-b") } else { None });
        }
        // Last wanted: none.
        sending.set(None);
        until("every capture to stop", || {
            slow.running.load(Ordering::SeqCst) == 0
                && sending.last.as_ref().is_none_or(JoinHandle::is_finished)
        })
        .await;
        assert!(slow.most.load(Ordering::SeqCst) <= 1, "never two at once");
        // Wanted once more: one capture, and it stops with the ask.
        let before = slow.started.load(Ordering::SeqCst);
        sending.set(Some("test-snd-a"));
        until("the capture to start", || {
            slow.started.load(Ordering::SeqCst) == before + 1
        })
        .await;
        assert_eq!(slow.running.load(Ordering::SeqCst), 1);
        drop(sending);
        until("the capture to stop", || {
            slow.running.load(Ordering::SeqCst) == 0
        })
        .await;
        assert!(slow.most.load(Ordering::SeqCst) <= 1);
    }

    #[tokio::test]
    async fn nothing_is_captured_while_nobody_asks() {
        let slow = Slow::default();
        let mut sending = Sending::new(slow.source(false));
        sending.set(None);
        sending.set(None);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(slow.started.load(Ordering::SeqCst), 0);
        assert_eq!(sending.sending_to(), None);
    }

    #[tokio::test]
    async fn a_capture_that_cannot_start_is_told_once_for_the_ask() {
        let slow = Slow::default();
        let mut sending = Sending::new(slow.source(true));
        sending.set(Some("test-snd-c"));
        let deadline = Instant::now() + Duration::from_secs(5);
        let told = loop {
            if let Some(told) = sending.take_unavailable() {
                break told;
            }
            assert!(Instant::now() < deadline, "TIMEOUT waiting for the word");
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert_eq!(told.0, "test-snd-c");
        assert!(told.1.contains("no backend here"), "{}", told.1);
        // The same ask again is no new ask: not told again.
        sending.set(Some("test-snd-c"));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(sending.take_unavailable(), None);
    }

    /// Only a computer listened to is played, and a packet for one no
    /// longer listened to goes nowhere.
    #[test]
    fn only_the_computer_listened_to_is_played() {
        let dir = std::env::temp_dir().join(format!("pd-bk6-listen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        said_unavailable("test-snd-l", "not listened to");
        assert_eq!(take_unavailable("test-snd-l"), None);
        let listening = Listening::start_with("test-snd-l", Some(dir.join("l.wav")));
        said_unavailable("test-snd-l", "none here");
        assert_eq!(take_unavailable("test-snd-l").as_deref(), Some("none here"));
        assert!(lock(&LISTENING).contains_key("test-snd-l"));
        // An older listening dropped after a newer took its place leaves
        // the newer alone.
        let newer = Listening::start_with("test-snd-l", Some(dir.join("l2.wav")));
        drop(listening);
        assert!(lock(&LISTENING).contains_key("test-snd-l"));
        drop(newer);
        assert!(!lock(&LISTENING).contains_key("test-snd-l"));
        heard("test-snd-l", 1, &[1, 2, 3]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The whole way between two computers on this machine, over a real
    /// link: the computer driven sends a 1 kHz tone (the capture seam, so
    /// nothing real is captured), and the driving side writes what it
    /// would play to a WAV file (the playing seam, so nothing is heard).
    /// What it wrote is the tone, at about its level.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_tone_crosses_the_link_and_plays_into_the_seam() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let dir = std::env::temp_dir().join(format!("pd-bk6-loop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("heard.wav");
        let (a, b, out, back) = link::tests::linked_pair().await;
        // The driving side calls the computer driven "test-snd-linux"; the
        // computer driven calls it "test-snd-windows".
        let _driving = link::hold("test-snd-linux", out);
        let _driven = link::hold("test-snd-windows", back);
        let listening = Listening::start_with("test-snd-linux", Some(wav.clone()));
        let mut sending = Sending::new(tone_source(1000.0));
        sending.set(Some("test-snd-windows"));
        // A second and a half of tone.
        let deadline = Instant::now() + Duration::from_secs(10);
        let enough = (48_000 * 2 * 2) as u64; // one second, stereo, 16-bit
        loop {
            let len = std::fs::metadata(&wav).map(|m| m.len()).unwrap_or(0);
            if len >= 44 + enough {
                break;
            }
            assert!(Instant::now() < deadline, "TIMEOUT: {len} bytes heard");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        sending.set(None);
        drop(listening);
        drop(sending);
        // The player's thread lets go of the file as it ends.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let bytes = std::fs::read(&wav).unwrap();
        let samples: Vec<f32> = bytes[44..]
            .chunks(2)
            .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])))
            .collect();
        // After the codec's start-up: the tone's level and its pitch.
        let tail = &samples[samples.len() / 2..];
        let rms = (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt();
        assert!(rms > 8_000.0 && rms < 14_000.0, "level {rms}");
        let left: Vec<f32> = tail.iter().step_by(2).copied().collect();
        let crossings = left
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        let hz = crossings as f32 / (left.len() as f32 / 48_000.0);
        assert!((950.0..1050.0).contains(&hz), "{hz} Hz");
        a.close().await;
        b.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
