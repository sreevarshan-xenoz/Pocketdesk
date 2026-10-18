# Control the phone from the computer (C8)

Date: 2026-10-18. Plan: `docs/research/2026-10-17-next-cycle.md` §C8. The owner's priority for this cycle: "the most interesting one", and the one needing the most testing and optimisation.

## The problem

"Show screen" (T10k) puts the phone's screen in a window on the computer, but the window is view only. To answer a message or tap through an app you still pick the phone up.

Other ways are partial:
- scrcpy controls a phone, but only over adb, with USB debugging on.
- Phone Link's app mirroring works only on some phone brands.
- KDE has an open request for mirroring with control without adb.

Android gives an app one way to act on the whole screen without adb or root: an Accessibility service that may perform gestures (`dispatchGesture`) and global actions (`performGlobalAction`: Back, Home, Recents, the notification shade).

## What changes

### The phone

1. **An Accessibility service, "Pocketdesk control"** (`control/ControlService`).
   - It may perform gestures and read the focused field (for text), and nothing more. It retrieves no other window content and listens for no events.
   - It is not marked as an accessibility tool, because it is not one. Under Android 17's Advanced Protection it is refused, and Settings says so (`AdvancedProtectionManager`, where present).
2. **Settings, "Let the computer control this phone"** (off by default).
   - Turning it on opens Android's Accessibility page at the service, with a line saying what it allows.
   - The row says whether the service is on.
3. **Control only during a cast,** and only from the computer the phone is casting to.
   - The phone tells that computer it can be controlled once both the setting and the service are on (`[FA 02][flags]`, a cast message).
   - The computer sends control only after hearing that, so an older phone app never receives a stream it does not know.
   - The cast notification says "Shown and controlled on <computer>" while control is on.
4. **What the computer can do:**
   - **Touch:** down, move and up at a point given as a fraction of the screen, mapped to the phone's real screen.
     - A drag is one continued gesture (`StrokeDescription.continueStroke`).
     - Moves that arrive while a segment is still being performed are joined into the next segment, so the finger follows the mouse without queueing up behind it.
     - A press and release with no move between them is one tap. A long press is a long hold.
   - **Scroll:** a short swipe at the pointer, one notch at a time, in the wheel's direction.
   - **Global actions:** Back, Home, Recents, the notification shade.
   - **Text:** typed into the focused field (`ACTION_SET_TEXT`) at its selection. Backspace deletes before the selection. Enter is the field's action (`ACTION_IME_ENTER`, Android 11+).
5. **Limits on the phone side:**
   - At most 200 events a second are taken; more are dropped.
   - An event older than 2 s is dropped.
   - A finger left down is lifted when the cast ends or after 2 s without an event.

### The wire

- **Phone → computer:** `[FA 02][flags u8]` on the cast session (bit 0: control on). The server forwards it to the desktop app as a new relay message, `CONTROL_STATE`.
- **Computer → phone:**
  - The app sends `CONTROL` messages to the relay; until now its only message was `WANT_KEYFRAME`.
  - The server sends each on the cast session's connection as a stream of a new kind, `STREAM_KIND_CAST_CONTROL`, one stream per batch.
  - The encoding is `core::streaming::cast_control`: a tag and fixed fields, with coordinates as `u16` fractions of the picture, and text as UTF-8 with a length.
  - It is sent only while the phone's last word was "control on".
- **Gates:**
  - The server drops control for a cast whose phone has not said control is on.
  - The relay drops control from any client but the one viewer.
  - The phone drops control unless its setting and service are on.

### The computer

1. **The cast window takes input** while control is on:
   - The primary button presses, drags and releases.
   - The wheel scrolls.
   - The right button is Back, and the middle button is Home.
   - Typing goes to the phone's focused field; Esc is Back.
   - A bar holds Back, Home and Recents, and a Control switch (on by default) to watch without touching.
   - Without control, the window says how to turn it on on the phone.
2. **Desktop Settings, "Control phones from their screen window"** (`cast_control`, on). Off, the window stays view only.

## Latency

Each hop is kept short:
- the window sends on loopback with Nagle off;
- the relay forwards at once;
- the server opens a stream per batch;
- the phone takes events with a blocking read, not a poll;
- moves are joined while a segment runs.

**The measurement**, on the A71 against this computer:
- **Transport:** the time from an event leaving the window to the phone performing it, from the two clocks set against each other through adb.
- **Click to picture:** the time from a tap to the first frame that shows its effect.
- Both are taken over the home network and the internet road, and the results go in this spec.

## Not in this change

- Hardware keys (volume, power).
- Clipboard paste through control: the clipboard already crosses.
- Multi-touch: pinch is a two-finger gesture, possible later.
- Control without a cast.

## Tests

- **Core:** the control encoding round-trips, and malformed input is refused.
- **Server:**
  - control reaches a cast client over loopback only after the phone said it is on;
  - the relay passes control from its viewer and drops anything else.
- **Desktop:** window points are mapped to fractions through the letterbox, and points outside it are not sent.
- **Phone (JVM):**
  - `ControlPlan`: the tap or drag decisions and the joining of moves;
  - the mapping to real pixels in each rotation;
  - the text edit at a selection;
  - the stale and rate limits.
- **Device (A71, a throwaway server):** taps, a drag (a list scrolled), a long press, the wheel, Back, Home, Recents, text into a field, the switch off, control refused with the setting off, Advanced Protection where available, and the latency numbers.
