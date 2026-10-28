//! Shared input's channel on the link between two of the owner's computers.
//!
//! It rides the iroh connection their link already keeps (2f): the computer
//! that paired dials the other and is its client. Each side registers that
//! connection here while the link is up (`peer_computers::run_link` on the
//! dialing side, `service::serve_control` on the other once it admitted a
//! computer), and a task reads what comes on it. Nothing else uses
//! datagrams or uni streams on that connection, so they are this channel's:
//! a pointer position, an activity note and a ping or its answer go as one
//! datagram each (the newest matters, and a lost one is not worth waiting
//! for); everything else is framed ([`wire::frame`]) on one ordered uni
//! stream per direction, opened with [`wire::STREAM_MAGIC`] and
//! [`wire::VERSION`]. A link whose traffic rides a relay is refused: the
//! keyboard and mouse are shared at home only.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use iroh::endpoint::{Connection, RecvStream, SendStream};
use tokio::sync::mpsc;

use super::wire::{self, Msg};

/// How many messages wait for whoever takes [`incoming`] before a stream's
/// reader waits too (a datagram is dropped instead).
const INCOMING_DEPTH: usize = 1024;

/// How many frames wait for a channel's stream before [`Channel::send`]
/// says the other computer is not taking them.
const OUTGOING_DEPTH: usize = 1024;

/// A stream's opening (the magic and the version) must come this soon.
const HEAD_WAIT: Duration = Duration::from_secs(5);

/// The channel's stream goes ahead of the control stream (priority 0) and
/// of files (-1) on the connection: a key is a few bytes, and late is wrong.
const STREAM_PRIORITY: i32 = 1;

/// The computers' connections registered now, by the other computer's id.
static LINKS: LazyLock<Mutex<HashMap<String, Registered>>> = LazyLock::new(Default::default);

/// A message from another computer: that computer's id, the message, when
/// it came off the link ([`now_us`]), and the link it came on (its
/// connection's `stable_id`).
pub type Arrival = (String, Msg, u64, usize);

/// Where what arrives goes: the receiver [`incoming`] handed out last.
static INCOMING: LazyLock<Mutex<Option<mpsc::Sender<Arrival>>>> = LazyLock::new(Default::default);

/// The way to the [`Channel`] waiting for the answers to its pings.
type PongsIn = mpsc::UnboundedSender<Pong>;

/// Where the answers to this computer's pings go, by the computer pinged
/// and the reading the ping carried: the [`Channel`] that sent that ping.
/// Several channels to one computer (a driver's, and one opened for a
/// refusal or an Identify) each hear only their own.
static PONGS: LazyLock<Mutex<HashMap<(String, u64), PongsIn>>> = LazyLock::new(Default::default);

/// A ping unanswered this long is forgotten (its answer is no use then).
const PONG_FORGET_US: u64 = 60_000_000;

/// The way to the [`Channel`] waiting for a computer's timings.
type TimingsIn = mpsc::UnboundedSender<Vec<wire::Timing>>;

/// Where the timings a computer sends back ([`Msg::Timings`]) go, by that
/// computer: the [`Channel`] that last asked it for them.
static TIMINGS: LazyLock<Mutex<HashMap<String, TimingsIn>>> = LazyLock::new(Default::default);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// A registered connection, and the task reading what comes on it.
struct Registered {
    conn: Connection,
    reader: tokio::task::AbortHandle,
}

impl Drop for Registered {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// Take `conn` as the link to the computer `device_id`, in place of any
/// earlier one, and read what comes on it. Called on a runtime.
pub fn register(device_id: &str, conn: Connection) {
    let reader = tokio::spawn(read_link(device_id.to_string(), conn.clone())).abort_handle();
    let stable_id = conn.stable_id();
    let replaced = lock(&LINKS).insert(device_id.to_string(), Registered { conn, reader });
    // An answer on the link replaced is no answer on this one.
    if let Some(old) = replaced.as_ref().map(|r| r.conn.stable_id())
        && old != stable_id
    {
        forget_answer(device_id, old);
    }
    // Its reader is stopped outside the lock.
    drop(replaced);
}

/// Forget the link to `device_id` and stop reading it.
pub fn unregister(device_id: &str) {
    let gone = lock(&LINKS).remove(device_id);
    if let Some(gone) = &gone {
        forget_answer(device_id, gone.conn.stable_id());
    }
    drop(gone);
}

/// The connection to the computer `device_id`, while its link is up.
pub fn connection(device_id: &str) -> Option<Connection> {
    lock(&LINKS).get(device_id).map(|r| r.conn.clone())
}

/// The computers whose links are registered now.
pub(crate) fn linked() -> Vec<String> {
    lock(&LINKS).keys().cloned().collect()
}

/// When each computer last sent its screens ([`Msg::Screens`], which a
/// computer sends only to one it lets drive it: in answer to its `Hello`,
/// and when its screens change), counted only on the link this computer
/// greeted it on ([`GREETED`]) and forgotten when that link goes. The
/// driving side takes a computer that answered this way since its channel
/// opened as one it may cross onto (`drive::reachable`); a `Hello` from
/// that computer's own driver is no answer, and nor are screens it sent
/// unasked.
/// Each with the link it came on.
static SCREENS_HEARD: LazyLock<Mutex<HashMap<String, (std::time::Instant, usize)>>> =
    LazyLock::new(Default::default);

/// The computers this one said hello to ([`Msg::Hello`]), each with the
/// link it said it on (its connection's `stable_id`).
static GREETED: LazyLock<Mutex<HashMap<String, usize>>> = LazyLock::new(Default::default);

/// Note that `device_id` just sent its screens on the link `link` (its
/// connection's `stable_id`): when this computer greeted it on that link,
/// it takes input from this one, whatever it refused before. Whether it
/// counted.
pub fn note_screens(device_id: &str, link: usize) -> bool {
    if lock(&GREETED).get(device_id) != Some(&link) {
        return false;
    }
    lock(&REFUSED).remove(device_id);
    lock(&SCREENS_HEARD).insert(device_id.to_string(), (std::time::Instant::now(), link));
    true
}

/// Whether `device_id` answered this computer's hello with its screens on
/// the link up now (and has not refused since): this computer may drive
/// it.
pub fn answered(device_id: &str) -> bool {
    lock(&SCREENS_HEARD).contains_key(device_id)
}

/// The link `link` to `device_id` went or was replaced: its greeting and
/// answer go with it. Those on a newer link stay (a hello said on it just
/// as it was registered).
fn forget_answer(device_id: &str, link: usize) {
    let mut greeted = lock(&GREETED);
    if greeted.get(device_id) == Some(&link) {
        greeted.remove(device_id);
    }
    drop(greeted);
    let mut heard = lock(&SCREENS_HEARD);
    if heard.get(device_id).is_some_and(|(_, on)| *on == link) {
        heard.remove(device_id);
    }
}

/// The computers that last answered this one by refusing to be driven
/// ([`Msg::Refuse`]): the reason, and when.
static REFUSED: LazyLock<Mutex<HashMap<String, (u8, std::time::Instant)>>> =
    LazyLock::new(Default::default);

/// Note that `device_id` refused to be driven (`reason`, as
/// [`Msg::Refuse`] says it): its answer before no longer counts, so it is
/// no computer to cross onto until it sends its screens again.
pub fn note_refused(device_id: &str, reason: u8) {
    lock(&SCREENS_HEARD).remove(device_id);
    lock(&REFUSED).insert(device_id.to_string(), (reason, std::time::Instant::now()));
}

/// The computers refusing to be driven now: why, and since when.
pub fn refusals() -> HashMap<String, (u8, std::time::Instant)> {
    lock(&REFUSED).clone()
}

/// When each computer last sent its screens.
pub fn screens_heard() -> HashMap<String, std::time::Instant> {
    lock(&SCREENS_HEARD)
        .iter()
        .map(|(id, (at, _))| (id.clone(), *at))
        .collect()
}

/// Whether `conn`'s traffic rides a direct path now: its selected path is
/// not a relay's, as `IrohConnection::stats` reads it. iroh keeps trying to
/// punch a direct path while the traffic rides a relay, so this can turn
/// true on its own; with no path selected yet it is false.
pub fn is_direct(conn: &Connection) -> bool {
    pocketdesk_core::streaming::iroh_connection::selected_path(&conn.paths())
        .is_some_and(|path| !path.is_relay())
}

/// A connection registered while this is held, and unregistered however
/// its holder ends (a link's task aborted too), unless a newer connection
/// to that computer took its place meanwhile.
#[must_use = "the link is unregistered when this is dropped"]
pub(crate) struct Held {
    device_id: String,
    stable_id: usize,
}

/// [`register`] `conn` as the link to `device_id` while the result is held.
pub(crate) fn hold(device_id: &str, conn: Connection) -> Held {
    let stable_id = conn.stable_id();
    register(device_id, conn);
    Held {
        device_id: device_id.to_string(),
        stable_id,
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let mut links = lock(&LINKS);
        let ours = links
            .get(&self.device_id)
            .is_some_and(|r| r.conn.stable_id() == self.stable_id);
        let gone = if ours {
            links.remove(&self.device_id)
        } else {
            None
        };
        drop(links);
        if gone.is_some() {
            forget_answer(&self.device_id, self.stable_id);
        }
        drop(gone);
    }
}

/// What the other computers send from now on, with each one's id and when
/// it came off the link: one receiver at a time, so a later call takes them
/// from the earlier one, whose receiver then ends. Pings are in it (each
/// already answered), the answers to this computer's own pings and asks
/// are not (they go to the [`Channel`] that sent them). With no receiver
/// taken, what arrives is dropped.
pub fn incoming() -> mpsc::Receiver<Arrival> {
    let (tx, rx) = mpsc::channel(INCOMING_DEPTH);
    *lock(&INCOMING) = Some(tx);
    rx
}

fn incoming_sender() -> Option<mpsc::Sender<Arrival>> {
    lock(&INCOMING).clone()
}

/// When anything last came off the link from each computer, handed on yet
/// or not.
static LAST_HEARD: LazyLock<Mutex<HashMap<String, std::time::Instant>>> =
    LazyLock::new(Default::default);

fn heard_now(device_id: &str) {
    let now = std::time::Instant::now();
    let mut heard = lock(&LAST_HEARD);
    match heard.get_mut(device_id) {
        Some(at) => *at = now,
        None => {
            heard.insert(device_id.to_string(), now);
        }
    }
}

/// When the last message from `device_id` came off its link, whether or
/// not whoever takes [`incoming`] has got to it: the computer being driven
/// counts its driver's silence from this, so a moment's hold-up in its own
/// loop is no silence.
pub fn last_heard(device_id: &str) -> Option<std::time::Instant> {
    lock(&LAST_HEARD).get(device_id).copied()
}

/// Read what the computer `device_id` sends on `conn` until it closes: its
/// datagrams, and each uni stream it opens, each in a task of its own (a
/// frame is read in parts, so a read is never raced against another).
async fn read_link(device_id: String, conn: Connection) {
    let mut streams = tokio::task::JoinSet::new();
    loop {
        // Both are cancel-safe: each takes its stream or datagram only in
        // the poll that returns it.
        tokio::select! {
            stream = conn.accept_uni() => match stream {
                Ok(stream) => {
                    streams.spawn(read_stream(device_id.clone(), conn.clone(), stream));
                }
                Err(_) => break,
            },
            datagram = conn.read_datagram() => match datagram {
                Ok(bytes) => match wire::decode(&bytes) {
                    Ok(msg) => {
                        heard_now(&device_id);
                        let at_us = now_us();
                        if answer(&device_id, &conn, &msg)
                            && let Some(incoming) = incoming_sender()
                        {
                            // A datagram is dropped rather than waited for.
                            let arrival = (device_id.clone(), msg, at_us, conn.stable_id());
                            let _ = incoming.try_send(arrival);
                        }
                    }
                    Err(e) => tracing::debug!(device_id, "A shared-input datagram did not read: {e}"),
                },
                Err(_) => break,
            },
            Some(_) = streams.join_next(), if !streams.is_empty() => {}
        }
    }
    // The connection is gone: its streams end on their own.
    while streams.join_next().await.is_some() {}
}

/// Read one uni stream the computer `device_id` opened: shared input when it
/// opens with the magic, else dropped.
async fn read_stream(device_id: String, conn: Connection, mut stream: RecvStream) {
    let mut head = [0u8; 5];
    let opened = tokio::time::timeout(HEAD_WAIT, stream.read_exact(&mut head)).await;
    if !matches!(opened, Ok(Ok(()))) || head[..4] != wire::STREAM_MAGIC {
        tracing::debug!(
            device_id,
            "A stream from another computer that is not shared input; dropped"
        );
        let _ = stream.stop(0u32.into());
        return;
    }
    let version = head[4];
    loop {
        let msg = match wire::read_frame(&mut stream).await {
            Ok(Some(msg)) => msg,
            Ok(None) => return,
            Err(e) => {
                tracing::debug!(device_id, "A shared-input stream broke off: {e}");
                return;
            }
        };
        if version != wire::VERSION {
            // Only a Hello reads the same in every version: it is handed on,
            // so that computer can be named, and the rest is left unread.
            tracing::info!(
                device_id,
                version,
                "Another computer shares input in another version"
            );
            if matches!(msg, Msg::Hello { .. }) {
                hand_on(&device_id, msg, now_us(), conn.stable_id()).await;
            }
            let _ = stream.stop(0u32.into());
            return;
        }
        heard_now(&device_id);
        let at_us = now_us();
        if answer(&device_id, &conn, &msg) {
            hand_on(&device_id, msg, at_us, conn.stable_id()).await;
        }
    }
}

/// Hand `msg` on to whoever takes [`incoming`], waiting while it is full:
/// what the stream carries is not dropped.
async fn hand_on(device_id: &str, msg: Msg, at_us: u64, link: usize) {
    if let Some(incoming) = incoming_sender() {
        let _ = incoming
            .send((device_id.to_string(), msg, at_us, link))
            .await;
    }
}

/// What the link itself does with `msg` from `device_id`: a ping is
/// answered at once (with this computer's clock), a pong goes to the
/// channel that sent its ping and timings to the one that asked for them. Whether it is handed on too (all but
/// those two).
fn answer(device_id: &str, conn: &Connection, msg: &Msg) -> bool {
    match msg {
        Msg::Ping { t_us } => {
            let pong = wire::encode(&Msg::Pong {
                t_us: *t_us,
                at_us: now_us(),
            });
            if let Err(e) = conn.send_datagram(pong.into()) {
                tracing::debug!(device_id, "A ping went unanswered: {e}");
            }
            true
        }
        Msg::Pong { t_us, at_us } => {
            let pong = Pong {
                sent_us: *t_us,
                back_us: now_us(),
                at_us: *at_us,
            };
            if let Some(waiting) = lock(&PONGS).remove(&(device_id.to_string(), *t_us)) {
                let _ = waiting.send(pong);
            }
            false
        }
        Msg::Timings(timings) => {
            if let Some(waiting) = lock(&TIMINGS).get(device_id) {
                let _ = waiting.send(timings.clone());
            }
            false
        }
        _ => true,
    }
}

/// Now, in microseconds since the Unix epoch: what a ping carries.
pub fn now_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_micros()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// An answer to one of this computer's pings: the reading the ping carried
/// ([`now_us`] as it was sent) and when the answer came back, both on this
/// computer's clock; and the other computer's clock as it answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pong {
    pub sent_us: u64,
    pub back_us: u64,
    pub at_us: u64,
}

impl Pong {
    pub fn round_trip(&self) -> Duration {
        Duration::from_micros(self.back_us.saturating_sub(self.sent_us))
    }
}

/// Whether `msg` goes as a datagram: the newest of each matters, and a lost
/// one is not worth the wait for its resend.
fn rides_datagram(msg: &Msg) -> bool {
    matches!(
        msg,
        Msg::Move { .. } | Msg::Active { .. } | Msg::Ping { .. } | Msg::Pong { .. }
    )
}

/// Sending shared input to one computer on its link.
pub struct Channel {
    device_id: String,
    conn: Connection,
    runtime: tokio::runtime::Handle,
    /// The frames for the stream, to the task writing them; that task and
    /// the stream start with the first.
    stream: Option<mpsc::Sender<Vec<u8>>>,
    /// The task writing the stream, once it started.
    writer: Option<tokio::task::JoinHandle<()>>,
    /// Why the stream stopped, once it did.
    broken: Arc<Mutex<Option<String>>>,
    pongs: mpsc::UnboundedReceiver<Pong>,
    /// The way into `pongs`, given with each ping sent and taken back on
    /// drop.
    pongs_in: PongsIn,
    /// The timings the other computer sends back, and the way into them.
    timings: mpsc::UnboundedReceiver<Vec<wire::Timing>>,
    timings_in: TimingsIn,
}

impl Channel {
    /// A channel to the computer `device_id`, refused when its link is not
    /// up or its traffic rides a relay. Called on a runtime.
    pub fn open(device_id: &str) -> Result<Channel, String> {
        Self::open_with(device_id, is_direct)
    }

    /// [`Channel::open`], with whether a connection is direct decided by
    /// `direct` (a seam for the tests).
    fn open_with(device_id: &str, direct: impl Fn(&Connection) -> bool) -> Result<Channel, String> {
        let conn = connection(device_id).ok_or("that computer is not linked now")?;
        if !direct(&conn) {
            return Err(
                "the link to that computer goes through a relay: the keyboard and mouse are \
                 shared only on a direct link, at home"
                    .to_string(),
            );
        }
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| "no runtime to send shared input on".to_string())?;
        // Nothing is routed here until this channel pings or asks: a
        // channel opened beside another takes none of its answers.
        let (pongs_in, pongs) = mpsc::unbounded_channel();
        let (timings_in, timings) = mpsc::unbounded_channel();
        Ok(Channel {
            device_id: device_id.to_string(),
            conn,
            runtime,
            stream: None,
            writer: None,
            broken: Arc::default(),
            pongs,
            pongs_in,
            timings,
            timings_in,
        })
    }

    /// Send `msg`: a move, an activity note, a ping or a pong as one
    /// datagram, the rest in order on this channel's stream (opened with the
    /// first). Never waits; an error means the link closed, the stream
    /// broke, or the other computer is not taking what was sent.
    pub fn send(&mut self, msg: &Msg) -> Result<(), String> {
        if let Some(why) = self.conn.close_reason() {
            return Err(format!("the link closed: {why}"));
        }
        if let Some(why) = lock(&self.broken).clone() {
            return Err(why);
        }
        match msg {
            Msg::Hello { .. } => {
                lock(&GREETED).insert(self.device_id.clone(), self.conn.stable_id());
            }
            Msg::Ping { t_us } => {
                let mut pongs = lock(&PONGS);
                // Pings never answered (a link that went quiet) are let go.
                let fresh = now_us().saturating_sub(PONG_FORGET_US);
                pongs.retain(|(_, sent), _| *sent >= fresh);
                pongs.insert((self.device_id.clone(), *t_us), self.pongs_in.clone());
            }
            Msg::AskTimings => {
                lock(&TIMINGS).insert(self.device_id.clone(), self.timings_in.clone());
            }
            _ => {}
        }
        if rides_datagram(msg) {
            return self
                .conn
                .send_datagram(wire::encode(msg).into())
                .map_err(|e| format!("not sent: {e}"));
        }
        let frames = match &self.stream {
            Some(frames) => frames,
            None => {
                let (frames, waiting) = mpsc::channel(OUTGOING_DEPTH);
                self.writer = Some(self.runtime.spawn(write_stream(
                    self.conn.clone(),
                    waiting,
                    Arc::clone(&self.broken),
                )));
                self.stream.insert(frames)
            }
        };
        frames.try_send(wire::frame(msg)).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => {
                "the other computer is not taking shared input".to_string()
            }
            mpsc::error::TrySendError::Closed(_) => lock(&self.broken)
                .clone()
                .unwrap_or_else(|| "the stream closed".to_string()),
        })
    }

    /// The next answer to one of this channel's pings, waited for at most
    /// `within`.
    pub async fn pong(&mut self, within: Duration) -> Option<Pong> {
        tokio::time::timeout(within, self.pongs.recv())
            .await
            .ok()
            .flatten()
    }

    /// Close the channel, waiting at most `within` for what was sent on its
    /// stream to be written to the link (the stream finished). Blocking:
    /// for a thread of its own, never the runtime's.
    pub fn finish_within(mut self, within: Duration) {
        drop(self.stream.take());
        let Some(writer) = self.writer.take() else {
            return;
        };
        let deadline = std::time::Instant::now() + within;
        while !writer.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// An answer to one of this channel's pings that has come, if any,
    /// without waiting.
    pub fn try_pong(&mut self) -> Option<Pong> {
        self.pongs.try_recv().ok()
    }

    /// The timings the other computer sent back after a
    /// [`Msg::AskTimings`], waited for at most `within`.
    pub async fn timings(&mut self, within: Duration) -> Option<Vec<wire::Timing>> {
        tokio::time::timeout(within, self.timings.recv())
            .await
            .ok()
            .flatten()
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        lock(&PONGS).retain(|_, waiting| !waiting.same_channel(&self.pongs_in));
        let mut timings = lock(&TIMINGS);
        if timings
            .get(&self.device_id)
            .is_some_and(|waiting| waiting.same_channel(&self.timings_in))
        {
            timings.remove(&self.device_id);
        }
    }
}

/// Write `frames` in order on a uni stream opened for them, the magic and
/// the version first; finish it once the channel is gone. Why it stopped,
/// if it broke, goes in `broken`.
async fn write_stream(
    conn: Connection,
    mut frames: mpsc::Receiver<Vec<u8>>,
    broken: Arc<Mutex<Option<String>>>,
) {
    let written: Result<(), String> = async {
        let mut stream: SendStream = conn
            .open_uni()
            .await
            .map_err(|e| format!("no stream for shared input: {e}"))?;
        let _ = stream.set_priority(STREAM_PRIORITY);
        let mut out = wire::STREAM_MAGIC.to_vec();
        out.push(wire::VERSION);
        while let Some(frame) = frames.recv().await {
            out.extend_from_slice(&frame);
            // Whatever else is waiting goes in the same write.
            while let Ok(frame) = frames.try_recv() {
                out.extend_from_slice(&frame);
            }
            stream
                .write_all(&out)
                .await
                .map_err(|e| format!("the shared-input stream broke: {e}"))?;
            out.clear();
        }
        let _ = stream.finish();
        Ok(())
    }
    .await;
    if let Err(why) = written {
        *lock(&broken) = Some(why);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::protocol::ALPN_CONTROL;

    async fn endpoint() -> iroh::Endpoint {
        iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .secret_key(iroh::SecretKey::generate())
            .alpns(vec![ALPN_CONTROL.to_vec()])
            .relay_mode(iroh::RelayMode::Disabled)
            .transport_config(pocketdesk_core::streaming::iroh_connection::iroh_transport())
            .bind_addr(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .unwrap()
            .bind()
            .await
            .unwrap()
    }

    /// Two computers' ends of one connection on this machine: the dialing
    /// one's, then the dialed one's.
    async fn linked_pair() -> (iroh::Endpoint, iroh::Endpoint, Connection, Connection) {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let dialed = endpoint().await;
        let dialing = endpoint().await;
        let addr = iroh::EndpointAddr::new(dialed.id()).with_ip_addr(dialed.bound_sockets()[0]);
        let accepting = {
            let dialed = dialed.clone();
            tokio::spawn(async move { dialed.accept().await.unwrap().await.unwrap() })
        };
        let out = dialing.connect(addr, ALPN_CONTROL).await.unwrap();
        let back = accepting.await.unwrap();
        (dialing, dialed, out, back)
    }

    #[tokio::test]
    async fn relayed_link_refuses() {
        let (a, b, out, _back) = linked_pair().await;
        let err = Channel::open("test-never-linked").err().unwrap();
        assert!(err.contains("not linked"), "{err}");
        register("test-relayed", out);
        let err = Channel::open_with("test-relayed", |_| false).err().unwrap();
        assert!(err.contains("relay"), "{err}");
        assert!(Channel::open_with("test-relayed", |_| true).is_ok());
        unregister("test-relayed");
        assert!(connection("test-relayed").is_none());
        a.close().await;
        b.close().await;
    }

    /// Over loopback the path is an IP one: direct, as at home.
    #[tokio::test]
    async fn a_loopback_link_is_direct() {
        let (a, b, out, _back) = linked_pair().await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !is_direct(&out) {
            assert!(tokio::time::Instant::now() < deadline, "direct in time");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        a.close().await;
        b.close().await;
    }

    /// A newer connection to a computer is not unregistered when the older
    /// one's holder ends (a link redialed before the old one is noticed
    /// gone).
    #[tokio::test]
    async fn the_older_holder_leaves_a_newer_link_alone() {
        let (a, b, first, second) = linked_pair().await;
        let first_id = first.stable_id();
        let second_id = second.stable_id();
        assert_ne!(first_id, second_id);
        let older = hold("test-held", first);
        let newer = hold("test-held", second);
        drop(older);
        assert_eq!(
            connection("test-held").map(|c| c.stable_id()),
            Some(second_id)
        );
        drop(newer);
        assert!(connection("test-held").is_none());
        a.close().await;
        b.close().await;
    }

    /// The one test that takes [`incoming`] (one receiver at a time): the
    /// stream's messages arrive in order, a stream without the magic is
    /// dropped, moves cross as datagrams, and a ping comes back answered.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn messages_cross_in_order_and_pings_come_back() {
        let (a, b, out, back) = linked_pair().await;
        let mut arriving = incoming();
        let _dialing = hold("test-pc-b", out.clone());
        let _dialed = hold("test-pc-a", back);

        // A stream that is not shared input: dropped, not read as frames.
        let mut stray = out.open_uni().await.unwrap();
        let mut bytes = b"XXXX".to_vec();
        bytes.push(wire::VERSION);
        bytes.extend(wire::frame(&Msg::Key {
            code: 1,
            pressed: true,
        }));
        stray.write_all(&bytes).await.unwrap();
        stray.finish().unwrap();

        let mut channel = Channel::open_with("test-pc-b", |_| true).unwrap();
        let hello = Msg::Hello {
            version: wire::VERSION,
            name: "A".to_string(),
            screens: Vec::new(),
        };
        let mut sent = vec![hello.clone(), Msg::Enter { x: 1, y: 2 }];
        for i in 0..100u16 {
            sent.push(Msg::Key {
                code: 30 + i % 9,
                pressed: i % 2 == 0,
            });
        }
        sent.push(Msg::Leave);
        for (i, msg) in sent.iter().enumerate() {
            channel.send(msg).unwrap();
            if i % 10 == 0 {
                channel
                    .send(&Msg::Move {
                        seq: i as u32,
                        x: 3,
                        y: 4,
                    })
                    .unwrap();
            }
        }
        channel.send(&Msg::Ping { t_us: now_us() }).unwrap();

        let mut on_stream = Vec::new();
        let mut moves = 0;
        let mut pinged = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while on_stream.last() != Some(&Msg::Leave) || !pinged {
            let (from, msg, _, _) = tokio::time::timeout_at(deadline, arriving.recv())
                .await
                .expect("everything in time")
                .expect("still taken");
            assert_eq!(from, "test-pc-a", "named by the receiving side's id");
            match msg {
                Msg::Move { .. } => moves += 1,
                Msg::Ping { .. } => pinged = true,
                Msg::Pong { .. } => panic!("a pong is not handed on"),
                other => on_stream.push(other),
            }
        }
        assert_eq!(on_stream, sent, "the stream's messages, in order");
        assert!(moves > 0, "moves cross as datagrams");
        let pong = channel
            .pong(Duration::from_secs(5))
            .await
            .expect("the ping answered");
        assert!(pong.round_trip() < Duration::from_secs(5));
        drop(channel);
        a.close().await;
        b.close().await;
    }

    /// A channel opened and dropped beside a live one (a refusal, an
    /// Identify) leaves the live one's pongs alone: each pong goes to the
    /// channel whose ping it answers.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_short_lived_channel_leaves_a_live_ones_pongs_alone() {
        let (a, b, out, back) = linked_pair().await;
        // The other end answers pings by hand: nothing it reads is handed
        // to `incoming`, which another test may hold.
        let answering = tokio::spawn(async move {
            while let Ok(bytes) = back.read_datagram().await {
                if let Ok(Msg::Ping { t_us }) = wire::decode(&bytes) {
                    let pong = Msg::Pong {
                        t_us,
                        at_us: now_us(),
                    };
                    let _ = back.send_datagram(wire::encode(&pong).into());
                }
            }
        });
        let _held = hold("test-pongs", out);
        let mut live = Channel::open_with("test-pongs", |_| true).unwrap();
        let mut short = Channel::open_with("test-pongs", |_| true).unwrap();
        // Each gets the answer to its own ping, not the other's.
        let (live_t, short_t) = (now_us(), now_us() + 1);
        live.send(&Msg::Ping { t_us: live_t }).unwrap();
        short.send(&Msg::Ping { t_us: short_t }).unwrap();
        let got = short.pong(Duration::from_secs(5)).await.expect("its pong");
        assert_eq!(got.sent_us, short_t);
        let got = live.pong(Duration::from_secs(5)).await.expect("its pong");
        assert_eq!(got.sent_us, live_t);
        drop(short);
        let t_us = now_us() + 2;
        live.send(&Msg::Ping { t_us }).unwrap();
        let got = live
            .pong(Duration::from_secs(5))
            .await
            .expect("the live channel still hears its answers");
        assert_eq!(got.sent_us, t_us);
        drop(live);
        answering.abort();
        a.close().await;
        b.close().await;
    }

    /// Screens count as an answer only on the link this computer said
    /// hello on: not sent unasked, and not once that link goes.
    #[tokio::test]
    async fn screens_count_only_as_an_answer_to_a_hello_on_the_link_up() {
        let (a, b, out, back) = linked_pair().await;
        let link = out.stable_id();
        assert!(!note_screens("test-answer", link), "no hello");
        let held = hold("test-answer", out);
        assert!(!note_screens("test-answer", link), "unasked");
        assert!(!answered("test-answer"));
        let mut channel = Channel::open_with("test-answer", |_| true).unwrap();
        channel
            .send(&Msg::Hello {
                version: wire::VERSION,
                name: "A".to_string(),
                screens: Vec::new(),
            })
            .unwrap();
        // Screens late from another link do not count.
        assert!(!note_screens("test-answer", back.stable_id()));
        assert!(note_screens("test-answer", link), "an answer to the hello");
        assert!(answered("test-answer"));
        drop(channel);
        drop(held);
        assert!(!answered("test-answer"), "forgotten with the link");
        assert!(!note_screens("test-answer", link));
        a.close().await;
        b.close().await;
    }

    /// The link that goes takes only its own greeting and answer: a hello
    /// said on the newer link just as it was registered stays.
    #[test]
    fn a_replaced_link_forgets_only_its_own_answer() {
        lock(&GREETED).insert("test-replace".to_string(), 2);
        forget_answer("test-replace", 1);
        assert!(
            note_screens("test-replace", 2),
            "the newer link's hello kept"
        );
        forget_answer("test-replace", 1);
        assert!(answered("test-replace"), "the newer link's answer kept");
        forget_answer("test-replace", 2);
        assert!(!answered("test-replace"));
        assert!(!note_screens("test-replace", 2));
    }

    #[test]
    fn only_the_newest_kind_rides_datagrams() {
        assert!(rides_datagram(&Msg::Move { seq: 1, x: 0, y: 0 }));
        assert!(rides_datagram(&Msg::Active { idle_ms: 0 }));
        assert!(rides_datagram(&Msg::Ping { t_us: 0 }));
        assert!(rides_datagram(&Msg::Pong { t_us: 0, at_us: 0 }));
        assert!(!rides_datagram(&Msg::Key {
            code: 30,
            pressed: true
        }));
        assert!(!rides_datagram(&Msg::Enter { x: 0, y: 0 }));
        assert!(!rides_datagram(&Msg::Leave));
        assert!(!rides_datagram(&Msg::ReleaseAll));
    }
}
