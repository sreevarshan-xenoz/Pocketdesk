# Desktop diagnostics (roadmap F10)

Date: 2026-10-13. Status: approved (autopilot).

## Why

"It doesn't connect" is the most common report, and nothing on the desktop explains it today: the person cannot see whether the server is listening, whether the firewall lets phones in, which addresses a phone can use, or how the last session went. F10 turns that into a list of checks with a reason and a next step each, plus a debug report to paste into an issue.

## What the person sees

A **Diagnostics** tab in the desktop app. It has a **Run checks** button, the results as one line each (✓, ⚠ or ✗, the finding, and a hint when something is wrong), and a **Copy debug report** button.

| Check | How | Wrong looks like |
|---|---|---|
| Server | the pid file and liveness (`core::local`) | not running: start it from Status |
| Control port | a TCP connect to `127.0.0.1:<control_port>` within 500 ms | nothing answers while the server runs: another program holds the port, or the server failed to bind |
| Stream port | binding UDP `<streaming_port>`: "in use" is right while the server runs | free while the server runs: the stream listener is down |
| Firewall (Windows) | the enabled rules in the `Pocketdesk` group (`Get-NetFirewallRule`, run from System32 with no console window) | none: run `linux-link install`, or allow the ports |
| Addresses | `core::local::local_ips()` | none: the computer is offline |
| Internet path | `endpoint.json` present | missing: WAN connections cannot find this computer yet |
| Last session | the last line of `streaming_sessions.jsonl` | shown, not judged: when it ended, how, the encoder, and the e2e, encode and RTT tails (p50 / p99) |

**The encoder.** The session record gains an `encoder` field (`VideoEncoder::backend_name()`, the last one the session used), so "which encoder" has an answer after the fact.

## The debug report

Plain text, meant to be pasted publicly. It contains:
- the app version and the OS;
- each check with its finding;
- the config ports and `pairing_required`;
- the last session.

It leaves out PINs, keys, certificate fingerprints, trusted ids and file names. Device ids are cut to 8 characters. Local addresses are included (they are private-network addresses, needed to diagnose), and the page says so next to the button.

## Pieces

1. **core.**
   - `SessionRecorder::note_encoder` and `SessionReport::encoder` (it appears in the JSON and in the log line).
   - The streamer notes the encoder when it opens one, and again after each rebuild.
2. **core, `local::diagnostics`.** Pure where it can be:
   - `last_session(state_dir)` parses the newest record;
   - `check_*` helpers with injectable ports;
   - `report_text`.
   - Tests use a temp state directory and loopback sockets.
3. **desktop.**
   - The Diagnostics tab. The checks run on a worker thread, since PowerShell takes about a second.
   - The Windows firewall probe lives here.
   - "Copy debug report" puts the report on the clipboard.
4. **Docs.** README (the desktop app section), AGENTS, and the roadmap entry marked shipped.

## Out of scope

- Probing from the phone's side (a "can the phone reach me" round trip).
- Fixing things automatically, such as adding firewall rules from the app; `linux-link install` already does that.
