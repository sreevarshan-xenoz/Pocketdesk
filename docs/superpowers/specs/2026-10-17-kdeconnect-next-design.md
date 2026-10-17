# KDE Connect, next steps (N9)

Date: 2026-10-17. Plan: `docs/research/2026-10-17-finish-and-polish.md` §N9. Builds on the bridge of phase 3a–3c (`docs/superpowers/specs/2026-10-17-kdeconnect-bridge-design.md`).

## What this pass does

1. **Notifications from KDE devices**, shown here as a phone's:
   - under the same "Phone notifications" switch and flood limit, with an event;
   - closed here when they close there (`isCancel`);
   - a new per-device switch, "Its notifications", on by default (the device sends them only with notification sync on in its own KDE Connect app);
   - the identity now offers `kdeconnect.notification`.

   KDE Connect's body differs from Pocketdesk's: the app is `appName`, a `ticker` may stand in for the text, and the notifications already on the device when it connects come marked `silent`. Those are not shown, or every connect would pop up a toast for each.
2. **A re-check against CVE-2025-32898 and CVE-2025-32900** (KDE's advisory 2025-04-18, fixed in KDE Connect 1.33 for Android and 25.04 for the desktop):
   - **CVE-2025-32898** is a pairing code short enough to brute-force. Ours is KDE's newer one, with the timestamp in the hash. A request waits 30 s for the owner, who must press Allow naming that code, at most one request every 10 s per link. Nothing here takes a code typed or guessed by the device.
   - **CVE-2025-32900** is a spoofed UDP broadcast changing a device's shown information. This computer only sends broadcasts and never reads one. A device is known by the identity it gives inside TLS, checked against the plaintext one (CVE-2025-66270) and against its pinned certificate once paired, so no broadcast can change what is shown.
3. **Beside KDE's own daemon on Linux:** `kdeconnectd` listens on TCP and UDP 1716, the same port as Pocketdesk's control channel, so the two cannot both run with their defaults. Use one or the other, or give Pocketdesk another `control_port`. Then KDE devices find this computer only through Find, which tells them the port in its identity. Written in the README, not changed in code.

## Not in this pass

Sending files to a KDE device. It needs a TLS listener on 1739–1764 for the device to fetch from, and on Windows a firewall rule only `install` can add with administrator rights. It is the next item.

## Tests

- The KDE notification's reading (`appName`, ticker, silent, a cancel).
- That a paired device's notifications are taken with its switch on, not off, and not while unpaired, and that a flood is bounded.
- The capabilities list.

## Check

On the A71 with KDE Connect (paired to a throwaway server), notification sync on: a notification posted on the phone shows here as a toast and goes when it is closed there; with the switch off, nothing.
