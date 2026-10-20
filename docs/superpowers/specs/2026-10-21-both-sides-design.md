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
| `notifications` | This phone's notifications | notification access | Notifications |
| `answers` | Let the computer answer notifications | notification access | Notifications |
| `texts` | Texts on the computer | SMS permissions | Texts |
| `clipboard` | Computer's clipboard in the background | — | Clipboard |
| `files` | Files from the computer in the background | — | Files |
| `folders` | Folders the computer may open (any shared) | — | Files |
| `photos` | Recent photos on the computer | photo access | Files |
| `screenshots` | New screenshots to the computer | photo access | Files |
| `links` | Links from the computer | — | Links |
| `ring` | Ring from the computer | its notifications | Ring |
| `media` | My media on the computer | notification access | Media |
| `mute` | Let the computer mute this phone | Do Not Disturb access | Mute |
| `control` | Let the computer control this phone | the Accessibility service | Cast |
| `camera` | Photos the computer asks for | the camera | Files |
| `dial` | Calls from the computer | — | Dial |
| `commands` | Commands from the computer to my automation apps | — | Command |

**When it is sent:** with the battery report's ticks, under the same switch ("Share the phone's state with the computer"): at once on a change, and every 5 minutes otherwise (`SwitchReport`, JVM-tested). It goes only to its own computer, on a link that is already up.

**The computer** keeps the last report per paired phone in `phone_switches.json` (owner-only, as `phone_status.json` is; `core::local::phone_switches`), and drops a phone that is no longer paired. Each phone in the Devices page gets "Features on both sides", collapsed by default. It shows one row per feature with three columns: "This computer allows", "The phone sends" and "Android allows". A row whose feature cannot work names the side to change (`model::switch_rows`, tested), for example "Turn on Notifications here", "Turn on "This phone's notifications" on the phone" or "Give Pocketdesk notification access on the phone". Before any report, the page says the phone has not said yet, and that it needs this version and "Share the phone's state" on. `pocketdesk devices --json` carries `switches`.

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
