//! A [`Connection`] over one ordered byte stream (the Bluetooth fallback,
//! phase 2: the screen over the Bluetooth road, whose link is one RFCOMM
//! channel carrying TLS, not a QUIC connection).
//!
//! The streaming pipeline speaks in uni streams: one per frame, per sound
//! packet, per input packet. Here each is a run of frames on the one byte
//! stream, interleaved with the others:
//!
//! ```text
//! [kind u8][stream id u32 LE][length u32 LE][payload]
//! ```
//!
//! `DATA` carries up to [`MAX_CHUNK`] bytes of a stream, `FIN` ends it,
//! `RESET` (a `u32` code) gives it up, `STOP` (a `u32` code, from the
//! reader) asks its writer to give it up, `PING` and `PONG` (a `u64`) time
//! the round trip, and `CLOSE` (a `u32` code and a reason) ends the
//! connection. The dialer opens even stream ids and the listener odd ones,
//! each counting up, so a stream is new exactly when its id is past the
//! last one the peer opened. Nothing is lost on the way (the byte stream is
//! reliable and in order), so a stream is delivered once its end is
//! written; what one writer has not written waits behind a shared budget of
//! [`WRITE_BUDGET`] bytes, so a fast writer cannot queue without bound.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

use super::connection::{
    Connection, ConnectionError, ConnectionStats, Delivered, InStream, OutStream, PeerKey,
    TransportFamily,
};

const DATA: u8 = 0;
const FIN: u8 = 1;
const RESET: u8 = 2;
const STOP: u8 = 3;
const PING: u8 = 4;
const PONG: u8 = 5;
const CLOSE: u8 = 6;

/// A frame's header: kind, stream id, length.
const HEADER: usize = 9;

/// The most of a stream one `DATA` frame carries, so streams interleave.
pub const MAX_CHUNK: usize = 16 * 1024;

/// The longest reason a `CLOSE` carries.
const MAX_REASON: usize = 1024;

/// Bytes written to streams and not yet to the link, at most.
pub const WRITE_BUDGET: usize = 256 * 1024;

/// Parts of an incoming stream waiting for its reader: past this the link
/// waits for it (the byte stream is in order anyway).
const STREAM_QUEUE: usize = 64;

/// Incoming streams waiting to be accepted.
const ACCEPT_QUEUE: usize = 256;

/// How often the round trip is timed.
const PING_EVERY: Duration = Duration::from_secs(1);

/// Which end of the byte stream this is: the dialer opens even stream ids,
/// the listener odd ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Dialer,
    Listener,
}

/// A frame for the writer, with the budget it holds until it is written.
struct Frame {
    kind: u8,
    id: u32,
    payload: Vec<u8>,
    _budget: Option<OwnedSemaphorePermit>,
}

/// What an incoming stream's reader is handed.
enum Chunk {
    Data(Vec<u8>),
    Fin,
    Reset(u32),
}

/// An outgoing stream's state the reader may change: the peer stopped it
/// (`stopped`), or it is over (`done`: written whole, reset, stopped, or
/// the connection ended).
#[derive(Clone)]
struct OutState {
    stopped: CancellationToken,
    done: CancellationToken,
}

struct Shared {
    writes: mpsc::UnboundedSender<Frame>,
    budget: Arc<Semaphore>,
    outgoing: Mutex<HashMap<u32, OutState>>,
    accept: tokio::sync::Mutex<mpsc::Receiver<ByteInStream>>,
    next_id: AtomicU32,
    ended: CancellationToken,
    close_reason: Mutex<Option<String>>,
    started: Instant,
    /// The last round trip in microseconds; `u64::MAX` before the first.
    rtt_us: AtomicU64,
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
    frames_sent: AtomicU64,
    frames_received: AtomicU64,
    remote: SocketAddr,
    peer_key: Option<PeerKey>,
}

impl Shared {
    /// The connection ended for `reason`, unless it already had one.
    fn end(&self, reason: impl Into<String>) {
        {
            let mut said = self.close_reason.lock().unwrap_or_else(|e| e.into_inner());
            if said.is_none() {
                *said = Some(reason.into());
            }
        }
        self.ended.cancel();
    }

    fn send(&self, kind: u8, id: u32, payload: Vec<u8>) -> Result<(), ConnectionError> {
        if self.ended.is_cancelled() {
            return Err(ConnectionError::Closed);
        }
        self.writes
            .send(Frame {
                kind,
                id,
                payload,
                _budget: None,
            })
            .map_err(|_| ConnectionError::Closed)
    }
}

/// See the module docs.
pub struct ByteStreamConnection {
    shared: Arc<Shared>,
}

impl ByteStreamConnection {
    /// A connection over `io` as `role`, said to be with `remote` (an
    /// address for the record; a Bluetooth link has none) and its peer
    /// proved `peer_key` (on the TLS the byte stream carries). Its reader,
    /// writer and round-trip timer run on the current Tokio runtime until it
    /// ends.
    pub fn new<S>(io: S, role: Role, remote: SocketAddr, peer_key: Option<PeerKey>) -> Arc<Self>
    where
        S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        let (read, write) = tokio::io::split(io);
        let (writes, frames) = mpsc::unbounded_channel();
        let (accepted, accept) = mpsc::channel(ACCEPT_QUEUE);
        let shared = Arc::new(Shared {
            writes,
            budget: Arc::new(Semaphore::new(WRITE_BUDGET)),
            outgoing: Mutex::new(HashMap::new()),
            accept: tokio::sync::Mutex::new(accept),
            next_id: AtomicU32::new(match role {
                Role::Dialer => 0,
                Role::Listener => 1,
            }),
            ended: CancellationToken::new(),
            close_reason: Mutex::new(None),
            started: Instant::now(),
            rtt_us: AtomicU64::new(u64::MAX),
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
            frames_sent: AtomicU64::new(0),
            frames_received: AtomicU64::new(0),
            remote,
            peer_key,
        });
        tokio::spawn(write_frames(write, frames, shared.clone()));
        tokio::spawn(read_frames(read, accepted, role, shared.clone()));
        tokio::spawn(time_round_trips(shared.clone()));
        Arc::new(Self { shared })
    }

    /// Whether a round trip was timed yet (the tests').
    #[cfg(test)]
    fn timed(&self) -> bool {
        self.shared.rtt_us.load(Ordering::Relaxed) != u64::MAX
    }
}

impl Drop for ByteStreamConnection {
    fn drop(&mut self) {
        self.shared.end("the connection was dropped");
    }
}

#[async_trait]
impl Connection for ByteStreamConnection {
    async fn open_uni(&self) -> Result<Box<dyn OutStream>, ConnectionError> {
        if self.shared.ended.is_cancelled() {
            return Err(ConnectionError::Closed);
        }
        let id = self.shared.next_id.fetch_add(2, Ordering::Relaxed);
        let state = OutState {
            stopped: CancellationToken::new(),
            done: self.shared.ended.child_token(),
        };
        self.shared
            .outgoing
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, state.clone());
        Ok(Box::new(ByteOutStream {
            id,
            shared: self.shared.clone(),
            state,
            finished: false,
        }))
    }

    async fn accept_uni(&self) -> Result<Box<dyn InStream>, ConnectionError> {
        let mut accept = self.shared.accept.lock().await;
        match accept.recv().await {
            Some(stream) => Ok(Box::new(stream)),
            None => Err(ConnectionError::Closed),
        }
    }

    fn remote_address(&self) -> SocketAddr {
        self.shared.remote
    }

    fn stats(&self) -> ConnectionStats {
        let rtt_us = self.shared.rtt_us.load(Ordering::Relaxed);
        ConnectionStats {
            rtt: if rtt_us == u64::MAX {
                Duration::ZERO
            } else {
                Duration::from_micros(rtt_us)
            },
            datagrams_sent: self.shared.frames_sent.load(Ordering::Relaxed),
            datagrams_received: self.shared.frames_received.load(Ordering::Relaxed),
            bytes_sent: self.shared.bytes_sent.load(Ordering::Relaxed),
            bytes_received: self.shared.bytes_received.load(Ordering::Relaxed),
            ..ConnectionStats::default()
        }
    }

    fn transport_family(&self) -> TransportFamily {
        TransportFamily::ByteStream
    }

    fn close(&self, error_code: u32, reason: &[u8]) {
        if self.shared.ended.is_cancelled() {
            return;
        }
        let reason = &reason[..reason.len().min(MAX_REASON)];
        {
            let mut said = self
                .shared
                .close_reason
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            said.get_or_insert_with(|| String::from_utf8_lossy(reason).into_owned());
        }
        let mut payload = error_code.to_le_bytes().to_vec();
        payload.extend_from_slice(reason);
        // Queued, then the connection ends here at once: the writer takes
        // what is queued before it looks at the end, so the peer hears why.
        let _ = self.shared.send(CLOSE, 0, payload);
        self.shared.ended.cancel();
    }

    fn close_reason(&self) -> Option<String> {
        if !self.shared.ended.is_cancelled() {
            return None;
        }
        self.shared
            .close_reason
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn peer_key(&self) -> Option<PeerKey> {
        self.shared.peer_key.clone()
    }
}

struct ByteOutStream {
    id: u32,
    shared: Arc<Shared>,
    state: OutState,
    finished: bool,
}

#[async_trait]
impl OutStream for ByteOutStream {
    async fn write_all(&mut self, data: &[u8]) -> Result<(), ConnectionError> {
        for chunk in data.chunks(MAX_CHUNK) {
            if self.state.stopped.is_cancelled() {
                return Err(ConnectionError::Other("stopped by the peer".into()));
            }
            if self.finished || self.state.done.is_cancelled() {
                return Err(ConnectionError::Closed);
            }
            let budget = tokio::select! {
                budget = self.shared.budget.clone().acquire_many_owned(chunk.len() as u32) => {
                    budget.map_err(|_| ConnectionError::Closed)?
                }
                () = self.shared.ended.cancelled() => return Err(ConnectionError::Closed),
            };
            self.shared
                .writes
                .send(Frame {
                    kind: DATA,
                    id: self.id,
                    payload: chunk.to_vec(),
                    _budget: Some(budget),
                })
                .map_err(|_| ConnectionError::Closed)?;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), ConnectionError> {
        if self.finished || self.state.done.is_cancelled() {
            return Err(ConnectionError::Closed);
        }
        self.finished = true;
        self.shared.send(FIN, self.id, Vec::new())
    }

    fn delivered(&self) -> Delivered {
        Box::pin(self.state.done.clone().cancelled_owned())
    }

    fn reset(&mut self, code: u32) {
        if self.state.done.is_cancelled() {
            return;
        }
        self.finished = true;
        let _ = self
            .shared
            .send(RESET, self.id, code.to_le_bytes().to_vec());
        self.state.done.cancel();
        forget_outgoing(&self.shared, self.id);
    }
}

impl Drop for ByteOutStream {
    fn drop(&mut self) {
        // Dropped unfinished: given up, as quinn resets a dropped stream.
        if !self.finished {
            self.reset(0);
        }
    }
}

fn forget_outgoing(shared: &Shared, id: u32) {
    shared
        .outgoing
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
}

struct ByteInStream {
    id: u32,
    chunks: mpsc::Receiver<Chunk>,
    /// What came and was not read yet.
    leftover: Vec<u8>,
    ended: bool,
    shared: Arc<Shared>,
}

impl ByteInStream {
    /// The next part of the stream: `Ok(None)` at its end.
    async fn next(&mut self) -> Result<Option<Vec<u8>>, ConnectionError> {
        if !self.leftover.is_empty() {
            return Ok(Some(std::mem::take(&mut self.leftover)));
        }
        if self.ended {
            return Ok(None);
        }
        match self.chunks.recv().await {
            Some(Chunk::Data(data)) => Ok(Some(data)),
            Some(Chunk::Fin) => {
                self.ended = true;
                Ok(None)
            }
            Some(Chunk::Reset(code)) => {
                self.ended = true;
                Err(ConnectionError::Other(format!(
                    "reset by the peer ({code})"
                )))
            }
            None => {
                self.ended = true;
                Err(ConnectionError::Closed)
            }
        }
    }
}

#[async_trait]
impl InStream for ByteInStream {
    async fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), ConnectionError> {
        let mut filled = 0;
        while filled < buf.len() {
            let Some(mut data) = self.next().await? else {
                return Err(ConnectionError::Other("the stream ended early".into()));
            };
            let take = data.len().min(buf.len() - filled);
            buf[filled..filled + take].copy_from_slice(&data[..take]);
            filled += take;
            if take < data.len() {
                self.leftover = data.split_off(take);
            }
        }
        Ok(())
    }

    async fn read_to_end(&mut self, limit: usize) -> Result<Vec<u8>, ConnectionError> {
        let mut all = Vec::new();
        while let Some(data) = self.next().await? {
            if all.len() + data.len() > limit {
                return Err(ConnectionError::Other("stream exceeds limit".into()));
            }
            all.extend_from_slice(&data);
        }
        Ok(all)
    }

    async fn read_to_end_counting(
        &mut self,
        limit: usize,
        progress: &AtomicU64,
    ) -> Result<Vec<u8>, ConnectionError> {
        let mut all = Vec::new();
        while let Some(data) = self.next().await? {
            if all.len() + data.len() > limit {
                return Err(ConnectionError::Other("stream exceeds limit".into()));
            }
            progress.fetch_add(data.len() as u64, Ordering::Relaxed);
            all.extend_from_slice(&data);
        }
        Ok(all)
    }

    fn stop(&mut self, code: u32) {
        if self.ended {
            return;
        }
        self.ended = true;
        self.leftover.clear();
        self.chunks.close();
        let _ = self.shared.send(STOP, self.id, code.to_le_bytes().to_vec());
    }
}

impl Drop for ByteInStream {
    fn drop(&mut self) {
        // Dropped before its end: the writer need not go on.
        self.stop(0);
    }
}

/// Write each frame to the link, flushing once nothing more waits. A
/// `CLOSE` is the last: written, then the connection ends.
async fn write_frames<W>(
    mut write: W,
    mut frames: mpsc::UnboundedReceiver<Frame>,
    shared: Arc<Shared>,
) where
    W: AsyncWrite + Send + Unpin,
{
    loop {
        let frame = tokio::select! {
            biased;
            frame = frames.recv() => frame,
            () = shared.ended.cancelled() => None,
        };
        let Some(frame) = frame else {
            break;
        };
        let mut head = [0u8; HEADER];
        head[0] = frame.kind;
        head[1..5].copy_from_slice(&frame.id.to_le_bytes());
        head[5..9].copy_from_slice(&(frame.payload.len() as u32).to_le_bytes());
        let wrote = async {
            write.write_all(&head).await?;
            write.write_all(&frame.payload).await?;
            if frames.is_empty() || frame.kind == CLOSE {
                write.flush().await?;
            }
            std::io::Result::Ok(())
        }
        .await;
        if let Err(e) = wrote {
            shared.end(format!("the byte stream failed: {e}"));
            break;
        }
        shared
            .bytes_sent
            .fetch_add((HEADER + frame.payload.len()) as u64, Ordering::Relaxed);
        shared.frames_sent.fetch_add(1, Ordering::Relaxed);
        match frame.kind {
            FIN => {
                // Written whole: delivered, since nothing is lost on the way.
                let state = shared
                    .outgoing
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&frame.id);
                if let Some(state) = state {
                    state.done.cancel();
                }
            }
            CLOSE => {
                let _ = write.shutdown().await;
                shared.end("closed");
                break;
            }
            _ => {}
        }
    }
}

/// Read frames off the link and hand each to its stream, until the link
/// ends, the peer closes, or a frame breaks the rules (which ends it too).
async fn read_frames<R>(
    mut read: R,
    accepted: mpsc::Sender<ByteInStream>,
    role: Role,
    shared: Arc<Shared>,
) where
    R: AsyncRead + Send + Unpin,
{
    // The peer opens the other parity of ids; `None` before its first.
    let peer_parity = match role {
        Role::Dialer => 1,
        Role::Listener => 0,
    };
    let mut incoming: HashMap<u32, mpsc::Sender<Chunk>> = HashMap::new();
    let mut last_opened: Option<u32> = None;
    let why = loop {
        let mut head = [0u8; HEADER];
        let read_head = tokio::select! {
            read = read.read_exact(&mut head) => read,
            () = shared.ended.cancelled() => break None,
        };
        if read_head.is_err() {
            break Some("the byte stream ended".to_string());
        }
        let kind = head[0];
        let id = u32::from_le_bytes(head[1..5].try_into().unwrap());
        let len = u32::from_le_bytes(head[5..9].try_into().unwrap()) as usize;
        let most = match kind {
            DATA => MAX_CHUNK,
            FIN => 0,
            RESET | STOP => 4,
            PING | PONG => 8,
            CLOSE => 4 + MAX_REASON,
            _ => break Some(format!("a frame of unknown kind {kind}")),
        };
        if len > most {
            break Some(format!("a frame of kind {kind} too long ({len} bytes)"));
        }
        let mut payload = vec![0u8; len];
        if read.read_exact(&mut payload).await.is_err() {
            break Some("the byte stream ended".to_string());
        }
        shared
            .bytes_received
            .fetch_add((HEADER + len) as u64, Ordering::Relaxed);
        shared.frames_received.fetch_add(1, Ordering::Relaxed);
        let code = || {
            u32::from_le_bytes(
                payload
                    .get(..4)
                    .and_then(|b| b.try_into().ok())
                    .unwrap_or([0; 4]),
            )
        };
        match kind {
            DATA | FIN | RESET => {
                if id % 2 != peer_parity {
                    break Some(format!("data on stream {id}, which the peer did not open"));
                }
                if last_opened.is_none_or(|last| id > last) {
                    last_opened = Some(id);
                    let (sender, chunks) = mpsc::channel(STREAM_QUEUE);
                    incoming.insert(id, sender);
                    let stream = ByteInStream {
                        id,
                        chunks,
                        leftover: Vec::new(),
                        ended: false,
                        shared: shared.clone(),
                    };
                    let sent = tokio::select! {
                        sent = accepted.send(stream) => sent.is_ok(),
                        () = shared.ended.cancelled() => break None,
                    };
                    if !sent {
                        incoming.remove(&id);
                    }
                }
                // A stream already ended or dropped here: its rest is dropped.
                let Some(sender) = incoming.get(&id) else {
                    continue;
                };
                let (chunk, last) = match kind {
                    DATA => (Chunk::Data(payload), false),
                    FIN => (Chunk::Fin, true),
                    _ => (Chunk::Reset(code()), true),
                };
                let delivered = tokio::select! {
                    sent = sender.send(chunk) => sent.is_ok(),
                    () = shared.ended.cancelled() => break None,
                };
                if last || !delivered {
                    incoming.remove(&id);
                }
            }
            STOP => {
                let state = shared
                    .outgoing
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                if let Some(state) = state {
                    state.stopped.cancel();
                    state.done.cancel();
                }
            }
            PING => {
                let _ = shared.send(PONG, 0, payload);
            }
            PONG => {
                if let Ok(sent) = <[u8; 8]>::try_from(payload.as_slice()) {
                    let sent = u64::from_le_bytes(sent);
                    let now = shared.started.elapsed().as_micros() as u64;
                    shared
                        .rtt_us
                        .store(now.saturating_sub(sent), Ordering::Relaxed);
                }
            }
            CLOSE => {
                let reason = String::from_utf8_lossy(payload.get(4..).unwrap_or_default());
                break Some(reason.into_owned());
            }
            _ => unreachable!("kinds checked above"),
        }
    };
    if let Some(why) = why {
        shared.end(why);
    }
    // Every stream still open ends with the connection: its reader sees it
    // closed, and nothing more is accepted.
    drop(incoming);
}

/// Time the round trip every [`PING_EVERY`], from the first moment on.
async fn time_round_trips(shared: Arc<Shared>) {
    let mut every = tokio::time::interval(PING_EVERY);
    loop {
        tokio::select! {
            _ = every.tick() => {}
            () = shared.ended.cancelled() => return,
        }
        let now = shared.started.elapsed().as_micros() as u64;
        if shared.send(PING, 0, now.to_le_bytes().to_vec()).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nowhere() -> SocketAddr {
        SocketAddr::from(([0, 0, 0, 0], 0))
    }

    /// A dialer and a listener over an in-memory byte stream.
    fn pair() -> (Arc<ByteStreamConnection>, Arc<ByteStreamConnection>) {
        let (a, b) = tokio::io::duplex(64 * 1024);
        (
            ByteStreamConnection::new(a, Role::Dialer, nowhere(), None),
            ByteStreamConnection::new(b, Role::Listener, nowhere(), Some(PeerKey::Tls([7; 32]))),
        )
    }

    async fn within<T>(what: &str, future: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(5), future)
            .await
            .unwrap_or_else(|_| panic!("{what} within 5 s"))
    }

    #[tokio::test]
    async fn streams_cross_whole_both_ways_and_side_by_side() {
        let (dialer, listener) = pair();
        assert_eq!(listener.peer_key(), Some(PeerKey::Tls([7; 32])));
        assert_eq!(dialer.transport_family(), TransportFamily::ByteStream);
        let big: Vec<u8> = (0..100_000u32).map(|n| (n % 251) as u8).collect();

        // Two streams written side by side, their parts interleaved.
        let mut first = dialer.open_uni().await.unwrap();
        let mut second = dialer.open_uni().await.unwrap();
        first.write_all(&big[..50_000]).await.unwrap();
        second.write_all(b"header").await.unwrap();
        first.write_all(&big[50_000..]).await.unwrap();
        second.write_all(b" and the rest").await.unwrap();
        first.finish().unwrap();
        second.finish().unwrap();
        within("delivered", first.delivered()).await;

        let mut a = within("accept", listener.accept_uni()).await.unwrap();
        let mut b = within("accept", listener.accept_uni()).await.unwrap();
        assert_eq!(within("read", a.read_to_end(1 << 20)).await.unwrap(), big);
        let mut head = [0u8; 6];
        within("read", b.read_exact(&mut head)).await.unwrap();
        assert_eq!(&head, b"header");
        assert_eq!(
            within("read", b.read_to_end(64)).await.unwrap(),
            b" and the rest"
        );

        // And the other way.
        let mut back = listener.open_uni().await.unwrap();
        back.write_all(b"pong").await.unwrap();
        back.finish().unwrap();
        let mut got = within("accept", dialer.accept_uni()).await.unwrap();
        assert_eq!(within("read", got.read_to_end(64)).await.unwrap(), b"pong");
    }

    #[tokio::test]
    async fn a_reset_reaches_the_reader_and_a_stop_the_writer() {
        let (dialer, listener) = pair();
        let mut out = dialer.open_uni().await.unwrap();
        out.write_all(b"part").await.unwrap();
        out.reset(0x4d4f);
        let mut got = within("accept", listener.accept_uni()).await.unwrap();
        let read = within("read", got.read_to_end(64)).await;
        assert!(
            matches!(&read, Err(ConnectionError::Other(e)) if e.contains("19791")),
            "{read:?}"
        );

        let mut out = dialer.open_uni().await.unwrap();
        out.write_all(b"part").await.unwrap();
        let mut got = within("accept", listener.accept_uni()).await.unwrap();
        got.stop(0x4d4f);
        within("delivered once stopped", out.delivered()).await;
        assert!(out.write_all(b"more").await.is_err(), "stopped by the peer");
    }

    #[tokio::test]
    async fn a_close_says_why_on_both_ends_and_ends_every_stream() {
        let (dialer, listener) = pair();
        let mut out = dialer.open_uni().await.unwrap();
        out.write_all(b"half").await.unwrap();
        let mut got = within("accept", listener.accept_uni()).await.unwrap();
        listener.close(1, b"not paired");
        assert_eq!(
            within("refused", dialer.accept_uni()).await.err(),
            Some(ConnectionError::Closed)
        );
        assert_eq!(dialer.close_reason().as_deref(), Some("not paired"));
        assert_eq!(listener.close_reason().as_deref(), Some("not paired"));
        assert!(within("read", got.read_to_end(64)).await.is_err());
        assert!(dialer.open_uni().await.is_err());
    }

    #[tokio::test]
    async fn a_frame_that_breaks_the_rules_ends_the_connection() {
        let (mut raw, b) = tokio::io::duplex(1024);
        let listener = ByteStreamConnection::new(b, Role::Listener, nowhere(), None);
        // DATA on an odd id: the listener's own parity.
        let mut frame = vec![DATA];
        frame.extend_from_slice(&1u32.to_le_bytes());
        frame.extend_from_slice(&0u32.to_le_bytes());
        raw.write_all(&frame).await.unwrap();
        assert!(within("ended", listener.accept_uni()).await.is_err());
        assert!(listener.close_reason().unwrap().contains("did not open"));

        let (mut raw, b) = tokio::io::duplex(1024);
        let listener = ByteStreamConnection::new(b, Role::Listener, nowhere(), None);
        let mut frame = vec![DATA];
        frame.extend_from_slice(&0u32.to_le_bytes());
        frame.extend_from_slice(&(MAX_CHUNK as u32 + 1).to_le_bytes());
        raw.write_all(&frame).await.unwrap();
        assert!(within("ended", listener.accept_uni()).await.is_err());
        assert!(listener.close_reason().unwrap().contains("too long"));
    }

    #[tokio::test]
    async fn the_round_trip_is_timed_and_the_bytes_counted() {
        let (dialer, listener) = pair();
        within("a round trip", async {
            while !dialer.timed() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        let mut out = dialer.open_uni().await.unwrap();
        out.write_all(&[1; 1000]).await.unwrap();
        out.finish().unwrap();
        within("delivered", out.delivered()).await;
        assert!(dialer.stats().bytes_sent >= 1000);
        let mut got = within("accept", listener.accept_uni()).await.unwrap();
        within("read", got.read_to_end(2000)).await.unwrap();
        assert!(listener.stats().bytes_received >= 1000);
    }

    /// The budget holds a writer back while the link does not take its
    /// bytes, and lets it go once it does.
    #[tokio::test]
    async fn a_writer_waits_for_the_link_past_the_budget() {
        let (a, mut b) = tokio::io::duplex(1024);
        let dialer = ByteStreamConnection::new(a, Role::Dialer, nowhere(), None);
        let mut out = dialer.open_uni().await.unwrap();
        let blob = vec![0u8; WRITE_BUDGET * 2];
        let write = tokio::spawn(async move {
            out.write_all(&blob).await.unwrap();
            out
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!write.is_finished(), "held back while nothing is read");
        let drain = tokio::spawn(async move {
            let mut sink = vec![0u8; 64 * 1024];
            while b.read(&mut sink).await.is_ok_and(|n| n > 0) {}
        });
        within("let go", write).await.unwrap();
        drain.abort();
    }
}
