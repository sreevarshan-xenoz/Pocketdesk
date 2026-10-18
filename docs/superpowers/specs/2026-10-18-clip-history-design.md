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
  - an image, kept as a line ("an image") without its bytes;
  - text over 64 KiB, kept cut, with a note.
- **Repeats:** the same text again moves to the top.

**A Clipboard page** in the desktop app (in the Phones group) lists the history, newest first, each entry showing:

- who it came from and when;
- a few lines of the text;
- **Copy** (onto this computer's clipboard; the watcher then pushes it as usual);
- **Send to a phone…** (a picker of linked phones with Clipboard on, through `clipboard_out/`; the server sends `kdeconnect.clipboard` over the live link).

"Clear history" empties it. Turning the setting off clears it at once.

**Local API:** `pocketdesk clipboard [--json]` lists the history, and `pocketdesk clipboard send <phone> <index>` sends one entry.

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
