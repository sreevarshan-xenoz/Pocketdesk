# "Send to phone" in the file manager (T10e)

The second half of item 5 in `docs/research/2026-10-15-what-makes-it-worth-using.md`. LocalSend's issue #115, asking for a context menu, has 49 votes; Windows 11 has "Send to my phone".

## The change

- **`pocketdesk send-to-phone <paths…>`** queues each file or folder for the phone that files reach now: one in a session first, else the first one linked at home (T10d).
  - It says how it went on stdout and in a desktop notification, since a file manager shows no console.
  - Exit codes: 0 queued, 1 failed (also when only some went), 2 nothing chosen, 3 no server running.
- **`pocketdesk send-menu [--remove]`** adds or removes the entries that run it, through `core::local::send_menu`:
  - Windows: `%APPDATA%\Microsoft\Windows\SendTo\Pocketdesk (phone).lnk`, minimized. It is made through the shell's COM object by PowerShell, resolved under System32, without the `\\?\` prefix of a canonical path.
  - Linux: a Nautilus and a Nemo script (`Send to phone`), and a Dolphin service menu (`pocketdesk-send.desktop`), each executable.
- **`pocketdesk install` on Windows** adds the Send To entry, and `uninstall` removes it.
- **The desktop app's Settings** has a switch for it, which runs the server program beside the app.

## Tests

- **Core:** the script and service menu contents, with a path that needs quoting; adding and removing the files (Linux), and the Send To place (Windows).
- **Server:**
  - `send_to_phone_in` queues for the phone in a session;
  - it says why not with nothing chosen, no server or no phone;
  - it names a file that failed among two.

## As built

- **Device-checked (A71 → Windows):** with the phone linked in the background, `send-to-phone` with two files said "Sending to your phone: sendto-a.bin, sendto-b.bin", and both were saved with their MD5s matching.
- **The Send To shortcut** was made and read back: the target is the server, the arguments `send-to-phone`, minimized. It was then removed.
- **Owed:** picking Send To in Explorer itself, the Linux file managers, and the desktop app's switch (checklist §69).
