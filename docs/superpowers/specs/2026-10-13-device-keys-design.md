# Device keys: a phone proves who it is — design

Status: decided 2026-10-13, branch `feat/device-keys`. Phase A of mutual authentication. This closes the first known limit in `SECURITY.md` for the stream; the control channel is phase B.

## Why

The desktop knows a paired phone only by the `deviceId` the phone announces. On the stream it arrives in-band (a `0xFE` uni stream or `IdentityPacketV2`); on the control channel it arrives in a plain-text identity packet. Anyone who has seen a paired phone's id can claim it and get that phone's session: the screen, input, and whatever else the owner allowed.

The pieces for a real proof already exist:
- **LAN.** The phone persists a P-256 certificate and key (`CertManager::load_or_create` in its cert dir) that it never presents.
- **WAN.** iroh authenticates both ends' ed25519 endpoint keys, but the phone dials with a fresh key each time and the server never reads the remote id.

## Decisions (made autonomously; each is cheap to revisit)

| # | Decision | Why |
|---|---|---|
| K1 | **The phone presents its certificate on LAN QUIC.** The phone's rustls client config sends its persisted certificate as a TLS client certificate. The server offers client authentication without requiring it. It accepts any self-signed certificate, because the binding below is what trusts it, but it verifies the handshake signature for real. | TLS proves possession of the key. Optional at the TLS layer, so the server can say *why* it refuses (K4) instead of failing the handshake. |
| K2 | **The phone keeps one iroh key.** It is a persistent `SecretKey` in the phone's cert dir, used for every WAN dial. The server reads the remote endpoint id from the iroh connection. | iroh already proves possession of that key; it only has to stop being thrown away. |
| K3 | **The proven key reaches the session gate.** `Connection` gains `peer_key() -> Option<PeerKey>`, where `PeerKey` is `Tls([u8; 32])` (the SHA-256 of the client certificate's DER) or `Iroh(String)` (the endpoint id). The gate's signature gains that argument, and so does v2's control gate. | One place decides, for every transport. |
| K4 | **Bound at first use after pairing.** `<state>/device_keys.json` maps `deviceId → {tls, iroh}`, written by the server under a lock file, separate from the trust store (whose writers rebuild it and would drop an unknown field). The rules:<br>1. A paired device whose slot for this transport is empty: the key it proves is recorded, and the session goes ahead.<br>2. A recorded key that matches: the session goes ahead.<br>3. One that differs, or no key on a transport where one is recorded: refused, logged, and the desktop is notified ("a device claimed to be <phone> without its key"). | Existing pairings carry over: each binds on its next connection. The window is the seconds between pairing and the first stream, and it is the same window pairing has today. A thief who loses the race is refused loudly, never silently. |
| K5 | **Pairing resets the binding.** A successful PIN pairing clears that device's keys, so a reinstalled phone (a new key) can pair again. Unpairing (`linux-link unpair`, the app's Remove) clears them too. | Without this, a reinstall would be locked out for good. |
| K6 | **The control channel is not covered here.** Phase B moves it, and pairing with it, inside TLS with the same certificate. Until then, privileged control packets still trust the announced id. | Encrypting TCP 1716 is a protocol upgrade of its own. Phase A already protects the screen, input and audio, which are what an impostor most wants. |

## Error handling

- **A refused session** ends with the reason the phone already shows for a refused session ("session refused"), and the server logs the device id and both key fingerprints.
- **The desktop notification** fires at most once a minute per device, so a retrying attacker cannot flood the desk.
- **A corrupt `device_keys.json`** refuses every session whose device has keys. An unreadable store could be hiding a binding, so it fails closed, as the trust store does.

## Testing

| Layer | Tests |
|---|---|
| core | The client-certificate verifier refuses a forged handshake signature. `peer_key` of a loopback QUIC connection is the client certificate's hash. The device key store: bind, match, mismatch, clear, and a corrupt file. |
| server | The gate with keys: first use binds; a match passes; a mismatch is refused; a missing key where one is bound is refused; pairing and unpairing clear. |
| bridge | The phone's client config presents its certificate. The iroh key persists across dials. |
| Device | The A71 streams after the update (LAN and WAN) and its keys are recorded. Then `device_keys.json` is edited to another fingerprint: refused, with a notification. Then re-pairing gets the session back. |

## As built (2026-10-13)

- **The certificate the phone already had.** The phone persists a P-256 certificate in its cert dir, and the rustls client presents it as a TLS client certificate.
- **The server's side.** `AnyClientCert` offers client authentication and verifies the handshake signature. The first commit on the branch (a3f3c16) fixed the same hole in the opposite direction: the phone's verifier had accepted any signature, so a pinned desktop certificate proved nothing.
- **Where the proof surfaces.** `Connection::peer_key()` yields `PeerKey::Tls(sha256)` from quinn's `peer_identity()` and `PeerKey::Iroh(id)` from iroh's `remote_id()`. The session gate gets a `SessionClaim`, and the v2 handshake is checked the same way.
- **The WAN key.** The phone names it on a LAN session in a fourth config stream, `[0xFC, 0x00, len, endpoint id]`. It keeps one iroh key, `iroh_device.key` in its cert dir, and dials with it.
- **Binding after the fresh review.**
  - `device_keys::check` only reads. The session gate decides the keys, then runs permissions and the owner's approval, then binds: `bind_first` under the trust store's lock file takes a TLS key only while the device has none, so the second of two racing first sessions is refused.
  - A WAN key never binds by first use; a WAN session from a device with no keys passes by id and records nothing. The WAN key is registered only from a session whose TLS key matched or was just bound.
  - v2 checks read-only and never binds.
  - Pairing clears keys only for the owner's PIN, never a scoped guest grant, and only after the trust store is written.
  - Writes take the lock, use their own temp file and sync before the rename. An unreadable store refuses with its own reason and notification; `clear` (Remove, unpair, re-pairing) renames it to `device_keys.json.unreadable` and starts afresh.
- **Device-verified (A71 ↔ Windows, before the review fixes):**
  - the first LAN session bound both keys;
  - a WAN session passed on the bound iroh key;
  - a changed stored fingerprint was refused, with the reason logged, the phone's "session turned down" card, and the desk notified;
  - restoring the binding brought the stream back.

## Out of scope

- Phase B: TLS on the control channel, and pairing inside it.
- Client certificates on a desktop-to-desktop path. There is none.
