# Pocketdesk feature roadmap — making the phone and desktop apps good

Status: decided 2026-10-12, from [the competitor research](../../research/2026-10-12-competitor-features.md) and an audit of the tree at `main` 8c2341f.

## Where the apps stand

The transport, video, audio and pairing are solid and device-verified. The apps on top of them are thin.

**Phone:**
- Missing: any way to type in a session (the bridge has `sendKeyboardEvent` and `sendText`, but nothing captures the phone keyboard), and pinch-zoom outside direct-touch mode.
- Partial: the trackpad has only one-finger move, tap and two-finger scroll.
- Missing: discovery, file transfer UI and image clipboard.

**Desktop app:** status, Pair, Devices, Sessions, Settings and Logs, all read-only views over files. It cannot approve connections, set permissions, transfer files or help diagnose a failure.

## Order of work

Each item is its own branch (`feat/<topic>`), spec, plan and device check on the Galaxy A71, and merges to `main` once CI is green.

| # | Feature | App | Why first | Size |
|---|---|---|---|---|
| F1 | **In-session keyboard.** The stream view is the IME target: committed text goes as Unicode text, Backspace/Enter/arrows as keys, composing text waits. A keyboard button is always in the chrome. A docked accessory row has latching Ctrl/Alt/Shift/Super (tap = once, double-tap = lock) and a scrollable row with Esc, Tab, arrows, Home/End, PgUp/PgDn, Del, F1–F12 and Ctrl+Alt+Del. Zoom is kept when the keyboard opens. | Phone | A remote desktop you cannot type into is the single biggest gap; every competitor has this. | L |
| F2 | **Send text:** a compose box (dictation and long text, with history) and "type the clipboard as keystrokes" for fields that block paste. | Phone | Cheap once F1 exists; loved in Moonlight, scrcpy and KDE Connect. | S |
| F3 | **Trackpad that feels like a laptop's:** two-finger tap = right click, three-finger tap = middle click, two-finger horizontal scroll, double-tap-and-hold drag, sensitivity setting, pinch-zoom in trackpad mode (still open: F3 shipped without it, and zoom remains direct-touch only). | Phone | Precision is the second most common complaint. | M |
| F4 | **Linux text path:** Unicode through `zwp_virtual_keyboard_v1` with an uploaded keymap (the `wtype` technique), and Shift for capitals on uinput (roadmap 2666). | Server | F1 types Unicode correctly on Windows (UTF-16 SendInput) but not on Linux. | M |
| F5 | **Approve connections and set per-device permissions:** an approve/deny prompt in the desktop app for a new session (once / always / deny), and per-device toggles (view-only, input, clipboard, files, audio, mic, power) enforced by the server. | Desktop + server | Every host has this; it is the security baseline a remote desktop owes its owner. | M |
| F6 | **LAN discovery** on the phone home screen (found desktops listed above saved ones, one tap to pair), plus a "visible on the network" switch in the desktop app. | Both | Removes the setup friction users complain about most after input. | M |
| F7 | **File transfer:** share files into Pocketdesk from any app, a remote file browser with download, and a drop zone plus transfer history in the desktop app. | Both | The bridge has file-list and send-file; there is no UI. | L |
| F8 | **Image clipboard** both ways. | Both | Expected next to text clipboard. | M |
| F9 | **Session polish:** in-session monitor quick-switch (shipped 2026-10-13: a chip per screen in the session chrome), optional three-finger swipes for Alt+Tab and workspaces, hardware keyboard and mouse passthrough with pointer capture. | Phone | Power users, on top of F1/F3. | M |
| F10 | **Desktop app diagnostics:** port and firewall check, advertised interfaces, encoder in use, last session's latency tails, "copy debug report" (shipped 2026-10-13). | Desktop | Turns "it doesn't connect" into a fixable message. | M |
| F11 | **Shortcuts outside the app:** home-screen widget, Quick Settings tile, launcher shortcuts for connect and wake (shipped 2026-10-13). | Phone | Small, makes the app feel native. | S |

**Next, agreed 2026-10-13 (in this order; each gets its own spec, review, device test and PR):**

| # | Feature | Side |
|---|---|---|
| N1 | **Cross-network without Tailscale, step 1.** A real cellular test of the built-in iroh path (owed: the test phone has no mobile service). Router port mapping (UPnP / NAT-PMP / PCP) is iroh's default and already on. The phone's HUD already shows direct or relayed. Diagnostics now says whether this computer's NAT allows direct connections, and whether IPv6 is public (shipped 2026-10-13). | Both |
| N2 | **Your own relay.** `pocketdesk relay` runs iroh's relay server: on the desktop itself with one port forwarded, on any always-on box, or on a free cloud VM. It admits only listed endpoints (`--allow-paired` admits this desktop and its phones). The desktop's `relay_urls` setting points at it, and the phone learns it from the desktop, so it has no setting of its own (shipped 2026-10-13). | Both |
| N3 | **Finding the desktop without anyone's server.** The desktop publishes a record signed with its iroh key on the Mainline DHT (pkarr): its relay, and with `dht_publish = "addresses"` its public IPs. A phone whose cached address went stale looks it up by id. IPv6 direct paths were already bound on both ends. Diagnostics' "Found by id" says whether the record reads back (shipped 2026-10-14). | Both |
| N4 | **Image clipboard** (F8): images both ways as PNG through the TLS transfer path, under the Clipboard permission (shipped 2026-10-14). | Both |
| N5 | **Notifications both ways, with actions**, and the phone's notifications and SMS on the desktop. Split in three: N5a, the phone's notifications on the desktop (shipped 2026-10-14: opt-in, per-app, lock-screen privacy kept, a desktop permission and a flood guard); N5b, SMS; N5c, actions and dismissal both ways (part 1 shipped 2026-10-14: a notification closed on the phone closes on the computer). N5b SMS shipped 2026-10-14, built on the user's word; its device checks wait for a phone with a SIM. | Both |
| N6 | **Wake and connect in one tap from anywhere**, without a second always-on device: the computer tells the phone its MAC and broadcast address, the phone sends the magic packet itself at home and to the home's public address (a router forward) from outside, then connects once the computer answers. Each computer's method, relay, outside address and port are the user's to set (shipped 2026-10-14). | Both |
| N7 | **Multiple phones at once**, with a desktop-side picker: one phone drives at a time (or all, a setting), the desktop hands control over, and the others are told. Two quick Ends both apply (shipped 2026-10-14; checked with one phone, since only one is at hand). | Desktop |

**Paused 2026-10-14, after an outside review of the repo (the user's call):** stop widening the feature list and prove that what exists works together. N8–N11 wait until Phase R is done:
- N8 gamepad mode (a Bluetooth controller paired to the phone);
- N9 session recording and a screenshot button;
- N10 drawing-tablet mode;
- N11 the phone as a second monitor.

The rest of N7's per-phone items (mic choice, a reference-counted GPU clock pin, the Files picker reading control connections) wait too, except the privacy grab's owner, which is a security fix and moves into R3.

## Phase R: reliability (next, from 2026-10-14)

Each item gets its own spec section, results written into the checklist, and fixes with regression tests.

| # | Work | Proof |
|---|---|---|
| R1 | **End-to-end matrix.** Every workflow on the A71 against Windows (this box) and against Linux (a throwaway Azure VM in another country, which is also a real cross-network path): pairing and certificate checks, LAN and WAN sessions, clipboard and files both ways, notifications, the phone's screen off and the app in the background, a network change mid-session. | A results table per platform in the checklist, with dates. |
| R2 | **Long sessions and recovery.** 30 to 60 minutes streaming. Reconnect after a network change, a server restart, and the computer sleeping (sleep is the user's to run). Memory and CPU are recorded at the start and end. | Session records and a resource log per run. |
| R3 | **Adversarial security tests:** a revoked device, a replayed or malformed frame, an expired or reused PIN, a permission narrowed mid-session, path traversal, and another phone releasing the privacy grab. Each fails closed with a logged reason. | Tests in `cargo test`, one per case. |
| R4 | **What a session costs.** Idle CPU, memory and background traffic of the server, and the phone's battery over an hour, added to Diagnostics and the session record. | Numbers in the checklist. |
| R5 | **Honest docs.** Normal WAN access needs no port forward; only waking from outside does. First-contact trust and what the DHT exposes are stated up front. A platform-parity table (Linux / Windows / Android) and a release checklist. | README and SECURITY.md changes. |

**Status (2026-10-14):**
- **R1:** run on the Azure Linux VM; eight bugs found and fixed. Input on a real Linux desktop, files and notifications on Linux, and other compositors are owed (checklist §44).
- **R3:** done (branch `fix/r3-security`). Every case has a test, and the fresh review's five Important findings are fixed (`2026-10-14-phase-r-reliability.md`).
- **R4:** the desktop's idle cost is measured on Windows. The phone's battery, idle traffic and Linux are owed (checklist §45). Diagnostics and the session record do not show it yet.
- **R5:** done (branch `docs/r5-honest-docs`): "Before you start" and "What works where" in README, first contact up front in SECURITY.md, and `docs/release-checklist.md`.
- **T1 (after Phase R):** Check connection, version 1, built and device-checked (merged, PR #44).
- **T2:** Home and Settings say when Android may stop the background features, and open the page that allows them (branch `feat/background-onboarding`).
- **T3:** the QR scanner no longer uses ML Kit (zxing-cpp), and the APK is arm64-only with no dependency-info block (branch `feat/fdroid-ready`). The rest of the F-Droid plan, and why IzzyOnDroid is unlikely, is in `docs/research/2026-10-14-fdroid.md`.
- **T5:** `docs/protocol.md` (branch `fix/protocol-audit`). Writing it found eleven fixes, among them the phone's relay lookups going to public DNS resolvers, the wake relay sending wherever a phone named, and TLS 1.2 still accepted on TCP.
- **T6a:** resumable, hash-checked transfers both ways (PR #50; spec `2026-10-14-resumable-transfers-design.md`).
- **T6b:** folders both ways, each file resumable (PR #51), and "Download this folder" in the remote browser (PR #52; spec `2026-10-15-download-folder-design.md`).
- **T7a:** the local API on the desktop: `--json` on the CLI, `devices`, `sent`, `received`, `send`, and `docs/local-api.md` (branch `feat/local-api`; spec `2026-10-15-local-api-design.md`). **T7b:** automation apps on the phone: the intents documented (`docs/phone-automation.md`), and a share may name the saved computer it goes to (branch `feat/phone-automation`).
- **R2:** run on the A71 against Windows (branch `fix/r2-control-link`). The control connection now comes back after a network drop, and a dropped link reconnects by itself. Owed: an hour-long run, sleep and wake, mobile data, and Linux.

**Deferred, with reason:**
- A virtual display sized to the phone: Hyprland's write IPC is broken upstream, and Windows needs an indirect display driver.
- Session recording.

## After Phase R (planned 2026-10-14)

From the user's outside review and a fresh survey of what people ask for in KDE Connect, RustDesk, Moonlight, LocalSend, Phone Link and others (`docs/research/2026-10-14-what-users-need.md`). In order:

| # | Work | Why |
|---|---|---|
| T1 | **Connection Doctor on the phone:** one screen that tests the LAN, Tailscale and internet paths and maps a failure to a cause and a fix (AP isolation, VPN, a Public network profile, battery optimization, a pin mismatch, an unreachable relay, a denied permission) | "Can't see the other device, and no way to tell why" is the top complaint across these tools |
| T2 | **Staying connected in the background:** a battery-optimization exemption flow with per-manufacturer guidance, and a "connected while locked" state | Phones dropping off is the second |
| T3 | **F-Droid:** swap the QR scanner's bundled ML Kit (proprietary) for ZXing, make the APK build reproducibly, and use one signing key everywhere | The privacy-minded Linux audience; KDE Connect's Play Protect episode |
| T4 | **The first release,** through `docs/release-checklist.md`, then winget, AUR and Flathub | Working packages in every store |
| T5 | **A protocol document:** what mDNS, the DHT and relays expose | Trust, as Syncthing does |
| T6 | **Resumable transfers and "send folder"** | Failed transfers that start over |
| T7 | **The local API:** `--json` on every verb, a documented owner-only interface, Android intents for Tasker | Scripts and automation |
| T8 | **Media controls both ways, phone status on the desktop, contact names in texts, link handoff** (T8a, link handoff: done, `docs/superpowers/specs/2026-10-15-link-handoff-design.md`; T8b, the phone's battery on the computer: done, `docs/superpowers/specs/2026-10-15-phone-battery-design.md`; T8c, the computer's media from the phone: done, `docs/superpowers/specs/2026-10-15-media-controls-design.md`; T8d, the phone's media from the computer: done, `docs/superpowers/specs/2026-10-15-phone-media-design.md`; still to come: contact names in texts) | KDE Connect's best-loved small features |
| T9 | **Mobile input:** pinch-zoom in trackpad mode, portrait sessions, pointer capture | Moonlight's open requests |

Then, larger:
- a portal-native Linux host (RemoteDesktop portal with libei, ScreenCast restore tokens), which is also what Flathub needs;
- a virtual display sized to the phone (N11 becomes this);
- unattended access at the lock and login screens;
- the phone's camera as a webcam;
- the phone's screen on the computer without adb.

**Not planned, with reason:**
- folder sync (Syncthing owns it);
- a plugin system (a permanent promise and a security surface for a small team);
- iOS and macOS for now;
- gaming features (N8, Moonlight's ground).

## Rules for every item

- **Verification on this box:** the phone (Galaxy A71 over USB, unlocked by waking it) against the Windows host. Linux-only behaviour is unit-tested here, with a Linux CI run; on-device Linux checks stay owed in the checklist.
- **Branch, review, merge:** a fresh reviewer reads each branch before merge. Each merge to `main` waits for CI.
- **Docs:** README, the checklist and AGENTS *Current Status* change in the same commits as the code they describe.
