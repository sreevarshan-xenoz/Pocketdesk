//! One keyboard and mouse across the owner's computers.
//!
//! This is the pure part, with no I/O of its own: the messages two computers
//! send each other ([`wire`]), where each computer's screens sit relative to
//! the others and where the cursor crosses from one to the next ([`layout`]),
//! and the keys and buttons a computer has sent as down so they can all be
//! let go of ([`held`]). The channel between two computers on their link
//! ([`link`], tried out with `pocketdesk share-test`, [`share_test`]) carries
//! them; the capture and the injection are built on these.

pub mod held;
pub mod layout;
pub mod link;
pub mod share_test;
pub mod wire;
