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

/// Copies GPU textures to the CPU through one reused staging texture.
pub(super) struct StagingCopy {
    pub(super) device: ID3D11Device,
    pub(super) context: ID3D11DeviceContext,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
}

impl StagingCopy {
    pub(super) fn new(device: ID3D11Device, context: ID3D11DeviceContext) -> Self {
        Self {
            device,
            context,
            staging: None,
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
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .context("Map (staging)")?;
        let row = width as usize * 4;
        let mut data = vec![0u8; row * height as usize];
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
        unsafe { self.context.Unmap(&staging, 0) };

        Ok(VideoFrame {
            data,
            width,
            height,
            stride: width * 4,
            timestamp: Instant::now(),
        })
    }
}

/// A fresh-timestamped copy of `frame`, for the idle heartbeat.
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

pub(super) fn clone_frame(frame: &VideoFrame) -> VideoFrame {
    VideoFrame {
        data: frame.data.clone(),
        width: frame.width,
        height: frame.height,
        stride: frame.stride,
        timestamp: Instant::now(),
    }
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
}
