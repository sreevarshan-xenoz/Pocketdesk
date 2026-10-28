//! This computer's screens, as another computer sharing its keyboard and
//! mouse needs them: in logical pixels, in this computer's own desktop
//! space, whose bounding box is what a position of 0..=65535 spans (as
//! `InputPacket::MouseMoveAbs`'s does).

use super::wire::Screen;
use crate::hyprland::HyprMonitor;

/// This computer's screens now: Windows' monitors; Hyprland's, at their
/// logical size; else the one rectangle the capture's layout spans. Empty
/// when none can be read.
pub async fn current() -> Vec<Screen> {
    #[cfg(windows)]
    {
        logical(&crate::windows_desktop::monitors())
    }
    #[cfg(not(windows))]
    {
        #[cfg(target_os = "linux")]
        if let Ok(ipc) = crate::hyprland::HyprlandIpc::from_env()
            && let Ok(monitors) = ipc.monitors().await
        {
            return logical(&monitors);
        }
        let layout = tokio::task::spawn_blocking(pocketdesk_core::streaming::capture::layout_rect)
            .await
            .ok()
            .flatten();
        layout
            .map(|(x, y, width, height)| Screen {
                x,
                y,
                width,
                height,
            })
            .into_iter()
            .filter(Screen::is_sound)
            .collect()
    }
}

/// `monitors` as screens of their logical size: the mode divided by the
/// scale (rounded), on its side for an odd transform. Hyprland already
/// places them in logical pixels; Windows' come at scale 1, in the physical
/// pixels its absolute input spans too. One without a size is left out.
pub fn logical(monitors: &[HyprMonitor]) -> Vec<Screen> {
    monitors
        .iter()
        .filter_map(|m| {
            let scale = if m.scale.is_finite() && m.scale > 0.0 {
                m.scale
            } else {
                1.0
            };
            let size = |px: i32| {
                let logical = (f64::from(px) / scale).round();
                // A float past u32 saturates; one of zero is dropped below.
                (logical >= 1.0).then_some(logical as u32)
            };
            let (mut width, mut height) = (size(m.width)?, size(m.height)?);
            if m.transform % 2 != 0 {
                std::mem::swap(&mut width, &mut height);
            }
            Some(Screen {
                x: m.x,
                y: m.y,
                width,
                height,
            })
        })
        .filter(Screen::is_sound)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `j/monitors` reply as Hyprland writes it (trimmed): a 2560x1600
    /// laptop panel at 125%, and a 1080p monitor beside it turned on its
    /// side.
    const MONITORS: &str = r#"[
        {"id": 0, "name": "eDP-1", "description": "panel", "make": "x",
         "width": 2560, "height": 1600, "refreshRate": 165.0,
         "x": 0, "y": 0, "activeWorkspace": {"id": 1, "name": "1"},
         "scale": 1.25, "transform": 0, "focused": true},
        {"id": 1, "name": "HDMI-A-1", "width": 1920, "height": 1080,
         "x": 2048, "y": -200, "scale": 1.00, "transform": 1}
    ]"#;

    #[test]
    fn hyprland_scale_gives_logical_size() {
        let monitors: Vec<HyprMonitor> = serde_json::from_str(MONITORS).unwrap();
        assert_eq!(monitors[0].scale, 1.25);
        assert_eq!(
            logical(&monitors),
            vec![
                Screen {
                    x: 0,
                    y: 0,
                    width: 2048,
                    height: 1280,
                },
                Screen {
                    x: 2048,
                    y: -200,
                    width: 1080,
                    height: 1920,
                },
            ]
        );
    }

    #[test]
    fn a_reply_without_a_scale_is_at_one_and_odd_ones_are_dropped() {
        let monitors: Vec<HyprMonitor> = serde_json::from_str(
            r#"[{"name": "A", "width": 1920, "height": 1080, "x": 0, "y": 0},
                {"name": "B", "width": 0, "height": 1080, "x": 0, "y": 0},
                {"name": "C", "width": 1920, "height": 1080, "x": 0, "y": 0, "scale": 0.0},
                {"name": "D", "width": 100, "height": 100, "x": 2147483600, "y": 0}]"#,
        )
        .unwrap();
        assert_eq!(monitors[0].scale, 1.0);
        let screens = logical(&monitors);
        // A has its mode; B has no width; C's scale is no scale; D's far
        // edge is past i32.
        assert_eq!(screens.len(), 2);
        assert!(screens.iter().all(|s| s.width == 1920 && s.height == 1080));
    }
}
