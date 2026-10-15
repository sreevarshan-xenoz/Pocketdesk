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
- **Fresh review (0 Critical, 2 Important), fixed:**
  - the shortcut's paths were spliced into PowerShell code, and a typographic quote in a folder name ended the string. They now reach the script as environment variables.
  - a file named like an option (`-x.txt`, `--help`) was read as one. Every entry now passes `--`; checked live with `-h`.
  - Minors fixed:
    - the Dolphin `Exec` line follows the Desktop Entry spec's quoting;
    - the Nautilus and Nemo script reads their full-path variables (bare names, and none in search results, otherwise), with a test running it through `sh`;
    - the desktop switch needs the server truly beside the app and reads its state once;
    - `default_phone` has its test.
  - **Accepted:**
    - "Sending to your phone" is said when the files are queued; the server may still refuse one later, and the sent list says so;
    - `installed_in` checks that the entries are there, not where they point.
