# Notifications that failed to send wait and go later (C3)

**Plan:** `docs/research/2026-10-17-next-cycle.md`, C3.

## What is there already

N7's review (f435dde8) left the phone with a queue for notifications that could not be sent (`notify/PendingNotifications`, `NotificationOutbox`):
- in memory only, at most 20, each at most 10 minutes;
- a new notification waits behind older ones so an old version never lands after a newer one;
- one closed on the phone is dropped from the wait, and closed on the computer if it was shown there;
- the listener's 5 s tick sends what waits once the computer answers.

What it lacks:
- **The computer cannot tell a late notification from a new one:** a message from 40 minutes ago is shown as if it just came, and the desktop's Notifications page dates it from its arrival.
- **Ten minutes is shorter than a phone's sleep:** in Doze the network is cut for minutes at a time (longer between maintenance windows as the phone stays idle), so most of what came while a phone lay on a desk was dropped before it could go.
- **No setting:** how long they wait is fixed.

## Design

**The phone:**
- Each notification sent from the wait carries `waited`, the whole seconds it waited, measured on the phone's monotonic clock (`elapsedRealtime`), so the two clocks never need to agree. One sent at once carries none.
- Settings, "This phone's notifications": **"When the computer is out of reach"**, a choice of *Wait up to 10 minutes*, *Wait up to an hour* (the default) or *Don't wait* (`notification_wait_minutes`: 10, 60, 0). The queue keeps its 20 at most; nothing is written to disk.

**The computer:**
- A notification with `waited` of a minute or more is marked late: its text gains a last line, "Sent N min late" (or "N h M min late"), on Windows' toasts and Linux's notifications alike.
- Its `at_unix` (the desktop's Notifications page) is its arrival less `waited`, so "ago" is true.
- Its `notification` event gains `late_secs`.
- `waited` is capped at a day; a bad value is ignored, never refused.
- The KDE Connect bridge's notifications are not touched: KDE Connect sends no such field.

**The wire:** `kdeconnect.notification` gains an optional `waited` (seconds, number). Older computers ignore it; older phones never send it. `docs/protocol.md` and the packets doc say so.

## The Doze run

An hour on the A71 with `dumpsys deviceidle force-idle`, the screen off and unplugged (`dumpsys battery unplug`), against a throwaway server:
- a notification posted every 5 minutes from the shell (`cmd notification post`), the shell's notifications shared;
- what arrived and when, from the throwaway server's `events.jsonl`;
- the battery before and after, as N7 measured;
- run as the phone is set (whether Pocketdesk is exempt from battery optimization is recorded, not changed).

## Testing

- JVM: the queue gives each sent one its wait; the age limit is the setting's, and *Don't wait* sends at once or drops.
- Rust: `late_line` words; `waited` read, capped and ignored when bad; `at_unix` moved back.

## Out of scope

- A wait that survives the app being killed (it would put notification text on disk).
- Raising the limit of 20.
