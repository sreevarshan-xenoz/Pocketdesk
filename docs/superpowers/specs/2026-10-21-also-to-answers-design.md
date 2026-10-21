# L8 part 2: the other computers answer and clear too

Part 1 is `docs/superpowers/specs/2026-10-21-also-to-design.md`.

## Why

Part 1 sent copies of the phone's notifications to the other computers the owner picked, to see but not answer. The phone takes answers and clears only from a computer it has a link with, and it kept one link: to its own computer. A copy therefore had to leave out its buttons and its reply, and a toast closed on another computer stayed on the phone.

## What

**Extra links.** The phone keeps a link to each other computer picked (at most three), besides its own computer's control link, while "Let the computer answer notifications" or "Let the computer clear my notifications" is on:
- The links are on the home road (this network or the tailnet).
- A link that fails is tried again a minute later.
- A computer no longer picked, or the one that became the phone's own, has its link closed.

The bridge keeps them apart from the control link (`extra_links.rs`). On an extra link the phone sends only notifications, their cancels and how an answer went (E1), and it takes only answers and clears (T10c, L1), each kept with the computer it came from. Everything else that computer sends is left alone. The phone works with its own computer only.

**Copies with answers.**
- **To a linked computer:** the copy is sent on its link, whole: buttons, reply, and D4's thread when the notification is answerable.
- **Otherwise:** the copy goes as in part 1, as a one-off connection without what offers an answer.

**Answers and clears from them.** The phone takes them as it takes the own computer's (`fromMine`): under the same switches, rules and budgets. It tells that computer how each answer went, on its link.

**The computer marks such a link.** As it opens, the phone names it as for copies (`kdeconnect.pocketdesk.link_role {role: "copies"}`). The server then keeps it out of its pushes and out of the phones it shows as linked, and sends only answers and clears to the phone's notifications on it (`clone_answer_targets`). Without this, the second computer would count the phone as linked, and its file offers, rings and links would go into a link that drops them.

**Calls too.** A ringing call's note is copied whole on a link, so that computer can silence or decline it, as the own computer can.

## Not now

- The internet road for extra links: they are home-road only, as part 1's one-off copies are.
- Toasts shown only on the computer in use.
- Measuring the background cost of several links (N7).

## Privacy and cost

- **Privacy:** what a picked computer gets is what the own computer gets. SECURITY.md limit 39 now says that a linked computer may also answer and clear the notifications.
- **Cost:** a link per picked computer stays up while answering is on, or for ten minutes after a notification while clearing is on (as the own link, L1), so the phone keeps more links. They cost what the own computer's link costs, which N7 measured. The links are kept on a thread of their own, so a computer out of reach never holds up the own computer's answers.

## Tests

- **Bridge:** `extra_links` refuses a packet type it does not send, a body that is not an object, and a send with no link up.
- **Phone:** `AlsoTo` (part 1), held as before.

## On the device (checklist §165)

- Two throwaway servers, both paired, the second picked, answering on: the second's toast offers the buttons, and pressing one runs it on the phone.
- A toast closed on the second computer is cleared on the phone.
- The second server stopped: its link is tried again a minute later, and copies fall back to one-offs.

## As built

- **Bridge:** `extra_links.rs` (connect with the role, send with a deadline that drops a link cut mid-line, `keep_only`, `live_json`); `notification_answers::forget_from`, so the main link's end keeps the other computers' answers.
- **Server:** `plugins::link_role`, `state::{mark_copy_link, clone_answer_targets}`; `run_answers` and `run_clears` send to the answer targets.
- **Phone:** `PhoneLink.keepOthers` on a thread of its own (close first, then dial, a minute between tries), `copy`/`copyCancel` on a link or as a one-off, `AlsoTo.targets` with one computer at two addresses counted once and never the own one again; `PhoneNotifications.fromMine`, and `tell` on the link the answer came by.

## Review

A fresh review (0 Critical, 5 Important, 8 Minor), fixed:

- **Important:**
  - one computer saved at two addresses got a link to itself, which took its pushes;
  - clearing alone kept links up all day;
  - dials on the answers tick could hold up the own computer's answers and calls;
  - the second computer counted the phone as linked and lost its sends;
  - the words and SECURITY.md still said the others could not answer.
- **Minor, fixed:** a send cut mid-line, the main link's end dropping the others' answers, dialing before closing, closing under the lock, the targets read every tick.
- **Left:**
  - a clear is limited to notifications sent, not to that computer's copies;
  - the answer and clear budgets are shared by all the phone's computers;
  - a copy on a link has no delivery ack (C3's dead-link case), bounded by TCP keepalive.

