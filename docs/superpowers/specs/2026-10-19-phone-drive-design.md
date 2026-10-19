# D8, the phone's folders as a drive (design)

Date: 2026-10-19. Plan: `docs/research/2026-10-18-next-cycle.md`, D8.

## Why

Windows 11 is adding the phone's storage to File Explorer through Link to Windows (https://winaero.com/windows-11-will-soon-let-you-view-and-manage-android-files-wirelessly). KDE Connect mounts the phone over SFTP. Pocketdesk's phone folders (T10g) could be browsed only on the desktop app's Phone page, one file fetched into Downloads at a time.

## What it does

- **The server serves the phone's shared folders as WebDAV**, read-only, on loopback only (`127.0.0.1`), with the setting "The phone's folders as a drive" on (`phone_drive`, off by default).
  - It listens on a port the system picks.
  - Every path starts with a token of 32 random hex digits.
  - `<state>/drive.json` (owner-only) holds the address. A path without the token is answered 404.
- **What it serves:**
  - `/<token>/`: one folder per paired phone with Files on, named as the app names it, with its short id when two share a name.
  - `/<token>/<phone>/`: the folders that phone shares (T10g's roots).
  - Below that, its folders and files, as the phone lists them.
- **Methods:**
  - `OPTIONS` says `DAV: 1`.
  - `PROPFIND` (depth 0 or 1; `infinity` is taken as 1) answers from a listing.
  - `GET` and `HEAD` give a file.
  - Everything else is 405: nothing on the phone is changed.
- **Listings:**
  - The server asks the phone for a folder's listing (T10g's `list`) and waits up to 15 s for it.
  - It keeps a listing 30 s, so a file manager's repeated looks (`desktop.ini`, thumbnails) ask the phone once.
  - A name not in its folder's listing is 404 without asking the phone.
  - The phone answers as for the Phone page.
- **A file:**
  - The server asks the phone to send it (T10g's `fetch`) with `drive: <id>`.
  - The phone's share carries the same `drive`. The server takes the share into `<state>/drive_cache/` (owner-only), not Downloads: no notification, no row in the received list, no event.
  - Once whole, it is served, kept a minute for a second read or a byte range (206), then removed. Two files are asked of the phones at once; another GET meanwhile is answered 503 with `Retry-After`. A share whose `drive` names no request waiting from that phone is refused, and a refused share, or a client that leaves, ends its request at once.
  - `HEAD` answers from the listing without fetching.
  - A fetch has 10 minutes to arrive. A phone from before D8 sends no `drive`, so its file lands in Downloads as a Phone-page fetch would, and the drive answers 504 when the wait runs out.
- **The desktop app:**
  - The Phone page shows the drive's address with Copy, and how to open it: in Explorer as `\\127.0.0.1@<port>\DavWWWRoot\<token>`, on Linux as `dav://127.0.0.1:<port>/<token>/`.
  - Settings holds the switch.
  - Mapping a drive letter is the owner's step.

## Limits

- **Read-only.**
- **Only this computer's own programs reach it,** and only with the token. Anyone who can read the state folder, or run programs as the owner, can read the phone's shared folders while it is on (SECURITY.md limit 30).
- **Windows' WebDAV client** refuses files over 50 MB by default (`FileSizeLimitInBytes`), gives up on a file that has not begun to arrive within about a minute (`SendReceiveTimeoutInSec`, and the drive sends nothing until the whole file is on the computer), and its WebClient service must be running.
- **A folder of more than 2000 files** shows its first 2000 (T10g's listing limit); the rest are not there.
- **A drive fetch is still a transfer:** it shows under the Files page's "Under way", and the phone says it sent the file, as for any share.

## Review

**Fresh review: 0 Critical, 5 Important, several Minor.** Fixed one commit each:
- a refused share, or a folder shared no more, left its request waiting 10 minutes;
- a client that left kept asking the phone and held its connection;
- a file whose size the provider cannot tell was sent without `drive`, into Downloads;
- Explorer's many reads (fetches without bound, nothing kept, no byte ranges);
- the limits on large folders and Windows' one-minute wait, now said above.

Minors fixed:
- a config that does not read keeps the drive's address;
- a failed listen is said once;
- a failed accept pauses;
- a head has 10 s in all;
- nothing below a hidden name is asked of the phone;
- Explorer's address names `127.0.0.1`.

Left as they are:
- the Phone page shows `drive.json` without asking whether the server still runs;
- a file that arrives after its request ended waits in the cache until the drive stops or the server starts again.

## Testing

- **Pure, tested:** the path's parts and the token, percent-decoding, a name the phone would refuse, the multistatus XML and its escaping, the dates, depth.
- **Server:** the WebDAV answers over a real loopback socket, against a stand-in for the phone (listings and files from memory); a share's `drive` routing (taken only when waiting for that phone).
- **Phone:** the request's `drive` reaches the share (JVM test of the request's parsing).
- **By hand (checklist §117):**
  - Explorer, and curl's `PROPFIND`, against the A71;
  - a file opened, and a big one;
  - a phone from before D8;
  - Linux's file managers.
