//! Use my phone's hotspot (item 5's Shizuku, Tier 2's requirement): this
//! computer asks a phone to turn its own Wi-Fi hotspot on, with the hotspot
//! settings the phone already has, so this computer can get internet through
//! it. The phone's name for the network and its password never travel: the ask
//! is `hotspot_on` or `hotspot_off` and nothing else.
//!
//! This module is the computer's side, pure so it is tested: whether a phone
//! offers it (from the phone's switches report, feature `hotspot`), and, on
//! Linux with NetworkManager, joining the phone's network only when the owner
//! named one that is already a saved connection here. No password is ever
//! read, kept or passed: `nmcli connection up id <name>` uses what
//! NetworkManager already holds.

use super::phone_switches::Switch;

/// The longest Wi-Fi name: 32 bytes, as 802.11 allows.
pub const MAX_NETWORK_BYTES: usize = 32;

/// How long joining waits for the phone's network to appear, in seconds.
pub const JOIN_WAIT_SECS: u32 = 15;

/// Whether `name` can be the name the owner joins: one to 32 bytes, no
/// control character, and not starting with a dash (so it can never be read as
/// an option), with something in it that is not a space.
pub fn valid_network_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NETWORK_BYTES
        && !name.starts_with('-')
        && !name.chars().any(char::is_control)
        && !name.trim().is_empty()
}

/// Whether a phone offers its hotspot, from what it last said of the
/// `hotspot` feature: `Ok` when its switch is on and Android and Shizuku let
/// it work; else the reason, for the button's hover text.
pub fn offered(said: Option<&Switch>) -> Result<(), &'static str> {
    let Some(said) = said else {
        return Err(
            "The phone has not said it can. It needs a Pocketdesk that has this, and Shizuku running on it.",
        );
    };
    if !said.on {
        return Err(
            "On the phone, \"Let my computers turn on my hotspot\" is off (it is off by default).",
        );
    }
    if said.android == Some(false) {
        return Err(
            "The phone's Shizuku is not running, or has not allowed Pocketdesk, so it cannot turn its hotspot on.",
        );
    }
    Ok(())
}

/// The command that lists this computer's saved connections, name, UUID and
/// type, in NetworkManager's terse form. No shell.
pub fn list_argv() -> Vec<String> {
    ["nmcli", "-t", "-f", "NAME,UUID,TYPE", "connection", "show"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Whether `s` is a UUID as NetworkManager writes one: 36 characters of hex
/// in groups of 8, 4, 4, 4 and 12 joined by dashes.
pub fn valid_uuid(s: &str) -> bool {
    let groups: Vec<&str> = s.split('-').collect();
    s.len() == 36
        && groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The command that brings up the saved connection with this `uuid`, waiting
/// at most [`JOIN_WAIT_SECS`]; `None` for anything that is not a UUID. Only a
/// saved connection is ever named, by its UUID (`connection up uuid`, never
/// `device wifi connect`, which would take a password, and never a name that
/// two connections could share), so no password passes and the exact
/// connection is the one brought up.
pub fn join_argv(uuid: &str) -> Option<Vec<String>> {
    if !valid_uuid(uuid) {
        return None;
    }
    Some(vec![
        "nmcli".to_string(),
        "--wait".to_string(),
        JOIN_WAIT_SECS.to_string(),
        "connection".to_string(),
        "up".to_string(),
        "uuid".to_string(),
        uuid.to_string(),
    ])
}

/// The UUID of the one saved Wi-Fi connection called `name` in `listing` (the
/// output of [`list_argv`]). `None` when there is none, or when more than one
/// has that name (which one is meant is not for Pocketdesk to guess). In the
/// terse form a colon inside a name is escaped as `\:` and a backslash as
/// `\\`; the type is the last field and the UUID the one before it.
pub fn saved_wifi_uuid(listing: &str, name: &str) -> Option<String> {
    let mut found: Vec<String> = listing
        .lines()
        .filter_map(|line| {
            let (entry, uuid, kind) = split_terse(line)?;
            (entry == name && (kind == "802-11-wireless" || kind == "wifi") && valid_uuid(&uuid))
                .then_some(uuid)
        })
        .collect();
    (found.len() == 1).then(|| found.remove(0))
}

/// Split a terse `NAME:UUID:TYPE` line at its unescaped colons, undoing the
/// escapes in the name.
fn split_terse(line: &str) -> Option<(String, String, String)> {
    let mut field = String::new();
    let mut fields: Vec<String> = Vec::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    field.push(next);
                }
            }
            ':' => fields.push(std::mem::take(&mut field)),
            c => field.push(c),
        }
    }
    fields.push(field);
    let kind = fields.pop()?;
    let uuid = fields.pop()?;
    // Whatever came before is the name, colons restored.
    (!fields.is_empty()).then(|| (fields.join(":"), uuid, kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(on: bool, android: Option<bool>) -> Switch {
        Switch { on, android }
    }

    /// A phone offers its hotspot only with its own switch on and Shizuku usable.
    #[test]
    fn a_phone_offers_its_hotspot_only_when_it_says_so() {
        assert!(offered(Some(&said(true, Some(true)))).is_ok());
        assert!(offered(Some(&said(true, None))).is_ok());
        assert!(offered(None).is_err(), "an old phone says nothing");
        assert!(offered(Some(&said(false, Some(true)))).is_err());
        assert!(offered(Some(&said(true, Some(false)))).is_err());
    }

    /// Only a name a person could have saved is joined, and never as an option.
    #[test]
    fn a_network_name_is_checked_before_a_command_names_it() {
        assert!(valid_network_name("My phone"));
        assert!(valid_network_name(&"x".repeat(MAX_NETWORK_BYTES)));
        for bad in [
            "",
            "   ",
            "-x",
            "--help",
            "a\nb",
            "a\0b",
            "caf\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}",
        ] {
            assert!(!valid_network_name(bad), "{bad:?}");
        }
        assert!(!valid_network_name(&"x".repeat(MAX_NETWORK_BYTES + 1)));
    }

    const U1: &str = "0b1c2d3e-4f50-4a61-8b72-93a4b5c6d7e8";
    const U2: &str = "11111111-2222-3333-4444-555555555555";
    const U3: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

    /// The commands are fixed words around the one UUID, with no shell and no
    /// password; nothing but a UUID is ever named.
    #[test]
    fn the_join_command_is_fixed_and_names_a_saved_connection_by_uuid_only() {
        assert_eq!(
            join_argv(U1).unwrap(),
            ["nmcli", "--wait", "15", "connection", "up", "uuid", U1]
        );
        for bad in [
            "",
            "Home phone",
            "--version",
            "a; rm -rf x $(y) `z`",
            "0b1c2d3e-4f50-4a61-8b72-93a4b5c6d7e",
            "0b1c2d3e-4f50-4a61-8b72-93a4b5c6d7e8x",
            "0b1c2d3e4f504a618b7293a4b5c6d7e8aaaa",
            "zb1c2d3e-4f50-4a61-8b72-93a4b5c6d7e8",
        ] {
            assert!(join_argv(bad).is_none(), "{bad:?}");
        }
        let all: Vec<String> = join_argv(U1).unwrap().into_iter().chain(list_argv()).collect();
        for word in ["password", "device", "connect", "secrets", "--ask", "id"] {
            assert!(!all.iter().any(|a| a == word), "{word}");
        }
        assert_eq!(list_argv()[0], "nmcli");
        assert!(list_argv().contains(&"NAME,UUID,TYPE".to_string()));
    }

    /// Only one saved Wi-Fi connection of that name is joined, by its UUID,
    /// colons in the name and all.
    #[test]
    fn only_one_saved_wifi_connection_is_joined_by_its_uuid() {
        let listing = format!(
            "Wired connection 1:{U3}:802-3-ethernet\nMy phone:{U1}:802-11-wireless\nA\:B:{U2}:802-11-wireless\nvpn one:{U3}:vpn\nSame:{U3}:wifi\n"
        );
        assert_eq!(saved_wifi_uuid(&listing, "My phone").as_deref(), Some(U1));
        assert_eq!(saved_wifi_uuid(&listing, "A:B").as_deref(), Some(U2));
        assert_eq!(saved_wifi_uuid(&listing, "Same").as_deref(), Some(U3));
        assert_eq!(saved_wifi_uuid(&listing, "Wired connection 1"), None, "not Wi-Fi");
        assert_eq!(saved_wifi_uuid(&listing, "vpn one"), None);
        assert_eq!(saved_wifi_uuid(&listing, "Unknown"), None, "not saved: never joined");
        assert_eq!(saved_wifi_uuid(&listing, "My"), None, "a whole name only");
        assert_eq!(saved_wifi_uuid("", "My phone"), None);
    }

    /// Two saved connections with one name are not guessed between; a line
    /// with a bad UUID is not trusted.
    #[test]
    fn two_connections_of_one_name_or_a_bad_uuid_join_nothing() {
        let twice = format!("Dup:{U1}:802-11-wireless\nDup:{U2}:802-11-wireless\n");
        assert_eq!(saved_wifi_uuid(&twice, "Dup"), None);
        let one_wifi_one_vpn = format!("Dup:{U1}:802-11-wireless\nDup:{U2}:vpn\n");
        assert_eq!(saved_wifi_uuid(&one_wifi_one_vpn, "Dup").as_deref(), Some(U1));
        assert_eq!(saved_wifi_uuid("Odd:not-a-uuid:802-11-wireless\n", "Odd"), None);
        assert_eq!(saved_wifi_uuid("Odd:802-11-wireless\n", "Odd"), None, "no uuid field");
    }
}
