# The phone app in Spanish and Tamil (U6)

Date: 2026-10-17. Plan: `docs/research/2026-10-17-finish-and-polish.md` §U6. From the UI audit: L2, L3; and what U4's check in Spanish and Tamil showed.

## The problem

- **English that never reaches the resources:** the connection details' labels and caption (`fps`, `kbps`, `rtt`, `e2e`, `drops`, `desk`, "rates over 2.6s · rtt median of 3", "no video yet", "rtt unsampled") and the shortcut bar's key captions (Super, Alt+Tab, Ctrl+Alt+Del, PrtSc, Esc).
- **One thing, two words:** Spanish calls direct touch "Tacto directo" on the session's chip and "Toque directo" in its menu; Tamil "நேரடி தொடும்" and "நேரடித் தொடுதல்".
- **Long labels in fixed rows:** Home's bottom row puts "Scan pairing code" and "Add computer" side by side; in Tamil the second breaks one word per line.

## The design

1. The details' labels and caption become string resources, with Spanish and Tamil (abbreviations that are the same in each language, such as `fps` and `ms`, stay as they are).
2. The key captions become resources: Spanish keyboards' own names where they differ (Supr for Del, Impr Pant for PrtSc); Tamil keeps the keys' printed names.
3. One word for direct touch in each language: the chip's string follows the menu's.
4. Home's bottom row wraps (a `FlowRow`), so a long label takes a line of its own instead of breaking.
5. Kept in English: what the phone sends for the computer to show (the phone's folders' errors, "New notification" for a hidden one), since the desktop app is in English.

## Tests

A JVM test that every string in `values/strings.xml` exists in `values-es` and `values-ta` (lint warns about a missing translation; a test fails the build).

## Check

On the A71 with the app's language set to Spanish and to Tamil (`cmd locale set-app-locales`, reset after): Home's bottom row, a session's chip and menu, the connection details on, the shortcut bar.
