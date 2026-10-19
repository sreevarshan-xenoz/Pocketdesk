# F5: ringing calls on the computer (2026-10-20)

Plan: `docs/research/2026-10-20-next-cycle.md` (D10's calls, the part that needs no answering on the computer).

## Design

A ringing call is told to the computer as one of the phone's notifications, so everything the computer already does with those does it:
- the toast with its buttons (N3);
- the Notifications page;
- the answer back, with how it went (E1).

There is no computer change.

- **The phone, Settings "Ringing calls on the computer"** (off; it asks to read the phone's state and its call log, for the number):
  - the notification listener watches the call state (`PhoneStateListener`, which gives the number; `TelephonyCallback` does not);
  - a ringing call sends `kdeconnect.notification {id: "pocketdesk:call:N", app: "Phone", title: "Call from Ann (+1…)", actions}`, with the name from the contacts when "Names from my contacts" is on;
  - its end sends the note's cancel.
- **The buttons:**
  - "Silence" mutes the ring until the call ends;
  - "Decline", with "Let the computer decline calls" (asks to end calls, Android 9+), ends it (`TelecomManager.endCall`).
- **Answers:** a call note's answers are taken before the notifications' own (`CallRule.isCall`) and decided by `CallRule` (JVM-tested): the phone's own computer, the switch on, the call still ringing, a button offered.

## As built

As designed, with what the review changed:
- the watcher registers whenever the switch and permission allow, retried on the listener's ticks;
- one note per call, known as ringing at once;
- a cancel retried until it goes;
- the ring unmuted at every end and stop.

**Not known until a device with a SIM:**
- whether Android lets the app mute a ringing call (Silence says so when it will not);
- which call `endCall` ends with a call waiting.

## Review

A fresh review found 1 Critical and 5 Important issues. Fixed, one commit each, or together where they share code:
- calls never watched when the permission came after the listener started;
- a stale note from a call ending at once, and a second note for the second "ringing";
- a cancel lost while the link rested;
- the ringer left muted;
- a Silence refusal sending the owner to a setting the app has no way to;
- no SECURITY.md limit (now 34).

Minors left:
- answers only on the 2 s tick;
- dual SIM;
- READ_CALL_LOG is a restricted permission on Google Play (Pocketdesk ships by F-Droid, Obtainium and its releases);
- D1's health check does not name the call permissions.

Owed (device with a SIM, with the owner): checklist §130.
