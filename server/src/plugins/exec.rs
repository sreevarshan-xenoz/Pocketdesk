use dashmap::DashMap;
use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

/// A command still running after this is stopped (ecosystem 1c).
const DEADLINE: Duration = Duration::from_secs(60);
/// The most of each of stdout and stderr sent back: its end.
const MAX_OUTPUT: usize = 32 * 1024;

/// The last `max` bytes of `bytes` as text, and whether any were left out.
pub(crate) fn tail_text(bytes: &[u8], max: usize) -> (String, bool) {
    if bytes.len() <= max {
        return (String::from_utf8_lossy(bytes).into_owned(), false);
    }
    (
        String::from_utf8_lossy(&bytes[bytes.len() - max..]).into_owned(),
        true,
    )
}

/// Run `cmd` to its end or `deadline`; `None` when it was stopped there.
async fn run(cmd: &str, deadline: Duration) -> std::io::Result<Option<std::process::Output>> {
    let child = shell(cmd)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    // Past the deadline the child is dropped, and so killed.
    match tokio::time::timeout(deadline, child.wait_with_output()).await {
        Ok(output) => output.map(Some),
        Err(_) => Ok(None),
    }
}

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
/// `System32\cmd.exe /S /C "command"` (located through the OS, never `PATH`), with no
/// console window and the command passed verbatim, since cmd does its own
/// parsing and Rust's argument quoting would corrupt it.
fn shell(cmd: &str) -> Command {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut command = Command::new(crate::platform::system32("cmd.exe"));
        // `/S` and the outer quotes: cmd strips just those, so a command
        // that starts with a quoted program and quotes more keeps them.
        command
            .raw_arg(format!("/S /C \"{cmd}\""))
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

                match run(cmd, DEADLINE).await {
                    Ok(Some(out)) => {
                        let (stdout, cut_out) = tail_text(&out.stdout, MAX_OUTPUT);
                        let (stderr, cut_err) = tail_text(&out.stderr, MAX_OUTPUT);
                        let exit_code = out.status.code().unwrap_or(-1);

                        let response =
                            NetworkPacket::new("kdeconnect.pocketdesk.exec").with_body(json!({
                                "stdout": stdout,
                                "stderr": stderr,
                                "exit_code": exit_code,
                                "truncated": cut_out || cut_err,
                            }));
                        sender.send_packet(&response).await?;
                    }
                    Ok(None) => {
                        let response =
                            NetworkPacket::new("kdeconnect.pocketdesk.exec").with_body(json!({
                                "stdout": "",
                                "stderr": format!("Stopped after {} s", DEADLINE.as_secs()),
                                "exit_code": -1,
                                "timed_out": true,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_past_the_cap_keeps_its_end() {
        assert_eq!(tail_text(b"abc", 8), ("abc".to_string(), false));
        assert_eq!(tail_text(b"0123456789", 4), ("6789".to_string(), true));
    }

    #[tokio::test]
    async fn a_command_past_its_deadline_is_stopped() {
        let slow = if cfg!(windows) {
            "ping -n 30 127.0.0.1 > NUL"
        } else {
            "sleep 30"
        };
        let started = std::time::Instant::now();
        assert!(
            run(slow, Duration::from_millis(500))
                .await
                .unwrap()
                .is_none()
        );
        assert!(started.elapsed() < Duration::from_secs(10));
        let quick = run("echo hi", Duration::from_secs(10))
            .await
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&quick.stdout).contains("hi"));
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

    /// A command naming a quoted program, with a quoted argument, runs as
    /// sent: cmd's own rule would strip its first and last quote.
    #[tokio::test]
    async fn a_command_starting_with_a_quoted_program_runs_as_sent() {
        let line = format!(
            "\"{}\" /C echo \"x y\"",
            crate::platform::system32("cmd.exe").display()
        );
        let out = shell(&line).output().await.unwrap();
        assert!(
            out.status.success(),
            "{:?}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "\"x y\"");
    }
}
