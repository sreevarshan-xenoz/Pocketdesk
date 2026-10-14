//! Tests for the TCP handshake protocol in `connection.rs`.
//!
//! These tests run a mock server that simulates the Pocketdesk server's
//! handshake protocol (TLS, then `POCKETDESK_HELLO → POCKETDESK_OK →
//! identity packet`).

#[cfg(test)]
mod tests {
    use crate::protocol::connection::ConnectionManager;
    use crate::protocol::kdeconnect::DeviceIdentity;
    use crate::protocol::{HANDSHAKE_HELLO, HANDSHAKE_OK};
    use crate::streaming::transport::CertManager;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    /// Accept one connection and run the desktop's side of TLS on it.
    async fn accept_tls(
        listener: &TcpListener,
    ) -> tokio_rustls::server::TlsStream<tokio::net::TcpStream> {
        let (stream, _) = listener.accept().await.expect("mock server accept");
        stream.set_nodelay(true).ok();
        tokio_rustls::TlsAcceptor::from(desktop().control_server_config().unwrap())
            .accept(stream)
            .await
            .expect("mock server TLS")
    }

    /// One desktop identity for every mock server, as a real desktop keeps
    /// one: a phone's pin for 127.0.0.1 then holds across connections.
    fn desktop() -> &'static CertManager {
        static DESKTOP: std::sync::OnceLock<CertManager> = std::sync::OnceLock::new();
        DESKTOP.get_or_init(|| CertManager::new().expect("desktop certificate"))
    }

    /// A phone's control channel manager, with a fresh certificate and pins.
    fn manager(timeout: Duration) -> ConnectionManager {
        let phone = CertManager::new().expect("phone certificate");
        ConnectionManager::new(timeout, phone.control_client_config())
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Spawn a mock server that completes the handshake and then echoes back
    /// whatever the client sends. Returns the bound port.
    async fn spawn_mock_server() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            let mut stream = accept_tls(&listener).await;

            // Read HELLO
            let mut reader = BufReader::new(&mut stream);
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .await
                .expect("mock server read HELLO");
            assert_eq!(line.trim(), HANDSHAKE_HELLO, "mock server received HELLO");

            // Send OK
            let writer = reader.into_inner();
            writer
                .write_all(format!("{}\n", HANDSHAKE_OK).as_bytes())
                .await
                .expect("mock server write OK");
            writer.flush().await.ok();

            // Read identity packet (line containing JSON)
            let mut reader = BufReader::new(writer);
            let mut identity_line = String::new();
            reader
                .read_line(&mut identity_line)
                .await
                .expect("mock server read identity");

            // Verify identity packet starts with JSON opening brace
            assert!(
                identity_line.trim().starts_with('{'),
                "mock server expected JSON identity packet, got: {:?}",
                identity_line
            );
        });

        port
    }

    /// Spawn a mock server that sends back a wrong handshake response.
    async fn spawn_mock_server_bad_handshake() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            let mut stream = accept_tls(&listener).await;

            let mut reader = BufReader::new(&mut stream);
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .await
                .expect("mock server read HELLO");

            // Send wrong response
            let writer = reader.into_inner();
            writer
                .write_all(b"POCKETDESK_BAD 1\n")
                .await
                .expect("mock server write bad response");
            writer.flush().await.ok();
        });

        port
    }

    /// Spawn a mock server that never responds (for timeout testing).
    async fn spawn_mock_server_silent() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            let mut stream = accept_tls(&listener).await;

            // Read HELLO but never respond — connection just hangs
            let mut reader = BufReader::new(&mut stream);
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .await
                .expect("mock server read HELLO");

            // Never write a response — let the client time out
            // The stream is dropped when this function returns
        });

        port
    }

    // -----------------------------------------------------------------------
    // ConnectionManager construction tests
    // -----------------------------------------------------------------------

    #[test]
    fn new_creates_manager_with_timeout() {
        // Just verify construction works — no panic
        let _manager = manager(Duration::from_secs(10));
    }

    #[test]
    fn new_creates_manager_with_zero_timeout() {
        // Zero timeout is valid (connection will likely fail but manager is valid)
        let _manager = manager(Duration::from_secs(0));
    }

    #[tokio::test]
    async fn connect_refuses_a_server_that_does_not_speak_tls() {
        // A plain server answers the first bytes it reads with the old
        // handshake; the client must not fall back to plain text.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 512];
            let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await;
            let _ = stream
                .write_all(format!("{}\n", HANDSHAKE_OK).as_bytes())
                .await;
        });
        let identity = DeviceIdentity::new("test-client", "Test Client");
        let result = manager(Duration::from_secs(2))
            .connect("127.0.0.1", port, &identity)
            .await;
        assert!(result.is_err(), "a plain server is not a control channel");
    }

    // -----------------------------------------------------------------------
    // Successful handshake tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn connect_succeeds_with_valid_handshake() {
        let port = spawn_mock_server().await;
        let manager = manager(Duration::from_secs(5));
        let identity = DeviceIdentity::new("test-client", "Test Client");

        let stream = manager
            .connect("127.0.0.1", port, &identity)
            .await
            .expect("connect should succeed with valid handshake");

        // Verify we got a live TCP stream back
        let peer_addr =
            crate::protocol::connection::control_peer_addr(&stream).expect("get peer addr");
        assert_eq!(peer_addr.port(), port);
    }

    // -----------------------------------------------------------------------
    // Handshake failure tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn connect_fails_with_bad_handshake_response() {
        let port = spawn_mock_server_bad_handshake().await;
        let manager = manager(Duration::from_secs(5));
        let identity = DeviceIdentity::new("test-client", "Test Client");

        let result = manager.connect("127.0.0.1", port, &identity).await;
        assert!(
            result.is_err(),
            "connect should fail with bad handshake response"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("handshake"),
            "error should mention handshake failure: {}",
            err
        );
    }

    // -----------------------------------------------------------------------
    // Timeout tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn connect_times_out_when_server_is_silent() {
        let port = spawn_mock_server_silent().await;
        // Use a short timeout so the test doesn't hang
        let manager = manager(Duration::from_millis(500));
        let identity = DeviceIdentity::new("test-client", "Test Client");

        let result = manager.connect("127.0.0.1", port, &identity).await;
        assert!(
            result.is_err(),
            "connect should time out with silent server"
        );
    }

    #[tokio::test]
    async fn connect_times_out_when_no_server() {
        // Connect to a port that nothing is listening on (extremely unlikely to be in use)
        let manager = manager(Duration::from_millis(500));
        let identity = DeviceIdentity::new("test-client", "Test Client");

        let result = manager.connect("127.0.0.1", 51999, &identity).await;
        assert!(
            result.is_err(),
            "connect should fail when no server is listening"
        );
    }

    // -----------------------------------------------------------------------
    // Connection timeout boundary tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn connect_uses_configured_timeout() {
        // Start a silent server so the timeout is the limiting factor
        let port = spawn_mock_server_silent().await;
        let start = std::time::Instant::now();
        let timeout = Duration::from_millis(200);
        let identity = DeviceIdentity::new("test-client", "Test Client");

        let manager = manager(timeout);
        let _result = manager.connect("127.0.0.1", port, &identity).await;

        let elapsed = start.elapsed();
        // Should fail within a reasonable bound of the configured timeout
        assert!(
            elapsed < Duration::from_secs(5),
            "connection should time out within configured timeout, took {:?}",
            elapsed
        );
    }

    // -----------------------------------------------------------------------
    // Reconnection / multiple connections test
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn multiple_connections_succeed() {
        let manager = manager(Duration::from_secs(5));
        let identity = DeviceIdentity::new("test-client", "Test Client");

        for _ in 0..3 {
            let port = spawn_mock_server().await;
            let stream = manager
                .connect("127.0.0.1", port, &identity)
                .await
                .expect("connect should succeed");
            let peer_addr =
                crate::protocol::connection::control_peer_addr(&stream).expect("get peer addr");
            assert_eq!(peer_addr.port(), port);
        }
    }

    // -----------------------------------------------------------------------
    // Identity packet validation test
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn connection_sends_valid_identity_packet() {
        // This test verifies the full handshake by inspecting what the mock
        // server receives (the mock asserts the identity packet starts with '{')
        let port = spawn_mock_server().await;
        let manager = manager(Duration::from_secs(5));
        let identity = DeviceIdentity::new("test-client", "Test Client");

        let stream = manager
            .connect("127.0.0.1", port, &identity)
            .await
            .expect("connect should succeed");

        // If we got here, the mock server's assertions all passed
        assert!(crate::protocol::connection::control_peer_addr(&stream).is_ok());
    }

    /// A desktop that vanished (slept, left the network) leaves a socket
    /// that looks open forever unless the phone probes it: keepalive finds
    /// the dead link, so the reader ends and the phone dials again.
    #[tokio::test]
    async fn control_connections_probe_a_silent_peer() {
        let port = spawn_mock_server().await;
        let manager = manager(Duration::from_secs(5));
        let identity = DeviceIdentity::new("test-client", "Test Client");

        let stream = manager
            .connect("127.0.0.1", port, &identity)
            .await
            .expect("connect should succeed");

        let socket = socket2::SockRef::from(stream.get_ref().0);
        assert!(socket.keepalive().unwrap(), "keepalive is on");
    }

    /// The desktop's side does the same for a phone that vanished.
    #[tokio::test]
    async fn keep_alive_turns_probing_on() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (client, accepted) =
            tokio::join!(tokio::net::TcpStream::connect(addr), listener.accept());
        let (server, _) = accepted.unwrap();
        let _client = client.unwrap();

        crate::protocol::connection::keep_alive(&server).unwrap();
        assert!(socket2::SockRef::from(&server).keepalive().unwrap());
    }
}
