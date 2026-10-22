# O9, Diagnostics' firewall row on Linux (design)

Date: 2026-10-22. Plan: `docs/research/2026-10-22-o-cycle.md` §O9.

**The problem.** On Windows, the desktop app's Diagnostics page says whether the firewall lets phones in (F10). Linux had no such row, and a ufw or firewalld that keeps the ports shut is the most common reason a phone cannot reach a Linux computer.

## What

**What it reads.** On Linux the Diagnostics page reads the firewall within 3 s per command:
- **ufw:** `ufw status verbose`. Without root that fails; `/etc/ufw/ufw.conf` then still says whether it is on.
- **firewalld:** `firewall-cmd --state` and `--list-all`, for the zone in use.

**What counts as letting phones in.** The control port (TCP) and the stream port (UDP), as configured, are each checked against (`parse_ufw`, `parse_firewalld`, tested):
- an ALLOW rule for that port, a range or a list holding it;
- an ALLOW for every port from a network;
- an allowing default;
- a zone whose target is ACCEPT;
- KDE Connect's service, which opens 1714-1764.

**What the row says** (`check_linux_firewall`):
- **Good:** no firewall found (never a failure); one installed but off; one that lets phones in.
- **Warn:** one that is on but whose rules need root to read. The hint is the command to read them.
- **Bad:** one that keeps a port shut. The hint is the command that opens it: `sudo ufw allow 4716/udp`, or `firewall-cmd --permanent --add-port=… && --reload`.

## Not done

- **nftables or iptables rules written by hand.** Only ufw and firewalld are read.
- **A rule limited to some addresses.** It counts as letting phones in.

## Checks

- **Unit tests:**
  - ufw: rules, ranges, v6 rules, the default, off, needs root;
  - firewalld: ports, ranges, KDE Connect's service, off, unreadable;
  - the finding: each status and its hint.
- **Owed:** the row on the VM with ufw on, off and with the ports allowed (checklist §192). The Linux code is built by CI only; this computer cannot build it.

## Review

A fresh review found 0 Critical, 3 Important and 5 Minor issues.

**Important, fixed:**
- **A helper that might not have compiled on Linux.** A closure returning a borrowed name is now a plain function.
- **Rules that read as open but were not.** An `OUT` or `FWD` rule counted as letting phones in. Only rules for what comes in count now.
- **Rules that read as shut but were not.** A rule on an interface (`on wlan0`), to one address, or a `LIMIT` rule was skipped. Each rule line is now read in full: what it is to, its action and its direction.

**Minor, fixed:**
- firewalld is read for every active zone, not the default one;
- its rich rules that accept a port are read;
- a ufw that is off no longer hides a firewalld that runs;
- "could not be read (they may need root)", where it had said they need root (a ufw in another language also lands there).

**Not done:**
- **The order of ufw's rules.** A DENY above an ALLOW still reads as open.
- **ufw's rules without root.** These cannot be read, so that row is mostly a warning with the command to run.
