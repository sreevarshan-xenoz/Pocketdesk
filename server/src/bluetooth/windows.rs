//! The Bluetooth road on Windows: a secure RFCOMM listening socket (Winsock
//! `AF_BTH`), its SDP record so a bonded phone finds the channel by the
//! service UUID, and the radio's address. Bluetooth sockets get no IOCP, so
//! each accepted socket is pumped by two blocking threads into a
//! `tokio::io::duplex`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use windows::Win32::Devices::Bluetooth::{
    AF_BTH, BLUETOOTH_FIND_RADIO_PARAMS, BLUETOOTH_RADIO_INFO, BTHPROTO_RFCOMM,
    BluetoothFindFirstRadio, BluetoothFindRadioClose, BluetoothGetRadioInfo, NS_BTH,
    SO_BTH_AUTHENTICATE, SO_BTH_ENCRYPT, SOCKADDR_BTH, SOL_RFCOMM,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Networking::WinSock::{
    CSADDR_INFO, POLLRDNORM, RNRSERVICE_DELETE, RNRSERVICE_REGISTER, SD_SEND, SEND_RECV_FLAGS,
    SO_SNDTIMEO, SOCK_STREAM, SOCKADDR, SOCKET, SOCKET_ADDRESS, SOCKET_ERROR, SOL_SOCKET, WSADATA,
    WSAGetLastError, WSAPOLLFD, WSAPoll, WSAQUERYSETW, WSASetServiceW, WSAStartup, accept, bind,
    closesocket, getsockname, recv, send, setsockopt, shutdown, socket,
};
use windows::core::{GUID, PWSTR};

/// Any free RFCOMM channel (`BT_PORT_ANY`).
const BT_PORT_ANY: u32 = u32::MAX;

/// Each accepted connection's buffer between its socket and the server.
const PUMP_BUFFER: usize = 64 * 1024;

/// The radio's address, when this computer has a Bluetooth radio (the
/// first one); `None` without one.
pub(crate) fn radio_address() -> Option<[u8; 6]> {
    let params = BLUETOOTH_FIND_RADIO_PARAMS {
        dwSize: std::mem::size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32,
    };
    let mut radio = HANDLE::default();
    // SAFETY: `params` and `radio` outlive the call; the find handle is
    // closed at once, the radio's handle after the read.
    let find = unsafe { BluetoothFindFirstRadio(&params, &mut radio) }.ok()?;
    let _ = unsafe { BluetoothFindRadioClose(find) };
    let mut info = BLUETOOTH_RADIO_INFO {
        dwSize: std::mem::size_of::<BLUETOOTH_RADIO_INFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `radio` is the handle the find gave; `info` is sized.
    let status = unsafe { BluetoothGetRadioInfo(radio, &mut info) };
    let _ = unsafe { CloseHandle(radio) };
    if status != 0 {
        return None;
    }
    // SAFETY: both views of the union are plain bytes.
    Some(address_bytes(unsafe { info.address.Anonymous.ullLong }))
}

/// The six bytes of an address Windows keeps in a `u64`'s low 48 bits,
/// most significant first (as `AA:BB:CC:DD:EE:FF` writes them).
fn address_bytes(raw: u64) -> [u8; 6] {
    let b = raw.to_be_bytes();
    [b[2], b[3], b[4], b[5], b[6], b[7]]
}

fn last_error() -> std::io::Error {
    // SAFETY: reads this thread's last Winsock error.
    std::io::Error::from_raw_os_error(unsafe { WSAGetLastError() }.0)
}

/// Winsock started for this process (std starts it too, on first use).
fn started() -> std::io::Result<()> {
    static STARTED: std::sync::OnceLock<i32> = std::sync::OnceLock::new();
    let code = *STARTED.get_or_init(|| {
        let mut data = WSADATA::default();
        // SAFETY: `data` outlives the call.
        unsafe { WSAStartup(0x0202, &mut data) }
    });
    match code {
        0 => Ok(()),
        code => Err(std::io::Error::from_raw_os_error(code)),
    }
}

/// The SDP record that names a service: its UUID, its name and the RFCOMM
/// channel the socket is bound to. The query points into the record's own
/// buffers, which do not move while it lives.
pub(crate) struct ServiceRecord {
    _name: Vec<u16>,
    _class: Box<GUID>,
    _address: Box<SOCKADDR_BTH>,
    _csaddr: Box<CSADDR_INFO>,
    query: Box<WSAQUERYSETW>,
}

// SAFETY: the raw pointers in `query` point only into the record's own
// heap buffers, which are never written after it is built.
unsafe impl Send for ServiceRecord {}
unsafe impl Sync for ServiceRecord {}

impl ServiceRecord {
    pub(crate) fn query(&self) -> &WSAQUERYSETW {
        &self.query
    }
}

/// The SDP record for `service`, called `name`, on the socket bound at
/// `bound`.
pub(crate) fn service_record(
    service: uuid::Uuid,
    name: &str,
    bound: SOCKADDR_BTH,
) -> ServiceRecord {
    let mut name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let mut class = Box::new(GUID::from_u128(service.as_u128()));
    let mut address = Box::new(bound);
    let mut csaddr = Box::new(CSADDR_INFO {
        LocalAddr: SOCKET_ADDRESS {
            lpSockaddr: (&mut *address as *mut SOCKADDR_BTH).cast::<SOCKADDR>(),
            iSockaddrLength: std::mem::size_of::<SOCKADDR_BTH>() as i32,
        },
        RemoteAddr: SOCKET_ADDRESS::default(),
        iSocketType: SOCK_STREAM.0,
        iProtocol: BTHPROTO_RFCOMM as i32,
    });
    let query = Box::new(WSAQUERYSETW {
        dwSize: std::mem::size_of::<WSAQUERYSETW>() as u32,
        lpszServiceInstanceName: PWSTR(name.as_mut_ptr()),
        lpServiceClassId: &mut *class,
        dwNameSpace: NS_BTH,
        dwNumberOfCsAddrs: 1,
        lpcsaBuffer: &mut *csaddr,
        ..Default::default()
    });
    ServiceRecord {
        _name: name,
        _class: class,
        _address: address,
        _csaddr: csaddr,
        query,
    }
}

/// A secure RFCOMM listening socket with its SDP record. Closing it (or
/// dropping it) removes the record and ends a blocking accept.
pub(crate) struct Listener {
    socket: SOCKET,
    record: ServiceRecord,
    closed: AtomicBool,
}

/// Listen on `service`: a secure (authenticated, encrypted) RFCOMM socket on
/// any free channel, and an SDP record naming it `name`.
pub(crate) fn listen(service: uuid::Uuid, name: &str) -> std::io::Result<Listener> {
    started()?;
    // SAFETY: plain Winsock calls on a socket this function owns; each
    // pointer passed outlives its call, and the socket is closed on every
    // error below.
    let socket = unsafe { socket(AF_BTH as i32, SOCK_STREAM, BTHPROTO_RFCOMM as i32) }
        .map_err(|_| last_error())?;
    let close_on_error = |e: std::io::Error| {
        unsafe { closesocket(socket) };
        e
    };
    let on = 1u32.to_ne_bytes();
    for option in [SO_BTH_AUTHENTICATE, SO_BTH_ENCRYPT] {
        if unsafe { setsockopt(socket, SOL_RFCOMM as i32, option as i32, Some(&on)) }
            == SOCKET_ERROR
        {
            return Err(close_on_error(last_error()));
        }
    }
    let any = SOCKADDR_BTH {
        addressFamily: AF_BTH,
        port: BT_PORT_ANY,
        ..Default::default()
    };
    let size = std::mem::size_of::<SOCKADDR_BTH>() as i32;
    if unsafe { bind(socket, (&any as *const SOCKADDR_BTH).cast(), size) } == SOCKET_ERROR {
        return Err(close_on_error(last_error()));
    }
    if unsafe { windows::Win32::Networking::WinSock::listen(socket, 4) } == SOCKET_ERROR {
        return Err(close_on_error(last_error()));
    }
    let mut bound = SOCKADDR_BTH::default();
    let mut length = size;
    if unsafe {
        getsockname(
            socket,
            (&mut bound as *mut SOCKADDR_BTH).cast(),
            &mut length,
        )
    } == SOCKET_ERROR
    {
        return Err(close_on_error(last_error()));
    }
    let record = service_record(service, name, bound);
    if unsafe { WSASetServiceW(record.query(), RNRSERVICE_REGISTER, 0) } == SOCKET_ERROR {
        return Err(close_on_error(last_error()));
    }
    Ok(Listener {
        socket,
        record,
        closed: AtomicBool::new(false),
    })
}

impl Listener {
    /// Wait for the next connection (blocking).
    fn accept(&self) -> std::io::Result<SOCKET> {
        // SAFETY: the listening socket stays open until `close`, which also
        // ends a wait here.
        unsafe { accept(self.socket, None, None) }.map_err(|_| last_error())
    }

    /// Remove the SDP record and close the socket, once.
    pub(crate) fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        // SAFETY: the record and the socket are this listener's own.
        unsafe {
            WSASetServiceW(self.record.query(), RNRSERVICE_DELETE, 0);
            closesocket(self.socket);
        }
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.close();
    }
}

/// The most Bluetooth connections open at once, all three services together:
/// this road has no address for the server's per-address budget, so it
/// keeps its own.
const MAX_OPEN: usize = 8;

/// How many accepted Bluetooth connections are open, shared by the three
/// services' accept threads.
#[derive(Clone, Default)]
pub(crate) struct Open(Arc<AtomicUsize>);

/// Accept connections on `listener` on a thread of its own, each pumped into
/// a stream sent to `accepted` as `kind`, until the listener is closed (or
/// fails, which closes it) or `accepted` is gone. Past [`MAX_OPEN`] open
/// connections, a new one is closed at once.
pub(crate) fn spawn_accepting(
    listener: Arc<Listener>,
    kind: super::Kind,
    accepted: tokio::sync::mpsc::Sender<super::Incoming>,
    open: Open,
    runtime: tokio::runtime::Handle,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("bluetooth-accept".into())
        .spawn(move || {
            loop {
                match listener.accept() {
                    Ok(socket) => {
                        let (closed_tx, closed) = tokio::sync::oneshot::channel();
                        let socket = match Accepted::new(socket, &open, closed_tx) {
                            Ok(socket) => Arc::new(socket),
                            Err(e) => {
                                tracing::warn!("Dropped a Bluetooth connection: {e}");
                                continue;
                            }
                        };
                        if open.0.load(Ordering::SeqCst) > MAX_OPEN {
                            tracing::warn!(
                                "Refused a Bluetooth connection: {MAX_OPEN} are open already"
                            );
                            continue;
                        }
                        let stream = pump(socket, runtime.clone());
                        let incoming = super::Incoming {
                            kind,
                            stream,
                            closed: Some(closed),
                        };
                        if accepted.blocking_send(incoming).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        if !listener.is_closed() {
                            tracing::warn!("Stopped listening over Bluetooth: {e}");
                            listener.close();
                        }
                        break;
                    }
                }
            }
        })
        .map(|_| ())
}

/// How long the in-pump waits for the phone's bytes before it looks
/// whether the server is done with the connection.
const POLL_MS: i32 = 1000;

/// How long a send to the phone may wait before the connection is given
/// up: a phone that stopped reading does not hold a pump forever.
const SEND_TIMEOUT_MS: u32 = 30_000;

/// An accepted socket, counted in [`Open`] and closed once, when both of
/// its pumps are done (so neither can ever use its handle after it is
/// closed and given to another socket).
struct Accepted {
    socket: SOCKET,
    open: Open,
    /// The server has ended its side (or the out-pump gave up): the
    /// in-pump stops at its next wait.
    server_done: AtomicBool,
    /// Dropped with the socket, as it closes: the transfer that held it
    /// gives its phone's turn back then.
    _closed: tokio::sync::oneshot::Sender<()>,
}

impl Accepted {
    /// Take `socket`, counted in `open`; an error (and the socket closed)
    /// when its send timeout cannot be set.
    fn new(
        socket: SOCKET,
        open: &Open,
        closed: tokio::sync::oneshot::Sender<()>,
    ) -> std::io::Result<Self> {
        open.0.fetch_add(1, Ordering::SeqCst);
        let accepted = Self {
            socket,
            open: open.clone(),
            server_done: AtomicBool::new(false),
            _closed: closed,
        };
        let timeout = SEND_TIMEOUT_MS.to_ne_bytes();
        // SAFETY: the socket is this connection's own; `timeout` outlives
        // the call.
        if unsafe { setsockopt(socket, SOL_SOCKET, SO_SNDTIMEO, Some(&timeout)) } == SOCKET_ERROR {
            // Dropped here, which closes it.
            return Err(last_error());
        }
        Ok(accepted)
    }

    /// Wait up to [`POLL_MS`] for bytes (or the end) from the phone:
    /// `Some(true)` when a read will not block, `Some(false)` on the
    /// timeout, `None` on an error.
    fn readable(&self) -> Option<bool> {
        let mut fd = WSAPOLLFD {
            fd: self.socket,
            events: POLLRDNORM,
            revents: Default::default(),
        };
        // SAFETY: `fd` outlives the call; the socket stays open while a
        // pump holds it.
        match unsafe { WSAPoll(&mut fd, 1, POLL_MS) } {
            0 => Some(false),
            ready if ready > 0 => Some(true),
            _ => None,
        }
    }
}

impl Drop for Accepted {
    fn drop(&mut self) {
        // SAFETY: the socket is this connection's own, and no pump holds it
        // any more.
        unsafe { closesocket(self.socket) };
        self.open.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The server's end of a stream joined to `socket` by two blocking threads:
/// one copies what the phone sends in, the other what the server writes out.
/// The in-pump reads only once a poll says a read will not block, and
/// between polls looks whether the server is done: once the server ends
/// its side (or a send times out), the phone is sent the end of the
/// stream and the in-pump stops within a second, after what was still in
/// flight. The socket closes when both have let go.
fn pump(socket: Arc<Accepted>, runtime: tokio::runtime::Handle) -> DuplexStream {
    let (ours, theirs) = tokio::io::duplex(PUMP_BUFFER);
    let (mut from_server, mut to_server) = tokio::io::split(theirs);
    let inbound = socket.clone();
    let inbound_runtime = runtime.clone();
    let _ = std::thread::Builder::new()
        .name("bluetooth-in".into())
        .spawn(move || {
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                match inbound.readable() {
                    Some(true) => {}
                    Some(false) if !inbound.server_done.load(Ordering::SeqCst) => continue,
                    _ => break,
                }
                // SAFETY: the socket stays open while this thread holds it.
                let read = unsafe { recv(inbound.socket, &mut buf, SEND_RECV_FLAGS(0)) };
                if read <= 0 {
                    break;
                }
                let chunk = &buf[..read as usize];
                if inbound_runtime
                    .block_on(to_server.write_all(chunk))
                    .is_err()
                {
                    break;
                }
            }
            let _ = inbound_runtime.block_on(to_server.shutdown());
        });
    let outbound = socket.clone();
    let spawned = std::thread::Builder::new()
        .name("bluetooth-out".into())
        .spawn(move || {
            let mut buf = vec![0u8; 16 * 1024];
            'out: loop {
                let read = match runtime.block_on(from_server.read(&mut buf)) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                let mut chunk = &buf[..read];
                while !chunk.is_empty() {
                    // SAFETY: as above. A send that times out gives up.
                    let sent = unsafe { send(outbound.socket, chunk, SEND_RECV_FLAGS(0)) };
                    if sent <= 0 {
                        break 'out;
                    }
                    chunk = &chunk[sent as usize..];
                }
            }
            // SAFETY: as above; the phone reads the end of the stream.
            unsafe { shutdown(outbound.socket, SD_SEND) };
            outbound.server_done.store(true, Ordering::SeqCst);
        });
    if spawned.is_err() {
        // No out-pump: the in-pump must not wait on it.
        socket.server_done.store(true, Ordering::SeqCst);
    }
    ours
}

#[cfg(test)]
mod tests {
    use super::*;

    use pocketdesk_core::protocol::bluetooth::SERVICE_NAME;

    #[test]
    fn service_record_names_the_uuid() {
        let service =
            uuid::Uuid::parse_str(pocketdesk_core::protocol::bluetooth::SERVICE_UUID).unwrap();
        let bound = SOCKADDR_BTH {
            addressFamily: AF_BTH,
            port: 5,
            ..Default::default()
        };
        let record = service_record(service, SERVICE_NAME, bound);
        let query = record.query();
        // SAFETY: the pointers are into `record`, alive here.
        unsafe {
            assert_eq!(*query.lpServiceClassId, GUID::from_u128(service.as_u128()));
            assert_eq!(
                query.lpszServiceInstanceName.to_string().unwrap(),
                SERVICE_NAME
            );
            assert_eq!(query.dwNameSpace, NS_BTH);
            assert_eq!(query.dwNumberOfCsAddrs, 1);
            let csaddr = &*query.lpcsaBuffer;
            assert_eq!(csaddr.iProtocol, BTHPROTO_RFCOMM as i32);
            assert_eq!(csaddr.iSocketType, SOCK_STREAM.0);
            let local = &*csaddr.LocalAddr.lpSockaddr.cast::<SOCKADDR_BTH>();
            let port = local.port;
            assert_eq!(port, 5, "the channel bound");
        }
        assert_eq!(query.dwSize as usize, std::mem::size_of::<WSAQUERYSETW>());
    }

    #[test]
    fn the_transfer_record_names_its_own_service() {
        use pocketdesk_core::protocol::bluetooth::{LINK_SERVICE_NAME, SERVICE_UUID, link_uuid};
        let control = uuid::Uuid::parse_str(SERVICE_UUID).unwrap();
        let record = service_record(
            link_uuid(control),
            LINK_SERVICE_NAME,
            SOCKADDR_BTH::default(),
        );
        let query = record.query();
        // SAFETY: the pointers are into `record`, alive here.
        unsafe {
            assert_eq!(
                *query.lpServiceClassId,
                GUID::from_u128(link_uuid(control).as_u128())
            );
            assert_ne!(*query.lpServiceClassId, GUID::from_u128(control.as_u128()));
            assert_eq!(
                query.lpszServiceInstanceName.to_string().unwrap(),
                LINK_SERVICE_NAME
            );
        }
    }

    #[test]
    fn the_screen_record_names_its_own_service() {
        use pocketdesk_core::protocol::bluetooth::{
            SERVICE_UUID, STREAM_SERVICE_NAME, link_uuid, stream_uuid,
        };
        let control = uuid::Uuid::parse_str(SERVICE_UUID).unwrap();
        let record = service_record(
            stream_uuid(control),
            STREAM_SERVICE_NAME,
            SOCKADDR_BTH::default(),
        );
        let query = record.query();
        // SAFETY: the pointers are into `record`, alive here.
        unsafe {
            assert_eq!(
                *query.lpServiceClassId,
                GUID::from_u128(stream_uuid(control).as_u128())
            );
            assert_ne!(*query.lpServiceClassId, GUID::from_u128(control.as_u128()));
            assert_ne!(
                *query.lpServiceClassId,
                GUID::from_u128(link_uuid(control).as_u128())
            );
            assert_eq!(
                query.lpszServiceInstanceName.to_string().unwrap(),
                STREAM_SERVICE_NAME
            );
        }
    }

    #[test]
    fn an_address_reads_most_significant_first() {
        assert_eq!(
            address_bytes(0x0000_5E00_5301),
            [0x00, 0x00, 0x5E, 0x00, 0x53, 0x01]
        );
        assert_eq!(
            address_bytes(0xFFFF_0000_5E00_5301),
            [0x00, 0x00, 0x5E, 0x00, 0x53, 0x01],
            "the top two bytes are not the address"
        );
    }
}
