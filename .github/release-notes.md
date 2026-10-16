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

Pocketdesk 0.3.0 makes the phone and the computer useful to each other outside a session, and adds a few new ways to use the phone. It updates 0.2.0 in place (same signing key), keeping your saved computers and pairings.

**Without opening a session**
- The control channel reaches the computer over the internet too, so the clipboard, notifications, texts, links and the battery keep working away from home without Tailscale.
- The clipboard both ways in the background, and a "Send clipboard" Quick Settings tile.
- Files from the computer arrive in the background (`pocketdesk send`, the desktop app's Files tab, and a new "Send to phone" entry in Explorer, Nautilus, Nemo and Dolphin).
- Answer the phone's notifications from the desktop app: reply, or press one of their buttons.
- Browse folders you share from the phone in the desktop app's new Phone tab, and copy files from them.
- New screenshots taken on the phone land on the computer's clipboard and in its Downloads (off by default).
- The phone's battery reaches the computer in the background, over a link another background feature keeps.

**New ways to use the phone**
- **Remote:** a trackpad, keyboard, media keys and presenter keys with no picture sent, so the computer captures and encodes nothing. The computer shows it as "control only".
- **A second screen** at the phone's shape, on Sway: the session's screen picker adds a screen beside your others, and windows dragged onto it show on the phone.

**Linux**
- Any character types correctly on any keyboard layout (accents, other scripts, emoji) on wlroots compositors, and capitals arrive as capitals.
- Input works where `/dev/uinput` is root-only (most distributions), through the compositor's own virtual pointer and keyboard on Sway and Hyprland, including shortcuts with their modifiers.
- A computer without audio capture (no PipeWire) no longer ends every session as it starts.

**Also fixed**
- A session's connection is closed when the session ends, so the phone learns at once.
- "Check connection" names the phone's own hotspot and USB tethering.
- Every Linux desktop notification had been failing (`notify-send`); fixed.

### Not yet checked on a device

These work in tests, and most were checked on one phone against Windows or a headless Sway VM, but not yet:
- the internet control path from a real outside network;
- answering a real chat app's notifications with buttons;
- the second screen on Hyprland (blocked upstream), GNOME, KDE and Windows (not supported yet);
- a computer on the phone's hotspot or USB tethering;
- the remote's keys into a Windows window (checked on Linux);
- texts on a phone with a SIM, and GNOME and KDE hosts;
- updating from 0.2.0 to this release on a phone.

The full list is in [the checklist](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/docs/device-verification-checklist.md) (§64–§76 are this release's).
