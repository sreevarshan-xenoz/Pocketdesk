# Find a setting on the desktop (J2)

**Date:** 2026-10-20. **Branch:** `feat/settings-search`. From I3's spec, where it was left out.

## What is wrong

- **Settings is long.** It holds about 90 rows in eight collapsing sections, and most sections start closed.
- **Finding one setting means guessing its section.** "Where is the laser pointer?" means opening sections one by one.

## What changes

- **A box at the top of Settings, "Find a setting".** Ctrl+F puts the cursor in it while Settings is open, and Esc empties it.
- **Typing narrows the page.**
  - **Rows:** a row shows when every word typed, case ignored, is in its name or its config key (`laser_pointer` reads as "laser pointer").
  - **Sections:** a section whose own title matches shows whole. A section with nothing matching is not drawn. A section with a match is open.
  - **The extras each section draws beside its rows** are held to the same words by their own names:
    - "Send from the browser" and the file manager's "Send to phone" (Phones);
    - the other computers (Computers);
    - KDE Connect's devices (KDE Connect).
- **Nothing matches:** the page says so under the box.
- **One rule, tested.** `settings_view::matches`, `row_shown`, `section_shown` and `extra_shown` are pure.
  - The rows read the words from the page's context data, so `row()` hides itself.
  - The section loop asks `section_shown`.
- **The save bar, the restart notice and a config error** show as before, whatever is typed.

## Left out

- Matching a row's notes or its control's text, such as a combo box's choices.
- Sections a search opened stay open once the box is emptied. egui keeps a header's state, and they can be closed by hand.

## Testing

- **Unit:**
  - words against a name and a key;
  - a section's title showing all of it;
  - a section with no match hidden;
  - an extra by its own name and by its section's title;
  - an empty query showing everything.
- **Harness:** typing "wake port" into the box shows Wake's "Outside wake port" with its section opened, and hides Connection's ports and Wake's other row. A query matching nothing says so.
- **By hand (checklist §144):** Ctrl+F and Esc; a search on each section's extras; the page at the minimum width.
