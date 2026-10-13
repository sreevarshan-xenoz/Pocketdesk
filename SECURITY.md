# Security

Pocketdesk lets a phone see and control a computer, so what it protects, and what it does not yet protect, matters more than its feature list. This page describes the design as it is in this tree, including its limits.

## Reporting a vulnerability

Please report security problems privately. Use GitHub's **Security → Report a vulnerability** on this repository, which opens a private advisory only the maintainer can see. Do not open a public issue. Include what you found, how to reproduce it, and what an attacker gains.

## The connections and what protects them

| Connection | Port | Encrypted | How the peer is identified |
|---|---|---|---|
| Control channel (pairing, clipboard, file browse, notifications, commands) | TCP 1716 | **No** | The phone's `deviceId`, sent in plain text in its identity packet |
| Video, audio and input stream (LAN) | UDP 4716 (QUIC) | Yes (TLS 1.3) | Both ways by key: the phone pins the desktop's certificate (first use, or the QR code's fingerprint) and checks the handshake signature; the phone presents its own certificate, and the desktop holds the device to it (device keys) |
| Stream over the internet (WAN) | iroh (QUIC) | Yes | Both ends' iroh keys; the desktop holds the phone to the iroh key it registered on the LAN |
| File transfers | a fresh TCP port per file | **No** | A one-time 32-hex key per transfer; the desktop dials only the address the request came from |

Tailscale, when you use it, wraps all of these in WireGuard, which encrypts and authenticates the whole path between your devices.

## What is protected

- **Pairing:**
  - A 6-digit PIN is valid for 5 minutes and pairs one phone.
  - It is discarded after 5 wrong guesses.
  - With `pairing_required = true` (the default), an unpaired device is refused on the control channel and on the stream.
- **The desktop's identity on the stream:**
  - The phone pins the desktop's certificate and checks that the desktop holds its key.
  - A pairing code can never replace a certificate the phone already trusts for an address.
- **The phone's identity on the stream (device keys):**
  - The first LAN stream after pairing that the desktop allows binds the phone to the certificate it proves, and registers the iroh key it names there for the WAN.
  - A WAN stream never binds a key by itself.
  - From then on, a stream that claims to be that phone with any other key, or with none, is refused, and the desk is notified.
  - Pairing again with the desktop's own PIN, `linux-link unpair` or the app's Remove clear a phone's keys, so a reinstalled phone pairs again. A guest PIN never clears another phone's keys.
- **Per-device permissions:**
  - Each paired phone can be limited to view-only, and denied the clipboard, files, sound, the microphone, power actions or commands.
  - Running commands is off until granted.
  - "Ask before each session" holds every session for that phone until someone at the desk allows it.
- **Files:**
  - Received files are confined to Downloads, never overwrite anything, are size-limited, and are marked as downloaded on Windows.
  - Browsing and downloading are confined to the home folder. Hidden files, the server's own keys and settings, network paths and device names are refused, and links that lead out of home are refused too.

## Known limits

These are real gaps in the current design. Plan around them.

1. **On the control channel, a paired phone's identity is not verified by a key.**
   - The stream holds each phone to its key, but the control channel still trusts the `deviceId` a connection announces. That id travels there in plain text.
   - So someone on the same network who has seen it can claim the phone on the control channel. That reaches the clipboard, file browsing and downloads, notifications and commands, within that phone's permissions. It does not reach the screen or the input.
   - The fix, moving the control channel and pairing inside TLS with the same keys, is the next security item on the roadmap.
   - Until a phone has streamed once on the LAN after pairing, its stream key is not yet bound. The first certificate it proves there is taken as its own, and until then a WAN stream is judged by its id alone.
2. **The control channel is not encrypted.** Clipboard contents, notification text, file listings and the pairing PIN cross it in plain text.
3. **File transfers are not encrypted.** The one-time key decides who may take the bytes, but not who can read them on the way.

## Using it safely today

- **Use Tailscale, or another VPN, anywhere you do not trust everyone on the network.** It closes the limits above for traffic between your own devices.
- **Keep `pairing_required = true`.**
- **On a shared network,** turn on "Ask before each session" for your phones, and grant each phone only what it needs.
- **Keep the control and stream ports closed to the internet.** The WAN path (iroh) does not need them open.
- **Pair by scanning the QR code** rather than typing an address. It pins the right computer before the first streaming connection.
