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
