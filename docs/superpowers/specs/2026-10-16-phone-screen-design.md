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

## As built

- **Which output:** adds run one at a time and take exactly one new `HEADLESS-N`; only such outputs are ever unplugged, so a real monitor never is. Asked again, a phone gets its own screen back.
- **Which index:** the capture matches an index by the origin xcap lists for it, else by its place among the outputs. The answer is the listed monitor at the new screen's origin (waited for up to 2 s, as XWayland learns of it late), else its place among Sway's outputs when the list is shorter; with neither, the screen is removed and the phone told.
- **How long:** a phone counts while it has a session or a control link (a session moving onto the screen, waiting at the approval gate or reconnecting keeps its link), and a new screen is left alone for 60 s. Screens a stopped server left are taken over by the next one only in the same Sway (its socket path); a restarted Sway has none of them and may reuse their names.
- **Limits:** at most 4 phone screens; a phone may always take its own back; the phone never keeps an added screen's index as the computer's default.
- **Fresh review (0 Critical, 5 Important), fixed:** the output and index above, the sweep during a switch, a second add freezing the session, and the saved index; plus the start-up cleanup, the cap and a doc comment.
