# E1, answers that say how they went (design)

Date: 2026-10-19. Plan: `docs/research/2026-10-19-next-cycle.md`, E1.

## Why

Phone Link's 2026 complaint: replies from notifications that never reach the phone, with nothing said (Microsoft Q&A, April and September 2026; Eleven Forum). Pocketdesk's answers (T10c, N3) are sent and forgotten too:
- the server drops one it cannot deliver (not offered any more, Notifications off, the phone away too long) with only a log line;
- the phone runs one, or refuses it, with only a logcat line.

## What it does

- **Each answer carries its id** (`answerId`, the request's id) in `kdeconnect.notification.reply` and `.action`. A phone from before E1 ignores it.
- **The phone answers each one it took** with `kdeconnect.pocketdesk.notification_answered {answerId, ok, reason}`, on its link, under the Notifications permission:
  - `ok`: handed to the app (`PendingIntent.send` returned). Whether the app then sent the message is the app's to say; the computer says "Handed to the app".
  - Not `ok`, with a plain reason:
    - the notification is gone;
    - its answer is not offered any more;
    - answering is off on the phone;
    - too many answers at once;
    - another computer's;
    - the app refused it (`PendingIntent.CanceledException`).
- **The computer keeps each answer's state** in `notification_answers.json` (owner-only, the newest 30):
  - waiting for the phone;
  - sent to the phone;
  - handed to the app;
  - not sent, and why.

  Its own drops are kept too: not offered, Notifications off for that phone, or the phone not connected within the answer's time. The phone's outcome is taken only from the phone the answer went to.
- **The Notifications page** shows the newest answer's state under its notification:
  - "Waiting for the phone…";
  - "Handed to the app";
  - "Not sent: …";
  - "No word from the phone": sent a minute ago, and no outcome since. A phone from before E1 never says.
- **A failed answer gets a desktop notification,** which matters for one sent from a toast (N3), whose window may be hidden.

## Not done

- **Whether the app's own message left the phone:** Android gives a notification's reply action no result.

## Testing

- **Core, pure:** the outcome's bounds (the id's form, the reason cut at 200 characters and cleaned); the store's write, read, cap and update.
- **Server:** the drops recorded; the outcome taken only from the answer's own phone; the answer packets carrying `answerId`.
- **Phone (JVM):** each decision's outcome and words; the app's refusal.
- **Desktop:** the line for each state and age.
- **Device (checklist §120):** with the throwaway chat app:
  - a reply handed to the app;
  - an answer to a notification swiped away meanwhile ("the notification is gone");
  - answering off on the phone;
  - the phone away past the answer's time.
