# D2, quiet hours kept in step (design)

Date: 2026-10-18. Plan: `docs/research/2026-10-18-next-cycle.md`, D2.

## Why

- **KDE bug 394712:** don't forward notifications while the phone is in Do Not Disturb. A 2025 comment asked again: https://bugs.kde.org/show_bug.cgi?id=394712
- **KDE bug 404043:** mute for a while.
- **A Plasma 6.5 thread** on Do Not Disturb conflicts: https://discuss.kde.org/t/fullscreen-do-not-disturb-ignores-per-app-show-in-dnd-and-audio-notifications-kde-connect/43359
- **Microsoft** is testing a Do Not Disturb switch in its phone flyout.

Today a phone in Do Not Disturb still sends every notification, and the computer shows each one.

## What it does

### On the phone

**Holding back.** "Hold back notifications in Do Not Disturb" (Settings, under This phone's notifications, on by default):
- While Android's interruption filter is anything but "all", a notification is not sent when Android itself would not let it through (`Ranking.matchesInterruptionFilter()` false).
- A notification Android lets through, from a priority contact or an allowed app, is sent as before.
- Held-back notifications are dropped, not queued: Android does not show them either, and its shade keeps them.
- Their removal is not sent, because the computer never had them.
- A pure `QuietRule` decides this, JVM-tested.

**Muting from the computer.** "Let the computer mute this phone" (Settings, off by default, like Ring). It keeps the background link, as ring does.
- The computer sends `kdeconnect.pocketdesk.quiet {minutes}`, from 1 to 1440, or 0 to end.
- The listener turns Do Not Disturb on with `requestInterruptionFilter(INTERRUPTION_FILTER_PRIORITY)`. Android then shows only what the owner's own priority settings allow.
- It keeps the end time, and its tick ends the mute when that time comes.
- It ends the mute only while the filter is still the one it set: if the owner changed it meanwhile, the owner's choice stands.
- A request with the switch off, or from a computer that is not this phone's own computer, is answered with a refusal and nothing else happens.
- A pure `QuietPlan` decides the end, JVM-tested.

**Reporting.** The phone tells its computer when its state changes, and when the link comes up:
- `kdeconnect.pocketdesk.quiet_state {quiet, until?, byComputer}`;
- `until` is in unix seconds and present only for the computer's own mute;
- changes come from `onInterruptionFilterChanged`.

### On the computer

- **A per-phone permission, "Mute",** on for phones and off for computers. It gates `quiet` going out, and `quiet_state` coming in.
- **Requests:** `<state>/quiet_out/<id>.json` (`core::local::quiet`, like rings: a TTL of 60 s, taken by the server and sent over the live link).
- **State:** the phone's last report is kept in `phone_status.json` (`quiet`, `quiet_until`), shown in the Overview's phone block and in Devices: "Do Not Disturb", or "Muted until 15:30".
- **Buttons:** "Mute for an hour" and "Unmute", beside Ring, each saying why when the phone cannot be reached or did not allow it.
- **Local API:** `pocketdesk quiet <phone> [--minutes N | --off] [--json]`.
- **Events:** `phone_quiet` (quiet or not, never why).

## Not done

- Phone notifications are not held back on the computer during the computer's own Do Not Disturb (Windows Focus, GNOME's). The computer's notification system already does that.
- No mirroring the other way (the computer's Focus muting the phone). That is a later item, if asked.

## Testing

- **JVM:** `QuietRuleTest`, `QuietPlanTest`.
- **Rust:**
  - the request files (TTL, ids);
  - the plugin's permission gate and phone_status fields, through the handler tests as for ring;
  - the local API's `quiet` command against the doc test.
- **Device (A71 against the throwaway server):**
  - Do Not Disturb on with no exceptions: the throwaway chat app's notification is not shown on the computer; with the setting off it is.
  - Mute for 2 minutes from the CLI: the phone enters Do Not Disturb, the computer shows "Muted until", and it ends by itself.
  - Changing the filter by hand meanwhile keeps the owner's choice.
  - The switch off: refused.
  - The owner's Do Not Disturb settings are put back afterwards.
