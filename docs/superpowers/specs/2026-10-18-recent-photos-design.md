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
- Thumbnails are decoded with the `image` crate (JPEG only), on a thread, at most 256 px a side.

**The wire, bounded at both ends** (`core::protocol::phone_photos`): at most 24 photos, names checked as T10g's, thumbnails of at most 48 KiB, sizes and times as numbers. The server drops an answer that breaks a bound, and keeps a paired phone's only.

## Testing

- **Rust:** the protocol's bounds; the store (keep, a newer answer replacing an older, unpaired phones dropped, thumbnails written owner-only); the request packets.
- **Kotlin (JVM):** which MediaStore rows are listed (`PhotoRule`: DCIM only, newest first, at most 24), and that a fetch for an id not in the last list is refused.
- **Desktop:** the harness shows the grid from a fixture answer.
- **On the A71:** `adb push` of camera-sized JPEGs into `DCIM/Camera` (with a media scan), then Refresh, Save (checksum matching), Open and Copy through the desktop app's request files and the server, against a throwaway server.

## Out of scope

- Videos.
- Albums other than DCIM.
- Photos taken while the computer is away being pushed (the computer asks).
- A wider permission than the computer's Files switch.
