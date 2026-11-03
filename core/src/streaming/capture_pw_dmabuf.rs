//! The PipeWire capture's DMA-BUF side (portal ScreenCast and mutter's own
//! ScreenCast): the compositor renders into buffers on the card and the
//! in-process VA-API encoder imports each one where it is, as the wlroots
//! capture's buffers are (`super::dmabuf` says why; `capture_screencopy`'s
//! `zero_copy` is the same idea with the capture as the allocator).
//!
//! What differs from wlroots: here the *compositor* allocates, so the capture
//! only says what it can take and checks what comes.
//!
//! 1. **Offer.** Before the stream connects, the card is asked which
//!    modifiers it can render to for XRGB8888 (EGL), each is tried on a
//!    buffer of this program's own (GBM) through the VA-API importer and the
//!    video processor, and the ones that work, tiled before linear and
//!    single-plane only, go to the compositor as a `DmaBuf` format with a
//!    modifier choice. The memory format (no modifier) is offered after it,
//!    so a compositor that cannot do DMA-BUF still gives memory.
//! 2. **Fixate.** A compositor that leaves the modifier open for the
//!    consumer to pick gets the first of ours it listed; one that fixes it
//!    itself is checked against ours. Either way the buffers are then asked
//!    for as `SPA_DATA_DmaBuf`, a few more than a copy needs because a frame
//!    holds its buffer until the encoder is done with it.
//! 3. **Each new buffer** is imported by this program's VA-API device and run
//!    through the video processor the first time it arrives
//!    ([`PwDma::take`]), before any frame of it goes to the encoder.
//! 4. **A frame is a lease on its stream buffer.** The buffer is not queued
//!    back to PipeWire when `process` returns; the lease the frame carries
//!    gives it back (on the stream's own thread, through an eventfd the main
//!    loop wakes on) once the last holder lets go, which is after the encoder
//!    has waited for the card (`vaSyncSurface`, `encoder_vaapi`'s `settle`).
//!
//! Any failure on the way (no libEGL/libgbm, no modifier the card takes, a
//! compositor on another GPU, a buffer the card refuses, a layout that is not
//! one plane) turns the capture back to memory for the rest of its session,
//! with one warning: the stream is asked again with the memory format only.
//! The same happens when the streamer needs the pixels (another encoder, a
//! crop, an odd size: [`super::dmabuf::needs_pixels`]) and clears the
//! capture's switch. A lent window's stream (`SPA_META_VideoCrop`) is always a
//! copy: the picture is cut to the window's rectangle on the processor, and
//! cutting on the card (`scale_vaapi`'s crop) is not built.
//! `capture_copy` / `POCKETDESK_CAPTURE_COPY` force copies, as for wlroots.

#![cfg_attr(not(feature = "inproc"), allow(dead_code))]

use libspa::pod::{ChoiceValue, Value};
use libspa::utils::ChoiceEnum;

/// What a Format param says about its modifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ModifierProp {
    /// One modifier: the peer fixed it.
    Fixed(u64),
    /// A choice the peer left for the consumer to pick from.
    Open(Vec<u64>),
}

/// What the capture does with a Format that was negotiated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Step {
    /// No modifier: the peer settled on memory buffers, as before.
    Memory,
    /// The modifier is open: fix it to this one and say so.
    Fixate(u64),
    /// The modifier is fixed and is one the capture takes: ask for the
    /// DMA-BUF buffers.
    Ready(u64),
    /// DMA-BUF is not to be used: ask again with the memory format only. The
    /// text is the warning; none means the streamer asked for copies, which
    /// needs none.
    GiveUp(Option<String>),
}

/// The modifier of an `EnumFormat`/`Format` pod, when it carries one.
pub(super) fn modifier_prop(param: &libspa::pod::Pod) -> Option<ModifierProp> {
    use libspa::param::format::FormatProperties;
    let (_, value) =
        libspa::pod::deserialize::PodDeserializer::deserialize_any_from(param.as_bytes()).ok()?;
    let Value::Object(object) = value else {
        return None;
    };
    let prop = object
        .properties
        .iter()
        .find(|p| p.key == FormatProperties::VideoModifier.as_raw())?;
    match &prop.value {
        Value::Long(m) => Some(ModifierProp::Fixed(*m as u64)),
        Value::Choice(ChoiceValue::Long(libspa::utils::Choice(_, choice))) => match choice {
            ChoiceEnum::None(m) => Some(ModifierProp::Fixed(*m as u64)),
            ChoiceEnum::Enum {
                default,
                alternatives,
            } => {
                let mut all = vec![*default as u64];
                for a in alternatives {
                    let a = *a as u64;
                    if !all.contains(&a) {
                        all.push(a);
                    }
                }
                Some(ModifierProp::Open(all))
            }
            ChoiceEnum::Range { default, .. } | ChoiceEnum::Step { default, .. } => {
                Some(ModifierProp::Open(vec![*default as u64]))
            }
            ChoiceEnum::Flags { default, .. } => Some(ModifierProp::Open(vec![*default as u64])),
        },
        _ => None,
    }
}

/// The first of `ours` (in our order: tiled first) that `theirs` lists.
pub(super) fn choose_modifier(theirs: &[u64], ours: &[u64]) -> Option<u64> {
    ours.iter().copied().find(|m| theirs.contains(m))
}

/// What to do with the modifier property of a negotiated Format. `ours` is the
/// list the capture offered; `copies_asked` is whether the streamer has
/// already asked for copies.
pub(super) fn decide(prop: Option<&ModifierProp>, ours: &[u64], copies_asked: bool) -> Step {
    let Some(prop) = prop else {
        return Step::Memory;
    };
    if copies_asked {
        return Step::GiveUp(None);
    }
    match prop {
        ModifierProp::Fixed(m) if ours.contains(m) => Step::Ready(*m),
        ModifierProp::Fixed(m) => Step::GiveUp(Some(format!(
            "the compositor chose DMA-BUF modifier {m:#x}, which the card did not take"
        ))),
        ModifierProp::Open(theirs) => match choose_modifier(theirs, ours) {
            Some(m) => Step::Fixate(m),
            None => Step::GiveUp(Some(
                "the compositor offers no DMA-BUF modifier the card takes".into(),
            )),
        },
    }
}

/// How many buffers the stream asks for in DMA-BUF mode: one the compositor
/// fills, one waiting for the encoder, one being encoded, the streamer's held
/// and last frames, and a spare.
pub(super) const BUFFERS: i32 = 6;

/// What kind of buffers a `Buffers` param asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// One plane of `DmaBuf`, a few more than a copy needs.
    DmaBuf,
    /// Memory (`MemFd` or `MemPtr`), as the capture asked for before: sent
    /// when DMA-BUF was in use and is given up, so the stream no longer
    /// demands the DMA-BUF buffers it asked for.
    Memory,
}

/// The `Buffers` param of `kind`.
pub(super) fn buffers_param_pod(kind: Kind) -> anyhow::Result<Vec<u8>> {
    use libspa::pod::{Object, Property, Value};
    use libspa::sys as spa;
    use libspa::utils::{Choice, ChoiceEnum, ChoiceFlags, SpaTypes};
    let int_range = |default: i32, min: i32, max: i32| {
        Value::Choice(ChoiceValue::Int(Choice(
            ChoiceFlags::empty(),
            ChoiceEnum::Range { default, min, max },
        )))
    };
    let (count, types) = match kind {
        Kind::DmaBuf => (int_range(BUFFERS, 4, 12), 1i32 << spa::SPA_DATA_DmaBuf),
        Kind::Memory => (
            int_range(4, 2, 16),
            (1i32 << spa::SPA_DATA_MemFd) | (1i32 << spa::SPA_DATA_MemPtr),
        ),
    };
    let object = Object {
        type_: SpaTypes::ObjectParamBuffers.as_raw(),
        id: libspa::param::ParamType::Buffers.as_raw(),
        properties: vec![
            Property::new(spa::SPA_PARAM_BUFFERS_buffers, count),
            Property::new(spa::SPA_PARAM_BUFFERS_blocks, Value::Int(1)),
            Property::new(
                spa::SPA_PARAM_BUFFERS_dataType,
                Value::Choice(ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Flags {
                        default: types,
                        flags: vec![types],
                    },
                ))),
            ),
        ],
    };
    let (bytes, _) = libspa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &Value::Object(object),
    )
    .map_err(|e| anyhow::anyhow!("The buffers param did not serialize: {e:?}"))?;
    Ok(bytes.into_inner())
}

#[cfg(feature = "inproc")]
mod real;
#[cfg(feature = "inproc")]
pub(super) use real::{PwDma, Take};

/// Without the in-process encoders (the portable tarball) nothing could take
/// a buffer, so the capture always copies and these are never made.
#[cfg(not(feature = "inproc"))]
pub(super) enum PwDma {}

#[cfg(not(feature = "inproc"))]
#[allow(dead_code)]
pub(super) enum Take {
    Frame(super::GpuFrame),
    Copy,
    Nothing,
    Drop,
}

#[cfg(not(feature = "inproc"))]
#[allow(dead_code)]
impl PwDma {
    pub(super) fn open(
        _config: &super::StreamingConfig,
        switch: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Option<std::sync::Arc<Self>> {
        switch.store(false, std::sync::atomic::Ordering::Relaxed);
        None
    }
    pub(super) fn modifiers(&self) -> &[u64] {
        match *self {}
    }
    pub(super) fn step(&self, _prop: Option<&ModifierProp>) -> Step {
        match *self {}
    }
    pub(super) fn buffer_added(&self, _ptr: *mut pipewire::sys::pw_buffer) {
        match *self {}
    }
    pub(super) fn buffer_removed(&self, _ptr: *mut pipewire::sys::pw_buffer) {
        match *self {}
    }
    pub(super) fn take(
        &self,
        _ptr: *mut pipewire::sys::pw_buffer,
        _width: u32,
        _height: u32,
    ) -> Take {
        match *self {}
    }
    pub(super) fn plain_wanted(&self) -> bool {
        match *self {}
    }
    pub(super) fn returned(&self) -> Vec<*mut pipewire::sys::pw_buffer> {
        match *self {}
    }
    pub(super) fn wake_fd(&self) -> anyhow::Result<std::os::fd::OwnedFd> {
        match *self {}
    }
    pub(super) fn close(&self) {
        match *self {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINEAR: u64 = 0;
    const X_TILED: u64 = 0x0100_0000_0000_0001;
    const Y_TILED: u64 = 0x0100_0000_0000_0002;

    #[test]
    fn an_open_choice_is_fixed_to_the_first_of_ours_it_lists() {
        let ours = [Y_TILED, X_TILED, LINEAR];
        assert_eq!(
            decide(
                Some(&ModifierProp::Open(vec![LINEAR, X_TILED])),
                &ours,
                false
            ),
            Step::Fixate(X_TILED)
        );
        // Our order wins, not theirs.
        assert_eq!(
            decide(
                Some(&ModifierProp::Open(vec![LINEAR, X_TILED, Y_TILED])),
                &ours,
                false
            ),
            Step::Fixate(Y_TILED)
        );
        assert!(matches!(
            decide(Some(&ModifierProp::Open(vec![5, 6])), &ours, false),
            Step::GiveUp(Some(_))
        ));
    }

    #[test]
    fn a_fixed_modifier_is_taken_only_when_it_is_one_of_ours() {
        let ours = [X_TILED, LINEAR];
        assert_eq!(
            decide(Some(&ModifierProp::Fixed(X_TILED)), &ours, false),
            Step::Ready(X_TILED)
        );
        assert!(matches!(
            decide(Some(&ModifierProp::Fixed(Y_TILED)), &ours, false),
            Step::GiveUp(Some(_))
        ));
    }

    #[test]
    fn no_modifier_is_memory_and_asked_copies_give_up_without_a_warning() {
        let ours = [LINEAR];
        assert_eq!(decide(None, &ours, false), Step::Memory);
        assert_eq!(decide(None, &ours, true), Step::Memory);
        assert_eq!(
            decide(Some(&ModifierProp::Fixed(LINEAR)), &ours, true),
            Step::GiveUp(None)
        );
        assert_eq!(
            decide(Some(&ModifierProp::Open(vec![LINEAR])), &ours, true),
            Step::GiveUp(None)
        );
    }

    #[test]
    fn the_buffers_param_asks_for_dma_buf_only_and_several_of_them() {
        use libspa::sys as spa;
        let bytes = buffers_param_pod(Kind::DmaBuf).unwrap();
        let (_, value) =
            libspa::pod::deserialize::PodDeserializer::deserialize_any_from(&bytes).unwrap();
        let Value::Object(object) = value else {
            panic!("not an object");
        };
        assert_eq!(object.id, libspa::param::ParamType::Buffers.as_raw());
        let find = |key: u32| object.properties.iter().find(|p| p.key == key).unwrap();
        let Value::Choice(ChoiceValue::Int(libspa::utils::Choice(
            _,
            ChoiceEnum::Range { default, min, .. },
        ))) = &find(spa::SPA_PARAM_BUFFERS_buffers).value
        else {
            panic!("the count is not a range");
        };
        assert_eq!(*default, BUFFERS);
        assert!(*min >= 4, "a frame in flight, one encoding, one filling");
        assert_eq!(find(spa::SPA_PARAM_BUFFERS_blocks).value, Value::Int(1));
        let Value::Choice(ChoiceValue::Int(libspa::utils::Choice(
            _,
            ChoiceEnum::Flags { default, .. },
        ))) = &find(spa::SPA_PARAM_BUFFERS_dataType).value
        else {
            panic!("the data type is not flags");
        };
        assert_eq!(*default, 1 << spa::SPA_DATA_DmaBuf);

        // The memory kind names the two memory types and not DmaBuf.
        let bytes = buffers_param_pod(Kind::Memory).unwrap();
        let (_, value) =
            libspa::pod::deserialize::PodDeserializer::deserialize_any_from(&bytes).unwrap();
        let Value::Object(object) = value else {
            panic!("not an object");
        };
        let types = object
            .properties
            .iter()
            .find(|p| p.key == spa::SPA_PARAM_BUFFERS_dataType)
            .unwrap();
        let Value::Choice(ChoiceValue::Int(libspa::utils::Choice(
            _,
            ChoiceEnum::Flags { default, .. },
        ))) = &types.value
        else {
            panic!("the data type is not flags");
        };
        assert_eq!(*default & (1 << spa::SPA_DATA_DmaBuf), 0);
        assert_ne!(*default & (1 << spa::SPA_DATA_MemFd), 0);
        assert_ne!(*default & (1 << spa::SPA_DATA_MemPtr), 0);
    }
}
