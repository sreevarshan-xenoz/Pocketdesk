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

## As built

- **The fixture** is as above. Its exported activities and receiver take only callers holding `DUMP`, which adb's shell has and no app can.
- **The runner** keeps to the throwaway server:
  - it refuses a root with no paired server, the owner's state or config folder (new and legacy names), the owner's control port, a server already running for the root, and an `adb reverse` already in place;
  - it turns the phones' input off in one move before the server starts, counts the server as started once it logs "Control listener ready", and stops the process it started in a `finally`;
  - a mirror older than 10 s links no one.
- **Each refusal is proved by a control.** `m4-locked` types once the phone is unlocked and needs that to arrive; `m10-sensitive` needs a plain copy after it to arrive.
- **Notifications** are matched by the fixture's app and the phone's id, after the event log's line count.
- **The tel check** reports the phone's own answer from its log: Pocketdesk's `Dials` now logs "opened" or "offered" as well as a refusal, never the number.
- **The control service** is waited for after each screen read by its own "control service connected" in logcat.
- **The run on the OPPO** (2026-10-21, ColorOS 14, after the fixes), 7 of 7:

  ```
  PASS     m4-typing        §171   read back 'Héllo தமிழ!'
  PASS     m4-locked        §171   nothing typed while locked; typed once unlocked
  PASS     m10-copy         §175   copy-… reached the computer
  PASS     m10-sensitive    §175   not sent; a plain copy after it was
  PASS     m6-in-hand       §170   held: phone_in_use
  PASS     m6-locked        §170   shown
  PASS     m8-tel           §173   chose the linked phone, whose answer was: was not taken: Off
  ```

  Calls from the computer was off, so the dial was refused, and the check says so.
- **Found on the way:** an APK built with a stale bridge library failed the typing and dial checks ("No implementation found" in logcat). `docs/development-setup.md` §5b now says so.

## Review

A fresh review found 0 Critical, 6 Important and 9 Minor. All were fixed:

- **Important:**
  - a server already running was reused with its input on, and never stopped;
  - a copied config could point the phone at the owner's server;
  - the server's process outlived a failed start;
  - the tel check passed on a refusal;
  - the locked and sensitive checks could pass on a dead link;
  - any app's notification matched.
- **Minor:**
  - the fixture's exported parts open to any app;
  - its notification permission unchecked;
  - `run-as` failures unseen;
  - the owner's legacy and config folders;
  - a swipe on an unlocked phone;
  - one phone and one id assumed (`--serial`, `--phone-id`);
  - an existing `adb reverse` replaced;
  - a fixed sleep for the control service;
  - no word on making a server root.
