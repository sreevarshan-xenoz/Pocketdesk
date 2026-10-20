# Pause the phones on the computer for a while (K2a)

**Date:** 2026-10-20. **Branch:** `feat/pause`, on K2b. The K cycle's second item, its second part.

## What is wrong

- **Turning everything off for an hour means turning many switches off, and then on again.** A screen share, a talk or a call wants the phones quiet on the computer for a while. Today that means toggling Settings and each phone's permissions, then remembering to turn them back.
- **Asked for:** KDE's forum, 2026-10-11, "pause KDE Connect temporarily": a call on a muted phone paused a video during a screen share.

## What changes

- **Pause for an hour, for 8 hours, or until resumed:**
  - from the tray's menu;
  - from the Overview ("Pause…" with the three, or "Resume" while paused);
  - from the CLI (`pocketdesk pause --for <minutes> | --until-resumed | --resume`, with `--json`).
- **While paused, the computer holds:**
  - phones' and KDE devices' notifications, calls among them: logged and kept to answer, not shown, as a muted app's are (K2b), but still counted against a phone's limit, so a week's pause does not unbound a phone;
  - texts: kept on the Messages page, not shown;
  - links from phones: listed on Activity, not opened;
  - a phone's ring for the computer (the siren): not sounded.
  - the other pop-ups a phone causes: a received file's notification (the file is still saved and on the Files page), a phone's low-battery warning (its battery is still on the Overview), and a KDE device's ping.
- **What it does not hold:**
  - sessions, which the owner chose to open;
  - files;
  - the clipboard;
  - anything the owner does from the computer.
  - the deck's presses (F1), which the owner makes from the phone: a deck link still opens;
  - hooks and Home Assistant, which see every event as before: they are the owner's own setup.
- **It shows that it is paused:**
  - the tray's menu leads with "Paused, 42 min left" (or "until resumed") and Resume;
  - the Overview says what is held and has Resume.
- **It ends by itself** at its time; "until resumed" never does, so it is never the default.
- **The state:** `pause.json` in the state folder, owner-only, changed under a lock (`core::local::pause`, tested). The server reads it at each held thing, so a pause applies to the next one.

## Left out

- "Until tomorrow": it needs the computer's time zone, and an 8-hour pause does the same at night.
- The phone saying the computer is paused: it needs a packet, and the phone's own Do Not Disturb (D2) already covers the phone's side.
- The clipboard: a pause is about interruptions, and a copy is the owner's own act.

## Testing

- **Unit:** the state (a pause for minutes ends at its time, "until resumed" never does, resume ends it, a broken file reads as not paused and is never written over); the tray's menu while paused; the CLI's arguments.
- **Server:** each held thing asks `paused_now`, and its test shows that a pause holds it.
- **Harness:** the Overview's Pause… and Resume.
- **By hand (checklist §152):** the tray's items; a phone's notification and a link while paused, and after.
