# P1, the computer's announcement follows its network (design)

Date: 2026-10-22. Plan: `docs/research/2026-10-22-p-cycle.md` §P1.

**The problem.** The server announced itself on the network once, at start, with the addresses it had then (F6). If the computer then joined another network, woke from sleep with a new lease, or brought up a VPN, phones saw its old address, or none. That lasted until the server restarted. KDE Connect users report the same (KDE bugs 524179, 526688, 514470, 521261).

## What

**How it watches.** The server reads its addresses every 30 s (`follow_addresses`). A new reading is announced once two readings in a row agree (`lan::Settle`).

**What counts as a change.** The addresses an announcement holds are compared, order aside, as `lan::announced_addresses` has them: the tailnet's left out where others exist, each once, sorted. The IPv4 addresses decide where there are any, so IPv6's rotating temporary addresses do not count.

**What happens on a change** (`Announcement::announce_at`):
- the old record is withdrawn with an mDNS goodbye, so phones drop the old address at once;
- after a 300 ms wait, the computer is announced again at the new addresses;
- the log says so.

**What does not count:**
- A reading with no addresses is no change. An adapter between networks, or a failed read, must not withdraw the computer.
- A reading left with only the tailnet's addresses, while the network's were announced, is no change either: mDNS goes nowhere then.
- A wake from sleep needs nothing of its own: the next reading after it sees the new lease.

**On shutdown,** whichever record is current is withdrawn, as before.

**The seam.** `POCKETDESK_LOCAL_IPS_FILE` (absolute) makes the addresses that file's lines, for tests. It is announced in the log when set.

## Not done

- **Listening for the system's network events** (Windows' `NotifyIpInterfaceChange`, netlink). Thirty seconds of polling costs one adapter list each time, and catches every case.
- **The phone's own side.** O3 already finds a paired computer at a new address by its key.

## Checks

- **Unit tests:** the comparison, covering order, a dropped duplicate, a move, no addresses, and a tailnet address beside the others.
- **On the OPPO:** a throwaway server announced at a documentation address, then the seam moved to this computer's address. The phone found it under "On this network" (checklist §193).

## Review

A fresh review, made from reading mdns-sd's source, found 0 Critical, 3 Important and several Minor issues.

**Important, fixed:**
- **A new address could be announced into nothing.** mdns-sd opens sockets on a new interface only at its own check, every 5 s. A record registered before then went nowhere, and was never sent again. A new reading now waits for a second, 30 s later (`Settle`).
- **The goodbye's resend removed the new record.** mdns-sd sends a goodbye again 120 ms later, and that removed the record just made. Dropping the goodbye instead was tried on the OPPO: Android kept the old address until the service was lost. So the goodbye stays, and the new record waits 300 ms.
- **Churn flapped phones.** The flapping came from Docker bridges, VPN tunnels and IPv6's temporary addresses. The second reading helps; IPv6 now counts only without IPv4.

**Minor, fixed:**
- shutdown takes the announcement off the main loop;
- a reading with only the tailnet left is no change;
- the addresses are logged at debug, not info;
- a seam path that is not absolute is said.

**Not done:** a test of announcing at a new address on the real network. Announcing a made-up address on the owner's network would mislead its phones; the OPPO check covers it.
