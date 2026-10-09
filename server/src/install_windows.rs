//! `linux-link install` / `uninstall` on Windows.
//!
//! The server must run inside the user's interactive session (Session 0, where
//! Windows services live, has no desktop to capture or inject into), so it is
//! registered as a per-user Scheduled Task that starts at logon — the Windows
//! counterpart of the Linux `graphical-session.target` user unit. The task is
//! defined in XML because the `schtasks` flags cannot express restart-on-failure
//! or "keep running on battery".
//!
//! Firewall rules need administrator rights, so only that step is re-launched
//! elevated (one UAC prompt). Every system tool is invoked by its absolute
//! `System32` path, never resolved off `PATH`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteTreeW};
use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
use windows::Win32::UI::Shell::{
    IsUserAnAdmin, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::{HSTRING, w};

/// The Scheduled Task's name.
pub const TASK_NAME: &str = "Linux Link";

/// Ports the phone reaches, with the protocol and what each carries.
const PORTS: [(&str, u16, &str); 3] = [
    ("TCP", 1716, "control"),
    ("UDP", 1716, "discovery"),
    ("UDP", 4716, "media"),
];

/// Tailscale's address ranges: rules for these apply on any network profile,
/// because Windows often files the Tailscale adapter under "Public".
const TAILNET: &str = "100.64.0.0/10,fd7a:115c:a1e0::/48";

fn system32(tool: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    PathBuf::from(root).join("System32").join(tool)
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
    <Description>Linux Link desktop server: lets your paired phone view and control this PC.</Description>
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

fn rule_name(protocol: &str, port: u16, what: &str, tailnet: bool) -> String {
    let scope = if tailnet { " (tailnet)" } else { "" };
    format!("Linux Link {what} ({protocol} {port}){scope}")
}

/// `netsh` argument lists that open the server's ports to `exe` only: on
/// private/domain networks from anywhere, and on any network from the tailnet.
pub fn firewall_add_args(exe: &Path) -> Vec<Vec<String>> {
    let mut all = Vec::new();
    for (protocol, port, what) in PORTS {
        for tailnet in [false, true] {
            let mut args: Vec<String> = ["advfirewall", "firewall", "add", "rule"]
                .map(String::from)
                .to_vec();
            args.push(format!("name={}", rule_name(protocol, port, what, tailnet)));
            args.push("dir=in".into());
            args.push("action=allow".into());
            args.push(format!("protocol={protocol}"));
            args.push(format!("localport={port}"));
            args.push(format!("program={}", exe.display()));
            if tailnet {
                args.push("profile=any".into());
                args.push(format!("remoteip={TAILNET}"));
            } else {
                args.push("profile=private,domain".into());
            }
            all.push(args);
        }
    }
    all
}

/// `netsh` argument lists that remove every rule [`firewall_add_args`] adds.
pub fn firewall_delete_args() -> Vec<Vec<String>> {
    let mut all = Vec::new();
    for (protocol, port, what) in PORTS {
        for tailnet in [false, true] {
            all.push(vec![
                "advfirewall".into(),
                "firewall".into(),
                "delete".into(),
                "rule".into(),
                format!("name={}", rule_name(protocol, port, what, tailnet)),
            ]);
        }
    }
    all
}

/// Apply or remove the firewall rules. Must already be elevated; this is what
/// the elevated re-launch runs.
pub fn apply_firewall(add: bool) -> Result<()> {
    if !unsafe { IsUserAnAdmin() }.as_bool() {
        bail!("changing firewall rules needs administrator rights");
    }
    let exe = std::env::current_exe()?;
    let commands = if add {
        // Start from a clean slate so re-running install never duplicates.
        let mut all = firewall_delete_args();
        all.extend(firewall_add_args(&exe));
        all
    } else {
        firewall_delete_args()
    };
    let netsh = system32("netsh.exe");
    for args in commands {
        let deleting = args[2] == "delete";
        let output = Command::new(&netsh).args(&args).output()?;
        // Deleting a rule that is not there is not an error worth stopping on.
        if !output.status.success() && !deleting {
            bail!(
                "netsh {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stdout).trim()
            );
        }
    }
    Ok(())
}

/// Re-run this program elevated with `args` (one UAC prompt) and wait for it.
fn run_elevated(args: &str) -> Result<u32> {
    let exe = HSTRING::from(std::env::current_exe()?.as_os_str());
    let params = HSTRING::from(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: windows::core::PCWSTR(exe.as_ptr()),
        lpParameters: windows::core::PCWSTR(params.as_ptr()),
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

fn write_utf16_with_bom(path: &Path, text: &str) -> Result<()> {
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

/// Register the logon task, start it, and open the firewall.
pub fn install(elevated: bool) -> Result<()> {
    let exe = std::env::current_exe()?;
    let xml = task_xml(&exe, &current_user()?, elevated);
    let xml_path = std::env::temp_dir().join(format!("linux-link-task-{}.xml", std::process::id()));
    write_utf16_with_bom(&xml_path, &xml)?;
    let created = Command::new(system32("schtasks.exe"))
        .args(["/Create", "/TN", TASK_NAME, "/XML"])
        .arg(&xml_path)
        .arg("/F")
        .output();
    let _ = std::fs::remove_file(&xml_path);
    let created = created.context("running schtasks")?;
    if !created.status.success() {
        bail!(
            "schtasks could not register the task: {}",
            String::from_utf8_lossy(&created.stderr).trim()
        );
    }
    println!("Registered the \"{TASK_NAME}\" logon task for {exe:?}");

    match run_elevated("firewall-add") {
        Ok(0) => println!("Opened TCP 1716, UDP 1716 and UDP 4716 for this program"),
        Ok(code) => println!(
            "Firewall rules were not added (helper exited {code}); add them as admin with `linux-link firewall-add`"
        ),
        Err(e) => {
            println!("Firewall rules were not added ({e:#}); the phone may not reach this PC")
        }
    }

    let started = Command::new(system32("schtasks.exe"))
        .args(["/Run", "/TN", TASK_NAME])
        .output()
        .context("running schtasks")?;
    if started.status.success() {
        println!("Started Linux Link; it will start again at every logon");
    }
    Ok(())
}

/// Remove the logon task, the firewall rules and the notification identity.
pub fn uninstall() -> Result<()> {
    let deleted = Command::new(system32("schtasks.exe"))
        .args(["/Delete", "/TN", TASK_NAME, "/F"])
        .output()
        .context("running schtasks")?;
    if deleted.status.success() {
        println!(
            "Removed the \"{TASK_NAME}\" logon task (a running server keeps running until `linux-link stop`)"
        );
    }
    match run_elevated("firewall-remove") {
        Ok(0) => println!("Removed the firewall rules"),
        Ok(code) => println!("Firewall rules were not removed (helper exited {code})"),
        Err(e) => println!("Firewall rules were not removed ({e:#})"),
    }
    let subkey = HSTRING::from(r"Software\Classes\AppUserModelId\LinuxLink.Server");
    let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &subkey) };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_task_runs_the_server_in_the_background_at_logon() {
        let xml = task_xml(
            Path::new(r"C:\Apps\Linux & Link\linux-link.exe"),
            r"PC\me",
            false,
        );
        assert!(xml.contains("<LogonTrigger>"));
        assert!(xml.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(xml.contains("<RunLevel>LeastPrivilege</RunLevel>"));
        assert!(xml.contains("<Arguments>start --background</Arguments>"));
        assert!(xml.contains(r"<Command>C:\Apps\Linux &amp; Link\linux-link.exe</Command>"));
        assert!(xml.contains("<RestartOnFailure>"));
        assert!(xml.contains("<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>"));
        assert!(task_xml(Path::new("x.exe"), "me", true).contains("HighestAvailable"));
        // Well-formed: the Task Scheduler rejects anything else.
        let doc = windows::Data::Xml::Dom::XmlDocument::new().unwrap();
        doc.LoadXml(&HSTRING::from(xml.replacen("UTF-16", "UTF-8", 1)))
            .expect("well-formed task XML");
    }

    #[test]
    fn every_port_is_opened_on_private_networks_and_to_the_tailnet() {
        let rules = firewall_add_args(Path::new(r"C:\ll\linux-link.exe"));
        assert_eq!(rules.len(), PORTS.len() * 2);
        for rule in &rules {
            assert!(rule.contains(&r"program=C:\ll\linux-link.exe".to_string()));
            assert!(rule.contains(&"dir=in".to_string()));
            let tailnet = rule.iter().any(|a| a.starts_with("remoteip="));
            let profile = rule.iter().find(|a| a.starts_with("profile=")).unwrap();
            if tailnet {
                assert_eq!(profile, "profile=any");
            } else {
                assert_eq!(profile, "profile=private,domain");
            }
        }
        for (protocol, port, _) in PORTS {
            let local = format!("localport={port}");
            let proto = format!("protocol={protocol}");
            assert_eq!(
                rules
                    .iter()
                    .filter(|r| r.contains(&local) && r.contains(&proto))
                    .count(),
                2
            );
        }
    }

    #[test]
    fn uninstall_deletes_exactly_the_rules_install_adds() {
        let added: Vec<String> = firewall_add_args(Path::new("x.exe"))
            .iter()
            .map(|r| r.iter().find(|a| a.starts_with("name=")).unwrap().clone())
            .collect();
        let deleted: Vec<String> = firewall_delete_args()
            .iter()
            .map(|r| r[4].clone())
            .collect();
        assert_eq!(added, deleted);
    }

    #[test]
    fn system_tools_are_resolved_inside_system32() {
        let netsh = system32("netsh.exe");
        assert!(netsh.is_absolute());
        assert!(netsh.ends_with(r"System32\netsh.exe"));
    }
}
