//! The Bluetooth road on Windows: a secure RFCOMM listening socket (Winsock
//! `AF_BTH`), its SDP record so a bonded phone finds the channel by the
//! service UUID, and the radio's address. Bluetooth sockets get no IOCP, so
//! each accepted socket is pumped by two blocking threads into a
//! `tokio::io::duplex`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use windows::Win32::Devices::Bluetooth::{
    AF_BTH, BLUETOOTH_FIND_RADIO_PARAMS, BLUETOOTH_RADIO_INFO, BTHPROTO_RFCOMM,
    BluetoothFindFirstRadio, BluetoothFindRadioClose, BluetoothGetRadioInfo, NS_BTH,
    SO_BTH_AUTHENTICATE, SO_BTH_ENCRYPT, SOCKADDR_BTH, SOL_RFCOMM,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Networking::WinSock::{
    CSADDR_INFO, RNRSERVICE_DELETE, RNRSERVICE_REGISTER, SD_SEND, SEND_RECV_FLAGS, SOCK_STREAM,
    SOCKADDR, SOCKET, SOCKET_ADDRESS, SOCKET_ERROR, WSADATA, WSAGetLastError, WSAQUERYSETW,
    WSASetServiceW, WSAStartup, accept, bind, closesocket, getsockname, recv, send, setsockopt,
    shutdown, socket,
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

/// The SDP record that names the service: its UUID, [`SERVICE_NAME`] and
/// the RFCOMM channel the socket is bound to. The query points into the
/// record's own buffers, which do not move while it lives.
///
/// [`SERVICE_NAME`]: pocketdesk_core::protocol::bluetooth::SERVICE_NAME
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

/// The SDP record for `service` on the socket bound at `bound`.
pub(crate) fn service_record(service: uuid::Uuid, bound: SOCKADDR_BTH) -> ServiceRecord {
    let mut name: Vec<u16> = pocketdesk_core::protocol::bluetooth::SERVICE_NAME
        .encode_utf16()
        .chain(Some(0))
        .collect();
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

/// Listen on `service`: a secure (authenticated, encrypted, so bonded)
/// RFCOMM socket on any free channel, and an SDP record naming it.
pub(crate) fn listen(service: uuid::Uuid) -> std::io::Result<Listener> {
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
    let record = service_record(service, bound);
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

/// Accept connections on `listener` on a thread of its own, each pumped into
/// a stream sent to `accepted`, until the listener is closed (or fails, which
/// closes it) or `accepted` is gone.
pub(crate) fn spawn_accepting(
    listener: Arc<Listener>,
    accepted: tokio::sync::mpsc::Sender<DuplexStream>,
    runtime: tokio::runtime::Handle,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("bluetooth-accept".into())
        .spawn(move || {
            loop {
                match listener.accept() {
                    Ok(socket) => {
                        let stream = pump(Arc::new(Accepted(socket)), runtime.clone());
                        if accepted.blocking_send(stream).is_err() {
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

/// An accepted socket, closed when both of its pumps are done.
struct Accepted(SOCKET);

impl Drop for Accepted {
    fn drop(&mut self) {
        // SAFETY: the socket is this connection's own.
        unsafe { closesocket(self.0) };
    }
}

/// The server's end of a stream joined to `socket` by two blocking threads:
/// one copies what the phone sends in, the other what the server writes out.
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
                // SAFETY: the socket stays open while this thread holds it.
                let read = unsafe { recv(inbound.0, &mut buf, SEND_RECV_FLAGS(0)) };
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
    let _ = std::thread::Builder::new()
        .name("bluetooth-out".into())
        .spawn(move || {
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                let read = match runtime.block_on(from_server.read(&mut buf)) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                let mut chunk = &buf[..read];
                while !chunk.is_empty() {
                    // SAFETY: as above.
                    let sent = unsafe { send(socket.0, chunk, SEND_RECV_FLAGS(0)) };
                    if sent <= 0 {
                        return;
                    }
                    chunk = &chunk[sent as usize..];
                }
            }
            // SAFETY: as above; the phone reads the end of the stream.
            unsafe { shutdown(socket.0, SD_SEND) };
        });
    ours
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_record_names_the_uuid() {
        let service =
            uuid::Uuid::parse_str(pocketdesk_core::protocol::bluetooth::SERVICE_UUID).unwrap();
        let bound = SOCKADDR_BTH {
            addressFamily: AF_BTH,
            port: 5,
            ..Default::default()
        };
        let record = service_record(service, bound);
        let query = record.query();
        // SAFETY: the pointers are into `record`, alive here.
        unsafe {
            assert_eq!(*query.lpServiceClassId, GUID::from_u128(service.as_u128()));
            assert_eq!(
                query.lpszServiceInstanceName.to_string().unwrap(),
                pocketdesk_core::protocol::bluetooth::SERVICE_NAME
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
