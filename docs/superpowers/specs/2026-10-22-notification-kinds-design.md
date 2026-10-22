# O5, notifications shared by their kind as well as their app (design)

Date: 2026-10-22. Plan: `docs/research/2026-10-22-o-cycle.md` §O5. Asked for in KDE bug 508971: a chat app's "Updating messages…" comes to the computer with its messages.

## What

Android files each notification under one of its app's channels: messages, syncing, calls, promotions. The phone's Settings list each shared app's kinds under it once it has posted more than one, each with a switch, on by default. A kind switched off stays on the phone while the app's other kinds go to the computer (`NotificationForwardRule`, its `keptKinds`).

## How a kind is shown

A notification listener cannot read another app's channel names: `getNotificationChannels` is only for companion apps. So a kind is known by:
- its id;
- the title of the first notification seen in it, cut to one line of 48 characters (`NotificationKinds`, JVM-tested). Only what the lock screen shows is kept: the public title for a private notification, none for a secret one, none that looks like a one-time code, and nothing from a notification that never leaves the phone.

Each app keeps its 20 newest kinds, on the phone only, in the notification settings' own file; a kind kept on the phone is never pushed out, so it keeps its row. Unsharing an app forgets its titles.

## Not done

- **The channel's own name.** That needs a companion-device association, which the plan leaves to the C11 spike.
- **Kinds per computer.** They follow the app's own switch, which holds for every computer, L8's copies included.

## Checks

- **Unit tests:** the rule (a kept kind not sent, the app's others sent, one with no channel by its app only) and the kinds' book (order, the cap, examples).
- **On the OPPO:** the fixture app's two channels (checklist §188).

## Review

A fresh review found 0 Critical, 3 Important and 5 Minor issues.

**Important, fixed:**
- A kind kept on the phone could fall out of the list of 20 and lose its row, staying off for good. A kept kind is now never pushed out, and one missing from the list still gets an untitled row (`NotificationKinds.listed`).
- Titles were stored without the filters that sending applies. A one-time code and a notification that never leaves the phone now keep no title.
- An app's titles outlived its being shared. Unsharing it now forgets them.

**Minor, fixed:**
- A write on nearly every notification. A known kind now stays where it is, keeping its first title.
- A waiting notification of a kind kept since then still went. The listener now knows each notification's kind and holds it.
- An update of a notification the computers were shown, once its kind is kept, left the old text there. It is now closed on them.
- The kind rows sat at their app's depth, and named no app for TalkBack. They are now nested, and the detail line names the app.

**Minor, not done:** no JVM test for the prefs layer itself, which needs an Android context. Its cap rule is the pure `seen`, which is tested.
