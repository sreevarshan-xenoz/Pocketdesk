# Find a setting on the desktop (J2)

**Date:** 2026-10-20. **Branch:** `feat/settings-search`. From I3's spec, where it was left out.

## What is wrong

- **Settings is long.** It holds some fifty settings, plus the lists beside them, in eight collapsing sections, and most sections start closed.
- **Finding one setting means guessing its section.** "Where is the laser pointer?" means opening sections one by one.

## What changes

- **A box at the top of Settings, "Find a setting".** Ctrl+F scrolls to it and puts the cursor in it while Settings is open. Esc in the box empties it, and that press does nothing else. The search is kept when you leave the page.
- **Typing narrows the page.**
  - **Rows:** a row shows when every word typed, case ignored, is in its name, its config key (`laser_pointer` reads as "laser pointer"), a setting drawn in it (Home Assistant's broker in its row), or its section's title.
  - **Sections:** a section whose own title matches shows whole. A section with nothing matching is not drawn. A section with a match is open, and stays open while the search lasts.
  - **Held sections:** a section held open by a problem, or KDE Connect's with a pairing request waiting, is drawn whole whatever is searched.
  - **Other systems:** a row this system does not draw (the screen-sharing grant on Windows, the clipboard key on Linux) is not counted as found.
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

## As built

As designed, with the review's fixes above. The box's hint is not read through AccessKit, so the harness finds the box as the page's first text box. Settings' snapshots are drawn again with the box.

## Review

A fresh review found 0 Critical, 2 Important and 5 Minor.

**Important, fixed one commit each:**

- **Settings drawn in another's row could not be found.** Home Assistant's broker, user and password, and the event log's details switch, opened Automation empty. A test now holds every setting to the row that draws it.
- **A held section vanished under a search**, together with the field the save bar named or KDE's Allow.

**Minor, fixed:**

- rows another system draws;
- Esc also cancelling a question drawn later in the frame;
- words across a row and its section's title;
- Ctrl+F on a scrolled page;
- the spec's count, sections forced open while searching, and the checklist section.
