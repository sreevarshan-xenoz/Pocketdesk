# M3: image copies without a session, both ways

Plan: `docs/research/2026-10-21-m-cycle.md`, M3. It builds on N4's image clipboard (`docs/superpowers/specs/2026-10-14-image-clipboard-design.md`) and T10b's clipboard outside a session (`docs/superpowers/specs/2026-10-16-clipboard-anytime-design.md`).

## Why

Copied images cross only during a session. Outside one, T10b pushes the computer's text, and the phone's "Send clipboard" tile sends text. A screenshot copied on the computer never reaches the phone's clipboard, and an image copied on the phone reads as "the clipboard is empty".

Phone Link copies images both ways; KDE Connect has had a merge request for it since 2020 (MR 221), and GSConnect issue 678 asks for it.

## What

### The computer's copied image to the phone

- **On the computer:** desktop Settings "Images to phones' clipboards" (`clipboard_images`, off by default). With it on, T10b's watcher notices when the clipboard holds an image and no text. Once per change, it pushes `kdeconnect.clipboard {image: {id, size}}` to each phone it pushes text to: phones with the Clipboard permission and "Clipboard to phones" on.
  - The id is the PNG's SHA-256, as N4's.
  - The image is kept for the phone's fetch until the clipboard changes.
  - Not a copy marked private (Windows' clipboard history and cloud marks, KDE's password-manager hint), as for text.
  - Not over 32 MiB, as N4.
  - Not to the owner's other computers or KDE Connect devices: they get text only, as now.
- **On the phone:** "Computer's copied images in the background" (off by default, under the clipboard's Settings). With it on and the background clipboard keeping its link (T10b's "Computer's clipboard in the background"), the notification listener fetches a pushed image with N4's `clipboard_image_fetch`, on the road the link came by (2e), and puts it on the phone's clipboard.
  - **Unmetered networks only:** Wi-Fi or Ethernet, never mobile data.
  - **Not fetched again:** an image the phone itself just sent (`ClipboardShared.sentImageId`), one it fetched already, or one over N4's limit.
  - **Once per image:** a failed fetch waits a minute before it is tried again.

### The phone's copied image to the computer

- The "Send clipboard" tile, holding an image and no text, sends it as N4 does (`clipboard_image`). It is downsampled as N4's, and it asks first when the copy is marked private, as for text.
- "To computer" stays text only: Android hands it the selected text.

### A seam for tests

`POCKETDESK_CLIPBOARD_IMAGE_FILE`, an absolute path to a PNG, stands in for the computer's clipboard image: the watcher and the fetch read it, and an image from the phone is written to it. The server warns at start when it is set, as for the text seam.

### Also fixed

N4's own image read in a session did not check the private marks, which text did. It does now: a copy marked private is offered to no phone.

## Not now

- **Images in clipboard history** (D3): the history keeps text only.
- **Images to other computers and KDE Connect devices.**
- **"To computer" with an image.**

## Privacy and cost

- **Privacy:** the image goes only where the computer's text goes, and only with both switches on. SECURITY.md limit 8 gains a line for it.
- **Cost:** a screenshot copied on the computer is fetched once per change, only on Wi-Fi, and only with the phone's switch on.

## Tests

- **Server:**
  - the watch noticing an image once, and text after it;
  - the push's body;
  - only the phone targets text goes to;
  - the seam read back.
- **Bridge:** an image push kept, a bad id or size dropped, and the newest of text or image taken.
- **Phone (JVM):** the background fetch's rule: the switch, the network, an image sent here, a repeat, a failure's wait, and the limit.

## On the device (checklist §168)

- A PNG in the seam on a throwaway server; the OPPO with both switches on, on Wi-Fi, and the screen on. The image reaches the phone's clipboard: read back by its SHA-256 through a throwaway app.
- The same with either switch off: nothing fetched.
- The tile with an image copied on the phone: the seam file holds it.
