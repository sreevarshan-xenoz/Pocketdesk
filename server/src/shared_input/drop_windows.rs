//! The catcher on Windows (BK4, [`super::drag_files`]): a small window
//! under the parked cursor, registered as a drop target, on a thread of its
//! own (an OLE single-threaded apartment with its own message loop, never
//! the hooks' thread). It sits on top, never takes the focus or a click's
//! activation, is in neither the taskbar nor Alt+Tab, and is all but
//! see-through: one step of 255, not none, because Windows finds a drop
//! target by the window under the cursor, and a window with none is passed
//! through. Files dropped on it go to the driver's loop as their paths, as
//! Windows hands them over; nothing is read from them here.
//!
//! Nothing here is run by the tests: they must not make a window or
//! register a drop target on the owner's computer.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINTL, WPARAM};
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, IDropTarget, IDropTarget_Impl, OleInitialize, OleUninitialize,
    RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetMessageW,
    GetWindowLongPtrW, HWND_TOPMOST, LWA_ALPHA, MA_NOACTIVATE, MSG, PostMessageW, PostQuitMessage,
    RegisterClassW, SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetLayeredWindowAttributes,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, WM_APP, WM_MOUSEACTIVATE, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{Ref, implement, w};

use super::capture_windows::{pack, unpack};
use super::drag_files::{self, CATCHER_PX};

/// The window messages that put the catcher up (its `LPARAM`: the point it
/// centres on, packed) and take it down; the `WPARAM` of each is its
/// number, answered once done.
const WM_CATCHER_SHOW: u32 = WM_APP + 1;
const WM_CATCHER_HIDE: u32 = WM_APP + 2;
/// The window message that ends the catcher's thread.
const WM_CATCHER_QUIT: u32 = WM_APP + 3;

/// How long the driver's loop waits for the catcher to be up (or down)
/// before it parks the cursor (or brings it home) anyway.
const ANSWER_WITHIN: Duration = Duration::from_millis(150);

/// How long the thread is given to start, and to end.
const START_WITHIN: Duration = Duration::from_secs(2);
const END_WITHIN: Duration = Duration::from_secs(1);

/// The most characters of a dropped path read (Windows' long paths).
const MOST_PATH: usize = 32_767;

/// Set while a drag of files is over the catcher: Esc is then Windows' own
/// ([`drag_files::esc_cancels_drag`]).
static FILES_OVER: AtomicBool = AtomicBool::new(false);

/// Whether a drag of files is over the catcher now.
pub fn files_over() -> bool {
    FILES_OVER.load(Ordering::SeqCst)
}

/// The catcher's thread, once it runs.
pub struct DropCatcher {
    /// Its window, as a number (only its thread uses the handle itself).
    window: isize,
    thread: Option<JoinHandle<()>>,
    answers: Receiver<usize>,
    asked: usize,
}

impl DropCatcher {
    /// Start the catcher's thread, its window down; the files dropped on it
    /// go to `dropped`. `None` when it could not start.
    pub fn start(dropped: Sender<Vec<PathBuf>>) -> Option<DropCatcher> {
        let (ready_tx, ready_rx) = mpsc::channel::<Result<isize, String>>();
        let (answer_tx, answers) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("pd-drop-catcher".into())
            .spawn(move || run(dropped, ready_tx, answer_tx))
            .map_err(|e| tracing::warn!("Shared input: no thread for dragged files: {e}"))
            .ok()?;
        match ready_rx.recv_timeout(START_WITHIN) {
            Ok(Ok(window)) => Some(DropCatcher {
                window,
                thread: Some(thread),
                answers,
                asked: 0,
            }),
            Ok(Err(e)) => {
                tracing::warn!("Shared input: files dragged across will not be caught: {e}");
                let _ = thread.join();
                None
            }
            Err(_) => {
                // It may yet get going: it ends by itself once it finds no
                // one waiting for it.
                tracing::warn!("Shared input: the dragged files' catcher did not start in time");
                None
            }
        }
    }

    /// Put the catcher up, centred on `at` (physical), and wait (briefly)
    /// until it is.
    pub fn show(&mut self, at: (i32, i32)) {
        if !self.ask(WM_CATCHER_SHOW, pack(at)) {
            tracing::debug!("Shared input: the dragged files' catcher was slow to show");
        }
    }

    /// Take the catcher down, and wait (briefly) until it is.
    pub fn hide(&mut self) {
        if !self.ask(WM_CATCHER_HIDE, 0) {
            tracing::debug!("Shared input: the dragged files' catcher was slow to go");
        }
    }

    /// Ask the window for `msg` and wait for its answer; whether it came in
    /// time. A window message, not a thread's: it is still handled while
    /// the thread waits inside a drop (OLE's own loop dispatches it).
    fn ask(&mut self, msg: u32, lparam: isize) -> bool {
        self.asked = self.asked.wrapping_add(1);
        let asked = self.asked;
        // SAFETY: a message to the catcher's own window.
        let posted = unsafe {
            PostMessageW(
                Some(HWND(self.window as *mut core::ffi::c_void)),
                msg,
                WPARAM(asked),
                LPARAM(lparam),
            )
        };
        if posted.is_err() {
            return false;
        }
        let deadline = Instant::now() + ANSWER_WITHIN;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.answers.recv_timeout(left) {
                Ok(answer) if answer == asked => return true,
                // An answer to an earlier ask that came too late.
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    }
}

impl Drop for DropCatcher {
    fn drop(&mut self) {
        // SAFETY: as in `ask`; its window ends the thread's loop.
        let _ = unsafe {
            PostMessageW(
                Some(HWND(self.window as *mut core::ffi::c_void)),
                WM_CATCHER_QUIT,
                WPARAM(0),
                LPARAM(0),
            )
        };
        if let Some(thread) = self.thread.take() {
            let deadline = Instant::now() + END_WITHIN;
            while !thread.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if thread.is_finished() {
                let _ = thread.join();
            } else {
                tracing::warn!("Shared input: the dragged files' catcher did not end; left");
            }
        }
        FILES_OVER.store(false, Ordering::SeqCst);
    }
}

/// The catcher's thread: OLE, its window and drop target, and its loop.
fn run(
    dropped: Sender<Vec<PathBuf>>,
    ready: Sender<Result<isize, String>>,
    answers: Sender<usize>,
) {
    // SAFETY: plain Win32 and OLE calls on this thread's own state; each
    // made here is undone below.
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if let Err(e) = OleInitialize(None) {
            let _ = ready.send(Err(format!("OLE: {e}")));
            return;
        }
        let window = match make_window() {
            Ok(window) => window,
            Err(e) => {
                let _ = ready.send(Err(e));
                OleUninitialize();
                return;
            }
        };
        let target: IDropTarget = DropTarget {
            dropped,
            has_files: Cell::new(false),
        }
        .into();
        if let Err(e) = RegisterDragDrop(window, &target) {
            let _ = ready.send(Err(format!("drop target: {e}")));
            let _ = DestroyWindow(window);
            OleUninitialize();
            return;
        }
        // Where the window answers that an ask is done: its own, freed
        // once it is gone.
        let answers = Box::into_raw(Box::new(answers));
        SetWindowLongPtrW(window, GWLP_USERDATA, answers as isize);
        // A starter that gave up waiting is gone: nothing would ever ask
        // this window anything, or end it.
        if ready.send(Ok(window.0 as isize)).is_ok() {
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                DispatchMessageW(&msg);
            }
        }
        let _ = RevokeDragDrop(window);
        let _ = DestroyWindow(window);
        drop(Box::from_raw(answers));
        drop(target);
        OleUninitialize();
    }
    FILES_OVER.store(false, Ordering::SeqCst);
}

/// The catcher's window, down: on top, never activated, never in the
/// taskbar or Alt+Tab, one step from see-through.
fn make_window() -> Result<HWND, String> {
    // SAFETY: this module's handle, a class of its window procedure, and a
    // window this thread owns.
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None)
            .map_err(|e| format!("module handle: {e}"))?
            .into();
        let class = WNDCLASSW {
            lpfnWndProc: Some(catcher_proc),
            hInstance: instance,
            lpszClassName: w!("PocketdeskDropCatcher"),
            ..Default::default()
        };
        // Registering it again (a later start) fails harmlessly.
        RegisterClassW(&class);
        let window = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            w!("PocketdeskDropCatcher"),
            w!("Pocketdesk"),
            WS_POPUP,
            0,
            0,
            CATCHER_PX,
            CATCHER_PX,
            None,
            None,
            Some(instance),
            None,
        )
        .map_err(|e| format!("window: {e}"))?;
        let _ = SetLayeredWindowAttributes(window, COLORREF(0), 1, LWA_ALPHA);
        Ok(window)
    }
}

/// The catcher's window: up and down when asked, never activated.
unsafe extern "system" fn catcher_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CATCHER_SHOW => {
            let (x, y, width, height) = drag_files::rect(unpack(lparam.0));
            // SAFETY: this thread's window.
            let _ = unsafe {
                SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    x,
                    y,
                    width,
                    height,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                )
            };
            answer(hwnd, wparam.0);
            LRESULT(0)
        }
        WM_CATCHER_HIDE => {
            // SAFETY: this thread's window.
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            FILES_OVER.store(false, Ordering::SeqCst);
            answer(hwnd, wparam.0);
            LRESULT(0)
        }
        WM_CATCHER_QUIT => {
            // SAFETY: ends this thread's own loop.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        // SAFETY: the default handling for this window's messages.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Say the ask numbered `asked` is done, where the window `hwnd` answers.
fn answer(hwnd: HWND, asked: usize) {
    // SAFETY: the window's own value, set by its thread to a sender that
    // lives until the window is gone (and read only on that thread).
    unsafe {
        let answers = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Sender<usize>;
        if let Some(answers) = answers.as_ref() {
            let _ = answers.send(asked);
        }
    }
}

/// Files as Windows hands a drag of them over: `CF_HDROP` in global memory.
fn hdrop_format() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

/// Whether `data` holds files.
fn holds_files(data: &IDataObject) -> bool {
    // SAFETY: a query of what the drag offers.
    unsafe { data.QueryGetData(&hdrop_format()) }.is_ok()
}

/// The paths of the files and folders `data` holds, as Windows lists them.
fn files_of(data: &IDataObject) -> Vec<PathBuf> {
    // SAFETY: the medium Windows hands over is read with DragQueryFileW
    // within its bounds, and released once read.
    unsafe {
        let Ok(mut medium) = data.GetData(&hdrop_format()) else {
            return Vec::new();
        };
        let drop = HDROP(medium.u.hGlobal.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        let paths = (0..count)
            .filter_map(|i| {
                let len = (DragQueryFileW(drop, i, None) as usize).min(MOST_PATH);
                let mut name = vec![0u16; len + 1];
                let got = DragQueryFileW(drop, i, Some(&mut name)) as usize;
                (got > 0).then(|| PathBuf::from(String::from_utf16_lossy(&name[..got])))
            })
            .collect();
        ReleaseStgMedium(&mut medium);
        paths
    }
}

/// The catcher's drop target: files are taken as a copy, anything else
/// not at all.
#[implement(IDropTarget)]
struct DropTarget {
    dropped: Sender<Vec<PathBuf>>,
    /// The drag over it now holds files.
    has_files: Cell<bool>,
}

/// Answer `effect` for a drag that holds files, or not.
fn answer_effect(effect: *mut DROPEFFECT, has_files: bool) {
    if !effect.is_null() {
        // SAFETY: Windows' out value for this call.
        unsafe { *effect = DROPEFFECT(drag_files::effect(has_files)) };
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        _at: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let has_files = data.as_ref().is_some_and(holds_files);
        self.has_files.set(has_files);
        FILES_OVER.store(has_files, Ordering::SeqCst);
        answer_effect(effect, has_files);
        Ok(())
    }

    fn DragOver(
        &self,
        _keys: MODIFIERKEYS_FLAGS,
        _at: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        answer_effect(effect, self.has_files.get());
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        self.has_files.set(false);
        FILES_OVER.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        _at: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let files = data.as_ref().map(files_of).unwrap_or_default();
        self.has_files.set(false);
        FILES_OVER.store(false, Ordering::SeqCst);
        answer_effect(effect, !files.is_empty());
        // The sends go on the driver's loop: the drag's own program waits
        // for this to return.
        if !files.is_empty() {
            let _ = self.dropped.send(files);
        }
        Ok(())
    }
}
