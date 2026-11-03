//! The wlroots capture's DMA-BUF side: the compositor renders into buffers
//! the capture made on the card instead of copying its picture into shared
//! memory (`super::super::dmabuf` says why and how the encoder takes them).
//!
//! What the capture does here, in order:
//!
//! 1. Binds `zwp_linux_dmabuf_v1` at version 4 and reads its default feedback:
//!    the compositor's main device and the format and modifier pairs it takes.
//!    A compositor that does not say which GPU it renders on (version 3 or
//!    older) is not guessed at: the capture copies through memory.
//! 2. When a screencopy frame offers a DMA-BUF (`linux_dmabuf`), opens a
//!    VA-API device and a GBM device on the same render node (the node's GPU
//!    must be the compositor's), and tries each modifier the compositor takes,
//!    tiled ones first: a buffer is made with it and the card must import it
//!    and run it through the video processor ([`Importer::verify`]). The first
//!    that works makes a pool of buffers, and the compositor is asked to
//!    import each (`create`, which answers `created` or `failed`: a modifier
//!    it cannot import is given up for the next, and never a protocol error
//!    that would end the connection).
//! 3. Each copy goes into a buffer no frame holds; the frame that carries it
//!    to the encoder is a lease on the buffer.
//!
//! Anything that cannot be done (no libgbm, a compositor on another GPU, a
//! driver that takes no buffer the compositor renders, a picture flipped on
//! the way) turns the capture back to the old copies for good, with one
//! warning, inside the same screencopy session: never the Wayland connection
//! and never the portal. A compositor refusing a copy before any has worked
//! takes the next modifier first.

use std::collections::HashMap;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tracing::{debug, info, warn};
use wayland_client::globals::GlobalList;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
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
    self, Agreement, DRM_FORMAT_ARGB8888, DRM_FORMAT_XRGB8888, Gbm, GpuFrame, MOD_LINEAR, Pool,
};
use crate::streaming::encoder_vaapi::Importer;

/// How many times a modifier's picture may differ from the copy's before the
/// modifier is given up.
const CHECKS: u32 = 3;

/// How long a modifier's check waits for a picture with detail in it (an
/// empty screen shows nothing of how the card reads a buffer). After this a
/// linear buffer is trusted (its layout has no tiles to get wrong), and a
/// tiled one is given up for the next modifier.
const UNDECIDED_FOR: Duration = Duration::from_secs(3);

/// How long all the checks of a capture may take, whatever they say: the
/// capture copies through memory for good after this. (An empty screen waits
/// out [`UNDECIDED_FOR`] for a tiled modifier, which gives them all up, and
/// again for linear.)
const CHECK_BUDGET: Duration = Duration::from_secs(12);

/// The wait between two checks of an empty screen.
const CHECK_GAP: Duration = Duration::from_millis(100);

/// Buffers a capture keeps: one the compositor fills, one waiting for the
/// encoder, one being encoded, the heartbeat's last, and a spare.
const POOL: usize = 5;

/// What the compositor said about its DMA-BUF support.
#[derive(Default)]
pub(super) struct DmabufInfo {
    /// The feedback's main device (`dev_t`), when the compositor said it.
    main_device: Option<u64>,
    /// The feedback's format table: (format, modifier).
    table: Vec<(u32, u64)>,
    /// The tranche being read: its device and the table rows it names.
    tranche_device: Option<u64>,
    tranche: Vec<(u32, u64)>,
    /// Every pair of the tranches for the main device.
    pairs: Vec<(u32, u64)>,
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
    /// an error when it renders on another GPU, or does not say which. A
    /// compositor that does not say is never guessed at: a buffer made on
    /// the wrong card would import on neither.
    fn offered(&self, format: u32, node: &str) -> Result<Vec<u64>> {
        let Some(main) = self.main_device else {
            bail!("the compositor does not say which GPU it renders on");
        };
        match dmabuf::node_device(node) {
            Some(ours) if same_gpu(main, ours) => {}
            _ => bail!("the compositor renders on another GPU than {node}"),
        }
        Ok(self
            .pairs
            .iter()
            .filter(|(f, _)| *f == format)
            .map(|(_, m)| *m)
            .collect())
    }
}

/// The compositor's own events of `zwp_linux_dmabuf_v1` (`format` and
/// `modifier`, deprecated at version 4 in favour of the feedback) say
/// nothing the feedback does not.
impl Dispatch<ZwpLinuxDmabufV1, ()> for WlState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpLinuxDmabufV1,
        _event: zwp_linux_dmabuf_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
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

/// What the compositor answered to one `create` of a buffer.
#[derive(Default)]
pub(super) struct ParamsState {
    /// The `wl_buffer` of `created`.
    buffer: Option<WlBuffer>,
    /// `failed`: the compositor could not import the DMA-BUF.
    failed: bool,
    /// Nobody waits for the answer any more: a `created` that still comes is
    /// let go.
    abandoned: bool,
}

pub(super) type ParamsSlot = Arc<Mutex<ParamsState>>;

impl Dispatch<ZwpLinuxBufferParamsV1, ParamsSlot> for WlState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        data: &ParamsSlot,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let mut slot = data.lock().unwrap();
        match event {
            zwp_linux_buffer_params_v1::Event::Created { buffer } => {
                if slot.abandoned {
                    buffer.destroy();
                } else {
                    slot.buffer = Some(buffer);
                }
            }
            zwp_linux_buffer_params_v1::Event::Failed => slot.failed = true,
            _ => {}
        }
    }

    // The `wl_buffer` of `created` is a new object the compositor makes.
    wayland_client::event_created_child!(WlState, ZwpLinuxBufferParamsV1, [
        zwp_linux_buffer_params_v1::EVT_CREATED_OPCODE => (WlBuffer, ()),
    ]);
}

/// Where a modifier's check stands, apart from the compositor and the card
/// (so the rules are tested without either).
#[derive(Default)]
struct Checks {
    /// The pool's modifier has not been trusted yet.
    pending: bool,
    /// How many checks of this modifier found a different picture.
    failed: u32,
    /// When this modifier's check began.
    since: Option<Instant>,
    /// When the first check of the capture began.
    first: Option<Instant>,
}

/// What one check's result means for the modifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    /// The modifier is trusted.
    Trusted,
    /// Check again, on the next picture (the capture copies meanwhile).
    Again,
    /// Give the modifier up for the next.
    GiveUp,
    /// A tiled modifier that an empty screen could not show to be right: no
    /// tiled modifier can be checked on it, so only linear is tried next.
    Unverifiable,
}

impl Checks {
    /// A new modifier's check begins.
    fn begin(&mut self, now: Instant) {
        self.pending = true;
        self.failed = 0;
        self.since = Some(now);
        self.first.get_or_insert(now);
    }

    /// Take a check's `agreement` for a modifier that is `linear` or tiled.
    /// A different picture is tried again before the modifier is given up (a
    /// moving screen differs between two copies). An empty screen shows
    /// nothing (`Unclear`): the check waits for a picture with detail, up to
    /// [`UNDECIDED_FOR`], and then trusts a linear buffer (there is no tiling
    /// to get wrong) and gives up a tiled one, and with it every other tiled
    /// one (the same empty screen shows none of them to be right).
    fn record(&mut self, agreement: Agreement, linear: bool, now: Instant) -> Verdict {
        if !self.pending {
            return Verdict::Trusted;
        }
        match agreement {
            Agreement::Same => {
                self.pending = false;
                Verdict::Trusted
            }
            Agreement::Differ => {
                self.failed += 1;
                if self.failed >= CHECKS {
                    Verdict::GiveUp
                } else {
                    Verdict::Again
                }
            }
            Agreement::Unclear => {
                let waited = self
                    .since
                    .map_or(Duration::ZERO, |t| now.saturating_duration_since(t));
                if waited < UNDECIDED_FOR {
                    Verdict::Again
                } else if linear {
                    self.pending = false;
                    Verdict::Trusted
                } else {
                    Verdict::Unverifiable
                }
            }
        }
    }

    /// Whether the capture's checks have gone on longer than they may.
    fn over_budget(&self, now: Instant) -> bool {
        self.first
            .is_some_and(|t| now.saturating_duration_since(t) >= CHECK_BUDGET)
    }
}

/// The capture's buffers on the card, and the way to check one against the
/// card before the compositor is given it.
struct Capturer {
    importer: Option<Importer>,
    gbm: Option<Gbm>,
    /// The render node chosen for the session: the converter and the buffers'
    /// allocator are both opened on it, and again on it when either is
    /// replaced.
    node: Option<String>,
    pool: Option<Pool>,
    /// The compositor's `wl_buffer` of each of the pool's buffers (by the
    /// buffer's id), made once with the pool.
    wl: HashMap<u64, WlBuffer>,
    /// What the pool was made for: format, width, height.
    made_for: Option<(u32, u32, u32)>,
    /// The modifiers still to try, after the one the pool uses.
    candidates: Vec<u64>,
    checks: Checks,
}

impl Capturer {
    fn new() -> Self {
        Self {
            importer: None,
            gbm: None,
            node: None,
            pool: None,
            wl: HashMap::new(),
            made_for: None,
            candidates: Vec::new(),
            checks: Checks::default(),
        }
    }

    /// A converter for `width`x`height`, on the session's node once it has
    /// one (the first opens on the driver's choice and sets it).
    fn open_importer(&mut self, width: u32, height: u32) -> Result<()> {
        self.importer = None;
        let importer = Importer::open_on(self.node.as_deref(), width, height)?;
        if self.node.is_none() {
            self.node = Some(importer.node().to_string());
        }
        self.importer = Some(importer);
        Ok(())
    }

    /// Let go of the compositor's buffers of the pool.
    fn release_wl(&mut self) {
        for (_, buffer) in self.wl.drain() {
            buffer.destroy();
        }
    }

    /// Have a pool for `format`, `width`x`height`, making one (and choosing
    /// a modifier) when the picture's shape changed.
    fn prepare(&mut self, info: &DmabufInfo, format: u32, width: u32, height: u32) -> Result<()> {
        if self.made_for == Some((format, width, height)) && self.pool.is_some() {
            return Ok(());
        }
        self.release_wl();
        self.pool = None;
        self.made_for = None;
        if self
            .importer
            .as_ref()
            .is_none_or(|i| !i.matches(width, height))
        {
            self.open_importer(width, height)?;
        }
        let node = self.node.clone().context("no render node")?;
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
    /// the pool with it. Each candidate is tried on a converter no earlier
    /// candidate has touched: one that failed leaves its own converter out
    /// of step, and must not fail the next for it.
    fn select(&mut self) -> Result<()> {
        let Some((format, width, height)) = self.made_for else {
            bail!("no buffer shape");
        };
        let mut last = String::from("no modifier offered");
        while !self.candidates.is_empty() {
            let modifier = self.candidates.remove(0);
            if self.importer.as_ref().is_none_or(|i| i.stuck()) {
                self.open_importer(width, height)?;
            }
            let (Some(gbm), Some(importer)) = (self.gbm.as_ref(), self.importer.as_mut()) else {
                bail!("no device");
            };
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
                drop(probe);
                // Whatever the failure left in this converter goes with it.
                self.open_importer(width, height)?;
                continue;
            }
            let actual = probe.modifier;
            let node = importer.node().to_string();
            drop(probe);
            let mut buffers = Vec::with_capacity(POOL);
            let mut made = Ok(());
            for _ in 0..POOL {
                match gbm.alloc(width, height, format, modifier) {
                    Ok(buf) => buffers.push(buf),
                    Err(e) => {
                        made = Err(e);
                        break;
                    }
                }
            }
            if let Err(e) = made {
                last = format!("{e:#}");
                debug!("zero-copy: modifier {modifier:#x}: {last}");
                continue;
            }
            info!(
                width,
                height,
                modifier = %format!("{actual:#x}"),
                node,
                "Zero-copy capture: the card imports the buffers"
            );
            self.pool = Some(Pool::new(buffers));
            self.checks.begin(Instant::now());
            return Ok(());
        }
        self.pool = None;
        bail!("no modifier works for both the compositor and the card ({last})")
    }

    /// The compositor refused the pool's buffers (at import, or a copy into
    /// one): the next modifier, or an error when there is none.
    fn reject(&mut self) -> Result<()> {
        self.release_wl();
        self.pool = None;
        // A fresh device: the surfaces the card made of the refused buffers
        // go with the old one, and nothing of a layout it read wrongly stays.
        if let Some((_, width, height)) = self.made_for {
            self.open_importer(width, height)?;
        }
        self.select()
    }

    /// Give up every tiled modifier still to try (an empty screen cannot show
    /// one to be right), then the pool's own, for the next: linear, or none.
    fn reject_tiled(&mut self) -> Result<()> {
        self.candidates.retain(|m| *m == MOD_LINEAR);
        self.reject()
    }

    /// Whether the pool's buffers are known to the compositor.
    fn imported(&self) -> bool {
        self.pool.is_some() && !self.wl.is_empty()
    }
}

impl Drop for Capturer {
    fn drop(&mut self) {
        self.release_wl();
    }
}

/// The result of asking for a buffer for one copy.
pub(super) enum Take {
    /// A buffer, and the `wl_buffer` for the compositor to render into (the
    /// capture's own, kept for the next copy: not to be destroyed).
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
    /// DMA-BUF support of a version that names its GPU.
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
        // Version 4 has the compositor say its main device and what it takes
        // in a feedback object; before it nothing says which GPU the buffers
        // must be made on, and the capture copies instead of guessing.
        let dmabuf: ZwpLinuxDmabufV1 = match globals.bind(qh, 4..=4, ()) {
            Ok(d) => d,
            Err(e) => {
                info!(
                    "Capture copies through memory: no zwp_linux_dmabuf_v1 of version 4, \
                     which says the compositor's GPU ({e})"
                );
                return None;
            }
        };
        let info = InfoSlot::default();
        let feedback = dmabuf.get_default_feedback(qh, info.clone());
        // The events sent for the bind and the request come before the
        // roundtrip's own reply.
        let read = queue.roundtrip(state);
        feedback.destroy();
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
    /// `offers` are the screencopy frame's `linux_dmabuf` events. The first
    /// call for a picture's shape makes the pool, which takes a few round
    /// trips to the compositor (once).
    pub(super) fn take(
        &mut self,
        offers: &[(u32, u32, u32)],
        qh: &QueueHandle<WlState>,
        queue: &mut EventQueue<WlState>,
        state: &mut WlState,
    ) -> Take {
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
        if let Err(e) = self.ready_pool(format, width, height, qh, queue, state) {
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
        let Some(buffer) = self.capturer.wl.get(&frame.dmabuf().id).cloned() else {
            self.fail("a buffer the compositor does not know");
            return Take::Copy;
        };
        Take::Frame(frame, buffer)
    }

    /// A pool the card imports and the compositor has taken: modifiers the
    /// card takes are offered to the compositor in turn, and one it cannot
    /// import (it answers `failed`) is given up like one the card refused.
    fn ready_pool(
        &mut self,
        format: u32,
        width: u32,
        height: u32,
        qh: &QueueHandle<WlState>,
        queue: &mut EventQueue<WlState>,
        state: &mut WlState,
    ) -> Result<()> {
        {
            let info = self.info.lock().unwrap();
            self.capturer.prepare(&info, format, width, height)?;
        }
        while !self.capturer.imported() {
            match self.import_pool(qh, queue, state) {
                Ok(()) => break,
                Err(e) => {
                    debug!("zero-copy: the compositor takes no buffer of this modifier: {e:#}");
                    self.capturer.reject()?;
                }
            }
        }
        Ok(())
    }

    /// Ask the compositor to import each of the pool's buffers, and wait for
    /// its answers. `create` is asked for and not `create_immed`: a buffer
    /// the compositor cannot import then comes back as `failed`, where
    /// `create_immed` makes it a protocol error that ends the connection.
    fn import_pool(
        &mut self,
        qh: &QueueHandle<WlState>,
        queue: &mut EventQueue<WlState>,
        state: &mut WlState,
    ) -> Result<()> {
        let Some(pool) = self.capturer.pool.as_ref() else {
            bail!("no buffers");
        };
        let mut asked = Vec::new();
        for buf in pool.buffers() {
            let slot = ParamsSlot::default();
            let params = self.dmabuf.create_params(qh, slot.clone());
            params.add(
                buf.fd.as_fd(),
                0,
                buf.offset,
                buf.stride,
                (buf.modifier >> 32) as u32,
                buf.modifier as u32,
            );
            params.create(
                buf.width as i32,
                buf.height as i32,
                buf.fourcc,
                zwp_linux_buffer_params_v1::Flags::empty(),
            );
            asked.push((buf.id, slot, params));
        }
        // The compositor answers each `create` as it handles it, before it
        // handles the roundtrip's own request.
        let waited = queue.roundtrip(state);
        let mut made = HashMap::new();
        let mut refused: Option<&str> = None;
        for (id, slot, params) in asked {
            let mut slot = slot.lock().unwrap();
            match slot.buffer.take() {
                Some(buffer) if !slot.failed => {
                    made.insert(id, buffer);
                }
                other => {
                    if let Some(buffer) = other {
                        buffer.destroy();
                    }
                    // An answer that is still to come is let go.
                    slot.abandoned = true;
                    refused.get_or_insert(if slot.failed {
                        "the compositor could not import the buffer"
                    } else {
                        "the compositor did not answer"
                    });
                }
            }
            drop(slot);
            params.destroy();
        }
        if let Err(e) = waited {
            for (_, buffer) in made {
                buffer.destroy();
            }
            return Err(e.into());
        }
        if let Some(why) = refused {
            for (_, buffer) in made {
                buffer.destroy();
            }
            bail!("{why}");
        }
        self.capturer.wl = made;
        Ok(())
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
        self.capturer.checks.pending && self.capturer.pool.is_some()
    }

    /// The result of a check, and how long to wait before the next one. A
    /// different picture is tried again before the modifier is given up for
    /// the next; an empty screen is waited out (see [`Checks::record`]). The
    /// capture copies through memory all the while, and for good once the
    /// checks of the session have taken longer than they may.
    pub(super) fn checked(&mut self, agreement: Agreement) -> Duration {
        let now = Instant::now();
        let linear = self.capturer.pool.as_ref().and_then(|p| p.modifier()) == Some(MOD_LINEAR);
        match self.capturer.checks.record(agreement, linear, now) {
            Verdict::Trusted => {}
            Verdict::GiveUp => {
                let modifier = self
                    .capturer
                    .pool
                    .as_ref()
                    .and_then(|p| p.modifier())
                    .unwrap_or(MOD_LINEAR);
                debug!(
                    "zero-copy: modifier {modifier:#x} is not trusted ({agreement:?}); the next one"
                );
                if let Err(e) = self.capturer.reject() {
                    self.fail(&format!("{e:#}"));
                }
            }
            Verdict::Unverifiable => {
                debug!("zero-copy: an empty screen shows no tiled modifier to be right; linear");
                if let Err(e) = self.capturer.reject_tiled() {
                    self.fail(&format!("{e:#}"));
                }
            }
            Verdict::Again => {
                if self.capturer.checks.over_budget(now) {
                    self.fail("the buffers could not be checked in time");
                }
            }
        }
        if agreement == Agreement::Unclear {
            CHECK_GAP
        } else {
            Duration::ZERO
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

impl Drop for ZeroCopy {
    fn drop(&mut self) {
        // The pool's `wl_buffer`s go with the capturer's own drop.
        self.dmabuf.destroy();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::dmabuf::DEVICE_TESTS;

    /// A compositor whose main device is `device` and which names these
    /// modifiers for XRGB8888.
    fn offering(device: u64, modifiers: &[u64]) -> DmabufInfo {
        DmabufInfo {
            main_device: Some(device),
            pairs: modifiers
                .iter()
                .map(|m| (DRM_FORMAT_XRGB8888, *m))
                .collect(),
            ..DmabufInfo::default()
        }
    }

    /// The device number of the render node a converter opens on here, if one
    /// does.
    fn our_device() -> Option<u64> {
        let probe = Importer::open(1280, 720).ok()?;
        dmabuf::node_device(probe.node())
    }

    #[test]
    fn a_compositor_that_does_not_name_its_gpu_is_not_guessed_at() {
        // `/dev/null` is a node that exists, with a device number.
        let dev = dmabuf::node_device("/dev/null").expect("a device number");
        let none = DmabufInfo {
            pairs: vec![(DRM_FORMAT_XRGB8888, MOD_LINEAR)],
            ..DmabufInfo::default()
        };
        assert!(none.offered(DRM_FORMAT_XRGB8888, "/dev/null").is_err());
        // Named, and the same device: its pairs for the format only.
        let mut named = offering(dev, &[MOD_LINEAR]);
        named.pairs.push((DRM_FORMAT_ARGB8888, 5));
        assert_eq!(
            named.offered(DRM_FORMAT_XRGB8888, "/dev/null").unwrap(),
            vec![MOD_LINEAR]
        );
        // Named, and another device (or a node that is not there).
        assert!(
            offering(dev + 1, &[MOD_LINEAR])
                .offered(DRM_FORMAT_XRGB8888, "/dev/null")
                .is_err()
        );
        assert!(
            offering(dev, &[MOD_LINEAR])
                .offered(DRM_FORMAT_XRGB8888, "/dev/pd-no-such-node")
                .is_err()
        );
        // A compositor that names no modifier for the format gets none tried
        // (and not the driver's own choice).
        let empty = offering(dev, &[]);
        let offered = empty.offered(DRM_FORMAT_XRGB8888, "/dev/null").unwrap();
        assert!(dmabuf::candidate_modifiers(&offered, None).is_empty());
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
        let Some(device) = our_device() else {
            eprintln!("skipping the modifier test: no VA-API device here");
            return;
        };
        let before = dmabuf::dmabuf_fds();
        {
            let info = offering(device, &[MOD_LINEAR, X_TILED, Y_TILED]);
            let mut capturer = Capturer::new();
            if let Err(e) = capturer.prepare(&info, DRM_FORMAT_XRGB8888, 1280, 720) {
                eprintln!("skipping the modifier test: {e:#}");
                return;
            }
            let node = capturer.node.clone().expect("a node");
            let mut taken = Vec::new();
            loop {
                let pool = capturer.pool.as_mut().expect("a pool");
                assert!(capturer.checks.pending, "a new pool is unchecked");
                let frame = pool.acquire().expect("a free buffer");
                taken.push(frame.dmabuf().modifier);
                drop(frame);
                if capturer.reject().is_err() {
                    break;
                }
                assert_eq!(capturer.node.as_ref(), Some(&node), "another node");
                assert_eq!(
                    capturer.importer.as_ref().map(|i| i.node().to_string()),
                    Some(node.clone()),
                    "the converter moved to another node"
                );
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

    /// A candidate that fails (GBM cannot make it, the card does not take it,
    /// or it left the converter out of step) must not fail the candidates
    /// after it: linear, the last, still works.
    #[test]
    fn a_failed_candidate_does_not_poison_the_next() {
        const BOGUS: u64 = 0x0100_0000_dead_beef;
        let _only = DEVICE_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let Some(device) = our_device() else {
            eprintln!("skipping the poison test: no VA-API device here");
            return;
        };
        let info = offering(device, &[BOGUS, MOD_LINEAR]);
        let mut capturer = Capturer::new();
        if let Err(e) = capturer.prepare(&info, DRM_FORMAT_XRGB8888, 1280, 720) {
            eprintln!("skipping the poison test: {e:#}");
            return;
        }
        let pool = capturer.pool.as_ref().expect("a pool");
        assert_eq!(pool.modifier(), Some(MOD_LINEAR), "the bogus one was used");
        // A converter that went out of step with one candidate: the next
        // candidate is tried on a new one.
        capturer.importer.as_mut().expect("a converter").poison();
        capturer.pool = None;
        capturer.candidates = vec![MOD_LINEAR];
        capturer
            .select()
            .expect("linear after a poisoned converter");
        assert!(
            !capturer.importer.as_ref().expect("a converter").stuck(),
            "the stuck converter was kept"
        );
        assert_eq!(
            capturer.pool.as_ref().and_then(|p| p.modifier()),
            Some(MOD_LINEAR)
        );
    }

    /// An empty screen shows no tiled modifier to be right: the tiled ones
    /// still to try are all given up and linear is next.
    #[test]
    fn an_unverifiable_tiled_modifier_gives_way_to_linear_at_once() {
        const X_TILED: u64 = 0x0100_0000_0000_0001;
        const Y_TILED: u64 = 0x0100_0000_0000_0002;
        let _only = DEVICE_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let Some(device) = our_device() else {
            eprintln!("skipping the linear test: no VA-API device here");
            return;
        };
        let info = offering(device, &[MOD_LINEAR, X_TILED, Y_TILED]);
        let mut capturer = Capturer::new();
        if let Err(e) = capturer.prepare(&info, DRM_FORMAT_XRGB8888, 1280, 720) {
            eprintln!("skipping the linear test: {e:#}");
            return;
        }
        if capturer.pool.as_ref().and_then(|p| p.modifier()) == Some(MOD_LINEAR) {
            eprintln!("skipping the linear test: the card takes no tiled modifier here");
            return;
        }
        capturer
            .reject_tiled()
            .expect("linear after the tiled ones");
        assert_eq!(
            capturer.pool.as_ref().and_then(|p| p.modifier()),
            Some(MOD_LINEAR)
        );
        assert!(capturer.reject_tiled().is_err(), "nothing after linear");
    }

    /// The checks of a modifier: an empty screen is waited out and then
    /// trusts linear and gives up tiled; a different picture is tried three
    /// times; a picture with detail that matches is trusted at once.
    #[test]
    fn the_check_waits_out_an_empty_screen_and_only_trusts_linear_on_it() {
        let t0 = Instant::now();
        let later = |ms: u64| t0 + Duration::from_millis(ms);
        let mut c = Checks::default();
        c.begin(t0);
        assert!(c.pending);
        // An empty screen: nothing known yet, whichever the layout.
        assert_eq!(
            c.record(Agreement::Unclear, false, later(100)),
            Verdict::Again
        );
        assert_eq!(
            c.record(Agreement::Unclear, true, later(2900)),
            Verdict::Again
        );
        assert!(c.pending);
        // A picture with detail comes: decided at once.
        assert_eq!(
            c.record(Agreement::Same, false, later(3000)),
            Verdict::Trusted
        );
        assert!(!c.pending);

        // Still empty after the wait: linear is trusted, a tiled one is not.
        let mut linear = Checks::default();
        linear.begin(t0);
        assert_eq!(
            linear.record(Agreement::Unclear, true, t0 + UNDECIDED_FOR),
            Verdict::Trusted
        );
        let mut tiled = Checks::default();
        tiled.begin(t0);
        assert_eq!(
            tiled.record(Agreement::Unclear, false, t0 + UNDECIDED_FOR),
            Verdict::Unverifiable
        );

        // A different picture is retried, and given up on the third.
        let mut differ = Checks::default();
        differ.begin(t0);
        assert_eq!(
            differ.record(Agreement::Differ, true, later(1)),
            Verdict::Again
        );
        assert_eq!(
            differ.record(Agreement::Differ, true, later(2)),
            Verdict::Again
        );
        assert_eq!(
            differ.record(Agreement::Differ, true, later(3)),
            Verdict::GiveUp
        );

        // An empty screen's waits are not failures.
        let mut mixed = Checks::default();
        mixed.begin(t0);
        for _ in 0..10 {
            assert_eq!(
                mixed.record(Agreement::Unclear, false, later(1)),
                Verdict::Again
            );
        }
        assert_eq!(
            mixed.record(Agreement::Differ, false, later(2)),
            Verdict::Again
        );

        // The whole session's checks have a budget, over the modifiers.
        let mut budget = Checks::default();
        budget.begin(t0);
        assert!(!budget.over_budget(t0 + CHECK_BUDGET - Duration::from_millis(1)));
        budget.begin(t0 + Duration::from_secs(5));
        assert!(
            budget.over_budget(t0 + CHECK_BUDGET),
            "the first begin counts"
        );
    }
}
