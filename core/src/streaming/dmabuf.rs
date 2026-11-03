//! Frames kept on the GPU from the compositor to the VA-API encoder
//! (phase 2 of the low-latency window handoff, "zero-copy capture").
//!
//! Until now a wlroots capture asked the compositor for pixels in shared
//! memory: the compositor read its picture back to the processor, the
//! capture copied it, and the encoder uploaded it to the card again before
//! the video processor converted it. Here the capture gives the compositor a
//! DMA-BUF to render into instead (`zwp_linux_dmabuf_v1`, the buffer made by
//! GBM on the encoder's own render node), and the in-process VA-API encoder
//! imports that same buffer as a VA surface (`av_hwframe_map` from
//! `AV_PIX_FMT_DRM_PRIME`) and feeds it to the `scale_vaapi` graph it
//! already runs. No copy and no upload.
//!
//! What lives here: the setting that forces the old copy
//! ([`copy_forced`]), the buffer and the frame that carries it
//! ([`DmaBuf`], [`GpuFrame`]), the small GBM binding that allocates it
//! ([`Gbm`], opened with `dlopen` so no build or package needs libgbm), and
//! the pure choice of which modifiers to offer the compositor
//! ([`candidate_modifiers`]). The import into VA-API is in
//! [`super::encoder_vaapi`] (it owns the device).
//!
//! A frame is a lease on one of a few buffers the capture owns ([`Pool`]):
//! the capture hands a buffer to the compositor only while no frame holds
//! it, and the buffer's file descriptor and any VA surface made of it close
//! when the last holder lets go.

use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::{Context, Result, bail, ensure};

/// The variable that forces the old copy through memory (a debugging and
/// measuring switch; the setting is `capture_copy`).
pub const COPY_ENV: &str = "POCKETDESK_CAPTURE_COPY";

/// The variable that offers the compositor one modifier only, `linear` or a
/// number in hex (a measuring and debugging switch: by default the tiled
/// ones the card takes come first).
pub const MODIFIER_ENV: &str = "POCKETDESK_CAPTURE_DMABUF_MODIFIER";

/// `DRM_FORMAT_XRGB8888` (`XR24`): B, G, R, unused, as the capture's BGRA.
pub const DRM_FORMAT_XRGB8888: u32 = 0x3432_5258;
/// `DRM_FORMAT_ARGB8888` (`AR24`).
pub const DRM_FORMAT_ARGB8888: u32 = 0x3432_5241;
/// `DRM_FORMAT_MOD_LINEAR`.
pub const MOD_LINEAR: u64 = 0;
/// `DRM_FORMAT_MOD_INVALID`: "the driver chooses", what a compositor with no
/// modifier support means.
pub const MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

static COPY_SETTING: AtomicBool = AtomicBool::new(false);

/// Set from the owner's setting when the server starts; read as each capture
/// begins.
pub fn set_copy(on: bool) {
    COPY_SETTING.store(on, Ordering::Relaxed);
}

/// Whether the capture must copy through memory, as it always did: the
/// setting `capture_copy`, or `POCKETDESK_CAPTURE_COPY=1`.
pub fn copy_forced() -> bool {
    copy_forced_by(
        std::env::var(COPY_ENV).ok().as_deref(),
        COPY_SETTING.load(Ordering::Relaxed),
    )
}

fn copy_forced_by(var: Option<&str>, set: bool) -> bool {
    match var.map(str::trim) {
        Some("") | Some("0") => set,
        Some(_) => true,
        None => set,
    }
}

/// The one modifier the switch asks for, if it is set.
#[cfg_attr(not(feature = "inproc"), allow(dead_code))]
pub(crate) fn forced_modifier() -> Option<u64> {
    parse_modifier(std::env::var(MODIFIER_ENV).ok().as_deref())
}

fn parse_modifier(var: Option<&str>) -> Option<u64> {
    let v = var?.trim().to_ascii_lowercase();
    match v.as_str() {
        "" => None,
        "linear" => Some(MOD_LINEAR),
        hex => u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok(),
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// One DMA-BUF: a single-plane 32-bit RGB buffer on the card.
#[derive(Debug)]
pub struct DmaBuf {
    /// Names the buffer for the encoder's cache of surfaces made of it.
    pub id: u64,
    pub fd: OwnedFd,
    pub width: u32,
    pub height: u32,
    /// The DRM fourcc the buffer was made with.
    pub fourcc: u32,
    pub modifier: u64,
    /// Bytes a row takes.
    pub stride: u32,
    /// Where the plane starts in the buffer.
    pub offset: u32,
}

impl DmaBuf {
    /// A new id for a buffer the encoder's cache of surfaces is keyed by.
    pub(crate) fn next_id() -> u64 {
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    }

    /// The buffer's size in bytes: what the kernel says, else the plane's.
    pub fn size(&self) -> usize {
        // SAFETY: lseek on a valid, owned descriptor; a dma-buf answers
        // SEEK_END with its size.
        let end = unsafe { libc::lseek(self.fd.as_raw_fd(), 0, libc::SEEK_END) };
        if end > 0 {
            end as usize
        } else {
            self.stride as usize * self.height as usize + self.offset as usize
        }
    }

    /// The picture's pixels read through a mapping of the buffer, for a
    /// linear buffer only: a tiled one is not in row order, and the card's
    /// own read-back of an imported tiled buffer is not reliable (the driver
    /// crashed in it), so the capture turns back to copies instead.
    pub fn read_linear(&self) -> Result<Vec<u8>> {
        ensure!(
            self.modifier == MOD_LINEAR,
            "a tiled buffer is not read back to memory"
        );
        let len = self.size();
        // SAFETY: a shared read-only mapping of a valid dma-buf of `len`
        // bytes; unmapped before returning.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                self.fd.as_raw_fd(),
                0,
            )
        };
        ensure!(
            ptr != libc::MAP_FAILED,
            "mmap of the dma-buf failed: {}",
            std::io::Error::last_os_error()
        );
        sync(self.fd.as_raw_fd(), true);
        let row = self.width as usize * 4;
        let mut out = vec![0u8; row * self.height as usize];
        for y in 0..self.height as usize {
            let from = self.offset as usize + y * self.stride as usize;
            if from + row > len {
                break;
            }
            // SAFETY: `from + row <= len`, inside the mapping.
            let src = unsafe { std::slice::from_raw_parts((ptr as *const u8).add(from), row) };
            out[y * row..(y + 1) * row].copy_from_slice(src);
        }
        sync(self.fd.as_raw_fd(), false);
        // SAFETY: the mapping made above, once.
        unsafe { libc::munmap(ptr, len) };
        Ok(out)
    }
}

/// `DMA_BUF_IOCTL_SYNC` around the processor's read of a mapping: the start
/// of the read, then its end.
fn sync(fd: c_int, start: bool) {
    #[repr(C)]
    struct DmaBufSync {
        flags: u64,
    }
    const DMA_BUF_SYNC_READ: u64 = 1;
    const DMA_BUF_SYNC_START: u64 = 0;
    const DMA_BUF_SYNC_END: u64 = 4;
    // _IOW('b', 0, struct dma_buf_sync)
    const DMA_BUF_IOCTL_SYNC: libc::c_ulong = 0x4008_6200;
    let arg = DmaBufSync {
        flags: DMA_BUF_SYNC_READ
            | if start {
                DMA_BUF_SYNC_START
            } else {
                DMA_BUF_SYNC_END
            },
    };
    // SAFETY: the ioctl takes a pointer to one `struct dma_buf_sync`; a
    // descriptor that refuses it only means the read is not fenced.
    unsafe { libc::ioctl(fd, DMA_BUF_IOCTL_SYNC, &arg) };
}

/// A frame still on the card: one of the capture's buffers, held for as long
/// as this (or any clone) lives, so the capture does not give it to the
/// compositor again.
#[derive(Clone)]
pub struct GpuFrame {
    buf: Arc<DmaBuf>,
    /// What the buffer's owner counts the frames holding it by: the pool's
    /// count of holders, or (the PipeWire capture) a lease that gives the
    /// buffer back to the stream when its last frame goes.
    _lease: Arc<dyn Send + Sync>,
}

impl std::fmt::Debug for GpuFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuFrame").field("buf", &self.buf).finish()
    }
}

impl GpuFrame {
    /// A frame on `buf` that holds `lease` for as long as it (or any clone)
    /// lives: dropping the last one is the owner's cue that the buffer is
    /// free again.
    #[cfg_attr(not(feature = "inproc"), allow(dead_code))]
    pub(crate) fn leased(buf: Arc<DmaBuf>, lease: Arc<dyn Send + Sync>) -> Self {
        Self { buf, _lease: lease }
    }

    /// The buffer, for the encoder's import.
    pub fn dmabuf(&self) -> &Arc<DmaBuf> {
        &self.buf
    }

    /// A crop narrows a frame on Windows. Here a frame is read back first
    /// (`VideoFrame::crop_region`), so there is nothing to move.
    pub(crate) fn offset(&mut self, _x: u32, _y: u32) {}

    /// The frame read back to the processor, BGRA, for an encoder or a step
    /// that needs its pixels. Only a linear buffer can be (see
    /// [`DmaBuf::read_linear`]): the streamer drops the few frames on their
    /// way when it turns the capture back to copies.
    pub(crate) fn download(&self, width: u32, height: u32) -> Result<Vec<u8>> {
        ensure!(
            width == self.buf.width && height == self.buf.height,
            "a frame of a different size than its buffer"
        );
        self.buf.read_linear()
    }
}

/// The capture's buffers: a few, each lent to a frame at a time.
pub struct Pool {
    slots: Vec<(Arc<DmaBuf>, Arc<()>)>,
    next: usize,
}

impl Pool {
    pub fn new(buffers: Vec<DmaBuf>) -> Self {
        Self {
            slots: buffers
                .into_iter()
                .map(|b| (Arc::new(b), Arc::new(())))
                .collect(),
            next: 0,
        }
    }

    /// A buffer no frame holds, as a frame (the capture sends it, or lets it
    /// go and the buffer is free again); `None` when every one is in use.
    pub fn acquire(&mut self) -> Option<GpuFrame> {
        let n = self.slots.len();
        for i in 0..n {
            let at = (self.next + i) % n;
            let (buf, lease) = &self.slots[at];
            if Arc::strong_count(lease) == 1 {
                self.next = (at + 1) % n;
                return Some(GpuFrame {
                    buf: buf.clone(),
                    _lease: lease.clone(),
                });
            }
        }
        None
    }

    /// The pool's buffers, held or not.
    pub fn buffers(&self) -> impl Iterator<Item = &Arc<DmaBuf>> {
        self.slots.iter().map(|(b, _)| b)
    }

    /// The modifier the pool's buffers were made with.
    pub fn modifier(&self) -> Option<u64> {
        self.slots.first().map(|(b, _)| b.modifier)
    }

    /// How many frames hold a buffer now.
    pub fn in_use(&self) -> usize {
        self.slots
            .iter()
            .filter(|(_, lease)| Arc::strong_count(lease) > 1)
            .count()
    }
}

/// The modifiers to try for a format, in order: the compositor's, those the
/// card's driver can take as a surface first, tiled ones before linear (the
/// card renders and reads them quicker), the others dropped. `offered` is
/// what the compositor takes for the format; `forced` is the measuring
/// switch, which names the one to try. A modifier the compositor did not name
/// is never tried (not even the driver's own choice, `MOD_INVALID`, unless it
/// is named): an empty `offered` has nothing to try, and the capture copies.
pub fn candidate_modifiers(offered: &[u64], forced: Option<u64>) -> Vec<u64> {
    let mut tiled = Vec::new();
    let mut linear = Vec::new();
    for &m in offered {
        let list = if m == MOD_LINEAR {
            &mut linear
        } else {
            &mut tiled
        };
        if !list.contains(&m) {
            list.push(m);
        }
    }
    if let Some(modifier) = forced {
        return vec![modifier];
    }
    let mut out = tiled;
    out.extend(linear);
    out
}

/// Whether a frame still on the card must be read back, and the capture
/// turned back to copies: its encoder cannot take it as it is, a crop is to
/// cut it, or its size is odd (the stream is 4:2:0, the software crop trims it
/// even).
pub fn needs_pixels(encoder_takes_dmabuf: bool, cropped: bool, width: u32, height: u32) -> bool {
    !encoder_takes_dmabuf || cropped || (width | height) & 1 != 0
}

/// What a comparison of the card's picture with the copy's came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agreement {
    /// The same picture, and one with enough in it that a buffer read in the
    /// wrong layout would not have matched.
    Same,
    /// Not the same picture.
    Differ,
    /// The same, but a picture with next to nothing in it (an empty desktop):
    /// a buffer read in the wrong layout would look the same, so nothing is
    /// known until a picture with detail comes.
    Unclear,
}

/// How far two NV12 bytes may be apart and still be the same: both pictures
/// went through the same video processor, so only a moving screen differs.
const TOLERANCE: u8 = 3;

/// A step in luma this big between neighbours is detail (an edge of text, a
/// window's border), and not a gradient.
const EDGE_STEP: u8 = 24;

/// The picture has enough detail when one luma sample in this many is an
/// edge.
const DETAIL_ONE_IN: usize = 400;

/// How many luma samples of `luma` (rows of `width`) sit on an edge: the next
/// sample on the row or the one below is `EDGE_STEP` or more away.
fn edges(luma: &[u8], width: usize) -> usize {
    let mut n = 0;
    for (i, &v) in luma.iter().enumerate() {
        let right = (i + 1) % width != 0 && luma[i + 1].abs_diff(v) >= EDGE_STEP;
        let below = luma
            .get(i + width)
            .is_some_and(|b| b.abs_diff(v) >= EDGE_STEP);
        n += usize::from(right || below);
    }
    n
}

/// Whether two NV12 pictures of rows `width` wide (`card`: the video
/// processor's output for the buffer, `copy`: its output for the
/// compositor's copy through memory) are the same, to a tolerance of a few
/// bytes' worth (a moving screen changes between the two copies).
///
/// A buffer read in the wrong layout comes out shuffled, and a shuffled
/// picture looks the same only if there is nothing in it: so a match counts
/// only for a picture with detail, and the mismatching bytes must be a tenth
/// at most of the picture's edges. A picture with next to nothing in it that
/// matches says [`Agreement::Unclear`], not yes.
pub fn agreement(card: &[u8], copy: &[u8], width: usize) -> Agreement {
    // NV12 is `width * height * 3 / 2` bytes.
    if card.len() != copy.len() || width == 0 || card.is_empty() || !card.len().is_multiple_of(3) {
        return Agreement::Differ;
    }
    let luma = card.len() / 3 * 2;
    if !luma.is_multiple_of(width) {
        return Agreement::Differ;
    }
    let mismatch = card
        .iter()
        .zip(copy)
        .filter(|(a, b)| a.abs_diff(**b) > TOLERANCE)
        .count();
    let detail = edges(&copy[..luma], width);
    if detail < luma / DETAIL_ONE_IN {
        // Nothing to tell a shuffled picture from the right one.
        return if mismatch * 200 <= card.len() {
            Agreement::Unclear
        } else {
            Agreement::Differ
        };
    }
    if mismatch * 10 <= detail {
        Agreement::Same
    } else {
        Agreement::Differ
    }
}

// ---------------------------------------------------------------------
// GBM, through dlopen
// ---------------------------------------------------------------------

/// `GBM_BO_USE_RENDERING`: the buffer is a render target.
const GBM_BO_USE_RENDERING: u32 = 1 << 2;

type CreateDevice = unsafe extern "C" fn(c_int) -> *mut c_void;
type DestroyDevice = unsafe extern "C" fn(*mut c_void);
type CreateWithModifiers2 =
    unsafe extern "C" fn(*mut c_void, u32, u32, u32, *const u64, c_uint, u32) -> *mut c_void;
type CreateBo = unsafe extern "C" fn(*mut c_void, u32, u32, u32, u32) -> *mut c_void;
type BoGetFd = unsafe extern "C" fn(*mut c_void) -> c_int;
type BoGetU64 = unsafe extern "C" fn(*mut c_void) -> u64;
type BoGetPlane = unsafe extern "C" fn(*mut c_void, c_int) -> u32;
type BoGetPlaneCount = unsafe extern "C" fn(*mut c_void) -> c_int;
type BoDestroy = unsafe extern "C" fn(*mut c_void);

/// A GBM device on one render node, for making buffers.
pub struct Gbm {
    lib: *mut c_void,
    device: *mut c_void,
    node: std::fs::File,
    destroy_device: DestroyDevice,
    create_with_modifiers2: CreateWithModifiers2,
    create_bo: CreateBo,
    get_fd: BoGetFd,
    get_stride: BoGetPlane,
    get_offset: BoGetPlane,
    get_modifier: BoGetU64,
    get_plane_count: BoGetPlaneCount,
    destroy_bo: BoDestroy,
}

// SAFETY: the device and its functions are used from one thread at a time,
// through `&mut`/`&` on the owner, which the capture thread holds.
unsafe impl Send for Gbm {}

impl Gbm {
    /// Open libgbm and a device on the render node `path`. Never a node the
    /// caller has not chosen: the caller names it (the one VA-API opened).
    pub fn open(path: &str) -> Result<Self> {
        let lib = ["libgbm.so.1", "libgbm.so"].iter().find_map(|name| {
            let name = CString::new(*name).ok()?;
            // SAFETY: dlopen of a library name; the handle is closed in Drop.
            let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
            (!handle.is_null()).then_some(handle)
        });
        let lib = lib.context("libgbm is not installed")?;
        // Everything below is closed through `Gbm::drop` once `Self` exists;
        // until then a failure closes the library itself.
        let symbol = |name: &CStr| -> Result<*mut c_void> {
            // SAFETY: dlsym on a live handle with a NUL-terminated name.
            let p = unsafe { libc::dlsym(lib, name.as_ptr()) };
            if p.is_null() {
                bail!("libgbm lacks {}", name.to_string_lossy());
            }
            Ok(p)
        };
        let made = (|| -> Result<Self> {
            let node = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .with_context(|| format!("opening {path}"))?;
            // SAFETY: each symbol is the libgbm function of that name, with
            // the signature gbm.h declares for it.
            let (create_device, destroy_device, with_mods, create_bo) = unsafe {
                (
                    std::mem::transmute::<*mut c_void, CreateDevice>(symbol(c"gbm_create_device")?),
                    std::mem::transmute::<*mut c_void, DestroyDevice>(symbol(
                        c"gbm_device_destroy",
                    )?),
                    std::mem::transmute::<*mut c_void, CreateWithModifiers2>(symbol(
                        c"gbm_bo_create_with_modifiers2",
                    )?),
                    std::mem::transmute::<*mut c_void, CreateBo>(symbol(c"gbm_bo_create")?),
                )
            };
            // SAFETY: as above.
            let (get_fd, get_stride, get_offset, get_modifier, get_plane_count, destroy_bo) = unsafe {
                (
                    std::mem::transmute::<*mut c_void, BoGetFd>(symbol(c"gbm_bo_get_fd")?),
                    std::mem::transmute::<*mut c_void, BoGetPlane>(symbol(
                        c"gbm_bo_get_stride_for_plane",
                    )?),
                    std::mem::transmute::<*mut c_void, BoGetPlane>(symbol(c"gbm_bo_get_offset")?),
                    std::mem::transmute::<*mut c_void, BoGetU64>(symbol(c"gbm_bo_get_modifier")?),
                    std::mem::transmute::<*mut c_void, BoGetPlaneCount>(symbol(
                        c"gbm_bo_get_plane_count",
                    )?),
                    std::mem::transmute::<*mut c_void, BoDestroy>(symbol(c"gbm_bo_destroy")?),
                )
            };
            // SAFETY: a valid render-node descriptor, which the device only
            // borrows: `node` is kept for as long as it lives.
            let device = unsafe { create_device(node.as_raw_fd()) };
            ensure!(!device.is_null(), "gbm_create_device failed on {path}");
            Ok(Self {
                lib,
                device,
                node,
                destroy_device,
                create_with_modifiers2: with_mods,
                create_bo,
                get_fd,
                get_stride,
                get_offset,
                get_modifier,
                get_plane_count,
                destroy_bo,
            })
        })();
        match made {
            Ok(gbm) => Ok(gbm),
            Err(e) => {
                // SAFETY: the handle from dlopen above, not yet owned by a Gbm.
                unsafe { libc::dlclose(lib) };
                Err(e)
            }
        }
    }

    /// The node this device allocates on.
    pub fn node_fd(&self) -> c_int {
        self.node.as_raw_fd()
    }

    /// The modifiers the card's EGL says it can render to and sample from
    /// for `fourcc` (`eglQueryDmaBufModifiersEXT` on a display made of this
    /// device), the ones that need the external-only sampler left out. This
    /// is the list a PipeWire stream offers its compositor: the card's own
    /// answer, with no table of modifiers in this program. `libEGL` is
    /// `dlopen`ed like libgbm, so nothing links it; an error says why the
    /// list is not known (no libEGL, no such extension, no display).
    pub fn egl_modifiers(&self, fourcc: u32) -> Result<Vec<u64>> {
        // `EGL_PLATFORM_GBM_KHR`.
        const PLATFORM_GBM: u32 = 0x31D7;
        type GetProc = unsafe extern "C" fn(*const c_char) -> *mut c_void;
        type GetDisplay = unsafe extern "C" fn(u32, *mut c_void, *const i32) -> *mut c_void;
        type Initialize = unsafe extern "C" fn(*mut c_void, *mut i32, *mut i32) -> c_uint;
        type Terminate = unsafe extern "C" fn(*mut c_void) -> c_uint;
        type Query =
            unsafe extern "C" fn(*mut c_void, i32, i32, *mut u64, *mut c_uint, *mut i32) -> c_uint;
        let lib = ["libEGL.so.1", "libEGL.so"].iter().find_map(|name| {
            let name = CString::new(*name).ok()?;
            // SAFETY: dlopen of a library name. The handle is kept open for
            // the life of the process (libglvnd does not like being closed
            // while a driver it loaded is still mapped).
            let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
            (!handle.is_null()).then_some(handle)
        });
        let lib = lib.context("libEGL is not installed")?;
        // SAFETY: dlsym on a live handle with NUL-terminated names; each
        // symbol is the EGL function of that name, with the signature of
        // EGL 1.4 and `EGL_EXT_platform_base`/`EGL_EXT_image_dma_buf_import_modifiers`.
        let (get_display, initialize, terminate, query) = unsafe {
            let sym = |name: &CStr| -> Result<*mut c_void> {
                let p = libc::dlsym(lib, name.as_ptr());
                if p.is_null() {
                    bail!("libEGL lacks {}", name.to_string_lossy());
                }
                Ok(p)
            };
            let get_proc = std::mem::transmute::<*mut c_void, GetProc>(sym(c"eglGetProcAddress")?);
            let ext = |name: &CStr| -> Result<*mut c_void> {
                let p = get_proc(name.as_ptr());
                if p.is_null() {
                    bail!("EGL lacks {}", name.to_string_lossy());
                }
                Ok(p)
            };
            (
                std::mem::transmute::<*mut c_void, GetDisplay>(ext(c"eglGetPlatformDisplayEXT")?),
                std::mem::transmute::<*mut c_void, Initialize>(sym(c"eglInitialize")?),
                std::mem::transmute::<*mut c_void, Terminate>(sym(c"eglTerminate")?),
                std::mem::transmute::<*mut c_void, Query>(ext(c"eglQueryDmaBufModifiersEXT")?),
            )
        };
        // SAFETY: the display is made of this live GBM device and terminated
        // before returning; the arrays handed to the query are as long as
        // the count the query itself reported.
        unsafe {
            let display = get_display(PLATFORM_GBM, self.device, std::ptr::null());
            ensure!(!display.is_null(), "no EGL display on the GBM device");
            let (mut major, mut minor) = (0i32, 0i32);
            if initialize(display, &mut major, &mut minor) == 0 {
                bail!("eglInitialize failed");
            }
            let listed = (|| -> Result<Vec<u64>> {
                let mut count = 0i32;
                ensure!(
                    query(
                        display,
                        fourcc as i32,
                        0,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        &mut count
                    ) != 0,
                    "EGL does not know the format's modifiers"
                );
                if count <= 0 {
                    return Ok(Vec::new());
                }
                let mut mods = vec![0u64; count as usize];
                let mut external = vec![0 as c_uint; count as usize];
                let mut got = 0i32;
                ensure!(
                    query(
                        display,
                        fourcc as i32,
                        count,
                        mods.as_mut_ptr(),
                        external.as_mut_ptr(),
                        &mut got
                    ) != 0,
                    "EGL's list of modifiers failed"
                );
                let got = (got.max(0) as usize).min(mods.len());
                Ok(mods
                    .into_iter()
                    .zip(external)
                    .take(got)
                    .filter(|(_, ext)| *ext == 0)
                    .map(|(m, _)| m)
                    .collect())
            })();
            terminate(display);
            listed
        }
    }

    /// A buffer of `width`x`height` in `fourcc`, tiled as `modifier` says
    /// (`MOD_INVALID`: the driver's choice). Single-plane buffers only.
    pub fn alloc(&self, width: u32, height: u32, fourcc: u32, modifier: u64) -> Result<DmaBuf> {
        // SAFETY: the device is live; the modifier array outlives the call;
        // the returned object is destroyed below on every path.
        let bo = unsafe {
            if modifier == MOD_INVALID {
                (self.create_bo)(self.device, width, height, fourcc, GBM_BO_USE_RENDERING)
            } else {
                let mods = [modifier];
                (self.create_with_modifiers2)(
                    self.device,
                    width,
                    height,
                    fourcc,
                    mods.as_ptr(),
                    1,
                    GBM_BO_USE_RENDERING,
                )
            }
        };
        ensure!(
            !bo.is_null(),
            "GBM cannot make a {width}x{height} buffer with modifier {modifier:#x}"
        );
        // SAFETY: `bo` is live until the destroy call at the end of this
        // block; the descriptor from `get_fd` is a new one we own.
        unsafe {
            let planes = (self.get_plane_count)(bo);
            let fd = (self.get_fd)(bo);
            let made = if planes != 1 {
                if fd >= 0 {
                    drop(OwnedFd::from_raw_fd(fd));
                }
                Err(anyhow::anyhow!(
                    "modifier {modifier:#x} makes a buffer of {planes} planes"
                ))
            } else if fd < 0 {
                Err(anyhow::anyhow!("gbm_bo_get_fd failed"))
            } else {
                Ok(DmaBuf {
                    id: DmaBuf::next_id(),
                    fd: OwnedFd::from_raw_fd(fd),
                    width,
                    height,
                    fourcc,
                    modifier: (self.get_modifier)(bo),
                    stride: (self.get_stride)(bo, 0),
                    offset: (self.get_offset)(bo, 0),
                })
            };
            (self.destroy_bo)(bo);
            made
        }
    }
}

impl Drop for Gbm {
    fn drop(&mut self) {
        // SAFETY: the device and library are ours, closed once; buffers made
        // from the device live on through their descriptors.
        unsafe {
            (self.destroy_device)(self.device);
            libc::dlclose(self.lib);
        }
    }
}

/// How many descriptors this process holds now (a leak check for tests and
/// the capture's own log).
pub fn open_fds() -> usize {
    std::fs::read_dir("/proc/self/fd").map_or(0, |d| d.count())
}

/// How many of this process's descriptors are dma-bufs (a leak check for
/// the tests that hold the card).
#[cfg(all(test, feature = "inproc"))]
pub(crate) fn dmabuf_fds() -> usize {
    std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(|e| std::fs::read_link(e.ok()?.path()).ok())
        .filter(|link| link.to_string_lossy().contains("dmabuf"))
        .count()
}

/// A `dev_t`'s device number for the node at `path`, to compare with the
/// compositor's main device.
pub fn node_device(path: &str) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.rdev())
}

/// The tests that open the card (here and in the encoder) take turns, so a
/// count of this process's dma-bufs means one test's.
#[cfg(all(test, feature = "inproc"))]
pub(crate) static DEVICE_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_or_the_variable_forces_the_copy() {
        assert!(!copy_forced_by(None, false));
        assert!(copy_forced_by(None, true));
        assert!(copy_forced_by(Some("1"), false));
        assert!(copy_forced_by(Some("yes"), false));
        // An empty or zero variable leaves the setting in charge.
        assert!(!copy_forced_by(Some(""), false));
        assert!(!copy_forced_by(Some("0"), false));
        assert!(copy_forced_by(Some("0"), true));
    }

    #[test]
    fn modifiers_are_tried_tiled_first_and_linear_last() {
        const Y_TILED: u64 = 0x0100_0000_0000_0002;
        const X_TILED: u64 = 0x0100_0000_0000_0001;
        assert_eq!(
            candidate_modifiers(&[MOD_LINEAR, Y_TILED, X_TILED, Y_TILED], None),
            vec![Y_TILED, X_TILED, MOD_LINEAR]
        );
        assert_eq!(
            candidate_modifiers(&[Y_TILED, MOD_LINEAR], Some(MOD_LINEAR)),
            vec![MOD_LINEAR]
        );
        // A compositor that names none gets nothing tried, and the driver's
        // own choice only when it is named.
        assert!(candidate_modifiers(&[], None).is_empty());
        assert_eq!(candidate_modifiers(&[MOD_INVALID], None), vec![MOD_INVALID]);
    }

    #[test]
    fn a_frame_on_the_card_is_read_back_only_where_something_needs_its_pixels() {
        assert!(!needs_pixels(true, false, 1920, 1080));
        // Another encoder, a crop, or an odd size.
        assert!(needs_pixels(false, false, 1920, 1080));
        assert!(needs_pixels(true, true, 1920, 1080));
        assert!(needs_pixels(true, false, 1919, 1080));
        assert!(needs_pixels(true, false, 1920, 1079));
    }

    #[test]
    fn the_modifier_switch_reads_linear_or_hex() {
        assert_eq!(parse_modifier(None), None);
        assert_eq!(parse_modifier(Some("")), None);
        assert_eq!(parse_modifier(Some(" Linear ")), Some(MOD_LINEAR));
        assert_eq!(
            parse_modifier(Some("0x0100000000000002")),
            Some(0x0100_0000_0000_0002)
        );
        assert_eq!(
            parse_modifier(Some("100000000000001")),
            Some(0x0100_0000_0000_0001)
        );
        assert_eq!(parse_modifier(Some("tiled")), None);
    }

    fn buffer(id: u64) -> DmaBuf {
        // A descriptor of a throwaway file stands in for the dma-buf: the
        // pool only counts holders.
        let file = std::fs::File::open("/dev/null").unwrap();
        DmaBuf {
            id,
            fd: file.into(),
            width: 4,
            height: 4,
            fourcc: DRM_FORMAT_XRGB8888,
            modifier: MOD_LINEAR,
            stride: 16,
            offset: 0,
        }
    }

    #[test]
    fn a_buffer_is_lent_to_one_frame_at_a_time() {
        let mut pool = Pool::new(vec![buffer(1), buffer(2)]);
        let a = pool.acquire().expect("first buffer");
        let b = pool.acquire().expect("second buffer");
        assert_ne!(a.dmabuf().id, b.dmabuf().id);
        assert!(pool.acquire().is_none(), "both are held");
        assert_eq!(pool.in_use(), 2);
        // A clone (the heartbeat's repeat) keeps the buffer held.
        let a2 = a.clone();
        drop(a);
        assert!(pool.acquire().is_none(), "a clone still holds it");
        drop(a2);
        let again = pool.acquire().expect("freed with its last holder");
        assert_eq!(again.dmabuf().id, 1);
        drop(b);
        drop(again);
        assert_eq!(pool.in_use(), 0);
    }

    /// How many of this process's descriptors point at `target`.
    fn fds_on(target: &std::path::Path) -> usize {
        std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(|e| std::fs::read_link(e.ok()?.path()).ok())
            .filter(|link| link == target)
            .count()
    }

    #[test]
    fn dropping_the_pool_and_its_frames_closes_every_descriptor() {
        // A file of this test's own, so other tests' descriptors do not count.
        let path = std::env::temp_dir().join(format!("pd-dmabuf-pool-{}", std::process::id()));
        std::fs::write(&path, b"x").unwrap();
        let open = |id| DmaBuf {
            id,
            fd: std::fs::File::open(&path).unwrap().into(),
            ..buffer(id)
        };
        let mut pool = Pool::new((0..4).map(open).collect());
        let frames: Vec<_> = (0..4).filter_map(|_| pool.acquire()).collect();
        assert_eq!(fds_on(&path), 4);
        drop(pool);
        // The frames still hold the buffers.
        assert_eq!(fds_on(&path), 4);
        drop(frames);
        assert_eq!(fds_on(&path), 0);
        let _ = std::fs::remove_file(&path);
    }

    /// An NV12 picture of `w`x`h` with plenty of edges, like text: luma
    /// stripes that shift down the rows, flat chroma.
    fn detailed(w: usize, h: usize) -> Vec<u8> {
        let mut p = vec![128u8; w * h * 3 / 2];
        for y in 0..h {
            for x in 0..w {
                p[y * w + x] = if (x / 3 + y / 5) % 2 == 0 { 40 } else { 210 };
            }
        }
        p
    }

    #[test]
    fn a_picture_with_detail_is_the_same_when_a_few_bytes_differ() {
        let (w, h) = (64, 48);
        let a = detailed(w, h);
        assert_eq!(agreement(&a, &a, w), Agreement::Same);
        // A cursor, a blink: a few bytes far off, one a little off.
        let mut c = a.clone();
        c[0] ^= 0xff;
        c[400] ^= 0xff;
        c[900] = c[900].wrapping_add(2);
        assert_eq!(agreement(&a, &c, w), Agreement::Same);
        // A picture that came out as zeros, or half wrong, is not.
        let zeros = vec![0u8; a.len()];
        assert_eq!(agreement(&zeros, &a, w), Agreement::Differ);
        let mut half = a.clone();
        for b in half.iter_mut().take(a.len() / 2) {
            *b ^= 0xff;
        }
        assert_eq!(agreement(&half, &a, w), Agreement::Differ);
        assert_eq!(agreement(&a[..16], &a, w), Agreement::Differ);
        assert_eq!(agreement(&[], &[], w), Agreement::Differ);
        assert_eq!(agreement(&a, &a, 0), Agreement::Differ);
        // A width the size does not split by.
        assert_eq!(agreement(&a, &a, 7), Agreement::Differ);
    }

    /// The failure the check is for: the card reads a tiled buffer in the
    /// wrong layout, so the picture comes out shuffled, with the same bytes
    /// in other places. A few differing bytes in a mostly flat picture used
    /// to pass for a match.
    #[test]
    fn a_shuffled_picture_is_not_the_same() {
        let (w, h) = (64, 48);
        let a = detailed(w, h);
        // Rows read in the wrong order, as tiles read in the wrong layout.
        let mut shuffled = a.clone();
        for y in 0..h {
            let from = (y * 7 + 3) % h;
            shuffled[y * w..(y + 1) * w].copy_from_slice(&a[from * w..(from + 1) * w]);
        }
        assert_eq!(agreement(&shuffled, &a, w), Agreement::Differ);
        // A mostly flat picture with a small window: garbling the window is
        // a handful of bytes out of many, not a tolerated blink.
        let mut flat = vec![16u8; w * h * 3 / 2];
        let mut garbled = flat.clone();
        for y in 10..22 {
            for x in 10..30 {
                let on = (x + y) % 2 == 0;
                flat[y * w + x] = if on { 20 } else { 235 };
                garbled[y * w + x] = if on { 235 } else { 20 };
            }
        }
        assert_eq!(agreement(&garbled, &flat, w), Agreement::Differ);
    }

    #[test]
    fn an_empty_screen_says_nothing_until_a_picture_with_detail_comes() {
        let (w, h) = (64, 48);
        let flat = vec![16u8; w * h * 3 / 2];
        // A uniform screen the same both ways cannot show the layout is right.
        assert_eq!(agreement(&flat, &flat, w), Agreement::Unclear);
        // A gradient has no edges either.
        let gradient: Vec<u8> = (0..w * h * 3 / 2).map(|i| (i / 40) as u8).collect();
        assert_eq!(agreement(&gradient, &gradient, w), Agreement::Unclear);
        // But an empty screen the card made something else of is a failure.
        let black = vec![0u8; flat.len()];
        let white = vec![235u8; flat.len()];
        assert_eq!(agreement(&black, &white, w), Agreement::Differ);
    }
}
