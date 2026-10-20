# The remote reconnects by itself (I1)

**Date:** 2026-10-20. **Branch:** `feat/remote-reconnect`. **Asked for by the owner** after the morning's device checks: a remote whose link dropped (the computer's server restarted, Wi-Fi went) waited on its card for Retry, unlike a screen session (R2) and a sound session (N5), which come back by themselves. A presenter with the talk timer running had to stop and tap.

## What changes

- **The remote reconnects as a sound session does.** A link that went away is tried again with the same backoff (2 s doubling to 30 s) for ten minutes, under the same Settings switch, "Reconnect by itself". An end the computer chose (a kick, a refusal, the Input permission off, an unpairing) and anything about trust or pairing wait for the person, as now. A remote the owner never admitted is not asked again on its own.
- **The card says so.** While it waits, "Reconnecting in N s…" under the reason; after ten minutes, "Stopped trying after 10 minutes."; Retry and Exit stay. These are the sound session's words, already in English, Spanish and Tamil.
- **What the remote holds survives.** The talk timer, the air mouse and laser choices live outside each attempt, so a reconnect keeps them (H2's review made the timer outlive a reconnect).
- **The phone sleeps once it gives up.** The session's foreground service stops when the remote stops trying, and starts again on Retry, as the sound session's does (N5 review: a drop had held a wake lock for hours).
- **One rule for why a session ended.** `AutoReconnect.endedBy(closed, wasWaiting)` names it for both the sound session and the remote, from the computer's close reason or a silent link; null when the owner never answered. JVM-tested.
- **A server restarting mid-handshake is retried.** Since #180 a handshake the far side closed (tokio-rustls's "tls handshake eof") reads as nothing answering; it is now also one of the transient failures a reconnect tries again, as "connection refused" is.

## Left out

- A screen session already reconnects (R2); nothing there changes.
- The remote does not reconnect in the background after its screen is gone: like a sound session, it lives with its screen.

## Testing

- `AutoReconnectTest`: `endedBy` for a close, a silent link and a remote never admitted; "tls handshake eof" retryable; a refused certificate still not.
- On a device (checklist §73): a remote, the throwaway server restarted, the card counting down and the remote back by itself with the timer still running; Kick from the computer not retried; Settings "Reconnect by itself" off, Retry only.
