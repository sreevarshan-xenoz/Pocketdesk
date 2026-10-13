# File transfer, desktop to phone and the remote browser — design

Status: proposed 2026-10-13, branch `feat/file-transfer-b` (cut from `feat/file-transfer`, PR #16). Roadmap F7, second half ([feature roadmap](2026-10-12-feature-roadmap.md); first half: [F7a](2026-10-13-file-transfer-design.md)).

## Why

F7a moves files one way. The phone still cannot:
- see what is on the computer;
- take a file from it;
- receive a file the desktop's owner sends.

F7a also left a gap: "sent" on the phone means the desktop took every byte, not that it saved the file.

What exists today:
- **Browse.** The server answers `kdeconnect.filebrowse.request`, confined to the home folder: a lexical check first, then a canonical one. Its reply echoes the *requested* path, though, and a sub-folder request must be an absolute path under home. So the phone, which never learns that path, cannot go below the top folder. No phone UI calls it.
- **Download, desktop-initiated send.** There is no download handler, and the server never sends a file to a phone.
- **App-to-server IPC.** The desktop app reaches the server only through files in the state directory, such as `kick` and `approvals/<id>.json`.

## Decisions (made autonomously; each is cheap to revisit)

| # | Decision | Why |
|---|---|---|
| B1 | **The desktop always dials the phone.** A download is a new phone→desktop packet, `kdeconnect.linuxlink.file_fetch` `{path \| offer, port, key}`. The phone listens on a random port with a fresh 32-hex key, exactly as F7a does. The server dials `DeviceSender::peer_addr` only, writes `key\n`, then the file. | It is F7a's connection, reversed in payload only. Every safety property carries over: the server never dials an address the peer names, and a key authenticates the dial. Windows needs no new inbound firewall rule; the server's random port would need one. |
| B2 | **One fetch path for both features.** A fetch names either a browsed `path` (confined to home by the existing `sanitize_path`, and a regular file) or an `offer` id the desktop issued (B5). `capability_for` maps `file_fetch` to `Files`. | One handler, one set of limits. The browse confinement already went through a security review. |
| B3 | **Framing on the transfer socket.** Desktop→phone: `key\n`, then a `u64` big-endian size, then the bytes. Phone→desktop (F7a) is unchanged up to the bytes. **In both directions the receiver then writes one status byte**: `1` saved, `0` failed. The sender reports success only on `1`. | The size header makes the stream self-describing, so a file that changed since it was listed cannot confuse the receiver. The status byte closes F7a's "sent means received, not saved" gap. Both ends ship together (project rule), so there is no shim for an ack-less peer. |
| B4 | **Browse replies say where they are.** The `filebrowse.response` gains `path` (the resolved absolute folder, in the form the lexical check accepts), `parent` (`null` at home) and, on every entry, its full `path`. The phone uses these strings as is and never joins paths. | This fixes the navigation gap, and the phone needs no knowledge of `\` versus `/`. |
| B5 | **Desktop→phone is an offer the phone pulls.** The desktop app writes `outbox/<id>.json` `{id, device_id, path, created_unix}` (owner-only, like approvals). A server watcher takes it within 250 ms, checks the file (regular, at most 8 GiB) and the device (trusted, Files allowed, has a live control connection), keeps `{id → path, device, expires 10 min}`, and pushes `kdeconnect.linuxlink.file_offer` `{id, name, size}`. The phone fetches with `offer: id`. An offer is single-use and valid only from the device it was sent to (as far as the in-band device id can be trusted; see the F5 limit). Every send reaches an end in `sent.json`: an offer whose time runs out is swept as failed, and a phone that will not fetch one (no room, too large) declines it with `kdeconnect.linuxlink.file_offer_declined` `{id, reason}`. | It reuses B1/B2 entirely. The offer id is the only thing the phone can name, so the owner's choice of file is never widened. |
| B6 | **The phone accepts offers from a paired computer without a prompt.** It saves to Downloads/Pocketdesk (MediaStore, Android 10+; the app's own external Downloads folder on 8–9, which needs no permission), with the same progress notification as F7a and a final "Saved name" that opens the file. It refuses an offer larger than the free space or 8 GiB. | KDE Connect and Phone Link behave this way. The sender is a computer the user paired, acting on its owner's explicit action, and the notification is the record. |
| B7 | **Offers reach the phone only while it has a session open.** The bridge's control reader latches offers the way it latches notifications, and the session drains them every second into `TransferService`. A phone without a live control connection makes the desktop record "phone not connected". | Only during a session does the phone hold a control connection. A background connection is battery and lifecycle work beyond F7. |
| B8 | **Phone UI: a Files sheet in the session.** The Quick Settings sheet gains "Files on this computer". The sheet opens at home, lists folders first and then files with their sizes, has an Up row and the current folder as a title, and downloads a file when it is tapped (after a confirm that shows the size). Downloads queue in `TransferService` beside sends. | The session is when the control connection is up and the computer is in reach. A Home-screen browser can come later. |
| B9 | **Desktop UI: the Files tab sends.** It gains a "Send to phone" box with a phone picker, listing only devices with a live session, and three ways to name a file: drag and drop (Windows and X11; winit has no Wayland drop), "Choose file…" (the Win32 common dialog, Windows only), and a path field (everywhere). Outcomes go to `<state>/sent.json` (the last 50: name, size, device, time, `offered`/`sent`/`failed: reason`), shown under the received list. | It needs no new dependency: the `windows` crate already has the dialog, and egui reports dropped files. Linux gets a path field instead of GTK. |
| B10 | **Limits are shared.** Fetches and receives take the same two transfer slots (a full pool refuses at once), the same `30 s + size/32 KiB` deadline and 30 s idle timeout, and the same 8 GiB cap. | One budget for one link. |

## Components

| Unit | Change |
|---|---|
| `core::local::outbox` | Request files: `request(state_dir, device_id, path)` writes one with a fresh id; `take_all(state_dir)` consumes them, ignoring stale or invalid ones (TTL 60 s). |
| `core::local::sent` | `sent.json`: `record` and `update` by id, keep 50, read; a corrupt file reads as empty. Same shape as `received.rs`. |
| `core::protocol::permissions` | `file_fetch → Files`. |
| server `plugins/file_browse.rs` | B4 reply fields. A shared `resolve_file(requested) -> Result<PathBuf>` for B2 (sanitize, then require a regular file). |
| server `plugins/share.rs` | The status byte after commit or failure (B3). `send_file_to(peer, key, path)`: dial, key, size, bytes, read the status. The `file_fetch` handler resolves the path or the offer and spawns the send under a slot. |
| server `offers.rs` | Offer table (id → path, device id, expiry). The outbox watcher: validate, find the device's control sender (a new `pair::address_of(device_id)` from the identity map), push the offer, record `offered`, and later `sent`/`failed`. |
| bridge `api.rs` | `send_stream` waits for the status byte. `fetch_file(address, port, target, out_fd)`: listen, send `file_fetch`, accept a keyed dial, read the size, pump into the fd with the same progress counters, write the status byte after `fsync`. Offers are latched in the reader (`take_file_offers`). |
| Kotlin | `TransferService` takes receive jobs: MediaStore pending entry, then fd, then `fetchFile`, then publish, or delete on failure. `RemoteFilesSheet` (browser). `FileOffersEffect` (poll and enqueue). Notification text for receiving and saved. |
| desktop | Files tab: the send box, the drop handling, the Windows dialog, and the sent list. |

## Error handling

- **Fetch refused by the server** (outside home, not a regular file, too large, no slot, unknown or expired offer, Files off): the server does not dial. It pushes `kdeconnect.linuxlink.file_fetch_refused` `{key, reason}` on the connection the fetch came in on, and the bridge, matching the key, returns the reason at once. The listener's 30 s accept timeout is only the backstop for a lost refusal.
- **Phone receive failure** (no space, write error, short stream, deadline): status byte `0`, the pending MediaStore entry is deleted, and the notification names the file and the reason. The desktop records `failed: phone could not save it`.
- **Desktop receive failure** (F7a path): status byte `0` before the `.part` guard cleans up. The phone's notification says the computer could not save the file.

## Out of scope

- WAN (iroh) and `adb reverse` transfers. The desktop must reach the phone's address, as in F7a.
- Folders: neither downloading a folder nor sending one.
- Offers outside a session, and a prompt before accepting.
- Resuming a broken transfer.
- Browsing outside the home folder.

## Testing

| Layer | Tests |
|---|---|
| core | Outbox: a request round-trips; a stale or malformed request is dropped and deleted. `sent.json`: record, update by id, keep 50. `capability_for("kdeconnect.linuxlink.file_fetch") == Files`. |
| server | Browse reply: `path`/`parent`/entry `path`s, and `parent` is null at home. `resolve_file` refuses a folder, a path outside home and a link out of home. An offer: single use, refused from another device, refused after expiry. Over loopback, `send_file_to` delivers size + bytes and reads the status, and a receiver that answers `0` fails it. The receive side writes `1` after the rename. |
| bridge | Over loopback: `fetch_file` with a fake desktop gets the bytes, the progress runs from 0 to the total, and a wrong key, a short stream and a refusal by key each fail. `send_stream` fails on status `0` and on a missing status. |
| desktop | The sent list model; the phone picker lists only connected devices. |
| JVM | Notification text for receiving and saved; the browser's entry ordering (folders first, by name) and size formatting. |
| Device (A71 ↔ Windows, LAN) | Browse into two levels and back up; download a file with a matching MD5 into Downloads/Pocketdesk, then open it; send from the desktop by drag and drop and by the dialog, with a matching MD5 and a `sent` row; a refused fetch fails within a second with its reason (an offer used twice); phone→desktop still works and now waits for the status. |
