//! A direct link (no shared network), the server's side: this computer's own
//! Wi-Fi network, made for one phone for a while
//! (`pocketdesk_core::local::direct_link` is the part both programs share).
//!
//! The desktop app leaves a request in `<state>/direct_link_out/`; this
//! makes the access point through NetworkManager over D-Bus, writes the
//! status the app draws its QR from (`<state>/direct_link.json`, owner-only),
//! and removes the access point when
//!
//! * the owner turns it off,
//! * no phone has used it for the idle limit,
//! * the server exits (and a server that was killed leaves its access point
//!   for the next start to sweep: nothing about it is kept on disk, since
//!   NetworkManager is asked for a *volatile* connection, which it deletes
//!   when it goes down).
//!
//! The access point is made on a second virtual interface when the card
//! allows an access point beside its own connection and this program may
//! add an interface (`CAP_NET_ADMIN`); otherwise the hotspot takes over the
//! card, which the desktop says before it asks (and the request must say the
//! owner agreed, or nothing starts).
//!
//! The network's name and passphrase are made here, at random for each link,
//! and are never logged: log lines name the step, not the network.

use std::net::Ipv4Addr;
use std::sync::LazyLock;
use std::time::Duration;

use pocketdesk_core::local::direct_link::{
    self as dl, Action, Band, IdleTimer, Mode, Plan, Request, State, Status, WifiJoin,
};
use pocketdesk_core::local::paths::DIRECT_LINK_DIR;
use tokio::sync::Mutex;

/// The connection's name in NetworkManager, which is how a left-over one is
/// found.
pub const CONNECTION_ID: &str = "Pocketdesk direct link";
/// The virtual interface's name, when a second one is made.
pub const SECOND_INTERFACE: &str = "pdlink0";
/// How often an access point that is on is looked at.
const TICK: Duration = Duration::from_secs(5);

/// What is on now, and what to take down.
struct Active {
    started_unix: u64,
    mode: Mode,
    join: WifiJoin,
    address: String,
    idle_minutes: u32,
    idle: IdleTimer,
    handle: backend::Handle,
}

static ACTIVE: LazyLock<Mutex<Option<Active>>> = LazyLock::new(Default::default);

/// Daemon task: sweep a left-over access point, then serve the app's asks
/// and keep the one that is on.
pub async fn run() {
    let Ok(dir) = crate::state::state_dir() else {
        tracing::warn!("No state directory: the direct link is off");
        return;
    };
    // Whatever an earlier run of this state folder's server left: its
    // status says nothing, and its access point goes (and the card's own
    // connection comes back). Only what `direct_link_owned.json` names.
    backend::sweep(&dir).await;
    dl::clear_status(&dir);
    loop {
        crate::inbox::wait_at_most(DIRECT_LINK_DIR, TICK, Duration::from_secs(2)).await;
        let now = pocketdesk_core::local::pin::now_unix();
        for request in dl::take_all(&dir, now) {
            match request.action {
                Action::Start => start(&dir, &request).await,
                Action::Stop => {
                    stop(&dir, "asked").await;
                }
            }
        }
        tick(&dir).await;
    }
}

/// Take the access point down as the server goes. Bounded: a stuck
/// NetworkManager does not hold the exit.
pub async fn stop_at_exit() {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let _ = tokio::time::timeout(
        Duration::from_secs(10),
        stop(&dir, "the server is stopping"),
    )
    .await;
}

async fn start(dir: &std::path::Path, request: &Request) {
    let now = pocketdesk_core::local::pin::now_unix();
    if ACTIVE.lock().await.is_some() {
        tracing::info!("Direct link: already on");
        return;
    }
    let fail = |reason: &str| {
        tracing::info!("Direct link: not made: {reason}");
        let status = Status {
            state: State::Failed,
            mode: None,
            wifi: None,
            address: None,
            started_unix: now,
            updated_unix: now,
            idle_minutes: request.idle_minutes,
            idle_left_secs: None,
            in_use: false,
            error: Some(reason.to_string()),
        };
        if let Err(e) = dl::write_status(dir, &status) {
            tracing::warn!("Direct link: could not write its status: {e}");
        }
    };
    if !crate::events::current_config().direct_link {
        return fail("Direct links are off in Settings.");
    }
    let plan = match tokio::task::spawn_blocking(dl::probe).await {
        Ok(Ok(plan)) => plan,
        Ok(Err(why)) => return fail(why.words()),
        Err(_) => return fail("This computer's Wi-Fi could not be read."),
    };
    // A hotspot that takes over the card drops this computer's own Wi-Fi:
    // never without the owner's yes, whatever asked.
    if plan.disconnects && !request.accept_takeover {
        return fail("This computer's Wi-Fi would disconnect; the owner has not agreed to that.");
    }
    let join = WifiJoin {
        ssid: dl::new_ssid(),
        passphrase: dl::new_passphrase(),
    };
    let starting = Status {
        state: State::Starting,
        mode: Some(plan.mode),
        wifi: None,
        address: None,
        started_unix: now,
        updated_unix: now,
        idle_minutes: request.idle_minutes,
        idle_left_secs: None,
        in_use: false,
        error: None,
    };
    let _ = dl::write_status(dir, &starting);
    tracing::info!("Direct link: starting ({:?})", plan.mode);
    match backend::start(dir, &plan, &join, request.band).await {
        Ok((handle, address)) => {
            let active = Active {
                started_unix: now,
                mode: plan.mode,
                join,
                address,
                idle_minutes: request.idle_minutes,
                idle: IdleTimer::new(request.idle_minutes, now),
                handle,
            };
            write_on(dir, &active, now, false);
            *ACTIVE.lock().await = Some(active);
            tracing::info!("Direct link: on");
        }
        Err(reason) => fail(&reason),
    }
}

/// Take the access point down, if one is up, and the status with it.
/// Returns whether one was.
async fn stop(dir: &std::path::Path, why: &str) -> bool {
    let taken = ACTIVE.lock().await.take();
    let was = taken.is_some();
    if let Some(active) = taken {
        backend::stop(active.handle).await;
        dl::clear_owned(dir);
        tracing::info!("Direct link: off ({why})");
    }
    // A failed status goes with a stop too: the owner dismissed it.
    dl::clear_status(dir);
    was
}

/// Keep the access point that is on: note whether a phone uses it, turn it
/// off when none has for the limit, and say so in the status.
async fn tick(dir: &std::path::Path) {
    let now = pocketdesk_core::local::pin::now_unix();
    let mut guard = ACTIVE.lock().await;
    let Some(active) = guard.as_mut() else {
        return;
    };
    // Settings turned direct links off while one was on.
    let off = !crate::events::current_config().direct_link;
    let busy = in_use(dir, &active.address);
    active.idle.tick(now, busy);
    if off || active.idle.expired(now) {
        drop(guard);
        let why = if off {
            "turned off in Settings"
        } else {
            "idle"
        };
        stop(dir, why).await;
        return;
    }
    write_on(dir, active, now, busy);
}

fn write_on(dir: &std::path::Path, active: &Active, now: u64, busy: bool) {
    let status = Status {
        state: State::On,
        mode: Some(active.mode),
        wifi: Some(active.join.clone()),
        address: Some(active.address.clone()),
        started_unix: active.started_unix,
        updated_unix: now,
        idle_minutes: active.idle_minutes,
        idle_left_secs: Some(active.idle.left(now)),
        in_use: busy,
        error: None,
    };
    if let Err(e) = dl::write_status(dir, &status) {
        tracing::warn!("Direct link: could not write its status: {e}");
    }
}

/// Whether a phone is using the link now: a control connection or a session
/// from an address on its network.
fn in_use(dir: &std::path::Path, address: &str) -> bool {
    let Ok(ours) = address.parse::<Ipv4Addr>() else {
        return false;
    };
    let config = crate::events::current_config();
    let port = pocketdesk_core::local::listening::control_port(dir, config.control_port);
    let tcp = std::fs::read_to_string("/proc/net/tcp").unwrap_or_default();
    if parse_established_to(&tcp, port)
        .into_iter()
        .any(|peer| same_network(ours, peer))
    {
        return true;
    }
    let now = pocketdesk_core::local::pin::now_unix();
    std::fs::read_to_string(dir.join(pocketdesk_core::local::paths::LIVE_FILE))
        .ok()
        .and_then(|raw| pocketdesk_core::local::sessions::parse_mirror(&raw))
        .filter(|m| m.is_fresh(now))
        .is_some_and(|m| {
            m.sessions.iter().any(|s| {
                s.peer.parse::<std::net::SocketAddr>().is_ok_and(
                    |a| matches!(a.ip(), std::net::IpAddr::V4(ip) if same_network(ours, ip)),
                )
            })
        })
}

/// The access point's network is a /24 (NetworkManager's shared mode).
fn same_network(ours: Ipv4Addr, other: Ipv4Addr) -> bool {
    ours.octets()[..3] == other.octets()[..3]
}

/// The peers of the established TCP connections to local `port`, from a
/// `/proc/net/tcp`. Addresses there are the host's byte order in hex.
fn parse_established_to(text: &str, port: u16) -> Vec<Ipv4Addr> {
    let mut peers = Vec::new();
    for line in text.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let (Some(local), Some(remote), Some(state)) =
            (fields.get(1), fields.get(2), fields.get(3))
        else {
            continue;
        };
        // 01 is ESTABLISHED.
        if *state != "01" {
            continue;
        }
        let local_port = local
            .split_once(':')
            .and_then(|(_, p)| u16::from_str_radix(p, 16).ok());
        if local_port != Some(port) {
            continue;
        }
        let Some((addr, _)) = remote.split_once(':') else {
            continue;
        };
        if let Ok(n) = u32::from_str_radix(addr, 16) {
            // Little-endian on the machines that write it so.
            peers.push(Ipv4Addr::from(n.swap_bytes()));
        }
    }
    peers
}

/// The settings an access point is made from, as NetworkManager's
/// connection dictionary (`a{sa{sv}}`) names them. Pure, so a test reads it
/// without a bus.
#[cfg(target_os = "linux")]
fn connection_settings<'a>(
    interface: &'a str,
    join: &'a WifiJoin,
    band: Band,
    channel: Option<u32>,
    uuid: &'a str,
) -> std::collections::HashMap<
    &'static str,
    std::collections::HashMap<&'static str, zbus::zvariant::Value<'a>>,
> {
    use std::collections::HashMap;
    use zbus::zvariant::Value;
    let mut connection: HashMap<&'static str, Value<'a>> = HashMap::new();
    connection.insert("id", Value::from(CONNECTION_ID));
    connection.insert("uuid", Value::from(uuid));
    connection.insert("type", Value::from("802-11-wireless"));
    connection.insert("interface-name", Value::from(interface));
    connection.insert("autoconnect", Value::from(false));
    let mut wireless: HashMap<&'static str, Value<'a>> = HashMap::new();
    wireless.insert("ssid", Value::from(join.ssid.as_bytes().to_vec()));
    wireless.insert("mode", Value::from("ap"));
    // A card that keeps one channel for both connections decides the band
    // itself; otherwise the owner's choice, and 2.4 GHz for auto, which
    // every phone can join.
    let band = match channel {
        Some(c) => dl::band_of_channel(c),
        None => band,
    };
    wireless.insert("band", Value::from(band.nm_band().unwrap_or("bg")));
    if let Some(channel) = channel {
        wireless.insert("channel", Value::from(channel));
    }
    let mut security: HashMap<&'static str, Value<'a>> = HashMap::new();
    security.insert("key-mgmt", Value::from("wpa-psk"));
    security.insert("psk", Value::from(join.passphrase.as_str()));
    security.insert("proto", Value::from(vec!["rsn"]));
    security.insert("pairwise", Value::from(vec!["ccmp"]));
    security.insert("group", Value::from(vec!["ccmp"]));
    let mut ipv4: HashMap<&'static str, Value<'a>> = HashMap::new();
    ipv4.insert("method", Value::from("shared"));
    let mut ipv6: HashMap<&'static str, Value<'a>> = HashMap::new();
    ipv6.insert("method", Value::from("ignore"));
    HashMap::from([
        ("connection", connection),
        ("802-11-wireless", wireless),
        ("802-11-wireless-security", security),
        ("ipv4", ipv4),
        ("ipv6", ipv6),
    ])
}

#[cfg(target_os = "linux")]
mod backend {
    //! NetworkManager over D-Bus, and `iw` for the second interface.

    use super::*;
    use pocketdesk_core::local::direct_link::{Owned, Restore};
    use std::collections::HashMap;
    use std::path::Path;
    use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
    use zbus::{Connection, Proxy};

    const NM: &str = "org.freedesktop.NetworkManager";
    const NM_PATH: &str = "/org/freedesktop/NetworkManager";
    const ACTIVE_IFACE: &str = "org.freedesktop.NetworkManager.Connection.Active";
    const SETTINGS_CONNECTION_IFACE: &str = "org.freedesktop.NetworkManager.Settings.Connection";
    /// `NM_ACTIVE_CONNECTION_STATE_ACTIVATED`.
    const ACTIVATED: u32 = 2;
    /// `NM_ACTIVE_CONNECTION_STATE_DEACTIVATED`.
    const DEACTIVATED: u32 = 4;
    /// How long NetworkManager has to bring the network up.
    const UP_WITHIN: Duration = Duration::from_secs(25);

    /// What to take down, and what to bring back.
    pub struct Handle {
        active: OwnedObjectPath,
        settings: OwnedObjectPath,
        /// The virtual interface this made, to remove.
        interface: Option<String>,
        /// The connection a takeover took the card from.
        restore: Option<Restore>,
    }

    /// A D-Bus error as a short phrase with no values in it: NetworkManager's
    /// own messages can quote what was asked.
    fn word(e: &zbus::Error) -> String {
        match e {
            zbus::Error::MethodError(name, _, _) => {
                let name = name.as_str();
                let short = name.rsplit('.').next().unwrap_or(name);
                format!("NetworkManager refused ({short})")
            }
            _ => "NetworkManager could not be reached".to_string(),
        }
    }

    pub async fn start(
        dir: &Path,
        plan: &Plan,
        join: &WifiJoin,
        band: Band,
    ) -> Result<(Handle, String), String> {
        let conn = Connection::system()
            .await
            .map_err(|_| "The system bus could not be reached.".to_string())?;
        let nm = Proxy::new(&conn, NM, NM_PATH, NM)
            .await
            .map_err(|e| word(&e))?;
        let running: bool = nm
            .get_property::<u32>("State")
            .await
            // 10 is asleep (networking switched off): no hotspot there.
            .map(|s| s >= 20)
            .unwrap_or(false);
        if !running {
            return Err(dl::Unavailable::NoNetworkManager.words().to_string());
        }
        // What a takeover takes from the card, to bring back.
        let previous = match plan.mode {
            Mode::TakesOverCard => active_connection_of(&conn, &nm, &plan.device).await,
            Mode::SecondInterface => None,
        };
        let restore = dl::restore_for(plan.mode, &plan.device, previous);
        let uuid = uuid::Uuid::new_v4().to_string();
        let made = (plan.mode == Mode::SecondInterface).then(|| SECOND_INTERFACE.to_string());
        // Written before anything is made, so a crash leaves the record a
        // start sweeps by: this folder's server removes and restores what it
        // made, and no other server's.
        let owned = Owned {
            uuid: uuid.clone(),
            interface: made.clone(),
            restore: restore.clone(),
        };
        dl::write_owned(dir, &owned)
            .map_err(|_| "The state folder is not writable.".to_string())?;
        let interface = match &made {
            Some(name) => {
                if let Err(reason) = add_interface(&plan.device, name).await {
                    dl::clear_owned(dir);
                    return Err(reason);
                }
                name.clone()
            }
            None => plan.device.clone(),
        };
        match bring_up(&conn, &nm, &interface, plan, join, band, &uuid).await {
            Ok((active, settings, address)) => Ok((
                Handle {
                    active,
                    settings,
                    interface: made,
                    restore,
                },
                address,
            )),
            Err(reason) => {
                if let Some(name) = &made {
                    remove_interface(name).await;
                }
                // A takeover that failed part way may have dropped the
                // card's own connection.
                if let Some(restore) = &restore {
                    reactivate(&conn, &nm, restore).await;
                }
                dl::clear_owned(dir);
                Err(reason)
            }
        }
    }

    /// The settings path of the connection active on `device` now, if any.
    async fn active_connection_of(
        conn: &Connection,
        nm: &Proxy<'_>,
        device: &str,
    ) -> Option<String> {
        let path: OwnedObjectPath = nm.call("GetDeviceByIpIface", &(device,)).await.ok()?;
        let device = Proxy::new(conn, NM, path, "org.freedesktop.NetworkManager.Device")
            .await
            .ok()?;
        let active: OwnedObjectPath = device.get_property("ActiveConnection").await.ok()?;
        if active.as_str() == "/" {
            return None;
        }
        let active = Proxy::new(conn, NM, active, ACTIVE_IFACE).await.ok()?;
        let settings: OwnedObjectPath = active.get_property("Connection").await.ok()?;
        Some(settings.as_str().to_string())
    }

    /// Bring a connection a takeover took the card from back up.
    async fn reactivate(conn: &Connection, nm: &Proxy<'_>, restore: &Restore) {
        let _ = conn;
        let Ok(device) = nm
            .call::<_, _, OwnedObjectPath>("GetDeviceByIpIface", &(restore.device.as_str(),))
            .await
        else {
            return;
        };
        let (Ok(connection), Ok(root)) = (
            ObjectPath::try_from(restore.connection.as_str()),
            ObjectPath::try_from("/"),
        ) else {
            return;
        };
        let _ = nm
            .call::<_, _, OwnedObjectPath>("ActivateConnection", &(&connection, &device, &root))
            .await;
    }

    async fn bring_up(
        conn: &Connection,
        nm: &Proxy<'_>,
        interface: &str,
        plan: &Plan,
        join: &WifiJoin,
        band: Band,
        uuid: &str,
    ) -> Result<(OwnedObjectPath, OwnedObjectPath, String), String> {
        // A device NetworkManager has only just learned of may take a moment.
        let mut device: Option<OwnedObjectPath> = None;
        for _ in 0..20 {
            if let Ok(path) = nm
                .call::<_, _, OwnedObjectPath>("GetDeviceByIpIface", &(interface,))
                .await
            {
                device = Some(path);
                break;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        let device = device.ok_or("NetworkManager does not list this Wi-Fi card.")?;
        let settings = connection_settings(interface, join, band, plan.channel, uuid);
        let root = ObjectPath::try_from("/").map_err(|_| "internal".to_string())?;
        // Volatile: kept in memory only, and deleted by NetworkManager when
        // it goes down, so the passphrase never reaches a file of its own.
        let options: HashMap<&str, Value<'_>> =
            HashMap::from([("persist", Value::from("volatile"))]);
        let (settings_path, active, _result): (
            OwnedObjectPath,
            OwnedObjectPath,
            HashMap<String, OwnedValue>,
        ) = nm
            .call(
                "AddAndActivateConnection2",
                &(settings, &device, &root, options),
            )
            .await
            .map_err(|e| word(&e))?;
        // Up, or not: a connection that did not come up is removed.
        let active_proxy = Proxy::new(conn, NM, active.clone(), ACTIVE_IFACE)
            .await
            .map_err(|e| word(&e))?;
        let deadline = tokio::time::Instant::now() + UP_WITHIN;
        let failed = loop {
            match active_proxy.get_property::<u32>("State").await {
                Ok(ACTIVATED) => break None,
                Ok(DEACTIVATED) => {
                    break Some(
                        "NetworkManager could not make the network (sharing needs dnsmasq, and the card must allow a hotspot)."
                            .to_string(),
                    );
                }
                Err(e) => break Some(word(&e)),
                Ok(_) => {}
            }
            if tokio::time::Instant::now() >= deadline {
                break Some("The network did not come up in time.".to_string());
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        };
        if let Some(reason) = failed {
            deactivate(conn, nm, &active, &settings_path).await;
            return Err(reason);
        }
        match address_of(conn, &active_proxy).await {
            Some(address) => Ok((active, settings_path, address)),
            None => {
                deactivate(conn, nm, &active, &settings_path).await;
                Err("The network came up without an address for this computer.".to_string())
            }
        }
    }

    /// The computer's own address on the network just made.
    async fn address_of(conn: &Connection, active: &Proxy<'_>) -> Option<String> {
        for _ in 0..20 {
            let path: OwnedObjectPath = active.get_property("Ip4Config").await.ok()?;
            if path.as_str() != "/" {
                let config = Proxy::new(conn, NM, path, "org.freedesktop.NetworkManager.IP4Config")
                    .await
                    .ok()?;
                let data: Vec<HashMap<String, OwnedValue>> =
                    config.get_property("AddressData").await.ok()?;
                if let Some(first) = data.first()
                    && let Some(value) = first.get("address")
                    && let Ok(text) = String::try_from(value.try_clone().ok()?)
                    && text.parse::<Ipv4Addr>().is_ok()
                {
                    return Some(text);
                }
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        None
    }

    async fn deactivate(
        conn: &Connection,
        nm: &Proxy<'_>,
        active: &OwnedObjectPath,
        settings: &OwnedObjectPath,
    ) {
        let _ = nm
            .call::<_, _, ()>("DeactivateConnection", &(active,))
            .await;
        // A volatile connection goes with its activation; this is for one
        // that did not.
        if let Ok(proxy) = Proxy::new(conn, NM, settings.clone(), SETTINGS_CONNECTION_IFACE).await {
            let _ = proxy.call::<_, _, ()>("Delete", &()).await;
        }
    }

    pub async fn stop(handle: Handle) {
        if let Ok(conn) = Connection::system().await
            && let Ok(nm) = Proxy::new(&conn, NM, NM_PATH, NM).await
        {
            deactivate(&conn, &nm, &handle.active, &handle.settings).await;
            if let Some(name) = &handle.interface {
                remove_interface(name).await;
            }
            // The card's own connection comes back, as it was.
            if let Some(restore) = &handle.restore {
                reactivate(&conn, &nm, restore).await;
            }
        } else if let Some(name) = &handle.interface {
            remove_interface(name).await;
        }
    }

    /// Take down what an earlier run of this state folder's server made (the
    /// access point, its interface), and bring back what it took the card
    /// from. Only what `direct_link_owned.json` names: another server's link
    /// is none of this one's.
    pub async fn sweep(dir: &Path) {
        let Some(owned) = dl::read_owned(dir) else {
            return;
        };
        if let Ok(conn) = Connection::system().await
            && let Ok(nm) = Proxy::new(&conn, NM, NM_PATH, NM).await
        {
            if let Ok(actives) = nm
                .get_property::<Vec<OwnedObjectPath>>("ActiveConnections")
                .await
            {
                for active in actives {
                    let Ok(proxy) = Proxy::new(&conn, NM, active.clone(), ACTIVE_IFACE).await
                    else {
                        continue;
                    };
                    if proxy.get_property::<String>("Uuid").await.ok().as_deref()
                        == Some(owned.uuid.as_str())
                    {
                        let _ = nm
                            .call::<_, _, ()>("DeactivateConnection", &(&active,))
                            .await;
                    }
                }
            }
            if let Ok(settings) = Proxy::new(
                &conn,
                NM,
                "/org/freedesktop/NetworkManager/Settings",
                "org.freedesktop.NetworkManager.Settings",
            )
            .await
                && let Ok(path) = settings
                    .call::<_, _, OwnedObjectPath>("GetConnectionByUuid", &(owned.uuid.as_str(),))
                    .await
                && let Ok(proxy) = Proxy::new(&conn, NM, path, SETTINGS_CONNECTION_IFACE).await
            {
                let _ = proxy.call::<_, _, ()>("Delete", &()).await;
            }
            if let Some(name) = &owned.interface {
                remove_interface(name).await;
            }
            if let Some(restore) = &owned.restore {
                reactivate(&conn, &nm, restore).await;
            }
        } else if let Some(name) = &owned.interface {
            remove_interface(name).await;
        }
        dl::clear_owned(dir);
        tracing::info!("Direct link: swept one an earlier run left");
    }

    /// `iw dev <card> interface add <name> type __ap`.
    async fn add_interface(card: &str, name: &str) -> Result<(), String> {
        let iw = dl::iw_path().ok_or("`iw` is not installed.")?;
        let (card, name) = (card.to_string(), name.to_string());
        let ok = tokio::task::spawn_blocking(move || {
            std::process::Command::new(iw)
                .args(["dev", &card, "interface", "add", &name, "type", "__ap"])
                .output()
                .is_ok_and(|o| o.status.success())
        })
        .await
        .unwrap_or(false);
        if ok {
            Ok(())
        } else {
            Err("A second Wi-Fi interface could not be added (it needs more privilege than this program has).".to_string())
        }
    }

    async fn remove_interface(name: &str) {
        let Some(iw) = dl::iw_path() else {
            return;
        };
        let name = name.to_string();
        let _ = tokio::task::spawn_blocking(move || {
            std::process::Command::new(iw)
                .args(["dev", &name, "del"])
                .output()
        })
        .await;
    }
}

#[cfg(not(target_os = "linux"))]
mod backend {
    //! Not on this system yet (Windows is its own change).

    use super::*;
    use std::path::Path;

    pub struct Handle;

    pub async fn start(
        _dir: &Path,
        _plan: &Plan,
        _join: &WifiJoin,
        _band: Band,
    ) -> Result<(Handle, String), String> {
        Err(dl::Unavailable::Unsupported.words().to_string())
    }

    pub async fn stop(_handle: Handle) {}

    pub async fn sweep(_dir: &Path) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `/proc/net/tcp` as the kernel writes it: 192.0.2.7 is `070200C0`, the
    /// address's four bytes as a little-endian number; ports in hex.
    const PROC_NET_TCP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 11111 1 0 100 0 0 10 0
   1: 010200C0:06B4 070200C0:D2F4 01 00000000:00000000 00:00000000 00000000  1000        0 22222 1 0 20 4 25 10 -1
   2: 010200C0:06B4 090200C0:A0A0 06 00000000:00000000 00:00000000 00000000  1000        0 0 1 0 0 0 0 10 0
   3: 010200C0:1F90 0A0200C0:B000 01 00000000:00000000 00:00000000 00000000  1000        0 33333 1 0 20 4 25 10 -1
";

    #[test]
    fn the_phones_connected_to_the_control_port_are_read() {
        // Port 0x06B4 is 1716: only the established one (state 01).
        assert_eq!(
            parse_established_to(PROC_NET_TCP, 1716),
            vec![Ipv4Addr::new(192, 0, 2, 7)]
        );
        assert_eq!(
            parse_established_to(PROC_NET_TCP, 8080),
            vec![Ipv4Addr::new(192, 0, 2, 10)]
        );
        assert!(parse_established_to(PROC_NET_TCP, 9999).is_empty());
        assert!(parse_established_to("", 1716).is_empty());
        assert!(parse_established_to("junk\nmore junk", 1716).is_empty());
    }

    #[test]
    fn a_phone_is_on_the_link_only_from_its_own_network() {
        let ours = Ipv4Addr::new(192, 0, 2, 1);
        assert!(same_network(ours, Ipv4Addr::new(192, 0, 2, 7)));
        assert!(!same_network(ours, Ipv4Addr::new(198, 51, 100, 7)));
        assert!(!same_network(ours, Ipv4Addr::new(192, 0, 3, 7)));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_access_point_is_asked_for_as_a_volatile_wpa2_hotspot() {
        use zbus::zvariant::Value;
        let join = WifiJoin {
            ssid: "pocketdesk-ab12".into(),
            passphrase: "Abcdef123456".into(),
        };
        let uuid = "00000000-0000-4000-8000-000000000000";
        let settings = connection_settings("wlan0", &join, Band::Auto, None, uuid);
        let text = |section: &str, key: &str| -> String {
            match &settings[section][key] {
                Value::Str(s) => s.as_str().to_string(),
                other => format!("{other:?}"),
            }
        };
        assert_eq!(text("802-11-wireless", "mode"), "ap");
        assert_eq!(text("802-11-wireless", "band"), "bg", "auto is 2.4 GHz");
        assert_eq!(text("802-11-wireless-security", "key-mgmt"), "wpa-psk");
        assert_eq!(text("802-11-wireless-security", "psk"), "Abcdef123456");
        assert_eq!(text("ipv4", "method"), "shared");
        assert_eq!(text("connection", "id"), CONNECTION_ID);
        assert_eq!(text("connection", "interface-name"), "wlan0");
        assert!(!settings["802-11-wireless"].contains_key("channel"));
        // The 5 GHz choice, and a card that keeps a channel.
        let five = connection_settings("wlan0", &join, Band::Ghz5, None, uuid);
        assert_eq!(five["802-11-wireless"]["band"], Value::from("a"));
        let pinned = connection_settings("pdlink0", &join, Band::Ghz5, Some(9), uuid);
        assert_eq!(
            pinned["802-11-wireless"]["band"],
            Value::from("bg"),
            "the card's own channel decides the band"
        );
        assert_eq!(pinned["802-11-wireless"]["channel"], Value::from(9u32));
    }

    /// Nothing about the link's passphrase or name reaches a file of its
    /// own: only the owner-only status, which holds both while it is on.
    #[test]
    fn the_status_is_the_only_place_the_passphrase_is_written() {
        let source = include_str!("direct_link.rs");
        // Every logging macro's line is free of the details.
        for line in source.lines() {
            let line = line.trim();
            if line.starts_with("tracing::") {
                assert!(
                    !line.contains("passphrase")
                        && !line.contains("ssid")
                        && !line.contains("join"),
                    "{line}"
                );
            }
        }
    }
}
