# Pocketdesk desktop app — design

Status: decided 2026-10-12, branch `feat/pocketdesk`.

## Why

Pairing, settings and session control are CLI-only today (`linux-link pair`, editing `config.toml`, `linux-link kick`). That hurt in real testing on Windows, where there is no terminal habit and the server runs hidden from a logon task. The user chose:
- a desktop companion in **Rust + egui**;
- a tray icon, settings, paired devices and logs;
- **QR pairing** that the phone scans.

Success means a person can do all of this without a terminal:
- install;
- pair a phone by scanning a code;
- see and revoke paired phones;
- see and end live sessions;
- change settings;
- read the log.

## Decisions (made autonomously; each is cheap to revisit)

| # | Decision | Why |
|---|---|---|
| D1 | A new workspace crate `desktop/` (package `pocketdesk-desktop`, binary `pocketdesk`), built with eframe 0.36 (glow), tray-icon 0.26 and qrcode 0.14. | All three are MIT/Apache. A spike on 2026-10-12 built and ran them on the `x86_64-pc-windows-gnu` toolchain, importing only system DLLs. |
| D2 | The app talks to the server **through the files they already share**, not a new IPC channel. | This covers the PIN file, trust store, `live_sessions.json`, the kick request file, `server.pid`, `config.toml` and the log files. Pairing, unpairing and kicking already work this way for the CLI. It also works when the server is stopped (settings, devices), and it adds no attack surface (no socket or port to authenticate). Live log streaming and push updates are the cost: the app polls once a second, which is what the CLI's `status` already trusts. |
| D3 | Shared file logic moves into a new **`core::local`** module (feature `local`). The server and the app both use it, and the app does not depend on the server crate. | Depending on `linux-link-server` would drag iroh, quinn, enigo, PipeWire and FFmpeg into a GUI. Paths, `Config` (gaining `Serialize` + `save`), the PIN-file format, the live-session mirror format and the kick request are small and dependency-free. The server keeps its current module paths as re-exports. |
| D4 | The server writes **`<state>/endpoint.json`** (the `kdeconnect.linuxlink.endpoint` body it already pushes to phones: endpointId, relayUrls, directAddrs) once its iroh endpoint is up. | The QR can carry the WAN identity without the app linking iroh. |
| D5 | The QR is a URI: `pocketdesk://pair?v=1&name=…&id=…&a=ip1,ip2&cp=1716&sp=4716&pin=…&fp=…&ep=…` | `fp` is the SHA-256 of `certs/identity.der` (the same hash the phone pins today on first use). `ep` is `endpoint.json`, base64url. A URI lets a phone camera app hand it to Pocketdesk through a deep link, and the in-app scanner reads the same string. |
| D6 | The QR's PIN is minted exactly as `linux-link pair` mints one: the CLI PIN file, a 5-minute TTL, permanent trust. The app shows a countdown and replaces the code at expiry. | One pairing path on the server, already tested; no protocol change. |
| D7 | `fp` lets the phone **pin the desktop's certificate before its first streaming connection** instead of trusting whoever answers first (TOFU). | A QR the user scans off the desktop's own screen is trustworthy, which closes TOFU's first-use hole for the QUIC streaming channel. Limits found in review: (a) a `pocketdesk://` link can be opened by any web page or app, so the phone always asks before pairing, and a link can never replace a certificate already trusted for an address (removing the computer clears it); (b) the PIN still rides the control channel, which is plain TCP — the same exposure as typing the PIN — so a network attacker can still observe it. Encrypting the control channel is separate work. |
| D8 | Phone side: an in-app scanner (CameraX + ML Kit barcode-scanning, the *bundled* model) plus an intent filter for `pocketdesk://pair`. | The bundled model needs no Google Play services. Manual PIN pairing stays as it is. |
| D9 | Settings edits are saved to `config.toml` by editing the existing document in place (`toml_edit`: comments and keys this version does not know survive) and take effect at the next server start. The app says so and offers **Restart server**: stop through the existing graceful-stop request, then start again (Windows: run the logon task; Linux: `systemctl --user restart linux-link` when the unit exists, otherwise spawn `linux-link start --background`). | The server reads config once at startup; hot reload is out of scope. |
| D10 | Closing the window hides it to the tray. The tray menu has Open, Pair a phone…, and Quit. Only one instance runs: a second launch focuses the first. | This is what people expect from a desktop companion. Without a single instance, two apps would race on the PIN file. |
| D11 | The app never runs power or lock actions, and it never stops the server on its own. Stopping happens only on the user's click (Restart). | Matches the project rule; the developer box never runs these in tests. |

## Out of scope (YAGNI)

- A socket/pipe IPC or live push.
- Hot config reload.
- Remote management of another PC.
- A packaged installer for the app.
- Autostarting the app (the server already autostarts).
- macOS.
- Renaming the binary or its paths.

## Architecture

```
desktop/ (pocketdesk-desktop)          core/src/local/ (feature "local")
  main.rs     single instance, tray,     paths.rs    config_dir, state_dir, file paths
              eframe app                 config.rs   Config + load + save (moved)
  app.rs      tabs, 1 s refresh          pin.rs      PIN file write/read, mint
  pair.rs     QR payload + render        sessions.rs live_sessions.json parse, kick request
  tray.rs     tray icon + menu           server.rs   pid-file liveness, start/stop requests
  restart.rs  stop + start               identity.rs cert fingerprint, endpoint.json, device id

server/  config.rs, state.rs, live_sessions.rs → re-export from core::local
         iroh_endpoint.rs writes endpoint.json (D4)

android/ ui/QrPairScreen.kt (CameraX + ML Kit), deep-link intent filter,
         PairLink.kt (URI → host + pin + fp + ep), bridge: pin a peer certificate
```

### core::local

- Pure std + serde + toml + toml_edit + dirs + sha2.
- No tokio.
- Compiles on every OS, the Android bridge's included (it does not enable it).

Every function takes explicit paths, or has a `*_in(dir)` twin, so tests use a temp dir and never touch the real state.

### Data flow: pairing

1. The user clicks **Pair a phone**. The app calls `local::pin::mint(state_dir)` (6 digits from the OS RNG, written owner-only), then builds a `PairLink` with:
   - local IPs (`core::tailscale::lan::get_local_ips` + the Tailscale IP when `tailscale ip -4` answers);
   - the ports from config;
   - `fp` from `certs/identity.der`;
   - `ep` from `endpoint.json`;
   - `id` = the desktop device id.
2. The app renders the QR and the six digits, with a countdown.
3. The phone scans and parses the link. It pins `fp` for each address in its known-peers store, connects to the first address that answers on `cp`, and pairs with `pin` over the existing control flow. It saves the host (address, ports), caches `ep` as the WAN identity, and opens the session.
4. The server's existing `handle_pair` matches the CLI PIN and trusts the phone. On its next poll the app sees the new device in `trusted_devices.json` and shows "Paired".

### Error handling

| Situation | What the app does |
|---|---|
| Server not running | Status says so, with a Start button. Devices and Settings still work (files only). Sessions and kick are disabled. |
| `certs/identity.der` missing (server never started) | The QR omits `fp`; the phone falls back to TOFU, and the app notes it. |
| `endpoint.json` missing | `ep` is omitted; LAN pairing still works. |
| Corrupt `config.toml` | Shown read-only with the parse error; saving is refused until the user resets it. |
| PIN expired | The QR is regenerated. |

## Testing

| Layer | What is tested |
|---|---|
| `core::local` | Unit tests in temp dirs: config save edits only the changed keys in the existing document (`toml_edit`), so unknown keys and comments survive and an unchanged save is byte-identical, PIN mint/parse and TTL, kick request format against the server's parser, live-session parse, fingerprint against a known DER, `PairLink` encode/decode round-trip. |
| Server | Existing tests keep passing through the re-exports; `endpoint.json` write is unit-tested. |
| Desktop | The UI state model (tabs, derived status) is plain structs tested without a window. On this box, launched and screenshotted (Windows). Tray/GTK on Linux is owed. |
| Android | `PairLink` parsing has a JVM unit test (the first Kotlin test in the tree). On-device: deep link via `adb shell am start -d pocketdesk://pair?...` exercises everything except camera decode. Scanning a real code with the camera is owed: the phone on USB cannot see the screen. |

## Owed / risks

- **Linux:** tray-icon needs GTK3 + libayatana-appindicator dev packages, and a GTK event loop on its own thread. This has not been built or run on Linux yet.
- `install` registers the server task only. Launching the app from the Start menu needs a shortcut, which comes later.
