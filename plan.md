# Pocketdesk (formerly Linux Link) - Development Plan and Journal

> **Historical document.** This is the project's original plan and journal, kept for its record; it predates the rename to Pocketdesk and the Windows host. The current state of every feature is in `AGENTS.md` (Current Status) and the README.

> **A Rust-based remote desktop solution combining low-latency Wayland screen streaming (Sunshine-level) + full KDE Connect feature set + Tailscale-native connectivity**

**Target Platforms:** Arch Linux + Hyprland (server), Android (client)

**Project Status:** Active Development (server/core through Phase 6 complete; Android client migrated from Flutter to native Kotlin — scaffold + JNI bridge in place, API port in progress)

**Estimated Timeline:** 4-6 months for MVP

**Execution Snapshot (April 2026):**
- Phase 0 completed (workspace scaffold, CI, docs, build/test baseline)
- Phase 1 completed (CLI + handshake + discovery watch + two-device discovery)
- Phase 2 completed (KDE protocol runtime, all 5 plugins with real behavior, KDE Connect TCP packet loop)
- Phase 3 completed (full streaming pipeline: PipeWire capture → persistent FFmpeg encoder → QUIC transport → adaptive bitrate; 50+ integration tests)
- Phase 4 completed (Android client: Flutter UI + Rust FFI + MediaCodec video decoding + background service + all 4 screens wired)
- Phase 5 completed (systemd service, config extension, remote file browsing, latency optimization, config.toml.example)
- Phase 6 completed (CHANGELOG, GitHub Release workflow, AUR packaging, man page, install script, CONTRIBUTING.md, CI expansion)
- **Phase A (Post-Phase Additions):**
  - ✅ A1 — X11 screen capture fallback (Wayland/X11 auto-detection, `x11rb` GetImage)
  - ✅ A2 — LAN mDNS discovery fallback (`mdns-sd`, polling-based peer discovery for local networks)
  - ✅ A3 — Structured error types (`LinuxLinkError` enum with 15 variants, `From` impls)
  - ✅ A4 — E2E encryption with TOFU certificate management (`CertManager`, `TofuVerifier`, persistent identity + known peers)
- Quality gates pass (`cargo fmt`, `cargo check`, `cargo clippy -D warnings`, `cargo test` — 66 tests)

**Client Migration (September 2026):**
- Android client switched from Flutter to native Kotlin (2026-09-19): the entire Flutter app (`android/lib`, `android/rust`, pubspec) was deleted and replaced with a Compose-based Kotlin scaffold (`android/app`).
- Rust side of the bridge is now `android/bridge` — a `cdylib` JNI crate exposing `dev.linuxlink.android.bridge.RustCore` (currently a version-echo proof of wiring; session/streaming/input API is being ported on top of it).
- Sections below that describe Flutter, `flutter_rust_bridge`, `android/rust`, or Dart are historical records from the April 2026 plan; live status is in AGENTS.md.

---

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [High-Level Architecture](#high-level-architecture)
3. [Technology Stack](#technology-stack)
4. [Phase-by-Phase Implementation Plan](#phase-by-phase-implementation-plan)
5. [Detailed Execution Steps](#detailed-execution-steps)
6. [Technical Deep Dives](#technical-deep-dives)
7. [Risk Assessment & Mitigation](#risk-assessment--mitigation)
8. [Testing Strategy](#testing-strategy)
9. [Deployment & Packaging](#deployment--packaging)
10. [Appendix: Quick Reference](#appendix-quick-reference)

---

## Executive Summary

### Project Vision

Linux Link is a pioneering pure-Rust remote desktop solution that fills a critical gap in the Linux ecosystem: **secure, low-latency remote access over Tailscale with full device integration features**. Unlike existing solutions that require public port forwarding or centralized servers, Linux Link leverages Tailscale's encrypted mesh network for automatic, secure connectivity.

### Key Differentiators

| Feature | Linux Link | RustDesk | Sunshine/Moonlight | KDE Connect |
|---------|------------|----------|-------------------|-------------|
| **Tailscale-native** | ✅ Yes | ❌ No | ❌ No | ❌ No |
| **Wayland-native** | ✅ Yes | ⚠️ Partial | ⚠️ Partial | ✅ Yes |
| **Hyprland optimized** | ✅ Yes | ❌ No | ⚠️ Limited | ⚠️ Limited |
| **Full KDE Connect** | ✅ Yes | ⚠️ Partial | ❌ No | ✅ Yes |
| **<100ms latency** | ✅ Target | ⚠️ Variable | ✅ Yes | N/A |
| **Pure Rust** | ✅ Yes | ✅ Yes | ❌ C++ | ❌ C++ |
| **Open Source** | ✅ MIT/Apache | ✅ AGPL | ✅ GPL | ✅ GPL |

### Success Criteria

- [ ] **Latency:** End-to-end screen streaming latency <100ms on LAN, <200ms over internet
- [ ] **Quality:** 1080p60 streaming with adaptive bitrate
- [ ] **Security:** All traffic encrypted via Tailscale; optional PIN pairing
- [ ] **Compatibility:** Works on all Wayland compositors (Hyprland, Sway, GNOME, KDE)
- [ ] **Features:** Full KDE Connect feature parity + screen streaming
- [ ] **Usability:** One-click connection via Tailscale; no manual IP configuration

---

## High-Level Architecture

### System Overview

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         Linux Link Architecture                          │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  ┌──────────────────────┐         Tailscale P2P          ┌─────────────┐│
│  │   Android Client     │◄───────Encrypted──────────────►│   Server    ││
│  │  (Kotlin + Rust JNI)  │        (Tailscale IP)          │(Hyprland)   ││
│  ├──────────────────────┤                                ├─────────────┤│
│  │  UI Layer (Compose)  │                                │ Rust Daemon ││
│  │  ├── Connection Mgr  │                                │   (tokio)   ││
│  │  ├── Video Player    │                                ├─────────────┤│
│  │  ├── File Browser    │                                │  Core Lib   ││
│  │  └── Settings        │                                │  (shared)   ││
│  ├──────────────────────┤                                ├─────────────┤│
│  │  Rust Backend (JNI)  │                                │  Modules:   ││
│  │  ├── Protocol Handler│                                │  ├── Screen ││
│  │  ├── Video Decoder   │                                │  ├── Input  ││
│  │  ├── File Transfer   │                                │  ├── Files  ││
│  │  └── Input Mapper    │                                │  ├── KDE C. ││
│  └──────────────────────┘                                │  └── Tails. ││
│                                                          └─────────────┘│
└─────────────────────────────────────────────────────────────────────────┘
```

### Component Breakdown

#### Server (Linux/Hyprland)

| Component | Responsibility | Technology |
|-----------|---------------|------------|
| **Screen Capture** | Monitor/window capture via PipeWire | `ashpd` + `pipewire` |
| **Video Encoding** | H.264/HEVC hardware encoding | `playa-ffmpeg` (VAAPI/NVENC) |
| **Input Injection** | Keyboard/mouse event handling | `ashpd` Input Capture + `enigo` |
| **KDE Connect** | File transfer, clipboard, notifications | `kdeconnect-proto` |
| **Tailscale** | Peer discovery, secure transport | `tailscale-localapi` |
| **Hyprland IPC** | App launching, window control | `hyprland` crate |
| **DBus Integration** | Notifications, media control | `zbus` |

#### Client (Android)

| Component | Responsibility | Technology |
|-----------|---------------|------------|
| **UI Framework** | User interface | Kotlin + Jetpack Compose |
| **Rust Bridge** | FFI communication | JNI (`jni` crate, `android/bridge` cdylib) |
| **Video Decoding** | H.264 hardware decoding | MediaCodec + SurfaceView |
| **Input Handling** | Touch gestures, virtual trackpad | Compose pointer/gesture API |
| **File Access** | File picker, transfer | Storage Access Framework |
| **Background Service** | Notifications when closed | Android Foreground Service |

#### Shared Protocol

```
┌─────────────────────────────────────────────────────────────┐
│                    Linux Link Protocol                       │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  Control Channel (TCP over Tailscale)                        │
│  ├── KDE Connect packets (JSON, newline-terminated)         │
│  ├── Custom streaming commands                               │
│  └── Authentication/handshake                                │
│                                                              │
│  Streaming Channel (QUIC/UDP over Tailscale)                 │
│  ├── H.264/HEVC video frames (binary)                        │
│  ├── Input events (binary)                                   │
│  └── Audio packets (Opus, optional)                          │
│                                                              │
│  File Transfer Channel (TCP, negotiated port)                │
│  └── Large file transfers with progress                      │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### Data Flow

#### Screen Streaming Flow

```
┌─────────────┐    ┌──────────────┐    ┌─────────────┐    ┌──────────────┐
│  PipeWire   │───►│  H.264/H.265 │───►│   QUIC/UDP  │───►│  MediaCodec  │
│  (ashpd)    │    │  (playa-    │    │   (quinn)   │    │  (Android)   │
│  Frame      │    │   ffmpeg)    │    │   Stream    │    │   Decode     │
│  Capture    │    │   Encode     │    │             │    │              │
└─────────────┘    └──────────────┘    └─────────────┘    └──────────────┘
                                                                  │
                                                                  ▼
┌─────────────┐    ┌──────────────┐    ┌─────────────┐    ┌──────────────┐
│  Compose    │◄───│  SurfaceView │◄───│  Surface     │◄───│   Frame      │
│  AndroidView│    │  (MediaCodec)│    │  Texture     │    │   Buffer     │
│             │    │              │    │              │    │              │
└─────────────┘    └──────────────┘    └─────────────┘    └──────────────┘
```

#### Input Event Flow

```
┌─────────────┐    ┌──────────────┐    ┌─────────────┐    ┌──────────────┐
│  Compose    │───►│  Rust via    │───►│  QUIC/TCP   │───►│  Input       │
│  Gestures   │    │  JNI         │    │  Stream     │    │  Handler     │
└─────────────┘    └──────────────┘    └─────────────┘    └──────────────┘
                                                                  │
                                                                  ▼
┌─────────────┐    ┌──────────────┐    ┌─────────────┐    ┌──────────────┐
│   Hyprland  │◄───│  Wayland     │◄───│  XDG Input  │◄───│  Event       │
│   Compositor│    │  Input      │    │  Capture    │    │  Injection   │
│             │    │  Protocol   │    │  (ashpd)    │    │  (enigo)     │
└─────────────┘    └──────────────┘    └─────────────┘    └──────────────┘
```

---

## Technology Stack

### Server Dependencies (Cargo.toml)

```toml
[package]
name = "linux-link-server"
version = "0.1.0"
edition = "2024"

[workspace]
members = ["core", "server", "android/bridge"]

[workspace.dependencies]
# Core async runtime
tokio = { version = "1", features = ["full"] }
tokio-util = "0.7"

# XDG Desktop Portal (screen capture, input)
ashpd = { version = "0.13", features = ["screencast", "input-capture", "tokio"] }
pipewire = "0.9"
libspa = "0.9"

# Hyprland integration
hyprland = "0.3"
wayland-client = "0.31"
wayland-protocols = "0.32"
wayland-protocols-wlr = "0.3"

# KDE Connect protocol
kdeconnect-proto = "0.2"

# Video encoding
playa-ffmpeg = { version = "8.0", features = ["vaapi", "nvenc"] }
# Alternative: ffmpeg-next = "7"

# Tailscale integration
tailscale-localapi = "0.2"

# QUIC streaming
quinn = "0.11"

# DBus integration
zbus = { version = "5.7", features = ["tokio"] }

# Input handling
enigo = { version = "0.6", features = ["wayland"] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Error handling
anyhow = "1"
thiserror = "2"

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# Configuration
toml = "0.8"
dirs = "5"

# Crypto (for optional PIN)
argon2 = "0.5"
rand = "0.8"
```

### Client Dependencies (Android — Gradle Kotlin DSL)

```kotlin
// android/app/build.gradle.kts (abridged)
android {
    namespace = "dev.linuxlink.android"
    compileSdk = 36
    defaultConfig {
        applicationId = "dev.linuxlink.android"
        minSdk = 26
    }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2026.08.00")
    implementation(composeBom)
    implementation("androidx.activity:activity-compose:1.11.0")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
    // state / navigation / background service — added as the API port lands
}
```

### Client Dependencies (Rust — android/bridge/Cargo.toml)

```toml
[package]
name = "linux-link-android-bridge"

[lib]
crate-type = ["cdylib"]

[dependencies]
# Shared with server, client feature only
linux-link-core = { path = "../../core", default-features = false, features = ["client"] }
jni = "0.21"
anyhow.workspace = true
tokio.workspace = true
tracing.workspace = true

# Video decoding is done on the Android side (MediaCodec);
# the Rust bridge handles protocol/transport and feeds frames.
```

---

## Phase-by-Phase Implementation Plan

### Timeline Overview

```
Month 1          Month 2          Month 3          Month 4          Month 5          Month 6
│                │                │                │                │                │
├────────────────┤                │                │                │                │
│ Phase 0: Setup │                │                │                │                │
│ (Week 1-2)     │                │                │                │                │
└────────────────┘                │                │                │                │
                ├─────────────────┤                │                │                │
                │ Phase 1: Basic  │                │                │                │
                │ Daemon +        │                │                │                │
                │ Tailscale       │                │                │                │
                │ (Week 3-6)      │                │                │                │
                └─────────────────┘                │                │                │
                                ├──────────────────┼───────────────┤                │
                                │ Phase 2: KDE     │               │                │
                                │ Connect Features │               │                │
                                │ (Week 7-12)      │               │                │
                                └──────────────────┼───────────────┘                │
                                                   │                ├──────────────┼──────────────┤
                                                   │                │ Phase 3:     │              │
                                                   │                │ Screen       │              │
                                                   │                │ Streaming    │              │
                                                   │                │ (Week 13-24) │              │
                                                   │                └──────────────┼──────────────┘
                                                                                  │
                                                   ┌──────────────────────────────┼──────────────┐
                                                   │ Phase 4: Android Client      │              │
                                                   │ Polish                       │              │
                                                   │ (Week 19-26)                 │              │
                                                   └──────────────────────────────┼──────────────┘
                                                                                  │
                                                   ┌──────────────────────────────┼──────────────┐
                                                   │ Phase 5: Polish & Extras     │              │
                                                   │ (Week 25-28)                 │              │
                                                   └──────────────────────────────┼──────────────┘
                                                                                  │
                                                                                  ├──────────────┤
                                                                                  │ Phase 6:     │
                                                                                  │ Release &    │
                                                                                  │ Packaging    │
                                                                                  │ (Week 29-30) │
                                                                                  └──────────────┘
```

### Phase Summary

| Phase | Duration | Key Deliverables | Success Criteria |
|-------|----------|------------------|------------------|
| **Phase 0** | 1-2 weeks | Project structure, build system, CI/CD | `cargo build` succeeds, Android app runs |
| **Phase 1** | 3-4 weeks | Tailscale daemon, peer discovery, basic CLI | Two devices connect over Tailscale |
| **Phase 2** | 5-6 weeks | KDE Connect features, Android basic UI | File transfer, clipboard, notifications work |
| **Phase 3** | 8-10 weeks | Screen capture, encoding, streaming, input | <150ms latency, 30+ FPS streaming |
| **Phase 4** | 6-8 weeks | Android UI polish, gestures, background service | Production-ready Android app |
| **Phase 5** | 3-4 weeks | systemd service, config, optimizations | Auto-start works, <100ms latency |
| **Phase 6** | 2-3 weeks | AUR package, Android APK, documentation | Published on AUR + GitHub Releases |

---

## Detailed Execution Steps

> **Historical record (April 2026).** The step-by-step commands, code samples and checklists below reflect the original Flutter-based client plan. On 2026-09-19 the client was migrated to native Kotlin + a JNI bridge (`android/app`, `android/bridge`); Flutter-specific steps (`flutter create`, `flutter_rust_bridge_codegen`, `android/rust`, Dart samples) are kept as history and are no longer the way to build the client. See AGENTS.md "Current Status" for the live picture.

### Phase 0: Project Setup (Week 1-2)

#### Day 1-3: Repository & Workspace Setup

**Step 0.1: Create Repository Structure**

```bash
# Initialize repository
mkdir linux-link
cd linux-link
git init

# Create workspace structure
mkdir -p core/src
mkdir -p server/src
mkdir -p android/{rust,lib,android/app/src/main/{kotlin,res}}

# Initialize Cargo workspace
cat > Cargo.toml << 'EOF'
[workspace]
resolver = "2"
members = [
    "core",
    "server",
    "android/rust",
]

[workspace.package]
version = "0.1.0"
edition = "2024"
license = "MIT OR Apache-2.0"
repository = "https://github.com/yourusername/linux-link"
EOF

# Initialize core crate
cat > core/Cargo.toml << 'EOF'
[package]
name = "linux-link-core"
version.workspace = true
edition.workspace = true

[dependencies]
# Shared dependencies will be added as needed
EOF

# Initialize server crate
cat > server/Cargo.toml << 'EOF'
[package]
name = "linux-link-server"
version.workspace = true
edition.workspace = true

[[bin]]
name = "linux-link"
path = "src/main.rs"

[dependencies]
linux-link-core = { path = "../core" }
# Add other dependencies as we implement
EOF

# Initialize Android Rust crate
cat > android/rust/Cargo.toml << 'EOF'
[package]
name = "linux-link-android"
version.workspace = true
edition.workspace = true

[lib]
crate-type = ["cdylib"]

[dependencies]
flutter_rust_bridge = "2.12.0"
linux-link-core = { path = "../../core" }
EOF
```

**Step 0.2: Create Initial Source Files**

```bash
# Core library structure
mkdir -p core/src/{protocol,tailscale,streaming,input}

cat > core/src/lib.rs << 'EOF'
//! Linux Link Core Library
//! 
//! Shared protocol, utilities, and business logic for Linux Link.

pub mod protocol;
pub mod tailscale;
pub mod streaming;
pub mod input;

/// Protocol version for compatibility checking
pub const PROTOCOL_VERSION: u32 = 1;

/// Default ports
pub const DEFAULT_CONTROL_PORT: u16 = 1716;  // KDE Connect compatible
pub const DEFAULT_STREAMING_PORT: u16 = 4716;
EOF

# Server main entry
cat > server/src/main.rs << 'EOF'
//! Linux Link Server
//! 
//! Background daemon for remote desktop access over Tailscale.

use anyhow::Result;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod config;
mod service;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    
    tracing::info!("Linux Link Server starting...");
    
    // Load configuration
    let config = config::Config::load()?;
    
    // Start service
    service::run(config).await?;
    
    Ok(())
}
EOF

# Android Rust bridge
cat > android/rust/src/lib.rs << 'EOF'
//! Linux Link Android - Rust Backend
//! 
//! Flutter Rust Bridge interface for Android client.

use flutter_rust_bridge::frb;

#[frb]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[frb]
pub async fn connect(address: String) -> Result<ConnectionState, String> {
    // Implementation in later phases
    Ok(ConnectionState::Disconnected)
}

#[frb]
pub enum ConnectionState {
    Connected,
    Disconnected,
    Connecting,
    Error(String),
}
EOF
```

**Step 0.3: Flutter Project Setup**

```bash
cd android

# Create Flutter project (if not exists)
flutter create --org com.linuxlink --project-name linux_link_client .

# Update pubspec.yaml (see Technology Stack section)

# Install dependencies
flutter pub get

# Generate FRB bindings
cargo install flutter_rust_bridge_codegen
flutter_rust_bridge_codegen generate
```

**Step 0.4: Development Environment Setup**

```bash
# Install Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup default stable
rustup target add x86_64-unknown-linux-gnu

# For Android cross-compilation
rustup target add aarch64-linux-android armv7-linux-androideabi

# Install system dependencies (Arch Linux)
sudo pacman -S \
    pipewire \
    xdg-desktop-portal \
    xdg-desktop-portal-hyprland \
    xdg-desktop-portal-gtk \
    hyprland \
    tailscale \
    ffmpeg \
    libva \
    libva-utils

# Enable Tailscale
sudo systemctl enable --now tailscaled

# Verify Tailscale status
tailscale status
```

**Step 0.5: CI/CD Setup**

```yaml
# .github/workflows/ci.yml
name: CI

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

env:
  CARGO_TERM_COLOR: always

jobs:
  build-server:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      
      - name: Install Rust
        uses: dtolnay/rust-action@stable
      
      - name: Cache cargo
        uses: Swatinem/rust-cache@v2
      
      - name: Build server
        run: cargo build --workspace --release
      
      - name: Run tests
        run: cargo test --workspace
  
  build-android:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      
      - name: Setup Flutter
        uses: subosito/flutter-action@v2
        with:
          flutter-version: '3.24.0'
      
      - name: Install Rust
        uses: dtolnay/rust-action@stable
      
      - name: Add Android targets
        run: |
          rustup target add aarch64-linux-android armv7-linux-androideabi
      
      - name: Build Flutter APK
        run: |
          cd android
          flutter build apk --release

  lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      
      - name: Install Rust
        uses: dtolnay/rust-action@stable
      
      - name: Run clippy
        run: cargo clippy --workspace -- -D warnings
      
      - name: Run fmt
        run: cargo fmt --all -- --check
```

**Deliverables Checklist - Phase 0:**

- [x] Cargo workspace with core, server, android crates
- [ ] Flutter project with FRB integration
- [x] Basic CI/CD pipeline
- [x] Development environment documented
- [x] `cargo build` succeeds for all targets
- [ ] Flutter app runs on Android emulator

---

### Phase 1: Basic Daemon + Tailscale (Week 3-6)

#### Goal: Establish secure Tailscale connectivity between devices

#### Step 1.1: Tailscale Integration (Week 3)

**Implement Tailscale Status Checker:**

```rust
// core/src/tailscale/mod.rs
use anyhow::{Context, Result};
use tailscale_localapi::{LocalApi, Status};
use std::time::Duration;
use tokio::time::sleep;

pub struct TailscaleClient {
    api: LocalApi,
}

impl TailscaleClient {
    pub fn new() -> Result<Self> {
        let socket_path = std::env::var("TAILSCALE_SOCKET")
            .unwrap_or_else(|_| "/var/run/tailscale/tailscaled.sock".to_string());
        
        let api = LocalApi::new_with_socket_path(&socket_path)
            .context("Failed to connect to Tailscale daemon")?;
        
        Ok(Self { api })
    }
    
    /// Wait for Tailscale to be ready
    pub async fn wait_for_ready(&self, timeout: Duration) -> Result<()> {
        let start = std::time::Instant::now();
        
        loop {
            if start.elapsed() > timeout {
                anyhow::bail!("Tailscale not ready within timeout");
            }
            
            match self.api.status().await {
                Ok(status) if status.backend_state == "Running" => {
                    tracing::info!("Tailscale is connected");
                    return Ok(());
                }
                Ok(status) => {
                    tracing::debug!("Waiting for Tailscale... state: {}", status.backend_state);
                }
                Err(e) => {
                    tracing::debug!("Tailscale not ready: {}", e);
                }
            }
            
            sleep(Duration::from_secs(2)).await;
        }
    }
    
    /// Get our Tailscale IP address
    pub async fn get_self_ip(&self) -> Result<String> {
        let status = self.api.status().await?;
        
        status.self_
            .as_ref()
            .and_then(|s| s.tailscale_ips.first())
            .cloned()
            .context("No Tailscale IP assigned")
    }
    
    /// Get all peers on the tailnet
    pub async fn get_peers(&self) -> Result<Vec<PeerInfo>> {
        let status = self.api.status().await?;
        
        let peers = status.peer
            .iter()
            .map(|p| PeerInfo {
                name: p.name.clone(),
                dns_name: p.dns_name.clone(),
                ips: p.tailscale_ips.clone(),
                online: p.active,
            })
            .collect();
        
        Ok(peers)
    }
    
    /// Check if a specific peer is online
    pub async fn is_peer_online(&self, peer_name: &str) -> Result<bool> {
        let peers = self.get_peers().await?;
        Ok(peers.iter().any(|p| p.name == peer_name && p.online))
    }
}

#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub name: String,
    pub dns_name: String,
    pub ips: Vec<String>,
    pub online: bool,
}
```

**Step 1.2: Device Discovery Service**

```rust
// core/src/tailscale/discovery.rs
use super::{TailscaleClient, PeerInfo};
use anyhow::Result;
use std::collections::HashMap;
use tokio::sync::broadcast;
use tokio::time::{interval, Duration};

pub struct DiscoveryService {
    client: TailscaleClient,
    tx: broadcast::Sender<DiscoveryEvent>,
}

#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    PeerDiscovered(PeerInfo),
    PeerOffline(String),
    ServiceReady,
}

impl DiscoveryService {
    pub fn new(client: TailscaleClient) -> Self {
        let (tx, _) = broadcast::channel(100);
        Self { client, tx }
    }
    
    /// Subscribe to discovery events
    pub fn subscribe(&self) -> broadcast::Receiver<DiscoveryEvent> {
        self.tx.subscribe()
    }
    
    /// Run continuous peer discovery
    pub async fn run(&self, check_interval: Duration) {
        let mut ticker = interval(check_interval);
        let mut known_peers: HashMap<String, bool> = HashMap::new();
        
        // Initial scan
        match self.scan_peers().await {
            Ok(peers) => {
                for peer in &peers {
                    known_peers.insert(peer.name.clone(), peer.online);
                    if peer.online {
                        let _ = self.tx.send(DiscoveryEvent::PeerDiscovered(peer.clone()));
                    }
                }
            }
            Err(e) => tracing::error!("Initial peer scan failed: {}", e),
        }
        
        let _ = self.tx.send(DiscoveryEvent::ServiceReady);
        
        // Continuous monitoring
        loop {
            ticker.tick().await;
            
            match self.scan_peers().await {
                Ok(peers) => {
                    let mut current_online: HashMap<String, bool> = HashMap::new();
                    
                    for peer in peers {
                        current_online.insert(peer.name.clone(), peer.online);
                        
                        let prev_online = known_peers.get(&peer.name).copied().unwrap_or(false);
                        
                        if peer.online && !prev_online {
                            tracing::info!("Peer came online: {}", peer.name);
                            let _ = self.tx.send(DiscoveryEvent::PeerDiscovered(peer));
                        } else if !peer.online && prev_online {
                            tracing::info!("Peer went offline: {}", peer.name);
                            let _ = self.tx.send(DiscoveryEvent::PeerOffline(peer.name));
                        }
                    }
                    
                    // Check for removed peers
                    for name in known_peers.keys() {
                        if !current_online.contains_key(name) {
                            let _ = self.tx.send(DiscoveryEvent::PeerOffline(name.clone()));
                        }
                    }
                    
                    known_peers = current_online;
                }
                Err(e) => tracing::error!("Peer scan failed: {}", e),
            }
        }
    }
    
    async fn scan_peers(&self) -> Result<Vec<PeerInfo>> {
        self.client.get_peers().await
    }
}
```

**Step 1.3: Connection Manager**

```rust
// core/src/protocol/connection.rs
use anyhow::{Context, Result};
use tokio::net::TcpStream;
use std::net::SocketAddr;
use std::time::Duration;

pub struct ConnectionManager {
    timeout: Duration,
}

impl ConnectionManager {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
    
    /// Connect to a peer over Tailscale
    pub async fn connect(&self, address: &str, port: u16) -> Result<TcpStream> {
        let socket_addr = format!("{}:{}", address, port);
        
        tracing::info!("Connecting to {}...", socket_addr);
        
        let stream = tokio::time::timeout(
            self.timeout,
            TcpStream::connect(&socket_addr)
        )
        .await
        .context("Connection timeout")?
        .context("Failed to connect")?;
        
        // Configure TCP keepalive for Tailscale connections
        stream.set_keepalive(Some(Duration::from_secs(30)))?;
        stream.set_nodelay(true)?;
        
        tracing::info!("Connected to {}", socket_addr);
        
        Ok(stream)
    }
    
    /// Connect using MagicDNS hostname
    pub async fn connect_magicdns(
        &self,
        peer_name: &str,
        tailnet_name: &str,
        port: u16,
    ) -> Result<TcpStream> {
        let hostname = format!("{}.{}.ts.net", peer_name, tailnet_name);
        self.connect(&hostname, port).await
    }
}
```

**Step 1.4: Basic CLI Interface**

```rust
// server/src/cli.rs
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "linux-link")]
#[command(about = "Linux Link - Secure remote desktop over Tailscale", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
    
    /// Config file path
    #[arg(short, long, default_value = "~/.config/linux-link/config.toml")]
    pub config: String,
    
    /// Verbose output
    #[arg(short, long)]
    pub verbose: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start the server daemon
    Start,
    
    /// Stop the running daemon
    Stop,
    
    /// Show connection status
    Status,
    
    /// List available peers on tailnet
    List,
    
    /// Connect to a specific peer
    Connect {
        /// Peer name or Tailscale IP
        peer: String,
    },
    
    /// Pair with a new device (generate PIN)
    Pair {
        /// Optional PIN (generated if not provided)
        pin: Option<String>,
    },
    
    /// Show this help message
    Help,
}
```

**Step 1.5: Server Service Implementation**

```rust
// server/src/service.rs
use anyhow::{Context, Result};
use linux_link_core::tailscale::{TailscaleClient, DiscoveryService};
use linux_link_core::protocol::ConnectionManager;
use std::time::Duration;
use crate::config::Config;

pub async fn run(config: Config) -> Result<()> {
    tracing::info!("Starting Linux Link service...");
    
    // Initialize Tailscale client
    let ts_client = TailscaleClient::new()
        .context("Failed to initialize Tailscale client")?;
    
    // Wait for Tailscale to be ready
    ts_client.wait_for_ready(Duration::from_secs(30)).await?;
    
    // Get our identity
    let self_ip = ts_client.get_self_ip().await?;
    tracing::info!("Running on Tailscale IP: {}", self_ip);
    
    // Start discovery service
    let discovery = DiscoveryService::new(ts_client.clone());
    let mut rx = discovery.subscribe();
    
    // Spawn discovery task
    tokio::spawn(async move {
        discovery.run(Duration::from_secs(10)).await;
    });
    
    // Create connection manager
    let conn_mgr = ConnectionManager::new(Duration::from_secs(10));
    
    // Start TCP listener for incoming connections
    let listener = tokio::net::TcpListener::bind(format!(
        "0.0.0.0:{}",
        config.control_port
    ))
    .await?;
    
    tracing::info!("Listening on port {}", config.control_port);
    
    // Accept connections
    loop {
        tokio::select! {
            Ok((stream, addr)) = listener.accept() => {
                tracing::info!("New connection from {}", addr);
                // Handle connection in spawned task
                tokio::spawn(handle_connection(stream, conn_mgr.clone()));
            }
            
            Ok(event) = rx.recv() => {
                handle_discovery_event(event).await;
            }
        }
    }
}

async fn handle_connection(stream: TcpStream, conn_mgr: ConnectionManager) {
    // Protocol handshake and authentication
    // Implementation in Phase 2
}

async fn handle_discovery_event(event: DiscoveryEvent) {
    match event {
        DiscoveryEvent::PeerDiscovered(peer) => {
            tracing::info!("Discovered peer: {} ({})", peer.name, peer.ips.join(", "));
        }
        DiscoveryEvent::PeerOffline(name) => {
            tracing::info!("Peer offline: {}", name);
        }
        DiscoveryEvent::ServiceReady => {
            tracing::info!("Discovery service ready");
        }
    }
}
```

**Deliverables Checklist - Phase 1:**

- [x] TailscaleClient with status, peer discovery
- [x] DiscoveryService with event broadcasting
- [x] ConnectionManager for TCP connections
- [x] CLI with start/stop/status/list/connect/pair commands
- [x] Server listens for incoming connections
- [ ] Two devices can discover each other over Tailscale
- [x] Basic connection established (handshake)

Current validation: local end-to-end handshake verified with `start` + `connect 127.0.0.1 --port 1716` + `stop`.
Two-device discovery validation command: `linux-link watch --interval 10` (run on one node while toggling peer availability on another tailnet device).

---

### Phase 2: KDE Connect Features (Week 7-12)

#### Goal: Implement full KDE Connect protocol for device integration

#### Step 2.1: KDE Connect Protocol Integration

**Status: COMPLETE (April 3, 2026)**

The KDE Connect protocol runtime is fully implemented with an internal scaffold — no external `kdeconnect-proto` dependency needed for MVP.

**Implemented architecture:**

| Component | Location | Description |
|-----------|----------|-------------|
| `NetworkPacket` | `core/src/protocol/kdeconnect.rs` | Serde-based JSON packet model with `to_wire()` / `from_wire()` for newline-terminated transport |
| `DeviceIdentity` | `core/src/protocol/kdeconnect.rs` | Device metadata (name, type, capabilities) serialized as `kdeconnect.identity` packet |
| `Plugin` trait | `core/src/protocol/kdeconnect.rs` | Async trait with `handle_packet(&self, packet, sender)` — plugins reply via `DeviceSender` |
| `DeviceSender` / `TcpDeviceSender` | `core/src/protocol/kdeconnect.rs` | Per-connection sender wrapping `Arc<Mutex<WriteHalf>>` for concurrent plugin replies |
| `PluginRegistry` | `core/src/protocol/kdeconnect.rs` | Capability-indexed registry with `dispatch_packet()` routing + `clone_for_dispatch()` |
| `TrustStore` | `core/src/protocol/kdeconnect.rs` | Persisted trusted-device list (JSON file) with `trust_device()` / `untrust_device()` |
| `KdeConnectService` | `core/src/protocol/kdeconnect.rs` | Facade assembling identity + registry + trust store |
| `build_default_service()` | `server/src/kde.rs` | Constructs service with all 5 plugins registered |

**Connection handler (`server/src/service.rs`):**

```
TCP Connection
    │
    ▼
┌─────────────────────────┐
│ 1. LINUX_LINK_HELLO     │  ← Custom handshake (8s timeout)
│    → LINUX_LINK_OK      │
├─────────────────────────┤
│ 2. Send identity packet │  ← kdeconnect.identity (JSON)
├─────────────────────────┤
│ 3. Packet loop          │  ← Read lines, parse NetworkPacket,
│    dispatch → plugins   │    route via PluginRegistry::dispatch_packet()
│    (30s idle timeout)   │
└─────────────────────────┘
```

#### Step 2.2: Plugin Implementations

**Status: COMPLETE — all 5 plugins have runtime behavior**

**Battery Plugin (`server/src/plugins/battery.rs`):**
- Handles `kdeconnect.battery.request` → responds with `kdeconnect.battery`
- Reads real battery data via `gdbus` querying UPower's DisplayDevice
- Falls back to 100% / isCharging=true for desktops without batteries

**Clipboard Plugin (`server/src/plugins/clipboard.rs`):**
- Handles `kdeconnect.clipboard` → sets local clipboard
- Handles `kdeconnect.clipboard.connect` → sends current clipboard to peer
- Uses `wl-clipboard` (Wayland native) with `xclip` fallback (X11 compat)

**Notification Plugin (`server/src/plugins/notification.rs`):**
- Handles `kdeconnect.notification` → shows desktop notification via `notify-send`
- Handles `kdeconnect.notification.request` → acknowledges support
- Extracts title, text, and app name from packet body

**Share Plugin (`server/src/plugins/share.rs`):**
- Handles `kdeconnect.share.request` with `payloadTransferInfo` → binds port, receives file over TCP in background task
- Handles `kdeconnect.share.request` with `url` → logs URL and sends notification
- Saves files to `~/Downloads` (or `dirs::download_dir()`)
- Streams in 64KB chunks with progress logging

**Input Plugin (`server/src/plugins/input.rs`):**
- Handles `kdeconnect.mousepad.request` → mouse movement, button press/release, text typing, special keys
- Handles `kdeconnect.presenter` → play/pause/next/previous via key simulation
- Uses `xdotool` for all input injection
- Responds with `kdeconnect.mousepad.echo`

**Plugin runtime summary:**

| Plugin | Packet Types Handled | System Integration | Reply Sent |
|--------|---------------------|-------------------|------------|
| Battery | `kdeconnect.battery.request` | UPower via `gdbus` | `kdeconnect.battery` |
| Clipboard | `kdeconnect.clipboard`, `.connect` | `wl-clipboard` / `xclip` | `kdeconnect.clipboard` |
| Notification | `kdeconnect.notification`, `.request` | `notify-send` | (none) |
| Share | `kdeconnect.share.request` | TCP listener + `~/Downloads` | `kdeconnect.notification` (URL) |
| Input | `kdeconnect.mousepad.request`, `kdeconnect.presenter` | `xdotool` | `kdeconnect.mousepad.echo` |

#### Step 2.3: Android Client - Basic UI
use anyhow::{Context, Result};
use std::path::Path;

pub struct KDEConnectService {
    device: Device<TokioIoImpl>,
}

impl KDEConnectService {
    pub async fn new(config_path: &Path) -> Result<Self> {
        // Load or generate device config
        let config = DeviceConfig::load_or_create(
            config_path.join("cert.pem"),
            config_path.join("key.pem"),
        )
        .await?;
        
        // Create plugin registry
        let mut registry = PluginRegistry::new();
        
        // Register our plugins
        registry.register(crate::plugins::BatteryPlugin::new());
        registry.register(crate::plugins::ClipboardPlugin::new());
        registry.register(crate::plugins::NotificationPlugin::new());
        registry.register(crate::plugins::SharePlugin::new());
        registry.register(crate::plugins::InputPlugin::new());
        
        // Create trust handler
        let trust_handler = FileTrustHandler::new(config_path.join("trusted_devices.toml"));
        
        // Create device
        let device = Device::new(config, registry, trust_handler, TokioIoImpl);
        
        Ok(Self { device })
    }
    
    /// Start listening for connections
    pub async fn start(&self) -> Result<()> {
        self.device.start().await?;
        Ok(())
    }
    
    /// Wait for incoming connection
    pub async fn wait_for_connection(&self) -> Result<String> {
        let link_id = self.device.wait_for_connection().await?;
        Ok(link_id)
    }
    
    /// Pair with a device
    pub async fn pair_with(&self, link_id: &str) -> Result<()> {
        self.device.pair_with(link_id).await?;
        Ok(())
    }
    
    /// Get connected devices
    pub fn get_connected_devices(&self) -> Vec<Device> {
        self.device.connected_devices()
    }
}

/// Custom trust handler that persists to file
pub struct FileTrustHandler {
    path: std::path::PathBuf,
}

impl FileTrustHandler {
    pub fn new(path: std::path::PathBuf) -> Self {
        Self { path }
    }
}

impl TrustHandler for FileTrustHandler {
    fn is_trusted(&self, device_id: &str) -> bool {
        // Check if device is in trusted list
        // Implementation depends on storage format
        false
    }
    
    fn trust(&self, device_id: &str) -> kdeconnect_proto::Result<()> {
        // Add device to trusted list
        Ok(())
    }
}
```

#### Step 2.2: Plugin Implementations

**Battery Plugin:**

```rust
// server/src/plugins/battery.rs
use kdeconnect_proto::{
    plugin::Plugin,
    packet::NetworkPacket,
    Result,
};

pub struct BatteryPlugin {
    // Battery state tracking
    charge_level: u8,
    is_charging: bool,
}

impl BatteryPlugin {
    pub fn new() -> Self {
        Self {
            charge_level: 100, // Desktops usually don't have battery
            is_charging: true,
        }
    }
}

impl Plugin for BatteryPlugin {
    fn name(&self) -> &'static str {
        "battery"
    }
    
    fn incoming_capabilities(&self) -> &[&'static str] {
        &["kdeconnect.battery.request"]
    }
    
    fn outgoing_capabilities(&self) -> &[&'static str] {
        &["kdeconnect.battery"]
    }
    
    async fn receive_packet(&mut self, packet: NetworkPacket) -> Result<()> {
        match packet.packet_type() {
            "kdeconnect.battery.request" => {
                // Send battery status
                let response = NetworkPacket::new("kdeconnect.battery")
                    .with_body(serde_json::json!({
                        "currentCharge": self.charge_level,
                        "isCharging": self.is_charging,
                    }));
                // Send response...
            }
            _ => {}
        }
        Ok(())
    }
}
```

**Clipboard Plugin:**

```rust
// server/src/plugins/clipboard.rs
use kdeconnect_proto::{plugin::Plugin, packet::NetworkPacket, Result};
use wl_clipboard_rs::{
    copy::{MimeType, Options, Source},
    paste::{get_contents, ClipboardType},
};

pub struct ClipboardPlugin;

impl ClipboardPlugin {
    pub fn new() -> Self {
        Self
    }
    
    fn get_clipboard_content(&self) -> Result<String> {
        let contents = get_contents(ClipboardType::Regular, MimeType::Text)?;
        Ok(contents.1)
    }
    
    fn set_clipboard_content(&self, content: String) -> Result<()> {
        let mut opts = Options::new();
        opts.copy(Source::Bytes(content.into_bytes().into()), MimeType::Text)?;
        Ok(())
    }
}

impl Plugin for ClipboardPlugin {
    fn name(&self) -> &'static str {
        "clipboard"
    }
    
    fn incoming_capabilities(&self) -> &[&'static str] {
        &["kdeconnect.clipboard", "kdeconnect.clipboard.connect"]
    }
    
    fn outgoing_capabilities(&self) -> &[&'static str] {
        &["kdeconnect.clipboard"]
    }
    
    async fn receive_packet(&mut self, packet: NetworkPacket) -> Result<()> {
        match packet.packet_type() {
            "kdeconnect.clipboard" => {
                if let Some(content) = packet.body().get("content").and_then(|v| v.as_str()) {
                    self.set_clipboard_content(content.to_string())?;
                }
            }
            "kdeconnect.clipboard.connect" => {
                // Client wants clipboard sync - send current content
                if let Ok(content) = self.get_clipboard_content() {
                    let response = NetworkPacket::new("kdeconnect.clipboard")
                        .with_body(serde_json::json!({
                            "content": content,
                            "timestamp": chrono::Utc::now().timestamp_millis(),
                        }));
                    // Send response...
                }
            }
            _ => {}
        }
        Ok(())
    }
}
```

**Notification Plugin:**

```rust
// server/src/plugins/notification.rs
use kdeconnect_proto::{plugin::Plugin, packet::NetworkPacket, Result};
use zbus::{Connection, proxy};

#[proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: std::collections::HashMap<&str, zbus::zvariant::Value>,
        timeout: i32,
    ) -> zbus::Result<u32>;
}

pub struct NotificationPlugin {
    dbus_connection: Connection,
}

impl NotificationPlugin {
    pub async fn new() -> Result<Self> {
        let connection = Connection::session().await?;
        Ok(Self { dbus_connection: connection })
    }
}

impl Plugin for NotificationPlugin {
    fn name(&self) -> &'static str {
        "notification"
    }
    
    fn incoming_capabilities(&self) -> &[&'static str] {
        &["kdeconnect.notification", "kdeconnect.notification.request"]
    }
    
    fn outgoing_capabilities(&self) -> &[&'static str] {
        &["kdeconnect.notification"]
    }
    
    async fn receive_packet(&mut self, packet: NetworkPacket) -> Result<()> {
        match packet.packet_type() {
            "kdeconnect.notification" => {
                let body = packet.body();
                let title = body.get("title").and_then(|v| v.as_str()).unwrap_or("Notification");
                let text = body.get("text").and_then(|v| v.as_str()).unwrap_or("");
                
                let proxy = NotificationsProxy::new(&self.dbus_connection).await?;
                proxy.notify(
                    "Linux Link",
                    0,
                    "",
                    title,
                    text,
                    &[],
                    std::collections::HashMap::new(),
                    5000,
                ).await?;
            }
            _ => {}
        }
        Ok(())
    }
}
```

**Share Plugin (File Transfer):**

```rust
// server/src/plugins/share.rs
use kdeconnect_proto::{plugin::Plugin, packet::NetworkPacket, Result};
use tokio::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::path::PathBuf;

pub struct SharePlugin {
    download_dir: PathBuf,
}

impl SharePlugin {
    pub fn new(download_dir: PathBuf) -> Self {
        Self { download_dir }
    }
}

impl Plugin for SharePlugin {
    fn name(&self) -> &'static str {
        "share"
    }
    
    fn incoming_capabilities(&self) -> &[&'static str] {
        &["kdeconnect.share.request"]
    }
    
    fn outgoing_capabilities(&self) -> &[&'static str] {
        &["kdeconnect.share.request"]
    }
    
    async fn receive_packet(&mut self, packet: NetworkPacket) -> Result<()> {
        match packet.packet_type() {
            "kdeconnect.share.request" => {
                let body = packet.body();
                
                if let Some(payload_size) = packet.payload_size() {
                    if let Some(transfer_info) = body.get("payloadTransferInfo") {
                        let port = transfer_info.get("port").and_then(|v| v.as_u64()).unwrap_or(1739) as u16;
                        let filename = body.get("filename").and_then(|v| v.as_str()).unwrap_or("shared_file");
                        
                        // Spawn file receiver
                        tokio::spawn(Self::receive_file(
                            self.download_dir.clone(),
                            filename.to_string(),
                            payload_size,
                            port,
                        ));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl SharePlugin {
    async fn receive_file(
        download_dir: PathBuf,
        filename: String,
        size: i64,
        port: u16,
    ) -> Result<()> {
        let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
        let (mut stream, addr) = listener.accept().await?;
        
        tracing::info!("Receiving file from {} ({} bytes)", addr, size);
        
        let filepath = download_dir.join(filename);
        let mut file = tokio::fs::File::create(&filepath).await?;
        
        let mut buffer = vec![0u8; 8192];
        let mut received = 0u64;
        
        while received < size as u64 {
            let n = stream.read(&mut buffer).await?;
            if n == 0 {
                break;
            }
            file.write_all(&buffer[..n]).await?;
            received += n as u64;
            
            // Progress logging
            if received % (1024 * 1024) == 0 {
                tracing::debug!("Received {} MB", received / (1024 * 1024));
            }
        }
        
        tracing::info!("File saved to {:?}", filepath);
        Ok(())
    }
}
```

#### Step 2.3: Android Client - Basic UI

```dart
// android/lib/screens/connection_screen.dart
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import '../rust/api.dart';
import '../widgets/peer_list_tile.dart';

class ConnectionScreen extends ConsumerStatefulWidget {
  const ConnectionScreen({super.key});

  @override
  ConsumerState<ConnectionScreen> createState() => _ConnectionScreenState();
}

class _ConnectionScreenState extends ConsumerState<ConnectionScreen> {
  StreamSubscription<DiscoveryEvent>? _discoverySubscription;
  List<PeerInfo> _peers = [];
  bool _isScanning = false;

  @override
  void initState() {
    super.initState();
    _startDiscovery();
  }

  Future<void> _startDiscovery() async {
    setState(() => _isScanning = true);
    
    // Subscribe to discovery events
    _discoverySubscription = RustApi.discoveryStream().listen((event) {
      setState(() {
        switch (event) {
          case PeerDiscoveredEvent(:final peer):
            if (!_peers.any((p) => p.name == peer.name)) {
              _peers.add(peer);
            }
            break;
          case PeerOfflineEvent(:final name):
            _peers.removeWhere((p) => p.name == name);
            break;
        }
      });
    });
    
    // Initial scan
    try {
      final peers = await RustApi.scanPeers();
      setState(() {
        _peers = peers;
        _isScanning = false;
      });
    } catch (e) {
      setState(() => _isScanning = false);
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Failed to scan peers: $e')),
        );
      }
    }
  }

  @override
  void dispose() {
    _discoverySubscription?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('Linux Link'),
        actions: [
          IconButton(
            icon: const Icon(Icons.refresh),
            onPressed: _isScanning ? null : _startDiscovery,
          ),
        ],
      ),
      body: _isScanning
          ? const Center(child: CircularProgressIndicator())
          : _peers.isEmpty
              ? Center(
                  child: Column(
                    mainAxisAlignment: MainAxisAlignment.center,
                    children: [
                      Icon(Icons.devices_off, size: 64, color: Colors.grey),
                      const SizedBox(height: 16),
                      Text('No peers found', style: Theme.of(context).textTheme.titleLarge),
                      const SizedBox(height: 8),
                      Text('Make sure Tailscale is running on both devices'),
                    ],
                  ),
                )
              : ListView.builder(
                  itemCount: _peers.length,
                  itemBuilder: (context, index) {
                    final peer = _peers[index];
                    return PeerListTile(
                      peer: peer,
                      onTap: () => _connectToPeer(peer),
                    );
                  },
                ),
      floatingActionButton: FloatingActionButton(
        onPressed: _startDiscovery,
        child: const Icon(Icons.search),
      ),
    );
  }

  Future<void> _connectToPeer(PeerInfo peer) async {
    try {
      final state = await RustApi.connect(address: peer.ips.first);
      
      if (mounted) {
        switch (state) {
          case ConnectionStateConnected():
            Navigator.push(
              context,
              MaterialPageRoute(
                builder: (context) => RemoteDesktopScreen(peer: peer),
              ),
            );
            break;
          case ConnectionStateError(:final message):
            ScaffoldMessenger.of(context).showSnackBar(
              SnackBar(content: Text('Connection failed: $message')),
            );
            break;
          default:
            break;
        }
      }
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Error: $e')),
        );
      }
    }
  }
}
```

**Deliverables Checklist - Phase 2:**

- [x] Core KDE protocol scaffolding (`NetworkPacket`, identity, plugin registry, trust store)
- [x] Server plugin metadata stubs (`battery`, `clipboard`, `notification`, `share`, `input`)
- [x] CLI capabilities inspection (`linux-link capabilities`)
- [x] KDE Connect protocol runtime (packet parsing, wire format, dispatch loop)
- [x] `Plugin` trait with async `handle_packet` + `DeviceSender` abstraction
- [x] `TcpDeviceSender` for per-connection packet replies
- [x] `PluginRegistry::dispatch_packet()` routing packets to registered plugins
- [x] Battery plugin runtime (UPower via `gdbus`, fallback to 100%)
- [x] Clipboard plugin runtime (bidirectional sync via `wl-clipboard` / `xclip`)
- [x] Notification plugin runtime (desktop notifications via `notify-send`)
- [x] Share plugin runtime (file transfer over TCP, URL sharing)
- [x] Input plugin runtime (mouse/keyboard via `xdotool`, presenter remote)
- [ ] Android connection screen with peer list (Kotlin client — pending bridge API port)
- [ ] File transfer UI in Kotlin client (pending bridge API port)
- [x] End-to-end KDE Connect packet exchange validated between two devices

---

### Phase 3: Screen Streaming (Week 13-24)

**Status: COMPLETE (April 8, 2026)**

#### ✅ Completed: Full Streaming Pipeline

The streaming pipeline is fully implemented with all core components integrated into a coordinated multi-task runtime.

**What's Implemented:**

| Component | File | Status | Notes |
|-----------|------|--------|-------|
| **Module Structure** | `core/src/streaming/mod.rs` | ✅ Complete | `StreamingConfig`, `VideoFrame`, `EncodedPacket` types with H264 profiles and encoder presets; re-exports `StreamingServer`, `StreamingClient`, `LossCeiling` |
| **PipeWire Capture** | `core/src/streaming/capture.rs` | ✅ Complete | XDG Portal screencast session + PipeWire stream with `on_process` callback, BGRA frame reception, `CancellationToken` lifecycle, `CaptureSession` struct |
| **H.264 Encoder** | `core/src/streaming/encoder.rs` | ✅ Complete | Persistent FFmpeg sidecar process, stdin/stdout non-blocking I/O (via `libc::fcntl`), NAL start code parsing, IDR keyframe detection, `drain()` and `Drop` cleanup |
| **QUIC Transport** | `core/src/streaming/transport.rs` | ✅ Complete | `pub PacketHeader` (17-byte binary), `StreamServer`/`StreamClient` with quinn, `send_packets`/`receive_packets`, `NoVerifier` cert bypass |
| **Streaming Loop** | `core/src/streaming/streamer.rs` | ✅ Complete | `StreamingServer` task pipeline: capture, encode, QUIC transport, connection monitor, client-input monitor, audio, telemetry poll, bitrate arbiter |
| **Adaptive Bitrate** | `core/src/streaming/bitrate.rs` | ✅ Complete | Loss-driven ceiling (`LossCeiling`, 20 % off per congested 2 s tick to a 1 Mbit/s floor, 10 % back up when clean) folded by the server's arbiter with the relay cap and the HUD preset. The RTT-based controller this row used to describe was never wired to a connection and is deleted (roadmap 2053) |
| **Input Injection** | `server/src/input_injector.rs` | ✅ Complete | Native enigo replacing xdotool: mouse, keyboard, scroll, text input |
| **Input Plugin** | `server/src/plugins/input.rs` | ✅ Complete | Lazy-initialized injector, KDE mousepad/presenter protocol via enigo |
| **Integration Tests** | All streaming modules | ✅ 50 tests | 47 core (7 bitrate + 2 capture + 11 encoder + 10 NAL/keyframe + 4 streamer + 2 transport + 11 KDE plugins) + 3 server (input_injector) |

**Dependencies Added:**
- `ashpd` 0.13 — XDG Desktop Portal for screen capture
- `pipewire` 0.9 + `libspa` 0.9 — PipeWire client for frame reception
- `ffmpeg-sidecar` 2.5 — FFmpeg binary wrapper for persistent encoding
- `quinn` 0.11 — QUIC transport for streaming
- `rcgen` 0.14 — Self-signed certificate generation
- `rustls` 0.23 — TLS implementation
- `enigo` 0.6 — Native input injection (replaces xdotool)
- `libc` 0.2 — Non-blocking I/O via `fcntl`
- `tokio-util` 0.7 — `CancellationToken`

**Quality Gates:**
- `cargo fmt`: ✅ Pass
- `cargo clippy -D warnings`: ✅ Pass
- `cargo test`: ✅ 50 tests pass (47 core + 3 server)
- `cargo check --workspace`: ✅ Clean compilation

**Architecture Decisions Made:**
1. **ffmpeg-sidecar over playa-ffmpeg** — playa-ffmpeg had enum compatibility issues with system FFmpeg 8.0; sidecar wraps binary directly
2. **Lazy input injector initialization** — `InputPlugin::new()` no longer returns `Result`; injector created on first use to avoid startup failures on headless systems
3. **Self-signed certs for QUIC** — Production should use Tailscale identity; `NoVerifier` allows local testing without CA setup
4. **Datagram mode for streaming** — `StreamTransportConfig::use_datagrams = true` for lower latency over reliability
5. **Task-per-concern pipeline** — Capture, encode, transport, connection monitor, audio, telemetry and the bitrate arbiter run as independent tokio tasks coordinated via mpsc channels
6. **Non-blocking FFmpeg I/O** — `libc::fcntl(O_NONBLOCK)` on stdout/stderr prevents blocking when encoder has internal latency

#### 📋 Phase 3 Deliverables Checklist (Updated)

**Completed:**
- [x] Streaming module structure (`core/src/streaming/`)
- [x] PipeWire frame capture (XDG Portal session + PipeWire stream with BGRA frame callbacks)
- [x] Persistent FFmpeg encoder (sidecar process, stdin/stdout, NAL keyframe detection)
- [x] QUIC transport layer with TLS and binary packet header
- [x] Streaming loop integration (`StreamingServer` task pipeline)
- [x] Adaptive bitrate (loss-driven; the RTT controller written here in Phase 3 was dead code and is gone — see roadmap 2053)
- [x] Native input injection via enigo (replaces xdotool)
- [x] 50 integration tests across streaming + KDE plugins

**Remaining (runtime validation):**
- [ ] End-to-end streaming test on live Wayland session (requires Hyprland + PipeWire running)
- [ ] Latency measurement and optimization (<150ms target)
- [ ] FPS benchmarking (30+ FPS target)
- [ ] Android video decoder (MediaCodec) — Phase 4
- [ ] Video surface rendering on Android (MediaCodec + SurfaceView via Kotlin bridge) — Phase 4

#### 📚 Implementation Reference

The streaming module is implemented in `core/src/streaming/` with five submodules:
- **capture.rs** — `CaptureSession` with ashpd XDG Portal + PipeWire stream callbacks
- **encoder.rs** — `VideoEncoder` with persistent FfmpegChild, non-blocking I/O, NAL parsing
- **streamer.rs** — `StreamingServer` task pipeline, `StreamingClient`, the bitrate arbiter
- **transport.rs** — `StreamServer`/`StreamClient` with quinn, `PacketHeader` binary protocol
- **bitrate.rs** — `LossCeiling`, the loss term the arbiter folds in

Input injection uses enigo in `server/src/input_injector.rs` — replaces all xdotool subprocess calls.

Full code documentation with Rustdoc comments is available in the source files.

---

### Phase 4: Android Client Polish (Week 19-26)

**Status: IN PROGRESS — client migrated from Flutter to native Kotlin (September 2026)**

#### Historical: Flutter-era completion (April 2026)

By April 2026 the Flutter client was feature-complete in scaffold form: 4 screens (Connection/Remote Desktop/Files/Settings) with GoRouter + Riverpod, an FRB-based Rust API in `android/rust` (clipboard, file transfer, mouse/keyboard input, streaming stubs), a MediaCodec decoding plugin, and a foreground service. All of this was deleted on 2026-09-19 in favor of a native Kotlin client (recoverable from git at `4bf3c73`). The Flutter-era feature list remains the design reference for the Kotlin port.

#### Current: Kotlin scaffold + JNI bridge

| Component | Location | Status | Notes |
|-----------|----------|--------|-------|
| **App Scaffold** | `android/app/` | ✅ Complete | Gradle Kotlin DSL (AGP 9.4.0, Kotlin 2.3.20), Compose Material 3, `MainActivity`, manifest, `minSdk 26` |
| **Rust Bridge** | `android/bridge/` | ✅ Wired | `cdylib` JNI crate; `dev.linuxlink.android.bridge.RustCore.nativeVersion()` proves Rust→JNI→Kotlin path |
| **Connection Screen (peer list)** | — | ⬜ Pending | Port alongside bridge session API |
| **Remote Desktop Screen** | — | ⬜ Pending | Compose gestures → JNI → QUIC input stream; MediaCodec via SurfaceView |
| **File Browser Screen** | — | ⬜ Pending | SAF picker + KDE Share protocol via bridge |
| **Settings Screen** | — | ⬜ Pending | Tailscale toggle, quality, input mode |
| **Background Service** | — | ⬜ Pending | Android foreground service + notification |
| **Bridge API (clipboard/files/input/streaming)** | `android/bridge/src/` | ⬜ Pending | Replaces Flutter-era `android/rust` FRB surface |

**Quality Gates (Rust side):**
- `cargo fmt` / `cargo check --workspace`: ✅
- `cargo clippy -D warnings`: ✅ in both default and `client` feature profiles
- `cargo test`: ✅ (see AGENTS.md Current Status for count)
- Kotlin side: ✅ Verified on host (JDK 21, SDK Platform 37, NDK 29, Gradle 9.7.1 wrapper; `assembleDebug` succeeds, bridge `.so` packaged in the APK)

**Remaining Work:**
- [x] Gradle wrapper generated and `./gradlew assembleDebug` verified on host
- [ ] Port session lifecycle (connect/discover/trust) into the bridge over `linux-link-core` (`client` feature)
- [ ] Frame delivery: QUIC client → JNI → MediaCodec SurfaceView
- [ ] Input events: Compose gestures → JNI → streaming channel
- [ ] Clipboard, file transfer, notification plugins end-to-end from Android
- [ ] Full E2E testing on Android device against a live Hyprland + PipeWire server

### Phase 5: Polish & Extras (Week 25-28)

**Status: COMPLETE ✅**

**Quality Gates:**
- `cargo fmt`: ✅ Pass
- `cargo clippy -D warnings`: ✅ Pass (0 warnings across workspace)
- `cargo test`: ✅ 66 tests pass (63 core + 3 server)
- `cargo check --workspace`: ✅ Clean compilation

**Completed:**
- [x] Code review fixes (I1: timeout-based recv, I2: mounted guards, I4: error logging, I5: stop confirmation, S6: remove redundant atomic)
- [x] Configuration extension (streaming_port, log_level, video_quality with VideoQualityPreset)
- [x] systemd service with installation documentation
- [x] Remote file browsing (server plugin with path sanitization + FFI + UI; Flutter UI removed in Kotlin migration — Android side pending)
- [x] Latency optimization (StreamingStats struct, encoder preset mapping via VideoQualityPreset)
- [x] config.toml.example with all documented defaults

**Remaining (requires live hardware):**
- [x] `./gradlew assembleDebug` verification (done on host, 2026-09-19)
- [ ] E2E latency measurement on live system (requires Hyprland + PipeWire server running)

### Phase 6: Release & Packaging (Week 29-30)

**Status: COMPLETE ✅**

- [x] CHANGELOG.md with Unreleased section
- [x] GitHub Release workflow (binary builds + checksums + draft release)
- [x] AUR packaging (PKGBUILD + install script)
- [x] CI expansion (cargo audit, Flutter analyze — Flutter lint job removed in Kotlin migration)
- [x] Man page (linux-link.1)
- [x] Install script (scripts/install.sh)
- [x] CONTRIBUTING.md

**Remaining (manual action required):**
- [ ] First release tagged and published (requires manual release workflow trigger)

---

## Improvement Roadmap (September 2026 research)

Candidate pool for Phase 7+, ranked by impact-per-effort. These are proposals from a research pass (2026-09-19), not committed scope.

### R1. Connectivity — direct P2P without a Tailscale hard requirement

Today the server requires Tailscale and connects via its IPs. Research conclusion: adopt **iroh 1.x** (1.0 stable June 2026, v1.2.x current) as the default WAN transport.

**Spike done 2026-09-20 (`spike/iroh`, run with `cargo test -p iroh-spike`):** loopback connectivity confirmed on iroh 1.2 — bidi + uni stream and app-datagram echo with relays/DNS disabled (`Minimal` preset, direct `EndpointAddr` dial; `addr()` even exposed the machine's routable IPv6 for free). Key finding: **iroh 1.x no longer uses the quinn fork — it runs on `noq`**, so `iroh::endpoint::Connection`/streams are type-incompatible with core's quinn 0.11 transport code. The crates coexist in one binary, so integration needs a connection-abstraction trait over `core/src/streaming` (quinn impl for LAN, iroh impl for WAN) rather than a drop-in swap — see `spike/iroh/README.md` for the API gotchas (datagram buffer opt-in, `bind_addr` returns `Result`, mandatory `Endpoint::close`, heartbeat/MTU-discovery/multipath on by default).

**Stage 1 done 2026-09-20 — the connection abstraction itself (`core/src/streaming/connection.rs`):** `Connection`/`OutStream`/`InStream` traits carrying exactly the operations the streaming pipeline uses — `open_uni`/`accept_uni`, `write_all`/`finish`, `read_exact`/`read_to_end`, `remote_address`, and narrow `ConnectionStats { rtt, lost_packets }` — plus `ConnectionError::Closed` as the one sentinel every accept/receive loop breaks on. `QuinnConnection` wraps quinn 0.11 and classifies its error trees into that. Migrated to `SharedConnection = Arc<dyn Connection>`: server pipeline (`streamer.rs` — transport/monitor/bitrate/audio tasks), client receive + input + stats loops (`client.rs`), `StreamingServer::run_on_connection` (server `service.rs` wraps the accepted quinn conn), and the Android bridge (`StreamingHandle.connection`; the v2-multiplexer fallback path wraps at use). Deliberately quinn-native still: the endpoints themselves (`StreamServer`/`StreamClient`), the v2 multiplexer, and TLS/TOFU — stage 2 (iroh endpoint + dialer, `open_bi`/datagrams on the trait when the v2 path needs it) builds on this. Wire format unchanged; loopback round-trip covered by `connection::tests::quinn_connection_roundtrip`.


**Stage 2 done 2026-09-20 — the iroh arm of the abstraction (`core/src/streaming/iroh_connection.rs`, behind a non-default `wan` feature so the bridge build never pulls iroh):** `IrohConnection` implements `Connection`/`OutStream`/`InStream` over `iroh::endpoint::Connection` (noq 1.3). The noq API mirrors quinn 0.11 closely enough that classification is structural: same `ConnectionError` taxonomy (`ApplicationClosed`/`ConnectionClosed`/`Reset`/`TimedOut`/`LocallyClosed` → `Closed`), async `open_uni`/`accept_uni`, `write_all`, sync `finish() -> Result<(), ClosedStream>`, `read_exact`/`read_to_end(limit)`. Two genuine divergences encoded: stats RTT comes per-path (`rtt(PathId::ZERO)`), and an established connection has no single peer socket — `remote_address()` reads the selected direct path's `TransportAddr::Ip`, falling back to unspecified for relay-only paths (proper transport-address surfacing waits for the dial wiring). Verified by `iroh_connection_roundtrip` — the same loopback assertions as the quinn test (uni round-trip split across read_exact/read_to_end, `Closed` sentinel on `accept_uni` after close) over relay-less `Minimal` endpoints on 127.0.0.1. Not yet done: wiring endpoint lifecycle + dialing into `StreamingServer`/`StreamingClient` (needs the EndpointId bootstrap design — how the phone learns the desktop's iroh identity), and `open_bi`/datagrams for the v2 path.

**Stage 3 (server slice) done 2026-09-20 — WAN endpoint lifecycle + EndpointId bootstrap (`server/src/iroh_endpoint.rs`):** the server now runs one always-on iroh endpoint next to the quinn LAN listener: identity persisted as a 32-byte key at `~/.config/linux-link/iroh_secret.key` (0600, `create_new` so a racing second instance adopts the winner's key; a corrupt file is a hard error, not a silent regenerate, because changing the `EndpointId` invalidates every phone's cache). `spawn_wan_endpoint` binds `Minimal` + `RelayMode::Default` (relays for hole punching only — no pkarr/DNS address lookup, since dialing rides the announced `EndpointAddr`), sets the LAN stream budget (uni 1024/bidi 128) on the noq transport config, and feeds every accepted connection into the same `StreamingServer::run_on_connection` pipeline via `IrohConnection::shared`. Bootstrap packet: `kdeconnect.linuxlink.endpoint` `{endpointId (hex), relayUrls, directAddrs}` pushed on TCP register (next to the Hyprland state seed) and re-broadcast every 30 s so a phone's cached address self-heals through relay reconnects and DHCP. A bind failure warns and leaves LAN streaming up — the endpoint is optional at runtime.

**Stage 3 (client slice) done 2026-09-20 — the phone dials the cached identity.** Core (commit f0c005c): `IrohDial::dial(endpoint_id, relay_urls, direct_addrs, use_relays, timeout)` in `iroh_connection.rs` reassembles the announced identity into an `EndpointAddr`, binds an ephemeral `Minimal` client endpoint with the stream ALPN, connects under a timeout, and *owns* the endpoint for the session (iroh aborts its socket task when a live `Endpoint` is dropped — `close()` on teardown). `StreamingClient::attach(SharedConnection, monitor_index)` adopts the dialed connection into the exact receive/input/stats pipeline quinn's `connect()` builds. Android: the bridge now builds core with `client,wan` (iroh cross-compiles for arm64-v8a — confirmed by the `cargo ndk` build), the control reader intercepts `kdeconnect.linuxlink.endpoint` packets into a `WAN_IDENTITY` cache (single ingestion point in Rust, ahead of Kotlin's packet pollers; cleared on disconnect), `api::connect_streaming_wan` parses the cached body and installs the session through the same `install_streaming` path as LAN (WAN sessions keyed address + port-0 sentinel; `StreamingHandle.wan_dial` closed on teardown), new JNI exports `nativeConnectStreamingWan`/`nativeGetWanIdentity`. Kotlin: `RustCore.connectStreamingWan`/`getWanIdentity`, `HostStore.saveWanIdentity/wanIdentity` per-desktop cache, `RemoteScreen` polls the bridge cache every 5 s while the control channel is up, and `DecoderHost` does LAN-first with automatic WAN fallback (`connectStreaming` fails → dial cached identity) — no new UI surface; the same Connect button works from either network. Device behavior unverified (no device); real-NAT punching untested end-to-end.
| Change | Effort | Rationale |
|--------|--------|-----------|
| | | Replace raw quinn WAN path with an iroh endpoint (QUIC end-to-end, built-in hole punching via their `noq` QUIC stack) | Med-Large (spike: needs the connection abstraction, not a swap) | Bidirectional dial/punch that plain quinn lacks; works on Android through the JNI bridge |
| Demote Tailscale to an optional *candidate address*, keep mDNS for LAN | Low | Removes the install prerequisite without losing the mesh users |
| Self-host one `iroh-relay` for symmetric-CGNAT/corporate fallback | Low ops / Med | Punching succeeds on ~80-85% of home-NAT pairs; relay must exist in the design for the rest (RustDesk's hbbs/hbbr model proves this) |
| PIN/QR pairing: iroh node-id (32-byte key) doubles as identity + address, bound to the existing TOFU cert | Medium | Moonlight has zero remote-access story (manual port forwarding); this beats both Moonlight and plain RustDesk UX |
| Android: QUIC connection migration keeps Wi-Fi↔LTE a path change, not a reconnect; `connectedDevice`/`specialUse` foreground service, WorkManager backoff resume | Medium | Kills the "app died in Doze" class of bugs |

**Dead ends avoided:** DIY quinn hole punching, `hole-punch-connect` (dead, 2023), `tsnet` rust binding (dead), standalone DERP (needs a tailscaled/headscale coordination plane), libp2p dcutr (QUIC punching still rough, imports whole swarm stack), WebRTC (SCTP head-of-line; throws away the datagram video path) — reconsider WebRTC only if worst-case symmetric NATs must be traversed without hosting a relay.

### R2. Streaming pipeline — latency & quality

Ranked by impact-per-effort against the current pipeline (portal capture → FFmpeg child process → QUIC datagrams → MediaCodec). Target: Sunshine-class sub-50ms end-to-end.

| # | Upgrade | Impact / Effort | Expected gain |
|---|---------|-----------------|---------------|
| 1 | **Encoder low-latency flags now** (`tune=zerolatency`, `bf=0`, CBR + tiny VBV, `slices=4`, no lookahead; NVENC `llhp`, VAAPI `vbr_latency`) — ✅ implemented 2026-09-19: `bframes=0` + 2-frame VBV on x264/x265, `-tune ll`/`-bf 0`/`-maxrate`/`-bufsize`/GOP on NVENC, `-bf 0`/VBV/GOP on VAAPI | High / Low | 5–15 ms, no architecture change |
| 2 | **Android decode/render tuning**: `KEY_LOW_LATENCY=1` + `KEY_PRIORITY=0` (realtime), decode straight to SurfaceView (never ImageReader), `Surface.setFrameRate` to display mode — ✅ implemented 2026-09-20: `H264Decoder.kt` (MediaCodec sync decode→Surface, `LOW_LATENCY`, realtime priority) + `RemoteDesktopView.kt` (SurfaceView host, `setFrameRate` to panel refresh on API 31+) draining `RustCore.receiveFrames`. Device behavior unverified | High / Low | 5–15 ms touch-to-photon |
| 3 | **In-process encode via `ffmpeg-next` (9.0.0)**, dropping the FFmpeg child-process stdio pipe; frames stay GPU-resident with `hevc_vaapi`/`h264_nvenc` — ✅ software path 2026-09-20 (`encoder_inproc.rs`: sws BGRA→YUV420P → libx264/libx265 `AVCodecContext`, zero-latency, forced IDR, `--ignored` tests assert Annex-B); ✅ **VAAPI path 2026-09-21** (`encoder_vaapi.rs`: hand-rolled raw-FFI `av_hwdevice_ctx_create → AVHWFramesContext(NV12) → av_hwframe_get_buffer → av_hwframe_transfer_data → h264/hevc_vaapi`, device-node auto-detect + `LINUX_LINK_VAAPI_DEVICE` override, live-verified on this box's Intel iHD `renderD129`); `VideoEncoder` facade: software→in-process, `HardwareEncoder::Vaapi`→in-process-VAAPI→sidecar→software (C2). NVENC in-process stays on the verified sidecar (this box's driver predates FFmpeg 9 NVENC API-13) | High / Med | 3–8 ms + lower jitter |
| 4 | **Input path**: dedicated QUIC stream/socket with its own congestion control so input never queues behind video; host-side injection via **`/dev/uinput` absolute-coordinate virtual device** (Sunshine's approach; the `uinput` crate is dead — write thin bindings with `nix`) — ✅ absolute-input implemented 2026-09-20: `InputPacket::MouseMoveAbs{x_norm,y_norm}` (normalized 0..=65535, tag 6) + `RustCore.sendMouseAbs`/JNI export; server injects via lazily-created uinput MT direct-touch device (`PropType::DIRECT`, finger-down on first motion, left-release lifts; enigo path scales via `main_display`). Input already rides its own per-event QUIC uni streams; separate-socket CC split not done (quinn shares the connection controller) | High / Med | 10–30 ms perceived under load |
| 5 | **Transport hardening**: quinn 0.11 pluggable CC (BBR present but experimental — A/B vs CUBIC/NewReno), keep one frame ≈ one datagram set, send-only-newest + request IDR on gap. Skip FEC: QUIC datagrams already discard, Rust FEC crates are unproven (low ROI) — 🟡 implemented 2026-09-20: `LINUX_LINK_CC=bbr\|cubic\|new_reno` env selects the congestion controller via a shared `configured_transport()` (also fixes the stream-limit override bug in `StreamServer/StreamClient::new`); server transport task now sends newest-only (trims stale backlog to the last keyframe); client detects video sequence gaps and sends a rate-limited (250 ms) `InputPacket::RequestKeyframe` (tag 7) which the monitor routes to an immediate-IDR watch signal on the encoder. Remaining slice: one-frame ≈ one-datagram (datagram-mode wire format) deferred | Med / Low-Med | jitter/tail-latency |
| 6 | **VFR capture pacing**: drive from compositor vsync/damage events; encode a static screen at 5–15 fps instead of fixed 60 — ✅ implemented 2026-09-20: the Wayland/PipeWire path was already damage-driven (portal emits no buffers on a static screen); added change detection to the X11 GetImage loop (`core/src/streaming/capture.rs::run_x11_capture_loop`): an unchanged grab is not forwarded to the encoder and polling backs off to 10 fps idle rate, resuming full fps on the first changed frame; `prev_frame` only advances on successful send so encoder-backlog drops are retried | Med / Med | big CPU/GPU/queue relief when idle |
| 7 | **Zero-copy capture — bypass the portal on Hyprland**: ScreenCast portal delivers shm buffers after conversion (dmabuf-through-portal open since portal-wlr #9); the 2026 path is `ext-image-copy-capture-v1` (standardized, Hyprland-supported) with direct dmabuf `vaImport`. Tradeoff: portal-permission UX vs no-permission capture is a *product decision*. Watch `lamco-wayland` crates (active, tiny adoption); `ashpd` 0.13 and `pipewire` 0.10 are healthy | High / High | 4–10 ms at 1440p+ |

**Codec call:** keep HEVC default. Android AV1 hw decode is flagship-mainstream by 2026 but patchy mid-range, and SVT-AV1 software encode is latency-hostile; add `av1_nvenc` only as an RTX-40+ preset. Dirty-region encoding: dead end on Wayland (full frames only). Cursor prediction: skip (host cursor is already in the video).

**Caveat on #3/intra-refresh:** it removes periodic IDRs — keep a forced recovery point (CRI) or Android mid-stream join/recovery breaks.

### R3. Android client features

**Tier 1 — quick wins (the "1.0 remote desktop" batch):**
1. ✅ Latency/FPS/bitrate stats HUD overlay (`stream/StatsHud.kt` + `RustCore.streamingStats()`, 2026-09-20)
2. ✅ Touch input modes: direct-touch / trackpad (2026-09-20) — `stream/RemoteDesktopView.kt` `InputMode` + gesture overlay: DirectTouch maps taps/drags to `MouseMoveAbs` (left-release lifts the server finger), Trackpad does indirect drag + tap-click + two-finger scroll. New bridge path `sendMouseClick` (the legacy `sendMouseEvent` reserves button 0 for movement, so a wire left click was unaddressable). Mouse-direct (BT/USB mouse passthrough) and mode-switch UI remain; `RemoteDesktopView` isn't composed into a session screen yet (Tier 1 #5).
3. ✅ Shortcut key bar (`stream/ShortcutBar.kt`, 2026-09-20): Super, Alt+Tab, Ctrl+Alt+Del, PrtSc, Esc, Super+1..9 Hyprland workspace switch — required the fixed Android→evdev keycode table (commit ba26e6c)
4. ✅ Clipboard UX (2026-09-20): sync toggle + engine (`ui/ClipboardSync.kt` — pushes local changes, polls `getClipboard` every 4 s since the server never broadcasts desktop clipboard changes, `lastLocal`/`lastRemote` guards kill the echo loop); phone-side history sheet (`ui/ClipboardUi.kt` — `ClipHistory` capped at 20 in SharedPreferences, pick to restore + push); share target (`ShareActivity` — `text/plain` → desktop clipboard, `EXTRA_STREAM` → copied to cache and pushed through the KDE Connect share pull: client opens a TCP listener, server fetches into `~/Downloads`). Control channel (KDE Connect TCP 1716) threaded through `HostStore`/`ConnectScreen`/session key (`addr|stream|control`, old 2-part keys default to 1716). Device behavior unverified.
5. ✅ Auto-connect to last host + connect/disconnect/lock actions in the foreground-service notification (2026-09-20): `SessionForegroundService` (Android 14 **`specialUse`** FGS type + subtype justification — `mediaProjection` rejected: nothing is captured on-device, consent dialog would mislead), notification with Disconnect action routed through `MainActivity.onNewIntent`; `ConnectScreen` (address/port + auto-connect switch) and `RemoteScreen` (video + HUD + ShortcutBar + mode toggle, keep-screen-on, service lifecycle) wired in `MainActivity`; last host persisted in `HostStore` (SharedPreferences). `lock` notification action and PIN-pad not included yet. Device behavior unverified.
6. ✅ Pinch-zoom + display scaling for HiDPI desktops (2026-09-20) — `stream/RemoteDesktopView.kt`: `ViewportZoom` (scale 1–8, clamped pan) applied as a `graphicsLayer` on the SurfaceView (composes correctly on Android 10+); two-finger pinch/pan detector enabled in DirectTouch only (Trackpad keeps two-finger = remote scroll); a second finger cancels an in-flight direct touch before any packet is sent, and touch coords + trackpad deltas map through the inverse zoom so taps hit the desktop point under the finger. Pinch back to 1× to reset. Device behavior unverified.

**Tier 2 — differentiators:**
7. ✅ **Hyprland window picker + single-window streaming** — done 2026-09-20 (device behavior unverified): (1) `server/src/hyprland.rs` raw IPC client (j/clients, j/activewindow, j/monitors, socket2 events; verified live on 0.56.2); (2) window list over the wire — `WindowsPlugin` answers `kdeconnect.linuxlink.windows` with visible windows incl. **monitor-local `local_at`** (crop-rect origin in capture coords), `monitor_size`, and the layout `screen` box (normalized-input remap space); (3) capture crop — `InputPacket::WindowCrop` (tag 8, monitor-local rect; all-zero clears) intercepted by the stream monitor task, applied CPU-side per-frame in the encode task (`VideoFrame::crop_region`), and the encoder is **rebuilt at the resulting resolution** so the negotiated window size always reaches the pipeline (also self-heals preset-vs-capture mismatch); (4) Android — `RustCore.getWindows`/`sendWindowCrop` bridge + JNI, `ui/WindowPicker.kt` bottom sheet on the RemoteScreen ("Window: …" button), `H264Decoder` reconfigures MediaCodec at mid-session resolution changes (caches the last keyframe and re-seeds after the server's post-rebuild IDR), and `RemoteDesktopView` now renders **aspect-fit letterboxed** with direct-touch mapped through the inverse layout transform (viewport → letterbox → video px → desktop rect → normalized screen space) — fixes the wrong-point mapping when video and view aspect differ. Caveat kept: socket1 *write* dispatchers broken upstream since 0.56's Lua IPC rewrite (hyprwm/Hyprland#16224) — actions ride input injection. A moat feature: Sunshine has no per-window Linux capture. Known limitation: a crop is per-pipeline — a full session reconnect (server restarts the pipeline) resets to the whole desktop and the phone must re-pick.
8. ✅ **Workspace state HUD** — done 2026-09-20 (device behavior unverified): server pushes Hyprland socket2 events over the TCP control channel as `kdeconnect.linuxlink.hyprland.event` packets (whitelisted: activewindow, workspace, open/close/move/focus window, create/destroy/rename workspace — `server/src/hypr_events.rs`), plus a full `kdeconnect.linuxlink.hyprland.state` snapshot pushed on client register and re-broadcast every 4 s (the client's packet poller re-subscribes per drain, so snapshots make the HUD self-healing across subscription gaps/lag). `HyprlandIpc::workspaces()` (`j/workspaces`) added. Android `ui/WorkspaceHud.kt`: poll loop over `RustCore.pollIncomingPackets()`, snapshot rebuild + incremental event apply, LazyRow of workspace chips (active highlighted), numeric workspaces ≤9 switch on tap via the existing Super+N `RustCore.hotkey` path (no Hyprland IPC dispatch — broken upstream), focused-window title beside the chips. Renders under StatsHud in `RemoteScreen`; hidden entirely off-Hyprland.
9. ✅ Gamepad passthrough (landed via user commit f0e51b0): Android InputDevice → `RustCore.sendGamepadEvent` → server uinput virtual gamepad
10. ✅ **Per-monitor selection** — done 2026-09-20 (device behavior unverified): the config-stream `monitor_index` is now actually honored end-to-end. Server (commit 050feb2, `core/src/streaming/capture.rs`): Wayland/portal picks the granted PipeWire *stream* whose compositor position matches the xcap-enumerated rect for the requested index (same enumeration the monitors plugin reports → picker index == capture target), falling back to the first stream as before; X11 `GetImage`s the monitor's rect in the virtual screen instead of the whole root (monitor-sized frames flow through the R3#7 encoder-rebuild path), with the legacy whole-root grab kept when enumeration fails or the index is out of range. Android: `ui/MonitorPicker.kt` bottom sheet (lists name/size/primary from `RustCore.getMonitors`, "Auto — primary" = -1), a "Monitor: …" button on `RemoteScreen`, selection persisted per-desktop in `HostStore` and threaded `RemoteDesktopView → DecoderHost → connectStreaming/connectStreamingWan(monitorIndex)`; mid-session switching tears the stream view down and stops the bridge session *before* recreating (the bridge's same-address idempotency check would otherwise no-op the reconnect), and clears the window crop since the new pipeline starts uncropped. No bridge/JNI change. HDR: skip
11. ✅ KDE Connect parity — **done 2026-09-20** (device behavior unverified): battery HUD, find-my-device siren both ways, PIN pairing, notification reply. Battery: `RustCore.getBattery` (bridge one-shot `kdeconnect.battery.request` over the control channel, same pattern as `get_monitors`) → `StatsHud` "desk NN%"/"NN%⚡" chip polled every 15 s (commit 78def39; server's BatteryPlugin already answered the request — no server change). Siren (find-my-device, both directions): server `SirenPlugin` (`server/src/plugins/siren.rs`, 11th plugin) answers `kdeconnect.findmydevice` `{ring:true}` by looping a freedesktop sound (`alarm-clock-elapsed.oga`/`bell.oga`) through `pw-play`→`paplay` for 30 s, headless-safe; phone→desktop via `RustCore.sendFindMyDevice` + a "Ring PC" button on `RemoteScreen`; desktop→phone via the bridge control reader latching `SIREN_RINGING` (single-ingestion point like `WAN_IDENTITY`, cleared on disconnect) → `RustCore.checkSiren` consume-once JNI → `ui/Siren.kt` `SirenWatcher` polls every 2 s and plays TYPE_ALARM at max volume, looping, for 30 s. PIN pairing (12th plugin `PairPlugin`, `server/src/plugins/pair.rs`): two flows converge on `kdeconnect.pair` `{pin}` — **push**: phone sends `kdeconnect.linuxlink.pair` `{requestPin:true}`, the desktop generates a CSPRNG 6-digit PIN, shows it via `notify-send` (log fallback) and answers `pinSent`; **pull**: `linux-link pair` prints a PIN to a 5-minute-timestamped file the plugin validates against (answers `pinReady` when live). The plugin learns the phone's real deviceId from its `kdeconnect.identity` (previously a dropped packet), persists trust to the long-dormant `TrustStore`, and replies `{pair:true, serverId}` so the phone persists the desktop id (`paired_servers.json`, data dir). Enforcement: the service control loop now blocks every non-pairing packet from unpaired connections behind `config.pairing_required` (default true; `false` restores open-LAN mode); `linux-link unpair` clears the store. **QUIC paths gated too (follow-on, same day):** the v2 multiplexer mirrors the TCP whitelist per-packet (only `kdeconnect.identity`/`pair`/`linuxlink.pair` dispatch until the handshake deviceId is in the TrustStore — pairing itself stays reachable), and the video-stream path gained an in-band identity uni-stream (`[0xFE, 0x00, len, utf8 deviceId]`, sent next to the monitor-config stream by `StreamingClient::connect`/`attach`, which is why the phone announces the same persisted deviceId it pairs with); `StreamingServer::set_pairing_gate` judges it before capture starts and closes rejected connections with a "pairing required" QUIC error — enforced on the quinn LAN arm AND the iroh WAN endpoint (`spawn_wan_endpoint(…, pairing_required)`). Android: `ui/PairingSheet.kt` (Show-PIN-on-desktop w/ 2 s latch poller, or 6-digit entry), auto-opens on unpaired sessions, "Paired ✓" button state via `HostStore.pairedDesktopId`; bridge adds `request_pair_pin`/`pair_with_pin` (sync raw-socket handshake+identity, own 8 s response wait) + `PAIR_RESULT` latch in the control reader (PIN-bearing packets never forwarded to pollers) + `check_pair_result`/`paired_servers_json`. Notification reply (#11c, 13th plugin `notification_reply`): core `NetworkPacket` gained KDE-Connect-standard `source`/`replyId` fields (the TCP sender stamps `source` with its deviceId) and desktop pushes carry stable ids (`app|summary`); the bridge control reader queues desktop-originated notifications (`PENDING_NOTIFICATIONS`, deduped by id, capped at 32) → `RustCore.takePendingNotifications` → `ui/DesktopNotifications.kt` posts them as Android notifications with a platform `RemoteInput` inline reply (`NotificationReplyReceiver`, manifest-registered); replies ride `kdeconnect.notification-reply` {id,reply} back on a one-shot control connection — the server plugin logs to `replies.log`, copies the text to the desktop clipboard (wl-copy/xclip) and shows a notify-send confirmation (honest relay: a true reply into the originating app's D-Bus object isn't feasible from a plugin). 49 JNI exports.
12. ✅ **Wake-on-LAN from WAN via LAN relay** — done 2026-09-20 (mechanism complete, device behavior unverified; needs a real second always-on peer to prove the wake): broadcast UDP can't cross the wire (Tailscale's own WoL guidance is the same relay pattern), so an always-on Linux peer on the target's LAN emits the magic packet for the sleeping machine. Server: 14th plugin `WakeRelayPlugin` (`plugins/wake_relay.rs`) answers `kdeconnect.linuxlink.wol` `{mac, broadcast?, name?}` by running the existing core `tailscale::wol::send_wol` on a blocking thread (default broadcast `255.255.255.255`, directed subnet recommended) and replies `{ok, mac, error?}`; reachable only from paired connections (the `pairing_required` gate covers it). Bridge: `wake_via_relay` one-shot control connect (send-findmydevice pattern, fire-and-forget — the relay can't confirm power-on) → `nativeWakeViaRelay` → 50 JNI exports. Android: `RustCore.wakeViaRelay(address, controlPort, mac)` (address = the relay), per-desktop MAC in `HostStore`, `ConnectScreen` gains a "Wake-on-LAN MAC" field + "Send Wake-on-LAN" button — the entered host is treated as the relay, status line reports the send.

**Tier 3 — epics:**
13. ✅ **Seamless roaming hardening** — done 2026-09-20 (device behavior unverified): the shared `configured_transport()` (`core/src/streaming/transport.rs`) now sets a QUIC `keep_alive_interval` of 15 s and a 45 s `max_idle_timeout` on every endpoint — the keepalive holds the NAT/relay binding through video stalls and short radio sleeps (so the idle timeout never fires on a live-but-quiet link), while the bounded idle makes a genuinely dead peer detach in well under a minute instead of lingering. Address migration (Wi-Fi↔LTE keeping the live connection via QUIC path validation) needs no code — quinn's `ServerConfig::migration` is `true` by default, so the server already accepts a client that changes source address; a new `roaming_hardening_settings` unit test locks the two timing values. Doze strategy on the phone: `SessionForegroundService` now holds a bounded (`6 h`, re-acquired per start, released in `onDestroy`) `PARTIAL_WAKE_LOCK` so a screen-off session keeps draining frames and answering keepalives instead of being CPU-suspended into link death; `WAKE_LOCK` permission added to the manifest. Caveat: real Wi-Fi↔LTE migration also wants a client-side socket rebind on `ConnectivityManager` callbacks (Android may keep an in-flight UDP socket pinned to the old default network) — **landed 2026-09-20**: `RemoteScreen` registers a default-network callback and on a genuinely *different* new network reruns the stream teardown+reconnect (`retryStream`) behind a 3 s cooldown, swallowing the initial baseline callback and standing down in PiP (`ACCESS_NETWORK_STATE` added). That is reconnect-on-new-radio, not a live path migration: Android gives Rust no handle to move quinn's bound socket onto a `Network`, so continuity comes from a fresh QUIC handshake on the new default (LAN address if reachable, else the cached WAN identity — already tried by the connect path).
14. ✅ **PiP remote-desktop window + DeX/docked mode** — done 2026-09-20 (device behavior unverified): `MainActivity` is `supportsPictureInPicture` with `smallestScreenSize` in `configChanges` (PiP transitions no longer recreate the activity → the session survives); a "PiP" button on the session bar calls `enterPictureInPictureMode` with the *decoded video frame's* aspect ratio (`RemoteDesktopView.onVideoSizeChanged` reports the real size after the first SPS; before that, or for sizes Android would reject, the ratio is coerced into the allowed 1:2.39–2.39:1 range and the params built bare). `onPictureInPictureModeChanged` flips a Compose state that hides all session chrome (HUDs, status chip, Exit, mode/shortcut bar) so the thumbnail is pure video; input, clipboard sync, and the foreground service keep running. DeX/docked mode falls out of the same resizability (the activity is resizeable by default API 24+, so a DeX window or docked display just works — the letterboxed aspect-fit view and pinch-zoom already handle arbitrary sizes); no separate code path.
15. ✅ **Privacy modes: block local input while remote** — desktop half done 2026-09-20 (device behavior unverified): `server/src/privacy.rs` exclusively grabs every *physical* keyboard/mouse via `EVIOCGRAB` (evdev crate, already a dep) — the compositor stops seeing local input while our uinput injection path flows on; classification never touches `Linux Link Virtual *` devices or switch-only devices (lid/power stay with logind). Every activation carries a 10-min TTL with a watchdog thread auto-releasing (phone crash can't strand the machine; process exit also frees the grabs) — the phone refreshes every 4 min while enabled and releases on session exit. `lock_screen()` tries `loginctl lock-session` → `xdg-screensaver lock` → `hyprlock`. 15th plugin `PrivacyPlugin` answers `kdeconnect.linuxlink.privacy` `{action: grab|release|status, lock?}` → `{ok, grabbed, ttlLeft?, locked?, lockError?, error?}` (pairing gate covers it). Bridge `desktop_privacy` (one-shot control connect like `get_battery`, matches the reply by packet type within 5 s) → `nativeDesktopPrivacy` → 51 JNI exports. Android: "Privacy: on/off" toggle + "Lock PC" button on `RemoteScreen` with the keepalive loop and release-on-dispose. Honest gaps: grab needs read permission on `/dev/input/event*` (input group); no "someone touched locally" alert (the grab swallows events invisibly). Phone side done next: session notification carries the Tier 1 #5 carry-over **`Lock desktop`** action (→ `MainActivity.onNewIntent` → one-shot `desktopPrivacy(..., lock=true)`, control port via `EXTRA_CONTROL_PORT`). Phone-side **blackout/pocket mode** also done 2026-09-20: a "Blackout" button on `RemoteScreen` throws an opaque black, touch-consuming overlay over the whole session (double-tap or back unlocks; back can never drop a session the user can't see) while the stream keeps running underneath; during blackout the activity window gets `FLAG_SECURE` and the video `SurfaceView` is set secure (decoded frames vanish from screenshots/Recents/screen recording — SurfaceHolder has no setSecure on this SDK, `SurfaceView.setSecure` does), brightness drops to 1 %, and PiP auto-disables blackout (the float window would be a black box). Honest limit: apps cannot power the panel off — that stays a physical button, and the FGS partial wake lock already keeps the link alive through it.
16. ✅ **Desktop audio control from the phone** — done 2026-09-20 (device behavior unverified): volume/mute for the default output + routing between sinks. Deliberately **not** Hyprland `dispatch setvolume` — socket1 write dispatchers are broken upstream (hyprwm/Hyprland#16224) — so the server talks to the sound server directly: `server/src/audio_control.rs` shells out **wpctl-first (WirePlumber), pactl fallback** (`get-volume`/`set-volume`/`set-mute` on `@DEFAULT_AUDIO_SINK@`; `pactl list short sinks` + `get-default-sink` for the device list; `pactl set-default-sink` to route), with pure parse fns unit-tested. 16th plugin `AudioControlPlugin` answers `kdeconnect.linuxlink.audio` `{action: status|setVolume|setMuted|sinks|selectSink}` → `{ok, volume, muted, sinks[], default, error?}` on `spawn_blocking` (pairing gate covers it). Bridge `audio_control` (one-shot control connect, reply matched by packet type, 8 s budget — CLI subprocesses) → `nativeAudioControl` → 52 JNI exports. Android: `ui/AudioSheet.kt` bottom sheet (live volume slider, mute switch, sink list with default marker; every action re-reads state) + an "Audio" button on `RemoteScreen`. Honest gaps: phone→desktop control only — no desktop→phone volume-state push (the sheet re-polls on open/action), and no per-app (sink-input) routing UI.
17. ✅ **Compose polish (2026 Play table stakes)** — done 2026-09-20, expressive half blocked by library (device behavior unverified): **Predictive back** — `android:enableOnBackInvokedCallback="true"` on the application (explicit opt-in; default-on from targetSdk 36), and `RemoteScreen` now installs an `androidx.activity.compose.BackHandler` so the system back gesture ends the session through the same clean path as "Exit" (recompose → disposal stops streaming, releases privacy grab, tears down the FGS) instead of killing the activity — disabled in PiP where the system owns the gesture, and bottom sheets keep consuming back themselves above it. **Per-app language** — `android:localeConfig="@xml/locales_config"` (`en`) declares the locale surface the system "Languages" setting reads; translations are genuinely pending a hardcoded-strings → `strings.xml` extraction (all UI strings live in Kotlin today), so the config lists en until locales land. **Material 3 expressive** — attempted and honestly deferred: material3 **1.4.0** (BOM 2026.08.00, latest stable) keeps `MaterialExpressiveTheme` and `MotionScheme.expressive()` `internal` (verified against the artifact's Kotlin metadata — the compiler rejects both call sites), and `ModalBottomSheet`/`ModalBottomSheetProperties` expose no predictive-back sheet motion; swapping to 1.5.x would mean an alpha dependency. The expressive token swap is a one-liner (`MaterialTheme` → `MaterialExpressiveTheme` in `MainActivity`) the moment Google publishes it stable — noted in the code where the swap goes. **Remainder done 2026-09-20 (i18n extraction):** ~86 UI chrome strings moved to `res/values/strings.xml`; all screens, sheets, the FGS, `ShareActivity` and the notification relay read them via `stringResource` (composables) or `getString` (plain `Context` code) — event callbacks now hold resource IDs / raw server text in state and resolve at render time (compose-ui 1.12 has no `LocalResources`; this satisfies `LocalContextGetResourceValueCall` properly instead of suppressing it). `values-es` + `values-ta` translations shipped, `locales_config.xml` lists en/es/ta, `ConnectScreen` gained a "Language" button (API 33+ deep-link to `ACTION_APP_LOCALE_SETTINGS`, hidden below), and manifest labels (`app_name`, `share_target_label`) are resources. Telemetry units, shortcut key labels and desktop-supplied content stay untranslated (policy documented in the strings.xml header). Gates: `assembleDebug` + `lintDebug` clean, `SetTextI18n` 6→0.

**Suggested order (R1–R3, executed):** R2#1-2 (encoder flags + MediaCodec tuning — days, pure config) → Tier 1 batch → R1 connectivity (iroh spike) → window picker (R3#7-8) → then gamepad vs KDE-parity/WoL depending on target audience.

### R4. RustDesk & friends — verified research roadmap (2026-09-20)

Full findings + phase plan live in **docs/rustdesk-research-roadmap.md** (adversarially-verified deep research: RustDesk AGPL-3.0 / Sunshine GPL-3.0 = ideas only; scrcpy Apache-2.0 / IronRDP MIT = code-borrowable; QUIC/iroh direction validated; ~70% conditional / ~50% unconditional hole-punch success → relay must be a first-class, instrumented path). Phases: A telemetry/relay-first, B Hyprland-native `zwlr_screencopy` capture, C encoder capability ladder, D session UX (view-only, consent, one-time scoped PINs, protocol versioning), E unique features (window-to-phone HUD stitch, phone-mic reverse audio, compositor-true latency probe, privacy shield, link-profile presets, foldable dual-pane).

- **A1 · Link-path reporting** — done 2026-09-20 (device behavior unverified): `ConnectionStats` gained `relayed: bool` — always false for quinn (no relay concept), computed for iroh from the connection's path snapshot (`paths().iter().any(|p| p.is_selected() && p.is_relay())` — relay paths exist while punching is pending *and* while riding a relay; noq offers no `is_direct`, `is_ip()` is its equivalent). The bridge's 1 s RTT poller now maps (is_wan, relayed) → `LINK_STATE` (lan / wan_direct / wan_relayed) exposed in the `streamingStats()` JSON, logs relay↔direct transitions once (warn/info) — a relayed→direct flip is expected (iroh keeps attempting DCUtR-style upgrades), so the chip reads "WAN · relayed — trying direct…". `RemoteScreen`'s WAN badge upgraded from a static "WAN link (iroh)" to the live path (es/ta strings shipped). Loopback tests assert `relayed == false` on both quinn and iroh roundtrips. No new JNI exports (stats ride the existing JSON).
- **A2 · Session telemetry log** — done 2026-09-20 (log format unit-verified; field population needs real sessions): core-side `SessionRecorder`/`SessionGuard` (`core/src/streaming/session_telemetry.rs`) observe every `run_pipeline` execution and emit one tab-separated `k=v` line (`ts`, `outcome`, `ever_relayed`, `dur`, `rtt_ms`, `kbps`, optional `dev`) through a process-wide `OnceLock` sink. The guard is armed *before* the pairing gate and defaults to `Failed` unless the pipeline marks completion — Drop-based emission means `?`-early-exits are logged too; the gate rejection records `Rejected` explicitly. Completed runs are graded by `TransportFamily` (new `transport_family()` on the `Connection` trait, quinn→`LanDirect`) and by whether a relay path was ever sampled (`WanPunched` vs `WanRelayed`; `ever_relayed` also catches mid-session punch-through on an otherwise-relayed link). A Task-7 sampler polls `Connection::stats()` every 5 s; goodput counts bytes the *transport task actually pushed* (newest-only trims make encoder output a different number — deliberately not logged). Server (`server/src/session_telemetry.rs`): sink appends to `$XDG_STATE_HOME/linux-link/streaming_sessions.log` (1 MB rotation keeping the newest half, partial leading line dropped), `linux-link sessions [--count N]` prints the whole-log outcome tally + "relayed share: X of Y (Z%)" + the requested tail; `set_session_telemetry(true)` at both StreamingServer sites (LAN quinn + WAN iroh). Clients register no sink → recorder construction is the only cost on Android; no JNI change. 4 core + 2 server unit tests.

- **B1 · wlroots `zwlr_screencopy` capture backend** — done 2026-09-20 (**verified live on this box's Hyprland** — a unit test grabs a real frame with no portal grant and lets the idle loop run multi-cycle to prove the per-frame buffer lifecycle): new `core/src/streaming/capture_screencopy.rs` — a hand-rolled wlroots backend (wayland-client 0.31 + wayland-protocols-wlr 0.3 behind the `capture` feature; protocol patterns attributed to libwayshot, BSD-2-Clause, used as API reference only — no dependency, which keeps gl/gbm/drm out of the server). Binds `zwlr_screencopy_manager_v1` ≥ 3 (v2+ gives damage events), requests `copy_with_damage` with `overlay_cursor=1` (parity with the portal's composited-cursor frames), and reads compositor-rendered ARGB/XRGB shm — memory-order-wise already the pipeline's BGRA, so the repack is just row-flip on `YInvert` + stride compaction. The persistent part is the memfd+mmap backing (re-allocated only on geometry drift); each frame mints a fresh `wl_shm_pool`+`wl_buffer` and destroys them after `ready`, because the protocol makes the compositor destroy the buffer with the frame — reuse would break the connection. Damage events drive genuine VFR: first frame always ships, undamaged cycles encode nothing, 10 fps idle back-off mirrors the X11 path, `try_send` Full drops stale frames newest-only. Selection: `start_capture_auto`'s Wayland arm tries screencopy **first** with a 5 s first-frame handshake (`start_screencopy_capture` returns only after a real frame, so *any* setup failure — no display, non-wlroots compositor, dmabuf-only/unsupported buffer — falls back to the PipeWire portal path unchanged); `LINUX_LINK_SCREENCOPY=0` forces the portal. Monitor pick matches `wl_output` geometry origin against the xcap enumeration (same index space as the Tier-2 #10 picker), else registry position, else first output. No portal grant dialog on Hyprland = the Sunshine-style native mode from the research. 2 tests (pure repack + live capture).

- **D1 · View-only & input-lock mode** — done 2026-09-20 (device behavior unverified): `InputPacket::ViewOnly` (tag 9, `[9, enabled u8]`) + per-session latch in the streaming server. `StreamingServer` (built per connection) gained a `view_only: Arc<AtomicBool>`; the connection-monitor task — the same choke point that already intercepts keyframe/crop control packets — handles the toggle and, while latched, drops every injectable packet before the broadcast channel to `InputInjector` (which gets a no-op arm). Server-side enforcement is the point: a queued or buggy client cannot inject once latched; video, clipboard, HUDs and the crop/keyframe control plane keep flowing. Bridge `api::set_view_only` + `nativeSetViewOnly` → 53 JNI exports; Kotlin `RustCore.setViewOnly`, a "View-only: on/off" TextButton on the `RemoteScreen` session bar (amber when on, error toast on failure), and a `LaunchedEffect(streamStatus)` that re-arms the latch after any stream rebuild (retry/monitor switch starts interactive server-side). State is per-session by design — no reconnect inherits it. Wire roundtrip + strict-framing tests (2-byte exact, like RequestKeyframe).

- **A3/E5 · Relay-quality bitrate floor + full-quality override** — done 2026-09-20 (device behavior unverified): relayed iroh sessions no longer saturate shared third-party relay bandwidth. `StreamingServer` gained a per-session `full_quality: Arc<AtomicBool>` latched by new `InputPacket::FullQuality` (tag 10, strict `[10, enabled u8]`; control-plane like ViewOnly — intercepted in the monitor task, never injected, survives the view-only drop). A relay-guard task spawns **only** when `connection.transport_family() == TransportFamily::Iroh`: it samples `conn.stats().relayed` (A1's field) every 5 s and edge-triggers — while relayed (and not overridden) it pushes `min(configured, 2 Mbit/s)` onto the existing `bitrate_tx` watch channel, which the encode task applies at keyframe boundaries; punching through to direct, or releasing the override, restores the configured rate. Quinn/LAN sessions never spawn the task. Phone: bridge `api::send_full_quality` + `nativeSetFullQuality` → 54 JNI exports; `RemoteScreen` surfaces a one-tap "Full quality" toggle under the WAN badge *only while* `linkState == "wan_relayed"` (amber while on, error toast, re-armed after stream rebuild like view-only), and the relaying badge now names the behavior ("bandwidth-saving quality"). E5 partial: the per-link-state **bitrate** preset + override landed; fps/resolution/codec preset axes and a HUD preset picker remain. Wire roundtrip + strict-framing tests; en/es/ta strings; checklist §11 gained the relay-clamp checks.

- **B2 · Window-granular screencopy** — done 2026-09-21 (**verified live on this box's Hyprland** — the round-trip test streams the focused window as 918×1020 frames from a 1920×1080 output, then clears back to output capture): a client window pick is now compositor-cropped per-window frames — occlusion-correct, no wasted encode bandwidth — instead of the server software-cropping full-output BGRA. Uses **`hyprland_toplevel_export_v1`** (the protocol Hyprland actually offers for this; its frame object mirrors `zwlr_screencopy_frame_v1` event for event) with in-crate generated bindings from a vendored, trimmed XML (`core/protocols/`, BSD-3-Clause upstream) via `wayland-scanner` (+`wayland-backend`,`bitflags` in scope), all under the `capture` feature. `InputPacket::WindowCrop` (tag 8) grew a u64 `window` Hyprland address — 17→25 bytes, strict; 0 keeps pre-B2 geometry-crop behavior exactly (non-Hyprland servers never see a nonzero address reach the capture side either, since the export global is absent). The connection-monitor task routes the address onto a watch channel; the capture thread requests `capture_toplevel` and *stages* it under a 2 s timeout — Hyprland answers an invalid/gone handle by silently never creating the frame object (no `failed` event exists to wait for), so silence is the only detector and a dead handle is suspended until the client picks again (no timeout spinning). Output- and window-flavored frames share one `FramePhase` slot and one copy/destroy tail (`Capture` enum). A `window_mode: AtomicBool` reports when frames genuinely are per-window: the encode task then skips its software crop (it would re-cut the window a second time) and every output↔window crossing forces an emit + IDR (pixel geometry changed regardless of reported damage). Any failure — no global, staging timeout, copy failure — falls back to full-output frames + software crop mid-session without dropping the stream. Client side: bridge `send_window_crop` gained the address string (`parse_window_address`, unparseable → 0), `RustCore.sendWindowCrop(..., address = "")`, `RemoteScreen` passes the picker's existing `w.address`; the rect still travels (other servers + touch mapping). No new JNI exports (54). Tests: live output↔window round-trip + updated wire framing (pre-B2 17-byte frames rejected).

- **D3 · One-time scoped access PIN** — done 2026-09-21 (device behavior unverified): `linux-link pair [pin] --grant <dur>` (s/m/h/d units, combinable `1h30m`, clap-validated `parse_grant_secs`) appends a third line to the PIN file (`pin\ncreated-secs\ngrant-secs`; 2-line pre-D3 files keep working). `PairPlugin::cli_pin` returns the grant; pairing via a CLI PIN with a grant stores `TrustStore::trust_device_with_ttl` instead of permanent trust. Core TrustStore gained a `grants: {deviceId → expiry-unix-secs}` map beside `trusted_device_ids`: expiry is LAZY (expired grants filtered at `load_or_create`, and every enforcement point — TCP `is_trusted`, v2/QUIC `is_paired_device` — loads per check, so no scheduler exists to fail); permanent trust is never demoted by a grant (grant-on-permanent is a no-op), `trust_device` clears a live grant (promotion), `untrust_device` removes both, `grant_remaining` exposes TTL, `trusted_devices()` lists the union. A grant scopes *new* pairings/sessions; a live session runs to its own end. No wire/phone change (pairing over TCP only). Tests: 4 core TrustStore (expiry/legacy file/permanent-wins + promotion/untrust-both) + 2 cli parser + grant-line format; server lib 61.

- **C1 · Codec capability negotiation** — done 2026-09-21 (device behavior unverified): the client now advertises its decodable codec set before the pipeline starts — third pre-pipeline uni-stream `[0xFD, 0x00, caps u8]` (bit 0 = HEVC; H.264 assumed for all; absent stream = legacy client → H.264, no compat break). `StreamingClient::connect`/`attach` gained a `codec_caps: Option<u8>` param; the bridge passes a value Kotlin computes once via an instantiate-and-release `MediaCodec.createDecoderByType("video/hevc")` probe (any HEVC decoder → bit 0; the original `MediaCodecList`/`isDecoder` scan was revised when API 37's android.jar removed `MediaCodecInfo.isDecoder`), threaded through the existing `nativeConnectStreaming`/`Wan`/`ReconnectStreaming` exports (still 54 JNI exports, no new ones). Server: `read_client_config` reads up to three config streams and applies the pure `negotiate_codec(caps, hevc_allowed, current)` — HEVC is picked only when client caps **and** the new `Config::allow_hevc` (config.toml, default false; wired via `StreamingServer::set_hevc_allowed` at both the LAN quinn arm and `spawn_wan_endpoint`) agree; negotiated codec rides the existing `StreamingConfig.codec` into the encoder (sidecar/in-process H.265 paths already shipped in R2#3). Android decode: `H264Decoder.kt` is codec-parameterized — MediaCodec configure deferred to the first keyframe, MIME sniffed from its Annex-B NAL header (VPS/SPS nal_unit_type 32/33 + layer byte 0x01 → `video/hevc`, anything else → `video/avc`), so the decoder always matches the actual bitstream with no extra server→client message; the size-reconfigure path reuses the sniffed MIME. Honest gap: HEVC encode availability is operator responsibility (a failing HEVC open falls back via C2, next in this phase). Tests: `codec_negotiation_matrix` unit + `client_config_streams_negotiate_codec_over_loopback` quinn loopback wire round-trip (caps + monitor + deviceId all land, codec ends H265). No on-device forced-HEVC run yet (checklist §2a).

- **E2 · Phone mic → desktop PipeWire source (reverse audio)** — done 2026-09-21 (server relay **verified live on this box** — the exact production `pw-loopback` command with piped PCM produced a `linux_link_mic float32le 1ch 48000Hz` source node; phone behavior unverified): `InputPacket::Mic` (tag 11, `[11, enabled u8, len u32 LE, opus]`, strict decode: length must match exactly) rides the existing client→server uni-stream path — one QUIC stream per 20 ms Opus frame, like every other input packet. Enabled+empty payload = start/keep-alive, disabled+empty = stop. The streamer's connection-monitor task intercepts Mic **before** the view-only drop (mic is session media, not injected input — deliberately survives view-only) into a bounded `mpsc::channel(64)` with try_send drop-on-full (live audio skips, never queues). New `server/src/mic_relay.rs` per session (wired at BOTH the LAN quinn arm and the WAN iroh accept): decodes Opus with the new core `AudioDecoder` (opus feature; decode_float → interleaved f32-le) and feeds piped stdin to `pw-loopback -c 1 -m '[[MONO]]' -i 'node.name=linux_link_mic node.description="Linux Link Mic" … media.class=Audio/Source/Virtual'` — float32 because PipeWire 1.6.8 ignores `audio.format` on stdin-fed nodes (probed); node teardown on stop frame, feed error (→ 5 s spawn-retry cooldown), or channel close; kill_on_drop as the backstop. `InputInjector` gets a no-op Mic arm. Phone: the `opus` crate cannot cross-compile under cargo-ndk (CMake-built audiopus_sys; ANDROID_ABI collision), so `android/stream/MicCapture.kt` does AudioRecord 48 kHz mono s16 → MediaCodec Opus encoder (32 kbit/s, `createEncoderByType`) → `RustCore.sendMicOpus`; `MicCapture.isAvailable()` probes the codec list for an Opus *encoder* (API 28+; minSdk 26 → feature hidden). Bridge exports `startMic/sendMicOpus/stopMic` (forwarding only — core built without opus for the bridge) → 57 JNI exports. `RemoteScreen` "Mic: on/off" session-bar toggle (RECORD_AUDIO request flow, amber while on, worker-side drop clears the toggle, release on dispose); manifest gains RECORD_AUDIO + `specialUse|microphone` FGS type with the microphone type OR-ed into `startForeground` only while the grant exists (Android 14), re-issued after the grant lands mid-session. Tests: 3 wire (roundtrip/start-stop/framing) + 2 decoder (silence roundtrip, garbage reject) + 4 relay lifecycle (real `cat` child, missing-binary cooldown); mic_relay unit tests run without PipeWire. Also fixed by this change's gates: HEAD's Kotlin no longer compiled against the installed android-37.2 platform (API 37 removed `MediaCodecInfo.isDecoder` + `createEncoderByString`) — C1's HEVC probe converted to instantiate-and-release `createDecoderByType`, MicCapture uses `createEncoderByType`.
- **C2 · Runtime encoder fallback discipline** — done 2026-09-21 (device behavior unverified): three layers. Open ladder (core `encoder.rs`): a hardware sidecar child is not trusted on spawn alone — measured here, a broken-VAAPI FFmpeg survives a pure exit-poll (init failure surfaces when it processes its *first input frame*, ~700 ms in) — so `SidecarEncoder::verify_startup` feeds black probe frames and reads stdout over a 600 ms window (`DEFAULT_STARTUP_VERIFY`), passing on real output or a child alive to the deadline, failing on exit/EPIPE/closed-stdout; a failed hardware rung makes `VideoEncoder::new` warn and open the bottom rung (`degrade_to_software` = Software + H.264, in-process first) — which also means session startup on this box now *lands* on a working x264 instead of a doomed VAAPI sidecar (all 9 previously-failing `--ignored` real-encoder tests pass via the ladder). Encode-task supervisor (`streamer.rs`): sticky `encoder_preferred` config synced from `encoder.config()` after every construction (resolution rebuilds clone it, so a startup-fallback session never re-probes the dead hardware); a resolution-rebuild failure degrades to software instead of ending the session; and after 5 consecutive `encode_frame` Errs or a 2 s no-output stall (≥30 frames consumed) the task rebuilds the software rung once — the bottom rung gets a 90-error budget, then the session ends cleanly (the old arm logged an Err and looped forever against a dead encoder). Client adaptation (`H264Decoder.kt`): the C1 assumption "codec never changes mid-session" is now false, so every keyframe is re-sniffed; on a MIME change a fresh MediaCodec instance is opened, the old released, and the same IDR re-seeds it — `onCodec` threads DecoderHost → `RemoteDesktopView(onCodecChanged)` → a RemoteScreen toast "Desktop switched encoder — now H.264" (es/ta included). Honest gaps: an HEVC-capable desktop losing its hardware encoder also loses HEVC (the ladder's bottom rung is H.264-only, which is exactly what the accept-line demands), and a *same-codec* hw→sw fallback is invisible to the client (identical bitstream — server log is the only signal; a status-plane notify would need the v2 config channel). Tests: `degrade_to_software_drops_hardware_and_hevc` + `degraded_config_skips_the_hardware_probe` unit, and machine-invariant `forced_hardware_rung_lands_on_a_working_encoder` (#[ignore], spawns real encoders: forced Vaapi must yield a packet-producing encoder <5 s on any box — verified here via the fallback branch).
- **D2 · Desktop-side session consent & kick** — done 2026-09-21 (desktop-observable now; phone-side effect unverified — no device). Core gains a process-global live-session registry (`core/src/streaming/sessions.rs`): `run_pipeline` registers the accepted session (device id, peer, lan/wan family, start time) right after the pairing gate and an RAII handle deregisters on every exit path; `kick(target)` (exact device id, ≥6-char prefix, peer IP for un-announced clients, or `all`) closes the QUIC connection — so capture, encoder children and the mic relay all tear down through the pipeline's own cancel path, not a special kill switch. Server gains `live_sessions.rs` (spawned at daemon start): 1 s tick mirrors the registry to `live_sessions.json` (atomic tmp+rename) whenever it changes, raises a notify-send "a device is streaming this desktop" per *new* session (log fallback when headless — same delivery discipline as the pairing PIN), and consumes a `kick` request file (`<target>\n<unix-secs>`, deleted before acting, ignored when >60 s stale so a leftover file can't kill a future session). CLI: `linux-link kick <device|all>` writes that file (refuses when no pid file — file handoff like `pair`'s PIN, no admin socket to attack); `linux-link status` grew a "Streaming sessions (live): short-id · transport · peer · up Ns" block. Honest gaps: no interactive confirm-on-arrival (RustDesk's dialog needs a GUI prompt + accept channel — indicator + kick is the promised parity), and sessions from clients too old to announce a device id are only kickable by peer IP. Tests: 3 core registry (register/list/deregister, kick-prefix rules + close-flag via a dummy Connection, pure matches() rules) + 2 server (kick-file TTL incl. clock-skew, render_json ↔ format_status_lines roundtrip); no JNI/Kotlin change (57 exports).
- **E3 · Compositor-true E2E latency probe** — done 2026-09-21 (unit-verified; HUD numbers need a device). No new wire field and no clock sync needed: every video packet header has always carried the frame's *capture→send age* on the desktop clock (`Instant` elapsed of the capture stamp, which survives both encoder backends, and on the damage-driven B1/portal paths the capture instant IS the compositor copy moment). The core client receive loop now turns each video header into a sample — age + transport RTT/2 for the network leg — EWMA-smoothed (¾/¼, seeded by the first sample) and reset per session (`streaming::client::{e2e_sample_ms,record_e2e_sample,e2e_estimate_ms,reset_e2e_probe}`). The bridge HUD field `e2e_latency_ms` now reads that real estimate instead of the old lie (it literally proxied RTT); RustCore.kt documents the semantics. Honest limits: excludes the phone's decode→panel present and post-read queueing; network leg is an RTT/2 estimate, not timestamped one-way; the survey's pixel-reading variant (render a timestamp overlay, OCR off the decoded surface) stays open as a device-side cross-check, manual procedure in checklist §2b. Tests: sample math + EWMA seed/smooth/reset; no JNI change (57).
- **B3 · Capture backend ordering + config override** — done 2026-09-21 (ordering unit-verified; end-to-end selection needs a real session). `start_capture_auto`'s hard-coded Wayland/X11 branch became a data-driven attempt list from a pure `capture_attempts(backend, detected) -> Result<Vec<CaptureBackend>>`: `Auto` → `[screencopy, portal]` on Wayland (screencopy present only on wlroots/Hyprland; any pre-first-frame failure incl. `LINUX_LINK_SCREENCOPY=0` falls to the portal), `[x11]` on bare X11, error on headless. New `CaptureBackend` enum (`Auto` default | `Screencopy` | `Portal` | `X11`, serde lowercase) exported from `streaming`; a `capture_backend` key in config.toml pins the pipeline to a single attempt so a runtime failure surfaces instead of silently switching (the operator escape hatch when screencopy misbehaves). Explicit-but-impossible pairs rejected at selection (`screencopy` off-Wayland, `portal`/`x11` headless); `x11` on Wayland allowed with the XWayland-root-only caveat, `portal` on X11 honoured as intent. Runner clones the cheap `frame_tx`/`cancel`/`window_rx`/`window_mode` per attempt (only screencopy consumes the window watch/Arc). Threaded via `StreamingServer::set_capture_backend` (field defaults `Auto`) + `Config::capture_backend`, applied at BOTH the LAN `service.rs` construction and `spawn_wan_endpoint` — mirroring the `set_hevc_allowed` clone-per-connection pattern. No phone/JNI change (57). Tests: 3 core (auto ordering per display server, explicit single-pin, impossible-combo errors) + 1 server (TOML default/override/unknown-rejects).
- **D4 · Protocol versioning discipline** — done 2026-09-21 (wire format unit-verified; a tagged byte stream needs a live desktop+phone pair). KDE Connect's doc is a self-disclaimed non-spec, so we version the packets we own and stay defensive about the rest. A new optional top-level `llVersion` field on `NetworkPacket` is auto-stamped by `to_wire()` — the single send choke point every server + bridge path goes through — on any packet whose type starts with `kdeconnect.linuxlink.` (new `LL_EXT_VERSION` const + `LINUXLINK_PACKET_PREFIX`), so no construction site has to remember it and KDE's native types (`kdeconnect.pair`, `kdeconnect.clipboard`, …) stay untagged; deliberately separate from the identity handshake `protocolVersion`, and it only bumps for a change to an existing field's *meaning* (additive fields are forward-compatible by design). Defensive parsing was already true (`body` is an opaque `Value`, `deny_unknown_fields` used nowhere) and is now locked by a regression test that feeds a future-shaped packet (extra `llVersion` + unknown `futureTopLevel`) and confirms known fields still parse. The one convention violator — `kdeconnect.notification-reply` (hyphen, rejected by KDE's `[a-z_]+` type regex) — moved to `kdeconnect.linuxlink.notification_reply`: a conformant `[a-z_]` name *and* our own namespace, so it now rides the `llVersion` tag too; it was never a real KDE type we interoperate with and both ends ship together (bridge send + server plugin capability string), so no compat shim. Tests: 4 core (tagged / native-untagged / explicit-version-preserved / unknown-field-tolerance) + 3 server under the new name; no new JNI exports (57), Kotlin change is a doc comment.
- **C3 · NVENC high-power pin** — done 2026-09-21 (**hardware-gated: no NVIDIA GPU on this box — the pure command-shape/parse logic is unit-verified; the `nvidia-smi` subprocess paths have never executed here and need a real GPU + privilege**). NVENC's default adaptive power management downclocks the GPU between bursty encode frames, so a session's first frames and any idle gap pay a clock ramp (latency jitter). New `core/src/streaming/nvenc_power.rs`: an RAII `NvencPowerGuard` that queries GPU 0's max graphics clock (`nvidia-smi -i 0 --query-gpu=clocks.max.graphics --format=csv,noheader,nounits`), enables persistence mode (`-pm 1`), and locks clocks to that ceiling (`--lock-gpu-clocks=MAX,MAX`) on acquire; `Drop` runs `--reset-gpu-clocks`. Every failure is a self-declining no-op with a log line — no `nvidia-smi` binary (not NVIDIA), unparsable clock (`N/A`/empty/0), or a refused lock (unprivileged) → session runs unpinned and, critically, a lock we never took is never "reset" (the guard is only constructed on a successful lock). Wired as an `Option<NvencPowerGuard>` field on `SidecarEncoder`, acquired in `new()` only when `config.hardware_encoder == Nvenc`, so it spans exactly the encoder's lifetime (session end and a C2-degraded dropped encoder both restore). VAAPI/software/this-box builds never construct it. Server-only (`encode` feature); no wire/JNI/Kotlin change (57 exports). Tests: 3 unit (`parse_max_clock` bare/`mhz`-suffixed/whitespace vs `N/A`/empty/0/non-numeric, and the four command builders all targeting `-i 0` + `--lock-gpu-clocks`/`--reset-gpu-clocks` shapes); subprocess paths are desktop/device gated like C4.
- **C4 · In-process VAAPI `AVHWDeviceContext` encode** — done 2026-09-21 (**verified live on this box** — the whole R2#3 "parked hardware in-process" slice). New `core/src/streaming/encoder_vaapi.rs`: a `VaapiEncoder` driven by hand-rolled raw FFmpeg FFI (ffmpeg-next 9.0 has *no* safe hwdevice bindings), mirroring the C2 fallback discipline: `av_hwdevice_ctx_create` → `av_hwframe_ctx_alloc` + fill `AVHWFramesContext`(format=VAAPI, sw_format=NV12, pool) → `av_hwframe_ctx_init` → set `AVCodecContext.hw_device_ctx`/`hw_frames_ctx` via `av_buffer_ref` → `avcodec_open2(h264_vaapi/hevc_vaapi)`; per frame: BGRA→NV12 `sws_scale` (reusing the `InProcessEncoder` `copy_frame` pattern) → `av_hwframe_get_buffer` → `av_hwframe_transfer_data` (hwupload) → force IDR via `AVFrame.pict_type` → `avcodec_send_frame`/`avcodec_receive_packet` (Annex-B out, `AV_PKT_FLAG_KEY` ∪ `detect_keyframe`). RAII `Drop` frees packet/frame/ctx and unrefs both buffer refs. **The real root cause was NOT "this box can't VAAPI"** (the AGENTS gotcha was wrong): FFmpeg binds a *specific* DRM render node, and on this hybrid Intel+NVIDIA laptop `/dev/dri/renderD128` is the NVIDIA card (no VA driver → `Failed to initialise VAAPI connection`) while the Intel **iHD** driver that encodes H.264/HEVC is `/dev/dri/renderD129`. `open_vaapi_device` therefore **tries each `/dev/dri/renderD*` and keeps the first that opens**, with a `LINUX_LINK_VAAPI_DEVICE` env override to pin one; the pure `vaapi_device_candidates` ordering is unit-tested. `VideoEncoder` gained a `VaapiInProcess` variant and routes `HardwareEncoder::Vaapi` in-process-first, degrading to the verified sidecar then software (same ladder). The **sidecar's own hardcoded `renderD128` was the matching bug** — `build_ffmpeg_args` now uses `vaapi_device_node()` (override → lowest present node), so a sidecar-VAAPI fallback also picks the right GPU. NVENC in-process stays on the verified sidecar (this box's 580.x driver predates FFmpeg 9's NVENC API-13 requirement — genuinely driver-gated). Server-only (`encode` feature); **no wire/JNI/Kotlin change (still 58 exports)**. Tests: 5 unit (candidate ordering incl. the "only renderD129 present" hybrid case + override + absent-node, `AVERROR_EOF` value) plus `test_vaapi_encode_roundtrip` which encodes 40 real frames through `h264_vaapi` here and asserts Annex-B start codes + ≥1 keyframe, self-skipping on boxes with no VA-capable node (so CI/NVIDIA-only hosts pass green). Gates: `cargo test --workspace` (core 177, server 68), `-p linux-link-core --features encode` clippy clean on the new files, core `client` + `client,wan` profiles clean (module `cfg(encode)`-excluded there), `cargo fmt` on touched files; workspace clippy unchanged at the pre-existing 7-error baseline (`capture.rs`/`capture_x11.rs`/`streamer.rs` debt, none in encoder files).
- **E1 · "Pull window to phone" gesture** — done 2026-09-21 (device behavior unverified — no device). The R3#7 window-crop pipeline and the R3#8 Workspace HUD existed separately; this stitches them so a tap on a window chip *in the live HUD* pulls that window into the stream — no bottom-sheet round-trip. Server (`hypr_events.rs::state_packet`): the `hyprland.state` snapshot's `windows[]` went from address/workspace/class/title to the full crop-capable shape by reusing the windows plugin's `entries()` + `screen_box()` (now `pub(crate)`) — every window now carries global `at`, monitor-local `local_at`, `size`, `monitor_size`, and the body grows the `screen` layout box, so the HUD has everything the picker's separate `getWindows` RPC had without a second request. The `active` flag is derived phone-side from `activeAddress`. Android: `WorkspaceHud` gained a window-chip row (`parseState` reads the enriched windows + screen; `desktopWindowFromJson` shared out of `WindowPicker` so both parsers agree on field names); tapping a chip routes through `RemoteScreen`'s new `pullWindow` lambda, which is the exact crop logic the picker's `onPick` already ran (`RustCore.sendWindowCrop(localAt, size, address)` + sets `cropWindow`/`cropScreen` for touch remapping) — the picker's `onPick` now calls the same lambda (single source). Re-tapping the streamed chip (highlighted solid, mirroring the active-workspace style) returns to the whole-desktop view; the roadmap's "swipe away" is realised as this re-tap because the chip row is horizontally scrollable and a swipe would fight the scroll. No new JNI exports (57), no wire format change (`sendWindowCrop` already carries the B2 window address). Gates: server lib tests (+1 live `state_packet_windows_carry_crop_geometry` asserting `screen` + per-window `local_at`/`size`, skipped off-Hyprland), `cargo test --workspace`, server clippy (only the pre-existing `monitor_of` needless-lifetimes in the touched file, unchanged from HEAD), rustfmt, `assembleDebug` + `lintDebug` clean.

- **E5-remainder · HUD link-profile presets** — done 2026-09-21 (device behavior unverified): the phone can one-tap a named quality profile. New `InputPacket::QualityPreset` (tag 12, strict `[12, preset u8]`; control-plane like FullQuality — latched in the monitor task, never injected, survives the view-only drop) carries an id in `PRESET_AUTO|QUALITY|BALANCED|ECONOMY` whose **bitrate ceiling** (`preset_bitrate_ceil`: 8 M-band auto, Balanced ≤5 Mbit/s, Economy ≤1.5 Mbit/s, expressed as a `min` against the session's configured rate so native resolution is never dropped — only bandwidth) is shared client↔server as a pure fn. Server side folds A3 + E5 into a single **bitrate arbiter** task that now spawns for **every** transport (quinn LAN included, where a preset is genuinely useful — the old relay-guard was iroh-only): it samples `conn.stats().relayed` + the `full_quality`/`preset` latches every 2 s and computes `configured.min(relay_cap).min(preset_ceil)`, pushing to `bitrate_tx` on edge. The encode task's bitrate-watch arm, which previously only logged (so an A3 relay clamp materialised lazily at the next resolution rebuild), now **rebuilds the encoder at the current frame resolution with the new rate** — reusing the sticky `encoder_preferred` (C2 discipline: never re-probes a failed hw rung) and emitting IDR first. **Honest axis scope:** a live preset moves **bitrate** only — resolution and fps are capture-loop parameters fixed at pipeline start, and a mid-session codec change needs the client to reconfigure MediaCodec (C1/C2 re-sniff); those axes remain session-start choices (the phone's existing retry/monitor-switch restarts the pipeline). Phone: bridge `api::send_quality_preset` + `nativeSendQualityPreset` → 58 JNI exports; `RemoteScreen` session bar gained a cycling "Quality: auto/max/balanced/economy" TextButton (blue while off-Auto, error toast, re-armed on stream re-Up like view-only). en/es/ta strings. Tests: preset roundtrip + strict 2-byte framing + `preset_bitrate_ceil` cap-relative-to-configured. Gates: `cargo test --workspace`, `-p linux-link-core --features wan`, core `client` + `client,wan` profiles clean, workspace clippy at the 7-error baseline, bridge clippy `-D warnings`, ndk arm64 + JNI parity 58/58, `assembleDebug` + `lintDebug`.

- **E4 · Compositor privacy shield** — done 2026-09-21 (**verified live on this Hyprland box**): while a session drives the desktop, bystanders now see it. New `core/src/streaming/shield.rs` (under the `capture` feature) paints a full-perimeter "session active" frame via `zwlr_layer_shell_v1` — a transparent-centre, opaque-red (`SHIELD_ARGB`) ring, `Layer::Overlay`, all-edge anchor, `KeyboardInteractivity::None` (click-through), so the desktop stays fully visible and the remote keeps working while the signal reads to anyone at the machine. `PrivacyShield::show()` returns `None` (no side effects) when there is no Wayland display or the compositor doesn't advertise `zwlr_layer_shell_v1`/`wl_shm` (GNOME/KDE/headless/X11) — same defensive posture as the B1 backend. Geometry is a pure unit-tested fn (`render_shield`/`is_ring`/`shield_thickness`); an `#[ignore]`d live test maps the layer and asserts the edge pixels through the B1 screencopy backend (this is what makes it box-verifiable, not device-gated). Wired into the Tier-3 #15 lifecycle in `server/src/privacy.rs`: `GrabState` gained `shield: Option<PrivacyShield>`, shown on the first successful `EVIOCGRAB` and dropped (unmapped) on `release_input`/TTL expiry — so the phone's **existing Privacy: on/off toggle** controls it end to end. **No wire packet, no new JNI (still 58), no Kotlin change.** Honest limits: the input-block half stays `EVIOCGRAB` (this Hyprland does *not* advertise `zwp_input_inhibitor_v1`, so the compositor-native invisible-block is intentionally not added — the grab already works); the ring is text-free (a font rasteriser is deliberately not pulled in); frame geometry is fixed at `show()` (a mid-session resolution change is acked, not re-rendered). Gates: `cargo test --workspace` (core 172, server 68) + `shield::` unit tests, `-p linux-link-core --features wan`, core `client` + `client,wan` profiles clean, workspace clippy at the 7-error baseline (new-file lints fixed), capture-profile clippy clean on `shield.rs`, rustfmt, ndk arm64 bridge build; shield live test run explicitly and green.

- **E6 · Foldable/tablet dual-pane** — done 2026-09-21 (device behavior unverified — no foldable attached): the phone's finger no longer has to cover the desktop to steer it. New `android/app/.../ui/FoldableLayout.kt`: `rememberFoldingSplit(activity)` observes `androidx.window`'s `WindowInfoTracker.windowLayoutInfo` and maps a `FoldingFeature` hinge to a split axis (vertical hinge → left|right, horizontal → top|bottom, none → single); `TrackpadDock` is a standalone indirect-control surface (drag → `sendMouseEvent` relative move, tap → `sendMouseClick` press+release, two fingers → wheel scroll) driven through the same `limitedParallelism(1)` ordered dispatcher as the in-view trackpad; `ControlDock` stacks it above the desktop `ShortcutBar`. `RemoteScreen`'s root became a `BoxWithConstraints`: the session (video surface + HUD + WAN/status chip + control buttons) now sits in a *stream pane* sized to one half via `Modifier.fillMaxWidth(0.5f)`/`fillMaxHeight(0.5f)` + `align`, and the dock occupies the other half; the pane `Box` is a stable subtree (only its modifier changes on a fold), so the `SurfaceView`/decoder is **not** torn down when the posture flips. Dual-pane auto-engages on a hinge **or** a ≥600 dp window (tablet / unfolded foldable); a new "Pane: auto/dual/single" button cycles the override. While dual, the `ShortcutBar` moves out from under the stream into the dock. `androidx.window:window:1.5.0` added to the app deps (present in the Gradle cache → builds `--offline`). Single-pane rendering is unchanged from before. **No Rust/JNI change (58 exports).** en/es/ta strings (`pane_*`, `trackpad_hint`). Gates: `assembleDebug` + `lintDebug` clean (no new lint findings).

**Execution order (roadmap §6):** A1 ✅ → A2 ✅ → B1 ✅ → D1 ✅ → A3/E5 ✅ → B2 ✅ → D3 ✅ → C1 ✅ → E2 ✅ → C2 ✅ → D2 ✅ → E3 ✅ → B3 ✅ → D4 ✅ → E1 ✅ → C3 ✅ → E5-remainder ✅ → E4 ✅ → E6 ✅ → C4 ✅ (in-process VAAPI, live on this box; NVENC in-process stays driver-gated).



### Sources (R1/R2/R3)

- iroh: [crates.io/iroh](https://crates.io/crates/iroh), [iroh-relay](https://crates.io/crates/iroh-relay), [why we forked Quinn](https://www.iroh.computer/blog/why-we-forked-quinn), [iroh vs libp2p](https://www.iroh.computer/blog/comparing-iroh-and-libp2p)
- NAT/QUIC punching: [Implementing NAT Hole Punching with QUIC](https://arxiv.org/abs/2408.01791), [2025 decentralized NAT traversal measurement](https://arxiv.org/html/2510.27500v1)
- Prior art: [RustDesk self-host docs](https://rustdesk.com/docs/en/self-host/), [Moonlight setup/port-forwarding](https://natchecker.com/blog/moonlight-port-forwarding), [Tailscale DERP reference](https://tailscale.com/docs/reference/derp-servers), [Tailscale WoL relay](https://tailscale.com/blog/wake-on-lan-tailscale-upsnap)
- Client features: [Hyprland IPC wiki](https://wiki.hyprland.org/0.41.2/IPC/), [Moonlight Android](https://github.com/moonlight-stream/moonlight-android), [RustDesk remote control on Android](https://rustdesk.com/blog/rustdesk-remote-control-android-ios), [Android 14 foreground service types](https://developer.android.com/about/versions/14/changes/fgs-types-required)
- Streaming: [Sunshine advanced-usage/optimization docs](https://docs.lizardbyte.dev/projects/sunshine/v0.23.1/about/advanced_usage.html), [AOSP low-latency decoding](https://source.android.com/docs/core/media/low-latency-media), [LiveVideo10ms](https://github.com/Consti10/LiveVideo10ms), [xdg-desktop-portal-wlr dmabuf #9](https://github.com/emersion/xdg-desktop-portal-wlr/issues/9), [Sunshine Hyprland portal issues #4662](https://github.com/LizardByte/Sunshine/issues/4662), [quinn congestion-control API](https://docs.rs/quinn-proto/latest/quinn_proto/congestion/index.html), [Cloudflare UDP GSO for QUIC](https://blog.cloudflare.com/accelerating-udp-packet-transmission-for-quic/)

---

## Technical Deep Dives

### A. Tailscale Integration Details

### B. KDE Connect Protocol Specification

### C. Video Encoding Pipeline

### D. Input Event Mapping

### E. Security Considerations

---

## Risk Assessment & Mitigation

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| Wayland API changes | Medium | High | Pin crate versions, monitor upstream |
| Hardware encoding issues | Medium | Medium | Software fallback, extensive testing |
| Tailscale connectivity | Low | High | Retry logic, clear error messages |
| Android permission changes | Medium | Medium | Regular updates, SAF adoption |
| Performance below target | Medium | High | Early profiling, optimization sprints |

---

## Testing Strategy

### Unit Tests
### Integration Tests
### E2E Tests
### Performance Benchmarks

---

## Deployment & Packaging

### AUR Package
### systemd Service
### Android APK Distribution
### Documentation

---

## Appendix: Quick Reference

### Command Reference
### Configuration Options
### Troubleshooting Guide
### Performance Tuning

---

## Next Steps

1. **Immediate (This Week):**
   - Set up repository structure
   - Configure CI/CD
   - Install development dependencies

2. **Short-term (Month 1):**
   - Complete Phase 0 & 1
   - Establish Tailscale connectivity
   - Basic CLI working

3. **Medium-term (Month 2-3):**
   - KDE Connect features
   - Android basic UI
   - File transfer working

4. **Long-term (Month 4-6):**
   - Screen streaming
   - Performance optimization
   - Public release

---

*Document Version: 1.1*
*Last Updated: September 19, 2026 (Kotlin client migration)*
*Author: Linux Link Development Team*
