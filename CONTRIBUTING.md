# Contributing to Pocketdesk

Thank you for your interest in contributing! This document covers the basics.

## Development Setup

### Prerequisites

- Rust 1.85+ (edition 2024)
- JDK 17+ (for Android client)
- Android SDK & NDK (for Android builds)
- Tailscale (optional: one of the ways a phone reaches the computer)
- FFmpeg, PipeWire, xdg-desktop-portal (for streaming)

### Building

```bash
# Build all Rust crates
cargo build --workspace

# Run tests
cargo test --workspace

# Format and lint
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# Build Android client (requires Android SDK/NDK)
cd android
./gradlew assembleDebug
```

### Running

```bash
# Start the server
cargo run --bin pocketdesk -- start

# There is no --config flag: the server reads $XDG_CONFIG_HOME/pocketdesk/config.toml,
# so point XDG_CONFIG_HOME at a scratch directory for a throwaway run.
XDG_CONFIG_HOME=$PWD/devconfig cargo run --bin pocketdesk -- start
```

## Code Style

- **Rust:** Follow `cargo fmt` and `cargo clippy -D warnings`. No warnings allowed.
- **Kotlin:** Follow Android Lint; format with `ktfmt`/`ktlint` defaults.
- **Commits:** Use conventional commit messages (`feat:`, `fix:`, `docs:`, `chore:`, etc.)
- **Generated docs:** `docs/capabilities.md` is the output of
  `pocketdesk capabilities --markdown`. If you change a protocol version constant, an ALPN,
  a capture backend, or a codec, regenerate it in the same commit —
  `cargo test -p pocketdesk-server --test capabilities_doc` fails while it is stale.
  A command's log output goes to stderr, never stdout, because stdout is data.

## Pull Request Process

1. Create a feature branch from `main`
2. Make your changes with clear, focused commits
3. Ensure all tests pass (`cargo test --workspace`)
4. Ensure clippy passes (`cargo clippy --workspace -- -D warnings`)
5. Check nothing sensitive is staged (`git diff --cached --name-only`): no private keys, QUIC/TLS
   certs, pairing PINs, `trusted_devices.json`, `.env`, real device IDs or tailnet addresses, logs or
   databases. Runtime state lives in `$XDG_STATE_HOME/pocketdesk`, outside this repo — keep it there.
   Stage files by name rather than `git add -A`.
6. Push and open a PR with a clear description of changes
7. Wait for review

Releases follow [docs/release-checklist.md](docs/release-checklist.md): every step there comes before the tag, since the tag publishes.

## Reporting Issues

- **Bug reports:** Use the bug report template
- **Feature requests:** Use the feature request template
- **Security issues:** report them privately, as [SECURITY.md](SECURITY.md) describes — never in a public issue

## Architecture Overview

See the README's Architecture section, and `AGENTS.md` for the current state of every feature.
Key components:
- `core/` — Shared protocol, streaming, and utility code
- `server/` — the desktop server for Linux and Windows (the `pocketdesk` command, CLI + service)
- `desktop/` — the Pocketdesk desktop app (egui): status, pairing, devices, sessions, files
- `android/` — Native Kotlin Android client + Rust JNI bridge

The server command, its config and state paths, and the Android package id `dev.pocketdesk.android`
keep their Pocketdesk-era names on purpose: renaming them would break existing installs, pairings
and app updates.
