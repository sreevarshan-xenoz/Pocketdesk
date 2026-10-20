# The phone's volume keys for the computer (K4)

**Date:** 2026-10-20. **Branch:** `feat/volume-keys`. The K cycle's fourth item (`docs/research/2026-10-20-k-cycle.md`).

## What is wrong

- **The phone's volume keys change only the phone.** In Remote, or in a sound session where the phone is the computer's speaker, turning the computer up means:
  - in Remote, finding "Vol +" on the screen;
  - in a sound session, opening the Sound sheet's slider.
- **Others have this.** KDE bug 398848 asked for it, and KDE Connect 1.35.10 added a switch for "remote volume control using volume keys".

## What changes

- **While Remote is admitted, the phone's volume keys change the computer's volume,** with the phone's screen off too.
- **How:** a `MediaSession` whose playback is remote, with an absolute `VolumeProvider`, as casting apps do. Android's own volume panel shows it, named after the computer, in 20 steps of 5 %.
  - A key press is one step.
  - Dragging the panel's slider sets the volume.
  - Either goes through the computer's audio request (`setVolume`, the Audio permission), as the Sound sheet's slider does.
  - The panel starts at the computer's volume as its `status` reports it.
- **The session lives while the remote is admitted,** and is released when it ends or the screen leaves.
  - Android gives the keys to a call while one rings or is held.
  - A phone whose computer refuses the Audio permission keeps its own keys: the panel is never offered.
- **Settings "Volume keys change the computer's volume"**, under Session, on by default.
- **The rule:** `VolumeKeys`, JVM-tested.
  - from the computer's percent to a step, and back;
  - one press's step, held between 0 and 20.

## Left out

- The sound session (K4 review): there the phone is the computer's speaker, and its own keys set what the owner hears. A remote session playing beside the phone's own sound would take those keys, and the headset's buttons, from it.

- Mute from the keys: Android has no mute key event for a remote session.
- Screen sessions: their picture already takes the whole phone, and its own keys there are the computer's by the session's keyboard.

## Testing

- **Unit:** `VolumeKeys`.
- **On the device (checklist §150):**
  - `adb shell input keyevent 24` and `25` during Remote against the throwaway server. Its log shows each `setVolume`. The throwaway server sets this computer's volume, so the run steps up once and back down, with the owner's OK only.
  - The panel's name, the screen off, and the setting off.
