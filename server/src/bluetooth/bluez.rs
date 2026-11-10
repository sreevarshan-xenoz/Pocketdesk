//! The Bluetooth road on Linux: BlueZ over D-Bus. The radio's address comes
//! from the default adapter (`org.bluez.Adapter1`), and each of the three
//! services is a profile registered with `org.bluez.ProfileManager1` on an
//! object this server exports (`org.bluez.Profile1`): bluetoothd listens on
//! an RFCOMM channel for it, writes its SDP record, and hands over a
//! connected socket (a file descriptor) per phone in `NewConnection`. Each
//! socket is made a tokio stream and pumped into a `tokio::io::duplex`, as
//! the Windows road does.
//!
//! The profiles live as long as the D-Bus connection the listener owns: a
//! dropped listener unregisters them and drops the connection (which would
//! unregister them anyway), and ends every connection it took.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use zbus::zvariant::{ObjectPath, OwnedFd, OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, fdo};

use super::{Incoming, Kind};

const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const PROFILE_MANAGER: &str = "org.bluez.ProfileManager1";
const PROFILE_MANAGER_PATH: &str = "/org/bluez";

/// How long one call to BlueZ may take.
const CALL_WAIT: Duration = Duration::from_secs(10);

/// The most Bluetooth connections open at once, all three services together
/// (as on Windows): this road has no address for the server's per-address
/// budget, so it keeps its own.
const MAX_OPEN: usize = 8;

/// Each accepted connection's buffer between its socket and the server.
const PUMP_BUFFER: usize = 64 * 1024;

/// How long a send to the phone may wait before the connection is given up:
/// a phone that stopped reading does not hold a pump forever.
const SEND_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the in-pump keeps reading after the server has ended its side,
/// for what the phone still had in flight.
const DRAIN: Duration = Duration::from_secs(1);

/// One of the three services: what BlueZ is told of it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProfileSpec {
    kind: Kind,
    uuid: uuid::Uuid,
    /// The service's name in its SDP record.
    name: &'static str,
    /// Where this server exports its `Profile1` object.
    path: &'static str,
}

/// The three services, in [`Kind`] order: control, transfers, screen.
fn specs(control: uuid::Uuid, link: uuid::Uuid, screen: uuid::Uuid) -> [ProfileSpec; 3] {
    use pocketdesk_core::protocol::bluetooth::{
        LINK_SERVICE_NAME, SERVICE_NAME, STREAM_SERVICE_NAME,
    };
    [
        ProfileSpec {
            kind: Kind::Control,
            uuid: control,
            name: SERVICE_NAME,
            path: "/dev/pocketdesk/bluetooth/control",
        },
        ProfileSpec {
            kind: Kind::Link,
            uuid: link,
            name: LINK_SERVICE_NAME,
            path: "/dev/pocketdesk/bluetooth/link",
        },
        ProfileSpec {
            kind: Kind::Screen,
            uuid: screen,
            name: STREAM_SERVICE_NAME,
            path: "/dev/pocketdesk/bluetooth/screen",
        },
    ]
}

/// The options of `RegisterProfile` for a service called `name`: a server
/// that asks BlueZ for an authenticated, encrypted link (a bonded phone;
/// TLS with the bound key is what decides after it), needs no per-connection
/// authorization (bonding is the gate, as on Windows), and is never dialled
/// out.
fn profile_options(name: &'static str) -> HashMap<&'static str, Value<'static>> {
    HashMap::from([
        ("Name", Value::from(name)),
        ("Role", Value::from("server")),
        ("RequireAuthentication", Value::from(true)),
        ("RequireAuthorization", Value::from(false)),
        ("AutoConnect", Value::from(false)),
    ])
}

/// BlueZ's `GetManagedObjects`: object, then interface, then property.
type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

/// The address of the default adapter: the powered adapter with the lowest
/// object path (`/org/bluez/hci0` before `hci1`); `None` with none powered.
fn default_adapter(objects: &Objects) -> Option<[u8; 6]> {
    let mut found: Vec<_> = objects
        .iter()
        .filter_map(|(path, interfaces)| {
            let adapter = interfaces.get(ADAPTER)?;
            let powered = bool::try_from(adapter.get("Powered")?).ok()?;
            let address = <&str>::try_from(adapter.get("Address")?).ok()?;
            let address = pocketdesk_core::protocol::bluetooth::parse_address(address)?;
            powered.then_some((path.as_str(), address))
        })
        .collect();
    found.sort();
    found.first().map(|(_, address)| *address)
}

/// A D-Bus error as a short phrase with no values in it.
fn word(e: &zbus::Error) -> io::Error {
    match e {
        zbus::Error::MethodError(name, _, _) => {
            let name = name.as_str();
            let short = name.rsplit('.').next().unwrap_or(name);
            io::Error::other(format!("BlueZ refused ({short})"))
        }
        _ => io::Error::other("BlueZ could not be reached"),
    }
}

/// One call to BlueZ's profile manager, bounded by [`CALL_WAIT`].
async fn call<B>(conn: &Connection, method: &str, body: &B) -> io::Result<()>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    tokio::time::timeout(
        CALL_WAIT,
        conn.call_method(
            Some(BLUEZ),
            PROFILE_MANAGER_PATH,
            Some(PROFILE_MANAGER),
            method,
            body,
        ),
    )
    .await
    .map_err(|_| io::Error::other("BlueZ did not answer"))?
    .map(|_| ())
    .map_err(|e| word(&e))
}

async fn register(conn: &Connection, spec: &ProfileSpec) -> io::Result<()> {
    let path = ObjectPath::try_from(spec.path).map_err(io::Error::other)?;
    let uuid = spec.uuid.to_string();
    call(
        conn,
        "RegisterProfile",
        &(path, uuid.as_str(), profile_options(spec.name)),
    )
    .await
}

async fn unregister(conn: &Connection, spec: &ProfileSpec) -> io::Result<()> {
    let path = ObjectPath::try_from(spec.path).map_err(io::Error::other)?;
    call(conn, "UnregisterProfile", &(path,)).await
}

/// Register items `0..count` in order with `register`; when one fails,
/// `unregister` the ones before it (best effort) and give its error: all or
/// none.
async fn all_or_none<R, RF, U, UF>(
    count: usize,
    mut register: R,
    mut unregister: U,
) -> io::Result<()>
where
    R: FnMut(usize) -> RF,
    RF: std::future::Future<Output = io::Result<()>>,
    U: FnMut(usize) -> UF,
    UF: std::future::Future<Output = io::Result<()>>,
{
    for i in 0..count {
        if let Err(e) = register(i).await {
            for done in (0..i).rev() {
                let _ = unregister(done).await;
            }
            return Err(e);
        }
    }
    Ok(())
}

/// The connections open now, across the three services: capped at `max`,
/// each with a token that ends it (its device asked BlueZ to disconnect, or
/// the listener stopped).
struct Table {
    max: usize,
    /// Cancelled when the listener stops: every connection's token is its
    /// child.
    stop: CancellationToken,
    state: Mutex<TableState>,
}

#[derive(Default)]
struct TableState {
    next: u64,
    /// Id, device (its D-Bus object path), and token of each open one.
    open: Vec<(u64, String, CancellationToken)>,
}

/// A place in the [`Table`], given back when dropped.
struct Slot {
    table: Arc<Table>,
    id: u64,
    token: CancellationToken,
}

impl Table {
    fn new(max: usize, stop: CancellationToken) -> Arc<Self> {
        Arc::new(Self {
            max,
            stop,
            state: Mutex::default(),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, TableState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A place for a connection from `device`; `None` with `max` open.
    fn take(self: &Arc<Self>, device: &str) -> Option<Slot> {
        let mut state = self.state();
        if state.open.len() >= self.max {
            return None;
        }
        let id = state.next;
        state.next += 1;
        let token = self.stop.child_token();
        state.open.push((id, device.to_string(), token.clone()));
        Some(Slot {
            table: self.clone(),
            id,
            token,
        })
    }

    /// End every connection from `device`.
    fn disconnect(&self, device: &str) {
        for (_, at, token) in &self.state().open {
            if at == device {
                token.cancel();
            }
        }
    }

    #[cfg(test)]
    fn open(&self) -> usize {
        self.state().open.len()
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.table.state().open.retain(|(id, _, _)| *id != self.id);
    }
}

/// What the three profile objects share.
struct Shared {
    accepted: mpsc::Sender<Incoming>,
    table: Arc<Table>,
    runtime: tokio::runtime::Handle,
    /// BlueZ released a profile or went away: the listener is over.
    ended: AtomicBool,
}

/// An exported `org.bluez.Profile1` for one of the services.
struct Profile {
    kind: Kind,
    shared: Arc<Shared>,
}

#[zbus::interface(name = "org.bluez.Profile1")]
impl Profile {
    /// A phone connected: `fd` is the connected RFCOMM socket.
    async fn new_connection(
        &self,
        device: ObjectPath<'_>,
        fd: OwnedFd,
        _properties: HashMap<String, OwnedValue>,
    ) -> fdo::Result<()> {
        let fd: std::os::fd::OwnedFd = fd.into();
        let Some(slot) = self.shared.table.take(device.as_str()) else {
            tracing::warn!("Refused a Bluetooth connection: {MAX_OPEN} are open already");
            // Dropping `fd` closes it; the error tells BlueZ it was refused.
            return Err(fdo::Error::LimitsExceeded("too many connections".into()));
        };
        // An RFCOMM socket is a plain stream socket to epoll and
        // read/write, so tokio's `UnixStream` (which only wraps the fd and
        // registers it) drives it; it needs no unix-socket calls.
        let socket = std::os::unix::net::UnixStream::from(fd);
        let socket = socket
            .set_nonblocking(true)
            .and_then(|()| tokio::net::UnixStream::from_std(socket));
        let socket = match socket {
            Ok(socket) => socket,
            Err(e) => {
                tracing::warn!("Dropped a Bluetooth connection: {e}");
                return Err(fdo::Error::Failed("the socket could not be used".into()));
            }
        };
        let (closed_tx, closed) = tokio::sync::oneshot::channel();
        let (ours, theirs) = tokio::io::duplex(PUMP_BUFFER);
        let incoming = Incoming {
            kind: self.kind,
            stream: ours,
            closed: Some(closed),
        };
        let accepted = self.shared.accepted.clone();
        self.shared.runtime.spawn(async move {
            if accepted.send(incoming).await.is_ok() {
                pump(socket, theirs, slot.token.clone()).await;
            }
            // The socket is closed (dropped) before these are.
            drop(slot);
            drop(closed_tx);
        });
        Ok(())
    }

    /// BlueZ asks for `device`'s connections to end.
    fn request_disconnection(&self, device: ObjectPath<'_>) {
        self.shared.table.disconnect(device.as_str());
    }

    /// BlueZ dropped the profile (bluetoothd is shutting down, say).
    fn release(&self) {
        self.shared.ended.store(true, Ordering::SeqCst);
    }
}

/// Join `socket` to the server's end of a stream until either side is done
/// or `end` is cancelled: what the phone sends goes in, what the server
/// writes goes out. Once the server ends its side (or a send times out) the
/// phone is sent the end of the stream and what was still in flight is read
/// for [`DRAIN`]; once the phone ends its side the server reads its end.
async fn pump(socket: tokio::net::UnixStream, theirs: DuplexStream, end: CancellationToken) {
    let (mut from_phone, mut to_phone) = socket.into_split();
    let (mut from_server, mut to_server) = tokio::io::split(theirs);
    let mut inbound = std::pin::pin!(async {
        let _ = tokio::io::copy(&mut from_phone, &mut to_server).await;
        let _ = to_server.shutdown().await;
    });
    let mut outbound = std::pin::pin!(async {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let read = match from_server.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            match tokio::time::timeout(SEND_TIMEOUT, to_phone.write_all(&buf[..read])).await {
                Ok(Ok(())) => {}
                _ => break,
            }
        }
        let _ = to_phone.shutdown().await;
    });
    let both = async {
        tokio::select! {
            () = &mut inbound => (&mut outbound).await,
            () = &mut outbound => {
                let _ = tokio::time::timeout(DRAIN, &mut inbound).await;
            }
        }
    };
    tokio::select! {
        () = both => {}
        () = end.cancelled() => {}
    }
}

/// BlueZ's radio.
pub(crate) struct BlueZ {
    /// The connection `address` asks on, made on first use and made again
    /// after an error.
    conn: tokio::sync::Mutex<Option<Connection>>,
}

impl BlueZ {
    pub(crate) fn new() -> Self {
        Self {
            conn: tokio::sync::Mutex::new(None),
        }
    }

    async fn adapter_address(&self) -> Option<[u8; 6]> {
        let mut slot = self.conn.lock().await;
        if slot.is_none() {
            *slot = Connection::system().await.ok();
        }
        let conn = slot.clone()?;
        let asked = conn.call_method(
            Some(BLUEZ),
            "/",
            Some("org.freedesktop.DBus.ObjectManager"),
            "GetManagedObjects",
            &(),
        );
        let objects = match tokio::time::timeout(CALL_WAIT, asked).await {
            Ok(Ok(reply)) => reply.body().deserialize::<Objects>().ok(),
            // bluetoothd not running is not an error of the connection.
            Ok(Err(zbus::Error::MethodError(..))) => None,
            _ => {
                *slot = None;
                None
            }
        };
        default_adapter(&objects?)
    }
}

impl super::Radio for BlueZ {
    fn address(&self) -> Option<[u8; 6]> {
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        runtime.block_on(self.adapter_address())
    }

    fn listen(
        &self,
        control: uuid::Uuid,
        link: uuid::Uuid,
        screen: uuid::Uuid,
        accepted: mpsc::Sender<Incoming>,
    ) -> io::Result<Box<dyn super::Listener>> {
        let runtime = tokio::runtime::Handle::try_current().map_err(io::Error::other)?;
        let specs = specs(control, link, screen);
        let listener = runtime.block_on(start(runtime.clone(), specs, accepted))?;
        Ok(Box::new(listener))
    }
}

/// The three profiles registered, until dropped.
struct BlueZListener {
    conn: Option<Connection>,
    specs: [ProfileSpec; 3],
    shared: Arc<Shared>,
    watcher: tokio::task::JoinHandle<()>,
    runtime: tokio::runtime::Handle,
}

/// Export the three profiles on a connection of their own and register them
/// all, or none.
async fn start(
    runtime: tokio::runtime::Handle,
    specs: [ProfileSpec; 3],
    accepted: mpsc::Sender<Incoming>,
) -> io::Result<BlueZListener> {
    let shared = Arc::new(Shared {
        accepted,
        table: Table::new(MAX_OPEN, CancellationToken::new()),
        runtime: runtime.clone(),
        ended: AtomicBool::new(false),
    });
    let mut builder = zbus::connection::Builder::system().map_err(|e| word(&e))?;
    for spec in &specs {
        let profile = Profile {
            kind: spec.kind,
            shared: shared.clone(),
        };
        builder = builder.serve_at(spec.path, profile).map_err(|e| word(&e))?;
    }
    let conn = tokio::time::timeout(CALL_WAIT, builder.build())
        .await
        .map_err(|_| io::Error::other("the system bus did not answer"))?
        .map_err(|e| word(&e))?;
    all_or_none(
        specs.len(),
        |i| register(&conn, &specs[i]),
        |i| unregister(&conn, &specs[i]),
    )
    .await?;
    let watcher = runtime.spawn(watch(conn.clone(), shared.clone()));
    Ok(BlueZListener {
        conn: Some(conn),
        specs,
        shared,
        watcher,
        runtime,
    })
}

/// Say the listener is over when bluetoothd leaves the bus: its profiles
/// went with it, and nothing else tells us.
async fn watch(conn: Connection, shared: Arc<Shared>) {
    use futures_lite::StreamExt;
    let ended = || shared.ended.store(true, Ordering::SeqCst);
    let Ok(dbus) = fdo::DBusProxy::new(&conn).await else {
        return ended();
    };
    let Ok(mut changes) = dbus
        .receive_name_owner_changed_with_args(&[(0, BLUEZ)])
        .await
    else {
        return ended();
    };
    while let Some(change) = changes.next().await {
        if change.args().is_ok_and(|args| args.new_owner().is_none()) {
            break;
        }
    }
    ended();
}

impl super::Listener for BlueZListener {
    /// BlueZ released a profile, or left the bus. (The adapter going or
    /// being switched off is seen by the server's own check of the radio's
    /// address, every 10 s, which stops the listener.)
    fn ended(&self) -> bool {
        self.shared.ended.load(Ordering::SeqCst)
    }
}

impl Drop for BlueZListener {
    fn drop(&mut self) {
        self.shared.table.stop.cancel();
        self.watcher.abort();
        if let Some(conn) = self.conn.take() {
            let specs = self.specs.clone();
            // Dropped inside the server's runtime, which must not block.
            self.runtime.spawn(async move {
                for spec in &specs {
                    let _ = unregister(&conn, spec).await;
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn objects(adapters: &[(&str, bool, &str)]) -> Objects {
        adapters
            .iter()
            .map(|(path, powered, address)| {
                let properties = HashMap::from([
                    (
                        "Powered".to_string(),
                        OwnedValue::try_from(Value::from(*powered)).unwrap(),
                    ),
                    (
                        "Address".to_string(),
                        OwnedValue::try_from(Value::from(*address)).unwrap(),
                    ),
                ]);
                (
                    OwnedObjectPath::try_from(*path).unwrap(),
                    HashMap::from([(ADAPTER.to_string(), properties)]),
                )
            })
            .collect()
    }

    #[test]
    fn the_default_adapter_is_the_first_powered_one() {
        let one = objects(&[("/org/bluez/hci0", true, "00:00:5E:00:53:01")]);
        assert_eq!(
            default_adapter(&one),
            Some([0x00, 0x00, 0x5E, 0x00, 0x53, 0x01])
        );
        let two = objects(&[
            ("/org/bluez/hci1", true, "00:00:5E:00:53:02"),
            ("/org/bluez/hci0", false, "00:00:5E:00:53:01"),
        ]);
        assert_eq!(
            default_adapter(&two),
            Some([0x00, 0x00, 0x5E, 0x00, 0x53, 0x02]),
            "hci0 is off: hci1 is the one"
        );
        let off = objects(&[("/org/bluez/hci0", false, "00:00:5E:00:53:01")]);
        assert_eq!(default_adapter(&off), None);
        assert_eq!(default_adapter(&Objects::new()), None, "no adapter");
        let bad = objects(&[("/org/bluez/hci0", true, "not an address")]);
        assert_eq!(default_adapter(&bad), None);
    }

    #[test]
    fn an_object_with_no_adapter_interface_is_no_adapter() {
        let mut other = objects(&[("/org/bluez/hci0", true, "00:00:5E:00:53:01")]);
        for interfaces in other.values_mut() {
            let adapter = interfaces.remove(ADAPTER).unwrap();
            interfaces.insert("org.bluez.Device1".to_string(), adapter);
        }
        assert_eq!(default_adapter(&other), None);
    }

    #[test]
    fn the_profile_options_ask_for_a_bonded_server() {
        let options = profile_options("Pocketdesk");
        assert_eq!(options["Name"], Value::from("Pocketdesk"));
        assert_eq!(options["Role"], Value::from("server"));
        assert_eq!(options["RequireAuthentication"], Value::from(true));
        assert_eq!(options["RequireAuthorization"], Value::from(false));
        assert_eq!(options["AutoConnect"], Value::from(false));
        assert_eq!(options.len(), 5, "nothing else is asked");
    }

    #[test]
    fn the_three_services_have_their_own_uuid_name_and_object() {
        use pocketdesk_core::protocol::bluetooth::{
            LINK_SERVICE_NAME, SERVICE_NAME, SERVICE_UUID, STREAM_SERVICE_NAME, link_uuid,
            stream_uuid,
        };
        let control = uuid::Uuid::parse_str(SERVICE_UUID).unwrap();
        let all = specs(control, link_uuid(control), stream_uuid(control));
        assert_eq!(
            all.iter().map(|s| s.kind).collect::<Vec<_>>(),
            [Kind::Control, Kind::Link, Kind::Screen]
        );
        assert_eq!(
            all.iter().map(|s| s.name).collect::<Vec<_>>(),
            [SERVICE_NAME, LINK_SERVICE_NAME, STREAM_SERVICE_NAME]
        );
        let mut uuids: Vec<_> = all.iter().map(|s| s.uuid).collect();
        uuids.sort();
        uuids.dedup();
        assert_eq!(uuids.len(), 3);
        let mut paths: Vec<_> = all.iter().map(|s| s.path).collect();
        paths.sort();
        paths.dedup();
        assert_eq!(paths.len(), 3);
        for spec in &all {
            assert!(ObjectPath::try_from(spec.path).is_ok(), "{}", spec.path);
        }
    }

    #[tokio::test]
    async fn one_refused_registration_takes_the_others_down() {
        let log = Mutex::new(Vec::new());
        let result = all_or_none(
            3,
            |i| {
                log.lock().unwrap().push(format!("register {i}"));
                async move {
                    if i == 2 {
                        Err(io::Error::other("refused"))
                    } else {
                        Ok(())
                    }
                }
            },
            |i| {
                log.lock().unwrap().push(format!("unregister {i}"));
                async { Ok(()) }
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(
            *log.lock().unwrap(),
            [
                "register 0",
                "register 1",
                "register 2",
                "unregister 1",
                "unregister 0"
            ]
        );
    }

    #[tokio::test]
    async fn all_registered_leaves_none_unregistered() {
        let log = Mutex::new(Vec::new());
        all_or_none(
            3,
            |i| {
                log.lock().unwrap().push(i);
                async { Ok(()) }
            },
            |_| async { panic!("nothing to take down") },
        )
        .await
        .unwrap();
        assert_eq!(*log.lock().unwrap(), [0, 1, 2]);
    }

    #[test]
    fn the_table_caps_the_open_connections_and_gives_places_back() {
        let table = Table::new(2, CancellationToken::new());
        let a = table.take("/dev_a").unwrap();
        let b = table.take("/dev_b").unwrap();
        assert!(table.take("/dev_c").is_none(), "two are open");
        assert_eq!(table.open(), 2);
        drop(a);
        assert_eq!(table.open(), 1);
        let c = table.take("/dev_c").expect("a place was given back");
        drop((b, c));
        assert_eq!(table.open(), 0);
    }

    #[test]
    fn a_disconnection_ends_that_devices_connections_only() {
        let table = Table::new(8, CancellationToken::new());
        let a1 = table.take("/dev_a").unwrap();
        let a2 = table.take("/dev_a").unwrap();
        let b = table.take("/dev_b").unwrap();
        table.disconnect("/dev_a");
        assert!(a1.token.is_cancelled() && a2.token.is_cancelled());
        assert!(!b.token.is_cancelled());
    }

    #[test]
    fn stopping_ends_every_connection() {
        let stop = CancellationToken::new();
        let table = Table::new(8, stop.clone());
        let a = table.take("/dev_a").unwrap();
        let b = table.take("/dev_b").unwrap();
        stop.cancel();
        assert!(a.token.is_cancelled() && b.token.is_cancelled());
    }
}
