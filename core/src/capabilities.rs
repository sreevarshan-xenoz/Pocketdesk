//! Single source of truth for what this build negotiates, carries, captures and
//! encodes (roadmap Phase 0 item 4).
//!
//! Every value here is *read* from the constants and decision tables the wire
//! actually uses — [`crate::protocol`], [`crate::streaming::capture`]'s ordering
//! function, [`VideoCodec`], [`AudioConfig`] — never restated. That is the whole
//! point: `pocketdesk capabilities`, `docs/capabilities.md` and the README's
//! capability prose are generated from this output, so a backend, codec or
//! version that changes in code changes in the docs by being regenerated, and
//! the drift test fails if it was not.
//!
//! Feature gates are reported rather than hidden: a client-profile build says
//! `capture: unavailable (this build has no capture backends)` instead of
//! printing an empty list that looks like a removed feature.

use serde::Serialize;

use crate::protocol::kdeconnect::LL_EXT_VERSION;
use crate::protocol::v2::{ALPN_V2, MAX_CONTROL_PAYLOAD_SIZE, V2_MAX_VERSION, V2_MIN_VERSION};
use crate::protocol::{
    ALPN_CONTROL, ALPN_TRANSFER, ALPN_V1_STREAM, HANDSHAKE_HELLO, PROTOCOL_VERSION,
};
use crate::streaming::audio::{MIC_CHANNELS, MIC_SAMPLE_RATE};
use crate::streaming::client::CODEC_CAPS_MARKER;
use crate::streaming::{AudioConfig, VideoCodec};
use crate::{
    DEFAULT_CONTROL_PORT, DEFAULT_STREAMING_PORT, PROTOCOL_VERSION as KDE_PROTOCOL_VERSION,
};

#[cfg(feature = "capture")]
use crate::streaming::capture::{CaptureBackend, DisplayServer, capture_attempts};

/// A codec as the protocol knows it: what it is called, how a client says it can
/// decode it, and whether this build can encode it.
#[derive(Debug, Clone, Serialize)]
pub struct VideoCodecInfo {
    pub name: &'static str,
    /// MediaCodec MIME type the phone selects on.
    pub mime_type: &'static str,
    /// FFmpeg encoder name this build selects when it picks the codec.
    pub ffmpeg_encoder: &'static str,
    /// The R4 C1 capability byte this codec is advertised with, or `null` when
    /// the codec needs no advertisement (H.264 is assumed for every client).
    pub client_cap_bit: Option<u8>,
    /// `false` only in a build without the `encode` feature.
    pub this_build_encodes: bool,
}

/// One of the negotiated audio directions.
#[derive(Debug, Clone, Serialize)]
pub struct AudioFormat {
    /// e.g. `desktop -> phone` (system audio) or `phone -> desktop` (mic relay).
    pub direction: &'static str,
    pub codec: &'static str,
    pub sample_rate: u32,
    pub channels: u16,
    /// `None` when the direction's bitrate is set by the encoder on the other
    /// end of the link (the phone's Opus encoder for the mic relay).
    pub bitrate_bps: Option<u32>,
    pub frame_duration_ms: u32,
    /// `false` only in a build without the `opus` feature.
    pub this_build_encodes: bool,
    /// Whether the device on the *receiving* end turns this stream into sound.
    ///
    /// Encoding and sending is half a feature (roadmap 2054). The desktop's mic
    /// sink plays what the phone sends (`mic_relay.rs` → `pw-loopback`), and the
    /// phone plays the desktop's sound (`DesktopAudioPlayer.kt`, roadmap 2791).
    pub played_on_the_receiving_end: bool,
}

/// A capture backend and where `Auto` would place it.
#[derive(Debug, Clone, Serialize)]
pub struct CaptureBackendInfo {
    /// Debug name of the enum variant.
    pub id: &'static str,
    /// The `capture_backend` value that pins it in config.toml.
    pub config_key: &'static str,
    /// 1-based position in `Auto`'s attempt list per display server; `None`
    /// means `Auto` never tries it there.
    pub auto_order_wayland: Option<usize>,
    pub auto_order_x11: Option<usize>,
    /// Same, on a Windows build (absent from a Linux report).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_order_windows: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaptureInfo {
    /// `false` in a build compiled without the `capture` feature.
    pub available: bool,
    pub backends: Vec<CaptureBackendInfo>,
    /// What `Auto` tries, in order, on each display server. Derived by calling
    /// the same function the capture pipeline calls.
    pub auto_order_wayland: Vec<String>,
    pub auto_order_x11: Vec<String>,
    /// What `Auto` tries on a Windows build (absent from a Linux report).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub auto_order_windows: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransportInfo {
    pub id: &'static str,
    pub label: &'static str,
    /// `tcp://…`, `udp://…` or an address-free description for `iroh`.
    pub endpoint: String,
    /// QUIC ALPN, when the transport is negotiated by one.
    pub alpn: Option<String>,
    /// `false` when the transport exists in code but this build excludes it
    /// (the `wan` feature gate around iroh).
    pub available: bool,
    pub note: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProtocolInfo {
    /// The v1 TCP control handshake greeting, version included verbatim.
    pub v1_handshake: &'static str,
    /// v1 control-handshake version string.
    pub v1_version: &'static str,
    /// The KDE Connect *identity* `protocolVersion` we emit — a different number
    /// space from the above, on purpose (see `protocol::kdeconnect`).
    pub kde_identity_version: u32,
    /// `llVersion` stamped on every `kdeconnect.pocketdesk.*` packet (R4 D4).
    pub kde_extension_version: u32,
    pub v2_min_version: u32,
    pub v2_max_version: u32,
    pub control_port: u16,
    pub streaming_port: u16,
    pub max_control_payload_bytes: usize,
}

/// The generated capability report. See the module docs.
#[derive(Debug, Clone, Serialize)]
pub struct Capabilities {
    pub core_version: &'static str,
    pub protocol: ProtocolInfo,
    pub transports: Vec<TransportInfo>,
    pub capture: CaptureInfo,
    pub video_codecs: Vec<VideoCodecInfo>,
    pub audio: Vec<AudioFormat>,
    /// The R4 C1 caps-stream marker bytes, so the report is readable without
    /// knowing where to look.
    pub codec_caps_stream_marker: Vec<u8>,
}

impl Capabilities {
    /// Read the capability set out of this build's constants and tables.
    pub fn collect() -> Self {
        Self {
            core_version: env!("CARGO_PKG_VERSION"),
            protocol: ProtocolInfo {
                v1_handshake: HANDSHAKE_HELLO,
                v1_version: PROTOCOL_VERSION,
                kde_identity_version: KDE_PROTOCOL_VERSION,
                kde_extension_version: LL_EXT_VERSION,
                v2_min_version: V2_MIN_VERSION,
                v2_max_version: V2_MAX_VERSION,
                control_port: DEFAULT_CONTROL_PORT,
                streaming_port: DEFAULT_STREAMING_PORT,
                max_control_payload_bytes: MAX_CONTROL_PAYLOAD_SIZE,
            },
            transports: vec![
                TransportInfo {
                    id: "tcp-v1",
                    label: "KDE Connect-style control channel, inside TLS 1.3",
                    endpoint: format!("tcp://0.0.0.0:{DEFAULT_CONTROL_PORT}"),
                    alpn: Some(String::from_utf8_lossy(ALPN_CONTROL).into_owned()),
                    available: true,
                    note: "pairing and plugin traffic (clipboard, battery, notifications, texts, file requests); both ends present certificates",
                },
                TransportInfo {
                    id: "tcp-transfer",
                    label: "File transfer, inside TLS 1.3",
                    endpoint: "tcp://<phone>:<port the phone names>".to_string(),
                    alpn: Some(String::from_utf8_lossy(ALPN_TRANSFER).into_owned()),
                    available: true,
                    note: "the phone listens and the desktop dials it, both ways; one-time key, size, bytes, status byte",
                },
                TransportInfo {
                    id: "quic-v2",
                    label: "QUIC multiplexer: media + control",
                    endpoint: format!("udp://0.0.0.0:{DEFAULT_STREAMING_PORT}"),
                    alpn: Some(String::from_utf8_lossy(ALPN_V2).into_owned()),
                    available: true,
                    note: "negotiated over LAN addresses and tailnet IPs alike",
                },
                TransportInfo {
                    id: "quic-v1-stream",
                    label: "QUIC streaming (pre-v2 handshake)",
                    endpoint: format!("udp://0.0.0.0:{DEFAULT_STREAMING_PORT}"),
                    alpn: Some(String::from_utf8_lossy(ALPN_V1_STREAM).into_owned()),
                    available: true,
                    note: "offered on the same port; the server dispatches on the ALPN the peer picks",
                },
                TransportInfo {
                    id: "iroh-noq",
                    label: "iroh hole-punched WAN endpoint",
                    endpoint: "address-free (endpoint id over any link)".to_string(),
                    alpn: Some(String::from_utf8_lossy(ALPN_V1_STREAM).into_owned()),
                    available: cfg!(feature = "wan"),
                    note: "behind the `wan` feature; the Android bridge builds with it",
                },
            ],
            capture: Self::capture_info(),
            video_codecs: VideoCodec::ALL
                .iter()
                .map(|codec| VideoCodecInfo {
                    name: codec.display_name(),
                    mime_type: codec.mime_type(),
                    ffmpeg_encoder: codec.ffmpeg_codec(),
                    client_cap_bit: codec.codec_cap_bit(),
                    this_build_encodes: cfg!(feature = "encode"),
                })
                .collect(),
            audio: {
                let out = AudioConfig::default();
                vec![
                    AudioFormat {
                        direction: "desktop -> phone",
                        codec: "Opus",
                        sample_rate: out.sample_rate,
                        channels: out.channels,
                        bitrate_bps: Some(out.bitrate_bps),
                        frame_duration_ms: out.frame_duration_ms,
                        this_build_encodes: cfg!(feature = "opus"),
                        // The Android app plays it: a MediaCodec Opus decoder into a
                        // low-latency AudioTrack (`DesktopAudioPlayer.kt`), on by
                        // default behind a Quick Settings switch.
                        played_on_the_receiving_end: true,
                    },
                    AudioFormat {
                        direction: "phone -> desktop",
                        codec: "Opus",
                        sample_rate: MIC_SAMPLE_RATE,
                        channels: MIC_CHANNELS,
                        bitrate_bps: None,
                        frame_duration_ms: out.frame_duration_ms,
                        this_build_encodes: cfg!(feature = "opus"),
                        // The desktop plays it: on Linux `mic_relay.rs` feeds a
                        // `pw-loopback` source named "Pocketdesk Mic"; on Windows
                        // `mic_relay_windows.rs` plays into a virtual audio cable
                        // (the note below says that one needs installing).
                        played_on_the_receiving_end: cfg!(any(target_os = "linux", windows)),
                    },
                ]
            },
            codec_caps_stream_marker: CODEC_CAPS_MARKER.to_vec(),
        }
    }

    #[cfg(feature = "capture")]
    fn capture_info() -> CaptureInfo {
        let order = |server| match capture_attempts(CaptureBackend::Auto, server) {
            Ok(attempts) => attempts
                .iter()
                .map(|b| b.config_key().to_string())
                .collect::<Vec<_>>(),
            Err(_) => Vec::new(),
        };
        // A build reports the display servers it can run on: the Linux pair,
        // or Windows.
        let (wayland, x11, windows) = if cfg!(windows) {
            (Vec::new(), Vec::new(), order(DisplayServer::Windows))
        } else {
            (
                order(DisplayServer::Wayland),
                order(DisplayServer::X11),
                Vec::new(),
            )
        };
        let position =
            |list: &[String], key: &str| list.iter().position(|k| k == key).map(|index| index + 1);
        CaptureInfo {
            available: true,
            backends: CaptureBackend::ALL
                .iter()
                .filter(|backend| !matches!(backend, CaptureBackend::Auto))
                .map(|backend| CaptureBackendInfo {
                    id: match backend {
                        CaptureBackend::Auto => "Auto",
                        CaptureBackend::Screencopy => "Screencopy",
                        CaptureBackend::Portal => "Portal",
                        CaptureBackend::X11 => "X11",
                        CaptureBackend::Wgc => "Wgc",
                        CaptureBackend::Dxgi => "Dxgi",
                    },
                    config_key: backend.config_key(),
                    auto_order_wayland: position(&wayland, backend.config_key()),
                    auto_order_x11: position(&x11, backend.config_key()),
                    auto_order_windows: position(&windows, backend.config_key()),
                })
                .collect(),
            auto_order_wayland: wayland,
            auto_order_x11: x11,
            auto_order_windows: windows,
        }
    }

    #[cfg(not(feature = "capture"))]
    fn capture_info() -> CaptureInfo {
        CaptureInfo {
            available: false,
            backends: Vec::new(),
            auto_order_wayland: Vec::new(),
            auto_order_x11: Vec::new(),
            auto_order_windows: Vec::new(),
        }
    }

    /// Machine-readable form: `pocketdesk capabilities --json`, and the input a
    /// script or the doctor command (Phase 9) consumes.
    pub fn render_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}"))
    }

    /// Human-readable listing for `pocketdesk capabilities`.
    pub fn render_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("pocketdesk-core {}\n", self.core_version));
        out.push_str("\nProtocol\n");
        out.push_str(&format!(
            "  v1 control handshake   {}\n",
            self.protocol.v1_handshake
        ));
        out.push_str(&format!(
            "  v1 version             {}\n",
            self.protocol.v1_version
        ));
        out.push_str(&format!(
            "  KDE identity version   {} (separate namespace; llVersion {})\n",
            self.protocol.kde_identity_version, self.protocol.kde_extension_version
        ));
        out.push_str(&format!(
            "  v2 negotiated range    {}-{}\n",
            self.protocol.v2_min_version, self.protocol.v2_max_version
        ));
        out.push_str(&format!(
            "  ports                  control {}/tcp, media {}/udp\n",
            self.protocol.control_port, self.protocol.streaming_port
        ));

        out.push_str("\nTransports\n");
        for transport in &self.transports {
            out.push_str(&format!(
                "  {:<16} {} [{}]{}\n",
                transport.id,
                transport.label,
                if transport.available { "yes" } else { "no" },
                transport
                    .alpn
                    .as_ref()
                    .map(|a| format!(" alpn={a}"))
                    .unwrap_or_default()
            ));
        }

        out.push_str("\nCapture backends\n");
        if self.capture.available && !self.capture.auto_order_windows.is_empty() {
            out.push_str(&format!(
                "  Auto on Windows: {}\n",
                self.capture.auto_order_windows.join(" -> ")
            ));
            for backend in &self.capture.backends {
                out.push_str(&format!(
                    "  {:<12} config `capture_backend = \"{}\"`\n",
                    backend.id, backend.config_key
                ));
            }
        } else if self.capture.available {
            out.push_str(&format!(
                "  Auto on Wayland: {}\n",
                if self.capture.auto_order_wayland.is_empty() {
                    "none".to_string()
                } else {
                    self.capture.auto_order_wayland.join(" -> ")
                }
            ));
            out.push_str(&format!(
                "  Auto on X11:     {}\n",
                if self.capture.auto_order_x11.is_empty() {
                    "none".to_string()
                } else {
                    self.capture.auto_order_x11.join(" -> ")
                }
            ));
            for backend in &self.capture.backends {
                out.push_str(&format!(
                    "  {:<12} config `capture_backend = \"{}\"`\n",
                    backend.id, backend.config_key
                ));
            }
        } else {
            out.push_str(
                "  unavailable: this build has no capture backends (feature `capture` off)\n",
            );
        }

        out.push_str("\nVideo codecs\n");
        for codec in &self.video_codecs {
            out.push_str(&format!(
                "  {:<14} encodes={} cap_bit={} mime={}\n",
                codec.name,
                if codec.this_build_encodes {
                    "yes"
                } else {
                    "no"
                },
                codec
                    .client_cap_bit
                    .map(|bit| format!("0b{bit:08b}"))
                    .unwrap_or_else(|| "assumed".to_string()),
                codec.mime_type
            ));
        }

        out.push_str("\nAudio\n");
        for format in &self.audio {
            out.push_str(&format!(
                "  {:<18} {} {}/{} @{}ms{}  receiver plays it: {}\n",
                format.direction,
                format.codec,
                format.sample_rate,
                format.channels,
                format.frame_duration_ms,
                format
                    .bitrate_bps
                    .map(|b| format!(", {b} bit/s"))
                    .unwrap_or_else(|| ", sender-set bitrate".to_string()),
                if format.played_on_the_receiving_end {
                    "yes"
                } else {
                    "no"
                }
            ));
        }
        out.push('\n');
        out
    }

    /// The Markdown block `docs/capabilities.md` is generated from. Tables
    /// because the doc is read by humans comparing behaviour across builds.
    pub fn render_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("# Pocketdesk — negotiated capabilities\n\n");
        out.push_str(
            "<!-- Generated by `pocketdesk capabilities --markdown` from pocketdesk-core ",
        );
        out.push_str(self.core_version);
        out.push_str(
            ".\n     Do not hand-edit: server/tests/capabilities_doc.rs fails when this file\n     \
             stops matching the build. Edit the constants in core instead. -->\n\n",
        );

        out.push_str("## Protocol versions\n\n");
        out.push_str("| Name | Value | Where it is on the wire |\n| --- | --- | --- |\n");
        out.push_str(&format!(
            "| v1 control handshake | `{}` | TCP greeting, `protocol::HANDSHAKE_HELLO` |\n",
            self.protocol.v1_handshake
        ));
        out.push_str(&format!(
            "| v1 version | `{}` | the `1` in the greeting above |\n",
            self.protocol.v1_version
        ));
        out.push_str(&format!(
            "| KDE Connect identity `protocolVersion` | `{}` | `kdeconnect.identity`, a separate namespace |\n",
            self.protocol.kde_identity_version
        ));
        out.push_str(&format!(
            "| Pocketdesk extension `llVersion` | `{}` | every `kdeconnect.pocketdesk.*` packet (R4 D4) |\n",
            self.protocol.kde_extension_version
        ));
        out.push_str(&format!(
            "| v2 negotiated range | `{}-{}` | `IdentityPacketV2` min/max, QUIC stream 0 |\n",
            self.protocol.v2_min_version, self.protocol.v2_max_version
        ));
        out.push_str(&format!(
            "| Control payload ceiling | `{}` bytes | framed v2 control JSON |\n",
            self.protocol.max_control_payload_bytes
        ));
        out.push_str(&format!(
            "| Ports | control `{}/tcp`, media `{}/udp` | `DEFAULT_CONTROL_PORT`, `DEFAULT_STREAMING_PORT` |\n",
            self.protocol.control_port, self.protocol.streaming_port
        ));

        out.push_str("\n## Transports\n\n");
        out.push_str(
            "| Id | What it carries | Address | ALPN | In this build | Notes |\n| --- | --- | --- | --- | --- | --- |\n",
        );
        for transport in &self.transports {
            out.push_str(&format!(
                "| `{}` | {} | `{}` | {} | {} | {} |\n",
                transport.id,
                transport.label,
                transport.endpoint,
                transport
                    .alpn
                    .as_ref()
                    .map(|a| format!("`{a}`"))
                    .unwrap_or_else(|| "—".to_string()),
                if transport.available { "yes" } else { "no" },
                transport.note
            ));
        }

        out.push_str("\n## Capture backends\n\n");
        if self.capture.available && !self.capture.auto_order_windows.is_empty() {
            out.push_str(&format!(
                "`Auto` attempts, in order: Windows `{}`. A config file may still name a Linux backend; it is refused at capture time with its name.\n\n",
                self.capture.auto_order_windows.join(" -> ")
            ));
            out.push_str(
                "| Backend | `capture_backend` value | Auto rank (Windows) |\n| --- | --- | --- |\n",
            );
            for backend in &self.capture.backends {
                out.push_str(&format!(
                    "| `{}` | `\"{}\"` | {} |\n",
                    backend.id,
                    backend.config_key,
                    backend
                        .auto_order_windows
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| "not tried".to_string()),
                ));
            }
        } else if self.capture.available {
            out.push_str(&format!(
                "`Auto` attempts, in order: Wayland `{}`, X11 `{}`.\n\n",
                self.capture.auto_order_wayland.join(" -> "),
                self.capture.auto_order_x11.join(" -> ")
            ));
            out.push_str(
                "| Backend | `capture_backend` value | Auto rank (Wayland) | Auto rank (X11) |\n| --- | --- | --- | --- |\n",
            );
            for backend in &self.capture.backends {
                out.push_str(&format!(
                    "| `{}` | `\"{}\"` | {} | {} |\n",
                    backend.id,
                    backend.config_key,
                    backend
                        .auto_order_wayland
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| "not tried".to_string()),
                    backend
                        .auto_order_x11
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| "not tried".to_string())
                ));
            }
        } else {
            out.push_str("Unavailable in this build: compiled without the `capture` feature.\n");
        }

        out.push_str("\n## Codecs\n\n");
        out.push_str(
            "| Video | This build encodes | Client capability bit | MediaCodec MIME | FFmpeg encoder |\n| --- | --- | --- | --- | --- |\n",
        );
        for codec in &self.video_codecs {
            out.push_str(&format!(
                "| {} | {} | {} | `{}` | `{}` |\n",
                codec.name,
                if codec.this_build_encodes {
                    "yes"
                } else {
                    "no"
                },
                codec
                    .client_cap_bit
                    .map(|bit| {
                        format!(
                            "`{}` caps stream, bit `0b{bit:08b}`",
                            hex(&CODEC_CAPS_MARKER)
                        )
                    })
                    .unwrap_or_else(|| "assumed for every client".to_string()),
                codec.mime_type,
                codec.ffmpeg_encoder
            ));
        }
        out.push_str("\n| Audio direction | Codec | This build encodes | Rate | Channels | Bitrate | Frame | Receiving end plays it |\n| --- | --- | --- | --- | --- | --- | --- | --- |\n");
        for format in &self.audio {
            out.push_str(&format!(
                "| {} | {} | {} | {} Hz | {} | {} | {} ms | {} |\n",
                format.direction,
                format.codec,
                if format.this_build_encodes {
                    "yes"
                } else {
                    "no"
                },
                format.sample_rate,
                format.channels,
                format
                    .bitrate_bps
                    .map(|b| format!("{b} bit/s"))
                    .unwrap_or_else(|| "set by the sender".to_string()),
                format.frame_duration_ms,
                if format.played_on_the_receiving_end {
                    "yes"
                } else {
                    "**no**"
                }
            ));
        }
        let unplayed: Vec<&str> = self
            .audio
            .iter()
            .filter(|format| !format.played_on_the_receiving_end)
            .map(|format| format.direction)
            .collect();
        if !unplayed.is_empty() {
            out.push_str(&format!(
                "\nThe `{}` direction is on the wire but not in the air: the receiving end has \
                 no playout path for it, so read that row as what the sender does, not as a \
                 feature you can hear (roadmap 2054 / 2791-2800).\n",
                unplayed.join("`, `")
            ));
        }
        if cfg!(windows) {
            out.push_str(
                "\nOn Windows the `phone -> desktop` direction plays into a virtual audio cable \
                 (VB-CABLE's \"CABLE Input\" by default, or `mic_relay_device` in config.toml), \
                 since a program cannot create a microphone without a driver; apps record from \
                 the cable's other end. With no cable installed the relay says so and drops the \
                 audio.\n",
            );
        }
        out
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_v1_greeting_carries_the_v1_version_it_reports() {
        // The greeting embeds the version as text; if either changes alone the
        // generated doc starts lying about the handshake.
        assert_eq!(
            HANDSHAKE_HELLO,
            &format!("POCKETDESK_HELLO {PROTOCOL_VERSION}")
        );
    }

    #[test]
    fn v2_range_is_sane_and_reported() {
        let caps = Capabilities::collect();
        assert!(caps.protocol.v2_min_version <= caps.protocol.v2_max_version);
        assert_eq!(caps.protocol.v2_min_version, V2_MIN_VERSION);
    }

    #[test]
    fn every_video_codec_is_described_exactly_once() {
        let caps = Capabilities::collect();
        assert_eq!(caps.video_codecs.len(), VideoCodec::ALL.len());
        // H.264 needs no capability bit; everything else must say how it is advertised.
        assert!(caps.video_codecs[0].client_cap_bit.is_none());
        assert!(
            caps.video_codecs[1..]
                .iter()
                .all(|c| c.client_cap_bit.is_some())
        );
    }

    #[test]
    fn renders_machine_and_human_readable_forms() {
        let caps = Capabilities::collect();
        let parsed: serde_json::Value = serde_json::from_str(&caps.render_json()).unwrap();
        assert_eq!(parsed["protocol"]["v2_max_version"], V2_MAX_VERSION);
        assert!(caps.render_text().contains("Transports"));
        assert!(caps.render_markdown().contains("## Capture backends"));
    }

    #[test]
    fn audio_rows_say_which_end_can_hear_them() {
        // Roadmap 2054: an audio direction is only a feature if the device at
        // the far end turns it into sound. Both ends now do (2791).
        let caps = Capabilities::collect();
        let played = |direction: &str| {
            caps.audio
                .iter()
                .find(|format| format.direction == direction)
                .map(|format| format.played_on_the_receiving_end)
                .unwrap_or_else(|| panic!("no `{direction}` row in the audio table"))
        };
        assert!(played("desktop -> phone"));
        // Linux plays the phone's mic into a pw-loopback source, Windows into a
        // virtual audio cable.
        assert!(played("phone -> desktop"));
        // With every row played, the doc has no unplayed note to explain.
        assert!(
            !caps.render_markdown().contains("no playout path for it"),
            "the doc still says a played direction is not heard"
        );
    }

    #[cfg(all(feature = "capture", windows))]
    #[test]
    fn a_windows_build_reports_windows_capture_only() {
        let caps = Capabilities::collect();
        assert_eq!(
            caps.capture.auto_order_windows,
            vec!["wgc".to_string(), "dxgi".to_string()]
        );
        assert!(caps.capture.auto_order_wayland.is_empty());
        assert!(caps.capture.auto_order_x11.is_empty());
        assert!(caps.render_markdown().contains("Auto rank (Windows)"));
    }

    #[cfg(all(feature = "capture", not(windows)))]
    #[test]
    fn auto_order_matches_the_capture_decision_table() {
        let caps = Capabilities::collect();
        let wayland = capture_attempts(CaptureBackend::Auto, DisplayServer::Wayland).unwrap();
        assert_eq!(
            caps.capture.auto_order_wayland,
            wayland
                .iter()
                .map(|b| b.config_key().to_string())
                .collect::<Vec<_>>()
        );
        // Screencopy leads on Wayland (R4 B1/B3), the portal is its fallback.
        assert_eq!(
            caps.capture.auto_order_wayland.first().map(String::as_str),
            Some("screencopy")
        );
        assert_eq!(
            caps.capture.auto_order_wayland.get(1).map(String::as_str),
            Some("portal")
        );
        // X11 never tries a Wayland-only backend.
        assert!(
            !caps
                .capture
                .auto_order_x11
                .contains(&"screencopy".to_string())
        );
    }
}
