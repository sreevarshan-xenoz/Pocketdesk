//! Windows audio endpoints: enumerate render devices by friendly name, read
//! and set the default output's volume, and switch the default output.
//!
//! Switching the default device has no public API. Every audio switcher uses
//! the undocumented `IPolicyConfig` COM interface, unchanged since Windows 7;
//! it is declared here and used in exactly one function, [`set_default`], so
//! a future Windows that breaks it costs device switching and nothing else.

use std::ffi::c_void;

use anyhow::{Context, Result, anyhow};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    DEVICE_STATE_ACTIVE, ERole, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX,
    eCommunications, eConsole, eMultimedia, eRender,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, STGM_READ,
};
use windows::core::{GUID, HRESULT, HSTRING, IUnknown, IUnknown_Vtbl, PCWSTR, interface};

/// One render endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Endpoint {
    /// The endpoint ID string (stable across reboots).
    pub(crate) id: String,
    /// What Windows shows the user ("Speakers (Realtek(R) Audio)").
    pub(crate) name: String,
    pub(crate) is_default: bool,
}

fn enumerator() -> Result<IMMDeviceEnumerator> {
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .context("opening the audio device enumerator")
}

fn take_string(text: windows::core::PWSTR) -> String {
    let value = unsafe { text.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(text.0 as *const c_void)) };
    value
}

fn device_id(device: &IMMDevice) -> Result<String> {
    Ok(take_string(unsafe { device.GetId() }?))
}

fn friendly_name(device: &IMMDevice) -> String {
    let named = (|| -> Result<String> {
        let store = unsafe { device.OpenPropertyStore(STGM_READ) }?;
        let value = unsafe { store.GetValue(&PKEY_Device_FriendlyName) }?;
        Ok(take_string(unsafe { PropVariantToStringAlloc(&value) }?))
    })();
    named.unwrap_or_else(|_| "Unknown device".to_string())
}

/// The default output device, where the phone's sound plays (K5).
pub(crate) fn default_output() -> Result<IMMDevice> {
    default_render(&enumerator()?)
}

fn default_render(enumerator: &IMMDeviceEnumerator) -> Result<IMMDevice> {
    unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
        .context("no default audio output device")
}

/// Every active output device, the default flagged.
pub(crate) fn outputs() -> Result<Vec<Endpoint>> {
    let enumerator = enumerator()?;
    let default = default_render(&enumerator)
        .and_then(|d| device_id(&d))
        .unwrap_or_default();
    let collection = unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) }?;
    let count = unsafe { collection.GetCount() }?;
    let mut list = Vec::new();
    for index in 0..count {
        let device = unsafe { collection.Item(index) }?;
        let id = device_id(&device)?;
        list.push(Endpoint {
            is_default: id == default,
            name: friendly_name(&device),
            id,
        });
    }
    Ok(list)
}

fn volume_of(device: &IMMDevice) -> Result<IAudioEndpointVolume> {
    unsafe { device.Activate(CLSCTX_ALL, None) }.context("opening the endpoint volume")
}

/// `(volume 0..=100, muted, default device id)` of the default output.
pub(crate) fn default_volume() -> Result<(u32, bool, String)> {
    let enumerator = enumerator()?;
    let device = default_render(&enumerator)?;
    let volume = volume_of(&device)?;
    let level = unsafe { volume.GetMasterVolumeLevelScalar() }?;
    let muted = unsafe { volume.GetMute() }?.as_bool();
    Ok((
        (level * 100.0).round().clamp(0.0, 100.0) as u32,
        muted,
        device_id(&device)?,
    ))
}

pub(crate) fn set_default_volume(percent: u32) -> Result<()> {
    let device = default_render(&enumerator()?)?;
    let level = percent.min(100) as f32 / 100.0;
    unsafe { volume_of(&device)?.SetMasterVolumeLevelScalar(level, std::ptr::null()) }?;
    Ok(())
}

pub(crate) fn set_default_muted(muted: bool) -> Result<()> {
    let device = default_render(&enumerator()?)?;
    unsafe { volume_of(&device)?.SetMute(muted, std::ptr::null()) }?;
    Ok(())
}

/// The policy-config client every Windows audio switcher talks to.
const CLSID_POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

/// `IPolicyConfig` (Windows 7+). Only `SetDefaultEndpoint` is called; the
/// methods before it are declared so its vtable slot lands where it is (the
/// names are Rust's; only the order is the ABI).
#[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
unsafe trait IPolicyConfig: IUnknown {
    fn get_mix_format(&self, id: PCWSTR, format: *mut *mut WAVEFORMATEX) -> HRESULT;
    fn get_device_format(
        &self,
        id: PCWSTR,
        default: i32,
        format: *mut *mut WAVEFORMATEX,
    ) -> HRESULT;
    fn reset_device_format(&self, id: PCWSTR) -> HRESULT;
    fn set_device_format(
        &self,
        id: PCWSTR,
        endpoint: *mut WAVEFORMATEX,
        mix: *mut WAVEFORMATEX,
    ) -> HRESULT;
    fn get_processing_period(
        &self,
        id: PCWSTR,
        default: i32,
        default_period: *mut i64,
        minimum_period: *mut i64,
    ) -> HRESULT;
    fn set_processing_period(&self, id: PCWSTR, period: *mut i64) -> HRESULT;
    fn get_share_mode(&self, id: PCWSTR, mode: *mut c_void) -> HRESULT;
    fn set_share_mode(&self, id: PCWSTR, mode: *mut c_void) -> HRESULT;
    fn get_property_value(&self, id: PCWSTR, key: *const c_void, value: *mut c_void) -> HRESULT;
    fn set_property_value(&self, id: PCWSTR, key: *const c_void, value: *mut c_void) -> HRESULT;
    fn set_default_endpoint(&self, id: PCWSTR, role: ERole) -> HRESULT;
    fn set_endpoint_visibility(&self, id: PCWSTR, visible: i32) -> HRESULT;
}

/// Make `id` the default output for every role.
pub(crate) fn set_default(id: &str) -> Result<()> {
    if !outputs()?.iter().any(|e| e.id == id) {
        return Err(anyhow!("no active output device with id {id}"));
    }
    let policy: IPolicyConfig =
        unsafe { CoCreateInstance(&CLSID_POLICY_CONFIG_CLIENT, None, CLSCTX_ALL) }
            .context("output switching is unavailable on this Windows (IPolicyConfig)")?;
    let id = HSTRING::from(id);
    for role in [eConsole, eMultimedia, eCommunications] {
        unsafe { policy.set_default_endpoint(PCWSTR(id.as_ptr()), role) }
            .ok()
            .context("switching the default output")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live: the output list is consistent and the default's volume reads.
    /// Read-only: nothing is changed.
    #[test]
    #[ignore = "needs an audio output device"]
    fn outputs_list_exactly_one_default_and_its_volume_reads() {
        let list = outputs().expect("outputs");
        assert!(!list.is_empty());
        assert_eq!(list.iter().filter(|e| e.is_default).count(), 1, "{list:?}");
        let (volume, _muted, id) = default_volume().expect("volume");
        assert!(volume <= 100);
        assert!(list.iter().any(|e| e.is_default && e.id == id));
        assert!(list.iter().all(|e| !e.name.is_empty()));
    }

    /// Live: setting the volume to its current value round-trips, and
    /// switching to the current default is accepted (a no-op on purpose).
    #[test]
    #[ignore = "touches the real output device (restores it)"]
    fn volume_and_default_round_trip_without_changing_anything() {
        let (volume, muted, id) = default_volume().unwrap();
        set_default_volume(volume).unwrap();
        set_default_muted(muted).unwrap();
        assert_eq!(default_volume().unwrap(), (volume, muted, id.clone()));
        set_default(&id).expect("IPolicyConfig switches to the current default");
        assert!(set_default("not-a-device").is_err());
    }
}
