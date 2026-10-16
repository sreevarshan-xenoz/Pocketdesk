# The Pocketdesk protocol, version 1

This is the contract between a phone (or any client) and a computer running Pocketdesk: what a second implementation must do to talk to one. `docs/protocol.md` maps the same wire from the other side: every listener, file and outside party, with the code each row comes from. Two files are generated or checked by CI, so they cannot drift from the code:

- `docs/packets.md`, every control packet and the permission that gates it;
- `docs/protocol-vectors/control-v1.json`, conversations replayed against the real handler.

## 1. Versions

- **The protocol** is version `1`. It is named in the control handshake (`POCKETDESK_HELLO 1` / `POCKETDESK_OK 1`) and in the identity packet (`protocolVersion`).
- **Pocketdesk's own packets** (`kdeconnect.pocketdesk.*`) carry `llVersion: 1` at the top level. A receiver must ignore fields it does not know, at the top level and in a body. A change that an older peer would misread gets a new packet type or a new `llVersion`; packets are never redefined in place.
- **The ALPNs** carry their own versions:
  - `pocketdesk-control/1`, the control channel over iroh; on TCP the version is in the HELLO line;
  - `pocketdesk-stream`, the v1 stream;
  - `pocketdesk-v2`, the multiplexer;
  - `pocketdesk-transfer/2`, a file transfer.

  A peer that speaks only an older transfer version fails the handshake rather than misreading it.
- **The stream's input tags** (§5) are never renumbered. An unknown tag fails that packet's decoding.

## 2. Roads

A phone reaches a computer:

- **At home, over the network:**
  - the control channel on TCP 1716, inside TLS 1.3 (§3);
  - the stream on UDP 4716, over QUIC (§5).
- **From anywhere, over iroh:**
  - the control channel on the computer's iroh endpoint with ALPN `pocketdesk-control/1`;
  - the stream with `pocketdesk-stream`.

  Both ends are proved by their iroh keys. The computer's endpoint id and relays reach a phone in `kdeconnect.pocketdesk.endpoint` at home.
- **File and clipboard-image bytes** go on a connection the computer dials to the phone, TLS 1.3 with ALPN `pocketdesk-transfer/2`. Each side accepts exactly one certificate: the pinned one (`docs/protocol.md` §1.4). It is not available on the iroh road.

## 3. The control channel

### 3.1 Framing

- **TLS:** TLS 1.3 only. The client must present a certificate; its SHA-256 is the key the computer holds the phone to. The first byte must be a TLS handshake record (`0x16`). A client that sends anything else gets the line `POCKETDESK_ERR 1 tls-required`, and the connection closes.
- **Lines:** inside TLS, everything is lines of UTF-8 ending in `\n`. Handshake lines are at most 64 KiB. A packet line is at most 4 MiB, and a longer one closes the connection.
- **A packet** is one JSON object per line:

  ```json
  {"id": 0, "type": "kdeconnect.pocketdesk.tasks", "body": {}, "llVersion": 1}
  ```

  - `type` is required. `body` is an object, `{}` when empty.
  - `id` (a number) may be 0.
  - `payload_size` and `replyId` are optional.
  - A line that is not a packet is skipped, and the connection goes on.

### 3.2 Handshake

**At home (TCP):**

1. The client sends `POCKETDESK_HELLO 1`; the computer answers `POCKETDESK_OK 1`.
2. The computer sends its identity packet (§4).
3. The client sends its own `kdeconnect.identity` within 5 s.
4. Its `deviceId` must be 1 to 128 bytes, with no control or invisible formatting characters, and not the id Pocketdesk keeps for Home Assistant (`home-assistant`).
5. The connection is known by that id. A second identity on it is ignored.

**Over iroh:** the same HELLO/OK, then the client's identity at once. The computer names itself only after it has admitted the phone: the phone must be paired, with this iroh key registered. Otherwise it answers `POCKETDESK_ERR 1 refused` and closes.

### 3.3 Standing and permissions

The computer re-reads standing for every packet, so pairing, unpairing or a key change applies to an open connection.

- **Unpaired** (or paired under another key): only `kdeconnect.pair` and `kdeconnect.pocketdesk.pair` are acted on. Everything else is dropped without an answer. Pairing happens only at home. (A computer whose owner set `pairing_required = false` serves an unpaired phone as a paired one with the default permissions, on the home road only.)
- **Paired:** each packet needs the permission `docs/packets.md` names for it, set per phone in the computer's Devices tab. A packet without its permission is dropped; a file or audio request is answered as refused, so the phone can say why. A packet type no plugin takes is ignored.
- **Pairing:** a phone asks for a PIN (`kdeconnect.pocketdesk.pair {requestPin: true}`), and the computer shows it on its own screen. The phone then sends `kdeconnect.pair {pin}` and is answered `kdeconnect.pair {pair, serverId}`. A wrong PIN is answered `pair: false`. Pairing binds the certificate that paired. The QR code's link (`pocketdesk://pair`) carries the computer's certificate fingerprint, so a phone can pin it before it first connects (`docs/protocol.md` §2.6).

## 4. Capabilities: how a client learns what a computer offers

- **The identity packet** carries `deviceId`, `deviceName`, `deviceType`, `protocolVersion`, `incomingCapabilities` and `outgoingCapabilities`:
  - `incomingCapabilities` lists every packet type the computer acts on;
  - `outgoingCapabilities` lists every packet type it pushes or answers with.

  `docs/packets.md` is the same lists, with the permission for each. A test holds every packet a plugin sends to its announcement.
- **What to do with them:** a client should offer a feature only when the computer lists its packets. A computer too old for a feature is told apart that way, and not by its version number.
- **The stream** is negotiated separately (§5):
  - codecs by the phone's capability marker;
  - the session's purpose by its mode marker.

  A computer that ignores a mode it does not know streams video as usual. A phone that asked for a remote and got video says the computer is too old.
- **`pocketdesk capabilities [--json]`** on the computer prints what the build can do: transports, capture backends, codecs and audio (`docs/capabilities.md`).

## 5. The stream

The stream is QUIC (`pocketdesk-stream`).

- **From the phone:** setup markers on uni-streams before or during the session:
  - the screen;
  - its device id;
  - its iroh key;
  - its codec capabilities;
  - the session mode: screen, input only, or cast.

  Input travels as tagged packets, tags 0 to 14. The tables are in `docs/protocol.md` §3.2, from `core/src/streaming/{client,streamer,input_packet}.rs`.
- **From the computer:** each frame or audio packet goes on its own uni-stream, with an 18-byte header:
  - sequence, u64;
  - kind: `0` video, `1` audio, `2` ready (an input-only session was admitted), `3` keyframe wanted (a cast);
  - a keyframe flag;
  - a timestamp, u64.

  Video is H.264, or HEVC when both ends allow it. Sound is Opus at 48 kHz, stereo.
- **Gates:** a stream session is held to the same pairing, keys and permissions as the control channel. Without the Input permission it is forced to view only.

## 6. Conformance

- **`docs/protocol-vectors/control-v1.json`** holds the control channel's conversations, each from a new, unpaired connection:
  - TLS required;
  - the handshake and the identity's capability lists;
  - an unpaired request unanswered;
  - a wrong PIN answered no;
  - a second identity ignored;
  - a line that is not a packet skipped;
  - an over-long line closing the connection.
- **Each case's steps** send a line, or expect the next line (matched on type and the body fields given), silence, or the end.
- **How they are checked:** `server/src/service.rs` (`vector_tests`) replays every case against the real handler with every plugin the computer runs, in CI.
- **For another implementation:** a client can replay the same file against a computer, and a computer against its own handler.
- **Not covered yet:** a paired phone's packets; the stream; the transfers; iroh.

## 7. Not in version 1

- Phone ↔ phone, and computer ↔ computer (ecosystem phase 2).
- KDE Connect's own pairing and packets beyond the ones listed (phase 3).
- Discovery beyond mDNS `_pocketdesk._tcp`.
