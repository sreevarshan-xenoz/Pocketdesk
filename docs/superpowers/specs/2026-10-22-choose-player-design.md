# O6, choose the computer's player (design)

Date: 2026-10-22. Plan: `docs/research/2026-10-22-o-cycle.md` §O6.

**The problem.** The phone's controls for the computer's music follow the player the computer's system picks:
- Windows' current media session;
- whichever player `playerctl` picks.

With a browser tab paused and Spotify playing, Play may go to the tab. GSConnect and KDE Connect let the phone pick the player.

## What

**The server.** The `media` answer on the audio request (T8c) also lists the computer's players, `players: [{id, name, isPlaying}]`, at most 16:
- Windows: each media session's app, except Pocketdesk's own entry (N5). An app with two sessions is one player.
- Linux: each MPRIS player, by its instance (`playerctl -a metadata`).

Any audio request may name one (`player`, an id from that list). This covers:
- `media`, `mediaControl`;
- N5's `computer_media_watch`, which reads that player for that phone.

Windows uses the named session while it is there, else the system's own as before. Linux asks `playerctl -p <id>,%any`, which falls back to any player in the same way.

An id is at most 256 bytes, with no control character and no comma (`media::valid_player`); any other id is ignored. The deck's media buttons stay on the system's choice. Audio permission covers the requests, as before.

**The phone.**
- The Sound sheet's "Now playing" card has "Player: …" when the computer lists two players or more, or one was chosen. Its menu offers "The computer's choice" and each player, marked when it plays.
- The choice is kept per computer (`HostStore.player`) and forgotten with the computer.
- The Sound sheet, the media widget and tile, and N5's watch and presses name it. A new choice asks N5's watch again, as a new link would.
- A chosen player that has gone reads as "The computer's choice", which the computer then makes. It is kept should the player come back.
- The rule is `ComputerPlayers`, JVM-tested.

**The seam.** `POCKETDESK_MEDIA_FILE`'s JSON may hold `players`, a list of the same objects, each known by its `app`. A press naming a player is written to the presses file followed by that player.

## Not done

- **Choosing on the computer.** The desktop app has no choice of its own: the computer's own media keys stay with the system.
- **Telling two windows of one browser apart.** Each is listed by the name the system gives it.

## Checks

- **Unit tests:**
  - the server: the player list's parse, the id rule, the answer, and Windows' choice;
  - the phone: `ComputerPlayers`.
- **On the OPPO, with the seam:** two players listed, one chosen, and a press reaching the chosen one (checklist §189).
- **Owed:** a press to a real player, which needs the owner.

## Review

A fresh review found 0 Critical, 3 Important and 5 Minor issues. It found no way to inject into `playerctl`: the arguments are passed as a list with no shell, and an id is one `-p` value with no comma.

**Important, fixed:**
- **The widget and tile kept the old player's track** while their presses went to the new one. A new choice now forgets what they held and asks again (`MediaWidget.playerChanged`).
- **Choosing a browser on Windows took its first session**, a paused tab, not the playing one. The chosen app's playing session now comes first.
- **Every `media` ask listed the players**, a second read each 2 s that could hold the link up to 6 s. Only the Sound sheet asks for the list now (`players: true`).

**Minor, fixed:**
- A push read for the player chosen before was shown after a new choice. Pushes now say their player, and the phone shows only the chosen one's (`ComputerPlayers.pushFor`).
- A stop after a new choice could go unsent. The watch keeps its link rule, and a new choice asks again on its own.
- The phone counted an id's length in characters where the computer counts bytes.
- Two players of one name (two Firefox windows) read the same in the menu. Each now has its id beside it.
- The menu's items said no selected state to TalkBack.
- The seam listed its own player twice.
- An older read could overwrite a choice's.

**Tests added:** the seam's players, Windows' choice between two sessions of one app, the label and the push rule.

**Not done:** a test of the watcher's grouping by player, which needs live clients.
