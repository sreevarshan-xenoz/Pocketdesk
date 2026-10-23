//! Links a phone hands over (roadmap T8a): `kdeconnect.pocketdesk.link
//! {url}`, under the Links permission (the dispatch gate checks it). A link
//! that passes `link_problem` opens in the default browser, or, with
//! `open_links` off, is shown as a notification; a phone may hand over at
//! most [`MAX_PER_MINUTE`] a minute.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use pocketdesk_core::protocol::link::{LINK, link_problem};

/// How many links one phone may hand over in a minute.
pub const MAX_PER_MINUTE: usize = 5;

/// `open_links` from the config, set at start.
static OPEN_LINKS: AtomicBool = AtomicBool::new(true);

/// Open links from phones (true) or only show them (false).
pub fn configure(open_links: bool) {
    OPEN_LINKS.store(open_links, Ordering::Relaxed);
}

type Opener = Arc<dyn Fn(&str) -> std::result::Result<(), String> + Send + Sync>;
type Shower = Arc<dyn Fn(&str, &str) + Send + Sync>;
type Paused = Arc<dyn Fn() -> bool + Send + Sync>;
/// The lines of Settings' links by site (P8).
type Rules = Arc<dyn Fn() -> Vec<String> + Send + Sync>;
/// Hands a link to a program a rule names (P8).
type Runner = Arc<dyn Fn(&std::path::Path, &str) -> std::result::Result<(), String> + Send + Sync>;

pub struct LinksPlugin {
    open: Opener,
    show: Shower,
    /// Whether this computer is paused (K2a): a link is then only listed.
    paused: Paused,
    rules: Rules,
    run: Runner,
    recent: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl Default for LinksPlugin {
    fn default() -> Self {
        Self {
            paused: Arc::new(crate::paused::now),
            rules: Arc::new(|| crate::events::current_config().link_rules),
            run: Arc::new(run_detached),
            ..Self::with(Arc::new(open_detached), Arc::new(show_link))
        }
    }
}

impl LinksPlugin {
    fn with(open: Opener, show: Shower) -> Self {
        Self {
            open,
            show,
            paused: Arc::new(|| false),
            rules: Arc::new(Vec::new),
            run: Arc::new(|_, _| Ok(())),
            recent: Mutex::default(),
        }
    }

    /// Whether `device` may hand over another link at `now`; counts it.
    fn within_rate(&self, device: &str, now: Instant) -> bool {
        let mut recent = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        let times = recent.entry(device.to_string()).or_default();
        while times
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= Duration::from_secs(60))
        {
            times.pop_front();
        }
        if times.len() >= MAX_PER_MINUTE {
            return false;
        }
        times.push_back(now);
        true
    }

    fn take(&self, url: &str, device: &str, now: Instant) {
        if let Some(problem) = link_problem(url) {
            tracing::warn!("Link from {device} refused: {problem}");
            return;
        }
        if !self.within_rate(device, now) {
            tracing::info!("Link from {device} refused: more than {MAX_PER_MINUTE} a minute");
            return;
        }
        crate::events::emit(
            "link_received",
            Some(device),
            serde_json::json!({ "host": pocketdesk_core::protocol::link::link_host(url) }),
            serde_json::json!({ "url": url }),
        );
        // Paused (K2a): on Activity with the event above, nothing more.
        if (self.paused)() {
            tracing::info!("A link from {device} kept on Activity: paused");
            return;
        }
        if OPEN_LINKS.load(Ordering::Relaxed) {
            // P8: what Settings say for this link's site, if anything.
            let rules = pocketdesk_core::local::link_rules::rules(&(self.rules)());
            let action = pocketdesk_core::protocol::link::link_host(url)
                .and_then(|host| pocketdesk_core::local::link_rules::pick(&rules, &host).cloned())
                .map(|rule| rule.action);
            use pocketdesk_core::local::link_rules::Action;
            match action {
                Some(Action::Show) => {}
                Some(Action::Program(program)) => match (self.run)(&program, url) {
                    Ok(()) => {
                        tracing::info!("Opening a link from {device} with {}", program.display());
                        return;
                    }
                    // Said, never silent (P8).
                    Err(e) => {
                        tracing::warn!(
                            "Could not open a link from {device} with {}: {e}",
                            program.display()
                        );
                        (self.show)(
                            &format!(
                                "Pocketdesk: could not open a link with {}",
                                program.display()
                            ),
                            url,
                        );
                        return;
                    }
                },
                Some(Action::Browser) | None => match (self.open)(url) {
                    Ok(()) => {
                        tracing::info!("Opening a link from {device}");
                        return;
                    }
                    Err(e) => {
                        tracing::warn!("Could not open a link from {device}: {e}");
                        (self.show)("Pocketdesk: could not open a link from your phone", url);
                        return;
                    }
                },
            }
        }
        (self.show)("Pocketdesk: a link from your phone", url);
    }
}

/// Send the numbers queued for phones to dial (L4), each to its own phone
/// if it is connected within a minute and its Dial permission is on. Runs
/// for the server's life.
pub async fn run_dials() {
    use pocketdesk_core::local::dials;
    use pocketdesk_core::protocol::dial::{DIAL, dial_body};
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let mut waiting: Vec<dials::DialRequest> = Vec::new();
    loop {
        if waiting.is_empty() {
            crate::inbox::wait(
                pocketdesk_core::local::paths::DIALS_DIR,
                Duration::from_millis(500),
            )
            .await;
        } else {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let now = pocketdesk_core::local::pin::now_unix();
        waiting.extend(dials::take_all(&dir, now));
        if waiting.is_empty() {
            continue;
        }
        let clients = crate::state::clone_clients().await;
        let mut still = Vec::new();
        for call in waiting.drain(..) {
            // Paired now, not only allowed by default (L4 review, as Ring is).
            if !crate::plugins::pair::allows(
                &call.device_id,
                pocketdesk_core::protocol::permissions::Capability::Dial,
            ) {
                tracing::info!("A call was not sent: Call numbers from here is off for the phone");
                continue;
            }
            let Some(phone) = clients.iter().find(|c| c.device_id() == call.device_id) else {
                if now.saturating_sub(call.created_unix) < dials::DIAL_TTL_SECS {
                    still.push(call);
                } else {
                    tracing::info!(
                        "A call was not sent: the phone did not connect within a minute"
                    );
                }
                continue;
            };
            let packet = NetworkPacket::new(DIAL).with_body(dial_body(&call.number));
            if let Err(e) = phone.send_packet(&packet).await {
                tracing::warn!("A call was not sent to the phone: {e}");
            }
        }
        waiting = still;
    }
}

/// Send the screenshots and locks asked of phones (O2), each to its own
/// phone if it is connected within a minute and its Phone actions
/// permission is on. Runs for the server's life.
pub async fn run_phone_actions() {
    use pocketdesk_core::local::phone_actions;
    use pocketdesk_core::protocol::phone_action::{PHONE_ACTION, body};
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let mut waiting: Vec<phone_actions::PhoneActionRequest> = Vec::new();
    loop {
        if waiting.is_empty() {
            crate::inbox::wait(
                pocketdesk_core::local::paths::PHONE_ACTIONS_DIR,
                Duration::from_millis(500),
            )
            .await;
        } else {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let now = pocketdesk_core::local::pin::now_unix();
        waiting.extend(phone_actions::take_all(&dir, now));
        if waiting.is_empty() {
            continue;
        }
        let clients = crate::state::clone_clients().await;
        let mut still = Vec::new();
        for ask in waiting.drain(..) {
            let Some(action) = ask.action() else {
                continue;
            };
            // Paired now, not only allowed by default, as a call is. The
            // contacts go with the texts (P10).
            let (capability, word) = match action {
                pocketdesk_core::protocol::phone_action::PhoneAction::Contacts => (
                    pocketdesk_core::protocol::permissions::Capability::Texts,
                    "Texts",
                ),
                _ => (
                    pocketdesk_core::protocol::permissions::Capability::PhoneActions,
                    "Phone actions",
                ),
            };
            if !crate::plugins::pair::allows(&ask.device_id, capability) {
                tracing::info!(
                    "A {} was not asked: the phone's {word} permission is off",
                    action.as_str()
                );
                continue;
            }
            let Some(phone) = clients.iter().find(|c| c.device_id() == ask.device_id) else {
                if now.saturating_sub(ask.created_unix) < phone_actions::ACTION_TTL_SECS {
                    still.push(ask);
                } else {
                    tracing::info!(
                        "A {} was not asked: the phone did not connect within a minute",
                        action.as_str()
                    );
                }
                continue;
            };
            let packet =
                NetworkPacket::new(PHONE_ACTION).with_body(body(action, ask.app.as_deref()));
            match phone.send_packet(&packet).await {
                Ok(()) => tracing::info!("Asked the phone for a {}", action.as_str()),
                Err(e) => tracing::warn!("A {} was not asked of the phone: {e}", action.as_str()),
            }
        }
        waiting = still;
    }
}

/// Send the commands queued for phones' automation apps (L6), each to its
/// own phone if it is connected within a minute and its Command permission
/// is on. Runs for the server's life.
pub async fn run_commands() {
    use pocketdesk_core::local::commands;
    use pocketdesk_core::protocol::command::{COMMAND, command_body};
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let mut waiting: Vec<commands::CommandRequest> = Vec::new();
    loop {
        if waiting.is_empty() {
            crate::inbox::wait(
                pocketdesk_core::local::paths::COMMANDS_DIR,
                Duration::from_millis(500),
            )
            .await;
        } else {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let now = pocketdesk_core::local::pin::now_unix();
        waiting.extend(commands::take_all(&dir, now));
        if waiting.is_empty() {
            continue;
        }
        let clients = crate::state::clone_clients().await;
        let mut still = Vec::new();
        for command in waiting.drain(..) {
            // Paired now, not only allowed by default (L6 review, as Ring is).
            if !crate::plugins::pair::allows(
                &command.device_id,
                pocketdesk_core::protocol::permissions::Capability::Command,
            ) {
                tracing::info!("A command was not sent: it is off for the phone");
                continue;
            }
            let Some(phone) = clients.iter().find(|c| c.device_id() == command.device_id) else {
                if now.saturating_sub(command.created_unix) < commands::COMMAND_TTL_SECS {
                    still.push(command);
                } else {
                    tracing::info!(
                        "A command was not sent: the phone did not connect within a minute"
                    );
                }
                continue;
            };
            let packet =
                NetworkPacket::new(COMMAND).with_body(command_body(&command.name, &command.text));
            if let Err(e) = phone.send_packet(&packet).await {
                tracing::warn!("A command was not sent to the phone: {e}");
            }
        }
        waiting = still;
    }
}

#[async_trait::async_trait]
impl Plugin for LinksPlugin {
    fn name(&self) -> &'static str {
        "links"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[LINK]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        // A number to dial (L4), a command for the phone's automation apps
        // (L6) and a screenshot or lock (O2) go on the same road as a link.
        &[
            LINK,
            pocketdesk_core::protocol::dial::DIAL,
            pocketdesk_core::protocol::command::COMMAND,
            pocketdesk_core::protocol::phone_action::PHONE_ACTION,
        ]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type == LINK
            && let Some(url) = packet.body.get("url").and_then(|v| v.as_str())
        {
            self.take(url, sender.device_id(), Instant::now());
        }
        Ok(())
    }
}

/// Daemon task: hand each link the desktop app or the local API queued
/// (`core::local::links`) to its phone, if the phone is connected and may
/// take links.
pub async fn run_watcher() {
    let Ok(dir) = crate::state::state_dir() else {
        tracing::warn!("No state directory: handing links to phones is off");
        return;
    };
    loop {
        crate::inbox::wait(
            pocketdesk_core::local::paths::LINKS_DIR,
            Duration::from_millis(250),
        )
        .await;
        let now = pocketdesk_core::local::pin::now_unix();
        for request in pocketdesk_core::local::links::take_all(&dir, now) {
            let clients = crate::state::clone_clients().await;
            let allowed = crate::plugins::pair::permits(&request.device_id, LINK);
            if let Err(reason) = push(&request, &clients, allowed).await {
                tracing::info!("Link for {} not sent: {reason}", request.device_id);
            }
        }
    }
}

/// Push one queued link to its phone.
async fn push(
    request: &pocketdesk_core::local::links::LinkRequest,
    clients: &[Arc<dyn DeviceSender>],
    allowed: bool,
) -> std::result::Result<(), String> {
    if !allowed {
        return Err("links are off for this phone".to_string());
    }
    let phone = clients
        .iter()
        .find(|c| c.device_id() == request.device_id)
        .ok_or("the phone is not connected")?;
    let packet = NetworkPacket::new(LINK).with_body(serde_json::json!({ "url": request.url }));
    phone
        .send_packet(&packet)
        .await
        .map_err(|e| format!("could not send: {e}"))
}

/// Open a checked link from a phone's deck (F1) as a phone's own link is:
/// in the browser with "Links from phones" opening them, else shown in a
/// notification.
pub(crate) fn open_or_show(url: &str) -> std::result::Result<(), String> {
    if OPEN_LINKS.load(Ordering::Relaxed) {
        open_detached(url)
    } else {
        show_link("Pocketdesk: a link from your phone's deck", url);
        Ok(())
    }
}

/// Open `url` on a thread of its own, never the phone's packet loop: a
/// browser that is slow to start, or Windows asking which app to use, would
/// hold it. A link that cannot be opened is shown instead.
fn open_detached(url: &str) -> std::result::Result<(), String> {
    let url = url.to_string();
    std::thread::Builder::new()
        .name("open-link".to_string())
        .spawn(move || {
            if let Err(e) = open_in_browser(&url) {
                tracing::warn!("Could not open a link: {e}");
                // Said as a failure, not as a link to look at (P8).
                show_link("Pocketdesk: could not open a link from your phone", &url);
            }
        })
        .map(|_| ())
        .map_err(|e| format!("no thread to open it on: {e}"))
}

/// Hand `url` (already checked) to `program`, a rule's (P8), on a thread of
/// its own: as its one argument, never through a shell. A program that is
/// not there is said at once; one that fails to start, once it has.
fn run_detached(program: &std::path::Path, url: &str) -> std::result::Result<(), String> {
    // At most this many at once, each waited for on its thread (P8 review:
    // a browser or a call runs for hours, a thread each).
    const MAX_RUNNING: usize = 8;
    static RUNNING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    if !program.is_file() {
        return Err(format!("{} is not there", program.display()));
    }
    if RUNNING.fetch_add(1, Ordering::SeqCst) >= MAX_RUNNING {
        RUNNING.fetch_sub(1, Ordering::SeqCst);
        return Err(format!("{MAX_RUNNING} links are open in programs already"));
    }
    let (program, url) = (program.to_path_buf(), url.to_string());
    std::thread::Builder::new()
        .name("open-link-with".to_string())
        .spawn(move || {
            let started = std::process::Command::new(&program)
                .arg(&url)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            match started {
                // Waited for, so it leaves no zombie on Linux.
                Ok(mut child) => {
                    let _ = child.wait();
                    RUNNING.fetch_sub(1, Ordering::SeqCst);
                }
                Err(e) => {
                    RUNNING.fetch_sub(1, Ordering::SeqCst);
                    tracing::warn!("Could not open a link with {}: {e}", program.display());
                    show_link(
                        &format!(
                            "Pocketdesk: could not open a link with {}",
                            program.display()
                        ),
                        &url,
                    );
                }
            }
        })
        .map(|_| ())
        .map_err(|e| {
            RUNNING.fetch_sub(1, Ordering::SeqCst);
            format!("no thread to open it on: {e}")
        })
}

/// Open `url` (already checked) with the system's handler for web links:
/// `ShellExecuteW` on Windows, `xdg-open` on Linux (waited for, so it
/// leaves no zombie). Never a shell. Blocks.
fn open_in_browser(url: &str) -> std::result::Result<(), String> {
    #[cfg(windows)]
    {
        use windows::Win32::System::Com::{
            COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
        };
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        use windows::core::{HSTRING, w};
        // ShellExecute may hand the link to a COM handler, so the thread
        // needs COM, as its documentation asks.
        // SAFETY: called on this thread before any COM use, paired below.
        let com =
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        // SAFETY: valid, NUL-terminated wide strings for the call's duration.
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                &HSTRING::from(url),
                None,
                None,
                SW_SHOWNORMAL,
            )
        };
        if com.is_ok() {
            // SAFETY: pairs the successful CoInitializeEx above.
            unsafe { CoUninitialize() };
        }
        // Above 32 is success (the old HINSTANCE convention).
        if result.0 as usize > 32 {
            Ok(())
        } else {
            Err(format!("ShellExecute returned {}", result.0 as usize))
        }
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("xdg-open: {e}"))
            .and_then(|status| {
                if status.success() {
                    Ok(())
                } else {
                    Err(format!("xdg-open: {status}"))
                }
            })
    }
}

fn show_link(summary: &str, url: &str) {
    let (summary, url) = (summary.to_string(), url.to_string());
    std::thread::spawn(move || {
        if !crate::platform::notify(&summary, &url, Duration::from_secs(15)) {
            tracing::info!("{summary}: {url}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    type Seen = Arc<Mutex<Vec<String>>>;

    fn plugin() -> (LinksPlugin, Seen, Seen) {
        let opened: Seen = Arc::default();
        let shown: Seen = Arc::default();
        let (o, s) = (opened.clone(), shown.clone());
        let plugin = LinksPlugin::with(
            Arc::new(move |url| {
                o.lock().unwrap().push(url.to_string());
                Ok(())
            }),
            Arc::new(move |_, url| s.lock().unwrap().push(url.to_string())),
        );
        (plugin, opened, shown)
    }

    /// The setting is process-wide: tests that read it run one at a time.
    static SETTING: Mutex<()> = Mutex::new(());

    #[test]
    fn a_web_link_from_a_phone_opens_and_anything_else_does_not() {
        let _setting = SETTING.lock().unwrap_or_else(|e| e.into_inner());
        configure(true);
        let (plugin, opened, shown) = plugin();
        let now = Instant::now();
        plugin.take("https://example.org/page", "phone-a", now);
        plugin.take("file:///etc/passwd", "phone-a", now);
        plugin.take("javascript:alert(1)", "phone-a", now);
        assert_eq!(*opened.lock().unwrap(), ["https://example.org/page"]);
        assert!(shown.lock().unwrap().is_empty());
    }

    /// P8: a site's rule shows, hands to a program, or opens; the rest as
    /// before; a program that fails is said.
    #[test]
    fn a_links_site_decides_how_it_opens() {
        // The setting is the process's: held, and on (P8 review).
        let _setting = SETTING.lock().unwrap_or_else(|e| e.into_inner());
        configure(true);
        let (opened, shown) = (Seen::default(), Seen::default());
        let ran: Seen = Seen::default();
        let mut plugin = LinksPlugin::with(
            {
                let opened = opened.clone();
                Arc::new(move |url: &str| {
                    opened.lock().unwrap().push(url.to_string());
                    Ok(())
                })
            },
            {
                let shown = shown.clone();
                Arc::new(move |title: &str, url: &str| {
                    shown.lock().unwrap().push(format!("{title}|{url}"))
                })
            },
        );
        // A whole path on either system.
        fn program(name: &str) -> String {
            if cfg!(windows) {
                format!("C:\\opt\\{name}")
            } else {
                format!("/opt/{name}")
            }
        }
        plugin.rules = Arc::new(|| {
            vec![
                "video.example.org = show".to_string(),
                format!("meet.example.org = {}", program("meet")),
                format!("bad.example.org = {}", program("missing")),
            ]
        });
        plugin.run = {
            let ran = ran.clone();
            Arc::new(move |program: &std::path::Path, url: &str| {
                if program.ends_with("missing") {
                    return Err("not there".to_string());
                }
                ran.lock()
                    .unwrap()
                    .push(format!("{}|{url}", program.display()));
                Ok(())
            })
        };
        let now = Instant::now();
        plugin.take("https://video.example.org/watch", "phone-a", now);
        plugin.take("https://a.meet.example.org/j/1", "phone-b", now);
        plugin.take("https://example.org/", "phone-c", now);
        plugin.take("https://bad.example.org/x", "phone-d", now);
        assert_eq!(
            *opened.lock().unwrap(),
            vec!["https://example.org/".to_string()]
        );
        assert_eq!(
            *ran.lock().unwrap(),
            vec![format!(
                "{}|https://a.meet.example.org/j/1",
                program("meet")
            )]
        );
        let shown = shown.lock().unwrap();
        assert_eq!(shown.len(), 2);
        assert!(shown[0].ends_with("|https://video.example.org/watch"));
        assert!(
            shown[1].starts_with("Pocketdesk: could not open a link with"),
            "{shown:?}"
        );
    }

    /// K2a: paused, a link is neither opened nor shown.
    #[test]
    fn a_pause_holds_a_link() {
        let _setting = SETTING.lock().unwrap_or_else(|e| e.into_inner());
        configure(true);
        let (plugin, opened, shown) = plugin();
        let plugin = LinksPlugin {
            paused: Arc::new(|| true),
            ..plugin
        };
        plugin.take("https://example.org/page", "phone-a", Instant::now());
        assert!(opened.lock().unwrap().is_empty());
        assert!(shown.lock().unwrap().is_empty());
    }

    #[test]
    fn with_the_setting_off_a_link_is_shown_not_opened() {
        let _setting = SETTING.lock().unwrap_or_else(|e| e.into_inner());
        configure(false);
        let (plugin, opened, shown) = plugin();
        plugin.take("https://example.org/", "phone-a", Instant::now());
        configure(true);
        assert!(opened.lock().unwrap().is_empty());
        assert_eq!(*shown.lock().unwrap(), ["https://example.org/"]);
    }

    struct Phone {
        id: &'static str,
        sent: Mutex<Vec<NetworkPacket>>,
    }

    #[async_trait::async_trait]
    impl DeviceSender for Phone {
        fn device_id(&self) -> &str {
            self.id
        }
        fn connection_id(&self) -> &str {
            self.id
        }
        async fn send_packet(&self, packet: &NetworkPacket) -> Result<()> {
            self.sent.lock().unwrap().push(packet.clone());
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_queued_link_goes_to_its_phone_when_it_may_take_links() {
        let a = Arc::new(Phone {
            id: "phone-a",
            sent: Mutex::default(),
        });
        let b = Arc::new(Phone {
            id: "phone-b",
            sent: Mutex::default(),
        });
        let clients: Vec<Arc<dyn DeviceSender>> = vec![a.clone(), b.clone()];
        let request = pocketdesk_core::local::links::LinkRequest {
            id: "x".to_string(),
            device_id: "phone-b".to_string(),
            url: "https://example.org/".to_string(),
            created_unix: 1,
        };
        push(&request, &clients, true).await.unwrap();
        assert!(a.sent.lock().unwrap().is_empty());
        let sent = b.sent.lock().unwrap().clone();
        assert_eq!(sent[0].packet_type, LINK);
        assert_eq!(sent[0].body["url"], "https://example.org/");
        assert!(
            push(&request, &clients, false)
                .await
                .unwrap_err()
                .contains("off")
        );
        let mut away = request.clone();
        away.device_id = "phone-c".to_string();
        assert!(
            push(&away, &clients, true)
                .await
                .unwrap_err()
                .contains("not connected")
        );
    }

    #[test]
    fn a_phone_opens_at_most_five_links_a_minute() {
        let _setting = SETTING.lock().unwrap_or_else(|e| e.into_inner());
        configure(true);
        let (plugin, opened, _) = plugin();
        let now = Instant::now();
        for n in 0..8 {
            plugin.take(&format!("https://example.org/{n}"), "phone-a", now);
        }
        assert_eq!(opened.lock().unwrap().len(), MAX_PER_MINUTE);
        // Another phone is not held up by this one, and a minute later
        // this one may again.
        plugin.take("https://example.org/b", "phone-b", now);
        plugin.take(
            "https://example.org/later",
            "phone-a",
            now + Duration::from_secs(61),
        );
        assert_eq!(opened.lock().unwrap().len(), MAX_PER_MINUTE + 2);
    }
}
