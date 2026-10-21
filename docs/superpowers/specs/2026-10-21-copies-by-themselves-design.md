# M10 part 2: copies on the phone sent by themselves

Plan: `docs/research/2026-10-21-m-cycle.md`, M10. The spike there found that a copy on the OPPO (Android 14, ColorOS 14) raises a window event from SystemUI's clipboard overlay, which C8's Accessibility service can see.

## Why

- **The gap:** on Android 10 and later only the app in front, or the keyboard, may read the clipboard. A copy on the phone therefore reaches the computer only when the owner taps the clipboard tile (T10b), or opens a session.
- **Asked for:** KDE Discuss "KDE Connect ClipBoard sync" (2023-10-24 to 2026-09-06), and the Shizuku builds users pass around.

## What

- **The phone's Settings:** "Send my copies by themselves", off by default, Android 13 and later only (where SystemUI shows the clipboard preview). It needs C8's Accessibility service ("Pocketdesk control"), and is its own switch, not C8's. The send dials as the tile does, so it needs no notification access.
- **The trigger:** with the switch on, C8's service listens for window changes only from `com.android.systemui`: a window's state, and its contents. It checks the switch and the lock first. When the event's own window is the clipboard overlay, a copy was just made. On the OPPO the overlay's window gives no title to Accessibility, so it is known by its own views: a view whose id starts with `com.android.systemui:id/clipboard` (`clipboard_ui` there). A window titled `ClipboardOverlay` counts too. A change inside a preview already up counts only if what it shows changed, judged by a hash of its texts kept in memory, never the text.

  With the switch off, the service listens for nothing, as before.
- **The read:** the service starts a window that never shows (`CopyReadActivity`). An app with a bound Accessibility service may start one from the background (`BAL_ALLOW_PERMISSION` on the OPPO). Once the window has focus, it reads the clipboard's text and closes at once. Android 12 and later say "Pocketdesk pasted from your clipboard" each time, and the Settings row says so.
- **What goes:** the text goes to the phone's own computer, as the tile sends it (`ClipboardSend`), with no message. It is not sent when it is:
  - marked sensitive by the app that copied it (a password manager);
  - what just crossed between the phone and the computer;
  - blank;
  - over 256 KiB;
  - copied while the phone is locked.

  At most one read every 1.5 s; a copy within that is read once it ends, never dropped. The text last read is never sent again while its send is on its way. An image is not sent: an image copy already reaches the computer by the tile (M3).

## Left out

- Android 12 and older: no clipboard overlay, so the row is not shown.
- A system that hides its overlay (some makers' settings): nothing is sent; the row says it needs Android's clipboard preview.
- **Seen on the OPPO:** a second copy within about two seconds of the first raised no event the service sees, so only the first went. Keys typed in the moment the reading window has focus (about a tenth of a second after a copy) go nowhere.
- Images.

## Privacy and safety

- **What it watches:** only SystemUI's window changes, and only while the switch is on. The service reads no other app's screen for it.
- **What it reads:** the clipboard, the moment after a copy, as the tile does.
- **Never sent:** a sensitive copy.
- **Advanced Protection** (Android 17) revokes the service, as for C8.
- **SECURITY.md limit 45.**
- **Android says so:** on 12 and later, "Pocketdesk pasted from your clipboard" shows after each read.

## Tests

- **JVM:** `CopyWatchRule`: the switch, a computer, the lock, the debounce, the overlay's title; what is sent (sensitive, the last crossed, blank, too long).
- **On the OPPO, by adb:** a copy by keys in a scratch field reaches the computer's clipboard seam on a throwaway server; a sensitive copy does not; the switch off sends nothing.

## As built

- **Phone:**
  - `clip/CopyWatch.kt`: `CopyWatchRule` (`supported`, `isOverlay`, `delay`, `send`) and `CopyMemory`;
  - `clip/CopyReadActivity.kt`;
  - `ControlService`: `syncCopyWatch`, `isClipboardOverlay`, `previewHash`, `readCopy`, with the service's events set at runtime only while the switch is on;
  - `Prefs.copiesByThemselves`; `ui/ControlSettings.kt`'s `CopiesSetting`; the health check;
  - strings in English, Spanish and Tamil.
- **Checked on the OPPO (ColorOS 14, Android 14), by adb:**
  - a copy by keys in the Google app's search field raised the overlay's window event (`clipboard_ui` in its views);
  - the reading window was started from the background (`BAL_ALLOW_PERMISSION`), took focus and read the clipboard ("Pocketdesk pasted from your clipboard");
  - two copies 6 s apart were both read.

  The send itself had no computer to reach (the phone's saved computer was a stopped throwaway server), so it is owed.

## Review

A fresh review found 0 Critical, 2 Important and 9 Minor. The fixes are in one commit.

- **Important:**
  - a second copy within the debounce was dropped;
  - on Android 12 and older the switch could say On while nothing could work, all day.
- **Minor, fixed:**
  - repeat reads while the preview stayed up;
  - the expensive checks run first;
  - words that claimed less than the code reads;
  - the docs;
  - the health card's words;
  - a notification-access requirement the send does not have;
  - no deadline on the reading window;
  - a deprecated transition;
  - tests for the debounce, the version and the last read.
- **Left:** the second copy within two seconds on ColorOS 14 (above).
