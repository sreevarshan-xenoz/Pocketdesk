# The Pocketdesk protocol, version 1

This is the contract between a phone (or any client) and a computer running Pocketdesk: what a second implementation must do to talk to one. `docs/protocol.md` maps the same wire from the other side: every listener, file and outside party, with the code each row comes from. Two files are generated or checked by CI, so they cannot drift from the code:

- `docs/packets.md`, every control packet and the permission that gates it;
- `docs/protocol-vectors/control-v1.json`, conversations replayed against the real handler.

## 1. Versions

- **The protocol** is version `1`. It is named in the control handshake (`POCKETDESK_HELLO 1` / `POCKETDESK_OK 1`) and in the identity packet (`protocolVersion`).
- **Pocketdesk's own packets** (`kdeconnect.pocketdesk.*`) carry `llVersion: 1` at the top level. A receiver must ignore fields it does not know, at the top level and in a body. A change that an older peer would misread gets a new packet type or a new `llVersion`; packets are never redefined in place.
- **The ALPNs** carry their own versions:
  - `pocketdesk-control/1`, the control channel, on both roads: a TLS client on TCP that offers ALPN must offer this one (offering none also passes); the HELLO line carries the version as well;
  - `pocketdesk-stream`, the v1 stream;
  - `pocketdesk-v2`, the multiplexer;
  - `pocketdesk-transfer/2`, a file transfer.

  A peer that speaks only an older transfer version fails the handshake rather than misreading it.
- **The stream's input tags** (§5) are never renumbered. An unknown tag fails that packet's decoding.

## 2. Roads

A phone reaches a computer:

- **At home, over the network:**
  - the control channel on TCP 1716 by default (`control_port`), inside TLS 1.3 (§3);
  - the stream on UDP 4716 by default (`streaming_port`), over QUIC (§5).
- **From anywhere, over iroh:**
  - the control channel on the computer's iroh endpoint with ALPN `pocketdesk-control/1`;
  - the stream with `pocketdesk-stream`.

  Both ends are proved by their iroh keys. The computer's endpoint id and relays reach a phone in `kdeconnect.pocketdesk.endpoint` at home.
- **File and clipboard-image bytes** go on a connection the computer dials to the phone, TLS 1.3 with ALPN `pocketdesk-transfer/2`. Each side accepts exactly one certificate: the pinned one (`docs/protocol.md` §1.4). On the iroh road a phone asks without a port, and the computer opens a bidirectional stream on the phone's control connection instead: the transfer key line, then the same transfer, with no TLS layer of ours, since iroh proved both keys (`docs/protocol.md` §1.2a step 7). A request naming a port there is refused, as is one naming none at home.

## 3. The control channel

### 3.1 Framing

- **TLS:** TLS 1.3 only. The client must present a certificate; its SHA-256 is the key the computer holds the phone to once one is bound (pairing binds it; a phone paired before keys existed is taken by its id until its first session at home binds its key). The first byte must be a TLS handshake record (`0x16`). A client that sends anything else gets the line `POCKETDESK_ERR 1 tls-required`, and the connection closes.
- **Lines:** inside TLS, everything is lines of UTF-8 ending in `\n`. Handshake lines are at most 64 KiB. A packet line is at most 4 MiB, and a longer one closes the connection, as does a line that is not UTF-8.
- **A packet** is one JSON object per line:

  ```json
  {"id": 0, "type": "kdeconnect.pocketdesk.tasks", "body": {}, "llVersion": 1}
  ```

  - `type` is required. `body` is an object, `{}` when empty.
  - `id` (a number) may be 0.
  - `payload_size` and `replyId` are optional.
  - `source` is the sender's device id; the computer sets it on everything it sends.
  - A line of UTF-8 that is not a packet is skipped, and the connection goes on.

### 3.2 Handshake

**At home (TCP):**

1. The client sends `POCKETDESK_HELLO 1`; the computer answers `POCKETDESK_OK 1`.
2. The computer sends its identity packet (§4).
3. The client sends its own `kdeconnect.identity` within 5 s.
4. Its `deviceId` must be 1 to 128 bytes, with no control or invisible formatting characters, and not the id Pocketdesk keeps for Home Assistant (`home-assistant`).
5. The connection is known by that id. A second identity on it is ignored.

**Over iroh:** the same HELLO/OK, then the client's identity at once. The computer names itself only after it has admitted the phone: the phone must be paired, with this iroh key registered. Otherwise it answers `POCKETDESK_ERR 1 refused` and closes (an identity whose `deviceId` breaks the rules above is closed without that line).

### 3.3 Standing and permissions

The computer re-reads standing for every packet, so pairing, unpairing or a key change applies to an open connection.

- **Unpaired** (or paired under another key): only `kdeconnect.pair` and `kdeconnect.pocketdesk.pair` are acted on. Everything else is dropped; the first such packet on a connection is answered once with `kdeconnect.pocketdesk.not_paired {}`, so a phone knows to stop (BB1), and a connection that had been let in is told so as it closes. Pairing happens only at home. (A computer whose owner set `pairing_required = false` acts on an unpaired phone's packets with the default permissions, on the home road only; what it keeps per phone, its battery, media and folders, it still keeps for paired phones alone.)
- **Paired:** each packet needs the permission `docs/packets.md` names for it, set per phone in the computer's Devices tab. A packet without its permission is dropped without an answer, except a folder fetch (`kdeconnect.pocketdesk.folder_fetch`), an audio request (`kdeconnect.pocketdesk.audio`) and a privacy request (`kdeconnect.pocketdesk.privacy`, which carries Lock), which are answered as refused so the phone can say why; a client should time out the rest. A packet type no plugin takes is ignored.
- **Pairing:** a phone asks for a PIN (`kdeconnect.pocketdesk.pair {requestPin: true}`), and the computer shows it on its own screen. The phone then sends `kdeconnect.pair {pin}` and is answered `kdeconnect.pair {pair, serverId}`. A wrong PIN is answered `pair: false`. Pairing binds the certificate that paired. The QR code's link (`pocketdesk://pair`) carries the computer's certificate fingerprint, so a phone can pin it before it first connects (`docs/protocol.md` §2.6).

## 4. Capabilities: how a client learns what a computer offers

- **The identity packet** carries `deviceId`, `deviceName`, `deviceType`, `protocolVersion`, `incomingCapabilities` and `outgoingCapabilities`:
  - `incomingCapabilities` lists every packet type the computer acts on;
  - `outgoingCapabilities` lists every packet type it pushes or answers with: its plugins' and those the server sends itself (the internet endpoint, wake info, control hand-over, the workspace HUD).

  `docs/packets.md` is the same lists, with the permission for each. A test scans the server's code for every packet it builds and holds each to the announcement.
- **What to do with them:** a client should offer a feature only when the computer lists its packets. A computer too old for a feature is told apart that way, and not by its version number.
- **The stream** is negotiated separately (§5):
  - codecs by the phone's capability marker;
  - the session's purpose by its mode marker.

  A computer that ignores a mode it does not know streams video as usual. A phone that asked for a remote and got video says the computer is too old.
- **`pocketdesk capabilities [--json]`** on the computer prints what the build can do: transports, capture backends, codecs and audio (`docs/capabilities.md`).

## 5. The stream

The stream is QUIC (`pocketdesk-stream`).

- **From the phone:** setup markers, each on its own uni-stream, as the session's first streams: at most 4 of them, each within 200 ms of the one before (the computer stops waiting after that, and reads every later stream as input):
  - `[0xFF,0x00]` + u32 LE, the screen;
  - `[0xFE,0x00]` + length u8 + UTF-8, its device id;
  - `[0xFC,0x00]` + length u8 + UTF-8, its iroh key;
  - `[0xFD,0x00]` + u8, its codec capabilities (bit 0, HEVC);
  - `[0xFB,0x00]` + u8, the session mode: `1` input only, `2` cast, `3` sound, `4` the phone's sound alone (none: a screen session). A sound session gets the computer's sound and no picture; the phone's sound alone sends a cast's sound streams and gets nothing but its ready stream; a computer that sends it a picture does not know the mode, and the client closes it.

  A cast then sends `[0xFA,…]` streams. Input travels as tagged packets, tags 0 to 15. The tables are in `docs/protocol.md` §3.2, from `core/src/streaming/{client,streamer,input_packet}.rs`.
- **From the computer:** each frame or audio packet goes on its own uni-stream, with an 18-byte header, little-endian:
  - sequence, u64;
  - kind, u8: `0` video, `1` audio, `2` ready (a session without a picture was admitted; a sound session's carries one byte: bit 7 set, bit 0 the computer's sound sent, bit 1 the phone's mic played; a screen session's, sent as it is admitted, one byte of what the computer takes: bit 7 set, bit 0 a pen, only with the Input permission), `3` keyframe wanted (a cast);
  - a keyframe flag, u8;
  - a timestamp in microseconds, u64.

  Video is H.264, or HEVC when both ends allow it. Sound is Opus at 48 kHz, stereo.
- **Gates:** a stream session is held to the same pairing, keys and permissions as the control channel. Without the Input permission it is forced to view only.

## 6. Conformance

- **`docs/protocol-vectors/control-v1.json`** holds the control channel's conversations, each from a new, unpaired connection:
  - TLS required;
  - the handshake and the identity's capability lists;
  - an unpaired request unanswered, told once that it is not paired;
  - a wrong PIN answered no;
  - a second identity not ending the connection;
  - a line that is not a packet skipped;
  - an over-long line closing the connection.
- **Each case's steps** send a line, or expect the next line (matched on type and the body fields given), silence, or the end.
- **How they are checked:** `server/src/service.rs` (`vector_tests`) replays every case against the real handler with every plugin the computer runs, in CI.
- **For another implementation:** a client can replay the same file against a computer, and a computer against its own handler.
- **Beside the file**, a test in the same module pairs a phone in its throwaway state and shows that a stranger's second identity naming it borrows nothing.
- **Not covered yet:** a paired phone's packets beyond that; the stream; the transfers; iroh.

## 7. Not in version 1

- Phone ↔ phone, and computer ↔ computer (ecosystem phase 2).
- KDE Connect's own pairing and packets beyond the ones listed (phase 3).
- Discovery beyond mDNS `_pocketdesk._tcp`.
