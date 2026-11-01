use super::*;

#[test]
fn a_camera_is_named_for_its_phone() {
    assert_eq!(camera_name(Some("Pixel 8")), "Pocketdesk camera (Pixel 8)");
    assert_eq!(camera_name(None), "Pocketdesk camera (phone)");
    assert_eq!(camera_name(Some("  ")), "Pocketdesk camera (phone)");
    assert_eq!(
        camera_name(Some("ab\u{202E}c\nd")),
        "Pocketdesk camera (abcd)",
        "no control or format characters"
    );
    let long = "x".repeat(100);
    assert_eq!(
        camera_name(Some(&long)).len(),
        "Pocketdesk camera ()".len() + 40
    );
}

#[test]
fn each_layout_has_its_rows_and_size() {
    assert_eq!(
        PixelFormat::Nv12.layout(1280, 720),
        (1280, 1280 * 720 * 3 / 2)
    );
    assert_eq!(PixelFormat::Yuy2.layout(1280, 720), (2560, 1280 * 720 * 2));
    assert_eq!(
        OFFERED,
        [PixelFormat::Nv12, PixelFormat::Yuy2],
        "NV12 first"
    );
}

/// A 4 by 2 NV12 picture, luma 0..8, chroma pairs (100, 200) and (101, 201).
fn tiny_nv12() -> Vec<u8> {
    let mut nv12: Vec<u8> = (0..8).collect();
    nv12.extend([100, 200, 101, 201]);
    nv12
}

#[test]
fn nv12_becomes_yuy2_pixel_pair_by_pixel_pair() {
    let nv12 = tiny_nv12();
    let mut out = vec![0u8; 16];
    assert!(PixelFormat::Yuy2.write(&nv12, 4, 2, &mut out));
    assert_eq!(
        out,
        [
            0, 100, 1, 200, 2, 101, 3, 201, // the first row
            4, 100, 5, 200, 6, 101, 7, 201, // the second shares its chroma
        ]
    );
    let mut same = vec![0u8; 12];
    assert!(PixelFormat::Nv12.write(&nv12, 4, 2, &mut same));
    assert_eq!(same, nv12);
}

#[test]
fn a_picture_of_the_wrong_size_is_not_written() {
    let nv12 = tiny_nv12();
    let mut small = vec![7u8; 15];
    assert!(!PixelFormat::Yuy2.write(&nv12, 4, 2, &mut small));
    assert!(small.iter().all(|b| *b == 7), "nothing written");
    assert!(!PixelFormat::Nv12.write(&nv12[1..], 4, 2, &mut [0u8; 12]));
    // A buffer larger than the picture takes it at its start.
    let mut large = vec![0u8; 20];
    assert!(PixelFormat::Nv12.write(&nv12, 4, 2, &mut large));
}

#[test]
fn a_camera_is_even_and_within_bounds() {
    assert_eq!(picture_size(1280, 720), Some((1280, 720)));
    assert_eq!(picture_size(1279, 721), Some((1278, 720)));
    assert_eq!(picture_size(8, 720), None);
    assert_eq!(picture_size(8192, 720), None);
}

#[test]
fn the_decoder_gives_nv12_at_the_cameras_size() {
    let args = decoder_args(1280, 720, "-fps_mode").join(" ");
    assert!(args.contains("-f h264 -i pipe:0"), "{args}");
    assert!(args.contains("-vf scale=1280:720"), "{args}");
    assert!(args.ends_with("-f rawvideo -pix_fmt nv12 pipe:1"), "{args}");
    assert!(args.contains("-fps_mode passthrough"), "{args}");
}

#[test]
fn a_camera_opens_at_its_size_and_closes_at_a_bad_one() {
    let mut life = Lifecycle::default();
    let now = Instant::now();
    assert_eq!(life.frame(true, 0, now), Step::Drop, "no size yet");
    assert_eq!(life.config(1280, 720), Step::Open(1280, 720));
    // A new encoder at the same size: the decoder starts again.
    assert_eq!(life.config(1280, 720), Step::Open(1280, 720));
    assert_eq!(life.config(1920, 1080), Step::Open(1920, 1080));
    assert_eq!(life.config(4, 4), Step::Close);
    assert_eq!(life.frame(true, 0, now), Step::Drop, "closed");
}

#[test]
fn a_camera_decodes_from_a_keyframe_and_asks_after_a_gap() {
    let mut life = Lifecycle::default();
    let now = Instant::now();
    assert_eq!(life.config(1280, 720), Step::Open(1280, 720));
    assert_eq!(life.frame(false, 0, now), Step::Drop, "before a keyframe");
    assert_eq!(life.frame(true, 1, now), Step::Decode);
    assert_eq!(life.frame(false, 2, now), Step::Decode);
    // Frame 3 lost: the chain is broken until the next keyframe.
    assert_eq!(life.frame(false, 4, now), Step::DropAndAsk);
    assert_eq!(life.frame(false, 5, now), Step::Drop);
    assert_eq!(life.frame(true, 6, now), Step::Decode);
    // A frame the decoder could not take waits for a keyframe too.
    let later = now + std::time::Duration::from_secs(1);
    assert!(life.lost(later));
    assert_eq!(life.frame(false, 7, later), Step::Drop);
    assert_eq!(life.frame(true, 8, later), Step::Decode);
}

#[test]
fn a_camera_without_a_size_takes_nothing() {
    let mut camera = PhoneCamera::new("Pocketdesk camera (test)".into(), 30);
    assert!(!camera.frame(true, 0, &[0, 0, 0, 1]));
    assert_eq!(camera.counts(), (0, 0));
    assert_eq!(camera.node_id(), None);
    assert!(matches!(camera.config(2, 2), Ok(false)), "too small");
}

/// `ffmpeg`'s H.264 of `count` 64 by 48 test pictures, one access unit a
/// picture, or `None` where there is no `ffmpeg`.
fn test_stream(count: usize) -> Option<Vec<Vec<u8>>> {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg("testsrc=size=64x48:rate=30")
        .args(["-frames:v", &count.to_string()])
        .args([
            "-c:v",
            "libx264",
            "-bf",
            "0",
            "-g",
            "30",
            "-pix_fmt",
            "yuv420p",
            "-bsf:v",
            "h264_metadata=aud=insert",
            "-f",
            "h264",
            "pipe:1",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    // Split at each access unit delimiter.
    let data = out.stdout;
    let mut starts: Vec<usize> = data
        .windows(5)
        .enumerate()
        .filter(|(_, w)| w[..4] == [0, 0, 0, 1] && w[4] & 0x1F == 9)
        .map(|(i, _)| i)
        .collect();
    starts.push(data.len());
    Some(
        starts
            .windows(2)
            .map(|w| data[w[0]..w[1]].to_vec())
            .collect(),
    )
}

#[test]
fn the_decoder_gives_a_picture_for_each_frame() {
    let Some(units) = test_stream(30) else {
        eprintln!("skipped: no ffmpeg with libx264 here");
        return;
    };
    assert_eq!(units.len(), 30);
    let latest = Arc::new(Latest::default());
    let decoder = Decoder::start(64, 48, latest.clone()).expect("ffmpeg starts");
    for unit in &units {
        while !decoder.feed(unit) {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    let deadline = Instant::now() + std::time::Duration::from_secs(10);
    while latest.count() < 29 && Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // The last may wait for the next unit's start.
    assert!(latest.count() >= 29, "decoded {}", latest.count());
    let picture = latest.take().expect("a picture");
    assert_eq!(picture.len(), PixelFormat::Nv12.layout(64, 48).1);
    drop(decoder);
}

#[cfg(target_os = "linux")]
mod pipewire_node {
    use super::super::node::{buffers_object, format_object, pod_bytes, taken_format};
    use super::*;
    use pipewire::spa::param::video::{VideoFormat, VideoInfoRaw};
    use pipewire::spa::pod::Pod;

    /// The offer reads back as raw video at the camera's size and rate,
    /// NV12 first then YUY2; an app that takes YUY2 gets YUY2.
    #[test]
    fn the_offer_names_nv12_and_yuy2_at_its_size_and_rate() {
        use pipewire::spa::param::format::FormatProperties;
        use pipewire::spa::pod::deserialize::PodDeserializer;
        use pipewire::spa::pod::{ChoiceValue, Value};
        use pipewire::spa::utils::{ChoiceEnum, Id};

        let bytes = pod_bytes(format_object((1280, 720), 30)).expect("a pod");
        assert!(Pod::from_bytes(&bytes).is_some());
        let (_, value) = PodDeserializer::deserialize_any_from(&bytes).expect("it reads back");
        let Value::Object(object) = value else {
            panic!("an object");
        };
        let property = |key: FormatProperties| {
            object
                .properties
                .iter()
                .find(|p| p.key == key.as_raw())
                .map(|p| p.value.clone())
                .expect("the property")
        };
        let Value::Choice(ChoiceValue::Id(choice)) = property(FormatProperties::VideoFormat) else {
            panic!("a choice of formats");
        };
        let ChoiceEnum::Enum {
            default,
            alternatives,
        } = choice.1
        else {
            panic!("an enum of formats");
        };
        assert_eq!(default, Id(VideoFormat::NV12.as_raw()), "NV12 first");
        assert_eq!(
            alternatives,
            vec![
                Id(VideoFormat::NV12.as_raw()),
                Id(VideoFormat::YUY2.as_raw())
            ]
        );
        let Value::Rectangle(size) = property(FormatProperties::VideoSize) else {
            panic!("a size");
        };
        assert_eq!((size.width, size.height), (1280, 720));
        let Value::Fraction(rate) = property(FormatProperties::VideoFramerate) else {
            panic!("a rate");
        };
        assert_eq!((rate.num, rate.denom), (30, 1));
        // What an app fixes on is read back as one of the two.
        assert_eq!(taken_format(VideoFormat::NV12), Some(PixelFormat::Nv12));
        assert_eq!(taken_format(VideoFormat::YUY2), Some(PixelFormat::Yuy2));
        assert_eq!(taken_format(VideoFormat::RGBA), None, "not offered");
        assert!(pod_bytes(buffers_object(2560, 1280 * 720 * 2)).is_some());
        let _ = VideoInfoRaw::default();
    }

    /// Live, against a running PipeWire (a private one: see
    /// docs/development-setup.md): the node is there while the camera is,
    /// with its name, class and role, and gone once it is dropped.
    #[test]
    #[ignore = "needs a running PipeWire"]
    fn the_node_is_there_while_the_camera_is_and_gone_after() {
        let name = "Pocketdesk camera (lifecycle test)";
        let latest = Arc::new(Latest::default());
        let node =
            super::super::node::Node::open(name, 640, 480, 30, latest).expect("PipeWire reachable");
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while node.id().is_none() && Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let id = node.id().expect("an id");
        let dump = pw_dump();
        let ours = dump
            .iter()
            .find(|o| o["id"] == id)
            .expect("the node is listed");
        let props = &ours["info"]["props"];
        assert_eq!(props["media.class"], "Video/Source");
        assert_eq!(props["media.role"], "Camera");
        assert_eq!(props["node.description"], name);
        drop(node);
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(
            !pw_dump()
                .iter()
                .any(|o| o["id"] == id && o["info"]["props"]["node.description"] == name),
            "the node is gone"
        );
    }

    /// Live, against a running PipeWire with a session manager: a cast's
    /// H.264 goes through the camera to an app that takes `format` alone,
    /// a picture for each frame, each of that layout's size.
    fn an_app_gets_each_picture(format: PixelFormat) {
        let Some(units) = test_stream(90) else {
            panic!("needs ffmpeg with libx264");
        };
        let mut camera = PhoneCamera::new("Pocketdesk camera (frames test)".into(), 30);
        assert!(camera.config(64, 48).expect("published"), "asks a keyframe");
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while camera.node_id().is_none() && Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let id = camera.node_id().expect("an id");
        let app = App::open(id, format);
        // The app links and takes a format before the frames come.
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while !app.streaming() && Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(app.streaming(), "the app is linked");
        for (seq, unit) in units.iter().enumerate() {
            // An IDR slice, after a three- or four-byte start code.
            let keyframe = unit
                .windows(4)
                .any(|w| w[..3] == [0, 0, 1] && w[3] & 0x1F == 5);
            camera.frame(keyframe, seq as u64, unit);
            // As a phone sends them, about 30 a second.
            std::thread::sleep(std::time::Duration::from_millis(33));
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        let (decoded, sent) = camera.counts();
        let (frames, sizes) = app.frames();
        eprintln!("{format:?}: decoded {decoded}, sent {sent}, the app got {frames}");
        assert!(decoded >= 89, "decoded {decoded}");
        assert!(frames >= 80, "the app got {frames} of {decoded}");
        let (_, len) = format.layout(64, 48);
        assert!(
            sizes.iter().all(|s| *s == len),
            "each a whole {format:?} picture"
        );
        drop(camera);
        drop(app);
    }

    #[test]
    #[ignore = "needs a running PipeWire and session manager"]
    fn an_app_that_takes_nv12_gets_each_picture() {
        an_app_gets_each_picture(PixelFormat::Nv12);
    }

    #[test]
    #[ignore = "needs a running PipeWire and session manager"]
    fn an_app_that_takes_only_yuy2_gets_each_picture() {
        an_app_gets_each_picture(PixelFormat::Yuy2);
    }

    /// Live, for a device check: an app opens a phone's camera already
    /// published by a running server (the first "Pocketdesk camera" node
    /// `pw-dump` lists), takes `POCKETDESK_CAMERA_FORMAT` (`nv12`, the
    /// default, or `yuy2`) for `POCKETDESK_CAMERA_SECONDS` (10), and prints
    /// how many pictures came and their sizes. Counts only: no picture is
    /// kept.
    #[test]
    #[ignore = "needs a running server publishing a phone's camera"]
    fn count_a_running_phone_cameras_frames() {
        let seconds: u64 = std::env::var("POCKETDESK_CAMERA_SECONDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);
        let format = match std::env::var("POCKETDESK_CAMERA_FORMAT").as_deref() {
            Ok("yuy2") => PixelFormat::Yuy2,
            _ => PixelFormat::Nv12,
        };
        let node = pw_dump()
            .into_iter()
            .find(|o| {
                o["info"]["props"]["node.description"]
                    .as_str()
                    .is_some_and(|d| d.starts_with("Pocketdesk camera"))
            })
            .expect("a published camera");
        let id = node["id"].as_u64().expect("its id") as u32;
        let app = App::open(id, format);
        std::thread::sleep(std::time::Duration::from_secs(seconds));
        let (frames, sizes) = app.frames();
        let mut distinct = sizes.clone();
        distinct.sort_unstable();
        distinct.dedup();
        eprintln!(
            "{format:?}: {frames} pictures in {seconds} s ({:.1} a second), sizes {distinct:?}",
            frames as f64 / seconds as f64
        );
        drop(app);
        assert!(frames > 0, "no pictures");
    }

    type AppState = Arc<Mutex<(bool, u64, Vec<usize>)>>;

    /// An app reading the camera: a PipeWire input stream on a thread.
    struct App {
        stop: Arc<std::sync::atomic::AtomicBool>,
        state: AppState,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl App {
        fn open(target: u32, format: PixelFormat) -> Self {
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let state: AppState = Arc::new(Mutex::new((false, 0u64, Vec::new())));
            let thread = {
                let (stop, state) = (stop.clone(), state.clone());
                std::thread::spawn(move || app_loop(target, format, &stop, &state))
            };
            Self {
                stop,
                state,
                thread: Some(thread),
            }
        }

        fn streaming(&self) -> bool {
            self.state.lock().unwrap().0
        }

        fn frames(&self) -> (u64, Vec<usize>) {
            let state = self.state.lock().unwrap();
            (state.1, state.2.clone())
        }
    }

    impl Drop for App {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::Release);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    fn app_loop(
        target: u32,
        format: PixelFormat,
        stop: &std::sync::atomic::AtomicBool,
        state: &AppState,
    ) {
        use pipewire::spa::param::ParamType;
        use pipewire::spa::param::format::{FormatProperties, MediaSubtype, MediaType};
        use pipewire::spa::pod::{ChoiceValue, Object, Property, Value};
        use pipewire::spa::utils::{
            Choice, ChoiceEnum, ChoiceFlags, Fraction, Id, Rectangle, SpaTypes,
        };
        pipewire::init();
        let mainloop = pipewire::main_loop::MainLoopBox::new(None).unwrap();
        let context = pipewire::context::ContextBox::new(mainloop.loop_(), None).unwrap();
        let core = context.connect(None).unwrap();
        let stream = pipewire::stream::StreamBox::new(
            &core,
            "pocketdesk-camera-test-app",
            pipewire::properties::properties! {
                *pipewire::keys::MEDIA_TYPE => "Video",
                *pipewire::keys::MEDIA_CATEGORY => "Capture",
                *pipewire::keys::MEDIA_ROLE => "Camera",
            },
        )
        .unwrap();
        let _listener = stream
            .add_local_listener_with_user_data(state.clone())
            .state_changed(|_, shared, _, new| {
                shared.lock().unwrap().0 = matches!(new, pipewire::stream::StreamState::Streaming);
            })
            .process(|stream, shared| {
                if let Some(mut buffer) = stream.dequeue_buffer()
                    && let Some(data) = buffer.datas_mut().first_mut()
                {
                    let size = data.chunk().size() as usize;
                    if size > 0 {
                        let mut s = shared.lock().unwrap();
                        s.1 += 1;
                        s.2.push(size);
                    }
                }
            })
            .register()
            .unwrap();
        let raw = match format {
            PixelFormat::Nv12 => VideoFormat::NV12,
            PixelFormat::Yuy2 => VideoFormat::YUY2,
        };
        let range = |default: Rectangle| {
            Value::Choice(ChoiceValue::Rectangle(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Range {
                    default,
                    min: Rectangle {
                        width: 1,
                        height: 1,
                    },
                    max: Rectangle {
                        width: 4096,
                        height: 4096,
                    },
                },
            )))
        };
        let rates = Value::Choice(ChoiceValue::Fraction(Choice(
            ChoiceFlags::empty(),
            ChoiceEnum::Range {
                default: Fraction { num: 30, denom: 1 },
                min: Fraction { num: 0, denom: 1 },
                max: Fraction { num: 120, denom: 1 },
            },
        )));
        let wants = Object {
            type_: SpaTypes::ObjectParamFormat.as_raw(),
            id: ParamType::EnumFormat.as_raw(),
            properties: vec![
                Property::new(
                    FormatProperties::MediaType.as_raw(),
                    Value::Id(Id(MediaType::Video.as_raw())),
                ),
                Property::new(
                    FormatProperties::MediaSubtype.as_raw(),
                    Value::Id(Id(MediaSubtype::Raw.as_raw())),
                ),
                Property::new(
                    FormatProperties::VideoFormat.as_raw(),
                    Value::Id(Id(raw.as_raw())),
                ),
                Property::new(
                    FormatProperties::VideoSize.as_raw(),
                    range(Rectangle {
                        width: 64,
                        height: 48,
                    }),
                ),
                Property::new(FormatProperties::VideoFramerate.as_raw(), rates),
            ],
        };
        let bytes = pod_bytes(wants).unwrap();
        let mut params = [Pod::from_bytes(&bytes).unwrap()];
        stream
            .connect(
                pipewire::spa::utils::Direction::Input,
                Some(target),
                pipewire::stream::StreamFlags::AUTOCONNECT
                    | pipewire::stream::StreamFlags::MAP_BUFFERS,
                &mut params,
            )
            .unwrap();
        while !stop.load(std::sync::atomic::Ordering::Acquire) {
            mainloop
                .loop_()
                .iterate(std::time::Duration::from_millis(5));
        }
        let _ = stream.disconnect();
    }

    fn pw_dump() -> Vec<serde_json::Value> {
        let out = Command::new("pw-dump").output().expect("pw-dump runs");
        serde_json::from_slice(&out.stdout).expect("pw-dump's JSON")
    }
}
