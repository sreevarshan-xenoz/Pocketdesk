Pocketdesk is a remote desktop for your own computers: an Android app that streams and controls a Linux or Windows desktop, with files both ways, the clipboard, notifications and sound, and now one keyboard and mouse across your computers.

**This is a beta.** Most features are verified on two Android phones against a Windows host, and many on-device checks are still open. The shared keyboard and mouse has not yet been tried at a desk with two real computers, and Linux driving it has run live on Hyprland only (see the end of these notes). See [`docs/device-verification-checklist.md`](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/docs/device-verification-checklist.md). Please read [SECURITY.md](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/SECURITY.md) before using it on a network you do not control.

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

Pocketdesk 0.8.2 lets a Linux computer drive the shared keyboard and mouse too, takes the lag out of sharing, and fixes what stopped the desktop app's picture and the Pair page's link on a plain Arch Linux laptop. It updates 0.8.1 (or 0.7.0) in place (same signing key), keeping your saved computers and pairings. The phone app has nothing new in this release; it is rebuilt as 0.8.2.

**Linux drives the shared keyboard and mouse too (new)**
- Push the pointer off the edge of a Linux screen that faces another computer, or press Ctrl+Alt+Right, and your keys and mouse go there; push back to come home, as from Windows. Linux asks the desktop's input-capture permission for it: GNOME 45 or later, KDE Plasma 6.1 or later, or Hyprland with xdg-desktop-portal-hyprland 1.4 or later. GNOME and KDE show their permission dialog once; Hyprland asks nothing.
- To use it, turn on "Share my keyboard and mouse with my computers" and "This computer's keyboard and mouse can move to the others" on the Linux computer (desktop app, Keyboard and mouse), and allow "Keyboard and mouse" for it on the other computer's Devices page. It works only while the link between the two is direct, never through a relay.
- While you work on the Linux computer itself, the computers it drives are not told someone is there (its desktop says nothing of keys made at home), so they stop being kept awake by it after a minute.
- It ran live on one real desktop only: Hyprland 0.56 with xdg-desktop-portal-hyprland 1.4.1, where a laptop's own touchpad drove a stand-in computer (7 crossings out and back, clicks, the wheel, typing in order, nothing left held). GNOME and KDE are not yet checked.

**No lag while sharing**
- While you share the keyboard and mouse, keys and moves are no longer held up: Windows' timer runs at 1 ms while you share (the network's share of a key fell from about 16 ms to about 3 ms), and a tiny packet every 50 ms keeps the other computer's Wi-Fi awake, only on your own network (first keys after a quiet spell no longer wait up to a tenth of a second for it). "Keep the connection quick while sharing" in Keyboard and mouse turns it off. How it feels at a real desk is not yet checked.

**Fixes from a real Arch Linux laptop**
- **The desktop app showed no picture:** FFmpeg 9 dropped `-vsync`, so the picture in the phone's cast window and in another computer's window never appeared. Pocketdesk now asks each ffmpeg which frame-rate flag it knows and uses that one, for the decoder and for the server's encoder alike (FFmpeg 4.4 lacks the new one).
- **The Pair page had no link** on a computer without the `hostname` program (a stock Arch), and its announcement on the network no address; Pocketdesk now reads this computer's addresses from its network interfaces.
- **The switch notice names the right computer:** a computer that only takes input (one with driving off) was shown as "this computer"; it now carries the name it was paired under, and says its own name with its screens.
- **A Linux desktop with neither Hyprland nor X** (Sway with Xwayland off, for one) now reports its screens, read from Wayland, so the keyboard and mouse can cross onto it.
- **Debug logs for crossings:** the log (at debug level) now says each crossing, release and landing, for finding why a crossing did not happen.
- **For developers:** running the tests no longer writes into your real Pocketdesk state folder (they used to create a trusted-devices file there).
- The winget manifests, the F-Droid recipe and the AUR package are brought to 0.8.1's files and checksums.

### New in 0.8, if you are updating from 0.7.0

Pocketdesk 0.8 brings your computers together. One keyboard and mouse moves between them, Windows driving Linux (and now Linux driving too), and the clipboard, images, files and sound follow the pointer; your phone can be another screen beside them; and one computer can show another's screen in a window. 0.8.0 was tagged but never published (its release run stopped on a flaky test), so these first reached people in 0.8.1.

The shared keyboard and mouse is new and largely unchecked at a desk. It is unit-tested throughout, and its receiving end ran on a real Linux laptop from a script, but nobody has yet pushed a real Windows mouse onto a Linux screen with it. Everything it does is off until you turn it on. Please try it and tell us how it went.

**One keyboard and mouse across your computers (new in 0.8)**
- **Windows drives Linux:** pair two computers, turn on "Share my keyboard and mouse with my computers" on both (desktop app, Keyboard and mouse), and allow "Keyboard and mouse" for the Windows computer on the Linux computer's Devices page. Then push the pointer off the edge that faces the other computer, or press Ctrl+Alt+Right, and your keys and mouse go there; push back to come home. It works only while the link between the two is direct (at home), never through a relay.
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
- **Linux driving, beyond Hyprland's stand-in run** (§220): crossing to a real Windows computer and back; keys and modifiers there; the safety nets (Wi-Fi off, the lock screen, a monitor plugged or unplugged); a fractional-scale screen's wall; a full-screen game keeping the pointer; the phone's screen sharing after driving; GNOME 45 or later and KDE Plasma 6.1 or later, the permission dialog included; and GNOME's screen sizes at scale 2.
- **No lag while sharing**: how it feels over real Wi-Fi at a desk, with a real Windows computer and a real Linux one: whether keys and moves are no longer held up, and a first key after a quiet spell. Nobody has felt it yet.
- **The shared keyboard and mouse at the desk** (checklist §214 and §215): Windows driving Linux with its real mouse and keyboard, at the edge and by the switch key; typing; Win+L; a pulled network and a server stopped while away; mixed display scales; the gaming lock with real games and anti-cheats; the badges, Identify and the tray; the clipboard and images following both ways; Linux kept awake while driven; the arrangement's crossings; and the latency over real Wi-Fi. What did run: a script typing into a headless Sway on the Linux laptop (102 keys in order, none left down; a killed sender's key let go about a second later; a lock screen typed into).
- **Files dragged across the edge** (§216): whether Windows' drag finds Pocketdesk's drop target under the parked cursor at all, and drops of several files, folders, and files from a zip or from a phone shown in Explorer.
- **The phone as a screen with a real mouse** (§217): it was checked on an OPPO phone from a script (taps, swipes, scrolls, Back, Home, typing, a held drag), not yet with a Windows mouse crossing onto it, nor its lag from away.
- **Sound following the mouse** (§218): not yet heard between two real computers; only a test tone crossed a link on one machine.
- **Another computer's screen in a window** (§219): it ran between two throwaway servers on one Windows computer, view-only, by both roads, but not yet between two real computers: control, keys, a second monitor, reconnecting, quality, the session list, sound, Linux showing Windows, screenshots and recordings.
- **Updating:** from 0.8.1 (and 0.7.0) to this release on a phone, and a clean install.

The full list is in [the checklist](https://github.com/sreevarshan-xenoz/Pocketdesk/blob/main/docs/device-verification-checklist.md) (§214 on are the 0.8 series').
