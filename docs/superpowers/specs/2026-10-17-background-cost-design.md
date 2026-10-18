# The phone's background cost (N7)

Date: 2026-10-17. Plan: `docs/research/2026-10-17-finish-and-polish.md` §N7. Owed from R4 (the phone's battery and idle traffic) and R6.

## The problem

With any background feature on (the computer's clipboard, files, ringing, answering notifications, screenshots, the phone's folders), the notification listener keeps a control link to the computer and runs six timers: answers, the phone's folders, the ring and screenshots every 2 s, replies and the clipboard, files and battery every 5 s. Nobody has measured what that costs a phone in a pocket, so nobody knows whether it matters, or which part does.

## Method

On the A71 against a throwaway server on this box (USB stays in for adb, so the battery is logically unplugged: `dumpsys battery unplug`, restored after):

1. `dumpsys batterystats --reset`, the screen off, nothing else touched, for one hour **with** the background features on (notification access granted for the test, then revoked): the clipboard in the background, files in the background, ring from the computer.
2. The same hour **without** them (the listener unbound: notification access revoked), as the baseline.
3. From `dumpsys batterystats` for the app's uid in each: CPU time (user and system), wake lock time, wakeup alarms, Wi-Fi bytes and packets, and the estimated power drain; from `dumpsys deviceidle` whether the phone dozed; from the server, how many packets the link carried.

## Then

Cut what the numbers show, and only that: for instance timers that run while nothing is enabled for them, a tick that does work with nothing to do, keepalives more often than a NAT needs. Each cut is measured again the same way. The results, with the numbers, go in this spec and the checklist.

## As built

**How it ran:**
- The A71 against a throwaway server on this box, with three background features on: the clipboard, files and ringing. Notification access was granted for the test and revoked after.
- The battery was logically unplugged (`dumpsys battery unplug`) with USB in for adb. The link rode `adb reverse`, so it crossed USB, not Wi-Fi.
- `dumpsys batterystats` was reset at the start; the screen was off for the hour, and nothing was touched.
- The debug build (no R8, JIT).

**The baseline without the features was not measured as an hour.** With notification access revoked and no session, the app has no process running, so its cost is nothing. The comparison that matters is before the cuts against after.

**The profile:** `simpleperf record --app dev.pocketdesk.android -g`, 40 s with the screen off.
- The six timer threads took 94 % of the app's cycles; the bridge's own runtime took about 5 %.
- On every tick, every 2 to 5 s, each timer worked out the phone's own computer by reading and parsing the saved computers and their pins. The folders timer did so even with no folder shared.

**The cuts (one commit each):**
1. The phone's own computer is worked out once and kept 5 s (`Recent`, JVM-tested). Any change to the saved computers forgets it at once (`HostStore.generation`), so a removed computer stops being the phone's own straight away (a review fix).
2. The folders timer does nothing when no folder is shared and nothing is asked.
3. No call to the system for folder grants when none is shared (a review fix).

**The numbers, for the app's uid over one hour each:**

| | Before | After |
|---|---|---|
| CPU, user | 18.9 s | 13.9 s |
| CPU, system | 2.2 s | 2.0 s |
| CPU in all | 21.0 s (0.58 % of one core) | 15.9 s (0.44 %) |
| Wake locks | none | none |
| Wakeup alarms | none | none |
| Doze | deep idle all hour | deep idle all hour |

- **A quarter less CPU.**
- **The app holds no wake lock and sets no alarm.** Its timers are coroutine delays, which run only while the CPU is awake for some other reason.
- **These figures are an upper bound for a phone in a pocket.** With USB in, the A71 never suspended (uptime was 100 % of the hour in both runs). A phone on battery sleeps between wakes, and the timers sleep with it.
- **No Wi-Fi cost was measured:** the link rode USB here. The phone's keepalives on Wi-Fi are owed (checklist §102).
- **What was not cut:** the timers themselves. Merging six into one would save little more than this cut did, now that a tick with nothing to do returns at once.

## Review

**Fresh review (0 Critical, 1 Important), fixed:**
- **Important:** a removed computer stayed the phone's own for up to 5 s, so its replies, answers, folder requests and clipboard could still be taken in that window. The saved computers now count their changes, and a change is seen at the next look.
- **Minor:** a call to the system for folder grants with none shared, now skipped.

