//! A live probe of the portal capture (N8): the screen through the portal,
//! and, with `PD_PORTAL_PROBE_INPUT` set, input through its RemoteDesktop
//! session. It prints the start time, the frames counted and which restore
//! tokens were kept; run it twice with the same folder to see whether the
//! grant is restored with no dialog. Meant for a throwaway desktop session (a
//! nested KWin or GNOME in a container or VM): the input lands in whatever
//! session the portal serves.
//!
//! `PD_PORTAL_PROBE_NAMED` names the probe to the portal first as the server
//! does ([`portal_grant::register_app`]; the desktop file must be installed).
//!
//! `PD_PORTAL_PROBE_ACTIONS` replaces the default input with a list, comma
//! separated: `click:X:Y` (0 to 1 across the screen), `key:CODE` (evdev),
//! `scroll:STEPS`, `wait:MS` and `snap:PATH` (the newest frame as a PPM).
//!
//! `PD_PORTAL_PROBE_DIR=/tmp/grant cargo test -p pocketdesk-core --test
//! portal_live -- --ignored --nocapture`
#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use pocketdesk_core::portal_grant;
use pocketdesk_core::streaming::portal_input::{self, PortalEvent};
use pocketdesk_core::streaming::{StreamingConfig, VideoFrame, capture_linux, latest_frame};
use tokio_util::sync::CancellationToken;

/// The newest frame as a binary PPM, reading BGRx.
fn snap(frame: &VideoFrame, path: &str) {
    let mut out = format!("P6\n{} {}\n255\n", frame.width, frame.height).into_bytes();
    for row in 0..frame.height as usize {
        let start = row * frame.stride as usize;
        for col in 0..frame.width as usize {
            let at = start + col * 4;
            out.extend_from_slice(&[frame.data[at + 2], frame.data[at + 1], frame.data[at]]);
        }
    }
    std::fs::write(path, out).unwrap();
}

/// Frames for `for_`, keeping the newest.
fn count(
    rx: &mut latest_frame::FrameReceiver<VideoFrame>,
    newest: &mut Option<VideoFrame>,
    for_: Duration,
) -> u32 {
    let until = Instant::now() + for_;
    let mut frames = 0u32;
    while Instant::now() < until {
        if let Some(frame) = rx.try_recv() {
            frames += 1;
            *newest = Some(frame);
        } else {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    frames
}

fn norm(v: &str) -> u16 {
    (v.parse::<f64>().unwrap().clamp(0.0, 1.0) * 65535.0) as u16
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs a desktop session with its screen-sharing portal"]
async fn the_portal_shares_the_screen_and_takes_input() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    let dir = PathBuf::from(std::env::var("PD_PORTAL_PROBE_DIR").expect("PD_PORTAL_PROBE_DIR"));
    std::fs::create_dir_all(&dir).unwrap();
    if std::env::var_os("PD_PORTAL_PROBE_NAMED").is_some() {
        println!("NAMED {}", portal_grant::register_app().await);
    }
    let input = std::env::var_os("PD_PORTAL_PROBE_INPUT").is_some();
    portal_input::want(input);
    println!(
        "TOKENS_BEFORE screen={} remote={}",
        portal_grant::read(&dir).is_some(),
        portal_grant::read_remote(&dir).is_some()
    );

    let (tx, mut rx) = latest_frame::channel();
    let cancel = CancellationToken::new();
    let started = Instant::now();
    let session = capture_linux::start_capture(
        StreamingConfig::default(),
        tx,
        cancel.clone(),
        Some(dir.clone()),
    )
    .await
    .expect("the capture starts");
    println!("START_SECS {:.1}", started.elapsed().as_secs_f32());

    let mut newest: Option<VideoFrame> = None;
    println!(
        "FRAMES {}",
        count(&mut rx, &mut newest, Duration::from_secs(3))
    );
    if let Some(frame) = &newest {
        println!("FRAME_SIZE {}x{}", frame.width, frame.height);
    }

    if input {
        let actions = std::env::var("PD_PORTAL_PROBE_ACTIONS")
            .unwrap_or_else(|_| "click:0.5:0.5,scroll:1,key:30,text:é".to_string());
        let mut sent = 0;
        for action in actions.split(',') {
            let parts: Vec<&str> = action.splitn(3, ':').collect();
            let events: Vec<PortalEvent> = match parts.as_slice() {
                ["click", x, y] => vec![
                    PortalEvent::Absolute {
                        x_norm: norm(x),
                        y_norm: norm(y),
                    },
                    PortalEvent::Button {
                        code: 0x110,
                        pressed: true,
                    },
                    PortalEvent::Button {
                        code: 0x110,
                        pressed: false,
                    },
                ],
                ["scroll", steps] => vec![PortalEvent::Scroll {
                    axis: 0,
                    steps: steps.parse().unwrap(),
                }],
                ["key", code] => {
                    let code = code.parse().unwrap();
                    vec![
                        PortalEvent::Key {
                            code,
                            pressed: true,
                        },
                        PortalEvent::Key {
                            code,
                            pressed: false,
                        },
                    ]
                }
                ["text", text] => portal_input::text_events(text),
                ["wait", ms] => {
                    count(
                        &mut rx,
                        &mut newest,
                        Duration::from_millis(ms.parse().unwrap()),
                    );
                    vec![]
                }
                ["snap", path] => {
                    count(&mut rx, &mut newest, Duration::from_millis(300));
                    match &newest {
                        Some(frame) => snap(frame, path),
                        None => println!("NO_FRAME_TO_SNAP"),
                    }
                    vec![]
                }
                other => panic!("unknown action {other:?}"),
            };
            for event in events {
                sent += usize::from(portal_input::send(event));
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        println!("INPUT_SENT {sent}");
        println!(
            "FRAMES_AFTER_INPUT {}",
            count(&mut rx, &mut newest, Duration::from_secs(1))
        );
    }

    cancel.cancel();
    drop(session);
    tokio::time::sleep(Duration::from_millis(500)).await;
    println!(
        "TOKENS_AFTER screen={} remote={}",
        portal_grant::read(&dir).is_some(),
        portal_grant::read_remote(&dir).is_some()
    );
}
