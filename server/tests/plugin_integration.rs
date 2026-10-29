//! Integration tests for KDE Connect plugin dispatch.
//!
//! Tests that each registered plugin correctly handles its expected
//! packet types and produces appropriate responses.

use pocketdesk_core::error::Result;
use std::sync::Arc;
use tokio::sync::Mutex;

use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket};

/// A mock sender that captures packets sent by plugins for verification.
#[derive(Clone)]
struct MockSender {
    sent: Arc<Mutex<Vec<NetworkPacket>>>,
}

impl MockSender {
    fn new() -> Self {
        Self {
            sent: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait::async_trait]
impl DeviceSender for MockSender {
    fn device_id(&self) -> &str {
        "mock-device"
    }

    fn connection_id(&self) -> &str {
        "mock-connection"
    }

    async fn send_packet(&self, packet: &NetworkPacket) -> Result<()> {
        let mut guard = self.sent.lock().await;
        guard.push(packet.clone());
        Ok(())
    }
}

fn build_test_registry() -> pocketdesk_server::plugins::PluginSet {
    pocketdesk_server::plugins::register_all()
}

#[tokio::test]
async fn test_battery_plugin() {
    let registry = build_test_registry();
    let sender = MockSender::new();
    let request = NetworkPacket::new("kdeconnect.battery.request");
    registry.dispatch_packet(&request, &sender).await.unwrap();
    let sent = sender.sent.lock().await;
    let battery_resp = sent.iter().find(|p| p.packet_type == "kdeconnect.battery");
    assert!(
        battery_resp.is_some(),
        "Battery plugin should respond with kdeconnect.battery"
    );
}

#[tokio::test]
async fn test_clipboard_plugin_set() {
    pocketdesk_core::local::paths::use_test_dirs("server-it");
    let registry = build_test_registry();
    let sender = MockSender::new();
    let request = NetworkPacket::new("kdeconnect.clipboard")
        .with_body(serde_json::json!({ "content": "hello world" }));
    registry.dispatch_packet(&request, &sender).await.unwrap();
    // Clipboard set should succeed without error (no response expected)
}

#[tokio::test]
async fn test_input_plugin_mouse() {
    let registry = build_test_registry();
    let sender = MockSender::new();
    let request = NetworkPacket::new("kdeconnect.mousepad.request")
        .with_body(serde_json::json!({ "dx": 100, "dy": 50 }));
    registry.dispatch_packet(&request, &sender).await.unwrap();
    // Input plugin echoes back on mouse events
    let sent = sender.sent.lock().await;
    let echo = sent
        .iter()
        .find(|p| p.packet_type == "kdeconnect.mousepad.echo");
    assert!(echo.is_some(), "Input plugin should echo mousepad events");
}

/// A link is its own packet now (`kdeconnect.pocketdesk.link`, T8a): a URL
/// share is no longer echoed back to the phone as a notification.
#[tokio::test]
async fn test_share_plugin_url_is_not_echoed() {
    let registry = build_test_registry();
    let sender = MockSender::new();
    let request = NetworkPacket::new("kdeconnect.share.request")
        .with_body(serde_json::json!({ "url": "https://example.com" }));
    let _ = registry.dispatch_packet(&request, &sender).await;
    let sent = sender.sent.lock().await;
    assert!(
        !sent
            .iter()
            .any(|p| p.packet_type == "kdeconnect.notification"),
        "a URL share is not echoed back as a notification"
    );
}

#[tokio::test]
async fn test_file_browse_plugin() {
    pocketdesk_core::local::paths::use_test_dirs("server-it");
    let registry = build_test_registry();
    let sender = MockSender::new();
    let request = NetworkPacket::new("kdeconnect.filebrowse.request")
        .with_body(serde_json::json!({ "path": "/tmp" }));
    registry.dispatch_packet(&request, &sender).await.unwrap();
    let sent = sender.sent.lock().await;
    let response = sent
        .iter()
        .find(|p| p.packet_type == "kdeconnect.filebrowse.response");
    assert!(
        response.is_some(),
        "File browse plugin should respond with filebrowse.response"
    );
}

#[tokio::test]
#[ignore]
async fn test_power_plugin() {
    let registry = build_test_registry();
    let sender = MockSender::new();
    // Power plugin should handle the request
    let request = NetworkPacket::new("kdeconnect.pocketdesk.power")
        .with_body(serde_json::json!({ "action": "shutdown" }));
    let result = registry.dispatch_packet(&request, &sender).await;
    // The action may fail in test env (no systemctl), but dispatch should not error
    assert!(result.is_ok(), "Power plugin dispatch should not error");
}

#[tokio::test]
async fn test_unknown_packet_type() {
    let registry = build_test_registry();
    let sender = MockSender::new();
    let request = NetworkPacket::new("kdeconnect.nonexistent");
    let result = registry.dispatch_packet(&request, &sender).await;
    assert!(result.is_ok(), "Unknown packet should not error");
}

#[tokio::test]
async fn test_clipboard_connect_request() {
    pocketdesk_core::local::paths::use_test_dirs("server-it");
    let registry = build_test_registry();
    let sender = MockSender::new();
    let request = NetworkPacket::new("kdeconnect.clipboard.connect");
    registry.dispatch_packet(&request, &sender).await.unwrap();
    // Should trigger a clipboard get — no specific response expected in mock
}

#[tokio::test]
async fn test_exec_plugin() {
    let registry = build_test_registry();
    let sender = MockSender::new();
    let request = NetworkPacket::new("kdeconnect.pocketdesk.exec")
        .with_body(serde_json::json!({ "command": "echo hello" }));
    let result = registry.dispatch_packet(&request, &sender).await;
    assert!(result.is_ok(), "Exec plugin should handle simple commands");
    let sent = sender.sent.lock().await;
    let response = sent
        .iter()
        .find(|p| p.packet_type == "kdeconnect.pocketdesk.exec");
    assert!(
        response.is_some(),
        "Exec plugin should respond with exec response"
    );
    if let Some(pkt) = response {
        let stdout = pkt.body.get("stdout").and_then(|v| v.as_str());
        // sh ends the line with LF, cmd.exe with CRLF.
        assert_eq!(stdout.map(str::trim_end), Some("hello"));
    }
}

/// Verify that all plugins register their expected capabilities.
#[test]
fn test_plugin_capabilities() {
    let registry = build_test_registry();
    let (incoming, _outgoing) = registry.capability_sets();

    assert!(
        incoming.contains(&"kdeconnect.battery.request".to_string()),
        "Battery plugin should register battery.request"
    );
    assert!(
        incoming.contains(&"kdeconnect.clipboard".to_string()),
        "Clipboard plugin should register clipboard"
    );
    assert!(
        incoming.contains(&"kdeconnect.mousepad.request".to_string()),
        "Input plugin should register mousepad.request"
    );
    assert!(
        incoming.contains(&"kdeconnect.share.request".to_string()),
        "Share plugin should register share.request"
    );
    assert!(
        incoming.contains(&"kdeconnect.filebrowse.request".to_string()),
        "File browse plugin should register filebrowse.request"
    );
    assert!(
        incoming.contains(&"kdeconnect.pocketdesk.power".to_string()),
        "Power plugin should register pocketdesk.power"
    );
    assert!(
        incoming.contains(&"kdeconnect.pocketdesk.exec".to_string()),
        "Exec plugin should register pocketdesk.exec"
    );
    assert!(
        incoming.contains(&"kdeconnect.pocketdesk.windows".to_string()),
        "Windows plugin should register pocketdesk.windows"
    );
}

/// R3#7: the windows plugin answers with the picker payload — an
/// `available` flag plus a `windows` array; on a Hyprland session every
/// entry must carry the fields the picker and crop math need.
#[tokio::test]
async fn test_windows_plugin_payload() {
    let registry = build_test_registry();
    let sender = MockSender::new();
    let request = NetworkPacket::new("kdeconnect.pocketdesk.windows");
    registry.dispatch_packet(&request, &sender).await.unwrap();
    let sent = sender.sent.lock().await;
    let resp = sent
        .iter()
        .find(|p| p.packet_type == "kdeconnect.pocketdesk.windows")
        .expect("Windows plugin should respond");

    assert!(
        resp.body
            .get("available")
            .and_then(|v| v.as_bool())
            .is_some()
    );
    let windows = resp
        .body
        .get("windows")
        .and_then(|v| v.as_array())
        .expect("windows must be an array");
    if resp.body["available"].as_bool().unwrap() {
        for w in windows {
            for field in ["address", "title", "class", "at", "size", "workspace"] {
                assert!(w.get(field).is_some(), "window entry missing {field}: {w}");
            }
        }
    }
}
