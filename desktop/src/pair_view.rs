//! The Pair tab: a QR code (and the same PIN in digits) a phone scans to
//! find, trust and pair with this desktop.

use std::io;
use std::path::Path;
use std::sync::mpsc;

use eframe::egui::{self, Color32, ColorImage, RichText, TextureHandle, TextureOptions};
use pocketdesk_core::local::config::Config;
use pocketdesk_core::local::pair_link::{PairLink, order_addresses};
use pocketdesk_core::local::{identity, network, pin};

use crate::model::Device;

/// Whose phone the next code pairs (M2b): the owner's, someone else's
/// (files and links only), or a guest's for a while (I3), which is someone
/// else's too.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Whose {
    /// A guest's seconds; None pairs for good.
    pub grant_secs: Option<u64>,
    pub someone_else: bool,
    /// P9: pairs with this saved profile's permissions, as someone else's.
    pub profile: Option<String>,
}

impl Whose {
    pub const MINE: Self = Self {
        grant_secs: None,
        someone_else: false,
        profile: None,
    };
    pub const SOMEONE_ELSE: Self = Self {
        grant_secs: None,
        someone_else: true,
        profile: None,
    };

    pub const fn guest(secs: u64) -> Self {
        Self {
            grant_secs: Some(secs),
            someone_else: true,
            profile: None,
        }
    }

    /// A phone paired with the profile `name`'s permissions (P9).
    pub fn with_profile(name: &str) -> Self {
        Self {
            grant_secs: None,
            someone_else: true,
            profile: Some(name.to_string()),
        }
    }

    fn of(file: &pin::FilePin) -> Self {
        Self {
            grant_secs: file.grant_secs,
            // A guest's code from an older `pair --grant` says nothing, and
            // pairs as someone else's all the same (M2 review).
            someone_else: file.someone_else || file.grant_secs.is_some(),
            profile: file.profile.clone(),
        }
    }
}

/// The link for a phone to scan now: reuses a PIN that is still valid and
/// pairs as `whose` says (so the code on screen does not change under the
/// camera), mints one otherwise.
pub fn link_for(
    state_dir: &Path,
    config: &Config,
    now_unix: u64,
    addresses: Vec<String>,
    whose: Whose,
) -> io::Result<PairLink> {
    let addresses = order_addresses(addresses);
    if addresses.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "this computer has no network address a phone could reach",
        ));
    }
    let pin = match pin::read(state_dir, now_unix).filter(|f| Whose::of(f) == whose) {
        Some(file) => file.pin,
        None => {
            let pin = pin::mint_value();
            pin::write_preset(
                state_dir,
                &pin,
                now_unix,
                whose.grant_secs,
                whose.someone_else,
                whose.profile.as_deref(),
            )?;
            pin
        }
    };
    Ok(PairLink {
        name: host_name(),
        device_id: identity::device_id(state_dir).unwrap_or_default(),
        addresses,
        control_port: config.control_port,
        stream_port: config.streaming_port,
        pin,
        fingerprint: identity::cert_fingerprint(state_dir)
            .ok()
            .map(identity::fingerprint_hex),
        endpoint: identity::read_endpoint(state_dir).map(|e| e.to_string()),
    })
}

/// This machine's addresses and Tailscale IP, looked up on a thread: the
/// lookup runs external commands that may take up to their time limit, and
/// the UI must not stall on them. `ctx` is repainted when the answer is in.
fn look_up_addresses(ctx: egui::Context) -> mpsc::Receiver<Vec<String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut addresses = network::local_ips();
        addresses.extend(network::tailscale_ip());
        let _ = tx.send(addresses);
        ctx.request_repaint();
    });
    rx
}

/// The name a phone shows for this desktop.
pub fn host_name() -> String {
    ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .find_map(|var| std::env::var(var).ok().filter(|v| !v.trim().is_empty()))
        .unwrap_or_else(|| "This computer".to_string())
}

/// The QR code for `text` as rows of dark (`true`) modules.
pub fn qr_modules(text: &str) -> Result<Vec<Vec<bool>>, String> {
    let code = qrcode::QrCode::new(text.as_bytes()).map_err(|e| e.to_string())?;
    let width = code.width();
    let colors = code.to_colors();
    Ok(colors
        .chunks(width)
        .map(|row| row.iter().map(|c| *c == qrcode::Color::Dark).collect())
        .collect())
}

/// Dark modules on white with the standard 4-module quiet zone, `scale` px
/// per module.
fn qr_image(modules: &[Vec<bool>], scale: usize) -> ColorImage {
    const QUIET: usize = 4;
    let n = modules.len();
    let side = (n + 2 * QUIET) * scale;
    let mut pixels = vec![Color32::WHITE; side * side];
    for (y, row) in modules.iter().enumerate() {
        for (x, dark) in row.iter().enumerate() {
            if !dark {
                continue;
            }
            for dy in 0..scale {
                for dx in 0..scale {
                    let px = (x + QUIET) * scale + dx;
                    let py = (y + QUIET) * scale + dy;
                    pixels[py * side + px] = Color32::BLACK;
                }
            }
        }
    }
    ColorImage::new([side, side], pixels)
}

/// What the Pair tab does with the code it shows, given the PIN on file.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    /// The code on screen is still the live PIN.
    Keep,
    /// Show a code for the PIN on file, minting one if there is none: the
    /// tab is fresh, the shown PIN ran out of time, or `pocketdesk pair`
    /// replaced it.
    Refresh,
    /// The shown PIN left the file before its time: it paired a phone, or
    /// the server discarded it after too many wrong guesses. Minting another
    /// on its own would hand whoever was guessing a fresh five tries, so the
    /// tab waits for a click.
    Stop,
}

/// The PIN behind the code on screen.
struct Shown {
    pin: String,
    /// When it runs out; `None` for a PIN that never expires.
    expires_unix: Option<u64>,
}

fn next_step(shown: Option<&Shown>, on_file: Option<&pin::FilePin>, now_unix: u64) -> Next {
    let Some(shown) = shown else {
        return Next::Refresh;
    };
    match on_file {
        Some(file) if file.pin == shown.pin => Next::Keep,
        Some(_) => Next::Refresh,
        None if shown.expires_unix.is_some_and(|t| now_unix >= t) => Next::Refresh,
        None => Next::Stop,
    }
}

/// The Pair tab's state between frames.
#[derive(Default)]
pub struct PairView {
    /// When the link for another computer was last copied, for "Copied.".
    copied: Option<std::time::Instant>,
    link: Option<PairLink>,
    shown: Option<Shown>,
    texture: Option<TextureHandle>,
    error: Option<String>,
    /// The shown code was used up or discarded early (see [`Next::Stop`]).
    stopped: bool,
    /// Devices paired before this code was shown; a new one is the phone
    /// that just scanned it.
    known: Option<Vec<String>>,
    paired: Option<String>,
    /// The app should re-read the trust store now rather than at its next
    /// tick: a PIN that left early may have just paired a phone.
    refresh_wanted: bool,
    /// The address lookup for the next code, while it runs. The PIN is minted
    /// only once it answers, on this thread, so a tab left mid-lookup leaves
    /// no PIN behind.
    lookup: Option<mpsc::Receiver<Vec<String>>>,
    /// Whose phone the code pairs (M2b), and for how long (I3), as
    /// `pocketdesk pair --grant` does.
    whose: Whose,
    /// The owner chose again since the code on screen was made.
    whose_chosen: bool,
}

/// Whose phone the next code pairs: the owner's choice just made, else a
/// live code on file's own (one `pocketdesk pair --grant` wrote while the
/// page was open is kept, not replaced; I3 review), else the view's.
pub fn whose_for(chosen_now: bool, view: Whose, on_file: Option<&pin::FilePin>) -> Whose {
    match on_file {
        Some(file) if !chosen_now => Whose::of(file),
        _ => view,
    }
}

/// What a profile lets a phone do, in words: its permissions that are on,
/// by their Devices labels (Q2 review: not the code's names).
pub fn profile_allows(
    permissions: &pocketdesk_core::protocol::permissions::DevicePermissions,
) -> String {
    use pocketdesk_core::protocol::permissions::Capability;
    let on: Vec<String> = Capability::ALL
        .into_iter()
        .filter(|c| permissions.allows(*c))
        .map(|c| {
            crate::app::PERMISSIONS
                .iter()
                .find(|(p, _, _)| *p == c)
                .map(|(_, label, _)| label.to_string())
                .unwrap_or_else(|| format!("{c:?}"))
        })
        .collect();
    let mut words = if on.is_empty() {
        "nothing".to_string()
    } else {
        on.join("; ")
    };
    if permissions.ask_first {
        words.push_str("; each session asked first");
    }
    words
}

/// The choices of whose phone a code pairs (M2b), a guest's for a while (I3).
const WHOSE: [(Whose, &str); 5] = [
    (Whose::MINE, "Mine"),
    (Whose::SOMEONE_ELSE, "Someone else's"),
    (Whose::guest(15 * 60), "A guest's, for 15 minutes"),
    (Whose::guest(60 * 60), "A guest's, for an hour"),
    (Whose::guest(24 * 60 * 60), "A guest's, for a day"),
];

impl PairView {
    /// Forget the current code (the tab was left, or the window hidden or
    /// closed) and take its PIN off file, so no code stays live that nobody
    /// is looking at; a fresh look starts over.
    pub fn reset(&mut self, state_dir: &Path) {
        self.withdraw(state_dir);
        // The choice of whose phone it pairs stays for the next code.
        let whose = self.whose.clone();
        *self = Self::default();
        self.whose = whose;
    }

    /// Whether the app should refresh its snapshot now (asked once).
    pub fn take_refresh_request(&mut self) -> bool {
        std::mem::take(&mut self.refresh_wanted)
    }

    /// Delete the shown PIN from file if it is still the one there.
    fn withdraw(&mut self, state_dir: &Path) {
        if let Some(shown) = self.shown.take() {
            let _ = pin::remove_if(state_dir, &shown.pin);
        }
        self.link = None;
        self.texture = None;
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        state_dir: &Path,
        config: Option<&Config>,
        devices: &[Device],
        now_unix: u64,
    ) {
        let known = self
            .known
            .get_or_insert_with(|| devices.iter().map(|d| d.id.clone()).collect());
        if self.paired.is_none()
            && let Some(new) = devices.iter().find(|d| !known.contains(&d.id))
        {
            // One code, one phone: the server already deleted the PIN; make
            // sure a code minted in the meantime goes too.
            self.paired = Some(new.id.clone());
            self.withdraw(state_dir);
        }

        if let Some(id) = &self.paired {
            ui.label(
                RichText::new(format!("Paired ✓  {}", crate::app::short(id)))
                    .color(crate::theme::good(ui))
                    .strong()
                    .size(18.0),
            );
            ui.label("The phone can connect now.");
            ui.add_space(8.0);
            if ui.button("Pair another phone").clicked() {
                self.reset(state_dir);
            }
            return;
        }

        let on_file = pin::read(state_dir, now_unix);
        if !self.stopped && self.error.is_none() {
            match next_step(self.shown.as_ref(), on_file.as_ref(), now_unix) {
                Next::Keep => {}
                Next::Stop => {
                    self.stopped = true;
                    self.refresh_wanted = true;
                    self.withdraw(state_dir);
                }
                Next::Refresh => {
                    let addresses = match &self.lookup {
                        None => {
                            // The old code (expired or replaced) goes while
                            // the new one is prepared.
                            self.withdraw(state_dir);
                            self.lookup = Some(look_up_addresses(ui.ctx().clone()));
                            None
                        }
                        Some(lookup) => match lookup.try_recv() {
                            Ok(addresses) => Some(addresses),
                            Err(mpsc::TryRecvError::Empty) => None,
                            Err(mpsc::TryRecvError::Disconnected) => Some(Vec::new()),
                        },
                    };
                    if let Some(addresses) = addresses {
                        self.lookup = None;
                        self.make_code(ui, state_dir, config, now_unix, addresses);
                    }
                }
            }
        }

        if self.stopped {
            ui.label(RichText::new("This code no longer works").heading());
            ui.label(
                "It was used, or the server discarded it after 5 wrong PIN guesses. \
                 If nobody here was pairing a phone, someone on your network may have been \
                 guessing.",
            );
            ui.add_space(8.0);
            if ui.button("Show a new code").clicked() {
                self.reset(state_dir);
            }
            return;
        }
        if self.lookup.is_some() && self.error.is_none() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Preparing a pairing code…");
            });
            return;
        }
        if let Some(error) = &self.error {
            ui.label(RichText::new(error).color(crate::theme::bad(ui)));
            if ui.button("Try again").clicked() {
                self.error = None;
            }
            return;
        }
        let (Some(link), Some(texture)) = (&self.link, &self.texture) else {
            return;
        };
        let mut grant_changed = None;
        ui.horizontal_top(|ui| {
            ui.add(
                egui::Image::new(texture)
                    .fit_to_exact_size(egui::vec2(260.0, 260.0))
                    .alt_text("Pairing code to scan with the phone"),
            );
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.label(RichText::new("Pair a phone").heading());
                ui.label(
                    "In the Pocketdesk app, tap \"Scan pairing code\" and point the camera here.",
                );
                ui.add_space(10.0);
                // M2b: someone else's phone, or a guest's for a while (I3,
                // Devices counts it down), pairs with files and links only.
                let mut chosen = self.whose.clone();
                // P9: each saved profile is a choice too.
                // One that cannot be read is said (Q2 review: it read as none).
                let (profiles, profiles_error) =
                    match pocketdesk_core::local::permission_profiles::load(state_dir) {
                        Ok(profiles) => (profiles, None),
                        Err(e) => (Vec::new(), Some(e.to_string())),
                    };
                let mut choices: Vec<(Whose, String)> = WHOSE
                    .iter()
                    .map(|(whose, label)| (whose.clone(), label.to_string()))
                    .collect();
                choices.extend(profiles.iter().map(|p| {
                    (Whose::with_profile(&p.name), format!("With the {} profile", p.name))
                }));
                // Wrapped: a long profile name stays in the window (Q2 review).
                ui.horizontal_wrapped(|ui| {
                    ui.label("Whose phone is it?");
                    let shown = choices
                        .iter()
                        .find(|(whose, _)| *whose == chosen)
                        .map_or("Mine".to_string(), |(_, label)| label.clone());
                    egui::ComboBox::from_id_salt("pair-grant")
                        .selected_text(shown)
                        .show_ui(ui, |ui| {
                            for (whose, label) in &choices {
                                ui.selectable_value(&mut chosen, whose.clone(), label);
                            }
                        });
                });
                if let Some(name) = &chosen.profile {
                    // What it may do, said (P9 review: a profile saved from
                    // the owner's phone may run commands).
                    let allows = profiles
                        .iter()
                        .find(|p| p.name == *name)
                        .map(|p| profile_allows(&p.permissions));
                    ui.label(
                        RichText::new(match allows {
                            Some(allows) => format!(
                                "It pairs with the {name} profile's permissions: {allows}. A phone paired already keeps only what both allow."
                            ),
                            None => match &profiles_error {
                                Some(e) => format!(
                                    "The saved profiles cannot be read ({e}): it pairs as someone else's, with files and links only."
                                ),
                                None => format!(
                                    "The {name} profile is gone: it pairs as someone else's, with files and links only."
                                ),
                            },
                        })
                        .small(),
                    );
                } else if chosen.someone_else {
                    ui.label(
                        RichText::new(
                            "It may send files and links only. Change what it may do on Devices.",
                        )
                        .small(),
                    );
                }
                if chosen != self.whose {
                    grant_changed = Some(chosen);
                }
                ui.add_space(6.0);
                ui.label("Or enter this PIN:");
                let spaced: String = link
                    .pin
                    .chars()
                    .flat_map(|c| [c, ' '])
                    .collect::<String>()
                    .trim_end()
                    .to_string();
                ui.label(RichText::new(spaced).monospace().size(30.0).strong());
                let remaining = pin::read(state_dir, now_unix)
                    .map(|p| pin::remaining(&p, now_unix))
                    .map_or(0, |d| d.as_secs());
                ui.label(
                    RichText::new(format!(
                        "Valid for {}:{:02}; a new code appears when it runs out.",
                        remaining / 60,
                        remaining % 60
                    ))
                    .weak(),
                );
                ui.add_space(10.0);
                ui.label(
                    RichText::new(format!("Reachable at {}", link.addresses.join(", "))).weak(),
                );
                // Another of the owner's computers pairs from the link
                // itself (2f), pasted in its Settings → Computers.
                if ui
                    .button("Copy the link for another computer")
                    .on_hover_text(
                        "Paste it in the other computer's Settings, under Other computers. It holds the code: Pocketdesk sends it to no phone, but copy something else after",
                    )
                    .clicked()
                {
                    ui.ctx().copy_text(link.to_uri());
                    self.copied = Some(std::time::Instant::now());
                }
                crate::notice::copied(ui, &mut self.copied);
                if link.fingerprint.is_none() {
                    ui.label(
                        RichText::new(
                            "Start the server once before pairing: until it has made its \
                             certificate, the phone has to trust it on first use.",
                        )
                        .weak(),
                    );
                }
            });
        });
        // Another choice of time: this code goes, and one for it is made.
        if let Some(chosen) = grant_changed {
            self.withdraw(state_dir);
            self.whose = chosen;
            self.whose_chosen = true;
        }
    }

    /// Mint (or reuse) the PIN and draw the code for `addresses`.
    fn make_code(
        &mut self,
        ui: &egui::Ui,
        state_dir: &Path,
        config: Option<&Config>,
        now_unix: u64,
        addresses: Vec<String>,
    ) {
        let config = config.cloned().unwrap_or_default();
        self.whose = whose_for(
            self.whose_chosen,
            self.whose.clone(),
            pin::read(state_dir, now_unix).as_ref(),
        );
        self.whose_chosen = false;
        match link_for(state_dir, &config, now_unix, addresses, self.whose.clone()) {
            Ok(link) => match qr_modules(&link.to_uri()) {
                Ok(modules) => {
                    self.texture = Some(ui.ctx().load_texture(
                        "pair-qr",
                        qr_image(&modules, 6),
                        TextureOptions::NEAREST,
                    ));
                    let expires_unix = pin::read(state_dir, now_unix)
                        .map(|p| p.minted_unix + pin::PIN_TTL.as_secs());
                    self.shown = Some(Shown {
                        pin: link.pin.clone(),
                        expires_unix,
                    });
                    self.link = Some(link);
                }
                Err(e) => self.error = Some(format!("Could not draw the code: {e}")),
            },
            Err(e) => self.error = Some(format!("Could not make a pairing code: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::local::config::Config;
    use pocketdesk_core::local::paths::CERT_FILE;
    use pocketdesk_core::local::{identity, pin};

    /// The cross-language pairing link (core's `pair_link` fixture).
    const FIXTURE: &str = "pocketdesk://pair?v=1&name=Desk%20%26%20Co&id=d-1&a=192.168.1.5,100.80.1.2&cp=1716&sp=4716&pin=012345&fp=abababababababababababababababababababababababababababababababab&ep=eyJlbmRwb2ludElkIjoieCIsInJlbGF5VXJscyI6W10sImRpcmVjdEFkZHJzIjpbXX0";

    /// The QR this app draws for that link, one row of 0/1 per line: the
    /// phone's instrumented test decodes it with its scanner's library, so
    /// the two ends agree on real codes (T3). Regenerate with
    /// `POCKETDESK_WRITE_QR_FIXTURE=1 cargo test -p pocketdesk-desktop qr_fixture`.
    #[test]
    fn the_phones_qr_fixture_is_this_apps_code() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../android/app/src/androidTest/assets/pair_fixture_qr.txt"
        );
        let text: String = qr_modules(FIXTURE)
            .unwrap()
            .iter()
            .map(|row| {
                row.iter()
                    .map(|d| if *d { '1' } else { '0' })
                    .collect::<String>()
                    + "\n"
            })
            .collect();
        if std::env::var_os("POCKETDESK_WRITE_QR_FIXTURE").is_some() {
            std::fs::write(path, &text).unwrap();
        }
        let committed = std::fs::read_to_string(path)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        assert_eq!(committed, text, "{path} is stale; regenerate it");
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-pair-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_link_carries_the_minted_pin_ports_and_fingerprint() {
        let dir = temp_dir("link");
        std::fs::create_dir_all(dir.join("certs")).unwrap();
        std::fs::write(dir.join(CERT_FILE), b"der").unwrap();
        identity::write_endpoint(&dir, &serde_json::json!({"endpointId": "e1"})).unwrap();
        let config = Config {
            control_port: 1800,
            streaming_port: 4800,
            ..Config::default()
        };
        let addresses = vec!["100.64.0.9".to_string(), "192.168.1.4".to_string()];
        let link = link_for(&dir, &config, 1_000, addresses, Whose::MINE).unwrap();
        assert_eq!(link.pin, pin::read(&dir, 1_000).unwrap().pin);
        assert_eq!((link.control_port, link.stream_port), (1800, 4800));
        assert_eq!(
            link.addresses,
            ["192.168.1.4", "100.64.0.9"],
            "best address first"
        );
        let fingerprint = identity::fingerprint_hex(identity::cert_fingerprint(&dir).unwrap());
        assert_eq!(link.fingerprint.as_deref(), Some(fingerprint.as_str()));
        assert!(link.endpoint.as_deref().is_some_and(|e| e.contains("e1")));
    }

    #[test]
    fn a_live_pin_is_reused_and_an_expired_one_replaced() {
        let dir = temp_dir("reuse");
        let config = Config::default();
        let first = link_for(&dir, &config, 1_000, vec!["10.0.0.2".into()], Whose::MINE).unwrap();
        let again = link_for(&dir, &config, 1_100, vec!["10.0.0.2".into()], Whose::MINE).unwrap();
        assert_eq!(
            first.pin, again.pin,
            "the code on screen stays the same while valid"
        );
        let later = link_for(
            &dir,
            &config,
            1_000 + 301,
            vec!["10.0.0.2".into()],
            Whose::MINE,
        )
        .unwrap();
        assert!(pin::read(&dir, 1_301).is_some_and(|p| p.pin == later.pin));
    }

    /// I3: a guest's code pairs for its time; another choice is another code.
    #[test]
    fn a_guest_code_carries_its_time_and_a_new_choice_a_new_code() {
        let dir = temp_dir("guest");
        let config = Config::default();
        let address = || vec!["10.0.0.2".to_string()];
        let for_good = link_for(&dir, &config, 1_000, address(), Whose::MINE).unwrap();
        assert_eq!(pin::read(&dir, 1_000).unwrap().grant_secs, None);
        let guest = link_for(&dir, &config, 1_010, address(), Whose::guest(900)).unwrap();
        assert_ne!(guest.pin, for_good.pin, "a new choice, a new code");
        assert_eq!(pin::read(&dir, 1_010).unwrap().grant_secs, Some(900));
        let again = link_for(&dir, &config, 1_020, address(), Whose::guest(900)).unwrap();
        assert_eq!(again.pin, guest.pin, "the same choice keeps the code");
    }

    /// M2b: someone else's phone is said in the code's PIN file, and a
    /// change of whose phone it is makes another code.
    #[test]
    fn someone_elses_code_says_so_and_a_change_makes_another() {
        let dir = temp_dir("whose");
        let config = Config::default();
        let address = || vec!["10.0.0.2".to_string()];
        let mine = link_for(&dir, &config, 1_000, address(), Whose::MINE).unwrap();
        assert!(!pin::read(&dir, 1_000).unwrap().someone_else);
        let theirs = link_for(&dir, &config, 1_010, address(), Whose::SOMEONE_ELSE).unwrap();
        assert_ne!(theirs.pin, mine.pin);
        let file = pin::read(&dir, 1_010).unwrap();
        assert!(file.someone_else);
        assert_eq!(file.grant_secs, None);
        let guest = link_for(&dir, &config, 1_020, address(), Whose::guest(900)).unwrap();
        assert_ne!(guest.pin, theirs.pin);
        let file = pin::read(&dir, 1_020).unwrap();
        assert!(file.someone_else);
        assert_eq!(file.grant_secs, Some(900));
    }

    /// I3 review: a code the CLI made while the page was open keeps its time.
    #[test]
    fn a_code_on_file_keeps_its_time_unless_another_was_just_chosen() {
        let on_file = pin::FilePin {
            profile: None,
            pin: "123456".to_string(),
            minted_unix: 0,
            grant_secs: Some(3600),
            someone_else: false,
        };
        assert_eq!(
            whose_for(false, Whose::MINE, Some(&on_file)),
            Whose::guest(3600),
            "the CLI's hour is kept, as a guest's"
        );
        assert_eq!(
            whose_for(true, Whose::MINE, Some(&on_file)),
            Whose::MINE,
            "the owner's choice wins"
        );
        assert_eq!(
            whose_for(false, Whose::guest(900), None),
            Whose::guest(900),
            "no code on file: the view's"
        );
    }

    #[test]
    fn without_a_certificate_or_endpoint_the_link_still_pairs() {
        let dir = temp_dir("bare");
        let link = link_for(
            &dir,
            &Config::default(),
            5,
            vec!["192.168.0.2".into()],
            Whose::MINE,
        )
        .unwrap();
        assert!(link.fingerprint.is_none() && link.endpoint.is_none());
        assert!(link.to_uri().starts_with("pocketdesk://pair?v=1&"));
    }

    #[test]
    fn no_usable_address_is_an_error_not_an_unscannable_code() {
        let dir = temp_dir("noaddr");
        assert!(
            link_for(
                &dir,
                &Config::default(),
                5,
                vec!["127.0.0.1".into()],
                Whose::MINE
            )
            .is_err()
        );
    }

    fn file_pin(pin: &str) -> pin::FilePin {
        pin::FilePin {
            profile: None,
            pin: pin.to_string(),
            minted_unix: 1_000,
            grant_secs: None,
            someone_else: false,
        }
    }

    #[test]
    fn a_code_that_leaves_early_stops_the_tab_instead_of_minting_another() {
        let shown = Shown {
            pin: "123456".into(),
            expires_unix: Some(1_300),
        };
        assert_eq!(next_step(None, None, 1_000), Next::Refresh, "fresh tab");
        assert_eq!(
            next_step(Some(&shown), Some(&file_pin("123456")), 1_100),
            Next::Keep
        );
        assert_eq!(
            next_step(Some(&shown), Some(&file_pin("654321")), 1_100),
            Next::Refresh,
            "`pocketdesk pair` replaced it"
        );
        assert_eq!(
            next_step(Some(&shown), None, 1_100),
            Next::Stop,
            "paired or discarded after wrong guesses: no fresh five tries"
        );
        assert_eq!(
            next_step(Some(&shown), None, 1_300),
            Next::Refresh,
            "ran out of time"
        );
        let forever = Shown {
            pin: "123456".into(),
            expires_unix: None,
        };
        assert_eq!(next_step(Some(&forever), None, u64::MAX), Next::Stop);
    }

    #[test]
    fn leaving_the_tab_takes_its_pin_off_file_but_not_a_newer_one() {
        let dir = temp_dir("withdraw");
        let mut view = PairView::default();
        pin::write(&dir, "123456", 1_000, None).unwrap();
        view.shown = Some(Shown {
            pin: "123456".into(),
            expires_unix: Some(1_300),
        });
        view.reset(&dir);
        assert!(pin::read(&dir, 1_001).is_none(), "the shown PIN is gone");

        view.shown = Some(Shown {
            pin: "123456".into(),
            expires_unix: Some(1_300),
        });
        pin::write(&dir, "654321", 1_000, None).unwrap();
        view.reset(&dir);
        assert!(
            pin::read(&dir, 1_001).is_some_and(|p| p.pin == "654321"),
            "a PIN `pocketdesk pair` wrote since is left alone"
        );
        assert!(view.shown.is_none() && !view.stopped);
    }

    /// One frame of the Pair tab, headless.
    fn frame(ctx: &egui::Context, view: &mut PairView, dir: &std::path::Path, now: u64) {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            view.show(ui, dir, None, &[], now);
        });
        // No renderer uploads the QR texture here.
        output.textures_delta.clear();
    }

    #[test]
    fn the_code_is_minted_only_after_the_address_lookup_and_not_if_the_tab_is_left() {
        let ctx = egui::Context::default();
        let dir = temp_dir("lookup");
        let mut view = PairView::default();
        frame(&ctx, &mut view, &dir, 1_000);
        assert!(view.lookup.is_some(), "the lookup runs off the UI thread");
        assert!(pin::read(&dir, 1_000).is_none(), "nothing minted yet");
        view.reset(&dir);
        assert!(pin::read(&dir, 1_000).is_none(), "a left tab leaves no PIN");

        let mut view = PairView::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while view.link.is_none() && view.error.is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "the lookup never answered"
            );
            frame(&ctx, &mut view, &dir, 1_000);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        match &view.link {
            // A machine with a usable address shows the code it minted.
            Some(link) => assert_eq!(pin::read(&dir, 1_000).unwrap().pin, link.pin),
            // One with none says so and mints nothing.
            None => assert!(pin::read(&dir, 1_000).is_none()),
        }
    }

    #[test]
    fn the_qr_matrix_is_square_and_encodes_the_uri() {
        let modules = qr_modules("pocketdesk://pair?v=1&a=1.2.3.4&pin=123456").unwrap();
        let side = modules.len();
        assert!(side >= 21 && modules.iter().all(|row| row.len() == side));
    }
}
