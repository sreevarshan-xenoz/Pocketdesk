# G6: the computer's music from Quick Settings (2026-10-20)

G5 put the computer's music on the home screen. The quickest place on a phone is the notification shade, and the commonest wish is to pause the computer's music from across the room, so this adds a tile.

## What it does

**The tile, "Computer's music",** is for the last computer:
- **a tap** plays or pauses its music;
- **it is drawn on** while the computer plays;
- **its subtitle** is the track, "Tap to play or pause", "Asking the computer…" or why not.

**One road with the widget:**
- a tap sends the media widget's own Play/Pause press (`MediaWidget.playPause`), so the ask, its bounds, the computer check and the kept answer are G5's;
- the tile reads that kept answer (`MediaWidget.latest`);
- it asks the computer only when tapped, never when the shade opens.

**While the shade is open,** the tile is drawn again as each answer comes:
- the widget draws the listening tile on the main thread (`MediaTileService.redrawOpen`, through a weak reference);
- `requestListeningState` would draw again only a tile declared active, and an active tile is not drawn when the shade opens.

**Its look,** in `MediaTilePlan` (JVM-tested):
- a reading older than 10 minutes is not shown as what plays now;
- before Android 10, which has no subtitle, the label says it.

**A locked phone** is unlocked first. With no computer saved, a tap opens the app.

## Review

A fresh review found 0 Critical, 1 Important and 3 Minor issues:
- **Important, fixed:** an open shade never showed the answer, because `requestListeningState` does nothing for a tile that is not active.
- **Minors, fixed in one commit:** an old reading shown as now, no track before Android 10, and the no-computer tap opened without unlocking.

Owed (device, with the owner): checklist §137.
