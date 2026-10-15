Pocketdesk is a remote desktop for your own computers: an Android app that streams and controls a Linux or Windows desktop, with files both ways, the clipboard, notifications and sound.

**This is a beta.** Most features are verified on one phone (Galaxy A71) against a Windows host, and many on-device checks are still open. See [`docs/device-verification-checklist.md`](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/docs/device-verification-checklist.md). Please read [SECURITY.md](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/SECURITY.md) before using it on a network you do not control.

## Install

| Platform | File | How |
|---|---|---|
| Windows | `pocketdesk-…-windows-x86_64.zip` | Unzip, then run `pocketdesk.exe install` to start the server at logon and open its firewall ports. `pocketdesk-app.exe` is the desktop app. FFmpeg must be on `PATH` for the encoder. |
| Linux | `pocketdesk-…-linux-x86_64.tar.gz` | `curl -fsSL https://raw.githubusercontent.com/sreevarshan-xenoz/Pocketdesk/main/scripts/install.sh \| bash` installs the server, the desktop app and the systemd user service. It needs PipeWire and FFmpeg (libav*). |
| Android | `pocketdesk-…-android-arm64*.apk` | Install it on the phone, then pair with the QR code from the desktop app (Pair tab). |

`SHA256SUMS` lists every file's checksum (`sha256sum -c SHA256SUMS`).

An APK named `…-debug-signed.apk` is the debug build. It works, but Android will not update it in place from a differently signed build, so uninstall it first when switching to a store or release-signed build.

## What's changed

This is the first release with files. Pocketdesk 0.2.0 has:

**Seeing and driving the computer**
- The desktop on the phone with hardware encoding: VAAPI and NVENC on Linux; NVENC, AMF, Quick Sync and Media Foundation on Windows. Each session shows its frame rate and latency.
- Direct touch, a laptop-style trackpad with gestures and pinch-to-zoom, a full keyboard with a modifier and function-key bar, and a USB or Bluetooth mouse plugged into the phone.
- Held upright, a session stacks the desktop on top and the trackpad below. Foldables and tablets split side by side.
- Several phones at once (one drives), view-only, privacy mode, a screen picker and single-window streaming.

**Between phone and computer**
- Clipboard both ways, text and images.
- Files and whole folders both ways, resumable after a dropped link. The remote file browser can download a folder.
- Links both ways: "Open on computer" in the share sheet, and "Send link" or `pocketdesk link` to the phone.
- The phone's notifications on the computer. Texts (SMS) with replies from the computer and, optionally, contact names.
- The phone's battery on the computer, with a warning when it runs low.
- Media both ways: the computer's music and video from the phone's Sound sheet, and the phone's from the desktop app's Sessions tab.
- The desktop's sound on the phone, and the phone's microphone on the desktop.

**Reaching the computer**
- Same network: computers are found on the network, and pairing is by QR code or PIN, with the certificate pinned.
- From anywhere: direct over the internet with hole punching, n0's relays or your own (`pocketdesk relay`), and discovery over the Mainline DHT.
- Wake from the phone: at home, from outside through a router forward, or through another computer left on.
- "Check connection" on the phone names what blocks a connection.

**On the computer**
- The desktop app: status, pairing, devices and their permissions (one switch per capability), sessions, files, messages, settings and diagnostics.
- A local JSON API for scripts (`pocketdesk … --json`), and Android intents for automation apps.
- Starts at login: a systemd user unit on Linux, `pocketdesk install` on Windows.

**Security**
- The control channel, the stream and file transfers run over TLS 1.3 / QUIC, with mutual device keys and per-phone permissions.
- `docs/protocol.md` maps everything Pocketdesk sends, and to whom; SECURITY.md lists the known limits.

### Not yet checked on a device

These work in tests, but no one has run them on real hardware yet:
- texts on a phone with a SIM;
- a mouse connected to the phone;
- the phone's media with a real music app;
- pinch-zoom in trackpad mode;
- GNOME and KDE hosts;
- a Linux host's sound and files;
- a Windows host started at logon by `install`;
- updating from this release to the next one.

The full list is in [the checklist](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/docs/device-verification-checklist.md).
