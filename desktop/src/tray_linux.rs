//! The tray on Linux (N6): a StatusNotifierItem over D-Bus (`ksni`), with
//! the same icon, tooltip and menu as on Windows ([`crate::tray_menu`]:
//! Open Pocketdesk, Pair a phone, each paired phone's actions (D5), Quit);
//! a click on the icon opens the window.
//!
//! tray-icon's Linux menu needs a GTK main loop the app does not run; this
//! needs none. ksni serves the item from a thread of its own with its own
//! D-Bus connection, so a pick arrives there: it is queued for the app and
//! the app woken, as the Windows tray does. With no StatusNotifierWatcher on
//! the session bus (a desktop with no tray host, such as GNOME without the
//! AppIndicator extension) `Tray::new` fails, and closing the window quits.
//!
//! ksni runs zbus on async-io, but a build that also compiles the server
//! (`cargo build --workspace`, or the server and the app in one `cargo
//! build`) unifies zbus's `tokio` feature in, and zbus then asks for a Tokio
//! runtime as it connects: on a plain thread that panicked ("there is no
//! reactor running") and the tray came up without an icon. So ksni starts
//! inside a small Tokio runtime of its own, with one worker thread that runs
//! zbus's tasks while ksni waits on them, kept for as long as the tray lives;
//! with zbus on async-io the runtime just idles.

use std::sync::Mutex;

use ksni::blocking::TrayMethods;

use crate::tray_menu::{Entry, TrayAction};

pub struct Tray {
    handle: ksni::blocking::Handle<Item>,
    /// The menu shown, so it is replaced only when it changes.
    shown: Vec<Entry>,
    /// Keeps the runtime ksni's D-Bus connection may run on.
    _runtime: Keep,
}

impl Tray {
    /// Register the tray with the desktop's tray host.
    pub fn new() -> Result<Self, String> {
        let icon = [32, 64]
            .into_iter()
            .map(|size| ksni::Icon {
                width: size as i32,
                height: size as i32,
                data: crate::icon::argb32(&crate::icon::icon_rgba(size)),
            })
            .collect();
        // A tray host that never answers must not hold up the window (N6
        // review): registration gets a few seconds, on a thread of its own.
        let shown = crate::tray_menu::menu(&[], &[], 0);
        let entries = shown.clone();
        let got =
            on_runtime_thread(move || Item { icon, entries }.spawn().map_err(|e| e.to_string()));
        match got.recv_timeout(REGISTER_WAIT) {
            Ok(Ok((handle, keep))) => Ok(Self {
                handle: handle?,
                shown,
                _runtime: keep,
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(format!(
                "the tray host did not answer within {} s",
                REGISTER_WAIT.as_secs()
            )),
        }
    }

    /// Show `entries`, if they differ from the menu shown.
    pub fn show(&mut self, entries: Vec<Entry>) {
        if entries == self.shown {
            return;
        }
        let next = entries.clone();
        // `None`: the tray's service has ended; nothing to update.
        if self
            .handle
            .update(move |item| item.entries = next)
            .is_some()
        {
            self.shown = entries;
        }
    }

    /// Wake the app on every pick, even while its window is hidden.
    pub fn wake_on_click(wake: impl Fn() + Send + 'static) {
        *WAKE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(wake));
    }

    /// Whether the tray's service has ended (its D-Bus connection gone): the
    /// icon is not shown any more, so a tray program exits (E2 review).
    pub fn closed(&self) -> bool {
        self.handle.is_closed()
    }

    /// The picks since the last call, oldest first.
    pub fn take_actions(&self) -> Vec<TrayAction> {
        std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// How long registering with the tray host may take before the app goes on
/// without a tray.
const REGISTER_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// While held, the runtime of [`on_runtime_thread`] keeps running.
struct Keep(#[allow(dead_code)] tokio::sync::oneshot::Sender<()>);

/// Run `work` on a thread of its own with a Tokio runtime entered. The
/// runtime has a worker thread of its own, so what `work` spawns there runs
/// while `work` blocks waiting on it (zbus, built with `tokio`, reads its
/// socket in such a task as ksni connects), and it keeps running until the
/// [`Keep`] sent with the result is dropped. If the result is never taken
/// (the receiver gave up waiting), the runtime ends.
fn on_runtime_thread<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> std::sync::mpsc::Receiver<Result<(T, Keep), String>> {
    let (sent, got) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("tray-dbus".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("tray-dbus-worker")
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(e) => {
                    let _ = sent.send(Err(e.to_string()));
                    return;
                }
            };
            let out = {
                let _entered = runtime.enter();
                work()
            };
            let (keep, until) = tokio::sync::oneshot::channel::<()>();
            if sent.send(Ok((out, Keep(keep)))).is_ok() {
                runtime.block_on(async {
                    let _ = until.await;
                });
            }
        });
    // A thread that could not start dropped its sender: the receiver then
    // reports the tray host as not answering.
    drop(spawned);
    got
}

/// Picks waiting for the app, made on ksni's thread.
static PENDING: Mutex<Vec<TrayAction>> = Mutex::new(Vec::new());

/// The app's context, to wake it from ksni's thread.
static WAKE: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);

fn picked(action: TrayAction) {
    PENDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(action);
    if let Some(wake) = WAKE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        wake();
    }
}

/// The item ksni serves.
pub struct Item {
    icon: Vec<ksni::Icon>,
    entries: Vec<Entry>,
}

impl ksni::Tray for Item {
    fn id(&self) -> String {
        "pocketdesk".into()
    }

    fn title(&self) -> String {
        "Pocketdesk".into()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::ApplicationStatus
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon.clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Pocketdesk".into(),
            ..Default::default()
        }
    }

    /// A click on the icon opens the window, as on Windows.
    fn activate(&mut self, _x: i32, _y: i32) {
        picked(TrayAction::Open);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        self.entries.iter().map(item).collect()
    }
}

fn item(entry: &Entry) -> ksni::MenuItem<Item> {
    match entry {
        Entry::Item { label, action } => ksni::menu::StandardItem {
            label: text(label),
            enabled: action.is_some(),
            activate: match action.clone() {
                Some(action) => Box::new(move |_: &mut Item| picked(action.clone())),
                None => Box::new(|_: &mut Item| {}),
            },
            ..Default::default()
        }
        .into(),
        Entry::Phone { label, items } => ksni::menu::SubMenu {
            label: text(label),
            submenu: items.iter().map(item).collect(),
            ..Default::default()
        }
        .into(),
        Entry::Separator => ksni::MenuItem::Separator,
    }
}

/// A label as a tray host shows it: `_` marks the next letter as the
/// item's key, so a name's own is doubled.
fn text(label: &str) -> String {
    label.replace('_', "__")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksni::Tray as _;

    /// The tests share the queue of picks.
    static QUEUE: Mutex<()> = Mutex::new(());

    /// The panic of 2026-10-30: zbus built with `tokio` asks for the current
    /// runtime as ksni connects, from a thread that had none; and then waits,
    /// blocked, on a task it spawned there (its socket reader). The work runs
    /// where a runtime is entered, a task it spawns runs while it blocks, and
    /// one spawned later keeps running after it returns, until the tray lets
    /// go.
    #[test]
    fn the_tray_starts_inside_a_runtime_that_keeps_running() {
        let wait = std::time::Duration::from_secs(10);
        let got = on_runtime_thread(move || {
            let handle = tokio::runtime::Handle::current();
            let (tell, told) = std::sync::mpsc::channel();
            handle.spawn(async move {
                tokio::task::yield_now().await;
                let _ = tell.send(());
            });
            let during = told.recv_timeout(wait).is_ok();
            let (tell, told) = std::sync::mpsc::channel();
            handle.spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let _ = tell.send(());
            });
            (during, told)
        });
        let ((during, told), keep) = got
            .recv_timeout(wait)
            .expect("the thread answered")
            .expect("a runtime");
        assert!(
            during,
            "a task spawned there ran while the work waited on it"
        );
        told.recv_timeout(wait)
            .expect("a task spawned there ran after the work returned");
        drop(keep);
    }

    #[test]
    fn the_menu_is_the_windows_one_and_each_entry_is_queued() {
        let _queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        let item = Item {
            icon: Vec::new(),
            entries: crate::tray_menu::menu(&[], &[], 0),
        };
        let menu = item.menu();
        let labels: Vec<String> = menu
            .iter()
            .map(|m| match m {
                ksni::MenuItem::Standard(s) => s.label.clone(),
                ksni::MenuItem::Separator => "-".into(),
                _ => "?".into(),
            })
            .collect();
        assert_eq!(labels, ["Open Pocketdesk", "Pair a phone", "-", "Quit"]);
        let mut item = item;
        for entry in &menu {
            if let ksni::MenuItem::Standard(s) = entry {
                (s.activate)(&mut item);
            }
        }
        item.activate(0, 0);
        let tray_actions = std::mem::take(&mut *PENDING.lock().unwrap());
        assert_eq!(
            tray_actions,
            [
                TrayAction::Open,
                TrayAction::Pair,
                TrayAction::Quit,
                TrayAction::Open
            ]
        );
    }

    /// D5: a phone is a submenu, its disabled items picking nothing, and
    /// a name's underscore is shown as itself.
    #[test]
    fn a_phone_is_a_submenu() {
        let _queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        let item = Item {
            icon: Vec::new(),
            entries: vec![Entry::Phone {
                label: "my_phone".to_string(),
                items: vec![
                    Entry::Item {
                        label: "Ring".to_string(),
                        action: Some(TrayAction::Ring("p".to_string())),
                    },
                    Entry::Item {
                        label: "Send file…".to_string(),
                        action: None,
                    },
                ],
            }],
        };
        let menu = item.menu();
        let ksni::MenuItem::SubMenu(sub) = &menu[0] else {
            panic!("not a submenu");
        };
        assert_eq!(sub.label, "my__phone");
        let ksni::MenuItem::Standard(off) = &sub.submenu[1] else {
            panic!("not an item");
        };
        assert!(!off.enabled);
        let mut item = item;
        for entry in &sub.submenu {
            if let ksni::MenuItem::Standard(s) = entry {
                (s.activate)(&mut item);
            }
        }
        assert_eq!(
            std::mem::take(&mut *PENDING.lock().unwrap()),
            [TrayAction::Ring("p".to_string())]
        );
    }
}
