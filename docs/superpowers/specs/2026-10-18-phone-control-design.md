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
   - At most 200 moves, scrolls and texts a second are taken; more are dropped. A press or release always goes.
   - A batch that waited on the phone longer than 2 s is dropped. The age is the phone's own clock, since the computer's differs.
   - A finger left down is lifted when the cast ends, when control goes off, when a new press comes, or after 2 s without an event.
   - Nothing is touched or typed while the phone is locked, so the computer cannot enter its PIN; Back and Home still go.

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

## As built

As above, with these differences and additions:
- **The relay tells the viewer the phone's control state only once control is on.** It says it again each heartbeat while on. A desktop app from before C8 cannot read the message, so a cast without control never sends it one, and the phone's first word is only ever "on".
- **A replaced viewer** (another joined, or a new cast) can no longer reach the phone.
- **Typing:**
  - It runs on a thread of its own, not the main one, and adjacent text in a batch is typed at once.
  - Each edit reads the focused field fresh: this service listens for no text events, so Android's cached copy could still hold the text before the last edit.
  - A password field, whose text Android hides from the service, is typed from what was typed into it here.
- **The window:**
  - It sends one move a pass, the newest, so a fast mouse does not send hundreds a second.
  - A release the window never saw (focus went mid-drag), or control turned off mid-press, lifts the finger where it last was.
  - The bar's buttons give up the keyboard after a click, so Enter and Space go to the phone.
  - Control is written outside the window's lock and stops at the first broken write.
  - Nothing is sent while the picture still has the old shape after the phone turned.
- **The service** listens for no events. Its description says Android lets it read the screen, and that Pocketdesk reads only the field it types into.
- **The Settings hint** says how to allow restricted settings for a sideloaded app on Android 13+.
- **Not built:**
  - an Advanced Protection check (the Settings text says Android's Advanced Protection does not allow the service);
  - the window's bar under screen readers: the second window has no AccessKit tree, an owed gap.

## The device run

On the A71 against a throwaway server on this box. The cast rode the internet road through n0's relay (`wan_relayed`): the phone's saved address for it was 127.0.0.1, and the cast's QUIC cannot use `adb reverse`. So every number below includes a relay round trip, the slowest path a cast takes.

Checked through a test client acting as the window (same relay messages):
- Home, Recents, Back and the shade were performed.
- A tap opened Settings' search, and "wifi" was typed into its field.
- Backspace and Enter worked; "wifiz", Backspace, "!" gave "wifi!" after the fix below.
- A drag scrolled the Settings list.
- The wheel scrolled it up and down.
- A long press opened a home-screen icon's menu (App info, Widgets).
- With the service turned off, the phone told the computer within a second, and its notification went back to "shown".

**The window itself, on this box:** it showed the phone's screen with its bar (Back, Home, Recents, Watch only). Clicks into it are owed by hand: messages posted to egui windows do not reach them, and the window has no UI Automation tree.

**Latency:**
- **From a click to the first changed frame:** 40 to 75 ms from a still screen. This covers the relay, the gesture, Android drawing it, the phone's encoder and the relay back.
- **To the first large frame of a transition:** 85 to 200 ms, most of it Android's own animation.
- **The transport alone** (window to phone) could not be separated: the two clocks, set against each other through adb, agreed only to about ±30 ms.

**Again after the review's fixes (the same run):** Home, Back, a long press, the wheel to the top and a drag down the list, and "wifiz", Backspace, "!" giving "wifi!", with no crash.

**Found on the device run, fixed:**
- A Backspace soon after typing undid the typing (the cached field).
- A desktop app from before C8, watching the same state folder, reconnected twice a second without showing the picture.

## Review

**Fresh review (0 Critical, 7 Important, 9 Minor), fixed one commit each:**
- the rate limit could drop a release and leave a finger down;
- nothing lifted the finger when the cast ended;
- a replaced viewer kept a path to the phone;
- password fields, and typing on the main thread;
- stale batches replayed;
- an "off" reaching older desktop apps;
- a locked phone's PIN reachable from the computer.

**Minors fixed:**
- a press left down on the desktop;
- the bar's buttons taking Enter;
- writes under the window's lock;
- a dangling stroke after a lost release;
- moves unbounded while a segment ran;
- the service's events and description;
- the restricted-settings hint;
- the rotation transient;
- the runner reading the main thread's state.

**Left as is:**
- On an old server, the phone's notification can say "controlled" though nothing will come. The server drops the phone's state as a malformed cast stream.
- `cast_control` off is the desktop app's choice. Any process of the owner's with the cast token can send control while the phone allows it (SECURITY.md).

