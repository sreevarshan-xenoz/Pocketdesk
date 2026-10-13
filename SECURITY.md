# Security

Pocketdesk lets a phone see and control a computer, so what it protects, and what it does not yet protect, matters more than its feature list. This page describes the design as it is in this tree, including its limits.

## Reporting a vulnerability

Please report security problems privately. Use GitHub's **Security → Report a vulnerability** on this repository, which opens a private advisory only the maintainer can see. Do not open a public issue. Include what you found, how to reproduce it, and what an attacker gains.

## The connections and what protects them

| Connection | Port | Encrypted | How the peer is identified |
|---|---|---|---|
| Control channel (pairing, clipboard, file browse, notifications, commands) | TCP 1716 | **No** | The phone's `deviceId`, sent in plain text in its identity packet |
| Video, audio and input stream (LAN) | UDP 4716 (QUIC) | Yes (TLS 1.3) | The phone pins the desktop's certificate (first use, or the QR code's fingerprint); the phone itself is identified by the `deviceId` it sends inside the stream |
| Stream over the internet (WAN) | iroh (QUIC) | Yes | iroh node keys for the link; the phone again by its in-band `deviceId` |
| File transfers | a fresh TCP port per file | **No** | A one-time 32-hex key per transfer; the desktop dials only the address the request came from |

Tailscale, when you use it, wraps all of these in WireGuard, which encrypts and authenticates the whole path between your devices.

## What is protected

- **Pairing:**
  - A 6-digit PIN is valid for 5 minutes and pairs one phone.
  - It is discarded after 5 wrong guesses.
  - With `pairing_required = true` (the default), an unpaired device is refused on the control channel and on the stream.
- **The desktop's identity on the stream:**
  - The phone pins the desktop's certificate.
  - A pairing code can never replace a certificate the phone already trusts for an address.
- **Per-device permissions:**
  - Each paired phone can be limited to view-only, and denied the clipboard, files, sound, the microphone, power actions or commands.
  - Running commands is off until granted.
  - "Ask before each session" holds every session for that phone until someone at the desk allows it.
- **Files:**
  - Received files are confined to Downloads, never overwrite anything, are size-limited, and are marked as downloaded on Windows.
  - Browsing and downloading are confined to the home folder. Hidden files, the server's own keys and settings, network paths and device names are refused, and links that lead out of home are refused too.

## Known limits

These are real gaps in the current design. Plan around them.

1. **A paired phone's identity is not cryptographically verified.**
   - The desktop trusts the `deviceId` a connection announces. That id travels in plain text on the control channel.
   - Someone on the same network who has seen a paired phone's `deviceId` can claim it on the control channel and on the stream, and gets that phone's permissions.
   - The fix is mutual authentication: binding each phone to a key it proves when it connects. It is the top security item on the roadmap.
2. **The control channel is not encrypted.** Clipboard contents, notification text, file listings and the pairing PIN cross it in plain text.
3. **File transfers are not encrypted.** The one-time key decides who may take the bytes, but not who can read them on the way.

## Using it safely today

- **Use Tailscale, or another VPN, anywhere you do not trust everyone on the network.** It closes all three limits above for traffic between your own devices.
- **Keep `pairing_required = true`.**
- **On a shared network,** turn on "Ask before each session" for your phones, and grant each phone only what it needs.
- **Keep the control and stream ports closed to the internet.** The WAN path (iroh) does not need them open.
- **Pair by scanning the QR code** rather than typing an address. It pins the right computer before the first streaming connection.
