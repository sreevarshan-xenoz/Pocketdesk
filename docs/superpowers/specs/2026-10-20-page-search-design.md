# Search on Activity and Clipboard (J4)

**Date:** 2026-10-20. **Branch:** `feat/page-search`. From the desktop audit's ranked list, after J1's Logs filter and J2's Settings search.

## What is wrong

- **Activity keeps the last 500 events.** Its kind chips narrow them, but "the file from this morning" or "what the Pixel did" still means scrolling.
- **Clipboard can keep up to 100 texts.** There is no way to find the address copied yesterday but by reading them all.

## What changes

- **Activity gets a Search box beside its kind chips.** An event shows when every word typed, case ignored, is in its line as shown: the phone's name and the event's words, a file's or link's name among them. The kind chip and the search apply together.
- **Clipboard gets a Search box over its list.** A text shows when every word is in the text itself or in where it came from ("From Pixel 8").
- **Both say when a search finds nothing**, with Clear search beside it.
  - Activity: "Nothing here matches" the words.
  - Clipboard: "No kept text holds" the words.
- **Ctrl+F** puts the cursor in the page's search box on both pages, as on Settings (J2). Esc in the box empties it.
- **One rule:** `words::holds_all(query, texts)`, tested. Each page's choice is a pure function, `activity_view::shown` and `clipboard_view::shown`, both tested.

## Left out

- Searching what an event's line does not show, such as a notification's hidden content.
- Searching a clipboard text past the 64 KiB the history keeps of it.

## Testing

- **Unit:**
  - `holds_all`: every word, case ignored, in any of the texts; nothing typed holds;
  - Activity's search with and without a kind chip;
  - Clipboard's by text and by origin.
- **Harness:** Clipboard narrowed by typing into its box, and the nothing-found line with Clear search.
- **By hand (checklist §146):** both pages' boxes at the minimum width; Ctrl+F and Esc.
