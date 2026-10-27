//! Frame buffers handed back for the next capture (BG9).
//!
//! A 2560x1080 BGRA frame is 11 MB, and a new buffer for each frame has its
//! pages faulted in again (about 1 ms a frame on Windows). While a capture
//! that takes its buffers from here runs (it holds a [`Lease`]), a
//! [`VideoFrame`](super::VideoFrame) dropped anywhere gives its buffer back,
//! and the capture's next frame takes it. With no capture running nothing is
//! kept, so a session's buffers go when it ends.

use std::sync::{Mutex, MutexGuard};

/// Buffers kept at most: about one being filled, one waiting, one being
/// encoded, and the heartbeat's.
const KEEP: usize = 4;

struct Pool {
    users: usize,
    free: Vec<Vec<u8>>,
}

impl Pool {
    const fn new() -> Self {
        Self {
            users: 0,
            free: Vec::new(),
        }
    }

    /// A kept buffer that holds `len` bytes; smaller ones are let go.
    fn take(&mut self, len: usize) -> Option<Vec<u8>> {
        while let Some(buffer) = self.free.pop() {
            if buffer.capacity() >= len {
                return Some(buffer);
            }
        }
        None
    }

    /// Keep `buffer` if a capture is running and there is room; else hand it
    /// back so it is freed outside the lock.
    fn give_back(&mut self, buffer: Vec<u8>) -> Option<Vec<u8>> {
        if self.users > 0 && self.free.len() < KEEP && buffer.capacity() > 0 {
            self.free.push(buffer);
            None
        } else {
            Some(buffer)
        }
    }

    fn lease(&mut self) {
        self.users += 1;
    }

    /// The last lease gone: the kept buffers, to be freed outside the lock.
    #[cfg_attr(not(all(feature = "capture", windows)), allow(dead_code))]
    fn release(&mut self) -> Vec<Vec<u8>> {
        self.users = self.users.saturating_sub(1);
        if self.users == 0 {
            std::mem::take(&mut self.free)
        } else {
            Vec::new()
        }
    }
}

static POOL: Mutex<Pool> = Mutex::new(Pool::new());

fn pool() -> MutexGuard<'static, Pool> {
    POOL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Held by a capture that takes its buffers with [`take`]: while any is
/// held, dropped frames give their buffers back; when the last goes, the
/// kept buffers are freed.
#[cfg_attr(not(all(feature = "capture", windows)), allow(dead_code))]
pub(crate) struct Lease(());

#[cfg_attr(not(all(feature = "capture", windows)), allow(dead_code))]
impl Lease {
    pub(crate) fn new() -> Self {
        pool().lease();
        Self(())
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let freed = pool().release();
        drop(freed);
    }
}

/// A buffer of `len` bytes for a frame, its contents unspecified: a kept one
/// when one is large enough, else a new one.
#[cfg_attr(not(all(feature = "capture", windows)), allow(dead_code))]
pub(crate) fn take(len: usize) -> Vec<u8> {
    let kept = pool().take(len);
    match kept {
        Some(mut buffer) => {
            buffer.resize(len, 0);
            buffer
        }
        None => vec![0u8; len],
    }
}

/// A dropped frame's buffer, kept for the next [`take`] while a capture
/// holds a [`Lease`].
pub(crate) fn give_back(buffer: Vec<u8>) {
    let unkept = pool().give_back(buffer);
    drop(unkept);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_buffer_is_kept_only_while_a_capture_runs() {
        let mut pool = Pool::new();
        assert!(pool.give_back(vec![1; 16]).is_some(), "no capture: let go");

        pool.lease();
        let buffer = vec![1u8; 16];
        let ptr = buffer.as_ptr();
        assert!(pool.give_back(buffer).is_none(), "kept");
        let again = pool.take(16).expect("the kept buffer");
        assert_eq!(again.as_ptr(), ptr, "the same memory, not a copy");
    }

    #[test]
    fn a_buffer_too_small_for_the_frame_is_let_go() {
        // A window crop grows: the old, smaller buffers do not fit it.
        let mut pool = Pool::new();
        pool.lease();
        assert!(pool.give_back(vec![0; 8]).is_none());
        assert!(pool.take(16).is_none());
        assert!(pool.free.is_empty(), "the small one went");

        assert!(pool.give_back(Vec::with_capacity(32)).is_none());
        assert!(
            pool.take(16).is_some(),
            "a larger one serves a smaller frame"
        );
    }

    #[test]
    fn at_most_a_few_are_kept() {
        let mut pool = Pool::new();
        pool.lease();
        for _ in 0..KEEP {
            assert!(pool.give_back(vec![0; 4]).is_none());
        }
        assert!(pool.give_back(vec![0; 4]).is_some(), "full: let go");
        assert!(
            pool.give_back(Vec::new()).is_some(),
            "an empty one is not kept"
        );
    }

    #[test]
    fn the_last_capture_to_end_frees_the_kept_buffers() {
        let mut pool = Pool::new();
        pool.lease();
        pool.lease();
        assert!(pool.give_back(vec![0; 4]).is_none());
        assert!(pool.release().is_empty(), "another capture still runs");
        assert_eq!(pool.release().len(), 1, "freed with the last");
        assert!(pool.give_back(vec![0; 4]).is_some(), "no capture: let go");
    }

    #[test]
    fn a_taken_buffer_is_the_frame_size() {
        let _lease = Lease::new();
        let buffer = take(64);
        assert_eq!(buffer.len(), 64);
        give_back(buffer);
        assert_eq!(take(16).len(), 16, "a larger kept buffer cut to size");
    }
}
