# O5, notifications shared by their kind as well as their app (design)

Date: 2026-10-22. Plan: `docs/research/2026-10-22-o-cycle.md` §O5. Asked for in KDE bug 508971: a chat app's "Updating messages…" comes to the computer with its messages.

## What

Android files each notification under one of its app's channels: messages, syncing, calls, promotions. The phone's Settings list each shared app's kinds under it once it has posted more than one, each with a switch, on by default. A kind switched off stays on the phone while the app's other kinds go to the computer (`NotificationForwardRule`, its `keptKinds`).

## How a kind is shown

A notification listener cannot read another app's channel names: `getNotificationChannels` is only for companion apps. So a kind is known by:
- its id;
- the title of the last notification seen in it, cut to one line of 48 characters (`NotificationKinds`, JVM-tested). Only what the lock screen shows is kept: the public title for a private notification, none for a secret one.

Each app keeps its 20 most recently seen kinds, on the phone only, in the notification settings' own file.

## Not done

- **The channel's own name.** That needs a companion-device association, which the plan leaves to the C11 spike.
- **Kinds per computer.** They follow the app's own switch, which holds for every computer, L8's copies included.

## Checks

- **Unit tests:** the rule (a kept kind not sent, the app's others sent, one with no channel by its app only) and the kinds' book (order, the cap, examples).
- **On the OPPO:** the fixture app's two channels (checklist §188).
