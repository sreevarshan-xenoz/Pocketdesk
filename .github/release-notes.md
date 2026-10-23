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

Pocketdesk 0.6.0 is about the phone and the computer staying linked and acting on each other:
- linked with the screen off;
- typing, opening apps, screenshots and locks on the phone from the computer;
- the phone's music, alarms and ongoing updates on the computer, and the computer's music in the phone's media controls;
- holding notifications by schedule, by words, or while you present.

Both apps had a design pass over what is new. It updates 0.5.0 in place (same signing key), keeping your saved computers and pairings.

**Staying linked**
- **With the screen off:** a quiet service keeps the phone linked on phones whose system freezes background apps (measured on ColorOS), with "Stay linked with the screen off" in Settings.
- **Found again:** a paired computer that moved to a new address is found by its key.
- **On the phone:** "What this computer allows" lines up its own switches with the computer's permissions, and pairing asks "Whose phone is it?" (Mine, someone else's, a guest's, or a saved profile).

**The phone from the computer**
- **Type on the phone** from the computer's keyboard, from the Overview, the tray or a key.
- **Open an app on the phone**, take a screenshot of it or lock it, from the Overview or the tray.
- **Its contacts** on the Messages page, to text or call (names and numbers only, when you turn it on).
- **Ring it:** it now vibrates and flashes its torch as it rings.
- **Its sound** on the computer without its screen.
- **Files into its folders** from the drive in File Explorer, into folders you mark.

**The phone's notifications on the computer**
- **Alarms:** a ringing alarm or finished timer rings on the computer too.
- **Ongoing updates:** a download's bar or a running timer shows as a live card that updates in place.
- **Open on the phone:** open a notification's app on the phone from the computer.
- **Where they show:** only on the computer you are using; not while the phone is in your hand; kinds you keep on the phone stay there.
- **Holding them:** quiet hours on a schedule, while presenting or in full screen (Windows; GNOME's or KDE's Do Not Disturb on Linux), and by words you choose.

**Music**
- **The phone's music** shows on the computer without a session, with its cover, in Windows' media controls and as an MPRIS player on Linux.
- **The computer's music** shows in the phone's media controls, lock screen and watch; choose which player when several play.

**Files and the clipboard**
- **Image copies** reach the phone's clipboard without a session, and received images can go onto its clipboard.
- **Copies on the phone** go to the computer by themselves (on phones that show Android's clipboard preview).
- **Copies kept here:** never send copies from programs or with looks you list (a password manager's, a key's).
- **Copied files:** files copied in Explorer or a file manager go with Send the clipboard and "Paste copied files" (`xclip` on X11).
- **Several at once:** send files to several phones or computers.
- **Photos keep where they were taken.**

**Automation**
- **Automation apps** on the phone (Tasker and others) can start the computer's tasks and press its deck buttons.
- **Home Assistant** gets buttons for commands to the phone.
- **`tel:` links** on the computer open the call on the phone, and a call's notification has Call back.
- **Links by site:** a site's links open in the browser, are only shown, or go to a program you name.

**The desktop app**
- **Permission profiles:** save a phone's permissions under a name, apply one to a phone (it asks first), or pair a phone with one.
- **A design pass** over the O and P pages: words, questions before what cannot be undone, empty and waiting states, screen-reader names.

**The phone app**
- **Find a setting** with search.
- **A design pass** over the new rows, with their permissions asked for and refusals said.
- **Its switches** for alarms, contacts, apps, tasks and images are told to the computer, so the Devices page can say why an ask got no answer.

### Not yet checked on a device

These work in tests, and many were checked on an OPPO phone against Windows, but not yet:
- **Linux:** GNOME's and KDE's Do Not Disturb, an alarm's sound, files copied on X11, and most of this release's features by hand.
- **By hand at the computer:** the new desktop pages and questions, the typing window, the Overview's buttons.
- **A phone with a SIM:** calls, Call back and `tel:` links reaching a call.
- **Other apps and systems:** Tasker itself, Home Assistant, a password manager's copies, Android 17.
- **Updating:** from 0.5.0 to this release on a phone, and a clean Linux install.

The full list is in [the checklist](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/docs/device-verification-checklist.md) (§166–§206 are this release's).
