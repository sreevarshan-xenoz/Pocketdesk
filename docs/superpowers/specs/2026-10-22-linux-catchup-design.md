# N10, Linux catch-up (design)

Date: 2026-10-22. Plan: `docs/research/2026-10-21-n-cycle.md` §N10.

Three things Windows has had since K7, G4 and M4 that Linux lacked or had never run.

## 1. The phone's music in Linux's media controls (K7, N2)

On Windows the tray program shows a phone's player in the system media controls (`SystemMediaTransportControls`). On Linux it now serves an MPRIS player on the session bus, `org.mpris.MediaPlayer2.pocketdesk_phone` at `/org/mpris/MediaPlayer2` (`desktop/src/phone_player_linux.rs`).

- **Same rules as Windows.** It follows the same Settings switch, "The phone's music in this computer's media controls" (`phone_media_controls`, off by default), and the same choice of phone (`phone_player::Choice`: a session, or a linked phone's background report; a paused one stays five minutes).
- **What it shows:** the song as `xesam:title`, `xesam:artist` and `xesam:album` (the phone's app), with the cover as a `file://` URL of the server's owner-only copy.
- **Presses.** PlayPause, Play, Pause, Next and Previous go through `model::press_phone_media`, as the Sessions page's buttons do. Stop pauses. Seek, SetPosition and OpenUri do nothing, and say so with `CanSeek` false.
- **Where it runs.** It runs on a thread of its own with a current-thread tokio runtime; zbus stays on async-io in this crate, since AccessKit's Linux backend panics with zbus's `tokio` on (it had left screen readers without the window since N6's tray brought ksni's default `tokio` in), and the release builds the server apart. The tray loop tells it of a change, and it sends PropertiesChanged.
  - With the switch off, the sender is dropped, which ends the thread and lets go of the name.
  - A bus that cannot be reached is tried again a minute later, as Windows' player is.

## 2. The laser dot on wlroots compositors (G4)

`core::streaming::laser_dot` draws the dot as a 24-pixel round surface on `zwlr_layer_shell_v1`'s overlay layer. The dot grows on taller screens, as on Windows.

- **No input.** It has an empty input region and no keyboard interactivity, so presses reach what is under it.
- **Placement.** It is anchored top-left with an exclusive zone of −1, and placed by its margins on the output under it. A move is a margin change and a commit.
- **Screens.** They are read from `xdg_output`'s logical box where offered. Otherwise they come from `wl_output`'s mode over its scale, with the sides swapped when the output is turned.
- **The thread.** `server/src/laser_linux.rs` holds the dot's thread, as `laser_windows` does.
  - It opens the overlay for each showing, so the screens are read as they are then.
  - It drops the overlay when the dot hides or goes still for two seconds; the place is kept.
  - A Wayland client cannot read the pointer, so a showing starts on the first screen's middle.
- **Who is offered it.** `laser::available()` is true on Linux when a Wayland display offers the layer shell and `wl_shm`. So Sway, Hyprland and other wlroots desktops offer the dot to the phone; GNOME, KDE and X11 do not.

## 3. A first Linux run

The typing window (M4) and the cast window (T10k, L2, L5) run on Linux through eframe, which has been built there since U1 but never run. Both are run on the Azure VM under headless Sway with the release workflow's Linux build:
- the windows open and draw;
- the MPRIS player is seen by `playerctl` and takes a press;
- the dot is drawn on screen.

A phone cannot reach the VM's windows in a way that is checked by eye, so the run is by `grim` screenshots and logs.

## Not done

- **GNOME and KDE's laser.** They have no layer shell. A dot there would need a fullscreen transparent window per output, which the desktop may not keep above others.
- **A pointer start.** On Hyprland the dot could start where the pointer is (`j/cursorpos`); left for when a Hyprland box is at hand.
