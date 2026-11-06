//! The Bluetooth road, the phone's side (the fallback when neither the home
//! nor the internet road reaches the computer; protocol.md 1.4a).
//!
//! Over a working road the computer says where to dial it by Bluetooth
//! (`kdeconnect.pocketdesk.bluetooth`): kept here by computer for the app.
//! The app dials RFCOMM itself and hands the stream over; the control link
//! then runs on it as at home ([`crate::api::connect_over_bluetooth`]).
//! There is no address to dial back, so for each transfer the computer asks
//! over the control link for a transfer link
//! (`kdeconnect.pocketdesk.bluetooth.link {"id": n}`): the app takes the
//! request, opens a second RFCOMM connection to the transfer service, and
//! hands it to [`offer_link`], which says the link hello, answers the
//! computer's transfer TLS as the home road's transfer listener does, and
//! gives the stream to the transfer its key names.

use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use pocketdesk_core::protocol::bluetooth::{
    BluetoothInfo, LINK_PACKET, PACKET, format_link_hello, link_uuid, parse_address,
};
use tokio::io::AsyncWriteExt;

use crate::Road;

/// How long the computer's transfer TLS may take on a link: longer than the
/// computer's own wait, so the slower radio is never the phone's reason to
/// give up.
const LINK_TLS_WAIT: Duration = Duration::from_secs(15);

/// The most link requests kept waiting for the app: the computer asks for
/// one at a time and waits 10 s for each.
const MAX_LINK_REQUESTS: usize = 8;

/// What each computer last said of its Bluetooth road, by its address: the
/// JSON [`info`] gives.
static INFO: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);

/// The link ids the computer asked for and the app has not taken, oldest
/// first.
static LINK_REQUESTS: LazyLock<Mutex<VecDeque<u64>>> = LazyLock::new(Default::default);

/// A packet from the computer at `from` on a control link that came by
/// `road`: where to dial it by Bluetooth, from any road; a link request
/// only on a Bluetooth control link (no other has a use for one).
pub(crate) fn receive(packet_type: &str, body: &serde_json::Value, from: &str, road: Road) {
    if packet_type == PACKET {
        if let Some(info) = info_of(body) {
            INFO.lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(from.to_string(), info);
        }
    } else if packet_type == LINK_PACKET
        && road == Road::Bluetooth
        && let Some(id) = body.get("id").and_then(|id| id.as_u64())
    {
        let mut requests = LINK_REQUESTS.lock().unwrap_or_else(|e| e.into_inner());
        if requests.len() >= MAX_LINK_REQUESTS {
            requests.pop_front();
        }
        requests.push_back(id);
    }
}

/// A [`PACKET`] body as the app keeps it, `{address, service, link}`: the
/// radio's address, the control service's UUID and the transfer service's
/// ([`link_uuid`]); `None` when the address or the service does not read.
fn info_of(body: &serde_json::Value) -> Option<String> {
    let info: BluetoothInfo = serde_json::from_value(body.clone()).ok()?;
    parse_address(&info.address)?;
    let service = uuid::Uuid::parse_str(&info.service).ok()?;
    Some(
        serde_json::json!({
            "address": info.address.to_ascii_uppercase(),
            "service": service.hyphenated().to_string(),
            "link": link_uuid(service).hyphenated().to_string(),
        })
        .to_string(),
    )
}

/// What the computer at `address` last said of its Bluetooth road, as
/// `{address, service, link}` JSON (`link` the transfer service the app
/// dials for a link request); `""` when it has said nothing.
pub(crate) fn info(address: &str) -> String {
    INFO.lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(address)
        .cloned()
        .unwrap_or_default()
}

/// The oldest link request not yet taken.
pub(crate) fn take_link_request() -> Option<u64> {
    LINK_REQUESTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .pop_front()
}

/// Forget the link requests: the control link that asked has ended.
pub(crate) fn forget_link_requests() {
    LINK_REQUESTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
}

/// `fd`, one end of the app's socket pair whose other end it pumps to and
/// from an RFCOMM socket, as a stream. Closed on failure. Called within the
/// runtime.
#[cfg(unix)]
pub(crate) fn stream_of(fd: std::os::fd::OwnedFd) -> std::io::Result<tokio::net::UnixStream> {
    let stream = std::os::unix::net::UnixStream::from(fd);
    stream.set_nonblocking(true)?;
    tokio::net::UnixStream::from_std(stream)
}

/// The address of the computer the live control link is to, when it came by
/// Bluetooth.
async fn bluetooth_peer() -> Option<String> {
    let peer = crate::CONTROL_PEER.lock().await.clone();
    peer.filter(|_| crate::control_road() == Road::Bluetooth)
        .map(|(host, _)| host)
}

/// Answer the computer's request for link `id` on `stream`, a new RFCOMM
/// connection to its transfer service: the link hello, then the transfer's
/// TLS with the computer the live Bluetooth control link is to (pinned as
/// at home), then the key line, and the stream goes to the transfer it
/// names. Whether a transfer took it.
pub(crate) async fn offer_link<S>(id: u64, stream: S) -> bool
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let Some(address) = bluetooth_peer().await else {
        tracing::warn!("Bluetooth: a transfer link with no Bluetooth control link");
        return false;
    };
    let tls = match crate::api::transfer_acceptor(&address) {
        Ok(tls) => tls,
        Err(e) => {
            tracing::warn!("Bluetooth: no transfer link: {e}");
            return false;
        }
    };
    offer_link_with(id, stream, &tls).await
}

/// [`offer_link`] with the computer's transfer TLS `tls`.
async fn offer_link_with<S>(id: u64, mut stream: S, tls: &tokio_rustls::TlsAcceptor) -> bool
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let hello = format_link_hello(id);
    if stream.write_all(hello.as_bytes()).await.is_err() || stream.flush().await.is_err() {
        tracing::warn!("Bluetooth: the transfer link closed before its hello");
        return false;
    }
    match crate::api::transfer_tls(stream, tls, LINK_TLS_WAIT).await {
        Ok(stream) => crate::link::hand_over(Box::new(stream)).await,
        Err(why) => {
            tracing::warn!("Bluetooth: dropped a transfer link: {why}");
            false
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use pocketdesk_core::streaming::transport::CertManager;
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};

    const SERVICE: &str = "b1f2c4e0-6a3d-4c8e-9f51-2d7e0a9c4b13";

    #[test]
    fn bluetooth_info_kept_per_computer() {
        let body = json!({"address": "00:00:5e:00:53:01", "service": SERVICE});
        receive(PACKET, &body, "192.0.2.10", Road::Home);
        let kept: serde_json::Value = serde_json::from_str(&info("192.0.2.10")).unwrap();
        assert_eq!(
            kept,
            json!({
                "address": "00:00:5E:00:53:01",
                "service": SERVICE,
                "link": "b1f2c4e0-6a3d-4c8e-9f51-2d7e0a9c4b12",
            })
        );
        assert_eq!(info("192.0.2.11"), "", "another computer said nothing");
        // What does not read leaves what was kept.
        for bad in [
            json!({"address": "not an address", "service": SERVICE}),
            json!({"address": "00:00:5E:00:53:02", "service": "not a uuid"}),
            json!({"service": SERVICE}),
        ] {
            receive(PACKET, &bad, "192.0.2.10", Road::Home);
        }
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&info("192.0.2.10")).unwrap(),
            kept
        );
    }

    /// Requests are taken in order; ids are u64s, the largest included.
    #[tokio::test]
    async fn link_request_queue() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget_link_requests();
        receive(
            LINK_PACKET,
            &json!({"id": 7}),
            "192.0.2.12",
            Road::Bluetooth,
        );
        receive(
            LINK_PACKET,
            &json!({"id": u64::MAX}),
            "192.0.2.12",
            Road::Bluetooth,
        );
        // Not on another road, and not without an id.
        receive(LINK_PACKET, &json!({"id": 8}), "192.0.2.12", Road::Home);
        receive(LINK_PACKET, &json!({"id": 9}), "192.0.2.12", Road::Internet);
        receive(
            LINK_PACKET,
            &json!({"id": -1}),
            "192.0.2.12",
            Road::Bluetooth,
        );
        receive(LINK_PACKET, &json!({}), "192.0.2.12", Road::Bluetooth);
        assert_eq!(take_link_request(), Some(7));
        assert_eq!(take_link_request(), Some(u64::MAX));
        assert_eq!(take_link_request(), None);
        for id in 0..20 {
            receive(
                LINK_PACKET,
                &json!({"id": id}),
                "192.0.2.12",
                Road::Bluetooth,
            );
        }
        assert_eq!(
            take_link_request(),
            Some(12),
            "the oldest go past the bound"
        );
        forget_link_requests();
        assert_eq!(take_link_request(), None);
    }

    /// The computer's side of a transfer link: read the hello, then the
    /// transfer TLS as its client, pinned to the phone's certificate, then
    /// the key line.
    async fn computer_answers(
        stream: tokio::net::UnixStream,
        computer: &CertManager,
        phone: [u8; 32],
        key: &str,
    ) -> std::io::Result<(u64, tokio_rustls::client::TlsStream<tokio::net::UnixStream>)> {
        let mut stream = tokio::io::BufReader::new(stream);
        let mut hello = String::new();
        stream.read_line(&mut hello).await?;
        let id = pocketdesk_core::protocol::bluetooth::parse_link_hello(&hello)
            .ok_or_else(|| std::io::Error::other(format!("not a hello: {hello:?}")))?;
        assert!(stream.buffer().is_empty(), "nothing after the hello yet");
        let name = tokio_rustls::rustls::pki_types::ServerName::IpAddress(
            std::net::IpAddr::from(std::net::Ipv4Addr::UNSPECIFIED).into(),
        );
        let mut tls = tokio_rustls::TlsConnector::from(computer.transfer_client_config(phone))
            .connect(name, stream.into_inner())
            .await?;
        tls.write_all(format!("{key}\n").as_bytes()).await?;
        tls.flush().await?;
        Ok((id, tls))
    }

    fn pair() -> (tokio::net::UnixStream, tokio::net::UnixStream) {
        tokio::net::UnixStream::pair().unwrap()
    }

    #[tokio::test]
    async fn a_transfer_link_reaches_its_transfer_inside_tls() {
        let dir = std::env::temp_dir().join(format!("bridge-bt-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let phone = CertManager::load_or_create(&dir).unwrap();
        let computer = CertManager::new().unwrap();
        let label =
            pocketdesk_core::streaming::transport::peer_label_for_address("192.0.2.10").unwrap();
        phone.pin_peer(&label, computer.cert_fingerprint()).unwrap();
        let tls = crate::api::transfer_acceptor_in(&dir, "192.0.2.10").unwrap();

        let key = "0123456789abcdef0123456789abcdee";
        let waiter = crate::link::Waiter::for_key(key);
        let (phone_end, computer_end) = pair();
        let id = u64::MAX - 3;
        let computer_side = tokio::spawn({
            let fingerprint = phone.cert_fingerprint();
            async move { computer_answers(computer_end, &computer, fingerprint, key).await }
        });
        assert!(offer_link_with(id, phone_end, &tls).await, "taken");
        let (said, mut computer_tls) = computer_side.await.unwrap().unwrap();
        assert_eq!(said, id, "the id round-trips");
        let mut stream = waiter.arrived().await.unwrap();
        computer_tls.write_all(b"file bytes").await.unwrap();
        computer_tls.flush().await.unwrap();
        let mut got = [0u8; 10];
        stream.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"file bytes");

        // A computer with another certificate gets no transfer.
        let stranger = CertManager::new().unwrap();
        let key = "0123456789abcdef0123456789abcded";
        let waiter = crate::link::Waiter::for_key(key);
        let (phone_end, computer_end) = pair();
        let stranger_side = tokio::spawn({
            let fingerprint = phone.cert_fingerprint();
            async move { computer_answers(computer_end, &stranger, fingerprint, key).await }
        });
        assert!(!offer_link_with(7, phone_end, &tls).await, "refused");
        let _ = stranger_side.await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), waiter.arrived())
                .await
                .is_err(),
            "the transfer still waits"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn no_link_without_a_bluetooth_control_link() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        let was_peer = crate::CONTROL_PEER.lock().await.clone();
        let was_road = crate::control_road();
        *crate::CONTROL_PEER.lock().await = Some(("192.0.2.10".to_string(), 1716));
        crate::set_control_road(Road::Home);
        let (phone_end, mut computer_end) = pair();
        let offered = offer_link(7, phone_end).await;
        *crate::CONTROL_PEER.lock().await = was_peer;
        crate::set_control_road(was_road);
        assert!(!offered);
        let mut rest = Vec::new();
        computer_end.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty(), "closed, nothing said");
    }
}
