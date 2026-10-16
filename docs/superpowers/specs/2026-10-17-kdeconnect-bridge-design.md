# Working with KDE Connect devices (3a–3c)

Date: 2026-10-17. Plan: `docs/research/2026-10-16-next-phase.md` §2.2 (the protocol's facts, with sources) and §3a–3c. Status: 3a built (see "As built" at the end); 3b and 3c to come.

## Goal

A device that runs KDE Connect (the KDE Connect app on Android or iOS, GSConnect, Valent) can pair with a Pocketdesk computer and use what both understand: ping, the clipboard, the battery, find my device, and sending the computer a file. Pocketdesk stays its own server and apps; this is a bridge, never a dependency.

## Owner's decisions (2026-10-16)

- **On by default**, with "Work with KDE Connect devices" in the desktop app's Settings (`kdeconnect_compat`, `config.toml`) to turn it off. Off, a connection that opens with `{` gets today's refusal, and nothing is broadcast.
- KDE Connect may be installed on the A71 for the device checks, and removed after.
- The phone app has no part in 3a–3c (they are the computer's side). A Pocketdesk phone working with a KDE Connect desktop would be its own item, with its own switch on the phone.

## Rules

- **Clean room.** KDE Connect is GPL. The facts below are behaviour, from its sources and advisories (the plan's §2.2 cites them); no code is taken.
- **Its own identity.** A KDE Connect device sees the computer under a separate id and certificate, never Pocketdesk's: KDE requires the certificate's subject name to be the device id, and Pocketdesk's certificate is pinned by phones already.
- **Its own trust.** KDE devices are paired, stored and permitted apart from Pocketdesk phones (`kdeconnect_devices.json`), with narrower defaults. Nothing a KDE device does changes a Pocketdesk phone's standing.
- **TLS 1.2 only here.** KDE Connect Android caps TLS at 1.2. The compat configs allow 1.2 and 1.3; the Pocketdesk road stays 1.3 only, with a test that holds it.

## The protocol, as far as 3a–3c need it (v8 only)

1. **Discovery.** A device broadcasts its identity packet on UDP 1716: `{"id": <ms>, "type": "kdeconnect.identity", "body": {deviceId, deviceName, deviceType, protocolVersion: 8, incomingCapabilities, outgoingCapabilities, tcpPort}}`, one line. KDE Connect Android broadcasts only on networks it trusts.
2. **The roles are reversed.** The side that *received* the broadcast opens TCP to the sender's `tcpPort` (1716–1764) and writes a plaintext connection packet (an identity with `deviceId`, `deviceName`, `protocolVersion`, `targetDeviceId`, `targetProtocolVersion`). Then the **TCP client runs TLS as the server**, and the TCP server runs TLS as the client.
3. **Inside TLS**, each side writes its full identity. The second identity's `deviceId` and `protocolVersion` must equal the plaintext one's, and the id must equal the peer certificate's subject name (CVE-2025-66270, where no implementation checked).
4. **Limits.** A line is at most 8192 bytes; each step has 1000 ms; at most 42 links from unpaired devices.
5. **Ids** match `^[a-zA-Z0-9_-]{32,38}$`; names are filtered of `"',;:.!?()[]<>` and cut to 32 characters.
6. **Pairing:** `kdeconnect.pair {pair: true, timestamp}`; one without a timestamp is ignored, and more than 1800 s of clock skew is refused. Both screens show a verification code: the first 8 hex characters (upper case) of SHA-256 over the larger public key's DER, then the smaller's, then the timestamp in decimal. `{pair: false}` unpairs.
7. **Payloads** (a file): the sender listens on 1739–1764 and runs TLS as the server; the receiver connects, runs TLS as the client and reads exactly `payloadSize` bytes. No key, no resume.

## 3a: the identity, discovery and the handshake

- **The identity:** `<state>/certs/kdeconnect/` holds a key (owner-only) and a certificate whose subject is CN = the compat id, O = KDE, OU = Kde connect (ECDSA P-256, self-signed, ten years), made on first use. The id is a UUID with `_` for `-`.
- **Broadcasting:** our identity (`deviceType: "desktop"`, `tcpPort` = the control port, the capabilities 3a–3c answer) to 255.255.255.255:1716 and each interface's broadcast address, while the desktop app's "Find KDE Connect devices" is open (a request in `kdeconnect_out/`, renewed every 2 s while it is) and once after a network change. KDE Connect Android dials in on TCP, which `install`'s firewall rule already allows for `pocketdesk.exe` on Private networks.
- **Accepting (the device dials in):** a `{` first byte on the control port, with the switch on, goes to compat (`server/src/kdeconnect_compat/`). It reads the connection packet (at most 8192 bytes, within 1 s) and holds it to the rules: `type` is `kdeconnect.identity`, the id matches, `protocolVersion` is 8 or more, `targetDeviceId`, if given, is our compat id. Then it runs **client** TLS on the accepted socket with the compat certificate, takes any server certificate whose subject name is that id (and, once paired, only the pinned one), writes our full identity and reads theirs within 1 s, held to the CVE rule. A connection that fails any of it is closed with a log line, and nothing is recorded.
- **Dialing a broadcaster** (a device that broadcast and expects us to dial): when UDP 1716 can be bound exclusively (on Linux, `kdeconnectd` holds it with address sharing; then compat says so and only accepts). It dials `tcpPort` (1716–1764 only) and runs **server** TLS. This half may come in a later PR: on Windows the phone dials in.
- **What a link is in 3a:** a handshaken connection from a device. Unpaired, it may only pair (3b); everything else is ignored. The desktop app's new Settings section lists the devices seen ("KDE Connect devices": name, not paired / paired).
- **The 42-link cap** counts unpaired links; the 43rd is closed.

## 3b: pairing and ping

- `kdeconnect.pair {pair: true, timestamp}` from an unpaired device: the server holds it for the owner (the approvals pattern, 30 s): the desktop app shows the device's name and the verification code with Allow and Deny; the KDE device shows the same code. Allow pins its certificate hash in owner-only `kdeconnect_devices.json` and answers `{pair: true}`. A timestamp missing or off by more than 1800 s is refused; no answer is a deny.
- From the desktop app, "Pair" on a listed device sends the request, with the same prompt on the device.
- `{pair: false}` either way unpairs; Remove in the list does it from here.
- `kdeconnect.ping` both ways: a desktop notification, and "Ping" in the list.
- Defaults for a paired KDE device: Clipboard, Ping, Find my device, Battery. Files off until the owner turns it on.

## 3c: clipboard, battery, find my device, and receiving a share

- `kdeconnect.clipboard` and `.clipboard.connect` both ways through the existing plugin and `clipboard_push`, under the compat permissions (and never a pairing link, as for phones).
- `kdeconnect.battery` into `phone_status.json`, marked as a KDE device, in the Devices view.
- "Ring this device" (`kdeconnect.findmydevice`) from the desktop app.
- A share to the computer: `kdeconnect.share.request {filename, payloadSize, payloadTransferInfo: {port}}` is answered by dialing the device's address at that port (1739–1764 only), client TLS with the compat certificate against the device's pinned one, reading exactly `payloadSize` bytes (at most 8 GiB) into Downloads through `share.rs`'s part and place, with Mark of the Web. No resume.
- Every Pocketdesk-only packet is refused from a KDE device (a test iterates `docs/packets.md`).
- Sending files to a KDE device (a listener on 1739–1764 and its firewall rule) is not in 3c.

## Tests

A Rust test client written from the facts above (not KDE's code), for each step:
- a v8 device that dials in after our broadcast, completing the handshake; a device whose plaintext and TLS identities differ (the CVE case) closed, nothing recorded; a certificate whose subject is not the id closed; protocol 7 closed; an oversized or slow line closed; the 43rd unpaired link closed;
- with the switch off, `{` gets `POCKETDESK_ERR 1 tls-required` as now;
- TLS 1.2 refused on the Pocketdesk road (a regression guard);
- the verification code against a vector computed independently (Python over fixed test keys, kept as hex in the test); timestamp skew; the pairing states;
- for 3c, a fake KDE device for each plugin, the refusal sweep, and a share declared over 8 GiB refused.

## Device checks

On the A71 with KDE Connect from F-Droid, on a Wi-Fi it trusts, against this Windows box (checklist): "Refresh" lists "<host> (Pocketdesk)" while Find is open; pairing with the same code on both screens; pings both ways; the clipboard both ways; a 50 MB share with its MD5 matching; the battery; the phone ringing.

## Docs it touches

`docs/protocol.md` (UDP 1716 on demand, the compat certificate, TLS 1.2 on this path only, the files), SECURITY.md (a new limit: KDE devices, TLS 1.2, pairing by a verification code, narrower permissions), `docs/protocol-spec.md` §7, `docs/protocol-vectors/kdeconnect-v8.json` (replayed by a test), README, the desktop app's Settings.

## Not in this design

GSConnect, Valent, KDE Connect on a desktop, and iOS are not checkable here (no GNOME machine on the phone's network, no iPhone). Sending files to a KDE device, mDNS discovery (`_kdeconnect._udp`, off by default in KDE Connect Android since 1.34), and plugins beyond 3c.

## As built (3a)

- **Broadcasting is once per Find, not every 2 s while looking.** On the A71, KDE Connect dialed in again at every broadcast and dropped its link for the new one, so a minute of looking churned the link every 2 s. The identity now goes out when the desktop app asks (each press names a new time) and once after a network change; the network is looked at every 10 s (R6's idle cost), and `idle_cost.rs` measures the loop.
- **Only limited broadcast** (`255.255.255.255`), not each interface's directed broadcast; on Windows it leaves by the default interface.
- **Dialing a broadcaster is not built:** UDP 1716 is not listened on, so a device that opens KDE Connect after the Find minute is found only by pressing Find again.
- **The device's name** is the one its identity inside TLS gives (the A71's connection packet said its model, the identity "Galaxy A71").
- **Tests:** a fake v8 device over TLS 1.2 (the handshake, the CVE case, a certificate naming another id, a pin, an old or misaddressed identity, slow and long lines), the identity made once with its subject, the 42-link cap, the seen list and the Find request; the Pocketdesk road's TLS 1.2 refusal was already held by `the_control_channel_refuses_tls_1_2`.
- **Device-checked** (checklist §87): the A71's KDE Connect dialed in after a Find, the handshake held, and it listed this computer.
