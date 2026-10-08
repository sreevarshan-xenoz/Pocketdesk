# Windows Host Implementation Plan

> **For agentic workers:** executed natively (superpowers:executing-plans) at the
> user's request ("run the whole plan as a goal"). Steps use checkbox syntax.

**Goal:** `linux-link.exe` runs on Windows 10/11 and the unchanged Android app
pairs with, views and controls the PC with the parity defined in the spec.

**Architecture:** Windows backends behind the existing seams (capture, encoder,
input, audio, plugins, platform helpers); Cargo features keep meaning *what* is
built, `cfg(target_os)` decides *how*.

**Tech Stack:** Rust 2024, `windows` crate (Win32 + WinRT), ffmpeg sidecar
(`h264_nvenc`/`h264_amf`/`h264_qsv`/`h264_mf`/`libx264`), quinn/iroh unchanged.

**Spec:** `docs/superpowers/specs/2026-10-08-windows-host-design.md`

## Global Constraints

- Windows target: `x86_64-pc-windows-gnu` (the box's default toolchain; WinLibs MinGW
  provides gcc/cmake). MSVC was the spec's first choice but this box has no Windows SDK
  (`kernel32.lib` absent), so MSVC cannot link here; see spec §3 amendment.
- No wire, JNI or Kotlin change.
- Linux behavior unchanged; Linux gates (fmt, both clippy profiles,
  `cargo test --workspace`) must be re-run on the Arch box before merge.
- One logical change per commit, staged by name, conventional-commit scopes.
- Windows gates per commit: `cargo fmt --all -- --check`;
  `cargo clippy --workspace --all-targets --exclude linux-link-android-bridge --exclude iroh-spike -- -D warnings`;
  `cargo test --workspace --exclude linux-link-android-bridge --exclude iroh-spike`.
- Cross-check Linux compile from this box where possible:
  `cargo check -p linux-link-core --target x86_64-unknown-linux-gnu --no-default-features --features client`
  (the full Linux server cannot be checked here — PipeWire/FFmpeg sys crates).

## Review Focus

1. A config.toml written on Linux (e.g. `capture_backend = "screencopy"`) loads on Windows and fails *at capture* with a readable reason, not at parse.
2. A phone request to an unsupported plugin gets an explicit "unsupported on this OS" reply, never silence.
3. Negative-origin secondary monitors (left of / above primary) map taps correctly.
4. Sidecar ffmpeg missing from PATH → session start reports "ffmpeg not found", not a hang.
5. Privacy hook crash/TTL → local input always comes back.

---

## Phase W0 — cfg skeleton (Windows compiles; everything unsupported)

### Task W0.1: LF line endings
- [ ] Add `.gitattributes` with `* text=auto eol=lf` (+ `*.bat text eol=crlf`, binary types).
- [ ] `git add --renormalize .` must show no content change for tracked files; commit `chore: pin LF line endings`.

### Task W0.2: Target-specific dependency tables
**Files:** `Cargo.toml`, `core/Cargo.toml`, `server/Cargo.toml`
- [ ] Move ashpd, pipewire, libspa, x11rb, wayland-*, bitflags, evdev, ffmpeg-next to `[target.'cfg(target_os = "linux")'.dependencies]` (still optional in core).
- [ ] Add `windows` (workspace dep) under `[target.'cfg(windows)'.dependencies]`.
- [ ] `encode` keeps `dep:ffmpeg-next`; on Windows it resolves to nothing.

### Task W0.3: core compiles on Windows
**Files:** `core/src/streaming/{mod.rs,capture.rs,capture_linux.rs (new, moved bodies),encoder.rs,encoder_detect.rs,audio_capture.rs,streamer.rs,capabilities.rs,bench.rs}`
- [ ] Linux-only modules gated `#[cfg(target_os = "linux")]`: capture_screencopy, shield (Wayland), encoder_inproc, encoder_vaapi, PipeWire audio body, portal/x11rb capture bodies.
- [ ] `DisplayServer::Windows`; `capture_attempts(Auto, Windows)` → error "no Windows capture backend in this build" (replaced in W1).
- [ ] Encoder: InProcess/VaapiInProcess variants linux-only; `set_nonblocking` unix-only (Windows sidecar replaced in W1).
- [ ] Unit test: `capture_attempts` returns Err for every backend on `DisplayServer::Windows` in W0.

### Task W0.4: server compiles on Windows
**Files:** `server/src/{lib.rs,input_injector.rs,privacy.rs,hyprland.rs,iroh_endpoint.rs,mic_relay.rs,audio_control.rs,notification_monitor.rs,service.rs}`, plugins as needed
- [ ] uinput backend + evdev table linux-only; enigo backend shared (works on Windows until W1's SendInput).
- [ ] privacy evdev grab linux-only; Windows stubs return `Err("unsupported on this OS")`.
- [ ] Hyprland socket code unix-only; `is_available()` false elsewhere.
- [ ] iroh key: `0o600` on unix, plain write elsewhere (DACL in W1).
- [ ] Gates green on Windows; commit per module group.

## Phase W1 — core remote desktop

- [ ] W1.1 Sidecar stdout/stderr reader threads on every OS (replaces fcntl); encoder tests.
- [ ] W1.2 `HardwareEncoder::{Amf,Qsv,MediaFoundation}`, Windows probe + arg sets + Auto order; NVENC/AMF live roundtrip tests.
- [ ] W1.3 WGC capture (`capture_wgc.rs`): monitor + HWND, BGRA VideoFrame; live test.
- [ ] W1.4 DXGI duplication fallback (`capture_dxgi.rs`); live test; `capture_attempts(Auto, Windows) = [Wgc, Dxgi]`.
- [ ] W1.5 DPI awareness at startup.
- [ ] W1.6 SendInput backend: scancode map over `PHONE_EVDEV_KEY_RANGES`, unicode text, absolute virtual-desk mapping; unit + live tests.
- [ ] W1.7 platform helpers: local_ips, hostname, secure_write (DACL), stop event, notify (toast).
- [ ] W1.8 clipboard plugin (Win32) + listener.
- [ ] W1.9 `install`/`uninstall` verbs: logon task, shortcut/AUMID, firewall; `--background` + file logs.
- [ ] W1.10 capabilities-windows.md + test; README/dev-setup/AGENTS docs.

## Phase W2 — audio
- [ ] W2.1 WASAPI loopback → PcmBuffer (convert + resample) + tests.
- [ ] W2.2 audio_control (endpoint volume, enumerate, IPolicyConfig switch).
- [ ] W2.3 mic relay into virtual cable endpoint.

## Phase W3 — plugins
- [ ] battery, power, exec, windows list/activate/close, WinEvent HUD feed, monitors, drive roots, siren.

## Phase W4 — privacy
- [ ] LL hooks + TTL watchdog, blackout window, lock; capture-excluded shield.

## Phase W5 — finish
- [ ] notification mirror (sparse MSIX identity, optional), CI windows job, release zip, checklist §22, AGENTS status.

## Owed by the user (cannot be done from this box)
- Linux gate run on the Arch box for every W0 move commit.
- Phone verification of W1–W4 exit gates.
- Push / CI first run.
