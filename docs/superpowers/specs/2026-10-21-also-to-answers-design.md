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

**No computer change.** An extra link is an ordinary control link from a paired phone.

## Not now

- The internet road for extra links: they are home-road only, as part 1's one-off copies are.
- Toasts shown only on the computer in use.
- Measuring the background cost of several links (N7).

## Privacy and cost

- **Privacy:** what a picked computer gets is what the own computer gets. SECURITY.md limit 39 now says that a linked computer may also answer and clear the notifications.
- **Cost:** a link per picked computer stays up while answering or clearing is on, so the phone keeps more links. They cost what the own computer's link costs, which N7 measured.

## Tests

- **Bridge:** `extra_links` refuses a packet type it does not send, a body that is not an object, and a send with no link up.
- **Phone:** `AlsoTo` (part 1), held as before.

## On the device (checklist §165)

- Two throwaway servers, both paired, the second picked, answering on: the second's toast offers the buttons, and pressing one runs it on the phone.
- A toast closed on the second computer is cleared on the phone.
- The second server stopped: its link is tried again a minute later, and copies fall back to one-offs.
