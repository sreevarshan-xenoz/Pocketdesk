# Files from the computer without a session (T10d)

Item 5 of `docs/research/2026-10-15-what-makes-it-worth-using.md`, first half.

## The problem

A file sent from the computer reached the phone only while a session was open:
- the desktop app and `pocketdesk send` offered only phones in a session;
- the phone took offers only in the session screen.

Sending a document to the phone meant opening a remote-desktop session first.

## The change

- **The mirror:** `live_sessions.json` gains `linked`. It lists each phone holding a control link, in a session or not, and whether files can reach it. They can when one of its links has an address the computer can dial back; an internet-only link (T10a) has none.
- **Sending:**
  - the desktop app's Files tab and `pocketdesk send` take a phone in a session, or linked with `files`;
  - `pocketdesk link` takes any linked phone.
- **The phone:** Settings "Files from the computer in the background" (off by default). The notification listener keeps the control link to the phone's computer, as the clipboard and answers do, and takes each offer on its 5 s tick.
  - Where Android lets it, the download starts at once in the transfer service.
  - Otherwise a notification "The computer sends a file" offers it, and Save starts it.

  A notification tap may always start a foreground service.

## Tests

- **Core:** `LiveMirror::files_reach` and `reaches`, and a mirror from before T10d.
- **Server:** `pocketdesk send` to a phone linked at home is queued, and one linked only over the internet is refused.
- **Device:** with the switch on, notification access, the app in the background and no session, `pocketdesk send --wait` ends "saved on the phone" and the MD5s match.

## As built

- **The background start was allowed on the A71 (Android 16).** The listener's bound service gives the process the state that may start a foreground service (`uidState: BFGS`), so the notification fallback did not run there. It is kept for devices that refuse.
- **Device-checked (A71 → Windows):** a 300 KB file sent with no session, saved, MD5s matching. Checklist §67.
