# Call a number from the computer (L4, part 1)

**Date:** 2026-10-21. **Branch:** `feat/dial`. The L cycle's fourth item (`docs/research/2026-10-20-l-cycle.md`).

## What is wrong

- **The computer cannot start a call.** A number in a text on the Messages page, or one in a notes file, has to be typed on the phone by hand.
- **Asked for:** KDE bugs 341950 and 397333; Apple's Continuity places calls from a Mac.

## What changes

- **The computer asks:** "Call" beside each conversation's number on the Messages page, and `pocketdesk call <phone> <number> [--json]`. The request goes through `<state>/dials_out/` (one file each, a minute to live), and the server sends `kdeconnect.pocketdesk.dial {number}` to that phone if it is connected.
- **The number:** spaces, dashes, dots and brackets are dropped; what is left must be digits, with at most one `+` at the start, 3 to 32 characters (`core::protocol::dial`, tested on both ends). Never `*` or `#`: those make MMI and USSD codes, and one tap on Call of `**21*…#` would forward every call.
- **Permissions:**
  - **The computer:** a per-phone permission, "Call numbers from here" (`Capability::Dial`, on). Like Ring, it is about what the computer sends.
  - **The phone:** Settings "Calls from the computer", off by default.
- **The phone opens its dialer with the number** (`ACTION_DIAL`, never `ACTION_CALL`): the owner always presses Call.
  - **In a session with the app in front:** the dialer opens at once.
  - **Otherwise** (a session behind another app, or a link a background feature keeps): a notification "Call +1 555 0100? From the computer", whose tap opens the dialer, since Android blocks starts from behind. A session takes the numbers while it runs; the listener only outside one.
  - **Only from its own computer** in the background, and at most 10 a minute.

## Left out

- **A `tel:` handler** on the computer (Windows' RegisteredApplications, Linux's `x-scheme-handler/tel`), so a `tel:` link anywhere goes to the phone: part 2.
- **"Call" on a ringing call's note (F5):** a call back after a missed call is part 2.

## Testing

- **Unit:** the number rule (both ends); the dials' outbox; the CLI's checks; the phone's rule (JVM: setting, own computer, budget).
- **On the device (checklist §159; L3 is §158):**
  - without a SIM, `ACTION_DIAL` still opens the dialer: `adb shell dumpsys activity activities` shows it with the number;
  - in the background, the notification in `dumpsys notification`.
- **Owed:** a real call (a SIM).

## As built

- **Core:** `core::protocol::dial` (`DIAL`, `clean_number`, `dial_body`), `core::local::dials` (the outbox), `Capability::Dial`.
- **Server:** `links::run_dials` (sent to the connected phone with Dial on, within a minute), the links plugin announcing `DIAL`; `pocketdesk call` (`local_api::call_in`, refusing a paired computer).
- **Desktop:** Call on the Messages page (refused at once with Dial off); the Devices switch, hidden for computers.
- **Bridge:** `dials.rs`, kept with the computer it came from and dropped when the phone leaves it.
- **Phone:** `calls/DialRule.kt` (JVM-tested), `calls/Dials.kt`, `ui/DialsEffect.kt`, the listener outside a session, Settings "Calls from the computer".

## Review

A fresh review (0 Critical, 3 Important, 9 Minor), fixed one commit each:

- **Important:** the session and the listener raced for the numbers (now the session takes them while it runs); a session behind another app lost the number (now offered in a notification); `*` and `#` let a computer put an MMI code one tap from running, such as call forwarding (now refused on both ends).
- **Minor, fixed:** 32 digits refused on the phone, the page's Call before the permission, a paired computer as a target, the CLI's JSON, a moved doc comment, a failed notification unsaid, the Settings words, `own!!`.
- **Left:** no pacing on the computer beyond the phone's ten a minute; a `tel:` handler and Call on a ringing call's note (part 2).
