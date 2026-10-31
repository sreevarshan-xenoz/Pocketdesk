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
//!
//! On Linux the same line says when this computer's firewall keeps other
//! devices out (Arch's stock nftables rules let in ssh only): Diagnostics'
//! firewall reading, asked off the UI thread every [`EVERY`], its words
//! from `diagnostics::firewall_pairing_notice`. Elsewhere nothing is asked
//! and nothing is said, so there the Windows words are built only by the
//! tests (PR #552 review C1: dead code there).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the networks (Windows) or the firewall (Linux, which starts a
/// few programs to read it) are asked again while a page that says it
/// shows.
pub const EVERY: Duration = if cfg!(target_os = "linux") {
    Duration::from_secs(30)
} else {
    Duration::from_secs(10)
};

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
    /// An ask is under way: no second one starts beside it, should
    /// Windows' Network List Manager hang (PR #552 review M2).
    #[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
    busy: Arc<AtomicBool>,
}

impl PublicNetwork {
    /// What to say now (as last asked), asking again when due; the page
    /// is drawn again when the answer changes it (`ctx`).
    pub fn notice(&mut self, ctx: &eframe::egui::Context) -> Option<String> {
        if self.asked.is_none_or(|at| at.elapsed() >= EVERY) {
            self.asked = Some(Instant::now());
            self.ask(ctx);
        }
        self.said.lock().ok().and_then(|said| said.clone())
    }

    /// Start an ask, unless one is under way.
    #[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
    fn begin(&self) -> bool {
        !self.busy.swap(true, Ordering::SeqCst)
    }

    /// The ask answered `now`: kept, and whether it changed what is said.
    #[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
    fn answered(said: &Mutex<Option<String>>, busy: &AtomicBool, now: Option<String>) -> bool {
        let changed = match said.lock() {
            Ok(mut said) if *said != now => {
                *said = now;
                true
            }
            _ => false,
        };
        busy.store(false, Ordering::SeqCst);
        changed
    }

    /// Not under test: the UI tests' pages never depend on the network of
    /// the computer they run on (a CI runner's is Public, and its line
    /// appearing mid-test moved the Pair page under a click).
    #[cfg(all(any(windows, target_os = "linux"), not(test)))]
    fn ask(&self, ctx: &eframe::egui::Context) {
        if !self.begin() {
            return;
        }
        let (said, busy, ctx) = (Arc::clone(&self.said), Arc::clone(&self.busy), ctx.clone());
        let started = std::thread::Builder::new()
            .name("network-profile".into())
            .spawn(move || {
                let now = current();
                if Self::answered(&said, &busy, now) {
                    ctx.request_repaint();
                }
            });
        // Not started: the next ask may try.
        if started.is_err() {
            self.busy.store(false, Ordering::SeqCst);
        }
    }

    #[cfg(any(not(any(windows, target_os = "linux")), test))]
    fn ask(&self, _ctx: &eframe::egui::Context) {}

    /// Show the notice, if any, as a warning line under `ui`'s cursor.
    pub fn show(&mut self, ui: &mut eframe::egui::Ui) {
        if let Some(words) = self.notice(&ui.ctx().clone()) {
            ui.label(eframe::egui::RichText::new(words).color(crate::theme::warn(ui)));
        }
    }
}

/// What to say now: a Public network's name.
#[cfg(all(windows, not(test)))]
fn current() -> Option<String> {
    connected().ok().and_then(|networks| notice(&networks))
}

/// What to say now: the firewall keeping other devices from the ports the
/// server listens on.
#[cfg(all(target_os = "linux", not(test)))]
fn current() -> Option<String> {
    use pocketdesk_core::local::{config::Config, diagnostics, listening, paths};
    let config = Config::load().unwrap_or_default();
    let control_port = paths::state_dir()
        .map(|dir| listening::control_port(&dir, config.control_port))
        .unwrap_or(config.control_port);
    let firewall = crate::diagnostics_view::linux_firewall(control_port, config.streaming_port);
    diagnostics::firewall_pairing_notice(&firewall, control_port, config.streaming_port)
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
            // One network that cannot say its category is passed over, not
            // the whole list (PR #552 review M1).
            let Ok(category) = network.GetCategory() else {
                continue;
            };
            let public = category == NLM_NETWORK_CATEGORY_PUBLIC;
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

    /// PR #552 review M2: one ask at a time, and the page drawn again only
    /// when the answer changes what it says.
    #[test]
    fn one_ask_at_a_time_and_a_change_redraws() {
        let public = PublicNetwork::default();
        assert!(public.begin());
        assert!(!public.begin(), "under way");
        let words = Some("Public".to_string());
        assert!(PublicNetwork::answered(
            &public.said,
            &public.busy,
            words.clone()
        ));
        assert!(public.begin(), "answered: the next may start");
        assert!(!PublicNetwork::answered(&public.said, &public.busy, words));
        assert!(PublicNetwork::answered(&public.said, &public.busy, None));
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
