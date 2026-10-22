# P1, the computer's announcement follows its network (design)

Date: 2026-10-22. Plan: `docs/research/2026-10-22-p-cycle.md` §P1.

**The problem.** The server announced itself on the network once, at start, with the addresses it had then (F6). If the computer then joined another network, woke from sleep with a new lease, or brought up a VPN, phones saw its old address, or none. That lasted until the server restarted. KDE Connect users report the same (KDE bugs 524179, 526688, 514470, 521261).

## What

**How it watches.** The server reads its addresses every 30 s (`follow_addresses`).

**What counts as a change.** The addresses an announcement holds are compared, order aside, as `lan::announced_addresses` has them: the tailnet's left out where others exist, each once, sorted.

**What happens on a change** (`Announcement::renew`):
- the old record is withdrawn with an mDNS goodbye, so phones drop the old address at once;
- the computer is announced again at the new addresses;
- the log says so.

**What does not count:**
- A reading with no addresses is no change. An adapter between networks, or a failed read, must not withdraw the computer.
- A wake from sleep needs nothing of its own: the next reading after it sees the new lease.

**On shutdown,** whichever record is current is withdrawn, as before.

**The seam.** `POCKETDESK_LOCAL_IPS_FILE` (absolute) makes the addresses that file's lines, for tests. It is announced in the log when set.

## Not done

- **Listening for the system's network events** (Windows' `NotifyIpInterfaceChange`, netlink). Thirty seconds of polling costs one adapter list each time, and catches every case.
- **The phone's own side.** O3 already finds a paired computer at a new address by its key.

## Checks

- **Unit tests:** the comparison, covering order, a dropped duplicate, a move, no addresses, and a tailnet address beside the others.
- **On the OPPO:** a throwaway server announced at a documentation address, then the seam moved to this computer's address. The phone found it under "On this network" (checklist §193).
