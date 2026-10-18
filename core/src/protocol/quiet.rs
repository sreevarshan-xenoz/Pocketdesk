//! Do Not Disturb between the computer and a phone (D2): the computer asks
//! a phone to mute itself for a while, and the phone says when it is quiet.
//! Both under the phone's Mute permission.

/// Computer to phone: `{minutes}`, 1 to [`MAX_MINUTES`], or 0 to end a mute.
pub const QUIET: &str = "kdeconnect.pocketdesk.quiet";

/// Phone to computer: `{quiet, until?, byComputer}`, on a change and when
/// its link comes up. `until` (unix seconds) only for a computer's mute.
pub const QUIET_STATE: &str = "kdeconnect.pocketdesk.quiet_state";

/// The longest mute: a day.
pub const MAX_MINUTES: u32 = 1_440;
