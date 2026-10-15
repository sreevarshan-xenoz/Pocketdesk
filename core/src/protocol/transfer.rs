//! The bytes of one file transfer, resumable (T6a).
//!
//! Runs inside the transfer TLS (`pocketdesk-transfer/2`) once the desktop
//! has presented the one-time key. The roles are the data's: the *sender*
//! has the file, the *receiver* keeps what it has made durable. Either may be the end
//! that dialled.
//!
//! 1. The sender: the file's size, a big-endian `u64`.
//! 2. The receiver: how much it already has of this file, `offset: u64`,
//!    and the SHA-256 of those first `offset` bytes.
//! 3. The sender: [`CONTINUE`] when its own first `offset` bytes hash the
//!    same, else [`START_OVER`] (the receiver then drops what it had, and
//!    the next attempt starts from 0).
//! 4. The sender: the bytes from `offset` to the end.
//! 5. The sender: the SHA-256 of the whole file.
//! 6. The receiver ([`finish`]): [`STATUS_SAVED`] once its own hash of the
//!    whole file matched and the file is kept, else [`STATUS_FAILED`].
//!
//! The receiver is the authority on the offset (as in tus); the prefix hash
//! proves the sender is sending the same file before anything is appended,
//! and the whole-file hash proves the result.

use sha2::{Digest, Sha256};
use std::future::Future;
use std::io;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt, AsyncWrite, AsyncWriteExt};

/// The sender continues from the receiver's offset.
pub const CONTINUE: u8 = 1;
/// The sender's file is not the one the receiver has part of.
pub const START_OVER: u8 = 0;
/// The receiver kept the file: its whole-file hash matched.
pub const STATUS_SAVED: u8 = 1;
/// The receiver did not keep the file.
pub const STATUS_FAILED: u8 = 0;

/// A slow disk's pace, for the waits on work the other end does on disk:
/// a flat idle limit would fail a large partial on every retry.
const SLOW_DISK_BYTES_PER_SEC: u64 = 20 * 1024 * 1024;

/// How often the receiver makes what it has durable, so a break loses at
/// most this much.
pub const COMMIT_EVERY: u64 = 4 * 1024 * 1024;

const CHUNK: usize = 64 * 1024;

/// How slow a transfer may be before it is given up.
#[derive(Debug, Clone, Copy)]
pub struct Pace {
    /// The longest one read or write of the connection may wait.
    pub idle: Duration,
    /// The slowest the whole transfer may be, in bytes a second.
    pub min_bytes_per_sec: u64,
}

impl Pace {
    /// 30 s idle and 32 KiB/s, the limits both ends already use.
    pub const DEFAULT: Pace = Pace {
        idle: Duration::from_secs(30),
        min_bytes_per_sec: 32 * 1024,
    };

    /// How long a step that works through `bytes` on the other end may take
    /// to answer: reading back a partial to hash it, or moving a finished
    /// file into place (a copy, across volumes). The idle limit, plus time
    /// at a slow disk's pace.
    pub fn slow_step(&self, bytes: u64) -> Duration {
        self.idle + Duration::from_secs(bytes / SLOW_DISK_BYTES_PER_SEC)
    }

    fn deadline(&self, bytes: u64) -> tokio::time::Instant {
        tokio::time::Instant::now()
            + self.idle
            + Duration::from_secs(bytes / self.min_bytes_per_sec.max(1))
    }
}

/// Why a transfer did not end with the file kept.
#[derive(Debug)]
pub enum TransferError {
    /// The connection failed, or was too slow.
    Io(io::Error),
    /// The sender's file is not the one the receiver had part of. What the
    /// receiver had is gone; the next attempt starts from the beginning.
    SourceChanged,
    /// The file's size is not acceptable (zero, too large, or not the size
    /// announced).
    Size(u64),
    /// The bytes did not hash to the sender's trailer: nothing is kept.
    Corrupt,
    /// The receiver said it did not keep the file.
    NotSaved,
}

impl std::fmt::Display for TransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransferError::Io(e) => write!(f, "{e}"),
            TransferError::SourceChanged => {
                write!(
                    f,
                    "the file changed since the last attempt, so it starts over"
                )
            }
            TransferError::Size(size) => write!(f, "a size of {size} bytes is not accepted"),
            TransferError::Corrupt => write!(f, "the file arrived damaged"),
            TransferError::NotSaved => write!(f, "the other end did not save the file"),
        }
    }
}

impl std::error::Error for TransferError {}

impl From<io::Error> for TransferError {
    fn from(e: io::Error) -> Self {
        TransferError::Io(e)
    }
}

fn timed_out(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, format!("{what} timed out"))
}

async fn within<T>(
    idle: Duration,
    what: &str,
    step: impl Future<Output = io::Result<T>>,
) -> io::Result<T> {
    tokio::time::timeout(idle, step)
        .await
        .map_err(|_| timed_out(what))?
}

/// Send `size` bytes of `file` (positioned at its start). `progress` is
/// told how many bytes the receiver has, counting what it already had.
/// Returns the offset the transfer resumed from.
///
/// `file` is read only forwards, so it can be a pipe: on
/// [`TransferError::SourceChanged`] the caller opens the file again for
/// the next attempt, which starts from 0.
pub async fn send<F, S>(
    file: &mut F,
    size: u64,
    stream: &mut S,
    pace: Pace,
    mut progress: impl FnMut(u64),
) -> Result<u64, TransferError>
where
    F: AsyncRead + Unpin,
    S: AsyncRead + AsyncWrite + Unpin,
{
    within(
        pace.idle,
        "sending the size",
        stream.write_all(&size.to_be_bytes()),
    )
    .await?;
    within(pace.idle, "sending the size", stream.flush()).await?;
    let mut point = [0u8; 40];
    // The receiver reads back and hashes what it has first.
    within(
        pace.slow_step(size),
        "waiting for the resume point",
        stream.read_exact(&mut point),
    )
    .await?;
    let offset = u64::from_be_bytes(point[..8].try_into().expect("8 bytes"));
    let deadline = pace.deadline(size);

    // Hash our own first `offset` bytes: the same file hashes the same.
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK];
    let mut read: u64 = 0;
    let same = offset <= size && {
        while read < offset {
            let want = (offset - read).min(CHUNK as u64) as usize;
            let n = tokio::time::timeout_at(deadline, file.read(&mut buffer[..want]))
                .await
                .map_err(|_| timed_out("reading the file"))??;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            read += n as u64;
        }
        read == offset && hasher.clone().finalize().as_slice() == &point[8..]
    };
    if !same {
        within(pace.idle, "answering", stream.write_all(&[START_OVER])).await?;
        within(pace.idle, "answering", stream.flush()).await?;
        return Err(TransferError::SourceChanged);
    }
    within(pace.idle, "answering", stream.write_all(&[CONTINUE])).await?;
    progress(offset);

    let mut sent = offset;
    while sent < size {
        let want = (size - sent).min(CHUNK as u64) as usize;
        let n = tokio::time::timeout_at(deadline, file.read(&mut buffer[..want]))
            .await
            .map_err(|_| timed_out("reading the file"))??;
        if n == 0 {
            return Err(TransferError::Size(sent));
        }
        hasher.update(&buffer[..n]);
        within(pace.idle, "sending", stream.write_all(&buffer[..n])).await?;
        sent += n as u64;
        progress(sent);
        if tokio::time::Instant::now() >= deadline {
            return Err(timed_out("the transfer").into());
        }
    }
    within(
        pace.idle,
        "sending the hash",
        stream.write_all(&hasher.finalize()),
    )
    .await?;
    within(pace.idle, "sending the hash", stream.flush()).await?;
    let mut status = [0u8; 1];
    // The receiver may copy the file into place before it answers.
    within(
        pace.slow_step(size),
        "waiting for the save",
        stream.read_exact(&mut status),
    )
    .await?;
    if status[0] == STATUS_SAVED {
        Ok(offset)
    } else {
        Err(TransferError::NotSaved)
    }
}

/// What a receiver keeps between attempts: a file it can read, write,
/// seek, cut and make durable.
pub trait Partial: AsyncRead + AsyncWrite + AsyncSeek + Unpin + Send {
    /// Cut the file to `len` bytes.
    fn truncate(&mut self, len: u64) -> impl Future<Output = io::Result<()>> + Send;
    /// Make what was written durable.
    fn sync(&mut self) -> impl Future<Output = io::Result<()>> + Send;
}

/// What the receiver knows of its partial file before the transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Have {
    /// The size of the file it was receiving, if any.
    pub size: u64,
    /// How many of its first bytes are durable.
    pub committed: u64,
}

/// Receive one file into `partial`, continuing from what `have` says it
/// holds when the sender sends the same file. `expected` is the size the
/// request announced, if it did; `max` the largest accepted.
/// `on_commit(size, committed)` is called each time more is durable, so the
/// caller can record it. On success `partial` holds exactly the file, and
/// the caller ends the transfer with [`finish`] once it has kept it.
///
/// After [`TransferError::SourceChanged`] the partial is empty; after a
/// connection failure it keeps what was committed, for the next attempt.
pub async fn receive<P, S>(
    stream: &mut S,
    partial: &mut P,
    have: Option<Have>,
    expected: Option<u64>,
    max: u64,
    pace: Pace,
    mut on_commit: impl FnMut(u64, u64),
) -> Result<u64, TransferError>
where
    P: Partial,
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut size = [0u8; 8];
    within(
        pace.idle,
        "waiting for the size",
        stream.read_exact(&mut size),
    )
    .await?;
    let size = u64::from_be_bytes(size);
    if size == 0 || size > max || expected.is_some_and(|e| e != size) {
        return Err(TransferError::Size(size));
    }
    // Only a partial of a file this size can be continued.
    let mut offset = have
        .filter(|h| h.size == size)
        .map_or(0, |h| h.committed.min(size));
    let mut hasher = Sha256::new();
    partial.seek(io::SeekFrom::Start(0)).await?;
    let mut buffer = vec![0u8; CHUNK];
    let mut hashed = 0;
    while hashed < offset {
        let want = (offset - hashed).min(CHUNK as u64) as usize;
        let n = partial.read(&mut buffer[..want]).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        hashed += n as u64;
    }
    // A partial shorter than its record says: continue from what is there.
    offset = hashed;
    if offset == 0 {
        partial.truncate(0).await?;
    }
    let mut point = [0u8; 40];
    point[..8].copy_from_slice(&offset.to_be_bytes());
    point[8..].copy_from_slice(&hasher.clone().finalize());
    within(
        pace.idle,
        "sending the resume point",
        stream.write_all(&point),
    )
    .await?;
    within(pace.idle, "sending the resume point", stream.flush()).await?;

    let mut answer = [0u8; 1];
    // The sender reads and hashes its own first `offset` bytes first.
    within(
        pace.slow_step(offset),
        "waiting for the answer",
        stream.read_exact(&mut answer),
    )
    .await?;
    if answer[0] != CONTINUE {
        partial.truncate(0).await?;
        partial.sync().await?;
        on_commit(size, 0);
        return Err(TransferError::SourceChanged);
    }
    partial.truncate(offset).await?;
    partial.seek(io::SeekFrom::Start(offset)).await?;
    on_commit(size, offset);

    let deadline = pace.deadline(size - offset);
    let mut received = offset;
    let mut committed = offset;
    while received < size {
        let want = (size - received).min(CHUNK as u64) as usize;
        let n = match within(pace.idle, "receiving", stream.read(&mut buffer[..want])).await {
            Ok(n) => n,
            Err(e) => {
                // A connection that fails (TLS closed without close_notify,
                // a reset, a stall) keeps what was committed for the next attempt.
                commit(partial, size, received, &mut committed, &mut on_commit).await?;
                return Err(e.into());
            }
        };
        if n == 0 {
            commit(partial, size, received, &mut committed, &mut on_commit).await?;
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into());
        }
        // Written before it is hashed: a write that fails leaves the hash
        // and the committed count describing what is on disk.
        if let Err(e) = partial.write_all(&buffer[..n]).await {
            return Err(e.into());
        }
        hasher.update(&buffer[..n]);
        received += n as u64;
        if received - committed >= COMMIT_EVERY {
            commit(partial, size, received, &mut committed, &mut on_commit).await?;
        }
        if tokio::time::Instant::now() >= deadline {
            commit(partial, size, received, &mut committed, &mut on_commit).await?;
            return Err(timed_out("the transfer").into());
        }
    }
    commit(partial, size, received, &mut committed, &mut on_commit).await?;

    let mut trailer = [0u8; 32];
    within(
        pace.idle,
        "waiting for the hash",
        stream.read_exact(&mut trailer),
    )
    .await?;
    if hasher.finalize().as_slice() != trailer {
        // Nothing of a damaged file is worth continuing.
        partial.truncate(0).await?;
        partial.sync().await?;
        on_commit(size, 0);
        finish(stream, false, pace).await;
        return Err(TransferError::Corrupt);
    }
    // The status waits for the caller, who sends it with [`finish`] once the
    // file is where it is kept: "saved" must mean saved.
    Ok(offset)
}

/// The receiver's last word: the status byte, then a clean close. After a
/// save, what the sender still has in flight is read and dropped first: a
/// socket closed with unread data is reset, and the reset can overtake the
/// status byte. After a failure it closes at once (the sender may still be
/// sending, and a lost "failed" reads as failed anyway). Best effort.
pub async fn finish<S>(stream: &mut S, saved: bool, pace: Pace)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let status = if saved { STATUS_SAVED } else { STATUS_FAILED };
    let _ = within(pace.idle, "sending the status", async {
        stream.write_all(&[status]).await?;
        stream.flush().await?;
        stream.shutdown().await?;
        if saved {
            let mut sink = [0u8; 512];
            while stream.read(&mut sink).await? > 0 {}
        }
        Ok(())
    })
    .await;
}

async fn commit<P: Partial>(
    partial: &mut P,
    size: u64,
    received: u64,
    committed: &mut u64,
    on_commit: &mut impl FnMut(u64, u64),
) -> io::Result<()> {
    if received == *committed {
        return Ok(());
    }
    partial.flush().await?;
    partial.sync().await?;
    *committed = received;
    on_commit(size, received);
    Ok(())
}

/// A partial file in memory: for small transfers kept only in memory (a
/// clipboard image), which never resume.
pub struct Memory(std::io::Cursor<Vec<u8>>);

impl Memory {
    pub fn holding(bytes: &[u8]) -> Self {
        Memory(std::io::Cursor::new(bytes.to_vec()))
    }
    pub fn bytes(&self) -> &[u8] {
        self.0.get_ref()
    }

    /// The bytes received.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0.into_inner()
    }
}

impl AsyncRead for Memory {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for Memory {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        std::pin::Pin::new(&mut self.0).poll_write(cx, buf)
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.0).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

impl AsyncSeek for Memory {
    fn start_seek(mut self: std::pin::Pin<&mut Self>, position: io::SeekFrom) -> io::Result<()> {
        std::pin::Pin::new(&mut self.0).start_seek(position)
    }
    fn poll_complete(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<u64>> {
        std::pin::Pin::new(&mut self.0).poll_complete(cx)
    }
}

impl Partial for Memory {
    async fn truncate(&mut self, len: u64) -> io::Result<()> {
        self.0.get_mut().truncate(len as usize);
        Ok(())
    }
    async fn sync(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Partial for tokio::fs::File {
    async fn truncate(&mut self, len: u64) -> io::Result<()> {
        self.set_len(len).await
    }
    async fn sync(&mut self) -> io::Result<()> {
        self.sync_data().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 % 251) as u8).collect()
    }

    const MAX: u64 = 1 << 30;

    /// Run both ends; return the sender's and the receiver's outcomes and
    /// how many bytes the sender put on the wire.
    async fn run(
        source: &[u8],
        partial: &mut Memory,
        have: Option<Have>,
    ) -> (Result<u64, TransferError>, Result<u64, TransferError>, u64) {
        let (mut a, mut b) = tokio::io::duplex(256 * 1024);
        let mut reader: &[u8] = source;
        let size = source.len() as u64;
        let mut furthest = 0u64;
        // The sender's end closes when it is done, as a real sender's does:
        // the receiver's clean close waits for that.
        let sending = async move {
            let sent = send(&mut reader, size, &mut a, Pace::DEFAULT, |n| furthest = n).await;
            drop(a);
            (sent, furthest)
        };
        let receiving = async {
            let received = receive(
                &mut b,
                partial,
                have,
                Some(size),
                MAX,
                Pace::DEFAULT,
                |_, _| {},
            )
            .await;
            if received.is_ok() {
                finish(&mut b, true, Pace::DEFAULT).await;
            }
            received
        };
        let ((sent, furthest), received) = tokio::join!(sending, receiving);
        (sent, received, furthest)
    }

    #[tokio::test]
    async fn a_file_on_disk_is_continued_and_cut_to_the_offset() {
        let source = file(300_000);
        let path = std::env::temp_dir().join(format!("pd-transfer-{}.part", std::process::id()));
        // Committed 100 000, plus 20 000 bytes written after the last commit.
        let mut on_disk = source[..100_000].to_vec();
        on_disk.extend_from_slice(&[9u8; 20_000]);
        std::fs::write(&path, &on_disk).unwrap();
        let mut partial = tokio::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .await
            .unwrap();
        let (mut a, mut b) = tokio::io::duplex(256 * 1024);
        let mut reader: &[u8] = &source;
        let sending = async move {
            let sent = send(&mut reader, 300_000, &mut a, Pace::DEFAULT, |_| {}).await;
            drop(a);
            sent
        };
        let have = Have {
            size: 300_000,
            committed: 100_000,
        };
        let receiving = async {
            let received = receive(
                &mut b,
                &mut partial,
                Some(have),
                Some(300_000),
                MAX,
                Pace::DEFAULT,
                |_, _| {},
            )
            .await;
            finish(&mut b, received.is_ok(), Pace::DEFAULT).await;
            received
        };
        let (sent, received) = tokio::join!(sending, receiving);
        assert_eq!(sent.unwrap(), 100_000);
        assert_eq!(received.unwrap(), 100_000);
        drop(partial);
        assert_eq!(std::fs::read(&path).unwrap(), source);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_step_on_a_large_file_gets_time_to_read_it() {
        let pace = Pace::DEFAULT;
        assert_eq!(pace.slow_step(0), pace.idle);
        let eight_gib = 8u64 << 30;
        assert!(
            pace.slow_step(eight_gib) >= pace.idle + Duration::from_secs(400),
            "8 GiB at 20 MiB/s is about 410 s"
        );
    }

    #[tokio::test]
    async fn a_fresh_file_arrives_whole() {
        let source = file(300_000);
        let mut partial = Memory::holding(&[]);
        let (sent, received, _) = run(&source, &mut partial, None).await;
        assert_eq!(sent.unwrap(), 0);
        assert_eq!(received.unwrap(), 0);
        assert_eq!(partial.bytes(), &source[..]);
    }

    #[tokio::test]
    async fn a_partial_of_the_same_file_is_continued() {
        let source = file(300_000);
        let mut partial = Memory::holding(&source[..120_000]);
        let have = Have {
            size: 300_000,
            committed: 120_000,
        };
        let (sent, received, _) = run(&source, &mut partial, Some(have)).await;
        assert_eq!(sent.unwrap(), 120_000, "resumed where it stopped");
        assert_eq!(received.unwrap(), 120_000);
        assert_eq!(partial.bytes(), &source[..]);
    }

    #[tokio::test]
    async fn a_partial_of_another_file_is_dropped_and_the_next_attempt_starts_over() {
        let source = file(300_000);
        let mut other = source.clone();
        other[5] ^= 0xFF;
        let mut partial = Memory::holding(&other[..120_000]);
        let have = Have {
            size: 300_000,
            committed: 120_000,
        };
        let (sent, received, _) = run(&source, &mut partial, Some(have)).await;
        assert!(matches!(sent, Err(TransferError::SourceChanged)));
        assert!(matches!(received, Err(TransferError::SourceChanged)));
        assert!(
            partial.bytes().is_empty(),
            "nothing of the other file is kept"
        );
        let (sent, received, _) = run(&source, &mut partial, None).await;
        assert_eq!(sent.unwrap(), 0);
        assert_eq!(received.unwrap(), 0);
        assert_eq!(partial.bytes(), &source[..]);
    }

    #[tokio::test]
    async fn a_partial_of_another_size_is_not_continued() {
        let source = file(300_000);
        let mut partial = Memory::holding(&source[..120_000]);
        let have = Have {
            size: 400_000,
            committed: 120_000,
        };
        let (sent, received, _) = run(&source, &mut partial, Some(have)).await;
        assert_eq!(sent.unwrap(), 0);
        assert_eq!(received.unwrap(), 0);
        assert_eq!(partial.bytes(), &source[..]);
    }

    #[tokio::test]
    async fn a_record_claiming_more_than_the_partial_holds_continues_from_what_is_there() {
        let source = file(300_000);
        let mut partial = Memory::holding(&source[..50_000]);
        let have = Have {
            size: 300_000,
            committed: 120_000,
        };
        let (sent, received, _) = run(&source, &mut partial, Some(have)).await;
        assert_eq!(sent.unwrap(), 50_000);
        assert_eq!(received.unwrap(), 50_000);
        assert_eq!(partial.bytes(), &source[..]);
    }

    #[tokio::test]
    async fn bytes_that_do_not_match_the_trailer_are_not_kept() {
        let source = file(100_000);
        let (mut a, mut b) = tokio::io::duplex(256 * 1024);
        let mut partial = Memory::holding(&[]);
        let liar = async {
            a.write_all(&100_000u64.to_be_bytes()).await.unwrap();
            let mut point = [0u8; 40];
            a.read_exact(&mut point).await.unwrap();
            a.write_all(&[CONTINUE]).await.unwrap();
            a.write_all(&source).await.unwrap();
            a.write_all(&[0u8; 32]).await.unwrap();
            let mut status = [0u8; 1];
            a.read_exact(&mut status).await.unwrap();
            status[0]
        };
        let receiving = receive(
            &mut b,
            &mut partial,
            None,
            None,
            MAX,
            Pace::DEFAULT,
            |_, _| {},
        );
        let (status, received) = tokio::join!(liar, receiving);
        assert_eq!(status, STATUS_FAILED);
        assert!(matches!(received, Err(TransferError::Corrupt)));
        assert!(partial.bytes().is_empty());
    }

    #[tokio::test]
    async fn a_broken_connection_keeps_what_was_committed() {
        let source = file(10 * 1024 * 1024);
        let (mut a, mut b) = tokio::io::duplex(256 * 1024);
        let mut partial = Memory::holding(&[]);
        let cut = 6 * 1024 * 1024;
        let breaking = async {
            a.write_all(&(source.len() as u64).to_be_bytes())
                .await
                .unwrap();
            let mut point = [0u8; 40];
            a.read_exact(&mut point).await.unwrap();
            a.write_all(&[CONTINUE]).await.unwrap();
            a.write_all(&source[..cut]).await.unwrap();
            drop(a);
        };
        let mut last = None;
        let receiving = receive(
            &mut b,
            &mut partial,
            None,
            None,
            MAX,
            Pace::DEFAULT,
            |s, c| {
                last = Some(Have {
                    size: s,
                    committed: c,
                })
            },
        );
        let (_, received) = tokio::join!(breaking, receiving);
        assert!(matches!(received, Err(TransferError::Io(_))));
        let have = last.expect("a commit was recorded");
        assert_eq!(
            have.committed, cut as u64,
            "everything that arrived is durable"
        );
        assert_eq!(&partial.bytes()[..cut], &source[..cut]);
        // The next attempt sends only the rest.
        let (sent, received, _) = run(&source, &mut partial, Some(have)).await;
        assert_eq!(sent.unwrap(), cut as u64);
        assert_eq!(received.unwrap(), cut as u64);
        assert_eq!(partial.bytes(), &source[..]);
    }

    /// A stream that hands over `bytes`, then fails as a TLS connection
    /// closed without close_notify does.
    struct Failing {
        bytes: std::io::Cursor<Vec<u8>>,
    }

    impl AsyncRead for Failing {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            if self.bytes.position() as usize == self.bytes.get_ref().len() {
                return std::task::Poll::Ready(Err(io::Error::from(io::ErrorKind::UnexpectedEof)));
            }
            std::pin::Pin::new(&mut self.bytes).poll_read(cx, buf)
        }
    }

    impl AsyncWrite for Failing {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<io::Result<usize>> {
            std::task::Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a_connection_that_fails_rather_than_ends_keeps_what_arrived() {
        let source = file(200_000);
        let mut wire = 200_000u64.to_be_bytes().to_vec();
        wire.push(CONTINUE);
        wire.extend_from_slice(&source[..70_000]);
        let mut stream = Failing {
            bytes: std::io::Cursor::new(wire),
        };
        let mut partial = Memory::holding(&[]);
        let mut last = 0;
        let received = receive(
            &mut stream,
            &mut partial,
            None,
            None,
            MAX,
            Pace::DEFAULT,
            |_, c| last = c,
        )
        .await;
        assert!(matches!(received, Err(TransferError::Io(_))));
        assert_eq!(last, 70_000, "what arrived is committed");
        assert_eq!(partial.bytes(), &source[..70_000]);
    }

    #[tokio::test]
    async fn a_size_other_than_announced_or_too_large_is_refused() {
        for (size, expected) in [(0u64, None), (MAX + 1, None), (10, Some(11))] {
            let (mut a, mut b) = tokio::io::duplex(1024);
            a.write_all(&size.to_be_bytes()).await.unwrap();
            let mut partial = Memory::holding(&[]);
            let received = receive(
                &mut b,
                &mut partial,
                None,
                expected,
                MAX,
                Pace::DEFAULT,
                |_, _| {},
            )
            .await;
            assert!(matches!(received, Err(TransferError::Size(_))), "{size}");
        }
    }
}
