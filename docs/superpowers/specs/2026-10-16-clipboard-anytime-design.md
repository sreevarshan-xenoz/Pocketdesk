# The clipboard outside a session (T10b)

Item 3 of `docs/research/2026-10-15-what-makes-it-worth-using.md`.

## The problem

The clipboard syncs only while a session is open. `ClipboardSyncEffect` runs inside the session screen and polls the desktop every 4 s. Copying on one device and pasting on the other is the most common reason people keep a phone link open, and KDE Connect, Phone Link and Pushbullet all do it without a screen session.

Android 10+ lets only the focused app, or the default keyboard, read the clipboard. So the two directions need different answers.

## Phone → computer: "Send clipboard"

- A transparent activity, `SendClipboardActivity`, reads the clipboard once its window has focus. That is the moment Android allows it, and KDE Connect does the same. It then:
  - sends the text to the computer the phone sends to (`PhoneLink.target`), over the live link or by opening one, over the internet when it must (T10a);
  - shows a toast: "Clipboard sent to <name>", or why not;
  - finishes. It never shows a window.
- **Entry points:**
  - a Quick Settings tile, "Send clipboard";
  - a launcher shortcut, which is static, so it is there without a saved session;
  - the share sheet already sends text, so nothing changes there.
- Images are not sent this way. The transfer dials the phone back and needs the same network; a later step can add them.

## Computer → phone: pushed, applied in the background

- **The desktop watches its clipboard** and pushes `kdeconnect.clipboard {content}` to every live control connection whose phone has the Clipboard permission. It does so when the text changes, at most once a second.
  - Windows: `GetClipboardSequenceNumber`, polled every 500 ms, reading the text only when the number moves.
  - Linux: `wl-paste --watch`, falling back to polling `wl-paste` every second.
  - The text the desktop itself just set from a phone is not echoed back to that phone.
- **The phone's bridge latches the latest pushed clipboard.** Whoever holds the link applies it to the phone's clipboard: the session loop, the background loop, or the notification listener's loop. Android allows writing the clipboard from the background.
- **The phone needs a link to receive pushes.** Today the session holds one, and so does the notification listener when it runs. A new switch, Settings "Computer's clipboard in the background" (off by default), keeps the link in the notification listener's loop, as texts replies do. Without the notification listener, the clipboard arrives the next time the app is open.
- **Settings:**
  - phone: "Computer's clipboard in the background";
  - desktop: "Send my clipboard to phones" (`clipboard_push`, default on), for those who copy secrets.

## Tests

- **Server:**
  - the clipboard watcher's change detection (a pure function over sequence numbers and texts);
  - the push goes only to live, paired connections with Clipboard on;
  - no echo to the phone that set it.
- **Bridge:** a pushed `kdeconnect.clipboard` is latched, and only the newest is kept.
- **Kotlin (JVM):**
  - `ClipboardPushRule`: apply only text that differs from what the phone has, and not what it just sent;
  - `SendClipboardActivity`'s choices: no computer, an empty clipboard, too long.
- **Device:**
  - the tile sends the phone's clipboard to Windows with the app in the background;
  - copy on Windows, then paste on the phone without opening the app, with the notification listener on and the background switch on.

## As built

- **Linux reads text only:** `wl-paste --no-newline --type text`, so an image on the clipboard is never read every 2 s.
- **The applier takes text only from the phone's computer** (`PhoneLink.target`), the one its notifications go to.
- **No launcher shortcut:** the four dynamic shortcuts are the saved computers'. The tile and the share sheet are the ways in.
- **Device-checked (A71 → Windows), checklist §65:** the background push and the tile.
- **Found on the device run:** a force-stopped app's notification listener is not bound again until the app is opened once. This matters to anyone testing with `am force-stop`, not to users.
- **Fresh review (0 Critical, 1 Important), fixed one commit each:**
  - copies a password manager marks private were pushed. They now stay on the computer: on Windows, `ExcludeClipboardContentFromMonitorProcessing`, `Clipboard Viewer Ignore`, or `CanIncludeInClipboardHistory` / `CanUploadToCloudClipboard` set to 0 (live-checked); on Wayland, `x-kde-passwordManagerHint`.
  - Minors fixed: a failed Windows read was never retried; the note of a phone's own copy lingered; a copy made while no phone was connected was pushed when one came back; the watcher held large text twice; the applier could overwrite a fresh local copy during a session, or apply old text when switched on; its connect could hold up text replies; "Send clipboard" closed before its send finished.
  - **Accepted:**
    - a push goes to a device's newest connection, which for a moment can be a short query connection;
    - the phone's switch decides only whether the text is applied. A phone with Clipboard on still receives the pushes while its link is up (SECURITY.md limit 8).
- **Re-checked on the A71 after the fixes:** both directions, and the window closing once its send was done.
