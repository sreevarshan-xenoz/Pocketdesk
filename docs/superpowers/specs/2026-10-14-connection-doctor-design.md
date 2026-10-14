# Connection Doctor on the phone (T1)

Status: design, 2026-10-14. Roadmap item T1 ("After Phase R"). The research is in `docs/research/2026-10-14-what-users-need.md` (why) and in this file's "Evidence" section (how).

## Problem

"The phone can't see the computer, and nothing says why" is the top complaint across KDE Connect, LocalSend and similar tools. Pocketdesk's desktop app has a Diagnostics tab, but the phone, where the failure is seen, only shows one humanized error. The causes users hit, roughly in order, and none of them visible from the phone today:
- AP isolation on guest Wi-Fi;
- a VPN that swallows home traffic;
- a Windows network set to Public;
- a stopped server;
- a computer asleep;
- a changed address;
- UDP blocked;
- a security key that changed after a reinstall;
- battery optimization;
- Android 17's local network permission.

## What it is

**Where it opens:**
- "Check connection" on each saved computer (Home, the card's menu);
- "Check connection" on the session's error card;
- automatically suggested after a connect fails twice.

**What it shows:**
- One screen that runs a fixed list of checks in a few seconds.
- Each check gets one row: an icon (good, warning, problem, not checked), one sentence of what was found, and, when something is wrong, one sentence of what to do, with a button when an app or settings page can do it.
- "Copy report" copies the findings with every address and id left out.

**When it runs:** only when the person taps, while the app is in front. No background probing, no subnet scans, no wake locks.

## The checks (version 1), in run order

1–4 are instant reads of the phone's own state. 5–7 are probes, run side by side, each a few seconds at most. The whole run takes about 5 s. A check that does not apply says "not checked" and why, as the desktop's Diagnostics does.

1. **This phone's network** (`ConnectivityManager`, `NetworkCapabilities`, `LinkProperties`).
   - Offline: "This phone is offline."
   - Captive portal: "This Wi-Fi wants you to sign in."
   - Not validated: a warning, "no internet; home connections can still work".
   - IPv6-only: "only the internet path can reach the computer".
2. **Local network access** (Android 17+: `ACCESS_LOCAL_NETWORK`). Denied: "Pocketdesk isn't allowed to reach devices on your network", with a button to ask again. It runs first, because a denial looks like a timeout.
3. **VPN** (`TRANSPORT_VPN` on the default network).
   - A tailnet address with no VPN: "Tailscale is off on this phone."
   - Otherwise "A VPN is on", with the verdict decided by check 5's Wi-Fi-bound retry: "Your VPN sends home traffic into the tunnel. Allow local network access in the VPN app, or pause it."
4. **Same network.** The saved address's class (private IPv4, tailnet, link-local, public, hostname) is compared with the phone's Wi-Fi or Ethernet prefixes.
   - On mobile data with a private address: "the home address won't work from here".
   - A different subnet on Wi-Fi: "This phone is on a different network than the computer (guest Wi-Fi?)."
5. **The computer answers** (TCP to the control port, 4 s, the error code kept).
   - Refused: "Nothing is listening on the computer. Is Pocketdesk running there?"
   - No route: "Nothing answers at this address. The computer may be asleep, off, or on another network." With Wake, when the computer has wake details.
   - Timeout: "The computer doesn't answer. A firewall may be blocking Pocketdesk, or this Wi-Fi keeps devices apart." The fix names the Diagnostics tab, a Private network on Windows, and AP isolation.
   - With a VPN on, the probe is repeated on a socket bound to Wi-Fi. The difference decides check 3.
6. **It's your computer** (TLS to the control port with a verifier that **compares and never saves**, through a new bridge call).
   - Match: "It's your computer: the security key matches."
   - Changed: "This computer's security key changed. If you reinstalled Pocketdesk there, pair again." The only fix offered is Pair again; never "trust anyway".
   - Not Pocketdesk: "Something else answers at this address. The computer's address may have changed."
   - Not paired: "Not paired from this phone yet."
7. **Video port** (UDP to the stream port: a 1200-byte QUIC packet with a reserved version, so the desktop answers with Version Negotiation without opening a connection or using its connection budget; three tries in 2 s).
   - Silent: "The video port is blocked (UDP)." The fix says to allow it in the firewall, or on Windows to run `pocketdesk install` again.
   - Refused: "The video service isn't running."
8. **Staying connected in the background.** A warning, never a problem, and only when phone notifications, texts or auto-reconnect are on.
   - Battery optimization: links to the battery settings page, without the permission Play reviews, plus a line linking `dontkillmyapp.com/<manufacturer>`.
   - "Restricted" battery use.
   - Data Saver.
9. **Phone features**, each only if that feature is switched on: notifications allowed, notification access, texts permission.

**Session rows**, on top when opened from a session's error card:
- the path it was on (LAN, Tailscale, internet direct or relayed);
- why it ended, from the desktop's close reason ("ended on the computer", "the computer stopped hearing this phone", ...).

## Later (version 2)

- **Internet path:** a dial over iroh with the cached identity, reporting relay reached or refused (`home_relay_status`, `auth_denied_reason`) and direct or relayed.
- **Found on this network:** an mDNS browse that notices the computer's address changed. The new address is accepted only if check 6 there matches the old pin, since an id in mDNS can be spoofed.
- **The desktop's own view:** over any working path, ask the desktop for its Diagnostics findings, which turns "a firewall may be blocking" into "the computer says its network is Public".

## Rules

- **Never trust by probing:** the identity check never saves a pin and never offers to accept a changed key.
- **Name things, never show them:** the copied report has no IPs (classes instead), names, SSIDs, MACs, device ids, certificate fingerprints, endpoint ids or relay URLs. A unit test holds the report to that, as the desktop's does.
- **No new permissions:**
  - no location for the SSID;
  - no `QUERY_ALL_PACKAGES` to name the VPN;
  - not the reviewed battery-exemption permission (the settings page is opened instead).
- **Customization (Settings):** "Suggest a check after a failed connect" (on by default).

## Design

**`ConnectionChecks` (pure Kotlin, JVM-tested):**
- Takes the probe results and the phone state, as plain data, and returns the findings in order: status, the sentence, the fix, and the fix action.
- Every wording above lives here, as string resources.
- `DoctorReport.redact(findings)` builds the copied text.

**`ConnectionProbe` (Kotlin, Android):**
- Reads the phone's state.
- Runs the TCP probe, keeping `ErrnoException.errno`: `ECONNREFUSED`, `EHOSTUNREACH`, `ENETUNREACH`, `EPERM`/`EACCES`, or a timeout.
- Retries the TCP probe on `Network.socketFactory` for Wi-Fi when a VPN is on.
- Sends the UDP Version Negotiation probe.

**Bridge `nativeCheckIdentity(address, port)`:**
- Opens TLS with ALPN `pocketdesk-control/1` and a verifier that records the presented certificate's fingerprint and accepts it for this one handshake only.
- Compares the fingerprint with the stored pin for that address without writing.
- Returns `match`, `mismatch`, `not_pinned`, `not_pocketdesk` (no TLS, or another ALPN) or `failed`.

**`ConnectionDoctorScreen` (Compose):**
- Opened from the card and the error card.
- Runs the checks, shows the rows, and offers the actions: Wake, Pair again, the settings pages, Copy report.

## Testing

- **JVM:**
  - the mapping for every outcome of every check;
  - the order;
  - "not checked" when a check does not apply;
  - the redaction test (no IPv4 or IPv6 literal, no MAC, no 16+ hex-digit run in the report).
- **Bridge:** `check_identity` against a local TLS server with a known certificate: match, mismatch, not pinned, and plain TCP with no TLS. It must never write the pin file.
- **Device (A71 → Windows):**
  - the server stopped (refused, or a timeout behind the firewall);
  - the computer's firewall dropping the stream port;
  - a wrong saved address;
  - a changed key (a test pin);
  - Wi-Fi off;
  - battery optimization on.

## Evidence (how each distinction holds up)

- **Refused:** proves the host is reachable and nothing listens. On Windows, Pocketdesk's program-scoped firewall rules and stealth mode probably turn a stopped server into a timeout instead (*unverified*), so the wording for a timeout includes "is Pocketdesk running?".
- **No route** after about 3 s on the same subnet: nothing answered ARP (asleep, off, moved, or isolation that filters ARP). A strong guess, not proof.
- **A timeout:** a firewall or AP isolation. It is a guess unless another path proves the computer is up (version 2's internet check, or a fresh mDNS announcement).
- **A Wi-Fi-bound probe that works while the default network fails:** the VPN is capturing home traffic. Reliable.
- **The pin comparison:** reliable that the key changed, but not why (a reinstall, another machine on that address, or interception). So the only fix offered is pairing again with the QR code.
- **The UDP probe:** quinn answers a 1200-byte unknown-version packet with Version Negotiation, statelessly. Silence after three tries, while TCP works, means UDP is blocked.
- **Android facts:**
  - The phone cannot read the ARP table (closed since Android 10), its own MAC, or the computer's firewall profile.
  - ICMP errors still surface as error codes on connected sockets.
  - Sources: developer.android.com (local network permission, ConnectivityManager, PowerManager), dontkillmyapp.com, and LocalSend's and KDE Connect's troubleshooting pages.
