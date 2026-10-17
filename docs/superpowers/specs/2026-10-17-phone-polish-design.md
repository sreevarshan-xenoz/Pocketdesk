# The phone's Home card, Add form, Settings and session chrome (U4)

Date: 2026-10-17. Plan: `docs/research/2026-10-17-finish-and-polish.md` §U4. From the UI audit: P-V1–P-V7, A2–A5.

## The problem

- **Home's card has no visible main action.** The whole card connects when tapped, but nothing says so; six equal text buttons sit in two ragged rows (Remote, Show screen, Wake; Check connection, Wake settings, Rename, Remove), with Remove beside Remote. TalkBack reads the card as "double tap to activate".
- **Ports are the card's subtitle** ("stream 4716 · control 1716"): noise where a status belongs.
- **The Add computer form looks unfinished:** no top bar or back arrow, content centred so the heading jumps when Advanced opens, a switch touching its label and tappable only on itself, no example address, no URI keyboard, no Go key.
- **Settings is one long list:** about twenty unrelated switches under "Session", summaries of three to seven lines, "Off by default." on many (the switch already says it), rows indented unevenly, and icons reused with no meaning (Wi-Fi seven times, a pushpin three times).
- **The stats overlay is always on** and reads like debug output (`fps`, `kbps`, `rtt`, `e2e`, `drops`, a monospace caption).
- **The session menu hides its choices:** Input mode, Quality and Pane change on each tap behind a chevron that suggests a sub-page; Ring and Lock show chevrons yet act at once; one "Desktop" group holds fourteen mixed rows.
- **Accessibility:** rows are two TalkBack stops each, headers are not headings, the trackpad speed slider has no name, small session controls.

## The design

1. **The card:** a filled **Connect** button, **Remote** and **Show screen** beside it, and a ⋮ menu for the rest (Wake, Wake settings, Check connection, Rename, Remove, Remove last and in the error colour). The card's click is labelled "Connect" for accessibility.
2. **A status line** under the name instead of ports: "On this network", "Paired" / "Not paired", and the address; the ports only when they are not 4716/1716.
3. **The Add form:** a top bar with a back arrow and the title, content from the top, the switch in a whole-row toggle, the address field with an example ("192.168.1.20 or mypc.local"), the URI keyboard and Go, which connects.
4. **Settings in groups**, each with a heading: This phone; Session (keep screen on, reconnect, mouse, trackpad speed, another phone drives, haptics, connection details); Files and links; In the background (clipboard, files, ring, battery, media, screenshots); Notifications and texts; Automation; Pairing; Wake; Appearance; System; About. No leading icons on switch rows (they carried no meaning); each row is one toggle target and one TalkBack stop; a summary shows two lines, and an ⓘ shows the whole when it is longer; "Off by default." is cut from the summaries.
5. **"Show connection details"** (off by default): with it off, the session shows a coloured dot (good, fair, poor) by Exit instead of the numbers; on, the overlay as today.
6. **The session menu in three groups**, Share (clipboard, history, send text, type the phone's clipboard, files, tasks, the mic), Computer (sound, ring, lock, blackout, privacy) and This phone (pairing, picture-in-picture), with segmented choices for Input mode, Quality and Pane, and no chevron on rows that act at once.
7. **Touch targets** of at least 48 dp in the session chrome, headings marked as such, the speed slider named.

## Tests

JVM: the card's status line (`HostStatus`: on this network, paired, ports shown only when not the defaults), the quality dot's thresholds (`LinkQuality` from fps, rtt and drops), and the summary rule (two lines, ⓘ when longer, is a rendering choice, so not tested).

## Device check

The A71: Home's card (Connect, the ⋮ menu, the status line), the Add form, Settings scrolled through in each group, the session's dot and the details switch, the session menu's groups and segmented choices; TalkBack on a card and a Settings row.

## As built

- **The card** (`HomeScreen.kt` `HostCard`): the status line is `HostStatus.parts` (JVM-tested) in words, "On this network" in the accent colour, "Connects at launch" for the computer that opens at launch. Connect is filled and Remote and Show screen are outlined, with 16 dp sides so the three fit one row on a phone (found on the A71: Show screen wrapped). The menu is a `DropdownMenu` under a ⋮ named "More for <computer>": Wake (when the computer can be woken), Wake settings, Check connection, Rename, a divider, Remove in the error colour. The card is `clickable(onClickLabel = "Connect")`.
- **The Add form** (`ConnectScreen.kt`): a `TopAppBar` whose back arrow replaces Cancel; "Computer's address" with the example as its placeholder, `KeyboardType.Uri` and `ImeAction.Go`, which connects when the form could; "Connect to it when the app opens" as one `toggleable` row.
- **Settings** (`SettingsScreen.kt`, `SettingsParts.kt`): `SettingsHeader` (a heading to TalkBack), `SettingSwitch` (one `toggleable` row, `Switch(onCheckedChange = null)`), `SettingLink` (a chevron) and `SettingRow`, whose summary is cut at two lines and gets an ⓘ (`IconButton`, "More about <title>") opening the whole in a dialog. Groups: This phone; Session (keep screen on, reconnect, mouse, trackpad speed, another phone drives, haptics, connection details); Files and links (retry, files over the internet, links, the folders the computer may open); In the background (clipboard, files, ring, battery, media, screenshots, then the background status rows and the reminder); Notifications and texts; Automation; Pairing; Wake; Appearance (the theme as a `selectableGroup`); System; About. Where a feature lacks notification access, its warning is no longer a tap target (the old row opened the access page, which made the switch hard to turn off): an "Allow notification access" row follows it. "Off by default." is cut from the three summaries that said it; "Background" became "In the background", and the notifications switch says whose notifications.
- **The dot** (`StatsHud.kt`, `LinkQuality.kt`): `LinkQuality.of` (JVM-tested) takes the round trip (good to 80 ms, fair to 200), the end-to-end delay (good to 120 ms, fair to 250) and frames lost in the last 3 s (`DropWindow`; 2 fair, 10 poor) and keeps the worst; not the frame rate, which a still desktop keeps low on a perfect link. A dot and a word ("Good connection"), in the HUD's place rather than beside Exit, shown only while the stream is up (found on the A71: a refused session read "Poor connection"). With `Prefs.connectionDetails` on, the old numbers, and only then the computer's battery is polled.
- **The session menu** (`QuickSettingsSheet.kt`): Screen and input (input mode, view-only, quality, pane, window, monitor), Share, Computer, This phone (desktop sound, pairing, picture-in-picture). `SingleChoiceSegmentedButtonRow`s set the input mode, quality preset and pane directly (`setQuality`, no cycling), the four quality options without the check mark to fit. Rows take the sheet's colour (found on the A71: the list's own colour painted bands).
- **Targets**: the mode and keyboard chips and the shortcut chips take `minimumInteractiveComponentSize()`, Exit is 48 dp across; the screen chips grow to 36 dp and rely on Compose widening their touch, so a third screen still fits the row.

## Review

A fresh review (0 Critical, 0 Important, 7 Minor), fixed one commit each where it asked for a change:

- The screenshots and texts sections never read notification access again on return, so the new Allow row stayed after access was granted (`rememberResumes`).
- A tap on the chosen quality could not retry a failed send (the old cycling always sent).
- Four segmented options risked an ellipsis in Spanish and Tamil: narrower sides, a smaller face, two lines; checked on the A71 in both.
- An indented row's padding sat outside its toggle; `onRowClick` had no caller; an info button is a second TalkBack stop, as the parts' doc now says.
- Kept: the shortcut chips' 48 dp height, which makes the bottom chrome taller in landscape (the targets are the point); the design's "three groups" is four as built (This phone holds the desktop's sound, pairing and picture-in-picture).

Seen for U6: Home's bottom row ("Scan pairing code", "Add computer") breaks a Tamil label one word per line, and Spanish calls direct touch "Tacto directo" on the chip but "Toque directo" in the menu.

