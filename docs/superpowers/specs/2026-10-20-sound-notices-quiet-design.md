# A sound session notices a computer gone quiet (J6)

**Date:** 2026-10-20. **Branch:** `fix/sound-notices-quiet`. Follows I1b, which taught the remote the same.

## What is wrong

- **A dead computer looks alive.** A sound session (N5) whose computer was switched off, crashed or lost its network shows as playing until the transport's 45 s idle timeout says it is gone. Only then does it reconnect.
- **The remote no longer waits that long.** Since I1b it ends a link quiet for 12 s and reconnects. The bridge's `remote_state` already reports `quietMs` for every session, the sound session included, but the sound session never reads it.

## What changes

- **A ready sound session quiet for 12 s ends as a lost link,** "the computer stopped answering", and reconnects as any drop does (I1's `AutoReconnect`). Like the remote, it closes the dead session first, so the reconnect dials afresh.
- **Why 12 s is safe here:** a sound session is input-only to the client (`!mode.gets_picture()`), so the phone sends its link report every 5 s and the computer's transport acknowledges each. 12 s is over two missed.
- **What it cannot see:** that acknowledgement is the transport's own. A server process that hangs but whose connection still answers is not noticed, as for the remote.
- **A cost, as for the remote:** a Wi-Fi roam longer than 12 s now ends the session and reconnects, where the 45 s timeout used to ride it out.
- **Health needs the computer heard from:** after 10 s admitted, a session's reconnects start over only while the computer was heard from within 5 s (`RemoteSession.heard`). Before, a computer that took each handshake and then fell silent restarted the backoff every cycle and never let it give up. This applies to the remote too.
- **One rule for both screens:** `RemoteSession.goneQuiet(next, closed, json)` is true for a ready session, not closed by the transport, quiet for 12 s. The remote and the sound session both ask it. It is JVM-tested.

## Testing

- **Unit:**
  - `goneQuiet` for a quiet ready session, and for one not yet ready, closed by the transport, heard from, down or too old;
  - `heard` on either side of 5 s.
- **On a device (checklist §148):**
  - with the computer's sound playing, stop the throwaway server: the card in about 14 s, then back by itself when it returns;
  - with nothing playing, a 5-minute session never ends on its own.

## Device run (the OPPO CPH2359, Android 14, against the throwaway server)

- **Held:** a sound session over the LAN for a minute.
- **Noticed:** with the server stopped, the card read "The sound stopped: the computer stopped answering" 14 s later, then "Reconnecting in 1 s…".
- **Back:** the phone's screen went off while it retried, and the session came back by itself 13 s after the server returned.
- **Owed (§148):** the 5-minute and screen-off hour with nothing playing, and a relayed path.

## Review

A fresh review found 0 Critical, 0 Important and 4 Minor.

- **Fixed:**
  - health counted from silence, in both screens (`heard`);
  - the down and too-old cases untested;
  - the spec's ACK, roam and checklist words.
- **Left as the remote has it:** the sound loop's wiring is checked on the device, not by a test.
