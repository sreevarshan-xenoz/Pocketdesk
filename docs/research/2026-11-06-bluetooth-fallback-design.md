# Bluetooth as a fallback road (design, 2026-11-06)

## Why

On 2026-11-06 the owner's home Wi-Fi went down. The laptop went on to the
owner's phone's mobile data over USB, and the test phone had no network at
all, so nothing of Pocketdesk worked between the phone and the laptop beside
it. The phone and the laptop were a metre apart, both with Bluetooth on.

The owner wants Pocketdesk to keep working over Bluetooth when Wi-Fi drops,
**only when the two devices are in range of each other**: the light things
first (clipboard, notifications, the remote, deck buttons, ringing the phone,
small files), then the screen session, in low quality.

## What the owner decided

- **A fallback, not a new main road.** Bluetooth is used when neither the home
  road nor the internet road reaches the computer, and Pocketdesk moves back to
  Wi-Fi when it returns.
- **On by default**, with a switch on the phone and one on the computer, like
  the project's other options.
- **The first time it takes over**, the phone shows a one-time popup: Wi-Fi
  dropped, Pocketdesk is using Bluetooth now, and a button that goes straight
  to the setting to turn it off. After that it is silent.
- **The light things first, then the screen.** A screen session that is
  running when Wi-Fi drops **keeps going in low quality** over Bluetooth rather
  than pausing.
- **The OS pairing is done once by hand**, in Windows' (or Linux's) and
  Android's own Bluetooth settings. Pocketdesk's own pairing and permissions
  still apply on top: Bluetooth only carries the link and grants nothing.
- **Earbuds and watches share the radio.** The owner uses Bluetooth earbuds
  sometimes, so Pocketdesk must give way to them (below).
- **Windows first.** The owner's laptop is Windows, and the test phone (the
  OPPO) is paired with it.

## Goals

1. With Wi-Fi gone on either side (or both) and the devices in range, the
   phone's link to the computer comes back over Bluetooth by itself, within
   about 10 s of the home and internet roads failing.
2. Over that link, the light features work as on Wi-Fi: clipboard both ways,
   notifications mirrored, the remote, deck buttons, ring, Do Not Disturb,
   questions (`ask`), links, small files.
3. A screen session running when Wi-Fi drops keeps going over Bluetooth at low
   quality (about 480p, 10–15 fps, under 1 Mbit/s), and a new session can be
   started over Bluetooth.
4. When Wi-Fi returns, the link and any session move back to it.

## Non-goals

- Bluetooth Low Energy. Its rate is too low for anything but the lightest
  messages; this design is Bluetooth Classic (RFCOMM) only.
- Bluetooth as a primary road, or chosen while Wi-Fi works.
- iPhone, macOS (not supported by Pocketdesk at all).
- Computer-to-computer links over Bluetooth (the shared keyboard and mouse,
  the window handoff). Phone to computer only.
- Interoperating with KDE Connect's own Bluetooth backend. Its design can be
  read for ideas; matching its wire is not a goal.

## Anti-goals (it fails even if it works)

- **Music or calls stutter** on the owner's earbuds because Pocketdesk is
  pushing data. Audio wins.
- **The phone's battery** drains faster with the option on and Wi-Fi fine. No
  Bluetooth discovery, scanning or connection is kept up while another road
  works.
- **Anything is let in over Bluetooth that is not already let in over Wi-Fi.**
  An unbonded device, or a bonded device that is not a Pocketdesk-paired phone,
  gets nothing.
- **A silent half-state:** the phone shows "connected" while nothing gets
  through. If Bluetooth is too weak to carry the link, it says so like any
  other lost link.

## Constraints and facts (from the code, 2026-11-06)

- **The control link is already road-agnostic.** `ControlDuplex`
  (`core/src/protocol/connection.rs:16-21`) is any `AsyncRead + AsyncWrite`
  stream; `control_handshake_named` is generic over the stream; the server's
  `serve_control` (`server/src/service.rs:1612`) is generic too and branches on
  `enum Road { Local, Internet }` (`service.rs:1521`) for identity, pairing,
  dial-back and the local address. Bluetooth is a third `Road`.
- **The phone dials; the computer listens.** The phone's bridge tries home,
  then the internet road under `INTERNET_PAUSE` (`android/bridge/src/api.rs`
  around 600–637). Bluetooth is a third try after both. `INTERNET_PAUSE` is
  cleared on a network change (`android/bridge/src/lib.rs:1213`).
- **Android already has an RFCOMM server**, for the Wear OS watch
  (`android/app/.../watch/WatchLinkServer.kt`): secure RFCOMM, bonded peers
  only, `BLUETOOTH_CONNECT` checked, an allowlist of bonded addresses. The
  phone's side of this design is an RFCOMM **client**; the patterns
  (permission, adapter state, threads, idle close) carry over.
- **Android Bluetooth sockets are Java streams**, not file descriptors the Rust
  bridge can poll. The bridge needs the bytes through Kotlin.
- **The computer has no Bluetooth code.** Windows: the workspace `windows`
  crate (0.62) has no Bluetooth features enabled yet; Winsock `AF_BTH` sockets
  with an SDP service record (`WSASetService`) work under the repo's MinGW
  build. Linux: `zbus` is already used by the server, and BlueZ's `Profile1`
  (`RegisterProfile`) hands over an RFCOMM file descriptor per connection.
- **Video is QUIC only.** Every phone↔computer video path is a QUIC
  `Connection` (`core/src/streaming/connection.rs:169`, implemented by
  `QuinnConnection` and `IrohConnection`), one uni stream per packet
  (`streamer.rs:1925-1951`). The only byte-stream video in the repo is the
  loopback cast relay's framed wire (`core/src/local/cast.rs:53-99`).
- **There is no resolution knob.** The encoder takes the captured frame's size
  (`streamer.rs:1619-1628`); the lowest bitrate preset is Economy at
  1.5 Mbit/s (`input_packet.rs:190-194`); `slow_link_fps` steps the frame rate
  down to 15 (`bitrate.rs:421-449`).
- **Settings conventions.** An on-by-default option with a switch on each side
  follows "Files over the internet": `internet_files` in
  `core/src/local/config.rs` (default true, `KNOWN_KEYS`, a desktop row) and
  `Prefs.internetFiles` on the phone, pushed to the bridge.
- **Repo rules** (`docs/AGENTS.md`): one logical change per commit, docs in the
  same commit, a `docs/status-history.md` entry per item, a SECURITY.md limit
  for anything that opens a new way in, gates on Windows and Linux, owed device
  checks in `docs/device-verification-checklist.md`.

## How it works

### The road

- **The computer listens** on an RFCOMM service with Pocketdesk's own service
  UUID (a new one, not the watch's), registered so a bonded phone can find its
  channel by SDP. It listens only while the computer's switch is on and its
  Bluetooth radio is on.
- **The phone dials** that service on a bonded device that is a known
  Pocketdesk computer, only after the home and internet roads have both failed,
  and only while the phone's switch is on and `BLUETOOTH_CONNECT` is granted.
- **Over the RFCOMM stream the control link is the same as at home:** TLS with
  the computer's pinned certificate, then the usual handshake. The server serves
  it as `Road::Bluetooth`. Secure RFCOMM (bonded, encrypted) is a first wall;
  the pinned TLS and Pocketdesk's pairing and permission gates are the ones that
  decide, exactly as at home.
- **Which bonded device is which computer.** The phone learns each paired
  computer's Bluetooth address over a working road (a new field the computer
  sends in its identity, or the phone matching the bonded device's name; the
  builder picks, see below) and keeps it with that computer's record. The phone
  never dials a bonded device it has not tied to a Pocketdesk computer this way.

### When it is used

- The phone's existing loop that keeps the link (the notification listener's
  `keepLinked`, every 10 s) tries home, then internet, then Bluetooth.
- On a network change back to a usable network, the phone tries home and
  internet again at once (the `INTERNET_PAUSE` clear already does this for
  internet); once one of them is up, the Bluetooth link is closed.
- Bluetooth dials back off like the internet road (a short rest after a failed
  dial, growing on repeated failures), so an out-of-range computer is not dialed
  every 10 s for hours.

### Giving way to earbuds and watches

- While the phone plays audio to a Bluetooth headset (A2DP) or is in a call
  (HFP/SCO), Pocketdesk keeps to the light features and sends them in small
  bursts; the screen over Bluetooth is not offered, and a running session drops
  to its lowest setting or pauses its video (the builder measures which keeps
  audio clean).
- The watch link already uses the phone's radio; the two share it, neither
  starves the other.

### What waits for Wi-Fi

- File transfers over a size threshold (about 8 MB, the builder's call) wait
  for Wi-Fi rather than crawl; small files go. The phone says the file is
  waiting for Wi-Fi.

### The screen over Bluetooth (phase 2)

- A new `Connection` over a byte stream: the same packets as today, framed with
  a stream id and length on one ordered stream (the cast relay's framing is the
  model), carried on its own RFCOMM link or on the control link (the builder
  measures which).
- A new low setting the server applies on that road: the picture scaled to
  about 480p, 10–15 fps, under 1 Mbit/s. This needs a downscale step in the
  encoder path that does not exist today.
- A session running on Wi-Fi when Wi-Fi drops keeps going: it reconnects over
  Bluetooth in the same way BG6's recovery reconnects (PR #650), at the low
  setting, and moves back when Wi-Fi returns.

### The one-time notice and the switches

- **Phone:** "Keep working over Bluetooth when Wi-Fi drops", on by default, in
  Settings near the connection options; a `Prefs` flag that the first takeover
  was told. The first time the Bluetooth road carries the link, a dialog (when
  the app is open) or a notification (when it is not) says so, with a button to
  the setting. It asks for `BLUETOOTH_CONNECT` where it is not yet granted;
  without it the option does nothing and Settings says why.
- **Computer:** `bluetooth_fallback` in `config.toml`, default true, with a row
  in the desktop app's Settings, read by the server.
- **The status:** wherever the session's road shows today (LAN, WAN, relayed),
  Bluetooth shows as its own road.

## Phases

1. **The light features over Bluetooth, Windows and Android.** The road on both
   sides, the switches, the notice, giving way to audio, files waiting for
   Wi-Fi, the docs, SECURITY.md limit, owed device checks. Ship this on its own.
2. **The screen over Bluetooth.** The byte-stream `Connection`, the downscale,
   the low setting, the move between roads mid-session.
3. **Linux** (BlueZ `Profile1` over `zbus`) for both phases.

## Testing

- **Unit tests** for everything pure: road choice and back-off, the framing,
  the audio rule, the size threshold, config and `Prefs` defaults, the
  computer's Bluetooth address in the identity.
- **Device checks on the OPPO** (bonded with the owner's Windows laptop),
  scripted in `scripts/device_checks.py` where adb can drive them: Wi-Fi off on
  the phone, the link comes back over Bluetooth within the target time; a
  clipboard copy each way and a test notification arrive; with Bluetooth off on
  the computer, nothing pretends to be connected; Wi-Fi back on, the link moves
  back. The screen checks come with phase 2.
- **Throwaway servers on this Windows PC** run only from
  `%LOCALAPPDATA%\pocketdesk-test\pocketdesk.exe` (the firewall rule for that
  path is in place). Bluetooth itself needs no firewall rule.

## Calls made after review (the designer's, not the owner's)

A builder's read of this document raised five questions. These answers are
the designer's calls, made so nothing stays open; the owner can overturn any.

1. **Which road's rules `Road::Bluetooth` follows** (`server/src/service.rs`
   branches on the road in several places). Per rule:
   - **Pairing: as the internet road.** A link that claims a paired phone with
     the wrong key is refused outright; it is never held for a PIN re-pair over
     Bluetooth. Pairing happens at home, as today. (Closes a new way in.)
   - **Naming the computer: as the internet road**, after the phone is admitted.
   - **The away lock: the phone counts as near.** A Bluetooth link proves the
     phone is within a few metres, so `away_lock`'s presence check treats a
     live Bluetooth link as present. (Otherwise the computer would lock with the
     owner beside it and Wi-Fi down.)
   - **Wake info and the computer-to-computer rules: as the internet road**
     (nothing is offered that depends on an IP the link does not have).
   Each gets a test, and SECURITY.md's new limit says all four.
2. **Earbuds on the laptop, not only the phone.** Phase 1 gives way on the
   phone (where the owner's earbuds usually are). The laptop's side is measured
   during phase 1's device checks with earbuds on the laptop; if audio there
   stutters, a server-side throttle comes in before phase 1 ships, else it is
   listed as owed.
3. **Files over Bluetooth: the phone opens a second RFCOMM connection per
   transfer** (same service, a first message that says it is a transfer for the
   link already open), so the control wire is not changed and needs no new
   version. Phase 2's video follows the same pattern (its own RFCOMM
   connection) unless measuring shows one link is better.
4. **"About 10 s" is a target, not a hard limit.** Plain tries in order (home,
   internet, Bluetooth), with one change: when the phone knows it has no usable
   network at all, it skips straight to Bluetooth. The device checks report the
   time; more than 20 s is a failure.
5. **What checks may do to the owner's Windows PC.** They never turn the
   laptop's Bluetooth radio off or unpair anything (it would drop the owner's
   own devices). "Nothing pretends to be connected" is checked by turning the
   throwaway's switch off or stopping it. The throwaway listens on a different
   service UUID, set by an environment variable that only tests use, so it can
   live beside the owner's own server. The scripted Bluetooth checks run with no
   `adb reverse` (it would keep the home road alive).

## Left to the builder

- How the phone ties a bonded device to a paired computer (an identity field,
  a name match, or asking once).
- How the Rust bridge gets the RFCOMM bytes on Android (a Kotlin pump over a
  local socket pair, or a JNI stream), and whether the TLS pin can stay keyed by
  the computer rather than by the address dialed.
- Winsock `AF_BTH` directly or WinRT `Windows.Devices.Bluetooth.Rfcomm` on
  Windows, as long as it builds under MinGW with no Windows SDK.
- Whether phase 2's video rides its own RFCOMM link or the control link.
- The exact thresholds (file size, audio behaviour, back-off), measured on the
  OPPO.
