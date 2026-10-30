//! The Diagnostics tab (F10): runs `core::local::diagnostics` off the UI
//! thread, lists what it found, and copies a debug report.

use std::path::Path;
use std::thread::JoinHandle;

use eframe::egui;
use pocketdesk_core::local::config::Config;
use pocketdesk_core::local::diagnostics::{self, Finding, Report, Status};
use pocketdesk_core::local::shared_input::LatencyReport;

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
    /// The checks were started once by opening the page (I3).
    started: bool,
    /// The shared keyboard and mouse's last measured latency
    /// (`pocketdesk share-test`), read with each run.
    latency: Option<LatencyReport>,
    /// Another page asked for the checks (UX 1: a failed pairing's "Run
    /// diagnostics").
    asked: bool,
}

impl DiagnosticsView {
    /// Run the checks as the page next shows (UX 1).
    pub fn run_now(&mut self) {
        self.asked = true;
    }

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
                    self.latency = pocketdesk_core::local::shared_input::read_latency(state_dir);
                }
                Err(_) => self.failed = true,
            }
        }
        // The checks run as the page first opens (I3): it is opened to see
        // them. Not under tests, where they would start the firewall probe.
        let open_run = (self.asked || (!self.started && self.report.is_none()))
            && self.running.is_none()
            && !cfg!(test);
        self.started = true;
        self.asked = false;
        ui.horizontal(|ui| {
            let busy = self.running.is_some();
            if open_run
                || ui
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
                // Its spinner now, not at the app's next refresh (I3 review).
                ui.ctx().request_repaint();
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
        if let Some(fonts) = crate::fonts::summary(crate::fonts::loaded()) {
            ui.label(egui::RichText::new(fonts).color(crate::theme::muted(ui)));
        }
        let Some(report) = &self.report else {
            // UX 4: what a run shows, before the first.
            crate::widgets::empty(
                ui,
                if self.running.is_some() {
                    CHECKING
                } else {
                    NOT_RUN
                },
                "They look at the server, its ports, the firewall and this network, \
                 and say what to fix.",
                |_| {},
            );
            return;
        };
        ui.add_space(8.0);
        // A block per finding, wrapping at a narrow width (I3).
        for finding in &report.findings {
            finding_row(ui, finding);
            ui.add_space(6.0);
        }
        ui.add_space(10.0);
        crate::theme::section(ui, "Last session");
        match &report.last_session {
            None => {
                ui.label("None recorded yet.");
            }
            Some(s) => {
                ui.label(format!(
                    "{}, {} s, {}{}; encoder: {}",
                    crate::words::ago(now_unix(), s.unix_secs),
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
        // Only once one was measured: most owners never run the test.
        if let Some(latency) = &self.latency {
            ui.add_space(10.0);
            crate::theme::section(ui, "Keyboard and mouse to another computer");
            for line in latency_lines(latency, now_unix()) {
                ui.label(line);
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

/// Before the first run, and during it (UX 4).
pub const NOT_RUN: &str = "No checks run yet";
pub const CHECKING: &str = "Running the checks";

/// The last measured latency, in words: when, then each share, in
/// milliseconds.
fn latency_lines(l: &LatencyReport, now: u64) -> [String; 3] {
    let ms = |us: u64| format!("{:.2} ms", us as f64 / 1000.0);
    [
        format!(
            "Measured {}, over {} keys and moves.",
            crate::words::ago(now, l.measured_unix),
            l.samples
        ),
        format!(
            "Pocketdesk: {} typical, {} at worst (p95).",
            ms(l.ours_p50_us),
            ms(l.ours_p95_us)
        ),
        format!(
            "The network: {} typical, {} at worst (p95), one way.",
            ms(l.network_p50_us),
            ms(l.network_p95_us)
        ),
    ]
}

fn finding_row(ui: &mut egui::Ui, finding: &Finding) {
    let (color, mark) = status_chip(ui, finding.status);
    ui.horizontal(|ui| {
        // The status as a filled chip (I3): a word in colour read as text.
        egui::Frame::new()
            .fill(color)
            .corner_radius(6.0)
            .inner_margin(egui::Margin::symmetric(8, 2))
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(mark)
                        .color(crate::theme::on_chip(ui, color))
                        .strong(),
                );
            });
        ui.strong(finding.check);
    });
    ui.label(plain(&finding.detail));
    if let Some(hint) = &finding.hint {
        ui.label(egui::RichText::new(plain(hint)).color(crate::theme::muted(ui)));
    }
}

/// A finding's words without their Markdown backticks: the CLI's report
/// marks commands with them, which a label shows as they are (AM5 review).
fn plain(text: &str) -> String {
    text.replace('`', "")
}

/// A status's chip: its colour and word.
fn status_chip(ui: &egui::Ui, status: Status) -> (egui::Color32, &'static str) {
    match status {
        Status::Good => (crate::theme::good(ui), "OK"),
        Status::Warn => (crate::theme::warn(ui), "Check"),
        Status::Bad => (crate::theme::bad(ui), "Problem"),
        Status::Info => (crate::theme::muted(ui), "Info"),
    }
}

/// Every check, as the worker thread runs them.
fn run(state_dir: &Path, config: Option<Config>) -> Report {
    let config = config.unwrap_or_default();
    let state = diagnostics::server_state(state_dir);
    // T1: the port the running server bound, when another program holds
    // the configured one.
    let control_port =
        pocketdesk_core::local::listening::control_port(state_dir, config.control_port);
    let mut findings = vec![
        diagnostics::check_server(state),
        diagnostics::check_control_port(control_port, state),
        diagnostics::check_stream_port(config.streaming_port, state),
    ];
    if let Some(moved) = diagnostics::check_port_moved(pocketdesk_core::local::listening::moved(
        state_dir,
        config.control_port,
    )) {
        findings.push(moved);
    }
    if let Some(firewall) = diagnostics::check_firewall(firewall_state(state_dir)) {
        findings.push(firewall);
    }
    // O9: ufw's or firewalld's rules for the two ports.
    #[cfg(target_os = "linux")]
    findings.push(diagnostics::check_linux_firewall(
        &linux_firewall(control_port, config.streaming_port),
        control_port,
        config.streaming_port,
    ));
    findings.push(diagnostics::check_addresses(
        &pocketdesk_core::local::network::local_ips(),
    ));
    if config.internet_path {
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
    } else {
        // Off in Settings: nothing outside to measure, and nothing failed.
        findings.push(diagnostics::internet_path_off());
    }
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
        control_port,
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

/// O9: ufw's or firewalld's rules for the control and stream ports, each
/// tool asked within 3 s. ufw's status needs root; its config file says
/// whether it is on without it.
#[cfg(target_os = "linux")]
fn linux_firewall(control_port: u16, stream_port: u16) -> diagnostics::LinuxFirewall {
    let ufw = linux_ufw(control_port, stream_port);
    // A ufw that is off does not hide a firewalld that runs (O9 review).
    if !matches!(
        ufw,
        diagnostics::LinuxFirewall::None | diagnostics::LinuxFirewall::Off { .. }
    ) {
        return ufw;
    }
    match linux_firewalld(control_port, stream_port) {
        diagnostics::LinuxFirewall::None => ufw,
        diagnostics::LinuxFirewall::Off { .. } if ufw != diagnostics::LinuxFirewall::None => ufw,
        firewalld => firewalld,
    }
}

/// The first of `paths` that exists.
#[cfg(target_os = "linux")]
fn installed(paths: &[&'static str]) -> Option<&'static str> {
    paths.iter().copied().find(|p| Path::new(p).exists())
}

#[cfg(target_os = "linux")]
const FIREWALL_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// ufw: its status needs root; its config file says whether it is on
/// without it.
#[cfg(target_os = "linux")]
fn linux_ufw(control_port: u16, stream_port: u16) -> diagnostics::LinuxFirewall {
    let Some(ufw) = installed(&["/usr/sbin/ufw", "/sbin/ufw", "/usr/bin/ufw"]) else {
        return diagnostics::LinuxFirewall::None;
    };
    if let Some(out) =
        pocketdesk_core::local::network::output_within(ufw, &["status", "verbose"], FIREWALL_WAIT)
    {
        return diagnostics::parse_ufw(&String::from_utf8_lossy(&out), control_port, stream_port);
    }
    let enabled = std::fs::read_to_string("/etc/ufw/ufw.conf")
        .map(|conf| conf.lines().any(|l| l.trim() == "ENABLED=yes"))
        .unwrap_or(true);
    if enabled {
        diagnostics::LinuxFirewall::Unreadable { tool: "ufw" }
    } else {
        diagnostics::LinuxFirewall::Off { tool: "ufw" }
    }
}

/// firewalld: each active zone's list read (at most four), since phones
/// may come in through any of them (O9 review).
#[cfg(target_os = "linux")]
fn linux_firewalld(control_port: u16, stream_port: u16) -> diagnostics::LinuxFirewall {
    use pocketdesk_core::local::network::output_within;
    let Some(cmd) = installed(&["/usr/bin/firewall-cmd", "/bin/firewall-cmd"]) else {
        return diagnostics::LinuxFirewall::None;
    };
    let text = |out: Vec<u8>| String::from_utf8_lossy(&out).into_owned();
    // "not running" is answered with a failing exit, which reads as none.
    let state = output_within(cmd, &["--state"], FIREWALL_WAIT)
        .map(text)
        .unwrap_or_default();
    if state.trim() != "running" {
        return diagnostics::parse_firewalld(&state, None, control_port, stream_port);
    }
    // Zone names start their line; the lines under them are indented.
    let zones: Vec<String> = output_within(cmd, &["--get-active-zones"], FIREWALL_WAIT)
        .map(text)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with(char::is_whitespace))
        .map(|l| l.trim().to_string())
        .take(4)
        .collect();
    let lists: Vec<String> = if zones.is_empty() {
        output_within(cmd, &["--list-all"], FIREWALL_WAIT)
            .map(text)
            .into_iter()
            .collect()
    } else {
        zones
            .iter()
            .filter_map(|zone| {
                let arg = format!("--zone={zone}");
                output_within(cmd, &["--list-all", arg.as_str()], FIREWALL_WAIT).map(text)
            })
            .collect()
    };
    let list = (!lists.is_empty()).then(|| lists.join("\n"));
    diagnostics::parse_firewalld(&state, list.as_deref(), control_port, stream_port)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{LatencyReport, latency_lines};

    #[test]
    fn the_last_latency_reads_in_milliseconds() {
        let latency = LatencyReport {
            network_p50_us: 1_250,
            network_p95_us: 4_000,
            ours_p50_us: 180,
            ours_p95_us: 2_350,
            samples: 400,
            measured_unix: 1_000,
            ..LatencyReport::default()
        };
        let lines = latency_lines(&latency, 1_000 + 120);
        assert!(lines[0].contains("over 400 keys and moves"), "{lines:?}");
        assert_eq!(
            lines[1],
            "Pocketdesk: 0.18 ms typical, 2.35 ms at worst (p95)."
        );
        assert_eq!(
            lines[2],
            "The network: 1.25 ms typical, 4.00 ms at worst (p95), one way."
        );
    }

    #[test]
    fn a_findings_commands_read_without_backticks() {
        assert_eq!(
            super::plain("Run `sudo ufw allow 4716/udp`, then check again."),
            "Run sudo ufw allow 4716/udp, then check again."
        );
        assert_eq!(super::plain("Nothing to strip."), "Nothing to strip.");
    }

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
