# Not from this app (K2b)

**Date:** 2026-10-20. **Branch:** `feat/mute-app`. The K cycle's second item, in two parts: this one first, then pausing everything for a while (K2a).

## What is wrong

- **The only per-app choice is on the phone:** its list of apps whose notifications go to a computer. That list is the same for every computer the phone reaches.
- **One mail shows twice.** With the same mail app on the computer and the phone, each mail shows twice: once from the computer's own app, once from the phone. KDE's forum asked for this, 2026-08-26 and 2026-08-20.
- **Silencing that app means picking up the phone** and changing a list that is not about this computer.

## What changes

- **"Not from this app"** on each phone notification's toast (Windows: a button; Linux: an action where the notification server offers them) and on each phone notification's row on the Activity page.
  - It mutes that app's notifications from phones on this computer, from the next one; a toast already shown stays until it goes. A toast press says what it did and where to unmute.
  - It is offered only for an app the phone named: a nameless notification's fallback ("Phone") stands for all of them.
  - The phone's own list is left alone.
- **What a muted app's notification still does:**
  - it is logged as an event (hooks, Home Assistant and Activity see it, Activity marking it "muted here");
  - one that can be answered is kept for the Notifications page;
  - it is not shown as a toast, and it never counts against the phone's limit of 20 a minute.
- **Settings, "Notifications and texts":** "Muted apps" lists each with Unmute. KDE Connect devices' notifications follow the same list.
- **The list:** `muted_apps.json` in the state folder, owner-only, changed under a lock (the desktop app and the server both write it), at most 200 apps, matched by the app's name as the phone gives it, case and spaces at either end ignored (`core::local::muted_apps`, tested).

## Left out

- Muting one app on one phone only: the same app is the same nuisance from each.
- Muting by keyword or sender: one request, and a pattern list needs its own design.

## Testing

- **Unit:**
  - the list: mute, unmute, a name matched whatever its case, the cap, a broken file read as empty and never written over;
  - the toast's XML with and without the button;
  - a press read as a mute.
- **Server:** `muted_here` for a muted app whatever its case; only a given name is muteable; each kind of toast keeps its own places.
- **Harness:** Activity's "Not from this app" mutes, and Settings lists it with Unmute.
- **By hand (checklist §150):** the toast's button on Windows; a Linux notification server with actions.

## As built

As designed, with the review's fixes. The list is `core::local::muted_apps`. The server's check is `muted_here`, and the button is `toast_answers::MUTE` on Windows' toasts and D-Bus' actions. The desktop's parts are Activity's row and Settings' "Muted apps", which Find a setting reaches.

## Review

A fresh review found 0 Critical, 3 Important and 7 Minor.

**Important, fixed one commit each:**

- **A muted app's burst used up the phone's limit** and dropped the next app's message.
- **Plain toasts pushed answerable ones out of their thirty places,** leaving their buttons dead. Each kind keeps its own places now.
- **The list's two writers raced,** sharing one temp file. Each change now holds a lock and writes owner-only.

**Minor:**

- Fixed:
  - fallback names muteable;
  - no word after a toast press;
  - a broken file shown as none;
  - Linux remembering notifications that show no button;
  - the spec's words.
- Left:
  - Activity's muted marks follow a mute from a toast within its second's read, not at once;
  - `muted_here` reads the small file on the packet loop.
