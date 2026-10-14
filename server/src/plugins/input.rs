use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::input_injector::{InputInjector, button_id_to_mouse, key_name_to_enigo_key};

/// Input plugin with native input injection via enigo
pub struct InputPlugin {
    injector: Arc<Mutex<Option<InputInjector>>>,
}

impl InputPlugin {
    pub fn new() -> Self {
        Self {
            injector: Arc::new(Mutex::new(None)),
        }
    }

    /// Lazily initialize the injector
    async fn get_injector(&self) -> Result<Arc<Mutex<Option<InputInjector>>>> {
        let mut opt = self.injector.lock().await;
        if opt.is_none() {
            *opt = Some(InputInjector::new().map_err(|e| {
                pocketdesk_core::error::PocketdeskError::Other {
                    detail: format!("Failed to create input injector: {e}"),
                }
            })?);
            debug!("Input injector initialized on first use");
        }
        Ok(self.injector.clone())
    }
}

impl Default for InputPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Plugin for InputPlugin {
    fn name(&self) -> &'static str {
        "input"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.mousepad.request", "kdeconnect.presenter"]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.mousepad.echo"]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        // Several phones (N7): while another phone drives, this one's mouse
        // and keyboard packets are dropped here too, as on the stream.
        if !crate::driving::may_drive(sender.device_id()) {
            debug!(
                "Dropped {} from {}: another phone drives",
                packet.packet_type,
                sender.device_id()
            );
            return Ok(());
        }
        match packet.packet_type.as_str() {
            "kdeconnect.mousepad.request" => {
                let body = &packet.body;

                // Handle keyboard input first (text)
                if let Some(text) = body.get("text").and_then(|v| v.as_str()) {
                    let _ = self.type_text(text).await;
                }

                // Handle special key
                if let Some(key) = body.get("key").and_then(|v| v.as_str()) {
                    let _ = self.press_key(key).await;
                }

                // Handle mouse movement
                if let (Some(x), Some(y)) = (
                    body.get("dx").and_then(|v| v.as_f64()),
                    body.get("dy").and_then(|v| v.as_f64()),
                ) && (x != 0.0 || y != 0.0)
                {
                    let _ = self.move_mouse(x as i32, y as i32).await;
                }

                // Handle mouse button events
                if let Some(is_pressed) = body.get("isPressed").and_then(|v| v.as_bool())
                    && let Some(button) = body.get("button").and_then(|v| v.as_i64())
                {
                    let _ = self.mouse_button(button as i32, is_pressed).await;
                }

                // Echo back for mousepad protocol
                let echo = NetworkPacket::new("kdeconnect.mousepad.echo");
                sender.send_packet(&echo).await?;
            }
            "kdeconnect.presenter" => {
                // Presenter remote - handle play/pause/next/previous
                if let Some(action) = packet.body.get("action").and_then(|v| v.as_str()) {
                    let _ = self.handle_presenter_action(action).await;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl InputPlugin {
    /// Move the mouse by relative amounts.
    async fn move_mouse(&self, dx: i32, dy: i32) -> Result<()> {
        let injector = self.get_injector().await?;
        let mut inj = injector.lock().await;
        let inj = inj
            .as_mut()
            .ok_or_else(|| pocketdesk_core::error::PocketdeskError::Other {
                detail: "InputInjector not initialized".to_string(),
            })?;
        inj.move_mouse_relative(dx, dy).map_err(Into::into)
    }

    /// Press or release a mouse button.
    async fn mouse_button(&self, button: i32, pressed: bool) -> Result<()> {
        let injector = self.get_injector().await?;
        let mut inj = injector.lock().await;
        let inj = inj
            .as_mut()
            .ok_or_else(|| pocketdesk_core::error::PocketdeskError::Other {
                detail: "InputInjector not initialized".to_string(),
            })?;
        let mouse_key = button_id_to_mouse(button);
        inj.mouse_button(mouse_key, pressed).map_err(Into::into)
    }

    /// Type text using native input.
    async fn type_text(&self, text: &str) -> Result<()> {
        let injector = self.get_injector().await?;
        let mut inj = injector.lock().await;
        let inj = inj
            .as_mut()
            .ok_or_else(|| pocketdesk_core::error::PocketdeskError::Other {
                detail: "InputInjector not initialized".to_string(),
            })?;
        inj.text(text).map_err(Into::into)
    }

    /// Handle special keys (Enter, Escape, arrows, etc.).
    async fn press_key(&self, key: &str) -> Result<()> {
        let injector = self.get_injector().await?;
        let mut inj = injector.lock().await;
        let inj = inj
            .as_mut()
            .ok_or_else(|| pocketdesk_core::error::PocketdeskError::Other {
                detail: "InputInjector not initialized".to_string(),
            })?;
        let enigo_key = key_name_to_enigo_key(key);
        inj.key(enigo_key, true)
            .map_err(|e| pocketdesk_core::error::PocketdeskError::Other {
                detail: e.to_string(),
            })?;
        inj.key(enigo_key, false)
            .map_err(|e| pocketdesk_core::error::PocketdeskError::Other {
                detail: e.to_string(),
            })
    }

    /// Handle presenter remote actions.
    async fn handle_presenter_action(&self, action: &str) -> Result<()> {
        let injector = self.get_injector().await?;
        let mut inj = injector.lock().await;
        if let Some(ref mut injector) = *inj {
            match action {
                "next" | "previous" | "prev" => {
                    let key = if action == "next" {
                        Key::RightArrow
                    } else {
                        Key::LeftArrow
                    };
                    injector.key(key, true).ok();
                    injector.key(key, false).ok();
                }
                "play" | "pause" => {
                    injector.key(Key::Space, true).ok();
                    injector.key(Key::Space, false).ok();
                }
                other => {
                    warn!("Unknown presenter action: {}", other);
                }
            }
        }
        Ok(())
    }
}

use enigo::Key;
