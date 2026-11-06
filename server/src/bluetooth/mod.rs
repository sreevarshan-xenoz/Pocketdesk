//! The Bluetooth road on this computer (the fallback when neither the home
//! nor the internet road reaches it): the listener a bonded phone dials, and
//! where it dials, told over a working road. Each connection the listener
//! accepts is a control link (TLS from its first byte, served by
//! `service::handle_bluetooth_control` under the internet road's rules) or
//! a transfer link a control link asked for (the link hello first,
//! [`links`]). It listens only on Windows for now (Linux is phase 3), while
//! `bluetooth_fallback` is on and the computer has a radio.

mod links;
#[cfg(windows)]
mod windows;

use links::{BluetoothLinks, LinkTable};

use pocketdesk_core::protocol::bluetooth::{BluetoothInfo, LINK_HELLO, PACKET};
use pocketdesk_core::protocol::kdeconnect::NetworkPacket;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, DuplexStream};
use tokio::sync::mpsc;

/// Where this computer listens by Bluetooth now: set when the listener
/// starts, cleared when it stops. Read on every connection's seed, so it
/// is kept here, never asked of the radio there.
struct Listening(RwLock<Option<BluetoothInfo>>);

impl Listening {
    const fn new() -> Self {
        Self(RwLock::new(None))
    }

    fn get(&self) -> Option<BluetoothInfo> {
        self.0.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set(&self, info: Option<BluetoothInfo>) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = info;
    }
}

static LISTENING: Listening = Listening::new();

/// Where a phone dials this computer by Bluetooth, while it listens there;
/// `None` while it does not (the switch off, no radio, or not Windows).
pub(crate) fn info() -> Option<BluetoothInfo> {
    LISTENING.get()
}

/// [`PACKET`] saying `info`.
pub(crate) fn packet(info: &BluetoothInfo) -> NetworkPacket {
    NetworkPacket::new(PACKET).with_body(serde_json::json!({
        "address": info.address,
        "service": info.service,
    }))
}

/// A radio address as `AA:BB:CC:DD:EE:FF`.
fn format_address(address: [u8; 6]) -> String {
    address
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// A control connection over Bluetooth, for the server's loop: its stream
/// (the first byte, read to tell it apart, put back) and the transfer links
/// it asks for.
pub(crate) struct Control {
    pub(crate) stream: Prefixed<DuplexStream>,
    pub(crate) links: Arc<BluetoothLinks>,
}

/// A stream with the one byte already read from it put back in front: the
/// Bluetooth road reads it to tell TLS from anything else, as the TCP road
/// peeks, and a Bluetooth stream cannot peek.
pub(crate) struct Prefixed<S> {
    first: Option<u8>,
    inner: S,
}

impl<S> Prefixed<S> {
    pub(crate) fn new(first: u8, inner: S) -> Self {
        Self {
            first: Some(first),
            inner,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Prefixed<S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if buf.remaining() > 0
            && let Some(byte) = self.first.take()
        {
            buf.put_slice(&[byte]);
            return std::task::Poll::Ready(Ok(()));
        }
        std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for Prefixed<S> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// How long a new connection has to say what it is (its first byte, or the
/// whole link hello).
const HELLO_WAIT: Duration = Duration::from_secs(5);

/// The longest link hello: the word, a space, a `u64` and the newline.
const HELLO_MAX: usize = LINK_HELLO.len() + 1 + 20 + 1;

/// What a new connection's first bytes say it is.
enum Arrival<S> {
    /// TLS: a control link.
    Control(Prefixed<S>),
    /// The link hello: the transfer link with this id.
    Link(u64, S),
}

/// Read what a new connection is: a TLS record or a link hello, within
/// [`HELLO_WAIT`]. Anything else, a hello longer than [`HELLO_MAX`], or
/// silence, is `None`. Nothing past the hello's newline is read.
async fn sort<S: AsyncRead + Unpin>(mut stream: S) -> Option<Arrival<S>> {
    let read = async move {
        let mut byte = [0u8; 1];
        if stream.read(&mut byte).await.ok()? == 0 {
            return None;
        }
        if byte[0] == pocketdesk_core::protocol::TLS_HANDSHAKE_RECORD {
            return Some(Arrival::Control(Prefixed::new(byte[0], stream)));
        }
        let mut line = vec![byte[0]];
        while line.len() < HELLO_MAX && byte[0] != b'\n' {
            if !LINK_HELLO
                .as_bytes()
                .starts_with(&line[..line.len().min(LINK_HELLO.len())])
            {
                return None;
            }
            if stream.read(&mut byte).await.ok()? == 0 {
                return None;
            }
            line.push(byte[0]);
        }
        if byte[0] != b'\n' {
            return None;
        }
        let id = pocketdesk_core::protocol::bluetooth::parse_link_hello(
            std::str::from_utf8(&line).ok()?,
        )?;
        Some(Arrival::Link(id, stream))
    };
    tokio::time::timeout(HELLO_WAIT, read).await.ok().flatten()
}

/// Sort each connection the listener accepts, each on a task of its own: a
/// transfer link goes to the transfer waiting for its id, a control link to
/// `control` with links of its own; anything else is dropped.
async fn serve(
    mut accepted: mpsc::Receiver<DuplexStream>,
    table: Arc<LinkTable>,
    control: mpsc::Sender<Control>,
) {
    while let Some(stream) = accepted.recv().await {
        let table = table.clone();
        let control = control.clone();
        tokio::spawn(async move {
            match sort(stream).await {
                Some(Arrival::Control(stream)) => {
                    let links = Arc::new(BluetoothLinks::new(table));
                    let _ = control.send(Control { stream, links }).await;
                }
                Some(Arrival::Link(id, stream)) => {
                    if !table.offer(id, stream) {
                        tracing::info!("Dropped a Bluetooth transfer link nothing waits for");
                    }
                }
                None => tracing::info!(
                    "Dropped a Bluetooth connection that sent neither TLS nor a link hello"
                ),
            }
        });
    }
}

/// A radio this computer can listen on.
trait Radio: Send + Sync + 'static {
    /// The radio's address, when there is a radio.
    fn address(&self) -> Option<[u8; 6]>;

    /// Listen on `service`, each connection accepted going to `accepted`,
    /// until the returned listener is dropped.
    fn listen(
        &self,
        service: uuid::Uuid,
        accepted: mpsc::Sender<DuplexStream>,
    ) -> std::io::Result<Box<dyn Listener>>;
}

/// A running listener; dropping it stops it (and takes its SDP record
/// down).
trait Listener: Send {
    /// Whether it stopped by itself (the radio went, say).
    fn ended(&self) -> bool;
}

/// No radio this computer listens on yet (Linux is phase 3).
#[cfg(not(windows))]
struct NoRadio;

#[cfg(not(windows))]
impl Radio for NoRadio {
    fn address(&self) -> Option<[u8; 6]> {
        None
    }

    fn listen(
        &self,
        _service: uuid::Uuid,
        _accepted: mpsc::Sender<DuplexStream>,
    ) -> std::io::Result<Box<dyn Listener>> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

/// Windows' radio, by Winsock.
#[cfg(windows)]
struct Winsock;

#[cfg(windows)]
struct WinsockListener(Arc<windows::Listener>);

#[cfg(windows)]
impl Listener for WinsockListener {
    fn ended(&self) -> bool {
        self.0.is_closed()
    }
}

#[cfg(windows)]
impl Drop for WinsockListener {
    fn drop(&mut self) {
        self.0.close();
    }
}

#[cfg(windows)]
impl Radio for Winsock {
    fn address(&self) -> Option<[u8; 6]> {
        windows::radio_address()
    }

    fn listen(
        &self,
        service: uuid::Uuid,
        accepted: mpsc::Sender<DuplexStream>,
    ) -> std::io::Result<Box<dyn Listener>> {
        let runtime = tokio::runtime::Handle::try_current().map_err(std::io::Error::other)?;
        let listener = Arc::new(windows::listen(service)?);
        windows::spawn_accepting(listener.clone(), accepted, runtime)?;
        Ok(Box::new(WinsockListener(listener)))
    }
}

fn radio() -> Arc<dyn Radio> {
    #[cfg(windows)]
    let radio = Arc::new(Winsock);
    #[cfg(not(windows))]
    let radio = Arc::new(NoRadio);
    radio
}

/// How often the listener checks the switch and the radio.
const CHECK_EVERY: Duration = Duration::from_secs(10);

/// Listen while `wanted()` (the `bluetooth_fallback` switch) is on and
/// `radio` has an address, checked every `every`: start when both hold,
/// stop (the SDP record taken down) when either goes, or the radio's
/// address changes, and start again after a listener that ended by itself.
/// `state` says where it listens meanwhile.
async fn keep_listening(
    radio: Arc<dyn Radio>,
    state: &Listening,
    wanted: impl Fn() -> bool,
    accepted: mpsc::Sender<DuplexStream>,
    every: Duration,
) {
    let mut running: Option<(Box<dyn Listener>, [u8; 6])> = None;
    let mut failure_said = false;
    loop {
        let address = if wanted() {
            let radio = radio.clone();
            tokio::task::spawn_blocking(move || radio.address())
                .await
                .ok()
                .flatten()
        } else {
            None
        };
        if running
            .as_ref()
            .is_some_and(|(listener, at)| listener.ended() || address != Some(*at))
        {
            running = None;
            state.set(None);
            tracing::info!("Stopped listening over Bluetooth");
        }
        if running.is_none()
            && let Some(at) = address
        {
            let service = pocketdesk_core::protocol::bluetooth::service_uuid();
            let started = match uuid::Uuid::parse_str(&service) {
                Ok(uuid) => {
                    let radio = radio.clone();
                    let accepted = accepted.clone();
                    tokio::task::spawn_blocking(move || radio.listen(uuid, accepted))
                        .await
                        .unwrap_or_else(|e| Err(std::io::Error::other(e)))
                }
                Err(e) => Err(std::io::Error::other(e)),
            };
            match started {
                Ok(listener) => {
                    tracing::info!("Listening over Bluetooth for service {service}");
                    state.set(Some(BluetoothInfo {
                        address: format_address(at),
                        service,
                    }));
                    running = Some((listener, at));
                    failure_said = false;
                }
                Err(e) => {
                    if !failure_said {
                        tracing::warn!("Could not listen over Bluetooth: {e}");
                        failure_said = true;
                    }
                }
            }
        }
        tokio::time::sleep(every).await;
    }
}

/// The Bluetooth road's listener, for the server's life: listens while
/// `bluetooth_fallback` is on and a radio is present (Windows only for
/// now), and hands each control link to `control`.
pub(crate) async fn run(control: mpsc::Sender<Control>) {
    let (accepted_tx, accepted_rx) = mpsc::channel(8);
    tokio::spawn(serve(accepted_rx, Arc::default(), control));
    keep_listening(
        radio(),
        &LISTENING,
        || crate::events::current_config().bluetooth_fallback,
        accepted_tx,
        CHECK_EVERY,
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::protocol::bluetooth::format_link_hello;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::io::AsyncWriteExt;

    #[test]
    fn an_address_is_written_as_the_packet_reads_it() {
        let address = [0x00, 0x00, 0x5E, 0x00, 0x53, 0x01];
        assert_eq!(format_address(address), "00:00:5E:00:53:01");
        assert_eq!(
            pocketdesk_core::protocol::bluetooth::parse_address(&format_address(address)),
            Some(address)
        );
    }

    /// The listener's side, in memory: what `serve` hands on.
    fn serving() -> (
        mpsc::Sender<DuplexStream>,
        Arc<LinkTable>,
        mpsc::Receiver<Control>,
    ) {
        let (accepted_tx, accepted_rx) = mpsc::channel(8);
        let (control_tx, control_rx) = mpsc::channel(8);
        let table = Arc::new(LinkTable::default());
        tokio::spawn(serve(accepted_rx, table.clone(), control_tx));
        (accepted_tx, table, control_rx)
    }

    /// A connection from the phone that sends `first`, and its phone end.
    async fn dial(accepted: &mpsc::Sender<DuplexStream>, first: &[u8]) -> DuplexStream {
        let (server_end, mut phone_end) = tokio::io::duplex(64 * 1024);
        accepted.send(server_end).await.unwrap();
        phone_end.write_all(first).await.unwrap();
        phone_end
    }

    async fn read_some(stream: &mut (impl AsyncRead + Unpin)) -> Vec<u8> {
        let mut buf = vec![0u8; 64];
        let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
            .await
            .expect("bytes in time")
            .unwrap();
        buf.truncate(read);
        buf
    }

    #[tokio::test]
    async fn a_link_hello_goes_to_the_transfer_waiting_for_it() {
        let (accepted, table, _control) = serving();
        let (waiting, arrived) = table.wait();
        let mut hello = format_link_hello(waiting.id).into_bytes();
        hello.extend_from_slice(b"after");
        let mut phone = dial(&accepted, &hello).await;
        let mut link = arrived.await.expect("the link arrived");
        assert_eq!(
            read_some(&mut link).await,
            b"after",
            "nothing past the hello was read"
        );
        link.write_all(b"back").await.unwrap();
        assert_eq!(read_some(&mut phone).await, b"back");
    }

    #[tokio::test]
    async fn tls_goes_to_the_control_loop_with_its_first_byte() {
        let (accepted, _table, mut control) = serving();
        let record = [pocketdesk_core::protocol::TLS_HANDSHAKE_RECORD, 3, 1];
        let _phone = dial(&accepted, &record).await;
        let mut link = control.recv().await.expect("a control link");
        let mut got = [0u8; 3];
        link.stream.read_exact(&mut got).await.unwrap();
        assert_eq!(got, record);
    }

    #[tokio::test]
    async fn anything_else_is_dropped() {
        let (accepted, table, mut control) = serving();
        let (waiting, _arrived) = table.wait();
        for first in [
            b"GET / HTTP/1.1\r\n".to_vec(),
            b"{\"id\":1}\n".to_vec(),
            format!("POCKETDESK-BT-LINK {}x\n", waiting.id).into_bytes(),
            format!("POCKETDESK-BT-LINK {}{}\n", waiting.id, "0".repeat(40)).into_bytes(),
        ] {
            let mut phone = dial(&accepted, &first).await;
            let mut rest = Vec::new();
            tokio::time::timeout(Duration::from_secs(5), phone.read_to_end(&mut rest))
                .await
                .expect("closed in time")
                .unwrap();
            assert!(rest.is_empty(), "{first:?}");
        }
        assert!(control.try_recv().is_err(), "no control link");
    }

    #[tokio::test(start_paused = true)]
    async fn a_hello_that_never_ends_is_dropped_in_time() {
        let (server_end, mut phone_end) = tokio::io::duplex(1024);
        phone_end.write_all(b"POCKETDESK-BT").await.unwrap();
        assert!(sort(server_end).await.is_none());
        let (server_end, _silent) = tokio::io::duplex(1024);
        assert!(sort(server_end).await.is_none(), "silence");
    }

    /// A radio in memory: its address, and whether its listener runs.
    #[derive(Default)]
    struct FakeRadio {
        address: std::sync::Mutex<Option<[u8; 6]>>,
        running: Arc<AtomicBool>,
        ended: Arc<AtomicBool>,
    }

    struct FakeListener {
        running: Arc<AtomicBool>,
        ended: Arc<AtomicBool>,
    }

    impl Listener for FakeListener {
        fn ended(&self) -> bool {
            self.ended.load(Ordering::SeqCst)
        }
    }

    impl Drop for FakeListener {
        fn drop(&mut self) {
            self.running.store(false, Ordering::SeqCst);
        }
    }

    impl Radio for FakeRadio {
        fn address(&self) -> Option<[u8; 6]> {
            *self.address.lock().unwrap()
        }

        fn listen(
            &self,
            _service: uuid::Uuid,
            _accepted: mpsc::Sender<DuplexStream>,
        ) -> std::io::Result<Box<dyn Listener>> {
            self.ended.store(false, Ordering::SeqCst);
            self.running.store(true, Ordering::SeqCst);
            Ok(Box::new(FakeListener {
                running: self.running.clone(),
                ended: self.ended.clone(),
            }))
        }
    }

    async fn until(what: &str, check: impl Fn() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !check() {
            assert!(tokio::time::Instant::now() < deadline, "{what}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn listens_only_while_the_switch_is_on_and_a_radio_is_there() {
        let radio = Arc::new(FakeRadio::default());
        let state: &'static Listening = Box::leak(Box::new(Listening::new()));
        let on = Arc::new(AtomicBool::new(true));
        let (accepted, _) = mpsc::channel(1);
        let task = tokio::spawn({
            let radio: Arc<dyn Radio> = radio.clone();
            let on = on.clone();
            keep_listening(
                radio,
                state,
                move || on.load(Ordering::SeqCst),
                accepted,
                Duration::from_millis(5),
            )
        });
        let running = || radio.running.load(Ordering::SeqCst);

        // No radio: nothing listens, nothing is told.
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!running());
        assert_eq!(state.get(), None);

        // A radio: it listens, and says where.
        *radio.address.lock().unwrap() = Some([0x00, 0x00, 0x5E, 0x00, 0x53, 0x01]);
        until("listening", || running() && state.get().is_some()).await;
        let told = state.get().unwrap();
        assert_eq!(told.address, "00:00:5E:00:53:01");
        assert_eq!(
            told.service,
            pocketdesk_core::protocol::bluetooth::service_uuid()
        );

        // The switch off: it stops, and tells nothing.
        on.store(false, Ordering::SeqCst);
        until("stopped", || !running() && state.get().is_none()).await;

        // On again: it listens again; a listener that ends by itself is
        // started again.
        on.store(true, Ordering::SeqCst);
        until("listening again", || running() && state.get().is_some()).await;
        radio.ended.store(true, Ordering::SeqCst);
        until("restarted", || {
            !radio.ended.load(Ordering::SeqCst) && running()
        })
        .await;

        // The radio gone: it stops.
        *radio.address.lock().unwrap() = None;
        until("stopped with the radio", || {
            !running() && state.get().is_none()
        })
        .await;
        task.abort();
    }
}
