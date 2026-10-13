# Control channel inside TLS (mutual auth, phase B)

Date: 2026-10-13. Status: approved (autopilot). Follows `2026-10-13-device-keys-design.md` (phase A).

## The problem

The control channel (TCP 1716) carries pairing, the clipboard, file browsing and downloads, notifications and commands, all in plain text:

- **Anyone on the network can read it.** That includes clipboard contents, notification text, file listings and the pairing PIN.
- **A connection is trusted by the id it announces.** The server keys each connection by the peer's IP. The phone's `deviceId` is learned from its identity packet into a map that is never cleared, and a connection whose IP once paired is trusted. So anyone who has seen a paired phone's id can claim it. Anyone who later gets the phone's old IP inherits its trust without even claiming it.

Phase A held the stream to keys. This phase holds the control channel to the same keys.

## Design

### Wire

- **TLS first.** TCP 1716 now starts with a TLS 1.3 handshake (rustls, ring, ALPN `linux-link-control/1`). Inside it the protocol is unchanged: `LINUX_LINK_HELLO 1`, `LINUX_LINK_OK 1`, the desktop's identity, the phone's identity, then newline-delimited JSON packets.
- **The desktop's side.** It presents the same certificate as its QUIC endpoint and requires a client certificate. The handshake signature is checked both ways, as on the stream.
- **The phone's side.** It verifies the desktop with the same TOFU pins as the stream, keyed by the address it dials. A pin set from a QR code therefore protects the control channel, and pairing with it, from the first byte.
- **Plain clients are refused.**
  - The server peeks the first byte. A TLS handshake record starts with `0x16`.
  - Anything else gets `LINUX_LINK_ERR 1 tls-required\n` and the connection is closed.
  - Both ends ship together, as before (no compat shim). An old app gets a clear error rather than a silent downgrade.
- **Discovery is unaffected.** The phone's network-discovery probe is a bare TCP connect, so TLS does not change it.

### Who a connection is

- **The identity is part of the handshake.** The server reads the phone's identity packet within 5 s of sending its own. A connection that sends none is closed.
- **The connection's key is the device id, not the IP.** `TcpDeviceSender::device_id`, the push registry, PINs, permissions and the pairing gate all use it. This is what v2 already does.
  - IP-keyed trust goes away: a new connection from a paired phone's old IP gets nothing.
  - The registry's "newest connection per device" now really means per device.
- **The id is held to the key.** The server checks the claimed `deviceId` against the TLS client certificate with `device_keys::check(.., Slot::Tls, Some(fingerprint))`:

  | The device is | Its keys | Outcome |
  |---|---|---|
  | not paired | anything | accepted; pairing will bind the key |
  | paired | match | accepted |
  | paired | none yet (paired before phase A, never streamed on the LAN since) | accepted by id, nothing bound. The next allowed LAN stream binds, after the gate, exactly as in phase A. The control channel never binds by first use, since it has no approval step. |
  | paired | a different key, or an unreadable store | refused: the connection is closed, the reason is logged with both keys, and the desk is notified (`report_refusal`, at most once a minute per device) |

- **The sender carries the proven key.** `DeviceSender` gains `peer_key() -> Option<PeerKey>`. The TCP sender carries the TLS key, and the v2 sender carries its QUIC key.

### Pairing binds the key

The PIN now travels inside TLS, so the connection that pairs proves a key, and pairing binds it. A newly paired phone is never unbound.

- **The owner's PIN:**
  - After the trust write, the device's keys are replaced with `{tls: this connection's key}`.
  - The WAN key is dropped and re-registered by the next LAN stream.
  - This replaces phase A's `clear`.
- **A guest grant:**
  - `bind_first` runs before the trust write.
  - If the id already has a different key, the pairing is refused (`pair:false`, logged). A guest can never claim, or extend, another phone's id.
- **A pairing connection without a proven key** (only possible over v2 without a client certificate): the owner's PIN clears the keys as before, and a guest grant binds nothing.

### Unchanged, and still open

- **File transfers stay plain TCP** with their one-time key. They are encrypted only under a VPN. That is the next item (phase C).
- **v2 control over QUIC is untouched.** It already rides TLS, and the app does not use it.
- **Without a QR code, the phone's first contact with an address is trust on first use.** A man in the middle at that moment could relay the pairing. Pairing by QR code closes this, and `SECURITY.md` says so.

## Pieces

1. **core, `transport.rs`.**
   - Add `CertManager::control_server_config()` and `control_client_config()`, each an `Arc` of the rustls config with the control ALPN.
   - The server config requires client auth: `AnyClientCert { mandatory }`.
   - Tests: a loopback TLS handshake over TCP in which each end sees the other's key. A client with no certificate is refused by the server.
2. **core, the protocol connection.**
   - Add `tokio-rustls` (no default features; `ring`).
   - `ConnectionManager::connect` takes the client config, dials TLS with the IP as the server name, and returns a `ControlStream`: `TlsStream<TcpStream>`, with a `peer_addr` helper.
   - The handshake test and the integration mock server move to TLS.
3. **server, the connection handler.**
   - Peek the first byte; plain gets the `tls-required` error.
   - Accept TLS and take the peer key.
   - Run HELLO/OK and the identity exchange, then the identity decision. Its pure core is `pair::decide_control_identity_in(dir, id, key, paired)`, with unit tests for each table row.
   - Build the sender keyed by the device id, with peer address and peer key.
   - The `kdeconnect.identity` packet the plugin sees is the one the handshake read; it is dispatched once so `PHONE_IDS` stays consistent.
   - The CLI `linux-link connect` uses the server's own certificate as its client certificate.
4. **server, pairing.**
   - `handle_pair` uses `sender.peer_key()` to bind as above, via a new `device_keys::replace(dir, id, tls_key)`.
   - Tests: the owner's PIN replaces another key, a guest grant binds a free id, and a guest grant for an id with a different key is refused.
5. **bridge.**
   - The control writer and readers become generic halves (`tokio::io::split` of the TLS stream) in `CONTROL_WRITER`, `oneshot_request`, `push_packet` and `download.rs`.
   - Every connect passes the client config from `CERT_DIR`. Without a cert dir, there is no control channel.
   - `push_packet`'s fallback must check `CONTROL_PEER`, not reuse any live writer.
6. **Docs.** `SECURITY.md` (the table and the limits), the README summary, AGENTS status, and an as-built section here.

## Device test (A71 ↔ Windows)

1. A LAN session works: control (battery HUD, clipboard, file browse) and video.
2. The server log shows the connection keyed by the device id over TLS.
3. A plain `HELLO` (from a script) gets `tls-required`.
4. Change the stored TLS key: the control connection is refused, logged and notified, and the phone shows its error.
5. Restore the key. Pair again (owner PIN through "Show PIN on desktop"): the keys are replaced with the phone's own.

## As built (2026-10-13)

- **Wire.** As designed. A plain client gets `LINUX_LINK_ERR 1 tls-required`. Its bytes are read and the socket half-closed first, because on Windows the reset that otherwise follows discarded the answer before the client read it. The two handshake lines (HELLO, the identity) are capped at 64 KiB.
- **Changed after the fresh review (1 Critical, 4 Important), one commit each:**
  - **v2 could re-point a TCP connection** (Critical). The plugin's id maps (`PHONE_IDS`, `TRUSTED_CONNECTIONS`) let any identity packet, including a v2 session's, decide which phone another connection counted as. They are gone.
    - A connection is trusted, permitted, offered files and paired as its own id only.
    - An identity naming another device changes nothing.
  - **Standing, per packet.** A connection admitted while a phone was unpaired would have gained its trust when the owner paired the real phone. Each packet now re-reads the connection's `Standing`:
    - `Paired`: paired, and it proved the bound key;
    - `Unpaired`;
    - `WrongKey`: paired, but not with this key.

    Pushes go only to connections whose key holds, chosen after that filter.
  - **The recovery the table above lacked.** A paired id presenting another key is not closed. It is held to pairing only, so the owner's PIN binds its new key (a phone whose certificate changed recovers that way). The table's "refused" row now reads "may only pair again".
  - **The owner's pairing fails closed.** It binds the key before trusting anything, and answers `pair:false` if it cannot.
  - **The phone's certificate pins** are merged into the file under a lock and replaced atomically.
    - A first use asks the file before accepting, and a pin set meanwhile wins.
    - A pin file that will not parse is an error, not an empty list.
  - **The phone's identity** is created once, under a lock, however many first loads run at once. A test reproduced the race on every run.
- **Minors also fixed:**
  - PIN minting is paced to one every 10 s, and expired PINs are dropped.
  - A phone leaving without a TLS `close_notify` is an ordinary disconnect.
  - `push_packet` reuses the live connection only to the same desktop.
- **Ruled, not changed:**
  - Someone who knows an unpaired phone's id can still get in the way of its pairing (PIN requests, the 5 guesses), though not pair as it.
  - The handler tests read the real state directory, read-only, under an id that cannot be paired.
- **Device-verified (A71 ↔ Windows, LAN):**
  - a plain HELLO is answered `tls-required` and closed;
  - a paired session runs control over TLS (keyed by the device id, queries answered) and video;
  - a tampered stored key refuses the stream, holds the control connections to pairing only, and shows the phone's "Pair again";
  - Pair again → Show PIN on desktop → the PIN binds the phone's real key, and that connection is seeded at once;
  - the next LAN stream registers the WAN key, and the store ends exactly as it began.
