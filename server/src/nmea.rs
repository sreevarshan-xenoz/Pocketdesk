//! An NMEA 0183 source for GeoClue (the phone's location for this
//! computer): GeoClue's network-NMEA source looks for `_nmea-0183._tcp`
//! services, connects to one and reads GGA and RMC sentences from it as if it
//! were a GPS. This serves them on this computer's loopback, to this
//! computer only, from the one fix its [`FixSource`] has (the phone's, asked
//! for when a client connects). Where the fix comes from, and the
//! announcement that lets GeoClue find this, are in `phone_location.rs`.
//!
//! Nothing here stores or logs where anything is.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use pocketdesk_core::protocol::phone_location::{Fix, Precision};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// How often a connected client is sent the fix again: a GPS speaks once a
/// second, but each sentence makes GeoClue tell every app a new place, so
/// this is slower.
pub const SEND_EVERY: Duration = Duration::from_secs(10);

/// How long a write to a client may take before it is dropped.
const WRITE_WITHIN: Duration = Duration::from_secs(5);

/// The most clients served at once; others are closed unserved.
pub const MOST_CLIENTS: usize = 8;

/// Where the fix comes from.
#[async_trait::async_trait]
pub trait FixSource: Send + Sync + 'static {
    /// The place to give now at `precision`, or none while there is no fix.
    /// May ask the phone first, and so take a while.
    async fn fix(&self, precision: Precision) -> Option<Fix>;
}

/// The checksum of a sentence's body: its bytes XORed, as two upper-case
/// hex digits tell it.
pub fn checksum(body: &str) -> u8 {
    body.bytes().fold(0, |sum, byte| sum ^ byte)
}

/// A whole sentence for `body` (the text between `$` and `*`): the
/// checksum and the CR LF line end NMEA ends a line with.
pub fn sentence(body: &str) -> String {
    format!("${body}*{:02X}\r\n", checksum(body))
}

/// A coordinate as NMEA writes it, as its two fields: degrees and minutes
/// with four decimals (`ddmm.mmmm` for a latitude, `degree_digits` 2;
/// `dddmm.mmmm` for a longitude, 3), and the hemisphere's letter.
fn coordinate(value: f64, degree_digits: usize, positive: char, negative: char) -> String {
    // In ten-thousandths of a minute: a whole count, so 59.99996 minutes
    // carries into the degree instead of printing as 60.0000.
    let total = (value.abs() * 60.0 * 10_000.0).round() as u64;
    let (degrees, rest) = (total / 600_000, total % 600_000);
    let letter = if value.is_sign_negative() && total != 0 {
        negative
    } else {
        positive
    };
    format!(
        "{degrees:0degree_digits$}{minutes:02}.{fraction:04},{letter}",
        minutes = rest / 10_000,
        fraction = rest % 10_000,
    )
}

/// Year, month, day, hour, minute and second (UTC) of `unix` seconds.
pub fn civil(unix: u64) -> (u64, u64, u64, u64, u64, u64) {
    let days = unix / 86_400;
    let secs = unix % 86_400;
    // Howard Hinnant's days-to-civil, for days since 1970-01-01.
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day, secs / 3600, secs % 3600 / 60, secs % 60)
}

/// The horizontal dilution of precision that makes a client read `accuracy_m`
/// metres: a GPS's rule of thumb is five metres a unit.
pub fn hdop(accuracy_m: u32) -> f64 {
    (f64::from(accuracy_m) / 5.0).clamp(1.0, 99.0)
}

/// The GGA sentence (fix data) for `fix` at `unix` seconds.
pub fn gga(fix: &Fix, unix: u64) -> String {
    let (_, _, _, h, m, s) = civil(unix);
    let latitude = coordinate(fix.lat, 2, 'N', 'S');
    let longitude = coordinate(fix.lon, 3, 'E', 'W');
    // Quality 1 (a GPS fix), eight satellites, altitude not known (0.0).
    sentence(&format!(
        "GPGGA,{h:02}{m:02}{s:02}.00,{},{},1,08,{:.1},0.0,M,0.0,M,,",
        latitude,
        longitude,
        hdop(fix.accuracy_m),
    ))
}

/// The RMC sentence (the minimum a GPS gives: a fix, speed, date) for `fix`
/// at `unix` seconds. Speed and course are zero: the phone's fix says none.
pub fn rmc(fix: &Fix, unix: u64) -> String {
    let (year, month, day, h, m, s) = civil(unix);
    let latitude = coordinate(fix.lat, 2, 'N', 'S');
    let longitude = coordinate(fix.lon, 3, 'E', 'W');
    sentence(&format!(
        "GPRMC,{h:02}{m:02}{s:02}.00,A,{},{},0.0,0.0,{day:02}{month:02}{:02},,,A",
        latitude,
        longitude,
        year % 100,
    ))
}

/// Whether a connection from `peer` is from this computer: loopback, or one
/// of `own` addresses (an IPv4 address in its IPv6-mapped form too).
pub fn is_own_address(peer: IpAddr, own: &[IpAddr]) -> bool {
    let peer = match peer {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    };
    peer.is_loopback() || own.contains(&peer)
}

/// This computer's own interface addresses.
pub fn own_addresses() -> Vec<IpAddr> {
    if_addrs::get_if_addrs()
        .map(|interfaces| interfaces.into_iter().map(|i| i.ip()).collect())
        .unwrap_or_default()
}

/// Whether a connection from `peer` is from this computer now: the check
/// [`serve`] is given.
pub fn from_this_computer(peer: IpAddr) -> bool {
    is_own_address(peer, &own_addresses())
}

/// The system users GeoClue runs as: `geoclue` on most distributions,
/// `srvGeoClue` on openSUSE.
const GEOCLUE_USERS: [&str; 2] = ["geoclue", "srvGeoClue"];

/// The uids of GeoClue's users in `passwd` (the text of `/etc/passwd`).
pub fn geoclue_uids(passwd: &str) -> Vec<u32> {
    passwd
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?;
            let uid = fields.nth(1)?.parse().ok()?;
            GEOCLUE_USERS.contains(&name).then_some(uid)
        })
        .collect()
}

/// An address as `/proc/net/tcp{,6}` prints it: each 32-bit word of the
/// address in host order (little-endian here), then the port, in hex.
fn proc_address(addr: SocketAddr) -> String {
    let words: Vec<String> = match addr.ip() {
        IpAddr::V4(v4) => vec![format!("{:08X}", u32::from_le_bytes(v4.octets()))],
        IpAddr::V6(v6) => v6
            .octets()
            .chunks(4)
            .map(|w| format!("{:08X}", u32::from_le_bytes([w[0], w[1], w[2], w[3]])))
            .collect(),
    };
    format!("{}:{:04X}", words.concat(), addr.port())
}

/// The uid owning the socket at `local` connected to `remote`, from the
/// text of `/proc/net/tcp` or `/proc/net/tcp6`.
pub fn socket_uid(table: &str, local: SocketAddr, remote: SocketAddr) -> Option<u32> {
    let (local, remote) = (proc_address(local), proc_address(remote));
    table.lines().skip(1).find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let matches = fields.get(1)?.eq_ignore_ascii_case(&local)
            && fields.get(2)?.eq_ignore_ascii_case(&remote);
        matches.then(|| fields.get(7)?.parse().ok()).flatten()
    })
}

/// Whether the client at `peer`, connected to this end `here`, is GeoClue
/// itself: from this computer, and its socket owned by GeoClue's user. Any
/// other program, of any user, is turned away, so GeoClue's own per-app
/// permission is the one that decides (#612 review). With no GeoClue user
/// on this computer no one is let in.
pub fn from_geoclue(peer: SocketAddr, here: SocketAddr) -> bool {
    if !from_this_computer(peer.ip()) {
        return false;
    }
    let uids = std::fs::read_to_string("/etc/passwd")
        .map(|passwd| geoclue_uids(&passwd))
        .unwrap_or_default();
    if uids.is_empty() {
        tracing::warn!("No GeoClue user on this computer: location clients are refused");
        return false;
    }
    let table = if peer.is_ipv4() { "/proc/net/tcp" } else { "/proc/net/tcp6" };
    std::fs::read_to_string(table)
        .ok()
        .and_then(|text| socket_uid(&text, peer, here))
        .is_some_and(|uid| uids.contains(&uid))
}

/// Listen on this computer's loopback only, IPv4 on a free port and IPv6 on
/// the same one where it can: nothing on the network can connect at all, and
/// [`serve`] still turns away what is not from this computer.
pub async fn bind() -> std::io::Result<Vec<TcpListener>> {
    let v4 = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    let port = v4.local_addr()?.port();
    let mut listeners = vec![v4];
    if let Ok(v6) = TcpListener::bind(SocketAddr::from((Ipv6Addr::LOCALHOST, port))).await {
        listeners.push(v6);
    }
    Ok(listeners)
}

/// Serve `source` to whoever connects to `listeners` until the task is
/// dropped: a client `admit` lets in (by its address and this end's: GeoClue's
/// own user on this computer, [`from_geoclue`])
/// is sent GGA and RMC for the fix at `precision()` at once and every `every`
/// while it stays; any other is closed unread.
pub async fn serve(
    listeners: Vec<TcpListener>,
    source: Arc<dyn FixSource>,
    precision: Arc<dyn Fn() -> Precision + Send + Sync>,
    admit: Arc<dyn Fn(SocketAddr, SocketAddr) -> bool + Send + Sync>,
    every: Duration,
) {
    let clients = Arc::new(tokio::sync::Semaphore::new(MOST_CLIENTS));
    let mut accepting = tokio::task::JoinSet::new();
    for listener in listeners {
        let (source, precision, admit, clients) = (
            source.clone(),
            precision.clone(),
            admit.clone(),
            clients.clone(),
        );
        accepting.spawn(async move {
            // Clients' own tasks go with this one: a JoinSet aborts them.
            let mut served = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream, peer)) = accepted else {
                            tokio::time::sleep(Duration::from_millis(200)).await;
                            continue;
                        };
                        let here = stream.local_addr().unwrap_or(peer);
                        if !admit(peer, here) {
                            tracing::info!("A location client that is not GeoClue was refused");
                            continue;
                        }
                        let Ok(permit) = clients.clone().try_acquire_owned() else {
                            tracing::info!("A location client was refused: {MOST_CLIENTS} are already served");
                            continue;
                        };
                        let (source, precision) = (source.clone(), precision.clone());
                        served.spawn(async move {
                            let _permit = permit;
                            client(stream, source, precision, every).await;
                        });
                    }
                    Some(_) = served.join_next(), if !served.is_empty() => {}
                }
            }
        });
    }
    while accepting.join_next().await.is_some() {}
}

/// One client: the sentences now and every `every`, until it closes or a
/// write does not finish.
async fn client(
    stream: TcpStream,
    source: Arc<dyn FixSource>,
    precision: Arc<dyn Fn() -> Precision + Send + Sync>,
    every: Duration,
) {
    let (mut reader, mut writer) = stream.into_split();
    // What the client says is never read for its words; a read that ends is
    // how it is known to have gone.
    let gone = async {
        let mut sink = [0u8; 256];
        while matches!(reader.read(&mut sink).await, Ok(n) if n > 0) {}
    };
    let sending = async {
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let Some(fix) = source.fix(precision()).await else {
                continue;
            };
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let lines = format!("{}{}", gga(&fix, now), rmc(&fix, now));
            let wrote = tokio::time::timeout(WRITE_WITHIN, writer.write_all(lines.as_bytes()));
            if !matches!(wrote.await, Ok(Ok(()))) {
                return;
            }
        }
    };
    tokio::select! {
        () = gone => {}
        () = sending => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::protocol::phone_location::Precision;

    fn fix(lat: f64, lon: f64, accuracy_m: u32) -> Fix {
        Fix {
            lat,
            lon,
            accuracy_m,
            precision: Precision::Exact,
        }
    }

    /// The checksum of the example sentence in NMEA's own documentation.
    #[test]
    fn the_checksum_is_the_xor_of_the_body() {
        assert_eq!(
            checksum("GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,"),
            0x47
        );
        assert_eq!(checksum(""), 0);
        assert_eq!(sentence("A"), "$A*41\r\n");
    }

    #[test]
    fn a_sentence_checks_out_by_its_own_checksum() {
        for line in [
            gga(&fix(12.5, -45.25, 30), 1_800_000_000),
            rmc(&fix(-33.5, 151.25, 30), 1_800_000_000),
        ] {
            let body = line
                .strip_prefix('$')
                .and_then(|l| l.strip_suffix("\r\n"))
                .expect("a $ line ending in CR LF");
            let (text, sum) = body.rsplit_once('*').expect("a checksum");
            assert_eq!(format!("{:02X}", checksum(text)), sum, "{line:?}");
        }
    }

    #[test]
    fn a_coordinate_is_degrees_and_minutes() {
        assert_eq!(coordinate(12.5, 2, 'N', 'S'), "1230.0000,N");
        assert_eq!(coordinate(-12.5, 2, 'N', 'S'), "1230.0000,S");
        assert_eq!(coordinate(151.25, 3, 'E', 'W'), "15115.0000,E");
        assert_eq!(coordinate(-0.5, 3, 'E', 'W'), "00030.0000,W");
        assert_eq!(coordinate(0.0, 3, 'E', 'W'), "00000.0000,E");
        // Four decimals of a minute: 0.01 degrees is 0.6 minutes.
        assert_eq!(coordinate(1.23, 2, 'N', 'S'), "0113.8000,N");
        // The edge carries into the degree.
        assert_eq!(coordinate(9.999_999_9, 2, 'N', 'S'), "1000.0000,N");
    }

    #[test]
    fn the_sentences_say_the_fix() {
        // 2027-01-15 08:00:00 UTC.
        let at = 1_800_000_000;
        assert_eq!(civil(at), (2027, 1, 15, 8, 0, 0));
        let f = fix(12.5, -45.25, 30);
        assert_eq!(
            gga(&f, at),
            sentence("GPGGA,080000.00,1230.0000,N,04515.0000,W,1,08,6.0,0.0,M,0.0,M,,")
        );
        assert_eq!(
            rmc(&f, at),
            sentence("GPRMC,080000.00,A,1230.0000,N,04515.0000,W,0.0,0.0,150127,,,A")
        );
    }

    #[test]
    fn civil_time_is_right_across_the_calendar() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil(951_782_400), (2000, 2, 29, 0, 0, 0), "a leap day");
        assert_eq!(civil(1_709_164_799), (2024, 2, 28, 23, 59, 59));
        assert_eq!(civil(1_709_164_800), (2024, 2, 29, 0, 0, 0));
        assert_eq!(civil(1_709_251_200), (2024, 3, 1, 0, 0, 0));
        assert_eq!(civil(4_102_444_799), (2099, 12, 31, 23, 59, 59));
    }

    #[test]
    fn accuracy_reads_back_as_about_the_same() {
        assert_eq!(hdop(1), 1.0);
        assert_eq!(hdop(5), 1.0);
        assert_eq!(hdop(30), 6.0);
        assert_eq!(hdop(1000), 99.0, "a city fix is as loose as NMEA says");
    }

    #[test]
    fn only_this_computer_may_connect() {
        let own: Vec<IpAddr> = vec![
            "192.0.2.10".parse().unwrap(),
            "2001:db8::10".parse().unwrap(),
        ];
        let yes = |s: &str| is_own_address(s.parse().unwrap(), &own);
        assert!(yes("127.0.0.1"));
        assert!(yes("127.9.9.9"));
        assert!(yes("::1"));
        assert!(yes("192.0.2.10"));
        assert!(yes("2001:db8::10"));
        assert!(yes("::ffff:192.0.2.10"), "an IPv4 address mapped into IPv6");
        assert!(yes("::ffff:127.0.0.1"));
        assert!(!yes("192.0.2.11"));
        assert!(!yes("198.51.100.1"));
        assert!(!yes("2001:db8::11"));
        assert!(!yes("::ffff:198.51.100.1"));
        assert!(!is_own_address("192.0.2.11".parse().unwrap(), &[]));
    }

    #[test]
    fn geoclues_users_are_found_by_name() {
        let passwd = "root:x:0:0::/root:/bin/bash
geoclue:x:964:964::/var/lib/geoclue:/usr/bin/nologin
srvGeoClue:x:470:470::/:/sbin/nologin
alice:x:1000:1000::/home/alice:/bin/sh
broken
";
        assert_eq!(geoclue_uids(passwd), vec![964, 470]);
        assert!(geoclue_uids("alice:x:1000:1000::/:/bin/sh
").is_empty());
    }

    #[test]
    fn a_sockets_owner_is_read_from_the_kernels_table() {
        // Two ends of one loopback connection, as /proc/net/tcp shows them:
        // the client's socket (owned by 964) and the server's (by 1000).
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:D431 0100007F:A2B4 01 00000000:00000000 00:00000000 00000000   964        0 1 1 0000000000000000 20 4 30 10 -1
   1: 0100007F:A2B4 0100007F:D431 01 00000000:00000000 00:00000000 00000000  1000        0 2 1 0000000000000000 20 4 30 10 -1
";
        let client: SocketAddr = "127.0.0.1:54321".parse().unwrap();
        let server: SocketAddr = "127.0.0.1:41652".parse().unwrap();
        assert_eq!(socket_uid(table, client, server), Some(964));
        assert_eq!(socket_uid(table, server, client), Some(1000));
        let other: SocketAddr = "127.0.0.1:1".parse().unwrap();
        assert_eq!(socket_uid(table, other, server), None);
    }

    #[test]
    fn an_ipv6_address_is_written_word_by_word() {
        let loopback: SocketAddr = "[::1]:80".parse().unwrap();
        assert_eq!(proc_address(loopback), "00000000000000000000000001000000:0050");
    }

    struct Fixed(Option<Fix>);

    #[async_trait::async_trait]
    impl FixSource for Fixed {
        async fn fix(&self, _: Precision) -> Option<Fix> {
            self.0
        }
    }

    async fn lines(stream: &mut TcpStream, count: usize) -> Vec<String> {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut reader = BufReader::new(stream);
        let mut out = Vec::new();
        for _ in 0..count {
            let mut line = String::new();
            tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line))
                .await
                .expect("a line in time")
                .expect("a line");
            out.push(line);
        }
        out
    }

    fn serving(
        source: Option<Fix>,
        admit: bool,
        listeners: Vec<TcpListener>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(serve(
            listeners,
            Arc::new(Fixed(source)),
            Arc::new(|| Precision::Exact),
            Arc::new(move |_, _| admit),
            Duration::from_millis(50),
        ))
    }

    #[tokio::test]
    async fn a_client_from_this_computer_is_sent_gga_and_rmc() {
        let listeners = bind().await.unwrap();
        let port = listeners[0].local_addr().unwrap().port();
        let server = serving(Some(fix(12.5, -45.25, 30)), true, listeners);
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        let got = lines(&mut client, 4).await;
        assert!(got[0].starts_with("$GPGGA,") && got[1].starts_with("$GPRMC,"));
        assert!(got[2].starts_with("$GPGGA,"), "again, as it stays");
        server.abort();
    }

    /// The check [`serve`] is given turns a client away: it is closed with
    /// nothing sent, though a fix is waiting.
    #[tokio::test]
    async fn a_client_the_check_refuses_is_closed_unserved() {
        let listeners = bind().await.unwrap();
        let port = listeners[0].local_addr().unwrap().port();
        let server = serving(Some(fix(12.5, -45.25, 30)), false, listeners);
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        let mut sink = [0u8; 16];
        let read = tokio::time::timeout(Duration::from_secs(5), client.read(&mut sink))
            .await
            .expect("closed in time");
        assert!(matches!(read, Ok(0) | Err(_)), "nothing was sent");
        server.abort();
    }

    #[tokio::test]
    async fn nothing_is_sent_while_there_is_no_fix() {
        let listeners = bind().await.unwrap();
        let port = listeners[0].local_addr().unwrap().port();
        let server = serving(None, true, listeners);
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        let mut sink = [0u8; 16];
        let read = tokio::time::timeout(Duration::from_millis(300), client.read(&mut sink)).await;
        assert!(read.is_err(), "still waiting, nothing sent");
        server.abort();
    }
}
