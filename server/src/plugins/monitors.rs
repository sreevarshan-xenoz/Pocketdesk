use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use pocketdesk_core::streaming::MonitorInfo;

/// F2: Monitor enumeration plugin.
///
/// Responds to `kdeconnect.pocketdesk.monitors` queries by reporting
/// the details of available monitors (names, resolutions, indices).
pub struct MonitorsPlugin;

#[async_trait::async_trait]
impl Plugin for MonitorsPlugin {
    fn name(&self) -> &'static str {
        "monitors"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.pocketdesk.monitors"]
    }

    // What it answers with: the identity announces it (ecosystem 1g).
    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.pocketdesk.monitors"]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type == "kdeconnect.pocketdesk.monitors" {
            // A computer paired both ways answering this one's question on
            // the link it keeps (PC1): kept for the window that shows its
            // screen, never answered back.
            if crate::peer_computers::is_screens_answer(&packet.body) {
                if crate::plugins::pair::is_computer_device(sender.device_id()) {
                    crate::peer_computers::keep_screens(sender.device_id(), &packet.body);
                }
                return Ok(());
            }
            let response = answer();
            sender.send_packet(&response).await?;
            tracing::info!(
                "Monitors queried: {} found",
                response.body["count"].as_u64().unwrap_or(0)
            );
        }
        Ok(())
    }
}

/// This computer's monitors, as the answer to a monitors question.
pub(crate) fn answer() -> NetworkPacket {
    let monitors = enumerate_monitors();
    NetworkPacket::new("kdeconnect.pocketdesk.monitors").with_body(serde_json::json!({
        "count": monitors.len(),
        "monitors": monitors
    }))
}

/// Each listed monitor's origin, in index order: what the capture matches a
/// monitor index against (`capture_screencopy::monitor_rect_physical`).
pub(crate) fn origins() -> Vec<(i32, i32)> {
    xcap::Monitor::all()
        .map(|all| {
            all.iter()
                .map(|m| (m.x().unwrap_or(i32::MIN), m.y().unwrap_or(i32::MIN)))
                .collect()
        })
        .unwrap_or_default()
}

/// Enumerate monitors on the current display server using xrandr (X11) or
/// xcap (cross-platform fallback).
fn enumerate_monitors() -> Vec<MonitorInfo> {
    // Try xcap first as it's cross-platform and already a dependency
    if let Ok(monitors) = xcap::Monitor::all()
        && !monitors.is_empty()
    {
        return monitors
            .into_iter()
            .enumerate()
            .map(|(i, m)| MonitorInfo {
                index: i as u32,
                name: m.name().unwrap_or_else(|_| format!("Monitor {i}")),
                width: m.width().unwrap_or(1920),
                height: m.height().unwrap_or(1080),
                is_primary: i == 0, // Heuristic: first one is primary
            })
            .collect();
    }

    // Fallback to manual enumeration logic if xcap fails
    // (Existing logic preserved for robustness but returns MonitorInfo)
    let mut fallback = Vec::new();
    let count = get_monitor_count_fallback();
    for i in 0..count {
        fallback.push(MonitorInfo {
            index: i,
            name: format!("Monitor {i}"),
            width: 1920,
            height: 1080,
            is_primary: i == 0,
        });
    }
    fallback
}

fn get_monitor_count_fallback() -> u32 {
    // Try xrandr (most reliable on X11)
    if let Ok(output) = std::process::Command::new("xrandr")
        .args(["--listmonitors"])
        .output()
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Some(rest) = line.strip_prefix("Monitors: ") {
                if let Ok(count) = rest.trim().parse::<u32>() {
                    return count.max(1);
                }
                break;
            }
        }
    }
    1
}
