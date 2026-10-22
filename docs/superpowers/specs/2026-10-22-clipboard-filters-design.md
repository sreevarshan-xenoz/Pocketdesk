# O4, copies kept on the computer by program or by look (design)

Date: 2026-10-22. Plan: `docs/research/2026-10-22-o-cycle.md` §O4. Asked for in KDE bug 519109 ("Clipboard Filter Setting") and KDE Discuss 51056, which notes that password apps do not always mark their copies private.

## What

Two desktop Settings, under Phones beside "Clipboard to phones", both empty by default:

- **"Never send copies from":** programs, one a line, by their file name (`KeePassXC.exe`; any case, with or without `.exe`, any folder). A copy names its program on Windows only: the process owning the clipboard's window (`GetClipboardOwner`, then the process's image name). "Add the usual password managers" offers KeePass, KeePassXC, 1Password and Bitwarden.
- **"Never send copies that look like":** regular expressions, one a line. A copy whose text matches any of them stays. "Add keys and tokens" offers four patterns:
  - a private key's first line;
  - AWS access key ids;
  - GitHub tokens;
  - Slack tokens.

  Settings will not save a pattern that does not read, or one over 512 characters.

## Where it holds

The rules are in `core::local::clip_filter` (JVM-free, tested). The server checks them in its two reads of the clipboard:

- **`read_text`:** T10b's push to phones and computers, the tray's and the Overview's Send the clipboard, and D3's history all read through it.
- **`read_clipboard`:** a phone asking for the clipboard in a session, for text and images. An image has no text, so only its program holds it back.

So a held copy goes nowhere, on top of the private marks a password manager sets (T10b, M3).

- **The settings.** They are read live, through the server's config cache, and the filter is built again only when they change.
- **The log.** A held copy is logged once, never its text.

## Not done

- **Linux has no program to name:** Wayland's clipboard says nothing of who copied. Only the patterns apply there.
- **What the phone copies is not filtered:** the phone's own copies are its owner's choice to send (the tile, M10).

## Checks

- **Unit tests:** the rules (names, patterns, images, bad patterns) and the Settings form (lists a line each, the usual ones added once, a bad pattern keeps Save off).
- **A throwaway server** with the clipboard stand-in and the OPPO linked: a copy with an AWS-shaped key was held, once in the log, and an ordinary copy was not.
- **Owed:** a real KeePassXC copy on Windows (checklist §187).

## Review

A fresh review found 0 Critical, 2 Important and 5 Minor. Every road out was checked and found to go through the two reads. Fixed in two commits:

- **A copy's program was read a moment after its text** (Important). A copy made between them was judged by the other's program. The clipboard's sequence number is now held across both reads, and the read is tried again when it moved.
- **What cannot be caught was unsaid** (Important). A copy whose program Windows does not name passes the program rule. That covers a program that has closed, another user's, and a browser extension's copy, which is the browser's. The note, README and this spec now say so, and point to the patterns.
- **Minor:**
  - the history's Resend is held to the rules as they are now;
  - the tray's refusal names Settings' rules;
  - Settings checks a pattern once per text, not every frame;
  - an unreadable pattern from a hand edit is logged;
  - a pattern's length is counted in characters.

Left: a server test of the reads with the stand-ins. The seams read the owner's config, so it waits for a test config folder of its own.
