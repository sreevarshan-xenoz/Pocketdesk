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

/// The link for a phone to scan now: reuses a PIN that is still valid (so
/// the code on screen does not change under the camera), mints one otherwise.
pub fn link_for(
    state_dir: &Path,
    config: &Config,
    now_unix: u64,
    addresses: Vec<String>,
) -> io::Result<PairLink> {
    let addresses = order_addresses(addresses);
    if addresses.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "this computer has no network address a phone could reach",
        ));
    }
    let pin = match pin::read(state_dir, now_unix) {
        Some(file) => file.pin,
        None => {
            let pin = pin::mint_value();
            pin::write(state_dir, &pin, now_unix, None)?;
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
}

impl PairView {
    /// Forget the current code (the tab was left, or the window hidden or
    /// closed) and take its PIN off file, so no code stays live that nobody
    /// is looking at; a fresh look starts over.
    pub fn reset(&mut self, state_dir: &Path) {
        self.withdraw(state_dir);
        *self = Self::default();
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
                    .color(Color32::from_rgb(0x2e, 0x9e, 0x6a))
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
            ui.label(RichText::new(error).color(Color32::from_rgb(0xd9, 0x5c, 0x4a)));
            if ui.button("Try again").clicked() {
                self.error = None;
            }
            return;
        }
        let (Some(link), Some(texture)) = (&self.link, &self.texture) else {
            return;
        };
        ui.horizontal_top(|ui| {
            ui.add(egui::Image::new(texture).fit_to_exact_size(egui::vec2(260.0, 260.0)));
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.label(RichText::new("Pair a phone").heading());
                ui.label(
                    "In the Pocketdesk app, tap \"Scan pairing code\" and point the camera here.",
                );
                ui.add_space(10.0);
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
        match link_for(state_dir, &config, now_unix, addresses) {
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
    use pocketdesk_core::local::paths::CERT_FILE;
    use pocketdesk_core::local::{identity, pin};

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
        let link = link_for(&dir, &config, 1_000, addresses).unwrap();
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
        let first = link_for(&dir, &config, 1_000, vec!["10.0.0.2".into()]).unwrap();
        let again = link_for(&dir, &config, 1_100, vec!["10.0.0.2".into()]).unwrap();
        assert_eq!(
            first.pin, again.pin,
            "the code on screen stays the same while valid"
        );
        let later = link_for(&dir, &config, 1_000 + 301, vec!["10.0.0.2".into()]).unwrap();
        assert!(pin::read(&dir, 1_301).is_some_and(|p| p.pin == later.pin));
    }

    #[test]
    fn without_a_certificate_or_endpoint_the_link_still_pairs() {
        let dir = temp_dir("bare");
        let link = link_for(&dir, &Config::default(), 5, vec!["192.168.0.2".into()]).unwrap();
        assert!(link.fingerprint.is_none() && link.endpoint.is_none());
        assert!(link.to_uri().starts_with("pocketdesk://pair?v=1&"));
    }

    #[test]
    fn no_usable_address_is_an_error_not_an_unscannable_code() {
        let dir = temp_dir("noaddr");
        assert!(link_for(&dir, &Config::default(), 5, vec!["127.0.0.1".into()]).is_err());
    }

    fn file_pin(pin: &str) -> pin::FilePin {
        pin::FilePin {
            pin: pin.to_string(),
            minted_unix: 1_000,
            grant_secs: None,
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
