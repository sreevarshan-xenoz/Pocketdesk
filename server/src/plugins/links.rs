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
        Self::with(Arc::new(open_in_browser), Arc::new(show_link))
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
        if OPEN_LINKS.load(Ordering::Relaxed) {
            match (self.open)(url) {
                Ok(()) => {
                    tracing::info!("Opened a link from {device}");
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

/// Open `url` (already checked) with the system's handler for web links:
/// `ShellExecuteW` on Windows, `xdg-open` on Linux. Never a shell.
fn open_in_browser(url: &str) -> std::result::Result<(), String> {
    #[cfg(windows)]
    {
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        use windows::core::{HSTRING, w};
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
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("xdg-open: {e}"))
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
