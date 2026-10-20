# "Charged to 80 %" (L3)

**Date:** 2026-10-21. **Branch:** `feat/charged-alert`. The L cycle's third item (`docs/research/2026-10-20-l-cycle.md`).

## What is wrong

- **The computer warns when a phone runs low (T8b), never when it has charged.** GSConnect has a charged alert; KDE Connect users ask for one.

## What changes

- **Desktop Settings "Tell me when a phone has charged to":** off (the default), 80, 90 or 100 % (`charged_alert`, read at each report, so a change applies at once).
- **One notification a charge:** "Phone 5f1c9b3d: charged to 82 %", from the battery reports that already arrive (a phone's, a KDE device's).
  - **When it tells:** at or above the level while charging. A report can skip a level, since it comes on a change or every five minutes.
  - **When it re-arms:** once the phone is unplugged, or 5 % under the level.
  - **While paused (K2a):** held like the low warning.
- **Android's own charging limit:** a phone that stops itself at 80 % never reaches 100; the Settings line says so.

## Testing

- **Unit:** once a charge, a skipped level, re-arming unplugged and 5 % under, off.
- **On the device (checklist §158):** `adb shell dumpsys battery set ac 1`, levels 79 then 80 with Share battery on, the toast read back, then `dumpsys battery reset`.
