# Pocketdesk as an ecosystem (2026-10-16)

The owner asked to turn Pocketdesk into "a whole ecosystem". This note is the research behind the plan, and the plan, as approved on 2026-10-16. Phase 6 (macOS, iOS) is set aside for now.

Labels:
- **[S]** a cited source says it;
- **[R]** read in this repo;
- **[I]** inference.

## What the field shows

- **Vendor ecosystems are walled by account and brand.**
  - Apple's Continuity needs one Apple Account, Wi-Fi and Bluetooth on every device [S: https://support.apple.com/102418]. iPhone Mirroring is still unavailable in the EU [S: https://heise.de/-10670669].
  - Samsung Multi Control needs one Samsung Account and One UI 5.1+ [S: https://www.samsung.com/uk/support/mobile-devices/navigate-your-galaxy-phone-and-galaxy-tablet-using-multi-control/].
  - Phone Link on iPhone has no images or message history [S: https://androidauthority.com/phone-link-iphone-3317453].
  - An open one wins on "across brands and systems, without an account" [I].
- **Interoperating with an existing protocol is a legitimate strategy.** Google's Quick Share reached AirDrop in November 2025 without Apple's help [S: https://9to5google.com/2025/11/20/android-quick-share-airdrop-pixel-10/].
- **Many clients make an ecosystem, and spread one spec bug to all of them.**
  - KDE Connect has official clients on Linux, Android, iOS, macOS and Windows, plus GSConnect and Valent [S: https://wikipedia.com/wiki/KDE_Connect].
  - Its iOS client cannot run in the background or read other apps' notifications [S: https://github.com/KDE/kdeconnect-ios].
  - CVE-2025-66270 hit every implementation at once: none checked that the second identity packet's id matched the first [S: https://app.opencve.io/cve/CVE-2025-66270].
- **A small, separately versioned spec lets third parties appear.** LocalSend publishes one, and it has Go, Python and Rust implementations [S: https://github.com/localsend/protocol].
- **Identity without an account comes in three shapes:**
  - pairwise (KDE Connect, Pocketdesk today);
  - introducers (Syncthing), convenient but surprising in practice [S: https://forum.syncthing.net/t/syn-thing-keeps-trying-to-share-folders/25250];
  - signed membership (Tailnet Lock, with a disablement secret against a compromised coordinator) [S: https://tailscale.com/kb/1226/tailnet-lock].
- **Desktop ↔ Home Assistant is an unfilled niche.** HASS.Agent (Windows, MQTT) sees little activity [S: https://github.com/hass-agent/HASS.Agent]. KDE's integration is an open task and a pre-alpha [S: https://phabricator.kde.org/T17435].
- **Device classes:**
  - **Browser:** native messaging launches a registered host program [S: https://developer.mozilla.org/en-US/Add-ons/WebExtensions/Native_messaging].
  - **Web clients:** iroh in the browser is relay-only [S: https://docs.iroh.computer/deployment/wasm-browser-support].
  - **Wear OS:** the Data Layer needs the same package and signing key on phone and watch [S: https://developer.android.com/training/wearables/data/overview].
  - **Android Auto:** limited to approved categories, which a remote desktop does not fit [S: https://developer.android.com/design/ui/cars/guides/templates/overview].
- **Sustainability without crippling the free product:**
  - Syncthing: a foundation plus a support company [S: https://syncthing.net/foundation/].
  - Home Assistant: a hosted convenience funding a foundation [S: https://nabucasa.com/].
  - RustDesk: self-hosted Pro licences [S: https://rustdesk.com/pricing].
  - KDE: a community fundraiser.
  - IzzyOnDroid refuses apps built partly with generative AI [S: https://izzyondroid.org/docs/general/AppInclusionPolicy/], so reproducible GitHub releases stay the channel nobody can take away.

## How close Pocketdesk is to KDE Connect [R]

**Shared:** packet names and many bodies (`kdeconnect.identity`, `.pair`, `.clipboard`, `.battery`, `.notification`, `.share.request`, `.sms.*`, `.mousepad.request`, `.findmydevice`, `.ping`).

**Not wire-compatible:**
- TLS first on TCP 1716 with ALPN `pocketdesk-control/1`;
- mDNS `_pocketdesk._tcp`, not a UDP 1716 broadcast;
- dial-back transfers with a one-time key and SHA-256 resume, not `payloadTransferInfo`.

`service.rs` already peeks the first byte on 1716, so a plaintext `{` could reach a compatibility handler.

## The plan

| Phase | Goal | Deliverables | Check |
|---|---|---|---|
| 1. Open the edges | Others build on Pocketdesk without its internals | A versioned protocol spec with test vectors replayed against the real handler; `pocketdesk watch --json` events and per-event hooks; a browser extension ("send to phone") over native messaging; a Home Assistant MQTT-discovery bridge (off by default, Settings); the same events as Android broadcasts | Conformance in CI; HA entities appear within 10 s; a link from the browser reaches the A71 |
| 2. Every feature on every road; computer ↔ computer | "Any of my devices" for two computers and a phone, from anywhere | Files, folders and image clipboard over iroh (no dial-back); pairing computers; clipboard and send between them; the other computer's screen in the desktop app; an input-only mode; edges crossing only where the OS allows (not wlroots) | 500 MB resumes over iroh with TCP 1716 blocked; Windows ↔ VM clipboard in under 2 s |
| 3. KDE Connect compatibility | The KDE Connect apps (iOS included), GSConnect and Valent work with Pocketdesk | An off-by-default compat backend (UDP 1716, the v8 identity exchange, `payloadTransferInfo`) for the plugins whose bodies match, with narrower default permissions; refuse while the real KDE Connect holds the port | Unmodified KDE Connect Android, iOS and GSConnect pair and do clipboard, a file and ping; a compat device refused on every Pocketdesk-only packet; the CVE-2025-66270 case tested |
| 4. A personal device mesh | Any device reaches any other without pairing each pair | Signer devices issue short-lived membership certificates; gossiped revocations; a recovery secret; membership grants nothing alone (each device keeps per-peer permissions); the relay's `--allow-paired` widens to members | A tablet added on computer A reaches computer B with no pairing on B; revoked within one link round |
| 5. More Android surfaces | Reach where the Android code already runs | Android TV (D-pad, gamepad), a Wear OS companion (media, wake, find, remote), a tablet pass | Emulator sessions on TV and Wear |
| 6. macOS, iOS | Set aside (2026-10-16) | — | — |

**Alongside every phase:**
- packaging: Flathub, winget, F-Droid, and a Play build without SMS;
- an RFC step for protocol changes;
- GitHub Sponsors;
- later, an optional hosted relay, with self-hosting always free.

## A second opinion, checked against the repo (2026-10-16)

The owner had ChatGPT review the public repository and propose its own plan. Each suggestion, checked against the tree:

| Suggestion | What the repo has | Taken? |
|---|---|---|
| Reliability and security first (Phase R) | R1–R5 ran on 2026-10-14; device checks are still owed per feature (checklist §§ 43–77) | **Yes:** a reliability pass before phase 2 (below) |
| An Activity Center: one timeline of transfers, sessions, pairing, notifications, automation, on the computer and the phone | The pieces live in separate files and tabs (`received.json`, `sent.json`, sessions, `streaming_sessions.jsonl`); phase 1a's event log is the shared model it needs | **Yes:** phase 1b, the desktop app's Activity tab over the event log, then the phone's |
| Connection Doctor on Android | Exists since T1 (`android/.../doctor/`, `ConnectionDoctorSheet.kt`), with "Copy report"; the desktop has Diagnostics (F10) | Already done |
| A local automation engine with rules | Phase 1a's hooks are the first, smaller step: a command per event kind | Hooks first; rules only if hooks prove too little |
| Named tasks the phone can start, with output and exit code back, timeouts, output limits, approval | **Found a real gap:** the Exec plugin runs *any* command a phone with Exec sends (off by default), with no timeout and no output limit | **Yes:** phase 1c, named tasks; and the old exec gets a deadline and an output cap |
| A capability registry with versioned negotiation | The identity packet lists each side's packet types; `llVersion`, the session mode marker (an old computer is detected as "too old"), `capabilities.rs` | Folded into phase 1g (the spec): write down how a client learns what a computer offers |
| Be honest that "no account" is not "no third-party infrastructure" | README "Before you start" and SECURITY.md already say what relays and the DHT see | Already done |
| No cloud accounts, no plugin marketplace, no folder sync (use Syncthing), no embedded AI agent | Matches "Not doing" | Agreed |

### The plan's phase 1, updated

1. **1a, events and hooks:** the event log, `pocketdesk watch`, `[hooks]` (in progress).
2. **1b, the Activity Center:** a desktop app tab over the event log (sessions, transfers, links, notifications, hooks run, refusals), then the phone's.
3. **1c, named tasks:** commands the owner names on the computer (Settings), which a phone with a new Tasks permission can start and see run: output (capped), exit code, a deadline, and an ask-first option. They show in the Activity Center. The raw Exec plugin gets the same deadline and cap.
4. **1d, the browser extension**, then **1e, the Home Assistant bridge**, then **1f, Android broadcasts**.
5. **1g, the protocol spec and test vectors,** including how capabilities are advertised and negotiated.
6. **A reliability pass before phase 2:** a long session, recovery after a network change, an interrupted transfer, and idle cost, re-run on current `main` on the A71 against Windows and the Azure VM, with the results in the checklist.

## Not doing

- **Accounts or cloud sync.** They throw away the one advantage.
- **Server plugins running third-party code.** The server reads the screen and injects input; extend out of process instead.
- **Android Auto.**
- **A Matter device.**
- **A full-session web client.** The browser is relay-only.
- **Edge-crossing on wlroots.**
- **An iOS companion that promises notifications or background work.**
