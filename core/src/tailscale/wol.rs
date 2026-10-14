use anyhow::{Context, Result};
use std::net::UdpSocket;

/// The Wake-on-LAN magic packet for `mac_address`: 6 bytes of 0xFF, then
/// the MAC 16 times (102 bytes).
pub fn magic_packet(mac_address: &str) -> Result<Vec<u8>> {
    let mac = parse_mac(mac_address).context("Failed to parse MAC address")?;
    let mut packet = Vec::with_capacity(6 + 16 * 6);
    packet.extend_from_slice(&[0xFF; 6]);
    for _ in 0..16 {
        packet.extend_from_slice(&mac);
    }
    Ok(packet)
}

/// Send a Wake-on-LAN magic packet to the specified MAC address, as a UDP
/// broadcast to `broadcast_addr` on port 9.
pub fn send_wol(mac_address: &str, broadcast_addr: &str) -> Result<()> {
    let packet = magic_packet(mac_address)?;

    let socket = UdpSocket::bind("0.0.0.0:0").context("Failed to bind UDP socket for WOL")?;
    socket
        .set_broadcast(true)
        .context("Failed to enable broadcast on WOL socket")?;

    let addr = format!("{}:9", broadcast_addr);
    let sent = socket
        .send_to(&packet, &addr)
        .context("Failed to send WOL magic packet")?;

    // The MAC identifies the computer: it stays out of the log.
    tracing::info!("Sent WOL magic packet to {addr} ({sent} bytes)");
    Ok(())
}

/// How many times [`send_wake`] sends to each target: a lost UDP packet
/// should not cost a wake.
pub const WAKE_REPEATS: usize = 3;

/// Send the magic packet for `mac_address` to each target (broadcasts on
/// this network, or an address outside whose router forwards it),
/// [`WAKE_REPEATS`] times, 100 ms apart, from one socket with broadcast on.
/// Returns how many targets took at least one packet; an error only when
/// none did.
pub fn send_wake(mac_address: &str, targets: &[std::net::SocketAddr]) -> Result<usize> {
    if targets.is_empty() {
        anyhow::bail!("nowhere to send the wake to");
    }
    let packet = magic_packet(mac_address)?;
    let socket = UdpSocket::bind("0.0.0.0:0").context("Failed to bind UDP socket for WOL")?;
    socket
        .set_broadcast(true)
        .context("Failed to enable broadcast on WOL socket")?;

    let mut reached = vec![false; targets.len()];
    let mut last_error = None;
    for round in 0..WAKE_REPEATS {
        if round > 0 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        for (i, target) in targets.iter().enumerate() {
            match socket.send_to(&packet, target) {
                Ok(_) => reached[i] = true,
                Err(e) => last_error = Some(format!("{target}: {e}")),
            }
        }
    }
    let count = reached.iter().filter(|r| **r).count();
    if count == 0 {
        anyhow::bail!(
            "the wake could not be sent ({})",
            last_error.unwrap_or_default()
        );
    }
    tracing::info!("Sent the wake to {count} of {} targets", targets.len());
    Ok(count)
}

/// Send WOL to the broadcast address with automatic retries.
pub fn send_wol_with_retry(mac_address: &str, broadcast_addr: &str, retries: u32) -> Result<()> {
    for attempt in 1..=retries {
        if attempt > 1 {
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        match send_wol(mac_address, broadcast_addr) {
            Ok(()) => return Ok(()),
            Err(e) => {
                tracing::warn!("WOL attempt {attempt}/{retries} failed: {e}");
                if attempt == retries {
                    return Err(e).context("All WOL retries exhausted");
                }
            }
        }
    }
    Ok(())
}

/// Parse a MAC address string (e.g., "AA:BB:CC:DD:EE:FF" or "aa-bb-cc-dd-ee-ff")
/// into a 6-byte array.
fn parse_mac(mac: &str) -> Result<[u8; 6]> {
    let clean: String = mac
        .chars()
        .filter(|c| c.is_ascii_hexdigit() || *c == ':')
        .collect();
    let hex: String = clean.chars().filter(|c| c.is_ascii_hexdigit()).collect();

    if hex.len() != 12 {
        anyhow::bail!(
            "Invalid MAC address: expected 12 hex digits, got {} from '{}'",
            hex.len(),
            mac
        );
    }

    let mut bytes = [0u8; 6];
    for i in 0..6 {
        bytes[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .context(format!("Failed to parse hex byte at position {}", i))?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mac_colon() {
        let mac = parse_mac("AA:BB:CC:DD:EE:FF").unwrap();
        assert_eq!(mac, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    }

    #[test]
    fn test_parse_mac_hyphen() {
        let mac = parse_mac("aa-bb-cc-dd-ee-ff").unwrap();
        assert_eq!(mac, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    }

    #[test]
    fn test_parse_mac_lowercase() {
        let mac = parse_mac("a1:b2:c3:d4:e5:f6").unwrap();
        assert_eq!(mac, [0xA1, 0xB2, 0xC3, 0xD4, 0xE5, 0xF6]);
    }

    #[test]
    fn test_parse_mac_invalid() {
        assert!(parse_mac("not-a-mac").is_err());
        assert!(parse_mac("AA:BB:CC:DD:EE").is_err());
    }

    #[test]
    fn test_parse_mac_no_separator() {
        let mac = parse_mac("AABBCCDDEEFF").unwrap();
        assert_eq!(mac, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    }

    #[test]
    fn a_magic_packet_is_six_ff_then_the_mac_sixteen_times() {
        let packet = magic_packet("00:11:22:33:44:55").unwrap();
        assert_eq!(packet.len(), 102);
        assert_eq!(packet[0..6], [0xFF; 6]);
        for i in 0..16 {
            let start = 6 + i * 6;
            assert_eq!(
                packet[start..start + 6],
                [0x00, 0x11, 0x22, 0x33, 0x44, 0x55],
                "repetition {i}"
            );
        }
        assert!(magic_packet("not a mac").is_err());
    }

    #[test]
    fn a_wake_reaches_every_target_three_times() {
        let first = UdpSocket::bind("127.0.0.1:0").unwrap();
        let second = UdpSocket::bind("127.0.0.1:0").unwrap();
        for socket in [&first, &second] {
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
        }
        let targets = [first.local_addr().unwrap(), second.local_addr().unwrap()];

        let sent = send_wake("AA:BB:CC:DD:EE:FF", &targets).unwrap();
        assert_eq!(sent, 2, "both targets were sent to");

        let expected = magic_packet("AA:BB:CC:DD:EE:FF").unwrap();
        for socket in [&first, &second] {
            let mut buf = [0u8; 256];
            for _ in 0..WAKE_REPEATS {
                let (n, _) = socket.recv_from(&mut buf).unwrap();
                assert_eq!(&buf[..n], expected.as_slice());
            }
        }
    }

    #[test]
    fn a_wake_with_no_targets_is_an_error() {
        assert!(send_wake("AA:BB:CC:DD:EE:FF", &[]).is_err());
    }
}
