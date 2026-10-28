//! The badge on Windows: a topmost, click-through, layered window centred
//! on each monitor (the privacy ring's pattern), shown without activation,
//! its text drawn with GDI. One thread of its own puts the windows up,
//! pumps their messages and takes them down when their time is up or a
//! newer badge replaces them.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateRoundRectRgn,
    CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CALCRECT, DT_CENTER, DT_NOPREFIX,
    DT_WORDBREAK, DeleteObject, DrawTextW, EndPaint, FF_SWISS, FW_SEMIBOLD, GetDC, HFONT, HGDIOBJ,
    OUT_DEFAULT_PRECIS, PAINTSTRUCT, ReleaseDC, SelectObject, SetBkMode, SetTextColor,
    SetWindowRgn, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, LWA_ALPHA,
    MA_NOACTIVATE, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT,
    RegisterClassW, SW_SHOWNOACTIVATE, SetLayeredWindowAttributes, ShowWindow, TranslateMessage,
    WM_MOUSEACTIVATE, WM_PAINT, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::w;

/// The badge's colours (a dark plate, white words) and its opacity.
const PLATE_RGB: (u8, u8, u8) = (0x20, 0x20, 0x20);
const TEXT_RGB: (u8, u8, u8) = (0xFF, 0xFF, 0xFF);
const ALPHA: u8 = 0xE6;

/// Its sizes at 100% scale, in pixels.
const FONT_PX: f64 = 26.0;
const PAD_PX: f64 = 18.0;
const CORNER_PX: f64 = 16.0;

/// How often the thread looks at the clock and its queue while a badge
/// shows.
const TICK: Duration = Duration::from_millis(30);

/// A badge to show: its text (lines joined) and until when.
struct Flash {
    text: Vec<u16>,
    until: Instant,
}

/// What a badge's window paints with: its window and font, as numbers
/// (only the badge thread makes, uses and destroys them), and its padding.
struct Painted {
    window: usize,
    font: usize,
    pad: i32,
}

impl Painted {
    fn font(&self) -> HFONT {
        HFONT(self.font as *mut core::ffi::c_void)
    }
}

/// The text the badge's windows show.
static TEXT: Mutex<Vec<u16>> = Mutex::new(Vec::new());

/// Each of its windows' font and padding.
static PAINTED: Mutex<Vec<Painted>> = Mutex::new(Vec::new());

/// The text, as last put up.
fn text() -> Vec<u16> {
    TEXT.lock().map(|t| t.clone()).unwrap_or_default()
}

static SENDER: OnceLock<Mutex<Option<Sender<Flash>>>> = OnceLock::new();

/// The badge thread, started at the first flash; `None` when it could not
/// start.
fn sender() -> Option<Sender<Flash>> {
    let slot = SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        let started = std::thread::Builder::new()
            .name("badge".into())
            .spawn(move || run(rx))
            .is_ok();
        Mutex::new(started.then_some(tx))
    });
    slot.lock().ok()?.clone()
}

/// Show `lines` on every monitor for `how_long`, in place of what shows.
pub fn flash(lines: Vec<String>, how_long: Duration) {
    let text = lines.join("\r\n").encode_utf16().collect();
    if let Some(tx) = sender() {
        let _ = tx.send(Flash {
            text,
            until: Instant::now() + how_long,
        });
    }
}

fn rgb((r, g, b): (u8, u8, u8)) -> COLORREF {
    COLORREF(u32::from(r) | u32::from(g) << 8 | u32::from(b) << 16)
}

/// The badge's thread: each flash replaces what shows; what shows goes
/// when its time is up.
fn run(flashes: Receiver<Flash>) {
    // SAFETY: this module's own handle.
    let Ok(module) = (unsafe { GetModuleHandleW(None) }) else {
        return;
    };
    let instance: HINSTANCE = module.into();
    let class = WNDCLASSW {
        lpfnWndProc: Some(badge_proc),
        hInstance: instance,
        lpszClassName: w!("PocketdeskBadge"),
        // SAFETY: a plain solid brush, kept for the class's life.
        hbrBackground: unsafe { CreateSolidBrush(rgb(PLATE_RGB)) },
        ..Default::default()
    };
    // SAFETY: a filled class, its name and procedure static.
    if unsafe { RegisterClassW(&class) } == 0 {
        tracing::debug!("Badge: its window class was not registered");
        return;
    }
    let mut shown: Vec<HWND> = Vec::new();
    let mut until = Instant::now();
    loop {
        let next = if shown.is_empty() {
            match flashes.recv() {
                Ok(flash) => Some(flash),
                Err(_) => return,
            }
        } else {
            pump();
            match flashes.try_recv() {
                Ok(flash) => Some(flash),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    take_down(&mut shown);
                    return;
                }
            }
        };
        let Some(mut flash) = next else {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                take_down(&mut shown);
            } else {
                // SAFETY: no handles; this thread's messages wake it.
                unsafe {
                    MsgWaitForMultipleObjects(
                        None,
                        false,
                        left.min(TICK).as_millis() as u32,
                        QS_ALLINPUT,
                    );
                }
            }
            continue;
        };
        // Only the newest of those waiting is shown.
        while let Ok(newer) = flashes.try_recv() {
            flash = newer;
        }
        take_down(&mut shown);
        if let Ok(mut text) = TEXT.lock() {
            *text = flash.text;
        }
        until = flash.until;
        shown = put_up(instance);
    }
}

/// Handle this thread's messages (the windows' paints).
fn pump() {
    let mut msg = MSG::default();
    // SAFETY: a valid MSG to fill; each is translated and dispatched.
    while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// A window per monitor, centred on it, with the text: topmost, clicked
/// through, never activated, never in the taskbar or Alt+Tab.
fn put_up(instance: HINSTANCE) -> Vec<HWND> {
    let mut shown = Vec::new();
    for monitor in crate::windows_desktop::monitors_scaled() {
        let scale = if monitor.scale > 0.0 {
            monitor.scale
        } else {
            1.0
        };
        let px = |v: f64| (v * scale).round() as i32;
        // SAFETY: plain values and a static face name.
        let font = unsafe {
            CreateFontW(
                -px(FONT_PX),
                0,
                0,
                0,
                FW_SEMIBOLD.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                u32::from(DEFAULT_PITCH.0 | FF_SWISS.0),
                w!("Segoe UI"),
            )
        };
        if font.is_invalid() {
            continue;
        }
        let pad = px(PAD_PX);
        let (text_w, text_h) = measure(font, (monitor.width * 4 / 5).max(1));
        let (width, height) = (text_w + 2 * pad, text_h + 2 * pad);
        // SAFETY: a registered class; the window is this thread's.
        let made = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST
                    | WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_NOACTIVATE
                    | WS_EX_TOOLWINDOW,
                w!("PocketdeskBadge"),
                w!("Pocketdesk"),
                WS_POPUP,
                monitor.x + (monitor.width - width) / 2,
                monitor.y + (monitor.height - height) / 2,
                width,
                height,
                None,
                None,
                Some(instance),
                None,
            )
        };
        let Ok(hwnd) = made else {
            // SAFETY: the font made above, selected nowhere.
            let _ = unsafe { DeleteObject(HGDIOBJ(font.0)) };
            continue;
        };
        if let Ok(mut painted) = PAINTED.lock() {
            painted.push(Painted {
                window: hwnd.0 as usize,
                font: font.0 as usize,
                pad,
            });
        }
        let corner = px(CORNER_PX);
        // SAFETY: this thread's window; the region is handed to it.
        unsafe {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), ALPHA, LWA_ALPHA);
            SetWindowRgn(
                hwnd,
                Some(CreateRoundRectRgn(
                    0,
                    0,
                    width + 1,
                    height + 1,
                    corner,
                    corner,
                )),
                false,
            );
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        shown.push(hwnd);
    }
    shown
}

/// The text's size in `font`, wrapped at `most` pixels.
fn measure(font: HFONT, most: i32) -> (i32, i32) {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: most,
        bottom: 0,
    };
    // SAFETY: the screen's DC, given back below; the font is selected
    // only while measuring.
    unsafe {
        let hdc = GetDC(None);
        let old = SelectObject(hdc, HGDIOBJ(font.0));
        DrawTextW(
            hdc,
            &mut text(),
            &mut rect,
            DT_CALCRECT | DT_CENTER | DT_NOPREFIX | DT_WORDBREAK,
        );
        SelectObject(hdc, old);
        ReleaseDC(None, hdc);
    }
    (rect.right - rect.left, rect.bottom - rect.top)
}

/// Destroy the windows and their fonts.
fn take_down(shown: &mut Vec<HWND>) {
    for hwnd in shown.drain(..) {
        // SAFETY: this thread's own window.
        let _ = unsafe { DestroyWindow(hwnd) };
    }
    let gone: Vec<Painted> = PAINTED
        .lock()
        .map(|mut painted| painted.drain(..).collect())
        .unwrap_or_default();
    for gone in gone {
        // SAFETY: its window is gone, so the font is selected nowhere.
        let _ = unsafe { DeleteObject(HGDIOBJ(gone.font().0)) };
    }
}

/// The badge's windows paint their text and never take activation.
unsafe extern "system" fn badge_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT if paint(hwnd) => LRESULT(0),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Paint the text, centred inside the padding (the class's brush paints
/// the plate); false for a window that is none of the badge's.
fn paint(hwnd: HWND) -> bool {
    let found = PAINTED.lock().ok().and_then(|painted| {
        painted
            .iter()
            .find(|p| p.window == hwnd.0 as usize)
            .map(|p| (p.font(), p.pad))
    });
    let Some((font, pad)) = found else {
        return false;
    };
    let mut ps = PAINTSTRUCT::default();
    // SAFETY: this thread's window, painted between BeginPaint and EndPaint.
    unsafe {
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rect = RECT::default();
        if GetClientRect(hwnd, &mut rect).is_ok() {
            rect.left += pad;
            rect.top += pad;
            rect.right -= pad;
            rect.bottom -= pad;
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, rgb(TEXT_RGB));
            let old = SelectObject(hdc, HGDIOBJ(font.0));
            DrawTextW(
                hdc,
                &mut text(),
                &mut rect,
                DT_CENTER | DT_NOPREFIX | DT_WORDBREAK,
            );
            SelectObject(hdc, old);
        }
        let _ = EndPaint(hwnd, &ps);
    }
    true
}
