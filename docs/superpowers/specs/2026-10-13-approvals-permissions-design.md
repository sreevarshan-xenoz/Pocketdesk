# Connection approval and per-device permissions — design

Status: decided 2026-10-13, branch `feat/approvals-permissions`. Roadmap F5 ([feature roadmap](2026-10-12-feature-roadmap.md)); research §8.

## Why

Today a paired phone can do everything, always: watch, type, read and set the clipboard, browse and send files, hear desktop audio, feed the mic, lock or sleep the desktop, and run any shell command (`plugins/exec.rs` passes the string to `sh -c` / `cmd /C`). Every comparable host lets its owner decide per device (Parsec per guest, RustDesk access modes, Apollo per client). Most also let the owner approve a session at the desk.

Success means the owner can, from the desktop app's Devices tab:
- choose for each paired phone what it may do;
- choose whether it must be approved before each session.

The server enforces both, whatever the phone sends.

## What exists

Mapped in detail on 2026-10-13 (file:line in the PR). The points that matter:
- **Packet gates.** Control packets pass a pre-dispatch gate on the TCP path (`service.rs`) and on QUIC v2 (`v2_multiplexer.rs`). On TCP the sender is keyed by **IP**; the real deviceId is in pair.rs's `PHONE_IDS`.
- **Stream gate.** Every stream (LAN and WAN) passes one pairing gate in `StreamingServer::run_pipeline`, after the deviceId is known and before capture starts. QUIC keepalive (15 s, idle timeout 45 s) tolerates a wait there.
- **View-only** is a flag the **phone** sets. Input is dropped in the session monitor task, the only place that still knows whose input it is. KDE mousepad input bypasses it.
- **Trust store.** `trusted_devices.json` is re-read on every check, and its unknown fields default safely on load.
- **Precedent.** The desktop app and server already talk through files polled every second (the live-session mirror, the kick request).

## Decisions (made autonomously; each is cheap to revisit)

| # | Decision | Why |
|---|---|---|
| P1 | Permissions per device: **input**, **clipboard**, **files**, **audio** (desktop sound to the phone), **mic** (phone mic to the desktop), **power** (lock/sleep/shutdown), **exec** (run commands). A device without an entry may do everything except **exec**, which is off until granted. | The first six keep today's behaviour, so nobody's own phone breaks. Exec is a remote shell, and a pairing PIN should not hand one out by default. |
| P2 | Stored as a new top-level `"permissions": {"<deviceId>": {...}}` map in `trusted_devices.json`, `#[serde(default)]`. Forgetting a device removes its entry. | Old readers ignore it, and new readers load old files. Each entry carries only what differs from the defaults (a missing field means default), so a later new permission defaults sensibly. |
| P3 | **Control packets** are gated by type at the two existing pre-dispatch gates, through one `capability_for(packet_type)` table. A denied packet is dropped and logged, and the phone gets an error reply where the plugin would have answered. TCP resolves IP → deviceId through pair.rs's map. | One check covers every plugin, and a new plugin gets a table row instead of its own check. |
| P4 | **Streams.** The pairing gate becomes a policy gate returning the session's permissions. Without **input**, the server forces view-only (ORed with the phone's flag, so the phone cannot lift it). Without **audio**, the audio pipeline is not started. Without **mic**, mic packets are dropped. KDE mousepad input obeys **input** too. | These are the points that know whose stream it is. |
| P5 | **Approval.** A per-device "Ask before each session" (off by default). When on, the stream gate holds the session: the server writes `<state>/pending_approvals.json` and raises a desktop notification. The desktop app shows a prompt with the phone's name and address and **Allow once / Always allow / Deny**, and writes its answer to `<state>/approval` (request id, verdict, time). The server consumes it on its existing 1 s tick. No answer in **30 s** is a deny. "Always allow" turns the per-device setting off. | It reuses the proven file channel, adds no socket, and still works when the app was closed (the request just times out). Off by default, because an owner reaching their own desktop from elsewhere has nobody at the desk to click Allow. |
| P6 | The Devices tab gets a row of toggles per device, plus the "Ask before each session" switch. Changes apply to the next packet on the control channel and to the next session for streams. | The server re-reads the file per check; a live session keeps the permissions it started with, the way Parsec does. |

## As built (2026-10-13)

Where the build differs from the decisions above:

- **P3:** a denied control packet is dropped and logged (`Denied <type> from <device>`). No error reply goes back to the phone yet, so a denied query looks like no answer.
- **P5:** each answer is its own file, `<state>/approvals/<id>.json`, not one shared `<state>/approval`, so two phones held at once cannot overwrite each other's answer. The held session polls for its own answer every 250 ms instead of waiting for the 1 s tick. An answer that arrives after its session went away is swept up when the next held session leaves the list. A WAN (iroh) session has no socket address, so the prompt names it "the internet (WAN)".
- **P5, the phone:** a held phone shows "Connected — waiting for the picture" with a hint to allow it at the computer, and waits up to 45 s for its first frame. A refusal (deny or no answer) closes the stream with the reason `session refused`. The bridge sees the close on its 1 s stats poll, so the phone shows the refusal in about a second.
- **P6:** a change to a control-channel permission applies to the next packet. Input, desktop sound and the microphone for a session already running keep what that session started with.
- **Limit:** approval holds the *stream* only. A paired device's control channel (window list, battery, the HUD) keeps working while its session is held or refused, as the permission switches allow.

## Out of scope

- Per-command allowlists for exec.
- Permissions for unpaired devices: they get nothing but pairing, as today.
- Time-of-day rules.
- Fixing the IP-keyed TCP identity, beyond resolving it at the gate.

## Testing

| Layer | Tests |
|---|---|
| core | The permissions map: defaults (exec off), a round trip through the file, an old file without the map, forgetting a device removes its entry, older-shaped writers. The approval request and answer files: format, time limit, a stale answer ignored. `capability_for` covers every incoming packet type the registry knows. |
| server | The control gate denies a packet whose capability is off (on both TCP and v2). The stream gate forces view-only, skips audio and drops mic per the permissions. An approval held then allowed, denied, and timed out. |
| desktop | The model shows each device's permissions, a toggle writes them, and a pending request turns into the prompt. |
| Device | Phone-only actions against the Windows host, with the result read from the server log and the files: clipboard denied, then allowed; exec denied by default. Approval prompt and click: only while the user is away from the desk. |
