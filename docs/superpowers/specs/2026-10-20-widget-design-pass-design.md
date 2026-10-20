# The home-screen widgets, a design pass (I2)

**Date:** 2026-10-20. **Branch:** `feat/widget-design`. **Asked for by the owner:** "make the widgets better, the UI and UX and the overall experience", after this morning's checks put the four widgets (G1 state, G3 deck, G5 music, F11 connect) on the A71's home screen side by side.

## What the A71 showed

1. **One fixed colour.** Every widget is a pine card (`#E6234E45`) with white text, whatever the phone's light or dark mode or wallpaper colours; widgets have followed the system's dynamic colours since Android 12.
2. **Text standing in for icons.** ↻, ⏮, ▶, ⏸ and ⏭ are characters, drawn by whatever font the phone has, at sizes that do not match the app's own icons; ↻ is a 40×32 dp target, under 48.
3. **The state widget is one line.** "CPU 10 % · Mem 41 % · Disk 84 % · Bat 55 %" wraps on a phone, leaving "%" alone on the next line, and a widget made taller only shows more card.
4. **The music widget is mostly empty.** With nothing to show (nothing playing, the computer refusing, still asking) the title and buttons are blank or hidden in place, leaving the middle empty and the reason pinned to the bottom edge.
5. **The deck's buttons do not say what they do.** A key combination, a typed text, a link and a task look alike.
6. **No previews.** The widget picker shows the app icon for each; three of the four give no target cell size.

## What changes, one commit each

1. **The widgets follow the system's look.** Colours come from resources: on Android 12 and later the system's dynamic colours (a neutral surface tinted by the wallpaper, an accent from it), and its widget corner radius; before 12, the brand's pine in dark mode and a light pine in light mode. Text, secondary text, tiles and a warning colour each have one name (`widget_on_surface`, `widget_on_surface_variant`, `widget_tile`, `widget_warning`), used by all four layouts and the deck's button style.
2. **Icons, not characters.** Refresh, previous, play, pause and next are vector icons (ImageViews tinted from the theme), each a 48 dp target with the description it had.
3. **The state widget shows bars.** Tall enough (from 110 dp), it shows a row per reading: its name, a bar and the percent; small, the one line as now, with no-break spaces so a number never parts from its unit. A reading the computer gave stays; "Not answering" and "from <time>" are in the warning colour. Which layout is decided from the widget's height by `StateWidgetPlan.layout`, JVM-tested.
4. **The music widget says what it has.** Its body is either the track (title, artist, the buttons) or a message in its middle with an icon: nothing playing, the computer's refusal, no answer, asking. `MediaWidgetPlan.body`, JVM-tested, decides which.
5. **Deck buttons carry their kind.** A small icon above each label: keys, text, media, a link, a task.
6. **Previews and sizes.** Each widget names a preview layout with sample content for the picker (Android 12+) and its target cells.

## As built

- Bars from 110 dp; on Android 12+ the state widget hands the launcher both layouts by size, so it picks by the widget's real size either way up; before 12, by the height for how the phone is held.
- The deck's buttons are tiles holding a centred label with the deck sheet's icons (`Deck.iconRes`, shared); the music and deck widgets' first draw reads as asking.
- The bars' track has its own colour (the bar draws it at 30 %); the state widget's warning is a twin view in the warning colour, since a colour set from code keeps the mode it was drawn in.
- Previews and cells live in `xml-v31` (Android 12+ reads only those); cells four wide, two tall for the state, three for the music and deck; the music widget's least height 160 dp, its buttons 56 dp.

## Review

A fresh review found 0 Critical, 3 Important and 3 Minor issues. All the Important ones and one Minor are fixed, one commit each:
- **Previews and cells on Android 12+.** The `xml-v31` copies of three widget infos shadowed the edits, so on Android 12+ the previews and cells never applied. They now live in `xml-v31`.
- **The vertical space.** The 48 dp header left a two-cell music widget no room for its buttons, and the deck's tiles were shorter than their content. Fixed with the header's height, the line moved, and new minimums and cells.
- **The layout chosen in landscape.** It was chosen from the height held upright. Fixed with the size map, and with the orientation on older Android.
- **A wordless refusal (Minor).** It drew an empty message, and now says the computer refused.

Left as they are:
- The previews' sample text is English in every language. It is sample content, as a screenshot is.
- The 68 dp bar labels are owed in Tamil and at a large font.

## Left out

- Widgets drawn with Jetpack Glance: a new dependency, and the RemoteViews code is tested where it decides.
- Asking the computer more often: the asking rules (G1, G3, G5 reviews) stay as they are.

## Testing

- JVM: `StateWidgetPlan.layout` and the readings' rows; `MediaWidgetPlan.body` for a track, nothing playing, a refusal, a failure and asking.
- On the A71 (checklist §141): each widget in light and dark mode, with a light and a dark wallpaper; the state widget small and tall; the music widget in each state; the deck's icons; the picker's previews.
