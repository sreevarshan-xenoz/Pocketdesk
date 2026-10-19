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

## As built

- **The library:** what the window and the tray program share is the desktop crate's library (`icon`, `model`, `tray`, `tray_menu`, `words`, `tray_instance`, `tray_watch`); the tray is woken by a callback, not egui's context. `cargo run -p pocketdesk-desktop` still runs the window (`default-run`).
- **One tray:** a named mutex (`Local\PocketdeskTray`) on Windows; a socket in the runtime folder on Linux, bound first and each probe taken.
- **The loop:** Windows waits on its messages (`MsgWaitForMultipleObjects`, a second at most) and pumps them; Linux waits on the tray's wake. Each second it reads the state, the session log only when it changed.
- **Open a page:** `core::local::app_open` (`app_open.json`, 10 s); the window takes it each refresh, shows itself and goes to the page; `quit` closes it.
- **The window:** starts `pocketdesk-tray` beside it when not running, and then shows no tray and closes when closed; a second launch with `--page` writes a request.
- **Releases:** the Linux tar.gz and the Windows zip carry `pocketdesk-tray`.
- **Measured:** `pocketdesk-tray` at rest holds 2.1 MB private (13 MB working set) where the window held 77–85 MB (the reliability spec's D7 section).

## Review

**Fresh review: 1 Critical, 2 Important, 7 Minor.**

- **Fixed, one commit each:**
  - **Critical:** with the window closed, nothing showed what only the window shows: a held session's Allow/Deny (denied after 30 s), an ask-first task's, a phone's screen. The tray program now starts the window once for each new one (`tray_watch`).
  - A Quit with no window open was taken by the next window started within 10 s, which closed at once. A window now ignores a Quit from before it started.
  - Linux: the tray's socket was never accepted, so after about 128 probes a launch would hang; and two trays starting together could both win. It binds first and takes each probe.
- **Minors fixed:** the tray program exits when its Linux tray service ends; the programs started are waited for (no zombies on Linux).
- **Left as they are:**
  - a failed window start from the tray is only logged;
  - the tray reads the same state as the window did each second;
  - with no `XDG_RUNTIME_DIR`, the socket is in the shared temp folder;
  - a click as the window closes can be taken by it and lost.

## Not done

- Starting the tray at login: `install` registers the server only, as now; Settings may add it later. After a restart there is no tray until the app is opened.

## Testing

- **Pure, tested:**
  - the open request (write, take, stale, the page names);
  - which tray the window shows (the tray program running or not);
  - the tray program's menu from the state files (D5's `tray_menu`, unchanged).
- **By hand (checklist §121):**
  - Windows: the tray program at rest (its memory), each item; Open with the window closed and with it open; Quit;
  - Linux: by the WSL stand-in.
