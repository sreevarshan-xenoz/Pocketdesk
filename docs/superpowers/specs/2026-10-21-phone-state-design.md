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

None of the fields names a network, a place or a carrier. They go only to the phone's own computer, on the link the battery already uses. SECURITY.md's list is unchanged: the battery's limit already covers what the phone tells its computer of itself. protocol.md's battery row names the new fields.

## Tests

- **Core:** `BatteryReport::from_body` reads the fields, drops ones out of range, and reads a body without them as before. `phone_status` round-trips a battery with them and reads an old file.
- **Server:** the plugin keeps them.
- **Desktop:** `phone_state_text` for each case, including an empty state.
- **Phone:** `PhoneStateRule` (JVM) covers the bars from RSSI, the storage rounding, the body with and without the state, and what triggers a send.

## On the device (checklist §162)

- The Overview line matches the phone's Settings: storage, Wi-Fi, and the ringer after a change.
- Mobile data with the bars, on a phone with a SIM.
- The switch off sends nothing.
