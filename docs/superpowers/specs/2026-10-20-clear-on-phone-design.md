# Cleared on the computer, cleared on the phone (L1)

**Date:** 2026-10-20. **Branch:** `feat/clear-on-phone`. The L cycle's first item (`docs/research/2026-10-20-l-cycle.md`).

## What is wrong

- **A phone notification closed on the computer stays on the phone.** The other direction works: one closed on the phone closes here (N5c part 1). But the phone never cancels a notification, and nothing asks it to.
- **Asked for:** KDE bug 400595, KDE's forum (2024–2025), the KDE Connect AMA.

## What changes

- **What the computer counts as cleared:**
  - **A toast closed with its ✕** (Windows: the toast's `Dismissed` with `UserCanceled`, while the server runs). Closing the banner counts too, so a notification closed to read later goes from the phone and the Action Center alike; Settings says so.
  - **Clear on the Notifications page,** beside each notification's answers.
  - **A Linux notification the person closed** (`NotificationClosed` with reason 2), but not one closed within three seconds of a button or a reply: GNOME closes those itself.
- **How it reaches the phone:** the clear goes through `<state>/notification_clears/` (one file each, as the answers' outbox), and the server sends KDE's own `kdeconnect.notification.request {cancel: id}` to that phone if it is connected within a minute. It needs the phone's Notifications permission and desktop Settings "Clearing here clears it on the phone" (`clear_on_phone`, on).
- **What the phone does:** with Settings "Let the computer clear my notifications" on (on by default), the notification listener takes the request and calls `cancelNotification(key)`, through `ClearRule` (JVM-tested), only when:
  - the request comes from its own computer;
  - the notification is one it sent there;
  - Android still shows it, and it is clearable;
  - it is not a call (F5);
  - fewer than 30 have been cleared in the last minute.
- **The link it rides:** the phone takes a clear whenever its link to the computer is up, and keeps a link up for clears only within ten minutes of sending a notification.

## Left out

- **The Action Center's "Clear all":** an app without a package identity hears no event for it (C9 was skipped). Settings says so.
- **Every computer at once (L8):** a phone's notifications go to one computer today.
- **KDE devices' notifications:** the request goes only to Pocketdesk phones.

## Testing

- **Unit:**
  - the clears' outbox (a request kept, taken once, stale ones dropped);
  - the packet's body;
  - `ClearRule` (JVM): own computer, sent, clearable, a call, the budget, the setting.
- **Server:**
  - the toast's tag maps back to its phone and notification;
  - a clear goes to a connected phone with Notifications on, and not with it off.
- **Harness:** the Notifications page's Clear writes the request.
- **On the device (checklist §156; K5 is §155):**
  - the throwaway chat app posts a notification;
  - a request file asks to clear it;
  - `dumpsys notification` no longer lists it.
  - **By hand:** the toast's ✕.

## As built

- **Core:** `core::local::note_clears` (the outbox), `notification_action::{NOTIFICATION_REQUEST, cancel_body, cancel_key}`.
- **Server:** `notification.rs`: `shown_notes` (tag to phone and id), `cleared_here`, `run_clears`; the toast's `Dismissed` handler and the plain toasts kept alive for it (`windows.rs`); the Linux `NotificationClosed` listener. The identity announces the request it sends.
- **Desktop:** the Notifications page's Clear; Settings `clear_on_phone`.
- **Bridge:** the clear kept with the answers.
- **Phone:** `ClearRule` (JVM-tested), `PhoneNotificationListener.clear` on the worker, Settings "Let the computer clear my notifications".

## Review

A fresh review (0 Critical, 4 Important, 9 Minor), fixed one commit each:

- **Important:** GNOME's close after a button or reply cleared the phone too, racing the answer; the request went unannounced; every phone sharing notifications kept a link all day for clears; a banner's ✕ clearing everywhere (now said in Settings; its by-hand check owed).
- **Minor, fixed:** the setting not governing the page's Clear, the phone's decision off its worker, every active notification read per clear, a stale toast kept, a closed notification re-kept, the page's notice, the Spanish word.
- **Left:** a Linux notification server restarting can reuse ids (the listener does not hear its restart); a clear that fails to send is dropped; whether a ✕ inside the Action Center raises `Dismissed` is unverified.
