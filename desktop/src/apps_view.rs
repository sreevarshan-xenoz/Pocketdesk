//! "Open an app on the phone…" (P2): the apps a phone listed, found by a
//! search, each with Open. The list is asked of the phone as the box opens
//! and read from `phone_apps.json` once a second while it shows.

use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui;
use pocketdesk_core::protocol::phone_action::PhoneApp;

/// How often the box reads the phone's list while it shows.
const READ_EVERY: Duration = Duration::from_secs(1);

/// The most apps drawn at once: a search narrows the rest.
const MAX_SHOWN: usize = 60;

/// The box, for one phone.
pub struct AppsBox {
    pub device_id: String,
    query: String,
    apps: Vec<PhoneApp>,
    /// When the list read was made by the phone (Unix seconds), if any.
    listed_at: Option<u64>,
    /// When the box asked the phone for its list (Unix seconds).
    asked_at: u64,
    read_at: Option<Instant>,
    focus: bool,
}

/// What the box asks of the app this frame.
pub enum Action {
    Open(String),
    Close,
}

impl AppsBox {
    /// A box for `device_id`, whose list was asked at `asked_at`.
    pub fn new(device_id: String, asked_at: u64) -> Self {
        Self {
            device_id,
            query: String::new(),
            apps: Vec::new(),
            listed_at: None,
            asked_at,
            read_at: None,
            focus: true,
        }
    }

    fn refresh(&mut self, state_dir: &Path) {
        if self.read_at.is_some_and(|at| at.elapsed() < READ_EVERY) {
            return;
        }
        self.read_at = Some(Instant::now());
        if let Some(list) =
            pocketdesk_core::local::phone_apps::read(state_dir).remove(&self.device_id)
        {
            self.apps = list.apps;
            self.listed_at = Some(list.at);
        }
    }

    /// Draw the box for the phone called `who`.
    pub fn ui(&mut self, ui: &mut egui::Ui, state_dir: &Path, who: &str) -> Option<Action> {
        self.refresh(state_dir);
        let mut action = None;
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("Open an app on {who}")).strong());
                if ui.button("Close").clicked() {
                    action = Some(Action::Close);
                }
            });
            let fresh = self.listed_at.is_some_and(|at| at + 2 >= self.asked_at);
            if self.apps.is_empty() || !fresh {
                ui.label(
                    "Asking the phone for its apps… It answers with \"Let the computer open my apps\" on in Pocketdesk's Settings there.",
                );
                if self.apps.is_empty() {
                    return;
                }
            }
            let search = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("Find an app")
                    .desired_width(240.0),
            );
            if std::mem::take(&mut self.focus) {
                search.request_focus();
            }
            let found = shown(&self.apps, &self.query);
            if found.is_empty() {
                ui.label("No app matches.");
            }
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                for app in found.iter().take(MAX_SHOWN) {
                    ui.horizontal(|ui| {
                        if ui
                            .button("Open")
                            .on_hover_text(format!("Open {} on the phone", app.name))
                            .clicked()
                        {
                            action = Some(Action::Open(app.id.clone()));
                        }
                        ui.label(&app.name);
                    });
                }
            });
            if found.len() > MAX_SHOWN {
                ui.label(format!("{} more: type to narrow them.", found.len() - MAX_SHOWN));
            }
        });
        action
    }
}

/// The apps `query` finds: every word in its name or id, any case, in the
/// order the phone listed them.
pub fn shown<'a>(apps: &'a [PhoneApp], query: &str) -> Vec<&'a PhoneApp> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    apps.iter()
        .filter(|app| {
            let text = format!("{} {}", app.name, app.id).to_lowercase();
            words.iter().all(|w| text.contains(w.as_str()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, name: &str) -> PhoneApp {
        PhoneApp {
            id: id.into(),
            name: name.into(),
        }
    }

    /// P2: a search finds apps by name or id, every word, any case.
    #[test]
    fn a_search_finds_apps_by_name_or_id() {
        let apps = vec![
            app("org.example.music", "Music Player"),
            app("org.example.maps", "Maps"),
        ];
        assert_eq!(shown(&apps, "").len(), 2);
        assert_eq!(shown(&apps, "MUSIC")[0].id, "org.example.music");
        assert_eq!(shown(&apps, "example maps")[0].name, "Maps");
        assert!(shown(&apps, "music maps").is_empty());
    }
}
