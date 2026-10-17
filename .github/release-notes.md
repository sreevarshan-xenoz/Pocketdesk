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

Pocketdesk 0.4.0 opens Pocketdesk to the rest of your setup. Automations, Home Assistant and your browser can now use it, and so can a second computer and phones running KDE Connect. It also sees the phone's screen on the computer, and it holds up better on slow and changing networks. It updates 0.3.0 in place (same signing key), keeping your saved computers and pairings.

**The phone's screen on the computer**
- "Show screen" on a saved computer casts the phone's screen into a window in the desktop app, with no adb. Viewing only, under its own per-phone permission.

**Automations and other apps**
- **Events and hooks:** the server logs what happens (sessions, phones linking, batteries, notifications, files and links) to an event log. `pocketdesk events` follows it, and a command can run on each kind of event.
- **Activity:** the desktop app's new Activity tab shows the latest events in words, with filters and Show in folder.
- **Tasks:** name commands in Settings. A phone you allow can list the names (never the commands) from Quick Settings and start one. The computer can ask you first, and it reports the result.
- **Send from the browser:** a browser extension (Chromium browsers and Firefox) sends the page, a link or an image to the phone from the right-click menu.
- **Home Assistant:** over MQTT, this computer appears with its sessions, an activity event, each paired phone's battery and connection, and a button per named task.
- **What the phone tells other apps:** with a switch on (off by default), the phone app tells other apps, such as Tasker, when a session starts or ends and when files or links come and go. It never sends their content.
- **The protocol, written down:** `docs/protocol-spec.md`, `docs/packets.md` and conformance vectors for a second implementation.

**Two computers**
- Pair your other computer from its pairing link. The two then keep a link over the internet, so the clipboard is shared and files can be sent between them from the desktop app or `pocketdesk send`.

**KDE Connect devices**
- A phone running KDE Connect can find this computer, pair with it (the code shows on both screens), and ping it. It can also share the clipboard both ways, show its battery here and be rung from here. It can send files here too, but only once you turn that on for it. On by default; Settings "KDE Connect devices" turns it off.

**Away from home and on bad networks**
- Files and clipboard images now reach a phone on the internet road. Before, a file offered to a phone outside the home network never arrived.
- The phone uses one internet endpoint per computer (iroh 1.3), and its DNS lookups no longer fall back to public resolvers without saying so.
- A broken transfer waits for the network to come back and then continues, instead of giving up after a fixed time.
- On a slow link, the session lowers its bitrate to what the link actually carries and steps the frame rate down, instead of showing a frozen picture.

**Reliability**
- Fixed a crash when the server stopped during a session: code was running inside an unloaded Windows capture library. The server now also holds COM's apartment for its life, which closes that kind of crash for the other Windows libraries it uses. A stress test of 400 overlapping sessions runs clean.
- When Windows Graphics Capture cannot start, the server now falls back to DXGI capture. Before, the fallback always failed.
- The phone's control link comes up with the stream after a reconnect, so sends no longer fail with "phone not connected".
- A Wi-Fi return no longer opens two streams.
- The idle server uses much less CPU: one folder watcher replaces eight pollers, and on Linux the clipboard is watched by a single `wl-paste --watch`.
- A crash leaves its backtrace in `crash.log` in the state folder.

### Not yet checked on a device

These work in tests, and most were checked on one phone (Galaxy A71) against Windows, but not yet:
- two real computers paired over the internet (checked with two servers on one box);
- KDE Connect on Linux beside `kdeconnectd`, the desktop app's KDE section on screen, GSConnect and Valent;
- Home Assistant itself (checked against a local MQTT broker), and Tasker with the phone's broadcasts;
- the browser extension loaded in a browser (checked through its native host);
- a slow link's behaviour on a real 1 Mbit/s network (checked in a throttled test);
- files on the internet road over a relay;
- updating from 0.3.0 to this release on a phone.

The full list is in [the checklist](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/docs/device-verification-checklist.md) (§77–§87 are this release's).
