//! DXGI Desktop Duplication backend: the Windows fallback when
//! Windows.Graphics.Capture is unavailable (pre-1903 builds, some RDP and
//! virtual-display sessions).
//!
//! Limits, by API: monitors only (a window the client picks is cropped in
//! software by the encode task, so `window_mode` stays false), and no cursor
//! (duplication delivers the pointer separately and it is not composited
//! here). Losing access — the secure desktop of a UAC prompt or the lock
//! screen, a mode change — rebuilds the duplication instead of ending the
//! session.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
    IDXGIAdapter, IDXGIFactory1, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
};
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::core::Interface;

use super::capture::CaptureSession;
use super::capture_d3d::{StagingCopy, pick_monitor, wait_before_sending};
use super::{StreamingConfig, VideoFrame};

/// How long [`start_dxgi_capture`] waits for the first frame.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(3);

/// While the screen is still, re-send the last frame this often (see the
/// same constant in `capture_wgc.rs`).
const IDLE_HEARTBEAT: Duration = Duration::from_secs(1);

/// `AcquireNextFrame` wait, which is also the cancel-check cadence.
const ACQUIRE_TIMEOUT_MS: u32 = 100;

/// Start duplicating the configured monitor.
///
/// Returns once the first frame has been copied.
pub fn start_dxgi_capture(
    config: StreamingConfig,
    frame_tx: crate::streaming::latest_frame::FrameSender<VideoFrame>,
    cancel: CancellationToken,
    _window_rx: watch::Receiver<u64>,
    _window_mode: Arc<AtomicBool>,
) -> Result<CaptureSession> {
    let (ready_tx, ready_rx) = std_mpsc::channel::<Result<()>>();
    let thread_config = config.clone();
    let thread_cancel = cancel.clone();
    std::thread::Builder::new()
        .name("dxgi-capture".into())
        .spawn(move || {
            if let Err(e) =
                run_capture_loop(thread_config, frame_tx, thread_cancel.clone(), &ready_tx)
            {
                warn!("DXGI capture ended: {e:#}");
                let _ = ready_tx.send(Err(e));
                thread_cancel.cancel();
            }
        })
        .context("failed to spawn the DXGI capture thread")?;

    match ready_rx.recv_timeout(FIRST_FRAME_TIMEOUT) {
        Ok(Ok(())) => Ok(CaptureSession::new(config, cancel)),
        Ok(Err(e)) => {
            cancel.cancel();
            Err(e)
        }
        Err(_) => {
            cancel.cancel();
            bail!("DXGI duplication produced no frame within {FIRST_FRAME_TIMEOUT:?}")
        }
    }
}

/// The output for `monitor`, its adapter's device, and a live duplication.
struct Duplication {
    output: IDXGIOutput1,
    copier: StagingCopy,
    dup: IDXGIOutputDuplication,
}

fn open_duplication(monitor: HMONITOR) -> Result<Duplication> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.context("CreateDXGIFactory1")?;
    let mut adapter_index = 0;
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(adapter_index) } {
        adapter_index += 1;
        let mut output_index = 0;
        while let Ok(output) = unsafe { adapter.EnumOutputs(output_index) } {
            output_index += 1;
            let desc = unsafe { output.GetDesc() }?;
            if desc.Monitor != monitor {
                continue;
            }
            // Duplication must run on the device of the adapter that owns
            // the output.
            let adapter: IDXGIAdapter = adapter.cast()?;
            let mut device = None;
            let mut context = None;
            unsafe {
                D3D11CreateDevice(
                    &adapter,
                    D3D_DRIVER_TYPE_UNKNOWN,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                )
            }
            .context("D3D11CreateDevice on the output's adapter")?;
            let device: ID3D11Device = device.ok_or_else(|| anyhow!("no D3D11 device"))?;
            let context = context.ok_or_else(|| anyhow!("no D3D11 context"))?;
            let output: IDXGIOutput1 = output.cast()?;
            let dup = unsafe { output.DuplicateOutput(&device) }.context("DuplicateOutput")?;
            return Ok(Duplication {
                output,
                copier: StagingCopy::new(device, context),
                dup,
            });
        }
    }
    bail!("no DXGI output drives the selected monitor")
}

fn run_capture_loop(
    config: StreamingConfig,
    frame_tx: crate::streaming::latest_frame::FrameSender<VideoFrame>,
    cancel: CancellationToken,
    ready_tx: &std_mpsc::Sender<Result<()>>,
) -> Result<()> {
    let monitor = pick_monitor(config.monitor_index);
    let mut duplication = open_duplication(monitor)?;
    let frame_period = Duration::from_micros(1_000_000 / u64::from(config.fps.max(1)));
    let mut last_sent: Option<Instant> = None;
    let mut ready_sent = false;

    while !cancel.is_cancelled() {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        let acquired = unsafe {
            duplication
                .dup
                .AcquireNextFrame(ACQUIRE_TIMEOUT_MS, &mut info, &mut resource)
        };
        let frame = match acquired {
            Ok(()) => {
                // A pointer-only update has no new image. An image that comes
                // early is still copied, and sent once its period is up (AI4):
                // dropped, it could be the last change, and the heartbeat
                // would repeat the one before it until the next.
                let fresh = info.LastPresentTime != 0;
                let copied = match (fresh, resource) {
                    (true, Some(resource)) => {
                        let texture: ID3D11Texture2D = resource.cast()?;
                        Some(duplication.copier.copy(&texture))
                    }
                    _ => None,
                };
                unsafe { duplication.dup.ReleaseFrame() }.ok();
                match copied {
                    Some(video) => {
                        let mut video = video?;
                        if let Some(wait) =
                            wait_before_sending(last_sent, Instant::now(), frame_period)
                        {
                            std::thread::sleep(wait);
                            // What changed meanwhile goes instead (review):
                            // duplication holds it for the next acquire.
                            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
                            let mut resource: Option<IDXGIResource> = None;
                            let newer = unsafe {
                                duplication
                                    .dup
                                    .AcquireNextFrame(0, &mut info, &mut resource)
                            };
                            if newer.is_ok() {
                                if info.LastPresentTime != 0
                                    && let Some(resource) = resource
                                {
                                    let texture: ID3D11Texture2D = resource.cast()?;
                                    video = duplication.copier.copy(&texture)?;
                                }
                                unsafe { duplication.dup.ReleaseFrame() }.ok();
                            }
                            // A timeout keeps the copy in hand; a lost access
                            // shows at the next acquire, as before.
                        }
                        video
                    }
                    None => continue,
                }
            }
            // The heartbeat reads the last frame again from the copier's
            // staging texture (BG9); a rebuilt duplication has none until its
            // first frame, which is the whole desktop.
            Err(e)
                if e.code() == DXGI_ERROR_WAIT_TIMEOUT
                    && last_sent.is_some_and(|t| t.elapsed() >= IDLE_HEARTBEAT) =>
            {
                match duplication.copier.copy_again() {
                    Some(frame) => frame?,
                    None => continue,
                }
            }
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => continue,
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                // Secure desktop, mode change or fullscreen switch: the old
                // duplication is dead. Retry until the desktop comes back.
                debug!("DXGI access lost; rebuilding the duplication");
                match open_duplication(monitor) {
                    Ok(next) => duplication = next,
                    Err(e) => {
                        debug!("DXGI duplication not yet available ({e:#})");
                        std::thread::sleep(Duration::from_millis(250));
                    }
                }
                continue;
            }
            Err(e) => return Err(e).context("AcquireNextFrame"),
        };

        last_sent = Some(Instant::now());
        match frame_tx.try_send(frame) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
            Err(mpsc::error::TrySendError::Closed(_)) => {
                debug!("Frame channel closed, stopping DXGI capture");
                break;
            }
        }
        if !ready_sent {
            let _ = ready_tx.send(Ok(()));
            ready_sent = true;
            let desc = unsafe { duplication.output.GetDesc() }?;
            info!(
                width = desc.DesktopCoordinates.right - desc.DesktopCoordinates.left,
                height = desc.DesktopCoordinates.bottom - desc.DesktopCoordinates.top,
                "DXGI duplication capture started"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::capture_d3d::enumerate_monitors;

    /// Live: duplicate the primary monitor and check the first frame. Run on
    /// a desktop session:
    /// `cargo test -p pocketdesk-core --lib dxgi_captures -- --ignored`.
    #[test]
    #[ignore = "needs an interactive Windows desktop"]
    fn dxgi_captures_a_real_frame_of_the_primary_monitor() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (tx, mut rx) = crate::streaming::latest_frame::channel();
            let cancel = CancellationToken::new();
            let (_window_tx, window_rx) = watch::channel(0u64);
            let _session = start_dxgi_capture(
                StreamingConfig::default(),
                tx,
                cancel.clone(),
                window_rx,
                Arc::new(AtomicBool::new(false)),
            )
            .expect("DXGI duplication should start on a desktop session");
            let frame = tokio::time::timeout(Duration::from_secs(3), rx.recv())
                .await
                .expect("a frame within 3 s")
                .expect("channel open");
            // On an NVIDIA adapter with `inproc-windows` the frame is still on
            // the GPU (BG10): its pixels, read back.
            let frame = frame.downloaded().unwrap().unwrap_or(frame);
            let (_, rect) = enumerate_monitors()[0];
            assert_eq!(frame.width, (rect.right - rect.left) as u32);
            assert_eq!(frame.height, (rect.bottom - rect.top) as u32);
            assert!(
                frame
                    .data
                    .chunks(4)
                    .any(|px| px[0] != 0 || px[1] != 0 || px[2] != 0),
                "the duplicated desktop is uniformly black"
            );
            cancel.cancel();
        });
    }
}
