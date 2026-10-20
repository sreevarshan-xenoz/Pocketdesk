# The phone's volume keys for the computer (K4)

**Date:** 2026-10-20. **Branch:** `feat/volume-keys`. The K cycle's fourth item (`docs/research/2026-10-20-k-cycle.md`).

## What is wrong

- **The phone's volume keys change only the phone.** In Remote, or in a sound session where the phone is the computer's speaker, turning the computer up means:
  - in Remote, finding "Vol +" on the screen;
  - in a sound session, opening the Sound sheet's slider.
- **Others have this.** KDE bug 398848 asked for it, and KDE Connect 1.35.10 added a switch for "remote volume control using volume keys".

## What changes

- **While Remote is admitted, the phone's volume keys change the computer's volume,** with the phone's screen off too.
- **How:** a `MediaSession` whose playback is remote, with an absolute `VolumeProvider` and a callback (without one the session drops the keys), as casting apps do. Android's own volume panel shows it in 20 steps of 5 %, labelled as Android chooses (likely "Pocketdesk").
  - A key press is one step.
  - Dragging the panel's slider sets the volume.
  - Either goes through the computer's audio request (`setVolume`, the Audio permission), as the Sound sheet's slider does.
  - **One request at a time:** presses made meanwhile are summed into the next, so a held key never leaves a backlog.
  - **From the computer's own volume:** read again when the last reading is over 3 s old, so a change from the Sound sheet or the computer is not jumped back. The panel takes the computer's answer, or its reading after a refusal.
  - The panel starts at the computer's volume as its `status` reports it, asked up to three times.
- **The session lives while the remote is admitted,** and is released when it ends or the screen leaves.
  - While a call is on (any call audio mode), the session pauses, so the keys stay the call's. Android 12+ follows calls as they start and end; older phones are judged as the remote starts.
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

- **Unit:** `VolumeKeys`: the steps, a press, presses summed, a reading's freshness, a call keeping the keys.
- **On the device (checklist §150):**
  - `adb shell input keyevent 24` and `25` during Remote against the throwaway server. Its log shows each `setVolume`. The throwaway server sets this computer's volume, so the run steps up once and back down, with the owner's OK only.
  - The panel's name, the screen off, and the setting off.
  - Phones that refuse remote volume without a MediaRouter2 session (Android 14+, `config_volumeAdjustmentForRemoteGroupSessions` off, as on Pixels): the keys stay the phone's there.

## As built

As designed, with the review's fixes. The rule is `media/VolumeKeys`, and the session is `media/ComputerVolume`, started from Remote's screen. Not yet run on a phone: the OPPO was away when it was built.

## Review

A fresh review found 1 Critical, 5 Important and 6 Minor.

**Critical, fixed:** the session had no callback, so Android handed it the keys and it dropped them. The phone's own keys did nothing while Remote was open.

**Important:**

- Fixed:
  - presses from a stale copy jumped the volume;
  - a held key left a backlog, each request its own connection in Remote;
  - the sound session took the phone's own keys and the headset's buttons (it is Remote only now);
  - calls.
- Left to the device check: whether a phone's build refuses remote volume.

**Minor:**

- Fixed: one failed first reading ended it, and the spec's words.
- Not a change:
  - the setting read at start, since Settings is another screen and leaving Remote ends the session;
  - the strings' place in the file.
