use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
#[cfg(not(windows))]
use tokio::process::Command;

/// Power management plugin — handles sleep, shutdown, and restart commands.
///
/// Listens for `kdeconnect.pocketdesk.power` packets and executes the
/// corresponding systemctl command on the host (on Windows: the power APIs,
/// see [`windows_power`]).
pub struct PowerPlugin;

impl Default for PowerPlugin {
    fn default() -> Self {
        Self
    }
}

impl PowerPlugin {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Plugin for PowerPlugin {
    fn name(&self) -> &'static str {
        "power"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.pocketdesk.power"]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[]
    }

    async fn handle_packet(
        &self,
        packet: &NetworkPacket,
        _sender: &dyn DeviceSender,
    ) -> Result<()> {
        if packet.packet_type.as_str() == "kdeconnect.pocketdesk.power" {
            let body = &packet.body;
            let action = body.get("action").and_then(|v| v.as_str()).unwrap_or("");

            #[cfg(windows)]
            if let Some(action) = windows_power::Action::parse(action) {
                return tokio::task::spawn_blocking(move || windows_power::perform(action))
                    .await
                    .map_err(|e| pocketdesk_core::error::PocketdeskError::Other {
                        detail: e.to_string(),
                    })?;
            }
            match action {
                #[cfg(not(windows))]
                "sleep" => execute_systemctl("suspend").await,
                #[cfg(not(windows))]
                "shutdown" => execute_systemctl("poweroff").await,
                #[cfg(not(windows))]
                "restart" => execute_systemctl("reboot").await,
                #[cfg(not(windows))]
                "hibernate" => execute_systemctl("hibernate").await,
                other => {
                    tracing::warn!("Unknown power action: {other}");
                    Ok(())
                }
            }
        } else {
            Ok(())
        }
    }
}

/// Execute a systemctl command with `pkexec` for privilege escalation
/// (falls back to direct systemctl for passwordless sudo configurations).
#[cfg(not(windows))]
async fn execute_systemctl(action: &str) -> Result<()> {
    tracing::info!("Executing power action: {action}");

    // Try pkexec first (Ubuntu/GNOME default), then fall back to direct systemctl
    let pkexec_result = match Command::new("pkexec").arg("systemctl").arg(action).spawn() {
        Ok(mut child) => match child.wait().await {
            Ok(status) => Some(status.success()),
            Err(e) => {
                tracing::warn!("pkexec wait error for '{action}': {e}");
                None
            }
        },
        Err(e) => {
            tracing::warn!("pkexec not available for '{action}': {e}");
            None
        }
    };

    match pkexec_result {
        Some(true) => {
            tracing::info!("Power action '{action}' succeeded via pkexec");
            return Ok(());
        }
        _ => {
            // pkexec failed or unavailable — try direct systemctl
            tracing::warn!("pkexec failed for '{action}', trying direct systemctl");
        }
    }

    // Fallback: try direct systemctl
    let output = Command::new("systemctl")
        .arg(action)
        .output()
        .await
        .map_err(|e| pocketdesk_core::error::PocketdeskError::Io {
            operation: "systemctl",
            detail: e.to_string(),
        })?;

    if output.status.success() {
        tracing::info!("Power action '{action}' succeeded via direct systemctl");
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(pocketdesk_core::error::PocketdeskError::Other {
            detail: format!("Power action '{action}' failed: {stderr}"),
        })
    }
}

/// The Windows power actions. Sleep and hibernate are `SetSuspendState`;
/// shutdown and restart are `ExitWindowsEx`, after enabling the
/// `SeShutdownPrivilege` every interactive user holds but has disabled.
#[cfg(windows)]
mod windows_power {
    use pocketdesk_core::error::{PocketdeskError, Result};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID};
    use windows::Win32::Security::{
        AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
        SE_SHUTDOWN_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES,
    };
    use windows::Win32::System::Power::SetSuspendState;
    use windows::Win32::System::Shutdown::{
        EWX_POWEROFF, EWX_REBOOT, EWX_SHUTDOWN, EXIT_WINDOWS_FLAGS, ExitWindowsEx,
        SHTDN_REASON_FLAG_PLANNED, SHTDN_REASON_MAJOR_OTHER,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::core::PCWSTR;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum Action {
        Sleep,
        Hibernate,
        Shutdown,
        Restart,
    }

    impl Action {
        pub(super) fn parse(name: &str) -> Option<Self> {
            match name {
                "sleep" => Some(Action::Sleep),
                "hibernate" => Some(Action::Hibernate),
                "shutdown" => Some(Action::Shutdown),
                "restart" => Some(Action::Restart),
                _ => None,
            }
        }

        /// `ExitWindowsEx` flags for the actions that end the session.
        pub(super) fn exit_flags(self) -> Option<EXIT_WINDOWS_FLAGS> {
            match self {
                Action::Shutdown => Some(EWX_SHUTDOWN | EWX_POWEROFF),
                Action::Restart => Some(EWX_REBOOT),
                Action::Sleep | Action::Hibernate => None,
            }
        }
    }

    fn other(detail: String) -> PocketdeskError {
        PocketdeskError::Other { detail }
    }

    fn enable_shutdown_privilege() -> Result<()> {
        let mut token = HANDLE::default();
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &mut token) }
            .map_err(|e| other(format!("opening the process token: {e}")))?;
        let mut luid = LUID::default();
        let looked_up =
            unsafe { LookupPrivilegeValueW(PCWSTR::null(), SE_SHUTDOWN_NAME, &mut luid) };
        let adjusted = looked_up.and_then(|()| {
            let privileges = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES {
                    Luid: luid,
                    Attributes: SE_PRIVILEGE_ENABLED,
                }],
            };
            unsafe { AdjustTokenPrivileges(token, false, Some(&privileges), 0, None, None) }
        });
        let _ = unsafe { CloseHandle(token) };
        adjusted.map_err(|e| other(format!("enabling SeShutdownPrivilege: {e}")))
    }

    pub(super) fn perform(action: Action) -> Result<()> {
        tracing::info!("Executing power action: {action:?}");
        match action.exit_flags() {
            Some(flags) => {
                enable_shutdown_privilege()?;
                unsafe {
                    ExitWindowsEx(flags, SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_FLAG_PLANNED)
                }
                .map_err(|e| other(format!("power action {action:?} failed: {e}")))
            }
            None => {
                let hibernate = action == Action::Hibernate;
                if unsafe { SetSuspendState(hibernate, false, false) } {
                    Ok(())
                } else {
                    Err(other(format!(
                        "power action {action:?} failed: {}",
                        windows::core::Error::from_thread()
                    )))
                }
            }
        }
    }

    // Mapping only: per the project rule, nothing here ever calls `perform`.
    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn every_phone_action_has_a_windows_meaning() {
            for (name, action) in [
                ("sleep", Action::Sleep),
                ("hibernate", Action::Hibernate),
                ("shutdown", Action::Shutdown),
                ("restart", Action::Restart),
            ] {
                assert_eq!(Action::parse(name), Some(action));
            }
            assert_eq!(Action::parse("reboot-now"), None);
        }

        #[test]
        fn only_shutdown_and_restart_end_the_session() {
            assert_eq!(Action::Sleep.exit_flags(), None);
            assert_eq!(Action::Hibernate.exit_flags(), None);
            assert_eq!(
                Action::Shutdown.exit_flags(),
                Some(EWX_SHUTDOWN | EWX_POWEROFF)
            );
            assert_eq!(Action::Restart.exit_flags(), Some(EWX_REBOOT));
        }
    }
}
