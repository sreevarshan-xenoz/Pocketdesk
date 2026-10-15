# Input on wlroots without uinput (T10f)

Found on the Azure VM during F4: when the server's user cannot write `/dev/uinput` (the default on most distributions, which make it root-only), Linux input has no injector at all. Pointer, keys and text all do nothing until the owner adds a udev rule or a group. Yet wlroots compositors (Sway, Hyprland, river, labwc, Wayfire) offer input to any client of the session:
- `zwlr_virtual_pointer_v1` for the pointer;
- `zwp_virtual_keyboard_v1` for keys.

## The change

A third Linux backend, `Wlr`, tried when uinput cannot be opened, before enigo (which only reaches XWayland windows). It has two parts.

**`core::input::wlr_pointer`**, a `zwlr_virtual_pointer_v1` with:
- relative motion;
- absolute motion over the output layout, in the extent the server already maps taps onto;
- buttons (left, right, middle, back, forward);
- vertical and horizontal axis, with discrete steps for wheel notches;
- a frame after each event.

**`core::input::vkeyboard`**, for keys as well as text:
- a "keys" keymap, the compositor's own compile of `evdev` keycodes with a `pc+us` layout. evdev codes go straight through, as they do on uinput;
- text keeps its one-key-per-character keymaps, switched back to the keys keymap afterwards.

Every request waits for the compositor within its deadline, as F4's text does.

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
