# M1: linked with the screen off

Plan: `docs/research/2026-10-21-m-cycle.md`, M1.

## Why

The L cycle's checks on the OPPO (CPH2359, ColorOS 14) saw pushes from the computer, such as dials and commands, wait until the phone's screen came on. This happened with Android's battery optimization lifted too. KDE Connect's users report the same on Samsungs and others (KDE Discuss 2026-04-30 and 2026-06-10; KDE bug 392950).

## What was found

Measured on the OPPO by adb, plugged in by USB, with the screen off (`input keyevent 26`):

- **Pocketdesk is frozen within 20 s.** ColorOS's own freezer ("hans") sets `cgroup.freeze` on the app's whole uid (`/sys/fs/cgroup/uid_<uid>/cgroup.freeze = 1`, `cgroup.events: frozen 1`), even though Android's own freezer says it is not frozen (`isFrozen=false`). The process sits in cpuset `/restricted`. A frozen process runs nothing: its links read no packet, and its tickers do not tick.
- **Why it freezes:** in the background, Pocketdesk is only a bound notification listener. Its seven tickers (answers, files, clipboard, ring, screenshots, photos, replies) and every link they keep run in that process. No foreground service runs outside a session or a transfer.
- **A foreground service keeps it out.** A throwaway probe app (not in the repo) held a `connectedDevice` foreground service with no wake lock:
  - it was never frozen in 15 minutes with the screen off;
  - a TCP connection to it was accepted within a second, every minute;
  - its 2 s ticker never missed a tick.

  Pocketdesk, beside it, stayed frozen the whole time.
- **Unplugged, in deep idle:** the same for 10 minutes with Android told the phone was unplugged (`dumpsys battery unplug`) and forced into deep Doze (`dumpsys deviceidle force-idle`, reading `IDLE` throughout): the probe was never frozen, and took each connection at once. Those connections came over adb's loopback, which Doze does not cut, so Doze and the Wi-Fi network are still owed.
- **ColorOS's freezer cannot be read from adb.** Its dumps (`oplus.hans`, `horae`, `osensemanager`) refuse the shell, so what it exempts was learned by trying.

## What

**Stay linked (the phone).** A foreground service, `LinkKeeper`, of type `connectedDevice`. It does no work of its own: while it runs, Android counts the app as in the foreground. ColorOS then leaves the process alone, and Doze leaves its network alone. The listener's tickers and links go on as they are.

- **When it runs:** while all of these hold:
  - Settings "Stay linked with the screen off" is on (on by default);
  - a feature that runs on the notification listener is on (`HealthCheck.listenerFeatures`);
  - notification access is granted;
  - a computer is paired.

  It stops when any of them stops holding.
- **Who starts it, and when:**
  - the listener, as it connects and on each answers tick;
  - the app, as it opens;
  - the boot and update receiver (D1's `HealthReceiver`).

  Each start is idempotent. Android may refuse a start from the background (`ForegroundServiceStartNotAllowedException`). Such a refusal is caught and remembered, and the next start from the app takes over.
- **Its notification:**
  - a channel of its own, "Staying linked", at the lowest importance, so it sits folded at the bottom of the shade with no icon in the status bar;
  - it reads "Linked to Ann's PC" (the own computer's name);
  - a tap opens Pocketdesk.
- **What it needs:**
  - `FOREGROUND_SERVICE_CONNECTED_DEVICE`;
  - `CHANGE_NETWORK_STATE`, which Android 14 requires for that type. It is a normal permission, granted at install.

  No wake lock and no exact alarm. Network packets wake the CPU, and the listener's own callbacks wake it for notifications.

**Notice a freeze (the phone).** `FreezeWatch` runs on the answers tick (every 2 s). It reads `SystemClock.uptimeMillis` and its own last reading:
- **What counts as a freeze:** a gap of 30 s or more. `uptimeMillis` stops while the CPU sleeps, so a phone merely asleep makes no gap. A gap means the CPU ran while the app did not, which is a freeze or a stop.
- **What is kept:** the last freeze's time, in Prefs.

**Say it (the phone).** The Health card (D1) gains `FROZEN_IN_BACKGROUND`, shown when both of these hold:
- a freeze was seen in the last 24 hours;
- a listener feature is on.

What it says and offers depends on the switch:
- **The switch is off:** "Android paused Pocketdesk with the screen off, so the computer's messages waited", with **Turn on** for "Stay linked".
- **The switch is on and the service is running, but it froze anyway:** the same words, with **Maker's guide**. It opens dontkillmyapp's page for the maker (T2's `makerGuide`), or the app's settings page when the maker is not known.
- **The switch is on but Android refused the start:** "Open Pocketdesk once to stay linked", with **Fix**, which opens the app.

**Settings.** "Stay linked with the screen off" in the System group, on by default. Its summary: "Keeps a quiet notification so Android does not pause Pocketdesk. Some phones (OPPO, Xiaomi, Samsung) pause apps without one."

## Not now

- **The computer saying a phone is asleep** (the plan's second half). The phone now stays linked, and the Health card says when it does not. A computer-side state waits for a measured need.
- **Exact alarms and a wake from the computer over UDP:** not needed, since the foreground service alone kept the probe unfrozen and reachable.

## Privacy and cost

- **Privacy:** nothing new is sent.
- **Cost:**
  - **CPU:** the process now runs its tickers through the night instead of frozen. N7 measured 0.44 % of one core for them on the A71. M1 owes the same hour on the OPPO, on battery.
  - **The notification:** it is visible but folded. On Android 13+ the owner may swipe it away, and the service keeps running.
  - **Default on:** chosen because the features it serves fail silently without it on common phones. Turning it off is one switch.

## Tests

JVM, pure rules:
- **`StayLinked.wanted`:** the switch, the features, access, a paired computer.
- **`FreezeWatch.frozen`:** no gap, a sleep (uptime still), a 30 s gap, a clock that went backwards.
- **`Health`:** the new problem's three cases, and its 24 h window.

## On the device (checklist §166)

- The OPPO with the screen off for 10 minutes and the switch on: `cgroup.freeze` stays 0, and a link from the computer reaches the phone at once (a ring sent from a throwaway server).
- The switch off: frozen within a minute, and on the next open the Health card offers Turn on.
- After a reboot: the service is back without opening the app.
- On battery without USB for an hour (N7's script): CPU and the battery drain.
