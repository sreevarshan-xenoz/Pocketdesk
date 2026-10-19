# E3, the phone as a pen (design)

Date: 2026-10-19. Plan: `docs/research/2026-10-19-next-cycle.md`, E3.

## Why

KDE's forum asks for the phone as a drawing tablet (March 2026). Pocketdesk's input has direct touch and a trackpad, both a mouse to the computer: no pressure, so a drawing app draws every stroke at one width.

## What it does

- **On the phone:** a third input mode, **Pen**, beside Direct touch and Trackpad (the mode chip and the session menu), offered only when the computer says it takes a pen.
  - A finger or a stylus on the stream is a pen in contact: where it is (as direct touch maps it, letterbox, zoom and a window crop included), its pressure and, for a stylus, its tilt and its button.
  - A stylus held above the screen is a pen hovering (the cursor follows it, nothing drawn).
  - A second finger still pinches the view, and lifts the pen first.
  - Pressure through a curve, from Settings "Pen pressure": Soft (more ink for a light touch), Even, Firm. A finger's pressure on most phones is a guess from its contact size, so Settings says a stylus draws better.
- **On the wire:** a new input packet, tag 15, `Pen {x_norm, y_norm, pressure, tilt_x, tilt_y, flags}`: position as `MouseMoveAbs` (0..=65535 over the picture), pressure 0..=65535, tilt in degrees (-90..=90), flags for in contact, in range (hovering) and the barrel button. An older computer drops an unknown tag and carries on.
- **The computer says it takes a pen:** a screen session's ready stream (`STREAM_KIND_READY`, until now sent only on sessions with no picture) carries a feature byte, `0x80` (said) with bit 0 for a pen. An older phone reads a ready stream on a screen session and ignores its bytes.
- **Windows:** a synthetic pointer device of type pen (`CreateSyntheticPointerDevice(PT_PEN)`, `InjectSyntheticPointerInput`, Windows 10 1809+, no driver), made at the first pen packet and kept for the session; position in the virtual screen's pixels after the multi-monitor remap. Where it cannot be made, the pen is a mouse: moves, and the left button for contact.
- **Linux:** a uinput tablet ("Pocketdesk Virtual Pen": `ABS_X`/`ABS_Y`, `ABS_PRESSURE`, `ABS_TILT_X`/`ABS_TILT_Y`, `BTN_TOOL_PEN`, `BTN_TOUCH`, `BTN_STYLUS`), built with the other uinput devices and rebuilt on a fault as they are. Without uinput (wlroots' virtual pointer, the portal, enigo), the pen is a mouse.
- **Held to the same rules as a mouse:** the Input permission, view-only, one phone driving (`input_control`); a pen in contact is lifted when control moves to another phone or the session ends, as a held button is.
- **Settings:** the phone's "Pen pressure". The computer has nothing to set: a pen is input, under Input.

## Not done

- A stylus's eraser end (Android reports it as a tool type; the packet has a bit for it, sent, but neither computer's device turns it into an eraser yet).
- Palm rejection beyond Android's own.
- A pen on input-only sessions (the remote has no picture to draw on).

## Testing

- **Pure, tested:**
  - the packet's encoding and its lengths; an unknown flag ignored;
  - the ready stream's feature byte, both ways, and an older phone's reading of it;
  - the pressure curve and the stroke's states from touch events (`PenStroke`, JVM);
  - the server's pen held and lifted when control moves;
  - the Windows pointer info built from a packet (pixels, pressure 0..=1024, tilt, flags);
  - the uinput pen's events for a stroke (a fault test as the other devices').
- **Here:** a pen session against the throwaway server, into a scratch window that reads pen input (Windows' pointer messages), from the phone by script.
- **By hand (checklist §122):** a drawing app with pressure on Windows; a stylus (the A71 has none); Linux.
