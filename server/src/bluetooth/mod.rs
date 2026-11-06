//! The Bluetooth road on this computer (the fallback when neither the home
//! nor the internet road reaches it): the listener a bonded phone dials, and
//! where it dials, told over a working road. It listens on two services:
//! the control service, whose connections are control links (TLS from the
//! first byte, served by `service::handle_bluetooth_control` under the
//! internet road's rules), and the transfer service
//! ([`link_uuid`](pocketdesk_core::protocol::bluetooth::link_uuid)), whose
//! connections are transfer links a control link asked for (the link hello
//! first, [`links`]): RFCOMM carries one connection per channel between two
//! devices, so a transfer cannot share the control link's channel. It
//! listens only on Windows for now (Linux is phase 3), while
//! `bluetooth_fallback` is on and the computer has a radio.

mod links;
#[cfg(windows)]
mod windows;

#[cfg(test)]
pub(crate) use links::turn;
use links::{BluetoothLinks, LinkTable};
pub(crate) use links::{ONE_AT_A_TIME, busy as transfer_busy};

use pocketdesk_core::protocol::bluetooth::{BluetoothInfo, LINK_HELLO, PACKET, RoadState};
use pocketdesk_core::protocol::kdeconnect::NetworkPacket;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, DuplexStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Where this computer listens by Bluetooth now, and the token that ends
/// what came while it listened. Set when the listener starts, cleared (and
/// every connection it took ended) when it stops. The address is read on
/// every connection's seed, so it is kept here, never asked of the radio
/// there.
struct Listening {
    info: RwLock<Option<BluetoothInfo>>,
    period: std::sync::Mutex<CancellationToken>,
    /// How the road stood at the last check; `None` before the first.
    road: RwLock<Option<RoadState>>,
}

impl Listening {
    fn new() -> Self {
        let ended = CancellationToken::new();
        ended.cancel();
        Self {
            info: RwLock::new(None),
            period: std::sync::Mutex::new(ended),
            road: RwLock::new(None),
        }
    }

    fn road(&self) -> Option<RoadState> {
        *self.road.read().unwrap_or_else(|e| e.into_inner())
    }

    fn set_road(&self, road: RoadState) {
        *self.road.write().unwrap_or_else(|e| e.into_inner()) = Some(road);
    }

    fn get(&self) -> Option<BluetoothInfo> {
        self.info.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// About to listen: connections from now on live until [`stop`]. Made
    /// before the sockets are, so the first connection is not born ended.
    ///
    /// [`stop`]: Self::stop
    fn begin(&self) {
        *self.period.lock().unwrap_or_else(|e| e.into_inner()) = CancellationToken::new();
    }

    /// Listening at `info` (after [`begin`]): it is told from now on.
    ///
    /// [`begin`]: Self::begin
    fn listening_at(&self, info: BluetoothInfo) {
        *self.info.write().unwrap_or_else(|e| e.into_inner()) = Some(info);
    }

    /// [`begin`] and [`listening_at`] at once (the tests').
    ///
    /// [`begin`]: Self::begin
    /// [`listening_at`]: Self::listening_at
    #[cfg(test)]
    fn start(&self, info: BluetoothInfo) {
        self.begin();
        self.listening_at(info);
    }

    /// Not listening: nothing is told, and every connection taken while it
    /// listened ends (its control link, and with it the away lock's
    /// presence, and its transfer links).
    fn stop(&self) {
        *self.info.write().unwrap_or_else(|e| e.into_inner()) = None;
        self.period
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .cancel();
    }

    /// The token of the period a connection arrives in.
    fn period(&self) -> CancellationToken {
        self.period
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

static LISTENING: std::sync::LazyLock<Listening> = std::sync::LazyLock::new(Listening::new);

/// Where a phone dials this computer by Bluetooth, while it listens there;
/// `None` while it does not (the switch off, no radio, or not Windows).
pub(crate) fn info() -> Option<BluetoothInfo> {
    LISTENING.get()
}

/// How the road stands, as last checked (for the live mirror, so
/// `pocketdesk status` and the desktop say it); `None` before the first
/// check. Cheap: no radio is asked.
pub(crate) fn road() -> Option<RoadState> {
    LISTENING.road()
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

/// An accepted connection, as the server sees it: it ends (reads and
/// writes fail) once the listener it came by stops.
pub(crate) type Stream = Endable<DuplexStream>;

/// A stream that fails from the moment its token is cancelled: a
/// Bluetooth connection, once the road is turned off. The server's own
/// read then ends it, with the cleanup a dropped connection gets.
pub(crate) struct Endable<S> {
    inner: S,
    token: CancellationToken,
    /// One per side, each with its own waker: a control link split into a
    /// reader and a writer has both woken when the road goes.
    read_ended: std::pin::Pin<Box<tokio_util::sync::WaitForCancellationFutureOwned>>,
    write_ended: std::pin::Pin<Box<tokio_util::sync::WaitForCancellationFutureOwned>>,
    closed: Option<tokio::sync::oneshot::Receiver<()>>,
}

impl<S> Endable<S> {
    pub(crate) fn new(inner: S, token: CancellationToken) -> Self {
        Self {
            inner,
            read_ended: Box::pin(token.clone().cancelled_owned()),
            write_ended: Box::pin(token.clone().cancelled_owned()),
            token,
            closed: None,
        }
    }

    /// With `closed`, which resolves once the connection's socket has
    /// closed.
    fn closing(mut self, closed: Option<tokio::sync::oneshot::Receiver<()>>) -> Self {
        self.closed = closed;
        self
    }

    /// What resolves once the connection's socket has closed, if known.
    pub(crate) fn take_closed(&mut self) -> Option<tokio::sync::oneshot::Receiver<()>> {
        self.closed.take()
    }

    /// `Err` once the token is cancelled; the reading (or writing) task is
    /// woken then.
    fn check(&mut self, cx: &mut std::task::Context<'_>, read: bool) -> std::io::Result<()> {
        use std::future::Future;
        let ended = if read {
            self.read_ended.as_mut()
        } else {
            self.write_ended.as_mut()
        };
        if self.token.is_cancelled() || ended.poll(cx).is_ready() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "the Bluetooth road was turned off",
            ));
        }
        Ok(())
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Endable<S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if let Err(e) = self.check(cx, true) {
            return std::task::Poll::Ready(Err(e));
        }
        std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for Endable<S> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        if let Err(e) = self.check(cx, false) {
            return std::task::Poll::Ready(Err(e));
        }
        std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if let Err(e) = self.check(cx, false) {
            return std::task::Poll::Ready(Err(e));
        }
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// A control connection over Bluetooth, for the server's loop: its stream
/// (the first byte, read to tell it apart, put back) and the transfer links
/// it asks for.
pub(crate) struct Control {
    pub(crate) stream: Prefixed<Stream>,
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

/// A connection the listener accepted: the service it came to, the
/// server's end of it, and what resolves once its socket has closed.
pub(crate) struct Incoming {
    pub(crate) kind: Kind,
    pub(crate) stream: DuplexStream,
    pub(crate) closed: Option<tokio::sync::oneshot::Receiver<()>>,
}

/// Which of the two services a connection came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// The control service: TLS only.
    Control,
    /// The transfer service: a link hello only.
    // Only the Windows radio listens yet.
    #[cfg_attr(not(windows), allow(dead_code))]
    Link,
}

/// How long a new transfer connection has to send its whole link hello.
const HELLO_WAIT: Duration = Duration::from_secs(5);

/// How long a new control connection has to send its first byte: longer,
/// as the phone may hold its RFCOMM socket open a while before its TLS
/// starts (another dial of its own finishing first). Still bounded by the
/// open connections' cap and the control link's own timeouts after it.
const CONTROL_HELLO_WAIT: Duration = Duration::from_secs(20);

/// The longest link hello: the word, a space, a `u64` and the newline.
const HELLO_MAX: usize = LINK_HELLO.len() + 1 + 20 + 1;

/// What a new connection's first bytes say it is.
enum Arrival<S> {
    /// TLS: a control link.
    Control(Prefixed<S>),
    /// The link hello: the transfer link with this id.
    Link(u64, S),
}

/// Read what a new connection to the `kind` service is, within
/// [`CONTROL_HELLO_WAIT`] (control) or [`HELLO_WAIT`] (transfer): a TLS record on the control service, a link hello on the
/// transfer service. Anything else (either on the other's service
/// included), a hello longer than [`HELLO_MAX`], or silence, is `None`.
/// Nothing past the hello's newline is read.
async fn sort<S: AsyncRead + Unpin>(mut stream: S, kind: Kind) -> Option<Arrival<S>> {
    let read = async move {
        let mut byte = [0u8; 1];
        if stream.read(&mut byte).await.ok()? == 0 {
            return None;
        }
        if kind == Kind::Control {
            return (byte[0] == pocketdesk_core::protocol::TLS_HANDSHAKE_RECORD)
                .then(|| Arrival::Control(Prefixed::new(byte[0], stream)));
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
    let wait = match kind {
        Kind::Control => CONTROL_HELLO_WAIT,
        Kind::Link => HELLO_WAIT,
    };
    tokio::time::timeout(wait, read).await.ok().flatten()
}

/// Sort each connection the listener accepts, each on a task of its own: a
/// transfer link goes to the transfer waiting for its id, a control link to
/// `control` with links of its own; anything else is dropped. Each lives
/// only while `state` listens.
async fn serve(
    mut accepted: mpsc::Receiver<Incoming>,
    table: Arc<LinkTable>,
    control: mpsc::Sender<Control>,
    state: &'static Listening,
) {
    while let Some(Incoming {
        kind,
        stream,
        closed,
    }) = accepted.recv().await
    {
        let table = table.clone();
        let control = control.clone();
        let stream = Endable::new(stream, state.period()).closing(closed);
        tokio::spawn(async move {
            match sort(stream, kind).await {
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
                    "Dropped a Bluetooth connection that did not open as its service asks ({kind:?})"
                ),
            }
        });
    }
}

/// A radio this computer can listen on.
trait Radio: Send + Sync + 'static {
    /// The radio's address, when there is a radio.
    fn address(&self) -> Option<[u8; 6]>;

    /// Listen on the control service `control` and the transfer service
    /// `link`, each connection accepted going to `accepted` with its kind,
    /// until the returned listener is dropped. Both or neither.
    fn listen(
        &self,
        control: uuid::Uuid,
        link: uuid::Uuid,
        accepted: mpsc::Sender<Incoming>,
    ) -> std::io::Result<Box<dyn Listener>>;
}

/// A running listener on both services; dropping it stops both (and takes
/// their SDP records down).
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
        _control: uuid::Uuid,
        _link: uuid::Uuid,
        _accepted: mpsc::Sender<Incoming>,
    ) -> std::io::Result<Box<dyn Listener>> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

/// Windows' radio, by Winsock.
#[cfg(windows)]
struct Winsock;

/// The two services' sockets: one ending ends both.
#[cfg(windows)]
struct WinsockListener([Arc<windows::Listener>; 2]);

#[cfg(windows)]
impl Listener for WinsockListener {
    fn ended(&self) -> bool {
        self.0.iter().any(|listener| listener.is_closed())
    }
}

#[cfg(windows)]
impl Drop for WinsockListener {
    fn drop(&mut self) {
        for listener in &self.0 {
            listener.close();
        }
    }
}

#[cfg(windows)]
impl Radio for Winsock {
    fn address(&self) -> Option<[u8; 6]> {
        windows::radio_address()
    }

    fn listen(
        &self,
        control: uuid::Uuid,
        link: uuid::Uuid,
        accepted: mpsc::Sender<Incoming>,
    ) -> std::io::Result<Box<dyn Listener>> {
        use pocketdesk_core::protocol::bluetooth::{LINK_SERVICE_NAME, SERVICE_NAME};
        let runtime = tokio::runtime::Handle::try_current().map_err(std::io::Error::other)?;
        // Made before either accepts, so a failure leaves neither (each
        // closes as it drops).
        let both = WinsockListener([
            Arc::new(windows::listen(control, SERVICE_NAME)?),
            Arc::new(windows::listen(link, LINK_SERVICE_NAME)?),
        ]);
        let open = windows::Open::default();
        for (listener, kind) in both.0.iter().zip([Kind::Control, Kind::Link]) {
            windows::spawn_accepting(
                listener.clone(),
                kind,
                accepted.clone(),
                open.clone(),
                runtime.clone(),
            )?;
        }
        Ok(Box::new(both))
    }
}

fn radio() -> Arc<dyn Radio> {
    #[cfg(windows)]
    let radio = Arc::new(Winsock);
    #[cfg(not(windows))]
    let radio = Arc::new(NoRadio);
    radio
}

/// How often the listener asks the radio (whether there is one, and its
/// address), and starts again a listener that would not start.
const CHECK_EVERY: Duration = Duration::from_secs(10);

/// How often the listener reads the switch: a change is acted on at once,
/// so turning it off ends the road within about a second. One look at the
/// config file's time each, the radio left alone.
const SWITCH_EVERY: Duration = Duration::from_secs(1);

/// Whether this system has a listener (Windows only for now).
const LISTENS_HERE: bool = cfg!(windows);

/// Listen while `wanted()` (the `bluetooth_fallback` switch) is on and
/// `radio` has an address: start when both hold, stop (the SDP record taken
/// down) when either goes, or the radio's address changes, and start again
/// after a listener that ended by itself. The switch is read every
/// `switch_every`; the radio is asked when the switch changes, when a
/// listener ended, and otherwise every `radio_every`. `state` says where it
/// listens meanwhile, and how the road stands ([`RoadState`]) after each
/// check.
async fn keep_listening(
    radio: Arc<dyn Radio>,
    state: &'static Listening,
    wanted: impl Fn() -> bool,
    accepted: mpsc::Sender<Incoming>,
    switch_every: Duration,
    radio_every: Duration,
) {
    let mut running: Option<(Box<dyn Listener>, [u8; 6])> = None;
    let mut failure_said = false;
    let mut last: Option<(bool, tokio::time::Instant)> = None;
    loop {
        let on = wanted();
        let now = tokio::time::Instant::now();
        let due = match last {
            None => true,
            Some((was, at)) => {
                was != on
                    || now.duration_since(at) >= radio_every
                    || running.as_ref().is_some_and(|(l, _)| l.ended())
            }
        };
        if !due {
            tokio::time::sleep(switch_every).await;
            continue;
        }
        last = Some((on, now));
        let address = if on {
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
            state.stop();
            tracing::info!("Stopped listening over Bluetooth");
        }
        if running.is_none()
            && let Some(at) = address
        {
            let service = pocketdesk_core::protocol::bluetooth::service_uuid();
            state.begin();
            let started = match uuid::Uuid::parse_str(&service) {
                Ok(uuid) => {
                    let link = pocketdesk_core::protocol::bluetooth::link_uuid(uuid);
                    let radio = radio.clone();
                    let accepted = accepted.clone();
                    tokio::task::spawn_blocking(move || radio.listen(uuid, link, accepted))
                        .await
                        .unwrap_or_else(|e| Err(std::io::Error::other(e)))
                }
                Err(e) => Err(std::io::Error::other(e)),
            };
            match started {
                Ok(listener) => {
                    tracing::info!("Listening over Bluetooth for service {service}");
                    state.listening_at(BluetoothInfo {
                        address: format_address(at),
                        service,
                    });
                    running = Some((listener, at));
                    failure_said = false;
                }
                Err(e) => {
                    state.stop();
                    if !failure_said {
                        tracing::warn!("Could not listen over Bluetooth: {e}");
                        failure_said = true;
                    }
                }
            }
        }
        state.set_road(RoadState::of(on, LISTENS_HERE, running.is_some()));
        tokio::time::sleep(switch_every).await;
    }
}

/// The Bluetooth road's listener, for the server's life: listens while
/// `bluetooth_fallback` is on and a radio is present (Windows only for
/// now), and hands each control link to `control`.
pub(crate) async fn run(control: mpsc::Sender<Control>) {
    let (accepted_tx, accepted_rx) = mpsc::channel(8);
    tokio::spawn(serve(accepted_rx, Arc::default(), control, &LISTENING));
    keep_listening(
        radio(),
        &LISTENING,
        || crate::events::current_config().bluetooth_fallback,
        accepted_tx,
        SWITCH_EVERY,
        CHECK_EVERY,
    )
    .await;
}

/// A stream that ends with no road (the tests').
#[cfg(test)]
fn live(stream: DuplexStream) -> Stream {
    Endable::new(stream, CancellationToken::new())
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
        mpsc::Sender<Incoming>,
        Arc<LinkTable>,
        mpsc::Receiver<Control>,
    ) {
        let (accepted_tx, accepted_rx) = mpsc::channel(8);
        let (control_tx, control_rx) = mpsc::channel(8);
        let table = Arc::new(LinkTable::default());
        let state: &'static Listening = Box::leak(Box::new(Listening::new()));
        state.start(BluetoothInfo {
            address: format_address([0, 0, 0x5E, 0, 0x53, 1]),
            service: pocketdesk_core::protocol::bluetooth::SERVICE_UUID.to_string(),
        });
        tokio::spawn(serve(accepted_rx, table.clone(), control_tx, state));
        (accepted_tx, table, control_rx)
    }

    /// A connection from the phone to the `kind` service that sends
    /// `first`, and its phone end.
    async fn dial(accepted: &mpsc::Sender<Incoming>, kind: Kind, first: &[u8]) -> DuplexStream {
        let (server_end, mut phone_end) = tokio::io::duplex(64 * 1024);
        accepted
            .send(Incoming {
                kind,
                stream: server_end,
                closed: None,
            })
            .await
            .unwrap();
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
        let mut phone = dial(&accepted, Kind::Link, &hello).await;
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
        let _phone = dial(&accepted, Kind::Control, &record).await;
        let mut link = control.recv().await.expect("a control link");
        let mut got = [0u8; 3];
        link.stream.read_exact(&mut got).await.unwrap();
        assert_eq!(got, record);
    }

    #[tokio::test]
    async fn anything_else_is_dropped() {
        let (accepted, table, mut control) = serving();
        let (waiting, _arrived) = table.wait();
        let tls = vec![pocketdesk_core::protocol::TLS_HANDSHAKE_RECORD, 3, 1];
        for (kind, first) in [
            (Kind::Link, b"GET / HTTP/1.1\r\n".to_vec()),
            (Kind::Control, b"GET / HTTP/1.1\r\n".to_vec()),
            (Kind::Link, b"{\"id\":1}\n".to_vec()),
            (
                Kind::Link,
                format!("POCKETDESK-BT-LINK {}x\n", waiting.id).into_bytes(),
            ),
            (
                Kind::Link,
                format!("POCKETDESK-BT-LINK {}{}\n", waiting.id, "0".repeat(40)).into_bytes(),
            ),
            // Each service takes only its own: TLS on the transfer
            // service, a hello on the control service.
            (Kind::Link, tls.clone()),
            (Kind::Control, format_link_hello(waiting.id).into_bytes()),
        ] {
            let mut phone = dial(&accepted, kind, &first).await;
            let mut rest = Vec::new();
            tokio::time::timeout(Duration::from_secs(5), phone.read_to_end(&mut rest))
                .await
                .expect("closed in time")
                .unwrap();
            assert!(rest.is_empty(), "{kind:?} {first:?}");
        }
        assert!(control.try_recv().is_err(), "no control link");
        let (late, _) = tokio::io::duplex(1024);
        assert!(
            table.offer(waiting.id, live(late)),
            "the id still waits: no wrong-service hello took it"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_hello_that_never_ends_is_dropped_in_time() {
        let (server_end, mut phone_end) = tokio::io::duplex(1024);
        phone_end.write_all(b"POCKETDESK-BT").await.unwrap();
        let at = tokio::time::Instant::now();
        assert!(sort(server_end, Kind::Link).await.is_none());
        assert_eq!(at.elapsed(), HELLO_WAIT);
        let (server_end, _silent) = tokio::io::duplex(1024);
        let at = tokio::time::Instant::now();
        assert!(sort(server_end, Kind::Control).await.is_none(), "silence");
        assert_eq!(at.elapsed(), CONTROL_HELLO_WAIT);
    }

    /// A control connection whose TLS starts late (the phone finishing
    /// another dial first) is still taken, within its longer wait.
    #[tokio::test(start_paused = true)]
    async fn a_control_link_may_start_its_tls_late() {
        let (server_end, mut phone_end) = tokio::io::duplex(1024);
        let late = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(12)).await;
            phone_end
                .write_all(&[pocketdesk_core::protocol::TLS_HANDSHAKE_RECORD])
                .await
                .unwrap();
            phone_end
        });
        assert!(matches!(
            sort(server_end, Kind::Control).await,
            Some(Arrival::Control(_))
        ));
        drop(late.await.unwrap());
    }

    /// A radio in memory: its address, and whether its listener runs.
    #[derive(Default)]
    struct FakeRadio {
        address: std::sync::Mutex<Option<[u8; 6]>>,
        running: Arc<AtomicBool>,
        ended: Arc<AtomicBool>,
        accepted: std::sync::Mutex<Option<mpsc::Sender<Incoming>>>,
        asked: std::sync::atomic::AtomicUsize,
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
            self.asked.fetch_add(1, Ordering::SeqCst);
            *self.address.lock().unwrap()
        }

        fn listen(
            &self,
            control: uuid::Uuid,
            link: uuid::Uuid,
            accepted: mpsc::Sender<Incoming>,
        ) -> std::io::Result<Box<dyn Listener>> {
            assert_eq!(
                link,
                pocketdesk_core::protocol::bluetooth::link_uuid(control)
            );
            *self.accepted.lock().unwrap() = Some(accepted);
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
                Duration::from_millis(5),
            )
        });
        let running = || radio.running.load(Ordering::SeqCst);

        // No radio: nothing listens, nothing is told, and the road says
        // so (off Windows, that it is Windows only).
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!running());
        assert_eq!(state.get(), None);
        let no_radio = if LISTENS_HERE {
            RoadState::NoRadio
        } else {
            RoadState::Unsupported
        };
        assert_eq!(state.road(), Some(no_radio));

        // A radio: it listens, and says where.
        *radio.address.lock().unwrap() = Some([0x00, 0x00, 0x5E, 0x00, 0x53, 0x01]);
        until("listening", || running() && state.get().is_some()).await;
        until("said listening", || {
            state.road() == Some(RoadState::Listening)
        })
        .await;
        let told = state.get().unwrap();
        assert_eq!(told.address, "00:00:5E:00:53:01");
        assert_eq!(
            told.service,
            pocketdesk_core::protocol::bluetooth::service_uuid()
        );

        // The switch off: it stops, and tells nothing.
        on.store(false, Ordering::SeqCst);
        until("stopped", || !running() && state.get().is_none()).await;
        until("said off", || state.road() == Some(RoadState::Off)).await;

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
        until("said no radio", || state.road() == Some(no_radio)).await;
        task.abort();
    }

    /// The switch is followed within about a second, on or off, while
    /// the radio is asked at most every ten seconds otherwise.
    #[tokio::test(start_paused = true)]
    async fn the_switch_is_read_each_second_and_the_radio_each_ten() {
        let radio = Arc::new(FakeRadio::default());
        *radio.address.lock().unwrap() = Some([0x00, 0x00, 0x5E, 0x00, 0x53, 0x01]);
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
                Duration::from_secs(1),
                Duration::from_secs(10),
            )
        });
        let running = || radio.running.load(Ordering::SeqCst);
        let asked = || radio.asked.load(Ordering::SeqCst);
        until("listening", || {
            running() && state.road() == Some(RoadState::Listening)
        })
        .await;

        // A steady switch: the radio is asked once in ten seconds, not
        // each second.
        let before = asked();
        tokio::time::sleep(Duration::from_millis(9_500)).await;
        assert!(asked() - before <= 1, "asked {} times", asked() - before);

        // Off: stopped, and said, within about a second.
        let at = tokio::time::Instant::now();
        on.store(false, Ordering::SeqCst);
        until("stopped", || {
            !running() && state.road() == Some(RoadState::Off)
        })
        .await;
        assert!(
            at.elapsed() <= Duration::from_millis(1_100),
            "{:?}",
            at.elapsed()
        );
        assert_eq!(state.get(), None);

        // On again: listening within about a second.
        let at = tokio::time::Instant::now();
        on.store(true, Ordering::SeqCst);
        until("listening again", || {
            running() && state.road() == Some(RoadState::Listening)
        })
        .await;
        assert!(
            at.elapsed() <= Duration::from_millis(1_100),
            "{:?}",
            at.elapsed()
        );
        task.abort();
    }

    /// Turning the road off ends the control links it took (and with them
    /// the away lock's presence), not only the listening.
    #[tokio::test]
    async fn turning_the_road_off_ends_its_live_links() {
        let radio = Arc::new(FakeRadio::default());
        *radio.address.lock().unwrap() = Some([0x00, 0x00, 0x5E, 0x00, 0x53, 0x01]);
        let state: &'static Listening = Box::leak(Box::new(Listening::new()));
        let on = Arc::new(AtomicBool::new(true));
        let (accepted_tx, accepted_rx) = mpsc::channel(8);
        let (control_tx, mut control_rx) = mpsc::channel(8);
        let table = Arc::new(LinkTable::default());
        let serving = tokio::spawn(serve(accepted_rx, table.clone(), control_tx, state));
        let listening = tokio::spawn({
            let radio: Arc<dyn Radio> = radio.clone();
            let on = on.clone();
            keep_listening(
                radio,
                state,
                move || on.load(Ordering::SeqCst),
                accepted_tx,
                Duration::from_millis(5),
                Duration::from_millis(5),
            )
        });
        until("listening", || state.get().is_some()).await;
        let accepted = radio.accepted.lock().unwrap().clone().unwrap();

        let record = [pocketdesk_core::protocol::TLS_HANDSHAKE_RECORD, 3, 1];
        let _phone = dial(&accepted, Kind::Control, &record).await;
        let mut link = control_rx.recv().await.expect("a control link");
        let mut got = [0u8; 3];
        link.stream.read_exact(&mut got).await.unwrap();
        // A transfer link that arrived meanwhile ends too.
        let (waiting, arrived) = table.wait();
        let _transfer_phone = dial(
            &accepted,
            Kind::Link,
            format_link_hello(waiting.id).as_bytes(),
        )
        .await;
        let mut transfer = arrived.await.unwrap();

        // Split as the server splits it: a reader waiting on the phone
        // while another task writes. The road off: the read fails at once.
        let (mut reader, mut writer) = tokio::io::split(link.stream);
        let reading = tokio::spawn(async move {
            let mut buf = [0u8; 8];
            reader.read(&mut buf).await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        writer.write_all(b"to the phone").await.unwrap();
        writer.flush().await.unwrap();
        on.store(false, Ordering::SeqCst);
        let read = tokio::time::timeout(Duration::from_secs(5), reading)
            .await
            .expect("ended in time")
            .unwrap();
        assert_eq!(
            read.err().map(|e| e.kind()),
            Some(std::io::ErrorKind::ConnectionAborted)
        );
        assert!(
            transfer.write_all(b"x").await.is_err(),
            "the transfer link ended"
        );
        assert!(writer.write_all(b"x").await.is_err(), "and its writes");

        // On again: a new link lives.
        on.store(true, Ordering::SeqCst);
        until("listening again", || state.get().is_some()).await;
        let accepted = radio.accepted.lock().unwrap().clone().unwrap();
        let _phone = dial(&accepted, Kind::Control, &record).await;
        let mut link = control_rx.recv().await.expect("a new control link");
        link.stream.read_exact(&mut got).await.unwrap();
        assert_eq!(got, record);
        listening.abort();
        serving.abort();
    }
}
