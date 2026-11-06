//! The Bluetooth road on this computer (the fallback when neither the home
//! nor the internet road reaches it): where a phone dials it by Bluetooth,
//! told over a working road. The control connection itself is
//! `service::handle_bluetooth_control`, under the internet road's rules.

use pocketdesk_core::protocol::bluetooth::{BluetoothInfo, PACKET};
use pocketdesk_core::protocol::kdeconnect::NetworkPacket;

/// Where a phone dials this computer by Bluetooth, when it listens there.
/// Nothing listens yet, so nothing is told.
pub(crate) fn info() -> Option<BluetoothInfo> {
    None
}

/// [`PACKET`] saying `info`.
pub(crate) fn packet(info: &BluetoothInfo) -> NetworkPacket {
    NetworkPacket::new(PACKET).with_body(serde_json::json!({
        "address": info.address,
        "service": info.service,
    }))
}
