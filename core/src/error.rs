//! Structured error types for Pocketdesk.
//!
//! Provides `PocketdeskError` as the primary error type with well-defined
//! variants covering protocol, streaming, discovery, and I/O failures.
//! Each variant carries contextual information for logging and debugging.

use std::fmt;

/// Machine-readable error codes for the FFI boundary and telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ErrorCode {
    Generic = 1000,

    // Transport (2xxx)
    ConnectionRefused = 2001,
    HandshakeFailed = 2002,
    HandshakeTimeout = 2003,
    TlsVerificationFailed = 2004,
    QuicProtocolError = 2005,
    ConnectionReset = 2006,

    // Auth & Trust (3xxx)
    PeerNotTrusted = 3001,
    AuthChallengeFailed = 3002,
    PairingRejected = 3003,

    // Discovery (4xxx)
    MdnsDaemonFailed = 4001,
    TailscaleCliNotFound = 4002,
    TailscaleAuthExpired = 4003,

    // Streaming & Codec (5xxx)
    EncoderInitializationFailed = 5001,
    PipeWireCaptureDenied = 5002,
    X11CaptureFailed = 5003,
    BitrateStall = 5004,

    // Lifecycle (6xxx)
    ShutdownInProgress = 6001,
    TaskCancelled = 6002,
}

/// The primary error type for Pocketdesk operations.
#[derive(Debug, Clone)]
pub enum PocketdeskError {
    /// A network connection could not be established.
    ConnectionFailed {
        address: String,
        port: u16,
        reason: String,
    },
    /// The KDE Connect protocol handshake did not complete.
    HandshakeFailed { peer: String, response: String },
    /// A timeout occurred while waiting for a response.
    Timeout {
        operation: &'static str,
        duration_ms: u64,
    },
    /// The remote peer sent an unexpected or malformed packet.
    ProtocolError { detail: String },
    /// Screen capture is not available on this system.
    CaptureNotAvailable { reason: String },
    /// An error from the PipeWire screen capture path.
    PipeWireError { detail: String },
    /// An error from the X11 screen capture path.
    X11Error { detail: String },
    /// Tailscale CLI or API returned an error.
    TailscaleError { reason: String },
    /// Peer discovery (Tailscale or mDNS) failed.
    DiscoveryFailed {
        method: &'static str,
        reason: String,
    },
    /// An I/O error (socket read/write, file I/O, etc.).
    Io {
        operation: &'static str,
        detail: String,
    },
    /// Serialization or deserialization failure.
    Serialization {
        format: &'static str,
        detail: String,
    },
    /// TLS / certificate error.
    TlsError { detail: String },
    /// QUIC connection error.
    QuicError { detail: String },
    /// The peer declined an operation (e.g., not paired).
    PeerDeclined { reason: String },
    /// Generic / catch-all for errors that don't fit other variants.
    Other { detail: String },
}

impl PocketdeskError {
    /// Get a stable machine-readable error code.
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::ConnectionFailed { .. } => ErrorCode::ConnectionRefused,
            Self::HandshakeFailed { .. } => ErrorCode::HandshakeFailed,
            Self::Timeout { operation, .. } => {
                if *operation == "handshake" {
                    ErrorCode::HandshakeTimeout
                } else {
                    ErrorCode::Generic
                }
            }
            Self::ProtocolError { .. } => ErrorCode::Generic,
            Self::CaptureNotAvailable { .. } => ErrorCode::PipeWireCaptureDenied,
            Self::PipeWireError { .. } => ErrorCode::PipeWireCaptureDenied,
            Self::X11Error { .. } => ErrorCode::X11CaptureFailed,
            Self::TailscaleError { .. } => ErrorCode::TailscaleCliNotFound,
            Self::DiscoveryFailed { .. } => ErrorCode::MdnsDaemonFailed,
            Self::TlsError { .. } => ErrorCode::TlsVerificationFailed,
            Self::QuicError { .. } => ErrorCode::QuicProtocolError,
            Self::PeerDeclined { .. } => ErrorCode::PairingRejected,
            Self::Other { .. } => ErrorCode::Generic,
            Self::Io { .. } => ErrorCode::Generic,
            Self::Serialization { .. } => ErrorCode::Generic,
        }
    }

    /// Whether the operation should be retried automatically.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self.code(),
            ErrorCode::ConnectionRefused
                | ErrorCode::HandshakeTimeout
                | ErrorCode::BitrateStall
                | ErrorCode::MdnsDaemonFailed
        )
    }
}

impl fmt::Display for PocketdeskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConnectionFailed {
                address,
                port,
                reason,
            } => {
                write!(f, "connection to {address}:{port} failed: {reason}")
            }
            Self::HandshakeFailed { peer, response } => {
                write!(f, "handshake with {peer} failed: {response}")
            }
            Self::Timeout {
                operation,
                duration_ms,
            } => {
                write!(f, "timeout after {duration_ms}ms during {operation}")
            }
            Self::ProtocolError { detail } => {
                write!(f, "protocol error: {detail}")
            }
            Self::CaptureNotAvailable { reason } => {
                write!(f, "capture not available: {reason}")
            }
            Self::PipeWireError { detail } => {
                write!(f, "PipeWire error: {detail}")
            }
            Self::X11Error { detail } => {
                write!(f, "X11 error: {detail}")
            }
            Self::TailscaleError { reason } => {
                write!(f, "Tailscale error: {reason}")
            }
            Self::DiscoveryFailed { method, reason } => {
                write!(f, "{method} discovery failed: {reason}")
            }
            Self::Io { operation, detail } => {
                write!(f, "I/O error during {operation}: {detail}")
            }
            Self::Serialization { format, detail } => {
                write!(f, "{format} serialization error: {detail}")
            }
            Self::TlsError { detail } => {
                write!(f, "TLS error: {detail}")
            }
            Self::QuicError { detail } => {
                write!(f, "QUIC error: {detail}")
            }
            Self::PeerDeclined { reason } => {
                write!(f, "peer declined: {reason}")
            }
            Self::Other { detail } => {
                write!(f, "{detail}")
            }
        }
    }
}

impl std::error::Error for PocketdeskError {}

// ---------------------------------------------------------------------------
// From implementations — convert common error types into PocketdeskError
// ---------------------------------------------------------------------------

impl From<std::io::Error> for PocketdeskError {
    fn from(e: std::io::Error) -> Self {
        Self::Io {
            operation: "I/O operation",
            detail: e.to_string(),
        }
    }
}

impl From<serde_json::Error> for PocketdeskError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serialization {
            format: "JSON",
            detail: e.to_string(),
        }
    }
}

impl From<quinn::ConnectionError> for PocketdeskError {
    fn from(e: quinn::ConnectionError) -> Self {
        Self::QuicError {
            detail: e.to_string(),
        }
    }
}

impl From<quinn::WriteError> for PocketdeskError {
    fn from(e: quinn::WriteError) -> Self {
        Self::QuicError {
            detail: e.to_string(),
        }
    }
}

impl From<quinn::ReadExactError> for PocketdeskError {
    fn from(e: quinn::ReadExactError) -> Self {
        Self::QuicError {
            detail: e.to_string(),
        }
    }
}

impl From<rustls::Error> for PocketdeskError {
    fn from(e: rustls::Error) -> Self {
        Self::TlsError {
            detail: e.to_string(),
        }
    }
}

impl From<rcgen::Error> for PocketdeskError {
    fn from(e: rcgen::Error) -> Self {
        Self::TlsError {
            detail: e.to_string(),
        }
    }
}

impl From<anyhow::Error> for PocketdeskError {
    fn from(e: anyhow::Error) -> Self {
        Self::Other {
            detail: format!("{e:#}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Result alias
// ---------------------------------------------------------------------------

/// A `Result` type using `PocketdeskError`.
pub type Result<T> = std::result::Result<T, PocketdeskError>;
