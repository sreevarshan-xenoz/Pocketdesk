# L9: the phone's state on the computer

Plan: `docs/research/2026-10-20-l-cycle.md`, L9.

## Why

The computer already knows a phone's battery (T8b, outside a session since item 11). Before you reach for the phone, you often want to know three more things:

- whether it has room for the file you are about to send;
- whether it is on Wi-Fi or mobile data;
- whether it will ring.

KDE Connect shows its connectivity report and Phone Link shows the signal. Pocketdesk showed none of these.

## What

**The phone** adds its state to the battery report it already sends. That is `kdeconnect.battery` (KDE's own type), with fields KDE does not have; a reader that does not know them ignores them:

| Field | What |
|---|---|
| `storageFreeMb`, `storageTotalMb` | The internal storage as the phone's Settings says it (`StorageStatsManager` for the default volume, `StatFs` if it will not answer), in whole decimal MB, the free space rounded down to 100 MB |
| `network` | `wifi`, `mobile`, `ethernet` or `none`: the default network's transport, never its name, SSID or carrier |
| `signal` | 0 to 4 bars, Wi-Fi from its RSSI or mobile from the modem's level; left out when not known |
| `ringer` | `normal`, `vibrate` or `silent` |

**When it is sent:**
- A change in the battery, the network or the ringer is sent at once, as a change in the battery is now.
- Storage and signal ride along. A change in them alone waits for the five-minute resend (`BatteryRule.RESEND_MS`), so a signal that wavers is not a packet every few seconds.

The phone's switch "Share battery with the computer" becomes "Share the phone's state with the computer". It is the same switch (`share_battery`), still on by default, and Settings says what it shares.

**The computer** keeps the new fields beside the battery in `phone_status.json`:
- `PhoneBattery` gains `storage_free_mb`, `storage_total_mb`, `network`, `signal` and `ringer`. Each is optional and left out when absent, so an older file or a KDE device's report reads as before.
- `pocketdesk devices --json` shows them in `battery`, as it shows the rest of it.
- The Overview's phone block shows one more line, "Wi-Fi, 3 of 4 bars · 12.3 GB free of 128 GB · on silent", made of what the phone said, in decimal units as the phone's Settings (`model::phone_state_text`, tested). A phone with less than 100 MB free reads "under 100 MB free".

## Not now

- A warning when the phone is nearly full.
- The KDE connectivity report (`kdeconnect.connectivity_report`) from a KDE device.
- A Home Assistant sensor for each field.

## Privacy

None of the fields names a network, a place or a carrier. They go only to the phone's own computer, on the link the battery already uses. protocol.md's battery row names the new fields, and its accepted limit 2 (a phone with every permission off still reports its battery) now names them too: they ride the same report.

## Tests

- **Core:** `BatteryReport::from_body` reads the fields, drops ones out of range, and reads a body without them as before. `phone_status` round-trips a battery with them and reads an old file.
- **Server:** the plugin keeps them.
- **Desktop:** `phone_state_text` for each case, including an empty state.
- **Phone:** `PhoneStateRule` (JVM) covers the bars from RSSI, the storage rounding, the body with and without the state, and what triggers a send.

## On the device (checklist §162)

- The Overview line matches the phone's Settings: storage, Wi-Fi, and the ringer after a change.
- Mobile data with the bars, on a phone with a SIM.
- The switch off sends nothing.

## As built

- **Core:** `core::local::phone_status`: `PhoneState` (flattened into `PhoneBattery`, each field optional and read leniently), `Network`, `Ringer`, `PhoneState::from_body`.
- **Server:** `phone_battery`'s `take` keeps `report.state`; `devices --json` carries it in `battery`.
- **Desktop:** `model::phone_state_text` (decimal units, "under 100 MB"), `model::state_is_now` (reached, and reported in the last 10 minutes), a line under the Overview's battery.
- **Phone:** `status/PhoneStateRule.kt` (JVM-tested), `status/PhoneState.kt` (`StorageStatsManager`, the network behind a VPN, the data SIM's bars), `BatteryRule.shouldSend` and `body`; each tick reads only what decides a send, off the main thread.

## Review

A fresh review (0 Critical, 2 Important, 9 Minor), fixed one commit each:

- **Important:** storage from the data partition in MiB, unlike the phone's Settings (now the volume's advertised size, decimal); a nearly full phone read "0 B free" (now "under 100 MB free").
- **Minor, fixed:** an unknown word emptied the whole file for its reader; an away phone's state shown as now; reads on the main thread and in full every background tick; a VPN's network named instead of the one it rides; the voice SIM's bars, and an unknown level said as none; `devices --json` untested with the fields; stale names of the switch; the spec's privacy claim.

