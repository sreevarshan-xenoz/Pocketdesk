pub mod audio_control;
pub mod battery;
pub mod clipboard;
pub mod exec;
pub mod file_browse;
pub mod input;
pub mod links;
pub mod monitors;
pub mod notification;
pub mod notification_reply;
pub mod pair;
pub mod power;
pub mod privacy;
pub mod share;
pub mod siren;
pub mod sms;
pub mod wake_relay;
pub mod windows;

use pocketdesk_core::protocol::kdeconnect::PluginRegistry;

/// Shorthand for the full plugin registry type with all 16 plugins registered.
pub type PluginSet = PluginRegistry;

/// Register all 16 KDE Connect plugins and return the registry.
pub fn register_all() -> PluginSet {
    let mut registry = PluginRegistry::new();
    registry.register(battery::BatteryPlugin::new());
    registry.register(clipboard::ClipboardPlugin::new());
    registry.register(notification::NotificationPlugin::new());
    registry.register(share::SharePlugin::new());
    registry.register(input::InputPlugin::new());
    registry.register(file_browse::FileBrowsePlugin::new());
    registry.register(power::PowerPlugin::new());
    registry.register(exec::ExecPlugin::new());
    registry.register(monitors::MonitorsPlugin);
    registry.register(windows::WindowsPlugin);
    registry.register(siren::SirenPlugin::new());
    registry.register(notification_reply::NotificationReplyPlugin);
    registry.register(wake_relay::WakeRelayPlugin);
    registry.register(privacy::PrivacyPlugin);
    registry.register(audio_control::AudioControlPlugin);
    registry.register(pair::PairPlugin);
    registry.register(sms::SmsPlugin);
    registry.register(links::LinksPlugin::default());
    registry
}
