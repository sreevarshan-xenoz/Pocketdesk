# The phone's battery on the computer (T8b)

Date: 2026-10-15. Status: built (see As built), on autopilot (the user asked for the roadmap to keep going; the decisions are mine).

## Why

T8 in the roadmap: "phone status on the desktop". The phone can already read the computer's battery (the session HUD), but the computer knows nothing of the phone's. KDE Connect shows it and warns when it runs low, which is the point: a phone driving a computer from across the room is the phone that dies mid-session.

## What

1. **The phone reports its battery** during a session: the charge (0–100) and whether it is charging, at the start and whenever either changes (a whole percent, or the plug), as KDE Connect's `kdeconnect.battery {currentCharge, isCharging, thresholdEvent}`. `thresholdEvent` is 1 when the phone itself says the battery is low (Android's `ACTION_BATTERY_LOW`), else 0. Only during a session, because that is when the phone keeps a link to the computer; between sessions the computer shows the last report and its age.
2. **The computer shows it.** The desktop app's Devices tab shows each phone's last charge, "charging", and how old the report is ("54 %, 3 min ago"); `pocketdesk devices --json` carries the same as `battery: {charge, charging, updated_unix}`.
3. **The computer warns when a phone runs low:** one notification, "<phone> battery is low (14 %)", when a phone not charging reports 15 % or less (or says so itself). It warns again only after the phone has been back above 20 %, and never more than once in 10 minutes for one phone.

## Settings

- Phone: **"Share battery with the computer"**, on by default.
- Desktop: **"Low battery warnings from phones"** (`phone_battery_alerts`), on by default.

## Safety and limits

- The battery is status, not an action: no permission gates it, as the phone's own battery query is always allowed. The report names nothing but a number and a flag.
- A report with a charge outside 0–100 or no flag is ignored.
- The server keeps reports in memory and writes `<state>/phone_status.json` at most every 5 s, so a phone flooding reports costs one small write per 5 s. The warning's 10-minute floor and its 20 % re-arm keep a lying phone from flooding notifications.

## Wire

- Phone → computer: `kdeconnect.battery {currentCharge, isCharging, thresholdEvent}`, KDE Connect's own packet. The computer already sends the same packet type to answer the phone's `kdeconnect.battery.request`; a `kdeconnect.battery` arriving at the computer is always a phone's report.
- The bridge's list of packets the app may send gains `kdeconnect.battery`.

## Tests

- core: parsing a report (bounds, missing fields); `phone_status.json` written and read back, and age.
- server: the low-battery warning (threshold, the phone's own flag, charging, the 20 % re-arm, the 10-minute floor); a report recorded; the setting.
- desktop: the Devices tab's text for a fresh, old and missing report; the setting saves.
- Kotlin: when the phone sends (start, a whole percent, the plug, nothing otherwise), JVM-tested.
- Device: the A71's battery on the Windows Devices tab and in `pocketdesk devices --json`.

## As built

- **Where it lives.** The phone's `BatteryRule` (JVM-tested) and `BatteryEffect`; the bridge's allow-list of packets the app may send; the server's `PhoneBatteryPlugin` (`server/src/plugins/phone_battery.rs`) beside the computer's own `BatteryPlugin`, which answers `kdeconnect.battery.request`; `core::local::phone_status`; the desktop's `battery_text`.
- **Changed from the design.**
  - The warning names the phone by the first 8 characters of its id ("Phone 1a2b3c4d: battery low (12 %)"): the computer keeps no names for phones.
  - The phone reads Android's battery every 15 s instead of keeping a receiver, and sends again every 5 minutes so the report's age stays fresh.
  - The low flag is the battery broadcast's `EXTRA_BATTERY_LOW` (Android 9 and newer; older phones never send it), not `ACTION_BATTERY_LOW`.
- **Fresh review (0 Critical, 3 Important, 7 Minor), fixed one commit each:**
  - the phone opened a throwaway connection every 15 s when the session's control link was down: it now sends only on that link, while the app is started;
  - reports were never pruned (a removed phone's stayed on disk, and 64 ids filled every slot): phones no longer paired are forgotten each tick, and the oldest report makes room;
  - with `pairing_required = false` anyone could write reports and raise warnings under made-up ids: only a paired phone's report is taken, with a test that dispatches real packets.
  - Minors fixed: two doc comments, and the phone's summary promising a warning the desktop may have turned off.
  - Ruled: the warning's state is in memory, so a restart may warn again at once; a phone's own low flag does not re-arm the warning; "charging" means plugged in, as KDE Connect reports it.
