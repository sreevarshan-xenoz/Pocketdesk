# Names on both sides (U3)

Date: 2026-10-17. Plan: `docs/research/2026-10-17-finish-and-polish.md` §U3. From the UI audit: X1.

## The problem

- **The phone lists computers by address.** The computer's name comes in the pairing link (`name=`) and in the identity it sends on every control connection (`deviceName`), and both are thrown away: `HostStore.Host` is an address and two ports, and the core's handshake reads the identity line only to log it. Cards, the remote's title, the session notification, launcher shortcuts and the Lock question all say an address or "the computer".
- **The computer lists phones by a 12-character id.** The phone's identity carries `deviceName` on every connection; the trust store keeps ids, grants, permissions and which devices are computers, not names. Devices, the approval prompt, Sessions, Files, Messages, Notifications, the Phone page and the cast window all show `short(id)`. Other computers (2f) and KDE Connect devices (3a) already have names.
- **Nothing can be renamed.** Two phones of the same model, or a computer called `DESKTOP-4F2K9QH`, cannot be told apart.

## The design

1. **One cleaner for names, in core:** `clean_device_name(raw) -> Option<String>`: trimmed, control and invisible format characters dropped (the device id rules' list), runs of white space made one space, at most 40 characters; empty is `None`. Used by the server for a phone's name, by the bridge for a computer's, and by the desktop app for a rename.
2. **The trust store keeps names.** Per paired device, `names: {id: {claimed?, chosen?}}`: what the device calls itself, and the owner's rename. The name shown is the rename, else the claimed name. Like permissions, an entry goes with the device (dropped for an id no longer trusted at load, and when unpaired). An old store without names reads as before; a store with names stays readable by a server without them (an unknown field is ignored).
3. **The server notes a phone's name** from the identity on its control connection (both roads), once the device is paired: at the connection's start and right after it pairs on it. It writes only when the claimed name changed, under the trust store's lock. A rename is never replaced: the claimed name is kept beside it.
4. **The desktop app shows names.** `Device.name`, and one label for a device id: the name, else the short id. Devices shows the name with the id under it and a Rename field (empty clears the rename); the approval prompt says "<name> (calls itself) wants a session" with the id, since a decision depends on it; Sessions, Files, Messages, Notifications, the Phone page, Activity's rows and the cast window use the label.
5. **The local API:** `pocketdesk devices --json` gains `name` (null without one). The API stays version 1: a field is added.
6. **The phone keeps each computer's name.** The core's handshakes return the identity's name beside the stream (`ControlStream` and the internet road both), cleaned; the bridge keeps it per address (`nativePeerName`). `HostStore` keeps a claimed name and the owner's rename per address, removed with the computer. The claimed name comes from the pairing link at pairing, then from each connection.
7. **The phone shows names:** the card's title (the address under it), the remote's title, the session notification, launcher shortcuts, the tile and widget labels, and the Lock question ("Lock <name>?"). A card's Rename opens a small dialog; empty clears it.

## Security

A name is the device's own claim. The approval prompt shows "calls itself" and the id beside it; nothing is decided by a name. Names are cleaned like ids (no control or invisible characters, no bidirectional overrides), and kept short so a long one cannot push the id or the buttons off a prompt.

## Tests

- core: the cleaner (control, invisible and bidi characters, white space, length, empty); the trust store's names round trip, an old store without names, a rename kept over a new claim, names gone with an unpaired device; the handshake returning the computer's name.
- server: a paired phone's claimed name noted from its identity, an unpaired one's not; `devices --json` with `name`.
- desktop: the label (name, else short id), the rename.
- phone (JVM): `HostStore`'s name choice (rename over claim over address) as a pure rule.

## Device check

The A71 against this box: a computer's name on its card after a connection, renamed and cleared; the phone's name in the desktop's Devices (read through UI Automation), renamed there; `devices --json`.

## As built

- **The cleaner:** `core::protocol::kdeconnect::clean_device_name` (40 characters at most, `MAX_NAME_CHARS`), and its Kotlin twin `ComputerName.clean` for names the phone takes in (JVM-tested on the same cases).
- **The trust store:** `names: {id: {claimed?, chosen?}}`, `TrustStore::{name, claimed_name, note_claimed_name, rename}`; written only when a claimed name changed, filtered at load like permissions, gone with `untrust_device`.
- **The server:** `pair::note_claimed_name`, called off the runtime at a paired connection's start and when it pairs on it (`service.rs`); `devices` prints "name (id)" and `--json` has `name`.
- **The desktop app:** `Device::{name, renamed}`, `model::device_label` (and `target_label` through it), `model::rename_device`; the Devices page shows the name over the id and a Name box with Rename in each phone's section; the approval and task prompts show the name with "(calls itself)" unless the owner gave it, and the id; the rest of the pages, the notices and the cast window use the label.
- **The phone hears the computer's name:** `connect_named`, `control_handshake_named` and `internet_control_handshake_named` return it with the stream; the bridge keeps it per dialed address (`peer_names`, `nativePeerName`) from every connection, one-off queries included (the pairing is one). The app saves it from the pairing link, when a session's stream comes up, when the remote is ready, and as Home opens.
- **The phone shows names:** `HostStore.{name, shownName, chosenName, claimedName, saveClaimedName, rename}` in `pocketdesk-host`; the card's title (the address and ports under it), Rename on the card, the remote's title, the session notification, launcher shortcuts and "Lock <name>?".
- **Found on the device:** the phone called itself "Pocketdesk Android Client" to every computer (a fixed string, recorded in `docs/protocol.md` as a choice not to send the model), so every phone would have shown the same name. It now sends Android's device name, or the name set in Settings ("This phone's name"; `PhoneName`), applied as Settings closes; a changed name closes the control link (`close_control_link`), since a name travels only in a handshake.
- **Checked on the A71** against a throwaway server named "Test Desk" (reached through `adb reverse`; checklist §90): the card showed "Test Desk" with "127.0.0.1 · stream 4716 · control 1716" under it after a connection, renamed to "Lab box" and back by clearing; the computer kept the phone's name at pairing ("Galaxy A71", Android's default for it), "Test Phone" after setting it, "Test Phone 2" after changing it mid-run, and the device name again once cleared; `devices --json` gave it; the desktop app's Devices page showed the name over the id.

