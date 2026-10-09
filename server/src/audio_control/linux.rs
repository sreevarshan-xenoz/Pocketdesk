//! Desktop audio control for the phone (Tier-3 #16).
//!
//! Volume sync + output-device routing for the desktop's default sink.
//! Hyprland IPC (`dispatch setvolume`) is deliberately NOT the mechanism:
//! socket1 write dispatchers are broken upstream since 0.56's Lua IPC
//! rewrite (hyprwm/Hyprland#16224), so we talk to the sound server directly
//! — `wpctl` (wireplumber) first, `pactl` as fallback. Both are spawning
//! CLI tools; every function here blocks, so callers use `spawn_blocking`.

use serde_json::{Value, json};
use std::process::Command;

fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{program} {:?}: exit {} {}",
            args,
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Parse `wpctl get-volume` output: `Volume: 0.4516` plus optional `MUTED`.
pub fn parse_wpctl_volume(line: &str) -> Option<(u32, bool)> {
    let muted = line.contains("MUTED");
    let frac = line.split_whitespace().nth(1)?.parse::<f64>().ok()?;
    Some(((frac * 100.0).round().clamp(0.0, 100.0) as u32, muted))
}

/// Parse one `pactl list short sinks` row: `index\tname\tdriver\t...`.
pub fn parse_sink_line(line: &str) -> Option<Value> {
    let mut cols = line.split('\t');
    let index: u32 = cols.next()?.trim().parse().ok()?;
    let name = cols.next()?.trim().to_string();
    // Description may live in a later column (pactl versions differ); take
    // the last non-driver field when present.
    let rest: Vec<&str> = cols.map(str::trim).filter(|s| !s.is_empty()).collect();
    let description = rest
        .iter()
        .rev()
        .find(|c| !c.contains('@') && !c.contains('.'))
        .map(|s| s.to_string())
        .unwrap_or_else(|| name.clone());
    Some(json!({ "index": index, "name": name, "description": description }))
}

fn default_sink_name() -> Option<String> {
    run("pactl", &["get-default-sink"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn status() -> Result<Value, String> {
    // Volume/mute: wpctl's default-sink token is the most reliable.
    let volume_line = run("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"])?;
    let (volume, muted) = parse_wpctl_volume(&volume_line)
        .ok_or_else(|| format!("unparsable wpctl get-volume output: {volume_line:?}"))?;
    let mut result = json!({ "ok": true, "volume": volume, "muted": muted });
    if let Some(default) = default_sink_name() {
        result["sink"] = json!(default);
    }
    Ok(result)
}

/// `percent` is 0..=100 (wpctl also accepts >100 for boost; we clamp).
pub fn set_volume(percent: u32) -> Result<(), String> {
    let percent = percent.min(100);
    run(
        "wpctl",
        &["set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{percent}%")],
    )?;
    Ok(())
}

pub fn set_muted(muted: bool) -> Result<(), String> {
    run(
        "wpctl",
        &[
            "set-mute",
            "@DEFAULT_AUDIO_SINK@",
            if muted { "1" } else { "0" },
        ],
    )?;
    Ok(())
}

/// Enumerate sinks with the default flagged.
pub fn sinks() -> Result<Value, String> {
    let default = default_sink_name().unwrap_or_default();
    let mut list = Vec::new();
    if let Ok(out) = run("pactl", &["list", "short", "sinks"]) {
        for line in out.lines().filter(|l| !l.trim().is_empty()) {
            if let Some(mut sink) = parse_sink_line(line) {
                let name = sink["name"].as_str().unwrap_or("").to_string();
                if let Some(obj) = sink.as_object_mut() {
                    obj.insert("isDefault".to_string(), json!(name == default));
                }
                list.push(sink);
            }
        }
    }
    if list.is_empty() {
        return Err("no sinks enumerated (pactl missing or silent)".to_string());
    }
    Ok(json!({ "ok": true, "sinks": list, "default": default }))
}

/// Route the default output to `name` (a `pactl` sink name; the index form
/// works too since pactl accepts both).
pub fn select_sink(name: String) -> Result<(), String> {
    run("pactl", &["set-default-sink", &name])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wpctl_volume_output() {
        assert_eq!(parse_wpctl_volume("Volume: 0.4516"), Some((45, false)));
        assert_eq!(
            parse_wpctl_volume("Volume: 1.0000 MUTED"),
            Some((100, true))
        );
        assert_eq!(parse_wpctl_volume("Volume: 0.0000"), Some((0, false)));
        assert_eq!(parse_wpctl_volume("garbage"), None);
    }

    #[test]
    fn parses_pactl_sink_rows() {
        let v =
            parse_sink_line("5\talsa_output.pci-0000_00_1f.3.analog-stereo\tmodule-alsa-card.c\t")
                .unwrap();
        assert_eq!(v["index"], 5);
        assert_eq!(v["name"], "alsa_output.pci-0000_00_1f.3.analog-stereo");
        assert!(parse_sink_line("not-a-row").is_none());
    }

    #[test]
    fn volume_clamps_above_100() {
        assert_eq!(parse_wpctl_volume("Volume: 1.5000"), Some((100, false)));
    }
}
