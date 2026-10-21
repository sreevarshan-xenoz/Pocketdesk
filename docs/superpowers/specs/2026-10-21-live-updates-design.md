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
- **The Notifications page:** "Going on now" lists the live cards with a bar, from `phone_live.json`. The file is owner-only, holds at most 20, and empties at each start. A card leaves it with its cancel, with a later update of its notification that has no `live`, with the setting off, or for a bar after half an hour without word; the page shows only a linked phone's.

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

## As built

- **As designed, with the review's fixes.**
  - **The computer knows each card this run** (`LiveCards`, tested): a card held at first (a pause, the phone in hand), or whose toast failed, shows in full once it may. It counts once whether or not phones' notifications show. A card closed here stays closed (Windows' update finding no toast, a close by the person). An update crossing its cancel, or within a second of the last, is dropped.
  - **The toast's title and text are bound to data with the bar,** so new words reach it.
  - **A timer's words are the phone's own** ("Ends at 14:32", in its clock and locale), so they never freeze.
  - **Only to a computer of this release:** the phone sends live updates only to a computer that sends N3's open (`nativeTakesLive`, from its identity, as C3's ack is). Another computer of mine gets a card's first only.
  - **A held-back update is looked at again as if posted,** so Do Not Disturb and the phone in hand are read anew.
  - **Other fixes:** a change to its buttons still goes between steps; a redacted notification's bar is not sent; Linux keeps the server's own timeout.
- **What the OPPO run showed** (2026-10-21, `n4-live`, after the fixes): the held-back 52 % arrived, then 100 %, and the card went with its end. One event for the card. Checked here: the toast at 10 % updated in place with new words.
- **Limits:**
  - after a restart of the server a card counts and pops once more;
  - Android 16's `ProgressStyle` is read through the same extras, unchecked on a phone that has it.

## Review

A fresh review found 1 Critical, 5 Important and 9 Minor. All were fixed except two:
- the per-tag pace is a second here, not the phone's 5 s;
- a restart's second pop is accepted.

- **Critical:** a card held at first never showed a toast.
- **Important:**
  - the words were not updated;
  - cards went stale;
  - every update counted with phones' notifications not shown;
  - Linux closes and timeout;
  - older computers re-popped.
- **Minor:**
  - rechecks lost the in-hand mark;
  - eviction;
  - the cancel race;
  - no server pace;
  - button changes skipped;
  - redacted bars;
  - an ended countdown;
  - tests;
  - the docs.
