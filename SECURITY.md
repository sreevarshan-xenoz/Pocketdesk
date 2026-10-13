# Security

Pocketdesk lets a phone see and control a computer, so what it protects, and what it does not yet protect, matters more than its feature list. This page describes the design as it is in this tree, including its limits.

## Reporting a vulnerability

Please report security problems privately. Use GitHub's **Security → Report a vulnerability** on this repository, which opens a private advisory only the maintainer can see. Do not open a public issue. Include what you found, how to reproduce it, and what an attacker gains.

## The connections and what protects them

| Connection | Port | Encrypted | How the peer is identified |
|---|---|---|---|
| Control channel (pairing, clipboard, file browse, notifications, commands) | TCP 1716 | Yes (TLS 1.3) | Both ways by key, as on the stream: the phone pins the desktop's certificate, and must present its own, which the desktop holds the device to (device keys). A client that does not speak TLS is refused |
| Video, audio and input stream (LAN) | UDP 4716 (QUIC) | Yes (TLS 1.3) | Both ways by key: the phone pins the desktop's certificate (first use, or the QR code's fingerprint) and checks the handshake signature; the phone presents its own certificate, and the desktop holds the device to it (device keys) |
| Stream over the internet (WAN) | iroh (QUIC) | Yes | Both ends' iroh keys; the desktop holds the phone to the iroh key it registered on the LAN |
| File transfers | a fresh TCP port per file | **No** | A one-time 32-hex key per transfer; the desktop dials only the address the request came from |

Tailscale, when you use it, wraps all of these in WireGuard, which encrypts and authenticates the whole path between your devices.

## What is protected

- **Pairing:**
  - A 6-digit PIN is valid for 5 minutes and pairs one phone.
  - It is discarded after 5 wrong guesses, and the desktop shows at most one new PIN every 10 seconds.
  - It travels inside TLS. With the QR code, the phone checks the desktop's certificate before the first byte, so the PIN reaches only that computer.
  - With `pairing_required = true` (the default), an unpaired device may only pair, on the control channel, and is refused on the stream.
- **The desktop's identity:**
  - The phone pins the desktop's certificate, on the stream and the control channel alike, and checks that the desktop holds its key.
  - A pairing code can never replace a certificate the phone already trusts for an address.
- **The phone's identity (device keys):**
  - Pairing binds the phone to the certificate it proves on the connection that pairs. The first LAN stream it then opens registers the iroh key it names there for the WAN. A WAN stream never binds a key by itself.
  - From then on, a connection that claims to be that phone with any other key, or with none, gets nothing: the stream is refused, the control channel may only pair again, and the desk is notified.
  - Each control connection is known by the id it opened with, and held to its key on every packet: pairing, unpairing or Remove apply at once to connections already open.
  - Pairing again with the desktop's own PIN binds the phone's new key (a phone whose certificate changed recovers this way). `linux-link unpair` and the app's Remove clear the keys.
  - A guest PIN binds only an id that no phone holds. It can never take over, or extend, a paired phone's id.
- **Per-device permissions:**
  - Each paired phone can be limited to view-only, and denied the clipboard, files, sound, the microphone, power actions or commands.
  - Running commands is off until granted.
  - "Ask before each session" holds every session for that phone until someone at the desk allows it.
- **Files:**
  - Received files are confined to Downloads, never overwrite anything, are size-limited, and are marked as downloaded on Windows.
  - Browsing and downloading are confined to the home folder. Hidden files, the server's own keys and settings, network paths and device names are refused, and links that lead out of home are refused too.

## Known limits

These are real gaps in the current design. Plan around them.

1. **File transfers are not encrypted.** The one-time key decides who may take the bytes, but not who can read them on the way. Moving them inside TLS is the next security item.
2. **Without the QR code, the first contact with an address is trust on first use.** Someone who can intercept that very first connection could relay the pairing. Scanning the QR code closes this.
3. **A phone paired before device keys, that has not streamed on the LAN since, holds no key yet.** Until it does, the control channel accepts it by its id alone (the first LAN stream the desktop allows binds its key). Pairing it again binds the key at once.
4. **Someone who knows an unpaired phone's id can get in the way of its pairing**, by asking for PINs or spending the wrong guesses under that id. They cannot pair as it without the PIN.

## Using it safely today

- **Use Tailscale, or another VPN, anywhere you do not trust everyone on the network.** It closes the limits above for traffic between your own devices.
- **Keep `pairing_required = true`.**
- **On a shared network,** turn on "Ask before each session" for your phones, and grant each phone only what it needs.
- **Keep the control and stream ports closed to the internet.** The WAN path (iroh) does not need them open.
- **Pair by scanning the QR code** rather than typing an address. It pins the right computer before the first connection.
