# Pocketdesk Development Setup

This guide covers the Rust workspace and the native Android (Kotlin) client.

## 1. Prerequisites

- Rust toolchain (stable, edition 2024) + `cargo-ndk`
- JDK 17+ and Android SDK/NDK (for the Android client)
- FFmpeg, PipeWire, xdg-desktop-portal (for the server/streaming)
- FFmpeg **development** libraries (server build now links `ffmpeg-next`
  in-process): on Debian/Ubuntu `libavcodec-dev libavdevice-dev
  libavfilter-dev libavformat-dev libavutil-dev libswresample-dev
  libswscale-dev`; on Arch, the `ffmpeg` package ships them.
- The desktop app (`desktop/`, built by `cargo build --workspace`) needs no
  extra packages: eframe loads X11/Wayland at runtime, and its Linux tray is a
  StatusNotifierItem over D-Bus (`ksni`, no GTK).

## 2. Rust Workspace Validation

From repository root:

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p pocketdesk-core --no-default-features --features client -- -D warnings
```

The second clippy command matters: the Android bridge builds `core` with
`default-features = false, features = ["client"]`, and that profile has
code paths the server build never compiles.

## 2b. Desktop app tests (C4)

- `cargo test -p pocketdesk-desktop` drives the app headless through AccessKit (`desktop/src/ui_tests.rs`, `egui_kittest`), over throwaway state and config folders that the test binary names in `POCKETDESK_STATE_DIR` and `POCKETDESK_CONFIG_DIR`; it never touches this computer's own.
- Snapshots of the Overview, Devices and Settings in both themes need a GPU renderer, so they sit behind a feature: `cargo test -p pocketdesk-desktop --features snapshots snapshots`. The committed pictures (`desktop/tests/snapshots/`) are drawn by CI's `snapshots` job with Mesa's software Vulkan; another GPU draws text a little differently, so compare against CI's.
- After a change to how a page looks, take the new pictures from the failed job's `desktop-snapshots` artifact, look at them, and commit them (or draw them with `UPDATE_SNAPSHOTS=1` on a Linux box with `mesa-vulkan-drivers`).

## 3. Server Run

```bash
cargo run --bin pocketdesk -- start
```

Optional config file location:

- `~/.config/pocketdesk/config.toml` (see `config.toml.example`)

Streaming QUIC congestion control can be A/B tested via the `POCKETDESK_CC`
environment variable (`bbr`, `cubic`, or `new_reno`; default `new_reno`).
It applies to both the server and client endpoints.

```bash
POCKETDESK_CC=bbr cargo run --bin pocketdesk -- start
```

`POCKETDESK_STATE_DIR` moves the state folder (pairings, device keys,
certificates, the PIN, session records) somewhere else, for a portable
setup or a separate set of pairings. It must be an absolute path, in a
folder only you can write (not a shared one such as `/tmp`, where another
user could create it first), and the desktop app needs the same value.
The config file, and so the ports, stay where they are:

```bash
POCKETDESK_STATE_DIR="$HOME/.local/state/pocketdesk-test" cargo run --bin pocketdesk -- start
```

`POCKETDESK_CONFIG_DIR` (also absolute) moves the config folder: `config.toml`
and the iroh identity (`iroh_secret.key`). With both set, a **second server**
runs beside the installed one, for measurements and two-computer tests, without
touching it: give it other ports and keep it off the network's list.

```bash
mkdir -p "$HOME/pd-second/config" "$HOME/pd-second/state"
printf 'control_port = 1816
streaming_port = 4816
lan_discovery = false
' > "$HOME/pd-second/config/config.toml"
export POCKETDESK_CONFIG_DIR="$HOME/pd-second/config" POCKETDESK_STATE_DIR="$HOME/pd-second/state"
cargo run --bin pocketdesk -- start      # its own device id, iroh id and pairings
cargo run --bin pocketdesk -- stop       # with the same variables: stops only this one
```

Its notifications (a PIN, a session) show on your desktop like the installed
server's; tell them apart by the port in its log.

The server's tests point the state folder at a throwaway one
(`core::local::paths::use_state_dir`), so they never read or change your
real pairings.

## 4. Windows server (the Windows host)

The server builds and runs on Windows 10 1903+ / 11 with the **GNU** toolchain
(`x86_64-pc-windows-gnu`, MinGW-w64 — e.g. WinLibs via winget). MSVC also
works where the Windows SDK is installed; the dev box this was written on has
no SDK, so GNU is the one that is exercised.

- **ffmpeg on `PATH`**: the encoder runs as a sidecar (`ffmpeg.exe`), and
  hardware encoders are runtime-verified against it (NVENC, AMF, Quick Sync,
  Media Foundation). A "full" build such as `winget install Gyan.FFmpeg` has
  all of them. In-process FFmpeg (`ffmpeg-next`) is Linux-only for now.
- **cmake** (ships with WinLibs) builds the vendored Opus; `.cargo/config.toml`
  sets `CMAKE_POLICY_VERSION_MINIMUM` so CMake 4 accepts it, and passes
  `--exclude-libs,ALL` to the MinGW linker (see the comment there).
- The Android bridge and the iroh spike are not part of the Windows build:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --exclude pocketdesk-android-bridge --exclude iroh-spike -- -D warnings
cargo test --workspace --exclude pocketdesk-android-bridge --exclude iroh-spike
# live tests that need this desktop / GPU (capture, encoders, input, clipboard, toast):
cargo test -p pocketdesk-core --lib -- --ignored wgc_ dxgi_ windows_rung_roundtrip
cargo test -p pocketdesk-server --lib -- --ignored sendinput_drives clipboard_round_trips a_toast_is_shown
# prints the current media session (start something playing first):
cargo test -p pocketdesk-server --lib -- --ignored --nocapture the_current_media_session_is_read
```

Run it with `cargo run --bin pocketdesk -- start`, or register it at logon
with `pocketdesk install`. Config lives in `%APPDATA%\pocketdesk\config.toml`,
state (pairing, trust store, certs, logs) in `%LOCALAPPDATA%\pocketdesk\`.

## 5. Android Client (Kotlin + Rust JNI bridge)

- `android/app` — Kotlin/Compose app.
- `android/bridge` — Rust cdylib (`libpocketdesk_android_bridge.so`) loaded via JNI by
  `dev.pocketdesk.android.bridge.RustCore`.

Full local build (SDK Platform 37, Build-Tools 36/37, an NDK of r27 or later, cargo-ndk 4.1.2):

```bash
# 1. Cross-compile the bridge into the app's jniLibs (a stripped release
#    build, as CI, releases and F-Droid make it)
sh android/bridge/build-android.sh
# On a Windows box whose default Rust is the GNU one:
#   POCKETDESK_RUST_TOOLCHAIN=stable-x86_64-pc-windows-gnu sh android/bridge/build-android.sh

# 2. Build the APK (picks the .so up automatically)
cd android
./gradlew assembleDebug   # APK: app/build/outputs/apk/debug/app-debug.apk
```

The script pins the Rust toolchain (`android/bridge/rust-toolchain-version`) and,
for CI and F-Droid, the NDK (`android/bridge/ndk-version`, through
`ANDROID_NDK_HOME`); without `ANDROID_NDK_HOME`, cargo-ndk takes the newest NDK it
finds. It builds with `--locked`, one codegen unit, stripped symbols and the
build's paths mapped away, so the library does not depend on the machine. For a
quicker debug build while working on the bridge,
`cd android/bridge && cargo ndk -t arm64-v8a -o ../app/src/main/jniLibs build`
still works.

Notes:

- Gradle finds the SDK via `android/local.properties` (`sdk.dir=...`, gitignored) or
  `ANDROID_HOME`.
- The generated `.so` under `jniLibs/` is gitignored (`*.so`) — every Android build
  must run step 1 first, or the APK will crash at `System.loadLibrary`.
- The APK holds only the ABIs the bridge is built for: arm64-v8a by default. For an
  emulator, build the bridge with an extra `-t x86_64` and pass
  `-Ppocketdesk.abis=arm64-v8a,x86_64` to Gradle.
- `./gradlew connectedDebugAndroidTest` uninstalls the app when it ends, which wipes
  the phone's pairings. Add `-Pandroid.injected.androidTest.leaveApksInstalledAfterRun=true`.

The bridge now exposes the full client session/streaming/input API (ported from
the Flutter-era `android/rust/src/api.rs`) via 34 `native*` JNI functions under
`dev.pocketdesk.android.bridge.RustCore`, with a typed Kotlin facade in
`android/app/.../bridge/RustCore.kt`. Fallible calls return a `{"ok":...}` /
`{"error":...}` JSON envelope; H.264 frames and Opus audio cross the boundary as
big-endian binary blobs. MediaCodec decode (feeding frames to a `Surface`) and
the Compose screens are the next step — see AGENTS.md *Current Status*.
