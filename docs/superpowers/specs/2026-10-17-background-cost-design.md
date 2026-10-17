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
