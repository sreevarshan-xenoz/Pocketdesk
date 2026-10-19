# The next cycle: answers you can trust, a lighter tray, the phone as a pen (2026-10-19)

Date: 2026-10-19, after the 2026-10-18 cycle (`docs/research/2026-10-18-next-cycle.md`). D1–D9 are done; D10 (calls) waits for the owner and a SIM.

## Inputs

**What users of these apps asked for in 2026** (forum threads and Q&A; single reports, not surveys):
- **KDE's forum:**
  - gamepad emulation (two threads, March: "Make KDE Connect simulate game controller inputs");
  - using the phone as a drawing tablet (March);
  - microphone sharing (January; Pocketdesk has it since N5);
  - an intent listener for automation (Pocketdesk has T7b and 1f);
  - a landing folder for files (C2).

  https://discuss.kde.org/tag/kdeconnect
- **Phone Link's complaints:**
  - message sync that stalls;
  - notifications 5–10 s late;
  - **replies from notifications that never reach the phone, with nothing said** (Microsoft Q&A, April and September 2026; Eleven Forum). https://techcommunity.microsoft.com/discussions/app-dev/phone-link-fails-to-sync-properly/4553137, https://www.elevenforum.com/t/start-menu-and-phone-link-questions.43187/
- **LocalSend** plans a Windows share-sheet entry and an Android Quick Settings tile (its changelog). Pocketdesk has the tile; the share sheet needs a package identity, which C9 set aside.

**What the repo shows:**
- An answer to a phone notification (T10c, N3) is sent and forgotten. The phone runs it, but never says whether the app took it, so a failed reply looks like a sent one, which is Phone Link's complaint.
- **D7 measured the desktop app at 77–85 MB private at rest,** all of it egui and the OpenGL driver; the app stays running only for its tray. A tray of its own, the window started on demand, would leave a few MB at rest.
- **The gamepad packet** exists on both ends (mapped to keys on the computer, `GamepadAction`), but no phone sends it (roadmap 2695).
- **The input path** has touch, a trackpad and keys, but no pen: no pressure, no tilt.
- About 300 owed checks can run here (C6's index); C6 ran the first batch.

**UI:** every cycle polishes both apps (the owner's standing rule). D3–D9 added pages and rows on both sides: the Clipboard page, threads on the Notifications page, the drive on the Phone page, the tray menu, Lock and the drive in Settings. None has had a design pass since.

The owner's standing rules hold:
- spec, then TDD; one logical change per commit;
- a fresh review per item, its findings fixed one commit each;
- device checks on the A71 (time-consuming tests batched at the end);
- docs with each change;
- knobs in Settings on both sides;
- no macOS or iOS;
- no power, lock or server-stop actions on the owner's computer.

## Order

| # | Item | Size | Checkable here |
|---|---|---|---|
| E1 | Answers that say how they went: the phone reports each answer's outcome to its computer (sent, the app refused, it expired), shown on the Notifications page and in the toast | S | Yes (the throwaway chat app) |
| E2 | The tray on its own: a small tray process with no window, the app's window started on demand and closed when hidden | M | Yes (Windows; Linux by the WSL stand-in) |
| E3 | The phone as a pen: pressure and the pen's tilt from the phone's screen, as pen input on the computer (Windows' synthetic pointer pen, Linux uinput tablet) | M | Partly (the A71 has no stylus; injected pen into a scratch window here) |
| E4 | A gamepad on the phone: an on-screen controller sending the existing gamepad packet | S–M | Partly (keys, into a scratch window) |
| E5 | The desktop app's design pass: the D-cycle pages and rows, spacing, empty states, dark mode, words | S–M | Yes (the C4 harness and snapshots) |
| E6 | The phone app's design pass: Settings' new rows, Home's menu, large fonts and TalkBack on them | S–M | Yes |
| E7 | The owed checks that run here, batch 2 | S | Yes |

**Why this order:**
- **E1** answers the complaint with the most behind it, and is small.
- **E2** follows D7 straight on.
- **E3–E4** are the two input requests with threads behind them. The pen is the bigger and newer; the gamepad finishes what exists.
- **E5–E6** polish what the cycle and the last added.
- **E7** spends the rest of the time on proof.

**Set aside:**
- the Windows share sheet (it needs a package identity; C9);
- a virtual game controller on Windows (it needs a driver, ViGEmBus);
- D10 calls (the owner, with a SIM).

## E1. Answers that say how they went

**Why:** Phone Link's 2026 complaint: replies from notifications that never reach the phone, with nothing said.

**Done:**
- the phone answers each reply or button press it ran with its outcome (`kdeconnect.pocketdesk.notification_answered {key, ok, reason}`): sent; the notification is gone; the app refused (`PendingIntent.CanceledException`); too many;
- the computer keeps it with the note and shows it on the Notifications page ("Sent", "Not sent: the app refused it"), and in a toast when it failed;
- an answer with no outcome within a minute says so ("No word from the phone").

## E2. The tray on its own

**Why:** D7: the app holds 77–85 MB private at rest, all the window's (egui, OpenGL), and it runs at rest only for the tray.

**Done:**
- a small `pocketdesk-tray` (Windows and Linux) holds the tray icon and D5's menu, reading the same state files and writing the same requests;
- Open, Pair and Send file… start the window (`pocketdesk-app --page …`), or raise it when it runs;
- the window, closed, exits instead of hiding, when the tray process runs; it hides as before when started alone;
- login starts the tray, not the window;
- measured before and after with `scripts/memory_at_rest.ps1`.

## E3. The phone as a pen

**Why:** KDE's forum asks for the phone as a drawing tablet (March 2026). Pocketdesk's input has touch and a trackpad, but no pressure.

**Done:**
- a "Pen" input mode in the session: each touch a pen stroke with its pressure (`MotionEvent.getPressure`, a stylus's tilt when it has one), sent as a new input packet;
- Windows: a synthetic pointer device of type pen (`CreateSyntheticPointerDevice(PT_PEN)`, `InjectSyntheticPointerInput`, Windows 10 1809+, no driver);
- Linux: a uinput tablet (`ABS_PRESSURE`, `BTN_TOOL_PEN`);
- the pressure curve in the phone's Settings.

## E4. A gamepad on the phone

**Why:** two KDE threads (March 2026); the gamepad packet already exists on the computer, mapped to keys, but no phone sends it (roadmap 2695).

**Done:**
- an on-screen controller in the session's menu (a pad and four buttons, landscape), sending the gamepad packet;
- the computer's key mapping as now, said in Settings;
- a real game controller connected to the phone passed through the same packet.

## E5 and E6. The design passes

**Done:**
- **Desktop:** each page audited against U1/U5's rules (headings, spacing, empty states with their next step, notices, both themes at 4.5:1), the D-cycle additions above all; snapshots updated.
- **Phone:** Settings' new rows grouped and worded as U4's; the card's menu; font scale 2.0 and TalkBack on the new screens, as C5.

## E7. The owed checks that run here, batch 2

**Done:** the next batch from `docs/owed-checks.md`'s "here" list, by script on the A71, its findings fixed.
