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
  - **More files dropped while it asks** join the files waiting, each path once, even if a target has gone meanwhile.
  - **Leaving the Files page** (another page, or the window hidden to the tray) drops the waiting files and says so.
  - **Several sent at once** get one notice for them all, or how many went and which did not, and why.
  - **Esc** cancels only when no text box has focus.
- **None can:** a drop says so, as before.
- **Which targets count** is judged on the state read at the drop, not up to a second before it.
- **A folder is called a folder** in the question and its notices.
- **If every target goes while the question waits,** the files are not sent and the page says why.
- **The rule:** `model::drop_to(snapshot)` says Send, Ask or Nobody. It is tested.

## Left out

- Remembering a choice across drops: each drop with several targets asks again.
- "Choose file…" and the path box: they send to the "To" choice shown beside them, as before.

## Testing

- **Unit:** `drop_to` with no target, one phone, one computer, and two (a phone and a computer); `join_drops`; `files_and_folders`.
- **Harness:** a file dropped through egui's `DroppedFile` with no server running opens Files and says nothing can take it. The asking path needs a running server's targets, which the harness cannot fake, so it is checked by hand.
- **By hand (checklist §147):**
  - a drop with one phone linked;
  - a drop with two, each button and Cancel;
  - a second drop while it asks;
  - the phone leaving while it asks.

## As built

As designed, with the review's fixes above. The rule is `model::drop_to`; the question is `App::drop_question`; several files go through `App::send_files`.

## Review

A fresh review found 0 Critical, 2 Important and 6 Minor.

**Important, fixed one commit each:**

- **A waiting question was invisible off the Files page and never expired.** Leaving the page now drops the waiting files and says so.
- **A drop was judged on a snapshot up to a second old,** so a phone that had just linked was not asked about.

**Minor, fixed:**

- a second drop going around the question, and a path dropped twice sent twice;
- folders counted as files;
- only the last file's notice shown;
- a notice a frame late;
- Esc in a text box cancelling the files;
- the spec's checklist section and docs.
