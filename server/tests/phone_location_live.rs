//! Live: GeoClue reading the phone's location from this computer's NMEA
//! source, with no phone. A stub stands in for the phone and gives a made-up
//! fix; GeoClue's `where-am-i` demo, started the way an app starts, must find
//! the source through Avahi, connect and report that place.
//!
//! Needs a Linux computer with `avahi-daemon` and `geoclue` running, the
//! GeoClue demo agent running in the user's session (it answers GeoClue's
//! "may this app know where you are?"), and `geoclue-where-am-i` allowed:
//!
//! ```text
//! sudo systemctl start avahi-daemon
//! printf '[geoclue-where-am-i]\nallowed=true\nsystem=true\nusers=\n' \
//!   | sudo tee /etc/geoclue/conf.d/90-pocketdesk-test.conf
//! /usr/lib/geoclue-2.0/demos/agent &
//! cargo test -p pocketdesk-server --test phone_location_live -- --ignored --nocapture
//! ```
//!
//! It prints counts only, and the place is made up (10 degrees north, 20
//! east).
#![cfg(target_os = "linux")]

use std::net::{IpAddr, SocketAddr};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use pocketdesk_core::protocol::phone_location::{Fix, Precision};
use pocketdesk_server::nmea::{self, FixSource};
use pocketdesk_server::nmea_announce::Announcement;
use pocketdesk_server::phone_location::{Asker, Fixes};

/// The stub phone: always the same made-up place, as the real one gives it.
struct StubPhone;

#[async_trait::async_trait]
impl Asker for StubPhone {
    async fn ask(&self, _precision: Precision) -> Result<(String, Fix), String> {
        Ok((
            "stub-phone".to_string(),
            Fix {
                lat: 10.0,
                lon: 20.0,
                accuracy_m: 5,
                precision: Precision::Exact,
            },
        ))
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs avahi-daemon, GeoClue and its demo agent (see the file's header)"]
async fn geoclue_reads_the_fake_fix_through_avahi() {
    let fixes: Arc<dyn FixSource> = Arc::new(Fixes::new(Arc::new(StubPhone), |_, _| {}));
    let listeners = nmea::bind().await.expect("a loopback port");
    let port = listeners[0].local_addr().unwrap().port();
    let announcement = Announcement::publish(port, Precision::Exact)
        .await
        .expect("avahi-daemon to take the announcement");
    let serving = tokio::spawn(nmea::serve(
        listeners,
        fixes,
        Arc::new(|| Precision::Exact),
        Arc::new(nmea::from_this_computer),
        Duration::from_secs(2),
    ));

    // Nothing on the network side can connect: not by any of this
    // computer's own addresses but its loopback.
    let mut refused = 0;
    let mut tried = 0;
    for ip in nmea::own_addresses()
        .into_iter()
        .filter(|ip| !ip.is_loopback())
    {
        if matches!(ip, IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80) {
            continue; // link-local needs a scope
        }
        tried += 1;
        let attempt = tokio::time::timeout(
            Duration::from_secs(2),
            tokio::net::TcpStream::connect(SocketAddr::new(ip, port)),
        )
        .await;
        if !matches!(attempt, Ok(Ok(_))) {
            refused += 1;
        }
    }
    println!("non-loopback addresses tried: {tried}, refused: {refused}");
    assert_eq!(refused, tried, "no network address may reach the source");

    let demo = std::env::var("POCKETDESK_WHERE_AM_I")
        .unwrap_or_else(|_| "/usr/lib/geoclue-2.0/demos/where-am-i".to_string());
    let output = tokio::task::spawn_blocking(move || {
        Command::new(demo)
            .args(["-t", "14", "-a", "8"])
            .output()
            .expect("where-am-i to run")
    })
    .await
    .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    let updates = text.matches("New location").count();
    let ours = text
        .lines()
        .filter(|l| l.starts_with("Latitude:") && l.contains("10.000000"))
        .count();
    let from_nmea = text.matches("GPS GGA+RMC").count();
    println!("updates: {updates}, made-up place: {ours}, read from NMEA: {from_nmea}");

    serving.abort();
    announcement.withdraw().await;
    assert!(updates > 0, "where-am-i got no location at all");
    assert!(ours > 0, "none of its updates was the stub's place");
    assert!(from_nmea > 0, "none came from the NMEA source");
}
