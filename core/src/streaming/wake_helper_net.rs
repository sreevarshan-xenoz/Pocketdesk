//! The wake helper's listener (`protocol::wake_helper`): an iroh endpoint on
//! the helper's own key that accepts [`ALPN_WAKE`] and nothing else, and the
//! owner phone's side of a request.
//!
//! The listener reads who is asking from the key the connection proved and
//! closes a key no computer introduced before reading a byte. A key that was
//! introduced gets one line read (at most 512 bytes, within 5 seconds), the
//! rules in `protocol::wake_helper` decide, and the helper answers one line.
//! The magic packet carries the MAC and goes to the address of the computer's
//! own [`Duty`](crate::protocol::wake_helper::Duty), never anything the
//! request said. The log says only that a request was served or refused,
//! never a key, a MAC or an address.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use iroh::endpoint::presets::Minimal;
use iroh::{Endpoint, RelayMode, SecretKey};

use super::iroh_connection::{IrohDial, dns_resolver};
use crate::protocol::wake_helper::{
    ALPN_WAKE, Answer, Decision, Duties, Limiter, MAX_REQUEST_BYTES, Request, targets,
};

/// How long a requester has to send its line, and a helper to answer.
const STEP: Duration = Duration::from_secs(5);

/// How long the helper waits for the requester to take the answer and close.
const LINGER: Duration = Duration::from_secs(3);

/// Connections handled at once; more are refused before the handshake.
const AT_ONCE: usize = 4;

/// What sends the magic packet: the phone's network in production, a
/// recorder in tests.
pub trait WakeSender: Send + Sync + 'static {
    /// Send the magic packet for `mac` to each of `targets`; how many took
    /// it.
    fn send(&self, mac: [u8; 6], targets: &[SocketAddr]) -> usize;
}

/// The real sender: the same magic packet the phone's own Wake sends.
pub struct NetworkSender;

impl WakeSender for NetworkSender {
    fn send(&self, mac: [u8; 6], targets: &[SocketAddr]) -> usize {
        let mac = mac.map(|b| format!("{b:02X}")).join(":");
        crate::tailscale::wol::send_wake(&mac, targets).unwrap_or(0)
    }
}

/// What the helper holds: the computers' duties, the phone's own networks'
/// broadcast addresses (the only ones a packet may go to), and the limiter.
pub struct HelperState {
    pub duties: Mutex<Duties>,
    /// Broadcast addresses of the networks the phone is on now.
    pub local: Mutex<Vec<Ipv4Addr>>,
    limiter: Mutex<Limiter>,
    sender: Box<dyn WakeSender>,
    served: Mutex<Served>,
}

/// What the helper has done, as counts.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Served {
    pub sent: u32,
    pub refused: u32,
    pub dropped: u32,
}

impl HelperState {
    pub fn new(sender: Box<dyn WakeSender>) -> Arc<Self> {
        Arc::new(Self {
            duties: Mutex::new(Duties::default()),
            local: Mutex::new(Vec::new()),
            limiter: Mutex::new(Limiter::new(Instant::now())),
            sender,
            served: Mutex::new(Served::default()),
        })
    }

    /// How many requests were sent, refused and dropped since it started.
    pub fn served(&self) -> Served {
        *self.served.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn count(&self, change: impl FnOnce(&mut Served)) {
        change(&mut self.served.lock().unwrap_or_else(|e| e.into_inner()));
    }

    /// The decision on `request` from `requester`, with its packet sent when
    /// it is to be: the answer, or `None` to close without one.
    async fn handle(self: &Arc<Self>, requester: &str, request: &Request) -> Option<Answer> {
        let decision = {
            let duties = self.duties.lock().unwrap_or_else(|e| e.into_inner());
            let mut limiter = self.limiter.lock().unwrap_or_else(|e| e.into_inner());
            duties.decide(requester, request, &mut limiter, Instant::now())
        };
        match decision {
            Decision::Drop => {
                self.count(|s| s.dropped += 1);
                None
            }
            Decision::Answer(answer) => {
                self.count(|s| s.refused += 1);
                Some(answer)
            }
            Decision::Send { mac, broadcast } => {
                let local = self.local.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let to = targets(broadcast, &local);
                let state = Arc::clone(self);
                let reached = tokio::task::spawn_blocking(move || state.sender.send(mac, &to))
                    .await
                    .unwrap_or(0);
                if reached > 0 {
                    self.count(|s| s.sent += 1);
                    tracing::info!("Wake helper: sent a wake for an owner phone");
                    Some(Answer::Sent)
                } else {
                    self.count(|s| s.refused += 1);
                    tracing::info!("Wake helper: a wake could not be sent");
                    Some(Answer::Failed)
                }
            }
        }
    }
}

/// A running helper endpoint. Stop it with [`Helper::stop`]: an iroh
/// endpoint is closed, never dropped.
pub struct Helper {
    endpoint: Endpoint,
    accept: tokio::task::JoinHandle<()>,
    state: Arc<HelperState>,
}

impl Helper {
    /// Bind the helper's endpoint on `key` and start accepting. `relay_mode`
    /// is n0's relays in production; `bind` is `0.0.0.0:0` there.
    pub async fn start(
        key: SecretKey,
        relay_mode: RelayMode,
        bind: SocketAddr,
        state: Arc<HelperState>,
    ) -> Result<Self> {
        let endpoint = Endpoint::builder(Minimal)
            .dns_resolver(dns_resolver())
            .alpns(vec![ALPN_WAKE.to_vec()])
            .relay_mode(relay_mode)
            .secret_key(key)
            .bind_addr(bind)
            .context("the wake helper's bind address was refused")?
            .bind()
            .await
            .context("the wake helper's endpoint could not bind")?;
        let accept = {
            let endpoint = endpoint.clone();
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                let slots = Arc::new(tokio::sync::Semaphore::new(AT_ONCE));
                while let Some(incoming) = endpoint.accept().await {
                    let Ok(slot) = Arc::clone(&slots).try_acquire_owned() else {
                        incoming.refuse();
                        continue;
                    };
                    let state = Arc::clone(&state);
                    tokio::spawn(async move {
                        let _slot = slot;
                        let Ok(accepting) = incoming.accept() else {
                            return;
                        };
                        let Ok(Ok(conn)) = tokio::time::timeout(STEP, accepting).await else {
                            return;
                        };
                        serve_one(conn, &state).await;
                    });
                }
            })
        };
        Ok(Self {
            endpoint,
            accept,
            state,
        })
    }

    /// The helper's key, as the computer is told it.
    pub fn id(&self) -> String {
        self.endpoint.id().to_string()
    }

    /// The relay URLs the endpoint is reachable through now: empty until it
    /// has reached its home relay.
    pub fn relay_urls(&self) -> Vec<String> {
        self.endpoint
            .addr()
            .relay_urls()
            .map(|u| u.to_string())
            .collect()
    }

    /// The endpoint's own address, for a test to dial.
    pub fn bound_sockets(&self) -> Vec<SocketAddr> {
        self.endpoint.bound_sockets()
    }

    pub fn state(&self) -> &Arc<HelperState> {
        &self.state
    }

    /// Stop listening and close the endpoint.
    pub async fn stop(self) {
        self.accept.abort();
        self.endpoint.close().await;
    }
}

/// One accepted connection: close a key no computer introduced, else read
/// its one line, decide and answer.
async fn serve_one(conn: iroh::endpoint::Connection, state: &Arc<HelperState>) {
    let requester = conn.remote_id().to_string();
    let known = state
        .duties
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .knows(&requester);
    if !known {
        state.count(|s| s.dropped += 1);
        conn.close(iroh::endpoint::VarInt::from_u32(1), b"not introduced");
        return;
    }
    let Ok(Ok((mut send, mut recv))) = tokio::time::timeout(STEP, conn.accept_bi()).await else {
        conn.close(iroh::endpoint::VarInt::from_u32(2), b"no request");
        return;
    };
    let line = match tokio::time::timeout(STEP, recv.read_to_end(MAX_REQUEST_BYTES)).await {
        Ok(Ok(line)) => line,
        _ => {
            conn.close(iroh::endpoint::VarInt::from_u32(2), b"no request");
            return;
        }
    };
    let answer = match Request::parse(&line) {
        Some(request) => state.handle(&requester, &request).await,
        None => {
            state.count(|s| s.dropped += 1);
            None
        }
    };
    if let Some(answer) = answer {
        if send.write_all(answer.line().as_bytes()).await.is_ok() {
            let _ = send.finish();
            // The requester takes the answer and closes; wait for it, so the
            // answer is not cut off by this close.
            let _ = tokio::time::timeout(LINGER, conn.closed()).await;
        }
        conn.close(iroh::endpoint::VarInt::from_u32(0), b"done");
    } else {
        conn.close(iroh::endpoint::VarInt::from_u32(1), b"not served");
    }
}

/// Where an owner phone finds its helper.
#[derive(Debug, Clone)]
pub struct HelperTarget {
    pub iroh: String,
    pub relays: Vec<String>,
}

/// Ask the helper to wake `computer`, dialing it as `device_key`.
/// `direct_addrs` and `use_relays` are for tests; a phone passes none and
/// `true`.
pub async fn ask(
    helper: &HelperTarget,
    computer: &str,
    device_key: Option<SecretKey>,
    direct_addrs: &[String],
    use_relays: bool,
    timeout: Duration,
) -> Result<Answer> {
    let request = Request {
        computer: computer.to_string(),
    };
    let answer = IrohDial::wake_exchange(
        &helper.iroh,
        &helper.relays,
        direct_addrs,
        use_relays,
        timeout,
        device_key,
        request.line().as_bytes(),
        MAX_REQUEST_BYTES,
    )
    .await?;
    Ok(Answer::parse(&answer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::wake_helper::{Duty, EVERY};
    use std::sync::atomic::{AtomicUsize, Ordering};

    type Sent = Vec<([u8; 6], Vec<SocketAddr>)>;

    /// Records what would be sent, and sends nothing.
    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Sent>>, Arc<AtomicUsize>);

    impl WakeSender for Recorder {
        fn send(&self, mac: [u8; 6], targets: &[SocketAddr]) -> usize {
            self.1.fetch_add(1, Ordering::SeqCst);
            self.0.lock().unwrap().push((mac, targets.to_vec()));
            targets.len()
        }
    }

    const MAC: [u8; 6] = [0x02, 0x00, 0x5e, 0x00, 0x53, 0x01];

    fn duty(requesters: &[String]) -> Duty {
        Duty {
            computer: "computer-1".to_string(),
            mac: MAC,
            broadcast: Ipv4Addr::new(192, 0, 2, 255),
            requesters: requesters.to_vec(),
        }
    }

    async fn helper(recorder: &Recorder, requesters: &[String]) -> Helper {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let state = HelperState::new(Box::new(recorder.clone()));
        state.duties.lock().unwrap().set(duty(requesters));
        *state.local.lock().unwrap() = vec![Ipv4Addr::new(192, 0, 2, 255)];
        Helper::start(
            SecretKey::generate(),
            RelayMode::Disabled,
            SocketAddr::from(([127, 0, 0, 1], 0)),
            state,
        )
        .await
        .expect("the helper binds")
    }

    fn loopback(helper: &Helper) -> Vec<String> {
        helper
            .bound_sockets()
            .into_iter()
            .filter(|a| a.ip().is_loopback() && a.port() != 0)
            .map(|a| a.to_string())
            .collect()
    }

    async fn wake(helper: &Helper, key: &SecretKey, computer: &str) -> Result<Answer> {
        ask(
            &HelperTarget {
                iroh: helper.id(),
                relays: Vec::new(),
            },
            computer,
            Some(key.clone()),
            &loopback(helper),
            false,
            Duration::from_secs(8),
        )
        .await
    }

    #[tokio::test]
    async fn an_introduced_phone_wakes_its_computer_with_the_computers_own_mac() {
        let recorder = Recorder::default();
        let phone = SecretKey::generate();
        let helper = helper(&recorder, &[phone.public().to_string()]).await;
        let answer = wake(&helper, &phone, "computer-1").await.expect("answered");
        assert_eq!(answer, Answer::Sent);
        {
            let sent = recorder.0.lock().unwrap();
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].0, MAC, "the MAC is the duty's");
            assert_eq!(sent[0].1[0], "192.0.2.255:9".parse().unwrap());
            assert_eq!(sent[0].1[1], "255.255.255.255:9".parse().unwrap());
        }
        assert_eq!(helper.state().served().sent, 1);
        helper.stop().await;
    }

    #[tokio::test]
    async fn a_stranger_is_closed_and_nothing_is_sent() {
        let recorder = Recorder::default();
        let phone = SecretKey::generate();
        let stranger = SecretKey::generate();
        let helper = helper(&recorder, &[phone.public().to_string()]).await;
        let answer = wake(&helper, &stranger, "computer-1").await;
        assert!(
            !matches!(answer, Ok(Answer::Sent)),
            "a stranger's key is not served: {answer:?}"
        );
        assert_eq!(recorder.1.load(Ordering::SeqCst), 0);
        assert!(helper.state().served().dropped >= 1);
        helper.stop().await;
    }

    #[tokio::test]
    async fn a_revoked_phone_is_refused_once_the_duty_drops_its_key() {
        let recorder = Recorder::default();
        let phone = SecretKey::generate();
        let other = SecretKey::generate();
        let helper = helper(
            &recorder,
            &[phone.public().to_string(), other.public().to_string()],
        )
        .await;
        assert_eq!(
            wake(&helper, &phone, "computer-1").await.unwrap(),
            Answer::Sent
        );
        // The computer revoked `phone` and sent its list again.
        helper
            .state()
            .duties
            .lock()
            .unwrap()
            .set(duty(&[other.public().to_string()]));
        let answer = wake(&helper, &phone, "computer-1").await;
        assert!(!matches!(answer, Ok(Answer::Sent)), "{answer:?}");
        assert_eq!(recorder.1.load(Ordering::SeqCst), 1);
        helper.stop().await;
    }

    #[tokio::test]
    async fn a_phone_asks_a_burst_and_then_waits() {
        let recorder = Recorder::default();
        let phone = SecretKey::generate();
        let helper = helper(&recorder, &[phone.public().to_string()]).await;
        let mut answers = Vec::new();
        for _ in 0..4 {
            answers.push(wake(&helper, &phone, "computer-1").await.unwrap());
        }
        assert_eq!(
            answers,
            vec![
                Answer::Sent,
                Answer::Sent,
                Answer::SlowDown,
                Answer::SlowDown
            ],
            "a burst of two, then one every {EVERY} seconds"
        );
        assert_eq!(recorder.1.load(Ordering::SeqCst), 2);
        helper.stop().await;
    }

    #[tokio::test]
    async fn another_computer_and_a_bad_line_send_nothing() {
        let recorder = Recorder::default();
        let phone = SecretKey::generate();
        let helper = helper(&recorder, &[phone.public().to_string()]).await;
        assert_eq!(
            wake(&helper, &phone, "computer-9").await.unwrap(),
            Answer::Unknown
        );
        // A line carrying a MAC of its own is no request.
        let raw = IrohDial::wake_exchange(
            &helper.id(),
            &[],
            &loopback(&helper),
            false,
            Duration::from_secs(8),
            Some(phone.clone()),
            br#"{"wake":"computer-1","mac":"02:00:5e:00:00:99"}"#,
            MAX_REQUEST_BYTES,
        )
        .await;
        assert!(
            !matches!(raw.map(|a| Answer::parse(&a)), Ok(Answer::Sent)),
            "not sent"
        );
        assert_eq!(recorder.1.load(Ordering::SeqCst), 0);
        helper.stop().await;
    }
}
