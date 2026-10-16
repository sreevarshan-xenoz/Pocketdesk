# Two computers: pairing, a link, the clipboard and files (2f–2h)

Date: 2026-10-17. Plan: `docs/research/2026-10-16-next-phase.md` §2f–2h (ecosystem phase 2, "any of my devices", gap 6). Status: built (see "As built" at the end).

## Goal

The owner's two computers act as one set: pair them once, and from then on their clipboards follow each other (when the owner turns that on) and files go from either to the other, at home or from anywhere. A phone stays what it is; nothing here changes how a phone pairs or what it may do.

## Roles

Pairing is started on one computer, **A**, with the other's pairing link; the other, **B**, shows its code as it does for a phone. After that:

- **A pinned B** (B's certificate fingerprint, from the link) and keeps a control **link** to B, as a phone's background link does: TLS on B's control port at home, else the internet road (iroh, `pocketdesk-control/1`).
- **B paired A** as a device, binding A's TLS key (pairing binds the key that paired, phase B), then A's iroh key (a new packet, below). B marks A as a **computer** and gives it narrow permissions.
- On the link, A is B's client. B pushes to A as it pushes to a phone (the clipboard, file offers); A asks B as a phone asks (shares). So one link carries both directions, and only A needs to reach B.

## Pairing (2f)

1. **The desktop app on A**: Settings → "Computers" → "Pair another computer": paste B's pairing link (the Pair tab's link, `pocketdesk://pair?...`, which carries B's addresses, ports, PIN, certificate fingerprint and iroh endpoint). The app writes `<state>/computers_out/<id>.json` `{link}`; it never dials itself (the server holds A's keys).
2. **A's server** (`server/src/peer_computers.rs`) takes the request (an inbox folder, R6c), parses the link (`core::local::pair_link::PairLink::from_uri`), refuses one without a fingerprint, pins B's fingerprint for each of B's addresses in A's own pin store (`CertManager::pin_peer`), and dials B's control port with A's certificate, the addresses in the link's order (`ConnectionManager`, TLS 1.3, the pin checked). Its identity is A's host id and name.
3. It sends `kdeconnect.pair {pin, computer: true}`. **B** checks the PIN exactly as for a phone (one code pairs one device, five wrong guesses discard it), binds A's TLS key, trusts A, and records it in `trusted_devices.json`'s new `computers` list with these permissions: Clipboard, Files and Links on; Input, Audio, Mic, Power, Exec, Notifications, Texts, Media, Cast and Tasks off; **ask first** on, so a screen session from A is held for B's owner. The owner can change each in B's Devices tab, as for a phone. `computer: true` without a valid PIN does nothing; it only chooses the profile of a pairing the PIN allowed.
4. Paired, A sends `kdeconnect.pocketdesk.wan_key {endpointId}`, A's server iroh id. B registers it as A's iroh key only from a paired device on the home road whose TLS key is the bound one (the same trust as a phone's `[0xFC,0x00]` stream marker, `docs/protocol.md` §2.3); anything else is ignored.
5. A records B in owner-only `<state>/computers.json`: id, name, addresses, ports, fingerprint, iroh endpoint, `paired_unix`, and the last outcome (`paired`, or `failed: <why>`), which the app shows. B's Activity tab gets `phone_paired` for A, with `kind: "computer"` in its metadata.

Unpairing: from A ("Forget"), A drops the pin and the entry and sends `kdeconnect.pair {pair: false}`; from B, the Devices tab's Remove, as for a phone (A's link is then refused and A shows it).

## The link (2f)

Pairing runs over TLS on B's control port (nothing pairs on the internet road). After that, `peer_computers` keeps one link per paired computer, and it **always rides iroh**: A dials B's iroh endpoint (from the link, kept current by B's endpoint packet on the link) with `pocketdesk-control/1`. On a LAN iroh finds the direct path, so nothing is lost at home, and one kind of link carries everything: the control packets, the clipboard, and file streams B opens (2c's route). B admits it as it admits a phone on that road: a paired device whose registered iroh key is the one the connection proved.

- A dials **from its own WAN endpoint** (the one the server already runs, `iroh_endpoint.rs`). Not a new endpoint: it would hold the same key, and two endpoints with one key cannot both use a relay (2b's red test).
- Backoff 2 s to 60 s; a refused link (unpaired on B) is not retried until the owner pairs again.
- **What A takes from B on the link:** only `kdeconnect.clipboard` (2g) and `file_offer` (2h), and the streams B opens for files. B's other pushes (the HUD, the endpoint, wake info, control) are for phones; A keeps B's endpoint from the endpoint packet and ignores the rest.

## The clipboard (2g)

With "Share the clipboard with my other computers" on (`computer_clipboard`, **off** by default: content crosses machines), on both:

- A's clipboard changes go to B as `kdeconnect.clipboard` on the link (A's watcher, `clipboard_push`'s `Watch`), and B's go to A by B's existing push (A is a client with Clipboard).
- Each applies what arrives, and an echo guard (the hash of the text last applied from the peer) keeps it from going back.
- Text only, as the push is; private copies (password managers) stay where they are.

## Files (2h)

- `pocketdesk send <computer> <path>` and the desktop app's Files tab, with paired computers among the targets.
- **A → B:** A sends a share request on the link without a port; B answers on a stream it opens on the link (2c's route), into B's Downloads, with resume and the SHA-256 checked, under B's Files permission for A.
- **B → A:** B's Files tab offers to A as to a phone; A fetches without a port, the same way.
- Either way the link is iroh, so files cross at home (direct) and from anywhere (direct or relayed) alike.

## What it changes for phones

Nothing, beyond two packets a phone never sends (`computer: true`, `wan_key`), both announced in the identity's capabilities and `docs/packets.md`.

## Tests

- **Unit:** link parsing for a computer (no fingerprint refused); B's computer profile (permissions exactly as above); `wan_key` accepted only from a bound key on the home road; the echo guard.
- **Two processes** (`server/tests/two_computers.rs`, R6a's seams: each server with its own config and state folders and free ports, B's PIN read from its PIN file): A pairs with B; B lists A as a computer with the profile; A's `computers.json` pins B; B has A's iroh key; a wrong PIN or a wrong fingerprint pairs nothing; the link comes up and a clipboard change on A reaches B and not back (an in-memory clipboard); a 64 MB file each way, broken and continued.

## Device checks

Windows (A, the R6a throwaway instance, so the installed server is untouched) ↔ the Azure VM (B), with B's TCP 1716 open to the home IP only for the run: pairing that survives both restarts; the clipboard both ways in under 2 s; a 500 MB file, broken and continued, MD5 matching.

## Knobs

Desktop only, in Settings → "Computers": the list with Forget, "Pair another computer", and "Share the clipboard with my other computers". Per-computer permissions live in the Devices tab, marked as computers.

## Not in this design

- A computer driving another's screen (a stream session between computers): held by "ask first" and not offered in the app.
- The Explorer "Send to" menu for computers (T10e addresses phones).
- More than two computers works by pairing each pair; there is no mesh (phase 4).

## As built (2f)

- **Where the knobs are.** Settings → Computers: paste the other's link and Pair, the last pairing's outcome, each paired computer with its link's state and Forget (`desktop/src/computers_view.rs`). The Pair tab has "Copy the link for another computer" (the link holds the PIN, so it is copied, never shown).
- **Pairing** (`server/src/peer_computers.rs`): the pins the link's addresses had are put back when pairing fails, so a wrong link leaves nothing trusted. After the `wan_key`, A closes its side and reads B's to its end: B takes packets in order, so once it closes, A's key is registered and the link can be dialed at once.
- **Forget** drops B's entry and the pins A set for it (only where the pin is still B's). It does not unpair A on B: `kdeconnect.pair` without a PIN does nothing on a computer, so B keeps A until its own owner removes it in the Devices tab (the app says so). B refusing the link then shows on A as "refused".
- **A refused link** is tried again every 5 minutes rather than never, in case the refusal was a moment's race (a key not yet registered); one connection every 5 minutes to a computer that dropped this one.
- **No event on A** for a pairing yet; B's Activity shows A as `phone_paired`, as for any device (the `kind: "computer"` metadata is still to do).
- **Tests:** `peer_computers` unit tests (the pairing exchange on an in-memory stream, a refused code naming no key, the endpoint and fingerprint parsing, the refusal), `core::local::computers` (requests, the file), and `server/tests/two_computers.rs` (ignored: two real servers; a forged certificate and a wrong PIN pair nothing, the code pairs, B lists A as a computer with the profile and holds A's iroh key, the link comes up over iroh, Forget drops it).

## As built (2g)

- `computer_clipboard` (desktop Settings "Clipboard with my computers", off) starts the clipboard watcher even with "Clipboard to phones" off. The watcher pushes a change to the connections whose device is a computer only while it is on, and to phones only while `clipboard_push` is, each with the Clipboard permission; and to the computers this one links to (`peer_computers::linked`). A computer's `kdeconnect.clipboard` is applied only while it is on here, on either side.
- The echo guard is T10b's: the device a copy came from is noted, and the change it causes is not sent back to it.
- `POCKETDESK_CLIPBOARD_FILE` stands in for the clipboard (read and written as a file, and read each tick on Windows, which has no sequence for it), so `two_computers.rs` checks a copy each way between two servers on one machine, which would otherwise share its real clipboard.

## As built (2h)

- **A → B:** the Files tab (targets now include the linked computers, by name) and `pocketdesk send <computer> <file>` (by id, prefix or name) queue an outbox request as for a phone; for a paired computer the server sends it on the link as a share request without a port, under a key of its own, and B answers on a stream it opens on the link, as for a phone by the internet road. The sent list says `sent` once B saved it. A break is tried again 3 times (5, 15, 45 s), continuing (the request carries the file's source id); a refusal is final.
- **B → A:** B's Files tab offers to A as to a phone (A is B's client); A fetches each offer at once without a port, and B opens a stream on the link. A receives it as a phone's share is received: into Downloads (a folder send's files under their own new folder), resumable, announced, in the received list.
- **On A,** the streams B opens are read for their key line (5 s) and handed to the transfer waiting for that key (30 s); a stream for no transfer is dropped. B's refusal of a transfer (`file_fetch_refused`) ends its wait at once.
- **Not yet:** a folder from A to B (refused with a reason; B's folders to A work), and "broken and continued" between two computers is the phone's resumable code, not checked by a two-computer test.
- `POCKETDESK_DOWNLOAD_DIR` stands in for Downloads, so `two_computers.rs` sends a 3 MB file each way between two servers on one machine and compares the bytes.
