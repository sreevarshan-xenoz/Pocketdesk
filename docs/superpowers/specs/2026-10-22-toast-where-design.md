# N7: toasts only on the computer in use

Plan: `docs/research/2026-10-21-n-cycle.md`, N7.

## Why

- **Every computer of mine toasts (L8).** With the phone's notifications going to a desktop and a laptop, both pop up the same message, and the one across the room lights up for no one.
- **Both L8 specs left "only where I am working" out.** Phone Link and KDE Connect have nothing for it either; Apple's continuity shows a notification on the device in use.

## What

### The computer

- **The question:** a paired phone asks `kdeconnect.pocketdesk.in_use {}` on any link, the copies-only one included (L8 part 2). It is held to the Notifications permission.
- **The answer:** `in_use_answer {inUse}`, by D9's reading of how long the keyboard and mouse have been quiet: in use when input came in the last two minutes. It answers `{}` when it does not say: desktop Settings "Tell phones when this computer is in use" (`tell_in_use`, on) is off, or it cannot tell (Linux, for now).
- **A computer that would not show a phone's notification** (paused, or phones' notifications off) says nothing, so it is never the one "in use" while the others hold.
- **A notification marked `elsewhere`** is held as one that came while the phone was in hand (M6): kept, counted, logged with `in_use_elsewhere` (Activity says "not shown: another computer was in use"), no pop-up.

### The phone

- **The switch:** Settings "Toast only on the computer I'm using", off by default, under the notifications' settings.
- **Asking:** with it on, the listener asks the own computer and each other computer linked every 15 s, in the ten minutes after a notification went, keeping a link to each picked computer meanwhile, as clears do (L1). The first notification after a quiet spell therefore goes to every computer as it would without the switch.
- **Marking (`ToastWhere`, JVM-tested):** a notification to a computer is marked `elsewhere` as it goes when that computer said, within 30 s, that it is not in use, and another said, within 30 s, that it is.
  - A computer that has not answered lately, or does not say, is never marked.
  - With no computer known to be in use, every one toasts.
  - The own computer's notification and each copy are marked by the same rule.

## Not in it

- Linux's idle reading (`ext-idle-notify`): a Linux computer does not say, so it always toasts.
- Moving a held notification's pop-up to a computer once it is used again.

## Safety

- **SECURITY.md limit 51:** a phone of the owner's, with its switch on, learns every 15 s whether each of the owner's computers had input in the last two minutes; nothing more (no times, no apps). Off on the computer, it says nothing.

## Tests

- **Core:** the answer by the quiet's length, and nothing when off or unknown.
- **Desktop:** Activity's line.
- **Bridge:** each computer's last answer kept with it.
- **Phone:** `ToastWhere` on the JVM.
- **Owed on devices:** two throwaway servers and the OPPO, the computers' input stood in for (checklist §181).

## As built

- **As designed, with the review's fixes.**
  - A computer paused, or with phones' notifications off, says nothing.
  - An answer is forgotten when its link ends, or its computer is no longer kept linked.
  - The asks run on a thread of their own, only in the ten minutes after a notification.
  - A ringing call's copy is never held.
- **Limits:** a computer in use with an app muted (K2b) still counts as in use, so the others hold that app's notifications too; a live card (N4) whose own computer's mark changes mid-card keeps its first toast.

## Review

A fresh review found 0 Critical, 4 Important and 6 Minor. Fixed:

- **Important:**
  - a paused or not-showing computer in use silenced all;
  - answers outlived their links;
  - asks blocked the answers tick;
  - links and asks all day.
- **Minor:** calls held on copies, words claiming the Notifications page for all, the docs (the protocol map, the copies link's comments, SECURITY.md's limit), and tests for the hold and the answer.
- **Left:** the live card's mark, said above.

