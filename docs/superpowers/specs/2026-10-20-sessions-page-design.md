# The Sessions page in blocks, with the recent ones (J3)

**Date:** 2026-10-20. **Branch:** `feat/sessions-page`. From the desktop audit's ranked list, left out of I3.

## What is wrong

- **The page is a five-column grid.** Its rows run past a narrow window. The Overview and Devices moved to a block per phone in N6 and E5.
- **The phone's music sits below the grid**, away from its session.
- **A session that has ended is gone.** Nothing on the page says who connected an hour ago, for how long, how, or whether it was refused. The session log keeps all of it.

## What changes

- **A block per live session**, as on the Overview:
  - the phone's name;
  - chips for how it is connected (on this network, or over the internet) and what it is (screen, control only, sound only, its screen or camera here);
  - how long it has run;
  - "Driving" or Give control;
  - End, and Show screen for a cast whose window was closed;
  - its music with Previous, Play/Pause and Next, inside its own block.
- **"Recent sessions" under them**, and under the empty page's words when none is live.
  - **What it lists:** the last 10 sessions from the session log, newest first.
  - **Each line:**
    - the phone's name (its short id if it is no longer paired). A refused one is named only by the id it claimed, "A device claiming to be …", since a refused client proved nothing;
    - when it ended, as the other pages say it;
    - how long it lasted;
    - how it connected: on this network, over the internet directly or through a relay, refused, or failed;
    - with the phone's own measurement, its median from capture to arrival ("38 ms capture to arrival", the rest on hover).
  - **Refusals in a row:** a device's refusals in a row are one line ("refused 6 times"), so a phone retrying does not push the real sessions out.
  - **The rule:** `model::recent_sessions`, tested.
  - **When the log is read:** only while the page is open and when the log's size or time changed, as the Overview's last sessions are read.
- **The server stopped:** the page still lists the recent sessions under its words.

## Left out

- Paging further back than the log's last 256 KiB. `pocketdesk sessions` has the whole log.
- A session's full numbers (RTT, encode, decode, link). Diagnostics shows the last one, and `pocketdesk sessions --json` all of them.

## Testing

- **Unit:**
  - `recent_sessions`: newest first, at most N;
  - lines that are not records skipped, a cut first line among them;
  - each outcome's words;
  - the latency when the record has it.
- **Harness:**
  - with no live session, the page lists a fixture log's sessions by the phone's name, a refused one as refused;
  - with the server not running, too.
- **By hand (checklist §145):** a live session's block at the minimum width; the music in the block; the list after a session ends.

## As built

As designed, with the review's fixes below. The block is `App::live_sessions` and the list `App::recent_sessions_list`. The rules are `model::recent_sessions`, `recent_who` and `session_kind`.

## Review

A fresh review found 0 Critical, 3 Important and 6 Minor.

**Important, fixed one commit each:**

- **A refused session was named as the paired phone whose id it claimed.**
- **Repeated refusals could push the real sessions out of the list.**
- **"Latency" said more than the number measures.**

**Minor:**

- Fixed:
  - two sessions ending in one second were listed oldest first;
  - a record without a length read "for 0s";
  - `control_cell` was asked twice;
  - the list came a frame late on opening the page.
- Left to the check by hand (§145): whether the chips wrap at the minimum width.
- Not a change: the hint for a session log turned off. The server always writes it, and there is no setting.
