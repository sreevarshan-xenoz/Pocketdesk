//! Frames kept on the GPU from capture to NVENC (BG10).
//!
//! With `inproc-windows`, a capture on an NVIDIA adapter hands the encoder a
//! texture instead of reading every frame back to the CPU (the staging map
//! and row copy, about 6 ms a frame at 2560x1080). The capture copies each
//! frame into one of a few textures of its own ([`GpuRing`]), since the
//! source's surface goes back to Windows; the in-process NVENC copies the
//! frame's region into its own input texture and encodes it there. A window
//! crop only narrows the region. Anything else that needs the pixels reads
//! them back with [`GpuFrame::download`].

use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use tracing::info;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, ID3D11Device,
    ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::core::Interface;

use super::VideoFrame;
use super::frame_pool;

/// NVIDIA's PCI vendor id: NVENC runs only on its adapters.
const NVIDIA: u32 = 0x10DE;

/// Textures a capture keeps for its frames: one being filled, one waiting,
/// one being encoded, the heartbeat's.
const RING: usize = 4;

/// The capture's device, shared by its frames.
pub(crate) struct GpuSource {
    pub(crate) device: ID3D11Device,
    pub(crate) context: ID3D11DeviceContext,
    /// For [`GpuFrame::download`]: (texture, width, height).
    staging: Mutex<Option<(ID3D11Texture2D, u32, u32)>>,
}

// SAFETY: a D3D11 device is free-threaded, and its immediate context is
// made safe to share with `SetMultithreadProtected` before any frame leaves
// the capture thread (`GpuRing::new`); the staging slot is behind a mutex.
unsafe impl Send for GpuSource {}
unsafe impl Sync for GpuSource {}

/// A frame still on the GPU: `width`x`height` (the [`VideoFrame`]'s) at
/// `x, y` in `texture`.
#[derive(Clone)]
pub struct GpuFrame {
    texture: Arc<ID3D11Texture2D>,
    x: u32,
    y: u32,
    source: Arc<GpuSource>,
}

// SAFETY: as `GpuSource`; the texture is only read once handed out, and the
// ring writes it again only when no frame holds it (`Arc::strong_count`).
unsafe impl Send for GpuFrame {}

impl std::fmt::Debug for GpuFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuFrame")
            .field("x", &self.x)
            .field("y", &self.y)
            .finish_non_exhaustive()
    }
}

impl GpuFrame {
    /// Move the region's origin by `x, y`, for a crop.
    pub(crate) fn offset(&mut self, x: u32, y: u32) {
        self.x += x;
        self.y += y;
    }

    pub(crate) fn device(&self) -> &ID3D11Device {
        &self.source.device
    }

    pub(crate) fn context(&self) -> &ID3D11DeviceContext {
        &self.source.context
    }

    pub(crate) fn texture(&self) -> &ID3D11Texture2D {
        &self.texture
    }

    /// The region `width`x`height` as a box in the texture.
    pub(crate) fn region(&self, width: u32, height: u32) -> D3D11_BOX {
        D3D11_BOX {
            left: self.x,
            top: self.y,
            front: 0,
            right: self.x + width,
            bottom: self.y + height,
            back: 1,
        }
    }

    /// The region's pixels, tightly strided BGRA, for anything that cannot
    /// take a texture.
    pub(crate) fn download(&self, width: u32, height: u32) -> Result<Vec<u8>> {
        let mut slot = self
            .source
            .staging
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let staging = match &*slot {
            Some((texture, w, h)) if (*w, *h) == (width, height) => texture.clone(),
            _ => {
                let texture = new_texture(
                    &self.source.device,
                    &self.texture,
                    width,
                    height,
                    D3D11_USAGE_STAGING,
                    D3D11_CPU_ACCESS_READ.0 as u32,
                )?;
                *slot = Some((texture.clone(), width, height));
                texture
            }
        };
        let context = &self.source.context;
        let region = self.region(width, height);
        unsafe {
            context.CopySubresourceRegion(&staging, 0, 0, 0, 0, &*self.texture, 0, Some(&region))
        };

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe { context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }
            .context("Map (download)")?;
        let row = width as usize * 4;
        let mut data = frame_pool::take(row * height as usize);
        for y in 0..height as usize {
            // SAFETY: the mapped staging texture holds `height` rows of
            // `RowPitch` bytes, each at least `row` long.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                    data.as_mut_ptr().add(y * row),
                    row,
                );
            }
        }
        unsafe { context.Unmap(&staging, 0) };
        Ok(data)
    }
}

/// A texture like `like` (its format) of `width`x`height`.
fn new_texture(
    device: &ID3D11Device,
    like: &ID3D11Texture2D,
    width: u32,
    height: u32,
    usage: windows::Win32::Graphics::Direct3D11::D3D11_USAGE,
    cpu_access: u32,
) -> Result<ID3D11Texture2D> {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { like.GetDesc(&mut desc) };
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Usage: usage,
        BindFlags: 0,
        CPUAccessFlags: cpu_access,
        MiscFlags: 0,
        ..desc
    };
    let mut texture = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture)) }
        .context("CreateTexture2D (frame)")?;
    texture.ok_or_else(|| anyhow!("no texture"))
}

/// Whether frames stay on the GPU for this device: an NVIDIA adapter, and
/// not turned off with `POCKETDESK_ZERO_COPY=0`.
fn wanted(device: &ID3D11Device) -> bool {
    if std::env::var("POCKETDESK_ZERO_COPY").is_ok_and(|v| v == "0") {
        return false;
    }
    let vendor = device
        .cast::<IDXGIDevice>()
        .and_then(|dxgi| unsafe { dxgi.GetAdapter() })
        .and_then(|adapter| unsafe { adapter.GetDesc() })
        .map(|desc| desc.VendorId);
    matches!(vendor, Ok(NVIDIA))
}

/// A capture's own textures, handed out as [`GpuFrame`]s.
pub(crate) struct GpuRing {
    source: Arc<GpuSource>,
    textures: Vec<Arc<ID3D11Texture2D>>,
    size: (u32, u32),
    /// The last frame, for the idle heartbeat.
    last: Option<GpuFrame>,
}

impl GpuRing {
    /// A ring on this capture's device, or `None` when frames are better read
    /// back as before (not an NVIDIA adapter, or turned off).
    pub(crate) fn new(device: &ID3D11Device, context: &ID3D11DeviceContext) -> Option<Self> {
        if !wanted(device) {
            return None;
        }
        Self::on(device, context)
    }

    /// A ring on any device, whatever its vendor.
    fn on(device: &ID3D11Device, context: &ID3D11DeviceContext) -> Option<Self> {
        // The encoder uses this device's immediate context from its own
        // thread, so the context must serialize its callers.
        let multithread: ID3D11Multithread = context.cast().ok()?;
        // It returns whether it was on before, which does not matter here.
        let _was_on = unsafe { multithread.SetMultithreadProtected(true) };
        info!("Captured frames stay on the GPU for the encoder (BG10)");
        Some(Self {
            source: Arc::new(GpuSource {
                device: device.clone(),
                context: context.clone(),
                staging: Mutex::new(None),
            }),
            textures: Vec::new(),
            size: (0, 0),
            last: None,
        })
    }

    /// Copy `texture` into a free texture of the ring, as a frame.
    pub(crate) fn copy(&mut self, texture: &ID3D11Texture2D) -> Result<VideoFrame> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&mut desc) };
        let (width, height) = (desc.Width, desc.Height);
        if self.size != (width, height) {
            self.textures.clear();
            self.size = (width, height);
        }
        // A texture no frame holds any more (only the ring's reference).
        let free = self
            .textures
            .iter()
            .find(|t| Arc::strong_count(t) == 1)
            .cloned();
        let target = match free {
            Some(target) => target,
            None => {
                let target = Arc::new(new_texture(
                    &self.source.device,
                    texture,
                    width,
                    height,
                    D3D11_USAGE_DEFAULT,
                    0,
                )?);
                // Past the ring's size a busy moment gets a texture of its own.
                if self.textures.len() < RING {
                    self.textures.push(target.clone());
                }
                target
            }
        };
        unsafe { self.source.context.CopyResource(&*target, texture) };
        let gpu = GpuFrame {
            texture: target,
            x: 0,
            y: 0,
            source: self.source.clone(),
        };
        self.last = Some(gpu.clone());
        Ok(frame(gpu, width, height))
    }

    /// The last frame again, with a fresh timestamp, for the idle heartbeat.
    pub(crate) fn copy_again(&self) -> Option<VideoFrame> {
        let gpu = self.last.clone()?;
        let (width, height) = self.size;
        Some(frame(gpu, width, height))
    }
}

fn frame(gpu: GpuFrame, width: u32, height: u32) -> VideoFrame {
    VideoFrame {
        data: Vec::new(),
        width,
        height,
        stride: width * 4,
        timestamp: Instant::now(),
        gpu: Some(gpu),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_DRIVER_TYPE_WARP};
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11_SUBRESOURCE_DATA,
        D3D11CreateDevice,
    };
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1};

    /// A software (WARP) device, or with `nvidia` the first NVIDIA adapter's
    /// (`None` when there is none).
    pub(crate) fn device(nvidia: bool) -> Option<(ID3D11Device, ID3D11DeviceContext)> {
        let adapter: Option<IDXGIAdapter> = if nvidia {
            let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;
            let mut found = None;
            let mut index = 0;
            while let Ok(adapter) = unsafe { factory.EnumAdapters1(index) } {
                index += 1;
                if unsafe { adapter.GetDesc1() }.is_ok_and(|d| d.VendorId == NVIDIA) {
                    found = adapter.cast().ok();
                    break;
                }
            }
            Some(found?)
        } else {
            None
        };
        let driver = if nvidia {
            D3D_DRIVER_TYPE_UNKNOWN
        } else {
            D3D_DRIVER_TYPE_WARP
        };
        let (mut device, mut context) = (None, None);
        unsafe {
            D3D11CreateDevice(
                adapter.as_ref(),
                driver,
                Default::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .ok()?;
        Some((device?, context?))
    }

    /// A `width`x`height` BGRA texture holding `pixels`.
    pub(crate) fn texture(
        device: &ID3D11Device,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> ID3D11Texture2D {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: 0,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let data = D3D11_SUBRESOURCE_DATA {
            pSysMem: pixels.as_ptr().cast(),
            SysMemPitch: width * 4,
            SysMemSlicePitch: 0,
        };
        let mut texture = None;
        unsafe { device.CreateTexture2D(&desc, Some(&data), Some(&mut texture)) }.unwrap();
        texture.unwrap()
    }

    /// A ring on any device, for tests off an NVIDIA adapter.
    pub(crate) fn ring(device: &ID3D11Device, context: &ID3D11DeviceContext) -> GpuRing {
        GpuRing::on(device, context).expect("a ring")
    }

    /// Each pixel holds its own (x, y) in B and G.
    fn pattern(width: u32, height: u32) -> Vec<u8> {
        (0..height)
            .flat_map(|y| (0..width).flat_map(move |x| [x as u8, y as u8, 7, 255]))
            .collect()
    }

    #[test]
    fn a_cropped_frame_on_the_gpu_reads_back_only_its_region() {
        let (device, context) = device(false).expect("a WARP device");
        let mut ring = ring(&device, &context);
        let source = texture(&device, 8, 6, &pattern(8, 6));

        let mut frame = ring.copy(&source).unwrap();
        assert!(frame.data.is_empty(), "the pixels stay on the GPU");
        assert!(frame.crop_region(2, 1, 4, 4));
        assert_eq!((frame.width, frame.height), (4, 4));

        let cpu = frame.downloaded().unwrap().expect("read back");
        assert_eq!(cpu.data.len(), 4 * 4 * 4);
        for y in 0..4 {
            for x in 0..4 {
                let at = (y * 4 + x) * 4;
                assert_eq!(
                    (cpu.data[at], cpu.data[at + 1]),
                    (x as u8 + 2, y as u8 + 1),
                    "at {x},{y}"
                );
            }
        }
    }

    #[test]
    fn the_ring_reuses_a_texture_no_frame_holds() {
        let (device, context) = device(false).expect("a WARP device");
        let mut ring = ring(&device, &context);
        let source = texture(&device, 4, 2, &[1; 32]);

        let first = ring.copy(&source).unwrap();
        let held = first.gpu.as_ref().unwrap().texture().clone();
        drop(first);
        // The heartbeat keeps the last frame, so the next copy takes another.
        let second = ring.copy(&source).unwrap();
        assert_ne!(second.gpu.as_ref().unwrap().texture(), &held);
        drop(second);
        // Now the first is held by nothing: it is used again.
        let third = ring.copy(&source).unwrap();
        assert_eq!(third.gpu.as_ref().unwrap().texture(), &held);
        assert_eq!(ring.textures.len(), 2);

        let again = ring.copy_again().expect("the last frame");
        assert_eq!(again.gpu.as_ref().unwrap().texture(), &held);
    }
}
