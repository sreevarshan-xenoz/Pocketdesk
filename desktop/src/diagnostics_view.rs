//! The Diagnostics tab (F10): runs `core::local::diagnostics` off the UI
//! thread, lists what it found, and copies a debug report.

use std::path::Path;
use std::thread::JoinHandle;

use eframe::egui::{self, Color32};
use linux_link_core::local::config::Config;
use linux_link_core::local::diagnostics::{self, Finding, Report, Status};

const GOOD: Color32 = Color32::from_rgb(0x2e, 0x9e, 0x6a);
const WARN: Color32 = Color32::from_rgb(0xd9, 0x9a, 0x2b);
const BAD: Color32 = Color32::from_rgb(0xd9, 0x5c, 0x4a);
const MUTED: Color32 = Color32::from_rgb(0x8a, 0x93, 0x91);

#[derive(Default)]
pub struct DiagnosticsView {
    running: Option<JoinHandle<Report>>,
    report: Option<Report>,
    /// When the shown report was made, for the debug report's "ago".
    ran_at: u64,
}

impl DiagnosticsView {
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        state_dir: &Path,
        config: Option<&Config>,
        server_running: bool,
    ) {
        if self.running.as_ref().is_some_and(|job| job.is_finished())
            && let Some(job) = self.running.take()
        {
            self.report = job.join().ok();
            self.ran_at = now_unix();
        }
        ui.label(
            "Checks why a phone might not connect: the server, its ports, the firewall, \
             the addresses phones can use, and the last session.",
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let busy = self.running.is_some();
            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(if self.report.is_some() {
                        "Run again"
                    } else {
                        "Run checks"
                    }),
                )
                .clicked()
            {
                let dir = state_dir.to_path_buf();
                let config = config.cloned();
                self.running = Some(std::thread::spawn(move || {
                    run(&dir, config, server_running)
                }));
            }
            if busy {
                ui.spinner();
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(200));
            }
        });
        let Some(report) = &self.report else {
            return;
        };
        ui.add_space(8.0);
        egui::Grid::new("diagnostics")
            .num_columns(3)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                for finding in &report.findings {
                    finding_row(ui, finding);
                }
            });
        ui.add_space(10.0);
        ui.strong("Last session");
        match &report.last_session {
            None => {
                ui.label("None recorded yet.");
            }
            Some(s) => {
                ui.label(format!(
                    "{} ago, {} s, {}{}; encoder: {}",
                    diagnostics::human_ago(now_unix().saturating_sub(s.unix_secs)),
                    s.duration_secs,
                    s.outcome.replace('_', " "),
                    if s.ever_relayed { " (relayed)" } else { "" },
                    s.encoder.as_deref().unwrap_or("not recorded"),
                ));
                for (name, tail) in [
                    ("End to end", s.e2e),
                    ("Encode", s.encode),
                    ("Round trip", s.rtt),
                ] {
                    if let Some(t) = tail {
                        ui.label(format!(
                            "{name}: {} ms typical, {} ms at worst (p99)",
                            t.p50_ms, t.p99_ms
                        ));
                    }
                }
            }
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("Copy debug report").clicked() {
                let text = diagnostics::report_text(
                    report,
                    env!("CARGO_PKG_VERSION"),
                    std::env::consts::OS,
                    self.ran_at,
                );
                ui.ctx().copy_text(text);
            }
            ui.label(
                egui::RichText::new(
                    "For an issue. It includes this computer's local addresses, but no PINs, keys or file names.",
                )
                .color(MUTED),
            );
        });
    }
}

fn finding_row(ui: &mut egui::Ui, finding: &Finding) {
    let (color, mark) = match finding.status {
        Status::Good => (GOOD, "ok"),
        Status::Warn => (WARN, "check"),
        Status::Bad => (BAD, "problem"),
        Status::Info => (MUTED, "info"),
    };
    ui.label(egui::RichText::new(mark).color(color).strong());
    ui.strong(finding.check);
    ui.vertical(|ui| {
        ui.label(&finding.detail);
        if let Some(hint) = &finding.hint {
            ui.label(egui::RichText::new(hint).color(MUTED));
        }
    });
    ui.end_row();
}

/// Every check, as the worker thread runs them.
fn run(state_dir: &Path, config: Option<Config>, server_running: bool) -> Report {
    let config = config.unwrap_or_default();
    let mut findings = vec![
        diagnostics::check_server(server_running),
        diagnostics::check_control_port(config.control_port, server_running),
        diagnostics::check_stream_port(config.streaming_port, server_running),
    ];
    if let Some(firewall) = diagnostics::check_firewall(firewall_rules()) {
        findings.push(firewall);
    }
    findings.push(diagnostics::check_addresses(
        &linux_link_core::local::network::local_ips(),
    ));
    findings.push(diagnostics::check_internet_path(state_dir));
    Report {
        findings,
        last_session: diagnostics::last_session(state_dir),
        control_port: config.control_port,
        streaming_port: config.streaming_port,
        pairing_required: config.pairing_required,
    }
}

/// How many enabled inbound rules let phones reach the server (Windows): the
/// ones `linux-link install` adds, and program rules for linux-link.exe that
/// Windows adds when someone answers its "Allow access" prompt. `None` where
/// there is no such check.
#[cfg(windows)]
fn firewall_rules() -> Option<Result<usize, String>> {
    const QUERY: &str = r"$g = @(Get-NetFirewallRule -Group 'Pocketdesk' -Enabled True -Direction Inbound -Action Allow -ErrorAction SilentlyContinue); $a = @(Get-NetFirewallApplicationFilter -ErrorAction SilentlyContinue | Where-Object { $_.Program -like '*\linux-link.exe' } | Get-NetFirewallRule -ErrorAction SilentlyContinue | Where-Object { $_.Enabled -eq 'True' -and $_.Direction -eq 'Inbound' -and $_.Action -eq 'Allow' }); (@($g) + @($a) | Select-Object -Unique -Property Name).Count";
    let output = crate::restart::quiet(crate::restart::system32(
        "WindowsPowerShell\\v1.0\\powershell.exe",
    ))
    .args(["-NoProfile", "-NonInteractive", "-Command", QUERY])
    .output();
    Some(match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse::<usize>()
            .map_err(|_| "an answer that is not a count".to_string()),
        Ok(out) => Err(String::from_utf8_lossy(&out.stderr)
            .trim()
            .chars()
            .take(120)
            .collect()),
        Err(e) => Err(e.to_string()),
    })
}

#[cfg(not(windows))]
fn firewall_rules() -> Option<Result<usize, String>> {
    None
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    /// The tab's checks against this computer's real state and server.
    /// Ignored: it reads the live state directory and needs the server.
    #[test]
    #[ignore]
    fn the_checks_run_on_this_computer() {
        let dir = linux_link_core::local::paths::state_dir().unwrap();
        let config = linux_link_core::local::config::Config::load().ok();
        let running = linux_link_core::local::server::is_running(&dir);
        let report = super::run(&dir, config, running);
        println!(
            "{}",
            linux_link_core::local::diagnostics::report_text(
                &report,
                "test",
                std::env::consts::OS,
                super::now_unix()
            )
        );
    }
}
