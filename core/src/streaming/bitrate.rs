//! Adaptive bitrate control
//!
//! Two things move the encoder's rate, and they are not alike. The presets and
//! the relay floor are *policy*: a known ceiling the user or the link path
//! picks, computable from state alone. Loss response is a *controller*: it has
//! to watch the link over time, decide from ratios, and be able to walk the
//! rate back up again. [`LossCeiling`] is that controller; the live arbiter in
//! [`super::streamer`] owns all of them and pushes the result to the encoder.
//!
//! There used to be a second class here, `AdaptiveBitrate`, trading rate
//! against smoothed RTT. It had no callers: the only way in was
//! `StreamingServer::with_adaptive_bitrate`, which nothing ever called, so it
//! never saw a connection and no RTT-driven adjustment had ever happened. It is
//! deleted rather than revived — RTT belongs in the same arbiter as a measured
//! input when there is a decision to make about it (Phase 5's ABR work), not in
//! a parallel owner that would fight the one that exists.

/// Loss-driven bitrate ceiling — the input the controller here has always had
/// a hook for and never used (roadmap 2053).
///
/// QUIC already reacts to loss by shrinking its congestion window, but that
/// bounds the *transport*, not the encoder: the desktop keeps producing frames
/// at the configured rate, and the mismatch comes out as a backlog the
/// transport task trims — dropped frames. Cutting the encoder's rate is the
/// other half of the response, and the only way a lossy link gets a smoother
/// picture rather than a stalling one.
///
/// The signal is the send-side loss ratio over one arbiter tick, from the
/// transport's own cumulative counters: `ConnectionStats::lost_packets` over
/// `datagrams_sent`. Both stacks report those — quinn for the current path,
/// iroh summed across paths, where a ratio of summed counts is still the
/// ratio it was before the summing.
pub struct LossCeiling {
    configured: u32,
    ceiling: u32,
    prev_lost: u64,
    prev_sent: u64,
    /// Whether a first reading has set the baseline.
    primed: bool,
}

/// Under this many datagrams in one tick, a loss ratio is noise. A still
/// desktop sends almost nothing (damage-driven capture), and reacting to "1 of
/// 4 lost" would cut a healthy link to the floor.
const MIN_TICK_DATAGRAMS: u64 = 100;

/// Losing more than one percent of what we sent means the link is congested.
const LOSS_REDUCE_RATIO: f64 = 0.01;

/// Under a tenth of a percent, let the rate climb back. Between the two is a
/// deadband where holding still is the answer: without it, a link that is
/// neither good nor bad rebuilds the encoder every tick.
const LOSS_CLEAN_RATIO: f64 = 0.001;

/// Cut 20 % per congested tick, recover 10 % per clean one — asymmetric on
/// purpose, so a link that starts dropping is off the cliff in a couple of
/// seconds and the climb back is the slow, non-oscillating direction.
const REDUCE_FACTOR: f64 = 0.80;
const RECOVER_FACTOR: f64 = 1.10;

/// Not a tuning preference: below about a megabit the encoder is producing an
/// unusable picture, and what a link that starved needs next is fewer pixels,
/// not fewer bits per pixel. This controller does not change resolution
/// mid-session — the same honest axis scope as the HUD presets — so it stops
/// rather than silently degrade the image past the point of use.
const LOSS_BITRATE_FLOOR_BPS: u32 = 1_000_000;

impl LossCeiling {
    /// Start disengaged: a session begins at its configured rate and only
    /// measured loss moves it.
    pub fn new(configured_bps: u32) -> Self {
        Self {
            configured: configured_bps,
            ceiling: u32::MAX,
            prev_lost: 0,
            prev_sent: 0,
            primed: false,
        }
    }

    /// Fold one tick of cumulative transport counters into the ceiling.
    /// Counters that go backwards yield a zero delta, which the sample floor
    /// turns into "no information" rather than a negative loss ratio.
    ///
    /// The first reading only sets the baseline. The counters run from the
    /// handshake, so it also holds whatever the connection lost before the
    /// stream existed — iroh's path probing above all — and judging that as
    /// stream loss cut every WAN session's rate on its first tick, then
    /// rebuilt a sidecar encoder on each of the four ticks that climbed back.
    pub fn sample(&mut self, lost_packets: u64, datagrams_sent: u64) {
        let lost_delta = lost_packets.saturating_sub(self.prev_lost);
        let sent_delta = datagrams_sent.saturating_sub(self.prev_sent);
        self.prev_lost = lost_packets;
        self.prev_sent = datagrams_sent;
        if !std::mem::replace(&mut self.primed, true) {
            return;
        }
        self.ceiling = next_loss_ceiling(self.configured, self.ceiling, sent_delta, lost_delta);
    }

    /// The ceiling to fold into the arbiter's `min` chain. [`u32::MAX`] means
    /// loss has never moved the rate, so the term is inert.
    pub fn ceiling(&self) -> u32 {
        self.ceiling
    }
}

/// One tick of [`LossCeiling::sample`], as a pure function so every branch is
/// reachable without a connection. Reductions are measured against whatever is
/// already in force, never against `configured`, so a preset or relay clamp
/// that already cut the rate is a starting point rather than something this
/// term quietly overrides upward.
fn next_loss_ceiling(configured: u32, ceiling: u32, sent: u64, lost: u64) -> u32 {
    if sent < MIN_TICK_DATAGRAMS {
        return ceiling;
    }
    let ratio = lost as f64 / sent as f64;
    let base = ceiling.min(configured);
    if ratio > LOSS_REDUCE_RATIO {
        ((base as f64 * REDUCE_FACTOR) as u32)
            .max(LOSS_BITRATE_FLOOR_BPS)
            .min(configured)
    } else if ratio < LOSS_CLEAN_RATIO {
        if base >= configured {
            u32::MAX
        } else {
            let climbing = (base as f64 * RECOVER_FACTOR) as u32;
            if climbing >= configured {
                u32::MAX
            } else {
                climbing.max(LOSS_BITRATE_FLOOR_BPS)
            }
        }
    } else {
        ceiling
    }
}

/// Delivery-driven bitrate ceiling (R6f): what the link actually carried.
///
/// On a slow hop that loses little at random, QUIC's congestion control
/// keeps the router's queue from overflowing, so [`LossCeiling`] sees almost
/// nothing: R6e's 1 Mbit/s link lost 89 datagrams in 5 minutes while the
/// encoder ran at 6.4-8 Mbit/s and the transport threw away what could not
/// leave (5-12 frames a second reached the phone). The transport knows,
/// though: it waits for its frames to be delivered before it sends more
/// (`super::in_flight`), and a link that is the limit keeps it waiting.
///
/// Each arbiter tick reads how long the video transport waited, and how many
/// video bytes it got onto the link. Waiting for most of a tick, twice in a
/// row (one keyframe can fill one), means the link is the limit, and the
/// rate that got through is the link's: the ceiling goes just under it.
/// Three ticks in a row that used most of the ceiling with the transport
/// hardly waiting let it climb back, a quarter a step, until it is above the
/// configured rate and inert again. A tick that sent far less (a still
/// desktop) says nothing about the link, so it holds: climbing then would
/// only meet the next motion with a rate the link cannot carry (the first
/// version did, on R6e's test). Each change rebuilds the encoder, which
/// starts on a keyframe, so steps are few.
pub struct DeliveryCeiling {
    configured: u32,
    ceiling: u32,
    starved: u32,
    clean: u32,
    prev_blocked_us: u64,
    prev_bytes: u64,
    primed: bool,
}

/// The transport waited for delivery this share of a tick, or more: the
/// link is the limit.
const BLOCKED_SHARE: f64 = 0.5;

/// Waiting this share of a tick or less: the link has room.
const UNBLOCKED_SHARE: f64 = 0.1;

/// Starved ticks in a row before a cut: one is a keyframe's burst.
const STARVED_TICKS: u32 = 2;

/// Clean ticks in a row before each step back up.
const CLEAN_TICKS: u32 = 3;

/// A clean tick counts only when it sent at least this share of the
/// ceiling: one that sent less shows nothing about the link's room.
const USED_SHARE: f64 = 0.7;

/// Each step back up.
const DELIVERY_RECOVER_FACTOR: f64 = 1.25;

/// The ceiling's share of what got through: room for sound, headers and the
/// link's own wobble.
const DELIVERY_MARGIN: f64 = 0.85;

/// The lowest rate this goes to. Under [`LOSS_BITRATE_FLOOR_BPS`] the picture
/// is coarse, but a moving coarse picture beats a frozen sharp one on a link
/// that carries no more (R6f lowers the frame rate too, which keeps it
/// readable).
pub const DELIVERY_FLOOR_BPS: u32 = 250_000;

impl DeliveryCeiling {
    pub fn new(configured_bps: u32) -> Self {
        Self {
            configured: configured_bps,
            ceiling: u32::MAX,
            starved: 0,
            clean: 0,
            prev_blocked_us: 0,
            prev_bytes: 0,
            primed: false,
        }
    }

    /// Fold one tick of the transport's cumulative counters: microseconds
    /// spent waiting for delivery, and video bytes sent, over `tick`.
    pub fn sample(&mut self, blocked_us: u64, bytes_sent: u64, tick: std::time::Duration) {
        let blocked = blocked_us.saturating_sub(self.prev_blocked_us);
        let bytes = bytes_sent.saturating_sub(self.prev_bytes);
        self.prev_blocked_us = blocked_us;
        self.prev_bytes = bytes_sent;
        if !std::mem::replace(&mut self.primed, true) {
            return;
        }
        let secs = tick.as_secs_f64().max(0.001);
        let share = (blocked as f64 / 1e6 / secs).min(1.0);
        let delivered_bps = bytes as f64 * 8.0 / secs;
        (self.ceiling, self.starved, self.clean) = next_delivery_ceiling(
            self.configured,
            self.ceiling,
            self.starved,
            self.clean,
            share,
            delivered_bps,
        );
    }

    /// The ceiling to fold into the arbiter's `min` chain; [`u32::MAX`] when
    /// the link has never been the limit, or has room again.
    pub fn ceiling(&self) -> u32 {
        self.ceiling
    }
}

/// One tick of [`DeliveryCeiling::sample`], pure: the new ceiling and the
/// starved and clean streaks.
fn next_delivery_ceiling(
    configured: u32,
    ceiling: u32,
    starved: u32,
    clean: u32,
    blocked_share: f64,
    delivered_bps: f64,
) -> (u32, u32, u32) {
    let base = ceiling.min(configured);
    if blocked_share >= BLOCKED_SHARE {
        let starved = starved + 1;
        if starved < STARVED_TICKS {
            return (ceiling, starved, 0);
        }
        let through = (delivered_bps * DELIVERY_MARGIN) as u32;
        return (
            through.clamp(DELIVERY_FLOOR_BPS.min(base), base),
            starved,
            0,
        );
    }
    if blocked_share > UNBLOCKED_SHARE
        || ceiling == u32::MAX
        || delivered_bps < base as f64 * USED_SHARE
    {
        return (ceiling, 0, 0);
    }
    let clean = clean + 1;
    if clean < CLEAN_TICKS {
        return (ceiling, 0, clean);
    }
    let climbing = (base as f64 * DELIVERY_RECOVER_FACTOR) as u32;
    if climbing >= configured {
        (u32::MAX, 0, 0)
    } else {
        (climbing, 0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: std::time::Duration = std::time::Duration::from_secs(2);

    /// One tick of a transport that waited `share` of it and sent `bps`.
    fn tick(d: &mut DeliveryCeiling, waited: &mut u64, sent: &mut u64, share: f64, bps: u64) {
        *waited += (share * 2e6) as u64;
        *sent += bps * 2 / 8;
        d.sample(*waited, *sent, TICK);
    }

    #[test]
    fn a_link_that_keeps_the_transport_waiting_caps_the_rate_under_what_got_through() {
        let mut d = DeliveryCeiling::new(8_000_000);
        let (mut waited, mut sent) = (0, 0);
        d.sample(0, 0, TICK); // the baseline
        tick(&mut d, &mut waited, &mut sent, 0.9, 1_000_000);
        assert_eq!(
            d.ceiling(),
            u32::MAX,
            "one starved tick is a keyframe's burst"
        );
        tick(&mut d, &mut waited, &mut sent, 0.9, 1_000_000);
        assert_eq!(
            d.ceiling(),
            850_000,
            "just under the 1 Mbit/s that got through"
        );
        tick(&mut d, &mut waited, &mut sent, 0.8, 600_000);
        assert_eq!(
            d.ceiling(),
            510_000,
            "still waiting: down to what gets through now"
        );
    }

    #[test]
    fn a_still_desktop_sending_little_is_not_starved() {
        let mut d = DeliveryCeiling::new(8_000_000);
        let (mut waited, mut sent) = (0, 0);
        d.sample(0, 0, TICK);
        for _ in 0..10 {
            tick(&mut d, &mut waited, &mut sent, 0.0, 50_000);
        }
        assert_eq!(d.ceiling(), u32::MAX);
    }

    #[test]
    fn the_rate_climbs_back_once_the_link_carries_the_ceiling_with_room() {
        let mut d = DeliveryCeiling::new(1_000_000);
        let (mut waited, mut sent) = (0, 0);
        d.sample(0, 0, TICK);
        for _ in 0..2 {
            tick(&mut d, &mut waited, &mut sent, 1.0, 600_000);
        }
        assert_eq!(d.ceiling(), 510_000);
        for _ in 0..2 {
            tick(&mut d, &mut waited, &mut sent, 0.0, 450_000);
        }
        assert_eq!(d.ceiling(), 510_000, "two clean ticks are not enough");
        tick(&mut d, &mut waited, &mut sent, 0.0, 450_000);
        assert_eq!(d.ceiling(), 637_500, "the third steps it up a quarter");
        for _ in 0..30 {
            let using = (d.ceiling() as f64 * 0.8) as u64;
            tick(&mut d, &mut waited, &mut sent, 0.0, using.min(900_000));
        }
        assert_eq!(
            d.ceiling(),
            u32::MAX,
            "inert again above the configured rate"
        );
    }

    #[test]
    fn a_still_desktop_after_a_cut_holds_the_ceiling() {
        let mut d = DeliveryCeiling::new(8_000_000);
        let (mut waited, mut sent) = (0, 0);
        d.sample(0, 0, TICK);
        for _ in 0..2 {
            tick(&mut d, &mut waited, &mut sent, 1.0, 1_000_000);
        }
        assert_eq!(d.ceiling(), 850_000);
        for _ in 0..20 {
            tick(&mut d, &mut waited, &mut sent, 0.0, 100_000);
        }
        assert_eq!(
            d.ceiling(),
            850_000,
            "sending little shows nothing of the link's room"
        );
    }

    #[test]
    fn a_starved_link_stops_at_the_floor_and_a_wobble_holds() {
        assert_eq!(
            next_delivery_ceiling(8_000_000, 900_000, 2, 0, 1.0, 10_000.0).0,
            DELIVERY_FLOOR_BPS
        );
        assert_eq!(
            next_delivery_ceiling(8_000_000, 900_000, 0, 2, 0.3, 500_000.0),
            (900_000, 0, 0),
            "between the thresholds nothing moves, and the streaks start over"
        );
        assert_eq!(
            next_delivery_ceiling(8_000_000, u32::MAX, 1, 0, 0.9, 20_000_000.0).0,
            8_000_000,
            "never above the configured rate"
        );
    }

    #[test]
    fn loss_starts_disengaged() {
        let ceiling = LossCeiling::new(8_000_000);
        assert_eq!(
            ceiling.ceiling(),
            u32::MAX,
            "a session must begin at its configured rate, not a reduced one"
        );
    }

    #[test]
    fn a_congested_link_cuts_and_a_barely_lossy_one_is_left_alone() {
        // Counters are cumulative since the handshake, which is what `sample`
        // is fed; each call below therefore only raises them.
        let mut ceiling = LossCeiling::new(8_000_000);
        ceiling.sample(0, 0); // baseline
        // A quiet tick: nothing sent, so nothing known.
        ceiling.sample(0, 0);
        assert_eq!(ceiling.ceiling(), u32::MAX, "an idle tick is not a signal");
        // 1 packet in 200 sits between the clean and congested thresholds —
        // the deadband, where the answer is to hold still.
        ceiling.sample(1, 200);
        assert_eq!(
            ceiling.ceiling(),
            u32::MAX,
            "0.5 % loss must not move the rate: got {}",
            ceiling.ceiling()
        );
        // Now it is congested: 20 lost in the next 200 sent.
        ceiling.sample(21, 400);
        assert_eq!(ceiling.ceiling(), 6_400_000, "20 % off the configured rate");
    }

    #[test]
    fn loss_from_before_the_first_reading_is_history_not_congestion() {
        // The first tick of a WAN session: 600 of 2000 datagrams lost while
        // iroh probed paths, before a single frame was sent.
        let mut ceiling = LossCeiling::new(5_000_000);
        ceiling.sample(600, 2_000);
        assert_eq!(
            ceiling.ceiling(),
            u32::MAX,
            "the first reading is a baseline"
        );
        // A clean tick after it: nothing to undo, nothing rebuilt.
        ceiling.sample(600, 2_500);
        assert_eq!(ceiling.ceiling(), u32::MAX);
        // Loss that happens while streaming still counts.
        ceiling.sample(700, 3_000);
        assert_eq!(ceiling.ceiling(), 4_000_000);
    }

    #[test]
    fn a_still_desktops_tiny_sample_cannot_cut_the_rate() {
        // Damage-driven capture sends almost nothing when nothing moves, and
        // one lost packet out of four is 25 % by ratio. Acting on that would
        // floor a healthy LAN link the moment the user stops moving windows.
        let mut ceiling = LossCeiling::new(8_000_000);
        ceiling.sample(0, 0); // baseline
        ceiling.sample(1, 4);
        assert_eq!(ceiling.ceiling(), u32::MAX);
    }

    #[test]
    fn sustained_loss_stops_at_the_floor() {
        let mut ceiling = LossCeiling::new(8_000_000);
        for tick in 1..=40u64 {
            ceiling.sample(tick * 1_000, tick * 1_000);
        }
        assert_eq!(
            ceiling.ceiling(),
            LOSS_BITRATE_FLOOR_BPS,
            "the rate must stop where the picture stops being usable"
        );
    }

    #[test]
    fn a_configured_rate_below_the_floor_is_never_raised() {
        let mut ceiling = LossCeiling::new(500_000);
        ceiling.sample(0, 0); // baseline
        ceiling.sample(1_000, 1_000);
        assert_eq!(
            ceiling.ceiling(),
            500_000,
            "the floor cannot out-run the user's setting"
        );
    }

    #[test]
    fn loss_recovers_stepwise_and_disengages_at_the_configured_rate() {
        let mut ceiling = LossCeiling::new(8_000_000);
        ceiling.sample(0, 0); // baseline
        ceiling.sample(20, 200);
        let cut = ceiling.ceiling();
        assert_eq!(cut, 6_400_000);
        ceiling.sample(20, 400);
        assert_eq!(
            ceiling.ceiling(),
            (cut as f64 * 1.10) as u32,
            "recovery must be a step, not a jump back"
        );
        for tick in 1..=20u64 {
            ceiling.sample(20, 400 + tick * 200);
        }
        assert_eq!(
            ceiling.ceiling(),
            u32::MAX,
            "a fully recovered link must stop being a term at all"
        );
    }

    #[test]
    fn loss_reduces_from_whatever_is_already_in_force() {
        // A preset or relay clamp that already cut the rate is the starting
        // point, so the loss term can never push the encoder above a ceiling
        // the user or the path asked for.
        assert_eq!(
            next_loss_ceiling(8_000_000, 2_000_000, 1_000, 100),
            1_600_000
        );
        assert_eq!(
            next_loss_ceiling(8_000_000, u32::MAX, 1_000, 100),
            6_400_000
        );
    }

    #[test]
    fn counters_going_backwards_are_not_negative_loss() {
        // A transport that restarted its counters must not read as having
        // *un*-lost packets, and must not be mistaken for a clean link either
        // — the sample floor holds the ceiling where it was.
        let mut ceiling = LossCeiling::new(8_000_000);
        ceiling.sample(0, 0); // baseline
        ceiling.sample(500, 1_000);
        let after_cut = ceiling.ceiling();
        assert!(after_cut < 8_000_000);
        ceiling.sample(0, 0);
        assert_eq!(
            ceiling.ceiling(),
            after_cut,
            "no information must mean no change"
        );
    }
}
