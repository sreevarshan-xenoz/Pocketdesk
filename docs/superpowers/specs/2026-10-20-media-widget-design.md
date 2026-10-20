# G5: the computer's music on the home screen (2026-10-20)

T8c put what the computer is playing in the Sound sheet, with Previous, Play/Pause and Next. That sheet is three taps deep: the computer's menu, a session or Sound, then the card. Pausing the computer's music from across the room is the commonest thing asked of it, so this puts the card on the home screen.

## What it shows

**The widget, "Pocketdesk: playing",** is for the last computer, the one the other widgets use. It shows:
- **what plays:** the title, or the app when there is no title, and under it the artist and the app (`NowPlaying.headline` / `subline`, as the sheet shows them);
- **its buttons:** ⏮, ⏯ and ⏭, each only when the player offers it, and ↻ to ask again;
- **a line** for when it was read ("Read at 14:05", the date before today), "Asking the computer…", the computer's refusal (Audio off for this phone, said as the app says errors) or "No answer from the computer yet". After a failure only why is shown, never the old track.

## How it gets it

**When it asks:**
- **A tap on a button** sends `{action: "mediaControl", command}` (Audio permission, as from the sheet), waits 300 ms for the player, then asks what plays (unless the command took more than 4 s; then ↻ reads it).
- **↻, placing the widget, or a new last computer** asks `{action: "media"}`.
- **Never by itself:** music changes by the minute, and a widget period of 30 minutes would show the wrong song more often than the right one. The line says when it was read.

**How each ask runs:** as the deck widget's (G3), through the helper both now share (`WidgetAsk`):
- the bridge is started first;
- under `goAsync`, bounded at 8 s;
- "Asking…" kept in memory only;
- only the newest ask writes;
- a press carries the computer it was drawn for.

The rules are in `MediaWidgetPlan` (JVM-tested):
- which buttons show;
- the time line;
- whose answer is kept.

## Left out

- Album art.
- A volume slider.
- A widget per computer.

## Review

A fresh review found 0 Critical, 0 Important and 6 Minor issues. Five are fixed, in two commits:
- the asking flag was set apart from the newest-ask number, so an older ask's end could clear a newer one's "Asking…";
- a worker that threw left "Asking…" up;
- a slow command still read what plays, past its wait;
- refusals were shown raw, with two English fallbacks;
- the old track stayed beside a failure;
- the glyphs could draw as colour emoji.

Left: a double tap sends two commands, as a media key does.

Owed (device, with the owner): checklist §136.
