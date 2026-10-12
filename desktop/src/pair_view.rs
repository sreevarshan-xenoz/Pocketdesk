//! The Pair tab: a QR code (and the same PIN in digits) a phone scans to
//! find, trust and pair with this desktop.

use std::io;
use std::path::Path;

use eframe::egui::{self, Color32, ColorImage, RichText, TextureHandle, TextureOptions};
use linux_link_core::local::config::Config;
use linux_link_core::local::pair_link::{PairLink, order_addresses};
use linux_link_core::local::{identity, network, pin};

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

/// [`link_for`] with this machine's addresses and Tailscale IP.
pub fn current_link(state_dir: &Path, config: &Config, now_unix: u64) -> io::Result<PairLink> {
    let mut addresses = network::local_ips();
    addresses.extend(network::tailscale_ip());
    link_for(state_dir, config, now_unix, addresses)
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

/// The Pair tab's state between frames.
#[derive(Default)]
pub struct PairView {
    link: Option<PairLink>,
    texture: Option<TextureHandle>,
    error: Option<String>,
    /// Devices paired before this code was shown; a new one is the phone
    /// that just scanned it.
    known: Option<Vec<String>>,
    paired: Option<String>,
}

impl PairView {
    /// Forget the current code (the tab was left); a fresh look starts over.
    pub fn reset(&mut self) {
        *self = Self::default();
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
        if let Some(new) = devices.iter().find(|d| !known.contains(&d.id)) {
            self.paired = Some(new.id.clone());
        }

        // A new code when there is none, or when the PIN behind it expired
        // (or was replaced by `linux-link pair`).
        let live_pin = pin::read(state_dir, now_unix).map(|p| p.pin);
        let stale = self
            .link
            .as_ref()
            .is_none_or(|link| live_pin.as_deref() != Some(link.pin.as_str()));
        if stale && self.error.is_none() {
            let config = config.cloned().unwrap_or_default();
            match current_link(state_dir, &config, now_unix) {
                Ok(link) => match qr_modules(&link.to_uri()) {
                    Ok(modules) => {
                        self.texture = Some(ui.ctx().load_texture(
                            "pair-qr",
                            qr_image(&modules, 6),
                            TextureOptions::NEAREST,
                        ));
                        self.link = Some(link);
                    }
                    Err(e) => self.error = Some(format!("Could not draw the code: {e}")),
                },
                Err(e) => self.error = Some(format!("Could not make a pairing code: {e}")),
            }
        }

        if let Some(id) = &self.paired {
            ui.label(
                RichText::new(format!("Paired ✓  {}", crate::app::short(id)))
                    .color(Color32::from_rgb(0x2e, 0x9e, 0x6a))
                    .strong()
                    .size(18.0),
            );
            ui.label("The phone can connect now. Close this tab or pair another phone.");
            ui.add_space(8.0);
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
                    .and_then(|p| pin::remaining(&p, now_unix))
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use linux_link_core::local::config::Config;
    use linux_link_core::local::paths::CERT_FILE;
    use linux_link_core::local::{identity, pin};

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

    #[test]
    fn the_qr_matrix_is_square_and_encodes_the_uri() {
        let modules = qr_modules("pocketdesk://pair?v=1&a=1.2.3.4&pin=123456").unwrap();
        let side = modules.len();
        assert!(side >= 21 && modules.iter().all(|row| row.len() == side));
    }
}
