# A sound session notices a computer gone quiet (J6)

**Date:** 2026-10-20. **Branch:** `fix/sound-notices-quiet`. Follows I1b, which taught the remote the same.

## What is wrong

- **A dead computer looks alive.** A sound session (N5) whose computer was switched off, crashed or lost its network shows as playing until the transport's 45 s idle timeout says it is gone. Only then does it reconnect.
- **The remote no longer waits that long.** Since I1b it ends a link quiet for 12 s and reconnects. The bridge's `remote_state` already reports `quietMs` for every session, the sound session included, but the sound session never reads it.

## What changes

- **A ready sound session quiet for 12 s ends as a lost link,** "the computer stopped answering", and reconnects as any drop does (I1's `AutoReconnect`). Like the remote, it closes the dead session first, so the reconnect dials afresh.
- **Why 12 s is safe here:** a sound session is input-only to the client (`!mode.gets_picture()`), so the phone sends its link report every 5 s and the computer acknowledges each. 12 s is over two missed.
- **One rule for both screens:** `RemoteSession.goneQuiet(next, closed, json)` is true for a ready session, not closed by the transport, quiet for 12 s. The remote and the sound session both ask it. It is JVM-tested.

## Testing

- **Unit:** `goneQuiet` for a quiet ready session, one not yet ready, one the transport closed, and one heard from.
- **On a device (checklist §148):**
  - with the computer's sound playing, stop the throwaway server: the card in about 14 s, then back by itself when it returns;
  - with nothing playing, a 5-minute session never ends on its own.
