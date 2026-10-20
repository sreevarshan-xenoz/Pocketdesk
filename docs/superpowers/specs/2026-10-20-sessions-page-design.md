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
    - the phone's name (its short id if it is no longer paired, "an unknown phone" with none);
    - when it ended, as the other pages say it;
    - how long it lasted;
    - how it connected: on this network, over the internet directly or through a relay, refused, or failed;
    - with the phone's own measurement, its median latency from capture to arrival.
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
