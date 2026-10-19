# D5, the tray you act from (design)

Date: 2026-10-19. Plan: `docs/research/2026-10-18-next-cycle.md`, D5.

## Why

A 2026 KDE thread asks for a device panel in the tray and dragging files onto a device (https://discuss.kde.org/t/some-ideas-about-kde-connect-enhancements/49144). Windows Central called the Start-menu phone panel "just links". Pocketdesk's tray had Open, Pair a phone… and Quit only: everything else meant opening the window.

## What it does

- **The tray menu** lists each paired phone (not other computers) as a submenu, titled with its name and battery ("Pixel 8 · 64 %, charging"), between Pair a phone… and Quit. Each holds:
  - **Ring**, as the Overview's Ring (N1);
  - **Mute for an hour**, or **Unmute** for a mute asked from here (D2), as the Overview's;
  - **Send clipboard**: this computer's clipboard, as it is when the server reads it, to that phone;
  - **Send file…**: the window, on Files with that phone chosen.
- **What cannot be done now is shown disabled**, by the same rules as the Overview's buttons (`model::phone_rows`): Ring and Mute need their permission and a link, Send clipboard needs Clipboard on and a link (as D3's Send to…), Send file… a phone files reach. A tray menu has no hover text, so the reasons stay on the Overview.
- **A pick is queued for the server** as the Overview's are, and its notice waits in the window (the app shows no notifications of its own). The phone's ring or mute is what answers.
- **The menu follows the phones.** It is rebuilt only when what it shows changes (a phone paired, linked, its battery, a mute), from the app's once-a-second refresh, which runs while the window is hidden.

## Send clipboard

- **The app never reads the clipboard itself.** It writes a `clipboard_out/` request (D3's) with `current: true` and no text. The server reads its clipboard when it takes the request, with the rules a push already keeps: a copy a password manager marks private, or a pairing link, is never sent ("nothing to send").
- **Requests from before D5** have no `current` and read as before. A `current` request carrying text is refused.

## Not done

- **Files dropped on the tray icon.** Windows' notification area takes no drops (`Shell_NotifyIcon` has no drop target), and neither does a StatusNotifierItem. Send To (T10e) and the window's drop stay the ways to drop.
- **Global shortcuts.**

## Testing

- **Pure, tested:**
  - the menu's entries from the phones (`tray_menu`): titles, which items are enabled, Mute's label and minutes;
  - a `current` request round-trip and refusals (`clip_history`);
  - the server's send of a current copy, with a stand-in clipboard: sent, empty, private.
- **By hand (checklist §116):**
  - the menu on Windows with the A71 linked: each item, the battery title changing;
  - the Linux tray.
