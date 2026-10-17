//! A KDE Connect device's link after the handshake (3b): pairing both ways,
//! held for the owner with the code both screens show; unpairing; ping.
//! The rest a paired device sends is 3c's.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use pocketdesk_core::local::kdeconnect::{self, SeenDevice};
use pocketdesk_core::local::kdeconnect_devices::{self as devices, KdeDevice, Pending};
use pocketdesk_core::protocol::kdeconnect::NetworkPacket;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use super::handshake::{DeviceLink, MAX_LINE};
use super::pairing::{self, PairSays};

/// The packet type of a ping, both ways.
pub const PING: &str = "kdeconnect.ping";

/// What the desktop app (through the server) asks of a device's link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Ask the device to pair.
    PairFromHere,
    /// The owner's answer to the device's request.
    Answer(bool),
    /// Unpair the device.
    Unpair,
    /// Ping it.
    Ping,
    /// Close: a newer link from the same device took this one's place.
    Replaced,
}

/// A live link: its token, and the way to tell it of an event.
type LiveLink = (u64, mpsc::Sender<Event>);

/// The links up now, by device id, each with a token of its own.
static LIVE: LazyLock<Mutex<HashMap<String, LiveLink>>> = LazyLock::new(Default::default);

/// The next link's token.
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Tell `device_id`'s link of `event`; false when it has none.
pub fn tell(device_id: &str, event: Event) -> bool {
    let link = LIVE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(device_id)
        .map(|(_, tx)| tx.clone());
    link.is_some_and(|tx| tx.try_send(event).is_ok())
}

/// A link counted live while held. A newer one from the same device takes
/// its place and closes it; a link going removes only itself (the A71's
/// new link had been dropped from the list by its old one going).
struct Live(String, u64);

impl Live {
    fn new(device_id: &str, tx: mpsc::Sender<Event>) -> Self {
        let token = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let older = LIVE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(device_id.to_string(), (token, tx));
        if let Some((_, older)) = older {
            let _ = older.try_send(Event::Replaced);
        }
        Self(device_id.to_string(), token)
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
        if live.get(&self.0).is_some_and(|(token, _)| *token == self.1) {
            live.remove(&self.0);
        }
    }
}

/// A pairing in progress on this link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Waiting {
    since: u64,
    from_device: bool,
}

fn now_unix() -> u64 {
    pocketdesk_core::local::pin::now_unix()
}

/// Show `summary` and `body` on the desktop, off the link's task. Not in
/// tests: a toast's thread outliving the test process crashes it.
fn notify(summary: &'static str, body: String) {
    if cfg!(test) {
        return;
    }
    std::thread::spawn(move || {
        if !crate::platform::notify(summary, &body, Duration::from_secs(10)) {
            tracing::info!("{summary}: {body}");
        }
    });
}

/// The link, until it closes. `paired`: the device is paired with the
/// certificate it brought (the handshake held it to the pin).
pub async fn run<S: AsyncRead + AsyncWrite + Unpin>(
    link: DeviceLink<S>,
    our_cert_der: &[u8],
    paired: bool,
) {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let device_id = link.hello.device_id.clone();
    let name = link.hello.device_name.clone();
    let (tx, mut events) = mpsc::channel(8);
    let _live = Live::new(&device_id, tx);
    let keys = (
        pairing::public_key_der(our_cert_der),
        pairing::public_key_der(&link.cert_der),
    );
    let mut state = State {
        dir,
        device_id: device_id.clone(),
        name,
        cert_hash: link.cert_hash,
        keys,
        paired,
        waiting: None,
    };
    let (reader, mut writer) = tokio::io::split(link.stream);
    let mut reader = BufReader::new(reader);
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    'link: loop {
        let out = tokio::select! {
            line = read_packet(&mut reader) => match line {
                Some(packet) => state.packet(&packet),
                None => break,
            },
            Some(event) = events.recv() => {
                if event == Event::Replaced {
                    break;
                }
                state.event(event)
            }
            _ = tick.tick() => state.expire(),
        };
        for packet in out {
            if send(&mut writer, &packet).await.is_err() {
                break 'link;
            }
        }
    }
    if state.waiting.is_some() {
        let _ = devices::clear_pending(&state.dir, &device_id, now_unix());
    }
    tracing::info!(device_id, "A KDE Connect device went");
}

/// A link's pairing state, and what it sends in answer.
struct State {
    dir: PathBuf,
    device_id: String,
    name: String,
    cert_hash: [u8; 32],
    /// Our public key and the device's, for the code.
    keys: (Option<Vec<u8>>, Option<Vec<u8>>),
    paired: bool,
    waiting: Option<Waiting>,
}

impl State {
    fn code(&self, timestamp: u64) -> Option<String> {
        match &self.keys {
            (Some(ours), Some(theirs)) => Some(pairing::code(ours, theirs, timestamp)),
            _ => None,
        }
    }

    /// A packet from the device; what to send back.
    fn packet(&mut self, packet: &NetworkPacket) -> Vec<NetworkPacket> {
        if let Some(says) = pairing::read(packet) {
            return self.pair_packet(says);
        }
        if packet.packet_type == PING && self.paired {
            tracing::info!(device_id = %self.device_id, "A ping from a KDE Connect device");
            notify("Ping from a KDE Connect device", self.name.clone());
        }
        // Unpaired, nothing else is taken; paired, the rest is 3c's.
        Vec::new()
    }

    fn pair_packet(&mut self, says: PairSays) -> Vec<NetworkPacket> {
        let now = now_unix();
        match says {
            PairSays::Yes(_) if self.paired => vec![pairing::answer(true)],
            // Its yes to ours, or its request while ours waits: paired.
            PairSays::Yes(_) if self.waiting.is_some_and(|w| !w.from_device) => {
                self.pair_now();
                Vec::new()
            }
            PairSays::Yes(Some(timestamp)) => {
                if !pairing::timestamp_ok(timestamp, now) {
                    tracing::warn!(device_id = %self.device_id, "A KDE pairing refused: its clock is off");
                    return vec![pairing::answer(false)];
                }
                let Some(code) = self.code(timestamp) else {
                    return vec![pairing::answer(false)];
                };
                let noted = devices::set_pending(
                    &self.dir,
                    Pending {
                        device_id: self.device_id.clone(),
                        name: self.name.clone(),
                        code: code.clone(),
                        asked_unix: now,
                        from_device: true,
                    },
                    now,
                );
                if let Err(e) = noted {
                    tracing::warn!("A KDE pairing could not be held for the owner: {e}");
                    return vec![pairing::answer(false)];
                }
                self.waiting = Some(Waiting {
                    since: now,
                    from_device: true,
                });
                notify(
                    "A KDE Connect device asks to pair",
                    format!(
                        "{}: check the code {code} on it, then allow it in Settings",
                        self.name
                    ),
                );
                Vec::new()
            }
            // A yes with no time, to nothing of ours: ignored.
            PairSays::Yes(None) => Vec::new(),
            PairSays::No => {
                if self.paired {
                    let _ = devices::remove(&self.dir, &self.device_id);
                    self.paired = false;
                    self.note_seen();
                    notify("A KDE Connect device unpaired", self.name.clone());
                }
                if self.waiting.take().is_some() {
                    let _ = devices::clear_pending(&self.dir, &self.device_id, now);
                }
                Vec::new()
            }
        }
    }

    /// Pair the device with the certificate it brought.
    fn pair_now(&mut self) {
        let now = now_unix();
        let added = devices::add(
            &self.dir,
            KdeDevice {
                device_id: self.device_id.clone(),
                name: self.name.clone(),
                cert_hash: devices::hash_hex(self.cert_hash),
                paired_unix: now,
            },
        );
        if let Err(e) = added {
            tracing::warn!("A KDE pairing could not be kept: {e}");
            return;
        }
        self.paired = true;
        self.waiting = None;
        let _ = devices::clear_pending(&self.dir, &self.device_id, now);
        self.note_seen();
        tracing::info!(device_id = %self.device_id, "Paired with a KDE Connect device");
    }

    fn note_seen(&self) {
        let _ = kdeconnect::record(
            &self.dir,
            SeenDevice {
                device_id: self.device_id.clone(),
                name: self.name.clone(),
                seen_unix: now_unix(),
                paired: self.paired,
            },
        );
    }

    /// What the app asks; what to send.
    fn event(&mut self, event: Event) -> Vec<NetworkPacket> {
        let now = now_unix();
        match event {
            Event::PairFromHere if !self.paired && self.waiting.is_none() => {
                let Some(code) = self.code(now) else {
                    return Vec::new();
                };
                let noted = devices::set_pending(
                    &self.dir,
                    Pending {
                        device_id: self.device_id.clone(),
                        name: self.name.clone(),
                        code,
                        asked_unix: now,
                        from_device: false,
                    },
                    now,
                );
                if noted.is_err() {
                    return Vec::new();
                }
                self.waiting = Some(Waiting {
                    since: now,
                    from_device: false,
                });
                vec![pairing::ask(now)]
            }
            Event::PairFromHere => Vec::new(),
            Event::Answer(yes) if self.waiting.is_some_and(|w| w.from_device) => {
                if yes {
                    self.pair_now();
                } else {
                    self.waiting = None;
                    let _ = devices::clear_pending(&self.dir, &self.device_id, now);
                }
                vec![pairing::answer(yes)]
            }
            Event::Answer(_) => Vec::new(),
            Event::Unpair => {
                let _ = devices::remove(&self.dir, &self.device_id);
                self.paired = false;
                self.note_seen();
                vec![pairing::answer(false)]
            }
            Event::Ping if self.paired => vec![NetworkPacket::new(PING)],
            Event::Ping | Event::Replaced => Vec::new(),
        }
    }

    /// A pairing past its time ends: a device's request with a no.
    fn expire(&mut self) -> Vec<NetworkPacket> {
        let Some(waiting) = self.waiting else {
            return Vec::new();
        };
        let now = now_unix();
        if now.saturating_sub(waiting.since) <= devices::PAIRING_TTL_SECS {
            return Vec::new();
        }
        self.waiting = None;
        let _ = devices::clear_pending(&self.dir, &self.device_id, now);
        if waiting.from_device {
            vec![pairing::answer(false)]
        } else {
            Vec::new()
        }
    }
}

async fn send<W: AsyncWrite + Unpin>(
    writer: &mut W,
    packet: &NetworkPacket,
) -> std::io::Result<()> {
    let mut packet = packet.clone();
    // A KDE device reads the body as an object: a null one (a ping's) made
    // the A71 drop the link.
    if packet.body.is_null() {
        packet.body = serde_json::json!({});
    }
    packet.id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let wire = packet.to_wire().map_err(std::io::Error::other)?;
    writer.write_all(&wire).await?;
    writer.flush().await
}

/// The next packet, `None` at the end or on a line too long; a line that
/// is not a packet is skipped.
async fn read_packet<R: AsyncRead + Unpin>(reader: &mut BufReader<R>) -> Option<NetworkPacket> {
    use tokio::io::AsyncReadExt;
    loop {
        let mut line = String::new();
        let read = (&mut *reader)
            .take(MAX_LINE as u64)
            .read_line(&mut line)
            .await
            .ok()?;
        if read == 0 || (read == MAX_LINE && !line.ends_with('\n')) {
            return None;
        }
        if let Ok(packet) = NetworkPacket::from_wire(&line) {
            return Some(packet);
        }
    }
}

/// Whether a device is paired with this computer, for the handshake's pin.
pub fn pin_of(dir: &Path, device_id: &str) -> Option<[u8; 32]> {
    devices::pinned(dir, device_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(name: &str, paired: bool) -> State {
        let dir = std::env::temp_dir().join(format!(
            "pd-kde-link-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        State {
            dir,
            device_id: "d".repeat(32),
            name: "Galaxy A71".to_string(),
            cert_hash: [9; 32],
            keys: (Some(vec![1; 91]), Some(vec![2; 91])),
            paired,
            waiting: None,
        }
    }

    fn says(packet: &NetworkPacket) -> Option<PairSays> {
        pairing::read(packet)
    }

    #[test]
    fn a_devices_request_waits_for_the_owner_and_allow_pairs_it() {
        let mut s = state("allow", false);
        let now = now_unix();
        assert!(
            s.pair_packet(PairSays::Yes(Some(now))).is_empty(),
            "held, not answered"
        );
        let held = devices::pending(&s.dir, now);
        assert_eq!(held.len(), 1);
        assert!(held[0].from_device);
        assert_eq!(held[0].code.len(), 8);
        let out = s.event(Event::Answer(true));
        assert_eq!(says(&out[0]), Some(PairSays::Yes(None)));
        assert_eq!(devices::pinned(&s.dir, &s.device_id), Some([9; 32]));
        assert!(devices::pending(&s.dir, now).is_empty());
    }

    #[test]
    fn a_denied_or_stale_request_pairs_nothing() {
        let mut s = state("deny", false);
        let now = now_unix();
        s.pair_packet(PairSays::Yes(Some(now)));
        let out = s.event(Event::Answer(false));
        assert_eq!(says(&out[0]), Some(PairSays::No));
        assert_eq!(devices::pinned(&s.dir, &s.device_id), None);

        let mut s = state("skew", false);
        let out = s.pair_packet(PairSays::Yes(Some(now - pairing::MAX_SKEW_SECS - 5)));
        assert_eq!(says(&out[0]), Some(PairSays::No), "its clock is off");
        assert!(devices::pending(&s.dir, now).is_empty());

        let mut s = state("expire", false);
        s.pair_packet(PairSays::Yes(Some(now)));
        s.waiting = Some(Waiting {
            since: now - devices::PAIRING_TTL_SECS - 1,
            from_device: true,
        });
        let out = s.expire();
        assert_eq!(says(&out[0]), Some(PairSays::No), "no answer is a no");
    }

    #[test]
    fn pairing_from_here_pairs_on_the_devices_yes() {
        let mut s = state("here", false);
        let out = s.event(Event::PairFromHere);
        assert!(matches!(says(&out[0]), Some(PairSays::Yes(Some(_)))));
        assert!(s.pair_packet(PairSays::Yes(None)).is_empty());
        assert!(s.paired);
        assert_eq!(devices::pinned(&s.dir, &s.device_id), Some([9; 32]));
    }

    #[test]
    fn an_answer_without_a_request_and_a_yes_to_nothing_do_nothing() {
        let mut s = state("nothing", false);
        assert!(s.event(Event::Answer(true)).is_empty());
        assert!(s.pair_packet(PairSays::Yes(None)).is_empty());
        assert!(!s.paired);
        assert_eq!(devices::pinned(&s.dir, &s.device_id), None);
    }

    #[test]
    fn unpairing_either_way_forgets_the_pin() {
        let mut s = state("unpair", false);
        s.pair_now();
        assert!(s.paired);
        assert!(s.pair_packet(PairSays::No).is_empty());
        assert!(!s.paired);
        assert_eq!(devices::pinned(&s.dir, &s.device_id), None);
        s.pair_now();
        let out = s.event(Event::Unpair);
        assert_eq!(says(&out[0]), Some(PairSays::No));
        assert_eq!(devices::pinned(&s.dir, &s.device_id), None);
    }

    #[tokio::test]
    async fn every_packet_goes_with_an_object_for_a_body() {
        let mut wire = Vec::new();
        send(&mut wire, &NetworkPacket::new(PING)).await.unwrap();
        let sent: serde_json::Value = serde_json::from_slice(&wire).unwrap();
        assert!(sent["body"].is_object(), "{sent}");
        assert!(sent["id"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn a_newer_link_takes_the_older_ones_place_and_its_going_leaves_the_new_one() {
        let id = "r".repeat(32);
        let (old_tx, mut old_rx) = mpsc::channel(4);
        let old = Live::new(&id, old_tx);
        let (new_tx, mut new_rx) = mpsc::channel(4);
        let new = Live::new(&id, new_tx);
        assert_eq!(
            old_rx.try_recv().unwrap(),
            Event::Replaced,
            "the old link is closed"
        );
        drop(old);
        assert!(tell(&id, Event::Ping), "the new link is still there");
        assert_eq!(new_rx.try_recv().unwrap(), Event::Ping);
        drop(new);
        assert!(!tell(&id, Event::Ping));
    }

    #[test]
    fn only_a_paired_device_is_pinged() {
        let mut s = state("ping", false);
        assert!(s.event(Event::Ping).is_empty());
        s.pair_now();
        assert_eq!(s.event(Event::Ping)[0].packet_type, PING);
    }
}
