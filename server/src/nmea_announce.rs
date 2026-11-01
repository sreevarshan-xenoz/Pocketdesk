//! Telling GeoClue where the NMEA source is (the phone's location for this
//! computer). GeoClue 2.8's network-NMEA source (on by default,
//! `[network-nmea]` in `geoclue.conf`) browses `_nmea-0183._tcp` through
//! **Avahi**, and then connects to the service's *host name*, not to the
//! address Avahi resolved. So the usual mDNS announcement, which names the
//! computer `<name>.local`, does not work where nothing resolves `.local`
//! (Arch has no nss-mdns by default).
//!
//! The announcement here is made to Avahi itself over D-Bus, on the
//! loopback interface only and under a `*.localhost` host name that every
//! system resolver (systemd's nss-myhostname and resolved, musl) turns into
//! this computer's loopback. Nothing about it reaches the network: no
//! other computer sees the service, and the name gives away nothing. It goes
//! when this process's D-Bus connection does.

use pocketdesk_core::protocol::phone_location::Precision;

/// Avahi's service type for an NMEA source, as GeoClue browses it.
pub const SERVICE_TYPE: &str = "_nmea-0183._tcp";

/// The TXT record GeoClue reads the source's accuracy from (`accuracy=city`
/// or `accuracy=exact`; GeoClue asks only sources as good as the app wants).
pub fn txt_record(precision: Precision) -> Vec<u8> {
    format!("accuracy={}", precision.name()).into_bytes()
}

/// The host name announced for this process: one a system resolver maps to
/// the loopback, and no other process's.
pub fn host_name(pid: u32) -> String {
    format!("pocketdesk-{pid}.localhost")
}

#[cfg(target_os = "linux")]
pub use linux::Announcement;

#[cfg(not(target_os = "linux"))]
pub use elsewhere::Announcement;

#[cfg(target_os = "linux")]
mod linux {
    use super::{Precision, SERVICE_TYPE, host_name, txt_record};

    const BUS_NAME: &str = "org.freedesktop.Avahi";
    /// AVAHI_PROTO_INET: IPv4 only; the loopback address is 127.0.0.1.
    const PROTO_INET: i32 = 0;
    /// AVAHI_PUBLISH_NO_REVERSE: no PTR record for the loopback address.
    const NO_REVERSE: u32 = 16;

    /// The live announcement: the entry group Avahi holds for this process.
    pub struct Announcement {
        group: zbus::Proxy<'static>,
        _conn: zbus::Connection,
    }

    impl Announcement {
        /// Announce an NMEA source on `port` with `precision`'s accuracy.
        /// Fails, with why in words, when Avahi is not running.
        pub async fn publish(port: u16, precision: Precision) -> Result<Self, String> {
            let conn = zbus::Connection::system()
                .await
                .map_err(|_| "no system D-Bus".to_string())?;
            let server = zbus::Proxy::new(&conn, BUS_NAME, "/", "org.freedesktop.Avahi.Server")
                .await
                .map_err(|_| "Avahi could not be reached".to_string())?;
            // Only the loopback: never the network's interfaces.
            let lo: i32 = server
                .call("GetNetworkInterfaceIndexByName", &("lo",))
                .await
                .map_err(|_| "avahi-daemon is not running".to_string())?;
            let path: zbus::zvariant::OwnedObjectPath = server
                .call("EntryGroupNew", &())
                .await
                .map_err(|_| "Avahi would not make an entry group".to_string())?;
            let group = zbus::Proxy::new(&conn, BUS_NAME, path, "org.freedesktop.Avahi.EntryGroup")
                .await
                .map_err(|_| "Avahi's entry group could not be reached".to_string())?;
            let pid = std::process::id();
            let host = host_name(pid);
            let name = format!("Pocketdesk {pid}");
            let failed = |what: &'static str| move |_: zbus::Error| what.to_string();
            let _: () = group
                .call(
                    "AddAddress",
                    &(lo, PROTO_INET, NO_REVERSE, host.as_str(), "127.0.0.1"),
                )
                .await
                .map_err(failed("Avahi refused the loopback address record"))?;
            let txt: Vec<Vec<u8>> = vec![txt_record(precision)];
            let _: () = group
                .call(
                    "AddService",
                    &(
                        lo,
                        PROTO_INET,
                        0u32,
                        name.as_str(),
                        SERVICE_TYPE,
                        "",
                        host.as_str(),
                        port,
                        txt,
                    ),
                )
                .await
                .map_err(failed("Avahi refused the service"))?;
            let _: () = group
                .call("Commit", &())
                .await
                .map_err(failed("Avahi would not publish the service"))?;
            Ok(Self { group, _conn: conn })
        }

        /// Whether Avahi still holds the announcement (it does not after
        /// avahi-daemon restarts, when it must be made again).
        pub async fn alive(&self) -> bool {
            self.group.call::<_, _, i32>("GetState", &()).await.is_ok()
        }

        /// Take the announcement back.
        pub async fn withdraw(self) {
            let _ = self.group.call::<_, _, ()>("Free", &()).await;
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod elsewhere {
    use super::Precision;

    /// Nothing announces here: GeoClue is Linux's.
    pub struct Announcement;

    impl Announcement {
        pub async fn publish(_port: u16, _precision: Precision) -> Result<Self, String> {
            Err("GeoClue is Linux's".to_string())
        }

        pub async fn alive(&self) -> bool {
            false
        }

        pub async fn withdraw(self) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_record_names_the_precision() {
        assert_eq!(txt_record(Precision::City), b"accuracy=city");
        assert_eq!(txt_record(Precision::Exact), b"accuracy=exact");
    }

    #[test]
    fn the_host_is_a_localhost_name() {
        let host = host_name(4242);
        assert_eq!(host, "pocketdesk-4242.localhost");
        assert!(host.ends_with(".localhost"));
    }
}
