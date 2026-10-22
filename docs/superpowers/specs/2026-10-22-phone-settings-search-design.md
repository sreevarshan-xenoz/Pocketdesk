# O8, find a setting on the phone (design)

Date: 2026-10-22. Plan: `docs/research/2026-10-22-o-cycle.md` §O8.

**The problem.** The phone's Settings has grown to twelve groups and more than a hundred rows, and the one you want takes scrolling to find. J2 gave the desktop a search; the phone had none.

## What

**The search box.** A "Find a setting" box sits at the top of Settings.
- **What it matches:** a row shows when every word typed is found, in any case and any order, in its title, its summary or its group's name. This is J2's rule (`SettingsSearch`, JVM-tested).
- **Groups:** a group's heading shows only while one of its rows is found. A group's name matches all of its rows.
- **When nothing matches:** the screen says "No setting matches …".
- **Clearing it:** Clear empties the box, and so does Back while it holds anything.

**How rows take part.**
- *Ordinary rows:* every `SettingRow`, so every switch and link, checks itself against the search and tells its group whether it was found (`rememberSettingShown`).
- *Other rows:* those that are not `SettingRow`s are wrapped in `SettingsBlock`, with the words a search finds them by. These are the phone's name, the theme choices, the version and the two speed sliders.
- *Groups:* each group is a `SettingsGroup`, which hides its heading when nothing in it is found.

## Not done

- **Finding words a row does not show,** such as synonyms or a setting's key. The phone's rows have no keys, unlike the desktop's.
- **Highlighting the words found.**

## Checks

- **Unit tests:** nothing typed shows every row; every word must be found, across title, summary and group; a group shows while a row is found; words split on any space.
- **On the OPPO:** a search, a group's name, nothing found, Clear and Back (checklist §191).

## Review

A fresh review found 0 Critical, 3 Important and 7 Minor issues. The OPPO check found one more.

**Found on the OPPO:**
- **"ring" found "Keep screen on"**, through "dur*ing*" and "stream*ing*". A word typed now matches the start of a word.
- **Accents:** "conexion" now finds "conexión".

**Important, fixed:**
- **Choices left showing while searching.** The choices under a row stayed on screen with their row hidden: the pen's pressures, the deck's columns, the notification wait, and each shared folder's rows. They now hide with it.
- **A flash at each key.** A new filter for each search started with nothing found, so the headings and "No setting matches" flashed at every key. One filter now lasts the screen's life, and rows tell it as they are drawn.
- **Untested state.** The filter's state is now tested (`SettingsFilterTest`).

**Minor, fixed:**
- the keyboard's Search key closes it;
- "No setting matches" is said to TalkBack as it comes.

**Minor, not done:**
- a hidden row's ⓘ comes back a frame late;
- a row can stop matching when its own switch changes its summary;
- the wrapped groups keep their old indentation.
