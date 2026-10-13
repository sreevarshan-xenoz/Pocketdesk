//! Integration tests for the KDE Connect protocol over TCP.
//!
//! Spins up a mock server that speaks TLS, then the LINUX_LINK_HELLO
//! handshake, and responds to KDE Connect packets. Tests the full client-side flow
//! from ConnectionManager through to NetworkPacket dispatch.

use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use linux_link_core::protocol::connection::ConnectionManager;
use linux_link_core::protocol::kdeconnect::{
    DeviceIdentity, DeviceSender, NetworkPacket, TcpDeviceSender,
};
use linux_link_core::protocol::{HANDSHAKE_HELLO, HANDSHAKE_OK};
use linux_link_core::streaming::transport::CertManager;

/// Accept one connection on `listener` and run the desktop's side of TLS.
async fn accept_tls(
    listener: &TcpListener,
) -> std::io::Result<tokio_rustls::server::TlsStream<tokio::net::TcpStream>> {
    let (stream, _) = listener.accept().await?;
    tokio_rustls::TlsAcceptor::from(desktop().control_server_config().unwrap())
        .accept(stream)
        .await
}

/// One desktop identity for every mock server, as a real desktop keeps one:
/// a phone's pin for 127.0.0.1 then holds across connections.
fn desktop() -> &'static CertManager {
    static DESKTOP: std::sync::OnceLock<CertManager> = std::sync::OnceLock::new();
    DESKTOP.get_or_init(|| CertManager::new().expect("desktop certificate"))
}

/// A phone's control channel manager, with a fresh certificate and pins.
fn manager(timeout: Duration) -> ConnectionManager {
    let phone = CertManager::new().expect("phone certificate");
    ConnectionManager::new(timeout, phone.control_client_config())
}

/// Start a mock TCP server that performs the KDE Connect handshake
/// and echoes back received packets with a "response." suffix.
async fn start_mock_server() -> (u16, Arc<Mutex<Vec<NetworkPacket>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let received = Arc::new(Mutex::new(Vec::new()));

    let recv_clone = received.clone();
    tokio::spawn(async move {
        let stream = accept_tls(&listener).await.unwrap();
        let (reader, mut writer) = tokio::io::split(stream);
        let mut reader = BufReader::new(reader);

        // Step 1: Expect LINUX_LINK_HELLO
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        assert_eq!(line.trim(), HANDSHAKE_HELLO);

        // Step 2: Send OK
        writer
            .write_all(format!("{}\n", HANDSHAKE_OK).as_bytes())
            .await
            .unwrap();

        // Step 3: Send identity packet
        let identity = DeviceIdentity::new("mock-server", "Mock Server");
        let identity_wire = identity.as_identity_packet().to_wire().unwrap();
        writer.write_all(&identity_wire).await.unwrap();
        writer.flush().await.unwrap();

        // Step 4: Expect client identity
        let mut ident_line = String::new();
        reader.read_line(&mut ident_line).await.unwrap();
        let client_ident = NetworkPacket::from_wire(&ident_line).unwrap();
        assert_eq!(client_ident.packet_type, "kdeconnect.identity");
        {
            let mut rx = recv_clone.lock().await;
            rx.push(client_ident);
        }

        // Step 5: Packet loop — echo packets back with response suffix
        loop {
            let mut pkt_line = String::new();
            match reader.read_line(&mut pkt_line).await {
                Ok(0) => break,
                Ok(_) => {
                    let trimmed = pkt_line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Ok(pkt) = NetworkPacket::from_wire(trimmed) {
                        {
                            let mut rx = recv_clone.lock().await;
                            rx.push(pkt.clone());
                        }
                        // Echo as response
                        let resp = NetworkPacket::new(format!("{}.response", pkt.packet_type));
                        writer.write_all(&resp.to_wire().unwrap()).await.unwrap();
                    }
                }
                Err(_) => break,
            }
        }
    });

    (port, received)
}

#[tokio::test]
async fn test_handshake_success() {
    let (port, _received) = start_mock_server().await;
    let manager = manager(Duration::from_secs(10));
    let identity = DeviceIdentity::new("test-client", "Test Client");
    let result = manager.connect("127.0.0.1", port, &identity).await;
    assert!(
        result.is_ok(),
        "Handshake should succeed: {:?}",
        result.err()
    );
}

#[tokio::test]
async fn test_handshake_wrong_protocol() {
    // Server that sends wrong handshake response
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        let mut stream = accept_tls(&listener).await.unwrap();
        let mut line = String::new();
        let mut reader = BufReader::new(&mut stream);
        reader.read_line(&mut line).await.unwrap();
        // Send wrong protocol version
        stream.write_all(b"LINUX_LINK_OK 999\n").await.unwrap();
    });

    let manager = manager(Duration::from_secs(5));
    let identity = DeviceIdentity::new("test-client", "Test Client");
    let result = manager.connect("127.0.0.1", port, &identity).await;
    assert!(result.is_err(), "Handshake with wrong protocol should fail");
}

#[tokio::test]
async fn test_handshake_timeout() {
    // Server that accepts but never responds
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        let mut stream = accept_tls(&listener).await.unwrap();
        let mut line = String::new();
        let mut reader = BufReader::new(&mut stream);
        // Read HELLO but never respond — just hold the connection
        let _ = reader.read_line(&mut line).await;
        // Keep connection open without sending anything
        tokio::time::sleep(Duration::from_secs(10)).await;
        drop(stream);
    });

    let manager = manager(Duration::from_secs(2));
    let identity = DeviceIdentity::new("test-client", "Test Client");
    let result = manager.connect("127.0.0.1", port, &identity).await;
    assert!(result.is_err(), "Timeout should cause handshake failure");
}

#[tokio::test]
async fn test_send_and_receive_packet() {
    let (port, received) = start_mock_server().await;
    let manager = manager(Duration::from_secs(10));
    let identity = DeviceIdentity::new("test-client", "Test Client");
    let stream = manager.connect("127.0.0.1", port, &identity).await.unwrap();

    let (_reader, writer) = tokio::io::split(stream);
    let sender = TcpDeviceSender::new(writer, "127.0.0.1".to_string());

    // Send a clipboard request
    let request = NetworkPacket::new("kdeconnect.clipboard.connect");
    sender.send_packet(&request).await.unwrap();

    // Verify the server received it
    tokio::time::sleep(Duration::from_millis(200)).await;
    let rx = received.lock().await;
    assert!(!rx.is_empty(), "Server should have received packets");
    let received_types: Vec<&str> = rx.iter().map(|p| p.packet_type.as_str()).collect();
    assert!(
        received_types.contains(&"kdeconnect.identity"),
        "Should have received identity"
    );
    assert!(
        received_types.contains(&"kdeconnect.clipboard.connect"),
        "Should have received clipboard request"
    );
}

#[tokio::test]
async fn test_packet_with_body() {
    let (port, received) = start_mock_server().await;
    let manager = manager(Duration::from_secs(10));
    let identity = DeviceIdentity::new("test-client", "Test Client");
    let stream = manager.connect("127.0.0.1", port, &identity).await.unwrap();

    let (_reader, writer) = tokio::io::split(stream);
    let sender = TcpDeviceSender::new(writer, "127.0.0.1".to_string());

    // Send a file browse request with body
    let request = NetworkPacket::new("kdeconnect.filebrowse.request")
        .with_body(serde_json::json!({ "path": "/home/test" }));
    sender.send_packet(&request).await.unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;
    let rx = received.lock().await;
    let file_req = rx
        .iter()
        .find(|p| p.packet_type == "kdeconnect.filebrowse.request");
    assert!(file_req.is_some());
    if let Some(pkt) = file_req {
        let path = pkt.body.get("path").and_then(|v| v.as_str());
        assert_eq!(path, Some("/home/test"));
    }
}

#[tokio::test]
async fn test_concurrent_connections() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let connection_count = Arc::new(Mutex::new(0u32));

    let count_clone = connection_count.clone();
    tokio::spawn(async move {
        while let Ok(mut stream) = accept_tls(&listener).await {
            {
                let mut c = count_clone.lock().await;
                *c += 1;
            }
            let mut line = String::new();
            let mut reader = BufReader::new(&mut stream);
            let _ = reader.read_line(&mut line).await;
            let _ = stream
                .write_all(format!("{}\n", HANDSHAKE_OK).as_bytes())
                .await;
        }
    });

    let manager = manager(Duration::from_secs(5));
    let identity = DeviceIdentity::new("test-client", "Test Client");

    // Connect 3 clients concurrently
    let mut handles = vec![];
    for _ in 0..3 {
        let mgr = manager.clone();
        let ident = identity.clone();
        handles.push(tokio::spawn(async move {
            mgr.connect("127.0.0.1", port, &ident).await
        }));
    }

    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_ok(), "Concurrent connection should succeed");
    }

    assert_eq!(*connection_count.lock().await, 3);
}
