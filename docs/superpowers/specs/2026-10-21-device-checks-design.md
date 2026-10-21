# N1: owed checks that run by script

Plan: `docs/research/2026-10-21-n-cycle.md`, N1. This is part 1; part 2 grows the list.

## Why

- **Each cycle's device checks are run by hand.** That means adb commands typed each time, a throwaway chat app made again (checklist §115, §120, §164), and prefs edited by hand.
- **`docs/owed-checks.md` lists 837 open checks, 509 runnable here.** Nothing runs any of them.
- **The owner's rule batches device checks.** A batch that runs by itself costs a command, not an evening.

## What

- **A fixture app,** `android/fixture/`: its own Gradle module and package (`dev.pocketdesk.fixture`), with no dependencies. It is never part of Pocketdesk's APK. It has:
  - `FieldActivity`: a text field that fills the screen, focused, with an optional starting text;
  - `CopyActivity`: puts the text it is given on the clipboard once it has focus, optionally marked sensitive, then closes;
  - `NotifyReceiver`: posts a notification on an adb broadcast. It may be plain, a chat with Reply and "Mark as read" (`MessagingStyle`), or a progress note. The answers are written to logcat (`FixtureReply`).
- **A runner,** `scripts/device_checks.py`:
  - **The computer side:** starts a throwaway server from a folder holding `state/` and `config/`. That server must already be paired with the phone: `--server-root`, made once by pairing by hand. It runs with the clipboard seam, makes `adb reverse` from the phone's saved port to the throwaway's, and waits until the phone is linked.
  - **The phone side:** it never force-stops Pocketdesk, since on ColorOS that turns its Accessibility service off. A check whose switch is off on the phone is SKIPped with what to turn on.
  - **Waits:** each one has a deadline and reports TIMEOUT.
  - **Results:** a line per check, `PASS`, `FAIL`, `SKIP` or `TIMEOUT` with what was seen, and an exit code: 0 with no FAIL or TIMEOUT.
  - **The end:** it stops the throwaway server it started and removes the `adb reverse`.
  - **Options:** `--list` names the checks; `--only ID…` runs some.
- **Part 1's checks**, those run by hand on 2026-10-21:

  | Id | Checklist | What it checks |
  |---|---|---|
  | `m4-typing` | §171 | text, a Backspace and Tamil typed into the fixture's field |
  | `m4-locked` | §171 | nothing typed while the phone is locked |
  | `m10-copy` | §175 | the fixture's copy reaches the computer's clipboard seam |
  | `m10-sensitive` | §175 | a copy marked sensitive does not |
  | `m6-in-hand` | §170 | a notification from the fixture marked `phone_in_use` with the phone unlocked and awake |
  | `m6-locked` | §170 | one without the mark while it is locked |
  | `m8-tel` | §173 | `pocketdesk tel` chooses the linked phone and the dial reaches it |

## Left out (part 2)

- Pairing a fresh throwaway by script (it needs taps on the phone's confirmation).
- The answer, clear, file, link, battery and clipboard checks of C to L.
- A Linux run.

## Safety

- **This computer:** only the throwaway server runs, from its own folders, with the phone's input off in its trust store. The runner refuses a `--server-root` that is the owner's state folder. It never stops the owner's server, and never sends input to this computer.
- **The phone:** it is a test phone (the OPPO). The runner wakes it and puts it to sleep.

## Tests

- **The runner's pure parts** (parsing the mirror, the event log and `uiautomator`'s dump; the refusal of the owner's folder) are tested with `python -m unittest scripts/test_device_checks.py`, in CI's Rust job beside the owed checks' index.
- **The fixture:** `assembleDebug` and `lintDebug` build it.
- **On the OPPO:** the runner's own first run is the check, and its output goes in the as-built section.
