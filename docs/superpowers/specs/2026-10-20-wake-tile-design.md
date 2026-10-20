# G7: wake the computer from Quick Settings (2026-10-20)

N6 wakes a computer from its card on Home or a launcher shortcut, both inside the app. Waking is what one does before sitting down at the computer, often from the shade, so this adds a tile.

## What it does

**The tile, "Wake computer,"** is for the last computer. A tap wakes it as its Wake settings say (`Waker.wake`: from this phone on its network, from outside through a set-up port, or through another computer).

**Its subtitle says:**
- the computer's name, when a wake can go;
- "Connect once while it is awake" with no MAC known yet;
- "Wake is off for it";
- after a tap, "Sending the wake…", then "Sent, waiting for it to answer…", then "It is awake" or "Sent, but it has not answered", or why the wake did not go.

An outcome is said for a minute, then the tile goes back to the name. The plan is read on a worker: it reads the network list.

## How a tap runs

- **The send:** a broadcast to `WakeTileReceiver` (not exported) runs it under `goAsync`, bounded at 8 s, so closing the shade does not cut it off. The bridge is started first, since a tap may start the process.
- **The check:** after a send that went, the computer's control port is watched for a minute, every 2 s, as the Home card does (`Waker.answers`). This runs past the broadcast as best effort.
- **Repeats:** a tap while one is under way is ignored.
- **A locked phone** is unlocked first. With no computer saved, a tap opens the app.

The words are chosen in `WakeTilePlan` (JVM-tested).

## Review

A fresh review found 0 Critical, 2 Important and 5 Minor issues:
- **Important:** "Wake sent" meant only that packets went, so a wrong MAC read as success. A tap now says whether the computer answered.
- **Important:** the wake ran on a bare thread that the process could lose once the shade closed. It now runs under a broadcast's `goAsync`.
- **Minor:**
  - the plan read on the main thread;
  - repeat taps;
  - an outcome never taken down while the shade stayed open;
  - a long error in the subtitle;
  - an unavailable tile with no computer saved.

All were fixed in one commit, because the tile was rewritten around the receiver.

Owed (device, with the owner): checklist §138.
