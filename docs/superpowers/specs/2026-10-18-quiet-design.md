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
- **State:** the phone's last report is kept in `phone_quiet.json` (`quiet`, `until_unix`, `by_computer`, `refused_unix`), shown in the Overview's phone block and in Devices: "Do Not Disturb", or "Muted until 15:30".
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

## As built, and the review

- **Where it is:**
  - phone: `notify/Quiet.kt` (the rules) and `notify/QuietPhone.kt` (the settings and the tick);
  - bridge: `quiet_requests.rs`;
  - core: `protocol/quiet.rs`, `local/quiet.rs` and `local/phone_quiet.rs`;
  - server: `quiet_phone.rs` and `pocketdesk quiet`;
  - desktop: the Overview, and the permission row in Devices.
- **A mute starts only from "all".** A phone already in Do Not Disturb is the owner's, and the Overview offers no Mute then.
- **Android 15 and newer** keep an app's filter as its own mode, "Do Not Disturb (Pocketdesk)". Asking for "all" ends only that mode, so an end, an Unmute or a forgotten mute always retires it. Before 15, an end asks for "all" only while the mute's filter stands.
- **Settling.** A mute's filter is given 10 s to take: Android applies a requested filter later, so a filter read just after asking is not the owner's change.
- **What the computer is told:**
  - The phone says nothing while its listener is not connected, since the filter is unknown then.
  - It tells its state again whenever its link comes back.
  - It answers a mute it may not take with `refused`. The Overview shows that for two minutes.
- **The computer** shows a phone's quiet only while that phone is linked. The state file is written one at a time, always the newest. A first "not quiet" makes no event.
- **Hold back:** a notification Do Not Disturb silences (`Ranking.matchesInterruptionFilter()` false) is not sent, whether or not the shade still lists it.
- **Fresh review: 0 Critical, 6 Important, 12 Minor.** Fixed one commit each:
  - a mute took over the owner's own Do Not Disturb and turned it off at its end;
  - an unknown filter read as "not quiet";
  - stale states were shown as now's;
  - refusals were silent;
  - the writer could leave an older state;
  - no report after a reconnect.

  Minors fixed: the order of keeping and asking, a clock set back, a minute's limit on a waiting mute, "silences", the first event, `--minutes 0`, the doc's exit codes.

  Left:
  - two dials a minute when ring and mute both keep a link to a computer away;
  - requests in the same second in either order;
  - Unmute refused once Mute is off in Devices.
- **Found on the device (A71, Android 16), fixed one commit each:**
  - the bridge did not list `quiet_state` among what the phone sends, so no state reached the computer and each failure paused the phone's notifications for a minute;
  - Android 15's mode outlived a forgotten mute and kept the phone quiet;
  - every mute was forgotten 11 ms after it began;
  - an end during the owner's own Do Not Disturb left the mode running.

  After the fixes every step of the run passed (checklist §112).
