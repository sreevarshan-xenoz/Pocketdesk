# L7: both sides' switches in one place

Plan: `docs/research/2026-10-20-l-cycle.md`, L7 (part 1).

## Why

A feature that does nothing is often off on the other side, and the owner has to work out which side:

- the computer's permission for that phone;
- the phone's own switch;
- Android's permission for the app.

KDE's forum asked for one place that says so (2026-10-17, 2026-07-20). Pocketdesk splits each feature's switch across two apps and Android, and the computer could not see the phone's.

## What

**The phone** tells its own computer which of its features are on, and whether Android lets each work. The packet is `kdeconnect.pocketdesk.switches {switches: {<feature>: {on, android}}}`:

- `on` is the phone's own switch, a boolean.
- `android` is a boolean, or null where Android needs no permission for that feature.

The features are a fixed list of names, and nothing else goes: no package lists, folder names or app names. Each feature maps to the computer's permission that governs it:

| Feature | The phone's switch | Android | The computer's permission |
|---|---|---|---|
| `notifications` | This phone's notifications on my computer | notification access | Notifications (and Settings "Phone notifications") |
| `answers` | Let the computer answer notifications (with notifications on) | notification access | Notifications (and Settings "Phone notifications") |
| `texts` | Show texts on my computer | the SMS permission | Texts |
| `clipboard` | Computer's clipboard in the background | notification access | Clipboard (and Settings "Clipboard to phones") |
| `files` | Files from the computer in the background | notification access and its notifications | Files |
| `folders` | Folders the computer may open (one shared) | notification access | Files |
| `photos` | Recent photos on the computer | notification access and photo access | Files |
| `screenshots` | Send new screenshots | notification access and photo access | Files |
| `camera` | Photos the computer asks for | notification access and its notifications | Files |
| `links` | Links from the computer | its notifications | Links |
| `ring` | Ring from the computer | notification access and its notifications | Ring |
| `media` | My media on the computer | notification access | Media |
| `mute` | Let the computer mute this phone | notification access | Mute |
| `control` | Let the computer control this phone | the Accessibility service | Cast (and Settings "Control phones from their screen window") |
| `dial` | Calls from the computer | its notifications | Dial |
| `commands` | Commands from the computer to my automation apps | notification access | Command |

Outside a session these run on the notification listener, which Android binds only with notification access; those that arrive as a notification need the app's own notifications too.

**When it is sent:** with the battery report's ticks, under the same switch ("Share the phone's state with the computer"): at once on a change, and every 5 minutes otherwise (`SwitchReport`, JVM-tested). It goes only to its own computer, on a link that is already up.

**The computer** keeps the last report per paired phone in `phone_switches.json` (owner-only, as `phone_status.json` is; `core::local::phone_switches`), and drops a phone that is no longer paired within seconds (with the batteries' writer). Each phone in the Devices page gets "Features on both sides", collapsed by default. It shows one row per feature with three columns: "This computer allows", "The phone sends" and "Android allows". A row whose feature cannot work names the side to change (`model::switch_rows`, tested), for example "Turn on Notifications here", "Turn on "This phone's notifications" on the phone" or "Give Pocketdesk notification access on the phone". Before any report, the page says the phone has not said yet, and that it needs this version and "Share the phone's state" on. `pocketdesk devices --json` carries `switches`.

## Not now (part 2)

- The phone's mirror: what the computer allows it, in each computer's menu.
- Presets at pairing ("Whose phone is it?").

## Privacy

Only booleans, over a fixed list of names, to the phone's own computer. They tell the computer's owner which of the phone's features are on, which is what they set on the phone themselves. This goes into SECURITY.md under protocol.md's accepted limit 2, and the switches row and `phone_switches.json` go into protocol.md.

## Tests

- **Core:** reading and writing the file; a body with unknown names or non-boolean values keeps only the known booleans.
- **Server:** the plugin keeps a paired phone's report and refuses an unpaired one.
- **Desktop:** `switch_rows`' advice for each blocked side; the harness opens the Devices page with a report.
- **Phone:** `SwitchReport`'s body and cadence.

## On the device (checklist §163)

- A switch turned off on the phone shows on the computer within 15 s in a session.
- Notification access revoked shows "Android allows: no", with its advice.

## As built

- **Core:** `core::local::phone_switches` (`SWITCHES`, `FEATURES`, `from_body`, the file), its list held to the phone's by a test that reads `SwitchReport.kt`.
- **Server:** `plugins::phone_switches` (a paired phone's report, written off the runtime, unchanged ones skipped for a minute; `prune` from the battery writer's loop), `devices --json` (`switches`, `switches_unix`).
- **Desktop:** `model::FEATURES`, `settings_off`, `switch_rows`; the Devices page's "Features on both sides" per phone (closed until opened, a block per feature).
- **Phone:** `status/SwitchReport.kt` (JVM-tested), `status/Switches.kt`, sent from `BatteryEffect` and `BackgroundBattery`.

## Review

A fresh review (0 Critical, 3 Important, 11 Minor), fixed one commit each where they were separate:

- **Important:** features the notification listener runs read as needing nothing from Android; Settings that turn a feature off for every phone were not counted; a phone as it comes (its features off by the owner's choice) read as a column of faults.
- **Minor, fixed:** answers on with notifications off; the phone's switches named unlike their labels; "here" twice; a doc comment moved; an unpaired phone's report kept, and the cap evicting before the paired were kept; the Kotlin list untested against Rust's; a nested key in `devices --json`; a read that could throw in a session.
- **Left:** text replies are not a row of their own (the texts row is what arrives); screenshots go as a clipboard image or a file, and the row names Files only; the server's refusal of an unpaired sender is covered by the shared `is_paired_device` check, not a test of this plugin.

