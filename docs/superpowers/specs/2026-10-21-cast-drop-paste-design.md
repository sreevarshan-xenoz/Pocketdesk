# Drop files and paste into the phone's window (L2)

**Date:** 2026-10-21. **Branch:** `feat/cast-drop-paste`. The L cycle's second item (`docs/research/2026-10-20-l-cycle.md`).

## What is wrong

- **The phone's screen window takes no files.** Dropped on the main window they go (J5); dropped on the phone's own window, nothing happens.
- **Ctrl+V does nothing there.** With control on (C8), typing reaches the phone's field, but a paste does not.
- **Asked for:** Phone Link drags files onto the phone's screen; scrcpy pushes a dropped file and pastes with Ctrl+V.

## What changes

- **Files or folders dropped on the phone's window** go to that phone, as the Files page sends them (the outbox), when files reach it now. A line at the window's foot says what happened, for six seconds:
  - "Sending 2 files to the phone…" (the Files page's Sent list says when they have gone);
  - why not: the phone takes files in a session, or with "Files from the computer in the background" on;
  - a file that could not be queued, by name.
- **Ctrl+V in the window, while it controls the phone,** types the computer's clipboard text at the phone's focused field, as typing does. At most 1024 bytes go (the control road's limit for one text), cut at a character.
  - A paste is the owner's own act, so a copy a password manager marks private is pasted too, as it would be into any app here.

## Left out

- An image on the clipboard (N4's image road) for a paste.
- A paste longer than 1024 bytes in several pieces.

## Testing

- **Unit:** what a drop says (each case); a paste as text, cut at a character, ignored while not controlling.
- **On the device (checklist §157):**
  - a cast from the phone with control on;
  - a 3 MB file dropped, its MD5 matching on the phone;
  - a paste into a scratch field, read back with `uiautomator dump`.
