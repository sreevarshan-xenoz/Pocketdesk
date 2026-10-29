//! Whether Windows treats a network this computer is on as Public: its
//! firewall rules for Pocketdesk (the installer's "Pocketdesk control",
//! "media" and "KDE Connect files") cover only the Domain and Private
//! profiles, on purpose, so on a Public network nothing reaches the server
//! and another computer's pairing times out with no word why (the owner's
//! first live test: a home Ethernet Windows had marked Public). The Pair
//! page and Settings → Other computers say so, and how to change it.
//!
//! The words are chosen here ([`notice`], pure); Windows' Network List
//! Manager is asked off the UI thread every [`EVERY`] ([`PublicNetwork`]).
//! Elsewhere nothing is asked and nothing is said, so off Windows the
//! words are built only by the tests (PR #552 review C1: dead code there).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the networks are asked again while a page that says it shows.
pub const EVERY: Duration = Duration::from_secs(10);

/// The most of a network's name shown.
#[cfg_attr(not(windows), allow(dead_code))]
const NAME_CHARS: usize = 40;

/// A network this computer is connected to, as Windows names it, and
/// whether Windows treats it as Public.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    pub name: String,
    pub public: bool,
}

/// What to tell the owner of `networks`: nothing while none is Public
/// (Private and Domain networks are what the firewall rules open).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn notice(networks: &[Network]) -> Option<String> {
    let public: Vec<String> = networks
        .iter()
        .filter(|n| n.public)
        .map(|n| shown_name(&n.name))
        .collect();
    let (which, it) = match public.as_slice() {
        [] => return None,
        [one] if one.is_empty() => ("this network".to_string(), "it"),
        [one] => (format!("the network \"{one}\""), "it"),
        many => {
            let names: Vec<String> = many
                .iter()
                .map(|n| {
                    if n.is_empty() {
                        "an unnamed one".to_string()
                    } else {
                        format!("\"{n}\"")
                    }
                })
                .collect();
            (format!("the networks {}", names.join(", ")), "each")
        }
    };
    Some(format!(
        "Windows treats {which} as Public, so other devices can't reach Pocketdesk. Set {it} to \
         Private in Settings → Network & internet → Ethernet or Wi-Fi → the network → Network \
         profile type."
    ))
}

/// A network's name fit to show: no control or format characters (a
/// right-to-left override), trimmed and cut short.
#[cfg_attr(not(windows), allow(dead_code))]
fn shown_name(raw: &str) -> String {
    let clean: String = raw
        .chars()
        .filter(|c| !c.is_control() && !matches!(u32::from(*c), 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2069 | 0xFEFF))
        .collect();
    let clean = clean.trim();
    if clean.chars().count() > NAME_CHARS {
        let cut: String = clean.chars().take(NAME_CHARS).collect();
        format!("{cut}…")
    } else {
        clean.to_string()
    }
}

/// The notice for the networks now, asked off the UI thread every
/// [`EVERY`] and kept between frames.
#[derive(Default)]
pub struct PublicNetwork {
    said: Arc<Mutex<Option<String>>>,
    asked: Option<Instant>,
}

impl PublicNetwork {
    /// What to say now (as last asked), asking again when due.
    pub fn notice(&mut self) -> Option<String> {
        if self.asked.is_none_or(|at| at.elapsed() >= EVERY) {
            self.asked = Some(Instant::now());
            self.ask();
        }
        self.said.lock().ok().and_then(|said| said.clone())
    }

    #[cfg(windows)]
    fn ask(&self) {
        let said = Arc::clone(&self.said);
        // Networks that cannot be read say nothing: the page only loses a
        // hint, and the next ask tries again.
        let _ = std::thread::Builder::new()
            .name("network-profile".into())
            .spawn(move || {
                let now = connected().ok().and_then(|networks| notice(&networks));
                if let Ok(mut said) = said.lock() {
                    *said = now;
                }
            });
    }

    #[cfg(not(windows))]
    fn ask(&self) {}

    /// Show the notice, if any, as a warning line under `ui`'s cursor.
    pub fn show(&mut self, ui: &mut eframe::egui::Ui) {
        if let Some(words) = self.notice() {
            ui.label(eframe::egui::RichText::new(words).color(crate::theme::warn(ui)));
        }
    }
}

/// The networks this computer is connected to now, from Windows' Network
/// List Manager.
#[cfg(windows)]
fn connected() -> windows::core::Result<Vec<Network>> {
    use windows::Win32::Networking::NetworkListManager::{
        INetwork, INetworkListManager, NLM_ENUM_NETWORK_CONNECTED, NLM_NETWORK_CATEGORY_PUBLIC,
        NetworkListManager,
    };
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    };

    /// COM on this thread for as long as it lives.
    struct Com;
    impl Drop for Com {
        fn drop(&mut self) {
            // SAFETY: paired with the successful CoInitializeEx below.
            unsafe { CoUninitialize() };
        }
    }
    // SAFETY: a fresh thread of its own, set up for COM once.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok()?;
    let _com = Com;
    // SAFETY: COM is set up on this thread; the objects are dropped before
    // it is torn down (`_com` drops last).
    unsafe {
        let list: INetworkListManager = CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL)?;
        let networks = list.GetNetworks(NLM_ENUM_NETWORK_CONNECTED)?;
        let mut out = Vec::new();
        loop {
            let mut one: [Option<INetwork>; 1] = [None];
            let mut fetched = 0u32;
            if networks.Next(&mut one, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            let Some(network) = one[0].take() else { break };
            let name = network
                .GetName()
                .map(|name| name.to_string())
                .unwrap_or_default();
            let public = network.GetCategory()? == NLM_NETWORK_CATEGORY_PUBLIC;
            out.push(Network { name, public });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(name: &str, public: bool) -> Network {
        Network {
            name: name.to_string(),
            public,
        }
    }

    #[test]
    fn nothing_is_said_without_a_public_network() {
        assert_eq!(notice(&[]), None);
        assert_eq!(notice(&[net("Home", false)]), None);
        assert_eq!(notice(&[net("Office", false), net("Home", false)]), None);
    }

    /// The owner's first live test: a home Ethernet marked Public.
    #[test]
    fn a_public_network_is_named_with_how_to_change_it() {
        let said = notice(&[net("Network 2", true)]).unwrap();
        assert_eq!(
            said,
            "Windows treats the network \"Network 2\" as Public, so other devices can't reach \
             Pocketdesk. Set it to Private in Settings → Network & internet → Ethernet or Wi-Fi \
             → the network → Network profile type."
        );
        // Beside a Private one, the Public one is still said.
        assert_eq!(
            notice(&[net("Home", false), net("Network 2", true)]),
            Some(said)
        );
    }

    #[test]
    fn several_or_unnamed_public_networks_are_said_too() {
        let said = notice(&[net("Cafe", true), net("", true)]).unwrap();
        assert!(
            said.starts_with("Windows treats the networks \"Cafe\", an unnamed one as Public"),
            "{said}"
        );
        assert!(said.contains("Set each to Private"), "{said}");
        let said = notice(&[net(" ", true)]).unwrap();
        assert!(
            said.starts_with("Windows treats this network as Public"),
            "{said}"
        );
    }

    /// Live: Windows' Network List Manager answers (counts only, never the
    /// networks' names, which may be the owner's Wi-Fi).
    #[cfg(windows)]
    #[test]
    #[ignore = "live: reads this computer's networks"]
    fn windows_says_the_networks_connected() {
        let networks = connected().unwrap();
        let public = networks.iter().filter(|n| n.public).count();
        println!("connected: {}, public: {public}", networks.len());
        assert!(!networks.is_empty());
    }

    #[test]
    fn a_networks_name_is_shown_clean_and_short() {
        let rlo = char::from_u32(0x202E).unwrap();
        assert_eq!(shown_name(&format!("Home{rlo}\tNet ")), "HomeNet");
        let long = "n".repeat(60);
        assert_eq!(shown_name(&long), format!("{}…", "n".repeat(40)));
    }
}
