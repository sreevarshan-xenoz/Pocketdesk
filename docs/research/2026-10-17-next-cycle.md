# The next cycle: transfers you can watch, checks that run themselves, and the phone from the computer (2026-10-17)

Date: 2026-10-17, after the finish-and-polish plan (`docs/research/2026-10-17-finish-and-polish.md`, U1–U6 and N1–N9). Three inputs:
- **What is owed.** The device checklist has about 525 unchecked items against 275 checked. A rough split by area:
  - the remote session: about 150;
  - connectivity and pairing: about 130;
  - files and the clipboard: about 70;
  - notifications, texts, battery and media: about 50;
  - the desktop app and the Windows host: about 45;
  - the phone's UI and Android 17: about 45;
  - Linux, KDE and the ecosystem: about 35.

  Perhaps a fifth can be checked on this box with the A71.
- **What the repo shows:**
  - The desktop app shows a transfer only once it has ended: `desktop/src` has no progress anywhere.
  - The phone's transfer notification has a bar but no Cancel.
  - Both ends save into Downloads with no choice. `POCKETDESK_DOWNLOAD_DIR` is a test seam only.
- **What users of these apps ask for, 2025–2026** (KDE Connect, GSConnect, Phone Link, LocalSend, scrcpy, RustDesk, Apollo; sources inline). The demand signals are moderate: forum threads with their view counts, and changelogs. Platform facts are firmer.

The owner's standing rules hold:
- spec, then TDD; one logical change per commit;
- a fresh review per item, its findings fixed one commit each;
- device checks on the A71 against this box, and the Azure VM for Linux;
- docs with each change;
- every feature's knobs in Settings on both sides;
- no macOS or iOS;
- no power, lock or server-stop actions on the owner's computer.

## Order

| # | Item | Size | Checkable here |
|---|---|---|---|
| C1 | Transfers you can watch and stop: progress, speed and Cancel on both sides | S–M | Yes |
| C2 | Choose where received files land, on each side | S | Yes |
| C3 | Notifications that failed to send wait and go later; a forced-Doze run | S–M | Yes |
| C4 | A headless test harness for the desktop app (`egui_kittest` through AccessKit), with light and dark snapshots | M | Yes |
| C5 | The phone with TalkBack, large fonts, landscape and split screen | S–M | Yes |
| C6 | The owed checks that can run here, tagged and run in batches | M | Yes |
| C7 | Recent photos on the computer (opt-in) | M | Yes |
| C8 | Control the phone from the computer: an opt-in Accessibility service behind the cast | L | Yes (by a seam; clicks by hand owed) |
| C9 | A Windows package identity (a sparse package): the computer's notifications on the phone, Pocketdesk in the Share sheet | M | Yes, with the owner's OK to trust a test certificate |
| C10 | Getting it to people: Obtainium-friendly release assets, a winget manifest, F-Droid's screenshots and recipe | S–M | Partly (registering for Android developer verification is the owner's call) |
| C11 | Android 17's local-network permission on a real Android 17 device | S | No (the owner checks it) |
| C12 | A second screen at the phone's shape on Windows (an indirect display driver) | M–L | Yes, with the owner's OK to install a driver |

**Why this order:**
- C1–C3 fix what a person meets every day: a send they cannot watch or stop, files landing where they did not choose, and notifications lost while the phone sleeps.
- C4–C6 turn owed checks into ones that run themselves, or that are at least written down where they can run.
- C7–C9 are the larger features people ask for.
- C10–C12 wait on the owner's decisions or on hardware.

---

## C1. Transfers you can watch and stop

**Why:** failed and miscounted transfers recur on KDE's forum ("Can't send multiple files from PC to android", https://discuss.kde.org/tag/kdeconnect). Pocketdesk's are resumable, but nobody can see one moving on the computer or stop one on the phone.

**Done:**
- the desktop's Files page shows each running transfer, and each folder, with its percent and rate;
- the phone's notification has Cancel;
- a Cancel on either end stops both;
- the resumable part is kept, so sending again continues;
- the server writes progress to the state folder at most once a second (the inbox and mirror rules from R6c).

## C2. Choose where received files land

**Why:** "Add ability to change default save location" (KDE Discuss, August 2026, https://discuss.kde.org/tag/kdeconnect).

**Done:**
- a folder setting on the computer (desktop Settings, with a Choose button);
- a folder on the phone, picked with Android's folder picker (Settings);
- each "saved" notice names the folder;
- a folder that has gone falls back to Downloads and says so.

## C3. Notifications that failed to send wait and go later

**Why:** notifications that stop while the phone is locked are the most common KDE Connect complaint found (https://discuss.kde.org/t/kde-connect-notifications-from-phone-not-showing-on-pc-when-screen-locked/47634, June 2026). GSConnect is preferred for staying connected while asleep (https://discuss.kde.org/t/kdeconnect-issues-and-how-gsconnect-works-better-in-that-regard/39391). T2 left this open: a notification that fails to send is dropped.

**Done:**
- a failed notification waits on the phone, bounded in count and age, and goes when the link is back;
- the computer marks it late, with its time;
- a removed notification is dropped from the wait;
- an hour with `dumpsys deviceidle force-idle` on the A71, measured as N7 was.

## C4. A headless test harness for the desktop app

**Why:** this comes from the repo. About twenty owed desktop checks are done by eye (§24, §88, §94, §99). `egui_kittest` drives egui through AccessKit, which the app already enables.

**Done:**
- tests for the navigation, the save bar, Unpair, the empty states and the phone panel;
- light and dark snapshots at 720×480;
- all of it running in CI on Windows and Linux.

## C5. The phone with TalkBack, large fonts, landscape and split screen

**Why:** this also comes from the repo. TalkBack, landscape and the keyboard in landscape are owed (§93, §21, §26, §17).

**Done:**
- a TalkBack walk through Home, the Add form, Settings and the session menu;
- font scale 1.3 and 2.0;
- landscape and split screen;
- what is found, fixed;
- Compose instrumented tests, run with `leaveApksInstalledAfterRun`.

## C6. The owed checks that can run here

**Done:**
- each unchecked item in the checklist is tagged with where it can run: here, Linux, a SIM, a second phone, or away from home;
- the "here" batch is run and ticked: scaling at 125 and 150 % with taps on a second monitor, sound underruns from logcat, the transfer edge cases, the background clipboard and files, and pressing a toast's answer through UI Automation;
- power and lock stay excluded.

## C7. Recent photos on the computer

**Why:** Windows' phone panel in the Start menu leads with photos (https://www.androidauthority.com/phone-link-panel-windows-start-menu-3558774/).

**Done:**
- off by default, and it asks for photo access;
- the newest camera photos as thumbnails in the desktop app's Phone page, with Save, Copy and Open;
- a full image is fetched only when asked, through the existing transfer;
- checked with `adb push` into DCIM.

## C8. Control the phone from the computer

**Why:** the cast is view only. KDE has an open request for mirroring with control without adb (https://invent.kde.org/network/kdeconnect-meta/-/issues/4). Phone Link's app mirroring works only on some phone brands.

**Done:**
- an Accessibility service, off by default, under its own permission on both sides, takes taps, swipes, Back, Home and text from the cast window;
- it says so when Android refuses it (Advanced Protection on Android 17);
- SECURITY.md gains a limit.

## C9. A Windows package identity

**Why:** a signed sparse package gives the plain exe an identity. That unlocks:
- the computer's notifications on the phone (`server/src/notification_listener_windows.rs` waits on it);
- Pocketdesk in the Share sheet (LocalSend shipped this in 1.18.0);
- the way to winget and the Store.

**Done:**
- a toast on this computer appears on the A71;
- Share → Pocketdesk sends a file;
- a test certificate is trusted only with the owner's OK.

## C10. Getting it to people

**Why:** Android developer verification began on 30 September 2026 in four countries and goes global in 2027 (secondary reporting). Users want signed APKs outside Play and F-Droid, through Obtainium for instance (https://discuss.kde.org/t/kde-connect-without-f-droid-or-play-store).

**Done:**
- release assets named so Obtainium finds the APK;
- a winget manifest;
- the F-Droid screenshots and the fdroiddata merge request;
- registering is the owner's decision.

## C11. Android 17's local-network permission

**Why:** an app targeting API 37 has all LAN traffic blocked until the permission is granted (https://developer.android.com/privacy-and-security/local-network-permission). LocalSend shipped a fix for exactly this in 1.18.1. Checklist §98's first item has never run, because the emulator stalled; the owner will check it.

## C12. A second screen at the phone's shape on Windows

**Why:** Intel Unison's end took away tablets as wireless displays. Apollo's per-client virtual display is much of its appeal (https://github.com/ClassicOldSong/Apollo/discussions/537). Pocketdesk does this on Sway only.

**Done:** an MIT, signed indirect display driver (VirtualDrivers) adds the screen when a session starts and removes it after. Installing the driver is the owner's call.

---

## Below the line

- **A quiet or pause switch:** one thread, weak demand.
- **The phone's camera as a webcam:** large; scrcpy #776 is still active, and Phone Link has it.
- **A background clipboard on GNOME:** Mutter still lacks `ext-data-control`, and it is Linux only.
- **Android 17 Handoff:** it works Android to Android only.
- **Windows Cross-Device Resume:** a partner API.
