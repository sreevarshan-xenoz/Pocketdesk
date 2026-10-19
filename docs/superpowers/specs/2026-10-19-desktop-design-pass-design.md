# E5, the desktop app's design pass (design)

Date: 2026-10-19. Plan: `docs/research/2026-10-19-next-cycle.md`, E5.

## Why

U1 and U5 set the desktop app's rules:
- each page headed with what it is for;
- every page scrolls;
- one notice, whose kind each place gives;
- colours at 4.5:1 in both themes;
- rows that wrap;
- "computer" for the machine, no command names, `words::ago` for times;
- empty pages with the next step and a button;
- unpairing asked by name.

Every page added since, in the C, D and E cycles, was built to them in part. An audit of the code (48 findings) found where they were not, worst first.

## What it changes

Grouped as they will be committed, one group each.

1. **The Phone page scrolls as a whole.** A long folder pushed the photos out of reach, and the photo grid's own rows past the window were cut off.
2. **One notice everywhere.** Notifications, Phone files, Photos, KDE Connect, Other computers and Messages kept a line of their own: it never cleared, had no kind, and put errors in plain text. They now give the shared notice: working, done or an error.
3. **The cast window's bar** keeps dark colours in the light theme. Its buttons were dark text on near black, about 1.5:1.
4. **KDE Connect's Unpair asks by name first**, as Devices and Other computers do.
5. **Rows that wrap** on a small window:
   - Devices' grid becomes one block per device;
   - KDE Connect's rows and its permissions;
   - Activity's events;
   - Notifications' buttons;
   - Phone files' rows and phone picker;
   - Sessions' now-playing line.
6. **Devices:**
   - "Lock this computer when this phone leaves", with a note of its wait while it is on, and off on Linux with the reason;
   - a paired computer is not shown a phone's switches (Ring, Mute, the lock);
   - where each phone is, in the Overview's words: In a session, Linked, Away.
7. **Words:**
   - switches named by their labels in Devices;
   - one way to name the phone's own Settings ("in Pocketdesk's Settings on the phone");
   - "Turn it on in Settings" for every off page;
   - Clipboard's "shortened" for "cut";
   - Notifications' "Asked the phone to send …" and "From {phone} · {ago}";
   - no command names on the Overview;
   - "Sending the clipboard to …" in place of "Asked the server…";
   - "Send reply" on Messages as on Notifications;
   - the Settings section summary and two rows into the row pattern;
   - the right section name in KDE Connect's hover;
   - "Pair a phone" with no ellipsis in the tray;
   - "Send the clipboard" in the tray and on the Overview's phone row.
8. **States said, not hidden:**
   - E1's "no word back" in the warning colour;
   - a refused mute on its own warning line;
   - the tray's phone title saying Do Not Disturb or the mute's minutes left;
   - Activity's filter showing lock events under Phones, and saying so when a filter hides everything, with Show all.
9. **Empty pages with a next step:**
   - Phone files, Clipboard and Notifications offer "Pair a phone" or the place to look;
   - KDE Connect says how to make a device appear;
   - an empty folder says so.
10. **Screen readers:**
    - photo thumbnails, the pairing QR and the cast picture have names;
    - each photo's Save, Open and Copy say which photo;
    - the Phone page's two Refresh buttons say which.
11. **Sizes the same everywhere** (`words::human_size`); Activity and Phone files had their own decimal counts.
12. **Clipboard's Clear asks**, "Forget all {n} texts kept here?", where the button had turned into "Clear: sure?" for 5 s.
13. **The lock's wait in Settings** is shown on Windows only, where it acts.

## Not done

- A tray action that fails still says so only in the log; showing it takes the window or a notification, left for the tray's next pass.

## Testing

- **The C4 harness:**
  - every page opens;
  - the Phone page scrolls to its photos;
  - KDE Connect's Unpair asks first (Cancel keeps it);
  - Devices shows no phone switches for a computer;
  - the empty pages' buttons;
  - the cast bar's contrast in both themes (the theme test's 4.5:1 rule on its colours).
- **Snapshots** (the CI job): updated for the pages whose look changed.
- **By hand (checklist §124):** each page at the 640 px minimum width and in both themes.
