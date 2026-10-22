# O1, automation apps start the computer's tasks and press its deck (design)

Date: 2026-10-22. Plan: `docs/research/2026-10-22-o-cycle.md` §O1. Asked for in KDE bugs 512641 ("Intent Listener to Trigger Commands") and 519904.

## What

Two broadcasts an automation app (Tasker, MacroDroid, Samsung's routines) sends to `dev.pocketdesk.android/.automation.AutomationReceiver`:

| Action | Extras | Does |
|---|---|---|
| `dev.pocketdesk.android.action.RUN_TASK` | `ADDRESS` (a saved computer), `NAME` (a task's name) | Starts the task, as the Tasks sheet does |
| `dev.pocketdesk.android.action.PRESS_DECK` | `ADDRESS`, `NAME` (a button's label) | Presses the button, as the deck does |

The extras are named `dev.pocketdesk.android.extra.<NAME>`, as T7b's and 1f's are.

## Rules (`AutomationRule`, JVM-tested)

- **The switch.** The phone's Settings "Let other apps run the computer's tasks" is off by default. It is read before anything an app sent, and with it off nothing more is read or said.
- **What is taken.**
  - Only a saved computer, by its exact address (`ShortcutPlan.savedHost`).
  - A name of 1 to 64 printable characters, matched exactly. Of two buttons with one label, the first is pressed.
- **The pace.** At most 10 asks a minute from all apps together. Refused asks do not count.
- **The link.** A task's start and its result ride the control link (1c review). The link is used when it goes to that computer. With no link, one is brought up only to the phone's own computer (`PhoneLink.target`, the one its background features work with). Otherwise the task is not sent: `linked_elsewhere`, or `not_linked`.
- **The computer's rules hold:** the phone's "Start my tasks" and Input permissions, and ask-first.

## How it went

How it went is told through 1f's events, with "Tell other apps what happens" on:
- **`TASK_FINISHED`** (task, state, code, timed out, reason), as the Tasks sheet tells it. The run is followed by `TaskFollow`, shared with the sheet, for at most 15 minutes.
- **`DECK_PRESSED`** (new: label, state, reason).

Each outcome is also logged under `Automation`.

## Not done

- **No result to the sender.** A broadcast cannot answer it: Tasker's "Send Intent" is not ordered.
- **No new road on the computer:** the computer sees a phone's ordinary task start or press.

## As built and checked

Checked on the OPPO (CPH2359) against a throwaway server through `adb reverse` (checklist §184):
- From a cold app, a task ran within a second, and `TASK_FINISHED` was told done with code 0.
- A task-kind deck button ran, told `DECK_PRESSED` done.
- A button or task that does not exist: `unknown`.
- An unsaved address: `not_saved`. An empty name: `bad_name`. The 11th ask in a minute: `too_many`.
- A task for a computer that is not the phone's own, with no link: `not_linked`.
- With the switch off, nothing ran and nothing was told.

## Review

A fresh review found 0 Critical, 4 Important and 5 Minor, fixed in two commits:

- **Any app could crash the process with an extra Pocketdesk cannot unpack** (Important). The extras were read before the switch. Now the switch is read first, and the extras are read inside a guard.
- **A task's follow had no end** (Important). A link that dropped after "running" brought no end, so the follow went on for the process's life. It now stops after 15 minutes as `no_answer`.
- **A task for a non-linked computer was refused without a reason** (Important). It is now told as `linked_elsewhere` or `not_linked`, and the docs say so.
- **A link brought up for a second computer took the background features with it** (Important). Links are now brought up only to the phone's own computer.
- **Minor:**
  - an unsaved address echoed at any length (now cut to 64);
  - existence probing (SECURITY.md limit 52 says so);
  - a throw from the link's connect now caught;
  - Android's ten seconds on a cold start over the internet road (said in the docs);
  - a stranded comment.
