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
//! screens it has ([`screens`]) and stays awake meanwhile ([`awake`]).
//! The computer driving decides where its keyboard and mouse are
//! ([`drive`], keys named as Windows' hook names them in [`keys_windows`])
//! and, on Windows, captures them (`capture_windows`), staying put while a
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
#[cfg(windows)]
pub mod capture_windows;
pub mod drag_files;
pub mod drive;
#[cfg(windows)]
pub mod drop_windows;
pub mod game_lock;
pub mod held;
pub mod keys_windows;
pub mod layout;
pub mod link;
pub mod phone;
pub mod phone_test;
pub mod receive;
pub mod screens;
pub mod share_test;
pub mod sound;
pub mod status;
pub mod wire;
