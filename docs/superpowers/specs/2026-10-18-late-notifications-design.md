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

## As built

As designed, plus what the device run found:
- **A notification written into a dead link was lost.** The phone learns a link has died only when its reader notices, some 35 s after the computer goes. A notification sent in that window was counted as sent and never reached the computer, so it never waited (found on the A71: posted 2 s after the computer stopped, never shown when it came back).
- **The fix:**
  - The computer's identity announces `kdeconnect.notification.ack` among what it sends. The control handshake returns that with the computer's name (`PeerClaims`), heard again at every handshake.
  - Sent to a computer that says so, a notification asks (`ack: true`). The computer answers `{id}` as soon as it reads it, or `{id, refused: true}` when the phone's Notifications permission is off.
  - The bridge waits 5 s for the answer. Silence leaves the notification in the queue, and the next try (after the usual minute's pause) goes on a fresh link.
  - A cancel never asks, and a computer from before C3 is sent to as before.
- **The late line** counts from when the notification's latest version was posted: an update keeps its first version's place and age in the queue.
- **"Don't wait"** sends at once or gives up, and queues nothing.

## The device run

On the A71 against a throwaway server on this box.

**The dead link:**
- the server stopped, a notification posted 2 s later, the server back 65 s after that: it arrived 10 s after the server returned, `late_secs` 68, the toast reading "Sent 1 min late";
- run again on the build with the review's fixes, with the same result;
- before the fix, the same steps lost the notification.

**A cancel:** a notification closed on the phone (snoozed, since this build of Android has no `cmd notification cancel`) closed its toast at once, with no pause.

**The Doze hour:**
- 15:47 to 16:47, `dumpsys deviceidle force-idle`, the screen off, `dumpsys battery unplug` (USB still in), a notification posted every 5 minutes;
- deep idle all hour, and all 12 reached the computer within a second of being posted, so the queue was never needed;
- Pocketdesk's process stayed a bound foreground service (BFGS) all hour, which is likely why Doze did not cut its network;
- its cost: 34.9 s of CPU (27.7 s user, 7.2 s system), and 4.2 MB received and 2.9 MB sent over Wi-Fi (8 493 and 8 665 packets). The traffic is worth a look of its own: the link rode n0's relay.

**Owed (checklist §106):**
- Doze on battery without USB, where the phone suspends;
- a phone whose listener is not exempt;
- a computer from before C3 (sent to as before);
- Linux;
- the Settings choice on screen.

## Review

**Fresh review (0 Critical, 3 Important), fixed one commit each:**
- whether a computer answers was remembered per address for good, so a downgraded computer, or another at that address, had every notification sent again each minute (now from its identity at each handshake);
- a cancel asked for an answer, held the phone's sends for 5 s and paused them for a minute;
- a notification whose id the computer would not keep to answer was never answered.

**Minors fixed:**
- an update shown as late for its first version's wait;
- "Don't wait" queueing until the next tick;
- `late_secs` for a few seconds of retries;
- stale words in the README;
- comments out of place.

**Left as is:**
- **A 5 s wait on the one worker:** a send on a dead link holds the phone's notification work for up to 5 s once, before the pause. A burst of more than 20 meanwhile drops the oldest tasks, as any burst did.
- **An answer before the flood guard:** a notification over the computer's limit of 20 a minute is answered, then not shown, as before.
- **A cancel lost on a dead link** leaves the toast on the computer, as before C3.
- **Tests:** none drives the bridge's wait against a live link; the device run did.

