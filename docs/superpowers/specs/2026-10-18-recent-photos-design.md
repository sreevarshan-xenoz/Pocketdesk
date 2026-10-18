# Recent photos on the computer (C7)

**Plan:** `docs/research/2026-10-17-next-cycle.md`, C7.

## Why

Windows' phone panel in the Start menu leads with the phone's recent photos. Pocketdesk can already reach a phone's folders (T10g), but a photo just taken means opening a shared folder, finding DCIM, and fetching a file blind, with no picture of it first.

## What

**The phone (off by default):**
- Settings "Recent photos on the computer" asks for photo access when it is turned on (the permission screenshots to the computer already asks for).
- The computer asks with `kdeconnect.pocketdesk.phone_photos.list`, and the phone answers `kdeconnect.pocketdesk.phone_photos` with its newest camera photos:
  - only `DCIM/`, newest first, at most 24;
  - for each: its MediaStore id, name, when it was taken, its size in bytes, and a JPEG thumbnail of at most 256 px a side (`ContentResolver.loadThumbnail`), base64, at most 48 KiB.
- The computer asks for one with `kdeconnect.pocketdesk.phone_photos.fetch {id}`, and the phone sends it as its share sheet does, through the existing transfer (resumable, into the computer's received files). It sends only an id it listed, still in `DCIM/`.
- Answered only for the phone's own computer, from the notification listener's link (as T10g), with the switch on and the permission granted. Otherwise the answer says why.

**The computer:**
- The server keeps a paired phone's last answer: the list in `phone_photos.json` (owner-only), the thumbnails as files under `<state>/phone_photos/<device>/` (owner-only). It sends the desktop app's requests (`phone_photos_out/`) to a connected phone with its Files permission on, as T10g does.
- The desktop app's Phone page gets **Recent photos**, for the chosen phone:
  - Refresh, and the thumbnails in a grid with each photo's name and age;
  - **Save** fetches it into the received files;
  - **Open** fetches it if it is not here yet, then opens it;
  - **Copy** fetches it if it is not here yet, then puts the image on the clipboard (egui's `copy_image`).
- Thumbnails are decoded with the `image` crate (JPEG only): kept only when their header declares at most 512 px a side (the phone makes them 256), and decoded within that.

**The wire, bounded at both ends** (`core::protocol::phone_photos`): at most 24 photos, names checked as T10g's, thumbnails of at most 48 KiB, sizes and times as numbers. The server leaves out photos that break a bound, drops a body that is not an answer, and keeps a paired phone's only.

## Testing

- **Rust:** the protocol's bounds; the store (keep, a newer answer replacing an older, unpaired phones dropped, thumbnails written owner-only); the request packets.
- **Kotlin (JVM):** which MediaStore rows are listed (`RecentPhotoRule`: DCIM only, newest first, at most 24), and that a fetch for an id not in the last list is refused.
- **Desktop:** the harness shows the grid from a fixture answer.
- **On the A71:** `adb push` of camera-sized JPEGs into `DCIM/Camera` (with a media scan), then Refresh, Save (checksum matching), Open and Copy through the desktop app's request files and the server, against a throwaway server.

## Out of scope

- Videos.
- Albums other than DCIM.
- Photos taken while the computer is away being pushed (the computer asks).
- A wider permission than the computer's Files switch.

## As built

- **Packets** (`core::protocol::phone_photos`): thumbnails are checked as base64 text there (the phone's bridge builds core without base64), and decoded, with a JPEG's first bytes required, in the computer's store.
- **The phone** answers from the notification listener's folders tick, and keeps the control link while the switch is on (the computer's requests come on it). The last answer's ids are held in memory: a fetch the phone no longer remembers listing (after the app restarted) is answered with a fresh list.
- **The desktop:** Open is offered only for a picture's name (the name is the phone's; a renamed program is never opened). Open and Copy use a copy already received since the answer; otherwise they fetch it and act when it arrives (two minutes at most). Copy decodes off the UI thread and hands the image to egui's clipboard.

## The device run

On the A71 against a throwaway server, three camera-sized JPEGs pushed into `DCIM/Camera` and scanned:
- the list answered with the three, newest first, each with a thumbnail of about 2.5 KB;
- one fetched into the computer's received files, checksums matching.

**Owed (checklist §110):**
- the Phone page by hand, with Open and Copy (they act on the owner's screen and clipboard);
- the switch off, and no photo access, on screen;
- a phone with thousands of photos;
- HEIC photos (the grid shows their thumbnails; Copy cannot decode them and says so);
- Android before 10 (the legacy thumbnail path).

## Review

**Fresh review (0 Critical, 3 Important), fixed one commit each:**
- a few kilobytes of JPEG declaring 65535 px a side would have crashed the desktop at its first paint, every launch until the server restarted (now refused by its header, and decoded within a thumbnail's size);
- the request folder was not watched, so each action waited for the 5 s sweep;
- Copy put portrait photos on the clipboard sideways and held gigabytes for a big photo (now turned by its orientation, at most 4096 px, and offered only for JPEG, PNG and WebP).

**Minors fixed:**
- DCIM's Screenshots and hidden folders are not camera photos;
- an unlisted fetch gets a fresh list;
- the link only with photo access;
- a thread of its own;
- the legacy path anchored at the storage's root, and no full decode on unread bounds;
- a fetched photo matched by size too;
- no Open or Copy long after the click;
- a computer's photos not kept;
- invisible characters out of the phone's words;
- the spec's words.

**Left as is:**
- **An unpaired phone's thumbnails** stay in the state folder until the next answer from any phone, or the server's next start (SECURITY.md limit 26 says so).
- **The legacy thumbnail path (Android 8 and 9)** ignores the photo's orientation.

