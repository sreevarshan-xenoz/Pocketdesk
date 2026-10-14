# Pocketdesk brand

A desktop window tucked into a pocket: your computer, carried with you.
Drawn from the concept in `src/concept.png`.

| File | What it is |
|---|---|
| `pocketdesk-mark.svg` | The mark, in colour, on transparent |
| `pocketdesk-mark-mono.svg` | The mark in one colour (`currentColor`), for small sizes and monochrome themes |
| `pocketdesk-icon.svg` | The app icon: the mark on the navy tile |
| `pocketdesk-icon-512.png`, `-1024.png` | The app icon, rendered |
| `pocketdesk-mark-512.png` | The mark, rendered |
| `pocketdesk-banner.png` | The README header (rendered from `src/banner.html`, font Outfit) |

Colours: navy `#0B1230`, pocket `#5AD1FF` → `#2F7BFF` and `#2E8BFF` → `#5B4BFF`,
window rim `#8FD8FF` → `#7B6CFF`, window `#0B1222`, dots `#FF5F57` `#FEBC2E` `#28C840`.

Where it is used:
- the Android launcher icon (`android/app/src/main/res/drawable/ic_launcher_foreground.xml`, and `ic_launcher_monochrome.xml` for themed icons);
- the desktop app's window and tray icon (`desktop/src/icon-32.rgba`, `icon-64.rgba`, rendered from the mark).

To re-render after changing an SVG, open it (or `src/banner.html`) in a
headless Chromium with `--screenshot`, at a device scale of 2.
