# E6, the phone app's design pass (design)

Date: 2026-10-19. Plan: `docs/research/2026-10-19-next-cycle.md`, E6.

## Why

U4, C5 and U6 set the phone app's rules:
- Settings as headed groups of rows, each one toggle and one TalkBack stop, with summaries cut at two lines and an ⓘ;
- a session menu of groups with segmented choices, and chevrons only where something opens;
- 48 dp targets and labels on everything;
- font scale 2.0 fits;
- every string in English, Spanish and Tamil;
- "computer" for the machine.

What the C, D and E cycles added since was built to those rules in part. An audit of the code found 26 places where it was not.

## What it changes

Grouped as they will be committed, one group each.

1. **"Computer" everywhere.** The session menu still said "Ring PC", "Lock PC", "Desktop sound on this phone" and "PiP"; connection errors, the session notification and its channels said "desktop".
2. **The session menu:**
   - the input mode's three choices stay segments at font scale 1.0 (measured at the buttons' own text, crowded like four);
   - "Show the gamepad" sits under the input mode;
   - the mode chip says what a tap does.
3. **The gamepad overlay:**
   - fits a 360 dp phone and a phone on its side (keys narrower down to 48 dp, Close clear of RB, the window's insets kept);
   - its text capped as the session chrome's is;
   - captions at least 11 sp;
   - its labels in resources, the D-pad's keys named for TalkBack.
4. **Home:**
   - the ⋮ menu's order: Sound, Send from clipboard history and Wake, then upkeep;
   - the history sheet says a tap sends, and its entries are 48 dp;
   - its Clear shows only with something to clear, and its empty state fits Home.
5. **The Health card's buttons do what they say:** Choose opens the folder picker; the other is "Open Settings".
6. **Computers by their names**, not their addresses, in Home's wake messages, the wake shortcut, the cast's, transfer, link and share notifications.
7. **Settings:**
   - a heading of its own, Input, for the mouse, trackpad speed, pen pressure and the game controller;
   - Pen pressure as a row (title and summary, the choice under it, one TalkBack stop for its name);
   - D2's mute first under Notifications and texts, the group that holds the other Do Not Disturb setting;
   - D1's switch under System, shown on every Android version;
   - the game controller's words like the mouse's;
   - the download folder's reset without a chevron and its lost folder in the warning colour;
   - the folders' Remove named;
   - Add folder as a row;
   - answering notifications hidden while sharing is off.
8. **Words:**
   - no full stop at the end of a summary;
   - one way to say notification access is needed;
   - "page" for the computer's pages;
   - no "Off until you turn it on".

## Not done

- A stylus- or controller-specific check (none here).
- Pen pressure's choices are not tied to its title for TalkBack (each choice is read alone).
- A window both narrow and short (split screen) may still crowd the gamepad.
- "Whole desktop" in the window picker, which means the screen's whole picture.

## Testing

- **JVM:** `ChoiceFit.layout`; the gamepad's layout rule for narrow and short windows (`GamepadFit`); the strings test (every string in Spanish and Tamil).
- **Instrumented, on the A71:** `ChoiceRowTest`: four choices segments at 1.0 and a list at 2.0; three choices segments at 1.0, crowded on one line at 2.0, and a list when even crowded they do not fit.
- **Device (checklist §125), on the A71:**
  - the session menu at font scale 1.0 and 2.0;
  - the gamepad in portrait and landscape;
  - Home's menu and the history sheet;
  - the Health card's Choose;
  - Settings' groups;
  - TalkBack on the new rows;
  - Spanish and Tamil.

## As built

The eight groups, one commit each. `ChoiceFit.layout` picks segments, crowded segments or a list; `ChoiceRow` measures in the face it draws and takes no label under a Settings row. `GamepadFit` lays the gamepad out for narrow and short windows. `rememberDownloadFolderPicker` is shared by Settings and the Health card.

**Checked on the A71 (checklist §125):**
- `ChoiceRowTest`'s five cases pass;
- Home's menu in its new order;
- Settings' Input group;
- Pen pressure saving its choice;
- font scale 2.0, where three choices stay segments and the summary is cut with its ⓘ.

## Review

**Fresh review: 0 Critical, 2 Important, 5 Minor.**

- **Fixed:**
  - the instrumented test still expected three choices at 2.0 to be a list: now crowded on one line, with a narrower case for the list;
  - the cast's "controlled" notification still named the address;
  - minors: the wake shortcut and a share named by address, and Pen pressure's choice redrawing on save.
- **Left as they are:**
  - Pen pressure's choices are read alone by TalkBack;
  - a narrow and short window may crowd the gamepad;
  - a stick is read by its caption twice;
  - the D2 mute and the hold-back sit in one group but not side by side (the hold-back shows only while sharing);
  - "Whole desktop".
