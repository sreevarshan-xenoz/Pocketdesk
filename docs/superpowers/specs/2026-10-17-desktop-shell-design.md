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
7. **Fonts.** At start, the system's fonts for other scripts are added after egui's own, in order: on Windows Nirmala UI (Indic), Segoe UI (Arabic, Hebrew and more), Leelawadee UI (Thai), Microsoft YaHei (Chinese), Yu Gothic (Japanese) and Malgun Gothic (Korean), from the Windows fonts folder; on Linux the Noto families fontconfig lists (`fc-list`). A font that is missing is skipped. What was found is listed in Diagnostics.
8. **Colours as tokens,** with a light and a dark value each, chosen from the visuals' dark mode; every pair at least 4.5:1 against its panel, held by a test computing WCAG contrast.
9. **Remembered.** The window's size and the last page are kept between runs (eframe's persistence). `pocketdesk-app --page <name>` opens on a page, for checks and for links from the tray or a notification later.

## Not in U1

Per-view notices (Messages, Notifications, Phone files, Computers, KDE) move to the shared component in U5, with the rest of the wording. The Overview's phone panel is N6.

## Tests

The navigation table (every page in exactly one group, names unique, `--page` parsing), the notice's lifetimes, the font candidates per OS and the skipping of missing ones, and the contrast of every colour token in both themes.

## Device check

The app on this box, at 720×480 and larger, each page opened with `--page`; screenshots of its own window only (no input into it).

## As built

- **Navigation:** `desktop/src/nav.rs` holds the groups, each page's name, purpose, `--page` key and whether it scrolls its own lists; the Overview is today's Status page under a new name.
- **Save in view:** `SettingsForm::save_bar`, shown in a bottom panel while the form has changes, or while what is saved has a problem (a hand-edited config); the restart line stays in the page. Undo keeps a saved change waiting for a restart.
- **The notice:** `desktop/src/notice.rs`. Each place that says something gives its kind: done, an error, or work under way (a start, a restart, a setting's change), which stays until its outcome replaces it, 5 minutes at most. Done goes after 6 s, an error after 12 s, any of them with Dismiss (a word, which every font has and a screen reader says). "Copied." shows beside the debug report's and the pairing link's copy buttons for 3 s.
- **Off the UI thread:** `run_setting` / `finish_setting`, beside the start and restart job; the two switches are off while a change runs.
- **Colours:** `desktop/src/theme.rs`, four tokens (good, bad, warn, muted), each at least 4.5:1 on its panel, held by a test; every one-off colour in the views uses them.
- **Fonts:** `desktop/src/fonts.rs`, mapped with `memmap2`, so the about 30 MB of fonts are not read into memory, each checked to parse (skrifa) before egui is given it, on a thread of its own. egui shapes each run of one font (harfrust) but has no bidirectional layout, so a line mixing right-to-left and left-to-right text can come out in the wrong order. Diagnostics lists the fonts found, or says it is still looking.
- **AccessKit:** checked through Windows UI Automation, which lists the navigation, group titles, the page's heading and text by name.
- **Persistence:** eframe's `persistence` feature (it adds `ron`): `app.ron` in `%APPDATA%\Pocketdesk\data` on Windows and `$XDG_DATA_HOME/pocketdesk` (`~/.local/share/pocketdesk`) on Linux, or `app-window.ron` beside the config when `POCKETDESK_CONFIG_DIR` is set. It holds the window's size and place, egui's layout and options, and the page last chosen under the key `page` (never Pair, whose page makes a code); no text typed in the app. `--page` opens a new copy on a page; with a copy already running, that copy comes to the front on its own page.
- **Checked on this box** (the app's own window, captured with `PrintWindow`; no input into it): Settings at 720×480 with the navigation and a scrolling page, file names in six scripts drawn, the accessibility tree, and the page kept across runs.

## Review

A fresh review, fixed one commit each:

- **A notice's kind guessed from its words:** real errors showed as done, and a success naming a file such as `failed-login.log` as an error; and work under way cleared after 6 s, before its outcome. Each place now gives its kind, and work under way stays until replaced.
- **The window smaller than its pages:** at 520×360 the navigation's last group was cut off and wide rows ran off the page. The window starts at 860×600 (at least 640×440), the navigation scrolls, and wide rows wrap.
- **Fonts parsed on the UI thread, and a font that failed to parse stopped the app** (egui panics on one). They are found on a thread of their own and each is checked first.
- **egui's lesser text below 4.5:1 in the light theme.** Its weak text colour is set per theme.

And the minors: the page kept being the one showing (Overview after hiding to the tray) or Pair, Undo forgetting a change waiting for a restart, a saved config's problem unsaid, `--page` panicking on an argument that is not Unicode, the fonts line in Diagnostics only after a run and saying "none found" while still looking, and these docs.
