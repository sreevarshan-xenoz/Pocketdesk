# O3, a paired computer found again at a new address (design)

Date: 2026-10-22. Plan: `docs/research/2026-10-22-o-cycle.md` §O3. Asked for in KDE bug 524452 and KDE Discuss 44308 ("No connect with android hotspot or USB-Thether").

## The problem

A saved computer is kept by its address, and its pin with it. When its address changes, the phone shows it on Home under "On this network" as a stranger at its new address, beside its old card, which no longer connects. This happens with a new DHCP lease, the phone's own hotspot, or USB tethering. F6 matches found computers to cards by address only, on purpose: an mDNS announcement is unauthenticated, and following an announced id would let anyone on the network take a card (and its pairing PIN).

## What

A found computer is a candidate to move when its announcement names the paired id of exactly one saved card (`FoundComputers.moved`, JVM-tested), and:
- that card is saved by an IPv4 address (one saved by name keeps its name);
- the computer is found at an IPv4 address no card holds.

Each candidate is tried once while Home is up:
- **The probe.** The phone opens the control port at the new address and runs the connection doctor's compare-only identity check against the pin held for the old address (`CertManager::probe_identity_as`). It needs the pinned certificate and proof of its key in that handshake.
- **On a match only:**
  - the pin is copied to the new address (`copy_pin`);
  - the card takes the new address and ports in its place in the list, with every per-address record (`HostStore.move`: wake, the internet identity, the screen chosen, the paired id, names);
  - the old address's pin is forgotten;
  - a snackbar says "Found Desk at its new address, 192.0.2.21, and moved it there."

## Not done

- **A sweep of the phone's hotspot subnet** for a saved computer's control port. That is a scan, which the plan's doctor never does. Discovery's announcements cover a computer that announces itself.
- **A computer with LAN discovery off** is not found, so it is not moved.

## Checks

- The rule's cases, in a JVM test: moved, still at its address, saved by name, two cards for one id, no id or an unknown one.
- The pin copy, in a core test.
- On a device: owed (checklist §186). It needs this computer at a second address the phone reaches, such as the phone's hotspot, which changes this computer's network: with the owner.
