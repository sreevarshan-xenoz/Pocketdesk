Pocketdesk is a remote desktop for your own computers: an Android app that streams and controls a Linux or Windows desktop, with files both ways, the clipboard, notifications and sound, one keyboard and mouse across your computers, and now each of your computers showing and controlling the others in a window.

**This is a beta.** Most features are verified on two Android phones against a Windows host, and many on-device checks are still open. Much of what is new in this release (the window onto another computer's screen and its extras, pairing two computers nearby, introductions) has run in tests and between throwaway servers, not yet at a real desk with two real computers (see the end of these notes). See [`docs/device-verification-checklist.md`](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/docs/device-verification-checklist.md). Please read [SECURITY.md](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/SECURITY.md) before using it on a network you do not control.

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

Pocketdesk 0.8.3 makes your computers one set: pair two of them once, both ways, and each can show and control the other in a window, with the clipboard, files, keys, the mic and every screen following. Your phone pairs once for all of them and can remove a lost device from any. KDE Plasma's screen sharing gets a picture at last. It updates 0.8.2 in place (same signing key), keeping your saved computers and pairings. Most new features need 0.8.3 at both ends, so update every device.

**Your computers, both ways**
- **Both are mine:** pair two of your computers once, both ways, with consent given at pairing ("This is another of my computers" on the Pair page, off by default), so you never walk to the far computer to click Allow or turn Control on.
- **Pair a computer nearby:** Pair → Computer lists the Pocketdesk computers on your network; Pair shows the same six-digit code on both screens, and the other computer accepts or declines. No link to carry across. "Let computers nearby ask to pair" turns it off. If a pairing times out on Windows, the Pair page now says when the network is marked Public (where the firewall lets nothing in) and how to make it Private.
- **Another computer's screen in a window** gained:
  - **Stats** (the road, size, codec, frame rate, bitrate, round trip, decode time, frames lost) and **reconnecting by itself** after a drop, for about two minutes; a computer whose server stops or restarts now says so at once, and the window has its picture back within seconds;
  - **the clipboard and files follow the window** while it controls that computer;
  - **Send keys** (the Windows key, Alt+Tab, Alt+F4, Print Screen, and Ctrl+Shift+Esc and Lock for a Windows computer), and on Windows the window can take Win and Alt+Tab while it has the focus;
  - **a faster picture:** hardware decoding (D3D11 on Windows, VA-API on Linux), a lighter pipe and HEVC;
  - **Lock the pointer** (Ctrl+Alt+Shift+M) with relative mouse moves, for games and 3D;
  - **Keys by position**, for games and other keyboard layouts: every key goes by where it sits, not what it types;
  - **Hide what I do there:** the other computer's privacy mode, from the window, if that computer allows it;
  - **Send my mic** to the other computer (off by default);
  - **every screen** of the other computer as a row of buttons laid out as they sit (Ctrl+Alt+Shift+1 to 9).
- **Your computers on the Overview and in the tray**, and **waking one that sleeps** (a computer paired both ways tells the other how to wake it).
- **Both computers driving at once:** with the shared keyboard and mouse driving from both sides, the pointer no longer bounces straight back home. A pasted pairing link that began with an invisible character no longer leaves Pair greyed out.
- **No pacing stall on the link between computers:** a key after a pause no longer waits about 25 ms for the link to wake (now about 3 ms over Wi-Fi). An older computer at the other end still connects.

**Your phone**
- **Remove a lost device from another of yours:** a phone paired with a computer's own code, or another of your computers paired both ways, lists a computer's paired devices and removes one there (the phone's computer card → Devices on this computer; the desktop's Settings → Other computers → Devices there…). The computer says who removed what. "Let my devices remove others from afar" turns it off on each computer.
- **Pair once for all your computers:** after you pair the phone with one computer, it offers your other computers paired both ways with that one ("Also pair with …?"). You tap Pair on the phone, and the other computer can refuse ("Let my computers add my phones here").
- **One version across your devices:** the phone's computer cards and the desktop's Devices page and Other computers say which release each device runs, with a hint to update the older one ("Tell me when my devices run different versions" turns the hint off). Nothing is checked online.
- **The phone as a second screen on Hyprland** (0.56 on), as on Sway.
- **Home and Settings:** one system card at a time, card buttons on one row, jumps to each Settings group, "Used 2 min ago" on each card, the card menu in two headed sections with its upkeep under Manage, long window titles cut in the middle (a long-press shows them whole), and Pair again on a card that is no longer paired. Spanish says "emparejar" and "equipo" throughout.

**Linux desktops**
- **KDE Plasma:** screen sharing through the desktop portal now gets a picture. Its capture never offered a picture format, so the stream stayed empty (the same capture serves GNOME, not yet checked there). An unanswered permission dialog now ends after one minute instead of two, and the server names itself to the portal as `dev.pocketdesk.Pocketdesk`, so the dialog no longer calls it "an unidentifiable application".
- **KDE Plasma with no one at the desk:** "Remember the screen-sharing grant" restores the next session with no dialog, and the guide's "KDE Plasma: no dialog at all" shows how to allow Pocketdesk ahead, with what that costs.
- **The wheel on KDE** without uinput scrolls a notch per notch, not a pixel.
- **The Linux tray** no longer crashed at start in a build that compiled the server and the app together (the release packages were not affected).

**Desktop app**
- Four rounds of polish: greyed controls say why and offer the fix, pairing shows its progress with Cancel, one Start server bar on every page, Pair has a Phone | Computer switch, the Keyboard and mouse page starts with a setup checklist and holds its options, a new look for both themes (spacing, focus ring, contrast), and every page reworked with clearer empty states.

**Security and fixes**
- A computer this one paired into only one way can no longer ask for this computer's privacy mode or lock it, and the always-on clipboard between computers now respects the Clipboard switch on the Devices page.
- A review of everything since 0.8.2 found and fixed more: the window's mic on by default only for a computer paired both ways and never played while the session is view-only; privacy refused for the computer the shared keyboard is on; the clipboard handed between windows without echo.
- SECURITY.md has two new known limits: an owner device can remove your other devices (71), and a computer paired both ways can add an owner phone (72). Read them before turning these on for a computer someone else can reach.
- On Windows, the server's sockets are no longer inherited by the programs it starts (ffmpeg), which could hold a port after a restart.

## Upgrade notes

- **Update every device.** Introductions, removing from afar, the version line and the window's new extras need 0.8.3 at both ends; an older end refuses or ignores them, and nothing breaks.
- **Phones paired before 0.8.3 pair again with the computer's own code** (its Pair page, not "Someone else's") to remove devices from afar or to be introduced to your other computers. Until then they are not owner phones.
- **Computers paired both ways before 0.8.3 keep "Use the phone as a microphone" off** for each other, so Send my mic plays nothing there until you turn it on in that computer's Devices page.
- **GNOME and KDE may ask once more** for screen sharing after the update: the server now names itself `dev.pocketdesk.Pocketdesk` (where the .deb, the AUR package or the tray installed its desktop file), and a grant kept for the unnamed program is not this name's. If you allowed Pocketdesk ahead on KDE, allow `dev.pocketdesk.Pocketdesk` again. That lets any program running as you that claims this name share your screen and drive it without asking; the guide shows how to take it back.
- **An older desktop app beside a newer server** (or going back to 0.8.2) drops the owner mark from `trusted_devices.json`, so owner phones must pair again.

## Known issues

- On Windows, saving the trusted devices list (`trusted_devices.json`) can fail with "Access is denied" while another program has the file open. Not fixed yet.
- The wheel through a desktop portal carries whole notches only: a touchpad's fine scrolling arrives as notches, and an app on KDE that scrolls only by high-resolution wheel steps still won't.

## Not yet checked on a device

These work in tests, and some ran on real hardware from a script or between throwaway servers, but not yet:
- **Another computer's screen in a window between two real computers** (checklist §224, §227, §231, §233 to §235, §237 to §240): stats and reconnecting, the clipboard and files following, Send keys, hardware decode and HEVC, the pointer lock, privacy, the mic, every screen, and Keys by position with real layouts and games. What did run: the Linux laptop viewing Windows view-only by both roads, with a server restart coming back in about 3 to 4 s (§219).
- **Your computers paired both ways and nearby** (§223, §228, §229, §232): pairing both ways and nearby with the code, the Public-network hint, the always-on clipboard held to its switch, the Overview and tray entries, and waking a computer.
- **Both computers driving at once** (§225) and **the link without the pacing stall** at a desk (§222).
- **Removing from afar and introductions between two real computers** (§241, §244): removing a guest, a computer paired one way refused, and the introduction from Windows to the Linux laptop end to end. The phone side ran on an OPPO against two throwaway servers.
- **The version line between computers a release apart** (§242), and the phone against a 0.8.2 computer.
- **The phone's second screen on Hyprland from a real phone** (§245); the screen itself was added, captured and removed on the laptop by a test.
- **GNOME** (§100): nothing yet. **KDE Plasma at a real desk** with a phone, two phones, and the systemd unit (§100); KDE ran headless in a container, driven by a script.
- **The desktop app's new pages** (§226, §230, §236) by eye at a desk, and the Android UX items with TalkBack and the phone's own sound (§243).
- **Updating** from 0.8.2 to this release on a phone, and a clean install.

The full list is in [the checklist](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/docs/device-verification-checklist.md) (§222 on are 0.8.3's) and, by where each can run, in [docs/owed-checks.md](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/docs/owed-checks.md).
