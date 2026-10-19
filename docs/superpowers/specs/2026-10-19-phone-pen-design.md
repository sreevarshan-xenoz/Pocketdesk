# E3, the phone as a pen (design)

Date: 2026-10-19. Plan: `docs/research/2026-10-19-next-cycle.md`, E3.

## Why

KDE's forum asks for the phone as a drawing tablet (March 2026). Pocketdesk's input has direct touch and a trackpad, both a mouse to the computer: no pressure, so a drawing app draws every stroke at one width.

## What it does

- **On the phone:** a third input mode, **Pen**, beside Direct touch and Trackpad (the mode chip and the session menu), offered only when the computer says it takes a pen.
  - A finger or a stylus on the stream is a pen in contact: where it is (as direct touch maps it, letterbox, zoom and a window crop included), its pressure and, for a stylus, its tilt and its button.
  - A stylus held above the screen is a pen hovering (the cursor follows it, nothing drawn).
  - A second finger pinches the view, as in Direct touch, and lifts the pen first.
  - A stylus's hover end waits 80 ms for its touch, so a stroke does not leave range just before it starts.
  - Pressure through a curve, from Settings "Pen pressure": Soft (more ink for a light touch), Even, Firm. A finger's pressure on most phones is a guess from its contact size, so Settings says a stylus draws better.
- **On the wire:** a new input packet, tag 15, `Pen {x_norm, y_norm, pressure, tilt_x, tilt_y, flags}`: position as `MouseMoveAbs` (0..=65535 over the picture), pressure 0..=65535, tilt in degrees (-90..=90), flags for in contact, in range (hovering) and the barrel button. An older computer drops an unknown tag and carries on.
- **The computer says it takes a pen:** a screen session's ready stream (`STREAM_KIND_READY`, until now sent only on sessions with no picture), sent as the session is admitted, carries a feature byte, `0x80` (said) with bit 0 for a pen, set only for a phone with the Input permission. An older phone reads a ready stream on a screen session and ignores its bytes.
- **Windows:** a synthetic pointer device of type pen (`CreateSyntheticPointerDevice(PT_PEN)`, `InjectSyntheticPointerInput`, Windows 10 1809+, no driver), made at the first pen packet and kept for the session; position in the virtual screen's pixels after the multi-monitor remap. Where it cannot be made, or Windows takes none of its first 8 samples, the pen is a mouse: moves, and the left button for contact.
- **Linux:** a uinput tablet ("Pocketdesk Virtual Pen": `ABS_X`/`ABS_Y`, `ABS_PRESSURE`, `ABS_TILT_X`/`ABS_TILT_Y`, `BTN_TOOL_PEN`, `BTN_TOUCH`, `BTN_STYLUS`), built with the other uinput devices and rebuilt on a fault as they are. Without uinput (wlroots' virtual pointer, the portal, enigo), the pen is a mouse.
- **Held to the same rules as a mouse:** the Input permission, view-only, one phone driving (`input_control`); a pen in contact is lifted when control moves to another phone or the session ends, as a held button is.
- **Settings:** the phone's "Pen pressure". The computer has nothing to set: a pen is input, under Input.

## Not done

- A stylus's eraser end on Linux (the packet carries it and Windows' pen passes it on as `PEN_FLAG_ERASER`; the uinput tablet has no eraser tool yet).
- Every sample Android batches: each event sends its newest, so a stylus's in-between samples are dropped.
- Palm rejection beyond Android's own.
- A pen on input-only sessions (the remote has no picture to draw on).

## Testing

- **Pure, tested:**
  - the packet's encoding and its exact length; an unknown flag ignored;
  - the ready stream's feature byte as the server makes it (with and without Input) and as the phone reads it, and that a screen session's ready stream is read, never decoded;
  - the pressure curve, the tilt, the stroke's states from touch events, a hover's end before a touch, and the modes offered (`PenStroke`, JVM);
  - the server's pen phases and steps, pixels and Windows pressure (`pen.rs`), the pen held and lifted when control moves, and the Windows pointer info and flags;
  - when the Windows pen gives way to the mouse;
  - the uinput tablet's events for touching, hovering and gone (Linux), and its device in the fault test.
- **Here:** the server's synthetic pen into a scratch window that reads pointer messages (done, checklist §122); a pen session from the phone by script, in the evening's batch.
- **By hand (checklist §122):** a drawing app with pressure on Windows; a stylus (the A71 has none); Linux.

## As built

- **Wire:** `InputPacket::Pen(PenSample)`, tag 15, exactly 10 bytes; `PEN_CONTACT`, `PEN_IN_RANGE`, `PEN_BARREL`, `PEN_ERASER`. The ready stream's byte: `SCREEN_FEATURES_SAID | SCREEN_FEATURE_PEN`, read into `RemoteFlags.screen_features`.
- **Server:** `pen.rs` (phases Away, Hovering, Touching; the steps between them, Enter before Down and Up before Leave), `input_pen_windows.rs` (one synthetic pen per injector, made at the first sample), the uinput tablet built with the other devices, the mouse otherwise. `Held` keeps the pen's last sample in range and lifts it where it was.
- **Phone:** the bridge's `SCREEN_FEATURES`, `pen_supported`, `send_pen`; `PenStroke`, `PenCurve` and `penTilt`; Pen in the mode chip and the session menu once the computer offers it (polled for 5 s after each stream comes up); Settings "Pen pressure".
- **Checked here:** the synthetic pen into a scratch window over both monitors: enter, down at the expected pixel, pressure 313 rising to 938 of 1024 as the test raised it, up, leave. The uinput tablet's events in a scratch crate under WSL.

## Review

**Fresh review: 0 Critical, 2 Important, 6 Minor.**

- **Fixed, one commit each:**
  - a pinch in Pen did not zoom the view, as the spec said it would;
  - a synthetic pen Windows never takes left strokes doing nothing: after 8 samples refused with none ever taken, the mouse stands in. The pen itself was then checked on this computer.
- **Minors fixed:**
  - a stylus's hover end before its touch left range and came back on every stroke;
  - a phone without the Input permission was offered a pen;
  - a pen packet with bytes left over was taken;
  - the tablet's events and the ready byte's making untested, the eraser and the ready stream's timing said wrongly.
- **Left as they are:**
  - a final sample lost on its way (a stream read timing out) leaves the pen touching until the next sample, a change of control or the session's end;
  - the batched samples Android holds between events are not sent;
  - a palm beside a stylus is a second finger and lifts the pen.
