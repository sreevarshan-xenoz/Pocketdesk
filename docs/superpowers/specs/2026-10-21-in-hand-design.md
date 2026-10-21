# M6: no toast while the phone is in hand

Plan: `docs/research/2026-10-21-m-cycle.md`, M6.

## Why

A phone's notification pops up on the computer while the owner is reading it on the phone. KDE's Android merge request 257 (2021, not merged) forwarded notifications only while the screen was off. That loses them on the computer; this keeps them and only skips the pop-up.

## What

- **The phone:** Settings "Quiet on the computer while I use the phone", in the notifications group, off by default. With it on, a notification that comes while the phone is in use is sent marked `phoneInUse: true` (`InHandRule.marks`).
  - **In use means all of these:** the screen is on, the phone is unlocked (a phone with no screen lock counts as unlocked whenever its screen is on), and Pocketdesk's own Remote is not in front. The owner driving the computer from Remote is looking at the computer, whose toasts still show.
  - **Read as it is posted:** the mark is read in `onNotificationPosted`, not when the worker gets to it, by which time the notification may have woken the screen. One that waits to be sent keeps its mark.
  - **No picture:** a marked one goes without its picture (K3), which the computer would not show; its app's icon still goes.
  - **Copies count too:** copies to other computers (L8) carry the mark.
  - **Calls never:** a ringing call's note (F5) is built apart and never marked.
- **The computer:** a notification marked so is held as a paused computer's are (K2a, `held_here`). It is:
  - logged as an event with `phone_in_use`, which the Activity page says ("not shown: the phone was in use");
  - kept on the Notifications page when the computer may answer it (the phone's "Let the computer answer notifications");
  - counted against the phone's limit;
  - shown in no toast or pop-up.

  With the defaults, the Activity page keeps only its app's name, not its text.
- **An older computer** ignores the field, and shows its toast as now.

## Not now

- "Quiet: you are on the phone" on the Overview. The mark is per notification, so no state is kept to show.
- Rules of its own per computer: the switch is the phone's.

## Privacy

Nothing new leaves the phone except one boolean on a notification it already sends: that the phone was unlocked and in use when it came.

## Tests

- **Phone (JVM):** `InHandRule.marks`: the switch, the screen, the lock, Remote in front.
- **Computer:** `phone_in_use` read from a body; `held_here` holding a mark as a mute or a pause; Activity's words.

## On the device (checklist §170)

- The throwaway chat app's notification with the OPPO's screen on and unlocked: kept and logged on the computer, no toast (the event's `phone_in_use`, and the toast log).
- The same with the screen off, or the switch off: a toast.

## As built

- **Phone:**
  - `notify/InHandRule.kt`, with `remoteInFront`, which `ui/RemoteInputScreen.kt` sets while it is resumed;
  - `notify/PhoneNotifications.kt`: `inUse()`, read in `onNotificationPosted`; `handle(…, inHand)`;
  - `Prefs.quietInHand`;
  - the row in `ui/PhoneNotificationsSettings.kt`;
  - strings in English, Spanish and Tamil.
- **Computer:**
  - `server/src/plugins/notification.rs`: `phone_in_use`, `held_here(muted, in_hand)`;
  - `desktop/src/activity_view.rs`: the line.

## Review

A fresh review found 0 Critical, 2 Important and 8 Minor. Each fix is its own commit, except those that share the phone's lines.

- **Important:**
  - Pocketdesk's own Remote counted as in hand, so a computer driven from the phone held its toasts;
  - the words promised the notification was kept to read and answer, which with the defaults it is not.
- **Minor, fixed:**
  - the mark read when the worker got to it;
  - the screen read with the switch off, and twice;
  - the picture sent for nothing;
  - the hold untested, and a test splitting another's doc;
  - Activity's silence about why;
  - the Spanish word.
- **Left:** a held update leaves an earlier toast of the same notification showing its older text, as a pause does.

