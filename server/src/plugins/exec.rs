use dashmap::DashMap;
use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use serde_json::json;
use std::path::PathBuf;
use tokio::process::Command;

/// Remote command execution plugin.
///
/// Allows running predefined commands on the host system.
///
/// NOTE: For security, in a production environment, this should only allow
/// executing scripts from a specific whitelist directory.
pub struct ExecPlugin {
    #[allow(dead_code)]
    commands: DashMap<String, PathBuf>,
}

impl ExecPlugin {
    pub fn new() -> Self {
        Self {
            commands: DashMap::new(),
        }
    }
}

impl Default for ExecPlugin {
    fn default() -> Self {
        Self::new()
    }
}

/// The command-line shell: `sh -c` on Unix; on Windows the real
/// `System32\cmd.exe /C` (located through the OS, never `PATH`), with no
/// console window and the command passed verbatim, since cmd does its own
/// parsing and Rust's argument quoting would corrupt it.
fn shell(cmd: &str) -> Command {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut command = Command::new(crate::platform::system32("cmd.exe"));
        command
            .arg("/C")
            .raw_arg(cmd)
            .creation_flags(CREATE_NO_WINDOW);
        command
    }
    #[cfg(not(windows))]
    {
        let mut command = Command::new("sh");
        command.arg("-c").arg(cmd);
        command
    }
}

#[async_trait::async_trait]
impl Plugin for ExecPlugin {
    fn name(&self) -> &'static str {
        "exec"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.pocketdesk.exec"]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.pocketdesk.exec"]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type.as_str() == "kdeconnect.pocketdesk.exec" {
            let command = packet.body.get("command").and_then(|v| v.as_str());

            if let Some(cmd) = command {
                tracing::info!("Executing remote command: {}", cmd);

                let output = shell(cmd).output().await;

                match output {
                    Ok(out) => {
                        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                        let exit_code = out.status.code().unwrap_or(-1);

                        let response =
                            NetworkPacket::new("kdeconnect.pocketdesk.exec").with_body(json!({
                                "stdout": stdout,
                                "stderr": stderr,
                                "exit_code": exit_code,
                            }));
                        sender.send_packet(&response).await?;
                    }
                    Err(e) => {
                        let response =
                            NetworkPacket::new("kdeconnect.pocketdesk.exec").with_body(json!({
                                "stdout": "",
                                "stderr": format!("Execution error: {e}"),
                                "exit_code": -1,
                            }));
                        sender.send_packet(&response).await?;
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    /// cmd.exe sees the command exactly as typed: quotes, `&` chaining and
    /// all — the reason for `raw_arg`.
    #[tokio::test]
    async fn cmd_receives_the_command_verbatim() {
        let out = shell(r#"echo "a  b"&echo c"#).output().await.unwrap();
        assert!(out.status.success());
        let text = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
        assert_eq!(text, "\"a  b\"\nc\n");
    }
}
