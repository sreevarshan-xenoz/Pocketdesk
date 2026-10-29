Pocketdesk is a remote desktop for your own computers: an Android app that streams and controls a Linux or Windows desktop, with files both ways, the clipboard, notifications and sound, and now one keyboard and mouse across your computers.

**This is a beta.** Most features are verified on two Android phones against a Windows host, and many on-device checks are still open. The shared keyboard and mouse, new in this release, has not yet been tried at a desk with a real Windows computer driving a Linux one (see the end of these notes). See [`docs/device-verification-checklist.md`](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/docs/device-verification-checklist.md). Please read [SECURITY.md](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/SECURITY.md) before using it on a network you do not control.

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

Pocketdesk 0.8.1 brings your computers together. One keyboard and mouse moves between them, Windows driving Linux, and the clipboard, images, files and sound follow the pointer; your phone can be another screen beside them; and one computer can show another's screen in a window. It updates 0.7.0 in place (same signing key), keeping your saved computers and pairings.

The shared keyboard and mouse is new and largely unchecked at a desk. It is unit-tested throughout, and its receiving end ran on a real Linux laptop from a script, but nobody has yet pushed a real Windows mouse onto a Linux screen with it. Everything it does is off until you turn it on. Please try it and tell us how it went.

**One keyboard and mouse across your computers (new)**
- **Windows drives Linux:** pair two computers, turn on "Share my keyboard and mouse with my computers" on both (desktop app, Keyboard and mouse), and allow "Keyboard and mouse" for the Windows computer on the Linux computer's Devices page. Then push the pointer off the edge that faces the other computer, or press Ctrl+Alt+Right, and your keys and mouse go there; push back to come home. Linux is driven but does not drive the others yet. It works only while the link between the two is direct (at home), never through a relay.
- **Arrange them:** the Keyboard and mouse page places your computers by dragging, shows each link and both permissions, and Identify flashes each computer's name on its screen. Each switch flashes the name of the computer that has the keyboard and mouse, and the tray says where they are.
- **Nothing left stuck:** if the other computer stops answering for a second (a Wi-Fi drop, a crash), the keyboard and mouse come home by themselves and every key held there is let go. Win+L while away comes home and then locks this computer, Ctrl+Alt+Del stays with Windows, and stopping or uninstalling the server brings them home first.
- **Games:** "Stay on this computer while a game runs" (on) keeps them on Windows while a full-screen game or a program you list is in front, with no hook installed at all. The switch key can still take them off a game, except while a kernel anti-cheat runs (Vanguard, FACEIT, Easy Anti-Cheat, BattlEye) or the program in front runs as administrator while the server does not.
- **The clipboard follows the mouse:** what you copied goes with the pointer to the other computer and comes back as it returns, text or, with no text, an image (a PNG of up to 32 MB; images need "Files over the internet" on on both). Never a password manager's private copy. "Clipboard with my computers" also shares images now.
- **Files dragged across the edge:** drag files or folders in Explorer onto the other computer and let go there; they are copied to it as the Files page sends them (at most 100 at once).
- **Hear the computer you're driving:** with "Hear the computer you're driving here" on (off by default), the other computer's sound plays on Windows while the pointer is there, and stops as it comes home. Update both computers before turning it on.
- **Your phone as another screen:** with the phone's "Let my computer's keyboard and mouse use this phone" on (it uses the "Pocketdesk control" Accessibility service) and "Keyboard and mouse" allowed for the phone on the computer's Devices page, the Windows mouse crosses onto the phone like onto a computer: a click taps, a drag swipes, the wheel scrolls, the right button is Back and the middle Home, and keys type into the field in front (on a US layout; never into a password field or on a locked phone). It needs only the phone's own link, at home or away.
- **How quick it is:** `pocketdesk share-test <computer>` measures how long keys and moves take, and the desktop app's Diagnostics page shows the result.

**Another computer's screen in a window (new)**
- In Settings → Other computers, "Show its screen" beside a paired computer opens a window with that computer's screen, as the phone shows it, on the home network or over the internet. Windows and Linux alike, either way round.
- It is view-only unless the other computer's Devices page turns on "Control the mouse and keyboard" for this one; by default the other computer asks before each session. Its bar has the screen, Quality, View only, Screenshot, Record and Full screen, and its sound plays here when that computer allows "Hear and change this computer's sound" for this one.
- Pairing between computers is one way: show the screen from the computer that pasted the other's pairing link.

**Installing, starting and stopping**
- **Reachable from the internet** (desktop Settings, on by default) can be turned off: the computer then contacts no relay and no DHT and maps no router port, and phones reach it on the same network or through Tailscale only.
- **A privacy policy** ([docs/privacy.md](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/docs/privacy.md)) says what goes where and how to turn each off; the Windows installer shows it before it installs.
- **A phone can pair as soon as the server starts:** on a computer without Tailscale, its probe kept the server's door shut for about 24 s after every start.
- **`pocketdesk stop` returns once the server has gone** (0.4 s, where it could take 27), and the Windows uninstaller and an upgrade stop a server that outlives its stop, so no file is left in use.
- **On Arch Linux** Pocketdesk is in the AUR (`yay -S pocketdesk`).

**Smaller fixes**
- The computer's sound from Linux: Pocketdesk now tells PipeWire the sound format it reads, where it named none, which the phone's sound in a session from a Linux computer needed too. Not yet checked by ear.
- A pointer sent to the screen's last pixel lands on it, not one past it (on Linux, a phone's touches land at most a pixel further up and left).

### Not yet checked on a device

These work in tests, and some ran on real hardware from a script, but not yet:
- **The shared keyboard and mouse at the desk** (checklist §214 and §215): Windows driving Linux with its real mouse and keyboard, at the edge and by the switch key; typing; Win+L; a pulled network and a server stopped while away; mixed display scales; the gaming lock with real games and anti-cheats; the badges, Identify and the tray; the clipboard and images following both ways; Linux kept awake while driven; the arrangement's crossings; and the latency over real Wi-Fi. What did run: a script typing into a headless Sway on the Linux laptop (102 keys in order, none left down; a killed sender's key let go about a second later; a lock screen typed into).
- **Files dragged across the edge** (§216): whether Windows' drag finds Pocketdesk's drop target under the parked cursor at all, and drops of several files, folders, and files from a zip or from a phone shown in Explorer.
- **The phone as a screen with a real mouse** (§217): it was checked on an OPPO phone from a script (taps, swipes, scrolls, Back, Home, typing, a held drag), not yet with a Windows mouse crossing onto it, nor its lag from away.
- **Sound following the mouse** (§218): not yet heard between two real computers; only a test tone crossed a link on one machine.
- **Another computer's screen in a window** (§219): it ran between two throwaway servers on one Windows computer, view-only, by both roads, but not yet between two real computers: control, keys, a second monitor, reconnecting, quality, the session list, sound, Linux showing Windows, screenshots and recordings.
- **Updating:** from 0.7.0 to this release on a phone, and a clean install.

The full list is in [the checklist](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/docs/device-verification-checklist.md) (§214 on are this release's).
