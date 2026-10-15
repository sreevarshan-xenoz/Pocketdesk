# The control channel over the internet path (T10a)

Item 1 of `docs/research/2026-10-15-what-makes-it-worth-using.md`.

## The problem

Away from home without Tailscale, a phone reaches the computer only for a session: the video rides iroh, but the control channel is TLS over TCP 1716 on a LAN or tailnet address. With no control link:
- the phone's notifications, texts, battery and media stop reaching the computer;
- clipboard sync, links and the remote file browser stop;
- the computer's pushes (links, offers, find-my-phone, now playing) stop.

## The change

The control channel gets a second road: an iroh connection with the ALPN `pocketdesk-control/1`, dialed to the endpoint id the phone learned over the trusted LAN channel. On it, one bidirectional stream carries the same lines as the TCP channel:
- `POCKETDESK_HELLO` / `POCKETDESK_OK`;
- the two identities;
- then packets, through the same packet loop, permission gate and plugins.

### Who may use it

Only a phone that is paired and whose iroh key is registered for its id, by a LAN session its TLS key proved (device keys). The connection's proved iroh key must be that one. Anything else is closed after the identity, with one log line:
- an unpaired phone;
- a paired phone with no iroh key registered;
- another key.

There is no first use and no pairing over this road, with or without `pairing_required`. Pairing stays at home, by QR or PIN. Standing is re-read on every packet, as on TCP, so unpairing or a key change applies to an open connection.

### What changes on the desktop

- The WAN endpoint offers both ALPNs. A connection with the control ALPN is handed to the service's control loop, which owns the plugins. It counts against the WAN listener's 128 connections, as a session does.
- `handle_connection_with_kde` is split in two:
  - the TCP/TLS prelude, which yields a stream, its proved key and its peer address;
  - `serve_control`, generic over the stream, which runs the handshake and the loop.

  The iroh road enters `serve_control` with the iroh key and no peer address.
- With no peer address, nothing that dials the phone back can run: file offers, shares, folder fetches and image clipboard transfers. Those are refused with their usual "phone not reachable" answers. Carrying transfers inside the connection is the next step (T10b).
- The wake packet is not sent on this road: it describes the adapter the phone reached at home, and an internet connection reached none. The phone keeps the one it learned at home.

### What changes on the phone

- `connect_to_peer(address, port, wanIdentity)`: TCP first, as today. If that fails and the computer's cached WAN identity is known, the bridge dials iroh with the control ALPN, using the phone's own iroh key and the DHT lookup when the identity says `dht`. It then runs the same handshake.
- The control slots (writer, peer, reader) are the same; the writer is now a boxed stream, so either road fills them.
- The connection keeps its iroh endpoint for its life and closes it when it ends.
- Kotlin passes `HostStore.wanIdentity` for the computer from every caller that keeps a control link:
  - the session's control link;
  - clipboard sync;
  - the notification and texts loop.

  Pairing passes none.

### Why not race TCP and iroh

A phone at home must keep the LAN road: it carries transfers, and it is the one that proves keys. TCP to a LAN address from outside fails within the connect deadline (10 s), usually at once with "network unreachable". A later step can shorten that.

## Tests

- **Server, pair.rs:** `internet_standing_in` admits only paired + registered iroh key + match. It refuses:
  - unpaired;
  - a TLS-only device;
  - another iroh key;
  - no key;
  - an unreadable store.
- **Server, end to end:** over loopback iroh with `use_state_dir`:
  - a phone whose iroh key is registered gets `POCKETDESK_OK` and the identity, and a clipboard packet reaches the plugin;
  - an unregistered key is closed after its identity.
- **Core:** the client handshake runs over any stream: a duplex pair test, and the TCP path's existing tests.
- **Bridge:**
  - a TCP failure with an identity dials iroh;
  - with no identity, it reports the TCP error as before.
- **Device:** the A71 on Wi-Fi with TCP 1716 blocked by a firewall rule scoped to my debug server's port:
  - the control link comes up over iroh;
  - clipboard and notifications cross.
