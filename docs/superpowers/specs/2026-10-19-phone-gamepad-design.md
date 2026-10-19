# E4, a gamepad on the phone (design)

Date: 2026-10-19. Plan: `docs/research/2026-10-19-next-cycle.md`, E4.

## Why

Two KDE threads (March 2026) ask to play on the computer from the phone. The gamepad packet has existed on both ends since the start (input tag 5, a snapshot of six axes and sixteen buttons), mapped to keys and the mouse on the computer, but no phone sends it (roadmap 2695).

## What it does

- **An on-screen controller:** "Gamepad" in the session menu opens a controller over the stream:
  - a stick on the left (the mouse, as now);
  - a D-pad under it (the arrow keys);
  - A, B, X and Y on the right;
  - LB and RB at the top;
  - Select and Start in the middle;
  - each button labelled with what it does on the computer.

  Close (×) ends it, and so does Back. It works upright or on its side.
- **A real controller connected to the phone** (USB or Bluetooth) sends the same packet while the session is in front, its buttons, D-pad (a hat or keys) and both sticks. Settings "Game controller to the computer" turns this off (on by default).
- **Sending:**
  - The packet goes on each change.
  - While a stick is off its centre, the packet is sent again 30 times a second, since the computer moves the mouse for each packet it gets.
  - While a send is still going (a stalled link), a repeat waits, so the moves do not pile up and play at once later.
  - All buttons up and the sticks centred are sent when the controller closes, the session's view goes, a controller is disconnected, or the window loses focus (the shade, another app, a call), so nothing stays held.
- **The computer's mapping, with what is new:**
  - A is Enter, B is Esc, X is Space (new), Y is Tab (new).
  - LB and RB are Page Up and Page Down (new).
  - Start is the left click; Select is Backspace (new).
  - The D-pad is the arrows.
  - The left stick moves the mouse, and the right stick scrolls (new): a notch a packet from a third of its travel, two from two thirds.
  - The phone's on-screen labels and the README say the mapping.
- **Held to the same rules as all input:** the Input permission, view-only, one phone driving. A gamepad's held buttons are already let go when control moves (N7).

## As built

- **Server:** `GAMEPAD_BUTTONS` gains X, Y, LB, RB and Select; `gamepad_scroll` turns the right stick into wheel notches. Releases come from the same edges as before, so control moving and a session ending let the new keys go too.
- **Phone:** `GamepadState` (buttons pressed and the hat's kept apart, sticks with a 15 % dead zone, `moving`), `GamepadOverlay` (a stick, D-pad, A/B/X/Y, bumpers, Select and Start, each 48 dp and labelled; touches between them stop at the overlay), `ConnectedGamepad` (the activity's keys and generic motion from a gamepad or joystick source, while a session takes them), and the session's send and repeat (`sendGamepadOrdered`, in order with the other input). The bridge already sent the packet (`nativeSendGamepadEvent`).
- **The activity** overrides `dispatchKeyEvent`, the framework's public method, with lint's restricted-API check suppressed for androidx's own override of it.

## Review

**Fresh review: 0 Critical, 4 Important, 8 Minor.**

- **Fixed, one commit each:**
  - a held stick was never let go when a controller went or the app lost focus, so the pointer kept moving: now let go on a device removed and on focus lost;
  - a controller's centred hat let go of a D-pad held as keys or by touch: the hat's bits are kept apart;
  - a touch between the controls reached the computer through the stream: the overlay takes it;
  - repeats piled up behind a stalled link and played at once: a repeat waits for the last send.
- **Minors fixed:** the wheel's two speeds, keys under 48 dp, two stops per key for a screen reader, two doc comments out of place, the setting's name in this spec.
- **Left as they are:**
  - the gamepad works in a view-only session and the computer drops it, unsaid;
  - a controller is taken while the session's screen is composed (its error card, picture-in-picture);
  - closing the on-screen gamepad lets go of a controller's held buttons too;
  - pads that report the right stick as `AXIS_RX`/`AXIS_RY` do not scroll;
  - enigo's wheel ignores its direction (from before E4).

## Not done

- A virtual game controller on the computer (Windows needs a driver, ViGEmBus; set aside in the plan).
- Choosing the mapping.
- Triggers (L2, R2): the packet carries them, and the computer maps nothing to them yet.

## Testing

- **Pure, tested:**
  - the controller's state from the on-screen pad, and from a real controller's keys and axes (`GamepadState`, JVM): each button's bit, the hat as the D-pad, a dead zone on the sticks;
  - when a packet goes: on a change, again while a stick is off centre, and all up at the end;
  - the computer's new mappings and their releases (the edge test covers every mapped bit), and the right stick's scrolling.
- **Here:** the on-screen pad from the A71 against the throwaway server, into a scratch window that logs the keys (input off for anything else).
- **By hand (checklist §123):** a game; a real controller (none here); Linux.
