Pocketdesk is a remote desktop for your own computers: an Android app that streams and controls a Linux or Windows desktop, with files both ways, the clipboard, notifications and sound.

**This is a beta.** Most features are verified on two Android phones against a Windows host, and many on-device checks are still open. See [`docs/device-verification-checklist.md`](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/docs/device-verification-checklist.md). Please read [SECURITY.md](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/SECURITY.md) before using it on a network you do not control.

## Install

| Platform | File | How |
|---|---|---|
| Windows | `pocketdesk-…-windows-x86_64.zip` | Unzip, then run `pocketdesk.exe install` to start the server at logon and open its firewall ports. `pocketdesk-app.exe` is the desktop app. FFmpeg must be on `PATH` for the encoder. |
| Linux | `pocketdesk-…-linux-x86_64.tar.gz` | `curl -fsSL https://raw.githubusercontent.com/sreevarshan-xenoz/Pocketdesk/main/scripts/install.sh \| bash` installs the server, the desktop app and the systemd user service. It needs PipeWire and FFmpeg (libav*). |
| Android | `pocketdesk-…-android-arm64*.apk` | Install it on the phone, then pair with the QR code from the desktop app (Pair tab). |

`SHA256SUMS` lists every file's checksum (`sha256sum -c SHA256SUMS`).

An APK named `…-debug-signed.apk` is the debug build. It works, but Android will not update it in place from a differently signed build, so uninstall it first when switching to a store or release-signed build.

## What's changed

Pocketdesk 0.7.0 brings Linux up to Windows, works beside KDE Connect, and moves more of your phone's photos and files to the computer by themselves. It also carries fixes from a long run of checks on a real phone, with sessions, transfers and the clipboard driven by script. It updates 0.6.0 in place (same signing key), keeping your saved computers and pairings.

**Beside KDE Connect**
- **Starts next to it:** with KDE Connect or GSConnect holding port 1716, the server takes the next free port and says so, instead of stopping.
- **Files to KDE Connect devices:** a paired KDE Connect device is a target on the Files page and for `pocketdesk send`.

**Photos and folders**
- **New camera photos** go to the computer by themselves, when you turn on "Send new camera photos" on the phone.
- **The whole gallery:** the Phone page's "More photos" goes back through every photo, not only the newest 24.
- **Folders between computers** go both ways.

**Linux**
- **The clipboard on X11** (Mint and other X11 sessions), both ways, with images, without polling.
- **The phone as a game controller:** its gamepad drives a real controller on Linux, so games see one.
- **Lock when the phone leaves** on Sway and Hyprland, from the compositor's own idle time; its minutes are now in Settings on Linux too.

**Sessions and the link**
- **Sessions from away start sooner:** a phone linked by the internet road no longer waits out a home dial first (about 3 s from the tap, down from 11).
- **A phone that vanishes** with data waiting counts as gone within a minute on a Linux computer, not after a quarter of an hour.
- **The away lock** counts only a phone on this computer's network: one that walked off on mobile data, still linked over the internet, now counts as gone.
- **The session's numbers** start fresh: a new session no longer shows the last one's round-trip figures while it connects.
- **A crop to one window** no longer ends the session when the window is still (its first frame was lost), and costs one encoder rebuild, not two.
- **A window you crop to and then resize** reaches the phone at its new size even if it then stays still; it used to keep its old size until something on it moved.
- **A file sent while the phone shares a big one** now waits for it: the phone moves one file at a time, and the computer's offer no longer runs out behind a share longer than 10 minutes.
- **A ringer turned during a session** shows on the computer within seconds; it could take up to 17.
- **A computer added on the phone** keeps to its own address: with the phone linked to another computer, it could take that one's name and reach it over the internet instead.
- **A phone that moves between computers** no longer carries the first one's links, file offers or pairing answer to the second.

**Smaller fixes**
- Home shows one card when Android pauses Pocketdesk, with its Allow button.
- The Overview names which Android setting stops a screenshot or a lock.
- A send to another computer says what became of it in a computer's words; a video without a date has no stray dot.
- The desktop app's hints and section names are larger (11 points, from 9); the Messages page's text no longer spreads its words, and Logs and Diagnostics show commands without Markdown backticks.
- The phone's Wake settings say what a blank field means under it, not inside its label.
- The desktop app's Files page says what can take a file in a whole sentence, and the phone's Tamil uses one word for the phone.

### Not yet checked on a device

These work in tests, and many were checked on an OPPO phone against Windows and a Linux VM, but not yet:
- **Linux desktops:** the game controller in real games, the away lock on a real Sway or Hyprland, and the clipboard on a real X11 desktop (they were seen on a VM).
- **KDE Connect:** files to a real KDE Connect device.
- **By hand:** the camera photos and gallery pages, and the Settings rows that are new.
- **Updating:** from 0.6.0 to this release on a phone, and a clean Linux install.

The full list is in [the checklist](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/docs/device-verification-checklist.md) (§207 on are this release's).
