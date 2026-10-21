# M6: no toast while the phone is in hand

Plan: `docs/research/2026-10-21-m-cycle.md`, M6.

## Why

A phone's notification pops up on the computer while the owner is reading it on the phone. KDE's Android merge request 257 (2021, not merged) forwarded notifications only while the screen was off. That loses them on the computer; this keeps them and only skips the pop-up.

## What

- **The phone:** Settings "Quiet on the computer while I use the phone", in the notifications group, off by default. With it on, a notification that comes while the phone's screen is on and unlocked is sent marked `phoneInUse: true` (`InHandRule`).
  - **Its own moment counts:** the mark is decided as it comes, so one that waits to be sent keeps it.
  - **Copies count too:** copies to other computers (L8) carry the mark.
  - **Calls never:** a ringing call's note (F5) is never marked.
- **The computer:** a notification marked so is held as a paused computer's are (K2a). It is logged as an event, with `phone_in_use`, kept on the Notifications page to answer, and counted against the phone's limit, but shown in no toast or pop-up.
- **An older computer** ignores the field, and shows its toast as now.

## Not now

- "Quiet: you are on the phone" on the Overview. The mark is per notification, so no state is kept to show.
- Rules of its own per computer: the switch is the phone's.

## Privacy

Nothing new leaves the phone except one boolean on a notification it already sends: that the phone was unlocked and in use when it came.

## Tests

- **Phone (JVM):** `InHandRule.marks`: the switch, the screen, the lock.
- **Computer:** `phone_in_use` read from a body, and a notification so marked held, not shown.

## On the device (checklist §170)

- The throwaway chat app's notification with the OPPO's screen on and unlocked: kept and logged on the computer, no toast (the event's `phone_in_use`, and the toast log).
- The same with the screen off, or the switch off: a toast.
