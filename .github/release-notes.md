Pocketdesk is a remote desktop for your own computers: an Android app that streams and controls a Linux or Windows desktop, with files both ways, the clipboard, notifications and sound, one keyboard and mouse across your computers, each of your computers showing and controlling the others in a window, and now your phone's apps, camera and place available on your computer, and one computer's window lent to another.

**This is a beta.** Most features are verified on two Android phones against a Windows host, and many on-device checks are still open. Much of what is new in this release has run in tests and between throwaway servers, some of it on an OPPO phone, and the features that use Shizuku (apps in windows of their own, keys to taps, the phone's hotspot), the wake helper and the watch tile have not yet run on a real phone or watch at all (see the end of these notes). See [`docs/device-verification-checklist.md`](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/docs/device-verification-checklist.md). Please read [SECURITY.md](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/SECURITY.md) before using it on a network you do not control.

## Install

| Platform | File | How |
|---|---|---|
| Windows | `pocketdesk-…-windows-x86_64-setup.exe` | Run it: it installs Pocketdesk, starts it at sign-in with its firewall ports open, puts `pocketdesk` on `PATH` and offers FFmpeg through winget. Uninstall it from Settings → Apps. |
| Windows, portable | `pocketdesk-…-windows-x86_64.zip` | Unzip, then run `pocketdesk.exe install` to start the server at logon and open its firewall ports. `pocketdesk-app.exe` is the desktop app. FFmpeg must be on `PATH` for the encoder. |
| Debian, Ubuntu | `pocketdesk-…-linux-amd64.deb` | `sudo apt install ./pocketdesk-…-linux-amd64.deb` installs the server, the desktop app and the tray, with the libraries they need, and starts the server at every sign-in. `sudo apt remove pocketdesk` takes it out. |
| Linux | `pocketdesk-…-linux-x86_64.tar.gz` | `curl -fsSL https://raw.githubusercontent.com/sreevarshan-xenoz/Pocketdesk/main/scripts/install.sh \| bash` installs the server, the desktop app, the tray and the systemd user service. It needs PipeWire and FFmpeg (libav*). On Arch, `yay -S pocketdesk` installs it from the AUR. |
| Android | `pocketdesk-…-android-arm64*.apk` | Install it on the phone, then pair with the QR code from the desktop app's Pair page. |

`SHA256SUMS` lists every file's checksum (`sha256sum -c SHA256SUMS`).

An APK named `…-debug-signed.apk` is the debug build. It works, but Android will not update it in place from a differently signed build, so uninstall it first when switching to a store or release-signed build.

## What's changed

Pocketdesk 0.9.1 fixes one crash in 0.9.0's Android app. It changes nothing else.

- **The Android app closed as it opened on some phones.** An F-Droid tester's Moto g37 (Android 16) showed it on every launch of 0.9.0's release APK (a fresh install and four retries): `NoSuchMethodException: androidx.work.impl.WorkDatabase_Impl.<init>`, thrown while WorkManager starts. The release build's code shrinker removed the constructor of the database WorkManager builds by reflection. It is now kept. 0.8.3 and earlier did not have WorkManager, so they were not affected, and builds that skip shrinking (debug builds) never were.
- **Two dependencies were updated for security advisories** that the release's audit step stops on: `url` to 2.5.9 and `quinn-udp` to 0.5.16 (a receive loop that could spin after a truncated datagram).

The Linux and Windows programs are built from the same code as 0.9.0 plus those two updates.

## Upgrade notes

- Updates 0.9.0 in place, on the phone and on the computer. Nothing in your settings, pairings or saved computers changes.
- Everything in [0.9.0's notes](https://github.com/sreevarshan-xenoz/Pocketdesk/releases/tag/v0.9.0) still applies, including its known issues.

## Not yet checked on a device

- **The fix on a phone:** this release's signed APK has not been started on a phone yet. What was checked is the built app itself: the release build's `WorkDatabase_Impl` has its constructor (0.9.0's did not), the APK is signed with the release key, and its checksums match. If 0.9.1 still closes as it opens on your phone, please open an issue with the phone model and Android version.
- Everything 0.9.0's notes list as not yet checked on a device is still open.
