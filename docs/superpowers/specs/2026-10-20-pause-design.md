# Pause the phones on the computer for a while (K2a)

**Date:** 2026-10-20. **Branch:** `feat/pause`, on K2b. The K cycle's second item, its second part.

## What is wrong

- **Turning everything off for an hour means turning many switches off, and then on again.** A screen share, a talk or a call wants the phones quiet on the computer for a while. Today that means toggling Settings and each phone's permissions, then remembering to turn them back.
- **Asked for:** KDE's forum, 2026-10-11, "pause KDE Connect temporarily": a call on a muted phone paused a video during a screen share.

## What changes

- **Pause for an hour, for 8 hours, or until resumed:**
  - from the tray's menu;
  - from the Overview ("Pause" with the three, or "Resume" while paused);
  - from the CLI (`pocketdesk pause --for <minutes> | --until-resumed | --resume`, with `--json`).
- **While paused, the computer holds:**
  - phones' and KDE devices' notifications, calls among them: not shown, as a muted app's are (K2b), but still counted against a phone's limit, so a week's pause does not unbound a phone. They stay on the phone; one that offers buttons or a reply waits on the Notifications page; the rest leave only their app's name in the event log;
  - texts: kept on the Messages page, not shown;
  - links from phones: not opened and not kept. Activity names the link's site; the owner sends it again after. Keeping them for later is left out;
  - a phone ringing this computer (the siren): silent.
  - the other pop-ups a phone causes: a received file's notification (the file is still saved and on the Files page), a phone's low-battery warning (its battery is still on the Overview), and a KDE device's ping.
- **What it does not hold:**
  - sessions, which the owner chose to open;
  - files;
  - the clipboard;
  - anything the owner does from the computer.
  - the deck's presses (F1), which the owner makes from the phone: a deck link still opens;
  - hooks and Home Assistant, which see every event as before: they are the owner's own setup.
- **It shows that it is paused:**
  - the tray's menu, after Open and Pair, says "Paused, 42 min left" (or "until resumed") and has Resume;
  - the Overview says what is held and has Resume.
- **It ends by itself** at its time; "until resumed" never does, so it is never the default.
- **The state:** `pause.json` in the state folder, owner-only, changed under a lock (`core::local::pause`, tested). The server reads it at each held thing, so a pause applies to the next one.

## Left out

- "Until tomorrow": it needs the computer's time zone, and an 8-hour pause does the same at night.
- The phone saying the computer is paused: it needs a packet, and the phone's own Do Not Disturb (D2) already covers the phone's side.
- The clipboard: a pause is about interruptions, and a copy is the owner's own act.

## Testing

- **Unit:** the state (a pause for minutes ends at its time, "until resumed" never does, resume ends it, a broken file reads as not paused and is never written over); the tray's menu while paused; the CLI's arguments.
- **Server:** each held thing asks `paused::now` (tests answer it per thread, never from the owner's pause), and a test holds notifications, texts, links and the ring.
- **Harness:** the Overview's Pause and Resume, and a pause that cannot be read.
- **By hand (checklist §152):** the tray's items; a phone's notification and a link while paused, and after.

## As built

- **Core:** `core::local::pause` (`pause.json`: `since_unix`, `until_unix` or none), changed under the store lock, written owner-only and atomically, never over a file that does not parse.
- **Server:** `paused::now`, asked by the notification paths (phones' and KDE devices', `held_here`), texts (`shows_texts`), links (`LinksPlugin`'s injectable `paused`), the siren (`sounds`), a received file's announcement, a phone's battery warning and a KDE device's ping. In tests it answers per thread.
- **Desktop:** the tray's menu (`tray_menu::with_pause`, both trays), the Overview's row (read once a second, a broken file said in red), and `pocketdesk pause` (`local_api::pause_in`).

## Review

A fresh review (0 Critical, 3 Important, 11 Minor), fixed one commit each:

- **Important:**
  - the docs said everything held was kept, where only answerable notifications and texts are (now said honestly; keeping links for later is left out);
  - received-file toasts, battery warnings and KDE pings still popped up (now held; the deck, hooks and Home Assistant named as not held);
  - the spec claimed tests that did not exist, and server tests read the owner's own pause (now a per-thread answer, and tests for each hold and the CLI's arguments).
- **Minor:**
  - `pause --json` said "not paused" for a broken file, and the Overview offered a Pause that failed;
  - a tray-program failure went to no console (it now opens the Overview);
  - a pause left a phone's stream out of the flood guard;
  - the Overview read the file on every frame;
  - words: `quiet`'s doc comment, "1 more minutes", the notice's hours, "rings", "Pause…" against "Pause", "leads with".
- **Left:** the Linux tray names its items by position, so a click racing a pause's start or end can land on the next item; it predates the pause, which only makes it likelier.
