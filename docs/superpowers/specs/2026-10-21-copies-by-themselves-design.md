# M10 part 2: copies on the phone sent by themselves

Plan: `docs/research/2026-10-21-m-cycle.md`, M10. The spike there found that a copy on the OPPO (Android 14, ColorOS 14) raises a window event from SystemUI's clipboard overlay, which C8's Accessibility service can see.

## Why

- **The gap:** on Android 10 and later only the app in front, or the keyboard, may read the clipboard. A copy on the phone therefore reaches the computer only when the owner taps the clipboard tile (T10b), or opens a session.
- **Asked for:** KDE Discuss "KDE Connect ClipBoard sync" (2023-10-24 to 2026-09-06), and the Shizuku builds users pass around.

## What

- **The phone's Settings:** "Send my copies by themselves", off by default. It needs:
  - C8's Accessibility service ("Pocketdesk control");
  - the notification listener, which keeps the computer's link.

  It is its own switch, not C8's.
- **The trigger:** with the switch on, C8's service listens for one kind of event only, a window's state changing, and only from `com.android.systemui`. When the event's window is the clipboard overlay, a copy was just made. On the OPPO the overlay's window gives no title to Accessibility, so it is known by its own views: SystemUI's `clipboard_ui` and `actions_container`, read with view ids reported. A window titled `ClipboardOverlay` counts too.

  With the switch off, the service listens for nothing, as before.
- **The read:** the service starts a window that never shows (`CopyReadActivity`). An app with a bound Accessibility service may start one from the background (`BAL_ALLOW_PERMISSION` on the OPPO). Once the window has focus, it reads the clipboard's text and closes at once. Android 12 and later say "Pocketdesk pasted from your clipboard" each time, and the Settings row says so.
- **What goes:** the text goes to the phone's own computer, as the tile sends it (`ClipboardSend`), with no message. It is not sent when it is:
  - marked sensitive by the app that copied it (a password manager);
  - what just crossed between the phone and the computer;
  - blank;
  - over 256 KiB;
  - copied while the phone is locked.

  At most one read every 1.5 s. An image is not sent: an image copy already reaches the computer by the tile (M3).

## Left out

- A phone whose system shows no clipboard overlay: nothing is sent, and the Settings row says it needs the overlay ("tested on ColorOS 14").
- Images.

## Privacy and safety

- **What it watches:** only SystemUI's window changes, and only while the switch is on. The service reads no other app's screen for it.
- **What it reads:** the clipboard, the moment after a copy, as the tile does.
- **Never sent:** a sensitive copy.
- **Advanced Protection** (Android 17) revokes the service, as for C8.
- **SECURITY.md limit 45.**

## Tests

- **JVM:** `CopyWatchRule`: the switch, a computer, the lock, the debounce, the overlay's title; what is sent (sensitive, the last crossed, blank, too long).
- **On the OPPO, by adb:** a copy by keys in a scratch field reaches the computer's clipboard seam on a throwaway server; a sensitive copy does not; the switch off sends nothing.
