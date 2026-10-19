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
   - the Settings section summary;
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
- Two Settings rows ("Send to phone", "Send from the browser") into the row pattern, and Home Assistant's broker fields out of their grid.

## Testing

- **Unit tests:** the Phone page is not one that scrolls itself; a computer is shown none of a phone's switches; the lock says why it is off where it never locks; the Phones filter takes lock events; an answer with no word back is a warning; one size for every page; the tray title's Do Not Disturb and mute.
- **The C4 harness:**
  - every page opens;
  - Notifications and Phone files with no phone offer Pair a phone, which opens Pair;
  - Clipboard's Clear asks, Cancel keeps and Clear clears;
  - each photo's buttons name the photo;
  - the Overview's phone has Send the clipboard;
  - Devices' Unpair still asks by name.
- **Snapshots** (the CI job): drawn anew for the Overview, Devices and Settings, whose look changed, from the job's own pictures.
- **By hand (checklist §124):** each page at the 640 px minimum width and in both themes, the cast bar in the light theme, KDE Connect's Unpair (it needs the server running), and the Phone page with a long folder and photos.

## As built

All thirteen groups, one commit each, plus the Overview's Send the clipboard (from the review). Pages hand their notice to the app (`take_notice`) and their empty state's button its page (`take_go_to`); the app gathers both each frame.

## Review

**Fresh review: 0 Critical, 2 Important, 9 Minor.**

- **Fixed:**
  - "Asked the phone…" notices waited up to five minutes for an answer only a page drawn could give: they are plain notices now, gone in seconds, and the answer shows on the page;
  - the spec claimed more than was built: the Overview's Send the clipboard is added, and the rest of the claims now match what was done;
  - minors: a stale Unpair question in KDE Connect, its empty state with the server stopped, a queued text replacing an earlier error, a notice shown a frame late, "Pairing…" said as done, a test for the tray title, a doc comment out of place.
- **Left as they are:**
  - one notice survives when two pages speak in one frame (the Phone page's two parts, rarely);
  - the tray rebuilds its menu each minute while a mute counts down;
  - Ring stays, greyed with its reason, for a paired computer;
  - on Linux an away lock already on shows checked and greyed.
