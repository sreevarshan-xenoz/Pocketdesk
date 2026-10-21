# L8 part 1: every computer of mine

Plan: `docs/research/2026-10-20-l-cycle.md`, L8.

## Why

The phone sends its notifications and texts to one computer: the paired computer it used most recently. With two computers (a desktop and a laptop, home and work), the other one sees nothing. Phone Link has the same limit, with one live PC at a time, and KDE users with two computers have asked for both.

## What (part 1)

The phone's Settings → "This phone's notifications" gains "Other computers of mine", after the app list. It lists every paired computer, each with its own "Also to <name>" switch, off by default. The computer in use is listed too: its pick counts whenever another computer is the one in use. Picks are kept by the computer's paired id, never its address, and at most four can be on (the computer in use and three more).

**The phone's own computer** (`PhoneLink.target`) still gets everything as before, through the same path, its waits (C3) and its link.

**Each computer picked** that is not the one in use also gets a copy of every notification the phone makes ready, and of its cancel. At most 3 computers get copies (`AlsoTo`, JVM-tested):
- **When:** a copy is taken where the notification is made, after its filters (the app's opt-in, the lock screen's privacy, one-time codes, Do Not Disturb), not where it is sent. It therefore goes at once even while the phone's own computer is away and its notifications wait (C3).
- **Delivery:** copies go on a worker of their own, so a computer out of reach never holds up the phone's own.
  - Each copy is a one-off connection to the computer's saved address, on this network or the tailnet, never over the internet (iroh).
  - A computer that fails is paused for a minute, and a copy that fails is dropped rather than waited.
  - Each one-off can show briefly on that computer as the phone linking and leaving.
- **See, not answer:** the phone takes answers and clears from its own computer only, so a copy leaves out what would offer one (`actions`, `requestReplyId` and the conversation's `messages`, by D4's rule).
- **Closing:** a notification closed on the phone closes on every computer that got a copy, because its cancel is copied whenever the notification was, whatever the own computer saw of it. One closed on the phone's own computer (L1) closes on the phone, and so on the others. A ringing call's note is copied (without Silence or Decline), and so is its end.
- **Icons:** an app's icon goes with the first notification the own computer gets from that app, so another computer gets one only when it is in that copy.
- **Texts stay with the own computer.** A copy would carry a reply box there, and a reply typed on another computer would be kept by it and sent once it became the computer in use, perhaps a day later.

The computers need no change: a copy is an ordinary notification from a paired phone, held to that computer's own Notifications permission and limit.

## Not now (part 2)

- Several background links at once, which would let each computer answer and clear. This needs the bridge to keep more than one control link.
- The battery, screenshots and other features for each computer.
- Toasts only on the computer in use, with the others keeping copies on their Notifications page.
- The background cost measured again (N7) with two computers.

## Privacy

The owner picks which computers get copies, by their paired id, and each copy goes only to a computer paired with this phone. What a copy carries is what the own computer gets, without answers. SECURITY.md limit 39.

## On the device (checklist §164)

- Two throwaway servers, both paired, the second picked: a notification reaches both, and a cancel closes it on both.
- The own server stopped: the second still gets each notification at once.
- The second server stopped: the first still gets every notification on time.
- The second server shows no answer buttons.
- The same chat posting, being cleared and posting again: the second is cleared on both.
- The computer in use swapped: the picks still mean the same computers.

## As built

- **Phone:** `notify/AlsoTo.kt` (`targets` by paired id, `mayPickMore`; JVM-tested), `PhoneLink.copy` and `copyCancel` on a worker of their own (`copyTargets`, `sendCopies`), called from `PhoneNotificationListener.handle` and `onNotificationRemoved` and from `CallWatch`; Settings in `ui/PhoneNotificationsSettings.kt`.
- **Computer:** no change.

## Review

A fresh review (1 Critical, 4 Important, 9 Minor), fixed in two commits:

- **Critical:** a text's copy lost its messages (they were dropped as D4's thread), so no text reached another computer. Texts now stay with the own computer.
- **Important:**
  - copies were taken where a notification is sent, which with the own computer away happened only for the first;
  - a cancel went only for a notification the own computer had seen, and a reused key's second cancel was taken for a duplicate;
  - the computer in use was hidden, so picks changed meaning;
  - a reply typed on another computer could be sent a day later.
- **Minor, fixed:** whole bodies kept as keys (gone with the dedup); the cost of looking up the targets on every retry (now once per notification); a queue of 40 dropping cancels (100); picks by address; picks past three ignored; the group before the app list.
- **Left:** icons rarely reach the other computers; copies go on this network or the tailnet only, and each shows briefly there as a link.

