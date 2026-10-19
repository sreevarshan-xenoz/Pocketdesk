# The next cycle: the phone as a deck, a camera and a status screen (2026-10-20)

Date: 2026-10-20, after the 2026-10-19 cycle (`docs/research/2026-10-19-next-cycle.md`). E1–E6 are done; E7, the owed checks here, goes on with the owner by day.

## Inputs

**What people use phones for beside a computer in 2026:**
- **A macro pad, a "Stream Deck":** Macro Deck (open source; its companion app was updated in October 2026), Deckboard, Touch Portal, MATRIC. Each is a separate app with its own server, and none rides a link the phone already keeps with its computer.
  - https://alternativeto.net/software/deckboard/
  - https://apkcombo.com/macro-deck-companion/app.macrodeck.companion/
- **The phone's camera for the computer:**
  - **As a webcam:** KDE Connect's task T4661 has been open since 2016, and its forum asked again ("Kde connect as a webcam"). Phone Link offers it on Windows 11.
  - **To take a photo or a scan onto the computer:** KDE bug 394547.
  - https://phabricator.kde.org/T4661
  - https://discuss.kde.org/t/kde-connect-as-a-webcam/17174
  - https://bugs.kde.org/show_bug.cgi?id=394547

**What the repo shows:**
- **The pieces a deck needs already run on the computer, but each is its own road:**
  - named tasks (1c);
  - the media controls (T8c);
  - links (T8a);
  - key combinations, only from inside a session (the shortcut bar).
- **The phone shows the computer's battery** (the HUD) and nothing else of its state. It cannot see the CPU, memory, disk or uptime without opening a session.
- **The phone's camera reaches the computer only through Screenshots (T10h) or a share,** never asked for from the computer.

**The owner's rules for this night (2026-10-20):**
- brainstorm and build on autopilot;
- fast tests and the gates per change, no device run per change;
- the device checks batched for the owner and run together.

The standing rules hold:
- spec, then TDD;
- one logical change per commit;
- a fresh review per item;
- docs with each change;
- knobs in Settings on both sides;
- no macOS or iOS;
- no power, lock or server-stop actions on the owner's computer.

## Order

| # | Item | Size | Checkable without a device |
|---|---|---|---|
| F1 | The deck: buttons the owner sets up on the computer (keys, a task, media, a link, text), pressed on the phone, done on the computer without a session | M | The computer's half by tests; the phone's by JVM tests |
| F2 | The computer's state on the phone: CPU, memory, disks, battery, uptime, in a card and a home-screen widget | S–M | Yes (the computer's readings by tests) |
| F3 | Take a photo with the phone: the computer asks, the phone's camera opens, the photo lands on the computer and its clipboard | S–M | The computer's half |
| F4 | An air mouse: the phone's gyroscope moves the pointer in Remote, with a laser pointer for presentations | S–M | JVM tests of the motion |
| F5 | Calls: the phone's ringing call shown on the computer, with Silence and Decline | S–M | JVM tests; a SIM for the rest |
| F6 | The phone as a webcam (a virtual camera on Windows, v4l2loopback on Linux) | L | Partly |

**Why this order:**
- **F1** brings four existing actions under one button. It is the most used of these apps, and nothing new runs on the computer.
- **F2** is small and asked of every companion app.
- **F3** is small, and the oldest camera request.
- **F4** finishes the Remote for presentations.
- **F5** waits on a SIM to be checked.
- **F6** is the biggest, last.

## F1. The deck

**Done:**
- **The buttons:** `[[deck]]` in config.toml, each with a label and one action:
  - `keys`, a combination such as `ctrl+shift+m`;
  - `task`, a named task;
  - `media`: play/pause, next or previous;
  - `link`, an http(s) link;
  - `text`, typed.
- **Where they are set:** the desktop app's Settings, a "Deck" section.
- **The phone:** lists them (`kdeconnect.pocketdesk.deck`) and presses one by id (`deck_press`). The computer does it under the phone's own permissions:
  - keys and text need Input;
  - a task needs Tasks;
  - media needs Audio;
  - a link needs Links.

  A refusal says why.
- **On the phone:** Deck on each computer's menu, a grid of buttons, and Settings "Deck".

## F2. The computer's state on the phone

**Done:**
- the computer answers `kdeconnect.pocketdesk.system`:
  - CPU %, memory used/total, each fixed disk's used/total, battery, uptime;
  - read with `sysinfo` on both systems;
- a card in each computer's menu on the phone, refreshed while open;
- a home-screen widget for the last computer;
- a new per-phone permission is not needed: the battery is already shared with every paired phone. Settings "Share this computer's state" on the computer, on by default.

## F3. Take a photo with the phone

**Done:**
- **The computer:** "Take a photo with the phone" on the Phone page and the tray, a request (`camera_out/`) sent to a linked phone with Files on.
- **The phone:** a notification "Take a photo for <computer>" opening the camera (Android forbids opening it from the background). The photo is shared to the computer as a file. The computer puts it on its clipboard as an image too (Settings).

## F4. An air mouse

**Done:**
- in Remote, "Air mouse": the gyroscope's rotation moves the pointer, held to a button so the phone at rest moves nothing;
- a "Laser" mode draws a dot on the computer's screen instead of moving its pointer (Windows: a click-through window; Linux: a layer-shell surface where there is one);
- sensitivity in Settings.

## F5. Calls

**Done:**
- the phone tells its computer of a ringing call (the number, a contact's name when "Names from my contacts" is on), and of its end;
- the computer shows it with Silence (the ringer) and Decline (`ANSWER_PHONE_CALLS`), each its own permission, off by default.

## F6. The phone as a webcam

**Done (if the night reaches it):**
- the phone's camera, encoded as the cast's screen is, sent on a camera session;
- on Windows, a virtual camera through Media Foundation (`MFCreateVirtualCamera`, Windows 11);
- on Linux, v4l2loopback.
