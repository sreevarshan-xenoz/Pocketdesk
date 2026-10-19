# D3, clipboard history and Resend (design)

Date: 2026-10-18. Plan: `docs/research/2026-10-18-next-cycle.md`, D3.

## Why

KDE's Remote Input request (2026-02-12, https://discuss.kde.org/t/kde-connect-remote-input-compose-send-history-and-send-as-clipboard-option-request/44275) asks for a history of what was sent, and for sending it again. Microsoft is prototyping a cross-device clipboard history (reported July 2026). Today:

- the phone keeps a history of its own copies (`ClipHistory`), but nothing there sends one to the computer;
- the computer keeps none. A copy that crossed and was then overwritten is gone.

## What it does

### On the computer

**History.** With "Clipboard history" on (desktop Settings, Phones section, off by default), the server keeps the last texts that crossed either way. The limit is 20 by default (`clipboard_history`, 0 for off, at most 100). The history is kept in `<state>/clip_history.json`, owner-only, through `core::local::clip_history`:

- **Phone → computer:** what a phone set on this computer's clipboard (`note_from_phone`), with the phone's id.
- **Computer → phone:** what the watcher pushed to phones (`push_change`), as "from this computer".
- **Left out:**
  - copies marked private, which never cross anyway;
  - a pairing link;
  - text over 64 KiB, kept cut, with a note.
- **Repeats:** the same text again moves to the top.

**A Clipboard page** in the desktop app (in the Phones group) lists the history, newest first, each entry showing:

- who it came from and when;
- a few lines of the text;
- **Copy** (onto this computer's clipboard; the watcher then pushes it as usual);
- **Send to a phone…** (a picker of linked phones with Clipboard on, through `clipboard_out/`; the server sends `kdeconnect.clipboard` over the live link).

"Clear history" empties it. Turning the setting off clears it at once.

**Local API:** `pocketdesk clipboard [--json]` lists the history, and `pocketdesk clipboard --send N --to <phone>` sends the Nth entry (1 the newest).

### On the phone

The history sheet's entries gain "Send to computer". It sends that text to the phone's own computer (the share path that "To computer" uses, N2) and says whether it went.

## Not done

- The computer's own copies that never crossed are not kept: the computer has its own clipboard history (Win+V, Klipper).
- No sync of the two histories.

## Testing

- **Rust:**
  - `clip_history` keeping, moving to the top, the limit, cutting, the private and pairing-link exclusions, clearing;
  - the server's hooks, through the pure parts;
  - `clipboard_out` request files;
  - the local API against its doc.
- **Desktop:** the headless harness (C4): the page lists entries, Copy, the phone picker, Clear.
- **Phone:** a JVM test of what the button sends; the device:
  - a copy on the phone shows on the computer's page;
  - the computer's copy shows as from this computer;
  - Send to a phone puts it on the A71's clipboard;
  - Send to computer from the phone's sheet works.

## As built, and the review

- **What counts as crossed:**
  - a phone's or computer's text that set this computer's clipboard (`note_from_phone`, which every phone, KDE device and computer path goes through);
  - this computer's own copy, once it went to anyone: a phone pushed to, a linked computer, a KDE device, or the phone that asked for the clipboard itself in a session.

  The same text as the newest is not written again, so a phone asking every few seconds writes nothing.
- **One writer:** a single thread in order, fed by a queue of 64 (more are dropped), reads the setting at each text, holds the file to it (emptied at 0, trimmed when lowered) and adds the text. The server also holds it to the setting at start. The page and `pocketdesk clipboard` read only what the setting allows.
- **Not kept:** a pairing link anywhere in a text, in any case (the check that keeps it off phones too); a copy marked private, which never crosses.
- **Send to…:**
  - goes to a linked phone or computer with Clipboard on;
  - a cut entry is not sent;
  - a resend keeps its entry's origin;
  - the page says it asked, not that it sent.
- **The phone:** "Send an earlier copy" on each computer's ⋮ menu opens its own history; a pick goes to that computer as the Send clipboard tile does, over the internet road when the one-off connection fails (`ClipboardSend`).
- **Fresh review: 0 Critical, 5 Important.** Fixed one commit each:
  - the setting held only after the app's Save;
  - a thread per text, out of order and unbounded;
  - crossings to computers, KDE devices and a phone's own asking left out;
  - the pairing-link check only at the start of a text;
  - no internet road from Home.

  Minors fixed: an unreadable file overwritten, a resend's origin, cut entries sent, Clear's confirm armed for good, "Sent" before it went, the menu's words, the doc comment. Left: the 2 s read of the file on the page; entries of an unpaired phone kept until they fall off; the list's places shifting between listing and `--send`.
- **Device check:** in the evening's batch (checklist §113).
