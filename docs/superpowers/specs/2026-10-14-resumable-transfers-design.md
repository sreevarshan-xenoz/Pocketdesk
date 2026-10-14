# T6: resumable transfers, then folders

Roadmap T6 ("Resumable transfers and send folder"). Research: how LocalSend, KDE Connect, Warpinator, Quick Share, croc, Syncthing, rsync and tus handle resume and folders (summarised below). Code map: every transfer path today (`server/src/plugins/share.rs`, `offers.rs`, `clipboard.rs`, `android/bridge/src/api.rs`, `download.rs`, `TransferService.kt`).

Split in two, each its own branch and review:
- **T6a, resume.** A file transfer that breaks continues where it stopped, and every file is checked end to end.
- **T6b, folders.** A folder goes either way, one resumable file at a time.

## What is there today

- The phone always listens and the desktop always dials, inside pinned TLS 1.3 (`pocketdesk-transfer/1`). The desktop sends `key\n` first.
- Desktop to phone (offers, fetch by path): `key\n`, u64 size, bytes, then the phone's status byte.
- Phone to desktop (shares): `key\n`, then the bytes; the size went in the control packet. Then the desktop's status byte.
- Any break fails the whole file, and both ends delete what they had. A failed offer is used up, so it cannot be fetched again.
- Nothing checks the bytes: no checksum anywhere.
- `share.request` refusals are silent: the phone waits out 30 s.
- The phone's send loop has no deadline of its own.
- No folders: the desktop app refuses a dropped folder, the phone can only share files, and fetching a folder from the remote browser says "not a file".

## What others do

- LocalSend, KDE Connect, Warpinator and Quick Share do not resume. LocalSend checks an optional SHA-256; its missing path checks on received names became CVE-2025-27142.
- croc resumes: the receiver pre-allocates the file and reports missing ranges; a whole-file hash decides at the end.
- tus: the receiver is the authority on the offset; the sender must match it.
- rsync `--append-verify` appends, then checks the whole file and resends on a mismatch.
- Syncthing keeps partial temp files for a day.
- None proves that a partial file came from the same source before appending to it.
- Folders are per-file relative paths everywhere (croc, Warpinator, Quick Share), never an on-the-fly zip.

## T6a design: resume

### Who decides

The receiver keeps the partial file and says how much it has. The sender proves it is sending the same file by matching a hash of that prefix. Every file ends with a SHA-256 of the whole content, which the receiver checks before it keeps the file.

### Wire (ALPN `pocketdesk-transfer/2`)

The ALPN changes, so an old phone and a new desktop fail the handshake cleanly instead of misreading each other. Both ends ship together, as with every wire change here.

Inside TLS, after the desktop's `key\n`:

1. **The sender's header.** `size: u64` (both directions now; for a share it must equal the control packet's).
2. **The receiver's resume point.** `offset: u64` and `prefix: [u8; 32]`, the SHA-256 of its first `offset` bytes. `offset = 0` (prefix all zero) means "from the start".
3. **The sender's answer.** One byte: `1` continue from `offset`, `0` start over (the receiver then truncates to 0). The sender continues only if `offset <= size` and its own first `offset` bytes hash to `prefix`.
4. **The bytes,** from the agreed offset to `size`.
5. **The trailer.** `sha256: [u8; 32]` of the whole file, which the sender computes while it reads (over the prefix it hashed, then the rest).
6. **The status byte,** as now: `1` saved only if the receiver's own whole-file hash equals the trailer; `0` otherwise, and the partial is dropped.

The "sender" and "receiver" roles are the data's, not the dial's: when the phone shares, the desktop dials but receives.

### Which partial a retry continues

The control packet that starts a transfer carries a **source id**: 32 hex characters naming the file at its source, the same on every retry of that file.
- Phone sending: SHA-256 of the content URI, the size and the last-modified time if the provider gives one, cut to 16 bytes.
- Desktop sending: SHA-256 of the canonical path, the size and the modified time.

The receiver keeps at most one partial per `(device id, source id)`. The source id only chooses the candidate; the prefix hash is what proves it. A partial from another device is never continued: the key in the record is the device id the transfer's TLS key is bound to.

### Where partials live, and for how long

- **Desktop:** in `<state>/partials/` (owner-only on Unix, inside the user's folder on Windows), not in Downloads, so a half file never sits where the person looks: `<id>.part` plus `<id>.json` `{device_id, source_id, name, size, offset, created_unix}`. `offset` is written only after the bytes are flushed and synced. Kept 24 hours, at most 4 per phone and 16 GiB in all; the oldest goes first. Swept at start and every hour. On success the file moves into Downloads under its unique name, as now (Mark of the Web on Windows).
- **Phone:** the MediaStore row stays pending (`IS_PENDING = 1`, hidden from other apps and from Downloads) between attempts, and a retry opens it `"rw"` and writes from the offset; on Android 8–9 the legacy file keeps a `.part` name until it is complete. A record in the app's files (`files/partials/<id>.json`) keeps `{computer, source_id, uri, name, size, offset, created}`. Kept 24 hours, at most 4 in all; the sweep deletes the row (Android itself would expire a pending row only after a week). No second copy of the file is written.

### Retries

- **Phone to desktop (shares):** `TransferService` retries a failed file up to 3 times, 5, 15 and 45 s apart, while the computer answers. A share that still fails says so in its notification with a "Try again" action, which resumes too.
- **Desktop to phone (offers):** a failed fetch no longer uses the offer up. The offer stays for its 10 minutes and its phone may fetch it again, at most 4 times; the phone retries as above. The Files tab says "retrying" while it does.
- **Remote browser downloads:** the phone retries as above.
- **Clipboard images** stay as they are (small, and a newer copy replaces them): no resume, but the trailer is checked.

### Fixes that come with it

- A refused share gets `file_fetch_refused {key, reason}`, as fetches do, so the phone stops waiting and shows the reason.
- The phone's send loop gets the same deadlines as the desktop's (30 s idle, at least 32 KiB/s).
- Progress counts the resumed offset as already sent.

### Settings

- Desktop app, Settings: "Keep unfinished transfers for" (off, 1 hour, 24 hours; default 24 hours). Off deletes partials at once, as today.
- Phone, Settings: "Retry failed transfers" (on by default) and the same "Keep unfinished transfers" choice.

### Tests

- Core: the resume handshake as a pure state machine (continue, start over on a prefix mismatch, start over on a size change, offset past the size, a wrong trailer) over a duplex pipe.
- Server: a share broken at 40 % resumes and ends with the same bytes; a partial from another device is not continued; the sweep keeps the limits; a bad trailer leaves no file.
- Bridge: the same for a download, and the source id is stable for the same file.
- Kotlin: the retry plan (delays, the limit, "Try again").
- Device: a 200 MB share with Wi-Fi turned off at half, back on: it finishes from where it stopped and the MD5s match.

## T6b design: folders (after T6a)

- **Desktop to phone:** the app's Files tab gets "Choose folder…" and accepts a dropped folder. The server sends one `file_offer` per file with `folder` (the top folder's name) and `path` (the components under it, an array, never a joined string). The phone saves them under `Download/Pocketdesk/<folder>/…`, a new top folder each time (`Name (1)`), never merged into an existing one.
- **Phone to desktop:** "Send folder" in the session's Files sheet opens `ACTION_OPEN_DOCUMENT_TREE`. The tree is walked with `DocumentsContract` child queries, skipping hidden files and links. Each file goes as a resumable share carrying `folder` and `path`; the desktop puts them under a new `Downloads/<folder>` (`Name (1)`).
- **The remote browser:** "Download folder" on a folder row fetches its files the same way.
- **Path checks on the receiver, per component:**
  - not empty, `.` or `..`;
  - no `/`, `\`, `:`, NUL, characters 1–31, or `<>"|?*`;
  - not ending in a space or a period;
  - not a Windows device name, with or without an extension (including the superscript forms);
  - at most 255 bytes;
  - at most 32 deep;
  - names that differ only by case or Unicode normalisation are refused as duplicates.

  The path is built under the new top folder, and its canonical form must stay inside it.
- **Limits:** 10 000 files and 32 GiB per folder; the 8 GiB per-file cap stays.
- **Progress:** one notification for the folder (files done of total, bytes).

## As built: T6a (2026-10-15)

- **Core:** `protocol::transfer` (the exchange, `finish` for the status after the file is kept) and `protocol::partials` (the record store and its sweep). The ALPN is `pocketdesk-transfer/2`.
- **Desktop:** shares keep their parts in `<state>/partials/`, swept at start and hourly, under `transfer_keep_hours` (Settings "Unfinished files"), 4 per phone, 16 GiB in all. Offers carry `source` and allow 4 attempts within their 10 minutes. A retry of a file replaces the attempt still running for it (`share::running`), which a silent Wi-Fi drop otherwise leaves holding a slot for 30 s.
- **Phone:** sends and downloads both resume. A download's part is the pending MediaStore row (Android 10+) or the legacy file, opened read-write; the bridge records it in `files/pocketdesk/partials/`. TransferService retries 3 times (5, 15, 45 s); Settings "Retry failed files" turns retries, and kept parts, off. Parts are swept after 24 hours.
- **Differences from the design:**
  - the phone has one switch, not a separate "keep for" choice: with retries off nothing is kept;
  - the desktop app's sent list says "waiting for the phone" while an offer is retried, not "retrying";
  - progress moves in 4 MiB steps while receiving (it follows what is durable).
- **Found on the device run:** the stale attempt holding a slot (fixed as above), and shell-granted content URIs are not readable by the app (a test-harness limit, not a product one).
- **Device-verified (A71 ↔ Windows, LAN):** a 150 MB offer with Wi-Fi off 2 s in: the phone's retry continued from 16.6 MB and the MD5s match; a 120 MB share the same way, continued from 12.6 MB after the retry replaced the stale attempt, MD5s match.
