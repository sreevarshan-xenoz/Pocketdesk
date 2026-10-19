# F4: an air mouse (2026-10-20)

Plan: `docs/research/2026-10-20-next-cycle.md`. It finishes the Remote (T10i) for presentations and sofas: point the phone, and the pointer follows.

## Design

- **"Air mouse" in Remote's title bar** (phones with a gyroscope) puts a pad where the trackpad was; "Trackpad" puts the trackpad back. The last choice is kept.
- **Held, the pad listens to the gyroscope** (`TYPE_GYROSCOPE`, game rate), only while held. For a phone held flat, its top toward the screen:
  - a turn about its z axis (left and right) moves the pointer across;
  - one about its x axis (tilting up and down) moves it up and down.
- **`stream/AirMouse.kt`** turns rates into whole-pixel moves, tested:
  - a still hand (under 0.03 rad/s) moves nothing;
  - fractions carry;
  - a gap over 0.2 s is a pause.
- **Moves go as the trackpad's do** (`sendMouseEvent` relative), adding up while one is on its way.
- **A press:**
  - its first 120 ms of moves are held back (`AirPress`);
  - a press under 250 ms moving under 40 px is a left click, its held moves dropped.
- **No change on the computer.**
- **Settings "Air mouse speed"** (Input), on the trackpad's range.

## Left out

- **A laser pointer drawn on the computer's screen.** It needs a click-through window there (Windows, a layer-shell surface on Linux); a later item.
- **Holding the phone upright or on its side.** The axes are the flat phone's; the hint says how to hold it.

## Review

A fresh review found 0 Critical and 3 Important issues, fixed together in one commit (they share the pad's code):
- taps too tight to tell from a move;
- moves after a release;
- no backpressure.

It also confirmed the axes for a phone held flat, and noted upright and landscape holds (left out, said in the hint).

Owed (device, with the owner): checklist §129.
