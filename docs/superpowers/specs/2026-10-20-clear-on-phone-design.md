# Cleared on the computer, cleared on the phone (L1)

**Date:** 2026-10-20. **Branch:** `feat/clear-on-phone`. The L cycle's first item (`docs/research/2026-10-20-l-cycle.md`).

## What is wrong

- **A phone notification closed on the computer stays on the phone.** The other direction works: one closed on the phone closes here (N5c part 1). But the phone never cancels a notification, and nothing asks it to.
- **Asked for:** KDE bug 400595, KDE's forum (2024–2025), the KDE Connect AMA.

## What changes

- **What the computer counts as cleared:**
  - **A toast closed with its ✕** (Windows: the toast's `Dismissed` with `UserCanceled`, while the server runs).
  - **Clear on the Notifications page,** beside each notification's answers.
  - **A Linux notification the person closed** (`NotificationClosed` with reason 2).
- **How it reaches the phone:** the clear goes through `<state>/notification_clears/` (one file each, as the answers' outbox), and the server sends KDE's own `kdeconnect.notification.request {cancel: id}` to that phone if it is connected within a minute. It needs the phone's Notifications permission and desktop Settings "Clearing here clears it on the phone" (`clear_on_phone`, on).
- **What the phone does:** with Settings "Let the computer clear my notifications" on (on by default), the notification listener takes the request and calls `cancelNotification(key)`, through `ClearRule` (JVM-tested), only when:
  - the request comes from its own computer;
  - the notification is one it sent there;
  - Android still shows it, and it is clearable;
  - it is not a call (F5);
  - fewer than 30 have been cleared in the last minute.

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
- **On the device (checklist §156):**
  - the throwaway chat app posts a notification;
  - a request file asks to clear it;
  - `dumpsys notification` no longer lists it.
  - **By hand:** the toast's ✕.
