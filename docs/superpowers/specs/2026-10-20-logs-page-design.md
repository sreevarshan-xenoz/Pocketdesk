# The Logs page reads less and finds more (J1)

**Date:** 2026-10-20. **Branch:** `feat/logs-page`. From the desktop audit's ranked list (item 9), left out of I3.

## What is wrong

- **It reads the whole log every second.** While the page is open, the refresh reads the newest daily log in full and splits it into lines, to show the last 400. A debug-level log grows to tens of megabytes in a day, so the window hitches once a second.
- **It cannot be searched.** 400 lines in one colour, no filter, no copy, no way to the file.

## What changes

- **Only the tail, only when it changed.** The page notes the log file's size and modification time. When they change it reads at most the last 256 KiB, dropping the cut first line, and keeps the last 400 lines (`model::log_tail`, tested on a large file and a cut line). Otherwise it reads nothing.
- **A filter.**
  - A box keeps the lines holding its words, case ignored.
  - "Warnings and errors only" keeps the lines the server logged at WARN or ERROR.
  - Both apply together, and the page says how many lines it shows of how many (`logs_view::shown`, tested).
- **Colour.** A warning is in the warning colour, an error in the error colour (`logs_view::level`, tested).
- **Copy and Open log folder.** Copy puts the lines shown on the clipboard. Open log folder opens the file manager at the file.
- **The page owns its reading.** A `LogsView` holds the lines, what it last read and the filter, as the other pages' views do. The app's refresh no longer reads the log.

## Left out

- Older days' logs, and following another file than the newest.
- A message the server wrote over several lines: its later lines carry no level, so "Warnings and errors only" leaves them out.

## Testing

- **Unit:** the tail of a file larger than 256 KiB; a cut first line dropped; nothing read again while the size and time are the same; the filter, the warnings switch and both together; each line's level.
- **Harness:** the page shows a fixture log's lines, and the filter narrows them.
- **By hand (checklist §143):** the page with a large debug log, no hitch; Copy; Open log folder.

## As built

As designed. The view is `desktop/src/logs_view.rs`; the reading, `model::log_stamp` and `model::log_tail`.

## Review

A fresh review found 0 Critical, 0 Important and 4 Minor. All four were fixed:

- **A whole line dropped:** a tail read that began on a line's first byte dropped that line as cut. It now reads from one byte earlier.
- **An empty log hidden:** a log just rotated and still empty read as "No log file yet" and hid Open log folder.
- **The filter a frame late:** the lines were counted before the filter's box was drawn. A harness test now types into the filter.
- **The spec:** it claimed a harness test of the filter that did not exist yet, and did not name the multi-line limit.
