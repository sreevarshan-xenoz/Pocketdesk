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

## As built

- **Connection details** (`StatsHud.kt`): `hud_fps`, `hud_kbps`, `hud_rtt`, `hud_delay`, `hud_lost`, `hud_computer` and the caption's `hud_no_video`, `hud_rates_over`, `hud_rtt_unmeasured`, `hud_rtt_median`. English now says delay, lost and computer where it said e2e, drops and desk; Spanish retardo, perdidos, equipo; Tamil தாமதம், இழந்தவை, கணினி. Numbers keep the phone's own formatting (Spanish shows "1,5 s"). The panel ends 72 dp from the edge, clear of Exit (found in Tamil, where it ran under it).
- **Keys** (`ShortcutBar.kt`, `KeyboardBar.kt`, the remote's `RemoteSession.Key.labelRes`): `key_*` resources. Spanish: Ctrl+Alt+Supr, Impr Pant, Inicio, Fin, Re Pág, Av Pág, Supr, Insert, workspaces E1–E9; Tamil keeps the printed names.
- **Direct touch**: the chip's `mode_direct_touch` follows the menu's `value_direct_touch` (Toque directo; நேரடித் தொடுதல்).
- **Home**: the bottom row is a `FlowRow`.
- **The guard**: `StringsTranslatedTest` fails when a string is in English only, or only in a translation.
- **Kept in English**, as designed: what the phone sends for the computer to show.

## Review

A fresh review (0 Critical, 0 Important, 7 Minor), fixed one commit each:

- The strings file, the bars' comments and the checklist still called the HUD's and keys' words English (the checklist now looks for delay, lost, "rtt not measured yet").
- Spanish "Áreas de trabajo" over workspaces captioned E1–E9 (now "Espacios de trabajo"), and two words for the trackpad in Spanish and in Tamil; Tamil seconds without the full stop.
- The keyboard bar's modifiers printed the enum's names (Spanish: Mayús for Shift).
- The mic share's failures were English sentences built from exceptions; now resources, the details in logcat.
- The translation test counted names inside comments and let a dropped placeholder pass; it strips comments and compares each string's distinct placeholders.

