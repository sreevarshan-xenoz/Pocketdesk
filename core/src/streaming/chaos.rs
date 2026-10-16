pub mod proxy {
    use rand::Rng;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::net::UdpSocket;
    use tokio::sync::RwLock;
    use tracing::{debug, info};

    #[derive(Debug, Clone)]
    pub struct ChaosConfig {
        pub drop_rate: f64, // 0.0 to 1.0
        pub base_latency_ms: u64,
        pub jitter_ms: u64,
        pub blackhole_duration_ms: Option<u64>,
        pub start_time: std::time::Instant,
        /// A slow hop on the way back, server to client (where video goes):
        /// `None` forwards return traffic as it comes.
        pub downlink: Option<RateLimit>,
    }

    impl Default for ChaosConfig {
        fn default() -> Self {
            Self {
                drop_rate: 0.0,
                base_latency_ms: 0,
                jitter_ms: 0,
                blackhole_duration_ms: None,
                start_time: std::time::Instant::now(),
                downlink: None,
            }
        }
    }

    /// A link of fixed rate behind a router queue of bounded length, as a
    /// slow Wi-Fi hop is: a packet waits for those ahead of it, and one that
    /// finds the queue full is lost (drop tail), never delayed without end.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct RateLimit {
        pub bits_per_sec: u64,
        pub queue_packets: usize,
    }

    /// The state of a [`RateLimit`]ed link: when each queued packet leaves.
    #[derive(Debug)]
    pub struct SlowLink {
        limit: RateLimit,
        departures: std::collections::VecDeque<std::time::Instant>,
    }

    impl SlowLink {
        pub fn new(limit: RateLimit) -> Self {
            Self {
                limit,
                departures: std::collections::VecDeque::new(),
            }
        }

        /// A `len`-byte packet arriving at `now`: when it has left the link,
        /// or `None` when the queue is full and it is lost.
        pub fn admit(&mut self, now: std::time::Instant, len: usize) -> Option<std::time::Instant> {
            while self.departures.front().is_some_and(|&t| t <= now) {
                self.departures.pop_front();
            }
            if self.departures.len() >= self.limit.queue_packets {
                return None;
            }
            let free = self.departures.back().copied().unwrap_or(now).max(now);
            let wire = Duration::from_nanos(
                (len as u64 * 8).saturating_mul(1_000_000_000) / self.limit.bits_per_sec.max(1),
            );
            let leaves = free + wire;
            self.departures.push_back(leaves);
            Some(leaves)
        }
    }

    impl ChaosConfig {
        pub fn coffee_shop_wifi() -> Self {
            Self {
                drop_rate: 0.10, // 10% packet loss
                base_latency_ms: 50,
                jitter_ms: 100,
                blackhole_duration_ms: None,
                start_time: std::time::Instant::now(),
                downlink: None,
            }
        }

        pub fn elevator_deadzone() -> Self {
            Self {
                drop_rate: 0.0,
                base_latency_ms: 20,
                jitter_ms: 10,
                blackhole_duration_ms: Some(3000), // 3-second absolute blackhole
                start_time: std::time::Instant::now(),
                downlink: None,
            }
        }

        pub fn should_drop(&self) -> bool {
            // Check blackhole
            if let Some(dur) = self.blackhole_duration_ms {
                let elapsed = self.start_time.elapsed().as_millis() as u64;
                if elapsed > 1000 && elapsed < (1000 + dur) {
                    return true;
                }
            }

            if self.drop_rate <= 0.0 {
                return false;
            }

            let mut rng = rand::thread_rng();
            rng.gen_bool(self.drop_rate)
        }

        pub fn calculate_delay(&self) -> Duration {
            if self.base_latency_ms == 0 && self.jitter_ms == 0 {
                return Duration::from_millis(0);
            }

            let mut delay = self.base_latency_ms;
            if self.jitter_ms > 0 {
                let mut rng = rand::thread_rng();
                let jitter = rng.gen_range(0..self.jitter_ms);
                delay += jitter;
            }
            Duration::from_millis(delay)
        }
    }

    /// A simple UDP proxy that injects chaos (latency, drops) between a client and server.
    ///
    /// The listen socket is bound by [`ChaosProxy::new`] so a caller can pass
    /// port `0` and read the real address back with [`ChaosProxy::local_addr`]
    /// — two tests must not fight each other, or the shipping server, over a
    /// fixed port. Every datagram the proxy sees is counted as either
    /// forwarded or dropped, which is what makes "the harness injected chaos"
    /// a checkable fact instead of a probability.
    pub struct ChaosProxy {
        socket: Arc<UdpSocket>,
        target_addr: SocketAddr,
        config: Arc<RwLock<ChaosConfig>>,
        cancel: tokio_util::sync::CancellationToken,
        forwarded: AtomicUsize,
        dropped: AtomicUsize,
        downlink_lost: Arc<AtomicUsize>,
    }

    impl ChaosProxy {
        pub async fn new(
            listen_addr: &str,
            target_addr: &str,
            config: ChaosConfig,
        ) -> anyhow::Result<Self> {
            let listen_addr: SocketAddr = listen_addr.parse()?;
            let target_addr: SocketAddr = target_addr.parse()?;
            let socket = Arc::new(UdpSocket::bind(listen_addr).await?);
            Ok(Self {
                socket,
                target_addr,
                config: Arc::new(RwLock::new(config)),
                cancel: tokio_util::sync::CancellationToken::new(),
                forwarded: AtomicUsize::new(0),
                dropped: AtomicUsize::new(0),
                downlink_lost: Arc::new(AtomicUsize::new(0)),
            })
        }

        /// The address the proxy is actually listening on (resolves port `0`).
        pub fn local_addr(&self) -> anyhow::Result<SocketAddr> {
            Ok(self.socket.local_addr()?)
        }

        /// Datagrams released towards the server (may still be in flight).
        pub fn forwarded(&self) -> usize {
            self.forwarded.load(Ordering::Relaxed)
        }

        /// Datagrams the proxy discarded.
        pub fn dropped(&self) -> usize {
            self.dropped.load(Ordering::Relaxed)
        }

        /// Return datagrams lost at a full [`ChaosConfig::downlink`] queue.
        pub fn downlink_lost(&self) -> usize {
            self.downlink_lost.load(Ordering::Relaxed)
        }

        pub fn update_config(&self, config: ChaosConfig) {
            let config_clone = self.config.clone();
            tokio::spawn(async move {
                let mut guard = config_clone.write().await;
                *guard = config;
            });
        }

        pub fn stop(&self) {
            self.cancel.cancel();
        }

        pub async fn run(&self) -> anyhow::Result<()> {
            let socket = self.socket.clone();
            let listen_addr = self.local_addr()?;

            // Map of client addresses to their dedicated forwarder sockets
            type ClientForwarder = (
                Arc<UdpSocket>,
                tokio::sync::mpsc::UnboundedSender<(tokio::time::Instant, Vec<u8>)>,
            );
            let clients: Arc<RwLock<std::collections::HashMap<SocketAddr, ClientForwarder>>> =
                Arc::new(RwLock::new(std::collections::HashMap::new()));

            info!(
                "ChaosProxy listening on {} -> forwarding to {}",
                listen_addr, self.target_addr
            );

            let mut buf = vec![0u8; 65535];
            let cancel = self.cancel.clone();

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        info!("ChaosProxy shutting down");
                        break;
                    }
                    res = socket.recv_from(&mut buf) => {
                        let (len, src_addr) = match res {
                            Ok(x) => x,
                            Err(e) => {
                                debug!("ChaosProxy read error: {}", e);
                                continue;
                            }
                        };

                        let data = buf[..len].to_vec();
                        let config = self.config.read().await.clone();

                        if config.should_drop() {
                            self.dropped.fetch_add(1, Ordering::Relaxed);
                            continue; // Drop packet
                        }
                        self.forwarded.fetch_add(1, Ordering::Relaxed);

                        let delay = config.calculate_delay();
                        let target = self.target_addr;
                        let clients_ref = clients.clone();
                        let server_sock = socket.clone();
                        let config_ref = self.config.clone();
                        let downlink_lost = self.downlink_lost.clone();

                        tokio::spawn(async move {
                            if delay.as_millis() > 0 {
                                tokio::time::sleep(delay).await;
                            }

                            // Ensure we have a proxy socket for this client to receive return traffic
                            let _fwd_sock = {
                                let c_guard = clients_ref.read().await;
                                if let Some((s, _)) = c_guard.get(&src_addr) {
                                    s.clone()
                                } else {
                                    drop(c_guard);
                                    let mut c_guard = clients_ref.write().await;
                                    if let Some((s, _)) = c_guard.get(&src_addr) {
                                        s.clone()
                                    } else {
                                        // Create a new socket to talk to the server on behalf of this client
                                        let s = Arc::new(UdpSocket::bind("0.0.0.0:0").await.unwrap());
                                        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(tokio::time::Instant, Vec<u8>)>();
                                        c_guard.insert(src_addr, (s.clone(), tx));

                                        // Spawn a task to listen for return traffic from the server,
                                        // through the slow downlink when one is set, and one to
                                        // send it on in order.
                                        let s_clone = s.clone();
                                        let mut ret_buf = vec![0u8; 65535];
                                        let (down_tx, mut down_rx) = tokio::sync::mpsc::unbounded_channel::<(tokio::time::Instant, Vec<u8>)>();
                                        let server_sock_clone = server_sock.clone();
                                        tokio::spawn(async move {
                                            while let Some((at, data)) = down_rx.recv().await {
                                                tokio::time::sleep_until(at).await;
                                                let _ = server_sock_clone.send_to(&data, src_addr).await;
                                            }
                                        });
                                        tokio::spawn(async move {
                                            let mut link: Option<SlowLink> = None;
                                            while let Ok((n, _)) =
                                                s_clone.recv_from(&mut ret_buf).await
                                            {
                                                let limit = config_ref.read().await.downlink;
                                                let at = match limit {
                                                    None => Some(std::time::Instant::now()),
                                                    Some(limit) => {
                                                        if link.as_ref().map(|l| l.limit) != Some(limit) {
                                                            link = Some(SlowLink::new(limit));
                                                        }
                                                        link.as_mut()
                                                            .and_then(|l| l.admit(std::time::Instant::now(), n))
                                                    }
                                                };
                                                match at {
                                                    Some(at) => {
                                                        let _ = down_tx.send((
                                                            tokio::time::Instant::from_std(at),
                                                            ret_buf[..n].to_vec(),
                                                        ));
                                                    }
                                                    None => {
                                                        downlink_lost.fetch_add(1, Ordering::Relaxed);
                                                    }
                                                }
                                            }
                                        });

                                        // Spawn a task to send forwarded traffic in order
                                        let s_clone2 = s.clone();
                                        let target_clone = target;
                                        tokio::spawn(async move {
                                            while let Some((deliver_at, data)) = rx.recv().await {
                                                tokio::time::sleep_until(deliver_at).await;
                                                let _ = s_clone2.send_to(&data, target_clone).await;
                                            }
                                        });

                                        s
                                    }
                                }
                            };

                            // Queue packet for ordered delivery
                            let c_guard = clients_ref.read().await;
                            if let Some((_, tx)) = c_guard.get(&src_addr) {
                                let deliver_at = tokio::time::Instant::now() + delay;
                                let _ = tx.send((deliver_at, data));
                            }
                        });
                    }
                }
            }
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn should_drop_honours_the_ends_of_its_rate() {
            let never = ChaosConfig {
                drop_rate: 0.0,
                ..Default::default()
            };
            let always = ChaosConfig {
                drop_rate: 1.0,
                ..Default::default()
            };
            assert!((0..100).all(|_| !never.should_drop()));
            assert!((0..100).all(|_| always.should_drop()));
        }

        /// The integration tests used to infer "chaos happened" from a sample
        /// of 100 datagrams, which fails 2.65% of the time on its own RNG. The
        /// rate is a property of this function, so measure it where a wide
        /// enough sample makes the check deterministic (σ ≈ 0.001 at N=100k).
        #[test]
        fn should_drop_hits_its_configured_rate() {
            let cfg = ChaosConfig {
                drop_rate: 0.10,
                ..Default::default()
            };
            let trials = 100_000;
            let drops = (0..trials).filter(|_| cfg.should_drop()).count();
            let share = drops as f64 / trials as f64;
            assert!(
                (0.08..=0.12).contains(&share),
                "drop_rate 0.10 produced share {share} over {trials} draws"
            );
        }

        #[test]
        fn blackhole_window_is_exact() {
            let mut cfg = ChaosConfig {
                drop_rate: 0.0,
                blackhole_duration_ms: Some(3000),
                ..Default::default()
            };
            // Before the window opens and after it closes: everything passes.
            cfg.start_time = std::time::Instant::now();
            assert!(!cfg.should_drop());
            cfg.start_time = std::time::Instant::now() - Duration::from_millis(5000);
            assert!(!cfg.should_drop());
            // Mid-window: nothing gets through.
            cfg.start_time = std::time::Instant::now() - Duration::from_millis(2000);
            assert!(cfg.should_drop());
        }

        #[test]
        fn delay_is_base_plus_bounded_jitter() {
            let silent = ChaosConfig::default();
            assert_eq!(silent.calculate_delay(), Duration::from_millis(0));

            let cfg = ChaosConfig {
                base_latency_ms: 50,
                jitter_ms: 100,
                ..Default::default()
            };
            for _ in 0..1000 {
                let d = cfg.calculate_delay();
                assert!((Duration::from_millis(50)..=Duration::from_millis(150)).contains(&d));
            }
        }

        #[test]
        fn a_slow_link_spaces_packets_by_their_size() {
            let start = std::time::Instant::now();
            // 1 Mbit/s: a 1250-byte packet takes 10 ms on the wire.
            let mut link = SlowLink::new(RateLimit {
                bits_per_sec: 1_000_000,
                queue_packets: 64,
            });
            let first = link.admit(start, 1250).unwrap();
            let second = link.admit(start, 1250).unwrap();
            assert_eq!(first - start, Duration::from_millis(10));
            assert_eq!(second - start, Duration::from_millis(20));
            // Once the queue has drained, a packet goes at once again.
            let later = start + Duration::from_secs(1);
            assert_eq!(
                link.admit(later, 1250).unwrap() - later,
                Duration::from_millis(10)
            );
        }

        #[test]
        fn a_full_queue_drops_its_tail() {
            let start = std::time::Instant::now();
            let mut link = SlowLink::new(RateLimit {
                bits_per_sec: 1_000_000,
                queue_packets: 4,
            });
            let admitted = (0..10)
                .filter(|_| link.admit(start, 1250).is_some())
                .count();
            assert_eq!(admitted, 4, "the router holds four, the rest are lost");
            // The head has left after 10 ms, so one more fits.
            assert!(
                link.admit(start + Duration::from_millis(10), 1250)
                    .is_some()
            );
            assert!(
                link.admit(start + Duration::from_millis(10), 1250)
                    .is_none()
            );
        }

        #[tokio::test]
        async fn the_return_path_rides_the_slow_downlink() {
            // An echo server stands in for the computer.
            let echo = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
            let echo_addr = echo.local_addr().unwrap();
            let echoing = {
                let echo = echo.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 2048];
                    while let Ok((n, from)) = echo.recv_from(&mut buf).await {
                        let _ = echo.send_to(&buf[..n], from).await;
                    }
                })
            };
            let proxy = Arc::new(
                ChaosProxy::new(
                    "127.0.0.1:0",
                    &echo_addr.to_string(),
                    ChaosConfig {
                        // 1250 bytes take 100 ms at 100 kbit/s.
                        downlink: Some(RateLimit {
                            bits_per_sec: 100_000,
                            queue_packets: 3,
                        }),
                        ..Default::default()
                    },
                )
                .await
                .unwrap(),
            );
            let addr = proxy.local_addr().unwrap();
            let running = {
                let proxy = proxy.clone();
                tokio::spawn(async move { proxy.run().await })
            };
            let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let sent_at = tokio::time::Instant::now();
            for _ in 0..10 {
                client.send_to(&[7u8; 1250], addr).await.unwrap();
            }
            let mut buf = vec![0u8; 2048];
            let mut arrivals = Vec::new();
            while let Ok(Ok(_)) =
                tokio::time::timeout(Duration::from_millis(600), client.recv_from(&mut buf)).await
            {
                arrivals.push(sent_at.elapsed());
            }
            assert_eq!(
                arrivals.len() + proxy.downlink_lost(),
                10,
                "each echo arrives or is lost at the queue"
            );
            assert!(
                (3..=5).contains(&arrivals.len()),
                "about the queue's worth gets through: {arrivals:?}"
            );
            assert!(
                arrivals[1] - arrivals[0] >= Duration::from_millis(80),
                "spaced by the rate: {arrivals:?}"
            );
            proxy.stop();
            running.await.unwrap().unwrap();
            echoing.abort();
        }

        #[tokio::test]
        async fn proxy_binds_an_ephemeral_port_and_accounts_every_datagram() {
            let proxy = Arc::new(
                ChaosProxy::new(
                    "127.0.0.1:0",
                    "127.0.0.1:9",
                    ChaosConfig {
                        drop_rate: 0.5,
                        ..Default::default()
                    },
                )
                .await
                .unwrap(),
            );
            let addr = proxy.local_addr().unwrap();
            assert_ne!(addr.port(), 0, "port 0 must be resolved by new()");

            let running = {
                let proxy = proxy.clone();
                tokio::spawn(async move { proxy.run().await })
            };

            let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            for _ in 0..40 {
                client.send_to(b"payload", addr).await.unwrap();
            }
            // The counters move at decision time, so this only waits for the
            // proxy to read what the client sent.
            for _ in 0..100 {
                if proxy.forwarded() + proxy.dropped() == 40 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert_eq!(
                proxy.forwarded() + proxy.dropped(),
                40,
                "every datagram must be either forwarded or dropped"
            );
            drop(client);
            proxy.stop();
            running.await.unwrap().unwrap();
        }
    }
}
