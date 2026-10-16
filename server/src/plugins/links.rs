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

pub struct LinksPlugin {
    open: Opener,
    show: Shower,
    recent: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl Default for LinksPlugin {
    fn default() -> Self {
        Self::with(Arc::new(open_detached), Arc::new(show_link))
    }
}

impl LinksPlugin {
    fn with(open: Opener, show: Shower) -> Self {
        Self {
            open,
            show,
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
        if OPEN_LINKS.load(Ordering::Relaxed) {
            match (self.open)(url) {
                Ok(()) => {
                    tracing::info!("Opening a link from {device}");
                    return;
                }
                Err(e) => tracing::warn!("Could not open a link from {device}: {e}"),
            }
        }
        (self.show)("Pocketdesk: a link from your phone", url);
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
        &[LINK]
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
                show_link("Pocketdesk: a link from your phone", &url);
            }
        })
        .map(|_| ())
        .map_err(|e| format!("no thread to open it on: {e}"))
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
