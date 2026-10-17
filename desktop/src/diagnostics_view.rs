//! The Diagnostics tab (F10): runs `core::local::diagnostics` off the UI
//! thread, lists what it found, and copies a debug report.

use std::path::Path;
use std::thread::JoinHandle;

use eframe::egui;
use pocketdesk_core::local::config::Config;
use pocketdesk_core::local::diagnostics::{self, Finding, Report, Status};

#[derive(Default)]
pub struct DiagnosticsView {
    running: Option<JoinHandle<Report>>,
    report: Option<Report>,
    /// When the shown report was made, for the debug report's "ago".
    ran_at: u64,
    /// The last run failed (the checks' thread ended without a report).
    failed: bool,
    /// When the debug report was last copied, for "Copied.".
    copied: Option<std::time::Instant>,
}

impl DiagnosticsView {
    pub fn show(&mut self, ui: &mut egui::Ui, state_dir: &Path, config: Option<&Config>) {
        if self.running.as_ref().is_some_and(|job| job.is_finished())
            && let Some(job) = self.running.take()
        {
            // A failed run keeps the last results and says so.
            match job.join() {
                Ok(report) => {
                    self.report = Some(report);
                    self.ran_at = now_unix();
                    self.failed = false;
                }
                Err(_) => self.failed = true,
            }
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
                self.running = Some(std::thread::spawn(move || run(&dir, config)));
            }
            if self.failed {
                ui.label(
                    egui::RichText::new("The checks failed to run.").color(crate::theme::bad(ui)),
                );
            }
            if busy {
                ui.spinner();
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(200));
            }
        });
        // Said with or without a report: it is about this app, not the
        // checks (U1 review: it showed only after a run).
        ui.label(
            egui::RichText::new(crate::fonts::summary(crate::fonts::loaded()))
                .color(crate::theme::muted(ui)),
        );
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
                self.copied = Some(std::time::Instant::now());
            }
            crate::notice::copied(ui, &mut self.copied);
        });
        ui.label(
            egui::RichText::new(
                "For an issue. It includes this computer's addresses (local, and any public or Tailscale ones), but no PINs, keys or file names.",
            )
            .color(crate::theme::muted(ui)),
        );
    }
}

fn finding_row(ui: &mut egui::Ui, finding: &Finding) {
    let (color, mark) = match finding.status {
        Status::Good => (crate::theme::good(ui), "ok"),
        Status::Warn => (crate::theme::warn(ui), "check"),
        Status::Bad => (crate::theme::bad(ui), "problem"),
        Status::Info => (crate::theme::muted(ui), "info"),
    };
    ui.label(egui::RichText::new(mark).color(color).strong());
    ui.strong(finding.check);
    ui.vertical(|ui| {
        ui.label(&finding.detail);
        if let Some(hint) = &finding.hint {
            ui.label(egui::RichText::new(hint).color(crate::theme::muted(ui)));
        }
    });
    ui.end_row();
}

/// Every check, as the worker thread runs them.
fn run(state_dir: &Path, config: Option<Config>) -> Report {
    let config = config.unwrap_or_default();
    let state = diagnostics::server_state(state_dir);
    let mut findings = vec![
        diagnostics::check_server(state),
        diagnostics::check_control_port(config.control_port, state),
        diagnostics::check_stream_port(config.streaming_port, state),
    ];
    if let Some(firewall) = diagnostics::check_firewall(firewall_state(state_dir)) {
        findings.push(firewall);
    }
    findings.push(diagnostics::check_addresses(
        &pocketdesk_core::local::network::local_ips(),
    ));
    findings.push(diagnostics::check_internet_path(
        state_dir,
        state,
        &config.relay_urls,
    ));
    let wan_status = diagnostics::read_wan_status(state_dir);
    findings.extend(diagnostics::check_outside(
        wan_status.as_ref(),
        state,
        !config.relay_urls.is_empty(),
    ));
    findings.push(diagnostics::check_dht(wan_status.as_ref(), state));
    // N6: the adapter a phone on this network would wake it through.
    let wake = pocketdesk_core::local::wake::wake_facts(None);
    let allowed = wake
        .as_ref()
        .map(|facts| wake_allowed(&facts.adapter))
        .unwrap_or(diagnostics::WakeAllowed::Unknown);
    findings.push(diagnostics::check_wake(
        wake.as_ref(),
        allowed,
        config.wake_announce,
    ));
    Report {
        findings,
        last_session: diagnostics::last_session(state_dir),
        control_port: config.control_port,
        streaming_port: config.streaming_port,
        pairing_required: config.pairing_required,
    }
}

/// The firewall as it applies to phones (Windows): the enabled inbound
/// rules for the server that apply to the active network profile, Allow and
/// Block. The rules are `pocketdesk install`'s Pocketdesk group, and program
/// rules for the running server's own exe (Windows adds those when someone
/// answers its prompt). `None` where there is no such check.
#[cfg(windows)]
fn firewall_state(state_dir: &Path) -> Option<Result<diagnostics::FirewallState, String>> {
    // The pid is a number, so it goes into the script as one.
    let exe = pocketdesk_core::local::server::server_pid(state_dir)
        .filter(|_| pocketdesk_core::local::server::is_running(state_dir))
        .map(|pid| format!("(Get-Process -Id {pid}).Path"))
        .unwrap_or_else(|| "''".to_string());
    let script = format!("$Exe = {exe}; {FIREWALL_QUERY}");
    let powershell = crate::restart::system32("WindowsPowerShell\\v1.0\\powershell.exe");
    let answer = pocketdesk_core::local::network::output_within(
        &powershell.to_string_lossy(),
        &["-NoProfile", "-NonInteractive", "-Command", &script],
        std::time::Duration::from_secs(10),
    );
    Some(match answer {
        Some(out) => diagnostics::parse_firewall(&String::from_utf8_lossy(&out)),
        None => Err("no answer from PowerShell within 10 s".to_string()),
    })
}

/// Counts, for `$Exe`, the rules that apply to the active profiles:
/// "allow,block,profiles".
#[cfg(windows)]
const FIREWALL_QUERY: &str = r"$ErrorActionPreference = 'SilentlyContinue'; $profiles = @(Get-NetConnectionProfile | ForEach-Object { $c = $_.NetworkCategory.ToString(); if ($c -eq 'DomainAuthenticated') { 'Domain' } else { $c } } | Select-Object -Unique); $rules = @(Get-NetFirewallRule -Group 'Pocketdesk' -Enabled True -Direction Inbound); if ($Exe) { $rules += @(Get-NetFirewallApplicationFilter | Where-Object { $_.Program -eq $Exe } | Get-NetFirewallRule | Where-Object { $_.Enabled -eq 'True' -and $_.Direction -eq 'Inbound' }) }; $rules = @($rules | Sort-Object -Property Name -Unique); $applies = @($rules | Where-Object { $p = $_.Profile.ToString(); $p -eq 'Any' -or @($profiles | Where-Object { $p -match $_ }).Count -gt 0 }); '{0},{1},{2}' -f @($applies | Where-Object { $_.Action -eq 'Allow' }).Count, @($applies | Where-Object { $_.Action -eq 'Block' }).Count, ($profiles -join '/')";

/// Whether `adapter` lets a magic packet wake this computer: Windows'
/// `Get-NetAdapterPowerManagement`.
#[cfg(windows)]
fn wake_allowed(adapter: &str) -> diagnostics::WakeAllowed {
    let name = single_quoted(adapter);
    // Some drivers (Realtek here) fail the power-management query; NDIS's
    // standard *WakeOnMagicPacket keyword answers for them.
    let script = format!(
        "$ErrorActionPreference = 'SilentlyContinue'; $v = (Get-NetAdapterPowerManagement -Name '{name}').WakeOnMagicPacket; if (-not $v) {{ $k = (Get-NetAdapterAdvancedProperty -Name '{name}' -RegistryKeyword '*WakeOnMagicPacket').RegistryValue; if ($k -eq '1') {{ $v = 'Enabled' }} elseif ($k -eq '0') {{ $v = 'Disabled' }} }}; $v"
    );
    let powershell = crate::restart::system32("WindowsPowerShell\\v1.0\\powershell.exe");
    pocketdesk_core::local::network::output_within(
        &powershell.to_string_lossy(),
        &["-NoProfile", "-NonInteractive", "-Command", &script],
        std::time::Duration::from_secs(10),
    )
    .map(|out| diagnostics::parse_wake_allowed(&String::from_utf8_lossy(&out)))
    .unwrap_or(diagnostics::WakeAllowed::Unknown)
}

/// The text of a PowerShell single-quoted string: each quote PowerShell
/// reads as one (the ASCII quote and U+2018-U+201B) is doubled.
#[cfg(any(windows, test))]
fn single_quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
        out.push(c);
    }
    out
}

/// Whether `adapter` lets a magic packet wake this computer: `ethtool`'s
/// `Wake-on:` line. Without ethtool it is not known: sysfs' power/wakeup
/// says only that the device may wake the system, not by magic packet.
#[cfg(not(windows))]
fn wake_allowed(adapter: &str) -> diagnostics::WakeAllowed {
    pocketdesk_core::local::network::output_within(
        "ethtool",
        &[adapter],
        std::time::Duration::from_secs(3),
    )
    .map(|out| diagnostics::parse_wake_allowed(&String::from_utf8_lossy(&out)))
    .unwrap_or(diagnostics::WakeAllowed::Unknown)
}

#[cfg(not(windows))]
fn firewall_state(_state_dir: &Path) -> Option<Result<diagnostics::FirewallState, String>> {
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
    #[test]
    fn every_quote_powershell_reads_is_doubled() {
        assert_eq!(super::single_quoted("Ethernet 2"), "Ethernet 2");
        assert_eq!(super::single_quoted("Bob's"), "Bob''s");
        assert_eq!(
            super::single_quoted("a\u{2019}b\u{2018}c"),
            "a\u{2019}\u{2019}b\u{2018}\u{2018}c"
        );
    }

    /// The tab's checks against this computer's real state and server.
    /// Ignored: it reads the live state directory and needs the server.
    #[test]
    #[ignore]
    fn the_checks_run_on_this_computer() {
        let dir = pocketdesk_core::local::paths::state_dir().unwrap();
        let config = pocketdesk_core::local::config::Config::load().ok();
        let report = super::run(&dir, config);
        println!(
            "{}",
            pocketdesk_core::local::diagnostics::report_text(
                &report,
                "test",
                std::env::consts::OS,
                super::now_unix()
            )
        );
    }
}
