//! Desktop → phone notification mirroring on Windows.
//!
//! Windows lets a program read other apps' notifications only through
//! `UserNotificationListener`, and only when the program has a *package
//! identity* (it runs from an MSIX, or a sparse "packaged with external
//! location" package) and the user has granted access. The plain
//! `linux-link.exe` has no identity, so on a stock install this reports why
//! and ends — the phone simply receives no desktop notifications, exactly as
//! on a Linux desktop without a notification daemon. Packaging is documented
//! in docs/development-setup.md; once the process has an identity, this code
//! needs no change.
//!
//! The listener is polled: every two seconds the current toasts are read and
//! any id not seen before is forwarded.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::sync::broadcast;
use windows::UI::Notifications::Management::{
    UserNotificationListener, UserNotificationListenerAccessStatus,
};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds, UserNotification};

use crate::notification_monitor::ForwardedNotification;

const POLL: Duration = Duration::from_secs(2);

/// The forwardable form of one toast: app name, then its first text line as
/// the title and the rest as the body.
pub(crate) fn forwarded_from(app: String, texts: Vec<String>) -> ForwardedNotification {
    let mut texts = texts.into_iter();
    let summary = texts.next().unwrap_or_default();
    let body = texts.collect::<Vec<_>>().join("\n");
    ForwardedNotification {
        app_name: if app.is_empty() {
            "Windows".to_string()
        } else {
            app
        },
        summary,
        body,
        urgency: "normal".to_string(),
    }
}

fn describe(notification: &UserNotification) -> Result<ForwardedNotification> {
    let app = notification
        .AppInfo()
        .and_then(|info| info.DisplayInfo())
        .and_then(|display| display.DisplayName())
        .map(|name| name.to_string())
        .unwrap_or_default();
    let binding = notification
        .Notification()?
        .Visual()?
        .GetBinding(&KnownNotificationBindings::ToastGeneric()?)?;
    let texts = binding
        .GetTextElements()?
        .into_iter()
        .filter_map(|element| element.Text().ok())
        .map(|text| text.to_string())
        .filter(|text| !text.is_empty())
        .collect();
    Ok(forwarded_from(app, texts))
}

/// Whether this process runs with a package identity. Answered by the loader
/// without touching any WinRT object, so it cannot block.
fn has_package_identity() -> bool {
    use windows::Win32::Foundation::APPMODEL_ERROR_NO_PACKAGE;
    use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
    let mut len = 0u32;
    (unsafe { GetCurrentPackageFullName(&mut len, None) }) != APPMODEL_ERROR_NO_PACKAGE
}

fn listen(tx: &broadcast::Sender<ForwardedNotification>) -> Result<()> {
    // Checked first: without an identity the listener calls can block
    // indefinitely instead of failing.
    if !has_package_identity() {
        bail!("this process has no package identity, which reading notifications requires");
    }
    let listener = UserNotificationListener::Current().context("opening the listener")?;
    // GetAccessStatus never blocks. RequestAccessAsync would raise a consent
    // prompt, which a background server cannot show and would wait on forever,
    // so access is granted by the user in Settings instead.
    let access = listener.GetAccessStatus()?;
    if access != UserNotificationListenerAccessStatus::Allowed {
        bail!(
            "notification access is not granted; allow Linux Link under \
             Settings > Privacy & security > Notifications"
        );
    }
    tracing::info!("Mirroring Windows notifications to the phone");
    let mut seen: HashSet<u32> = HashSet::new();
    let mut first = true;
    loop {
        let current = listener
            .GetNotificationsAsync(NotificationKinds::Toast)?
            .join()?;
        let mut now = HashSet::new();
        for notification in current {
            let Ok(id) = notification.Id() else { continue };
            now.insert(id);
            // What was already on screen at start-up is history, not news.
            if first || seen.contains(&id) {
                continue;
            }
            match describe(&notification) {
                Ok(forwarded) => {
                    let _ = tx.send(forwarded);
                }
                Err(e) => tracing::debug!("unreadable notification {id}: {e:#}"),
            }
        }
        seen = now;
        first = false;
        std::thread::sleep(POLL);
    }
}

/// Run the listener on a blocking thread until it fails; failure (usually
/// "no package identity") is logged once and ends mirroring.
pub(crate) fn spawn(tx: broadcast::Sender<ForwardedNotification>) {
    tokio::task::spawn_blocking(move || {
        let _ = unsafe {
            windows::Win32::System::WinRT::RoInitialize(
                windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
            )
        };
        if let Err(e) = listen(&tx) {
            tracing::info!("Desktop notifications are not mirrored to the phone: {e:#}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_text_is_the_title_and_the_rest_the_body() {
        let n = forwarded_from(
            "Mail".into(),
            vec!["New message".into(), "From: a".into(), "Hi".into()],
        );
        assert_eq!(n.app_name, "Mail");
        assert_eq!(n.summary, "New message");
        assert_eq!(n.body, "From: a\nHi");
        let bare = forwarded_from(String::new(), vec![]);
        assert_eq!(bare.app_name, "Windows");
        assert!(bare.summary.is_empty() && bare.body.is_empty());
    }

    /// Without a package identity the listener must fail with a reason, not
    /// hang or panic (the dev box runs unpackaged).
    #[test]
    fn an_unpackaged_process_is_told_why() {
        let (tx, _rx) = broadcast::channel(4);
        let err = listen(&tx).expect_err("an unpackaged test binary cannot listen");
        assert!(!format!("{err:#}").is_empty());
    }
}
