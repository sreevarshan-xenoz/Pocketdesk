# L8 part 1: every computer of mine

Plan: `docs/research/2026-10-20-l-cycle.md`, L8.

## Why

The phone sends its notifications and texts to one computer: the paired computer it used most recently. With two computers (a desktop and a laptop, home and work), the other one sees nothing. Phone Link has the same limit, with one live PC at a time, and KDE users with two computers have asked for both.

## What (part 1)

The phone's Settings → "This phone's notifications" gains "Other computers of mine". It lists every other paired computer, each with its own "Also to <name>" switch, off by default.

**The phone's own computer** (`PhoneLink.target`) still gets everything as before, through the same path, its waits (C3) and its link.

**Each computer picked** also gets a copy of every notification, cancel and text the phone sends, at most 3 of them (`AlsoTo`, JVM-tested):
- **Delivery:** copies go on a worker of their own, so a computer out of reach never holds up the phone's own. Each is sent as a one-off, or on a link if one is up. A computer that fails is paused for a minute, and a copy that fails is dropped rather than waited.
- **No duplicates:** a notification sent again after a wait (C3) is copied only once (`AlsoTo.key` leaves out `waited`).
- **See, not answer:** the phone takes answers and clears from its own computer only, so a copy leaves out what would offer one (`actions`, `requestReplyId` and the conversation's `messages`, by D4's rule).
- **Closing:** a notification closed on the phone closes on every computer that got a copy, because its cancel is copied too. One closed on the phone's own computer (L1) closes on the phone, and so on the others.

The computers need no change: a copy is an ordinary notification from a paired phone, held to that computer's own Notifications permission and limit.

## Not now (part 2)

- Several background links at once, which would let each computer answer and clear. This needs the bridge to keep more than one control link.
- The battery, screenshots and other features for each computer.
- Toasts only on the computer in use, with the others keeping copies on their Notifications page.
- The background cost measured again (N7) with two computers.

## Privacy

The owner picks which computers get copies, and each copy goes only to a computer paired with this phone. This is in SECURITY.md beside the notifications' limit.

## On the device (checklist §164)

- Two throwaway servers, both paired, the second picked: a notification reaches both, and a cancel closes it on both.
- The second server stopped: the first still gets every notification on time.
- The second server shows no answer buttons.
