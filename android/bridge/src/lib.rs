//! JNI entry points for the Pocketdesk Android client.
//!
//! Kotlin side: `dev.pocketdesk.android.bridge.RustCore`. Every async API call
//! is bridged with `block_on` on a shared Tokio runtime and returns a JSON
//! envelope string: `{"ok": <value>}` or `{"error": "<message>"}`. Kotlin
//! polls; there is no callback channel yet.

mod api;
mod download;
mod logcat;
mod rates;
mod session;

pub(crate) use session::*;

use jni::JNIEnv;
use jni::objects::{JClass, JIntArray, JObject, JString, ReleaseMode};
use jni::sys::{jboolean, jbyteArray, jfloat, jint, jlong, jstring};
use std::sync::LazyLock;

/// Shared Tokio runtime used to `block_on` the async client API.
static RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    // Every bridge call goes through this runtime, so this is the first
    // moment a log line can exist; install the logcat subscriber here.
    logcat::init();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("failed to build Tokio runtime")
});

/// Wrap a Rust result as the JSON envelope Kotlin expects.
fn envelope<T: serde::Serialize>(result: Result<T, String>) -> String {
    match result {
        Ok(v) => serde_json::json!({ "ok": v }).to_string(),
        Err(msg) => serde_json::json!({ "error": msg }).to_string(),
    }
}

/// Envelope for `Result<(), String>`.
fn envelope_unit(result: Result<(), String>) -> String {
    envelope(result.map(|()| serde_json::Value::Null))
}

fn to_jstring(env: &mut JNIEnv<'_>, json: String) -> jstring {
    match env.new_string(json) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

fn jstring_to_string(env: &mut JNIEnv<'_>, s: &JString) -> String {
    env.get_string(s)
        .map(|guard| guard.into())
        .unwrap_or_default()
}

/// Proves the Rust -> JNI -> Kotlin wiring works end to end.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeVersion(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    to_jstring(&mut env, api::version())
}

/// Initialize logging and backend state, and hand iroh the application
/// context. Idempotent.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeInit(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
    context: JObject<'_>,
) {
    api::init_app();
    install_android_context(&env, &context);
}

/// Give iroh's resolver the JVM and the application context, once per
/// process. Through them it reads the network's DNS servers
/// (`LinkProperties.getDnsServers`); without them it has no system
/// configuration and sends every relay lookup to its public fallback
/// resolvers (Cloudflare, Google, Quad9), which also fails on networks that
/// allow only their own DNS. The global reference is kept for the process's
/// life, as the context must stay valid until it exits.
#[cfg(target_os = "android")]
fn install_android_context(env: &JNIEnv<'_>, context: &JObject<'_>) {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let vm = match env.get_java_vm() {
            Ok(vm) => vm,
            Err(e) => {
                tracing::warn!("No JVM for the DNS resolver: {e}");
                return;
            }
        };
        let context = match env.new_global_ref(context) {
            Ok(global) => global,
            Err(e) => {
                tracing::warn!("No application context for the DNS resolver: {e}");
                return;
            }
        };
        let raw = context.as_obj().as_raw();
        std::mem::forget(context);
        // SAFETY: the JVM outlives the process's native code, and the
        // context's global reference is never deleted (forgotten above).
        unsafe {
            pocketdesk_core::streaming::iroh_connection::install_android_jni_context(
                vm.get_java_vm_pointer().cast(),
                raw.cast(),
            );
        }
    });
}

#[cfg(not(target_os = "android"))]
fn install_android_context(_env: &JNIEnv<'_>, _context: &JObject<'_>) {}

/// Set the persistent data directory (certs, trust store).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeSetDataDir(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    path: JString<'_>,
) {
    let path = jstring_to_string(&mut env, &path);
    if !path.is_empty() {
        api::set_data_dir(path);
    }
}

/// Pin a desktop's certificate for `address` from a scanned pairing code.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativePinPeerCertificate(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    fingerprint: JString<'_>,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let fingerprint = jstring_to_string(&mut env, &fingerprint);
    let json = envelope(api::pin_peer_certificate(address, fingerprint));
    to_jstring(&mut env, json)
}

/// Forget the certificate trusted for `address` (the computer was removed).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeForgetPeerCertificate(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let json = envelope_unit(api::forget_peer_certificate(address).map(|_| ()));
    to_jstring(&mut env, json)
}

/// Whether LAN (mDNS) discovery is currently active.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeIsDiscoveryActive(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jboolean {
    api::is_discovery_active() as jboolean
}

/// Tailscale availability check: `{"ok": bool}` or `{"error": ...}`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeCheckTailscaleStatus(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let json = envelope(RUNTIME.block_on(api::check_tailscale_status()));
    to_jstring(&mut env, json)
}

/// Peer list from Tailscale: `{"ok": [PeerInfoDto, ...]}`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeGetPeers(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let json = envelope(RUNTIME.block_on(api::get_peers()));
    to_jstring(&mut env, json)
}

/// Open the KDE Connect control TCP connection. Returns the new connection
/// state (Debug formatting) in the envelope.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeConnectToPeer(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let json = envelope(
        RUNTIME
            .block_on(api::connect_to_peer(address, port as u16))
            .map(|state| format!("{state:?}")),
    );
    to_jstring(&mut env, json)
}

/// Push clipboard content to the remote device.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeSendClipboard(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
    content: JString<'_>,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let content = jstring_to_string(&mut env, &content);
    let json = envelope_unit(RUNTIME.block_on(api::send_clipboard(address, port as u16, content)));
    to_jstring(&mut env, json)
}

/// Pull clipboard content from the remote device.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeGetClipboard(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let json = envelope(RUNTIME.block_on(api::get_clipboard(address, port as u16)));
    to_jstring(&mut env, json)
}

/// The desktop's clipboard as JSON: `{content}` or `{image: {id, size}}` (N4).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeGetClipboardState(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let json = envelope(RUNTIME.block_on(api::get_clipboard_state(address, port as u16)));
    to_jstring(&mut env, json)
}

/// Put the PNG at `path` on the desktop's clipboard (N4).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeSendClipboardImage(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
    path: JString<'_>,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let path = jstring_to_string(&mut env, &path);
    let json =
        envelope_unit(RUNTIME.block_on(api::send_clipboard_image(address, port as u16, path)));
    to_jstring(&mut env, json)
}

/// Download the desktop's clipboard image `id` into `path` (N4).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeFetchClipboardImage(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
    id: JString<'_>,
    path: JString<'_>,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let id = jstring_to_string(&mut env, &id);
    let path = jstring_to_string(&mut env, &path);
    let json = envelope(RUNTIME.block_on(download::fetch_clipboard_image(
        address,
        port as u16,
        id,
        path,
    )));
    to_jstring(&mut env, json)
}

/// Show one of this phone's notifications on the desktop (N5a).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeSendNotification(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
    body: JString<'_>,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let body = jstring_to_string(&mut env, &body);
    let json = envelope_unit(RUNTIME.block_on(api::send_notification(address, port as u16, body)));
    to_jstring(&mut env, json)
}

/// Drain pending KDE Connect packets as JSON strings.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativePollIncomingPackets(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let packets = RUNTIME.block_on(api::poll_incoming_packets());
    let json = serde_json::to_string(&packets).unwrap_or_else(|_| "[]".to_string());
    to_jstring(&mut env, json)
}

/// High-level session status as JSON (SessionStatus enum).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeSessionStatus(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let status = api::get_session_status();
    let json = serde_json::to_string(&status).unwrap_or_else(|e| format!(r#"{{"error":"{e}"}}"#));
    to_jstring(&mut env, json)
}

/// Whether a streaming session is currently active.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeIsStreamingActive(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jboolean {
    api::is_streaming_active() as jboolean
}

/// Why the streaming connection closed; empty while it is open.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeStreamClosedReason(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    to_jstring(&mut env, api::stream_closed_reason().unwrap_or_default())
}

/// Last measured streaming RTT in microseconds.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeGetStreamingRtt(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jint {
    let rtt_us = api::get_streaming_rtt();
    i32::try_from(rtt_us).unwrap_or(i32::MAX)
}

/// Streaming stats snapshot (fps, bitrate, ...) as JSON.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeGetStreamingStats(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let stats = api::get_streaming_stats();
    let json = serde_json::to_string(&stats).unwrap_or_else(|e| format!(r#"{{"error":"{e}"}}"#));
    to_jstring(&mut env, json)
}

/// Labels of currently trusted server certificates.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeListTrustedPeers(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let peers = api::list_trusted_peers();
    let json = serde_json::to_string(&peers).unwrap_or_else(|_| "[]".to_string());
    to_jstring(&mut env, json)
}

/// Remove a trusted peer by label. Returns `{"ok": bool}`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeForgetTrustedPeer(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    label: JString<'_>,
) -> jstring {
    let label = jstring_to_string(&mut env, &label);
    let json = envelope(Ok(api::forget_trusted_peer(label)));
    to_jstring(&mut env, json)
}

/// Stop the unified v2 connection.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeStopV2(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let json = envelope_unit(RUNTIME.block_on(api::stop_v2()));
    to_jstring(&mut env, json)
}

/// Stop the active streaming session.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeStopStreaming(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let json = envelope_unit(RUNTIME.block_on(api::stop_streaming()));
    to_jstring(&mut env, json)
}

/// Start a QUIC streaming session. `monitor_index` < 0 selects the default.
/// `codec_caps` is the client's decodable codec bitmask (R4 C1: bit 0 = HEVC).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeConnectStreaming(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
    monitor_index: jint,
    codec_caps: jint,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let monitor = if monitor_index < 0 {
        None
    } else {
        Some(monitor_index as u32)
    };
    let json = envelope_unit(RUNTIME.block_on(api::connect_streaming(
        address,
        port as u16,
        monitor,
        codec_caps as u8,
    )));
    to_jstring(&mut env, json)
}

/// Connect to the streaming server over the iroh WAN path using a cached
/// endpoint identity (the `kdeconnect.pocketdesk.endpoint` body JSON learned
/// from the desktop over the control channel).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeConnectStreamingWan(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    identity_json: JString<'_>,
    monitor_index: jint,
    codec_caps: jint,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let identity = jstring_to_string(&mut env, &identity_json);
    let monitor = if monitor_index < 0 {
        None
    } else {
        Some(monitor_index as u32)
    };
    let json = envelope_unit(RUNTIME.block_on(api::connect_streaming_wan(
        address,
        identity,
        monitor,
        codec_caps as u8,
    )));
    to_jstring(&mut env, json)
}

/// The connected desktop's cached iroh WAN identity, or `{"ok": null}` if it
/// has not announced one.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeGetWanIdentity(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let json = envelope(Ok(RUNTIME.block_on(api::get_wan_identity())));
    to_jstring(&mut env, json)
}

/// Reconnect a dropped streaming session with backoff state.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeReconnectStreaming(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
    monitor_index: jint,
    attempt: jint,
    codec_caps: jint,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let monitor = if monitor_index < 0 {
        None
    } else {
        Some(monitor_index as u32)
    };
    let json = envelope_unit(RUNTIME.block_on(api::reconnect_streaming(
        address,
        port as u16,
        monitor,
        attempt.max(0) as u32,
        codec_caps as u8,
    )));
    to_jstring(&mut env, json)
}

/// Reset the reconnection backoff (after a successful manual connect).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeResetReconnectBackoff(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) {
    api::reset_reconnect_backoff();
}

/// Connect via the v2 multiplexed QUIC protocol.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeConnectV2(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let json = envelope_unit(RUNTIME.block_on(api::connect_v2(address, port as u16)));
    to_jstring(&mut env, json)
}

/// Drain up to 16 encoded H.264 frames as a binary blob for MediaCodec.
///
/// Wire format (all big-endian):
/// `u32 count`, then per frame `u32 data_len`, `u8 flags` (bit 0 = keyframe),
/// `u64 sequence`, `data_len` bytes of Annex-B NAL data.
/// Returns an empty byte array when no frames are available.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeReceiveFrames(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
    timeout_ms: jint,
) -> jbyteArray {
    let frames = RUNTIME.block_on(api::receive_frames(timeout_ms.max(0) as u64));
    let total: usize = frames.iter().map(|f| 17 + f.data.len()).sum();
    let mut buf = Vec::with_capacity(4 + total);
    buf.extend_from_slice(&(frames.len() as u32).to_be_bytes());
    for frame in &frames {
        buf.extend_from_slice(&(frame.data.len() as u32).to_be_bytes());
        buf.push(frame.is_keyframe as u8);
        buf.extend_from_slice(&frame.sequence.to_be_bytes());
        buf.extend_from_slice(&frame.data);
    }
    match env.byte_array_from_slice(&buf) {
        Ok(arr) => arr.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Drain up to 32 Opus audio packets as a binary blob; see
/// [`api::audio_payload`] for the layout.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeReceiveAudio(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
    timeout_ms: jint,
) -> jbyteArray {
    let packets = RUNTIME.block_on(api::receive_audio(timeout_ms.max(0) as u64));
    match env.byte_array_from_slice(&api::audio_payload(&packets)) {
        Ok(arr) => arr.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Send a pointer event over the streaming (QUIC) or control channel.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeSendMouseEvent(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
    x: jfloat,
    y: jfloat,
    button: jint,
    is_pressed: jboolean,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let json = envelope_unit(RUNTIME.block_on(api::send_mouse_event(
        address,
        port as u16,
        x,
        y,
        button,
        is_pressed != 0,
    )));
    to_jstring(&mut env, json)
}

/// Send a normalized absolute pointer position (0..=65535 per axis).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeSendMouseAbs(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    x_norm: jint,
    y_norm: jint,
) -> jstring {
    let clamp = |v: jint| v.clamp(0, u16::MAX as i32) as u16;
    let json = envelope_unit(RUNTIME.block_on(api::send_mouse_abs(clamp(x_norm), clamp(y_norm))));
    to_jstring(&mut env, json)
}

/// Send a mouse button press/release. `button` is the wire encoding:
/// 0=Left, 1=Middle, 2=Right, 3=Back, 4=Forward.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeSendMouseClick(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    button: jint,
    pressed: jboolean,
) -> jstring {
    let json = envelope_unit(RUNTIME.block_on(api::send_mouse_click(
        button.clamp(0, 255) as u8,
        pressed != 0,
    )));
    to_jstring(&mut env, json)
}

/// Restrict capture to a window (R3#7 crop + R4 B2 compositor-side capture);
/// zero width/height clears it. `address` is the Hyprland window address as
/// reported by `get_windows` — empty when unknown, which leaves the server
/// cropping by rect instead.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeSendWindowCrop(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    x: jint,
    y: jint,
    width: jint,
    height: jint,
    address: JString<'_>,
) -> jstring {
    let nonneg = |v: jint| v.max(0) as u32;
    let address = jstring_to_string(&mut env, &address);
    let json = envelope_unit(RUNTIME.block_on(api::send_window_crop(
        nonneg(x),
        nonneg(y),
        nonneg(width),
        nonneg(height),
        &address,
    )));
    to_jstring(&mut env, json)
}

/// Toggle server-enforced view-only mode (R4 D1): while enabled the server
/// drops all injected input from this session; video keeps flowing.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeSetViewOnly(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    enabled: jboolean,
) -> jstring {
    let json = envelope_unit(RUNTIME.block_on(api::send_view_only(enabled != 0)));
    to_jstring(&mut env, json)
}

/// Toggle the R4 A3 full-quality override: opt out of the server's
/// relayed-path bitrate clamp (one-tap "give me everything" on WAN).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeSetFullQuality(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    enabled: jboolean,
) -> jstring {
    let json = envelope_unit(RUNTIME.block_on(api::send_full_quality(enabled != 0)));
    to_jstring(&mut env, json)
}

/// R4 E5: select a named link-profile preset (0 Auto, 1 Quality,
/// 2 Balanced, 3 Economy) the server folds into the live encoder bitrate.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeSendQualityPreset(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    preset: jint,
) -> jstring {
    let json = envelope_unit(RUNTIME.block_on(api::send_quality_preset(preset as u8)));
    to_jstring(&mut env, json)
}

/// Hand the bridge one duration the app measured, in microseconds: kind 0 is
/// decode (buffer fed → frame drained), kind 1 is render (gap between frames
/// reaching the panel). The streaming poller reports what has accumulated to the
/// desktop once a second, which is where a session's percentile tails are built.
/// Fire-and-forget from a decoder thread: no runtime, no envelope, no error.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeRecordSample(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    kind: jint,
    micros: jlong,
) {
    if micros >= 0 {
        api::record_sample(kind as u8, micros as u64);
    }
}

/// Open the R4 E2 phone→desktop mic (start frame; the server creates its
/// "Pocketdesk Mic" PipeWire source). Opus encoding happens in Kotlin.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeStartMic(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let json = envelope_unit(RUNTIME.block_on(api::start_mic()));
    to_jstring(&mut env, json)
}

/// Push one encoded Opus frame (20 ms, 48 kHz mono) to the desktop mic relay.
///
/// # Safety
/// `opus` must be a valid `jbyteArray` reference from the JNI caller.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeSendMicOpus(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    opus: jbyteArray,
) -> jstring {
    let json = match env.convert_byte_array(unsafe { jni::objects::JByteArray::from_raw(opus) }) {
        Ok(bytes) => envelope_unit(RUNTIME.block_on(api::send_mic_opus(bytes))),
        Err(e) => format!(r#"{{"error":"JNI byte array: {e}"}}"#),
    };
    to_jstring(&mut env, json)
}

/// Close the phone→desktop mic (server removes its virtual source).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeStopMic(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let json = envelope_unit(RUNTIME.block_on(api::stop_mic()));
    to_jstring(&mut env, json)
}

/// Send a keyboard event. `key_code` is an Android KeyCode; the bridge maps
/// it to evdev internally. `text` carries the char for UTF-8 input paths.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeSendKeyboardEvent(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
    key_code: jint,
    text: JString<'_>,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let text = jstring_to_string(&mut env, &text);
    let json = envelope_unit(RUNTIME.block_on(api::send_keyboard_event(
        address,
        port as u16,
        key_code,
        text,
    )));
    to_jstring(&mut env, json)
}

/// Send a gamepad state update. `axes` is an IntArray of i16 axis values.
// JNI contracts require taking raw array handles; the JVM guarantees validity.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeSendGamepadEvent(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    axes: jni::sys::jintArray,
    buttons: jint,
) -> jstring {
    let axes = if axes.is_null() {
        Vec::new()
    } else {
        let axes_obj = JIntArray::from(unsafe { JObject::from_raw(axes) });
        match unsafe { env.get_array_elements(&axes_obj, ReleaseMode::CopyBack) } {
            Ok(iter) => iter.iter().map(|&v| v as i16).collect(),
            Err(_) => Vec::new(),
        }
    };
    let json =
        envelope_unit(RUNTIME.block_on(api::send_gamepad_event(axes, buttons.max(0) as u32)));
    to_jstring(&mut env, json)
}

/// Push a local file to the remote device.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeSendFile(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
    file_path: JString<'_>,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let file_path = jstring_to_string(&mut env, &file_path);
    let json = envelope_unit(RUNTIME.block_on(api::send_file(address, port as u16, file_path)));
    to_jstring(&mut env, json)
}

/// Send a file from a file descriptor the caller gives up (F7), naming it by
/// `source` so a retry continues it (T6a): `{"ok": null}` or `{"err": "..."}`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeSendFileFd(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
    fd: jint,
    name: JString<'_>,
    size: jlong,
    source: JString<'_>,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let name = jstring_to_string(&mut env, &name);
    let source = jstring_to_string(&mut env, &source);
    let size = u64::try_from(size).unwrap_or(0);
    let json = envelope_unit(RUNTIME.block_on(api::send_file_fd(
        address,
        port as u16,
        fd,
        name,
        size,
        source,
    )));
    to_jstring(&mut env, json)
}

/// The running file transfer as `[sent, total]` bytes.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeTransferProgress(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jni::sys::jlongArray {
    let (sent, total) = api::transfer_progress();
    let values = [
        i64::try_from(sent).unwrap_or(i64::MAX),
        i64::try_from(total).unwrap_or(i64::MAX),
    ];
    match env.new_long_array(2) {
        Ok(array) => {
            if env.set_long_array_region(&array, 0, &values).is_err() {
                return std::ptr::null_mut();
            }
            array.into_raw()
        }
        Err(_) => std::ptr::null_mut(),
    }
}

/// Download a file from the desktop into `fd`, which the bridge takes over
/// (F7b): a browsed `value` path, or an offer id when `is_offer`. With a
/// `source`, a broken download keeps its part at `location` (the place `fd`
/// opens) for a retry to continue (T6a). `{"ok": bytes}` or
/// `{"error": "..."}`.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeFetchFile(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
    is_offer: jni::sys::jboolean,
    value: JString<'_>,
    fd: jint,
    source: JString<'_>,
    name: JString<'_>,
    location: JString<'_>,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let value = jstring_to_string(&mut env, &value);
    let source = jstring_to_string(&mut env, &source);
    let name = jstring_to_string(&mut env, &name);
    let location = jstring_to_string(&mut env, &location);
    let target = if is_offer != 0 {
        download::FetchTarget::Offer(value)
    } else {
        download::FetchTarget::Path(value)
    };
    let resume = (pocketdesk_core::protocol::partials::valid_source(&source)
        && !location.is_empty())
    .then_some(download::Resume {
        source,
        name,
        location,
    });
    let json = envelope(RUNTIME.block_on(download::fetch_file_fd(
        address,
        port as u16,
        target,
        fd,
        resume,
    )));
    to_jstring(&mut env, json)
}

/// Where the part of `address`'s file `source` an earlier download kept is
/// (T6a), or "" when there is none.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativePartialLocation(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    source: JString<'_>,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let source = jstring_to_string(&mut env, &source);
    let location = download::partial_location(&address, &source).unwrap_or_default();
    to_jstring(&mut env, location)
}

/// Forget unfinished downloads older than `max_age_hours`, and return where
/// their bytes are as a JSON array, for the app to delete (T6a).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeSweepPartials(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    max_age_hours: jint,
) -> jstring {
    let dropped = download::sweep_partials(u32::try_from(max_age_hours).unwrap_or(0));
    let json = serde_json::to_string(&dropped).unwrap_or_else(|_| "[]".to_string());
    to_jstring(&mut env, json)
}

/// Tell the desktop the phone will not fetch offer `id` (F7b).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeDeclineOffer(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
    id: JString<'_>,
    reason: JString<'_>,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let id = jstring_to_string(&mut env, &id);
    let reason = jstring_to_string(&mut env, &reason);
    let json =
        envelope_unit(RUNTIME.block_on(download::decline_offer(address, port as u16, id, reason)));
    to_jstring(&mut env, json)
}

/// Files the desktop offered since the last call: `{"ok": "[{id,name,size}]"}`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeTakeFileOffers(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let json = envelope(Ok(download::take_file_offers()));
    to_jstring(&mut env, json)
}

/// A folder on the desktop (F7b): `{"ok": {path, parent, files, error?}}`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeListRemoteFiles(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
    remote_path: JString<'_>,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let remote_path = jstring_to_string(&mut env, &remote_path);
    let json =
        envelope(RUNTIME.block_on(api::list_remote_files(address, port as u16, remote_path)));
    to_jstring(&mut env, json)
}

/// Enumerate remote monitors: `{"ok": [MonitorInfoDto, ...]}`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeGetMonitors(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let json = envelope(RUNTIME.block_on(api::get_monitors(address, port as u16)));
    to_jstring(&mut env, json)
}

/// Number of remote monitors.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeGetMonitorCount(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let json = envelope(RUNTIME.block_on(api::get_monitor_count(address, port as u16)));
    to_jstring(&mut env, json)
}

/// Desktop battery state JSON (`{currentCharge, isCharging}` / `{noBattery}`).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeGetBattery(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let json = envelope(RUNTIME.block_on(api::get_battery(address, port as u16)));
    to_jstring(&mut env, json)
}

/// Desktop privacy mode (Tier-3 #15): `action` is "grab" | "release" |
/// "status"; `lock` additionally engages the desktop screen locker.
/// Returns the plugin reply as `{"ok": {json}}`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeDesktopPrivacy(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
    action: JString<'_>,
    lock: jboolean,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let action = jstring_to_string(&mut env, &action);
    let json = envelope(RUNTIME.block_on(api::desktop_privacy(
        address,
        port as u16,
        action,
        lock != 0,
    )));
    to_jstring(&mut env, json)
}

/// Desktop audio control (Tier-3 #16): `body_json` is the
/// `kdeconnect.pocketdesk.audio` request body; returns the plugin reply.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeAudioControl(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
    body_json: JString<'_>,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let body_json = jstring_to_string(&mut env, &body_json);
    let json = envelope(RUNTIME.block_on(api::audio_control(address, port as u16, body_json)));
    to_jstring(&mut env, json)
}

/// Consume the find-my-device siren latch — `{"ok":true}` once per desktop
/// `kdeconnect.findmydevice` push (R3 Tier-2 #11).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeCheckSiren(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let json = envelope(Ok(api::check_siren()));
    to_jstring(&mut env, json)
}

/// Make the remote desktop ring (find-my-device siren, phone → desktop).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeSendFindMyDevice(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let json = envelope_unit(RUNTIME.block_on(api::send_findmydevice(address, port as u16)));
    to_jstring(&mut env, json)
}

/// Ask the desktop to show a pairing PIN — `{"ok":"pinSent"|"pinReady"}`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeRequestPairPin(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let json = envelope(RUNTIME.block_on(api::request_pair_pin(address, port as u16)));
    to_jstring(&mut env, json)
}

/// The Connection Doctor's identity check (T1): `{"ok":"matches"|"changed"|
/// "not_pinned"|"not_pocketdesk"|"failed"}`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeCheckIdentity(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let json = envelope(RUNTIME.block_on(api::check_identity(address, port as u16)));
    to_jstring(&mut env, json)
}

/// Submit a pairing PIN; `{"ok":"<deviceId>"}` when the desktop paired us,
/// `{"ok":null}` for a wrong/expired PIN.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativePairWithPin(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
    pin: JString<'_>,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let pin = jstring_to_string(&mut env, &pin);
    let json = envelope(RUNTIME.block_on(api::pair_with_pin(address, port as u16, pin)));
    to_jstring(&mut env, json)
}

/// Wait for a pushed pairing decision: `{"ok":"<deviceId>"}` when paired,
/// `{"ok":null}` while still waiting.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeCheckPairResult(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    wait_secs: jlong,
) -> jstring {
    let json = envelope(Ok(RUNTIME
        .block_on(api::check_pair_result(wait_secs as u64))
        .map(|id| id.to_string())));
    to_jstring(&mut env, json)
}

/// JSON array of desktop device ids this phone is paired with.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativePairedServers(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let json = envelope(Ok(api::paired_servers_json()));
    to_jstring(&mut env, json)
}

/// Drain queued desktop notifications as a JSON array
/// (`[{"id","app","title","text","source"}, …]`).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeTakePendingNotifications(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let json = envelope(Ok(api::take_pending_notifications()));
    to_jstring(&mut env, json)
}

/// The computer the live control connection is to, as JSON, or "".
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeControlPeer(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let json = RUNTIME.block_on(api::control_peer()).unwrap_or_default();
    to_jstring(&mut env, json)
}

/// The replies the desktop asked this phone to text (N5b), as JSON.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeTakeSmsRequests(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let json = envelope(Ok(api::take_sms_requests()));
    to_jstring(&mut env, json)
}

/// Send the phone's texts or a reply's result to the desktop (N5b).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeSendPhonePacket(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
    packet_type: JString<'_>,
    body: JString<'_>,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let packet_type = jstring_to_string(&mut env, &packet_type);
    let body = jstring_to_string(&mut env, &body);
    let json = envelope_unit(RUNTIME.block_on(api::send_phone_packet(
        address,
        port as u16,
        packet_type,
        body,
    )));
    to_jstring(&mut env, json)
}

/// Reply to a desktop notification from the phone (Tier-2 #11c).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeSendNotificationReply(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
    id: JString<'_>,
    text: JString<'_>,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let id = jstring_to_string(&mut env, &id);
    let text = jstring_to_string(&mut env, &text);
    let json = envelope_unit(RUNTIME.block_on(api::send_notification_reply(
        address,
        port as u16,
        id,
        text,
    )));
    to_jstring(&mut env, json)
}

/// Hyprland window list (R3#7 picker):
/// `{"ok": [[WindowInfoDto, ...], activeAddress, screenBoxOrNull]}` where
/// `screenBoxOrNull` is the monitor layout `[x, y, w, h]` in desktop coords.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeGetWindows(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let json = envelope(RUNTIME.block_on(api::get_windows(address, port as u16)));
    to_jstring(&mut env, json)
}

/// Send a power command (action strings defined by the server protocol).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeSendPowerCommand(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
    action: JString<'_>,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let action = jstring_to_string(&mut env, &action);
    let json =
        envelope_unit(RUNTIME.block_on(api::send_power_command(address, port as u16, action)));
    to_jstring(&mut env, json)
}

/// Execute a shell command remotely; `{"ok": "<stdout>"}`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeExecuteRemoteCommand(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
    command: JString<'_>,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let command = jstring_to_string(&mut env, &command);
    let json =
        envelope(RUNTIME.block_on(api::execute_remote_command(address, port as u16, command)));
    to_jstring(&mut env, json)
}

/// Wake a computer from this phone: the magic packet to each target (N6).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeWake(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    mac: JString<'_>,
    targets_json: JString<'_>,
) -> jstring {
    let mac = jstring_to_string(&mut env, &mac);
    let targets_json = jstring_to_string(&mut env, &targets_json);
    let json = envelope(api::wake(mac, targets_json).map(|n| n.to_string()));
    to_jstring(&mut env, json)
}

/// How the computer at `address`:`port` said it can be woken, as JSON, or
/// "" (N6).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeWakeInfo(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let json = RUNTIME
        .block_on(api::get_wake_info(&address, port as u16))
        .unwrap_or_default();
    to_jstring(&mut env, json)
}

/// Who drives, as the computer at `address`:`port` last said, as JSON, or
/// "" (N7).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeControlState(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let json = RUNTIME
        .block_on(api::get_control_state(&address, port as u16))
        .unwrap_or_default();
    to_jstring(&mut env, json)
}

/// Ask an always-on LAN relay to send the WoL magic packet (Tier-2 #12).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_pocketdesk_android_bridge_RustCore_nativeWakeViaRelay(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    address: JString<'_>,
    port: jint,
    mac: JString<'_>,
    broadcast: JString<'_>,
) -> jstring {
    let address = jstring_to_string(&mut env, &address);
    let mac = jstring_to_string(&mut env, &mac);
    let broadcast = jstring_to_string(&mut env, &broadcast);
    let json =
        envelope_unit(RUNTIME.block_on(api::wake_via_relay(address, port as u16, mac, broadcast)));
    to_jstring(&mut env, json)
}
