//! Use my phone's hotspot: after the phone is asked to turn its hotspot on,
//! tell the owner to connect to its network, and on Linux with NetworkManager
//! optionally join it for them, but only if the owner named a network in
//! Settings that is already a saved connection here. The phone's password is
//! never asked for, kept or passed: `nmcli connection up id <name>` uses what
//! NetworkManager already holds. The decisions and the commands are
//! `pocketdesk_core::local::phone_hotspot`'s, pure and tested there.

use std::io;
use std::process::{Command, Stdio};
use std::time::Duration;

use pocketdesk_core::local::phone_hotspot as hotspot;

/// How many times joining is tried, and how long between tries: the phone's
/// network takes a few seconds to appear after its hotspot starts.
const ATTEMPTS: u32 = 4;
const BETWEEN: Duration = Duration::from_secs(5);

/// What the owner is told when the phone was asked: always to connect, and
/// that a join is under way when one is.
pub fn asked_words(who: &str, joining: Option<&str>) -> String {
    match joining {
        Some(name) => format!(
            "Asked {who} to turn on its hotspot. Joining \u{201c}{name}\u{201d} once it appears (a network saved here)."
        ),
        None => format!(
            "Asked {who} to turn on its hotspot. When its network appears, connect to it in this computer's Wi-Fi list; the phone's password is never kept here."
        ),
    }
}

/// The network to join after an ask, if the owner named a valid one and this
/// computer can join it (Linux). Otherwise `None`: only the words above.
pub fn network_to_join(setting: &str, linux: bool) -> Option<String> {
    let name = setting.trim();
    (linux && hotspot::valid_network_name(name)).then(|| name.to_string())
}

/// Join the saved Wi-Fi connection `name`, retrying while the phone's network
/// starts up; blocks for up to about a minute, so it runs off the window's
/// thread. A name that is not a saved Wi-Fi connection is never joined.
pub fn join(name: &str) -> Result<String, String> {
    if !hotspot::valid_network_name(name) {
        return Err("That is not a Wi-Fi network name.".to_string());
    }
    if !cfg!(target_os = "linux") {
        return Err("Joining by itself is for Linux with NetworkManager.".to_string());
    }
    let listing = run(&hotspot::list_argv())
        .map_err(|e| format!("Could not look at the saved networks (is NetworkManager's nmcli installed?): {e}"))?;
    if !hotspot::is_saved_wifi(&listing.1, name) {
        return Err(format!(
            "\u{201c}{name}\u{201d} is not a Wi-Fi network saved on this computer, so it was not joined. Connect to your phone's network once yourself, then it can be joined from here."
        ));
    }
    let Some(argv) = hotspot::join_argv(name) else {
        return Err("That is not a Wi-Fi network name.".to_string());
    };
    for attempt in 1..=ATTEMPTS {
        if let Ok((true, _)) = run(&argv) {
            return Ok(format!("Joined \u{201c}{name}\u{201d}."));
        }
        if attempt < ATTEMPTS {
            std::thread::sleep(BETWEEN);
        }
    }
    Err(format!(
        "The phone's network \u{201c}{name}\u{201d} did not come up. Check the phone's hotspot, then connect to it yourself."
    ))
}

/// Run `argv` (no shell), returning whether it succeeded and its output.
fn run(argv: &[String]) -> io::Result<(bool, String)> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no command"))?;
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()?;
    Ok((
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A join happens only when the owner named a valid network, and only on Linux.
    #[test]
    fn a_network_is_joined_only_when_named_and_valid_on_linux() {
        assert_eq!(network_to_join("My phone", true).as_deref(), Some("My phone"));
        assert_eq!(network_to_join("  My phone  ", true).as_deref(), Some("My phone"));
        assert_eq!(network_to_join("My phone", false), None);
        for bad in ["", "   ", "--help", "a\nb"] {
            assert_eq!(network_to_join(bad, true), None, "{bad:?}");
        }
    }

    /// A name that cannot be one is refused before any program runs.
    #[test]
    fn a_bad_name_is_refused_before_anything_runs() {
        assert!(join("--version").is_err());
        assert!(join("").is_err());
        assert!(join("a\nb").is_err());
    }

    /// The words never name a password, and say what a join does and does not do.
    #[test]
    fn the_words_say_to_connect_and_never_ask_for_a_password() {
        let plain = asked_words("Pixel", None);
        assert!(plain.contains("connect to it"));
        assert!(plain.contains("never kept"));
        let joining = asked_words("Pixel", Some("My phone"));
        assert!(joining.contains("Joining"));
        assert!(joining.contains("saved here"));
        assert!(!joining.to_lowercase().contains("enter the password"));
    }
}
