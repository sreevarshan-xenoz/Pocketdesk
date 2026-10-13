use pocketdesk_core::streaming::chaos::proxy::{ChaosConfig, ChaosProxy};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::net::UdpSocket;

/// Coffee-shop Wi-Fi: 10% loss plus 50-150 ms of latency/jitter between a
/// client and a server. Both endpoints bind port 0 so the suite can never
/// collide with itself or with a running `pocketdesk` service.
#[tokio::test]
async fn test_coffee_shop_wifi() {
    // 1. Dummy server on an ephemeral port.
    let server_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let server_addr: SocketAddr = server_socket.local_addr().unwrap();
    let received_count = Arc::new(AtomicUsize::new(0));
    let received_count_clone = received_count.clone();

    tokio::spawn(async move {
        let mut buf = vec![0u8; 1024];
        while let Ok((_len, _src)) = server_socket.recv_from(&mut buf).await {
            received_count_clone.fetch_add(1, Ordering::SeqCst);
        }
    });

    // 2. ChaosProxy in front of it.
    let config = ChaosConfig::coffee_shop_wifi(); // 10% drop rate, latency, jitter
    let proxy = Arc::new(
        ChaosProxy::new("127.0.0.1:0", &server_addr.to_string(), config)
            .await
            .expect("Failed to create ChaosProxy"),
    );
    let proxy_addr = proxy.local_addr().expect("proxy address");

    {
        let proxy = proxy.clone();
        tokio::spawn(async move {
            let _ = proxy.run().await;
        });
    }

    // 3. Wait 1s for proxy to start
    tokio::time::sleep(Duration::from_secs(1)).await;

    // 4. Send 100 datagrams
    let client_socket = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind client socket");
    let payload = b"hello chaos";
    for _ in 0..100 {
        client_socket
            .send_to(payload, proxy_addr)
            .await
            .expect("Failed to send datagram");
        // small delay to prevent UDP buffer overflow and allow latency/jitter to apply well
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // 5. Wait for packets to arrive (accounting for latency and jitter)
    tokio::time::sleep(Duration::from_secs(2)).await;

    let total_received = received_count.load(Ordering::SeqCst);
    let forwarded = proxy.forwarded();
    let dropped = proxy.dropped();
    println!("Total received: {total_received} ({forwarded} forwarded, {dropped} dropped)");

    // The proxy's own accounting is exact, so nothing can vanish unexplained
    // and the assertion does not depend on what the RNG happened to do.
    assert_eq!(
        forwarded + dropped,
        100,
        "every datagram must be forwarded or dropped, not lost"
    );
    assert_eq!(
        total_received, forwarded,
        "the server must see exactly what the proxy released"
    );
    // Sanity bound on the configured loss: at 10% over 100 datagrams, seeing
    // zero drops is a 2.65% event, so it is asserted as a range, not equality.
    assert!(
        (60..=100).contains(&forwarded),
        "proxy forwarded {forwarded} of 100 at drop_rate 0.10"
    );
}
