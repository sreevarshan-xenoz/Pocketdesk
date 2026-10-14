# Your own relay (roadmap N2)

Date: 2026-10-13. Status: approved (autopilot). Follows N1 (`wan_status.json`, Diagnostics "From outside").

## Why

Away from home, a phone reaches the desktop directly when the desktop's NAT allows it (N1 measures that), and through a relay when it does not. Today that relay is n0's free public service. For people who want nothing in the path but their own machines, Pocketdesk can run the relay itself, the way RustDesk users run their own server.

The relay never sees a session: sessions are end-to-end encrypted (QUIC TLS between the two ends' keys), and the relay forwards ciphertext. What it sees is which endpoints talk, and how much.

## Where it runs

A relay must be reachable from the internet:
- the desktop itself, with one port forwarded (or opened by UPnP);
- any always-on box at home with a port forwarded;
- or a free cloud VM (Oracle Cloud's Always Free tier costs nothing).

A relay behind the same NAT as an unreachable desktop helps no one, and the docs say so.

## Design

### `pocketdesk relay`

Runs iroh's relay server (the `iroh-relay` crate, `server` feature).

| Mode | Flags | URL phones and desktops use |
|---|---|---|
| Plain | `--port 3340` (the default) | `http://<public address>:3340` |
| HTTPS | `--hostname relay.example.org --email you@example.org` | `https://relay.example.org`, with a Let's Encrypt certificate obtained automatically (ports 80 and 443 must reach it). A free DuckDNS name is enough. |

Plain mode is honest about what it shows. Sessions stay end-to-end encrypted, but an observer on the path can see that this relay connects these endpoint ids.

### Who may use it

A relay open to the internet is free bandwidth for anyone. So the relay admits only listed endpoint ids:
- `--allow <endpoint id>`, repeatable;
- on the desktop itself, `--allow-paired`: this desktop's own iroh id, and the iroh keys its paired phones registered (device keys), re-read on each connection, so a newly paired phone works without a restart;
- `--open` admits anyone, and says so loudly at start.

With none of these, the relay refuses to start.

### The desktop and the phone

`pocketdesk relay-ids` prints the ids to `--allow` on a relay elsewhere: this desktop's, and each paired phone's registered WAN key.

config.toml gains `relay_urls = ["https://relay.example.org"]`. When it is set, the desktop's WAN endpoint uses those relays (`RelayMode::Custom`) instead of n0's. `endpoint.json` (and so the QR code and the phone's cached identity) carries the relay URL, and the phone dials through it. Nothing is set on the phone.

Diagnostics' "Internet path" names the relay in use: n0's, or yours.

## Pieces

1. **server:**
   - `relay` and `relay-ids` subcommands;
   - access control (`Allowlist`: a fixed set plus, with `--allow-paired`, the live device keys);
   - `relay_urls` in config and the WAN endpoint's relay mode.
2. **core:** `Config::relay_urls`, and Diagnostics naming the relay.
3. **Tests:**
   - two endpoints connect through an in-process relay (done in the spike);
   - an endpoint that is not on the list is refused;
   - the config is parsed;
   - `relay-ids` lists the right ids.
4. **Docs:** README (Remote access), the man page, AGENTS, and the roadmap entry.

## Out of scope

- Running the relay inside the desktop app's UI.
- A phone-side relay setting: the desktop's announced relay is enough.
- STUN-like QUIC address discovery on our relay: iroh-relay's QUIC server needs TLS and is optional; endpoints already learn their addresses through the relay's HTTP path.

## As built

- **Address discovery mattered more than this spec said.** iroh's net report measures UDP and the NAT through the relay's QUIC address discovery, so with a plain relay of your own, "UDP" reads as false. Diagnostics' "From outside" now says "not measured" when the desktop announces its own relay, instead of "UDP is blocked".
- **A re-pair with the same certificate kept dropping the WAN key.** It was found on the device run: a phone re-paired over `adb reverse`, then was refused on WAN. Pairing again with the key already bound now keeps the registered iroh key (`6fa2362`).
- **Device-verified on this box:**
  - `pocketdesk relay --allow-paired` ran on the desktop;
  - the server with `relay_urls` announced it in `endpoint.json`;
  - an A71 WAN session went through it (`wan_relayed`: 15 s through the relay, then direct, since the phone was on the same Wi-Fi);
  - `relay-ids` listed the desktop and the phones.

  Not yet run: HTTPS mode with a real name, and a relay reached from outside the home network.
- **Fresh review (0 Critical, 3 Important, plus one Minor raised to Important), fixed one commit each:**
  - `--allow-paired` admitted every key in `device_keys.json`, including an expired guest's or a phone unpaired by hand, with no way to revoke it. It now admits only devices the trust store trusts now; a missing or unreadable store admits no phone.
  - The phone still homed on n0's relays, so "nothing but your own machines" was false. When the desktop announces only its own relays, the phone now dials with only those. Device-verified: the phone's home relay became the user's relay, and its log names no n0 host.
  - A `relay_urls` entry like `relay.example.org:3340` parsed with the host as the scheme, and Diagnostics then blamed internet access. Entries must be `http://` or `https://` with a host. Diagnostics reads `relay_urls` from the config, and says when an entry is refused or the relay is not reached.
  - The Let's Encrypt cache (`relay-acme`) holds private keys and was created with default permissions. It is now owner-only.
- **Deferred minors:**
  - "Relay on https://…" is printed before a certificate exists, and there is no staging option. Only 443 must be reachable for issuance.
  - Binding 80 and 443 on Linux needs privileges, which the docs do not cover yet.
  - The desktop id is loaded even without `--allow-paired`, and a failure to load it is swallowed.
  - The relay process waits only for Ctrl+C, not for the relay to stop.
  - Access is checked at connect, so an unpaired phone keeps a relay connection that is already open.
  - Refusals are logged without a limit.
  - The refusal integration test has no positive control.
  - A same-certificate re-pair no longer revokes a leaked WAN key; Remove does.
  - A type error in `relay_urls` stops every subcommand.
  - Without address discovery, Diagnostics cannot see an outbound UDP block.
