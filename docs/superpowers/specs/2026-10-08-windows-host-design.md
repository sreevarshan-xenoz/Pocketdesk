# Windows host — design

Status: approved in conversation 2026-10-08, pending written-spec review.
Branch: `feat/windows-host`. Sub-project A of "make Linux Link work with Windows";
sub-project B (a Windows *client*) gets its own spec after this one ships.

## 1. Intent

**Outcome.** The `linux-link` desktop server runs on Windows 10/11 so the existing
Android app can pair with, view and control a Windows PC exactly as it does a
Linux desktop today.

**Success.** Full feature parity with the Linux server, delivered in phases
(W0–W5, §9) that each end verifiable on a real Windows box with a real phone.
Where literal parity is impossible on Windows the gap is named here, reported
truthfully by `linux-link capabilities`, and surfaced to the phone as
"unavailable" — never as silence or a fake success.

**Constraints.**
- No wire, JNI or Kotlin change. The phone and bridge ship as they are.
- The Linux build keeps its behavior; restructuring commits are pure moves held
  to the existing Linux gates.
- AGENTS.md rules apply: one logical change per commit, gates before each
  commit, docs travel with the change, nothing sensitive staged.

**Known non-parity (accepted up front).**
| Item | Why | What Windows does instead |
|---|---|---|
| Hyprland workspace HUD | Virtual desktops have no stable public API | Window-chip row + active window stay live; workspace list is empty |
| Phone-mic relay without a virtual cable | No `pw-loopback`; shipping an audio driver is out of scope | Works with VB-Cable (or a configured cable); otherwise reported unavailable with the reason |
| UAC prompts / lock screen | Secure desktop is not capturable or injectable from a user session | Capture rebuilds when the desktop returns; input to it is dropped and logged |
| Input into elevated windows | UIPI | Works when the logon task is installed `--elevated` (opt-in) |
| NVENC clock pin | `nvidia-smi` clock lock needs admin | Self-declines, as it already does when refused |
| Desktop→phone notification mirror when unpackaged | `UserNotificationListener` needs package identity | Works under the sparse-MSIX identity package (W5); otherwise unavailable |

## 2. Approach

Windows backends are added behind the seams that already exist, not in a
separate crate and not via an up-front trait refactor:

- capture: `start_capture_auto` + `CaptureBackend` + `capture_attempts`, emitting BGRA `VideoFrame`s
- encode: `VideoEncoder` + `HardwareEncoder` + the C2 fallback ladder
- input: `InputInjector` + its private `InputBackend` enum
- audio: the `PcmBuffer` contract of `audio_capture`
- plugins: `plugins::register_all` + a new per-OS `server/src/platform/` helper module

Cargo **features keep meaning *what* is built**; **`cfg(target_os)` decides *how***.

Rejected: a separate `server-windows` crate (duplicates service, registry,
config, pairing, CLI — the code most likely to drift); trait-ifying every seam
first (large Linux-side refactor with no Windows value until done).

## 3. Build and cfg layout

**Toolchain.** Windows builds target `x86_64-pc-windows-msvc` (VS Build Tools
2022). The GNU host toolchain is not used for the Windows server.

**Cargo.**
- Workspace adds `windows` (version pinned at implementation) with only the
  feature sets used (Graphics_Capture, Win32_Graphics_{Dxgi,Direct3D11},
  Win32_Media_Audio, Win32_UI_Input_KeyboardAndMouse,
  Win32_UI_WindowsAndMessaging, Win32_System_{Power,DataExchange}, …).
- `core`: feature names unchanged (`server`, `capture`, `encode`, `injection`,
  `opus`, `client`, `wan`). Linux-only crates (ashpd, pipewire, libspa, x11rb,
  wayland-*, bitflags, evdev) move to `[target.'cfg(target_os = "linux")'.dependencies]`,
  still optional; `windows` goes in `[target.'cfg(windows)'.dependencies]`,
  optional. A feature lists both `dep:`s; only the one existing for the target
  activates.
- `server`: `evdev`, `zbus` → Linux target table; `windows` → Windows table;
  `enigo`, `xcap` stay shared.

**Source layout.**
```
core/src/streaming/
  capture.rs             shared: CaptureBackend, DisplayServer, capture_attempts, start_capture_auto
  capture_linux/…        portal + x11 bodies moved out of capture.rs (pure move)
  capture_screencopy.rs  cfg(linux)
  capture_wgc.rs         cfg(windows)
  capture_dxgi.rs        cfg(windows)
  audio_capture/{pipewire.rs, wasapi.rs}
  shield/{wayland.rs, windows.rs}
  encoder_vaapi.rs       cfg(linux)
server/src/
  input_injector.rs      shared API; backends in input/{uinput.rs, enigo.rs, sendinput.rs}
  hyprland.rs, hypr_events.rs, mic_relay pw half, privacy evdev half → cfg(linux)
  platform/{linux,windows}.rs   notify, local_ips, hostname, secure_write, stop_pid, lock_screen, …
```

**Rules.**
- Shared code never names an OS crate; it calls the seam.
- Config enum variants that do not exist on the running OS still *parse* and
  report "not available on this OS", so one `config.toml` loads on either OS.
- `capabilities.rs` reads cfg-gated tables. `docs/capabilities.md` stays the
  Linux output; `docs/capabilities-windows.md` is the Windows output, held to
  the build by the same kind of test, gated `cfg(windows)`.

**Gates (added; Linux gates unchanged).** On Windows:
`cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets --exclude linux-link-android-bridge --exclude iroh-spike -- -D warnings`,
`cargo test --workspace` with the same excludes.
Every pure-move commit additionally needs a real Linux run of the existing
gates before the branch merges; this box can only `cargo check --target
x86_64-unknown-linux-gnu`, which does not link PipeWire/FFmpeg. The plan lists
those runs explicitly.

## 4. Capture and encode

**Capture.**
- `DisplayServer::Windows` via `cfg(windows)`. `capture_attempts(Auto, Windows) = [Wgc, Dxgi]`.
  A pinned Linux backend on Windows yields the existing "pinned backend unavailable" error.
- **WGC** (`capture_wgc.rs`): `GraphicsCaptureItem` from `HMONITOR` or `HWND`;
  free-threaded `Direct3D11CaptureFramePool` (BGRA8, 2 buffers); per frame, copy
  into one reused staging texture, map, copy rows into
  `VideoFrame { data, width, height, stride }` (BGRA). `IsBorderRequired(false)`
  where supported, cursor capture on. Same newest-wins backpressure; the
  `CaptureSession` cancel token ends it.
- **DXGI Desktop Duplication** (`capture_dxgi.rs`): fallback. `AcquireNextFrame`
  → staging copy → same `VideoFrame`. `DXGI_ERROR_ACCESS_LOST` rebuilds the
  duplication rather than ending the session.
- **Window crop.** The 25-byte `WindowCrop` already carries a `u64` window
  address; on Windows it is the `HWND`. WGC captures the window directly. A
  closed window ends capture and the server falls back to whole-desktop, as
  today. No wire change.
- **Monitors.** `xcap` enumerates HMONITORs; monitor index *n* maps to the
  *n*-th in the same order on both the monitors plugin and capture.
- **DPI.** Per-monitor-v2 awareness via embedded manifest +
  `SetProcessDpiAwarenessContext` at startup.

**Encode.**
- `set_nonblocking` (fcntl) is replaced **on every OS** by one reader thread
  draining sidecar stdout into a bounded channel. The only shared behavior
  change; Linux re-runs its encoder tests on it.
- `HardwareEncoder` gains `Amf | Qsv | MediaFoundation`. On Windows,
  `probe_encoders()` runtime-verifies each of `h264_nvenc`, `h264_amf`,
  `h264_qsv`, `h264_mf` with a throwaway 1-frame encode. Auto order on Windows:
  NVENC → AMF → QSV → MF → software. `build_ffmpeg_args` gets a low-latency arg
  set per encoder: no B-frames (the phone's decode/render pairing depends on it,
  R2#1), `keyint=60`, CBR.
- `encoder_vaapi.rs` and VAAPI device scanning: `cfg(target_os = "linux")`.
- `nvenc_power`: unchanged; self-declines without rights.
- C2 ladder unchanged in shape: failed hardware rung → next hardware → software.
- Software rung on Windows: sidecar `libx264` until FFmpeg dev libs are set up;
  in-process ffmpeg-next on Windows is a W5 commit with
  `docs/development-setup.md` instructions (`FFMPEG_DIR`).
- ffmpeg discovery: `ffmpeg_path` config key, then `PATH`, then the binary
  bundled next to `linux-link.exe`.

**Verification on this box** (RTX 3060 + AMD iGPU): live WGC and DXGI one-frame
tests (size + not all-black), NVENC and AMF 40-frame roundtrips mirroring
`test_vaapi_encode_roundtrip`, committed Windows `linux-link bench` baselines.

## 5. Input

- `InputInjector` public API unchanged. `InputBackend` gains `SendInput`, the
  only backend constructed on Windows. Enigo stays Linux/X11.
- **Keyboard.** `KEY_SCANCODE_MAP: evdev u16 → (scancode, extended)` covering
  exactly `core::input::keys::PHONE_EVDEV_KEY_RANGES`; a test asserts every
  member is mapped. Injection via `KEYEVENTF_SCANCODE` (+`EXTENDEDKEY` for
  arrows, nav cluster, right Ctrl/Alt, Super), preserving the phone's
  physical-position semantics. Keys with no scancode (some media/volume) use VK
  codes, flagged per entry. Layout reported at construction via
  `GetKeyboardLayoutNameW` (Windows branch of 2065).
- **Text.** `KEYEVENTF_UNICODE` per UTF-16 unit, surrogate pairs included —
  correct capitals and non-ASCII on Windows (the Linux 2666 defect does not
  apply).
- **Pointer.** Absolute: `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK`, the
  phone's normalized point mapped through the streamed monitor's rect (or the
  cropped window's rect) into 0..65535 of the virtual desktop. Relative:
  `MOUSEEVENTF_MOVE`. Buttons L/R/M/X1/X2; `WHEEL`/`HWHEEL` scaled by
  `WHEEL_DELTA`.
- Drag (2061) and gamepad edge-diff (2062) live above the backend; unchanged.
- **Faults.** `SendInput` short count = blocked (UIPI / secure desktop). The
  2064 emit helper logs device, action and `GetLastError` once per
  foreground-window change, then continues; nothing to rebuild.
- **Tests.** Map contract, extended-key set, absolute mapping (multi-monitor,
  negative-origin monitors, crop rect), UTF-16 surrogates; `#[ignore]`d live
  test injecting a key and observing it.

## 6. Audio

- **Desktop capture** (`audio_capture/wasapi.rs`): event-driven WASAPI loopback
  on the default render endpoint, own thread, emitting the existing
  `PcmBuffer` (48 kHz, stereo, 20 ms). Float→i16 and a linear resampler when the
  mix rate is not 48 kHz (logged once). No packets while nothing plays — never
  synthesized silence (2054). Default-device change rebuilds the client; open
  failure ends the audio task softly. `played_on_the_receiving_end` unchanged.
- **Mic relay** (`mic_relay/windows.rs`): Opus decode as today; PCM rendered to
  the render endpoint whose friendly name matches `mic_relay_device`
  (config.toml, default the VB-Audio "CABLE Input"). Absent → relay reports
  unavailable with the reason; capabilities says
  `phone -> desktop: requires a virtual audio cable`.
- **Audio control** (`audio_control/windows.rs`): master volume/mute via
  `IAudioEndpointVolume`; sink list via `IMMDeviceEnumerator`; default-sink
  switch via `IPolicyConfig`, isolated in one function — on failure, switching
  reports unavailable while list/volume/mute keep working.
- **Tests.** Conversion/resampler/name-matching units; `#[ignore]`d live
  loopback capture and volume set/restore. Mic relay live only if a virtual
  cable is installed; otherwise only its unavailable branch is verified.

## 7. Plugins and platform helpers

Registry, packet types and phone UI unchanged. A plugin with no Windows
equivalent still registers and replies "unsupported on this OS".

| Plugin / module | Windows |
|---|---|
| clipboard | Win32 clipboard (`CF_UNICODETEXT`); change detection via `AddClipboardFormatListener` on a message-only window |
| battery | `GetSystemPowerStatus` |
| notification (mirror) | `UserNotificationListener`; requires sparse-MSIX identity (W5) + one-time consent; else unavailable |
| notification_reply | Reply text to clipboard + toast (same as the Linux fallback) |
| pair / live_sessions / kick notices | `platform::notify()` = WinRT toast under the installer's AUMID; tray balloon fallback |
| power | `LockWorkStation`, `SetSuspendState`, `InitiateSystemShutdownEx`/`ExitWindowsEx` with `SeShutdownPrivilege` |
| privacy | `WH_KEYBOARD_LL`+`WH_MOUSE_LL` on a dedicated thread swallow events lacking `LL*_INJECTED`; RAII guard + TTL watchdog unhook on expiry/session end/panic; Ctrl+Alt+Del remains as local escape; blackout = topmost layered window; lock = `LockWorkStation` |
| shield | Four topmost click-through edge windows per monitor (`WS_EX_TRANSPARENT|LAYERED|NOACTIVATE|TOOLWINDOW`) with `WDA_EXCLUDEFROMCAPTURE` |
| monitors | xcap; `xrandr` fallback becomes Linux-only |
| windows | `EnumWindows` filtered (visible, not cloaked, owner-less, titled); address = HWND; bounds via `DWMWA_EXTENDED_FRAME_BOUNDS`; activate via `SetForegroundWindow` (+Alt-tap workaround); close via `WM_CLOSE` |
| workspace HUD | `SetWinEventHook` (foreground / object create-destroy-move) feeds the existing `state_packet`; workspaces empty |
| exec | `cmd /C` with `CREATE_NO_WINDOW` |
| siren | Bundled WAV looped via `PlaySound`; unmute + raise master volume through §6, restore on silence |
| file_browse / share | `dirs` (portable); `"/"` lists drive roots (`GetLogicalDriveStringsW`) on Windows |
| notification_monitor (dbus) | Linux-only |

**Platform helpers** (`server/src/platform/windows.rs`): `local_ips()` via
`GetAdaptersAddresses` (fixes empty mDNS registration); `hostname()` from
`COMPUTERNAME`; `secure_write()` with an owner-only DACL for the iroh key and
certs; `stop_pid()` via a named event `Local\LinuxLink-<pid>-stop` the server
waits on, so `linux-link stop` shuts down gracefully.

**Linux-side fixes surfaced by the audit, each its own commit:**
`CertManager` key write gets `secure_write` (0600) on Linux too; Linux gains a
SIGTERM handler so `linux-link stop` runs the PID-file guard.

**Tests.** Pure units for window filtering, drive listing and the hook's
injected-flag predicate; `#[ignore]`d live tests for clipboard round-trip,
battery read, `EnumWindows` seeing a spawned Notepad, shield capture-exclusion,
and hook TTL release.

## 8. Service, packaging, CI, docs

- **Run/autostart.** One `linux-link.exe`, same CLI verbs. New Windows verbs
  `install` / `uninstall`: per-user Scheduled Task at logon (limited token,
  restart on failure; `--elevated` opt-in), Start-menu shortcut carrying the
  AUMID, firewall rules for 1716/tcp, 1716/udp, 4716/udp scoped to Private +
  the Tailscale interface (only this step elevates, via `runas`). No Windows
  Service — Session 0 cannot see the desktop.
- **Paths.** State `%LOCALAPPDATA%\linux-link\`, config
  `%APPDATA%\linux-link\config.toml` (both via `dirs`). Nothing next to the
  binary.
- **Console/logs.** Task runs `--background` (`FreeConsole`); logs to a
  daily-rolling file in the state dir (7 kept) via `tracing-appender`. Linux
  keeps journald.
- **Packaging.** (1) Portable zip: `linux-link.exe`, `ffmpeg.exe` (license in
  `THIRD_PARTY.md`), siren WAV, README-windows — ships at W1. (2) Sparse MSIX
  identity package (self-signed dev cert) solely for the notification mirror —
  W5, optional.
- **CI.** `ci.yml` gains a `windows-latest` MSVC job (fmt, clippy `-D warnings`,
  build, tests with live tests `#[ignore]`d; workspace runs exclude
  `linux-link-android-bridge` and `iroh-spike`). `release.yml` gains the Windows
  zip. Both are unverified until first pushed run (the user's call).
- **Docs.** README "Windows host" section; `docs/development-setup.md` Windows
  toolchain; `docs/capabilities-windows.md` (generated); on-device checklist
  §22 "Windows host"; AGENTS.md Current Status bullet per phase + Windows
  gotchas.

## 9. Phases

| Phase | Contents | Exit gate |
|---|---|---|
| W0 cfg skeleton | Pure Linux moves + target tables; Windows compiles with every OS hook returning "unsupported" | Linux gates green on the Arch box; Windows build/clippy/test green |
| W1 core remote desktop | WGC + DXGI; NVENC/AMF/QSV/MF/x264 via sidecar + reader thread; SendInput; clipboard; local_ips/hostname/secure_write/stop; toasts; install/firewall/logon task; portable zip | Phone pairs, streams and drives this PC over LAN and the tailnet with a hardware encoder on the HUD; NVENC + AMF bench baselines committed |
| W2 audio | WASAPI loopback, audio control, mic relay | Live tests green; volume/mute from the phone verified |
| W3 plugins | battery, power, exec, windows + crop + WinEvent HUD feed, monitors, drives, siren | Each exercised from the phone; checklist §22 filled |
| W4 privacy | LL hooks + TTL watchdog, blackout, capture-excluded shield | Local input blocked while phone input passes; TTL release proven |
| W5 finish | Sparse-MSIX notification mirror, in-process ffmpeg-next, CI job, release zip | Full-parity checklist complete |

Then sub-project B (Windows client) gets its own brainstorm → spec → plan on
`feat/windows-client`, using this host as its test target.

## 10. Risks

- **Notification mirror** depends on package identity and a consent prompt —
  highest parity risk; isolated to W5 and optional.
- **`IPolicyConfig`** is undocumented; isolated to one function with a
  degrade path.
- **Linux regressions from pure moves** can only be caught on Linux; each move
  commit is listed for an Arch-box gate run before merge.
- **Hosted CI** has no GPU or interactive desktop; Windows live tests are
  box-verified only.
