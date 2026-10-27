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
use super::capture_d3d::{StagingCopy, pick_monitor, wait_before_sending};
use super::{StreamingConfig, VideoFrame};

/// How long [`start_wgc_capture`] waits for the first frame before reporting
/// the backend unusable (WGC delivers one immediately on a working desktop).
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(3);

/// While the screen is still, re-send the last frame this often.
const IDLE_HEARTBEAT: Duration = Duration::from_secs(1);

/// Frames the pool may hold; two lets one be copied while the next lands.
const POOL_BUFFERS: i32 = 2;

/// How long a pool recreated for a new size waits for a frame at that size
/// before the capture restarts to get one (see [`restart_due`]).
const RESIZE_SETTLE: Duration = Duration::from_millis(250);

/// Start capturing the configured monitor through Windows.Graphics.Capture.
///
/// Returns once the first frame has been copied, so `start_capture_auto` can
/// fall back to DXGI duplication on any setup failure.
pub fn start_wgc_capture(
    config: StreamingConfig,
    frame_tx: crate::streaming::latest_frame::FrameSender<VideoFrame>,
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
    frame_tx: crate::streaming::latest_frame::FrameSender<VideoFrame>,
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
    // Whether the copier's staging texture holds this source's last frame,
    // which the heartbeat repeats (BG9).
    let mut have_last = false;
    // When the pool was last recreated for a new size with no frame at that
    // size since.
    let mut resized_at: Option<Instant> = None;
    // Restarted for a new size with no frame used since: once is enough (a
    // source whose first frame is not its item's size would restart forever).
    let mut restarted = false;
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
            let switched = source_item(want_source, monitor)
                .and_then(|item| start_item(&gpu, &item, wake_tx.clone()));
            match switched {
                Ok(next) => {
                    capture = next;
                    source = want_source;
                    window_mode.store(matches!(source, Source::Window(_)), Ordering::Relaxed);
                    have_last = false;
                    resized_at = None;
                    restarted = false;
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
            have_last = false;
            resized_at = None;
            restarted = false;
        }

        let woke = wake_rx.recv_timeout(Duration::from_millis(100)).is_ok();
        let mut fresh = None;
        if woke {
            // Drain to the newest frame: older ones are already stale.
            while let Ok(frame) = capture.pool.TryGetNextFrame() {
                fresh = Some(frame);
            }
        }
        // The frame whose size changed went to the pool's recreation, and a
        // still source sends no other (AL1): a new capture of it sends its
        // first frame at once.
        if fresh.is_none() && restart_due(resized_at, Instant::now()) {
            resized_at = None;
            match source_item(source, monitor)
                .and_then(|item| start_item(&gpu, &item, wake_tx.clone()))
            {
                Ok(next) => {
                    capture = next;
                    restarted = true;
                    debug!(?source, "WGC capture restarted for its new size");
                }
                Err(e) => {
                    // Tried again after another wait (review).
                    resized_at = Some(Instant::now());
                    warn!("WGC capture could not restart for its new size ({e:#})");
                }
            }
            continue;
        }

        let frame = match fresh {
            Some(frame) => {
                if resize_pool(&gpu, &mut capture, &frame)? {
                    resized_at = (!restarted).then(Instant::now);
                    continue; // the next frame arrives at the new size
                }
                resized_at = None;
                restarted = false;
                let mut video = copy_frame(&mut gpu, &frame)?;
                drop(frame);
                // Faster than the configured rate: this frame waits its turn
                // rather than being dropped (AI3), copied first so the pool's
                // buffer is free meanwhile; a newer one by then goes instead.
                if let Some(wait) = wait_before_sending(last_sent, Instant::now(), frame_period) {
                    std::thread::sleep(wait);
                    let mut newer = None;
                    while let Ok(frame) = capture.pool.TryGetNextFrame() {
                        newer = Some(frame);
                    }
                    if let Some(frame) = newer {
                        if resize_pool(&gpu, &mut capture, &frame)? {
                            resized_at = (!restarted).then(Instant::now);
                        } else {
                            video = copy_frame(&mut gpu, &frame)?;
                        }
                    }
                }
                have_last = true;
                video
            }
            None if have_last && last_sent.is_some_and(|t| t.elapsed() >= IDLE_HEARTBEAT) => {
                match gpu.copier.copy_again() {
                    Some(frame) => frame?,
                    None => continue,
                }
            }
            None => continue,
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

/// Whether a pool recreated at `resized_at` has waited long enough for a
/// frame at its new size: a window resized while it is dragged sends frames
/// at each size, one left still sends none.
fn restart_due(resized_at: Option<Instant>, now: Instant) -> bool {
    resized_at.is_some_and(|at| now.saturating_duration_since(at) >= RESIZE_SETTLE)
}

fn source_item(source: Source, monitor: HMONITOR) -> Result<GraphicsCaptureItem> {
    match source {
        Source::Monitor => monitor_item(monitor),
        Source::Window(hwnd) => window_item(hwnd),
    }
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

/// Recreate the pool when `frame`'s content is another size than the pool
/// (a window resized): `true` when it did, and `frame` is not to be used.
fn resize_pool(gpu: &Gpu, capture: &mut Capture, frame: &Direct3D11CaptureFrame) -> Result<bool> {
    let content = frame.ContentSize()?;
    if content.Width == capture.size.Width && content.Height == capture.size.Height {
        return Ok(false);
    }
    debug!(?content, "WGC content size changed; recreating the pool");
    capture.pool.Recreate(
        &gpu.winrt_device,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        POOL_BUFFERS,
        content,
    )?;
    capture.size = content;
    Ok(true)
}

/// `frame` copied into a CPU-side [`VideoFrame`], so the pool's buffer can go
/// back at once.
fn copy_frame(gpu: &mut Gpu, frame: &Direct3D11CaptureFrame) -> Result<VideoFrame> {
    let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
    let texture: ID3D11Texture2D = unsafe { access.GetInterface() }?;
    gpu.copier.copy(&texture)
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
    fn a_recreated_pool_restarts_only_after_it_settles() {
        let at = Instant::now();
        assert!(!restart_due(None, at + Duration::from_secs(5)));
        assert!(!restart_due(Some(at), at + Duration::from_millis(100)));
        assert!(restart_due(Some(at), at + RESIZE_SETTLE));
        // A clock read before the recreation is not a wait.
        assert!(!restart_due(Some(at + Duration::from_secs(1)), at));
    }

    /// Live: capture one real frame of the primary monitor and check it is
    /// the monitor's size and not uniformly black. Run on a desktop session:
    /// `cargo test -p pocketdesk-core --lib wgc_captures -- --ignored`.
    #[test]
    #[ignore = "needs an interactive Windows desktop"]
    fn wgc_captures_a_real_frame_of_the_primary_monitor() {
        per_monitor_dpi();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (tx, mut rx) = crate::streaming::latest_frame::channel();
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

    /// Read monitor sizes in pixels, as the server does from its start: a
    /// test process is not DPI-aware, and on a scaled display it reads them
    /// scaled (2048 wide for a 2560 monitor at 125 %).
    fn per_monitor_dpi() {
        use windows::Win32::UI::HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
        };
        // Already set by an earlier test in this process: nothing to do.
        let _ =
            unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    }

    /// A window of the test's own, one colour, that resizes itself when told
    /// to: its own thread calls `SetWindowPos`, so no input reaches anything.
    struct OwnWindow {
        hwnd: isize,
        resize: Option<std_mpsc::Sender<(i32, i32)>>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl OwnWindow {
        fn open(width: i32, height: i32) -> Self {
            use windows::Win32::Foundation::{COLORREF, HINSTANCE, LPARAM, LRESULT, WPARAM};
            use windows::Win32::Graphics::Gdi::CreateSolidBrush;
            use windows::Win32::System::LibraryLoader::GetModuleHandleW;
            use windows::Win32::UI::WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE,
                PeekMessageW, RegisterClassW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE,
                SWP_NOZORDER, SetWindowPos, ShowWindow, WNDCLASSW, WS_EX_NOACTIVATE,
                WS_OVERLAPPEDWINDOW,
            };
            use windows::core::w;

            unsafe extern "system" fn proc(
                hwnd: HWND,
                msg: u32,
                wparam: WPARAM,
                lparam: LPARAM,
            ) -> LRESULT {
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }

            let (hwnd_tx, hwnd_rx) = std_mpsc::channel();
            let (resize, resize_rx) = std_mpsc::channel::<(i32, i32)>();
            let thread = std::thread::spawn(move || {
                let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.unwrap().into();
                let class = WNDCLASSW {
                    lpfnWndProc: Some(proc),
                    hInstance: instance,
                    lpszClassName: w!("PocketdeskResizeTest"),
                    hbrBackground: unsafe { CreateSolidBrush(COLORREF(0x0030_a050)) },
                    ..Default::default()
                };
                unsafe { RegisterClassW(&class) };
                let hwnd = unsafe {
                    CreateWindowExW(
                        WS_EX_NOACTIVATE,
                        w!("PocketdeskResizeTest"),
                        w!("Pocketdesk resize test"),
                        WS_OVERLAPPEDWINDOW,
                        120,
                        120,
                        width,
                        height,
                        None,
                        None,
                        Some(instance),
                        None,
                    )
                }
                .unwrap();
                let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
                hwnd_tx.send(hwnd.0 as isize).unwrap();
                loop {
                    let mut msg = MSG::default();
                    while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                        unsafe { DispatchMessageW(&msg) };
                    }
                    match resize_rx.recv_timeout(Duration::from_millis(10)) {
                        Ok((w, h)) => unsafe {
                            let _ = SetWindowPos(
                                hwnd,
                                None,
                                0,
                                0,
                                w,
                                h,
                                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                            );
                        },
                        Err(std_mpsc::RecvTimeoutError::Timeout) => {}
                        Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                let _ = unsafe { DestroyWindow(hwnd) };
            });
            Self {
                hwnd: hwnd_rx.recv().unwrap(),
                resize: Some(resize),
                thread: Some(thread),
            }
        }

        fn resize(&self, width: i32, height: i32) {
            self.resize.as_ref().unwrap().send((width, height)).unwrap();
        }
    }

    impl Drop for OwnWindow {
        fn drop(&mut self) {
            self.resize = None;
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    /// The next frame from `rx` within `within` that `wanted` accepts.
    async fn frame_where(
        rx: &mut crate::streaming::latest_frame::FrameReceiver<VideoFrame>,
        within: Duration,
        wanted: impl Fn(&VideoFrame) -> bool,
    ) -> Option<VideoFrame> {
        tokio::time::timeout(within, async {
            while let Some(frame) = rx.recv().await {
                if wanted(&frame) {
                    return Some(frame);
                }
            }
            None
        })
        .await
        .ok()
        .flatten()
    }

    /// Live: a captured window resized once and then left still reaches the
    /// phone at its new size. The frame whose size changed used to be dropped
    /// for the pool's recreation, and a still window sends no other, so the
    /// heartbeat repeated the old size until something moved. Run on a
    /// desktop session (it opens a small window of its own for a few
    /// seconds): `cargo test -p pocketdesk-core --lib wgc_resized -- --ignored`.
    #[test]
    #[ignore = "needs an interactive Windows desktop"]
    fn wgc_resized_still_window_reaches_its_new_size() {
        per_monitor_dpi();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let window = OwnWindow::open(480, 360);
            let (tx, mut rx) = crate::streaming::latest_frame::channel();
            let cancel = CancellationToken::new();
            let (window_tx, window_rx) = watch::channel(0u64);
            let window_mode = Arc::new(AtomicBool::new(false));
            let _session = start_wgc_capture(
                StreamingConfig::default(),
                tx,
                cancel.clone(),
                window_rx,
                window_mode.clone(),
            )
            .expect("WGC should start on a desktop session");
            let (_, monitor) = enumerate_monitors()[0];
            let monitor_width = (monitor.right - monitor.left) as u32;
            window_tx.send(window.hwnd as u64).unwrap();
            let first = frame_where(&mut rx, Duration::from_secs(5), |f| {
                window_mode.load(Ordering::Relaxed) && f.width < monitor_width
            })
            .await
            .expect("a frame of the window within 5 s");
            // Let the window settle: whatever its first frames were, none is
            // still to come once a heartbeat repeats it.
            tokio::time::sleep(Duration::from_millis(1500)).await;
            while rx.try_recv().is_some() {}

            window.resize(720, 540);
            let resized = frame_where(&mut rx, Duration::from_secs(5), |f| {
                (f.width, f.height) != (first.width, first.height)
            })
            .await;
            cancel.cancel();
            let resized = resized.unwrap_or_else(|| {
                panic!(
                    "no frame at the window's new size within 5 s (still {}x{})",
                    first.width, first.height
                )
            });
            assert!(
                resized.width > first.width && resized.height > first.height,
                "{}x{} after growing from {}x{}",
                resized.width,
                resized.height,
                first.width,
                first.height
            );
        });
    }
}
