//! Transfer links on the Bluetooth road: a control link over RFCOMM has no
//! address to dial back and no streams of its own, so for each transfer the
//! computer asks the phone over it ([`LINK_PACKET`]) to open a second
//! RFCOMM connection, to the transfer service (RFCOMM carries one
//! connection per channel between two devices, so not to the control
//! link's own). That connection's first line is the link hello with the id
//! asked for; the listener hands it here ([`LinkTable::offer`]), and the
//! computer runs the home road's transfer TLS on it as the client, holding
//! the phone to the key its control link proved. For the same reason a
//! phone has one transfer link at a time: the next waits its turn.

use pocketdesk_core::protocol::bluetooth::LINK_PACKET;
use pocketdesk_core::protocol::kdeconnect::{
    DeviceSender, NetworkPacket, OpenLinkStream, TransferIo,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::io::DuplexStream;
use tokio::sync::oneshot;

/// How long the computer waits for the phone's transfer link once it has
/// asked for it.
pub(crate) const LINK_WAIT: Duration = Duration::from_secs(10);

/// How long a transfer waits for the phone's previous transfer link to
/// close before it gives up.
pub(crate) const TURN_WAIT: Duration = Duration::from_secs(60);

/// The transfer links asked for and not yet arrived, by link id, for every
/// control link on the Bluetooth road.
#[derive(Default)]
pub(crate) struct LinkTable {
    waiting: Mutex<HashMap<u64, oneshot::Sender<DuplexStream>>>,
}

impl LinkTable {
    /// A new link id, random and not waiting already, and where its
    /// connection arrives. The id waits until the [`Waiting`] is dropped.
    pub(super) fn wait(self: &Arc<Self>) -> (Waiting, oneshot::Receiver<DuplexStream>) {
        let (tx, rx) = oneshot::channel();
        let mut waiting = self.waiting.lock().unwrap_or_else(|e| e.into_inner());
        let id = loop {
            let id = rand::random::<u64>();
            if !waiting.contains_key(&id) {
                break id;
            }
        };
        waiting.insert(id, tx);
        (
            Waiting {
                table: self.clone(),
                id,
            },
            rx,
        )
    }

    /// Hand the phone's connection for link `id` to the transfer waiting
    /// for it. `false`, and the stream dropped, when nothing waits for that
    /// id: an id is taken once.
    pub(crate) fn offer(&self, id: u64, stream: DuplexStream) -> bool {
        let taken = self
            .waiting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        match taken {
            Some(tx) => tx.send(stream).is_ok(),
            None => false,
        }
    }
}

/// A link id waiting in its table, taken out when the wait ends, however
/// it ends.
pub(super) struct Waiting {
    table: Arc<LinkTable>,
    pub(super) id: u64,
}

impl Drop for Waiting {
    fn drop(&mut self) {
        self.table
            .waiting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}

/// The transfer links of one control link on the Bluetooth road. It learns
/// the control link's sender once the phone is admitted, and asks over it.
pub(crate) struct BluetoothLinks {
    table: Arc<LinkTable>,
    control: Mutex<Option<Weak<dyn DeviceSender>>>,
    /// One transfer link at a time: its permit lives as long as the link.
    turn: Arc<tokio::sync::Semaphore>,
}

impl BluetoothLinks {
    pub(crate) fn new(table: Arc<LinkTable>) -> Self {
        Self {
            table,
            control: Mutex::new(None),
            turn: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }
}

/// A transfer link holding its phone's turn until it is dropped.
struct Turned<S> {
    stream: S,
    _turn: tokio::sync::OwnedSemaphorePermit,
}

impl<S: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for Turned<S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl<S: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for Turned<S> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

/// The name the transfer's TLS is opened with on a link: none is checked
/// (the phone's key is pinned), and an address sends no name.
fn link_name() -> tokio_rustls::rustls::pki_types::ServerName<'static> {
    tokio_rustls::rustls::pki_types::ServerName::IpAddress(
        std::net::IpAddr::from(std::net::Ipv4Addr::UNSPECIFIED).into(),
    )
}

#[async_trait::async_trait]
impl OpenLinkStream for BluetoothLinks {
    async fn open(&self) -> std::io::Result<Box<dyn TransferIo>> {
        let control = self
            .control
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "no admitted control link to ask on",
                )
            })?;
        // Checked before the phone is asked: the key the control link
        // proved, held to the bound one.
        let tls = crate::plugins::share::transfer_tls(&*control).map_err(std::io::Error::other)?;
        // The phone's previous transfer link must close first: one RFCOMM
        // connection per channel.
        let turn = match tokio::time::timeout(TURN_WAIT, self.turn.clone().acquire_owned()).await {
            Ok(Ok(turn)) => turn,
            Ok(Err(_)) => return Err(std::io::Error::other("the Bluetooth link is closed")),
            Err(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "another file is still crossing over Bluetooth; this one waited its turn \
                     too long",
                ));
            }
        };
        let (waiting, arrived) = self.table.wait();
        let ask =
            NetworkPacket::new(LINK_PACKET).with_body(serde_json::json!({ "id": waiting.id }));
        control
            .send_packet(&ask)
            .await
            .map_err(std::io::Error::other)?;
        drop(control);
        let stream = match tokio::time::timeout(LINK_WAIT, arrived).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(_)) => return Err(std::io::Error::other("the transfer link was dropped")),
            Err(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "the phone did not open the transfer link in time",
                ));
            }
        };
        drop(waiting);
        let stream = crate::plugins::share::transfer_tls_on(stream, link_name(), tls)
            .await
            .map_err(std::io::Error::other)?;
        Ok(Box::new(Turned {
            stream,
            _turn: turn,
        }))
    }

    fn admitted(&self, control: Weak<dyn DeviceSender>) {
        *self.control.lock().unwrap_or_else(|e| e.into_inner()) = Some(control);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::error::Result;
    use pocketdesk_core::streaming::transport::CertManager;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// The phone's control link, as the links see it: what is sent on it
    /// goes to the test.
    struct Control {
        id: String,
        key: [u8; 32],
        sent: tokio::sync::mpsc::UnboundedSender<NetworkPacket>,
    }

    #[async_trait::async_trait]
    impl DeviceSender for Control {
        fn device_id(&self) -> &str {
            &self.id
        }
        fn connection_id(&self) -> &str {
            "conn"
        }
        fn peer_key(&self) -> Option<pocketdesk_core::streaming::PeerKey> {
            Some(pocketdesk_core::streaming::PeerKey::Tls(self.key))
        }
        fn nearby_by_radio(&self) -> bool {
            true
        }
        async fn send_packet(&self, packet: &NetworkPacket) -> Result<()> {
            let _ = self.sent.send(packet.clone());
            Ok(())
        }
    }

    fn phone_certs() -> &'static (CertManager, CertManager) {
        static CERTS: std::sync::OnceLock<(CertManager, CertManager)> = std::sync::OnceLock::new();
        CERTS.get_or_init(|| (CertManager::new().unwrap(), CertManager::new().unwrap()))
    }

    /// Links for a control link from a phone (not paired here) whose
    /// control link proved `key`; the packets it is sent, and the sender,
    /// which must outlive the test's use of the links.
    fn admitted(
        key: [u8; 32],
    ) -> (
        Arc<BluetoothLinks>,
        tokio::sync::mpsc::UnboundedReceiver<NetworkPacket>,
        Arc<dyn DeviceSender>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let control: Arc<dyn DeviceSender> = Arc::new(Control {
            id: format!("bt-link-{}", uuid::Uuid::new_v4().simple()),
            key,
            sent: tx,
        });
        let links = Arc::new(BluetoothLinks::new(Arc::default()));
        links.admitted(Arc::downgrade(&control));
        (links, rx, control)
    }

    fn asked_id(packet: &NetworkPacket) -> u64 {
        assert_eq!(packet.packet_type, LINK_PACKET);
        packet.body["id"].as_u64().expect("a u64 id")
    }

    /// The phone's side of a transfer link, presenting `phone`'s
    /// certificate as its transfer listener does at home.
    async fn phone_answers(
        phone: &CertManager,
        stream: DuplexStream,
    ) -> std::io::Result<tokio_rustls::server::TlsStream<DuplexStream>> {
        let desktop = crate::plugins::share::desktop_certs().unwrap();
        tokio_rustls::TlsAcceptor::from(
            phone
                .transfer_server_config(desktop.cert_fingerprint())
                .unwrap(),
        )
        .accept(stream)
        .await
    }

    #[tokio::test]
    async fn open_waits_for_the_phones_link() {
        let _state = crate::state::use_test_dirs();
        let (phone, _) = phone_certs();
        let (links, mut asked, _control) = admitted(phone.cert_fingerprint());
        let opening = tokio::spawn({
            let links = links.clone();
            async move { links.open().await }
        });
        let id = asked_id(&asked.recv().await.unwrap());
        let (computer_end, phone_end) = tokio::io::duplex(64 * 1024);
        assert!(links.table.offer(id, computer_end));
        let mut phone_side = phone_answers(phone, phone_end).await.unwrap();
        let mut opened = opening.await.unwrap().expect("the link, inside TLS");
        phone_side.write_all(b"from the phone").await.unwrap();
        phone_side.flush().await.unwrap();
        let mut got = [0u8; 14];
        opened.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"from the phone");
        let (again, _) = tokio::io::duplex(1024);
        assert!(!links.table.offer(id, again), "an id is taken once");
    }

    /// Open one link, the phone answering as it should.
    async fn open_one(
        links: &Arc<BluetoothLinks>,
        asked: &mut tokio::sync::mpsc::UnboundedReceiver<NetworkPacket>,
    ) -> (
        Box<dyn TransferIo>,
        tokio_rustls::server::TlsStream<DuplexStream>,
    ) {
        let opening = tokio::spawn({
            let links = links.clone();
            async move { links.open().await }
        });
        let id = asked_id(&asked.recv().await.unwrap());
        let (computer_end, phone_end) = tokio::io::duplex(64 * 1024);
        assert!(links.table.offer(id, computer_end));
        let phone_side = phone_answers(&phone_certs().0, phone_end).await.unwrap();
        (opening.await.unwrap().unwrap(), phone_side)
    }

    /// RFCOMM takes one connection per channel: the next link is asked for
    /// only once the last one is closed.
    #[tokio::test]
    async fn one_link_at_a_time() {
        let _state = crate::state::use_test_dirs();
        let (links, mut asked, _control) = admitted(phone_certs().0.cert_fingerprint());
        let (first, _phone_side) = open_one(&links, &mut asked).await;
        let second = tokio::spawn({
            let links = links.clone();
            async move { links.open().await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            asked.try_recv().is_err(),
            "not asked while the first is open"
        );
        drop(first);
        let id = tokio::time::timeout(Duration::from_secs(5), asked.recv())
            .await
            .expect("asked once the first closed")
            .map(|p| asked_id(&p))
            .unwrap();
        let (computer_end, phone_end) = tokio::io::duplex(64 * 1024);
        assert!(links.table.offer(id, computer_end));
        let _phone_side = phone_answers(&phone_certs().0, phone_end).await.unwrap();
        assert!(second.await.unwrap().is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn a_link_waiting_too_long_for_its_turn_says_why() {
        let _state = crate::state::use_test_dirs();
        let (links, mut asked, _control) = admitted(phone_certs().0.cert_fingerprint());
        let (_first, _phone_side) = open_one(&links, &mut asked).await;
        let error = links.open().await.err().expect("no turn came");
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(error.to_string().contains("another file"), "{error}");
        assert!(asked.try_recv().is_err(), "the phone was never asked");
    }

    #[tokio::test]
    async fn a_phone_with_another_key_gets_no_link() {
        let _state = crate::state::use_test_dirs();
        let (phone, stranger) = phone_certs();
        let (links, mut asked, _control) = admitted(phone.cert_fingerprint());
        let opening = tokio::spawn({
            let links = links.clone();
            async move { links.open().await }
        });
        let id = asked_id(&asked.recv().await.unwrap());
        let (computer_end, phone_end) = tokio::io::duplex(64 * 1024);
        assert!(links.table.offer(id, computer_end));
        let _ = phone_answers(stranger, phone_end).await;
        assert!(opening.await.unwrap().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn open_times_out() {
        let _state = crate::state::use_test_dirs();
        let (links, mut asked, _control) = admitted(phone_certs().0.cert_fingerprint());
        let opened = links.open().await;
        let error = opened.err().expect("no link came");
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        let id = asked_id(&asked.recv().await.unwrap());
        let (late, _) = tokio::io::duplex(1024);
        assert!(!links.table.offer(id, late), "the id waits no longer");
    }

    #[tokio::test]
    async fn offer_unknown_id_is_refused() {
        let table = LinkTable::default();
        let (computer_end, mut phone_end) = tokio::io::duplex(1024);
        assert!(!table.offer(7, computer_end));
        let mut rest = Vec::new();
        phone_end.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty(), "the stream was dropped");
    }

    #[tokio::test]
    async fn open_before_admission_asks_nothing() {
        let links = BluetoothLinks::new(Arc::default());
        let error = links.open().await.err().expect("nothing to ask on");
        assert_eq!(error.kind(), std::io::ErrorKind::NotConnected);
    }
}
