# Dropped files ask where they go (J5)

**Date:** 2026-10-20. **Branch:** `feat/drop-target`. From the desktop audit's ranked list, left out of I3.

## What is wrong

- **Files dropped on the window go to the Files page's "To" choice.** That is whatever it last showed, or the first phone that can take files.
- **With several reachable,** two phones, or a phone and another computer, a drop goes to one of them without being asked. The owner may not have looked at the Files page this hour.

## What changes

- **One phone or computer can take files:** a drop is sent to it at once, as before.
- **Several can:**
  - **The files wait.** The Files page opens with a question at its top: "Send 3 files to:", then a button per phone or computer by its name, and Cancel. Esc cancels as well.
  - **A button** sends them all there, and that becomes the "To" choice.
  - **Cancel** sends nothing and says so.
  - **More files dropped while it asks** join the files waiting.
- **None can:** a drop says so, as before.
- **If every target goes while the question waits,** the files are not sent and the page says why.
- **The rule:** `model::drop_to(snapshot)` says Send, Ask or Nobody. It is tested.

## Left out

- Remembering a choice across drops: each drop with several targets asks again.
- "Choose file…" and the path box: they send to the "To" choice shown beside them, as before.

## Testing

- **Unit:** `drop_to` with no target, one phone, one computer, and two (a phone and a computer).
- **By hand (checklist §147):**
  - a drop with one phone linked;
  - a drop with two, each button and Cancel;
  - a second drop while it asks;
  - the phone leaving while it asks.
