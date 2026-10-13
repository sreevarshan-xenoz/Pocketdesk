# LAN discovery — design

Status: decided 2026-10-13, branch `feat/lan-discovery`. Roadmap F6 ([feature roadmap](2026-10-12-feature-roadmap.md)).

## Why

To reach a computer the first time, a phone user scans the desktop app's QR code or types an address and two ports. The server already announces itself over mDNS (`_linux-link._tcp`, from `core::tailscale::lan`), but nothing on the phone listens. KDE Connect, Moonlight and RustDesk all list computers on the same network, and the roadmap ranks this as the setup friction users complain about most after input.

## Decisions (made autonomously; each is cheap to revisit)

| # | Decision | Why |
|---|---|---|
| L1 | The phone browses `_linux-link._tcp` with Android's `NsdManager` while the Home screen is visible, and stops when it is not. | `NsdManager` takes the multicast lock itself and needs no new permission. Browsing only on Home keeps battery and the network quiet. |
| L2 | The announcement's TXT record gains `id` (the desktop's device id) and `stream` (the streaming port). The record keeps `name` and `version`; its port stays the control port. | The phone can then fill in both ports, and recognise a saved computer whose address changed, by the id it saved at pairing. |
| L3 | Home lists found computers that are not saved under **On this network**, above the saved ones. Each shows its name and address. A tap connects, and an unpaired computer opens the PIN sheet as it already does. A saved computer that is found gets an **On this network** label. | One tap to the PIN. Nothing is saved until the user connects. |
| L4 | When the announcement lists several addresses, the phone prefers a private IPv4 address (10/8, 172.16/12, 192.168/16) over Tailscale's 100.64/10 and over IPv6. | The phone found it on this network, so the LAN address is the direct one. |
| L5 | Discovery never pins a certificate or trusts anything. Pairing still needs the PIN, and the QR code is still the only way to pin a fingerprint before the first stream. | mDNS is unauthenticated: anyone on the network can announce anything. The name is shown as the computer "calls itself", the same as the pairing link. |
| L6 | A new `lan_discovery` key in `config.toml` (on by default). The desktop app's Settings has a **Visible on this network** switch for it, and the change applies when the server restarts. | Some owners do not want to announce the machine on a café network. On by default, because that is what makes discovery useful. |

## Out of scope

- Discovery over Tailscale or the internet (WAN connections go through iroh, by pairing).
- The desktop app listing phones.
- Auto-connect to a found computer.

## Testing

| Layer | Tests |
|---|---|
| core | The TXT record carries `id` and `stream`; `lan_discovery` defaults on, round-trips, and its key is known to the saver. |
| server | Registration is skipped when `lan_discovery` is off. |
| JVM | Choosing an address (private IPv4 first, then the rest), parsing the TXT record (missing or bad ports fall back to the defaults), and merging found computers with saved ones (by address or by device id). |
| desktop | The Settings switch writes the key. |
| Device | The A71 on the same Wi-Fi as the Windows host lists it under "On this network" with its name; tapping it opens a session (already paired) and the saved card shows the label. With the switch off and the server restarted, it disappears. |
