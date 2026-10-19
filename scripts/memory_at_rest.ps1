# What the server and the desktop app hold in memory at rest (D7).
#
# Copies a release build's `pocketdesk.exe` and `pocketdesk-app.exe` to a
# folder of its own, starts them over a throwaway state and config folder
# (no phone needed), the app minimized as it rests, lets them settle a
# minute, samples each every 10 s for 3 minutes, and prints the medians of
# the working set and the private bytes. Only the processes it started are
# stopped. Build first: `cargo build --release -p pocketdesk-server -p pocketdesk-desktop`.
#
# With `-Tray`, it measures the tray program (`pocketdesk-tray.exe`, E2) in
# the app's place: what rests once the window is closed. Without it the
# tray program is left out, so the app keeps a tray of its own, as before E2.
#
#   scripts/memory_at_rest.ps1 -Release target\release -StateDir C:\tmp\pd\state -ConfigDir C:\tmp\pd\config [-Tray]
param(
    [Parameter(Mandatory = $true)][string]$Release,
    [Parameter(Mandatory = $true)][string]$StateDir,
    [Parameter(Mandatory = $true)][string]$ConfigDir,
    [string]$Label = "now",
    [int]$Samples = 18,
    [switch]$Tray
)
$ErrorActionPreference = "Stop"
$bin = Join-Path $env:TEMP "pocketdesk-memory-at-rest"
New-Item -ItemType Directory -Force $bin, $StateDir, $ConfigDir | Out-Null
Copy-Item (Join-Path $Release "pocketdesk.exe") $bin -Force
Copy-Item (Join-Path $Release "pocketdesk-app.exe") $bin -Force
$trayExe = Join-Path $bin "pocketdesk-tray.exe"
if ($Tray) {
    Copy-Item (Join-Path $Release "pocketdesk-tray.exe") $bin -Force
}
elseif (Test-Path $trayExe) {
    Remove-Item $trayExe -Force
}

function Median($xs) {
    $s = @($xs | Sort-Object)
    return $s[[int][math]::Floor($s.Count / 2)]
}

$env:POCKETDESK_STATE_DIR = (Resolve-Path $StateDir).Path
$env:POCKETDESK_CONFIG_DIR = (Resolve-Path $ConfigDir).Path
$server = Start-Process (Join-Path $bin "pocketdesk.exe") -ArgumentList "start" -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $StateDir "memory-at-rest.log") `
    -RedirectStandardError (Join-Path $StateDir "memory-at-rest.err")
Start-Sleep -Seconds 5
if ($Tray) {
    $app = Start-Process $trayExe -PassThru
}
else {
    $app = Start-Process (Join-Path $bin "pocketdesk-app.exe") -PassThru -WindowStyle Minimized
}
try {
    Start-Sleep -Seconds 60
    $rows = @()
    for ($i = 0; $i -lt $Samples; $i++) {
        foreach ($p in @($server, $app)) {
            $p.Refresh()
            if ($p.HasExited) { throw "$($p.ProcessName) exited early" }
            $rows += [pscustomobject]@{
                Name    = $p.ProcessName
                WS      = $p.WorkingSet64 / 1MB
                Private = $p.PrivateMemorySize64 / 1MB
            }
        }
        Start-Sleep -Seconds 10
    }
}
finally {
    foreach ($p in @($server, $app)) {
        if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
    }
}
foreach ($name in "pocketdesk", $(if ($Tray) { "pocketdesk-tray" } else { "pocketdesk-app" })) {
    $mine = $rows | Where-Object Name -eq $name
    "{0} [{1}]: working set {2:N1} MB, private {3:N1} MB (medians of {4}; private first {5:N1}, last {6:N1})" -f `
        $name, $Label, (Median $mine.WS), (Median $mine.Private), $mine.Count, $mine[0].Private, $mine[-1].Private
}
