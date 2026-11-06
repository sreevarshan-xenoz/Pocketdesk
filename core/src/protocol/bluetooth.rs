//! The Bluetooth road's constants and packet (the fallback when neither the
//! home nor the internet road reaches the computer).
//!
//! The computer listens on an RFCOMM service with Pocketdesk's own UUID; a
//! bonded phone dials it. Over that stream the control link is the one at
//! home: TLS with the pinned certificate, then the usual handshake. A file
//! transfer opens a second RFCOMM connection, to the transfer service
//! ([`link_uuid`]: RFCOMM carries one connection per channel between two
//! devices), whose first line is the link hello ([`format_link_hello`]).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Pocketdesk's RFCOMM service UUID.
pub const SERVICE_UUID: &str = "b1f2c4e0-6a3d-4c8e-9f51-2d7e0a9c4b13";

/// The service's name in its SDP record.
pub const SERVICE_NAME: &str = "Pocketdesk";

/// The environment variable a test throwaway sets so it listens beside the
/// owner's own server on a service UUID of its own.
pub const SERVICE_UUID_ENV: &str = "POCKETDESK_BT_SERVICE_UUID";

/// The service UUID in use: [`SERVICE_UUID_ENV`] when it holds a UUID, else
/// [`SERVICE_UUID`].
pub fn service_uuid() -> String {
    service_uuid_from(std::env::var(SERVICE_UUID_ENV).ok().as_deref())
}

/// [`service_uuid`] for a given value of the variable: `env` when it parses
/// as a UUID (given back in lower case, hyphenated), else [`SERVICE_UUID`].
/// An override equal to the default's transfer service
/// ([`link_uuid`]`(SERVICE_UUID)`) is refused too: its own transfer service
/// would be the owner's control service.
pub fn service_uuid_from(env: Option<&str>) -> String {
    let default = Uuid::parse_str(SERVICE_UUID).expect("SERVICE_UUID is a UUID");
    env.and_then(|value| Uuid::parse_str(value.trim()).ok())
        .filter(|uuid| *uuid != link_uuid(default))
        .map(|uuid| uuid.hyphenated().to_string())
        .unwrap_or_else(|| SERVICE_UUID.to_string())
}

/// Computer → phone, over a working road: where to dial it by Bluetooth,
/// `{address, service}`. The phone keeps it with that computer's record.
pub const PACKET: &str = "kdeconnect.pocketdesk.bluetooth";

/// The body of [`PACKET`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BluetoothInfo {
    /// The radio's address, `AA:BB:CC:DD:EE:FF`.
    pub address: String,
    /// The service UUID the computer listens on.
    pub service: String,
}

/// The six bytes of a Bluetooth address written as `AA:BB:CC:DD:EE:FF`
/// (either case); `None` for anything else.
pub fn parse_address(text: &str) -> Option<[u8; 6]> {
    let mut bytes = [0u8; 6];
    let mut parts = text.split(':');
    for byte in &mut bytes {
        let part = parts.next()?;
        if part.len() != 2 || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        *byte = u8::from_str_radix(part, 16).ok()?;
    }
    parts.next().is_none().then_some(bytes)
}

/// The transfer service's name in its SDP record.
pub const LINK_SERVICE_NAME: &str = "Pocketdesk transfers";

/// The UUID of the transfer service that goes with the control service
/// `service`: the same UUID with the lowest bit of its last byte flipped.
/// RFCOMM carries one connection per channel between two devices, so a
/// transfer cannot open on the control link's own channel; it opens on this
/// second service. Both ends work it out from the control service, so a
/// throwaway's override separates its transfers too. Never `service` itself.
pub fn link_uuid(service: Uuid) -> Uuid {
    let mut bytes = *service.as_bytes();
    bytes[15] ^= 0x01;
    Uuid::from_bytes(bytes)
}

/// The first word of a transfer connection's first line: it says this
/// RFCOMM connection carries a transfer for a control link already open.
pub const LINK_HELLO: &str = "POCKETDESK-BT-LINK";

/// The first line of a transfer connection: [`LINK_HELLO`], a space, the
/// link's id and a newline.
pub fn format_link_hello(id: u64) -> String {
    format!("{LINK_HELLO} {id}\n")
}

/// The link id in a [`format_link_hello`] line, with or without its newline;
/// `None` for any other line.
pub fn parse_link_hello(line: &str) -> Option<u64> {
    let line = line.strip_suffix('\n').unwrap_or(line);
    let id = line.strip_prefix(LINK_HELLO)?.strip_prefix(' ')?;
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    id.parse().ok()
}

/// Computer → phone, over a Bluetooth control link: open a transfer
/// connection for link `{id}` (a `u64`). The phone opens a second RFCOMM
/// connection to the transfer service ([`link_uuid`]), sends the link hello
/// with that id, and then
/// answers the computer's TLS as its transfer listener does at home.
pub const LINK_PACKET: &str = "kdeconnect.pocketdesk.bluetooth.link";

/// The most a file may be to go over Bluetooth, in bytes (8 MiB): a slow
/// link is for a note or a photo, and a bigger file waits for Wi-Fi.
pub const TRANSFER_MAX: u64 = 8_388_608;

/// How the Bluetooth road stands on a computer, as its server last checked
/// (the switch each second, the radio every 10 s): what `pocketdesk status`
/// and the desktop's Settings say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoadState {
    /// Listening for a bonded phone.
    Listening,
    /// The switch is on, but there is no radio to listen on (none, it is
    /// off, or it would not listen).
    NoRadio,
    /// The switch (`bluetooth_fallback`) is off.
    Off,
    /// The switch is on, but this system does not listen yet: Windows only
    /// for now.
    Unsupported,
}

impl RoadState {
    /// The state with the switch `on`, on a system that `listens_here`,
    /// and a listener `listening` or not.
    pub fn of(on: bool, listens_here: bool, listening: bool) -> Self {
        if !on {
            RoadState::Off
        } else if listening {
            RoadState::Listening
        } else if !listens_here {
            RoadState::Unsupported
        } else {
            RoadState::NoRadio
        }
    }

    /// The state in words, after "Bluetooth: ".
    pub fn words(self) -> &'static str {
        match self {
            RoadState::Listening => "listening",
            RoadState::NoRadio => "no radio",
            RoadState::Off => "off",
            RoadState::Unsupported => "Windows only for now",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_road_state_follows_the_switch_and_the_radio() {
        assert_eq!(RoadState::of(true, true, true), RoadState::Listening);
        assert_eq!(RoadState::of(true, true, false), RoadState::NoRadio);
        assert_eq!(RoadState::of(false, true, false), RoadState::Off);
        // Off wins while a listener still stops.
        assert_eq!(RoadState::of(false, true, true), RoadState::Off);
        assert_eq!(RoadState::of(true, false, false), RoadState::Unsupported);
        assert_eq!(RoadState::of(false, false, false), RoadState::Off);
        let words: Vec<_> = [
            RoadState::Listening,
            RoadState::NoRadio,
            RoadState::Off,
            RoadState::Unsupported,
        ]
        .iter()
        .map(|s| s.words())
        .collect();
        assert_eq!(
            words,
            ["listening", "no radio", "off", "Windows only for now"]
        );
        assert_eq!(
            serde_json::to_value(RoadState::NoRadio).unwrap(),
            serde_json::json!("no_radio")
        );
    }

    #[test]
    fn service_uuid_override() {
        let other = "0f1e2d3c-4b5a-4968-8776-655443322110";
        assert_eq!(service_uuid_from(Some(other)), other);
        assert_eq!(
            service_uuid_from(Some("0F1E2D3C-4B5A-4968-8776-655443322110")),
            other,
            "upper case reads, lower case comes back"
        );
        assert_eq!(service_uuid_from(Some("not-a-uuid")), SERVICE_UUID);
        assert_eq!(service_uuid_from(Some("")), SERVICE_UUID);
        assert_eq!(service_uuid_from(None), SERVICE_UUID);
        // The default's transfer service as an override would take the
        // owner's control service for its transfers.
        assert_eq!(
            service_uuid_from(Some("b1f2c4e0-6a3d-4c8e-9f51-2d7e0a9c4b12")),
            SERVICE_UUID
        );
        assert!(Uuid::parse_str(SERVICE_UUID).is_ok(), "the default parses");
    }

    #[test]
    fn the_transfer_service_is_derived_and_never_the_control_one() {
        let control = Uuid::parse_str(SERVICE_UUID).unwrap();
        assert_eq!(
            link_uuid(control),
            Uuid::parse_str("b1f2c4e0-6a3d-4c8e-9f51-2d7e0a9c4b12").unwrap()
        );
        for service in [control, Uuid::nil(), Uuid::max(), Uuid::new_v4()] {
            assert_ne!(link_uuid(service), service);
            assert_eq!(link_uuid(link_uuid(service)), service, "one bit, both ways");
        }
        let other = Uuid::parse_str("0f1e2d3c-4b5a-4968-8776-655443322110").unwrap();
        assert_ne!(
            link_uuid(other),
            link_uuid(control),
            "an override separates them"
        );
    }

    #[test]
    fn address_round_trip() {
        let want = Some([0, 0, 0x5e, 0, 0x53, 1]);
        assert_eq!(parse_address("00:00:5E:00:53:01"), want);
        assert_eq!(parse_address("00:00:5e:00:53:01"), want);
        assert_eq!(parse_address("00:00:5E:00:53"), None);
        assert_eq!(parse_address("00:00:5E:00:53:01:02"), None);
        assert_eq!(parse_address("zz:00:5E:00:53:01"), None);
        assert_eq!(parse_address("0:0:5E:0:53:1"), None);
        assert_eq!(parse_address("+0:00:5E:00:53:01"), None);
        assert_eq!(parse_address(""), None);
    }

    #[test]
    fn the_packet_body_reads_and_writes_as_documented() {
        let info = BluetoothInfo {
            address: "00:00:5E:00:53:01".to_string(),
            service: SERVICE_UUID.to_string(),
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"address": "00:00:5E:00:53:01", "service": SERVICE_UUID})
        );
        assert_eq!(serde_json::from_value::<BluetoothInfo>(json).unwrap(), info);
        assert_eq!(PACKET, "kdeconnect.pocketdesk.bluetooth");
        assert_eq!(LINK_PACKET, "kdeconnect.pocketdesk.bluetooth.link");
    }

    #[test]
    fn link_hello_line() {
        assert_eq!(format_link_hello(7), "POCKETDESK-BT-LINK 7\n");
        for id in [0, 7, u64::MAX] {
            assert_eq!(parse_link_hello(&format_link_hello(id)), Some(id));
        }
        assert_eq!(parse_link_hello("POCKETDESK-BT-LINK 7"), Some(7));
        for bad in [
            "",
            "\n",
            "POCKETDESK-BT-LINK",
            "POCKETDESK-BT-LINK \n",
            "POCKETDESK-BT-LINK  7\n",
            "POCKETDESK-BT-LINK 7 \n",
            "POCKETDESK-BT-LINK -7\n",
            "POCKETDESK-BT-LINK +7\n",
            "POCKETDESK-BT-LINK 7x\n",
            "POCKETDESK-BT-LINK 18446744073709551616\n",
            "pocketdesk-bt-link 7\n",
            "POCKETDESK-BT-LINK 7\r\n",
            "POCKETDESK-BT-LINK 7\n\n",
            "POCKETDESK-BT-LINKS 7\n",
            "7\n",
        ] {
            assert_eq!(parse_link_hello(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_transfer_is_at_most_eight_mebibytes() {
        assert_eq!(TRANSFER_MAX, 8 * 1024 * 1024);
    }
}
