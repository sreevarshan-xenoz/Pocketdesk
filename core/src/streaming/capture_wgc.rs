//! Windows.Graphics.Capture backend (Windows 10 1903+).
//!
//! Captures a monitor — or, when the phone asks for one window, that window —
//! through a free-threaded `Direct3D11CaptureFramePool`. Each arrived frame is
//! copied into one reused CPU-readable staging texture and repacked into the
//! tightly strided BGRA [`VideoFrame`] every encoder already takes.
//!
//! Frames arrive only when the screen changes. To keep a still desktop from
//! looking like a dead link on the phone (its decoder declares the link dead
//! after 10 s without a frame), the last frame is re-sent once a second while
//! nothing changes.
//!
//! Window crop: `window_rx` carries the window the client picked (an `HWND`
//! value on Windows, 0 = whole monitor). While frames of that window are being
//! emitted `window_mode` is set, which tells the encode task to skip its own
//! software crop — the same contract as the wlroots screencopy backend.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
    MonitorFromPoint,
};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::WindowsAndMessaging::IsWindow;
use windows::core::{BOOL, IInspectable, Interface};

use super::capture::{CaptureSession, monitor_rect};
use super::{StreamingConfig, VideoFrame};

/// How long [`start_wgc_capture`] waits for the first frame before reporting
/// the backend unusable (WGC delivers one immediately on a working desktop).
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(3);

/// While the screen is still, re-send the last frame this often.
const IDLE_HEARTBEAT: Duration = Duration::from_secs(1);

/// Frames the pool may hold; two lets one be copied while the next lands.
const POOL_BUFFERS: i32 = 2;

/// Start capturing the configured monitor through Windows.Graphics.Capture.
///
/// Returns once the first frame has been copied, so `start_capture_auto` can
/// fall back to DXGI duplication on any setup failure.
pub fn start_wgc_capture(
    config: StreamingConfig,
    frame_tx: mpsc::Sender<VideoFrame>,
    cancel: CancellationToken,
    window_rx: watch::Receiver<u64>,
    window_mode: Arc<AtomicBool>,
) -> Result<CaptureSession> {
    if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
        bail!("Windows.Graphics.Capture is not supported on this Windows build");
    }
    let (ready_tx, ready_rx) = std_mpsc::channel::<Result<()>>();
    let thread_config = config.clone();
    let thread_cancel = cancel.clone();
    std::thread::Builder::new()
        .name("wgc-capture".into())
        .spawn(move || {
            let result = run_capture_loop(
                thread_config,
                frame_tx,
                thread_cancel.clone(),
                &ready_tx,
                window_rx,
                window_mode,
            );
            if let Err(e) = result {
                // Before the first frame this is the setup failure the caller
                // falls back on; after it, the session simply ends.
                warn!("WGC capture ended: {e:#}");
                let _ = ready_tx.send(Err(e));
                thread_cancel.cancel();
            }
        })
        .context("failed to spawn the WGC capture thread")?;

    match ready_rx.recv_timeout(FIRST_FRAME_TIMEOUT) {
        Ok(Ok(())) => Ok(CaptureSession::new(config, cancel)),
        Ok(Err(e)) => {
            cancel.cancel();
            Err(e)
        }
        Err(_) => {
            cancel.cancel();
            bail!("WGC produced no frame within {FIRST_FRAME_TIMEOUT:?}")
        }
    }
}

/// What is being captured right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Monitor,
    Window(isize),
}

/// One live capture: item, pool and session, plus the event that wakes the
/// loop. Dropping it closes the session and the pool.
struct Capture {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    size: SizeInt32,
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

/// The D3D device pair the capture copies through.
struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt_device: IDirect3DDevice,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
}

fn run_capture_loop(
    config: StreamingConfig,
    frame_tx: mpsc::Sender<VideoFrame>,
    cancel: CancellationToken,
    ready_tx: &std_mpsc::Sender<Result<()>>,
    mut window_rx: watch::Receiver<u64>,
    window_mode: Arc<AtomicBool>,
) -> Result<()> {
    // WinRT needs an apartment on this thread; MTA because the pool is
    // free-threaded and its event fires on a worker thread.
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.context("RoInitialize")?;
    let mut gpu = create_gpu()?;
    let monitor = pick_monitor(config.monitor_index);
    let (wake_tx, wake_rx) = std_mpsc::sync_channel::<()>(1);

    let mut source = Source::Monitor;
    let mut capture = start_item(&gpu, &monitor_item(monitor)?, wake_tx.clone())?;
    // A window that failed to capture is not retried until the client picks
    // something else.
    let mut refused_window: Option<u64> = None;
    let frame_period = Duration::from_micros(1_000_000 / u64::from(config.fps.max(1)));
    let mut last_sent: Option<Instant> = None;
    let mut last_frame: Option<VideoFrame> = None;
    let mut ready_sent = false;

    while !cancel.is_cancelled() {
        // Follow the client's window choice.
        let wanted = *window_rx.borrow_and_update();
        let want_source = match wanted {
            0 => Source::Monitor,
            hwnd if refused_window == Some(hwnd) => Source::Monitor,
            hwnd => Source::Window(hwnd as isize),
        };
        if want_source != source {
            let switched = match want_source {
                Source::Monitor => monitor_item(monitor),
                Source::Window(hwnd) => window_item(hwnd),
            }
            .and_then(|item| start_item(&gpu, &item, wake_tx.clone()));
            match switched {
                Ok(next) => {
                    capture = next;
                    source = want_source;
                    window_mode.store(matches!(source, Source::Window(_)), Ordering::Relaxed);
                    last_frame = None;
                    info!(?source, "WGC capture source switched");
                }
                Err(e) => {
                    warn!(
                        window = wanted,
                        "WGC cannot capture that window ({e:#}); staying on the monitor"
                    );
                    refused_window = Some(wanted);
                }
            }
        }
        if wanted == 0 {
            refused_window = None;
        }
        // A captured window that closed: back to the monitor.
        if let Source::Window(hwnd) = source
            && !unsafe { IsWindow(Some(HWND(hwnd as *mut _))) }.as_bool()
        {
            info!("WGC window closed; returning to the monitor");
            capture = start_item(&gpu, &monitor_item(monitor)?, wake_tx.clone())?;
            source = Source::Monitor;
            window_mode.store(false, Ordering::Relaxed);
            refused_window = Some(wanted);
            last_frame = None;
        }

        let woke = wake_rx.recv_timeout(Duration::from_millis(100)).is_ok();
        let mut fresh = None;
        if woke {
            // Drain to the newest frame: older ones are already stale.
            while let Ok(frame) = capture.pool.TryGetNextFrame() {
                fresh = Some(frame);
            }
        }

        let frame = match fresh {
            Some(frame) => {
                if last_sent.is_some_and(|t| t.elapsed() < frame_period) {
                    continue; // faster than the configured rate
                }
                let content = frame.ContentSize()?;
                if content.Width != capture.size.Width || content.Height != capture.size.Height {
                    debug!(?content, "WGC content size changed; recreating the pool");
                    capture.pool.Recreate(
                        &gpu.winrt_device,
                        DirectXPixelFormat::B8G8R8A8UIntNormalized,
                        POOL_BUFFERS,
                        content,
                    )?;
                    capture.size = content;
                    continue; // the next frame arrives at the new size
                }
                let video = copy_frame(&mut gpu, &frame)?;
                last_frame = Some(clone_frame(&video));
                video
            }
            None => match &last_frame {
                Some(frame) if last_sent.is_some_and(|t| t.elapsed() >= IDLE_HEARTBEAT) => {
                    clone_frame(frame)
                }
                _ => continue,
            },
        };

        last_sent = Some(Instant::now());
        match frame_tx.try_send(frame) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
            Err(mpsc::error::TrySendError::Closed(_)) => {
                debug!("Frame channel closed, stopping WGC capture");
                break;
            }
        }
        if !ready_sent {
            let _ = ready_tx.send(Ok(()));
            ready_sent = true;
            info!(
                width = capture.size.Width,
                height = capture.size.Height,
                "WGC capture started"
            );
        }
    }
    window_mode.store(false, Ordering::Relaxed);
    Ok(())
}

fn clone_frame(frame: &VideoFrame) -> VideoFrame {
    VideoFrame {
        data: frame.data.clone(),
        width: frame.width,
        height: frame.height,
        stride: frame.stride,
        timestamp: Instant::now(),
    }
}

fn create_gpu() -> Result<Gpu> {
    let mut device = None;
    let mut context = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .context("D3D11CreateDevice")?;
    let device: ID3D11Device = device.ok_or_else(|| anyhow!("no D3D11 device"))?;
    let context = context.ok_or_else(|| anyhow!("no D3D11 context"))?;
    let dxgi: IDXGIDevice = device.cast()?;
    let inspectable: IInspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
        .context("CreateDirect3D11DeviceFromDXGIDevice")?;
    Ok(Gpu {
        device,
        context,
        winrt_device: inspectable.cast()?,
        staging: None,
    })
}

fn monitor_item(monitor: HMONITOR) -> Result<GraphicsCaptureItem> {
    let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    unsafe { interop.CreateForMonitor(monitor) }.context("CreateForMonitor")
}

fn window_item(hwnd: isize) -> Result<GraphicsCaptureItem> {
    let hwnd = HWND(hwnd as *mut _);
    if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        bail!("not a window");
    }
    let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    unsafe { interop.CreateForWindow(hwnd) }.context("CreateForWindow")
}

fn start_item(
    gpu: &Gpu,
    item: &GraphicsCaptureItem,
    wake: std_mpsc::SyncSender<()>,
) -> Result<Capture> {
    let size = item.Size()?;
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &gpu.winrt_device,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        POOL_BUFFERS,
        size,
    )?;
    pool.FrameArrived(
        &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |_, _| {
            // Full means a wake-up is already pending; that is enough.
            let _ = wake.try_send(());
            Ok(())
        }),
    )?;
    let session = pool.CreateCaptureSession(item)?;
    // The yellow "being captured" border would be in every frame the phone
    // sees; both setters are newer than WGC itself, so failures are ignored.
    let _ = session.SetIsBorderRequired(false);
    let _ = session.SetIsCursorCaptureEnabled(true);
    session.StartCapture()?;
    Ok(Capture {
        pool,
        session,
        size,
    })
}

/// Copy one captured frame through the staging texture into a tightly strided
/// BGRA [`VideoFrame`].
fn copy_frame(gpu: &mut Gpu, frame: &Direct3D11CaptureFrame) -> Result<VideoFrame> {
    let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
    let texture: ID3D11Texture2D = unsafe { access.GetInterface() }?;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { texture.GetDesc(&mut desc) };
    let (width, height) = (desc.Width, desc.Height);

    let staging = match &gpu.staging {
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
                gpu.device
                    .CreateTexture2D(&staging_desc, None, Some(&mut tex))
            }
            .context("CreateTexture2D (staging)")?;
            let tex = tex.ok_or_else(|| anyhow!("no staging texture"))?;
            gpu.staging = Some((tex.clone(), width, height));
            tex
        }
    };

    unsafe { gpu.context.CopyResource(&staging, &texture) };
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
        gpu.context
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
    unsafe { gpu.context.Unmap(&staging, 0) };

    Ok(VideoFrame {
        data,
        width,
        height,
        stride: width * 4,
        timestamp: Instant::now(),
    })
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
fn pick_monitor(index: u32) -> HMONITOR {
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
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let primary = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
    let _ = unsafe { GetMonitorInfoW(primary, &mut info) };
    warn!(
        index,
        "Monitor index not found; capturing the primary monitor"
    );
    primary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_is_at_least_one_monitor_to_capture() {
        // Every interactive Windows session has a desktop; CI's headless
        // runners still enumerate a virtual display.
        assert!(!enumerate_monitors().is_empty());
    }

    /// Live: capture one real frame of the primary monitor and check it is
    /// the monitor's size and not uniformly black. Run on a desktop session:
    /// `cargo test -p linux-link-core --lib wgc_captures -- --ignored`.
    #[test]
    #[ignore = "needs an interactive Windows desktop"]
    fn wgc_captures_a_real_frame_of_the_primary_monitor() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (tx, mut rx) = mpsc::channel(4);
            let cancel = CancellationToken::new();
            let (_window_tx, window_rx) = watch::channel(0u64);
            let window_mode = Arc::new(AtomicBool::new(false));
            let config = StreamingConfig::default();
            let _session = start_wgc_capture(config, tx, cancel.clone(), window_rx, window_mode)
                .expect("WGC should start on a desktop session");
            let frame = tokio::time::timeout(Duration::from_secs(3), rx.recv())
                .await
                .expect("a frame within 3 s")
                .expect("channel open");
            let (_, rect) = enumerate_monitors()[0];
            assert_eq!(frame.width, (rect.right - rect.left) as u32);
            assert_eq!(frame.height, (rect.bottom - rect.top) as u32);
            assert_eq!(frame.data.len(), (frame.stride * frame.height) as usize);
            assert!(
                frame
                    .data
                    .chunks(4)
                    .any(|px| px[0] != 0 || px[1] != 0 || px[2] != 0),
                "the captured desktop is uniformly black"
            );
            cancel.cancel();
        });
    }
}
