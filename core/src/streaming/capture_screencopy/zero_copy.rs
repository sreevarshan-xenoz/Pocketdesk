//! The wlroots capture's DMA-BUF side: the compositor renders into buffers
//! the capture made on the card instead of copying its picture into shared
//! memory (`super::super::dmabuf` says why and how the encoder takes them).
//!
//! What the capture does here, in order:
//!
//! 1. Binds `zwp_linux_dmabuf_v1` and learns which format and modifier pairs
//!    the compositor takes (version 4's default feedback, with its main
//!    device; version 3's `format` and `modifier` events).
//! 2. When a screencopy frame offers a DMA-BUF (`linux_dmabuf`), opens a
//!    VA-API device and a GBM device on the same render node, and tries each
//!    modifier the compositor takes, tiled ones first: a buffer is made with
//!    it and the card must import it and run it through the video processor
//!    ([`Importer::verify`]). The first that works makes a pool of buffers.
//! 3. Each copy goes into a buffer no frame holds; the frame that carries it
//!    to the encoder is a lease on the buffer.
//!
//! Anything that cannot be done (no libgbm, a compositor on another GPU, a
//! driver that takes no buffer the compositor renders, a picture flipped on
//! the way) turns the capture back to the old copies for good, with one
//! warning. A compositor refusing a copy before any has worked takes the next
//! modifier first.

use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use tracing::{debug, info, warn};
use wayland_client::globals::GlobalList;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::{
    self, ZwpLinuxBufferParamsV1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_feedback_v1::{
    self, ZwpLinuxDmabufFeedbackV1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1::{
    self, ZwpLinuxDmabufV1,
};

use super::WlState;
use crate::streaming::dmabuf::{
    self, DRM_FORMAT_ARGB8888, DRM_FORMAT_XRGB8888, Gbm, GpuFrame, MOD_INVALID, Pool,
};
use crate::streaming::encoder_vaapi::Importer;

/// How many times a modifier's picture may differ from the copy's before the
/// modifier is given up.
const CHECKS: u32 = 3;

/// Buffers a capture keeps: one the compositor fills, one waiting for the
/// encoder, one being encoded, the heartbeat's last, and a spare.
const POOL: usize = 5;

/// What the compositor said about its DMA-BUF support.
#[derive(Default)]
pub(super) struct DmabufInfo {
    /// Version 4's main device (`dev_t`), when the compositor said it.
    main_device: Option<u64>,
    /// The feedback's format table: (format, modifier).
    table: Vec<(u32, u64)>,
    /// The tranche being read: its device and the table rows it names.
    tranche_device: Option<u64>,
    tranche: Vec<(u32, u64)>,
    /// Every pair of the tranches for the main device.
    pairs: Vec<(u32, u64)>,
    /// Version 3's `format` and `modifier` events.
    legacy: Vec<(u32, u64)>,
}

pub(super) type InfoSlot = Arc<Mutex<DmabufInfo>>;

fn dev_t(bytes: &[u8]) -> Option<u64> {
    let eight: [u8; 8] = bytes.get(..8)?.try_into().ok()?;
    Some(u64::from_ne_bytes(eight))
}

/// The format table a feedback sends in a file: 16 bytes a row, a format
/// (u32), four bytes of padding and a modifier (u64).
fn read_table(fd: OwnedFd, size: u32) -> Vec<(u32, u64)> {
    let len = size as usize;
    if len == 0 {
        return Vec::new();
    }
    // SAFETY: a private read-only mapping of the descriptor the compositor
    // sent, `len` bytes as it said; unmapped before returning.
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ,
            libc::MAP_PRIVATE,
            fd.as_raw_fd(),
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return Vec::new();
    }
    // SAFETY: the mapping above is `len` readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
    let table = bytes
        .chunks(16)
        .filter(|row| row.len() == 16)
        .map(|row| {
            let format = u32::from_ne_bytes([row[0], row[1], row[2], row[3]]);
            let modifier = u64::from_ne_bytes([
                row[8], row[9], row[10], row[11], row[12], row[13], row[14], row[15],
            ]);
            (format, modifier)
        })
        .collect();
    // SAFETY: the mapping made above, once.
    unsafe { libc::munmap(ptr, len) };
    table
}

/// Whether two device numbers are the same GPU: the same node, or two nodes
/// (render and primary) of one device.
fn same_gpu(a: u64, b: u64) -> bool {
    if a == b {
        return true;
    }
    let link = |d: u64| {
        std::fs::canonicalize(format!(
            "/sys/dev/char/{}:{}/device",
            libc::major(d),
            libc::minor(d)
        ))
        .ok()
    };
    matches!((link(a), link(b)), (Some(x), Some(y)) if x == y)
}

impl DmabufInfo {
    /// The modifiers the compositor takes for `format` on the GPU at `node`:
    /// an error when it renders on another GPU.
    fn offered(&self, format: u32, node: &str) -> Result<Vec<u64>> {
        if let Some(main) = self.main_device {
            match dmabuf::node_device(node) {
                Some(ours) if same_gpu(main, ours) => {}
                _ => bail!("the compositor renders on another GPU than {node}"),
            }
        }
        let list = if self.main_device.is_some() {
            &self.pairs
        } else {
            &self.legacy
        };
        Ok(list
            .iter()
            .filter(|(f, _)| *f == format)
            .map(|(_, m)| *m)
            .collect())
    }
}

impl Dispatch<ZwpLinuxDmabufV1, InfoSlot> for WlState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpLinuxDmabufV1,
        event: zwp_linux_dmabuf_v1::Event,
        data: &InfoSlot,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let mut info = data.lock().unwrap();
        match event {
            // A format with no modifier named: the driver's own layout.
            zwp_linux_dmabuf_v1::Event::Format { format } => {
                info.legacy.push((format, MOD_INVALID));
            }
            zwp_linux_dmabuf_v1::Event::Modifier {
                format,
                modifier_hi,
                modifier_lo,
            } => {
                let modifier = (u64::from(modifier_hi) << 32) | u64::from(modifier_lo);
                info.legacy.push((format, modifier));
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpLinuxDmabufFeedbackV1, InfoSlot> for WlState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpLinuxDmabufFeedbackV1,
        event: zwp_linux_dmabuf_feedback_v1::Event,
        data: &InfoSlot,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use zwp_linux_dmabuf_feedback_v1::Event;
        let mut info = data.lock().unwrap();
        match event {
            Event::MainDevice { device } => info.main_device = dev_t(&device),
            Event::FormatTable { fd, size } => info.table = read_table(fd, size),
            Event::TrancheTargetDevice { device } => info.tranche_device = dev_t(&device),
            Event::TrancheFormats { indices } => {
                let rows: Vec<(u32, u64)> = indices
                    .chunks(2)
                    .filter(|i| i.len() == 2)
                    .filter_map(|i| {
                        info.table
                            .get(usize::from(u16::from_ne_bytes([i[0], i[1]])))
                    })
                    .copied()
                    .collect();
                info.tranche = rows;
            }
            Event::TrancheDone => {
                let on_main = match (info.tranche_device, info.main_device) {
                    (Some(t), Some(m)) => same_gpu(t, m),
                    _ => true,
                };
                let rows = std::mem::take(&mut info.tranche);
                if on_main {
                    info.pairs.extend(rows);
                }
                info.tranche_device = None;
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpLinuxBufferParamsV1, ()> for WlState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpLinuxBufferParamsV1,
        _event: zwp_linux_buffer_params_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

/// The capture's buffers on the card, and the way to check one against the
/// card before the compositor is given it.
struct Capturer {
    importer: Option<Importer>,
    gbm: Option<Gbm>,
    pool: Option<Pool>,
    /// What the pool was made for: format, width, height.
    made_for: Option<(u32, u32, u32)>,
    /// The modifiers still to try, after the one the pool uses.
    candidates: Vec<u64>,
    /// The pool's modifier has not been checked against the compositor's own
    /// copy yet, and how many checks it has failed.
    unchecked: bool,
    failed_checks: u32,
}

impl Capturer {
    fn new() -> Self {
        Self {
            importer: None,
            gbm: None,
            pool: None,
            made_for: None,
            candidates: Vec::new(),
            unchecked: false,
            failed_checks: 0,
        }
    }

    /// Have a pool for `format`, `width`x`height`, making one (and choosing
    /// a modifier) when the picture's shape changed.
    fn prepare(&mut self, info: &DmabufInfo, format: u32, width: u32, height: u32) -> Result<()> {
        if self.made_for == Some((format, width, height)) && self.pool.is_some() {
            return Ok(());
        }
        self.pool = None;
        self.made_for = None;
        if self
            .importer
            .as_ref()
            .is_none_or(|i| !i.matches(width, height))
        {
            self.importer = Some(Importer::open(width, height)?);
        }
        let node = self.importer.as_ref().map(|i| i.node().to_string());
        let node = node.unwrap_or_default();
        if self.gbm.is_none() {
            self.gbm = Some(Gbm::open(&node)?);
        }
        let offered = info.offered(format, &node)?;
        self.candidates = dmabuf::candidate_modifiers(&offered, dmabuf::forced_modifier());
        debug!(
            "zero-copy: the compositor takes modifiers {:x?} for format {format:#x}; trying {:x?}",
            offered, self.candidates
        );
        self.made_for = Some((format, width, height));
        self.select()
    }

    /// Take the next modifier that makes a buffer the card imports, and make
    /// the pool with it.
    fn select(&mut self) -> Result<()> {
        let Some((format, width, height)) = self.made_for else {
            bail!("no buffer shape");
        };
        let (Some(gbm), Some(importer)) = (self.gbm.as_ref(), self.importer.as_mut()) else {
            bail!("no device");
        };
        let mut last = String::from("no modifier offered");
        while !self.candidates.is_empty() {
            let modifier = self.candidates.remove(0);
            // A buffer made to be tried, and let go.
            let probe = match gbm.alloc(width, height, format, modifier) {
                Ok(buf) => Arc::new(buf),
                Err(e) => {
                    last = format!("{e:#}");
                    debug!("zero-copy: modifier {modifier:#x}: {last}");
                    continue;
                }
            };
            if let Err(e) = importer.verify(&probe) {
                last = format!("{e:#}");
                debug!("zero-copy: modifier {modifier:#x} not taken by the card: {last}");
                continue;
            }
            let actual = probe.modifier;
            drop(probe);
            let mut buffers = Vec::with_capacity(POOL);
            for _ in 0..POOL {
                buffers.push(gbm.alloc(width, height, format, modifier)?);
            }
            info!(
                width,
                height,
                modifier = %format!("{actual:#x}"),
                node = importer.node(),
                "Zero-copy capture: the compositor renders into buffers the card imports"
            );
            self.pool = Some(Pool::new(buffers));
            self.unchecked = true;
            self.failed_checks = 0;
            return Ok(());
        }
        self.pool = None;
        bail!("no modifier works for both the compositor and the card ({last})")
    }

    /// The compositor refused a copy into the pool's buffers: the next
    /// modifier, or an error when there is none.
    fn reject(&mut self) -> Result<()> {
        self.pool = None;
        // A fresh device: the surfaces the card made of the refused buffers
        // go with the old one, and nothing of a layout it read wrongly stays.
        if let Some((_, width, height)) = self.made_for {
            self.importer = None;
            self.importer = Some(Importer::open(width, height)?);
        }
        self.select()
    }
}

/// The result of asking for a buffer for one copy.
pub(super) enum Take {
    /// A buffer, and the `wl_buffer` for the compositor to render into.
    Frame(GpuFrame, WlBuffer),
    /// Every buffer is held by a frame still on its way; skip this copy.
    Busy,
    /// Copy through memory this time (and perhaps always).
    Copy,
}

/// The capture's zero-copy state.
pub(super) struct ZeroCopy {
    dmabuf: ZwpLinuxDmabufV1,
    info: InfoSlot,
    capturer: Capturer,
    /// Turned off by the streamer when its encoder needs the pixels, or a
    /// crop does.
    switch: Arc<AtomicBool>,
    /// Turned off for good by a failure.
    off: bool,
    /// Whether any frame has come through (a refusal before one is a
    /// modifier the compositor will not render into).
    worked: bool,
}

impl ZeroCopy {
    /// `None` where the old copies are wanted or the compositor has no
    /// DMA-BUF support.
    pub(super) fn new(
        globals: &GlobalList,
        qh: &QueueHandle<WlState>,
        queue: &mut EventQueue<WlState>,
        state: &mut WlState,
        switch: Arc<AtomicBool>,
    ) -> Option<Self> {
        if dmabuf::copy_forced() {
            info!("Capture copies through memory (capture_copy / POCKETDESK_CAPTURE_COPY)");
            return None;
        }
        if !switch.load(Ordering::Relaxed) {
            return None;
        }
        let info = InfoSlot::default();
        let dmabuf: ZwpLinuxDmabufV1 = match globals.bind(qh, 3..=4, info.clone()) {
            Ok(d) => d,
            Err(e) => {
                debug!("No zwp_linux_dmabuf_v1 (version 3 or 4): {e}");
                return None;
            }
        };
        // Version 4 has the compositor say its main device and what it takes
        // in a feedback object instead of format events.
        let feedback =
            (dmabuf.version() >= 4).then(|| dmabuf.get_default_feedback(qh, info.clone()));
        // The events sent for the bind and the request come before the
        // roundtrip's own reply.
        let read = queue.roundtrip(state);
        if let Some(feedback) = feedback {
            feedback.destroy();
        }
        if let Err(e) = read {
            debug!("DMA-BUF support not read: {e}");
            return None;
        }
        Some(Self {
            dmabuf,
            info,
            capturer: Capturer::new(),
            switch,
            off: false,
            worked: false,
        })
    }

    /// Whether DMA-BUFs may be used now.
    pub(super) fn active(&self) -> bool {
        !self.off && self.switch.load(Ordering::Relaxed)
    }

    /// A buffer for this copy, if the compositor offers one we can use.
    /// `offers` are the screencopy frame's `linux_dmabuf` events.
    pub(super) fn take(&mut self, offers: &[(u32, u32, u32)], qh: &QueueHandle<WlState>) -> Take {
        if !self.active() {
            return Take::Copy;
        }
        let offer = [DRM_FORMAT_XRGB8888, DRM_FORMAT_ARGB8888]
            .iter()
            .find_map(|f| offers.iter().find(|o| o.0 == *f));
        let Some(&(format, width, height)) = offer else {
            self.decline("the compositor offers no DMA-BUF of 32-bit RGB");
            return Take::Copy;
        };
        let prepared = {
            let info = self.info.lock().unwrap();
            self.capturer.prepare(&info, format, width, height)
        };
        if let Err(e) = prepared {
            self.fail(&format!("{e:#}"));
            return Take::Copy;
        }
        let Some(pool) = self.capturer.pool.as_mut() else {
            self.fail("no buffers");
            return Take::Copy;
        };
        let Some(frame) = pool.acquire() else {
            return Take::Busy;
        };
        let buf = frame.dmabuf();
        let params = self.dmabuf.create_params(qh, ());
        params.add(
            buf.fd.as_fd(),
            0,
            buf.offset,
            buf.stride,
            (buf.modifier >> 32) as u32,
            buf.modifier as u32,
        );
        let wl_buffer = params.create_immed(
            buf.width as i32,
            buf.height as i32,
            buf.fourcc,
            zwp_linux_buffer_params_v1::Flags::empty(),
            qh,
            (),
        );
        params.destroy();
        Take::Frame(frame, wl_buffer)
    }

    /// A copy into a buffer finished and the picture arrived.
    pub(super) fn worked(&mut self) {
        self.worked = true;
    }

    /// Whether the pool's buffers are still to be checked against the
    /// compositor's copy of the same picture. A card can take a tiled buffer
    /// (and so can the compositor) and still read it differently from how the
    /// compositor wrote it; the modifier is trusted only once a picture came
    /// out the same both ways.
    pub(super) fn needs_check(&self) -> bool {
        self.capturer.unchecked && self.capturer.pool.is_some()
    }

    /// The result of a check: `agreed` is whether the card's read-back and
    /// the copy through memory were the same picture. A disagreement is tried
    /// again (a moving screen differs between two copies) before the
    /// modifier is given up for the next.
    pub(super) fn checked(&mut self, agreed: bool) {
        if agreed {
            self.capturer.unchecked = false;
            return;
        }
        self.capturer.failed_checks += 1;
        if self.capturer.failed_checks >= CHECKS {
            let modifier = self
                .capturer
                .pool
                .as_ref()
                .and_then(|p| p.modifier())
                .unwrap_or(MOD_INVALID);
            debug!(
                "zero-copy: modifier {modifier:#x} reads differently from how the compositor writes it"
            );
            if let Err(e) = self.capturer.reject() {
                self.fail(&format!("{e:#}"));
            }
        }
    }

    /// The compositor failed a copy into a buffer. Before any worked, the
    /// modifier is one it will not render into: the next is tried (or, when
    /// none is left, copies through memory), and this says `true`, the
    /// capture goes on. After a copy worked it is the failure a copy through
    /// memory has too (the screen went away, the compositor is busy), and
    /// this says `false`: the capture ends as it does for that.
    pub(super) fn refused(&mut self) -> bool {
        if self.worked {
            return false;
        }
        if let Err(e) = self.capturer.reject() {
            self.fail(&format!("{e:#}"));
        } else {
            debug!("zero-copy: the compositor refused a modifier; the next one");
        }
        true
    }

    /// Turn the DMA-BUFs off for good, saying why once.
    pub(super) fn fail(&mut self, why: &str) {
        if !self.off {
            self.off = true;
            warn!("Zero-copy capture off ({why}); copying through memory");
            self.capturer = Capturer::new();
        }
    }

    /// As [`Self::fail`], for a compositor that simply has no DMA-BUF to give
    /// (a software renderer): not a warning.
    fn decline(&mut self, why: &str) {
        if !self.off {
            self.off = true;
            info!("Capture copies through memory: {why}");
            self.capturer = Capturer::new();
        }
    }

    /// A frame's picture as the video processor makes it for the encoder
    /// (NV12 bytes): for the heartbeat's check that a still screen is still,
    /// and for the check of a new kind of buffer.
    pub(super) fn read_back(&mut self, frame: &GpuFrame) -> Option<Vec<u8>> {
        self.capturer
            .importer
            .as_mut()?
            .nv12_of(frame.dmabuf())
            .ok()
    }

    /// The picture copied through memory (BGRA rows, tightly packed), as the
    /// video processor makes it from an upload: what [`Self::read_back`] of a
    /// buffer is compared with.
    pub(super) fn read_back_copy(&mut self, bgra: &[u8], width: u32) -> Option<Vec<u8>> {
        self.capturer
            .importer
            .as_mut()?
            .nv12_of_bytes(bgra, width * 4)
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::dmabuf::{DEVICE_TESTS, MOD_LINEAR};

    /// A compositor that names these modifiers for XRGB8888 (version 3 style).
    fn offering(modifiers: &[u64]) -> DmabufInfo {
        DmabufInfo {
            legacy: modifiers
                .iter()
                .map(|m| (DRM_FORMAT_XRGB8888, *m))
                .collect(),
            ..DmabufInfo::default()
        }
    }

    /// The capture takes the first modifier the card imports, and when it is
    /// refused (by the compositor or by the check) each next one in turn on a
    /// fresh device, until none is left; no descriptor is left behind.
    /// Self-skips where no VA-API device opens, libgbm is missing, or the
    /// card imports none of these.
    #[test]
    fn a_refused_modifier_gives_way_to_the_next_and_then_to_none() {
        const X_TILED: u64 = 0x0100_0000_0000_0001;
        const Y_TILED: u64 = 0x0100_0000_0000_0002;
        let _only = DEVICE_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let before = dmabuf::dmabuf_fds();
        {
            let info = offering(&[MOD_LINEAR, X_TILED, Y_TILED]);
            let mut capturer = Capturer::new();
            if let Err(e) = capturer.prepare(&info, DRM_FORMAT_XRGB8888, 1280, 720) {
                eprintln!("skipping the modifier test: {e:#}");
                return;
            }
            let mut taken = Vec::new();
            loop {
                let pool = capturer.pool.as_mut().expect("a pool");
                assert!(capturer.unchecked, "a new pool is unchecked");
                let frame = pool.acquire().expect("a free buffer");
                taken.push(frame.dmabuf().modifier);
                drop(frame);
                if capturer.reject().is_err() {
                    break;
                }
            }
            println!("modifiers taken in turn: {taken:x?}");
            assert!(!taken.is_empty());
            // Tiled first, linear last, none twice.
            if let Some(at) = taken.iter().position(|m| *m == MOD_LINEAR) {
                assert_eq!(at, taken.len() - 1, "linear is the last resort");
            }
            let mut seen = taken.clone();
            seen.dedup();
            assert_eq!(seen.len(), taken.len(), "a modifier tried twice");
            assert!(capturer.pool.is_none(), "no pool when none is left");
        }
        assert_eq!(dmabuf::dmabuf_fds(), before, "descriptors left behind");
    }
}
