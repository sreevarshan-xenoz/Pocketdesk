//! PC5's live measure of the window's decoder: two real throwaway servers
//! side by side (R6a's seams, as `server/tests/computer_view_live.rs`), A
//! paired with B, A showing B's screen; a stand-in for A's window reads
//! A's view relay and feeds the window's own decoder ([`crate::video`]),
//! for ten seconds a way, and prints the pictures that came out, the
//! decoder's time over them (median and 95th percentile) and the bytes each
//! picture brought out of the sidecar. B's trust entry for A keeps Input
//! off, so the view is view-only and nothing is sent as input at all; no
//! picture is kept, numbers only. B's screen is this computer's, or the
//! test pattern with `PD_LIVE_PATTERN=1`; `PD_LIVE_HEVC=1` lets B send
//! HEVC.
//!
//! Ignored: it starts two real servers (`POCKETDESK_SERVER_EXE`, the
//! server's binary) with a real encoder, which reach iroh's relays. Run it
//! with
//! `POCKETDESK_SERVER_EXE=<target>/debug/pocketdesk.exe cargo test -p pocketdesk-desktop view_live -- --ignored --nocapture`.

use std::io::Write;
use std::net::{Ipv4Addr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pocketdesk_core::local::computers::{self, ComputerAsk};
use pocketdesk_core::local::pair_link::PairLink;
use pocketdesk_core::local::paths::TRUST_FILE;
use pocketdesk_core::local::view::{self as wire, Message};
use pocketdesk_core::local::{identity, pin};
use pocketdesk_core::protocol::kdeconnect::TrustStore;
use pocketdesk_core::protocol::permissions::Capability;

use crate::computer_view::percentile;
use crate::video::{Accel, Admit, Codec, DecodeWanted, Decoder, KeyGate, Planes};

/// Stops the servers and removes their folders however the run ends.
struct Cleanup {
    servers: Vec<Child>,
    folders: Vec<PathBuf>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        for server in &mut self.servers {
            let _ = server.kill();
            let _ = server.wait();
        }
        for folder in &self.folders {
            let _ = std::fs::remove_dir_all(folder);
        }
    }
}

fn wait_for<T>(secs: u64, what: &str, mut ready: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(value) = ready() {
            return value;
        }
        assert!(
            start.elapsed() < Duration::from_secs(secs),
            "{what} within {secs} s"
        );
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// A throwaway server's base, config and state folders, under `pd-pc5-…`
/// in the temporary folder.
fn folders(name: &str, control: u16, extra: &str) -> (PathBuf, PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("pd-pc5-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (config, state) = (base.join("config"), base.join("state"));
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(
        config.join("config.toml"),
        format!(
            "control_port = {control}\nstreaming_port = {}\nlan_discovery = false\ndht_publish = \"off\"\n{extra}",
            control + 1
        ),
    )
    .unwrap();
    (base, config, state)
}

fn server(exe: &Path, base: &Path, config: &Path, state: &Path, pattern: bool) -> Child {
    let mut command = Command::new(exe);
    command
        .env("POCKETDESK_CONFIG_DIR", config)
        .env("POCKETDESK_STATE_DIR", state)
        .env("POCKETDESK_CLIPBOARD_FILE", base.join("clipboard.txt"))
        .env(
            "POCKETDESK_CLIPBOARD_IMAGE_FILE",
            base.join("clipboard.png"),
        )
        .env("POCKETDESK_DOWNLOAD_DIR", base.join("downloads"))
        .env("POCKETDESK_VIEW_SOUND_FILE", base.join("view-sound.wav"))
        .arg("start");
    // `PD_LIVE_LOG=1`: the server's log beside its folders, for its codec
    // lines (printed at the end, nothing else).
    if std::env::var("PD_LIVE_LOG").is_ok_and(|v| v == "1") {
        let log = std::fs::File::create(base.join("server.log")).unwrap();
        command.stdout(log.try_clone().unwrap()).stderr(log);
    } else {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    if pattern {
        command.env("POCKETDESK_TEST_PATTERN", "1");
    }
    command.spawn().unwrap()
}

/// Each picture's decode time, bytes, size and when it came.
type Came = Vec<(Duration, usize, (usize, usize), Instant)>;

/// What one way of decoding did over the measured seconds.
#[derive(Debug, Default)]
struct Measured {
    frames_in: u64,
    /// The access units' bytes over the measured seconds.
    encoded: u64,
    pictures: u64,
    bytes_per_picture: u64,
    p50: Option<Duration>,
    p95: Option<Duration>,
    size: Option<(usize, usize)>,
    codec: &'static str,
}

/// Join A's view relay as the window does and decode for `secs` after the
/// first picture, `accel`'s way to `planes`; `None` when no picture came
/// within 20 s of the decoder's start.
fn measure(
    a_state: &Path,
    b_id: &str,
    secs: u64,
    accel: &Accel,
    planes: Planes,
) -> Option<Measured> {
    let file = wait_for(30, "A's view.json for B", || {
        wire::read(a_state).filter(|f| f.device_id == b_id)
    });
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, file.port)).unwrap();
    stream.write_all(&file.token_bytes().unwrap()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut asks = stream.try_clone().unwrap();
    // Each picture's decode time, its bytes and its size, as the window's
    // publisher sees them.
    let fed: Arc<Mutex<std::collections::VecDeque<Instant>>> = Arc::default();
    let out: Arc<Mutex<Came>> = Arc::default();
    let mut decoder: Option<Decoder> = None;
    let mut gate = KeyGate::default();
    let mut measured = Measured::default();
    let begun = Instant::now();
    let mut started: Option<Instant> = None;
    let mut window: Option<Instant> = None;
    loop {
        if window.is_none() && !out.lock().unwrap().is_empty() {
            // The measure starts at the first picture.
            window = Some(Instant::now());
            out.lock().unwrap().clear();
            measured.frames_in = 0;
            measured.encoded = 0;
        }
        if window.is_some_and(|t| t.elapsed() >= Duration::from_secs(secs)) {
            break;
        }
        assert!(
            begun.elapsed() < Duration::from_secs(90),
            "no picture within 90 s"
        );
        if window.is_none() && started.is_some_and(|t| t.elapsed() > Duration::from_secs(20)) {
            return None;
        }
        let message = Message::read(&mut stream).expect("the relay");
        match &message {
            Message::Config { .. } | Message::HevcConfig => {
                fed.lock().unwrap().clear();
                let (fed, out) = (fed.clone(), out.clone());
                let (codec, size, name) = match &message {
                    Message::Config { width, height } => {
                        (Codec::H264, Some((*width, *height)), "H.264")
                    }
                    _ => (Codec::Hevc, None, "HEVC"),
                };
                let wanted = DecodeWanted {
                    codec,
                    size,
                    accel: accel.clone(),
                    planes,
                };
                decoder = Decoder::open(
                    &wanted,
                    Box::new(move |p| {
                        let now = Instant::now();
                        let took = fed
                            .lock()
                            .unwrap()
                            .pop_front()
                            .map_or(Duration::ZERO, |t| now - t);
                        out.lock()
                            .unwrap()
                            .push((took, p.data.len(), (p.width, p.height), now));
                    }),
                )
                .ok();
                started = Some(Instant::now());
                gate = KeyGate::default();
                measured.codec = name;
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
                        Message::WantKeyframe.write(&mut asks).unwrap();
                        continue;
                    }
                }
                measured.frames_in += 1;
                measured.encoded += data.len() as u64;
                fed.lock().unwrap().push_back(Instant::now());
                if d.feed(data).is_err() {
                    return None;
                }
            }
            Message::End { why } => panic!("the view ended: {why}"),
            _ => {}
        }
    }
    let got = std::mem::take(&mut *out.lock().unwrap());
    measured.pictures = got.len() as u64;
    measured.bytes_per_picture =
        got.iter().map(|g| g.1 as u64).sum::<u64>() / got.len().max(1) as u64;
    measured.size = got.last().map(|g| g.2);
    let mut took: Vec<Duration> = got.iter().map(|g| g.0).collect();
    measured.p50 = percentile(&mut took, 0.5);
    measured.p95 = percentile(&mut took, 0.95);
    Some(measured)
}

fn ms(d: Option<Duration>) -> String {
    d.map_or("-".into(), |d| {
        format!("{:.1} ms", d.as_secs_f64() * 1000.0)
    })
}

#[test]
#[ignore = "starts two real servers and a real encoder, which reach iroh's relays"]
fn the_windows_decoder_on_a_real_view() {
    let exe = PathBuf::from(
        std::env::var_os("POCKETDESK_SERVER_EXE").expect("POCKETDESK_SERVER_EXE, the server"),
    );
    let pattern = std::env::var("PD_LIVE_PATTERN").is_ok_and(|v| v == "1");
    let hevc = std::env::var("PD_LIVE_HEVC").is_ok_and(|v| v == "1");
    let secs = 10;
    let (a_control, b_control) = (19316u16, 19416u16);
    for port in [a_control, b_control] {
        assert!(
            TcpStream::connect(("127.0.0.1", port)).is_err(),
            "something already listens on {port}"
        );
    }
    let (a_base, a_config, a_state) = folders("a", a_control, "");
    let b_extra = if hevc { "allow_hevc = true\n" } else { "" };
    let (b_base, b_config, b_state) = folders("b", b_control, b_extra);
    let mut run = Cleanup {
        servers: Vec::new(),
        folders: vec![a_base.clone(), b_base.clone()],
    };
    run.servers
        .push(server(&exe, &a_base, &a_config, &a_state, false));
    run.servers
        .push(server(&exe, &b_base, &b_config, &b_state, pattern));
    for (port, state) in [(a_control, &a_state), (b_control, &b_state)] {
        wait_for(60, "the control port and the internet endpoint", || {
            (TcpStream::connect(("127.0.0.1", port)).is_ok()
                && identity::read_endpoint(state).is_some())
            .then_some(())
        });
    }
    let a_id = identity::device_id(&a_state).unwrap();
    let b_id = identity::device_id(&b_state).unwrap();
    let code = pin::mint_value();
    pin::write(&b_state, &code, pin::now_unix(), None).unwrap();
    let link = PairLink {
        name: "Computer B".to_string(),
        device_id: b_id.clone(),
        addresses: vec!["127.0.0.1".to_string()],
        control_port: b_control,
        stream_port: b_control + 1,
        pin: code,
        fingerprint: Some(identity::fingerprint_hex(
            identity::cert_fingerprint(&b_state).unwrap(),
        )),
        endpoint: identity::read_endpoint(&b_state).map(|e| e.to_string()),
        mine: false,
    };
    let asked = computers::request(
        &a_state,
        ComputerAsk::Pair {
            link: link.to_uri(),
            mine: false,
        },
        pin::now_unix(),
    )
    .unwrap();
    let paired = wait_for(60, "a pairing outcome", || {
        computers::read(&a_state)
            .last_pairing
            .filter(|p| p.request == asked.id)
            .map(|p| p.outcome)
    });
    assert_eq!(paired, "paired");
    // B lets A in without asking, and keeps A's Input and Audio off.
    let mut trust = TrustStore::read(b_state.join(TRUST_FILE)).unwrap();
    let mut permissions = trust.permissions(&a_id);
    assert!(!permissions.allows(Capability::Input));
    assert!(!permissions.allows(Capability::Audio));
    permissions.ask_first = false;
    trust.set_permissions(&a_id, permissions).unwrap();
    computers::request(
        &a_state,
        ComputerAsk::View {
            device_id: b_id.clone(),
            monitor: 0,
            // The app's own probe, as the window asks (PR #562 review I3).
            hevc: hevc && crate::video::probe_hevc("ffmpeg"),
        },
        pin::now_unix(),
    )
    .unwrap();

    let screen = if pattern {
        "test pattern"
    } else {
        "this screen"
    };
    let ways = [
        ("software, RGBA (before PC5)", Accel::Software, Planes::Rgba),
        ("software, NV12", Accel::Software, Planes::Nv12),
        ("d3d11va, NV12", Accel::D3d11va, Planes::Nv12),
        ("auto, NV12", Accel::Auto, Planes::Nv12),
    ];
    let wanted = std::env::var("PD_LIVE_WAYS").unwrap_or_default();
    for (name, accel, planes) in ways {
        if !wanted.is_empty() && !wanted.split(',').any(|w| name.starts_with(w.trim())) {
            continue;
        }
        let Some(m) = measure(&a_state, &b_id, secs, &accel, planes) else {
            println!("{screen}, {name}: no picture");
            continue;
        };
        println!(
            "{screen}, {}, {name}: {} frames in ({} KB each), {} pictures in {secs} s, decode p50 {} p95 {}, {} bytes a picture, {:?}",
            m.codec,
            m.frames_in,
            m.encoded / m.frames_in.max(1) / 1000,
            m.pictures,
            ms(m.p50),
            ms(m.p95),
            m.bytes_per_picture,
            m.size
        );
        if accel == Accel::Software {
            assert!(m.pictures > 0, "{name}: no picture");
        }
    }
    let session_id = wire::read(&a_state).map(|v| v.session_id);
    let _ = computers::request(
        &a_state,
        ComputerAsk::StopView { session_id },
        pin::now_unix(),
    );
    for base in [&a_base, &b_base] {
        let log = std::fs::read_to_string(base.join("server.log")).unwrap_or_default();
        for line in log.lines().filter(|l| {
            let l = l.to_ascii_lowercase();
            l.contains("hevc") || l.contains("codec") || l.contains("showing another")
        }) {
            let at = line.find("pocketdesk").unwrap_or(0);
            println!("log: {}", &line[at..]);
        }
    }
    assert!(
        !a_base.join("view-sound.wav").exists(),
        "no sound came to A"
    );
    drop(run);
}
