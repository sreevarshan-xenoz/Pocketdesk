# Drop files and paste into the phone's window (L2)

**Date:** 2026-10-21. **Branch:** `feat/cast-drop-paste`. The L cycle's second item (`docs/research/2026-10-20-l-cycle.md`).

## What is wrong

- **The phone's screen window takes no files.** Dropped on the main window they go (J5); dropped on the phone's own window, nothing happens.
- **Ctrl+V does nothing there.** With control on (C8), typing reaches the phone's field, but a paste does not.
- **Asked for:** Phone Link drags files onto the phone's screen; scrcpy pushes a dropped file and pastes with Ctrl+V.

## What changes

- **Files or folders dropped on the phone's window** go to that phone, as the Files page sends them (the outbox), when files reach it now. A cast alone does not: the phone takes files in a session, or with "Files from the computer in the background" on. A line over the window's foot says what happened, for six seconds:
  - "Sending 2 files to the phone…" (the Files page's Sent list says when they have gone);
  - why not: the phone takes files in a session, or with "Files from the computer in the background" on;
  - how many were queued, and what was not.
- **Ctrl+V in the window, while it controls the phone,** types the computer's clipboard text at the phone's focused field, as typing does. At most 1024 bytes go (the control road's limit for one text), cut at a character, and the window says when it cut. Line breaks go as they are.
  - A paste is the owner's own act, so a copy a password manager marks private is pasted too, as it would be into any app here.

## Left out

- An image on the clipboard (N4's image road) for a paste.
- A paste longer than 1024 bytes in several pieces.

## Testing

- **Unit:** what a drop says (each case); a paste as text, cut at a character. Whether control is on is decided where the window draws, as for all of its input.
- **On the device (checklist §157):**
  - a cast from the phone with control on, and "Files from the computer in the background" on;
  - a 3 MB file dropped, its MD5 matching on the phone;
  - a paste into a scratch field, read back with `uiautomator dump`.

## As built

- `desktop/src/cast_view.rs`: `take_drops` on the window's own input, through `model::queue_send`; `drop_words`; the line as an overlay; `Event::Paste` in `control_events`, cut by `pasted`. The app passes the phones files reach now (`model::send_target_ids`).

## Review

A fresh review (0 Critical, 1 Important, 7 Minor):

- **Important:** a drop during a plain cast is refused, since a cast keeps no link files ride; the window says why, and this spec and the device check now say so (a cast taking files itself is left for later).
- **Minor, fixed:** the line resized the picture, a partial failure hid what went, a cut paste was silent, the spec's test claim, and the docs owed.
- **Left:** the phones files reach are judged up to a second late; a multi-line paste into a one-line field goes as it is.
