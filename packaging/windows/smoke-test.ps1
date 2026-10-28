# Install Pocketdesk with its setup.exe, check what it made, uninstall it,
# and check that all of it went (BH1). For a throwaway Windows machine or a
# CI runner, run as an administrator; never on a computer whose Pocketdesk
# you use, since it installs over it and uninstalls it.
#
#   pwsh packaging/windows/smoke-test.ps1 release/pocketdesk-<version>-windows-x86_64-setup.exe

param([Parameter(Mandatory)][string]$Setup)
$ErrorActionPreference = 'Stop'

$app = Join-Path $env:ProgramFiles 'Pocketdesk'
$shortcut = Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\Pocketdesk.lnk'
$uninstallKey = 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{8D3F6C2E-5B1A-4E7C-9A2F-3C6D1E8B4A70}_is1'
$schtasks = Join-Path $env:SystemRoot 'System32\schtasks.exe'
$failures = @()

function Check([bool]$ok, [string]$what) {
    if ($ok) { Write-Output "ok    $what" } else { Write-Output "FAIL  $what"; $script:failures += $what }
}
function OnPath { ([Environment]::GetEnvironmentVariable('Path', 'Machine') -split ';') -contains $app }
function HasTask { & $schtasks /Query /TN Pocketdesk *> $null; $LASTEXITCODE -eq 0 }
function RuleCount { @(Get-NetFirewallRule -Group 'Pocketdesk' -ErrorAction SilentlyContinue).Count }

# Install, with the tasks that need no network (not FFmpeg).
$log = Join-Path $env:TEMP 'pocketdesk-setup.log'
$p = Start-Process $Setup -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/TASKS=autostart,addtopath', "/LOG=$log" -Wait -PassThru
Check ($p.ExitCode -eq 0) "setup exited 0 (it exited $($p.ExitCode))"
foreach ($file in 'pocketdesk.exe', 'pocketdesk-app.exe', 'pocketdesk-tray.exe', 'pocketdesk.ico', 'unins000.exe') {
    Check (Test-Path (Join-Path $app $file)) "installed $file"
}
Check (Test-Path $shortcut) 'a Start menu entry'
Check (Test-Path $uninstallKey) 'an Apps & features entry'
Check (OnPath) 'Pocketdesk on the system PATH'
Check (HasTask) 'the logon task'
Check ((RuleCount) -gt 0) "the firewall rules ($(RuleCount))"
$version = & (Join-Path $app 'pocketdesk.exe') --version
Check ($LASTEXITCODE -eq 0) "pocketdesk --version runs ($version)"

# Pocketdesk's processes running from the install folder, for the log.
function ShowProcesses([string]$when) {
    $running = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($app, [StringComparison]::OrdinalIgnoreCase) })
    Write-Output "      $when`: $($running.Count) Pocketdesk process(es) running from $app"
    foreach ($proc in $running) { Write-Output "        $($proc.Name) (pid $($proc.Id), started $($proc.StartTime.ToString('HH:mm:ss')))" }
}
ShowProcesses 'before uninstalling'

# Uninstall. The uninstaller copies itself to TEMP and exits at once, so wait
# for its work (the folder's files) to go, for up to two minutes.
$uninstallLog = Join-Path $env:TEMP 'pocketdesk-uninstall.log'
Start-Process (Join-Path $app 'unins000.exe') -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=$uninstallLog" -Wait
$deadline = (Get-Date).AddMinutes(2)
while ((Test-Path (Join-Path $app 'pocketdesk.exe')) -and (Get-Date) -lt $deadline) { Start-Sleep -Seconds 2 }
while ((Test-Path $uninstallKey) -and (Get-Date) -lt $deadline) { Start-Sleep -Seconds 2 }
$filesGone = -not (Test-Path (Join-Path $app 'pocketdesk.exe'))
Check $filesGone 'uninstalled the files'
if (-not $filesGone) {
    ShowProcesses 'after uninstalling'
    Write-Output "      left in $app`: $((Get-ChildItem $app -ErrorAction SilentlyContinue).Name -join ', ')"
    if (Test-Path $uninstallLog) { Get-Content $uninstallLog -Tail 40 | ForEach-Object { "      | $_" } }
}
Check (-not (Test-Path $shortcut)) 'removed the Start menu entry'
Check (-not (Test-Path $uninstallKey)) 'removed the Apps & features entry'
Check (-not (OnPath)) 'took Pocketdesk off PATH'
Check (-not (HasTask)) 'removed the logon task'
Check ((RuleCount) -eq 0) 'removed the firewall rules'

if ($failures.Count -gt 0) {
    Write-Output "--- setup log ---"
    if (Test-Path $log) { Get-Content $log -Tail 80 }
    Write-Error "$($failures.Count) check(s) failed"
}
Write-Output 'The installer installs and uninstalls cleanly.'
