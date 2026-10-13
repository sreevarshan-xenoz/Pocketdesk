//! Integration tests for the KDE Connect packet loop.
//!
//! These tests exercise the full packet handling flow through `KdeConnectService`,
//! using a mock `DeviceSender` that captures outgoing packets for verification.

#[cfg(test)]
mod tests {
    use super::super::kdeconnect::{
        DeviceSender, KdeConnectService, NetworkPacket, Plugin, PluginRegistry,
    };
    use crate::error::Result;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    // -----------------------------------------------------------------------
    // MockDeviceSender – captures every packet sent via `send_packet`
    // -----------------------------------------------------------------------

    /// A test double for `DeviceSender` that records all sent packets in memory.
    struct MockDeviceSender {
        packets: Arc<Mutex<Vec<NetworkPacket>>>,
    }

    impl MockDeviceSender {
        fn new() -> Self {
            Self {
                packets: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn packets_arc(&self) -> Arc<Mutex<Vec<NetworkPacket>>> {
            self.packets.clone()
        }
    }

    #[async_trait::async_trait]
    impl DeviceSender for MockDeviceSender {
        fn device_id(&self) -> &str {
            "mock-device"
        }

        fn connection_id(&self) -> &str {
            "mock-connection"
        }

        async fn send_packet(&self, packet: &NetworkPacket) -> Result<()> {
            self.packets.lock().await.push(packet.clone());
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Helper: drain captured packets into a Vec for assertions
    // -----------------------------------------------------------------------

    async fn captured_packets(packets: &Arc<Mutex<Vec<NetworkPacket>>>) -> Vec<NetworkPacket> {
        packets.lock().await.clone()
    }

    // -----------------------------------------------------------------------
    // BatteryPlugin integration tests
    // -----------------------------------------------------------------------

    mod battery_plugin {
        use super::*;

        /// Minimal battery plugin that responds to kdeconnect.battery.request
        /// with a fixed battery state (avoids UPower dependency in tests).
        struct TestBatteryPlugin;

        #[async_trait::async_trait]
        impl Plugin for TestBatteryPlugin {
            fn name(&self) -> &'static str {
                "battery"
            }

            fn incoming_capabilities(&self) -> &'static [&'static str] {
                &["kdeconnect.battery.request"]
            }

            fn outgoing_capabilities(&self) -> &'static [&'static str] {
                &["kdeconnect.battery"]
            }

            async fn handle_packet(
                &self,
                packet: &NetworkPacket,
                sender: &dyn DeviceSender,
            ) -> Result<()> {
                if packet.packet_type == "kdeconnect.battery.request" {
                    let response = NetworkPacket::new("kdeconnect.battery").with_body(json!({
                        "currentCharge": 75u8,
                        "isCharging": false,
                    }));
                    sender.send_packet(&response).await?;
                }
                Ok(())
            }
        }

        #[tokio::test]
        async fn battery_request_produces_battery_response() {
            let sender = MockDeviceSender::new();
            let packets = sender.packets_arc();

            let mut service = KdeConnectService::new();
            service.register_plugin(TestBatteryPlugin);

            let request = NetworkPacket::new("kdeconnect.battery.request");
            service
                .registry
                .dispatch_packet(&request, &sender)
                .await
                .expect("dispatch should succeed");

            let sent = captured_packets(&packets).await;
            assert_eq!(sent.len(), 1, "expected exactly one packet sent");
            assert_eq!(sent[0].packet_type, "kdeconnect.battery");
            assert_eq!(
                sent[0].body.get("currentCharge").and_then(|v| v.as_u64()),
                Some(75)
            );
            assert_eq!(
                sent[0].body.get("isCharging").and_then(|v| v.as_bool()),
                Some(false)
            );
        }

        #[tokio::test]
        async fn battery_plugin_ignores_unrelated_packets() {
            let sender = MockDeviceSender::new();
            let packets = sender.packets_arc();

            let mut service = KdeConnectService::new();
            service.register_plugin(TestBatteryPlugin);

            // Send a non-battery packet
            let request = NetworkPacket::new("kdeconnect.clipboard");
            service
                .registry
                .dispatch_packet(&request, &sender)
                .await
                .expect("dispatch should succeed");

            let sent = captured_packets(&packets).await;
            assert!(
                sent.is_empty(),
                "battery plugin should not respond to clipboard packets"
            );
        }
    }

    // -----------------------------------------------------------------------
    // ClipboardPlugin integration tests
    // -----------------------------------------------------------------------

    mod clipboard_plugin {
        use super::*;

        /// Test clipboard plugin that echoes content back on connect request.
        struct TestClipboardPlugin;

        #[async_trait::async_trait]
        impl Plugin for TestClipboardPlugin {
            fn name(&self) -> &'static str {
                "clipboard"
            }

            fn incoming_capabilities(&self) -> &'static [&'static str] {
                &["kdeconnect.clipboard", "kdeconnect.clipboard.connect"]
            }

            fn outgoing_capabilities(&self) -> &'static [&'static str] {
                &["kdeconnect.clipboard"]
            }

            async fn handle_packet(
                &self,
                packet: &NetworkPacket,
                sender: &dyn DeviceSender,
            ) -> Result<()> {
                match packet.packet_type.as_str() {
                    "kdeconnect.clipboard.connect" => {
                        let response =
                            NetworkPacket::new("kdeconnect.clipboard").with_body(json!({
                                "content": "test clipboard content",
                            }));
                        sender.send_packet(&response).await?;
                    }
                    "kdeconnect.clipboard" => {
                        // Received remote clipboard update – no response needed
                    }
                    _ => {}
                }
                Ok(())
            }
        }

        #[tokio::test]
        async fn clipboard_connect_produces_clipboard_response() {
            let sender = MockDeviceSender::new();
            let packets = sender.packets_arc();

            let mut service = KdeConnectService::new();
            service.register_plugin(TestClipboardPlugin);

            let request = NetworkPacket::new("kdeconnect.clipboard.connect");
            service
                .registry
                .dispatch_packet(&request, &sender)
                .await
                .expect("dispatch should succeed");

            let sent = captured_packets(&packets).await;
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].packet_type, "kdeconnect.clipboard");
            assert_eq!(
                sent[0].body.get("content").and_then(|v| v.as_str()),
                Some("test clipboard content")
            );
        }

        #[tokio::test]
        async fn clipboard_update_does_not_respond() {
            let sender = MockDeviceSender::new();
            let packets = sender.packets_arc();

            let mut service = KdeConnectService::new();
            service.register_plugin(TestClipboardPlugin);

            // A plain clipboard update (remote pushed content) should not trigger a response
            let request = NetworkPacket::new("kdeconnect.clipboard")
                .with_body(json!({ "content": "remote text" }));
            service
                .registry
                .dispatch_packet(&request, &sender)
                .await
                .expect("dispatch should succeed");

            let sent = captured_packets(&packets).await;
            assert!(
                sent.is_empty(),
                "clipboard update should not produce a response"
            );
        }
    }

    // -----------------------------------------------------------------------
    // InputPlugin integration tests
    // -----------------------------------------------------------------------

    mod input_plugin {
        use super::*;

        /// Test input plugin that echoes back on mousepad requests.
        struct TestInputPlugin;

        #[async_trait::async_trait]
        impl Plugin for TestInputPlugin {
            fn name(&self) -> &'static str {
                "input"
            }

            fn incoming_capabilities(&self) -> &'static [&'static str] {
                &["kdeconnect.mousepad.request", "kdeconnect.presenter"]
            }

            fn outgoing_capabilities(&self) -> &'static [&'static str] {
                &["kdeconnect.mousepad.echo"]
            }

            async fn handle_packet(
                &self,
                packet: &NetworkPacket,
                sender: &dyn DeviceSender,
            ) -> Result<()> {
                match packet.packet_type.as_str() {
                    "kdeconnect.mousepad.request" => {
                        let echo = NetworkPacket::new("kdeconnect.mousepad.echo");
                        sender.send_packet(&echo).await?;
                    }
                    "kdeconnect.presenter" => {
                        // Handle presenter actions (no response in test)
                    }
                    _ => {}
                }
                Ok(())
            }
        }

        #[tokio::test]
        async fn mousepad_request_produces_echo_response() {
            let sender = MockDeviceSender::new();
            let packets = sender.packets_arc();

            let mut service = KdeConnectService::new();
            service.register_plugin(TestInputPlugin);

            let request = NetworkPacket::new("kdeconnect.mousepad.request").with_body(json!({
                "dx": 10.0,
                "dy": -5.0,
                "isPressed": true,
                "button": 1,
            }));
            service
                .registry
                .dispatch_packet(&request, &sender)
                .await
                .expect("dispatch should succeed");

            let sent = captured_packets(&packets).await;
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].packet_type, "kdeconnect.mousepad.echo");
        }

        #[tokio::test]
        async fn mousepad_keyboard_input_produces_echo() {
            let sender = MockDeviceSender::new();
            let packets = sender.packets_arc();

            let mut service = KdeConnectService::new();
            service.register_plugin(TestInputPlugin);

            let request = NetworkPacket::new("kdeconnect.mousepad.request").with_body(json!({
                "text": "Hello World",
                "key": "Enter",
            }));
            service
                .registry
                .dispatch_packet(&request, &sender)
                .await
                .expect("dispatch should succeed");

            let sent = captured_packets(&packets).await;
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].packet_type, "kdeconnect.mousepad.echo");
        }

        #[tokio::test]
        async fn input_plugin_ignores_unrelated_packets() {
            let sender = MockDeviceSender::new();
            let packets = sender.packets_arc();

            let mut service = KdeConnectService::new();
            service.register_plugin(TestInputPlugin);

            let request = NetworkPacket::new("kdeconnect.battery.request");
            service
                .registry
                .dispatch_packet(&request, &sender)
                .await
                .expect("dispatch should succeed");

            let sent = captured_packets(&packets).await;
            assert!(
                sent.is_empty(),
                "input plugin should not respond to battery packets"
            );
        }
    }

    // -----------------------------------------------------------------------
    // NotificationPlugin integration tests
    // -----------------------------------------------------------------------

    mod notification_plugin {
        use super::*;

        /// Test notification plugin that acknowledges request packets.
        struct TestNotificationPlugin;

        #[async_trait::async_trait]
        impl Plugin for TestNotificationPlugin {
            fn name(&self) -> &'static str {
                "notification"
            }

            fn incoming_capabilities(&self) -> &'static [&'static str] {
                &["kdeconnect.notification", "kdeconnect.notification.request"]
            }

            fn outgoing_capabilities(&self) -> &'static [&'static str] {
                &["kdeconnect.notification"]
            }

            async fn handle_packet(
                &self,
                packet: &NetworkPacket,
                sender: &dyn DeviceSender,
            ) -> Result<()> {
                match packet.packet_type.as_str() {
                    "kdeconnect.notification.request" => {
                        let response = NetworkPacket::new("kdeconnect.notification");
                        sender.send_packet(&response).await?;
                    }
                    "kdeconnect.notification" => {
                        // Show notification (no response needed in test)
                    }
                    _ => {}
                }
                Ok(())
            }
        }

        #[tokio::test]
        async fn notification_request_produces_notification_response() {
            let sender = MockDeviceSender::new();
            let packets = sender.packets_arc();

            let mut service = KdeConnectService::new();
            service.register_plugin(TestNotificationPlugin);

            let request = NetworkPacket::new("kdeconnect.notification.request");
            service
                .registry
                .dispatch_packet(&request, &sender)
                .await
                .expect("dispatch should succeed");

            let sent = captured_packets(&packets).await;
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].packet_type, "kdeconnect.notification");
        }

        #[tokio::test]
        async fn notification_update_does_not_respond() {
            let sender = MockDeviceSender::new();
            let packets = sender.packets_arc();

            let mut service = KdeConnectService::new();
            service.register_plugin(TestNotificationPlugin);

            let request = NetworkPacket::new("kdeconnect.notification").with_body(json!({
                "title": "Test",
                "text": "Body",
                "app": "TestApp",
            }));
            service
                .registry
                .dispatch_packet(&request, &sender)
                .await
                .expect("dispatch should succeed");

            let sent = captured_packets(&packets).await;
            assert!(
                sent.is_empty(),
                "notification update should not produce a response"
            );
        }
    }

    // -----------------------------------------------------------------------
    // SharePlugin integration tests
    // -----------------------------------------------------------------------

    mod share_plugin {
        use super::*;

        /// Test share plugin that responds with a file URL on share request.
        struct TestSharePlugin;

        #[async_trait::async_trait]
        impl Plugin for TestSharePlugin {
            fn name(&self) -> &'static str {
                "share"
            }

            fn incoming_capabilities(&self) -> &'static [&'static str] {
                &["kdeconnect.share.request"]
            }

            fn outgoing_capabilities(&self) -> &'static [&'static str] {
                &["kdeconnect.share.request"]
            }

            async fn handle_packet(
                &self,
                packet: &NetworkPacket,
                sender: &dyn DeviceSender,
            ) -> Result<()> {
                if packet.packet_type == "kdeconnect.share.request" {
                    // If the packet contains a URL, respond with a notification
                    if let Some(url) = packet.body.get("url").and_then(|v| v.as_str()) {
                        let notification =
                            NetworkPacket::new("kdeconnect.notification").with_body(json!({
                                "title": "URL Received",
                                "text": url,
                                "app": "Pocketdesk",
                            }));
                        sender.send_packet(&notification).await?;
                    }
                    // If there's no payloadTransferInfo and no url, no response
                }
                Ok(())
            }
        }

        #[tokio::test]
        async fn share_url_request_produces_notification_response() {
            let sender = MockDeviceSender::new();
            let packets = sender.packets_arc();

            let mut service = KdeConnectService::new();
            service.register_plugin(TestSharePlugin);

            let request = NetworkPacket::new("kdeconnect.share.request").with_body(json!({
                "url": "https://example.com/shared-file.txt",
                "filename": "shared-file.txt",
            }));
            service
                .registry
                .dispatch_packet(&request, &sender)
                .await
                .expect("dispatch should succeed");

            let sent = captured_packets(&packets).await;
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].packet_type, "kdeconnect.notification");
            assert_eq!(
                sent[0].body.get("title").and_then(|v| v.as_str()),
                Some("URL Received")
            );
            assert_eq!(
                sent[0].body.get("text").and_then(|v| v.as_str()),
                Some("https://example.com/shared-file.txt")
            );
        }

        #[tokio::test]
        async fn share_file_request_without_url_does_not_respond() {
            let sender = MockDeviceSender::new();
            let packets = sender.packets_arc();

            let mut service = KdeConnectService::new();
            service.register_plugin(TestSharePlugin);

            // A share request with port info but no URL spawns a background task
            // and does not send an immediate response
            let request = NetworkPacket::new("kdeconnect.share.request")
                .with_body(json!({
                    "filename": "photo.jpg",
                    "payloadTransferInfo": {
                        "port": 12345u64
                    }
                }))
                .with_payload_size(1024u64);
            service
                .registry
                .dispatch_packet(&request, &sender)
                .await
                .expect("dispatch should succeed");

            // Give any spawned tasks a moment, then check
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let sent = captured_packets(&packets).await;
            // No immediate response expected (file transfer happens in background)
            assert!(
                sent.is_empty(),
                "share file request should not produce an immediate response packet"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Capability registration tests
    // -----------------------------------------------------------------------

    mod capability_registration {
        use super::*;

        #[test]
        fn registry_reports_combined_capabilities() {
            struct PluginA;
            #[async_trait::async_trait]
            impl Plugin for PluginA {
                fn name(&self) -> &'static str {
                    "plugin_a"
                }
                fn incoming_capabilities(&self) -> &'static [&'static str] {
                    &["cap.a.in"]
                }
                fn outgoing_capabilities(&self) -> &'static [&'static str] {
                    &["cap.a.out"]
                }
                async fn handle_packet(
                    &self,
                    _packet: &NetworkPacket,
                    _sender: &dyn DeviceSender,
                ) -> Result<()> {
                    Ok(())
                }
            }

            struct PluginB;
            #[async_trait::async_trait]
            impl Plugin for PluginB {
                fn name(&self) -> &'static str {
                    "plugin_b"
                }
                fn incoming_capabilities(&self) -> &'static [&'static str] {
                    &["cap.b.in"]
                }
                fn outgoing_capabilities(&self) -> &'static [&'static str] {
                    &["cap.b.out"]
                }
                async fn handle_packet(
                    &self,
                    _packet: &NetworkPacket,
                    _sender: &dyn DeviceSender,
                ) -> Result<()> {
                    Ok(())
                }
            }

            let mut registry = PluginRegistry::new();
            registry.register(PluginA);
            registry.register(PluginB);

            let (incoming, outgoing) = registry.capability_sets();
            assert_eq!(
                incoming,
                vec!["cap.a.in".to_string(), "cap.b.in".to_string()]
            );
            assert_eq!(
                outgoing,
                vec!["cap.a.out".to_string(), "cap.b.out".to_string()]
            );
        }

        #[test]
        fn registry_deduplicates_capabilities() {
            struct PluginA;
            #[async_trait::async_trait]
            impl Plugin for PluginA {
                fn name(&self) -> &'static str {
                    "plugin_a"
                }
                fn incoming_capabilities(&self) -> &'static [&'static str] {
                    &["shared.cap"]
                }
                fn outgoing_capabilities(&self) -> &'static [&'static str] {
                    &["shared.out"]
                }
                async fn handle_packet(
                    &self,
                    _packet: &NetworkPacket,
                    _sender: &dyn DeviceSender,
                ) -> Result<()> {
                    Ok(())
                }
            }

            struct PluginB;
            #[async_trait::async_trait]
            impl Plugin for PluginB {
                fn name(&self) -> &'static str {
                    "plugin_b"
                }
                fn incoming_capabilities(&self) -> &'static [&'static str] {
                    &["shared.cap"]
                }
                fn outgoing_capabilities(&self) -> &'static [&'static str] {
                    &["shared.out"]
                }
                async fn handle_packet(
                    &self,
                    _packet: &NetworkPacket,
                    _sender: &dyn DeviceSender,
                ) -> Result<()> {
                    Ok(())
                }
            }

            let mut registry = PluginRegistry::new();
            registry.register(PluginA);
            registry.register(PluginB);

            let (incoming, outgoing) = registry.capability_sets();
            assert_eq!(incoming, vec!["shared.cap".to_string()]);
            assert_eq!(outgoing, vec!["shared.out".to_string()]);
        }

        #[test]
        fn service_exposes_capability_sets() {
            struct CapPlugin;
            #[async_trait::async_trait]
            impl Plugin for CapPlugin {
                fn name(&self) -> &'static str {
                    "cap_plugin"
                }
                fn incoming_capabilities(&self) -> &'static [&'static str] {
                    &["cap.in"]
                }
                fn outgoing_capabilities(&self) -> &'static [&'static str] {
                    &["cap.out"]
                }
                async fn handle_packet(
                    &self,
                    _packet: &NetworkPacket,
                    _sender: &dyn DeviceSender,
                ) -> Result<()> {
                    Ok(())
                }
            }

            let mut service = KdeConnectService::new();
            service.register_plugin(CapPlugin);

            let (incoming, outgoing) = service.registry.capability_sets();
            assert_eq!(incoming, vec!["cap.in".to_string()]);
            assert_eq!(outgoing, vec!["cap.out".to_string()]);
        }
    }

    // -----------------------------------------------------------------------
    // Multi-plugin routing tests
    // -----------------------------------------------------------------------

    mod multi_plugin_routing {
        use super::*;

        /// A plugin that records when it receives a packet (via shared counter).
        struct RecordingPlugin {
            call_count: Arc<Mutex<u32>>,
            packet_type_filter: String,
            plugin_name: &'static str,
        }

        #[async_trait::async_trait]
        impl Plugin for RecordingPlugin {
            fn name(&self) -> &'static str {
                self.plugin_name
            }

            fn incoming_capabilities(&self) -> &'static [&'static str] {
                match self.packet_type_filter.as_str() {
                    "kdeconnect.alpha" => &["kdeconnect.alpha"],
                    "kdeconnect.beta" => &["kdeconnect.beta"],
                    "kdeconnect.known" => &["kdeconnect.known"],
                    "kdeconnect.shared" => &["kdeconnect.shared"],
                    _ => &[],
                }
            }

            fn outgoing_capabilities(&self) -> &'static [&'static str] {
                &[]
            }

            async fn handle_packet(
                &self,
                packet: &NetworkPacket,
                _sender: &dyn DeviceSender,
            ) -> Result<()> {
                if packet.packet_type == self.packet_type_filter {
                    let mut count = self.call_count.lock().await;
                    *count += 1;
                }
                Ok(())
            }
        }

        #[tokio::test]
        async fn packet_routed_to_correct_plugin_only() {
            let counter_a = Arc::new(Mutex::new(0u32));
            let counter_b = Arc::new(Mutex::new(0u32));

            let mut registry = PluginRegistry::new();
            registry.register(RecordingPlugin {
                call_count: counter_a.clone(),
                packet_type_filter: "kdeconnect.alpha".to_string(),
                plugin_name: "recorder_a",
            });
            registry.register(RecordingPlugin {
                call_count: counter_b.clone(),
                packet_type_filter: "kdeconnect.beta".to_string(),
                plugin_name: "recorder_b",
            });

            let sender = MockDeviceSender::new();

            // Send an alpha packet
            let alpha_packet = NetworkPacket::new("kdeconnect.alpha");
            registry
                .dispatch_packet(&alpha_packet, &sender)
                .await
                .expect("dispatch should succeed");

            assert_eq!(*counter_a.lock().await, 1);
            assert_eq!(*counter_b.lock().await, 0);

            // Send a beta packet
            let beta_packet = NetworkPacket::new("kdeconnect.beta");
            registry
                .dispatch_packet(&beta_packet, &sender)
                .await
                .expect("dispatch should succeed");

            assert_eq!(*counter_a.lock().await, 1);
            assert_eq!(*counter_b.lock().await, 1);
        }

        #[tokio::test]
        async fn unknown_packet_type_is_not_dispatched() {
            let counter = Arc::new(Mutex::new(0u32));

            let mut registry = PluginRegistry::new();
            registry.register(RecordingPlugin {
                call_count: counter.clone(),
                packet_type_filter: "kdeconnect.known".to_string(),
                plugin_name: "recorder_known",
            });

            let sender = MockDeviceSender::new();

            let unknown_packet = NetworkPacket::new("kdeconnect.unknown.type");
            registry
                .dispatch_packet(&unknown_packet, &sender)
                .await
                .expect("dispatch should succeed");

            assert_eq!(*counter.lock().await, 0);
        }

        #[tokio::test]
        async fn multiple_plugins_can_handle_same_packet_type() {
            let counter_a = Arc::new(Mutex::new(0u32));
            let counter_b = Arc::new(Mutex::new(0u32));

            let mut registry = PluginRegistry::new();
            // Both plugins register for the same capability
            registry.register(RecordingPlugin {
                call_count: counter_a.clone(),
                packet_type_filter: "kdeconnect.shared".to_string(),
                plugin_name: "recorder_shared_a",
            });
            registry.register(RecordingPlugin {
                call_count: counter_b.clone(),
                packet_type_filter: "kdeconnect.shared".to_string(),
                plugin_name: "recorder_shared_b",
            });

            let sender = MockDeviceSender::new();

            let packet = NetworkPacket::new("kdeconnect.shared");
            registry
                .dispatch_packet(&packet, &sender)
                .await
                .expect("dispatch should succeed");

            // Both plugins should have been called
            assert_eq!(*counter_a.lock().await, 1);
            assert_eq!(*counter_b.lock().await, 1);
        }
    }

    // -----------------------------------------------------------------------
    // Full service-level integration test
    // -----------------------------------------------------------------------

    mod service_integration {
        use super::*;

        /// A comprehensive plugin that exercises multiple packet types.
        struct MultiCapPlugin;

        #[async_trait::async_trait]
        impl Plugin for MultiCapPlugin {
            fn name(&self) -> &'static str {
                "multi_cap"
            }

            fn incoming_capabilities(&self) -> &'static [&'static str] {
                &[
                    "kdeconnect.battery.request",
                    "kdeconnect.clipboard.connect",
                    "kdeconnect.mousepad.request",
                ]
            }

            fn outgoing_capabilities(&self) -> &'static [&'static str] {
                &[
                    "kdeconnect.battery",
                    "kdeconnect.clipboard",
                    "kdeconnect.mousepad.echo",
                ]
            }

            async fn handle_packet(
                &self,
                packet: &NetworkPacket,
                sender: &dyn DeviceSender,
            ) -> Result<()> {
                match packet.packet_type.as_str() {
                    "kdeconnect.battery.request" => {
                        let resp = NetworkPacket::new("kdeconnect.battery").with_body(json!({
                            "currentCharge": 50u8,
                            "isCharging": true,
                        }));
                        sender.send_packet(&resp).await?;
                    }
                    "kdeconnect.clipboard.connect" => {
                        let resp = NetworkPacket::new("kdeconnect.clipboard")
                            .with_body(json!({ "content": "hello" }));
                        sender.send_packet(&resp).await?;
                    }
                    "kdeconnect.mousepad.request" => {
                        let resp = NetworkPacket::new("kdeconnect.mousepad.echo");
                        sender.send_packet(&resp).await?;
                    }
                    _ => {}
                }
                Ok(())
            }
        }

        #[tokio::test]
        async fn service_dispatches_multiple_packet_types() {
            let sender = MockDeviceSender::new();
            let packets = sender.packets_arc();

            let mut service = KdeConnectService::new();
            service.register_plugin(MultiCapPlugin);

            // Battery request
            service
                .registry
                .dispatch_packet(&NetworkPacket::new("kdeconnect.battery.request"), &sender)
                .await
                .unwrap();

            // Clipboard connect
            service
                .registry
                .dispatch_packet(&NetworkPacket::new("kdeconnect.clipboard.connect"), &sender)
                .await
                .unwrap();

            // Mousepad request
            service
                .registry
                .dispatch_packet(&NetworkPacket::new("kdeconnect.mousepad.request"), &sender)
                .await
                .unwrap();

            let sent = captured_packets(&packets).await;
            assert_eq!(sent.len(), 3);

            let types: Vec<&str> = sent.iter().map(|p| p.packet_type.as_str()).collect();
            assert_eq!(
                types,
                vec![
                    "kdeconnect.battery",
                    "kdeconnect.clipboard",
                    "kdeconnect.mousepad.echo"
                ]
            );
        }

        #[tokio::test]
        async fn service_identity_packet_is_available() {
            use crate::protocol::kdeconnect::DeviceIdentity;

            let mut service = KdeConnectService::new();
            let identity = DeviceIdentity::new("test-device-id", "Test Device");
            service.set_identity(identity);

            let identity_packet = service.identity_packet();
            assert!(identity_packet.is_some());
            let pkt = identity_packet.unwrap();
            assert_eq!(pkt.packet_type, "kdeconnect.identity");
            assert_eq!(
                pkt.body.get("deviceId").and_then(|v| v.as_str()),
                Some("test-device-id")
            );
            assert_eq!(
                pkt.body.get("deviceName").and_then(|v| v.as_str()),
                Some("Test Device")
            );
        }
    }
}
