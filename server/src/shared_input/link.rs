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

/// A message from another computer, with that computer's id.
type Arrival = (String, Msg);

/// Where what arrives goes: the receiver [`incoming`] handed out last.
static INCOMING: LazyLock<Mutex<Option<mpsc::Sender<Arrival>>>> = LazyLock::new(Default::default);

/// Where the answers to this computer's pings go, by the computer that
/// answered: the [`Channel`] opened to it last.
static PONGS: LazyLock<Mutex<HashMap<String, mpsc::UnboundedSender<Pong>>>> =
    LazyLock::new(Default::default);

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
    let replaced = lock(&LINKS).insert(device_id.to_string(), Registered { conn, reader });
    // Its reader is stopped outside the lock.
    drop(replaced);
}

/// Forget the link to `device_id` and stop reading it.
pub fn unregister(device_id: &str) {
    let gone = lock(&LINKS).remove(device_id);
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

/// When a shared-input message (a `Hello` or `Screens`) was last heard from
/// each computer: the driving side takes a computer that answered its
/// `Hello` within a short window as one it may cross onto, so sharing turned
/// off or the permission revoked on the other side stops being a crossing
/// target once it goes quiet.
static SEEN: LazyLock<Mutex<HashMap<String, std::time::Instant>>> = LazyLock::new(Default::default);

/// Note that a shared-input message was just heard from `device_id`.
pub fn note_seen(device_id: &str) {
    lock(&SEEN).insert(device_id.to_string(), std::time::Instant::now());
}

/// Whether a shared-input message was heard from `device_id` within `within`.
pub fn seen_recently(device_id: &str, within: Duration) -> bool {
    lock(&SEEN)
        .get(device_id)
        .is_some_and(|at| at.elapsed() < within)
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
        drop(gone);
    }
}

/// What the other computers send from now on, with each one's id: one
/// receiver at a time, so a later call takes them from the earlier one,
/// whose receiver then ends. Pings are in it (each already answered), the
/// answers to this computer's own pings are not (they go to the
/// [`Channel`] that sent them). With no receiver taken, what arrives is
/// dropped.
pub fn incoming() -> mpsc::Receiver<(String, Msg)> {
    let (tx, rx) = mpsc::channel(INCOMING_DEPTH);
    *lock(&INCOMING) = Some(tx);
    rx
}

fn incoming_sender() -> Option<mpsc::Sender<Arrival>> {
    lock(&INCOMING).clone()
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
                        if answer(&device_id, &conn, &msg)
                            && let Some(incoming) = incoming_sender()
                        {
                            // A datagram is dropped rather than waited for.
                            let _ = incoming.try_send((device_id.clone(), msg));
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
                hand_on(&device_id, msg).await;
            }
            let _ = stream.stop(0u32.into());
            return;
        }
        if answer(&device_id, &conn, &msg) {
            hand_on(&device_id, msg).await;
        }
    }
}

/// Hand `msg` on to whoever takes [`incoming`], waiting while it is full:
/// what the stream carries is not dropped.
async fn hand_on(device_id: &str, msg: Msg) {
    if let Some(incoming) = incoming_sender() {
        let _ = incoming.send((device_id.to_string(), msg)).await;
    }
}

/// What the link itself does with `msg` from `device_id`: a ping is
/// answered at once, a pong goes to the channel whose ping it answers.
/// Whether it is handed on too (all but a pong).
fn answer(device_id: &str, conn: &Connection, msg: &Msg) -> bool {
    match msg {
        Msg::Ping { t_us } => {
            let pong = wire::encode(&Msg::Pong { t_us: *t_us });
            if let Err(e) = conn.send_datagram(pong.into()) {
                tracing::debug!(device_id, "A ping went unanswered: {e}");
            }
            true
        }
        Msg::Pong { t_us } => {
            let pong = Pong {
                sent_us: *t_us,
                back_us: now_us(),
            };
            if let Some(waiting) = lock(&PONGS).get(device_id) {
                let _ = waiting.send(pong);
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
/// computer's clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pong {
    pub sent_us: u64,
    pub back_us: u64,
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
    /// Why the stream stopped, once it did.
    broken: Arc<Mutex<Option<String>>>,
    pongs: mpsc::UnboundedReceiver<Pong>,
    /// The way into `pongs`, to know it is still this channel's on drop.
    pongs_in: mpsc::UnboundedSender<Pong>,
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
        let (pongs_in, pongs) = mpsc::unbounded_channel();
        lock(&PONGS).insert(device_id.to_string(), pongs_in.clone());
        Ok(Channel {
            device_id: device_id.to_string(),
            conn,
            runtime,
            stream: None,
            broken: Arc::default(),
            pongs,
            pongs_in,
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
                self.runtime.spawn(write_stream(
                    self.conn.clone(),
                    waiting,
                    Arc::clone(&self.broken),
                ));
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
}

impl Drop for Channel {
    fn drop(&mut self) {
        let mut pongs = lock(&PONGS);
        if pongs
            .get(&self.device_id)
            .is_some_and(|waiting| waiting.same_channel(&self.pongs_in))
        {
            pongs.remove(&self.device_id);
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
            let (from, msg) = tokio::time::timeout_at(deadline, arriving.recv())
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

    #[test]
    fn only_the_newest_kind_rides_datagrams() {
        assert!(rides_datagram(&Msg::Move { seq: 1, x: 0, y: 0 }));
        assert!(rides_datagram(&Msg::Active { idle_ms: 0 }));
        assert!(rides_datagram(&Msg::Ping { t_us: 0 }));
        assert!(rides_datagram(&Msg::Pong { t_us: 0 }));
        assert!(!rides_datagram(&Msg::Key {
            code: 30,
            pressed: true
        }));
        assert!(!rides_datagram(&Msg::Enter { x: 0, y: 0 }));
        assert!(!rides_datagram(&Msg::Leave));
        assert!(!rides_datagram(&Msg::ReleaseAll));
    }
}
