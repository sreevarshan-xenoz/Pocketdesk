//! The PipeWire capture's DMA-BUF machinery with the card behind it (see the
//! parent module for the whole picture).

use std::collections::HashMap;
use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use super::{ModifierProp, Step, decide};
use crate::streaming::StreamingConfig;
use crate::streaming::dmabuf::{
    self, DRM_FORMAT_XRGB8888, DmaBuf, Gbm, GpuFrame, MOD_INVALID, MOD_LINEAR,
};
use crate::streaming::encoder_vaapi::Importer;

/// The size of the buffers each modifier is tried on: big enough for any
/// tiling's alignment, small enough to cost nothing.
const PROBE: u32 = 256;

/// What [`PwDma::take`] made of a dequeued buffer.
pub(in crate::streaming) enum Take {
    /// A DMA-BUF the card imported: the frame holds the stream's buffer until
    /// it is let go. The caller must not queue the buffer back itself.
    Frame(GpuFrame),
    /// A memory buffer: the caller copies it out as it always did.
    Copy,
    /// A buffer that carries no picture (the pointer moved): queue it back.
    Nothing,
    /// A DMA-BUF that cannot be used now (the capture is turning back to
    /// copies, or the card refused it): queue it back, no frame.
    Drop,
}

/// The stream buffers a frame is waiting to give back, and the way to wake
/// the stream's thread to do it (`pw_stream_queue_buffer` belongs to that
/// thread).
struct Returns {
    queue: Mutex<Vec<(usize, u64)>>,
    wake: OwnedFd,
    open: AtomicBool,
}

impl Returns {
    fn new() -> Result<Self> {
        // SAFETY: eventfd with no initial count; the descriptor is checked
        // and owned from here on.
        let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        anyhow::ensure!(fd >= 0, "eventfd: {}", std::io::Error::last_os_error());
        Ok(Self {
            queue: Mutex::new(Vec::new()),
            // SAFETY: a new descriptor nobody else owns.
            wake: unsafe { OwnedFd::from_raw_fd(fd) },
            open: AtomicBool::new(true),
        })
    }

    /// A frame is done with buffer `ptr` of generation `epoch`.
    fn give(&self, ptr: usize, epoch: u64) {
        if !self.open.load(Ordering::Acquire) {
            return;
        }
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((ptr, epoch));
        let one = 1u64.to_ne_bytes();
        // SAFETY: a write of eight bytes to the eventfd; a full counter
        // (EAGAIN) means the loop is already due to wake.
        unsafe {
            libc::write(
                std::os::fd::AsRawFd::as_raw_fd(&self.wake),
                one.as_ptr().cast(),
                8,
            )
        };
    }
}

/// The lease a frame holds on a stream buffer: letting go of the last clone
/// of the frame gives the buffer back.
struct PwLease {
    ptr: usize,
    epoch: u64,
    returns: Arc<Returns>,
}

impl Drop for PwLease {
    fn drop(&mut self) {
        self.returns.give(self.ptr, self.epoch);
    }
}

/// One stream buffer: which generation of buffers it belongs to (the address
/// of a `pw_buffer` can come back for a new one after a renegotiation), and
/// the import the card took of it.
struct Live {
    epoch: u64,
    buf: Option<Arc<DmaBuf>>,
}

struct State {
    live: HashMap<usize, Live>,
    next_gen: u64,
    /// The modifier of the DMA-BUF format that was negotiated.
    modifier: Option<u64>,
    /// Gives each new buffer its first import and conversion.
    importer: Option<Importer>,
    node: String,
    /// DMA-BUF was given up for the rest of the session.
    gave_up: bool,
    /// The memory-only format is wanted, and has not been sent yet.
    plain_due: bool,
    /// ... and has been sent.
    plain_sent: bool,
}

/// The PipeWire capture's DMA-BUF state, shared between the stream's
/// callbacks, its main loop and the frames that hold its buffers.
pub(in crate::streaming) struct PwDma {
    switch: Arc<AtomicBool>,
    ours: Vec<u64>,
    returns: Arc<Returns>,
    state: Mutex<State>,
}

impl PwDma {
    /// Find which modifiers the card takes and, with any, make the capture's
    /// DMA-BUF state. `None` (and `switch` cleared, so the streamer knows the
    /// frames are copies) when the session copies, or the card, libgbm or the
    /// compositor's GPU leave nothing to offer.
    pub(in crate::streaming) fn open(
        _config: &StreamingConfig,
        switch: &Arc<AtomicBool>,
    ) -> Option<Arc<Self>> {
        if !switch.load(Ordering::Relaxed) {
            return None;
        }
        match probe() {
            Ok((ours, node)) if !ours.is_empty() => {
                info!(
                    node = %node,
                    modifiers = ?ours.iter().map(|m| format!("{m:#x}")).collect::<Vec<_>>(),
                    "PipeWire capture offers DMA-BUF buffers"
                );
                let returns = match Returns::new() {
                    Ok(r) => Arc::new(r),
                    Err(e) => {
                        info!("PipeWire capture copies through memory: {e:#}");
                        switch.store(false, Ordering::Relaxed);
                        return None;
                    }
                };
                Some(Arc::new(Self {
                    switch: switch.clone(),
                    ours,
                    returns,
                    state: Mutex::new(State {
                        live: HashMap::new(),
                        next_gen: 1,
                        modifier: None,
                        importer: None,
                        node,
                        gave_up: false,
                        plain_due: false,
                        plain_sent: false,
                    }),
                }))
            }
            Ok(_) => {
                info!("PipeWire capture copies through memory: the card takes no DMA-BUF modifier");
                switch.store(false, Ordering::Relaxed);
                None
            }
            Err(e) => {
                info!("PipeWire capture copies through memory: {e:#}");
                switch.store(false, Ordering::Relaxed);
                None
            }
        }
    }

    /// The modifiers offered, tiled first.
    pub(in crate::streaming) fn modifiers(&self) -> &[u64] {
        &self.ours
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// What to do with the modifier of the Format that was just negotiated.
    pub(in crate::streaming) fn step(&self, prop: Option<&ModifierProp>) -> Step {
        let copies_asked = !self.switch.load(Ordering::Relaxed);
        let mut st = self.lock();
        let step = decide(prop, &self.ours, copies_asked || st.gave_up);
        match &step {
            Step::Memory => st.modifier = None,
            Step::Ready(m) => st.modifier = Some(*m),
            Step::Fixate(_) => {}
            Step::GiveUp(why) => {
                st.modifier = None;
                if !st.gave_up {
                    st.gave_up = true;
                    match why {
                        Some(why) => {
                            warn!("PipeWire capture back to copies through memory: {why}")
                        }
                        None => {
                            debug!("PipeWire capture back to copies through memory: asked for")
                        }
                    }
                }
                st.plain_due = true;
            }
        }
        step
    }

    /// The stream added a buffer: a new generation of it.
    pub(in crate::streaming) fn buffer_added(&self, ptr: *mut pipewire::sys::pw_buffer) {
        let mut st = self.lock();
        let epoch = st.next_gen;
        st.next_gen += 1;
        st.live.insert(ptr as usize, Live { epoch, buf: None });
    }

    /// The stream is freeing a buffer: it is never queued again, and its
    /// import is let go (frames still holding it keep their own reference).
    pub(in crate::streaming) fn buffer_removed(&self, ptr: *mut pipewire::sys::pw_buffer) {
        self.lock().live.remove(&(ptr as usize));
    }

    /// Look at the buffer just dequeued (`ptr`, of `width`x`height`): a
    /// DMA-BUF the card has imported becomes a frame that holds it.
    pub(in crate::streaming) fn take(
        &self,
        ptr: *mut pipewire::sys::pw_buffer,
        width: u32,
        height: u32,
    ) -> Take {
        use libspa::sys as spa;
        // SAFETY: `ptr` is a `pw_buffer` the stream just dequeued, and stays
        // valid and unqueued until it is given back; the reads below are of
        // its `spa_buffer`, plain data.
        let (kind, fd, offset, size, stride, corrupted, planes) = unsafe {
            let buffer = (*ptr).buffer;
            if buffer.is_null() || (*buffer).n_datas == 0 || (*buffer).datas.is_null() {
                return Take::Copy;
            }
            let data = &*(*buffer).datas;
            let (offset, size, stride, flags) = if data.chunk.is_null() {
                (0, 0, 0, 0)
            } else {
                let c = &*data.chunk;
                (c.offset, c.size, c.stride, c.flags)
            };
            (
                data.type_,
                data.fd,
                offset,
                size,
                stride,
                flags & spa::SPA_CHUNK_FLAG_CORRUPTED as i32 != 0,
                (*buffer).n_datas,
            )
        };
        if kind != spa::SPA_DATA_DmaBuf {
            return Take::Copy;
        }
        if corrupted || size == 0 {
            return Take::Nothing;
        }
        let mut st = self.lock();
        if st.gave_up || !self.switch.load(Ordering::Relaxed) {
            return Take::Drop;
        }
        let Some(modifier) = st.modifier else {
            return Take::Drop;
        };
        if planes != 1 {
            self.give_up(&mut st, "the compositor's buffers have several planes");
            return Take::Drop;
        }
        if fd < 0 || stride < (width * 4) as i32 {
            self.give_up(&mut st, "the compositor's buffer says no usable layout");
            return Take::Drop;
        }
        let key = ptr as usize;
        if !st.live.contains_key(&key) {
            let epoch = st.next_gen;
            st.next_gen += 1;
            st.live.insert(key, Live { epoch, buf: None });
        }
        let known = st.live[&key].buf.clone().filter(|b| {
            b.width == width
                && b.height == height
                && b.modifier == modifier
                && b.stride == stride as u32
                && b.offset == offset
        });
        let buf = match known {
            Some(buf) => buf,
            None => {
                match self.import(&mut st, fd, width, height, modifier, stride as u32, offset) {
                    Ok(buf) => {
                        st.live.get_mut(&key).expect("inserted above").buf = Some(buf.clone());
                        buf
                    }
                    Err(e) => {
                        self.give_up(&mut st, &format!("the card did not take a buffer: {e:#}"));
                        return Take::Drop;
                    }
                }
            }
        };
        let epoch = st.live[&key].epoch;
        let lease = Arc::new(PwLease {
            ptr: key,
            epoch,
            returns: self.returns.clone(),
        });
        Take::Frame(GpuFrame::leased(buf, lease))
    }

    /// Make this process's own reference to the stream buffer `fd` and have
    /// the card import it and run it through the video processor, as the
    /// encoder will: the check before any frame of it is sent.
    #[allow(clippy::too_many_arguments)]
    fn import(
        &self,
        st: &mut State,
        fd: i64,
        width: u32,
        height: u32,
        modifier: u64,
        stride: u32,
        offset: u32,
    ) -> Result<Arc<DmaBuf>> {
        // SAFETY: duplicating a descriptor the stream owns and keeps open
        // while the buffer lives; the duplicate is ours, close-on-exec.
        let dup = unsafe { libc::fcntl(fd as i32, libc::F_DUPFD_CLOEXEC, 3) };
        anyhow::ensure!(
            dup >= 0,
            "dup of the buffer: {}",
            std::io::Error::last_os_error()
        );
        let buf = Arc::new(DmaBuf {
            id: DmaBuf::next_id(),
            // SAFETY: a new descriptor nobody else owns.
            fd: unsafe { OwnedFd::from_raw_fd(dup) },
            width,
            height,
            fourcc: DRM_FORMAT_XRGB8888,
            modifier,
            stride,
            offset,
        });
        let fits = st
            .importer
            .as_ref()
            .is_some_and(|i| i.matches(width, height) && !i.stuck());
        if !fits {
            st.importer = None;
            st.importer = Some(Importer::open_on(Some(&st.node), width, height)?);
        }
        let importer = st.importer.as_mut().context("no converter")?;
        if let Err(e) = importer.verify(&buf) {
            st.importer = None;
            return Err(e);
        }
        Ok(buf)
    }

    fn give_up(&self, st: &mut State, why: &str) {
        if !st.gave_up {
            st.gave_up = true;
            warn!("PipeWire capture back to copies through memory: {why}");
        }
        st.plain_due = true;
    }

    /// Whether the memory-only format must be sent now (once): DMA-BUF was
    /// given up, or the streamer cleared the switch while DMA-BUF was in use.
    pub(in crate::streaming) fn plain_wanted(&self) -> bool {
        let mut st = self.lock();
        if st.plain_sent {
            return false;
        }
        let asked = !self.switch.load(Ordering::Relaxed) && st.modifier.is_some();
        if st.plain_due || asked {
            st.plain_sent = true;
            st.plain_due = false;
            if asked {
                st.gave_up = true;
                info!(
                    "PipeWire capture back to copies through memory: the picture needs its pixels"
                );
            }
            return true;
        }
        false
    }

    /// The stream buffers whose frames are done, ready to be queued back by
    /// the stream's thread (a buffer that was freed meanwhile is not named).
    pub(in crate::streaming) fn returned(&self) -> Vec<*mut pipewire::sys::pw_buffer> {
        let taken =
            std::mem::take(&mut *self.returns.queue.lock().unwrap_or_else(|e| e.into_inner()));
        if taken.is_empty() {
            return Vec::new();
        }
        let st = self.lock();
        taken
            .into_iter()
            .filter(|(ptr, epoch)| st.live.get(ptr).is_some_and(|l| l.epoch == *epoch))
            .map(|(ptr, _)| ptr as *mut pipewire::sys::pw_buffer)
            .collect()
    }

    /// A descriptor the stream's loop can wait on: readable when a frame
    /// gave a buffer back. The loop reads it to clear it.
    pub(in crate::streaming) fn wake_fd(&self) -> Result<OwnedFd> {
        self.returns
            .wake
            .try_clone()
            .context("duplicating the wake descriptor")
    }

    /// The stream is ending: frames that outlive it give nothing back.
    pub(in crate::streaming) fn close(&self) {
        self.returns.open.store(false, Ordering::Release);
        self.returns
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.lock().live.clear();
    }
}

/// Which modifiers the card takes for XRGB8888 buffers the compositor
/// renders: the card's EGL lists what it can render to, and each is tried on
/// a buffer of this program's own through the VA-API importer and the video
/// processor. Tiled first, linear last; single-plane only (the buffer must be
/// one plane for the importer), and never the driver's own choice
/// (`MOD_INVALID`: a layout nobody was told).
fn probe() -> Result<(Vec<u64>, String)> {
    let mut importer = Importer::open_on(None, PROBE, PROBE)?;
    let node = importer.node().to_string();
    let gbm = Gbm::open(&node)?;
    let forced = dmabuf::forced_modifier();
    let listed = match gbm.egl_modifiers(DRM_FORMAT_XRGB8888) {
        Ok(list) => list,
        Err(e) => {
            debug!("EGL lists no modifiers ({e:#}): linear only");
            vec![MOD_LINEAR]
        }
    };
    let mut candidates = dmabuf::candidate_modifiers(&listed, forced);
    candidates.retain(|m| *m != MOD_INVALID);
    let mut taken = Vec::new();
    for modifier in candidates {
        let tried = gbm
            .alloc(PROBE, PROBE, DRM_FORMAT_XRGB8888, modifier)
            .and_then(|b| {
                let b = Arc::new(b);
                importer.verify(&b)
            });
        match tried {
            Ok(()) => taken.push(modifier),
            Err(e) => {
                debug!("The card does not take modifier {modifier:#x}: {e:#}");
                if importer.stuck() {
                    importer = Importer::open_on(Some(&node), PROBE, PROBE)?;
                }
            }
        }
    }
    Ok((taken, node))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dma(modifiers: Vec<u64>) -> Arc<PwDma> {
        Arc::new(PwDma {
            switch: Arc::new(AtomicBool::new(true)),
            ours: modifiers,
            returns: Arc::new(Returns::new().unwrap()),
            state: Mutex::new(State {
                live: HashMap::new(),
                next_gen: 1,
                modifier: None,
                importer: None,
                node: String::new(),
                gave_up: false,
                plain_due: false,
                plain_sent: false,
            }),
        })
    }

    fn ptr(n: usize) -> *mut pipewire::sys::pw_buffer {
        n as *mut pipewire::sys::pw_buffer
    }

    #[test]
    fn a_lease_gives_its_buffer_back_when_the_last_frame_goes() {
        let d = dma(vec![MOD_LINEAR]);
        d.buffer_added(ptr(0x1000));
        let epoch = d.lock().live[&0x1000].epoch;
        let lease: Arc<dyn Send + Sync> = Arc::new(PwLease {
            ptr: 0x1000,
            epoch,
            returns: d.returns.clone(),
        });
        let held = lease.clone();
        drop(lease);
        assert!(d.returned().is_empty(), "a clone still holds it");
        drop(held);
        assert_eq!(d.returned(), vec![ptr(0x1000)]);
        assert!(d.returned().is_empty(), "named once");
    }

    #[test]
    fn a_buffer_freed_or_replaced_is_not_queued_again() {
        let d = dma(vec![MOD_LINEAR]);
        d.buffer_added(ptr(0x1000));
        let old = d.lock().live[&0x1000].epoch;
        // The stream frees it and makes a new one at the same address.
        d.buffer_removed(ptr(0x1000));
        d.buffer_added(ptr(0x1000));
        d.returns.give(0x1000, old);
        assert!(
            d.returned().is_empty(),
            "the old generation's frame is late"
        );
        let new = d.lock().live[&0x1000].epoch;
        d.returns.give(0x1000, new);
        assert_eq!(d.returned(), vec![ptr(0x1000)]);
        // Freed with a frame still holding it.
        d.buffer_removed(ptr(0x1000));
        d.returns.give(0x1000, new);
        assert!(d.returned().is_empty());
    }

    #[test]
    fn frames_that_outlive_the_stream_give_nothing_back() {
        let d = dma(vec![MOD_LINEAR]);
        d.buffer_added(ptr(0x2000));
        let epoch = d.lock().live[&0x2000].epoch;
        d.close();
        d.returns.give(0x2000, epoch);
        assert!(d.returned().is_empty());
    }

    #[test]
    fn a_give_wakes_the_loop() {
        let d = dma(vec![MOD_LINEAR]);
        let fd = d.wake_fd().unwrap();
        let mut pollfd = libc::pollfd {
            fd: std::os::fd::AsRawFd::as_raw_fd(&fd),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd, no wait.
        assert_eq!(unsafe { libc::poll(&mut pollfd, 1, 0) }, 0);
        d.returns.give(1, 1);
        // SAFETY: as above.
        assert_eq!(unsafe { libc::poll(&mut pollfd, 1, 0) }, 1);
    }

    #[test]
    fn giving_up_asks_for_the_memory_format_once() {
        let d = dma(vec![0x0100_0000_0000_0001]);
        assert!(!d.plain_wanted());
        let step = d.step(Some(&ModifierProp::Fixed(5)));
        assert!(matches!(step, Step::GiveUp(Some(_))));
        assert!(d.plain_wanted());
        assert!(!d.plain_wanted(), "once");
        // Later formats stay given up, quietly.
        assert!(matches!(
            d.step(Some(&ModifierProp::Fixed(0x0100_0000_0000_0001))),
            Step::GiveUp(None)
        ));
    }

    #[test]
    fn the_streamer_asking_for_copies_renegotiates_only_if_dma_buf_was_in_use() {
        let d = dma(vec![MOD_LINEAR]);
        // Cleared before anything was negotiated: no stream to redo.
        d.switch.store(false, Ordering::Relaxed);
        assert!(!d.plain_wanted());
        let d = dma(vec![MOD_LINEAR]);
        assert_eq!(
            d.step(Some(&ModifierProp::Fixed(MOD_LINEAR))),
            Step::Ready(MOD_LINEAR)
        );
        assert!(!d.plain_wanted());
        d.switch.store(false, Ordering::Relaxed);
        assert!(d.plain_wanted());
        assert!(!d.plain_wanted());
    }
}
