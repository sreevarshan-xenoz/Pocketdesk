# Pocketdesk

**Your desktop from your phone: low-latency remote desktop for Linux (Wayland-native) and Windows, plus the KDE Connect feature set — over your LAN, Tailscale or a hole-punched iroh path. No port forwarding required.**

> Formerly *Linux Link* (this repository was `Linux-Link`; old links redirect). The name changed when Windows support landed. The `linux-link` command, its config/state paths, the wire protocol and the Android package id (`dev.linuxlink.android`) keep the old name on purpose, so existing installs, pairings and app updates carry over.

[![Rust](https://img.shields.io/badge/Rust-Pure%20Rust-orange?logo=rust)](https://www.rust-lang.org/)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue)](#license)
[![CI](https://github.com/sreevarshan-xenoz/Pocketdesk/actions/workflows/ci.yml/badge.svg)](https://github.com/sreevarshan-xenoz/Pocketdesk/actions)
[![Issues](https://img.shields.io/github/issues/sreevarshan-xenoz/Pocketdesk)](https://github.com/sreevarshan-xenoz/Pocketdesk/issues)
[![Status](https://img.shields.io/badge/status-beta%20-%20feature%20complete%2C%20gates%20green-orange)](#roadmap)

> **Target:** Sub-100ms latency screen streaming + KDE Connect integration + zero-config Tailscale connectivity

## What is Pocketdesk?

Pocketdesk is a **pure Rust** remote desktop solution, built first for Linux (Wayland/Hyprland) and now also for Windows 10/11. It combines:

- **Low-latency screen streaming** — wlroots/PipeWire capture → FFmpeg H.264 encoding → QUIC transport → Android MediaCodec decode
- **KDE Connect-style features** — clipboard sync, files both ways and a remote file browser (LAN), notifications, input control, battery info
- **Tailscale-native connectivity** — No manual port forwarding or NAT traversal; just pair and connect

### Key Differentiators

| Feature | Pocketdesk | RustDesk | Sunshine/Moonlight | KDE Connect |
|---------|:----------:|:--------:|:-------------------:|:-----------:|
| Tailscale-native | ✅ | ❌ | ❌ | ❌ |
| Wayland-native | ✅ | ⚠️ Partial | ⚠️ Partial | ✅ |
| Hyprland optimized | ✅ | ❌ | ⚠️ Limited | ⚠️ Limited |
| Full KDE Connect | ✅ | ⚠️ Partial | ❌ | ✅ |
| Screen streaming | ✅ | ✅ | ✅ | ❌ |
| Pure Rust | ✅ | ✅ | ❌ (C++) | ❌ (C++) |
| Open Source | ✅ | ✅ | ✅ | ✅ |

## Features

### Screen Streaming
- **Native wlroots capture** — direct `zwlr_screencopy` on Hyprland (no portal grant dialog), damage-driven variable frame rate; automatic fallback to **PipeWire capture** via XDG Desktop Portal on any other Wayland compositor, X11 grab last
- **FFmpeg H.264 encoding** with persistent sidecar process for low latency
- **QUIC transport** (datagram mode) with self-signed TLS certificates
- **Adaptive bitrate** — one arbiter owns every rate change: a 2 Mbit/s courtesy ceiling while a WAN session rides a relay, the session's quality preset, and a loss response that backs the encoder off a congested link (20 % per congested tick, to a 1 Mbit/s floor) and walks it back up once the link goes clean
- **MediaCodec hardware decode** on the native Android client via JNI
- **Single-window streaming** (Hyprland) — pick any window on the phone and the server crops + re-encodes to just that window

### KDE Connect Integration
- **Clipboard sync** — bidirectional clipboard sharing via `wl-clipboard`
- **File transfer** — files both ways over the LAN; each transfer ends with the receiver confirming the save
- **Remote file browsing** — browse the computer's home folder from a session and download a file to the phone's Downloads/Pocketdesk
- **Notifications** — receive Android notifications on your Linux desktop
- **Notification reply** — answer desktop notifications from the phone; the desktop records the reply, copies it to the clipboard and shows a confirmation
- **Find my device** — ring the desktop from the phone, or ring the phone from the desktop
- **PIN pairing** — 6-digit PIN pairing (shown on the desktop or printed by `linux-link pair`); unpaired connections are locked out — the control channel (TCP and v2/QUIC) and the video/input stream all enforce it — unless `pairing_required = false`
- **Privacy mode** — block the desktop's physical keyboard+mouse while remote (EVIOCGRAB with a 10-min auto-release TTL), lock the desktop from the phone or its session notification; uinput remote input keeps flowing
- **View-only mode** — one tap makes the phone a pure viewer: the *server* drops all remote input for the session (video, clipboard and HUDs keep working), so nothing slips through from a stale tap queue; re-arms automatically after a stream reconnect
- **Relay bandwidth courtesy** — a WAN session riding a relay automatically drops to a conservative video bitrate (relay bandwidth is shared, not ours to saturate) and restores full quality the moment it punches through to direct; one tap on the "Full quality" toggle overrides the floor if you want every bit of it
- **Desktop audio control** — adjust the desktop's volume, mute, and default output device (headphones/speakers) from the phone (wpctl/pactl under the hood)
- **Desktop sound on the phone** — what the desktop plays comes out of the phone during a session (Opus, low-latency); a Quick Settings switch turns it off
- **In-session keyboard** — type into the desktop with the phone's own keyboard (live typing, swipe and autocorrect work), with a bar of latching Ctrl/Alt/Shift/Super, Esc, Tab, arrows, Home/End/PgUp/PgDn, F1–F12 and Ctrl+Alt+Del; a hardware keyboard works too
- **Laptop-style trackpad** — one-finger tap left-clicks, two- and three-finger taps right- and middle-click, tap-then-drag holds the button, two fingers scroll, and the cursor speed is a slider in Settings
- **Phone mic share** — one tap turns the phone's microphone into a desktop input device ("Pocketdesk Mic" PipeWire source, Opus over the session stream): take calls on the PC from the phone; stays live in view-only mode and dies with the session
- **Wake-on-LAN relay** — wake a sleeping desktop from WAN by asking an always-on Linux peer on its LAN to emit the magic packet ("Send Wake-on-LAN" in the app)
- **Input control** — remote mouse/keyboard via trackpad gestures
- **Battery info** — monitor Android device battery from Linux
- **Presenter mode** — play/pause/next/previous from Android

### Android Client
- **Native Kotlin app** (Jetpack Compose, Material 3 dark theme)
- **Rust core** reused via a JNI/UniFFI bridge (no Flutter)
- **Computers on this network** — Home lists Pocketdesk computers on the same Wi-Fi (mDNS); a tap connects and an unpaired one asks for its PIN. Saved computers that are reachable get an "On this network" label
- **Remote desktop** with tap/drag/double-tap gesture input
- **Link status** — connecting/LAN/WAN indicator on the video surface, with failure reasons and one-tap retry
- **Session resilience** — QUIC keepalive + bounded idle timeout hold the link through NAT stalls; a wake-locked foreground service keeps screen-off sessions alive
- **Picture-in-picture** — shrink the live session into a video-only PiP window (desktop aspect ratio, chrome hidden) and keep working in other apps; also enables DeX/docked windows
- **Blackout / pocket mode** — black, touch-locked screen while the session keeps streaming: the phone and its video surface go secure (no screenshots or Recents leakage), brightness drops; double-tap or back to unlock
- **Localized UI** — English, Spanish and Tamil; pick per-app language from the "Language" button on the connect screen (Android 13+)
- **Share to the desktop** — share text (to the desktop clipboard) or one or more files (to its Downloads folder, over the LAN) from any app, with progress in a notification; the desktop shows a notification per file. "Sent" means the desktop saved it
- **Files on this computer** — in a session, browse the computer's home folder (hidden files left out) and download a file to Downloads/Pocketdesk, with progress in a notification. Files the desktop app sends arrive the same way while the session is up
- **Every screen, one tap away** — with more than one monitor, the session's controls show a numbered chip per screen; tap one to switch the stream to it (Quick Settings → Monitor still lists them by name)
- **Settings** with DataStore persistence

### Server
- **CLI** with start/stop/status/list/watch/connect/pair/capabilities commands
- **systemd service** for auto-start on boot
- **TOML configuration** with video quality presets (low/balanced/high)
- **Tests** across core, server, the desktop app and the Android bridge, plus JVM tests for the app; CI runs them on Linux and Windows for every pull request

## Installation

### Quick Install (Recommended)

```bash
# Install latest release
curl -fsSL https://raw.githubusercontent.com/sreevarshan-xenoz/Pocketdesk/main/scripts/install.sh | bash

# Install specific version
curl -fsSL https://raw.githubusercontent.com/sreevarshan-xenoz/Pocketdesk/main/scripts/install.sh | bash -s -- v0.2.0

# Non-interactive install (no prompts)
curl -fsSL https://raw.githubusercontent.com/sreevarshan-xenoz/Pocketdesk/main/scripts/install.sh | bash -s -- --yes

# Install to custom prefix
curl -fsSL https://raw.githubusercontent.com/sreevarshan-xenoz/Pocketdesk/main/scripts/install.sh | bash -s -- --prefix /opt

# Preview without changes
curl -fsSL https://raw.githubusercontent.com/sreevarshan-xenoz/Pocketdesk/main/scripts/install.sh | bash -s -- --dry-run
```

### Install Script Options

| Flag | Description |
|------|-------------|
| `--yes`, `-y` | Non-interactive; accept all defaults |
| `--dry-run` | Preview actions without making changes |
| `--verbose` | Show detailed debug output |
| `--force` | Force reinstall even if same version |
| `--no-service` | Skip systemd service installation |
| `--no-config` | Skip config file creation |
| `--no-docs` | Skip documentation installation |
| `--no-man` | Skip man page installation |
| `--prefix PATH` | Install prefix (default: `/usr`) |
| `--check-updates` | Check if installed version is current |
| `--list-versions` | List all available releases |
| `--status` | Show installation status |
| `--rollback` | Roll back to previous version |
| `--uninstall` | Remove installation |

**Management commands** (after install):

```bash
linux-link status             # Tailscale status of this machine
linux-link sessions           # Recent streaming-session outcomes
linux-link bench --baseline bench/baselines   # Encode tail vs the committed baseline
linux-link capabilities       # KDE Connect capability sets in use
linux-link pair --grant 15m   # Time-boxed one-off support PIN
linux-link kick <id|prefix|peer-ip|all>   # Drop a live session
```

The update/rollback/uninstall verbs belong to the install script, not the binary:

```bash
./scripts/install.sh --status           # Installation info (prefix, version, service)
./scripts/install.sh --check-updates    # Compare installed version with GitHub
./scripts/install.sh --list-versions    # List published releases
./scripts/install.sh --rollback         # Return to the previously installed version
./scripts/install.sh --uninstall        # Remove the installation
```

### Build from Source

```bash
# Clone the repo
git clone https://github.com/sreevarshan-xenoz/Pocketdesk.git
cd Pocketdesk

# Build the workspace
cargo build --release

# Run tests
cargo test --workspace

# Run lints
cargo fmt --check
cargo clippy --workspace -- -D warnings
```

### Windows (preview)

The server also runs on Windows 10 1903+ / 11, so the same Android app can
view and control a Windows PC: Windows.Graphics.Capture (DXGI duplication as
fallback), NVENC/AMF/Quick Sync/Media Foundation through `ffmpeg.exe`,
`SendInput` with scancodes, the Win32 clipboard and toasts. It also covers:
- desktop audio (WASAPI loopback) and volume/output control;
- the phone's mic, through a virtual audio cable;
- privacy mode: local input blocked, plus a ring that does not appear in the stream;
- battery, power, exec, siren and the window HUD.

Unzip `linux-link-<version>-x86_64-pc-windows-gnu.zip` from the releases page
(one self-contained exe), install FFmpeg (`winget install Gyan.FFmpeg`, which
puts `ffmpeg.exe` on `PATH`), then:

```powershell
linux-link.exe install   # start at logon + open the firewall (one UAC prompt)
linux-link.exe pair      # pairing PIN, also shown as a toast
```

`install` registers a per-user logon task (Windows' answer to the systemd user
unit; a Windows service cannot see the desktop), `uninstall` removes it. Input
cannot reach elevated windows or the UAC/lock screen unless the server runs
elevated (`install --elevated`). Mirroring desktop notifications to the phone
needs a package identity that the plain exe does not have, so it stays off and
says so in the log. Build notes: [docs/development-setup.md](docs/development-setup.md) §4.

### AUR (Arch Linux)

```bash
# Using an AUR helper
yay -S linux-link

# Or manually
git clone https://aur.archlinux.org/linux-link.git
cd linux-link
makepkg -si
```

## Usage

### Start the Server

```bash
# Using the installed binary
linux-link start

# Or with cargo
cargo run --release --bin linux-link -- start
```

There is no `--config` flag: the server reads `$XDG_CONFIG_HOME/linux-link/config.toml`
(default `~/.config/linux-link/config.toml`) if present, and falls back to built-in defaults.
`-v`/`--verbose` and `RUST_LOG` control logging.

### Desktop App

`pocketdesk` is the companion window for the server; it runs on Linux and Windows. It works through the files the server already uses, so it runs whether or not the server is up, and it opens no port.

- **Status**: whether the server runs, with Start and Restart.
- **Pair**: a QR code and the same PIN in digits. The phone's "Scan pairing code" reads it and pins the desktop's certificate before its first streaming connection.
- **Devices**: paired phones, which one is connected, and Remove. Each phone also has switches for what it may do: control, clipboard, files, sound, microphone, power and privacy, and running commands. Running commands is off until you turn it on. "Ask before each session" holds each of that phone's sessions until you allow it.
- **Approval prompt**: when a phone that must ask connects, the window comes to the front with **Allow once**, **Always allow** and **Deny**. No answer within 30 s is a deny.
- **Sessions**: live sessions, with End.
- **Files**: send a file to a phone that is in a session (pick the phone, then a path, "Choose file…" on Windows, or drop files on the window), the files sent and whether each was saved, and files received from phones, newest first, with Show in folder.
- **Settings**: edits `config.toml`, keeping your comments and unknown keys. "Visible on this network" turns the LAN announcement off (`lan_discovery`).
- **Logs**: the server's log.

Closing the window keeps it in the tray on Windows; Linux has no tray yet, so there closing it quits.

```bash
cargo run --release -p pocketdesk-desktop   # or run the installed pocketdesk / pocketdesk.exe
```

A phone always asks before it acts on a pairing code (a `pocketdesk://pair` link can come from anywhere). A code can never replace the certificate the phone already trusts for an address; remove that computer in the app first. The PIN still travels over the control channel, which is not encrypted, the same as typing it by hand.

### Configuration

Copy the example config and customize:

```bash
mkdir -p ~/.config/linux-link
cp config.toml.example ~/.config/linux-link/config.toml
```

```toml
# ~/.config/linux-link/config.toml
control_port = 1716        # KDE Connect compatible
streaming_port = 4716      # QUIC streaming port
log_level = "info"         # trace/debug/info/warn/error
video_quality = "balanced" # low/balanced/high
pairing_required = true    # PIN-pair devices before the control channel or streaming serves requests
# allow_hevc = false       # let negotiating phones drive an H.265/HEVC stream (R4 C1)
# lan_discovery = true     # announce this computer so phones on the network list it
```

### systemd Service

The server captures the desktop session (Wayland/X11) and talks to PipeWire, so
it runs as a **systemd user service** — a system unit started at boot could
never see the graphical session. `install.sh` drops the unit into
`~/.config/systemd/user/` and enables it for you; from a desktop session:

```bash
systemctl --user enable --now linux-link
systemctl --user status linux-link
journalctl --user -u linux-link -f
```

The unit is `WantedBy=graphical-session.target`, so it starts at login and
stops with the session. To run it without a login session (headless), enable
lingering first: `sudo loginctl enable-linger $USER`.

### CLI Commands

| Command | Description |
|---------|-------------|
| `linux-link start` | Start the server daemon |
| `linux-link stop` | Stop the running daemon |
| `linux-link status` | Show connection status |
| `linux-link list` | List available peers on tailnet |
| `linux-link watch` | Watch for peer discovery events |
| `linux-link connect <peer>` | Connect to a specific peer |
| `linux-link pair [pin] [--grant 15m]` | Print a pairing PIN for the phone (generate or set): valid 5 minutes, for one phone, and discarded after 5 wrong guesses; `--grant` time-boxes the trust pairing stores (`s/m/h/d`, e.g. `45s`, `15m`, `2h`, `1h30m`) for one-off support sessions |
| `linux-link unpair [device-id]` | Remove a paired device from the trust store (all if omitted) |
| `linux-link sessions [--count N] [--json]` | Show recorded streaming-session outcomes (LAN/WAN-punched/WAN-relayed tally + recent log tail); each line carries the transport's own numbers — packets/bytes lost, datagrams sent, path changes, seconds rode a relay, peak congestion window, discovered path MTU (read off the path the session actually rides, on both transports) — and the p50/p90/p95/p99/max tails for the whole chain: `rtt_*`/`enc_*` measured on the desktop, `dec_*`/`rnd_*`/`e2e_*` reported back by the device doing the decoding, plus its reading of the path as `phone_rtt`/`phone_lost`; `--json` emits the retained per-session records as JSON lines for tooling and benchmark comparison |
| `linux-link bench [--target software\|vaapi] [--baseline PATH] [--record PATH]` | Run the pinned 720p encode clip on this machine; with `--baseline`, exit non-zero if a percentile regressed past `--tolerance-pct` (exit 3 means no baseline matches this host, which is a skip, not a pass) |
| `linux-link capabilities [--json\|--markdown]` | Show the negotiated protocol versions, transports, capture backends and codecs this build actually speaks, plus its KDE Connect capability sets |
| `linux-link kick <id\|prefix\|peer-ip\|all>` | Drop a live streaming session (R4 D2) |
| `linux-link start --background` | Run detached, logging to daily files under the state directory (what the Windows logon task runs) |
| `linux-link install [--elevated]` | *Windows:* register the logon task, start it, and open the firewall ports |
| `linux-link uninstall` | *Windows:* remove the logon task, firewall rules and notification identity |

### Capabilities Are Generated, Not Written

`linux-link capabilities` is the single source of truth for the four numbers and
lists a reader otherwise has to trust prose for: the negotiated protocol version,
which transports a given build carries, which capture backends exist and in what
order `Auto` tries them, and which codecs can be encoded/decoded on each side.
Every value is read from the constants and decision tables the wire uses —
`protocol::{HANDSHAKE_HELLO, ALPN_V1_STREAM}`, `protocol::v2::{ALPN_V2, V2_MIN_VERSION..}`,
`streaming::capture::capture_attempts`, `VideoCodec::ALL`, `AudioConfig::default()` —
so a client-profile build reports `capture: unavailable` instead of an empty list,
and a version bump appears in the report the moment it appears in the code.

```bash
linux-link capabilities              # human listing
linux-link capabilities --json       # for scripts
linux-link capabilities --markdown   # the source of docs/capabilities.md
```

[`docs/capabilities.md`](docs/capabilities.md) is that Markdown output, committed,
and `server/tests/capabilities_doc.rs` fails when it stops matching the build —
regenerate it in the same commit as the constant you changed.

### The Encoder Benchmark Is Pinned, Not Improvised

`linux-link bench` exists because "it felt laggy today" is not a regression test. It encodes
one generated 720p clip — 300 frames, 5 Mbit/s, veryfast, H.264 — and reports the per-frame
encode distribution (`p50/p90/p95/p99/max`). The clip is a pure function of its frame index
(`scene_frame`), so the workload is byte-identical between runs without committing a gigabyte
of raw frames, and it mixes a static gradient with a moving block so the encoder cannot coast
on zero-residual skip blocks and measure nothing.

```bash
linux-link bench --target vaapi --repeat 5           # measure this machine
linux-link bench --baseline bench/baselines          # grade against a committed record
linux-link bench --record bench/baselines            # add this machine's record
```

A baseline is only comparable to a run on the same host, backend, geometry, bitrate, preset
and codec — anything else is reported as `SKIP` (exit 3), never as a pass, because encode
throughput does not travel between CPUs. `--repeat N` keeps the fastest run: other work on the
machine can only make an encode slower, and on a desktop with a load average near 6 the best of
seven runs reproduced the quiet-machine number (p50 7 ms) while a single run reported 11 ms.
Encode time is the half this measures; decode and render live on the phone. CI runs the same
command on every push — see [`bench/README.md`](bench/README.md).

### Man Page

```bash
man linux-link
```

## Android Client

The Android client is a native Kotlin app in `android/`, bridged to the shared Rust core via JNI/UniFFI.

```bash
cd android
./gradlew assembleDebug   # Debug build
./gradlew assembleRelease # Release build
```

Requires JDK 17+ and Android SDK/NDK.

## Security

Read [SECURITY.md](SECURITY.md) before using Pocketdesk on a network you do not control. In short: the video stream is encrypted (TLS 1.3 over QUIC) and both ends prove their keys on it, but the control channel and file transfers are not encrypted, and on the control channel a paired phone is still identified by an id it announces rather than a key it proves. On anything but a trusted home network, run it over Tailscale (or another VPN) and keep "Ask before each session" on. Report vulnerabilities privately through GitHub's **Security → Report a vulnerability**.

## Architecture

```
┌─────────────────────────┐         Tailscale          ┌─────────────────────────┐
│   Android Client        │◄──── Encrypted P2P ────────│   Linux Server          │
│   (Kotlin + Rust FFI)   │        (Tailscale IP)      │   (Hyprland)            │
├─────────────────────────┤                            ├─────────────────────────┤
│  UI Layer (Compose)     │                            │  Rust Daemon (tokio)    │
│  ├── Connection Screen  │                            │  ├── Screen Capture     │
│  ├── Remote Desktop     │                            │  ├── FFmpeg H.264 Enc.  │
│  ├── File Browser       │                            │  ├── QUIC Transport     │
│  └── Settings           │                            │  └── Adaptive Bitrate   │
├─────────────────────────┤                            ├─────────────────────────┤
│  Rust Backend (FFI)     │                            │  KDE Connect Plugins    │
│  ├── Connection Mgr     │                            │  ├── Battery            │
│  ├── Video Decoder      │                            │  ├── Clipboard          │
│  ├── File Transfer      │                            │  ├── Notification       │
│  └── Input Handler      │                            │  ├── Share              │
└─────────────────────────┘                            │  ├── File Browse        │
                                                       │  └── Input              │
                                                       └─────────────────────────┘
```

**Data Flow (Streaming):**
```
PipeWire → BGRA Frame → FFmpeg H.264 → QUIC Datagram → MediaCodec → SurfaceView
```

**Data Flow (Input):**
```
Touch Gesture → Rust FFI → QUIC/TCP → KDE mousepad packet → enigo → Wayland
```

## Project Structure

```
Pocketdesk/
├── core/                   # Shared Rust library
│   └── src/
│       ├── protocol/       # KDE Connect protocol + connection handling
│       ├── local/          # Files the server and the desktop app share (config, PIN, sessions)
│       ├── streaming/      # Capture, encoder, QUIC transport, adaptive bitrate
│       └── tailscale/      # Tailscale integration
├── desktop/                # Pocketdesk desktop app (egui): pair, devices, sessions, settings, logs
├── server/                 # Server daemon (Linux and Windows)
│   └── src/
│       ├── plugins/        # KDE plugins (5 + file browse)
│       ├── cli.rs          # CLI argument parsing
│       ├── config.rs       # TOML configuration
│       ├── kde.rs          # Plugin registry + service setup
│       └── service.rs      # Main server loop
├── android/                # Android client (native Kotlin)
│   ├── app/                # Kotlin app module (UI, services, MediaCodec decode)
│   └── bridge/             # Rust cdylib crate (UniFFI/JNI bindings to core)
├── aur/                    # AUR packaging (PKGBUILD)
├── docs/                   # Design specs and plans
├── man/                    # Man pages
├── scripts/                # Install script
├── .github/workflows/      # CI/CD (build, test, release, audit)
├── CHANGELOG.md            # Project changelog
├── CONTRIBUTING.md         # Contributor guidelines
├── config.toml.example     # Example configuration
├── linux-link.service      # systemd service file
└── plan.md                 # Full development plan
```

## Quality Gates

| Check | Status |
|-------|--------|
| `cargo fmt` | ✅ Pass |
| `cargo clippy -D warnings` | ✅ Pass (0 warnings) |
| `cargo test --workspace` | ✅ 52 tests pass |
| `cargo check --workspace` | ✅ Clean compilation |

## Contributing

Pocketdesk is actively developed and looking for contributors!

### Prerequisites

- Rust 1.80+ (edition 2024)
- JDK 17+ and Android SDK/NDK (for the Kotlin client)
- Tailscale (for testing)
- FFmpeg (runtime **and** dev libraries — the server encodes in-process via
  `ffmpeg-next`), PipeWire, xdg-desktop-portal (for streaming)

### Development Workflow

```bash
# Build + test
cargo build --workspace && cargo test --workspace

# Format + lint
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# Android build
cd android && ./gradlew assembleDebug
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for full guidelines.

## Roadmap

All 6 development phases and the RustDesk research round (R4) have landed in code: the native
Kotlin client, the Rust↔JNI bridge, MediaCodec decode, and the streaming/transport/security stack
are in `main`. Releases are built by [the release workflow](.github/workflows/release.yml): a `vX.Y.Z` tag (or `vX.Y.Z-beta.N` for a pre-release) that matches the workspace and Android versions publishes Linux, Windows and Android files with a `SHA256SUMS`.

What is *not* done is verification and hardening, so treat this as beta rather than a release:

- [x] `cargo fmt --all -- --check`, `clippy -D warnings` across every `core` feature profile and
      `cargo test --workspace` all pass from a clean checkout of `main` (verified 2026-09-24).
      [CI](.github/workflows/ci.yml) runs that matrix, a Windows job and the Android build on every
      pull request, and `cargo audit` can fail a run.
- [ ] On-device verification for the large majority of shipped Android features (see
      [docs/device-verification-checklist.md](docs/device-verification-checklist.md))
- [ ] Cross-network (cellular ↔ LAN) WAN test over iroh; the Tailscale path is device-verified
- [ ] The first published release (`v0.1.0` was a tag only; the workflow is ready, and a `v0.2.0-beta.1` tag cuts the first one)

The full backlog, with every item's verified status against this tree, is
[docs/roadmap-3000.md](docs/roadmap-3000.md); how that backlog gets executed is
[docs/roadmap-execution-plan.md](docs/roadmap-execution-plan.md). See [plan.md](plan.md) for the
development journal.

## License

Pocketdesk is licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as
above, without any additional terms or conditions.

---

**Stars, forks, and PRs are welcome!** If Pocketdesk sounds interesting to you, drop a ⭐ and say hi.
