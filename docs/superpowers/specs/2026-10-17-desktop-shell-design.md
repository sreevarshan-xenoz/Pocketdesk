# The desktop app's shell (U1)

Date: 2026-10-17. Plan: `docs/research/2026-10-17-finish-and-polish.md` §U1. From the UI audit: D-L1, D-L2, D-W1, D-W3, A1, A6, L1.

## The problem

- **Pages that cannot scroll.** Only Files, Logs, Activity, Messages, Notifications and the Phone tab scroll. At the default 720×480 window, Settings' Save button and its Computers and KDE Connect sections are off-screen, and Settings is about 1,500 px tall, so this holds maximized too. Devices, Sessions, Pair and Diagnostics also overflow.
- **A tab row that does not fit.** Twelve tabs and the title sit in one row that does not wrap, about 900 px against the 720 px default.
- **A notice that never clears.** One line under the tabs shows the last thing said ("Saved.", "Not changed: …") forever, on every page, errors and successes alike. Copy buttons say nothing.
- **A frozen window.** "Send to phone" in the file manager and "Send from the browser" run PowerShell and `browser-host` on the UI thread, about 1–2 s each.
- **No screen reader.** eframe is built without AccessKit.
- **Boxes for most scripts.** Only egui's default fonts ship, so Tamil, other Indic scripts, CJK, Arabic and Thai in phone notifications, texts, names and file names show as empty boxes.
- **Hard to read in the light theme.** The hint, good and bad colours are about 2.8:1, 3.2:1 and 3.5:1 on the light panel.

## The design

1. **Navigation down the left, in groups:** Overview (today's Status); Phones and computers: Pair, Devices, Sessions; Sharing: Files, Messages, Notifications, Phone files; Activity; Settings; Help: Diagnostics, Logs. The title above. A `nav` module holds the groups and each page's name and one-line purpose, so a test holds every page to exactly one group.
2. **Every page scrolls.** Pages that keep lists of their own (Files, Logs, Activity, Messages, Notifications, Phone files) keep their scroll areas; the others are wrapped in one. Each page opens with its heading and its line of purpose.
3. **Save in view.** While Settings has changes, Save and Undo sit in a bar at the bottom of the window, with the reason Save is off when it is.
4. **One notice.** A notice has a kind: done, information or error, each with its colour. Done and information go after 6 s, an error after 12 s, any of them at the next notice or with its close button. A copy says "Copied.".
5. **Off the UI thread.** The two settings run on a worker as Start and Restart already do, with "Adding…" or "Removing…" until done.
6. **AccessKit** on: eframe's `accesskit` feature.
7. **Fonts.** At start, the system's fonts for other scripts are added after egui's own, in order: on Windows Nirmala UI (Indic), Segoe UI (Arabic, Hebrew and more), Leelawadee UI (Thai), Microsoft YaHei (Chinese), Yu Gothic (Japanese) and Malgun Gothic (Korean), from the Windows fonts folder; on Linux the Noto families fontconfig finds (`fc-match`). A font that is missing is skipped. What was found is listed in Diagnostics.
8. **Colours as tokens,** with a light and a dark value each, chosen from the visuals' dark mode; every pair at least 4.5:1 against its panel, held by a test computing WCAG contrast.
9. **Remembered.** The window's size and the last page are kept between runs (eframe's persistence). `pocketdesk-app --page <name>` opens on a page, for checks and for links from the tray or a notification later.

## Not in U1

Per-view notices (Messages, Notifications, Phone files, Computers, KDE) move to the shared component in U5, with the rest of the wording. The Overview's phone panel is N6.

## Tests

The navigation table (every page in exactly one group, names unique, `--page` parsing), the notice's lifetimes, the font candidates per OS and the skipping of missing ones, and the contrast of every colour token in both themes.

## Device check

The app on this box, at 720×480 and larger, each page opened with `--page`; screenshots of its own window only (no input into it).
