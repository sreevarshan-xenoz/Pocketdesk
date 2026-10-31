//! Settings → Phones → "Approve with my phone" (Linux): sudo, polkit and the
//! lock screen approved on the phone, through `pocketdesk-pam-ask` in PAM.
//! Pocketdesk never edits `/etc/pam.d`: this shows the lines to add, the
//! command that pins the phones' keys as root, and a Test button that makes
//! a dry ask (no PAM involved).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use pocketdesk_core::local::approve::PhoneKey;
use pocketdesk_core::protocol::approve::{HELPER_PATH, PAM_SERVICES, PIN_DIR, pam_line};

/// What the section keeps between frames.
#[derive(Default)]
pub struct ApproveUi {
    /// The keys the phones gave, read again every 2 s.
    keys: Option<(Instant, Vec<PhoneKey>)>,
    /// A Test running: its output when done.
    test: Option<std::thread::JoinHandle<String>>,
    /// The last Test's output.
    said: Option<String>,
}

/// The helper to test with: the installed one, else one beside this app
/// (a build from source).
fn helper() -> Option<PathBuf> {
    let installed = Path::new(HELPER_PATH);
    if installed.is_file() {
        return Some(installed.to_path_buf());
    }
    let beside = std::env::current_exe()
        .ok()?
        .parent()?
        .join("pocketdesk-pam-ask");
    beside.is_file().then_some(beside)
}

fn user() -> String {
    std::env::var("USER").unwrap_or_else(|_| "you".to_string())
}

impl ApproveUi {
    fn keys(&mut self) -> &[PhoneKey] {
        let stale = self
            .keys
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(2));
        if stale {
            let keys = pocketdesk_core::local::paths::state_dir()
                .map(|dir| pocketdesk_core::local::approve::keys(&dir))
                .unwrap_or_default();
            self.keys = Some((Instant::now(), keys));
        }
        self.keys.as_ref().map(|(_, k)| k.as_slice()).unwrap_or(&[])
    }

    /// The set-up below "Approve with my phone": which phones, the pin, the
    /// PAM lines and the Test.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        chosen: &mut Vec<String>,
        phones: &[(String, String)],
    ) {
        if self.test.as_ref().is_some_and(|t| t.is_finished())
            && let Some(Ok(out)) = self.test.take().map(|t| t.join())
        {
            self.said = Some(out);
        }
        let keys = self.keys().to_vec();
        ui.add_space(4.0);
        ui.label(RichText::new("1. On the phone").strong());
        crate::settings_view::note(
            ui,
            "Turn on \"Answer my questions\" for the phone on the Devices page here, then \"Approve my computer's requests\" in Pocketdesk's Settings on the phone. It makes a key that signs only after your fingerprint (or screen lock), and gives this computer its public key.",
        );
        if keys.is_empty() {
            ui.label(RichText::new("No phone has given its key yet.").color(crate::theme::bad(ui)));
        }
        for key in &keys {
            let name = phones
                .iter()
                .find(|(id, _)| *id == key.device_id)
                .map_or(key.device_id.as_str(), |(_, n)| n.as_str());
            let mut on = chosen.is_empty() || chosen.contains(&key.device_id);
            let label = format!(
                "{name}: key {}{}",
                key.fingerprint,
                if key.biometric_only {
                    ", fingerprint only"
                } else {
                    ""
                }
            );
            if ui.checkbox(&mut on, label).changed() {
                // Empty is every phone: a first untick names the others.
                if chosen.is_empty() {
                    *chosen = keys.iter().map(|k| k.device_id.clone()).collect();
                }
                chosen.retain(|d| *d != key.device_id);
                if on {
                    chosen.push(key.device_id.clone());
                }
                if keys.iter().all(|k| chosen.contains(&k.device_id)) {
                    chosen.clear();
                }
            }
        }

        ui.add_space(4.0);
        ui.label(RichText::new("2. Pin the keys as root").strong());
        let pin = format!("sudo {HELPER_PATH} --pin {}", user());
        copy_line(ui, &pin);
        crate::settings_view::note(
            ui,
            "It copies the phones' keys and this computer's certificate hash into /etc/pocketdesk/pam, where only root can change them, and prints each key: check it matches the phone's Settings. Pin again after the phone makes a new key; forget them with --unpin.",
        );
        let pinned = Path::new(PIN_DIR)
            .join(format!("{}.json", user()))
            .is_file();
        if pinned {
            ui.label("Pinned for you.");
        }
        if !Path::new(HELPER_PATH).is_file() {
            ui.label(
                RichText::new(format!(
                    "{HELPER_PATH} is not installed: the AUR and .deb packages install it; from source, sudo install -Dm755 target/release/pocketdesk-pam-ask {HELPER_PATH}. It must belong to root."
                ))
                .color(crate::theme::bad(ui)),
            );
        }

        ui.add_space(4.0);
        ui.label(RichText::new("3. Add the line to PAM yourself").strong());
        crate::settings_view::note(
            ui,
            "As root, put this line first among the auth lines of each service you want (Pocketdesk never edits /etc/pam.d). It is \"sufficient\": when the phone does not approve, the password prompt comes as before. Keep a root shell open while you try it.",
        );
        copy_line(ui, &pam_line());
        let services: Vec<String> = PAM_SERVICES
            .iter()
            .map(|(s, k)| format!("/etc/pam.d/{s} ({})", k.word()))
            .collect();
        crate::settings_view::note(
            ui,
            format!(
                "Services: {}. The lock-screen ones only with \"The lock screen too\" on.",
                services.join(", ")
            ),
        );

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let running = self.test.is_some();
            let helper = helper();
            let button = ui.add_enabled(
                !running && helper.is_some(),
                egui::Button::new(if running { "Asking your phone…" } else { "Test" }),
            );
            if button.clicked()
                && let Some(helper) = helper
            {
                self.said = None;
                self.test = Some(std::thread::spawn(move || {
                    match std::process::Command::new(helper).arg("--test").output() {
                        Ok(out) => {
                            let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                            if text.is_empty() {
                                String::from_utf8_lossy(&out.stderr).trim().to_string()
                            } else {
                                text
                            }
                        }
                        Err(e) => format!("The test did not start: {e}"),
                    }
                }));
            }
            crate::settings_view::note(
                ui,
                "A dry ask to your phone, as you: nothing runs and PAM is not involved. Save first.",
            );
        });
        if self.test.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
        if let Some(said) = &self.said {
            ui.label(said);
        }
    }
}

fn copy_line(ui: &mut egui::Ui, line: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(line).monospace());
        if ui.small_button("Copy").clicked() {
            ui.ctx().copy_text(line.to_string());
        }
    });
}
