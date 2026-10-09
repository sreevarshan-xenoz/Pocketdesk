//! Windows backend for [`super`]: Core Audio endpoints, answering with the
//! same JSON the Linux backend produces (`sink`/`name` carry the endpoint ID,
//! `description` the name Windows shows).

use serde_json::{Value, json};

use crate::platform::audio_windows as endpoints;

fn reason(e: anyhow::Error) -> String {
    format!("{e:#}")
}

pub fn status() -> Result<Value, String> {
    let (volume, muted, default) = endpoints::default_volume().map_err(reason)?;
    Ok(json!({ "ok": true, "volume": volume, "muted": muted, "sink": default }))
}

/// `percent` is 0..=100.
pub fn set_volume(percent: u32) -> Result<(), String> {
    endpoints::set_default_volume(percent).map_err(reason)
}

pub fn set_muted(muted: bool) -> Result<(), String> {
    endpoints::set_default_muted(muted).map_err(reason)
}

/// Enumerate outputs with the default flagged.
pub fn sinks() -> Result<Value, String> {
    let outputs = endpoints::outputs().map_err(reason)?;
    if outputs.is_empty() {
        return Err("no active audio output devices".to_string());
    }
    let default = outputs
        .iter()
        .find(|e| e.is_default)
        .map(|e| e.id.clone())
        .unwrap_or_default();
    let list: Vec<Value> = outputs
        .iter()
        .enumerate()
        .map(|(index, e)| {
            json!({
                "index": index,
                "name": e.id,
                "description": e.name,
                "isDefault": e.is_default,
            })
        })
        .collect();
    Ok(json!({ "ok": true, "sinks": list, "default": default }))
}

/// Route the default output to the endpoint whose ID is `name`.
pub fn select_sink(name: String) -> Result<(), String> {
    endpoints::set_default(&name).map_err(reason)
}
