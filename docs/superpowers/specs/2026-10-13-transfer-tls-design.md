# File transfers inside TLS (mutual auth, phase C)

Date: 2026-10-13. Status: approved (autopilot). Follows phase A (`2026-10-13-device-keys-design.md`) and phase B (`2026-10-13-control-tls-design.md`).

## The problem

The stream and the control channel now run inside TLS, with both ends held to their keys. File transfers do not.

- **How a transfer runs today.** It goes over a fresh TCP port. The phone listens and sends `{port, key}` over the control channel; the desktop dials the control connection's address and presents the key.
- **What the key does.** It decides who may take or send the bytes.
- **What is open.** The bytes cross the network in plain text, and nothing proves which computer dialed in, or which phone answered.

This is the last plain-text channel, and the top limit in `SECURITY.md`.

## Design

### Roles

The roles stay as they are: the phone listens and the desktop dials.

| | Phone (TLS server) | Desktop (TLS client) |
|---|---|---|
| Presents | its certificate (the one its control channel proves) | its certificate (the one its control channel presents) |
| Accepts only | the desktop certificate the phone has pinned for the address it sent the request to | the certificate whose key the requesting control connection proved (`sender.peer_key()`) |

- **One-time pins, both ways.** Each side is configured with the single certificate it expects, so there is no trust on first use here.
  - The handshake signature is checked both ways.
  - ALPN is `linux-link-transfer/1`.
- **The rest is unchanged.** The one-time key is still presented first (inside TLS), followed by the bytes, the size header (downloads) and the status byte.
- **When there is nothing to pin:**
  - The phone refuses to start a transfer with a desktop whose address has no pin. After any control connection there is one.
  - The desktop refuses a transfer requested on a connection that proved no key.
- **No mixed versions.** Both ends ship together; a plain transfer is not accepted.

### Pieces

1. **core, `transport.rs`.**
   - `CertManager::transfer_server_config(desktop: [u8; 32])` and `transfer_client_config(phone: [u8; 32])`, built on an `OnlyCert` verifier: one hash, the signature checked.
   - Tests: a loopback handshake succeeds with the right pins and fails with a wrong one on either side.
2. **bridge.**
   - One `accept_transfer(listener, key, acceptor)` used by sending (`send_stream`) and by downloads (`await_and_receive`): accept, run TLS, present the key, and drop anything that fails.
   - The acceptor comes from the phone's certificate and its pin for the address.
3. **server.**
   - `receive_file` (the phone's share) and `send_reader` (a fetch) dial, then run TLS with `transfer_client_config(peer key)`.
   - The share and fetch handlers pass the sender's proved key; without one the request is refused.
4. **Docs.**
   - `SECURITY.md`: the transfer row, and the limit removed.
   - README, AGENTS, and an as-built section here.

## Device test (A71 ↔ Windows, LAN)

A share from the phone, a desktop send (an offer), and a browse download, each with MD5s matching. The server log shows the transfer's TLS handshake. A transfer with the pin removed on the phone is refused.

## As built (2026-10-13)

- **As designed, with these changes from the fresh review (0 Critical, 2 Important):**
  - **A computer saved by name** (`mypc.local`, a Tailscale name) had lost every transfer: the pin was looked up only for an IP. `peer_label_for_address` gives the control channel's label for any address, and both the transfer and the app's Remove use it (Remove had left named pins behind).
  - **The pin is looked up once the request is out,** since a connection made for that request may pin the desktop a moment before.
  - **A paired phone must have its key bound** (`pair::may_transfer`). One paired before device keys and not yet bound moves no files until it pairs again, because someone may be claiming its id. With pairing off, an unpaired device's own key is used.
  - **The desktop's certificate is read once** (it had been re-read from disk per transfer).
  - **Both ends require ALPN `linux-link-transfer/1`.**
  - **The phone checks incoming connections side by side**, up to 16. Checked one at a time, two silent connections had been enough to time the desktop out.
- **Ruled, not changed:**
  - An offer is spent when the key check refuses it (it is finished with the reason).
  - When the desktop's handshake fails, the phone waits out its 30 s window rather than hearing a reason.
- **Device-verified (A71 ↔ Windows, LAN):** desktop to phone (an offer) 3 MB and phone to desktop (a share) 2 MB, MD5s matching. A desktop or phone with the wrong certificate is covered by unit tests on both ends (removing the phone's pin on the device would only re-pin it on the next control connection).
