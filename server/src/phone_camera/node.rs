//! The camera's PipeWire node (F6 part 2): a `Video/Source` stream with
//! `media.role = Camera` that drives its own graph, a picture each time the
//! decoder gives one, in NV12 or YUY2, whichever the app takes. It runs on
//! a thread of its own with its own connection, and goes with it: dropping
//! the [`Node`] stops the thread and disconnects, and a server that dies
//! takes the connection, and with it the node, along.

use std::io;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use pipewire::spa;
use pipewire::spa::param::ParamType;
use pipewire::spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use pipewire::spa::param::video::{VideoFormat, VideoInfoRaw};
use pipewire::spa::pod::{ChoiceValue, Object, Pod, Property, Value};
use pipewire::spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Fraction, Id, Rectangle, SpaTypes};
use pipewire::stream::{StreamFlags, StreamState};

use super::{Latest, OFFERED, PixelFormat};

/// How long a node may take to connect before it is given up on.
const OPEN_WAIT: Duration = Duration::from_secs(5);

/// How often the node's loop looks for a new picture and for its stop.
const TICK: Duration = Duration::from_millis(4);

/// A node id PipeWire has not given yet.
const NO_ID: u32 = u32::MAX;

/// Buffers the camera keeps, at least, by default and at most.
const BUFFERS: (i32, i32, i32) = (2, 4, 8);

/// A camera published as a PipeWire node.
pub(crate) struct Node {
    size: (usize, usize),
    id: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Node {
    /// Publish the camera `name`, `width` by `height` at `fps`, sending
    /// `latest`'s pictures; an error when PipeWire cannot be reached.
    pub(crate) fn open(
        name: &str,
        width: usize,
        height: usize,
        fps: u32,
        latest: Arc<Latest>,
    ) -> io::Result<Self> {
        let id = Arc::new(AtomicU32::new(NO_ID));
        let stop = Arc::new(AtomicBool::new(false));
        let (opened_tx, opened_rx) = mpsc::channel::<Result<(), String>>();
        let thread = {
            let (name, id, stop) = (name.to_string(), id.clone(), stop.clone());
            std::thread::Builder::new()
                .name("camera-node".into())
                .spawn(move || {
                    let size = (width, height);
                    if let Err(e) = run(&name, size, fps, &latest, &id, &stop, &opened_tx) {
                        let _ = opened_tx.send(Err(e));
                    }
                })?
        };
        let node = Self {
            size: (width, height),
            id,
            stop,
            thread: Some(thread),
        };
        match opened_rx.recv_timeout(OPEN_WAIT) {
            Ok(Ok(())) => {
                tracing::info!(width, height, fps, "The phone's camera is published");
                Ok(node)
            }
            // The node is dropped, its thread stopped.
            Ok(Err(e)) => Err(io::Error::other(e)),
            Err(_) => Err(io::Error::other("PipeWire did not answer")),
        }
    }

    pub(crate) fn size(&self) -> (usize, usize) {
        self.size
    }

    /// Its id in PipeWire, once given.
    pub(crate) fn id(&self) -> Option<u32> {
        Some(self.id.load(Ordering::Acquire)).filter(|id| *id != NO_ID)
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        tracing::info!("The phone's camera is no longer published");
    }
}

/// What the stream's callbacks hold.
struct Shared {
    size: (usize, usize),
    /// The layout the app took, once it took one.
    format: Option<PixelFormat>,
    latest: Arc<Latest>,
}

/// The node's thread: connect, publish, send pictures until stopped.
fn run(
    name: &str,
    size: (usize, usize),
    fps: u32,
    latest: &Arc<Latest>,
    id: &AtomicU32,
    stop: &AtomicBool,
    opened: &mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    pipewire::init();
    let mainloop =
        pipewire::main_loop::MainLoopBox::new(None).map_err(|e| format!("no main loop: {e}"))?;
    let context = pipewire::context::ContextBox::new(mainloop.loop_(), None)
        .map_err(|e| format!("no context: {e}"))?;
    let core = context
        .connect(None)
        .map_err(|e| format!("PipeWire is not running: {e}"))?;
    let props = pipewire::properties::properties! {
        *pipewire::keys::MEDIA_TYPE => "Video",
        *pipewire::keys::MEDIA_CATEGORY => "Capture",
        *pipewire::keys::MEDIA_ROLE => "Camera",
        *pipewire::keys::MEDIA_CLASS => "Video/Source",
        *pipewire::keys::NODE_NAME => "pocketdesk-camera",
        *pipewire::keys::NODE_DESCRIPTION => name,
    };
    let stream = pipewire::stream::StreamBox::new(&core, name, props)
        .map_err(|e| format!("no stream: {e}"))?;
    let shared = Shared {
        size,
        format: None,
        latest: latest.clone(),
    };
    let _listener = stream
        .add_local_listener_with_user_data(shared)
        .param_changed(|stream, shared, param_id, param| {
            if param_id != ParamType::Format.as_raw() {
                return;
            }
            let Some(param) = param else {
                // The app let the format go.
                shared.format = None;
                return;
            };
            let mut info = VideoInfoRaw::default();
            if info.parse(param).is_err() {
                return;
            }
            shared.format = taken_format(info.format());
            let Some(format) = shared.format else {
                tracing::warn!("An app took a camera format that was not offered");
                return;
            };
            tracing::info!(?format, "An app opened the phone's camera");
            let (stride, len) = format.layout(shared.size.0, shared.size.1);
            if let Some(buffers) = pod_bytes(buffers_object(stride, len))
                && let Some(pod) = Pod::from_bytes(&buffers)
            {
                let _ = stream.update_params(&mut [pod]);
            }
        })
        .process(send_picture)
        .register()
        .map_err(|e| format!("no listener: {e}"))?;
    let format = pod_bytes(format_object(size, fps)).ok_or("the format would not build")?;
    let mut params = [Pod::from_bytes(&format).ok_or("the format is not a pod")?];
    stream
        .connect(
            spa::utils::Direction::Output,
            None,
            StreamFlags::DRIVER | StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|e| format!("the camera would not connect: {e}"))?;
    let _ = opened.send(Ok(()));
    let mut sent_up_to = latest.count();
    while !stop.load(Ordering::Acquire) {
        mainloop.loop_().iterate(TICK);
        id.store(stream.node_id(), Ordering::Release);
        let decoded = latest.count();
        if decoded != sent_up_to && matches!(stream.state(), StreamState::Streaming) {
            sent_up_to = decoded;
            let _ = stream.trigger_process();
        }
    }
    let _ = stream.disconnect();
    Ok(())
}

/// The newest picture into the next buffer, in the layout the app took.
fn send_picture(stream: &pipewire::stream::Stream, shared: &mut Shared) {
    let Some(format) = shared.format else { return };
    let Some(picture) = shared.latest.take() else {
        return;
    };
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };
    let (width, height) = shared.size;
    let (stride, len) = format.layout(width, height);
    let Some(data) = buffer.datas_mut().first_mut() else {
        return;
    };
    let written = data
        .data()
        .is_some_and(|out| format.write(&picture, width, height, out));
    let chunk = data.chunk_mut();
    *chunk.offset_mut() = 0;
    *chunk.stride_mut() = i32::try_from(stride).unwrap_or(0);
    *chunk.size_mut() = if written {
        u32::try_from(len).unwrap_or(0)
    } else {
        0
    };
    if written {
        shared.latest.note_sent();
    }
}

/// The offered layout `format` names, if it is one.
pub(super) fn taken_format(format: VideoFormat) -> Option<PixelFormat> {
    if format == VideoFormat::NV12 {
        Some(PixelFormat::Nv12)
    } else if format == VideoFormat::YUY2 {
        Some(PixelFormat::Yuy2)
    } else {
        None
    }
}

fn video_format(format: PixelFormat) -> VideoFormat {
    match format {
        PixelFormat::Nv12 => VideoFormat::NV12,
        PixelFormat::Yuy2 => VideoFormat::YUY2,
    }
}

/// The formats the camera offers: raw video, NV12 or YUY2, at its one
/// size and rate.
pub(super) fn format_object((width, height): (usize, usize), fps: u32) -> Object {
    let id = |raw: u32| Id(raw);
    let formats: Vec<Id> = OFFERED
        .iter()
        .map(|f| id(video_format(*f).as_raw()))
        .collect();
    Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: vec![
            Property::new(
                FormatProperties::MediaType.as_raw(),
                Value::Id(id(MediaType::Video.as_raw())),
            ),
            Property::new(
                FormatProperties::MediaSubtype.as_raw(),
                Value::Id(id(MediaSubtype::Raw.as_raw())),
            ),
            Property::new(
                FormatProperties::VideoFormat.as_raw(),
                Value::Choice(ChoiceValue::Id(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Enum {
                        default: formats[0],
                        alternatives: formats,
                    },
                ))),
            ),
            Property::new(
                FormatProperties::VideoSize.as_raw(),
                Value::Rectangle(Rectangle {
                    width: u32::try_from(width).unwrap_or(0),
                    height: u32::try_from(height).unwrap_or(0),
                }),
            ),
            Property::new(
                FormatProperties::VideoFramerate.as_raw(),
                Value::Fraction(Fraction { num: fps, denom: 1 }),
            ),
        ],
    }
}

/// The buffers the camera fills: one block of `len` bytes, rows of
/// `stride`, in shared memory.
pub(super) fn buffers_object(stride: usize, len: usize) -> Object {
    let int = |n: usize| i32::try_from(n).unwrap_or(i32::MAX);
    let memory = (1 << spa::sys::SPA_DATA_MemFd) | (1 << spa::sys::SPA_DATA_MemPtr);
    Object {
        type_: SpaTypes::ObjectParamBuffers.as_raw(),
        id: ParamType::Buffers.as_raw(),
        properties: vec![
            Property::new(
                spa::sys::SPA_PARAM_BUFFERS_buffers,
                Value::Choice(ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range {
                        default: BUFFERS.1,
                        min: BUFFERS.0,
                        max: BUFFERS.2,
                    },
                ))),
            ),
            Property::new(spa::sys::SPA_PARAM_BUFFERS_blocks, Value::Int(1)),
            Property::new(spa::sys::SPA_PARAM_BUFFERS_size, Value::Int(int(len))),
            Property::new(spa::sys::SPA_PARAM_BUFFERS_stride, Value::Int(int(stride))),
            Property::new(
                spa::sys::SPA_PARAM_BUFFERS_dataType,
                Value::Choice(ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Flags {
                        default: memory,
                        flags: vec![memory],
                    },
                ))),
            ),
        ],
    }
}

/// An object as a pod's bytes.
pub(super) fn pod_bytes(object: Object) -> Option<Vec<u8>> {
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &Value::Object(object),
    )
    .ok()
    .map(|(cursor, _)| cursor.into_inner())
}
