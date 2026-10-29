<p align="center">
  <img src="assets/brand/pocketdesk-banner.png" alt="Pocketdesk: your desktop. Anywhere." width="100%">
</p>

<h1 align="center">Pocketdesk</h1>

<p align="center"><b>Your Windows or Linux computer, on your Android phone.</b><br>
Its live screen under your fingers, its sound, and the clipboard, files, notifications and texts moving both ways: on your network, through Tailscale, or over the internet with no port forward and no account.</p>

<p align="center">
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Rust-Pure%20Rust-orange?logo=rust" alt="Rust"></a>
  <a href="#license"><img src="https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue" alt="License: MIT OR Apache-2.0"></a>
  <a href="https://github.com/sreevarshan-xenoz/Pocketdesk/actions"><img src="https://github.com/sreevarshan-xenoz/Pocketdesk/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/sreevarshan-xenoz/Pocketdesk/issues"><img src="https://img.shields.io/github/issues/sreevarshan-xenoz/Pocketdesk" alt="Issues"></a>
  <a href="docs/guide.md#roadmap"><img src="https://img.shields.io/badge/status-beta-orange" alt="Status: beta"></a>
</p>

<p align="center">
  <a href="assets/brand/pocketdesk-launch.mp4"><img src="assets/brand/pocketdesk-launch.gif" alt="Pocketdesk in 32 seconds: drive the computer from the phone, the phone's notifications, calls and screen on the computer, files and the clipboard both ways, the computer's state, deck and music on the phone, from anywhere" width="800"></a><br>
  <sub>▶ <a href="assets/brand/pocketdesk-launch.mp4">Watch it with sound (32 seconds)</a></sub>
</p>

## What is Pocketdesk?

A **pure Rust** server for Linux (Wayland-native, Hyprland first) and Windows 10/11, a desktop app to manage it, and a native Android app. Together they give you:

- **Your screen on the phone, at low latency:** hardware H.264/HEVC over QUIC, decoded by the phone's MediaCodec.
- **Control from the phone:** touch, a trackpad with gestures, a pen with pressure, a gamepad, an air mouse and a full keyboard, with the desktop's sound on the phone.
- **The KDE Connect set, both ways:** the clipboard, files and a remote file browser, the phone's notifications, texts and calls on the computer, ringing either one to find it, and waking the computer.
- **The phone on the computer:** its screen or camera in a window you can drive with the mouse, its photos and folders, its music in the computer's media controls.
- **Your computers together:** one keyboard and mouse across them (Windows driving Linux), with the clipboard, files and sound following the pointer, the phone as another screen, and one computer's screen in a window on another.
- **From anywhere:** on your network, through Tailscale, or over the internet with no port forward and no VPN (iroh punches a direct path, or uses a relay).
- **Yours alone:** no account and no Pocketdesk server. Each end holds the other to its key (TLS 1.3), and each phone has its own permissions.

## Quickstart

**1. Install it on your computer** from the [latest release](https://github.com/sreevarshan-xenoz/Pocketdesk/releases/latest):

| Computer | Install |
|---|---|
| Windows 10/11 | Run `pocketdesk-…-windows-x86_64-setup.exe`. It starts Pocketdesk at sign-in, opens its firewall ports and offers to install FFmpeg. |
| Debian, Ubuntu | `sudo apt install ./pocketdesk-…-linux-amd64.deb` |
| Other Linux | `curl -fsSL https://raw.githubusercontent.com/sreevarshan-xenoz/Pocketdesk/main/scripts/install.sh \| bash` (needs PipeWire and FFmpeg). On Arch: `yay -S pocketdesk` ([AUR](https://aur.archlinux.org/packages/pocketdesk)). |

**2. Install the app on your phone** (Android 8 or newer): `pocketdesk-…-android-arm64.apk` from the same release, or add `https://github.com/sreevarshan-xenoz/Pocketdesk` to [Obtainium](https://github.com/ImranR98/Obtainium) to keep it updated.

**3. Pair them.** Open Pocketdesk on the computer and go to its **Pair** page. On the phone, tap **Scan pairing code** and point it at the QR code. Do this once with both on the same network (or tailnet); after that the phone reaches the computer from anywhere. If the app says the server is not running, press **Start server** on its Overview page.

Something not working? The desktop app's **Diagnostics** page checks the server, its ports, the firewall and the internet path, and copies a report you can paste into an issue.

## How it compares

| Feature | Pocketdesk | RustDesk | Sunshine/Moonlight | KDE Connect |
|---------|:----------:|:--------:|:-------------------:|:-----------:|
| From outside without a port forward or VPN | ✅ (after one session at home) | ✅ | ⚠️ UPnP or a forward | ❌ |
| Wayland-native | ✅ | ⚠️ Partial | ⚠️ Partial | ✅ |
| Hyprland optimized | ✅ | ❌ | ⚠️ Limited | ⚠️ Limited |
| Works with KDE Connect devices | ⚠️ Pairs, ping, clipboard, battery, ring, notifications, files both ways | ❌ | ❌ | ✅ |
| Screen streaming | ✅ | ✅ | ✅ | ❌ |
| Rust server, Kotlin phone app | ✅ | ✅ (Rust) | ❌ (C++) | ❌ (C++) |
| Open Source | ✅ | ✅ | ✅ | ✅ |

**Status: beta.** Partly device-checked: Hyprland on a laptop and Sway in a VM (on Linux, not yet files, notifications or sound), and Windows with a Galaxy A71 and an OPPO phone. GNOME and KDE Plasma are supported but not yet checked. The shared keyboard and mouse (new in 0.8.1) is checked only in parts, from scripts: its receiving end on the Linux laptop and the phone as a screen; Windows driving Linux at the desk is not yet. [What works where](docs/guide.md#what-works-where) has the details, and [the checklist](docs/device-verification-checklist.md) every check still open.

## Learn more

- [Before you start](docs/guide.md#before-you-start): which network carries what, trust at first contact, and what the internet path reveals.
- [Features](docs/guide.md#features), [installing every other way](docs/guide.md#installation), [commands and settings](docs/guide.md#usage), and [architecture](docs/guide.md#architecture): all in the [guide](docs/guide.md).
- [SECURITY.md](SECURITY.md): read it before using Pocketdesk on a network you do not control.
- [Privacy policy](docs/privacy.md): no account, no analytics; which public services it uses by default, and how to turn each off.
- [docs/protocol.md](docs/protocol.md): everything that is sent, and to whom.

## Community

- **Questions, ideas, "it worked on my setup":** [Discussions](https://github.com/sreevarshan-xenoz/Pocketdesk/discussions).
- **Something broken:** [open an issue](https://github.com/sreevarshan-xenoz/Pocketdesk/issues/new/choose), with the report from the Diagnostics page.
- **Security problems:** privately, through **Security → Report a vulnerability** ([SECURITY.md](SECURITY.md)).
- **Contributing:** [CONTRIBUTING.md](CONTRIBUTING.md), and [building from source](docs/guide.md#build-from-source).

## License

Pocketdesk is licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as
above, without any additional terms or conditions.

---

**Stars, forks, and PRs are welcome!** If Pocketdesk sounds interesting to you, drop a ⭐ and say hi.
