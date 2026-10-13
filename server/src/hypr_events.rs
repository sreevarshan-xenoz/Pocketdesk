//! Hyprland workspace-HUD event bridge (R3#8).
//!
//! Streams socket2 compositor events and pushes them to connected clients
//! as `kdeconnect.pocketdesk.hyprland.event` packets, plus a full
//! `kdeconnect.pocketdesk.hyprland.state` snapshot whenever a client
//! registers. The phone renders both into a workspace HUD and switches
//! workspaces through the existing Super+N input path (Hyprland's socket1
//! write dispatchers are broken upstream, so actions never ride IPC).

use std::sync::Arc;
use std::time::Duration;

use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket};

use crate::hyprland::{HyprEvent, HyprlandIpc};
use crate::plugins::windows::{entries, screen_box};

/// Packet type for a single Hyprland event (body: `{event, data}`).
pub const EVENT_PACKET: &str = "kdeconnect.pocketdesk.hyprland.event";

/// Packet type for the full workspace/window snapshot (sent on register).
pub const STATE_PACKET: &str = "kdeconnect.pocketdesk.hyprland.state";

/// Compositor events the HUD consumes. Everything socket2 emits
/// (`render`, `monitorremoved`, …) that is not listed here is filtered out.
/// Output changes are not HUD deltas — see [`is_monitor_event`].
pub fn is_hud_event(name: &str) -> bool {
    matches!(
        name,
        "activewindow"
            | "workspace"
            | "openwindow"
            | "closewindow"
            | "movewindow"
            | "focuswindow"
            | "createworkspace"
            | "destroyworkspace"
            | "renameworkspace"
    )
}

/// Compositor events that mean the layout of outputs changed.
///
/// These are *not* HUD events: a monitor going away changes the geometry of
/// every window and the screen box, which is the snapshot's job, so they trigger
/// a re-snapshot rather than being forwarded as a delta (roadmap 2066). The
/// names are the ones this Hyprland's socket2 emits — both strings exist in the
/// compositor's own binary.
pub fn is_monitor_event(name: &str) -> bool {
    matches!(name, "monitoradded" | "monitorremoved")
}

/// Wrap a compositor event into a push packet, or `None` if irrelevant.
pub fn event_packet(ev: &HyprEvent) -> Option<NetworkPacket> {
    if !is_hud_event(&ev.name) {
        return None;
    }
    Some(
        NetworkPacket::new(EVENT_PACKET).with_body(serde_json::json!({
            "event": ev.name,
            "data": ev.data,
        })),
    )
}

/// Full HUD state: workspaces, visible windows (with the geometry a tapped
/// chip needs to crop the stream), and the focused window.
pub async fn state_packet(ipc: &HyprlandIpc) -> Option<NetworkPacket> {
    let workspaces = ipc.workspaces().await.ok()?;
    let clients = ipc.visible_windows().await.unwrap_or_default();
    let active = ipc.active_window().await.ok().flatten();
    // Best-effort: without the monitor table windows still render, they just
    // lose crop geometry (single-monitor offset 0,0 is what entries() falls
    // back to). Same tolerance the windows plugin response uses.
    let monitors = ipc.monitors().await.unwrap_or_default();
    Some(state_packet_from(&workspaces, clients, active, &monitors))
}

/// The HUD state packet from an already-taken snapshot — shared with the
/// Windows desktop source, which has windows and monitors but no workspaces.
pub fn state_packet_from(
    workspaces: &[crate::hyprland::HyprWorkspace],
    clients: Vec<crate::hyprland::HyprWindow>,
    active: Option<crate::hyprland::HyprWindow>,
    monitors: &[crate::hyprland::HyprMonitor],
) -> NetworkPacket {
    {
        NetworkPacket::new(STATE_PACKET).with_body(serde_json::json!({
            "workspaces": workspaces
                .iter()
                .map(|w| serde_json::json!({"id": w.id, "name": w.name, "active": w.active}))
                .collect::<Vec<_>>(),
            "activeWorkspace": workspaces.iter().find(|w| w.active).map(|w| w.id),
            "windows": entries(clients, monitors),
            "screen": screen_box(monitors),
            "activeAddress": active.as_ref().map(|a| a.address.clone()),
            "activeTitle": active.as_ref().map(|a| a.title.clone()),
        }))
    }
}

/// Send the current snapshot to one freshly-registered client. Best-effort.
pub async fn push_state_to(sender: &Arc<dyn DeviceSender>, ipc: &HyprlandIpc) {
    if let Some(packet) = state_packet(ipc).await {
        let _ = sender.send_packet(&packet).await;
    }
}

/// Snapshot the compositor and hand the new state to every connected client.
///
/// Both callers are about staleness: the periodic tick exists because a
/// register-time push can fall into the client's packet-poll gap, and the
/// output-change branch because a monitor being added or removed invalidates
/// every geometry the snapshot carries, which must not wait up to a tick to
/// become visible (roadmap 2066).
pub async fn push_state_to_clients(ipc: &HyprlandIpc) {
    let clients = crate::state::clone_clients().await;
    if clients.is_empty() {
        return;
    }
    let Some(packet) = state_packet(ipc).await else {
        return;
    };
    for sender in clients.iter() {
        let _ = sender.send_packet(&packet).await;
    }
}

/// Spawn the socket2 reader with reconnect and fan events out on a
/// broadcast channel. The loop is cheap when Hyprland goes away (a failed
/// unix-socket connect per retry) so it self-heals on compositor restart
/// within the same session.
pub fn start_hypr_event_monitor(ipc: HyprlandIpc) -> tokio::sync::broadcast::Sender<HyprEvent> {
    let (tx, _) = tokio::sync::broadcast::channel::<HyprEvent>(128);
    let monitor_tx = tx.clone();
    tokio::spawn(async move {
        run_monitor(ipc, monitor_tx).await;
    });
    tx
}

async fn run_monitor(ipc: HyprlandIpc, tx: tokio::sync::broadcast::Sender<HyprEvent>) {
    loop {
        match ipc.subscribe_events().await {
            Ok(mut rx) => {
                tracing::info!("Hyprland event monitor started");
                while let Some(ev) = rx.recv().await {
                    let _ = tx.send(ev);
                }
                tracing::warn!("Hyprland event stream ended; resubscribing");
            }
            Err(e) => {
                tracing::debug!("Hyprland event socket unavailable: {e}");
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// The Windows desktop's HUD state: windows and monitors, no workspaces.
#[cfg(windows)]
pub async fn windows_state_packet() -> NetworkPacket {
    let (windows, active, monitors) =
        tokio::task::spawn_blocking(crate::windows_desktop::visible_windows)
            .await
            .unwrap_or_default();
    state_packet_from(&[], windows, active, &monitors)
}

/// Keep every client's HUD current on Windows: snapshot once a second and
/// push when anything changed (focus, a window opening/closing/moving, a
/// display coming or going), plus the same 4 s refresh the Hyprland path
/// sends for clients that missed a push.
#[cfg(windows)]
pub fn start_windows_hud() {
    tokio::spawn(async {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_body = String::new();
        let mut last_push = std::time::Instant::now();
        loop {
            tick.tick().await;
            let clients = crate::state::clone_clients().await;
            if clients.is_empty() {
                continue;
            }
            let packet = windows_state_packet().await;
            let body = packet.body.to_string();
            if body == last_body && last_push.elapsed() < Duration::from_secs(4) {
                continue;
            }
            last_body = body;
            last_push = std::time::Instant::now();
            for sender in clients.iter() {
                let _ = sender.send_packet(&packet).await;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hud_events_are_whitelisted() {
        assert!(is_hud_event("activewindow"));
        assert!(is_hud_event("workspace"));
        assert!(is_hud_event("renameworkspace"));
        assert!(!is_hud_event("render"));
        assert!(!is_hud_event("monitorremoved"));
    }

    #[test]
    fn monitor_events_drive_a_snapshot_rather_than_a_delta() {
        assert!(is_monitor_event("monitoradded"));
        assert!(is_monitor_event("monitorremoved"));
        assert!(!is_monitor_event("activewindow"));
        assert!(!is_monitor_event("render"));
        // The refresh rides the state packet, so an output event must NOT also
        // reach the phone as a HUD delta it has no handler for.
        let ev = HyprEvent::parse("monitorremoved>>1").expect("monitorremoved parses");
        assert!(!is_hud_event(&ev.name));
        assert!(event_packet(&ev).is_none());
    }

    #[test]
    fn event_packet_wraps_name_and_data() {
        let ev = HyprEvent::parse("activewindow>>brave-browser,Some, Title").unwrap();
        let packet = event_packet(&ev).expect("activewindow is pushed");
        assert_eq!(packet.packet_type, EVENT_PACKET);
        assert_eq!(packet.body["event"], "activewindow");
        assert_eq!(packet.body["data"], "brave-browser,Some, Title");
        assert!(event_packet(&HyprEvent::parse("render>>").unwrap()).is_none());
    }

    /// E1: the state snapshot's windows must carry the geometry a HUD chip
    /// taps to crop — verified live, skipped on non-Hyprland hosts.
    #[tokio::test]
    async fn state_packet_windows_carry_crop_geometry() {
        let Ok(ipc) = HyprlandIpc::from_env() else {
            eprintln!("no Hyprland IPC here, skipping");
            return;
        };
        let Some(packet) = state_packet(&ipc).await else {
            eprintln!("no workspaces here, skipping");
            return;
        };
        assert_eq!(packet.packet_type, STATE_PACKET);
        assert!(
            packet.body.get("screen").is_some(),
            "snapshot carries the layout box"
        );
        let windows = packet.body["windows"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for w in &windows {
            assert!(w.get("local_at").is_some(), "window has crop origin: {w}");
            assert!(w.get("size").is_some(), "window has size: {w}");
            assert!(w["address"].is_string(), "window has address: {w}");
        }
    }
}
