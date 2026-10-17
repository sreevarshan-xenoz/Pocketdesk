# Transfers you can watch and stop (C1)

Date: 2026-10-17. Plan: `docs/research/2026-10-17-next-cycle.md` §C1.

## The problem

A file on its way between the phone and the computer has two lives:

- **On the computer, it is invisible until it ends.** The desktop app's Files page lists sends and received files only once they settle (`sent.json`, `received.json`). A 2 GB send to the phone shows "waiting for the phone" and then, minutes later, "saved"; nothing in between, and no way to stop it.
- **On the phone, it can be watched but not stopped.** The notification has a bar, but no Cancel. A share that was a mistake runs to the end, retries included.

## What changes

### The computer

1. **One table of what is moving.**
   - Both kinds of transfer already run under one keyed registry (`share::running()`, T6a): a download the phone asked for (an offer or a browsed file) and a share it sends.
   - Each now also has a row: an id for the app, the direction, the phone, the file's name, its folder (for a folder send), its size, and the bytes so far.
   - The bytes come from the transfer's own progress (`transfer::send`'s callback; `transfer::receive` gains one), and count what the other end already had.
   - The row goes when the transfer ends, however it ends.
2. **A mirror for the app.**
   - The server writes the rows to `<state>/transfers.json`, owner-only:
     - each row with its bytes and a rate over the last few seconds;
     - an `updated_ms` stamp.
   - It is written:
     - when a row comes or goes;
     - at most once a second while bytes move;
     - every 2 s while any row exists, so a reader can tell a live list from one a stopped server left (the 5 s rule of `live_sessions.json`).
   - With no rows, the list is written empty once.
3. **Cancel.**
   - The app's Cancel writes `<state>/transfer_cancels/<request>.json`, naming the row's id. It is consumed by the inbox watcher (R6c), and requests older than a minute are dropped.
   - The server stops the transfer: its registry entry is aborted, as a retry's supersede does.
   - It remembers the file for 10 minutes as cancelled here, by the same key. The phone's next attempt is refused with "cancelled on the computer", so it does not retry. A refusal is not retried on the phone (T6a's review), and a download's refusal (`file_fetch_refused`) and a share's are answered already.
   - An offer that is cancelled is settled as failed, "cancelled on this computer", so the Sent list says so.
   - A share's partial stays in `partials/` (swept as before), so the phone's next share of the same file continues.
4. **The Files page.**
   - "Under way", above Sent and Received, while anything moves:
     - the file (with its folder);
     - the phone, and which way;
     - a bar with "42 % · 3.1 MB/s";
     - Cancel.
   - The list is read with the page's other files, about once a second.

### The phone

1. **Cancel in the notification.**
   - The transfer notification has Cancel. It stops the file under way and the files still queued.
   - The bridge holds one stop for the transfer it is running (`nativeCancelTransfer`). The running native call returns "cancelled", which the retry logic never retries.
   - A download from an offer is declined, "cancelled on the phone", so the computer's Sent list says so.
   - A download's part is kept, as for any break, so the offer sent again continues.
   - The notification ends with "Cancelled".
2. **The speed.** The notification's text gains the rate beside the percent. It is computed from the bridge's counters over the last few seconds (`TransferProgress`, JVM-tested).
3. **The computer's Cancel.** It arrives as the refusal of the phone's next attempt. The notification says "Cancelled on the computer".

## Not in this change

- **A whole folder's totals:** the bytes of all its files, and which file of how many is under way. Each file of a folder is its own row, named with its folder.
- **Transfers between two computers (2h).** They run on the computers' link, outside the registry, and are not shown.
- **Pause.**

## Tests

- **Server:**
  - a row appears with a transfer and goes with it;
  - progress counts from what the other end had;
  - the mirror's shape, its stamp and its empty write;
  - a cancel request stops the transfer, refuses the next attempt with the reason, settles an offer, and is dropped when stale or for an unknown row.
- **Desktop:** the "Under way" rows from a mirror, the freshness rule, and the rate's words (`model`).
- **Bridge:** a cancelled transfer returns "cancelled", and a cancel with nothing running does nothing.
- **Phone (JVM):** the rate, and the notification's text.
- **Device (A71 and this box):**
  - a large offer and a large share, each watched on the Files page and cancelled from each side;
  - the refusal on retry;
  - sending again continues from the part.
