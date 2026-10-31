//! One keyboard and mouse across the owner's computers.
//!
//! Its pure part has no I/O of its own: the messages two computers
//! send each other ([`wire`]), where each computer's screens sit relative to
//! the others and where the cursor crosses from one to the next ([`layout`]),
//! and the keys and buttons a computer has sent as down so they can all be
//! let go of ([`held`]). The channel between two computers on their link
//! ([`link`], tried out with `pocketdesk share-test`, [`share_test`]) carries
//! them. The computer being driven acts on what arrives ([`receive`]): it
//! types and moves what a computer allowed to drive it sends, says what
//! screens it has ([`screens`]) and stays awake meanwhile ([`awake`]),
//! the link kept quick while either side's keyboard and mouse are elsewhere
//! ([`quick`]).
//! The computer driving decides where its keyboard and mouse are
//! ([`drive`], keys named as Windows' hook names them in [`keys_windows`])
//! and captures them: on Windows by its hooks (`capture_windows`), on Linux
//! by the input-capture portal and libei (`capture_linux`, through
//! [`portal_glue`] and [`portal_drive`], this computer's edges as the
//! portal's [`barriers`], its keys in [`keys_evdev`]), staying put while a
//! game is in front ([`game_lock`]); files dragged in Explorer and let go
//! on the computer driven are sent to it ([`drag_files`], caught on Windows
//! by `drop_windows`). A computer that will not be driven
//! says so ([`wire::Msg::Refuse`]); how things stand is written for the
//! desktop app ([`status`]), and its requests are taken ([`asks`]). Each
//! switch, and Identify, flashes a computer's name on its screens
//! ([`badge`], drawn by [`crate::overlay`]). The sound of the computer
//! driven can play on the computer driving it ([`sound`]).

pub mod asks;
pub mod awake;
pub mod badge;
pub mod barriers;
#[cfg(target_os = "linux")]
pub mod capture_linux;
pub mod capture_restore;
#[cfg(windows)]
pub mod capture_windows;
pub mod drag_files;
pub mod drive;
#[cfg(test)]
mod driver_bench;
#[cfg(windows)]
pub mod drop_windows;
pub mod game_lock;
pub mod held;
pub mod keys_evdev;
pub mod keys_windows;
pub mod layout;
pub mod link;
pub mod pace;
pub mod phone;
pub mod phone_test;
pub mod portal_drive;
pub mod portal_glue;
pub mod portal_session;
pub mod quick;
pub mod receive;
pub mod screens;
#[cfg(target_os = "linux")]
pub mod screens_wayland;
pub mod share_test;
pub mod sound;
pub mod status;
pub mod view_hold;
pub mod wire;

/// What a test logs, caught as text (no times, no colours), from debug up,
/// on this thread while the guard lives.
///
/// Through one process-wide subscriber that writes each thread's lines to
/// the buffer that thread set, never a scoped one: tracing caches whether
/// a log line is wanted when its callsite is first met, and a scoped
/// subscriber missed lines another test's thread met first (seen on
/// Windows, where tests run in parallel).
#[cfg(test)]
pub(crate) mod caught_log {
    use std::collections::HashMap;
    use std::io;
    use std::sync::{Arc, LazyLock, Mutex, Once};
    use std::thread::ThreadId;

    type Buffer = Arc<Mutex<Vec<u8>>>;

    /// The buffer each catching thread set.
    static CATCHING: LazyLock<Mutex<HashMap<ThreadId, Buffer>>> = LazyLock::new(Default::default);

    fn this_thread() -> ThreadId {
        std::thread::current().id()
    }

    /// The lines caught so far.
    pub struct Caught(Buffer);

    impl Caught {
        pub fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    /// Catching stops on this thread when it drops.
    pub struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            CATCHING.lock().unwrap().remove(&this_thread());
        }
    }

    /// Writes to the buffer of the thread logging, if it catches.
    struct ThisThread;

    impl io::Write for ThisThread {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let buffer = CATCHING.lock().unwrap().get(&this_thread()).cloned();
            if let Some(buffer) = buffer {
                buffer.lock().unwrap().extend_from_slice(bytes);
            }
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Catch this thread's logs until the guard drops.
    pub fn catch() -> (Caught, Guard) {
        static GLOBAL: Once = Once::new();
        GLOBAL.call_once(|| {
            let subscriber = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::DEBUG)
                .without_time()
                .with_ansi(false)
                .with_writer(|| ThisThread)
                .finish();
            // Set once. Should another global subscriber have been set
            // first, nothing reaches the buffers and the catching tests
            // fail with an empty log, never pass falsely.
            let _ = tracing::subscriber::set_global_default(subscriber);
            // A callsite first met on another thread while the subscriber
            // was being set may have kept "never": asked again now.
            tracing::callsite::rebuild_interest_cache();
        });
        let buffer = Buffer::default();
        CATCHING
            .lock()
            .unwrap()
            .insert(this_thread(), buffer.clone());
        (Caught(buffer), Guard)
    }
}
