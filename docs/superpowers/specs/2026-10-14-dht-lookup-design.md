# Finding the desktop on the Mainline DHT (roadmap N3)

Date: 2026-10-14. Status: approved (autopilot). Follows N1 (`wan_status.json`) and N2 (your own relay).

## Why

Away from home, the phone dials the address it last learned at home over the control channel (`endpoint.json`'s `endpointId`, `relayUrls` and `directAddrs`). That address goes stale in two ways:
- **The desktop's home relay changes.** On n0's relays the desktop picks the closest region, and that pick can move. A phone that dials the old relay never meets the desktop there.
- **The desktop's addresses change.** Most home connections get a new public IPv4 or IPv6 prefix every so often. A desktop that phones could reach directly (a public IPv6 address, or a port the router mapped) is then dialed at the old address.

The phone can only refresh its copy by coming home. N3 lets it look the desktop up by id instead, without any server of ours or n0's: the desktop publishes a record signed with its iroh key to the BitTorrent Mainline DHT (pkarr), and the phone reads it.

## What already works, and stays

- **IPv6 is already bound on both ends.** Our `bind_addr(0.0.0.0:0)` replaces only iroh's default IPv4 socket. The default `[::]` socket stays, so both ends already use IPv6 direct paths when both have IPv6. iroh learns them through the relay.
- **The relay is not chosen from a list.** iroh dials any relay URL it learns, and the relay map supplies only auth tokens. So a relay learned from the DHT works even when the phone was set up with only the desktop's own relays (N2).

## Design

### The desktop publishes

The desktop's WAN endpoint gets the DHT address lookup (the `iroh-mainline-address-lookup` crate). It signs the record with the desktop's iroh key, publishes when its addresses change, and republishes hourly.

config.toml gains `dht_publish`:

| Value | What the record holds |
|---|---|
| `"relay"` (default) | The relay URLs only. |
| `"addresses"` | The relay URLs plus this computer's **public** IP addresses with their ports. |
| `"off"` | Nothing is published. |

**Never published:** private, loopback, link-local, carrier-NAT (100.64/10, which is also Tailscale's) or unique-local IPv6 (fc00::/7, Tailscale's too) addresses, documentation ranges, and relay URLs whose host is such an address (a relay at `http://192.168.0.10:3340` is useless outside and names the LAN). One pure function, `publishable`, makes that cut, and it applies in every mode.

**Why `relay` by default:** a relay URL says little. On n0's relays it names a region. The record is readable by anyone who knows the endpoint id, and by the DHT nodes that store it. The id is in the QR code, on the phone and on every relay the desktop uses. **Why `addresses` is opt-in:** it ties a stable id to the home's public IP over time. It is what makes a relay-free session possible, when the desktop has a public IPv6 address or a mapped port.

### The phone looks it up

The phone adds a resolve-only DHT lookup to its WAN dial. It never publishes, since its key is its device key. iroh starts the lookup alongside the cached addresses and merges what it finds, so a good cache still connects as fast as before.

It looks only when the desktop says it publishes. The endpoint packet gains `"dht": true|false`. Kotlin stores the identity JSON whole, so the field reaches the bridge with no Kotlin change. A lookup for a desktop that publishes nothing would only tell the DHT that this phone is looking.

The lookup cannot be forged: the record is signed with the desktop's key, and the iroh handshake proves that key again. A wrong record can at worst add an address that fails.

### Measured, not assumed

Publishing is fire-and-forget, so the server checks it: about a minute after start, and then every 15 minutes, it resolves its own id on the DHT with the same lookup. `wan_status.json` gains `dht` (the mode) and `dht_found` (`true`, `false`, or absent until the first check).

Diagnostics gets a row, **"Found by id"**:
- `relay` / `addresses`, found: ok, "published on the Mainline DHT and read back (relay address only | with public addresses)";
- published, not found: a warning, "published, but not read back: the DHT may be unreachable from this network (outbound UDP)". It shows "checking" for the first two minutes;
- `off`: info, "off: a phone away from home dials the address it learned at home";
- server not running: skipped, like the other server-side rows.

The debug report carries the row; it names no address.

## Pieces

1. **core:**
   - `Config::dht_publish` (`DhtPublish` enum, default `Relay`);
   - `WanStatus.dht`, `WanStatus.dht_found`;
   - `check_dht` in Diagnostics;
   - under `wan`, `IrohDial::dial` takes an optional `DhtAddressLookup`, and `dht_resolver()` builds the resolve-only one for the real DHT.
2. **server:**
   - `publishable` (the address cut);
   - the WAN endpoint's DHT lookup per `dht_publish`;
   - the self-check task feeding the recorder;
   - `"dht"` in the endpoint packet.
3. **bridge:** dial with `dht_resolver()` when the cached identity says `dht: true`.
4. **desktop:** the Diagnostics row.
5. **Tests:**
   - `publishable` keeps public v4/v6 and every relay with a hostname, and drops each excluded range;
   - the config is parsed, and an unknown value is refused;
   - `check_dht` gives each verdict above;
   - integration: on a local Mainline testnet, a desktop endpoint publishes, and a phone endpoint given **only the id** (no relay, no address) finds it through the lookup and opens a session.
6. **Docs:**
   - README (Remote access) and the man page;
   - `config.toml.example`;
   - SECURITY.md: what the record reveals;
   - checklist §37, the roadmap entry and AGENTS.

## Device check (A71 ↔ this box)

1. Start the server with `dht_publish = "relay"` on n0's relays, and confirm Diagnostics reads "read back".
2. Edit the phone's cached identity (run-as, debug build) so its relay URL is wrong and its direct addresses are empty, then connect over WAN. Only the DHT can supply the right relay.
3. The same with `dht_publish = "off"`: the dial fails, which is the control.

Owed: a session from outside the home network found only by the DHT, and `addresses` mode with a real public IPv6 (this box has none).

## Out of scope

- n0's DNS (`dns.iroh.link`) or pkarr relays: that is someone's server.
- Publishing from the phone.
- A desktop that changed its iroh key: the phone holds the id, and a new key means pairing again.
- Reacting to a DHT lookup during a live session: lookup happens on dial.

## As built

- **Device-verified on this box and the A71** (n0's relays, the default `relay` mode):
  - the server published within 4 s of joining and read its record back a minute later ("Found by id": read back, relay address only);
  - a phone whose cached identity was made stale (an n0 relay the desktop is not on, no direct addresses, a closed LAN port) found the record in about 3 s, connected through the relay it named, and went direct a second later;
  - the same identity with `dht: false` timed out (the control).

  Owed (checklist §37): a session from outside, `addresses` mode with a real public IPv6, a network that blocks UDP, and the Linux host.
- **Fresh review (0 Critical, 7 Important), fixed one commit each:**
  - **The DHT's bootstrap.** n0-mainline resolves its bootstrap names with blocking DNS inside its constructor and never retries them, so a server started before the network was up never joined. It also bound 6881, a torrent client's port. `core::streaming::dht` now resolves the names on a blocking thread and builds the node on a port the OS picks. The server joins in the background, each minute until DNS answers, and the phone builds its lookup only when it dials.
  - **The phone's DHT node lived for the whole session.** The dial now clears it once connected.
  - **The self-check's verdicts stuck for 15 minutes.** It now checks each minute until found, treats "no relay yet" as checking, and rebuilds the node after three misses in a row. A failed publish otherwise waits an hour.
  - **Diagnostics over-claimed.** A DHT that had not started read "off"; it is now "unjoined", a warning. "Published" was shown before anything was. "With public addresses" was claimed from a relay match alone, and now needs an IP address read back.
  - **LAN names in the record.** Relays named `nas` or `pi.lan` (single labels, LAN suffixes) are now left out, and so is `3fff::/20`.
  - **Telling the phone to look up an empty record.** `dht: true` now needs a publishable address.
  - **The docs understated what the DHT sees.** In every mode, the storing nodes see the home IP tied to the id, and each phone dial shows its IP and the id it looks for.

  Minor fixes: a test of the real publish path on a testnet; the self-check ends with the endpoint.
- **Accepted:**
  - A still-valid record from an earlier run with the same relay counts as read back; it is equally correct.
  - Mainline switches a node that is reachable from outside to server mode after 15 minutes; a desktop behind NAT stays a client.
  - A dial that fails drops its iroh endpoint without closing it (an ERROR line in logcat). This predates N3.
