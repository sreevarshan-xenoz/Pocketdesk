//! The computer's clipboard to connected phones as it changes (T10b).
//!
//! A phone used to see the computer's clipboard only by asking for it every
//! 4 s during a session. Now the server watches the clipboard and pushes
//! new text to every live control connection whose phone may receive
//! pushes and has the Clipboard permission, so a phone holding a link in the
//! background gets it too. Text only: an image needs a transfer that dials
//! the phone back.
//!
//! The owner's other computers (2g) get it the same way when
//! `computer_clipboard` is on: those linked to this one as its clients, and
//! those this one keeps a link to (`peer_computers`). Never back to the
//! device it came from.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket};

/// The longest clipboard text pushed. A phone asking in a session still
/// gets longer text; a push goes to every phone, so it stays small.
pub(crate) const MAX_PUSH: usize = 256 * 1024;

/// How often the clipboard is looked at while a phone could receive it. On
/// Windows a look costs a sequence number; elsewhere it costs a flag set by
/// one `wl-paste --watch` (R6d), or runs `wl-paste` where that cannot run.
#[cfg(windows)]
const TICK: Duration = Duration::from_secs(1);
#[cfg(not(windows))]
const TICK: Duration = Duration::from_secs(2);

/// Change detection over successive reads of the clipboard's text. It keeps
/// a hash of what it saw, not the text: a large copy is not held twice.
#[derive(Debug, Default)]
pub(crate) struct Watch {
    seen: Option<u64>,
    primed: bool,
}

fn hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

impl Watch {
    /// The text to push after reading `text` (`None`: no text on the
    /// clipboard). The first read only learns what is there: the phone
    /// asks for the current clipboard itself when a session starts.
    pub(crate) fn observe(&mut self, text: Option<&str>) -> Option<String> {
        let first = !self.primed;
        self.primed = true;
        let text = text.filter(|t| !t.is_empty());
        let seen = text.map(hash);
        if seen == self.seen {
            return None;
        }
        self.seen = seen;
        if first {
            return None;
        }
        text.filter(|t| t.len() <= MAX_PUSH).map(str::to_string)
    }
}

/// The marks a copy carries on Windows that say it is private: password
/// managers set them so clipboard history, cloud sync and monitors skip it.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowsMarks {
    /// `ExcludeClipboardContentFromMonitorProcessing` is on the clipboard.
    pub exclude_from_monitors: bool,
    /// `Clipboard Viewer Ignore` is on the clipboard.
    pub viewer_ignore: bool,
    /// The DWORD in `CanIncludeInClipboardHistory`, when present.
    pub history: Option<u32>,
    /// The DWORD in `CanUploadToCloudClipboard`, when present.
    pub cloud: Option<u32>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl WindowsMarks {
    /// Is the copy private? Then it never leaves this computer.
    pub(crate) fn private(self) -> bool {
        self.exclude_from_monitors
            || self.viewer_ignore
            || self.history == Some(0)
            || self.cloud == Some(0)
    }
}

/// Is a Wayland copy private? KDE's password managers (KeePassXC among
/// them) offer `x-kde-passwordManagerHint` with it.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn private_types(types: &str) -> bool {
    types
        .lines()
        .any(|t| t.trim() == "x-kde-passwordManagerHint")
}

/// Pushes to phones (`clipboard_push`).
static TO_PHONES: AtomicBool = AtomicBool::new(true);

/// Shares with the owner's other computers (`computer_clipboard`, 2g).
static WITH_COMPUTERS: AtomicBool = AtomicBool::new(false);

/// What the watcher sends to, from the config at start; and whether it
/// runs at all.
pub(crate) fn configure(phones: bool, computers: bool) -> bool {
    TO_PHONES.store(phones, Ordering::Relaxed);
    WITH_COMPUTERS.store(computers, Ordering::Relaxed);
    phones || computers
}

/// Whether the clipboard is shared with the owner's other computers.
pub(crate) fn with_computers() -> bool {
    WITH_COMPUTERS.load(Ordering::Relaxed)
}

/// Whether a connection may be pushed the clipboard: a computer while
/// sharing with computers, a phone while pushing to phones, either with
/// the Clipboard permission.
fn may_push(device_id: &str) -> bool {
    let wanted = if crate::plugins::pair::is_computer_device(device_id) {
        with_computers()
    } else {
        TO_PHONES.load(Ordering::Relaxed)
    };
    wanted && crate::plugins::pair::permits(device_id, "kdeconnect.clipboard")
}

/// Which phone last set the computer's clipboard, and to what: that phone
/// is not sent its own text back.
static FROM_PHONE: Mutex<Option<(String, String)>> = Mutex::new(None);

/// Note that `device_id` just set the clipboard to `text`.
pub(crate) fn note_from_phone(device_id: &str, text: &str) {
    // It crossed: kept in the history (D3), with the device it came from.
    crate::clip_history::note(text, Some(device_id));
    *FROM_PHONE.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((device_id.to_string(), text.to_string()));
}

/// The note, used once: the next change it explains, or not. A copy of the
/// same text later is the computer's own, and goes to that phone too.
fn take_from_phone() -> Option<(String, String)> {
    FROM_PHONE.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Whether `text` came from `device_id` (the note): not sent back to it.
fn came_from(from: Option<&(String, String)>, device_id: &str, text: &str) -> bool {
    from.is_some_and(|(id, sent)| id == device_id && sent == text)
}

/// The connections to push `text` to: those that may have it (`may`), but
/// not the phone the text came from.
pub(crate) fn targets(
    clients: Vec<Arc<dyn DeviceSender>>,
    text: &str,
    from_phone: Option<&(String, String)>,
    may: impl Fn(&dyn DeviceSender) -> bool,
) -> Vec<Arc<dyn DeviceSender>> {
    clients
        .into_iter()
        .filter(|c| !came_from(from_phone, c.device_id(), text))
        .filter(|c| may(&**c))
        .collect()
}

/// The packet a push sends: what the phone's own request is answered with.
pub(crate) fn packet(text: &str) -> NetworkPacket {
    NetworkPacket::new("kdeconnect.clipboard").with_body(serde_json::json!({ "content": text }))
}

/// One `wl-paste --watch` while a phone could receive the clipboard (R6d):
/// it runs `echo` at each change, so a line on its output says the
/// clipboard changed, and the loop reads it only then, instead of spawning
/// `wl-paste` every tick.
#[cfg(not(windows))]
struct ChangeWatch {
    child: tokio::process::Child,
    changed: Arc<AtomicBool>,
}

/// After a watcher could not start or ended, how long the loop reads every
/// tick before it tries one again (a compositor restarting, or an old
/// `wl-clipboard` without `--watch`).
#[cfg(not(windows))]
const WATCH_RETRY: Duration = Duration::from_secs(60);

#[cfg(not(windows))]
impl ChangeWatch {
    fn start() -> std::io::Result<Self> {
        Self::start_with("wl-paste", &["--watch", "echo"])
    }

    fn start_with(program: &str, args: &[&str]) -> std::io::Result<Self> {
        let mut child = tokio::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("no output to read"))?;
        // What is there when it starts is read once.
        let changed = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&changed);
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            while let Ok(Some(_)) = lines.next_line().await {
                flag.store(true, Ordering::Relaxed);
            }
        });
        Ok(Self { child, changed })
    }

    /// Whether the clipboard changed since the last look; `None` once the
    /// watcher has ended.
    fn take_change(&mut self) -> Option<bool> {
        match self.child.try_wait() {
            Ok(None) => Some(self.changed.swap(false, Ordering::Relaxed)),
            _ => None,
        }
    }
}

/// Watch the clipboard while any connection could receive it, and push
/// what changes. Runs for the server's life when `clipboard_push` is on.
pub(crate) async fn run() {
    let mut watch = Watch::default();
    #[cfg(windows)]
    let mut sequence = None;
    #[cfg(not(windows))]
    let mut change_watch: Option<ChangeWatch> = None;
    #[cfg(not(windows))]
    let mut watch_again_at: Option<tokio::time::Instant> = None;
    #[cfg(not(windows))]
    let mut said = false;
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let clients = crate::state::clone_clients().await;
        let linked = crate::peer_computers::linked();
        // A paired KDE Connect device's link counts too (3c): it is no
        // client of the control channel's.
        if clients.is_empty()
            && linked.is_empty()
            && crate::kdeconnect_compat::link::live_paired().is_empty()
        {
            // No phone to tell: what is copied meanwhile is not pushed when
            // one comes back (a phone in a session asks for it itself).
            watch = Watch::default();
            #[cfg(windows)]
            {
                sequence = None;
            }
            // No one to tell: the watcher ends with the last of them.
            #[cfg(not(windows))]
            {
                change_watch = None;
            }
            continue;
        }
        // On Windows the clipboard's sequence number says whether anything
        // changed, without opening the clipboard.
        #[cfg(windows)]
        {
            // A stand-in clipboard (tests) has no sequence: read each tick.
            let now = crate::plugins::clipboard::stand_in()
                .is_none()
                .then(crate::platform::clipboard_windows::sequence);
            if now.is_some() && sequence == now {
                continue;
            }
            // Noted only once the clipboard was read: a read that failed
            // (another app held it) is tried again next tick.
            match crate::plugins::clipboard::read_text().await {
                Ok(text) => {
                    sequence = now;
                    push_change(&mut watch, text, clients, linked).await;
                }
                Err(e) => tracing::debug!("The clipboard was not read: {e}"),
            }
        }
        #[cfg(not(windows))]
        {
            let may_watch = crate::plugins::clipboard::stand_in().is_none()
                && watch_again_at.is_none_or(|at| tokio::time::Instant::now() >= at);
            if change_watch.is_none() && may_watch {
                match ChangeWatch::start() {
                    Ok(started) => change_watch = Some(started),
                    Err(e) => {
                        if !std::mem::replace(&mut said, true) {
                            tracing::info!(
                                "No wl-paste --watch ({e}): the clipboard is read every 2 s"
                            );
                        }
                        watch_again_at = Some(tokio::time::Instant::now() + WATCH_RETRY);
                    }
                }
            }
            if let Some(watcher) = change_watch.as_mut() {
                match watcher.take_change() {
                    Some(false) => continue,
                    Some(true) => {}
                    None => {
                        if !std::mem::replace(&mut said, true) {
                            tracing::info!(
                                "wl-paste --watch ended: the clipboard is read every 2 s for a while"
                            );
                        }
                        change_watch = None;
                        watch_again_at = Some(tokio::time::Instant::now() + WATCH_RETRY);
                    }
                }
            }
            let text = crate::plugins::clipboard::read_text().await.ok().flatten();
            push_change(&mut watch, text, clients, linked).await;
        }
    }
}

/// Push `text` to `clients`, and to the computers this one keeps a link to
/// (`linked`), if it is a change.
async fn push_change(
    watch: &mut Watch,
    text: Option<String>,
    clients: Vec<Arc<dyn DeviceSender>>,
    linked: Vec<crate::peer_computers::LinkedComputer>,
) {
    let Some(text) = watch.observe(text.as_deref()) else {
        return;
    };
    let from_phone = take_from_phone();
    let targets = targets(clients, &text, from_phone.as_ref(), |c| {
        may_push(c.device_id())
    });
    let packet = packet(&text);
    // Paired KDE Connect devices, as phones (3c): each link sends it only
    // to a device allowed the clipboard.
    let kde: Vec<String> = if TO_PHONES.load(Ordering::Relaxed) {
        crate::kdeconnect_compat::link::live_paired()
            .into_iter()
            .filter(|id| !came_from(from_phone.as_ref(), id, &text))
            .collect()
    } else {
        Vec::new()
    };
    // This computer's own copy, gone to anyone, a phone, a computer or a KDE
    // device: kept in the history (D3 review). A phone's own text was kept
    // as it arrived.
    let goes = !targets.is_empty()
        || !kde.is_empty()
        || linked
            .iter()
            .any(|c| !came_from(from_phone.as_ref(), &c.device_id, &text));
    if from_phone.as_ref().is_none_or(|(_, from)| from != &text) && goes {
        crate::clip_history::note(&text, None);
    }
    for target in targets {
        if let Err(e) = target.send_packet(&packet).await {
            tracing::debug!("Clipboard not pushed to {}: {e}", target.device_id());
        }
    }
    for computer in linked {
        if !came_from(from_phone.as_ref(), &computer.device_id, &text) {
            computer.send(packet.clone());
        }
    }
    for device_id in kde {
        crate::kdeconnect_compat::link::tell(
            &device_id,
            crate::kdeconnect_compat::link::Event::Clipboard(text.clone()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::protocol::kdeconnect::TcpDeviceSender;

    #[cfg(not(windows))]
    #[tokio::test]
    async fn a_watcher_says_a_change_once_and_none_once_it_ends() {
        // Two changes, then it stays: `sh` stands in for `wl-paste --watch`.
        let mut watcher = ChangeWatch::start_with("sh", &["-c", "echo; echo; sleep 5"]).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(watcher.take_change(), Some(true));
        assert_eq!(watcher.take_change(), Some(false), "said once");
        let mut ended = ChangeWatch::start_with("sh", &["-c", "exit 1"]).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(ended.take_change(), None, "an old wl-paste without --watch");
    }

    #[test]
    fn a_copy_is_not_sent_back_to_the_device_it_came_from() {
        let from = ("desk-b".to_string(), "hello".to_string());
        assert!(came_from(Some(&from), "desk-b", "hello"));
        assert!(
            !came_from(Some(&from), "desk-b", "a later copy"),
            "its own copy goes"
        );
        assert!(!came_from(Some(&from), "phone", "hello"), "others get it");
        assert!(!came_from(None, "desk-b", "hello"));
    }

    #[test]
    fn a_copy_a_password_manager_marks_stays_on_the_computer() {
        assert!(!WindowsMarks::default().private());
        for marks in [
            WindowsMarks {
                exclude_from_monitors: true,
                ..Default::default()
            },
            WindowsMarks {
                viewer_ignore: true,
                ..Default::default()
            },
            WindowsMarks {
                history: Some(0),
                ..Default::default()
            },
            WindowsMarks {
                cloud: Some(0),
                ..Default::default()
            },
        ] {
            assert!(marks.private(), "{marks:?}");
        }
        assert!(
            !WindowsMarks {
                history: Some(1),
                cloud: Some(1),
                ..Default::default()
            }
            .private(),
            "allowed explicitly"
        );
        assert!(private_types("text/plain\nx-kde-passwordManagerHint\n"));
        assert!(!private_types("text/plain;charset=utf-8\nTEXT\n"));
    }

    #[test]
    fn the_note_of_a_phone_copy_is_used_once() {
        note_from_phone("a", "hello");
        assert_eq!(take_from_phone(), Some(("a".into(), "hello".into())));
        assert_eq!(take_from_phone(), None);
    }

    #[test]
    fn only_a_change_after_the_first_read_is_pushed() {
        let mut watch = Watch::default();
        assert_eq!(watch.observe(Some("there at start")), None);
        assert_eq!(watch.observe(Some("there at start")), None);
        assert_eq!(watch.observe(Some("copied")), Some("copied".into()));
        assert_eq!(watch.observe(Some("copied")), None, "once");
        assert_eq!(watch.observe(None), None, "an image or nothing");
        assert_eq!(
            watch.observe(Some("copied")),
            Some("copied".into()),
            "the same text copied again after something else"
        );
        assert_eq!(watch.observe(Some("")), None);
        let long = "x".repeat(MAX_PUSH + 1);
        assert_eq!(watch.observe(Some(&long)), None, "too long to push");
    }

    fn phone(id: &str) -> Arc<dyn DeviceSender> {
        Arc::new(TcpDeviceSender::from_arc(
            Arc::new(tokio::sync::Mutex::new(tokio::io::sink())),
            id.to_string(),
        ))
    }

    #[test]
    fn the_text_goes_to_permitted_phones_but_not_back_to_its_sender() {
        let clients = vec![phone("a"), phone("b"), phone("c")];
        let ids = |v: Vec<Arc<dyn DeviceSender>>| -> Vec<String> {
            v.iter().map(|c| c.device_id().to_string()).collect()
        };
        let from_a = ("a".to_string(), "hello".to_string());
        assert_eq!(
            ids(targets(clients.clone(), "hello", Some(&from_a), |c| c
                .device_id()
                != "c")),
            ["b"],
            "not back to a; c may not have it"
        );
        assert_eq!(
            ids(targets(clients, "other", Some(&from_a), |_| true)),
            ["a", "b", "c"],
            "a gets text it did not send"
        );
    }
}
