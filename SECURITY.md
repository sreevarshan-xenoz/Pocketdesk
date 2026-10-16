# Security

Pocketdesk lets a phone see and control a computer, so what it protects, and what it does not yet protect, matters more than its feature list. This page describes the design as it is in this tree, including its limits.

**Two things to know before you pair.** Pair by scanning the desktop app's QR code: typing an address, or picking a computer found on the network, trusts whatever answers there first (limit 1 below). And the internet path is always on: relays see both ends' IP addresses, which endpoints talk and how much, and the Mainline DHT links the computer's WAN id to its home IP, and the phone's IP to the id it looks for (limit 4), unless `dht_publish = "off"`. None of it needs a port forward, except waking a sleeping computer from outside and running your own relay.

## Reporting a vulnerability

Please report security problems privately. Use GitHub's **Security → Report a vulnerability** on this repository, which opens a private advisory only the maintainer can see. Do not open a public issue. Include what you found, how to reproduce it, and what an attacker gains.

## The connections and what protects them

| Connection | Port | Encrypted | How the peer is identified |
|---|---|---|---|
| Control channel (pairing, clipboard, file browse, notifications, commands) | TCP 1716 | Yes (TLS 1.3) | Both ways by key, as on the stream: the phone pins the desktop's certificate, and must present its own, which the desktop holds the device to (device keys). A client that does not speak TLS is refused |
| Video, audio and input stream (LAN) | UDP 4716 (QUIC) | Yes (TLS 1.3) | Both ways by key: the phone pins the desktop's certificate (first use, or the QR code's fingerprint) and checks the handshake signature; the phone presents its own certificate, and the desktop holds the device to it (device keys) |
| Stream over the internet (WAN) | iroh (QUIC) | Yes | Both ends' iroh keys; the desktop holds the phone to the iroh key it registered on the LAN |
| Control channel over the internet, when TCP 1716 cannot be reached | iroh (QUIC) | Yes | Both ends' iroh keys. Only a paired phone whose iroh key was registered on the LAN; nothing pairs over it, and it carries no file transfers |
| File transfers | a fresh TCP port per file | Yes (TLS 1.3) | Each end accepts exactly one certificate: the phone, the desktop it pinned for the address it asked; the desktop, the key bound to the phone that asked. A one-time key per transfer is checked inside, and the desktop dials only the address the request came from |

Tailscale, when you use it, wraps all of these in WireGuard, which encrypts and authenticates the whole path between your devices.

## What is protected

- **Pairing:**
  - A 6-digit PIN is valid for 5 minutes and pairs one phone. A PIN that paired stays spent even if its file cannot be deleted, and a PIN file without its minting time is no PIN.
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
  - Pairing again with the desktop's own PIN binds the phone's new key (a phone whose certificate changed recovers this way). `pocketdesk unpair` and the app's Remove clear the keys.
  - A guest PIN binds only an id that no phone holds. It can never take over, or extend, a paired phone's id.
- **Per-device permissions:**
  - Each paired phone can be limited to view-only, and denied the clipboard, files, sound, the microphone, power actions or commands.
  - Running commands is off until granted.
  - "Ask before each session" holds every session for that phone until someone at the desk allows it.
  - The server re-reads them each second for running sessions: a phone that is no longer paired (unpaired from the CLI or the app, an expired guest grant), or whose Input, Sound or Microphone was turned off, loses its session within a second, whatever made the change.
  - Privacy mode belongs to the phone that turned it on: no other phone can refresh or end it, and it ends once that phone is no longer paired.
- **Abuse by a connection:**
  - A control packet is at most 4 MiB, on both control channels: a longer line ends the connection, before anything is buffered past that.
  - The control and stream ports hold at most 8 connections from one address and 128 in all; more are refused. The stream port counts an address only once it has answered a QUIC Retry, so a forged source address cannot use up a phone's share. The internet (WAN) listener holds at most 128 in all.
  - One phone runs at most two file transfers at a time, within the desktop's four.
  - A device id must be printable (no control or invisible formatting characters) and at most 128 bytes, on the control channel, the stream and the v2 handshake, where the device name is held to the same rule.
  - A folder listing fits in one packet: a huge folder is listed in part, and the phone says so.
  - Refusals are logged: the first on each connection at the default level.
- **Files:**
  - Transfers run inside TLS, pinned both ways: no other device can read the bytes, send them, or take them.
  - A paired phone moves files only once its key is bound (pairing binds it).
  - Received files are confined to Downloads, never overwrite anything, are size-limited, and are marked as downloaded on Windows.
  - Browsing and downloading are confined to the home folder. Hidden files, the server's own keys and settings, network paths and device names are refused, and links that lead out of home are refused too.

## Known limits

These are real gaps in the current design. Plan around them.

1. **Without the QR code, the first contact with an address is trust on first use.** Someone who can intercept that very first connection could relay the pairing. Scanning the QR code closes this.
2. **A phone paired before device keys, that has not streamed on the LAN since, holds no key yet.** Until it does, the control channel accepts it by its id alone, and it moves no files. The first LAN stream the desktop allows binds its key, and pairing it again binds it at once.
3. **Someone who knows an unpaired phone's id can get in the way of its pairing**, by asking for PINs or spending the wrong guesses under that id. They cannot pair as it without the PIN.

4. **The Mainline DHT links the desktop's WAN id to its home IP** (roadmap N3), unless `dht_publish = "off"`.
   - The record is public to anyone who knows the id. By default it holds the relay URLs, which name your own relay when you run one. With `"addresses"` it also holds your home's public IP addresses.
   - In every mode, the DHT nodes that store the record see the home IP it was published from, hourly, so the id can be followed to the home over time.
   - A phone's WAN dial asks the DHT for the id, which shows strangers' nodes the phone's IP and the id it is looking for.
   - The id is in the pairing QR code, on each paired phone and on every relay the desktop uses. A forged record gets nowhere: it is signed, and the phone checks the desktop's key again when it connects.
5. **The connection limits are shared.** Phones behind one public address (carrier NAT, or a forwarded port) share that address's 8 connections, so one can use up the others'. Anyone who can reach the internet listener can hold its 128 until each handshake times out; the gate still refuses them.
6. **Privacy mode is held to its phone's id, as strongly as that id is held to a key** (limit 2). If that phone's Power permission is turned off, it cannot end the grab and no other phone may, so the local keyboard and mouse stay blocked until it expires (10 minutes).
7. **The desktop names itself to anyone who connects.** Any client that completes the TLS or QUIC handshake, with any certificate, gets the desktop's device id, name and plugin list before it is judged, and its certificate. Anyone on the network sees the same id and name in the mDNS announcement unless `lan_discovery = false`.
8. **The permissions limit what a phone may ask for, not what the desktop tells it.** A paired phone with every permission off still receives the desktop's notifications, its window titles (window list and workspace HUD), its battery, the wake details (the MAC) and the internet endpoint. Unpair a phone to stop them. The clipboard is the exception: the text you copy is pushed only to phones with Clipboard on, and with "Clipboard to phones" (`clipboard_push`) off, to none, though a phone in a session can still ask for it. A copy a password manager marks private (on Windows the formats that keep it out of clipboard history; on Wayland `x-kde-passwordManagerHint`) is never pushed, but other copied secrets reach every connected phone with Clipboard on. The phone's "Computer's clipboard in the background" decides only whether the text is put on its clipboard: the phone receives it while its link is up either way.

9. **A phone with Links on can open web pages in this computer's browser** (roadmap T8a), up to 5 a minute, without asking, including pages on your home network such as a router's. Only `http` and `https` links go either way, never one to the computer's own loopback (`localhost`, 127/8, `::1`, in any spelling a browser reads as one) and never one with a user name or password. Links is on for every paired phone and guest by default, as Input is, and a phone with Input can type the same link. Turn Links off per phone in the Devices tab, or set "Links from phones" (`open_links = false`) to have links shown instead of opened. On the phone, any app can share a link to "Open on computer"; it goes to the last computer used.

10. **A phone with "My media on the computer" on tells the computer what it plays** (roadmap T8d): the title, artist and app, kept owner-only in the state folder while it is connected. The computer, with that phone's Media permission on, can pause and skip it. Both sides must agree: the phone's setting (off by default) and the desktop's Media switch.

11. **With "Names from my contacts" on, the phone tells its computer who texts it** (roadmap T8e): the sender's name from the phone's contacts goes with each forwarded text and is kept owner-only in `sms.json` (turning `sms_names` off forgets the kept names at the next start). A name says what the phone's contacts call that number; a text's sender number can be spoofed, so a name is not proof of who sent it.

12. **With "Let the computer answer notifications" on, the computer can act as you in the phone's apps** (roadmap T10c): it can press a notification's buttons (up to three, by title, never one Android runs only once the phone is unlocked) and reply to a chat, from the desktop app's Notifications tab, at most 10 a minute. Only a notification sent in full (not one whose content the lock screen hides, nor one carrying a code), and only from the phone's own computer. The notifications that can be answered are kept owner-only in `phone_notifications.json` while the server runs. It is off by default, and the desktop's Notifications switch for the phone turns it off too.

13. **The folders a phone shares (roadmap T10g) can be read by its computer.** In the desktop app's Phone tab, or by anything running as you that writes its requests, the computer lists them and copies any file in them, hidden ones (a leading dot) left out. Only folders the phone's owner picked in Settings, "Folders the computer may open", and only from that phone's own computer, with its Files permission on. Removing a folder there gives its grant back.

14. **With "New screenshots to the computer" on (roadmap T10h), every screenshot taken on the phone goes to its computer**, whatever it shows: onto the computer's clipboard and into its Downloads. On the computer it is then in the clipboard's history (Windows' Win+V, and its cloud clipboard if that is on) and in Downloads. Only to the phone's own computer, only images in a folder named `Screenshots` (an app that drops other images there sends them too), at most 10 minutes old; the computer's Clipboard and Files permissions gate each half. It is off by default.

15. **A phone's cast shows everything on its screen on the computer** (roadmap T10k), notifications and codes included, to whoever sees the computer's screen, while the phone's owner casts it. Android asks for consent each time, and its status bar shows that the screen is shared. The computer hands the picture from the server to the desktop app over loopback TCP, behind a token only its owner can read (`cast.json`); any program running as that owner can read the token, and so watch the cast. The Cast permission ("Show its screen here", on by default) refuses it per phone.

16. **The event log and hooks keep a record of what happens, and run what you tell them** (ecosystem phase 1a). `events.jsonl` in the state folder (owner-only, 1 MiB and one older file kept) lists sessions, phones linking, batteries, and the notifications, texts, files and links that arrive. By default it records only who, when and how much: the app a notification came from, a file's size, a link's host. With "details in events" on (`events_detail = "full"`) it also keeps their content (notification and text bodies, senders, file names, whole links), on disk, until the log rotates. A hook in `[hooks]` runs as you, with the event on stdin and in `POCKETDESK_EVENT`, through the shell: treat the event's fields as untrusted text (a phone chooses a notification's text and a file's name), and never paste them into a command line unquoted. Turning "Event log" off writes nothing; hooks still run.

[docs/protocol.md](docs/protocol.md) lists every port, packet, file and outside party, and what each one sees.

## Using it safely today

- **Use Tailscale, or another VPN, anywhere you do not trust everyone on the network.** It closes the limits above for traffic between your own devices.
- **Keep `pairing_required = true`.**
- **On a shared network,** turn on "Ask before each session" for your phones, and grant each phone only what it needs.
- **Keep the control and stream ports closed to the internet.** The WAN path (iroh) does not need them open.
- **Pair by scanning the QR code** rather than typing an address. It pins the right computer before the first connection.
