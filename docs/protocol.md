# What Pocketdesk sends, and to whom

Pocketdesk is a desktop server (`pocketdesk`, Windows and Linux) and an Android app that sees and controls that desktop. On the local network the phone talks to the desktop over two ports: TCP 1716, a TLS 1.3 control channel for pairing, clipboard, files, notifications and commands, and UDP 4716, QUIC for video, sound and input. File and clipboard-image bytes take a separate TLS connection that the desktop opens to the phone for each transfer. Away from home the same video, sound and input session runs over iroh (QUIC with hole punching). iroh uses either n0's public relays or your own `pocketdesk relay`, and it can find the desktop through a record on the BitTorrent Mainline DHT. Both ends are identified by keys, not addresses. The phone pins the desktop's certificate on first use, or from the QR code's fingerprint. The desktop binds each paired phone to the certificate and iroh key that phone has proved. Nothing is sent to a Pocketdesk server, because no such server exists. No update check or remote telemetry was found in the code. The traffic that leaves your network goes to relays, to DHT nodes, to the bootstrap hosts that DHT nodes are found through, and to the DNS servers that resolve those names. The page states what each of these parties can see.

Read from the code as of 2026-10-14. Every row names the file it was read from, with paths relative to the repository root. Where the code did not answer a question, the page says so; those cases are collected in the last section. A change to what crosses the wire, or to who can see it, updates this page in the same commit.

---

## 1. Ports and listeners

### 1.1 Summary

| Who listens | Address and port | Transport and encryption | ALPN | Who is let in | Source |
|---|---|---|---|---|---|
| Desktop | `0.0.0.0:1716` TCP (`control_port`) | TLS 1.3 over TCP (1.2 is refused). Clients that send no TLS are refused with `POCKETDESK_ERR 1 tls-required`. A client certificate is required, and any certificate is accepted at the handshake. The device is then held to its key | `pocketdesk-control/1` | Anyone can connect. Until a device is paired it may send only pairing packets (§3.1) | `server/src/service.rs` (`bind_addr`, `handle_connection_with_kde`), `core/src/protocol/mod.rs`, `core/src/streaming/transport.rs` (`control_server_config`) |
| Desktop | `0.0.0.0:4716` UDP (`streaming_port`) | QUIC with TLS 1.3. A client certificate is offered, not required | `pocketdesk-v2` and `pocketdesk-stream`. The server branches on the ALPN the peer picks | QUIC Retry proves the source address first. A session gate then refuses unpaired devices, or keys that do not match the device | `server/src/service.rs` (`alpns`, `quic_endpoint.accept()`), `server/src/conn_budget.rs` (`proven`), `core/src/protocol/v2.rs` |
| Desktop | iroh endpoint, `0.0.0.0:0` UDP (port picked by the OS) | iroh QUIC (noq). Each end is identified by its iroh key | `pocketdesk-stream` (`StreamTransportConfig::default().alpn`) | Any iroh key can dial, up to 128 at once. The session gate holds the phone to the iroh key it registered on the LAN | `server/src/iroh_endpoint.rs` (`spawn_wan_endpoint`), `core/src/streaming/transport.rs` (default ALPN) |
| Desktop | mDNS, UDP 5353 multicast (mdns-sd) | Plain mDNS | — | Anyone on the link | `core/src/tailscale/lan.rs`, `server/src/service.rs` |
| Desktop | Mainline DHT node, UDP port picked by the OS (never 6881) | Mainline DHT (plain UDP, signed records) | — | The public DHT | `core/src/streaming/dht.rs` (`dht_builder`), `server/src/iroh_endpoint.rs` (`spawn_dht`) |
| Phone | `0.0.0.0:<random>` TCP, one per transfer, open only while that transfer is waiting | TLS 1.3. The phone's server accepts exactly one client certificate: the one pinned for the desktop it asked | `pocketdesk-transfer/2` | Only that desktop, which must also present the one-time key | `android/bridge/src/api.rs` (`send_clipboard_image`, share), `android/bridge/src/download.rs`, `core/src/streaming/transport.rs` (`transfer_server_config`) |
| Phone | iroh endpoint, `0.0.0.0:0` UDP, one per WAN dial | iroh QUIC | `pocketdesk-stream` | — (outbound dial) | `core/src/streaming/iroh_connection.rs` (`IrohDial::dial`) |
| Relay host | `0.0.0.0:3340` TCP plain HTTP by default (`--port`), or `:80` + `:443` with `--hostname` (HTTPS, Let's Encrypt) | iroh relay protocol. The relay forwards traffic that stays end-to-end encrypted between the two endpoints | — | Only listed endpoint ids. With `--allow-paired`, this desktop and the WAN keys of its currently paired phones, re-read on every connection. With `--open`, anyone. The relay refuses to start with none of these | `server/src/relay.rs`, `server/src/cli.rs` (`Relay`) |

Neither the phone nor the desktop app announces itself on mDNS. The phone only browses, and only while Home is open (`android/app/.../discovery/LanDiscovery.kt`).

### 1.2 The control channel (TCP 1716), step by step

From `server/src/service.rs` (`handle_connection_with_kde`):

1. The desktop peeks at the first byte within 8 s. If it is not a TLS handshake record (`0x16`), the desktop answers `POCKETDESK_ERR 1 tls-required` and closes the connection.
2. The TLS handshake must finish within 8 s. The client must present a certificate (`AnyClientCert { mandatory: true }`). Its SHA-256 becomes the connection's `peer_key`.
3. The client sends `POCKETDESK_HELLO 1` and the desktop answers `POCKETDESK_OK 1`. Any other line gets `POCKETDESK_ERR 1`. A handshake line is at most 64 KiB (`MAX_HANDSHAKE_LINE`).
4. **The desktop sends its own `kdeconnect.identity` before the client has identified itself.** It contains `deviceId`, `deviceName` (the host name), `deviceType: "desktop"`, `protocolVersion` and the plugin capability lists (`core/src/protocol/kdeconnect.rs`, `as_identity_packet`). Any TLS client with any certificate therefore learns the desktop's id and name.
5. The client sends its `kdeconnect.identity` within 5 s. Its `deviceId` must pass `valid_device_id` (§2.4). The connection is known by that id from then on. A second identity on the same connection is ignored.
6. Packets follow as JSON lines, each at most 4 MiB (`MAX_PACKET_LINE`). Every packet is checked against two things. First, the device's standing (`pair::standing`: paired with this key, unpaired, or wrong key), re-read for each packet. Second, the per-device permission (`pair::permits` → `capability_for`).
7. A connection that may receive pushes (paired with its key, or any connection when `pairing_required = false`) is seeded with the window and workspace HUD state, the WAN endpoint (`kdeconnect.pocketdesk.endpoint`) and `kdeconnect.pocketdesk.wake_info` (`seed_desktop_state`, `server/src/state.rs` `may_receive_pushes`).

TCP keepalive is turned on for accepted connections (`core/src/protocol/connection.rs` `keep_alive`, called in `service.rs`).

### 1.3 The QUIC port (UDP 4716)

- **Address check.** A QUIC attempt is counted only after it answers a Retry (`conn_budget::proven`). After that, the per-address and total budgets apply (§6).
- **`pocketdesk-stream` (v1 stream).** This ALPN runs one `StreamingServer` session: video and sound to the phone, input and mic from the phone. The session gate (`plugins::pair::session_gate`) judges the in-band device id against the TLS client key. Sources: `server/src/service.rs`, `core/src/streaming/streamer.rs`.
- **`pocketdesk-v2` (multiplexer).**
  - A 4-byte length-prefixed JSON identity exchange runs on stream 0 (`IdentityPacketV2`: `device_id`, `device_name`, `min_version`, `max_version`, `capabilities`). The desktop writes its identity at the same time as it reads the peer's, so the desktop's id, name and plugin list go to any QUIC peer that completes the handshake.
  - The device key is checked once, at session start (`v2_multiplexer.rs`, `device_keys_refused`).
  - Control packets on stream 0 are then gated by `is_paired_device(id)` and `permits`.
  - Sources: `core/src/protocol/v2.rs`, `server/src/v2_multiplexer.rs`.
  - The Android UI does not call v2: no caller of `RustCore.connectV2` was found in `android/app/src/main`.
- **Connection settings.** Keepalive is 15 s and the idle timeout is 45 s. Address migration is on (quinn's default). Source: `core/src/streaming/transport.rs` (`configured_transport`).

### 1.4 File and clipboard-image transfers

The flow is the same for a phone-to-desktop share, a desktop-to-phone download or offer, and clipboard images in either direction:

1. **The phone listens.** It binds `0.0.0.0:0` TCP and sends a control packet with `{port, key}`. The key is a UUIDv4 in simple form: 32 hex characters, 122 random bits (`uuid::Uuid::new_v4().simple()`). Sources: `android/bridge/src/api.rs`, `android/bridge/src/download.rs`.
2. **The desktop dials back.** It dials the IP address the control connection came from, never an address the phone names, and never loopback or link-local addresses (`server/src/plugins/share.rs`, `dialable`, `sender.peer_addr()`). A request that arrives on a channel with no peer address (v2) is refused.
3. **TLS, pinned both ways.**
   - The desktop's client accepts only the phone certificate its control connection proved (`transfer_client_config(phone)`).
   - The phone's server accepts only the desktop certificate pinned for that address (`transfer_server_config(desktop)`).
   - The ALPN must be `pocketdesk-transfer/2` on both ends.
   - A phone whose key is not bound gets no transfer (`transfer_tls` → `pair::may_transfer`).
4. **Inside TLS** (`core/src/protocol/transfer.rs`). The desktop, which dials, sends the key first, ended by a newline. Then, whichever way the file goes:
   - the sender gives the size (a big-endian u64; for a share it must equal the control packet's);
   - the receiver answers how much of this file it already has (`offset`, u64) and the SHA-256 of those bytes;
   - the sender continues from there only if its own first `offset` bytes hash the same, else both start over;
   - the sender sends the rest, then the SHA-256 of the whole file;
   - the receiver keeps the file only if its own hash matches, and only once it is in place answers one status byte: `1` saved, `0` failed. After a save it sends TLS close_notify and reads what is still in flight before closing (within the deadline), so a reset cannot overtake the status byte.
   A share names its file with a `sourceId` (32 hex digits, the same on every attempt). The desktop keeps a broken share's bytes in `<state>/partials/` for `transfer_keep_hours` (default 24), at most 4 per phone and 16 GiB in all, and continues them when that phone retries the same file. Clipboard images never resume, but their hash is checked.
5. **Limits.** Each connect and each TLS handshake has 10 s, and the stream may sit idle 30 s. The minimum rate is 32 KiB/s, files are at most 8 GiB and clipboard images at most 32 MiB. Transfers take four slots on the desktop, two per phone (§6).

Text clipboard does not use this path. It rides the control channel (`core/src/protocol/clipboard.rs`).

### 1.5 Windows firewall rules made by `pocketdesk install`

| Rule | Scope | Source |
|---|---|---|
| TCP 1716 "control" and UDP 4716 "media", limited to `pocketdesk.exe` | Private and Domain profiles, `RemoteAddress LocalSubnet` | `server/src/install_windows.rs` (`PORTS`, `firewall_add_script`) |
| The same two ports again | Interface `Tailscale`, remote `100.64.0.0/10,fd7a:115c:a1e0::/48`, any profile, added only if that adapter exists | same |

Discovery is mDNS (UDP 5353), so no rule is made for UDP 1716. `install` removes the whole rule group first, so an older install's UDP 1716 rule goes the next time it runs. `install` makes no rule for UDP 5353 either; whether phones see the announcement then depends on the rules Windows already has (its own mDNS rule, or one it added when `pocketdesk.exe` first listened). Not confirmed on a fresh machine.

---

## 2. Identities and keys

### 2.1 Keys and identifiers

| Item | Holder | What it is | Stored at | Who sees it | Source |
|---|---|---|---|---|---|
| Desktop certificate | Desktop | Self-signed ECDSA P-256, SAN `pocketdesk.local` (rcgen). One certificate serves the control channel, QUIC and transfers | `<state>/certs/identity.der`; key in `<state>/certs/identity.key` (owner-only) | Anyone who opens a TLS or QUIC connection to 1716 or 4716 | `core/src/streaming/transport.rs` (`generate_identity`, `load_or_create`) |
| Desktop device id | Desktop | UUIDv4 | `<state>/device_id` | mDNS TXT `id`; the identity packet (any TLS client); the v2 handshake; the QR link `id` | `server/src/kde.rs` (`host_device_id`) |
| Desktop name | Desktop | Host name (`COMPUTERNAME`/`HOSTNAME`/`/proc/sys/kernel/hostname`, else `pocketdesk-host`) | — | mDNS instance, host and TXT `name`; identity packet; v2 handshake; QR `name` | `server/src/kde.rs` (`host_device_name`) |
| Desktop iroh key | Desktop | 32-byte iroh secret key. Its public half is the WAN endpoint id | `<config>/iroh_secret.key` (mode 0600, or an owner-only DACL on Windows) | Endpoint packet, QR `ep`, relays it uses, the DHT record key | `server/src/iroh_endpoint.rs` (`load_or_create_secret_key`, `key_path`) |
| Phone certificate | Phone | Same generator as the desktop's | app `files/pocketdesk/certs/identity.der` and `identity.key` | Every desktop the phone connects to (sent as the TLS client certificate) | `android/bridge/src/api.rs` (`set_data_dir`), `core/src/streaming/transport.rs` |
| Phone device id | Phone | UUIDv4 | app `files/pocketdesk/certs/device_id` | Desktops: control identity, stream marker `[0xFE,0x00]` | `android/bridge/src/api.rs` (lines near `new_id`) |
| Phone name | Phone | The fixed string `Pocketdesk Android Client`. The phone model is not sent | — | Desktops | `android/bridge/src/api.rs` |
| Phone iroh key | Phone | iroh secret key, used for every WAN dial | app `files/pocketdesk/certs/iroh_device.key` (owner-only write) | The desktop (registered on the LAN), relays, DHT nodes the phone queries | `android/bridge/src/api.rs` (`device_wan_key`), `core/src/streaming/iroh_connection.rs` |

### 2.2 How the phone trusts the desktop

- **Where pins live.** Pins sit in `known_peers.json` beside the certificate. Each one maps a label to the SHA-256 of a certificate. The label is the IP address the phone dialed, or the host name for a computer saved by name (`transport.rs`, `peer_label`, `TofuVerifier`).
- **First use.** An unknown label is pinned on the first connection. A mismatch afterwards refuses the connection.
- **QR fingerprint.** `pin_peer_certificate` pins the QR code's `fp` before the first connection. It refuses to replace a different pin that already exists for that address (`android/bridge/src/api.rs`).
- **Pin file damage.** A pin file that does not parse is an error, not an empty file (`read_pins`).
- **Signatures.** The handshake signature is verified with ring's algorithms (`verify_tls12_signature`/`verify_tls13_signature` in `transport.rs`).

### 2.3 How the desktop holds a phone to its keys (device keys)

All of this is from `core/src/local/device_keys.rs`. The store is `<state>/device_keys.json`.

- **What is stored.** Each device id maps to `{tls, iroh}`: the hex SHA-256 of its TLS client certificate, and its iroh endpoint id.
- **First use.** A TLS key binds on first use only to a device that has no key at all, and only after the whole session gate has allowed the session (`check`, then `bind_first`).
- **The WAN key.** It is never bound by first use. It is `register`ed from a LAN session that the device's TLS key already proves, through the `[0xFC,0x00]` stream marker (`core/src/streaming/client.rs`).
- **Clearing keys.** Pairing again with the owner's PIN replaces the TLS key (`replace`). Unpair and Remove clear the keys (`clear`).
- **Unreadable store.** An unreadable store refuses rather than forgets.

### 2.4 Device id rules

A device id must be 1 to 128 bytes. It may contain no control characters and no invisible formatting characters (U+061C, U+200B–U+200F, U+202A–U+202E, U+2060–U+2069, U+FEFF). Source: `core/src/protocol/kdeconnect.rs` (`valid_device_id`, `is_invisible_format`). The rule is applied on the control channel (`service.rs`), and in the v2 handshake to both the id and the name (`v2_multiplexer.rs`, `usable_identity`).

### 2.5 Pairing PIN

| Property | Value | Source |
|---|---|---|
| Format | 6 ASCII digits, uniform over 000000–999999, from the OS CSPRNG (`OsRng`) | `core/src/local/pin.rs` (`mint_value`, `is_valid`) |
| Where it comes from | (a) `pocketdesk pair` or the desktop app writes `<state>/pairing.pin` (owner-only) with the PIN, the minting time and an optional grant length. (b) When a phone asks and no file PIN is live, the server mints a PIN per connection and shows it in a desktop notification | `core/src/local/pin.rs`, `server/src/plugins/pair.rs` (`request_pin`, `show_pin_notification`) |
| Lifetime | 300 s. A minting time more than 60 s in the future is refused. A file without a minting time holds no PIN | `pin.rs` (`PIN_TTL`, `MAX_FUTURE_SECS`) |
| Wrong guesses | After 5 wrong guesses against the current PIN, it is discarded | `pair.rs` (`MAX_WRONG_GUESSES`) |
| Minting pace | At most one new PIN every 10 s. A request inside that window gets `pinSent` again and no new PIN | `pair.rs` (`MintPace::GAP`) |
| One use | A PIN pairs one phone. A guest grant (`pair --grant`) trusts the phone for that long only | `pair.rs` (`trust_device_with_ttl`), `SECURITY.md` |
| Where it travels | Inside the control channel's TLS | §1.2 |
| Where it is written down | The desktop notification. The server log gets it only when no notification can be shown (`"No desktop notification available; the pairing PIN is …"`) | `pair.rs` (`show_pin_notification`) |

### 2.6 The `pocketdesk://pair` link (QR code)

From `core/src/local/pair_link.rs`. Values are percent-encoded.

| Key | Holds | Required |
|---|---|---|
| `v` | Format version, `1` | yes |
| `name` | The desktop's name | — |
| `id` | The desktop's device id | — |
| `a` | Addresses, comma-separated, best first | yes, at least one |
| `cp`, `sp` | Control and stream ports (default when absent) | — |
| `pin` | The 6-digit PIN | yes |
| `fp` | Hex SHA-256 of the desktop's certificate | absent before the server has made one |
| `ep` | The iroh endpoint JSON (`endpointId`, `relayUrls`, `directAddrs`, `dht`), base64url | absent until the server has recorded it |

Anyone who sees the code while it is valid holds everything needed to pair within the PIN's 300 s. `directAddrs` can include the desktop's LAN and public addresses.

---

## 3. What each channel carries

### 3.1 Control channel packets

The permission column comes from `capability_for` in `core/src/protocol/permissions.rs`. "Always" means a paired device may always send the packet. Before pairing (with `pairing_required = true`), only `kdeconnect.pair` and `kdeconnect.pocketdesk.pair` are dispatched (`service.rs`). On v2, `kdeconnect.identity` is also allowed (`v2_multiplexer.rs`). Packets from the desktop to the phone are not gated by per-device permissions. They go only to devices that may receive pushes (`server/src/state.rs`, `may_receive_pushes`).

Defaults: every permission is on except Exec (`Capability::default_allowed`).

| Feature | Packet type | Direction | What it carries (in brief) | Permission | Source |
|---|---|---|---|---|---|
| Identity | `kdeconnect.identity` | both | Device id, name, type, capability lists | Always (handshake) | `core/src/protocol/kdeconnect.rs`, `server/src/plugins/pair.rs` |
| Pairing | `kdeconnect.pocketdesk.pair` | both | PIN request, `pairStatus` (`pinSent`, `pinReady`) | Always, also before pairing | `server/src/plugins/pair.rs` |
| Pairing | `kdeconnect.pair` | both | The PIN; reply `{pair, serverId}` | Always, also before pairing | same |
| Input (KDE style) | `kdeconnect.mousepad.request`, `kdeconnect.presenter` | phone → desktop | Pointer and keys | Input | `server/src/plugins/input.rs` |
| Input (KDE style) | `kdeconnect.mousepad.echo` | desktop → phone | Acknowledgement | — | same |
| Clipboard text | `kdeconnect.clipboard`, `kdeconnect.clipboard.connect` | both (the phone polls; the desktop replies with its clipboard text, or `{image:{id,size}}`) | Clipboard contents | Clipboard | `server/src/plugins/clipboard.rs` |
| Clipboard image | `kdeconnect.pocketdesk.clipboard_image` | phone → desktop | `{size, port, key}` (the desktop then dials back, §1.4) | Clipboard | `core/src/protocol/clipboard.rs` |
| Clipboard image | `kdeconnect.pocketdesk.clipboard_image_fetch` | phone → desktop | `{id, port, key}` | Clipboard | same |
| Notification reply | `kdeconnect.pocketdesk.notification_reply` | phone → desktop | Reply text for a desktop notification. Put on the desktop clipboard and appended to `replies.log` | Clipboard | `server/src/plugins/notification_reply.rs` |
| Files | `kdeconnect.share.request` | phone → desktop | File name, size, `{port, key}`, `sourceId` (the file's id, so a retry continues it); for a file of a folder also `folder`, `folderId` (one per send) and `path` (its names under the folder, checked one by one and refused if any could leave it: `core::protocol::relpath`). Each send's files go under one new `Downloads/<folder>` (`Name (1)` when taken). Or a URL (shown as a notification) | Files | `server/src/plugins/share.rs` |
| Files | `kdeconnect.filebrowse.request` / `.response` | phone → desktop / back | A folder listing under home | Files | `server/src/plugins/file_browse.rs` |
| Files | `kdeconnect.pocketdesk.file_fetch` | phone → desktop | `{path or offer, port, key}` | Files | `server/src/plugins/share.rs` |
| Files | `kdeconnect.pocketdesk.file_fetch_refused` | desktop → phone | `{key, reason}` | — | same |
| Files | `kdeconnect.pocketdesk.file_offer` | desktop → phone | `{id, name, size, source}`; `source` names the file so a retry continues it. The phone may fetch an offer up to 4 times within its 10 minutes, one attempt at a time | — | `server/src/offers.rs` |
| Files | `kdeconnect.pocketdesk.file_offer_declined` | phone → desktop | Offer id, reason | Files | same |
| Desktop sound control | `kdeconnect.pocketdesk.audio` | both | Volume, mute, output device | Audio | `server/src/plugins/audio_control.rs` |
| Power | `kdeconnect.pocketdesk.power` | phone → desktop | Lock, sleep, shut down | Power | `server/src/plugins/power.rs` |
| Privacy mode | `kdeconnect.pocketdesk.privacy` | both | Grab the desk's keyboard and mouse, show the shield | Power | `server/src/plugins/privacy.rs` |
| Commands | `kdeconnect.pocketdesk.exec` | both | A command to run (`sh` or `System32\cmd.exe`) and its result | Exec (off by default) | `server/src/plugins/exec.rs` |
| Phone notifications | `kdeconnect.notification` | phone → desktop | `{id, app, title, text, time, isClearable}`, or `{id, isCancel}` | Notifications | `server/src/plugins/notification.rs`, `android/app/.../notify/PhoneNotifications.kt` |
| Desktop notifications | `kdeconnect.notification` | desktop → phone | The desktop's own notifications `{id, app, title, text, ticker, urgency}`, except Pocketdesk's own | — (sent to every phone that may receive pushes) | `server/src/notification_monitor.rs`, `server/src/service.rs` |
| Notifications | `kdeconnect.notification.request` | phone → desktop | Request. The reply is an empty `kdeconnect.notification` | Always | `server/src/plugins/notification.rs` |
| Texts (SMS) | `kdeconnect.sms.messages` | phone → desktop | `{address, body, date, event}` per text | Texts | `server/src/plugins/sms.rs`, `android/app/.../notify/Texts.kt` |
| Texts (SMS) | `kdeconnect.sms.request` | desktop → phone | A reply to send: number and body | — | same |
| Texts (SMS) | `kdeconnect.pocketdesk.sms_sent` | phone → desktop | `{id, ok, error}` | Texts | same |
| Battery | `kdeconnect.battery.request` / `kdeconnect.battery` | phone → desktop / back | The desktop's charge, whether it is charging, `noBattery` | Always | `server/src/plugins/battery.rs` |
| Monitors | `kdeconnect.pocketdesk.monitors` | both | Monitor list | Always | `server/src/plugins/monitors.rs` |
| Windows | `kdeconnect.pocketdesk.windows` | both | Window list: address, **title**, class, pid, geometry | Always | `server/src/plugins/windows.rs`, `server/src/hyprland.rs` |
| Workspace HUD | `kdeconnect.pocketdesk.hyprland.state`, `.hyprland.event` | desktop → phone | Workspaces, windows (with titles) and geometry. Pushed at registration and every 4 s, and on events | — | `server/src/hypr_events.rs`, `server/src/service.rs` |
| Find my device | `kdeconnect.findmydevice` | phone → desktop | Ring a siren on the desktop | Always | `server/src/plugins/siren.rs` |
| Wake relay | `kdeconnect.pocketdesk.wol` | both | `{mac, broadcast?, name?}`. The desktop sends a magic packet to `broadcast:9` and replies with the outcome. The target must be the limited broadcast or inside one of the desktop's own IPv4 subnets | Power | `server/src/plugins/wake_relay.rs`, `core/src/local/wake.rs` (`may_relay_to`), `core/src/tailscale/wol.rs` |
| Wake info | `kdeconnect.pocketdesk.wake_info` | desktop → phone | `{mac, broadcast, wired, port}` for the adapter the phone reached, or `{}` when `wake_announce = false` | — | `server/src/wake_info.rs` |
| WAN identity | `kdeconnect.pocketdesk.endpoint` | desktop → phone | `{endpointId, relayUrls, directAddrs, dht}`. Pushed at registration and every 30 s | — | `server/src/iroh_endpoint.rs`, `server/src/service.rs` |
| Input control | `kdeconnect.pocketdesk.control` | desktop → phone | Which phone is driving | — | `server/src/driving.rs` |

Every `kdeconnect.pocketdesk.*` packet carries `"llVersion": 1` (`kdeconnect.rs`, `LL_EXT_VERSION`).

### 3.2 The stream (v1, on LAN QUIC or iroh)

**Setup markers.** The phone sends these on uni-streams (`core/src/streaming/client.rs`, `streamer.rs`):

| Marker | Contents | Purpose |
|---|---|---|
| `[0xFF,0x00]` + u32 LE | Monitor index | Which screen to stream |
| `[0xFE,0x00]` + len + UTF-8 | Phone device id | Which paired device this is (judged against the proved key) |
| `[0xFC,0x00]` + len + UTF-8 | Phone iroh endpoint id | Registers the WAN key from a LAN session |
| `[0xFD,0x00]` + u8 | Codec capability bits (bit 0 = HEVC) | Codec negotiation |

**Media from the desktop.** Each video frame or audio packet travels on its own uni-stream. It starts with an 18-byte header: sequence u64, kind (`0` video, `1` audio), keyframe flag and timestamp u64 (`core/src/streaming/transport.rs`, `PacketHeader`).

- Video is H.264, or HEVC when `allow_hevc` is set and the phone says it can decode it (`docs/capabilities.md`).
- Desktop sound is Opus at 48 kHz, stereo, 64 kbit/s. It is not sent when the Audio permission is off (`core/src/streaming/streamer.rs`).

**Input packets from the phone** (`core/src/streaming/input_packet.rs`). When the Input permission is off, the session is forced to view-only, so tags 0–6 are dropped. Mic frames are dropped when Mic is off (`streamer.rs`: `force_view_only`, `allow_mic`).

| Tag | Packet | Payload | Effect / gate |
|---|---|---|---|
| 0 | `MouseMove` | dx, dy i16 | Injected; Input |
| 1 | `MouseClick` | Button, pressed | Injected; Input |
| 2 | `MouseScroll` | Scroll amount | Injected; Input |
| 3 | `KeyEvent` | evdev code, pressed | Injected; Input |
| 4 | `Text` | UTF-8 string | Injected; Input |
| 5 | `Gamepad` | Button mask | Injected; Input |
| 6 | `MouseMoveAbs` | x, y normalized u16 | Injected; Input |
| 7 | `RequestKeyframe` | — | Control |
| 8 | `WindowCrop` | Rectangle and window address | Control |
| 9 | `ViewOnly` | Enabled flag | Control |
| 10 | `FullQuality` | Enabled flag (skip the relay bitrate floor) | Control |
| 11 | `Mic` | Enabled flag plus one 20 ms Opus frame (48 kHz mono) | Fed to a virtual microphone; Mic |
| 12 | `QualityPreset` | Preset id 0–3 | Control |
| 13 | `LinkFeedback` | The phone's RTT and lost packets | Measurement, stored in session records |
| 14 | `ClientSamples` | Kind (decode, render or e2e) plus u32 µs values | Measurement, stored in session records |

### 3.3 The v2 multiplexer

`ChannelKind` declares Control (0), Video (1), Audio (2), Input (3) and File (4) (`core/src/protocol/v2.rs`). In this tree the server runs the control channel (stream 0) through the same plugin registry and gates (`server/src/v2_multiplexer.rs`). The rest of the v2 channel handling was not traced. No caller in the Android UI uses v2 (§1.3).

---

## 4. What leaves the local network, and who sees it

### 4.1 mDNS announcement (local link only)

Made when `lan_discovery = true`, which is the default (`core/src/local/config.rs`; `server/src/service.rs`). The announcement is withdrawn on a graceful stop.

| Field | Value | Source |
|---|---|---|
| Service type | `_pocketdesk._tcp.local.` | `core/src/tailscale/lan.rs` (`SERVICE_TYPE`) |
| Instance name | The desktop's name | `register_service` |
| Host name | `<name>.local.` | same |
| Port | The control port (1716) | same |
| Addresses (A/AAAA) | The non-loopback, non-link-local adapter addresses (from `if-addrs`, or `hostname -I` on Linux), less the tailnet's (`100.64.0.0/10`, `fd7a:115c:a1e0::/48`) | `lan.rs` (`get_local_ips`, `announceable`) |
| TXT `name` | The desktop's name | `lan.rs` (`announcement_properties`) |
| TXT `version` | `1` | same |
| TXT `id` | The desktop's device id | same |
| TXT `stream` | The stream port (4716) | same |

The server also browses for other Pocketdesk servers every 30 s (`service.rs`, `lan_discovery.run`).

### 4.2 Mainline DHT

The DHT publication is controlled by `dht_publish` (default `relay`) in `core/src/local/config.rs` (`DhtPublish`). It is used in `server/src/iroh_endpoint.rs` and `core/src/streaming/dht.rs`.

| Mode | The signed record holds | Notes |
|---|---|---|
| `relay` (default) | Relay URLs that a phone anywhere could reach | Relays with a private IP, a single-label name or a LAN suffix (`.lan`, `.local`, `.home`, `.home.arpa`, `.internal`, `.localdomain`, `.intranet`, `.corp`, `localhost`) are never published (`is_public_relay`) |
| `addresses` | The above, plus public IP addresses | Private, CGNAT/Tailscale (100.64/10, fc00::/7), loopback, link-local, documentation and reserved addresses are never published (`is_public_ip`) |
| `off` | No DHT node is started, and nothing is published | `spawn_dht` returns early |

- **Bootstrap.** The DHT is joined through `router.bittorrent.com:6881`, `dht.transmissionbt.com:6881`, `dht.libtorrent.org:25401` and `relay.pkarr.org:6881`. Their names are resolved with the system resolver, IPv4 only (`dht.rs`, `BOOTSTRAP_NODES`).
- **What the desktop does.**
  - It publishes the record under its iroh key and republishes it every hour (`iroh-mainline-address-lookup` 0.6, `REPUBLISH_DELAY`).
  - It reads its own record back: a minute after joining, then each minute until the record is found, then every 15 minutes (`DHT_RETRY`, `DHT_RECHECK`).
  - It rejoins after 3 misses in a row.
  - DHT nodes that store or route the record see the desktop's public IP alongside its WAN id, in every mode.
- **What the phone does.**
  - It looks the desktop up only when the endpoint packet says `dht: true`, only during a WAN dial, and read-only (`no_publish`) (`android/bridge/src/api.rs`, `core/src/streaming/dht.rs` `dht_resolver`).
  - The lookup is dropped once connected (`iroh_connection.rs`).
  - DHT nodes see the phone's IP and the id it looks up.
- **Who can read the record.** Anyone who knows the id. The id appears in the QR code, on each paired phone, on relays, and in `endpoint.json`.

### 4.3 Relays

- **Which relays.**
  - With `relay_urls` empty, the desktop uses iroh's default (n0) relays: `use1-1`, `usw1-1`, `euc1-1` and `aps1-1.relay.n0.iroh.link` over HTTPS. This comes from iroh 1.2 `defaults.rs` and `RelayMode::Default` in `server/src/iroh_endpoint.rs` `relay_mode`.
  - Otherwise the desktop uses only your relays (`RelayMode::Custom`).
  - The phone uses n0's relays, or only the desktop's own when the desktop announces only those (`core/src/streaming/iroh_connection.rs`, `dial_relay_mode`).
- **What iroh does with every relay** (iroh 1.2 defaults: `NetReportConfig::default()`, `RelayConfig::from(RelayUrl)`; `Endpoint::builder(Minimal)` does not change them):
  - It keeps a connection to its home relay over HTTPS (TCP 443 for n0).
  - It sends QUIC address discovery (QAD) probes to UDP 7842 (`DEFAULT_RELAY_QUIC_PORT`).
  - It sends an empty HTTPS GET to each relay as a latency probe.
  - It sends a captive-portal check, `GET http://<relay>/generate_204`.
  - It asks the home router for a port mapping over UPnP, NAT-PMP or PCP (`portmapper_config` default).
- **The `Minimal` preset** adds no n0 DNS or pkarr address lookup (iroh `endpoint/presets.rs`). Nothing is published to `dns.iroh.link`.
- **What a relay sees.**
  - Both ends' public IP addresses, and their endpoint ids (needed for routing and for `--allow` checks).
  - Which ids talk to each other, when, and how much.
  - Not the content: the session runs end to end between the two iroh keys (`server/src/relay.rs` module doc).
- **Your own relay.**
  - `ServerConfig::default()` turns on no QUIC/QAD server and no metrics endpoint (`relay.rs`; iroh-relay 1.2 `ServerConfig`).
  - In HTTPS mode, the Let's Encrypt account and certificate keys are cached in `<state>/relay-acme` (owner-only folder). The `--email` contact is sent to Let's Encrypt.
  - It logs refused endpoint ids, shortened (`Allowlist::on_connect`).

### 4.4 Other outbound traffic from the desktop

| Destination | When | What is sent | Source |
|---|---|---|---|
| n0 relays, or yours (§4.3) | Always while the server runs: the WAN endpoint cannot be turned off in config | iroh relay traffic and probes | `server/src/service.rs` (`spawn_wan_endpoint` is unconditional), `iroh_endpoint.rs` |
| Home router (UPnP, NAT-PMP, PCP) | iroh's port mapper | A mapping request for the iroh UDP port | iroh 1.2 `endpoint.rs` (`portmapper_config` default) |
| DHT bootstrap hosts and DHT nodes (§4.2) | Unless `dht_publish = "off"` | DHT traffic and the signed record | `core/src/streaming/dht.rs` |
| DNS: the system resolver, and iroh's fallback tier | Resolving relay hosts and DHT bootstrap names | Those host names | `dht.rs` (`to_socket_addrs`). iroh-dns 1.3 keeps a deferred fallback tier of public resolvers, Cloudflare 1.1.1.1 among them (`FallbackMode`, `default_fallback_nameservers`) |
| The `tailscale` CLI: `status --json`, `ip -4` | At start, and every 10 s for peer discovery (`DiscoveryService::run(10 s)`) | Local calls | `core/src/tailscale/mod.rs`, `core/src/tailscale/discovery.rs`, `core/src/local/network.rs` |
| Tailscale LocalAPI over HTTP: `127.0.0.1:52552`, `127.0.0.1:50502`, `100.100.100.100:1053` (`/localapi/v0/status`, `/api/status`) | When the CLI fails | A GET with no body | `core/src/tailscale/mod.rs` |
| A phone's IP and port (§1.4) | Each file or clipboard-image transfer | The transfer | `server/src/plugins/share.rs` |
| `<broadcast>:9` UDP | A `kdeconnect.pocketdesk.wol` request from a phone with Power | A magic packet for the given MAC. The address is the one in the packet, else `255.255.255.255`; one outside the desktop's own subnets is refused | `server/src/plugins/wake_relay.rs`, `core/src/tailscale/wol.rs` |
| A peer named on the command line | `pocketdesk connect` only | A control-channel handshake | `server/src/service.rs` |
| Let's Encrypt | `pocketdesk relay --hostname` only | ACME | `server/src/relay.rs` |

Not found in the code: update checks, crash reporting, or analytics sent anywhere. The only HTTP client dependency is `reqwest` in `core/Cargo.toml`. In this code it is used only for the Tailscale LocalAPI calls above (a search for `reqwest` and URLs in `core`, `server`, `desktop` and `android/bridge`). Session telemetry stays in local files (§5).

The desktop app (`desktop/`) probes only `127.0.0.1` for its Diagnostics tab (`core/src/local/diagnostics.rs`). No other network call was found there.

### 4.5 Outbound traffic from the phone

| Destination | When | Source |
|---|---|---|
| Saved or found computers: TCP control port, UDP stream port | Connecting, polling the clipboard, sending notifications and texts to the most recently used paired computer | `android/bridge/src/api.rs`, `android/app/.../notify/PhoneLink.kt` |
| Found computers' control port (bare TCP connect) | Confirming an mDNS result before listing it | `android/app/.../discovery/LanDiscovery.kt` |
| One computer's ports (TCP connect, QUIC version-negotiation probe) | Connection Doctor, on request | `android/app/.../doctor/ConnectionProbe.kt` |
| Relays, DHT bootstrap hosts and nodes, DNS | WAN dial (§4.2, §4.3) | `core/src/streaming/iroh_connection.rs`, `dht.rs` |
| Wake targets, UDP | Wake: subnet broadcasts at home, or an outside address and port | `android/bridge/src/api.rs` (`wake`) |
| `https://dontkillmyapp.com/<maker>` | Only when the user taps the background-limits link (it opens in the browser) | `android/app/.../doctor/BackgroundRunning.kt` |

- **No Google services.** The app has no Google Play services, Firebase or ML Kit dependency. The QR scanner is `zxing-cpp` (`android/app/build.gradle*`).
- **Tailscale code compiled into the bridge.** The bridge contains `TailscaleClient` and `scan_lan_once` (`api.rs` `get_peers`, `check_tailscale_status`), but nothing in the UI calls `RustCore.getPeers` or `checkTailscaleStatus`.

### 4.6 Who can see what

| Party | Sees | Source |
|---|---|---|
| Anyone on the LAN | The mDNS announcement (§4.1). Anyone who reaches TCP 1716 or UDP 4716 with any certificate also gets the desktop's identity packet or v2 identity (id, name, plugin list) and its certificate | §1.2, §1.3 |
| A network observer on the LAN or path | Ports, timing and sizes. TLS and QUIC hide the content. mDNS and magic packets (which carry the MAC) are in the clear | §1, `wol.rs` |
| A desktop the phone connects to | The phone's certificate (a stable identifier across desktops), its device id and its iroh id (on the LAN) | §2.1 |
| A paired phone, with every permission off | Battery, the monitor and window lists with window titles, the HUD, desktop notifications, the WAN endpoint (with direct addresses) and wake info (the MAC) | §3.1 |
| Relay operator | §4.3 | — |
| DHT nodes | §4.2 | — |
| DNS resolvers | Relay and DHT bootstrap host names, and saved computer names | §4.4 |

---

## 5. Files on disk

### 5.1 Desktop

- `<state>` is `$XDG_STATE_HOME/pocketdesk` (normally `~/.local/state/pocketdesk`), or `%LOCALAPPDATA%\pocketdesk` on Windows, or `POCKETDESK_STATE_DIR` (must be absolute). `<config>` is `~/.config/pocketdesk`, or `%APPDATA%\pocketdesk`. A `linux-link` folder from before the rename is moved on first use (`core/src/local/paths.rs`).
- "Owner-only" means `secure_fs::write_private` or `create_private_dir`: mode 0600/0700 on Unix, an owner-only DACL on Windows (`core/src/secure_fs.rs`).
- "Default" means no owner-only call was found for the file itself. On Unix the server makes `<state>` `0700` at start (`paths::protect_state_dir`), so no other local user can open any file in it; where it cannot (a folder it does not own, a filesystem without modes) it starts anyway and logs a warning. On Windows `%LOCALAPPDATA%` is already the user's own and its DACL is left alone; a `POCKETDESK_STATE_DIR` there keeps whatever ACL its folder has.

| File | Holds | Protection | Source |
|---|---|---|---|
| `<config>/config.toml` | Settings | Owner-only when saved through `core::local::config` | `core/src/local/config.rs` |
| `<config>/iroh_secret.key` | The desktop's iroh secret key | 0600, or an owner-only DACL | `server/src/iroh_endpoint.rs` |
| `<state>/certs/identity.key` | The TLS private key | Owner-only | `core/src/streaming/transport.rs` |
| `<state>/certs/identity.der` | The TLS certificate | Default | same |
| `<state>/certs/known_peers.json` | Certificate pins (for `pocketdesk connect`) | Default | same (`update_pins`) |
| `<state>/device_id` | The desktop's device id | Default | `server/src/kde.rs` |
| `<state>/pairing.pin` | The PIN, minting time, grant length | Owner-only | `core/src/local/pin.rs` |
| `<state>/trusted_devices.json` (+ `.lock`) | Paired device ids, guest grants, per-device permissions | Default | `core/src/protocol/kdeconnect.rs` (`TrustStore`) |
| `<state>/device_keys.json` | Per device: TLS key hash and iroh id | Default; atomic and synced | `core/src/local/device_keys.rs` |
| `<state>/endpoint.json` | The WAN endpoint body: id, relay URLs, direct addresses, `dht` | Default | `core/src/local/identity.rs`, `iroh_endpoint.rs` |
| `<state>/wan_status.json` | Yes/no facts (UDP, NAT varies, public IPv6, captive portal, DHT state). No addresses | Default | `core/src/local/diagnostics.rs` |
| `<state>/live_sessions.json` | Live sessions: id, device id, peer address, transport, start time | Default | `server/src/live_sessions.rs`, `core/src/local/sessions.rs` |
| `<state>/streaming_sessions.log`, `streaming_sessions.jsonl` | One record per session: outcome, duration, device id, latency tails, link counters, encoder. Capped at 1 MiB and 4 MiB | Default | `server/src/session_telemetry.rs`, `core/src/streaming/session_telemetry.rs` |
| `<state>/pending_approvals.json`, `approvals/` | Held sessions (device id, peer address) and the owner's answers | Default | `core/src/local/approval.rs` |
| `<state>/received.json` | The last 50 received files: name, path, size, time, sending device | Default | `core/src/local/received.rs` |
| `<state>/outbox/`, `sent.json` | Files the app sends to a phone, and their state | Default | `core/src/local/outbox.rs`, `sent.rs` |
| `<state>/sms.json`, `sms_outbox/` | Texts and replies (newest 1000 by default) | Owner-only | `core/src/local/sms.rs` |
| `<state>/replies.log` | `<time>\t<notification id>\t<reply text>` for each notification reply | Default | `server/src/plugins/notification_reply.rs` |
| `<state>/kick`, `kicks/`, `control/` | Requests from the app and CLI to end a session or hand control over | Default | `core/src/local/sessions.rs`, `paths.rs` |
| `<state>/server.pid` | Server pid | Default | `server/src/state.rs` |
| `<state>/logs/pocketdesk.*.log` | Daily logs, 7 kept, only with `start --background`. They can contain peer addresses and device ids, and a pairing PIN only when no desktop notification could show it | Default | `server/src/main.rs`, `server/src/plugins/pair.rs` |
| `<state>/relay-acme/` | Let's Encrypt account and certificate keys (relay HTTPS mode) | Owner-only folder | `server/src/relay.rs` |
| `Downloads/` (else home) | Files received from phones; Mark of the Web on Windows | — | `server/src/plugins/share.rs`, `SECURITY.md` |

### 5.2 Phone

All of these are inside the app's private storage unless noted. None of it is backed up or moved to a new phone: `allowBackup="false"`, and the extraction and full-backup rules exclude every domain (`AndroidManifest.xml`, `res/xml/data_extraction_rules.xml`, `res/xml/backup_rules.xml`).

| Location | Holds | Source |
|---|---|---|
| `files/pocketdesk/certs/identity.key` (owner-only write), `identity.der` | The phone's TLS key and certificate | `android/bridge/src/api.rs`, `core/src/streaming/transport.rs` |
| `files/pocketdesk/certs/known_peers.json` | Desktop certificate pins | same |
| `files/pocketdesk/certs/device_id`, `paired_servers.json` | The phone's id, and the desktops it paired with | `android/bridge/src/api.rs` |
| `files/pocketdesk/certs/iroh_device.key` | The phone's iroh key | `api.rs` (`device_wan_key`), `iroh_connection.rs` |
| SharedPreferences `pocketdesk-host` | Saved computers: address, ports, WoL MAC, `wake_info`, wake settings, `wan_identity` (endpoint JSON), monitor, paired flag | `android/app/.../HostStore.kt` |
| SharedPreferences `pocketdesk-ui` | App settings | `Prefs.kt` |
| SharedPreferences `pocketdesk-phone-notifications` | On/off, the opted-in app list, seen ids | `notify/PhoneNotifications.kt` |
| SharedPreferences `pocketdesk-texts` | On/off, and up to 200 texts queued while the computer is away, with send tracking | `notify/Texts.kt` |
| `cache/share`, `cache/clipboard`, `cache/clipboard-out` | Files being shared, clipboard images | `service/TransferService.kt`, `ui/ClipboardImages.kt` |
| MediaStore `Downloads/Pocketdesk` (shared storage) | Files received from the desktop | `service/DownloadTarget.kt` |

---

## 6. Limits and refusals

| Limit | Value | Source |
|---|---|---|
| Connections per address, control and QUIC ports | 8 (QUIC counted only after Retry) | `server/src/conn_budget.rs` |
| Connections in all, control and QUIC ports | 128 | same |
| WAN (iroh) connections in all | 128, no per-address limit | `server/src/iroh_endpoint.rs` |
| Control handshake | 8 s for TLS and HELLO, 5 s for the identity, 64 KiB per handshake line | `server/src/service.rs` |
| Control packet | 4 MiB per line (TCP), 4 MiB per frame (v2) | `service.rs` (`MAX_PACKET_LINE`), `core/src/protocol/v2.rs` |
| File transfers | 4 at once on the desktop, 2 per phone; refused, not queued | `server/src/plugins/share.rs` |
| File size, name | 8 GiB; names cut to 200 bytes | same, `server/src/offers.rs` |
| Transfer pace | 10 s to connect, 30 s idle, at least 32 KiB/s | `share.rs` |
| Clipboard image | 32 MiB PNG, 40 megapixels | `core/src/protocol/clipboard.rs`, `server/src/platform/clip_image.rs` |
| Phone notifications shown | 20 a minute by default (`notifications_per_minute`) | `core/src/local/config.rs` |
| Text replies from the phone | 5 a minute; at most 2000 characters | `android/app/.../notify/SmsReplyRule.kt`, `core/src/local/sms.rs` |
| QUIC idle and keepalive | 45 s idle, 15 s keepalive | `core/src/streaming/transport.rs` |
| PIN | 300 s; 5 wrong guesses; one new PIN per 10 s | §2.5 |

`SECURITY.md` covers the rest: approval holds, permission changes applied within a second, file-browse confinement, and the known limits.

---

## 7. Not found in the code / to confirm

Resolved on 2026-10-14 and folded into the sections above: the capabilities report's ALPNs, TLS 1.2 on TCP, the UDP 1716 firewall rule, the PIN in the log, state-file permissions on Unix, Android backup, tailnet addresses in mDNS, `tailscale netcheck`, the wake relay's targets, and the phone's DNS: the bridge hands iroh the application context, so relay names go to the network's DNS servers (`LinkProperties.getDnsServers`), not public resolvers (`android/bridge/src/lib.rs`, `install_android_context`).

1. **The Tailscale LocalAPI fallback to `100.100.100.100:1053`.** Off a tailnet, that address (CGNAT space) goes to the default route. Confirm whether this is acceptable.
2. **Desktop-to-phone data needs no permission.** Desktop notifications, window titles (monitors, windows, HUD), battery, wake info (MAC) and the WAN endpoint go to every phone that may receive pushes. No per-device permission gates them; the permission table covers only packets from the phone.
3. **v2 checks the device key once, at session start.** It also does not use `standing` per packet, as TCP does (`v2_multiplexer.rs`). The Android UI does not use v2. Confirm whether v2 is still meant to be reachable on UDP 4716.
4. **DHT node mode.** Not checked: whether the desktop's n0-mainline node runs in server mode, which would store and route other people's records, or client mode.
5. **The `--hostname` relay.** Not traced: which ACME challenge type it uses (port 80 vs 443), and what iroh-relay logs about client IPs by default.
6. **The phone's Tailscale code is unreachable from the UI.** `get_peers` and `check_tailscale_status` in the bridge include Tailscale LocalAPI calls, but no UI caller was found. Confirm whether they can be removed.
7. **The rest of the v2 channel kinds.** Video, audio, input and file handling over v2 was not traced.
8. **The desktop identifies itself before the client does.** On TCP 1716 and on v2, any client that completes a handshake with any certificate learns the desktop's id, name and plugin list (§1.2 step 4, §1.3).
