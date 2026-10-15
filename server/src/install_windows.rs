//! `pocketdesk install` / `uninstall` on Windows.
//!
//! The server must run inside the user's interactive session (Session 0, where
//! Windows services live, has no desktop to capture or inject into), so it is
//! registered as a per-user Scheduled Task that starts at logon — the Windows
//! counterpart of the Linux `graphical-session.target` user unit. The task is
//! registered through the Task Scheduler COM API from an in-memory XML
//! definition: no temp file sits between writing the definition and
//! registering it, and XML is the only form that can express restart-on-failure
//! or "keep running on battery".
//!
//! Firewall rules need administrator rights, so only that step is re-launched
//! elevated (one UAC prompt). The rules admit the phone from the local subnet
//! on private/domain networks, and from the tailnet only *through the Tailscale
//! adapter* — an address range alone would also match carrier-grade NAT on a
//! public network.
//!
//! Every path that decides what runs (or what counts as administrator-owned) is
//! asked of the OS, never read from an environment variable the user controls
//! and an elevated child would inherit.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::platform::system32;
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteTreeW};
use windows::Win32::System::TaskScheduler::{
    ITaskService, TASK_CREATE_OR_UPDATE, TASK_LOGON_INTERACTIVE_TOKEN, TaskScheduler,
};
use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::{
    FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86, IsUserAnAdmin, KNOWN_FOLDER_FLAG,
    SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, SHGetKnownFolderPath, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::{BSTR, GUID, HSTRING, PCWSTR, w};

/// The Scheduled Task's name.
pub const TASK_NAME: &str = "Pocketdesk";

/// The firewall rule group; removal deletes the whole group.
const RULE_GROUP: &str = "Pocketdesk";

/// What the task and the rule group were called before the rename. An old
/// install's task would otherwise start a second server at every logon next
/// to the new one, and its rules would stay open after `uninstall`, so both
/// are removed wherever the current names are.
const LEGACY_NAME: &str = "Linux Link";

/// The adapter Tailscale installs.
const TAILSCALE_ADAPTER: &str = "Tailscale";

/// Ports the phone reaches, with the protocol and what each carries.
const PORTS: [(&str, u16, &str); 2] = [("TCP", 1716, "control"), ("UDP", 4716, "media")];

/// Tailscale's address ranges (IPv4 CGNAT block and its IPv6 ULA prefix).
const TAILNET: &str = "100.64.0.0/10,fd7a:115c:a1e0::/48";

fn known_folder(id: &GUID) -> Option<PathBuf> {
    let path = unsafe { SHGetKnownFolderPath(id, KNOWN_FOLDER_FLAG(0), None) }.ok()?;
    let text = unsafe { path.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(path.0 as *const _)) };
    text.map(PathBuf::from)
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The Scheduled Task definition: start `exe start --background` at `user`'s
/// logon, in their interactive session, restarting it if it dies.
pub fn task_xml(exe: &Path, user: &str, elevated: bool) -> String {
    let run_level = if elevated {
        "HighestAvailable"
    } else {
        "LeastPrivilege"
    };
    let user = xml_escape(user);
    let exe = xml_escape(&exe.display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Pocketdesk desktop server: lets your paired phone view and control this PC.</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>{run_level}</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>5</Count>
    </RestartOnFailure>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <Enabled>true</Enabled>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
      <Arguments>start --background</Arguments>
    </Exec>
  </Actions>
</Task>
"#
    )
}

/// A PowerShell single-quoted string literal.
fn ps_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// The PowerShell that replaces the group's rules with the current set.
///
/// LAN rules: private/domain profiles, local subnet only. Tailnet rules: bound
/// to the Tailscale adapter (any profile, since Windows usually files that
/// adapter as Public) and to Tailscale's ranges; skipped, with a note, when the
/// adapter is absent.
pub fn firewall_add_script(exe: &Path) -> String {
    let group = ps_quote(RULE_GROUP);
    let program = ps_quote(&exe.display().to_string());
    let mut script = format!(
        "$ErrorActionPreference = 'Stop'\n{}",
        firewall_remove_script()
    );
    for (protocol, port, what) in PORTS {
        let name = ps_quote(&format!("Pocketdesk {what} ({protocol} {port})"));
        script.push_str(&format!(
            "New-NetFirewallRule -Group {group} -DisplayName {name} -Direction Inbound -Action Allow \
             -Protocol {protocol} -LocalPort {port} -Program {program} -Profile Private,Domain \
             -RemoteAddress LocalSubnet | Out-Null\n"
        ));
    }
    let adapter = ps_quote(TAILSCALE_ADAPTER);
    script.push_str(&format!(
        "if (Get-NetAdapter -Name {adapter} -ErrorAction SilentlyContinue) {{\n"
    ));
    for (protocol, port, what) in PORTS {
        let name = ps_quote(&format!("Pocketdesk {what} ({protocol} {port}) (tailnet)"));
        script.push_str(&format!(
            "  New-NetFirewallRule -Group {group} -DisplayName {name} -Direction Inbound -Action Allow \
             -Protocol {protocol} -LocalPort {port} -Program {program} -Profile Any \
             -InterfaceAlias {adapter} -RemoteAddress {TAILNET} | Out-Null\n"
        ));
    }
    script.push_str("} else { Write-Output 'no Tailscale adapter: tailnet rules skipped' }\n");
    script
}

/// The PowerShell that removes every rule [`firewall_add_script`] adds, and
/// those an install from before the rename added.
pub fn firewall_remove_script() -> String {
    [RULE_GROUP, LEGACY_NAME]
        .iter()
        .map(|group| {
            format!(
                "Get-NetFirewallRule -Group {} -ErrorAction SilentlyContinue | Remove-NetFirewallRule\n",
                ps_quote(group)
            )
        })
        .collect()
}

fn powershell(script: &str) -> Result<String> {
    let exe = system32(r"WindowsPowerShell\v1.0\powershell.exe");
    let output = Command::new(exe)
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .context("running PowerShell")?;
    if !output.status.success() {
        bail!(
            "PowerShell failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Apply or remove the firewall rules. Must already be elevated; this is what
/// the elevated re-launch runs.
pub fn apply_firewall(add: bool) -> Result<()> {
    if !unsafe { IsUserAnAdmin() }.as_bool() {
        bail!("changing firewall rules needs administrator rights");
    }
    let script = if add {
        firewall_add_script(&std::env::current_exe()?)
    } else {
        firewall_remove_script()
    };
    let said = powershell(&script)?;
    if !said.is_empty() {
        println!("{said}");
    }
    Ok(())
}

/// Re-run this program elevated with `args` (one UAC prompt, which names the
/// binary) and wait for it.
fn run_elevated(args: &str) -> Result<u32> {
    let exe = HSTRING::from(std::env::current_exe()?.as_os_str());
    let params = HSTRING::from(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(exe.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.context("the elevation prompt was declined or failed")?;
    let process = info.hProcess;
    let waited = unsafe { WaitForSingleObject(process, INFINITE) };
    let mut code = 1u32;
    let _ = unsafe { GetExitCodeProcess(process, &mut code) };
    let _ = unsafe { CloseHandle(process) };
    if waited != WAIT_OBJECT_0 {
        bail!("lost track of the elevated helper");
    }
    Ok(code)
}

fn current_user() -> Result<String> {
    let user = std::env::var("USERNAME").context("USERNAME is not set")?;
    Ok(match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => format!("{domain}\\{user}"),
        _ => user,
    })
}

/// Whether `exe` sits under a Program Files directory, which only
/// administrators can write — the precondition for an elevated task, whose
/// binary must not be replaceable by the (unelevated) user's own processes.
fn in_program_files(exe: &Path) -> bool {
    let exe = exe.to_string_lossy().to_ascii_lowercase();
    [FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86]
        .iter()
        .filter_map(known_folder)
        .map(|dir| {
            let mut dir = dir.to_string_lossy().to_ascii_lowercase();
            if !dir.ends_with('\\') {
                dir.push('\\');
            }
            dir
        })
        .any(|dir| exe.starts_with(&dir))
}

fn task_service() -> Result<ITaskService> {
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let service: ITaskService =
        unsafe { CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER) }
            .context("opening the Task Scheduler")?;
    let none = VARIANT::default();
    unsafe { service.Connect(&none, &none, &none, &none) }
        .context("connecting to the Task Scheduler")?;
    Ok(service)
}

/// Register the logon task, start it, and open the firewall.
pub fn install(elevated: bool) -> Result<()> {
    let exe = std::env::current_exe()?.canonicalize()?;
    if elevated {
        if !unsafe { IsUserAnAdmin() }.as_bool() {
            bail!("`install --elevated` must itself run elevated (from an administrator terminal)");
        }
        if !in_program_files(&exe) {
            bail!(
                "`install --elevated` needs pocketdesk.exe under Program Files: an elevated task \
                 that starts a file the user can replace ({}) is a privilege escalation",
                exe.display()
            );
        }
    }
    let xml = task_xml(&exe, &current_user()?, elevated);
    let none = VARIANT::default();
    let task = unsafe {
        task_service()?.GetFolder(&BSTR::from("\\"))?.RegisterTask(
            &BSTR::from(TASK_NAME),
            &BSTR::from(xml),
            TASK_CREATE_OR_UPDATE.0,
            &none,
            &none,
            TASK_LOGON_INTERACTIVE_TOKEN,
            &none,
        )
    }
    .context("registering the logon task")?;
    println!(
        "Registered the \"{TASK_NAME}\" logon task for {}",
        exe.display()
    );
    if delete_task(LEGACY_NAME).is_ok() {
        println!("Removed the \"{LEGACY_NAME}\" logon task an earlier version registered");
    }

    match run_elevated("firewall-add") {
        Ok(0) => println!("Opened TCP 1716 and UDP 4716 to this program (local subnet + tailnet)"),
        Ok(code) => println!(
            "Firewall rules were not added (helper exited {code}); add them from an administrator terminal with `pocketdesk firewall-add`"
        ),
        Err(e) => {
            println!("Firewall rules were not added ({e:#}); the phone may not reach this PC")
        }
    }

    // "Send to phone" in Explorer's Send To menu (T10e).
    use pocketdesk_core::local::send_menu;
    match send_menu::data_dir().and_then(|data| send_menu::install_in(&data, &exe)) {
        Ok(()) => println!("Added \"{}\" to Explorer's Send To menu", send_menu::LABEL),
        Err(e) => println!("\"{}\" was not added to Send To ({e})", send_menu::LABEL),
    }

    match unsafe { task.Run(&none) } {
        Ok(_) => println!("Started Pocketdesk; it will start again at every logon"),
        Err(e) => {
            println!("Registered, but could not start it now ({e}); it will start at next logon")
        }
    }
    Ok(())
}

/// Delete the logon task called `name`.
fn delete_task(name: &str) -> Result<()> {
    unsafe {
        task_service()?
            .GetFolder(&BSTR::from("\\"))?
            .DeleteTask(&BSTR::from(name), 0)?;
    }
    Ok(())
}

/// Remove the logon task, the firewall rules and the notification identity.
pub fn uninstall() -> Result<()> {
    if delete_task(LEGACY_NAME).is_ok() {
        println!("Removed the \"{LEGACY_NAME}\" logon task an earlier version registered");
    }
    let removed = delete_task(TASK_NAME);
    match removed {
        Ok(()) => println!(
            "Removed the \"{TASK_NAME}\" logon task (a running server keeps running until `pocketdesk stop`)"
        ),
        Err(e) => println!("No logon task removed ({e})"),
    }
    match run_elevated("firewall-remove") {
        Ok(0) => println!("Removed the firewall rules"),
        Ok(code) => println!("Firewall rules were not removed (helper exited {code})"),
        Err(e) => println!("Firewall rules were not removed ({e:#})"),
    }
    use pocketdesk_core::local::send_menu;
    if send_menu::data_dir()
        .and_then(|data| send_menu::remove_in(&data))
        .is_ok()
    {
        println!(
            "Removed \"{}\" from Explorer's Send To menu",
            send_menu::LABEL
        );
    }
    let subkey = HSTRING::from(r"Software\Classes\AppUserModelId\Pocketdesk.Server");
    let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &subkey) };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_task_runs_the_server_in_the_background_at_logon() {
        let xml = task_xml(
            Path::new(r"C:\Apps\Linux & Link\pocketdesk.exe"),
            r"PC\me",
            false,
        );
        assert!(xml.contains("<LogonTrigger>"));
        assert!(xml.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(xml.contains("<RunLevel>LeastPrivilege</RunLevel>"));
        assert!(xml.contains("<Arguments>start --background</Arguments>"));
        assert!(xml.contains(r"<Command>C:\Apps\Linux &amp; Link\pocketdesk.exe</Command>"));
        assert!(xml.contains("<RestartOnFailure>"));
        assert!(xml.contains("<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>"));
        assert!(task_xml(Path::new("x.exe"), "me", true).contains("HighestAvailable"));
        // Well-formed: the Task Scheduler rejects anything else.
        let doc = windows::Data::Xml::Dom::XmlDocument::new().unwrap();
        doc.LoadXml(&HSTRING::from(xml.replacen("UTF-16", "UTF-8", 1)))
            .expect("well-formed task XML");
    }

    #[test]
    fn only_the_ports_the_server_listens_on_are_opened() {
        // TCP 1716 control, UDP 4716 QUIC. Discovery is mDNS; nothing
        // listens on UDP 1716, so an open rule there is only exposure.
        let opened: Vec<(&str, u16)> = PORTS.iter().map(|(p, port, _)| (*p, *port)).collect();
        assert_eq!(opened, [("TCP", 1716), ("UDP", 4716)]);
    }

    #[test]
    fn lan_rules_are_local_subnet_and_tailnet_rules_are_bound_to_its_adapter() {
        let script = firewall_add_script(Path::new(r"C:\Program Files\Pocketdesk\pocketdesk.exe"));
        let rules: Vec<&str> = script
            .lines()
            .filter(|l| l.contains("New-NetFirewallRule"))
            .collect();
        assert_eq!(rules.len(), PORTS.len() * 2);
        for rule in &rules {
            assert!(rule.contains(r"-Program 'C:\Program Files\Pocketdesk\pocketdesk.exe'"));
            assert!(rule.contains("-Direction Inbound"));
            if rule.contains("(tailnet)") {
                assert!(rule.contains("-InterfaceAlias 'Tailscale'"), "{rule}");
                assert!(
                    rule.contains(&format!("-RemoteAddress {TAILNET}")),
                    "{rule}"
                );
            } else {
                assert!(rule.contains("-Profile Private,Domain"), "{rule}");
                assert!(rule.contains("-RemoteAddress LocalSubnet"), "{rule}");
            }
        }
        // The tailnet rules only exist when the adapter does.
        assert!(script.contains("if (Get-NetAdapter -Name 'Tailscale'"));
        // Re-running install replaces the group instead of duplicating it.
        assert!(script.starts_with(
            "$ErrorActionPreference = 'Stop'\nGet-NetFirewallRule -Group 'Pocketdesk'"
        ));
    }

    #[test]
    fn uninstall_removes_the_group_install_creates() {
        assert!(firewall_remove_script().contains("-Group 'Pocketdesk'"));
        assert!(firewall_add_script(Path::new("x.exe")).contains("-Group 'Pocketdesk'"));
    }

    #[test]
    fn rules_from_before_the_rename_are_removed_too() {
        // Both install (which replaces the group) and uninstall clear the
        // old "Linux Link" group, or its ports stay open forever.
        let legacy = "Get-NetFirewallRule -Group 'Linux Link' -ErrorAction SilentlyContinue \
                      | Remove-NetFirewallRule";
        let add = firewall_add_script(Path::new("x.exe"));
        assert!(firewall_remove_script().contains(legacy));
        assert!(add.contains(legacy));
        // New rules are only ever created in the new group.
        assert!(
            add.lines()
                .filter(|l| l.contains("New-NetFirewallRule"))
                .all(|l| l.contains("-Group 'Pocketdesk'"))
        );
    }

    #[test]
    fn powershell_strings_cannot_be_broken_out_of() {
        assert_eq!(ps_quote("it's"), "'it''s'");
        let script = firewall_add_script(Path::new(r"C:\a'b\pocketdesk.exe"));
        assert!(script.contains(r"-Program 'C:\a''b\pocketdesk.exe'"));
    }

    #[test]
    fn only_program_files_counts_as_admin_owned_and_env_cannot_move_it() {
        let pf = known_folder(&FOLDERID_ProgramFiles).expect("Program Files exists");
        assert!(in_program_files(&pf.join(r"Pocketdesk\pocketdesk.exe")));
        assert!(!in_program_files(Path::new(
            r"C:\Users\me\Downloads\pocketdesk.exe"
        )));
        assert!(!in_program_files(Path::new(
            r"E:\Linux-Link\target\debug\pocketdesk.exe"
        )));
    }

    #[test]
    fn system_tools_are_resolved_from_the_os_not_the_environment() {
        let ps = system32(r"WindowsPowerShell\v1.0\powershell.exe");
        assert!(ps.is_absolute());
        assert!(ps.exists(), "{}", ps.display());
    }
}
