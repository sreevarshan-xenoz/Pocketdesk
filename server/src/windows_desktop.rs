//! The Windows desktop in the shape the phone already understands.
//!
//! The window picker and the Workspace HUD speak Hyprland's JSON (address,
//! title, class, geometry, monitor). This module fills the same
//! [`HyprWindow`]/[`HyprMonitor`] structs from Win32 so the plugin, the HUD
//! and the phone need no Windows branch: the address is the `HWND` (which the
//! window-crop path hands straight to Windows.Graphics.Capture), geometry is
//! the DWM frame in physical pixels (what capture produces under per-monitor
//! DPI awareness), and the "class" is the program's file name.
//!
//! Windows has virtual desktops but no stable public API for them, so every
//! window reports one workspace and the HUD's workspace list stays empty.

use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MONITORINFOEXW, MonitorFromWindow,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GWL_EXSTYLE, GetForegroundWindow, GetWindow, GetWindowLongW,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    WS_EX_TOOLWINDOW,
};
use windows::core::BOOL;

use crate::hyprland::{HyprMonitor, HyprWindow, HyprWorkspaceRef};

/// The `address` string for a window: its HWND in Hyprland's `0x…` form.
pub fn address_of(hwnd: HWND) -> String {
    format!("0x{:x}", hwnd.0 as usize)
}

/// Monitors in `EnumDisplayMonitors` order (the order capture and the
/// monitors plugin use), with their device names.
pub fn monitors() -> Vec<HyprMonitor> {
    monitor_handles().into_iter().map(|(_, m)| m).collect()
}

fn monitor_info(monitor: HMONITOR) -> Option<HyprMonitor> {
    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    unsafe { GetMonitorInfoW(monitor, &mut info.monitorInfo) }
        .as_bool()
        .then_some(())?;
    let r = info.monitorInfo.rcMonitor;
    let len = info.szDevice.iter().position(|&c| c == 0).unwrap_or(0);
    Some(HyprMonitor {
        name: String::from_utf16_lossy(&info.szDevice[..len]),
        x: r.left,
        y: r.top,
        width: r.right - r.left,
        height: r.bottom - r.top,
    })
}

/// Whether a top-level window is one a user would pick (what Alt-Tab shows).
pub(crate) fn is_pickable(
    visible: bool,
    minimized: bool,
    owned: bool,
    tool: bool,
    cloaked: bool,
    titled: bool,
) -> bool {
    visible && !minimized && !owned && !tool && !cloaked && titled
}

fn title_of(hwnd: HWND) -> String {
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return String::new();
    }
    let mut buffer = vec![0u16; len as usize + 1];
    let copied = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..copied.max(0) as usize])
}

fn program_of(pid: u32) -> String {
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
    else {
        return String::new();
    };
    let mut buffer = [0u16; 1024];
    let mut len = buffer.len() as u32;
    let named = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut len,
        )
    };
    let _ = unsafe { windows::Win32::Foundation::CloseHandle(process) };
    if named.is_err() {
        return String::new();
    }
    std::path::Path::new(&String::from_utf16_lossy(&buffer[..len as usize]))
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn frame_of(hwnd: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut _ as *mut _,
            std::mem::size_of::<RECT>() as u32,
        )
    }
    .ok()?;
    (rect.right > rect.left && rect.bottom > rect.top).then_some(rect)
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut _ as *mut _,
            std::mem::size_of::<u32>() as u32,
        )
    }
    .is_ok()
        && cloaked != 0
}

/// Describe one window, or `None` when it is not one the user would pick.
fn describe(hwnd: HWND, monitors: &[(HMONITOR, HyprMonitor)]) -> Option<HyprWindow> {
    let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
    let minimized = unsafe { IsIconic(hwnd) }.as_bool();
    let owned = unsafe { GetWindow(hwnd, GW_OWNER) }.is_ok_and(|o| !o.is_invalid());
    let tool = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32 & WS_EX_TOOLWINDOW.0 != 0;
    let title = title_of(hwnd);
    if !is_pickable(
        visible,
        minimized,
        owned,
        tool,
        is_cloaked(hwnd),
        !title.is_empty(),
    ) {
        return None;
    }
    let frame = frame_of(hwnd)?;
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    let on = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let monitor = monitors.iter().position(|(m, _)| *m == on).unwrap_or(0) as i32;
    Some(HyprWindow {
        address: address_of(hwnd),
        title,
        class: program_of(pid),
        pid,
        mapped: true,
        hidden: false,
        at: [frame.left, frame.top],
        size: [frame.right - frame.left, frame.bottom - frame.top],
        monitor,
        fullscreen: 0,
        workspace: HyprWorkspaceRef {
            id: 1,
            name: "desktop".to_string(),
        },
    })
}

fn monitor_handles() -> Vec<(HMONITOR, HyprMonitor)> {
    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        // SAFETY: as in `monitors`.
        let list = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
        list.push(monitor);
        BOOL(1)
    }
    let mut handles: Vec<HMONITOR> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(&mut handles as *mut _ as isize),
        );
    }
    handles
        .into_iter()
        .filter_map(|m| monitor_info(m).map(|info| (m, info)))
        .collect()
}

/// The pickable windows, top of the Z-order first, plus the focused one.
pub fn visible_windows() -> (Vec<HyprWindow>, Option<HyprWindow>, Vec<HyprMonitor>) {
    unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the Vec passed below, alive for the whole call.
        let list = unsafe { &mut *(data.0 as *mut Vec<HWND>) };
        list.push(hwnd);
        BOOL(1)
    }
    let mut handles: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut handles as *mut _ as isize));
    }
    let monitors = monitor_handles();
    let windows: Vec<HyprWindow> = handles
        .into_iter()
        .filter_map(|hwnd| describe(hwnd, &monitors))
        .collect();
    let focused = address_of(unsafe { GetForegroundWindow() });
    let active = windows.iter().find(|w| w.address == focused).cloned();
    (
        windows,
        active,
        monitors.into_iter().map(|(_, m)| m).collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_alt_tab_windows_are_pickable() {
        assert!(is_pickable(true, false, false, false, false, true));
        for (visible, minimized, owned, tool, cloaked, titled) in [
            (false, false, false, false, false, true), // hidden
            (true, true, false, false, false, true),   // minimized
            (true, false, true, false, false, true),   // dialog owned by another
            (true, false, false, true, false, true),   // tool window
            (true, false, false, false, true, true),   // on another virtual desktop
            (true, false, false, false, false, false), // untitled
        ] {
            assert!(!is_pickable(
                visible, minimized, owned, tool, cloaked, titled
            ));
        }
    }

    #[test]
    fn addresses_are_hyprland_style_hex_handles() {
        assert_eq!(address_of(HWND(0x1a2b as *mut _)), "0x1a2b");
    }

    /// Live, read-only: this desktop has monitors and at least one window,
    /// each placed on a listed monitor with a real size.
    #[test]
    #[ignore = "needs an interactive Windows desktop"]
    fn the_desktop_lists_its_monitors_and_windows() {
        crate::platform::init_process();
        let (windows, _active, monitors) = visible_windows();
        assert!(!monitors.is_empty());
        assert!(!windows.is_empty(), "no pickable windows found");
        for w in &windows {
            assert!((w.monitor as usize) < monitors.len(), "{w:?}");
            assert!(w.size[0] > 0 && w.size[1] > 0, "{w:?}");
            assert!(w.address.starts_with("0x"));
        }
        assert_eq!(monitors.len(), super::monitors().len());
    }
}
