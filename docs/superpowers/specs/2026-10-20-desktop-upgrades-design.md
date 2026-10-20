# The desktop app: upgrades and UI tweaks (I3)

**Date:** 2026-10-20. **Branch:** `feat/desktop-upgrades`. **Asked for by the owner:** "some upgrades and UI tweaks for the desktop app". A read-only audit of the app after E5's design pass ranked fourteen items; these are the ones taken, most useful to a daily user first, one commit each.

## Bugs

1. **A reply could go to the wrong chat.** On Messages one draft served every conversation and "New text": text half-typed for Ann went to Bob once Bob's thread was opened and Send pressed. Each conversation, and a new text, keeps its own draft (`Drafts`, tested).
2. **Save in Settings undid hand edits.** The form was read once per run and Save wrote every key that differed from the file, so a hand edit to `config.toml` made while the app ran (hooks, `[[deck]]`, a port) went back at the next Save. Save writes only the keys changed in the form.

## Tweaks

3. **The notice keeps its place.** It sat above each page's heading, so every notice pushed the page down a line and back six seconds later, under the pointer. It now has a bar of its own at the foot of the window while it shows; its time grows with its length; work under way shows a spinner.
4. **Keys.** Enter sends in the Files path and link boxes and the Notifications reply box; Enter sends a text on Messages, Shift+Enter starting a new line, as in a chat; Ctrl+S saves Settings; Esc closes an open question (Unpair, Restart, Clear); Ctrl+Page Up/Down moves between pages.
5. **The page list says what waits.** A count after Sessions (live sessions) and Files (transfers under way), and a dot after Settings while it has unsaved changes. Notifications' count is left out: it would read its file at every frame.

## Upgrades

6. **Files: Send again and Open.** A send that failed has Send again while its phone can take files; a received file has Open beside Show in folder; the lists wrap as blocks at narrow widths.
7. **Overview signals.** Each phone's battery is a small bar, in the warning colour at 15 % or below and not charging (the server's own low mark); "Pairing required: no" is a warning line with the way to Settings.
8. **Diagnostics runs by itself** on its first visit, and each finding's status is a coloured chip.
9. **Guest pairing from Pair.** "This code pairs for": always, 15 minutes, an hour or a day, as `pocketdesk pair --grant` does; the Devices page already counts a guest's time down.
10. **Phone files says it is asking.** A spinner and "Asking <phone>…" while a folder or a refresh is on its way (a download keeps its own notice).

## Left out (this round)

A Settings search, Sessions' recent list, the Logs page's filter and tail-reading, search on Activity and Clipboard, a phone choice for files dropped on the window: each is worth doing and larger; they are listed for the next round.

## Testing

Unit tests: the drafts, Save's keys (in core and through the form), the notice's time, the badges and the page order, the battery's low mark, the chips' contrast, the guest code's time on file and a code kept from the CLI, and what Open may open. Through the harness: Esc on Unpair's question, Ctrl+Page Down, the Overview's battery line. By hand on this box (checklist §142): the rest, Send again among them.

## Review

A fresh review found 0 Critical, 0 Important and 8 Minor issues. Fixed, one commit each:
- **Open** judged the phone's name for the file, not the path it opens. Both are now held to the allow-list.
- **The badges** lost the label's selected colours, and the unsaved dot was green. The name takes the label's colour, and the dot is a warning.
- **Pair** replaced a code `pocketdesk pair --grant` made while the page was open. The page now takes its time instead.
- **A download** from Phone files dropped a listing's spinner and its timeout.
- **Diagnostics' first run** waited a second to draw.
- **This spec's claims:** Ctrl+Enter, the download spinner, the strip, the tests.

Left as they are:
- After a save the form's baseline is the form, not the file. That is cosmetic, and the form is read again at the next start.
- The 15 % mark is the server's, repeated here because the desktop cannot take it from the server crate. A comment says so.
