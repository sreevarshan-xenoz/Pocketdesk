# D4, conversations (design and as built)

Date: 2026-10-19. Plan: `docs/research/2026-10-18-next-cycle.md`, D4.

## Why

RCS cannot be read by any app but the default SMS app, so KDE Connect and Zorin Connect cannot show it (https://forum.zorin.com/t/zorin-connect-rcs-message-reply-support/43590). A messaging app keeps one notification per conversation and updates it as messages come (Android's `MessagingStyle`), so the notification itself carries the conversation. Pocketdesk already answers such notifications (T10c).

## What it does

- **The phone** reads a chat notification's recent messages (`NotificationCompat.MessagingStyle`). It sends them as `messages: [{from, text, time, mine}]` only when all of these hold:
  - the notification goes in full (the lock-screen and app rules unchanged);
  - "Let the computer answer notifications" is on;
  - the notification offers a reply or buttons.
- **What `ConversationRule` sends** (JVM-tested):
  - the newest 10, oldest first, none blank;
  - texts cut at 1000 characters and names at 64;
  - nothing at all when any message, with its sender, looks like a one-time code, or two in a row from one sender do together.
- **The owner's own** are a message with no person, or the style's user (by object, by key, or by name when the app gives no key).
- **A new message** under an unchanged title and text is sent: the thread's digest is part of what is compared with what was last sent.
- **The computer** reads them with limits (`core::protocol::conversation`: invisible characters out, a time before 1973 taken as none). It keeps them on the note it can answer (`phone_notifications.json`, owner-only, the newest 30 notes).
- **The Notifications page** shows a note with messages as a thread ("Ann · 5 min ago: …"), the owner's lines set apart by style, above the existing reply box. Ages come from when the note came on the phone and the spacing between messages, not from the phone's clock.

## Not done

- No thread outside the Notifications page: the toast shows the notification's own text, as before.
- No messages from texts (SMS): those are N5b's, from the phone's own SMS receiver.

## Review

**Fresh review: 0 Critical, 3 Important, 6 Minor.** Fixed one commit each:
- the history went with only the toast agreed to;
- a new message with an unchanged title and text was never sent;
- the code test looked at each text alone.

Minors fixed:
- the phone's clock and times in seconds;
- the owner's lines by name only;
- `mine` for an app that gives the user no key;
- texts cut only on the computer.

Left: the page copies its notes each frame, and a full thread of long messages makes a tall note.

## Testing

- **JVM:** `ConversationRuleTest` (order, blanks, cuts, the code cases, the digest).
- **Rust:** `conversation` (limits, cleaning, times), `answerable` keeping messages, and `thread_lines`.
- **Owed** (checklist §115): a real chat app on the A71 and the thread on the page; a group chat; an app that sets a constant text.
