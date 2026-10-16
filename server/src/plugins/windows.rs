use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use tracing::info;

#[cfg(not(windows))]
use crate::hyprland::HyprlandIpc;
use crate::hyprland::{HyprMonitor, HyprWindow};

/// A window plus the monitor context the phone needs to drive window-crop:
/// `local_at` is the crop rect origin in the monitor's own coordinate space
/// (what the capture stream carries), `at` stays global for input remapping.
#[derive(serde::Serialize)]
pub(crate) struct WindowEntry {
    #[serde(flatten)]
    window: HyprWindow,
    local_at: [i32; 2],
    monitor_size: [i32; 2],
}

fn monitor_of(monitors: &[HyprMonitor], index: i32) -> Option<&HyprMonitor> {
    usize::try_from(index).ok().and_then(|i| monitors.get(i))
}

pub(crate) fn entries(windows: Vec<HyprWindow>, monitors: &[HyprMonitor]) -> Vec<WindowEntry> {
    windows
        .into_iter()
        .map(|w| {
            let m = monitor_of(monitors, w.monitor);
            WindowEntry {
                local_at: [
                    w.at[0] - m.map_or(0, |m| m.x),
                    w.at[1] - m.map_or(0, |m| m.y),
                ],
                monitor_size: m.map_or([0, 0], |m| [m.width, m.height]),
                window: w,
            }
        })
        .collect()
}

/// Layout bounding box of all monitors as `[x, y, width, height]` — the
/// space normalized direct-touch input maps across.
pub(crate) fn screen_box(monitors: &[HyprMonitor]) -> Option<[i32; 4]> {
    let min_x = monitors.iter().map(|m| m.x).min()?;
    let min_y = monitors.iter().map(|m| m.y).min()?;
    let max_x = monitors.iter().map(|m| m.x + m.width).max()?;
    let max_y = monitors.iter().map(|m| m.y + m.height).max()?;
    Some([min_x, min_y, max_x - min_x, max_y - min_y])
}

/// R3#7: Hyprland window enumeration for the phone-side picker.
///
/// Responds to `kdeconnect.pocketdesk.windows` queries with the visible
/// windows (address, title, class, global + monitor-local geometry, size,
/// workspace) plus the active window address and the monitor layout box.
/// On non-Hyprland sessions replies `available: false`.
pub struct WindowsPlugin;

#[async_trait::async_trait]
impl Plugin for WindowsPlugin {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.pocketdesk.windows"]
    }

    // What it answers with: the identity announces it (ecosystem 1g).
    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.pocketdesk.windows"]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type != "kdeconnect.pocketdesk.windows" {
            return Ok(());
        }

        #[cfg(windows)]
        let body = {
            let (windows, active, monitors) =
                tokio::task::spawn_blocking(crate::windows_desktop::visible_windows)
                    .await
                    .unwrap_or_default();
            info!("Windows queried: {} visible", windows.len());
            serde_json::json!({
                "available": true,
                "activeAddress": active.map(|w| w.address),
                "screen": screen_box(&monitors),
                "windows": entries(windows, &monitors),
            })
        };
        #[cfg(not(windows))]
        let body = match HyprlandIpc::from_env() {
            Ok(ipc) => match ipc.visible_windows().await {
                Ok(windows) => {
                    let active = ipc.active_window().await.ok().flatten().map(|w| w.address);
                    // Best-effort: without the monitor table the response
                    // still works for single-monitor setups (offset 0,0).
                    let monitors = ipc.monitors().await.unwrap_or_default();
                    info!("Windows queried: {} visible", windows.len());
                    serde_json::json!({
                        "available": true,
                        "activeAddress": active,
                        "screen": screen_box(&monitors),
                        "windows": entries(windows, &monitors),
                    })
                }
                Err(e) => {
                    info!("Hyprland window query failed: {e}");
                    serde_json::json!({
                        "available": false,
                        "activeAddress": null,
                        "screen": null,
                        "windows": [],
                    })
                }
            },
            Err(_) => serde_json::json!({
                "available": false,
                "activeAddress": null,
                "screen": null,
                "windows": [],
            }),
        };

        let response = NetworkPacket::new("kdeconnect.pocketdesk.windows").with_body(body);
        sender.send_packet(&response).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hyprland::HyprWorkspaceRef;

    fn monitor(name: &str, x: i32, y: i32, w: i32, h: i32) -> HyprMonitor {
        HyprMonitor {
            name: name.into(),
            x,
            y,
            width: w,
            height: h,
        }
    }

    fn window(at: [i32; 2], size: [i32; 2], monitor: i32) -> HyprWindow {
        HyprWindow {
            address: "0x1".into(),
            title: "t".into(),
            class: "c".into(),
            pid: 1,
            mapped: true,
            hidden: false,
            at,
            size,
            monitor,
            fullscreen: 0,
            workspace: HyprWorkspaceRef::default(),
        }
    }

    #[test]
    fn local_at_is_monitor_relative() {
        let monitors = vec![
            monitor("eDP-1", 0, 0, 1920, 1080),
            monitor("HDMI-A-1", 1920, -100, 2560, 1440),
        ];
        let out = entries(vec![window([2100, 40], [800, 600], 1)], &monitors);
        assert_eq!(out[0].local_at, [180, 140]);
        assert_eq!(out[0].monitor_size, [2560, 1440]);
    }

    #[test]
    fn unknown_monitor_falls_back_to_global() {
        let monitors = vec![monitor("eDP-1", 0, 0, 1920, 1080)];
        let out = entries(vec![window([10, 20], [800, 600], 7)], &monitors);
        assert_eq!(out[0].local_at, [10, 20]);
        assert_eq!(out[0].monitor_size, [0, 0]);
    }

    #[test]
    fn screen_box_spans_layout() {
        assert_eq!(
            screen_box(&[
                monitor("a", 0, 0, 1920, 1080),
                monitor("b", 1920, -100, 2560, 1440),
            ]),
            Some([0, -100, 4480, 1440])
        );
        assert_eq!(screen_box(&[]), None);
    }

    #[test]
    fn entry_serialization_keeps_window_fields() {
        let json = serde_json::to_value(
            &entries(
                vec![window([5, 5], [100, 50], 0)],
                &[monitor("eDP-1", 0, 0, 1920, 1080)],
            )[0],
        )
        .unwrap();
        assert_eq!(json["address"], "0x1");
        assert_eq!(json["at"][0], 5);
        assert_eq!(json["local_at"][0], 5);
        assert_eq!(json["monitor_size"][1], 1080);
        assert!(json["workspace"].is_object());
    }
}
