# Several phones at once (roadmap N7)

Date: 2026-10-14. Status: approved (autopilot; the user asked for the roadmap to continue without waiting, with every feature customizable in Settings).

## Why

Several phones can already stream at once. Each session has its own capture, encoder, audio, bitrate arbiter, monitor and view-only state, and the desktop app lists them with an End button each. The gaps are in what they share:
- **Input.** Every session's input goes into one channel and one injector, with no record of which phone sent it. Two phones fight over the cursor, and their key and modifier sequences interleave. The gamepad's button state is shared, so one phone's snapshot releases the other's buttons.
- **The privacy grab** is one global with no owner, so any phone can release another's.
- **The mic.** On Linux each phone's relay makes a source with the same name. On Windows they are mixed with no choice.
- **The NVENC clock pin** is released by the first session that ends, under the others.
- **The kick request** is one file, so two quick Ends can lose one.
- **The Files tab** offers phones that have a stream, but an offer needs a control connection.

The roadmap's "desktop-side picker" is the missing control: who drives.

## N7a: who drives

### The rule (server)

- **One phone drives at a time** (`input_control = "one"`, the default):
  - The first phone to send input takes control when no one holds it.
  - Input from the others is dropped while the driver is active.
  - Control frees itself after `input_idle_secs` (default 10) without input, or when the driving session ends.
  - With one phone, nothing changes.
- **Shared** (`input_control = "shared"`) is today's behaviour: every phone with the Input permission drives.
- **Code.**
  - `core::input::control::InputControl` is pure and tested: `admit(session, now) -> bool`, `release(session)`, `give(session)`, `holder()`.
  - Input packets carry their session id through the channel (`(u64, InputPacket)`), so the injector task asks `admit` before injecting.
  - The gamepad's last button mask is kept per session.

### The desktop picks

- **Sessions tab.** Each session shows "Driving" when it holds control, and a **Give control** button that hands it over at once.
- **Request files.** The app writes `control/<id>.json {session}`, a directory of one-file requests like approvals. The server applies them and deletes them.
- **Mirror.** `live_sessions.json` gains `driving: bool`.

### The phone is told

- **Packet.** When control moves, the server pushes `kdeconnect.pocketdesk.control {driving: bool, by: "<other phone's name>"}` to the phones in a session, on their control connections.
- **On the phone.** The session chrome shows a chip "<phone> is driving" while another phone drives, so a dropped tap is not a mystery. It goes away when control frees.

### Settings

- **Desktop, "Several phones":**
  - "Who controls the computer": One phone at a time / Shared (`input_control`).
  - "Hand control back after": 3–120 s without input (`input_idle_secs`).
- **Phone:** "Show who is driving" (on): the chip.

## N7b: per-phone resources

- **Privacy grab.** It is held by the phone that took it, and only that phone (or the desktop, or its session ending) releases it.
- **Mic.**
  - Linux: each phone's source is named `pocketdesk_mic_<8 hex of device id>` and described "Pocketdesk Mic (<phone name>)".
  - Windows: the virtual cable mixes them, which stays. Desktop Settings "Phone mics" chooses: all mixed (the default), or only the driving phone's (`mic_policy = "mix" | "driver"`).
- **NVENC.** The clock pin is reference-counted: the last session to end resets the clocks.
- **Kick.** Requests become one file each in `kick/`; the server applies every one.
- **Files tab.** It offers phones with a control connection. The server mirrors them to `connected.json {device_id, name, since}`, and `send_targets` reads that.

## Not in N7

Shared capture (one capture fanned out to several encoders) is a performance change. It is recorded as a follow-up, measured first: each phone costs one capture and one encode today.

## Tests

- **core:** `InputControl`:
  - first come takes control;
  - others are dropped;
  - control frees after idle;
  - `give` hands it over;
  - shared mode admits everyone;
  - a session's end releases it.
- **server:**
  - the input task drops a non-driver's packets;
  - per-session gamepad edges;
  - `control/` requests are applied and deleted;
  - the control push names the driver;
  - privacy release by a non-owner is refused;
  - the NVENC refcount (the pure counter);
  - kick requests in a directory;
  - `connected.json`.
- **desktop:** the Sessions row's Driving / Give control, the Files picker reading `connected.json`, and the Settings validation (3–120).
- **Kotlin:** the chip's text and lifetime (pure, JVM).
- **Device:** two phones are not at hand (one A71). The second phone is simulated with a second client on this box (an ignored test that opens a second v1 session and sends input), checking that the A71's chip appears and the desktop's Sessions tab shows who drives.

## Order of work

1. `core::input::control` and the session-tagged input channel (N7a server).
2. Control requests, the mirror, and the phone push.
3. Bridge and app: the chip, and its setting.
4. Desktop: the Sessions tab and Settings.
5. N7b, each item its own commit.
6. Docs: README, checklist §43, roadmap N7 (and remove "Multi-session" from Deferred), AGENTS.
