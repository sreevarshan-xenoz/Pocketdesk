# What users need next (research, 2026-10-14)

Two inputs:
- an outside review the user shared (ChatGPT): "Connection Doctor", "Performance Lab", seamless roaming, zero-account pairing, an Action Center, folder sync, a local API, and plugins;
- a fresh survey of what people complain about and ask for in the closest tools. It covered KDE Connect / GSConnect, RustDesk, scrcpy, Moonlight / Sunshine / Apollo, Parsec, Chrome Remote Desktop, Phone Link, Jump, Splashtop, AnyDesk, LocalSend, Warpinator and Syncthing, through GitHub issues, Hacker News and KDE Discuss.

This file says what Pocketdesk already has, where users hurt, and what to build next. The plan itself is in the roadmap (`docs/superpowers/specs/2026-10-12-feature-roadmap.md`, "After Phase R").

**Evidence notes:**
- Reaction and star counts are snapshots taken on 2026-10-14.
- Reddit could not be searched directly, so community evidence comes from Hacker News, KDE Discuss, Lemmy and GitHub.
- Claims marked *unverified* were not confirmed.

## What users complain about most

Ranked by how often and how strongly each comes up. Sources are linked.

1. **Devices do not find each other on the same network, and nobody can tell why.**
   - Causes: AP isolation, guest or enterprise Wi-Fi, VPNs, a Windows "Public" network profile.
   - This was the largest theme in the KDE Connect Hacker News thread of October 2025 (476 points: "no logs or diagnostics are exposed"). <https://news.ycombinator.com/item?id=45557599>
   - LocalSend's README has a whole troubleshooting section for it. <https://github.com/localsend/localsend>
2. **The phone drops off in the background** (Doze, OEM battery killers, a locked phone), and nothing reconnects.
   - KDE bugs 399236, 404163 and 393840.
   - KDE Discuss threads from 2025. <https://discuss.kde.org/t/kdeconnect-issues-and-how-gsconnect-works-better-in-that-regard/39391>
3. **Remote control of Wayland is fragile:**
   - a portal prompt every session;
   - no access at the lock or login screen;
   - input ignored under Flatpak.

   RustDesk's "Wayland support" issue has 95 👍 (<https://github.com/rustdesk/rustdesk/issues/56>), and its unattended-Wayland preview drew 346 points on Hacker News in August 2026.
4. **Same-network only.** People want to connect from outside without a VPN (LocalSend #1123, #2469). **Pocketdesk already does this** (iroh, your own relay, the DHT). It is a selling point to document, not a gap.
5. **No diagnostics.** Users cannot tell a firewall from AP isolation from a killed app.
6. **Clipboard sync broken by OS rules:**
   - Android blocks background clipboard reads;
   - GSConnect breaks with GNOME releases.
7. **Notification mirroring has gaps.** Android 15 hides "sensitive" notifications, such as one-time codes, from third-party listeners (<https://www.androidauthority.com/android-15-sensitive-notifications-3416414>). **This limits Pocketdesk's phone notifications too.** It is now stated in README.
8. **File transfers fail, cannot resume, and leave corrupt partial files** (LocalSend #1191).
9. **Typing and precise pointing from a phone are hard.** Moonlight issues: pinch-zoom #1572, portrait #1485, multi-touch #1348.
10. **The stream does not match the phone's screen**, and headless hosts show nothing. Apollo, a Sunshine fork with 11.3k stars, exists mostly for a virtual display that matches the client.
11. **Accounts, paywalls and "commercial use detected"** (AnyDesk, TeamViewer). RustDesk's public server now requires a login because of scams.
12. **Security doubts.**
    - RustDesk: unencrypted direct-IP mode, and closed parts.
    - LocalSend's first CVEs were path traversal and a man-in-the-middle in discovery: the classic first findings.
13. **Store policy kills features.**
    - KDE Connect lost SMS on Play in 2019.
    - KDE Connect's F-Droid build was flagged by Play Protect because it used a different signing key.
    - Syncthing quit Play in 2024.
    - Google's developer verification for sideloaded apps starts in 2026–2027 (*unverified whether enforcement began*).
14. **Calls and contacts:**
    - texts show bare numbers instead of names;
    - people ask to answer and place calls.
15. **Cost and swings in performance:**
    - RustDesk using several cores;
    - LocalSend idling at 20 % of a core;
    - Wi-Fi jitter.

## The outside review, item by item

| Suggestion | What Pocketdesk has today | Verdict |
|---|---|---|
| **Connection Doctor** | The desktop app's Diagnostics tab: server, ports, firewall, addresses, internet path, wake, last session, and a redacted report. The phone shows humanized errors and the reason a desktop closed. | **Yes, first.** The gap is the phone side: one screen that tests each path (LAN, Tailscale, internet) and maps a failure to a cause and a fix. The causes to cover: AP isolation, VPN, a Public network profile, battery optimization, a pin mismatch, an unreachable relay, a denied permission. It answers complaints 1, 2 and 5. |
| **Performance Lab** | The latency HUD with percentiles and the basis they were measured over; session records with five percentile tails; `pocketdesk bench`; the desktop's debug report. | **Mostly built.** Add two things: exporting a session's record from the phone into the same redacted report, and plain-language thresholds ("decode 16 ms: fine"). A separate lab mode would be gold-plating. |
| **Seamless roaming** | QUIC and iroh migration. Phase R2 added reconnect after a network drop and after a server restart, and the control connection coming back. | **Keep proving it, as reliability work.** Still owed: sleep and wake, mobile data, an hour-long run. Resumable transfers are a separate item (below). |
| **Zero-account pairing** | QR pairing that pins the certificate, device keys, per-device permissions, "ask before each session", Remove, and session records. No account exists. | **Already there.** Remaining: key rotation, and the phone showing its session history. |
| **Action Center** | The phone's progress notifications. The desktop app's Files (sent and received), Messages, Sessions and Devices tabs. | **Thin version only.** One "Activity" list (transfers, texts, sessions, refusals) helps support and the Connection Doctor. Do not build a second notification center. |
| **Folder sync** | — | **No.** Syncthing owns this, and conflict handling is a project of its own. Do a one-shot "send folder", and document using Syncthing alongside. |
| **Local API / CLI** | `pd` has verbs. The desktop app works through owner-only files. | **Yes, small:** `--json` output on every verb, a documented owner-only interface under the same per-device permissions, and Android intents for Tasker (KDE bug 377850). |
| **Plugin system** | — | **No, not for a small team.** A stable plugin interface is a permanent promise and a security surface sitting on input, files, texts and commands. "Run a command on an event" plus the local API covers most uses. |

## What to build after Phase R

These are ranked by value for effort, with evidence from the survey. S is under a week, M is 1–4 weeks, L is more.

1. **Connection Doctor on the phone** (S–M). Complaints 1, 2 and 5.
2. **Staying connected in the background** (M): a battery-optimization exemption flow with per-manufacturer guidance, a clear "connected while locked" state, and the measured cost (R4). Complaint 2.
3. **F-Droid, reproducibly built and signed with our own key** (M).
   - The privacy-minded Linux audience lives there, and KDE Connect, LocalSend and RustDesk all ship there.
   - **Blocker:** the QR scanner's bundled ML Kit is proprietary. Swap it for ZXing, at least in an F-Droid flavor.
   - One signing key everywhere avoids KDE Connect's Play Protect episode. IzzyOnDroid or Obtainium can serve meanwhile.
4. **A first real release through `docs/release-checklist.md`**, then winget, AUR and Flathub (S–M). LocalSend's lesson is to be in every store, each with working packages on release day.
5. **A protocol document** next to SECURITY.md (S), saying what mDNS, the DHT and relays expose, as Syncthing does.
6. **Resumable transfers and "send folder"** (M). The `.part` files and one-time keys make an offset resume with a hash check tractable. Complaint 8.
7. **The local API and Tasker intents** (S–M).
8. **Media controls both ways** (S–M): the desktop's now-playing through MPRIS on Linux and the Windows media session, shown on the phone, and the phone's media controlled from the desktop. This is among KDE Connect's best-loved features.
9. **Phone status on the desktop** (S): battery with a threshold alert, signal, Do Not Disturb, ring or silence.
10. **Contact names in texts and notifications, and an incoming-call toast** (S–M). The call-log policy on Play tightens in 2027, so skip full call handling.
11. **Link and text handoff** (S): "Open on computer" from the share sheet, and "Send to phone" from the desktop.
12. **Finishing mobile input** (S–M): pinch-zoom in trackpad mode, portrait sessions, mouse pointer capture.
13. **Portal-native Linux host** (L):
    - input through the RemoteDesktop portal and libei;
    - capture through ScreenCast with `persist_mode=2`, storing the new restore token after every start, since tokens are single-use;
    - this removes the uinput setup and is the prerequisite for Flathub.
14. **Virtual display sized to the phone** (M on Linux, L on Windows):
    - Linux: the portal's virtual source, GNOME's `RecordVirtual`, or Sway's `create_output`;
    - Windows: an indirect display driver, like Apollo's.
15. **Unattended access**, the lock and login screens (L). On Windows this needs a SYSTEM service on the secure desktop. Opt in only, with the owner's PIN, and visible.
16. **The phone's camera as a webcam** (M on Linux, L on Windows).
17. **The phone's screen on the computer, without adb** (M to view, L to control).
18. **Tablets, DeX and ChromeOS layouts** (M), and **accessibility** (S).
19. Later, with reasons:
    - macOS host and iOS viewer (L each; iOS kills background work);
    - gaming features (Moonlight and Apollo own this);
    - presentation remote (thin demand).

## Adoption and trust

- **One clear pitch, and working packages when it lands.** LocalSend ("an open-source AirDrop") reached the Hacker News front page three times, and RustDesk ("a TeamViewer alternative") three times. Pocketdesk's line: *an open-source Phone Link and remote desktop for Linux and Windows that works from anywhere without a VPN or an account.*
- **Every store, one signing identity, and a path outside Play** for permissions Play may refuse (texts).
- **Publish the protocol and what it exposes;** SECURITY.md already covers most of it. Use GitHub Security Advisories for fixes, and seek an outside review. KDE Connect's best security result came from a distribution's packager review.
- **Abuse shapes products:** RustDesk's scam problem. Pocketdesk's "nothing is accepted without the owner's PIN" design is worth stating.
- **Contributors:** a Weblate project for translations, "good first issue" labels, and a dev container for the Linux build (contributors on Windows cannot run the Linux gates).
- **Honesty about maturity is an asset** (RustDesk was hurt by its open- and closed-source mix). Keep README's "What works where" table current.
