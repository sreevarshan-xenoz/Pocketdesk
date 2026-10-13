# Pocketdesk (formerly Linux Link) — Agent Context

This file provides architectural context for AI agents working on the Pocketdesk project (formerly Linux Link).

## Project Context

```json
{
  "project_context": {
    "subsystems": {
      "streaming": {
        "capture": "PipeWire (Wayland) / XShm (X11)",
        "encoding": "FFmpeg Sidecar (Annex B H.264/H.265)",
        "transport": "QUIC (quinn) over Port 4716",
        "input": "uinput (Kernel) fallback to enigo (X11)"
      },
      "protocol": {
        "control": "KDE Connect (JSON-over-TCP) over Port 1716",
        "binary_input": "InputPacket (Binary-over-QUIC)"
      }
    },
    "features": {
        "F1": "Audio Streaming (PipeWire Loopback)",
        "F2": "Multi-Monitor Support (Unified Discovery)",
        "F3": "HEVC Support (H.265 Encoding)",
        "F4": "Low-Latency Input (QUIC Unification)"
    },
    "critical_files": [
      "core/src/streaming/streamer.rs",
      "server/src/input_injector.rs",
      "server/src/plugins/monitors.rs"
    ]
  }
}
```

## Architectural Mandates

1.  **Low Latency First:** Always prefer binary QUIC streams for real-time data (video, audio, input) over the legacy TCP control channel.
2.  **Zero-Copy Capture:** Use PipeWire for Wayland capture to ensure security and performance.
3.  **Process Isolation:** Use FFmpeg as a sidecar process rather than linking to shared libraries to maintain stability and ease of deployment.
4.  **Hardware Acceleration:** Always probe for VAAPI and NVENC before falling back to software encoding.
