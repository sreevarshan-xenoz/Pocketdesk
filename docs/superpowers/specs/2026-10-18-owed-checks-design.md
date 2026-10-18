# The owed checks that can run here (C6)

**Plan:** `docs/research/2026-10-17-next-cycle.md`, C6.

## What

About 550 checklist items are open. Many need something this box does not have (a SIM, Linux, a second phone, a router), and some must never run here (power, lock or stopping the owner's server, input into the owner's Windows). Nobody could say which could run today.

- **An index, not 550 edits.** `scripts/owed_checks.py` reads the checklist and places each open item by its own words and its section's: here; the owner's OK (input reaching Windows, its display settings); never here (power, lock, server stop); a SIM; a second phone; away from home; a second computer; Linux; other hardware; by hand or eye. The first place that matches wins, and a line can name its own with `(where: …)`. `docs/owed-checks.md` is its output, with a count per place; CI fails when it is stale.
- **A batch run here,** by script against a throwaway server and the A71, every wait bounded, everything restored after.

## The first batch (2026-10-18)

- **Sound (§25):** a 40 s session, `logcat -s DesktopAudio`: 1 underrun in 34 s.
- **The background clipboard (§65):** with the phone linked and a stand-in clipboard, a change arrived in 5 s with "Clipboard to phones" on; nothing in 40 s with it off, or with the phone's Clipboard permission off.
- **A folder with no session (§67):** a folder with a sub-folder, checksums matching.
- **A folder cancelled on the computer (§103):** a cancel request for its first file stopped all four ("4 of 4 files not sent: cancelled on this computer"), and nothing more arrived in 30 s.
- **Found and fixed:** the phone kept the cancelled file's part (a hidden pending entry in Download/Pocketdesk) for a retry that never comes. `TransferRetry.keepsPart` now drops a part when the computer refused the download as cancelled; re-run, none was left. The phone's own Cancel keeps its part, as C1 designed, so sending the file again continues from it.

**Not reached:** a folder cancelled on the phone. The app's notifications sit folded under "Silent" in the shade, so the script did not find its Cancel; a single file's Cancel was device-checked in C1.

**Left out on purpose:**
- the display at 125 and 150 % (the owner's settings);
- taps on a second monitor (input into the owner's Windows).

## Review

**Fresh review (0 Critical, 4 Important, 6 Minor), fixed:**
- the first fix also dropped the part after the phone's own Cancel, which C1 keeps on purpose (a resend continued from 615 MB);
- "here" still held checks the owner's rule forbids (Win+L, UAC, `pocketdesk stop`, install, typing into Windows, a scaled display); now placed apart, with a self-test CI runs;
- a mistyped `(where: …)` dropped its item from the index;
- 57 wrapped items were placed and listed by their first line only;
- plus false positives (drop, watch, mouse), a KDE Connect section counted as Linux, fenced blocks, Python before 3.10, and the words.

**Left as is:**
- **A cancel in mid-file:** a Cancel on the computer while a file streams breaks the connection first; the part is dropped only when the retry is refused as cancelled. With retries off, no part is kept anyway.
- **The bridge's record of a dropped part** stays until its 24-hour sweep. A resend checks what is really in the file, so it starts over correctly, but the record counts toward the per-phone limit of four meanwhile.

## Out of scope

Correcting the index by hand: a misplaced line gets a `(where: …)` when someone runs it.
