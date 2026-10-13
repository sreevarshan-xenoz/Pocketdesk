# R1 spike: iroh as the WAN transport

Status: **spike complete — iroh 1.2.0 works locally; integration requires a
connection abstraction in `core` (iroh is not quinn).**

Run it: `cargo test -p iroh-spike` (loopback bidi/uni/datagram echo) or
`cargo run -p iroh-spike` (same thing as a demo binary). Relays and DNS are
disabled — everything runs over direct UDP on 127.0.0.1, so the tests exercise
iroh's real QUIC stack with zero infrastructure.

## The headline finding

iroh 1.x is built on **`noq`** (n0's own QUIC implementation, `noq`/`noq-proto`
1.3), *not* on a quinn fork. `iroh::endpoint::Connection`, `SendStream`,
`RecvStream`, and `QuicTransportConfig` are noq types re-exported through
iroh; they are type-incompatible with the `quinn::Connection` (quinn 0.11 /
quinn-proto 0.11.14) that `core/src/streaming/{transport,streamer,client}.rs`
is written against today. `tests/loopback.rs::iroh_types_are_not_quinn_types`
pins this. The two crates do coexist in one binary (this spike links both), so
a phased design — quinn for LAN, iroh for WAN — compiles.

Consequently there is no drop-in swap. Two viable paths:

1. **Connection abstraction (recommended).** Define a trait pair over the
   handful of operations core uses — `open_bi/open_uni/accept_bi/accept_uni/
   send_datagram/read_datagram/close` plus stream read/write — with a quinn
   impl today and an iroh impl added for WAN dialing. Touches
   `streamer.rs`, `client.rs`, `transport.rs`; the packet wire format
   (`EncodedPacket` header + `InputPacket`) is transport-agnostic and stays.
2. **Full migration to iroh.** Deletes `CertManager`/rcgen self-signed TLS
   (iroh authenticates with its own `EndpointId` keys via spanning TLS) and
   drops quinn entirely, but re-platforms the whole transport layer at once.

Either way the R1 roadmap items land as: iroh `EndpointAddr{id, direct addrs}`
replaces "phone must know desktop IP", NAT traversal comes from iroh, and
Tailscale demotes to a candidate address source.

## API facts learned (save the next person an hour)

- `Endpoint::builder(preset)` → `.alpns(vec![…])` → `.relay_mode(…)` →
  `.bind_addr(addr)` → `.bind().await`. `Builder::bind_addr` returns
  `Result<Self, InvalidSocketAddr>` (easy to miss — it's not infallible like
  quinn's).
- Presets: `Minimal` = crypto provider only (what the spike uses); `N0` =
  n0 public relays + pkarr/DNS publishing at `iroh.link`; `N0DisableRelay` =
  N0 minus relays. Self-hosting an iroh-relay (R1 roadmap) swaps
  `RelayMode::Default` for `RelayMode::Custom(RelayMap)`.
- Dialing without infrastructure works: `server.addr()` gives an
  `EndpointAddr` whose `addrs` include every local socket — loopback **and**
  the machine's routable IPv6 came free. `connect(addr, alpn)` resolves to
  that; no relay, no DNS, no Tailscale needed on a shared LAN/IPv6 network.
- `read_datagram().await` returns `Result<Bytes, ConnectionError>` (one
  level, not quinn's nested future shape), and datagrams only work if
  `QuicTransportConfig::builder().datagram_receive_buffer_size(Some(n))` is
  set — the direct analogue of quinn's `max_datagram_frame_size`. R2#5's
  "one frame ≈ one datagram" idea stays PMTU-capped (~1200 B) under iroh too.
- iroh's transport defaults are not quinn's: keep-alive heartbeat, path
  max-idle timeout, MTU discovery, and multipath (up to N paths) are enabled
  out of the box; a `connect` attempt times out at 10 s. For latency work,
  review `QuicTransportConfig` explicitly — including congestion controller,
  which is set per-connection via `ConnectOptions::with_transport_config`
  (R2#5's `POCKETDESK_CC` env would move there under iroh).
- Always `Endpoint::close().await` — dropping a live endpoint aborts its
  socket task with an ERROR log.
- `iroh::endpoint::Connection` is `Clone` and handle-like; cheap to share
  between tasks (the spike echoes over clones).

## What the spike does NOT answer

- Real-world NAT traversal (the tests are same-host loopback; symmetric-NAT
  behavior needs two networks or a relay).
- Throughput/latency vs quinn on the same link — needs the two boxes (phone +
  desktop) and the abstraction layer first.
- Self-hosted relay deployment (iroh-relay setup is roadmap R1 work, not
  needed for direct-connection validation).
