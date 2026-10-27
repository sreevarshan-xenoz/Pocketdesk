//! Direct3D 11 plumbing shared by the Windows capture backends
//! (`capture_wgc.rs`, `capture_dxgi.rs`): turning a GPU texture into a BGRA
//! [`VideoFrame`], and resolving the phone's monitor index to an `HMONITOR`.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use tracing::warn;
use windows::Win32::Foundation::{LPARAM, POINT, RECT};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_STAGING, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, HDC, HMONITOR, MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint,
};
use windows::core::BOOL;

use super::VideoFrame;
use super::capture::monitor_rect;
use super::frame_pool;

/// Copies GPU textures to the CPU through one reused staging texture.
pub(super) struct StagingCopy {
    pub(super) device: ID3D11Device,
    pub(super) context: ID3D11DeviceContext,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    /// Frames' buffers come back for the next copy while this runs (BG9).
    _pool: frame_pool::Lease,
}

impl StagingCopy {
    pub(super) fn new(device: ID3D11Device, context: ID3D11DeviceContext) -> Self {
        Self {
            device,
            context,
            staging: None,
            _pool: frame_pool::Lease::new(),
        }
    }

    /// Copy a BGRA texture into a tightly strided [`VideoFrame`].
    pub(super) fn copy(&mut self, texture: &ID3D11Texture2D) -> Result<VideoFrame> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&mut desc) };
        let (width, height) = (desc.Width, desc.Height);

        let staging = match &self.staging {
            Some((tex, w, h)) if *w == width && *h == height => tex.clone(),
            _ => {
                let staging_desc = D3D11_TEXTURE2D_DESC {
                    Width: width,
                    Height: height,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: desc.Format,
                    SampleDesc: desc.SampleDesc,
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags: 0,
                };
                let mut tex = None;
                unsafe {
                    self.device
                        .CreateTexture2D(&staging_desc, None, Some(&mut tex))
                }
                .context("CreateTexture2D (staging)")?;
                let tex = tex.ok_or_else(|| anyhow!("no staging texture"))?;
                self.staging = Some((tex.clone(), width, height));
                tex
            }
        };

        unsafe { self.context.CopyResource(&staging, texture) };
        Self::read(&self.context, &staging, width, height)
    }

    /// The last frame [`copy`](Self::copy) made, read again from the staging
    /// texture with a fresh timestamp, for the idle heartbeat (BG9): keeping
    /// a clone of every frame for it cost a full copy of each (1.7 ms at
    /// 2560x1080), where this costs one a second. `None` before any copy.
    pub(super) fn copy_again(&mut self) -> Option<Result<VideoFrame>> {
        let (staging, width, height) = self.staging.as_ref()?;
        Some(Self::read(&self.context, staging, *width, *height))
    }

    /// Map `staging` and copy its rows into a tightly strided frame, its
    /// buffer a dropped frame's when there is one (BG9).
    fn read(
        context: &ID3D11DeviceContext,
        staging: &ID3D11Texture2D,
        width: u32,
        height: u32,
    ) -> Result<VideoFrame> {
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe { context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }
            .context("Map (staging)")?;
        let row = width as usize * 4;
        let mut data = frame_pool::take(row * height as usize);
        let src = mapped.pData as *const u8;
        for y in 0..height as usize {
            // SAFETY: the mapped staging texture holds `height` rows of
            // `RowPitch` bytes, each at least `row` long.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    src.add(y * mapped.RowPitch as usize),
                    data.as_mut_ptr().add(y * row),
                    row,
                );
            }
        }
        unsafe { context.Unmap(staging, 0) };

        Ok(VideoFrame {
            data,
            width,
            height,
            stride: width * 4,
            timestamp: Instant::now(),
        })
    }
}

/// How long a frame that arrived `now` waits before it is sent, so frames go
/// no faster than one a `period`; `None` sends it at once. A frame is never
/// dropped for coming early (AI3, AI4): it may be the last the source sends for a
/// long while, a still window's first or the end of a burst of changes, and
/// the heartbeat can only repeat a frame that was kept.
pub(super) fn wait_before_sending(
    last_sent: Option<Instant>,
    now: Instant,
    period: Duration,
) -> Option<Duration> {
    let since = now.saturating_duration_since(last_sent?);
    (since < period).then(|| period - since)
}

/// All monitors in `EnumDisplayMonitors` order, with their desktop rects.
pub(super) fn enumerate_monitors() -> Vec<(HMONITOR, RECT)> {
    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _: HDC,
        rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        // SAFETY: `data` is the Vec passed below, alive for the whole call.
        let list = unsafe { &mut *(data.0 as *mut Vec<(HMONITOR, RECT)>) };
        list.push((monitor, unsafe { *rect }));
        BOOL(1)
    }
    let mut list: Vec<(HMONITOR, RECT)> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(&mut list as *mut _ as isize),
        );
    }
    list
}

/// The monitor for `index` in the order the monitors plugin reports (xcap's).
///
/// Matched by desktop rect rather than by position in our own enumeration, so
/// the phone's picker and the captured monitor agree even if the two
/// enumerations ever order differently; the primary monitor is the fallback.
pub(super) fn pick_monitor(index: u32) -> HMONITOR {
    let monitors = enumerate_monitors();
    if let Some((x, y, w, h)) = monitor_rect(index)
        && let Some((monitor, _)) = monitors.iter().find(|(_, r)| {
            r.left == x
                && r.top == y
                && (r.right - r.left) as u32 == w
                && (r.bottom - r.top) as u32 == h
        })
    {
        return *monitor;
    }
    if let Some((monitor, _)) = monitors.get(index as usize) {
        return *monitor;
    }
    warn!(
        index,
        "Monitor index not found; capturing the primary monitor"
    );
    unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_that_comes_early_waits_its_turn_instead_of_being_dropped() {
        // AI3: a window's first frame after a switch came within one frame
        // period of the monitor's last; dropped, a still window sent no
        // other, and the phone called the link dead 10 s later.
        let period = Duration::from_millis(33);
        let now = Instant::now();
        assert_eq!(
            wait_before_sending(None, now, period),
            None,
            "the first frame goes at once"
        );
        assert_eq!(
            wait_before_sending(Some(now - period), now, period),
            None,
            "on time"
        );
        assert_eq!(
            wait_before_sending(Some(now - Duration::from_millis(3)), now, period),
            Some(Duration::from_millis(30)),
            "early: held for the rest of the period, then sent"
        );
    }

    #[test]
    fn there_is_at_least_one_monitor_to_capture() {
        // Every interactive Windows session has a desktop; CI's headless
        // runners still enumerate a virtual display.
        assert!(!enumerate_monitors().is_empty());
    }

    #[test]
    fn an_out_of_range_index_still_picks_a_monitor() {
        assert!(!pick_monitor(u32::MAX).is_invalid());
    }

    /// A 4x2 BGRA texture on a software (WARP) device, filled with `fill`.
    fn warp_texture(fill: u8) -> (ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D) {
        use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_WARP;
        use windows::Win32::Graphics::Direct3D11::{
            D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11_SUBRESOURCE_DATA,
            D3D11_USAGE_DEFAULT, D3D11CreateDevice,
        };
        use windows::Win32::Graphics::Dxgi::Common::{
            DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
        };
        let (mut device, mut context) = (None, None);
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_WARP,
                Default::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .expect("a WARP device");
        let (device, context) = (device.unwrap(), context.unwrap());
        let pixels = [fill; 4 * 2 * 4];
        let desc = D3D11_TEXTURE2D_DESC {
            Width: 4,
            Height: 2,
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
        let initial = D3D11_SUBRESOURCE_DATA {
            pSysMem: pixels.as_ptr().cast(),
            SysMemPitch: 4 * 4,
            SysMemSlicePitch: 0,
        };
        let mut texture = None;
        unsafe { device.CreateTexture2D(&desc, Some(&initial), Some(&mut texture)) }
            .expect("a texture");
        (device, context, texture.unwrap())
    }

    #[test]
    fn the_heartbeat_reads_the_last_copy_again_instead_of_keeping_a_clone() {
        // BG9: each frame was cloned (11 MB at 2560x1080, 1.7 ms) only so the
        // once-a-second heartbeat could repeat it; the staging texture
        // already holds it.
        let (device, context, texture) = warp_texture(7);
        let mut copier = StagingCopy::new(device, context.clone());
        assert!(copier.copy_again().is_none(), "nothing copied yet");
        let first = copier.copy(&texture).unwrap();
        assert_eq!((first.width, first.height, first.stride), (4, 2, 16));
        assert!(first.data.iter().all(|&b| b == 7));

        // The source moves on without a new copy: the heartbeat repeats what
        // was sent, not the source's new content.
        let newer = [9u8; 4 * 2 * 4];
        unsafe { context.UpdateSubresource(&texture, 0, None, newer.as_ptr().cast(), 16, 0) };
        let again = copier.copy_again().expect("the last copy").unwrap();
        assert_eq!(again.data, first.data);
        assert_eq!((again.width, again.height, again.stride), (4, 2, 16));
        assert!(again.timestamp >= first.timestamp, "a fresh timestamp");

        let next = copier.copy(&texture).unwrap();
        assert!(next.data.iter().all(|&b| b == 9));
        assert_eq!(copier.copy_again().unwrap().unwrap().data, next.data);
    }

    #[test]
    fn a_reused_buffer_holds_only_the_new_frame() {
        // BG9: a copy takes a dropped frame's buffer instead of a new one, so
        // every byte of the old frame must be written over.
        let (device, context, texture) = warp_texture(3);
        let mut copier = StagingCopy::new(device, context.clone());
        drop(copier.copy(&texture).unwrap());

        let newer = [5u8; 4 * 2 * 4];
        unsafe { context.UpdateSubresource(&texture, 0, None, newer.as_ptr().cast(), 16, 0) };
        let next = copier.copy(&texture).unwrap();
        assert_eq!(next.data.len(), 4 * 2 * 4);
        assert!(next.data.iter().all(|&b| b == 5));
    }
}
