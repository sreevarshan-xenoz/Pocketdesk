# N4: the phone's ongoing updates on the computer

Plan: `docs/research/2026-10-21-n-cycle.md`, N4.

## Why

- **What a progress bar or a timer on the phone says never reaches the computer.** `NotificationForwardRule` drops every ongoing notification: a download, an upload, a timer, a ride on its way, a delivery.
- **Android 16's Live Updates** (`Notification.ProgressStyle`, with OPPO among the partners) and macOS Tahoe's Live Activities from the iPhone show these where the person is looking.

## What

### The phone

- **The switch:** Settings "Ongoing updates on the computer", off by default, under the notifications' settings. It covers the apps whose notifications are shared, so it is chosen per app through the list that exists. Its summary says that a navigation or delivery app's updates say where the phone is.
- **What is sent** (`LiveRule`, JVM-tested): an ongoing notification of a shared app showing progress (`EXTRA_PROGRESS_MAX` above 0, or `EXTRA_PROGRESS_INDETERMINATE`) or a running timer (`EXTRA_SHOW_CHRONOMETER`). Never:
  - Pocketdesk's own;
  - a media player's (`EXTRA_MEDIA_SESSION`; N2 has it);
  - a call (`CATEGORY_CALL`; F5 has it);
  - an ongoing note with neither progress nor a timer;
  - against the lock screen's rule, as any notification (a secret one is not sent; a private one goes as its public version).
- **How often:**
  - the first at once;
  - then when the progress moved 5 % of its whole, the title or text changed, or a timer started, stopped or changed;
  - at most one every 5 s;
  - a change held back by the 5 s is sent once they pass, so the last state always arrives;
  - its end goes as any notification's cancel.
- **The packet:** the notification's own `kdeconnect.notification`, its buttons and N3's open as for any other, with `live: {progress, max, indeterminate, chronometer, countdown}`. `chronometer` is the timer's base in Unix ms, and `countdown` says whether it counts down.

### The computer

- **The setting:** "Ongoing updates from phones" (`phone_live_updates`), on, applies at once. With it off, a live notification is not shown or kept.
- **One card per notification that updates in place:**
  - **Windows:** the first is a toast with a progress bar bound to data (`{progressValue}`, `{progressStatus}`) under the notification's tag. Each later one goes through `ToastNotifier.Update` with `NotificationData`, which changes the toast without showing it again. A toast the owner closed stays closed.
  - **Linux:** the notification's `replaces_id`, with the `value` hint (0 to 100) for a determinate bar.
  - **What the status says:** "42 %", or "Started 5 min ago" or "8 min left" for a timer, worked out when each update comes.
- **The flood guard and the events:** a card counts against the per-minute limit once, at its first update. The `notification` event is written only then.
- **The Notifications page:** "Going on now" lists the live cards with a bar, from `phone_live.json`. The file is owner-only, holds at most 20, and empties at each start; a card leaves it with its cancel, or with its phone once that phone is no longer connected.

## Not in it

- A live card on the Overview or the tray.
- A Linux server without the `value` hint shows the status in the text only.

## Safety

- **SECURITY.md limit 48:** with the switch on, a shared app's ongoing progress, timers and their text go to the own computer while they last. That includes where a navigation or delivery app says the phone is.
- **The flood guard** still holds a burst of new cards.

## Tests

- **Phone:** `LiveRule` (what is live, and when it is due) on the JVM.
- **Core:** reading `live`.
- **Server:**
  - the status words;
  - the first update counted and the rest not;
  - the store kept and pruned.
- **Desktop:** the page's section.
- **On the OPPO, by the runner:** the fixture's progress notification at 10, 50 and 100 % reaching `phone_live.json`, its last state arriving, and its end clearing it.
