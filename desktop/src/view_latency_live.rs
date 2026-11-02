//! The input-to-picture latency probe, phase 0 of the window handoff work:
//! how long from a pointer move sent through the computer window's relay to
//! the first decoded picture that shows it, for "another computer's screen
//! in a window" (the PC window). Telemetry only estimates this (frame age
//! plus half the round trip); this measures it.
//!
//! Two real throwaway servers side by side, A paired with B and A showing
//! B's screen, as [`crate::view_live`] starts them. B shows the test pattern
//! in its probe mode (`POCKETDESK_TEST_PATTERN=probe`): the pattern draws a
//! bright square along a reserved band at the last absolute pointer move
//! its session sent, and B injects none of that session's input anywhere,
//! so trusting A with Input here touches nothing but the pattern. A
//! stand-in for A's window reads A's view relay, feeds the window's own
//! decoder ([`crate::video`]) and every `PD_LIVE_GAP_MS` (250) sends an
//! absolute move to one of two known places, alternating; each decoded
//! NV12 picture's luma is scanned for the marker
//! ([`pocketdesk_core::streaming::latency_probe::find`]) and the time from
//! the send to the first picture showing the marker in its new place is
//! kept. Printed: p50, p95 and max over the moves that were seen, how many
//! pictures came, the misses (no marker in its new place within 1 s), the
//! codec and the decode way.
//!
//! What it measures: from the stand-in writing the move to A's relay to the
//! decoder handing over the picture, through both servers, the link between
//! them, B's pattern frame (up to one frame period at B's frame rate: the
//! marker is drawn at the next frame, as a real screen draws a pointer at
//! its next refresh), B's encoder, and the decoder. What it leaves out: the
//! window's own present (the upload and the display's scan-out), so a real
//! screen sees a little more.
//!
//! Ignored: it starts two real servers (`POCKETDESK_SERVER_EXE`, the
//! server's binary) with a real encoder, which reach iroh's relays. Run it
//! with
//! `POCKETDESK_SERVER_EXE=<target>/debug/pocketdesk cargo test -p pocketdesk-desktop input_to_picture -- --ignored --nocapture`.
//! `PD_LIVE_HEVC=1` lets B send HEVC; `PD_LIVE_N` (200) is the moves a way,
//! `PD_LIVE_GAP_MS` (250) the time between them, `PD_LIVE_WAYS` the decode
//! ways by the starts of their names (default `auto`; `software`, or a
//! VA-API node's name, as `view_live` lists them), `PD_LIVE_LOG=1` keeps
//! the servers' logs for their codec lines.
//!
//! `input_to_picture_on_a_running_pair` measures a pair that already runs,
//! across a network too (see its comment: B must be a throwaway in probe
//! mode).

use std::net::{Ipv4Addr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pocketdesk_core::local::computers::{self, ComputerAsk};
use pocketdesk_core::local::pin;
use pocketdesk_core::local::view::{self as wire, Message};
use pocketdesk_core::streaming::InputPacket;
use pocketdesk_core::streaming::latency_probe;

use crate::computer_view::percentile;
use crate::nv12::Layout;
use crate::video::{Accel, Admit, Codec, DecodeWanted, Decoder, KeyGate, Picture, Planes};
use crate::view_live::{StopViewOnDrop, ms, print_codec_lines, start_pair, wait_for, ways};

/// The two places the pointer alternates between, over the picture's width
/// (`x_norm`); far apart, so a late picture of the last place is never
/// taken for the new one.
const PLACES: [u16; 2] = [16384, 49152];
/// The pointer's height, the same each time (the marker follows its column
/// only).
const Y_NORM: u16 = 32768;
/// How long a move may wait for its picture before it is a miss.
const MISS_AFTER: Duration = Duration::from_secs(1);
/// How far, in columns, the marker found may be from where it was drawn:
/// the encoder blurs its edges a little.
const TOLERANCE: u32 = 3;

/// The place for move `i`.
fn place(i: usize) -> u16 {
    PLACES[i % PLACES.len()]
}

/// Whether a marker found at `found` (its first column) is the one drawn
/// for `expected`.
fn is_it(found: u32, expected: u32) -> bool {
    found.abs_diff(expected) <= TOLERANCE
}

/// What a way measured.
#[derive(Debug, Default)]
struct Report {
    codec: &'static str,
    size: Option<(usize, usize)>,
    pictures: u64,
    seen: Vec<Duration>,
    misses: u32,
}

impl Report {
    /// The one line that says it (`way` names the decode way).
    fn line(&mut self, way: &str) -> String {
        let p50 = percentile(&mut self.seen, 0.5);
        let p95 = percentile(&mut self.seen, 0.95);
        let max = self.seen.iter().max().copied();
        format!(
            "input to picture, {}, {way}: p50 {} p95 {} max {}, {} moves seen, {} misses, {} pictures, {:?}",
            self.codec,
            ms(p50),
            ms(p95),
            ms(max),
            self.seen.len(),
            self.misses,
            self.pictures,
            self.size
        )
    }
}

/// What the decoder's callback and the sender share.
#[derive(Default)]
struct Shared {
    /// The move sent and not yet seen: where its marker should be, and when
    /// it was sent.
    pending: Option<(u32, Instant)>,
    /// How long the last pending move took, once its picture came.
    seen: Option<Duration>,
    pictures: u64,
    size: Option<(usize, usize)>,
}

/// One picture: counted, and if it shows the pending move's marker where
/// it should be, the move is seen.
fn on_picture(shared: &Mutex<Shared>, p: &Picture, now: Instant) {
    let mut s = shared.lock().unwrap();
    s.pictures += 1;
    s.size = Some((p.width, p.height));
    if p.planes != Planes::Nv12 {
        return;
    }
    let Some((luma, _)) = Layout::new(p.width, p.height).split(&p.data) else {
        return;
    };
    if let Some((expected, sent)) = s.pending
        && let Some(found) = latency_probe::find(luma, p.width, p.height)
        && is_it(found, expected)
    {
        s.seen = Some(now - sent);
        s.pending = None;
    }
}

/// Join A's view relay as the window does, decode `accel`'s way, send `n`
/// moves `gap` apart and measure each; `None` when no picture came within
/// 30 s of the join.
fn run_way(
    a_state: &std::path::Path,
    b_id: &str,
    accel: &Accel,
    n: usize,
    gap: Duration,
) -> Option<Report> {
    let file = wait_for(30, "A's view.json for B", || {
        wire::read(a_state).filter(|f| f.device_id == b_id)
    });
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, file.port)).unwrap();
    std::io::Write::write_all(&mut stream, &file.token_bytes().unwrap()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    // The reader and the sender both write to the relay: one writer, locked.
    let writer = Arc::new(Mutex::new(stream.try_clone().unwrap()));
    let shared: Arc<Mutex<Shared>> = Arc::default();
    let stop = Arc::new(AtomicBool::new(false));
    let codec: Arc<Mutex<&'static str>> = Arc::new(Mutex::new(""));
    let reader = {
        let (writer, shared, stop, codec) =
            (writer.clone(), shared.clone(), stop.clone(), codec.clone());
        let accel = accel.clone();
        std::thread::spawn(move || {
            let mut stream = stream;
            let mut decoder: Option<Decoder> = None;
            let mut gate = KeyGate::default();
            while !stop.load(Ordering::Relaxed) {
                // A read that times out or fails ends the reader; the sender
                // finds out by its misses and the lack of pictures.
                let Ok(message) = Message::read(&mut stream) else {
                    break;
                };
                match &message {
                    Message::Config { .. } | Message::HevcConfig => {
                        let (wanted_codec, size, name) = match &message {
                            Message::Config { width, height } => {
                                (Codec::H264, Some((*width, *height)), "H.264")
                            }
                            _ => (Codec::Hevc, None, "HEVC"),
                        };
                        let wanted = DecodeWanted {
                            codec: wanted_codec,
                            size,
                            accel: accel.clone(),
                            planes: Planes::Nv12,
                        };
                        let shared = shared.clone();
                        decoder = Decoder::open(
                            &wanted,
                            Box::new(move |p| on_picture(&shared, &p, Instant::now())),
                        )
                        .ok();
                        gate = KeyGate::default();
                        *codec.lock().unwrap() = name;
                    }
                    Message::Frame {
                        keyframe,
                        seq,
                        data,
                    } => {
                        let Some(d) = decoder.as_mut() else { continue };
                        match gate.admit(*keyframe, *seq) {
                            Admit::Feed => {}
                            Admit::Skip => continue,
                            Admit::SkipAndAsk => {
                                let _ = Message::WantKeyframe.write(&mut *writer.lock().unwrap());
                                continue;
                            }
                        }
                        if d.feed(data).is_err() {
                            break;
                        }
                    }
                    Message::End { why } => panic!("the view ended: {why}"),
                    _ => {}
                }
            }
        })
    };
    // Pictures, and so the picture's size, before the first move.
    let started = Instant::now();
    loop {
        {
            let s = shared.lock().unwrap();
            if s.pictures >= 30 {
                break;
            }
        }
        if started.elapsed() > Duration::from_secs(30) {
            stop.store(true, Ordering::Relaxed);
            let _ = writer.lock().unwrap().shutdown(std::net::Shutdown::Both);
            let _ = reader.join();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let width = shared.lock().unwrap().size.unwrap().0 as u32;
    let mut report = Report::default();
    let mut next = Instant::now();
    for i in 0..n {
        let x_norm = place(i);
        let expected = latency_probe::marker_left(width, x_norm);
        {
            let mut s = shared.lock().unwrap();
            s.seen = None;
            // The send instant, taken just before the write.
            s.pending = Some((expected, Instant::now()));
        }
        let packet = InputPacket::MouseMoveAbs {
            x_norm,
            y_norm: Y_NORM,
        };
        if Message::Input(packet.encode())
            .write(&mut *writer.lock().unwrap())
            .is_err()
        {
            break;
        }
        let waited = Instant::now();
        let took = loop {
            if let Some(took) = shared.lock().unwrap().seen {
                break Some(took);
            }
            if waited.elapsed() >= MISS_AFTER {
                break None;
            }
            std::thread::sleep(Duration::from_micros(500));
        };
        match took {
            Some(took) => report.seen.push(took),
            None => {
                report.misses += 1;
                shared.lock().unwrap().pending = None;
            }
        }
        next += gap;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
    {
        let s = shared.lock().unwrap();
        report.pictures = s.pictures;
        report.size = s.size;
    }
    report.codec = *codec.lock().unwrap();
    stop.store(true, Ordering::Relaxed);
    let _ = writer.lock().unwrap().shutdown(std::net::Shutdown::Both);
    let _ = reader.join();
    Some(report)
}

/// The moves a way, the gap between them and the ways, from the
/// environment (see the top of this file).
fn settings() -> (usize, Duration, String) {
    let n = std::env::var("PD_LIVE_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);
    let gap = Duration::from_millis(
        std::env::var("PD_LIVE_GAP_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(250),
    );
    let wanted = std::env::var("PD_LIVE_WAYS").unwrap_or_else(|_| "auto".into());
    (n, gap, wanted)
}

/// Measure each wanted way on A's view of B and print its line; how many
/// ways gave pictures. A view must be running (or asked for).
fn measure_ways(a_state: &std::path::Path, b_id: &str) -> usize {
    let (n, gap, wanted) = settings();
    let mut gave = 0;
    for (name, accel, planes) in ways() {
        if planes != Planes::Nv12 || !wanted.split(',').any(|w| name.starts_with(w.trim())) {
            continue;
        }
        let Some(mut report) = run_way(a_state, b_id, &accel, n, gap) else {
            println!("input to picture, {name}: no picture");
            continue;
        };
        println!("{}", report.line(&name));
        // Most moves were seen: the marker was found where it was drawn.
        // Fewer says the probe or the stream is broken, not slow.
        assert!(
            report.seen.len() * 10 >= n * 9,
            "{name}: {} of {n} moves seen",
            report.seen.len()
        );
        gave += 1;
    }
    gave
}

#[test]
#[ignore = "starts two real servers and a real encoder, which reach iroh's relays"]
fn input_to_picture() {
    let hevc = std::env::var("PD_LIVE_HEVC").is_ok_and(|v| v == "1");
    let pair = start_pair((19516, 19616), Some(latency_probe::MODE), hevc, true);
    let (a_state, b_id) = (&pair.a_state, &pair.b_id);
    computers::request(
        a_state,
        ComputerAsk::View {
            device_id: b_id.clone(),
            monitor: 0,
            hevc: hevc && crate::video::probe_hevc("ffmpeg"),
            window: 0,
            fit: None,
        },
        pin::now_unix(),
    )
    .unwrap();
    let gave = {
        let _stop = StopViewOnDrop(a_state);
        measure_ways(a_state, b_id)
    };
    print_codec_lines(&[&pair.a_base, &pair.b_base]);
    assert!(gave > 0, "some way gave pictures");
    drop(pair);
}

/// The same on a pair already running, across a network too: B (the
/// computer shown) must run with `POCKETDESK_TEST_PATTERN=probe` (so its
/// screen is the pattern and none of A's input is injected there, which is
/// what makes it safe to let A send input), a throwaway server, and trust
/// A with Input; `PD_LIVE_A_STATE` is A's state folder, `PD_LIVE_B_ID` B's
/// device id, `PD_LIVE_HEVC=1` asks for HEVC (B needs `allow_hevc`). Never
/// point it at a server showing a real screen: the moves would be injected
/// there.
#[test]
#[ignore = "needs a running pair whose shown computer runs the test pattern in probe mode"]
fn input_to_picture_on_a_running_pair() {
    let a_state = std::path::PathBuf::from(
        std::env::var_os("PD_LIVE_A_STATE").expect("PD_LIVE_A_STATE, the viewer's state folder"),
    );
    let b_id = std::env::var("PD_LIVE_B_ID").expect("PD_LIVE_B_ID, the computer shown");
    let hevc = std::env::var("PD_LIVE_HEVC").is_ok_and(|v| v == "1");
    computers::request(
        &a_state,
        ComputerAsk::View {
            device_id: b_id.clone(),
            monitor: 0,
            hevc: hevc && crate::video::probe_hevc("ffmpeg"),
            window: 0,
            fit: None,
        },
        pin::now_unix(),
    )
    .unwrap();
    let _stop = StopViewOnDrop(&a_state);
    assert!(measure_ways(&a_state, &b_id) > 0, "some way gave pictures");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pointer_alternates_between_two_far_places() {
        assert_ne!(place(0), place(1));
        assert_eq!(place(0), place(2));
        let (a, b) = (
            latency_probe::marker_left(1280, place(0)),
            latency_probe::marker_left(1280, place(1)),
        );
        assert!(a.abs_diff(b) > 100 * TOLERANCE, "{a} and {b} are told apart");
    }

    #[test]
    fn a_found_marker_counts_when_it_is_near_where_it_was_drawn() {
        assert!(is_it(500, 500));
        assert!(is_it(503, 500));
        assert!(is_it(497, 500));
        assert!(!is_it(504, 500));
        assert!(!is_it(0, 500));
    }

    #[test]
    fn a_picture_with_the_marker_where_it_should_be_sees_the_move() {
        let (w, h) = (640usize, 360usize);
        let mut data = vec![latency_probe::DARK; Layout::new(w, h).len()];
        let left = latency_probe::marker_left(w as u32, place(0)) as usize;
        let top = latency_probe::marker_top(h as u32) as usize;
        for y in top..top + latency_probe::SIZE as usize {
            for x in left..left + latency_probe::SIZE as usize {
                data[y * w + x] = latency_probe::LIGHT;
            }
        }
        let picture = |data: Vec<u8>| Picture {
            width: w,
            height: h,
            planes: Planes::Nv12,
            data,
            colour: Default::default(),
        };
        let sent = Instant::now();
        let shared = Mutex::new(Shared {
            pending: Some((latency_probe::marker_left(w as u32, place(1)), sent)),
            ..Shared::default()
        });
        // The marker is still at the old place: the move is not seen yet.
        on_picture(&shared, &picture(data.clone()), sent + Duration::from_millis(9));
        assert!(shared.lock().unwrap().seen.is_none());
        assert_eq!(shared.lock().unwrap().pictures, 1);
        // At the new place it is, with the time since the send.
        shared.lock().unwrap().pending =
            Some((latency_probe::marker_left(w as u32, place(0)), sent));
        on_picture(&shared, &picture(data), sent + Duration::from_millis(12));
        let s = shared.lock().unwrap();
        assert_eq!(s.seen, Some(Duration::from_millis(12)));
        assert!(s.pending.is_none());
        assert_eq!(s.size, Some((w, h)));
    }

    #[test]
    fn the_report_says_the_percentiles_the_misses_and_the_way() {
        let mut report = Report {
            codec: "H.264",
            size: Some((1280, 720)),
            pictures: 900,
            seen: (1..=100).map(Duration::from_millis).collect(),
            misses: 2,
        };
        let line = report.line("auto, NV12");
        assert!(line.contains("H.264, auto, NV12"), "{line}");
        assert!(line.contains("p50 50.0 ms p95 95.0 ms max 100.0 ms"), "{line}");
        assert!(line.contains("100 moves seen, 2 misses, 900 pictures"), "{line}");
        let mut none = Report::default();
        assert!(none.line("x").contains("p50 -"));
    }
}
