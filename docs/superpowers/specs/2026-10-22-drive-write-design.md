# N6: files into the phone's folders from the computer's drive

Plan: `docs/research/2026-10-21-n-cycle.md`, N6.

## Why

- **The phone's drive (D8) is read-only.** A file copied onto it in Explorer is refused (405), so putting a file in a folder on the phone means the Files page, which lands it in Downloads/Pocketdesk (or C2's one picked folder), and moving it there by hand.
- **Windows 11's File Explorer copies both ways** with an Android phone through Link to Windows; KDE Connect's SFTP mount writes too.

## What

### The phone

- **A mark per shared folder:** Settings "Folders the computer may open" gets, under each folder, "The computer may add files", off by default.
  - Turning it on opens Android's folder picker at that folder, since the grant kept when it was shared is read-only. The folder picked must be the same one; its grant is then kept for writing too.
  - Turning it off gives the write grant back.
  - A folder whose write grant is gone (taken back in Android's settings) is unmarked.
- **What the computer is told:** a marked folder's root says `add: true` in every listing.
- **Two new requests from the own computer, on T10g's road**, each held to `AddRule` (JVM-tested): from the own computer, a marked folder, a path and name within T10g's bounds, no name starting with a dot.
  - `phone_files.mkdir {root, path}`: makes that folder (the folders above it must exist) and answers with its listing, or why not ("exists", "not allowed").
  - `phone_files.put {root, path, name, id, size}`: fetches the computer's offer `id` (T6a's transfer, no resume) into that folder, as C2 writes into a picked folder: a `.part` renamed once whole, and the next free name when one is taken. One it may not take is declined with why; a file larger than the phone's free space is declined.
- **Never:** a delete, a rename of what is there, or a write over a file.

### The computer

- **The setting:** desktop Settings "The phone's folders take files" (`phone_drive_write`, on). Off, the drive stays read-only, as D8 made it.
- **WebDAV methods, under a folder of a marked root only** (anything else is 403):
  - `PUT`: the body (its `Content-Length` required, at most 4 GiB) is taken into `<state>/drive_uploads/` (owner-only), offered to the phone as `id`, and `phone_files.put` asks the phone to fetch it. The answer waits for the offer's end, at most 45 s: 201 once saved; 403 or 507 with the phone's reason; 202 when it is still under way (it carries on, and a failure is said in a desktop notification). The upload is removed when its offer ends, or expires (10 minutes).
  - A **zero-byte `PUT`** is held here two minutes, shown in its folder's listing, and replaced by a `PUT` with the content to the same place: Windows' WebDAV client makes a file empty first and writes it on close. One that is never written goes to the phone empty.
  - `MKCOL`: `phone_files.mkdir`, waiting for its listing (15 s): 201, or 405 when it exists.
  - `LOCK` and `UNLOCK` are answered with a lock that holds nothing, and `PROPPATCH` with every property taken, so Windows treats the drive as writable (it writes only to WebDAV class 2). `OPTIONS` says `DAV: 1, 2` only with writing on.
  - `DELETE`, `MOVE` and `COPY` stay 405.
- **After a write,** that folder's kept listing is forgotten, so the next look shows the new file.
- **Uploads** show under the Files page's "Under way" and in the sent list, as any send to the phone.

## Not in it

- Writing over, deleting or renaming on the phone.
- Resuming a broken upload (T6a's resume needs a source that stays; an upload is a cached copy).
- Office's save (a temporary file renamed over the original) and anything else that needs `MOVE`.

## Safety

- **SECURITY.md limit 50:** with a folder marked on the phone (off by default) and the computer's setting on, any program of the owner's on the computer that reads the drive's token can add files and folders in that folder; nothing on the phone is overwritten, removed or renamed, and only marked folders take anything.
- **What reaches the phone:** files the owner copies, as any send to it; names held to T10g's rule (no separators, no controls, no leading dot).

## Tests

- **Core:** the root's `add`, the requests' bodies.
- **Server, over the test phones:** PUT and MKCOL refused outside a marked root, under a hidden name, or with writing off; a PUT taken, offered and answered 201, 403 and 202 as the phone says; a zero-byte PUT shown and replaced; the listing forgotten after a write; LOCK, UNLOCK and PROPPATCH; DELETE still 405.
- **Bridge:** the new requests kept within bounds.
- **Phone:** `AddRule` on the JVM.
- **On the OPPO, by the runner (`n6-put`),** with a folder shared and marked by hand once (the runner never changes the phone's settings, and Android's picker is the only way to a grant): curl's MKCOL and PUT through the drive, the file read back by the drive's GET with its SHA-256 matching, and a PUT into an unmarked folder refused.
