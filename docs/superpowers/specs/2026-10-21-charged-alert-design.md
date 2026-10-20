# "Charged to 80 %" (L3)

**Date:** 2026-10-21. **Branch:** `feat/charged-alert`. The L cycle's third item (`docs/research/2026-10-20-l-cycle.md`).

## What is wrong

- **The computer warns when a phone runs low (T8b), never when it has charged.** GSConnect has a charged alert; KDE Connect users ask for one.

## What changes

- **Desktop Settings "Tell me when a phone has charged to":** off (the default), 80, 90 or 100 % (`charged_alert`, read at each report, so a change applies at once).
- **One notification a charge:** "Phone 5f1c9b3d: charged to 82 %", from the battery reports that already arrive (a phone's, a KDE device's).
  - **When it tells:** at or above the level while charging. A report can skip a level, since it comes on a change or every five minutes.
  - **When it re-arms:** once the phone is unplugged, or 5 % under the level.
  - **A charge it saw:** a phone first seen past the level (on its charger as the server starts), or past a level just set, tells nothing until it charges again.
  - **While paused (K2a):** not shown, like the low warning.
- **Android's own charging limit:** a phone that stops itself at 80 % never reaches 100; the Settings line says so.

## Testing

- **Unit:** once a charge, a skipped level, re-arming unplugged and 5 % under, off.
- **On the device (checklist §158; L2 is §157):** `adb shell dumpsys battery set ac 1`, levels 79 then 80 with Share battery on, the toast read back, then `dumpsys battery reset`.

## Review

A fresh review (0 Critical, 1 Important, 5 Minor):

- **Important, fixed:** it told of a phone found already past the level (at each start, or as the setting was turned on), not of a charge it saw.
- **Minor:** the spec said "held" while paused (it is not shown; now said); the docs owed. Left: the message names the phone by its id, as the low warning does (`TrustStore::name` would serve both later).
