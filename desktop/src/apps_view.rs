//! "Open an app on the phone…" (P2): the apps a phone listed, found by a
//! search, each with Open. The list is asked of the phone as the box opens
//! and read from `phone_apps.json` once a second while it shows.

use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui;
use pocketdesk_core::protocol::phone_action::PhoneApp;

/// How often the box reads the phone's list while it shows.
const READ_EVERY: Duration = Duration::from_secs(1);

/// How long the box waits for the phone's answer before saying it has none.
const ANSWER_WITHIN_SECS: u64 = 15;

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
    scroll: bool,
}

/// What the box asks of the app this frame.
pub enum Action {
    /// The app's id and its name.
    Open(String, String),
    /// Ask the phone for its list again (Q2 review).
    AskAgain,
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
            scroll: true,
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
        let drawn = ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("Open an app on {who}")).strong());
                if ui.button("Close").clicked() {
                    action = Some(Action::Close);
                }
            });
            // A list made since this ask; an older one is shown greyed (P2 review).
            let fresh = list_fresh(self.listed_at, self.asked_at);
            if !fresh {
                let waited = now_unix().saturating_sub(self.asked_at);
                if waited < ANSWER_WITHIN_SECS {
                    ui.label("Asking the phone for its apps…");
                } else {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("No answer from the phone. It answers while unlocked, with \"Let the computer open my apps\" on in Pocketdesk's Settings there.");
                        if ui.button("Ask again").clicked() {
                            action = Some(Action::AskAgain);
                        }
                    });
                }
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
            if self.apps.is_empty() {
                ui.label("The phone sent no apps.");
            } else if found.is_empty() {
                ui.label("No app matches.");
            }
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                for app in found.iter().take(MAX_SHOWN) {
                    ui.horizontal_wrapped(|ui| {
                        let open = ui
                            .add_enabled(fresh, egui::Button::new("Open"))
                            .on_hover_text(format!("Open {} on the phone", app.name))
                            .on_disabled_hover_text(
                                "An older list: waiting for the phone's answer.",
                            );
                        // Which app, to a screen reader (Q2 review).
                        let label = format!("Open {}", app.name);
                        open.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Button, fresh, &label)
                        });
                        if open.clicked()
                        {
                            action = Some(Action::Open(app.id.clone(), app.name.clone()));
                        }
                        ui.label(&app.name);
                    });
                }
            });
            if found.len() > MAX_SHOWN {
                ui.label(format!("{} more: type to narrow them.", found.len() - MAX_SHOWN));
            }
        });
        // Brought into view as it opens (Q2 review: it is drawn under
        // every phone, perhaps out of sight of its button).
        if std::mem::take(&mut self.scroll) {
            drawn.response.scroll_to_me(Some(egui::Align::Center));
        }
        action
    }
}

/// Whether a list made at `listed_at` answers the ask made at `asked_at`
/// (both this computer's clock).
pub fn list_fresh(listed_at: Option<u64>, asked_at: u64) -> bool {
    listed_at.is_some_and(|at| at >= asked_at)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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

    /// P2 review: only a list made since the ask answers it.
    #[test]
    fn a_list_answers_only_the_ask_before_it() {
        assert!(list_fresh(Some(100), 100));
        assert!(!list_fresh(Some(99), 100));
        assert!(!list_fresh(None, 100));
    }
}
