//! Clipboard images between the phone and the desktop (roadmap N4).
//!
//! Text rides `kdeconnect.clipboard` on the control channel. An image is
//! megabytes, so its bytes take the file-transfer path inside TLS: the phone
//! listens with a one-time key and the desktop dials back. These packet types
//! fall under the Clipboard permission, not Files.

/// Phone → desktop: `{size, port, key}`. The desktop dials back, reads the
/// PNG and puts it on its clipboard.
pub const CLIPBOARD_IMAGE: &str = "kdeconnect.pocketdesk.clipboard_image";

/// Phone → desktop: `{id, port, key}`. The desktop dials back and sends the
/// image its last clipboard reply named, while the clipboard still holds it.
pub const CLIPBOARD_IMAGE_FETCH: &str = "kdeconnect.pocketdesk.clipboard_image_fetch";

/// Computer → computer (PC3): `{}`. The window showing the receiver's
/// screen lost the focus while it controlled it; the receiver answers with
/// its clipboard, as a push marked `{follows: true, view: true}`.
pub const CLIPBOARD_VIEW: &str = "kdeconnect.pocketdesk.clipboard_view";

/// The largest image either side moves, as PNG bytes.
pub const MAX_CLIPBOARD_IMAGE: u64 = 32 * 1024 * 1024;
