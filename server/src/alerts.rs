//! Alerts for the phone (BF1): what `pocketdesk done` leaves in
//! `alerts_out/` is shown as one of this computer's notifications, which
//! every phone that shows them gets.

use std::time::Duration;

use pocketdesk_core::local::alerts::{self, Alert};
use pocketdesk_core::local::paths::ALERTS_DIR;
use tokio::sync::broadcast;

use crate::notification_monitor::ForwardedNotification;

/// The app an alert is shown as on the phone: not this server's own name,
/// whose notifications are never relayed to phones.
pub const APP: &str = "Pocketdesk done";

/// How often the folder is looked at without a file watcher.
const POLL: Duration = Duration::from_secs(2);

/// Take the alerts as they come and show them, for the server's life.
pub async fn run(notifications: broadcast::Sender<ForwardedNotification>) {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    loop {
        crate::inbox::wait(ALERTS_DIR, POLL).await;
        for alert in alerts::take_all(&dir, pocketdesk_core::local::pin::now_unix()) {
            tracing::info!(title = %alert.title, "An alert for the phone");
            // None listening is no phone to tell: nothing is kept for later.
            let _ = notifications.send(forwarded(&alert));
        }
    }
}

/// `alert` as one of this computer's notifications.
fn forwarded(alert: &Alert) -> ForwardedNotification {
    ForwardedNotification {
        app_name: APP.to_string(),
        summary: alert.title.clone(),
        body: alert.text.clone(),
        urgency: "normal".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BF1: an alert goes to the phones, as the computer's notifications
    /// do, never held back as the server's own.
    #[test]
    fn an_alert_is_relayed_as_the_computer_s_notification() {
        let alert = Alert {
            id: "a".to_string(),
            title: "Done: make".to_string(),
            text: "took 3 s".to_string(),
            created_unix: 1,
        };
        let shown = forwarded(&alert);
        assert!(!shown.is_own());
        let packet: serde_json::Value =
            serde_json::from_str(&shown.to_kdeconnect_payload()).unwrap();
        assert_eq!(packet["body"]["app"], APP);
        assert_eq!(packet["body"]["title"], "Done: make");
        assert_eq!(packet["body"]["text"], "took 3 s");
    }
}
