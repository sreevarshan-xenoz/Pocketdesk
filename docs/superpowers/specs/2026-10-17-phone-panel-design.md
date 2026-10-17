# The phone panel and a Linux tray (N6)

Date: 2026-10-17. Plan: `docs/research/2026-10-17-finish-and-polish.md` §N6. Research: Windows 11's Start menu phone panel (the phone, its battery, connected or not, and a few actions at a glance); the desktop app has no tray on Linux (`desktop/src/tray_none.rs`).

## The problem

The Overview says whether the server runs and counts phones and sessions, but not which phone is there, how its battery is, or when it was last used. Ringing it, sending it a file or a link means finding the right page and picking the phone again. On Linux, closing the window quits the app: tray-icon's Linux menu needs a GTK main loop the app does not run, so there is no tray there.

## The design

1. **Your phones, on the Overview.** Under the server's status, one row per paired phone (not other computers, which have their own section in Settings):
   - its name (or short id);
   - where it is: in a session now, linked (a background feature keeps a link), or away;
   - its battery as it last reported it (the Devices page's words);
   - its last session, as how long ago (the newest record for it in `streaming_sessions.jsonl`, read from the file's tail, so a long log costs no more);
   - **Ring** (as on Devices, with the same rule and reasons), **Send file…** and **Send link…**, which open the Files page with that phone chosen (Send link puts the cursor in the link box). Each is greyed with its reason when it cannot reach the phone now.
   - With no phone paired, the row says so and offers Pair a phone.
2. **A tray on Linux** through StatusNotifierItem over D-Bus (`ksni`, Unlicense, pure Rust on zbus, no GTK): the same icon, tooltip and menu as on Windows (Open Pocketdesk, Pair a phone…, Quit); a click on the icon opens the window. It runs on its own thread with its own D-Bus connection, queues menu picks for the app and wakes it, as the Windows tray does. With no StatusNotifierWatcher on the session bus (a desktop without a tray host, such as GNOME without the AppIndicator extension), it reports no tray and closing quits, as now. If the tray host goes away later, the item waits for it; the window can still be raised by starting the app again (single instance).

## Tests

- The phone rows (`model::phone_rows`): phones only, their place (session, linked, away), the last session per phone from a log's lines (newest wins, unknown lines skipped, other phones' records ignored), and the actions each row offers.
- The tray's menu mapping and icon conversion (RGBA to the ARGB32 big-endian pixmaps SNI wants).

## Check

On this box through the app's own window: the Overview with the A71 linked and in a session, Ring, Send file… and Send link… opening Files with it chosen. On the Linux VM (a session bus with a StatusNotifierWatcher stood in by a small script): the item registers, its menu lists the three entries, Activate and each menu entry reach the app (Quit quits); with no watcher, the app starts without a tray and closing quits.

## As built

As designed, with these found on the way:

- **Each phone is a short block**, not a table row: the row with its three buttons was wider than the window at its default size (Send link… cut off). The name and where it is, then its battery and last session, then the buttons.
- **No last session while one runs**: a session is recorded when it ends, so "last session 45 min ago" beside "In a session" read as wrong.
- **Send link…** opens a box right on the Overview (the phone it is for, the link, Send and Cancel), since Files lists only the phones files can reach, and a link reaches more.
- **The Linux tray** replaces the no-tray stub; the icon conversion (`icon::argb32`) is tested on every OS, the menu mapping on Linux.

**Checked:**
- **On this box through the app's own window** (a throwaway server, the A71): the phone in a session, then linked with its last session; Send link… opening its box (Cancel closes it); Send file… opening Files with the phone chosen. Typing into the box is not checked: egui takes no value through UI Automation, and nothing was typed into the desktop.
- **The tray on Linux, in WSL** (Arch, a private D-Bus session, a stand-in watcher written for it, the tray module run on its own since WSL has no display libraries):
  - registered, with title Pocketdesk, category ApplicationStatus and the icon's pixmaps;
  - the menu's three entries;
  - a click on the icon, Pair a phone… and Quit reaching the app as Open, Pair and Quit;
  - with no watcher, no tray (closing quits).
- **The whole desktop app built in WSL** registered its item before failing to open a window there (no Wayland or X11 libraries).

**Owed:** the tray in a real panel (KDE Plasma, waybar, GNOME with AppIndicator), with the window hidden and brought back.

