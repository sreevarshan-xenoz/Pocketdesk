# Linux Link Development Setup

This guide covers the Rust workspace and the native Android (Kotlin) client.

## 1. Prerequisites

- Rust toolchain (stable, edition 2024) + `cargo-ndk`
- JDK 17+ and Android SDK/NDK (for the Android client)
- FFmpeg, PipeWire, xdg-desktop-portal (for the server/streaming)
- FFmpeg **development** libraries (server build now links `ffmpeg-next`
  in-process): on Debian/Ubuntu `libavcodec-dev libavdevice-dev
  libavfilter-dev libavformat-dev libavutil-dev libswresample-dev
  libswscale-dev`; on Arch, the `ffmpeg` package ships them.
- For the desktop app (`desktop/`, built by `cargo build --workspace`): GTK 3,
  the Ayatana app indicator and libxdo for its tray icon. On Debian/Ubuntu
  `libgtk-3-dev libayatana-appindicator3-dev libxdo-dev`; on Arch
  `gtk3 libayatana-appindicator xdotool`. Windows needs nothing extra.

## 2. Rust Workspace Validation

From repository root:

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p linux-link-core --no-default-features --features client -- -D warnings
```

The second clippy command matters: the Android bridge builds `core` with
`default-features = false, features = ["client"]`, and that profile has
code paths the server build never compiles.

## 3. Server Run

```bash
cargo run --bin linux-link -- start
```

Optional config file location:

- `~/.config/linux-link/config.toml` (see `config.toml.example`)

Streaming QUIC congestion control can be A/B tested via the `LINUX_LINK_CC`
environment variable (`bbr`, `cubic`, or `new_reno`; default `new_reno`).
It applies to both the server and client endpoints.

```bash
LINUX_LINK_CC=bbr cargo run --bin linux-link -- start
```

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
cargo clippy --workspace --all-targets --exclude linux-link-android-bridge --exclude iroh-spike -- -D warnings
cargo test --workspace --exclude linux-link-android-bridge --exclude iroh-spike
# live tests that need this desktop / GPU (capture, encoders, input, clipboard, toast):
cargo test -p linux-link-core --lib -- --ignored wgc_ dxgi_ windows_rung_roundtrip
cargo test -p linux-link-server --lib -- --ignored sendinput_drives clipboard_round_trips a_toast_is_shown
```

Run it with `cargo run --bin linux-link -- start`, or register it at logon
with `linux-link install`. Config lives in `%APPDATA%\linux-link\config.toml`,
state (pairing, trust store, certs, logs) in `%LOCALAPPDATA%\linux-link\`.

## 5. Android Client (Kotlin + Rust JNI bridge)

- `android/app` — Kotlin/Compose app.
- `android/bridge` — Rust cdylib (`liblinux_link_android_bridge.so`) loaded via JNI by
  `dev.linuxlink.android.bridge.RustCore`.

Full local build (verified working: SDK Platform 36/37, Build-Tools 36/37, NDK 29):

```bash
# 1. Cross-compile the bridge into the app's jniLibs
cd android/bridge
cargo ndk -t arm64-v8a -o ../app/src/main/jniLibs build

# 2. Build the APK (picks the .so up automatically)
cd android
./gradlew assembleDebug   # APK: app/build/outputs/apk/debug/app-debug.apk
```

Notes:

- Gradle finds the SDK via `android/local.properties` (`sdk.dir=...`, gitignored) or
  `ANDROID_HOME`.
- The generated `.so` under `jniLibs/` is gitignored (`*.so`) — every Android build
  must run step 1 first, or the APK will crash at `System.loadLibrary`.
- Add more ABIs with extra `-t` flags (`x86_64-linux-android` target is installed for
  emulator testing).

The bridge now exposes the full client session/streaming/input API (ported from
the Flutter-era `android/rust/src/api.rs`) via 34 `native*` JNI functions under
`dev.linuxlink.android.bridge.RustCore`, with a typed Kotlin facade in
`android/app/.../bridge/RustCore.kt`. Fallible calls return a `{"ok":...}` /
`{"error":...}` JSON envelope; H.264 frames and Opus audio cross the boundary as
big-endian binary blobs. MediaCodec decode (feeding frames to a `Surface`) and
the Compose screens are the next step — see AGENTS.md *Current Status*.
