# A remote notices a computer gone quiet (I1b)

**Date:** 2026-10-20. **Branch:** `fix/remote-notices-quiet`. **Found on the OPPO** while checking I1 (the remote reconnecting by itself): with the throwaway server killed under a remote, the remote went on looking ready for 47 s before its connection closed ("timed out") and the reconnect began. It then came back 2.6 s later, the talk timer still counting.

## Why 47 s

A screen session notices a dead computer in 10 s: frames stop, and its stall watch ends it. A remote receives no frames, so only the transport's idle timeout can end it, and that is 45 s (with a 15 s keepalive), chosen so a session survives a phone moving from Wi-Fi to mobile data.

## What changes

- **The bridge says how long the link has been quiet.** While the computer is alive it acknowledges what the remote sends: the link report every 5 s, the keepalive every 15 s. So the count of datagrams the phone has received rises at least every 5 s. The bridge notes when that count last rose (`Quiet`, tested), and `remote_state` adds `quietMs`.
- **The remote takes 12 s of quiet as a lost link.** A ready remote whose link has been quiet that long ends as a silent link does (`RemoteSession.silent`, JVM-tested): "The computer stopped answering." on its card, and the reconnect (I1) starts. 12 s is over two missed link reports.
- **Nothing changes on the computer or in the transport.** The idle timeout stays 45 s for screen sessions and roaming.

## Left out

- The sound session reads the same `quietMs` but keeps its own rule for now. It receives audio while the computer plays, and its drop is a separate check.

## Testing

- **Unit:** `Quiet` (a rising count resets it, a still one ages, a new connection's lower count starts over) and `RemoteSession.silent` at its edge.
- **On the OPPO (checklist §73):** the throwaway server killed under a remote. The remote's card in about 12 s, back by itself once the server is up, the timer still counting.

## As built

- A remote that finds its link quiet closes the session itself before reconnecting. The bridge held the dead connection until the transport timed it out, so the first build's reconnect took it for a live session ("Session already active … skipping connect") and the remote never came back. Found on the OPPO.
- Checked there after that fix: the throwaway server killed under a remote with a talk timer. The remote noticed at 14 s and was ready again 14 s after the server returned. The timer counted through: 9:48 at the kill, 9:14 when it was back.

## Review

A fresh review found 0 Critical, 0 Important and 2 Minor issues.
- **Fixed:** the tracker kept the last session's count and time. A new session whose count matched could be taken as quiet at once. It is now reset as a session connects and as it stops.
- **Left as is:** the 12 s margin on a lossy or relayed path. Any datagram received (acks, retransmits) resets it, so a lost report does not trip it, and a 12 s outage is meant to.
