# A second screen at the phone's shape (T10j)

Research item 8 (`docs/research/2026-10-15-what-makes-it-worth-using.md`): Deskreen, Sunshine and Moonlight's most-asked request, and why Apollo and the Virtual Display Driver exist. The phone becomes another screen of the computer, the size of the phone, that windows can be moved to, instead of showing a shrunk copy of a monitor.

## What it does

- **Phone:** the session's screen picker gains "Add a screen for this phone". The computer makes a screen of the phone's size (its window, in pixels, the long side at most 2560) to the right of its others, and the session switches to it. Windows are dragged onto it with the mouse as onto any monitor.
- **Computer:** the screen lasts while that phone has a session: once the phone has had none for about 10 s it is removed, and one left from a server that stopped is removed at the next start. Desktop Settings "Phones may add a screen" (`virtual_screens`, on) turns it off. It needs the phone's Input permission, since it changes the desktop's layout.
- **Where:** Sway, through its IPC (`create_output`, `output … mode --custom`, `output … unplug`). Elsewhere the phone is told the computer cannot (Hyprland's socket writes are broken upstream, GNOME and KDE have no such request, and Windows needs a signed display driver).

## How

- **Wire:** `kdeconnect.pocketdesk.virtual_screen` `{action: "add", width, height}` or `{action: "remove"}`, answered with `{name, index}` or `{error}`. Under the Input permission.
- **Server (`server/src/virtual_screen.rs`):** a small i3-IPC client on `$SWAYSOCK`; pure helpers for the size (even, at most 2560 on the long side, at least 320), the new output's name, and its place; per-phone outputs, kept in `virtual_screens.json` so a restart removes them; a sweep every 5 s against the live sessions.
- **Phone:** the picker's row, the request through the bridge (`nativeAddScreen`), and a switch to the answered index.

## Checks

- **Rust:** the size, the new output's name and place, and the i3-IPC framing.
- **Device (A71 → Azure VM, Sway):** the screen added and streamed at the phone's shape, a window moved onto it, and removed after the session.
