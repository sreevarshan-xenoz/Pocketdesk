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
   - **CVE-2025-32898** is a pairing code short enough to brute-force. Ours is KDE's fixed one, with the timestamp in the hash, so it carries the same residue as patched KDE: the requester picks the timestamp within 30 minutes, and the code is 32 bits. On top of that:
     - only protocol 8 is spoken, so there is no downgrade to the old fixed codes;
     - a request waits 30 s for the owner, who must press Allow naming that code;
     - one request every 10 s per link;
     - nothing here takes a code typed or guessed by the device.
   - **CVE-2025-32900** is a spoofed UDP broadcast changing a device's shown information. This computer only sends broadcasts and never reads one. A device is known by the identity it gives inside TLS, checked against the plaintext one (CVE-2025-66270) and against its pinned certificate once paired, so no broadcast can change what is shown.
3. **Beside KDE's own daemon on Linux:** `kdeconnectd` takes TCP 1716 (and up to 1764 when that is taken) and UDP 1716 for discovery. Pocketdesk takes TCP 1716 only, and never listens on UDP. So whichever starts second loses TCP 1716: `kdeconnectd` moves aside, but Pocketdesk's server does not start. Use one of them. Written in the README, not changed in code.

## Not in this pass

Sending files to a KDE device. It needs a TLS listener on 1739–1764 for the device to fetch from, and on Windows a firewall rule only `install` can add with administrator rights. It is the next item.

## Tests

- The KDE notification's reading (`appName`, ticker, silent, a cancel).
- That a paired device's notifications are taken with its switch on, not off, and not while unpaired, and that a flood is bounded.
- The capabilities list.

## Check

On the A71 with KDE Connect (paired to a throwaway server), notification sync on: a notification posted on the phone shows here as a toast and goes when it is closed there; with the switch off, nothing.

## Review

**Fresh review (0 Critical, 1 Important, 6 Minor), fixed:**
- SECURITY.md's KDE limit said nothing else a device sends is taken, and nothing said notification text is shown in full (KDE Connect does not keep private ones back). The limit now says so, and the switch's words do too.
- Minors:
  - Showing has a deadline, as the clipboard has.
  - A cap that the loop's draining made moot is gone.
  - A KDE device's tags and its share of the flood limit are its own (its self-chosen id could look like a phone's).
  - The words of the two switches.
  - The `kdeconnectd` conflict as it is: TCP only, and Pocketdesk's server is the one that does not start.
  - CVE-2025-32898: the same residue as patched KDE, not "not affected".
  - Notifications posted while a device was away come back marked silent and are not shown.

**Device-checked on the A71** with KDE Connect 1.36 paired with a throwaway server, notification sync on:
- a notification posted on the phone showed here as a toast, and closing it there took the toast away;
- the ones already there at connect, marked silent, did not show;
- with the switch off, none did.

Found there: KDE Connect loads its notification plugin only once it has notification access, which a restart picks up; and a restarted KDE Connect finds this computer again only while it broadcasts (Find), since this computer never listens on UDP (3a).

