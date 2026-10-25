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
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
    ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::WindowsAndMessaging::IsWindow;
use windows::core::{IInspectable, Interface};

use super::capture::CaptureSession;
#[cfg(test)]
use super::capture_d3d::enumerate_monitors;
use super::capture_d3d::{StagingCopy, clone_frame, pick_monitor};
use super::{StreamingConfig, VideoFrame};

/// How long [`start_wgc_capture`] waits for the first frame before reporting
/// the backend unusable (WGC delivers one immediately on a working desktop).
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(3);

/// While the screen is still, re-send the last frame this often.
const IDLE_HEARTBEAT: Duration = Duration::from_secs(1);

/// Frames the pool may hold; two lets one be copied while the next lands.
const POOL_BUFFERS: i32 = 2;

/// How long a frame that arrived `now` waits before it is sent, so frames go
/// no faster than one a `period`; `None` sends it at once. A frame is never
/// dropped for coming early (AI3): it may be the last the source sends for a
/// long while, a still window's first or the end of a burst of changes, and
/// the heartbeat can only repeat a frame that was kept.
fn wait_before_sending(
    last_sent: Option<Instant>,
    now: Instant,
    period: Duration,
) -> Option<Duration> {
    let since = now.saturating_duration_since(last_sent?);
    (since < period).then(|| period - since)
}

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
    pin_capture_library();
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

/// Keep `GraphicsCapture.dll` loaded for the rest of the process's life.
///
/// About one server stop in eight during a session died with an access
/// violation executing inside this DLL after it had been unloaded, most
/// likely a closed session finishing on the DLL's own threads after COM
/// let it go (no capture object left). A reconnect closing the old session
/// may meet the same race. A pinned module is never unloaded; the process
/// exits with it.
fn pin_capture_library() {
    static PINNED: std::sync::Once = std::sync::Once::new();
    PINNED.call_once(|| {
        use windows::Win32::System::LibraryLoader::{
            GET_MODULE_HANDLE_EX_FLAG_PIN, GetModuleHandleExW,
        };
        use windows::core::w;
        // `IsSupported` has loaded it. Only a module already loaded is
        // pinned, so nothing is searched for by name.
        let mut module = HMODULE::default();
        // SAFETY: a constant, NUL-terminated module name and a handle to
        // fill; a pinned handle is never freed.
        let pinned = unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_PIN,
                w!("GraphicsCapture.dll"),
                &mut module,
            )
        };
        match pinned {
            Ok(()) => debug!("GraphicsCapture.dll pinned"),
            Err(e) => warn!("Could not pin GraphicsCapture.dll: {e}"),
        }
    });
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

/// The D3D device the capture copies through, and its WinRT face.
struct Gpu {
    copier: StagingCopy,
    winrt_device: IDirect3DDevice,
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
                // Faster than the configured rate: this frame waits its turn
                // rather than being dropped (AI3).
                if let Some(wait) = wait_before_sending(last_sent, Instant::now(), frame_period) {
                    std::thread::sleep(wait);
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
                let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
                let texture: ID3D11Texture2D = unsafe { access.GetInterface() }?;
                let video = gpu.copier.copy(&texture)?;
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
        copier: StagingCopy::new(device, context),
        winrt_device: inspectable.cast()?,
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

    /// Live: capture one real frame of the primary monitor and check it is
    /// the monitor's size and not uniformly black. Run on a desktop session:
    /// `cargo test -p pocketdesk-core --lib wgc_captures -- --ignored`.
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
