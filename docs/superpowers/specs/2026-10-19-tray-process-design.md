# E2, the tray on its own (design)

Date: 2026-10-19. Plan: `docs/research/2026-10-19-next-cycle.md`, E2.

## Why

D7 measured the desktop app at 77–85 MB private at rest. A bare eframe window holds as much, so it is the window's: egui and the OpenGL driver. The app rests in the tray for one reason: to show the tray. A tray of its own, with no window, would leave a few MB at rest, the window started only when someone opens it.

## What it does

- **`pocketdesk-tray`**, a second program from the desktop crate, holds the tray icon and D5's menu (Windows: `tray-icon` with its own message loop; Linux: `ksni`). It links none of egui or OpenGL. It reads the state files the window reads, once a second, and rebuilds the menu only when it changes. One at a time.
- **Its menu does what the window's did:**
  - Ring, Mute or Unmute, and Send clipboard: it writes the same requests itself.
  - Open, Pair a phone… and Send file…: it starts the window, or raises the one running, on that page (Files with the phone chosen).
  - Quit: it ends the tray and asks the window to close.
- **Opening a page:**
  - The tray, or a second launch of the window, writes `<state>/app_open.json` (the page, and a phone for Files; owner-only), then starts `pocketdesk-app`.
  - A running window raises itself (as N6's second launch does), takes the request and goes to that page.
  - A request older than 10 s is ignored.
- **The window:**
  - It starts the tray program beside it, when that is there and not already running.
  - While the tray program runs, the window shows no tray of its own and closing it exits.
  - With no tray program (an older install, or no tray host on Linux), it keeps its own tray, as now.
- **Measured** before and after with `scripts/memory_at_rest.ps1`, which learns to measure the tray program as well.

## Not done

- Starting the tray at login: `install` registers the server only, as now; Settings may add it later.

## Testing

- **Pure, tested:**
  - the open request (write, take, stale, the page names);
  - which tray the window shows (the tray program running or not);
  - the tray program's menu from the state files (D5's `tray_menu`, unchanged).
- **By hand (checklist §121):**
  - Windows: the tray program at rest (its memory), each item; Open with the window closed and with it open; Quit;
  - Linux: by the WSL stand-in.
