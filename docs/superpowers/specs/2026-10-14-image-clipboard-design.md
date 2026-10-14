# Image clipboard, both ways (roadmap N4 / F8)

Date: 2026-10-14. Status: approved (autopilot). Builds on the clipboard sync (Tier 1 #4) and the file transfers inside TLS (F7, phase C).

## Why

Copying a screenshot on the desktop and pasting it into a chat on the phone, or the other way round, is the next thing people expect after text. Today the clipboard carries text only.

## How the clipboard works now

The phone drives it. While clipboard sync is on, every 4 s it:
- pushes its own text when that changed (`kdeconnect.clipboard`);
- asks for the desktop's (`kdeconnect.clipboard.connect`), which answers with `kdeconnect.clipboard {content}`.

Both packets ride the control channel (TLS) and fall under the Clipboard permission (F5).

## Design

### Bytes travel like files, not in the control channel

An image is megabytes, and a JSON line is the wrong carrier. The image takes the file-transfer path:
- the phone listens with a one-time key;
- the desktop dials the transport's peer;
- both ends are pinned inside TLS (phase C), and a status byte ends the transfer.

Each direction gets its own packet type, so the Clipboard permission covers it: a phone allowed the clipboard but not files still gets images. A packet type named `share.request` or `file_fetch` would fall under Files.

- **Phone → desktop: `kdeconnect.pocketdesk.clipboard_image` `{size, port, key}`.** The desktop dials back, reads at most 32 MiB into memory, checks for the PNG signature, and puts the image on its clipboard. Its status byte is 1 only once the clipboard holds it.
- **Desktop → phone: the reply to `kdeconnect.clipboard.connect`.** When the desktop's clipboard holds an image and no text, the reply is `kdeconnect.clipboard {image: {id, size}}`. The `id` is the hex SHA-256 of the PNG. The phone fetches it with `kdeconnect.pocketdesk.clipboard_image_fetch` `{id, port, key}`. The desktop sends it only while the clipboard still holds that image, and refuses otherwise (`file_fetch_refused {key, reason}`, as for files).

Text wins when both are there, so nothing changes for text.

### The wire format is PNG

- **The phone** decodes any image clip (`image/*`) to a bitmap and sends it as PNG.
- **Windows** has three clipboard formats for images:
  - **Reading:** the registered `PNG` format when an app put one there (browsers and the Snipping Tool do). Otherwise `CF_DIB`, converted to PNG with the `png` crate.
  - **Writing:** both `PNG` and `CF_DIB`, since Paint and older apps read only DIB.
  - A PNG that it wrote reads back as the same bytes, so the same id.
- **Linux:** `wl-paste --type image/png` and `wl-copy --type image/png`. Other image types are not offered.

The desktop caches the last image it read, keyed by the Windows clipboard sequence number, so a 4 s poll does not re-encode an unchanged image.

### No echo

The id is the same SHA-256 on both ends, so neither side sends back what it just received:
- The phone remembers the id of the image it last sent or received.
- An image the phone itself put on its clipboard (its own FileProvider URI) is never sent.

### The phone

- Received images go to `cache/clipboard/<id>.png`, served by a FileProvider (`grantUriPermissions`), and land on the clipboard as `ClipData.newUri`. Only the newest file is kept.
- Clipboard history stays text-only; an image is not an entry.
- Limits: 32 MiB of PNG. A larger image is skipped with a log line, not an error card.

## Pieces

1. **core:**
   - the two packet types in `capability_for` (Clipboard);
   - the shared constants (`CLIPBOARD_IMAGE`, `CLIPBOARD_IMAGE_FETCH`, `MAX_CLIPBOARD_IMAGE`).
2. **server:**
   - `platform::clipboard_windows` gains image get/set (PNG and DIB), and the Linux arm uses `wl-paste` / `wl-copy`;
   - the clipboard plugin answers with an image and handles both packets, reusing `share.rs`'s TLS, dial and stream helpers.
3. **bridge:**
   - `send_clipboard_image(fd, size)` and `fetch_clipboard_image(id, path)`, from the existing send and fetch code;
   - the reply's `image` reaches Kotlin through `getClipboard`'s result, which becomes JSON.
4. **app:**
   - `ClipboardSync` handles image clips both ways;
   - a FileProvider in the manifest.
5. **Tests:**
   - the DIB ↔ PNG conversion round-trips (top-down and bottom-up, 24- and 32-bit);
   - the reply prefers text, and gives the id;
   - a fetch for a stale id is refused;
   - a size over the limit is refused before any byte is read;
   - the capability table;
   - JVM tests for the phone's echo rule.
6. **Docs:** README (clipboard), the checklist and AGENTS.

## Device check (A71 ↔ this box)

- A screenshot copied on the desktop (Snipping Tool) appears in the phone's clipboard, and pastes into an app.
- An image copied on the phone (a photo's "Copy") pastes into Paint on the desktop.
- Neither bounces back: the logs show one transfer each.
- With the phone's Clipboard permission off, neither moves.

## Out of scope

- Files on the clipboard (copied files in Explorer): that is F7's job.
- Rich text and HTML.
- Images over 32 MiB.

## As built

- **Device-verified on this box and the A71 (LAN), desktop to phone:**
  - a bitmap copied on Windows (no "PNG" format, so the DIB path ran) reached the phone as a PNG whose SHA-256 is its id, once, and never came back;
  - after the review fixes, the same image copied again after some text in between was fetched again, and the text reached the phone.

  The Windows clipboard's PNG and DIB round trip is a live ignored test.
- **Owed (checklist §38):**
  - phone to desktop on a device: the A71 has no app that copies an image;
  - pasting into a phone app;
  - the permission switch;
  - a 4K screenshot;
  - the Linux host.
- **Fresh review (0 Critical, 6 Important), fixed:**
  - **A decode bomb.** A few hundred bytes of PNG claiming 16384 x 16384 made the server commit about 4 GiB. Both readers now refuse more than 40 megapixels from the header.
  - **The clipboard stayed open while a large bitmap was encoded,** and an unusable image was re-read every poll. It is now copied out first, and failures are remembered by sequence number.
  - **The phone's sync state went stale across text and image,** and a stale image overwrote the desktop at every session start.
  - **Real photos were dropped or sideways.** They are now decoded at most 4096 px on the long side, from their EXIF orientation.
  - **A failed fetch could delete the file the clipboard pointed at,** and retried every 4 s.

  The four phone fixes share one commit, since they interleave in the same two files.
- **Minors fixed:**
  - the Windows write allocates before emptying the clipboard;
  - copying text forgets the cached image;
  - image transfers leave a file share's progress alone.
- **A review suggestion not taken:** reading the sequence number while the clipboard is still open. Windows moves the number on close (91 inside, 93 after), so it is read just after, and the race of another app copying in that instant is accepted.
- **Deferred minors:**
  - Linux reads the whole PNG on every poll, with no cache and no bound before the size check;
  - text wins over an image that comes with text, so some browser and Office copies send their text;
  - no `CF_DIBV5` fallback for 8- and 16-bit bitmaps;
  - no end-to-end test of the two packets against the permission gate.
- **Already there, seen on the device:** at session start the phone pushes its own clipboard text, which replaces whatever the desktop held.
