//! Settings → Computers (2f): the owner's other computers this one paired
//! with, how each one's link is doing, and pairing with another from its
//! pairing link (its Pair tab's "Copy the link for another computer"). The
//! server does the pairing and keeps the links; this asks it through
//! `computers_out/`.

use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use pocketdesk_core::local::computers::{self, ComputerAsk, ComputersFile, PeerComputer};

/// How often the file is read again while the tab is open.
const REFRESH: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct ComputersView {
    file: ComputersFile,
    read_at: Option<Instant>,
    /// The pairing link being pasted.
    link: String,
    /// The computer whose Unpair waits for its answer.
    forgetting: Option<String>,
    /// What it said happened, until the app takes it (E5).
    notice: Option<crate::notice::Notice>,
    /// The computer whose screen was asked for, until the app takes it (BK7).
    view: Option<(String, String)>,
    /// PC1: the owner ticked that the pasted link's computer, which asks to
    /// pair both ways, is their own; and the link it was ticked for.
    consent: Option<String>,
    /// Whether Windows treats this computer's network as Public, where
    /// the other computer cannot reach it.
    public: crate::network_profile::PublicNetwork,
}

/// A link's state in words.
pub fn link_words(computer: &PeerComputer) -> String {
    match computer.link.as_str() {
        "linked" => "linked".to_string(),
        "connecting" | "" => "connecting…".to_string(),
        "refused" => {
            "refused: it did not admit this computer (removed there, or older); pair again"
                .to_string()
        }
        other => other
            .strip_prefix("away: ")
            .map_or_else(|| other.to_string(), |why| format!("not linked ({why})")),
    }
}

/// Whether "Show its screen" can ask for `computer` now (BK7): the server
/// dials it, so it must run, and the computer must be reachable some way,
/// its link up or an address known. Why not, when it cannot.
pub fn view_reach(computer: &PeerComputer, server_running: bool) -> Result<(), &'static str> {
    if !server_running {
        return Err("Start the server first: it reaches the other computer");
    }
    if computer.link != "linked" && computer.addresses.is_empty() {
        return Err("Not reachable now: its link is down and no address of it is known");
    }
    Ok(())
}

/// Whether `text` looks like a pairing link (the server checks it fully),
/// once cleaned of what a paste carries unseen around it
/// ([`pair_link::clean_link`]: a byte-order mark kept Pair greyed out).
///
/// [`pair_link::clean_link`]: pocketdesk_core::local::pair_link::clean_link
pub fn looks_like_link(text: &str) -> bool {
    pocketdesk_core::local::pair_link::clean_link(text).starts_with("pocketdesk://pair?")
}

impl ComputersView {
    /// What this page said happened since the last call, for the app's one
    /// notice (E5).
    pub fn take_notice(&mut self) -> Option<crate::notice::Notice> {
        self.notice.take()
    }

    /// The computer whose screen "Show its screen" asked for, by id and
    /// name, once (BK7).
    pub fn take_view(&mut self) -> Option<(String, String)> {
        self.view.take()
    }
    fn refresh(&mut self, state_dir: &Path) {
        if self.read_at.is_some_and(|at| at.elapsed() < REFRESH) {
            return;
        }
        self.file = computers::read(state_dir);
        self.read_at = Some(Instant::now());
    }

    fn ask(&mut self, state_dir: &Path, ask: ComputerAsk, now_unix: u64, done: &str) {
        match computers::request(state_dir, ask, now_unix) {
            Ok(_) => {
                self.notice = crate::notice::Notice::done(done);
                // Read again soon, for the outcome.
                self.read_at = None;
            }
            Err(e) => self.notice = crate::notice::Notice::error(format!("Not asked: {e}")),
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        state_dir: &Path,
        server_running: bool,
        now_unix: u64,
    ) {
        self.refresh(state_dir);
        // Under Settings' "Other computers" heading (U5).
        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "Your other computers. On the other one, open Pair and copy its link for \
                 another computer, then paste it here.",
            )
            .weak(),
        );
        // A pairing from the other side times out with no word why here.
        self.public.show(ui);
        // Masked, as it holds the other computer's PIN (PC1 re-review m-1);
        // what it asks for is read below it, before Pair.
        ui.add(
            egui::TextEdit::singleline(&mut self.link)
                .hint_text("pocketdesk://pair?...")
                .password(true)
                .desired_width(420.0),
        );
        // What is sent, as what is checked: without what the paste
        // carried unseen around it.
        let pasted = pocketdesk_core::local::pair_link::clean_link(&self.link).to_string();
        let parsed = pocketdesk_core::local::pair_link::PairLink::from_uri(&pasted).ok();
        // A yes is for the link it was given for, never a later one.
        if self.consent.as_deref().is_some_and(|l| l != pasted) {
            self.consent = None;
        }
        let asks = parsed.as_ref().is_some_and(|l| l.mine);
        // Both ways needs the other's internet endpoint (PC1 review M4).
        let asks_both = asks && parsed.as_ref().is_some_and(names_endpoint);
        if let Some(link) = &parsed {
            let name = link_name(&link.name);
            // Its name is the link's own word; its addresses show whose it
            // is (PC1 re-review m-1, m-2).
            ui.label(format!(
                "A pairing link from {name}, at {}.",
                link.addresses.join(", ")
            ));
            if asks && !asks_both {
                ui.label(
                    RichText::new(format!(
                        "{name}'s link asks to pair both ways, but names no internet endpoint, so it can pair only one way (its internet path may be off)."
                    ))
                    .weak(),
                );
            }
            if asks_both {
                ui.label(
                    RichText::new(format!(
                        "{name}'s link asks to pair both ways: {name} would see and control this computer at any time without asking, with nobody here. Tick this only if {name} is your own computer, its addresses above are its own, and you copied the link yourself."
                    ))
                    .color(crate::theme::warn(ui)),
                );
                let mut yes = self.consent.is_some();
                if ui.checkbox(&mut yes, consent_label(&name)).changed() {
                    self.consent = yes.then(|| pasted.clone());
                }
                if !yes {
                    ui.label(
                        RichText::new(format!(
                            "Unticked, it pairs one way: this computer shows {name} and shares with it, and lets it do nothing more here."
                        ))
                        .weak(),
                    );
                }
            }
        }
        let both = asks_both && self.consent.is_some();
        let ready = looks_like_link(&self.link) && server_running;
        let pair = match (asks, both) {
            (_, true) => "Pair both ways",
            (true, false) => "Pair one way",
            _ => "Pair",
        };
        if ui
            .add_enabled(ready, egui::Button::new(pair))
            .on_disabled_hover_text(if server_running {
                "Paste the other computer's pairing link"
            } else {
                "Start the server first: it does the pairing"
            })
            .clicked()
        {
            self.link.clear();
            self.consent = None;
            self.ask(
                state_dir,
                ComputerAsk::Pair {
                    link: pasted,
                    mine: both,
                },
                now_unix,
                "Asked the other computer to pair.",
            );
        }
        if let Some(last) = &self.file.last_pairing {
            let failed = last.outcome.starts_with("failed");
            let text = format!("Last pairing, with {}: {}", last.name, last.outcome);
            ui.label(if failed {
                RichText::new(text).color(crate::theme::bad(ui))
            } else {
                RichText::new(text).weak()
            });
        }
        if self.file.computers.is_empty() {
            ui.label(RichText::new("No other computer paired yet.").weak());
        }
        let mut forget = None;
        for computer in &self.file.computers {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&computer.name).strong());
                ui.label(RichText::new(link_words(computer)).weak());
                // PC1: paired both ways, with consent given at pairing; the
                // grant here waits for the link (review M1).
                if computer.mine && self.file.lapsed.contains(&computer.device_id) {
                    ui.label(RichText::new(MINE_LAPSED).color(crate::theme::warn(ui)))
                        .on_hover_text(MINE_HOVER);
                } else if computer.mine && self.file.pending.contains(&computer.device_id) {
                    ui.label(RichText::new(MINE_PENDING).color(crate::theme::warn(ui)))
                        .on_hover_text(MINE_HOVER);
                } else if computer.mine {
                    ui.label(RichText::new(MINE).color(crate::theme::good(ui)))
                        .on_hover_text(MINE_HOVER);
                }
                // BK7: its screen in a window here.
                let reach = view_reach(computer, server_running);
                if ui
                    .add_enabled(reach.is_ok(), egui::Button::new("Show its screen"))
                    .on_hover_text(
                        "Its screen in a window here, controlled from it if it lets this computer (its Devices page's Control switch for this one). It may ask its owner first.",
                    )
                    .on_disabled_hover_text(reach.err().unwrap_or_default())
                    .clicked()
                {
                    self.view = Some((computer.device_id.clone(), computer.name.clone()));
                }
                // U5: "Unpair" for a pairing everywhere, asked by name.
                if self.forgetting.as_deref() == Some(computer.device_id.as_str()) {
                    ui.label(format!(
                        "Unpair {}? It must pair again to share with this one.",
                        computer.name
                    ));
                    if ui
                        .add_enabled(server_running, egui::Button::new("Unpair"))
                        .on_disabled_hover_text(STOPPED)
                        .clicked()
                    {
                        forget = Some((computer.device_id.clone(), computer.name.clone()));
                    }
                    if ui.button("Cancel").clicked() {
                        self.forgetting = None;
                    }
                } else if ui
                    // The server does the unpairing (BA3): asked with it
                    // stopped, the request expired unread and the link came
                    // back at its next start, while this said it was done.
                    .add_enabled(server_running, egui::Button::new("Unpair…"))
                    .on_disabled_hover_text(STOPPED)
                    .clicked()
                {
                    self.forgetting = Some(computer.device_id.clone());
                }
            });
        }
        if let Some((device_id, name)) = forget {
            self.forgetting = None;
            // Named, as the app's other notices are (BA3 review).
            self.ask(
                state_dir,
                ComputerAsk::Forget { device_id },
                now_unix,
                &format!(
                    "Asked the server to unpair {name} here: it leaves the list once done. Unpair this computer on {name}'s Devices page too."
                ),
            );
        }
    }
}

/// The box the owner ticks for a link that asks to pair both ways (PC1).
pub fn consent_label(name: &str) -> String {
    format!(
        "{name} is my own computer: let it see and control this computer at any time without asking"
    )
}

/// Whether `link` names an internet endpoint (an `endpointId` in its `ep`),
/// which pairing both ways needs; the server checks it is a real key.
fn names_endpoint(link: &pocketdesk_core::local::pair_link::PairLink) -> bool {
    link.endpoint
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .is_some_and(|ep| ep.get("endpointId").and_then(|id| id.as_str()).is_some())
}

/// A link's computer name fit to show: one printable line, else
/// "a computer".
fn link_name(name: &str) -> String {
    pocketdesk_core::protocol::kdeconnect::clean_device_name(name)
        .unwrap_or_else(|| "A computer".to_string())
}

/// Why Unpair waits (BA3).
const STOPPED: &str = "Start the server first: it does the unpairing";

/// What a computer paired both ways is (PC1), and how to have it ask again.
pub const MINE: &str = "Mine: sees and controls without asking";
/// Paired both ways, but no link came within ten minutes: one way here
/// until it pairs again (PC1 re-review).
pub const MINE_LAPSED: &str =
    "Paired one way here: no link came from it within ten minutes; pair again for both ways";
/// Paired both ways, but its grant here waits for the link (PC1 review M1).
pub const MINE_PENDING: &str =
    "Mine once linked: until the link comes up, it asks first and does not control this computer";
const MINE_HOVER: &str = "Paired both ways from a link made for another of your computers: each lists the other, and either shows and controls the other with no prompt. To have it ask first again, turn on its Ask before each session (and off its Control switches) on Devices. Unpair takes both ways away.";

#[cfg(test)]
mod tests {
    use super::*;

    fn computer(link: &str) -> PeerComputer {
        PeerComputer {
            device_id: "b".to_string(),
            name: "Desk".to_string(),
            addresses: Vec::new(),
            control_port: 1716,
            stream_port: 4716,
            fingerprint: String::new(),
            endpoint: None,
            paired_unix: 0,
            link: link.to_string(),
            link_unix: 0,
            lets_drive: false,
            mine: false,
        }
    }

    #[test]
    fn a_links_state_reads_as_words() {
        assert_eq!(link_words(&computer("linked")), "linked");
        assert_eq!(link_words(&computer("")), "connecting…");
        assert_eq!(
            link_words(&computer("away: iroh dial timed out")),
            "not linked (iroh dial timed out)"
        );
        assert!(link_words(&computer("refused")).starts_with("refused"));
    }

    /// BK7: a computer's screen is asked for through the running server,
    /// for a computer linked or with an address.
    #[test]
    fn its_screen_can_be_asked_for_when_it_can_be_reached() {
        assert!(view_reach(&computer("linked"), true).is_ok());
        assert!(view_reach(&computer("linked"), false).is_err(), "no server");
        assert!(
            view_reach(&computer("away: the link closed"), true).is_err(),
            "no link and no address"
        );
        let at_home = PeerComputer {
            addresses: vec!["192.0.2.10".into()],
            ..computer("away: the link closed")
        };
        assert!(
            view_reach(&at_home, true).is_ok(),
            "tried on the home network"
        );
    }

    /// A link for another of the owner's computers, as its Pair page copies
    /// it with the box ticked.
    fn mine_link() -> String {
        mine_link_with(Some(
            r#"{"endpointId":"made-up","relayUrls":[],"directAddrs":[]}"#.to_string(),
        ))
    }

    /// [`mine_link`] with `endpoint` as its internet endpoint.
    fn mine_link_with(endpoint: Option<String>) -> String {
        pocketdesk_core::local::pair_link::PairLink {
            name: "Test Tower".to_string(),
            device_id: "00000000-0000-4000-8000-0000000000f1".to_string(),
            addresses: vec!["192.0.2.70".to_string(), "192.0.2.71".to_string()],
            control_port: 1716,
            stream_port: 4716,
            pin: "123456".to_string(),
            fingerprint: Some("ab".repeat(32)),
            endpoint,
            mine: true,
        }
        .to_uri()
    }

    /// The Pair button of the page with `link` pasted, after ticking the
    /// consent first when `consent`; what it asked the server.
    fn paired(link: String, consent: bool) -> ComputerAsk {
        use egui_kittest::kittest::Queryable;
        let dir = std::env::temp_dir().join(format!(
            "pd-computers-view-{consent}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let now = pocketdesk_core::local::pin::now_unix();
        let state = dir.clone();
        let mut harness = egui_kittest::Harness::new_ui_state(
            move |ui, view: &mut ComputersView| view.show(ui, &state, true, now),
            ComputersView {
                link,
                ..ComputersView::default()
            },
        );
        harness.run_steps(2);
        // What it asks for is said before anything is pressed, by name and
        // by the addresses the link names (PC1 re-review m-1, m-2): a link
        // wearing one of the owner's computers' names is told by them.
        assert!(
            harness
                .query_by_label_contains("Test Tower's link asks to pair both ways")
                .is_some()
        );
        assert!(
            harness
                .query_by_label_contains("at 192.0.2.70, 192.0.2.71")
                .is_some()
        );
        let pair = if consent {
            harness.get_by_label(&consent_label("Test Tower")).click();
            harness.run_steps(2);
            "Pair both ways"
        } else {
            "Pair one way"
        };
        harness.get_by_label(pair).click();
        harness.run_steps(2);
        let asked = computers::take_all(&dir, now);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(asked.len(), 1, "{asked:?}");
        asked[0].ask.clone()
    }

    /// PC1 review C1: a pasted link that asks to pair both ways says so by
    /// the computer's name and what it would be allowed, and pairs both
    /// ways only once the owner here ticks that it is their own; else it
    /// pairs one way.
    #[test]
    fn a_link_asking_for_both_ways_needs_this_owners_yes() {
        assert_eq!(
            paired(mine_link(), false),
            ComputerAsk::Pair {
                link: mine_link(),
                mine: false
            }
        );
        assert_eq!(
            paired(mine_link(), true),
            ComputerAsk::Pair {
                link: mine_link(),
                mine: true
            }
        );
    }

    /// PC1 re-review m-3: a link that asks for both ways but names no
    /// internet endpoint can pair only one way, and says so before Pair:
    /// no box to tick, and the button says one way.
    #[test]
    fn a_link_without_an_endpoint_says_it_pairs_one_way() {
        use egui_kittest::kittest::Queryable;
        let dir =
            std::env::temp_dir().join(format!("pd-computers-view-no-road-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let now = pocketdesk_core::local::pin::now_unix();
        let state = dir.clone();
        let link = mine_link_with(None);
        let mut harness = egui_kittest::Harness::new_ui_state(
            move |ui, view: &mut ComputersView| view.show(ui, &state, true, now),
            ComputersView {
                link: link.clone(),
                ..ComputersView::default()
            },
        );
        harness.run_steps(2);
        let said = harness
            .query_by_label_contains("names no internet endpoint, so it can pair only one way")
            .is_some();
        let no_box = harness
            .query_by_label(&consent_label("Test Tower"))
            .is_none();
        harness.get_by_label("Pair one way").click();
        harness.run_steps(2);
        let asked = computers::take_all(&dir, now);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(said && no_box);
        assert_eq!(
            asked.iter().map(|r| r.ask.clone()).collect::<Vec<_>>(),
            [ComputerAsk::Pair { link, mine: false }]
        );
    }

    #[test]
    fn only_a_pairing_link_can_be_sent() {
        assert!(looks_like_link("  pocketdesk://pair?v=1&id=b "));
        // The owner's first live test: a paste that began with a
        // byte-order mark, and one with a zero-width space and a line break.
        let bom = char::from_u32(0xFEFF).unwrap();
        let zwsp = char::from_u32(0x200B).unwrap();
        assert!(looks_like_link(&format!("{bom}pocketdesk://pair?v=1&id=b")));
        assert!(looks_like_link(&format!(
            "{zwsp}pocketdesk://pair?v=1&id=b{zwsp}{}",
            char::from_u32(10).unwrap()
        )));
        assert!(!looks_like_link("https://example.org/"));
        assert!(!looks_like_link(""));
    }
}
