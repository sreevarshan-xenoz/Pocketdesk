# M3: image copies without a session, both ways

Plan: `docs/research/2026-10-21-m-cycle.md`, M3. It builds on N4's image clipboard (`docs/superpowers/specs/2026-10-14-image-clipboard-design.md`) and T10b's clipboard outside a session (`docs/superpowers/specs/2026-10-16-clipboard-anytime-design.md`).

## Why

Copied images cross only during a session. Outside one, T10b pushes the computer's text, and the phone's "Send clipboard" tile sends text. A screenshot copied on the computer never reaches the phone's clipboard, and an image copied on the phone reads as "the clipboard is empty".

Phone Link copies images both ways; KDE Connect has had a merge request for it since 2020 (MR 221), and GSConnect issue 678 asks for it.

## What

### The computer's copied image to the phone

- **On the computer:** desktop Settings "Images to phones' clipboards" (`clipboard_images`, off by default, applies at once). With it on, T10b's watcher notices when the clipboard holds an image and no text, and tells each phone it pushes text to: phones with the Clipboard permission while "Clipboard to phones" is on.
  - **The push:** `kdeconnect.clipboard {image: {id, size}}`, once per change, the id the PNG's SHA-256 as N4's. The image is kept for the phone's fetch until the clipboard changes.
  - **Never sent:** a copy marked private, as for text; one over 32 MiB, as N4; anything to the owner's other computers or KDE Connect devices, which get text only.
  - **When it is read:** an image is read only while a phone could take it. On Windows it is read again when another app held the clipboard (the Snipping Tool and clipboard history do, right after a screenshot). Where no `wl-paste --watch` says the clipboard changed (GNOME), it is read at most every 30 s.
- **On the phone:** "Computer's copied images in the background" (off by default, under the background clipboard, which keeps the link). The notification listener fetches a pushed image with N4's `clipboard_image_fetch`, on the road the link came by (2e), and puts it on the phone's clipboard.
  - **Wi-Fi or Ethernet only,** never mobile data, even on an unmetered plan. A VPN over Wi-Fi counts.
  - **On a thread of its own,** one fetch at a time, so a large image holds up nothing else on the listener.
  - **It waits until fetched or replaced:** a failed fetch is tried again a minute later, and one that came on mobile data is fetched on Wi-Fi. A newer push replaces it. A session that starts fetches it itself.
  - **Not fetched:** an image the phone itself just sent (`ClipboardShared.sentImageId`), or one it already has. Text from the computer clears that memory, so the same image copied again is fetched again.

### The phone's copied image to the computer

- The "Send clipboard" tile, holding an image and no text, sends it as N4 does (`clipboard_image`): downsampled as N4's, known by its id, so the computer's push of it is not fetched back.
  - It asks first when the copy is marked private, as for text.
  - Away from home with no link up, it opens one (over the internet when it must) and tries again, as text does.
  - It never sends an image Pocketdesk put there from the computer.
- "To computer" stays text only: Android hands it the selected text.

### A seam for tests

`POCKETDESK_CLIPBOARD_IMAGE_FILE`, an absolute path to a PNG, stands in for the computer's clipboard image, with or without `POCKETDESK_CLIPBOARD_FILE` (the text). Together they are the whole clipboard:
- either turns off the real clipboard's change signal;
- the text's text wins;
- the image's alone has no text;
- text from a phone replaces the image;
- an image from the phone is written to it.

The server warns at start when it is set, as for the text seam.

### Also fixed

- **N4's read for a phone that asks** (a session's poll) did not check the private marks, which T10b's watcher did. A password copied on the computer reached a phone in a session. It now answers as for an empty clipboard, text or image. A computer of the owner's (2g) asking is answered the same way.
- **A Windows privacy mark that cannot be read** (another app held the clipboard) now counts as private, not as allowed.

## Not now

- **Images in clipboard history** (D3): the history keeps text only.
- **Images to other computers and KDE Connect devices.**
- **"To computer" with an image.**

## Privacy and cost

- **Privacy:** the image goes only where the computer's text goes, and only with both switches on. SECURITY.md limit 8 says so.
- **Cost:**
  - **On the phone:** a screenshot copied on the computer is fetched once per change, only on Wi-Fi, only with the phone's switch on.
  - **On the computer:** it reads an image only for a phone that could take it.

## Tests

- **Server:**
  - the watch noticing an image once, and text after it;
  - the push's body;
  - the image stand-in read back and kept for a fetch;
  - a privacy mark that cannot be read counted private.
- **Bridge:** an image push kept; a bad id or size dropped; the newest of text or image taken.
- **Phone (JVM):**
  - `ClipboardPushRule.mayFetchImage`: the switch, the network, an image sent here;
  - `onWifi`: Wi-Fi, Ethernet, a VPN over Wi-Fi, an unmetered 5G plan, a metered hotspot;
  - N4's `ImageClipRule` as before.

## As built

- **Core:** `config.rs`, `clipboard_images`.
- **Server:**
  - `clipboard_push.rs`: `image_now`, `push_image`, `image_packet`, `may_push_image`, the 30 s fallback, the unreadable mark;
  - `plugins/clipboard.rs`: `image_for_push`, `image_stand_in`, `stand_in_clip`, the private check in `read_clipboard`;
  - `platform/clipboard_windows.rs`: `privacy_marks` failing closed;
  - `service.rs`: the seam warned.
- **Desktop:** the Settings row, which is greyed while "Clipboard to phones" is off.
- **Bridge:** `clipboard_push.rs` keeps an image push.
- **Phone:**
  - `RustCore.ClipboardPushed` and `PushedImage`;
  - `clip/ClipboardPush.kt`, `BackgroundClipboard`: the pending image, its fetcher thread and the rules;
  - `clip/SendClipboardActivity.kt`: the image branch;
  - `Prefs.clipboardImagesInBackground` and the Settings row;
  - strings in English, Spanish and Tamil.

## Review

A fresh review found 0 Critical, 4 Important and 16 Minor. Each fix is its own commit, except findings that share lines, which share a commit.

- **Important:**
  - on Windows, a read another app blocked lost that copy's push;
  - a fetch was tried once;
  - the fetch held up the listener's files and battery;
  - GNOME re-read and hashed an image every 2 s.
- **Minor, fixed:**
  - an unreadable privacy mark read as allowed;
  - the seam did not work alone, or beside the text's;
  - "unmetered" was not Wi-Fi;
  - a re-copied image was not fetched again;
  - an image was read for no one;
  - the tile:
    - kept the id of a failed send;
    - had no internet link;
    - said the computer was out of reach for an unreadable image;
    - its description;
  - a session starting mid-fetch;
  - `docs/protocol.md`'s clipboard row.
- **Minor, left:**
  - **The private check:** it reads the marks in a separate look from the data, as T10b's does.
  - **The tests:** the private reply and `may_push_image` have no test of their own. The stand-in's test shares the image cache with others.
  - **The tile and the session:**
    - the tile says "no text" when the clipboard holds Pocketdesk's own image;
    - a session's and the background's fetch of one image share its part file (the background does not start while a session runs);
    - a session's last poll, kept after it ends, may be fetched once more.

## On the device (checklist §168)

- A PNG in the seam on a throwaway server, the OPPO with both switches on, on Wi-Fi: the image reaches the phone's clipboard, read back by its SHA-256. Off, or on mobile data: not fetched, until on Wi-Fi.
- A screenshot copied on Windows with a phone linked; a private copy; the tile with an image, at home and away; GNOME.
