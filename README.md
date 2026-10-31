<p align="center">
  <img src="assets/brand/pocketdesk-banner.png" alt="Pocketdesk: your desktop. Anywhere." width="100%">
</p>

<h1 align="center">Pocketdesk</h1>

<p align="center"><b>Your Windows or Linux computer on your Android phone, and your computers on each other.</b><br>
Its live screen under your fingers, its sound, and the clipboard, files, notifications and texts moving both ways: on your network, through Tailscale, or over the internet with no port forward and no account.</p>

<p align="center">
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Rust-orange?logo=rust" alt="Written in Rust"></a>
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

A server for Linux (Wayland-native, Hyprland first) and Windows 10/11, a desktop app to manage it, and a native Android app. It is written in Rust and uses FFmpeg for video encoding. Together they give you:

**Your computer on your phone**
- **The screen at low latency:** hardware H.264/HEVC over QUIC, decoded by the phone's MediaCodec, with the computer's sound.
- **Control:** touch, a trackpad with gestures, a pen with pressure, a gamepad, an air mouse and a full keyboard.
- **Peek:** one still of the screen with no session, off until you allow it for each phone. **How it is doing** shows CPU, memory, disks and battery the same way.
- **A second screen at the phone's shape** on Sway and on Hyprland 0.56 or newer (on Hyprland so far tested by script, not from a real phone).
- **Several phones at once**, each with its own stream and its own permissions, and picture-in-picture on the phone.

**The KDE Connect set, both ways**
- The clipboard, files and a remote file browser, the phone's notifications, texts and calls on the computer, ringing either one to find it, and waking the computer.
- **The phone on the computer:** its screen or camera in a window you can drive with the mouse, its photos and folders, its music in the computer's media controls.
- Phones running KDE Connect can pair with it for the basics (ping, clipboard, battery, Ring, notifications, files).

**Automation and the rest of your day**
- **`pocketdesk ask`:** a Yes or No question from a script or terminal, answered on the phone (Yes asks for its fingerprint or screen lock); the answer is the exit code.
- **The deck, tasks, home-screen widgets and tiles:** buttons you set up on the computer press keys, run tasks or open links from the phone, with no session open. `pocketdesk done` tells the phone when a command ends.
- **Home Assistant** (over MQTT, no YAML) and a **browser extension** that sends a page, link or image to the phone.
- **Quiet hours, pause, and lock when the phone leaves**, plus a privacy mode that blocks the computer's own keyboard and mouse while you drive it.

**Your computers together**
- **One keyboard and mouse across them** (Windows driving Linux first; Linux drives too from 0.8.2), with the clipboard, files and sound following the pointer, and the phone as another screen.
- **One computer's screen in a window on another**, either way round. The window carries the clipboard and files, keys (including Win and Alt+Tab on Windows), your mic, every screen of the other computer, a pointer lock for games and **Keys by position**.
- **Pair once, for all of them:** pair two of your computers both ways, pair a computer nearby by comparing a six-digit code on both screens, and let your phone pair with your other computers in the same step ("introductions").
- **Remove a lost device from afar:** a phone or computer of yours lists another computer's paired devices and removes one there.
- **Version hints:** each device says which release it runs, and the app tells you which to update.

**From anywhere, and yours alone**
- On your network, through Tailscale, or over the internet with no port forward and no VPN (iroh punches a direct path, or uses a relay).
- No account and no Pocketdesk server. Each end holds the other to its key (TLS 1.3), and each phone has its own permissions.

**Linux hosts**
- **KDE Plasma:** screen sharing through the desktop portal now gets a picture, and the grant can be remembered or allowed ahead (checked headless only; GNOME is supported but not yet checked).
- **Firewall check:** Diagnostics reads ufw, firewalld, nftables and iptables and says which Pocketdesk ports get in, and the Pair page says so when the firewall is why a pairing times out. It only reads the rules.
- **Software encoding on stock Fedora** through Cisco's OpenH264, where `ffmpeg-free` has no libx264 (tested in a container; a phone session on real Fedora is still owed).

## Quickstart

**1. Install it on your computer** from the [latest release](https://github.com/sreevarshan-xenoz/Pocketdesk/releases/latest):

| Computer | Install |
|---|---|
| Windows 10/11 | Run `pocketdesk-…-windows-x86_64-setup.exe`. It starts Pocketdesk at sign-in, opens its firewall ports and offers to install FFmpeg. A portable `…-windows-x86_64.zip` is there too (run `pocketdesk.exe install` once). |
| Windows, winget | The manifests are in [`packaging/winget/`](packaging/winget/), not yet in winget's own catalogue; the [guide](docs/guide.md#the-phone-app-and-windows-through-winget) has the command. |
| Debian, Ubuntu | `sudo apt install ./pocketdesk-…-linux-amd64.deb` |
| Arch | `yay -S pocketdesk` ([AUR](https://aur.archlinux.org/packages/pocketdesk)) |
| Other Linux | `curl -fsSL https://raw.githubusercontent.com/sreevarshan-xenoz/Pocketdesk/main/scripts/install.sh \| bash` installs the portable `linux-x86_64` tarball. It needs glibc 2.39 or newer, PipeWire and FFmpeg; the script offers to install what is missing. |

**2. Install the app on your phone** (Android 8 or newer): `pocketdesk-...-android-arm64.apk` from the same release, or add `https://github.com/sreevarshan-xenoz/Pocketdesk` to [Obtainium](https://github.com/ImranR98/Obtainium) to keep it updated. F-Droid is not available yet: the metadata is ready but has not been submitted.

**3. Pair them.** Open Pocketdesk on the computer and go to its **Pair** page. On the phone, tap **Scan pairing code** and point it at the QR code. Do this once with both on the same network (or tailnet); after that the phone reaches the computer from anywhere. If the app says the server is not running, press **Start server** on its Overview page.

Something not working? The desktop app's **Diagnostics** page checks the server, its ports, the firewall and the internet path, and copies a report you can paste into an issue.

## How it compares

| Feature | Pocketdesk | RustDesk | Sunshine / Moonlight | KDE Connect | Phone Link | LocalSend | Deskflow | scrcpy |
|---------|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| See and drive the computer from a phone | ✅ | ✅ | ✅ | ⚠️ keyboard and touchpad only | ❌ | ❌ | ❌ | ❌ |
| See and drive the phone from the computer | ✅ | ⚠️ Android only, needs permissions | ❌ | ❌ | ✅ supported phones | ❌ | ❌ | ✅ |
| Phone's notifications, texts and calls on the computer | ✅ checked on Windows, not yet on Linux | ❌ | ❌ | ✅ | ✅ | ❌ | ❌ | ❌ |
| Clipboard and files, both ways | ✅ checked on Windows, files not yet on Linux | ✅ | ⚠️ | ✅ | ⚠️ | ⚠️ files and text | ⚠️ clipboard | ⚠️ |
| One keyboard and mouse across computers | ✅ partly device-checked | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ |
| One computer's screen in a window on another | ✅ partly device-checked | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| From outside without a port forward or VPN | ✅ after one session at home | ✅ | ⚠️ UPnP or a forward | ❌ | ⚠️ through a Microsoft account | ❌ | ❌ | ❌ |
| No account needed | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ |
| Wayland-native | ✅ | ⚠️ Partial | ⚠️ Partial | ✅ | n/a | n/a | ⚠️ Partial | n/a |
| Hyprland optimized | ✅ | ❌ | ⚠️ Limited | ⚠️ Limited | n/a | n/a | ❌ | n/a |
| Works with KDE Connect devices | ⚠️ Pairs, ping, clipboard, battery, ring, notifications, files both ways | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |
| Open source | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ |

Close calls were checked against each project's documentation on 2026-10-31 (the rest were not rechecked). The other columns come from those projects' public documentation and may be out of date. Each tool is good at what it was made for, and some do things Pocketdesk does not: Phone Link and scrcpy are mature at showing a phone on a computer, and LocalSend and Deskflow are small and focused. If a cell is wrong, [open an issue](https://github.com/sreevarshan-xenoz/Pocketdesk/issues/new/choose).

**Status: beta.** 0.8.3 is the latest release; main has since fixed the Linux tarball on other distros (it did not start on Arch) and added the Fedora OpenH264 fallback and the firewall check, which ship with the next release. Partly device-checked: Hyprland on a laptop and Sway in a VM (on Linux, not yet files, notifications or sound), and Windows with a Galaxy A71 and an OPPO phone. KDE Plasma 6 has been checked headless only (the screen, input and remembered grants through its portal, with no phone); GNOME is supported but not yet checked. Your computers paired both ways and their window onto each other's screen (new in 0.8.3, with its extras) have run between throwaway servers and from the Linux laptop viewing Windows, view-only; at a desk with two real computers, not yet. The shared keyboard and mouse (new in 0.8, with Linux driving too from 0.8.2) is checked only in parts: from scripts, its receiving end on the Linux laptop and the phone as a screen, and live on Hyprland, Linux driving through its portal; two real computers at a desk, and Linux driving on GNOME and KDE, are not yet. [What works where](docs/guide.md#what-works-where) has the details, and [the checklist](docs/device-verification-checklist.md) every check still open.

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
