//! The apps a phone lists for its computer (P2): `kdeconnect.pocketdesk.
//! phone_apps {apps}`, answering O2's `apps` ask, kept owner-only in
//! `<state>/phone_apps.json` for the desktop app's Apps page. Beside it
//! (Apps part 1): `phone_app_icons {icons}`, the icons the computer did not
//! have at that version, kept in `<state>/phone_app_icons/`, and
//! `phone_app_opened {about, app?, result}`, how an ask went, kept in
//! `<state>/phone_app_results.json`. All held to the phone's Phone actions
//! permission, as the ask is.

use base64::Engine;
use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use pocketdesk_core::protocol::permissions::Capability;
use pocketdesk_core::protocol::phone_action::{
    AppReport, MAX_ICONS_PER_PACKET, PHONE_APP_ICONS, PHONE_APP_OPENED, PHONE_APPS, parse_apps,
    valid_app_id,
};

#[derive(Debug, Default)]
pub struct PhoneAppsPlugin;

/// The longest base64 text an icon may be: [`MAX_ICON_BYTES`] encoded.
///
/// [`MAX_ICON_BYTES`]: pocketdesk_core::local::notification_icons::MAX_ICON_BYTES
const MAX_ICON_BASE64: usize =
    pocketdesk_core::local::notification_icons::MAX_ICON_BYTES / 3 * 4 + 4;

/// One writer at a time to these small files (P2 review: two lists at once
/// could lose one).
static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The icons in an [`PHONE_APP_ICONS`] body: an app that is a package name,
/// a version, and a PNG that decodes from base64 within the size limit;
/// at most [`MAX_ICONS_PER_PACKET`].
fn icons_of(body: &serde_json::Value) -> Vec<(String, u64, Vec<u8>)> {
    body.get("icons")
        .and_then(|i| i.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .take(MAX_ICONS_PER_PACKET)
        .filter_map(|icon| {
            let id = icon.get("id")?.as_str().filter(|id| valid_app_id(id))?;
            let version = icon.get("v").and_then(|v| v.as_u64()).unwrap_or(0);
            let text = icon
                .get("png")?
                .as_str()
                .filter(|t| t.len() <= MAX_ICON_BASE64)?;
            let png = base64::engine::general_purpose::STANDARD
                .decode(text)
                .ok()?;
            Some((id.to_string(), version, png))
        })
        .collect()
}

#[async_trait::async_trait]
impl Plugin for PhoneAppsPlugin {
    fn name(&self) -> &'static str {
        "phone_apps"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[PHONE_APPS, PHONE_APP_ICONS, PHONE_APP_OPENED]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        let kind = packet.packet_type.as_str();
        if ![PHONE_APPS, PHONE_APP_ICONS, PHONE_APP_OPENED].contains(&kind) {
            return Ok(());
        }
        let device = sender.device_id().to_string();
        if !crate::plugins::pair::allows(&device, Capability::PhoneActions) {
            tracing::debug!("{kind} from {device} was ignored: Phone actions is off");
            return Ok(());
        }
        let Ok(dir) = crate::state::state_dir() else {
            return Ok(());
        };
        if kind == PHONE_APPS {
            let apps = parse_apps(&packet.body);
            // Phone apps tier 2: whether the phone opens apps in windows of
            // its own, as it said with this list.
            let windows = pocketdesk_core::protocol::phone_action::parse_windows_mode(&packet.body);
            let count = apps.len();
            // Off the runtime: a read and a write of a small file.
            let kept = tokio::task::spawn_blocking(move || {
                let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
                let versions = apps.iter().map(|a| (a.id.clone(), a.version)).collect();
                pocketdesk_core::local::phone_app_icons::forget_unlisted(&dir, &device, &versions);
                pocketdesk_core::local::phone_apps::keep_with_windows(
                    &dir,
                    &device,
                    apps,
                    pocketdesk_core::local::pin::now_unix(),
                    windows,
                    &|id| crate::plugins::pair::is_paired_device(id),
                )
            })
            .await;
            match kept {
                Ok(Ok(())) => tracing::info!("A phone listed {count} apps"),
                Ok(Err(e)) => tracing::warn!("A phone's apps were not kept: {e}"),
                Err(_) => {}
            }
            return Ok(());
        }
        if kind == PHONE_APP_ICONS {
            let icons = icons_of(&packet.body);
            let sent = icons.len();
            let kept = tokio::task::spawn_blocking(move || {
                let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
                icons
                    .into_iter()
                    .filter(|(id, version, png)| {
                        pocketdesk_core::local::phone_app_icons::keep(
                            &dir, &device, id, *version, png,
                        )
                        .is_ok()
                    })
                    .count()
            })
            .await
            .unwrap_or(0);
            tracing::info!("A phone sent {sent} app icons, {kept} kept");
            return Ok(());
        }
        // PHONE_APP_OPENED: how an ask went.
        let Some(report) = AppReport::from_body(&packet.body) else {
            tracing::debug!("A report of an ask from {device} was ignored: not one");
            return Ok(());
        };
        let result = report.result.as_str();
        let kept = tokio::task::spawn_blocking(move || {
            let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
            pocketdesk_core::local::phone_apps::keep_result(
                &dir,
                &device,
                report,
                pocketdesk_core::local::pin::now_unix(),
                &|id| crate::plugins::pair::is_paired_device(id),
            )
        })
        .await;
        match kept {
            Ok(Ok(())) => tracing::info!("A phone said how an ask for its apps went: {result}"),
            Ok(Err(e)) => tracing::warn!("A phone's report on an ask was not kept: {e}"),
            Err(_) => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(side: u32) -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
        bytes.extend_from_slice(&[0, 0, 0, 13]);
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&side.to_be_bytes());
        bytes.extend_from_slice(&side.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes
    }

    fn text(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    /// An icons packet gives the icons with a package name and base64, no more
    /// than a packet holds; anything else in it is passed over.
    #[test]
    fn an_icons_packet_is_read_with_care() {
        let body = serde_json::json!({"icons": [
            {"id": "org.example.a", "v": 4, "png": text(&png(64))},
            {"id": "bad id", "v": 1, "png": text(&png(64))},
            {"id": "org.example.b", "v": 1, "png": "not base64 !!"},
            {"id": "org.example.c", "png": text(&png(64))},
            {"v": 1, "png": text(&png(64))},
            {"id": "org.example.d", "v": 2, "png": "x".repeat(MAX_ICON_BASE64 + 1)},
        ]});
        let icons = icons_of(&body);
        assert_eq!(icons.len(), 2);
        assert_eq!((icons[0].0.as_str(), icons[0].1), ("org.example.a", 4));
        assert_eq!((icons[1].0.as_str(), icons[1].1), ("org.example.c", 0));
        let many: Vec<_> = (0..MAX_ICONS_PER_PACKET + 10)
            .map(|i| serde_json::json!({"id": format!("org.example.n{i}"), "v": 1, "png": text(&png(8))}))
            .collect();
        assert_eq!(
            icons_of(&serde_json::json!({ "icons": many })).len(),
            MAX_ICONS_PER_PACKET
        );
        assert!(icons_of(&serde_json::json!({})).is_empty());
    }
}
