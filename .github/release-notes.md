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

Pocketdesk 0.5.0 is about the phone and the computer working as one:
- the phone's notifications, calls, battery and music on the computer;
- the computer's music, state and buttons on the phone, its home screen and Quick Settings;
- the phone's screen, camera and sound in a window you can control;
- files and the clipboard you can watch, stop and send again.

Both apps also had a design pass, and the desktop has a tray of its own. It updates 0.4.0 in place (same signing key), keeping your saved computers and pairings.

**The phone's notifications and calls on the computer**
- **Answering:** answer a notification in its Windows toast or Linux notification. Each answer says whether it reached the app.
- **Waiting:** notifications that could not be sent wait up to an hour and arrive marked late.
- **What a notification shows:** chats show their recent messages, and notifications carry their app's icon (and, if you turn it on, their picture).
- **Quiet:**
  - Do Not Disturb on the phone keeps its notifications off the computer.
  - The computer can mute the phone for a while.
  - "Not from this app" mutes one app here.
  - Pause quiets every phone for an hour, 8 hours or until resumed.
- **Clearing:** a notification closed on the computer is cleared on the phone too.
- **Calls:** a ringing call shows on the computer, where you can silence it or decline it.
- **Other computers:** other paired computers you pick can get a copy of each notification, to see but not answer.

**The phone from the computer**
- **Ring it:** ring the phone from the Devices page or the tray.
- **See it:** show its screen or its camera in a window. With "Let the computer control this phone" on, the mouse and keyboard drive it there. The window takes dropped files and pasted text, takes screenshots and records video, and can play the phone's sound.
- **Ask it for things:**
  - ask it to take a photo, and see its recent photos;
  - open its shared folders as a drive;
  - send it a number to call, which you then press Call for on the phone;
  - hand a command to Tasker, Automate or MacroDroid.
- **How it is doing:**
  - its battery, free storage, network and ringer;
  - an alert when it has charged to 80, 90 or 100 %;
  - on the Devices page, which of its features are on, on each side, and which side holds one back.
- **Its music:** shown in the computer's media controls (Windows).

**The computer from the phone**
- **Remote** (the phone as a remote control) reconnects by itself and notices a computer that went quiet. It now has a talk timer, an air mouse and a laser pointer.
- **More ways to use it:** the phone works as a pen and as a gamepad, a connected game controller passes through, and the volume keys set the computer's volume.
- **The deck:** buttons you set up on the computer show on the phone, each running keys, a task, media, a link or text.
- **"How it is doing":** the computer's CPU, memory, disks, battery and uptime.
- **Home-screen widgets:** for the computer's state, the deck and its music.
- **Quick Settings tiles:** the computer's music, and waking the computer.
- **Sound session:** the phone as the computer's speaker and microphone, with no picture.

**Files and the clipboard**
- **Transfers:** each transfer shows its progress, with Cancel on both sides.
- **Where files land:** choose where received files go on each side, and files keep their dates.
- **Clipboard:**
  - its history, with Resend;
  - send the clipboard with a key;
  - send selected text from any app with "To computer".
- **Reading:** continue reading a web page on the phone, at the paragraph you were on.
- **Dropping files** on the desktop app asks which phone they go to when there is more than one.

**The desktop app**
- **The tray** is a small program of its own, with each phone's battery and actions. Linux has a tray too.
- **Pages:** new Activity and Logs pages, search on Settings, Activity and Clipboard, and a Sessions page with recent sessions.
- **Design:** a design pass over every page, with the light and dark themes and a screen reader in mind.
- **Lock when the phone leaves:** the computer can lock once your phone is gone (Windows).

**The phone app**
- Its own design pass: names on both sides, Spanish and Tamil, large fonts, TalkBack and landscape.
- A health card that says when an update stopped something from working.
- An Android 17 pass, and a lower background cost.

**Reliability**
- **The battery in the background:** the phone now reads its battery from Android's battery service, since some phones (ColorOS) left the background reading stale.
- **GNOME and KDE:** they can keep the screen-sharing grant and take input through the portal.

### Not yet checked on a device

These work in tests, and many were checked on two phones against Windows, but not yet:
- **Calls, a number to call and texts** on a phone with a SIM.
- **Linux:** most of this release's features, including the tray, pen and gamepad input, phone sound and media controls.
- **By hand at the computer:** the desktop app's new pages at their smallest size, the tray's menus, drops and paste on the phone's window, and screenshots and recordings.
- **Other apps and systems:**
  - Tasker with the automation commands;
  - Home Assistant itself;
  - Android 17;
  - a real stylus and game controller.
- **Updating:** from 0.4.0 to this release on a phone.

The full list is in [the checklist](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/docs/device-verification-checklist.md) (§88–§164 are this release's).
