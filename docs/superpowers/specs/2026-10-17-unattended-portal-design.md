# GNOME and KDE hosts that work unattended (N8)

Date: 2026-10-17. Plan: `docs/research/2026-10-17-finish-and-polish.md` §N8. Research: xdg-desktop-portal's ScreenCast and RemoteDesktop (`persist_mode`, `restore_token`), xdg-desktop-portal-gnome MR 88 (GNOME keeps restore tokens), KDE bug 517454 (KDE's have been unreliable).

## The problem

On GNOME and KDE, capture goes through the ScreenCast portal (`core/src/streaming/capture_linux.rs`), asked afresh at every session start and reconnect with no persistence. A dialog appears at the desk each time, and a session from away can never be approved. Input there needs uinput, root or a group `install.sh` does not set up. The wlroots path (Sway, Hyprland) needs neither and is not touched.

## The design

1. **Remember the grant (opt-in).** A new setting, `remember_screen_grant` (desktop Settings, Connection: "Remember the screen-sharing grant", off by default). With it on:
   - the capture asks the portal with `persist_mode = explicitly revoked` and the restore token it was last given;
   - the token the portal answers with is kept in `<state>/portal_restore_token`, owner-only (each start gives a new one);
   - the first session still asks at the desk once, and later sessions start with no one there, until the owner revokes it in the desktop's own settings (GNOME: Privacy, Screen Sharing; KDE: the portal's permissions).
   - "Forget the grant" in the app deletes the token; turning the setting off deletes it too.
   - A token the portal refuses (revoked, or KDE losing it) means a new dialog, never a failed session.
2. **Input through the portal.** Where uinput cannot be opened and the compositor is not wlroots, the capture's portal session is a RemoteDesktop session that also selects the screen, so one grant covers the screen and the keyboard and pointer. The input task routes events through it:
   - pointer motion, relative and absolute (against the session's stream);
   - buttons, the wheel, and keys by evdev code;
   - text as keysyms.
   The input task then hands the session's injector to whichever session is live. Under the setting, the RemoteDesktop grant is remembered the same way.
3. **Security.** A remembered grant means sessions start with no one at the desk. It is the owner's explicit choice, revocable in two places. SECURITY.md says so, and every session is still held to the phone's pairing, its key and its permissions (and "ask first" where set).

## Tests

- The token store: written owner-only, read back, forgotten, and a missing or unreadable one read as none.
- The choice of what to ask the portal: persist mode and token by setting and token.
- The routing of each input packet to a RemoteDesktop call: a pure mapping, tested without a portal.
- Settings: the row and that the setting applies live.

## Check

On a GNOME VM (Ubuntu 24.04, a GNOME session; the first dialog answered once at the console):
- a session from the A71 asks once;
- a second session and a reconnect start with no dialog;
- input works without uinput;
- revoking in GNOME's settings brings the dialog back;
- with the setting off, every session asks.

KDE where a VM allows.
