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
- **Unplugged, in deep idle:** the same held for 10 minutes with Android told the phone was unplugged (`dumpsys battery unplug`) and forced into deep Doze (`dumpsys deviceidle force-idle`, reading `IDLE` throughout). The probe was never frozen, and it took each connection at once. Those connections came over adb's loopback, which Doze does not cut, so Doze and the Wi-Fi network are still owed.
- **ColorOS's freezer cannot be read from adb.** Its dumps (`oplus.hans`, `horae`, `osensemanager`) refuse the shell, so what it exempts was learned by trying.

## What

**Stay linked (the phone).** A foreground service, `LinkKeeperService`, of type `connectedDevice`. It does no work of its own: while it runs, Android counts the app as in the foreground. ColorOS then leaves the process alone, and Doze leaves its network alone. The listener's tickers and links go on as they are.

- **When it runs:** while all of these hold (`StayLinked.wanted`):
  - Settings "Stay linked with the screen off" is on (on by default);
  - a feature that runs on the notification listener is on (`HealthCheck.listenerFeatures`);
  - notification access is granted;
  - a computer is paired.

  It stops when any of them stops holding. It also checks this itself once it is in the foreground, so a start that raced the reason going ends there.
- **Who starts or stops it** (`LinkKeeperService.sync`, which is idempotent):
  - the listener, on a 2 s tick of its own that does nothing slow, deciding every 10 s;
  - the app, as it resumes, on each change of a setting, and every 10 s while it is open;
  - the boot and update receiver (D1's `HealthReceiver`), on every boot and update, before its own checks.

  Android may refuse a start from the background. Such a refusal is caught, logged once as a warning (debug after that), and retried a minute later; a start from the app takes over. Android also refuses the restart of a sticky service in a new process, so the service is not sticky. A failure to go to the foreground stops it rather than crashing the app.
- **Its notification:**
  - a channel of its own, "Staying linked", at the lowest importance, so it sits folded at the bottom of the shade with no icon in the status bar;
  - it reads "Keeps the link to Ann's PC with the screen off", naming the own computer;
  - it is posted again when those words would change: a rename, or another computer becoming the phone's own;
  - a tap opens Pocketdesk.
- **What it needs:**
  - `FOREGROUND_SERVICE_CONNECTED_DEVICE`;
  - `CHANGE_NETWORK_STATE`, which Android 14 requires for that type. It is a normal permission, granted at install.

  No wake lock and no exact alarm. Network packets wake the CPU, and the listener's own callbacks wake it for notifications.

**Notice a freeze (the phone).** `FreezeWatch` runs on the listener's keeper tick, which does nothing slow. Each tick, every 2 s, it reads `SystemClock.uptimeMillis` and compares it with the last reading:
- **What counts as a freeze:** a gap of 30 s or more. `uptimeMillis` stops while the CPU sleeps, so a phone merely asleep makes no gap. A gap means the CPU ran while the app did not, which is a freeze or a stop.
- **What is kept:** the freeze's time, and whether the service was running through it.

**Say it (the phone).** The Health card (D1) gains two problems. Each is shown while a listener feature is on and a freeze was seen in the last day:
- **`PAUSED_STAY_LINKED_OFF`**, when the switch is off: "Android paused Pocketdesk with the screen off, so the computer's messages waited", with **Turn on**, which turns the switch on and starts the service.
- **`PAUSED_ANYWAY`**, when the switch was on and the service ran through the freeze: "…even with "Stay linked with the screen off" on. This phone's own setting must let it run in the background", with T2's button for the phone's own setting. That button opens dontkillmyapp's page for the maker (`makerGuide`), or the app's settings page when the maker is not known.
- **No card for a freeze while a start was refused:** the app, opened to see the card, starts the service itself.

A pause is never told in the after-update notification, since no update caused it. Turning the switch on forgets the freeze.

**Settings.** "Stay linked with the screen off" in the System group, on by default. Its summary: "Keeps a quiet notification so Android does not pause Pocketdesk. Some phones (OPPO, Xiaomi, Samsung) pause apps without one."

## Not now

- **The computer saying a phone is asleep** (the plan's second half). The phone now stays linked, and the Health card says when it does not. A computer-side state waits for a measured need.
- **Exact alarms and a wake from the computer over UDP:** not needed, since the foreground service alone kept the probe unfrozen and reachable.
- **A freeze that ends with ColorOS killing the app goes unrecorded.** The watch keeps its last reading in the process, so a new process starts afresh. Recording a time at screen-off would catch it, and waits for a need.
- **A stop in Android's Task Manager does not hold.** On Android 13+, Android binds the listener again, and its first tick starts the service. The Settings switch is the way to turn it off.

## Privacy and cost

- **Privacy:** nothing new is sent.
- **Cost:**
  - **CPU:** the process now runs its tickers through the night instead of frozen. N7 measured 0.44 % of one core for them on the A71; the keeper tick adds a clock read every 2 s and a decision every 10 s. M1 owes the same hour on the OPPO, on battery.
  - **The notification:** it is visible but folded. On Android 13+ the owner may swipe it away, and the service keeps running.
  - **Default on:** chosen because the features it serves fail silently without it on common phones. Turning it off is one switch.

## Tests

JVM, pure rules:
- **`StayLinked.wanted`:** the switch, the features, access, a paired computer.
- **`FreezeWatch.frozen` and `recent`:** the first reading, ticks on time, a 30 s gap, uptime going back (a reboot), the day's window, a time ahead of the clock.
- **`Health`:** the switch off, the service through the freeze, a refused start (no card), no listener feature, and the after-update notification leaving pauses out.

## As built

- **Service:** `service/LinkKeeperService.kt`, with `wanted`, `sync` and `isRunning`.
- **Rules:** `doctor/StayLinked.kt`, holding `StayLinked` and `FreezeWatch`.
- **Listener:** its `keeperTicker` and `keepLinked` (`notify/PhoneNotifications.kt`).
- **Prefs:** `stayLinked`, and `lastFreezeMs` with `lastFreezeLinked`.
- **Health:** `Health` and `HealthCheck`, Home's card, and the Settings row.
- **Starts:** `MainActivity` (on resume, and in a loop on settings) and `HealthReceiver` (boot and update).
- **Strings:** English, Spanish and Tamil.

**Checked on the OPPO**, by adb with "Ring from the computer" turned on for the run:
- **Started without opening the app:** the listener started the service from the background. Android allowed it because a bound listener counts as a bound foreground service (`uidState: BFGS`).
- **The switch on:** Pocketdesk stayed unfrozen for 5 minutes with the screen off, where before it froze within 20 s.
- **The switch off:** it froze within 20 s. After the screen woke, the listener logged "Paused for 85 s while the phone ran" and kept the time. The gap was uptime, so it was less than the two minutes the screen was off.
- **Restored:** the phone's settings went back to their own, and the service stopped with no feature on.

## Review

A fresh review found 0 Critical, 4 Important and 9 Minor. Each fix is its own commit:

- **Important:**
  - a sticky restart could crash the app;
  - an ordinary reboot never started the service;
  - the card blamed the phone's maker for a refused start, the service's start erased a real freeze, and the card did not redraw;
  - this spec did not say what was built.
- **Minor, fixed:**
  - the service outlived its reason;
  - the notification's name did not follow a rename, and it claimed a live link;
  - a pause was told as an update's doing;
  - the decision cost a read every tick, and a refusal was logged every minute;
  - a test repeated another's inputs;
  - the card's words;
  - README's claim.
- **Minor, left and said above:** a freeze ending in a kill goes unrecorded, and a stop in Task Manager does not hold.

## On the device (checklist §166)

- The Health card on screen, both problems and their buttons. Opening the app here would start a session with this computer, so it is owed.
- After a reboot, the service is back without opening the app.
- On battery without USB, on Wi-Fi, for an hour (N7's script): a ring from a throwaway server arriving with the screen off, CPU use, and battery drain.
- A Samsung, a Xiaomi and a Pixel.
