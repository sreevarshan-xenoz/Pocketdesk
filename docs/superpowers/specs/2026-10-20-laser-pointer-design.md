# G4: a laser pointer for the air mouse (2026-10-20)

F4's air mouse moves the computer's pointer. For a talk, a presenter wants a dot the room sees without moving the pointer, which may be resting on a slide's link or hovering a menu. F4 left the laser out; this adds it.

## What the owner sees

**On the phone:**
- In Remote, with the air mouse on, a "Laser pointer" switch sits under the pad.
- With it on, pressing the pad shows a red dot on the computer's screen, and turning the phone while it is held moves the dot instead of the pointer. The dot stays while the pad is held still; letting go hides it. A tap still clicks, at the pointer, where the slide software listens.
- The switch is offered only when the computer says it can draw the dot.

**On the computer:**
- **The dot:** a red dot, 24 px per 1080 lines of its screen, drawn above every window and never taking a click.
- **Where it starts:** in the middle of the screen the pointer is on.
- **How it moves:** as the pointer would, held to the whole desktop.
- **When it goes:** when the pad is let go. While the pad is held, the phone says the dot is there each second; a dot unheard of for 2 s (its release lost) is hidden but keeps its place.
- **The setting:** desktop Settings "Laser pointer from phones" (Phones section, on by default).

## The wire

**A new input packet,** `InputPacket::Laser { dx, dy, show }` (tag 16, 6 bytes):
- `dx` and `dy` are the motion, in the pointer's pixels;
- `show: false` hides the dot.

The packet:
- **is input,** so it is dropped in view-only and needs Input, as a mouse move does;
- **goes to the input task,** so it follows who drives (N7) as the pointer does;
- **in a session without a picture,** is not dropped as "about the picture".

**A Remote's ready stream now carries one byte** (`REMOTE_FEATURES_SAID | REMOTE_FEATURE_LASER`), as a screen session's carries the pen (E3):
- **When the laser bit is set:** the phone's input is taken, the setting is on, and this computer can draw the dot.
- **Older computers** send no byte and read as offering no laser.
- **Older phones** read the ready stream without its payload.

## Where the dot is drawn

**On Windows, `server/src/laser_windows.rs`.** A thread owns a small layered window:
- `WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`;
- an elliptic region, a red brush, and 90 % alpha.

**What the thread does:** it moves and sizes the window with `SetWindowPos`, never activating it, and hides it when told to or after 2 s unheard of. Hidden, it wakes every 250 ms to pump its window's messages; a hide before any dot starts no thread.

**Elsewhere:** not built yet. A wlroots layer-shell dot, the privacy shield's road, is left for later. The bit stays off there, so the phone does not offer the switch.

**The dot's state** is in `server/src/laser.rs`, pure and tested:
- its position;
- the motion clamped to the desktop;
- when it hides.

## Left out

- Linux.
- A colour or size choice.
- The dot in a screen session's own picture: it is drawn on the screen, so a screen session sees it as anyone in the room does.

## Review

A fresh review found 0 Critical and 2 Important issues. Each was fixed in its own commit:
- **The dot vanished while the presenter held still to aim.** The computer hid a dot still for 2 s, and the phone sends nothing while the gyroscope is still. The next nudge then brought the dot back in the middle of the screen. Held, the phone now shows the dot at once and says so each second, and a dot unheard of for 2 s keeps its place.
- **The hide sent on letting go could hide the next press's dot** when the pad was pressed again before the hide went out. A press now drops a pending hide.

Minors fixed in one commit:
- the dot sized to its screen;
- no thread for a hide before any dot;
- the hidden thread's wake-ups from every 50 ms to every 250 ms.

Minors left:
- a window that could not be made still leaves the switch offered (one warning in the log);
- the dot stays up to 2 s after control moves to another phone;
- an older computer logs a warning per laser packet. It is never sent one, since the switch needs the ready byte.

Owed (device, with the owner): checklist §135.
