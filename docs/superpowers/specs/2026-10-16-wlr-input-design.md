# Input on wlroots without uinput (T10f)

Found on the Azure VM during F4: when the server's user cannot write `/dev/uinput` (the default on most distributions, which make it root-only), Linux input has no injector at all. Pointer, keys and text all do nothing until the owner adds a udev rule or a group. Yet wlroots compositors (Sway, Hyprland, river, labwc, Wayfire) offer input to any client of the session:
- `zwlr_virtual_pointer_v1` for the pointer;
- `zwp_virtual_keyboard_v1` for keys.

## The change

A third Linux backend, `Wlr`, tried when uinput cannot be opened, before enigo (which only reaches XWayland windows). It is one module, **`core::input::wlr_input`**, holding one connection's:
- `zwlr_virtual_pointer_v1`: relative motion; absolute motion over the output layout, in the extent the server already maps taps onto; buttons (left, right, middle, back, forward); vertical and horizontal wheel notches as discrete steps; a frame after each event;
- `zwp_virtual_keyboard_v1` with a "keys" keymap, the compositor's own compile of `evdev` keycodes with a `pc+us` layout, so evdev codes go straight through, as they do on uinput.

Text keeps F4's own virtual keyboard (`vkeyboard`, one key per character) on a connection of its own, so the keys keymap is never swapped out.

Pointer and key requests are sent without waiting for the compositor (waiting would add a roundtrip to every motion); connecting and text wait within their deadlines. A connection that breaks is opened again.

## What is not covered

- **GNOME and KDE.** They offer neither protocol to clients, so they still need uinput (or the portal's RemoteDesktop, a later item).
- **Absolute moves on a multi-output layout.** These use the layout's bounding box, as the uinput path does.

## Tests

- **Pure, on every OS:**
  - the button and axis mapping;
  - the keys keymap text;
  - the absolute extent.
- **Live:** on the Azure VM, with `/dev/uinput` root-only:
  - a session moves the pointer;
  - clicks land (seen with `wev`);
  - keys and text type.

## As built

- **One module, `core::input::wlr_input`:** a `WlrInput` holds the virtual pointer and a virtual keyboard with the keys keymap (`evdev+aliases(qwerty)`, `pc+us+inet(evdev)`). Text keeps its own F4 keyboard (`vkeyboard`), so the keys keymap is never swapped out.
- **The server's order on Wayland:** uinput, then `Wlr`, then enigo. The log names the choice: "using the compositor's virtual pointer and keyboard (uinput: …)".
- **Device-checked (A71 → Azure VM, Sway, `/dev/uinput` root-only, over the internet):** trackpad motion (43 motion events in `wev`), a tap as a left press and release, "Hi PD!" with its capitals and `!`, Enter, and Esc from the shortcut bar as a key.
