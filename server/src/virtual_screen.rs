//! A second screen at the phone's shape (T10j): a phone asks
//! (`kdeconnect.pocketdesk.virtual_screen`), the server makes an output of
//! the phone's size to the right of the others and answers with its monitor
//! index, and the session switches to it. Sway only, through its IPC
//! (`create_output`, `output … mode --custom`, `output … unplug`); elsewhere
//! the phone is told the computer cannot. An output lasts while its phone
//! has a session, and one left by a server that stopped goes at the next
//! start (`virtual_screens.json`).

use std::collections::BTreeMap;
use std::sync::Mutex;

use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use serde::{Deserialize, Serialize};

pub const PACKET: &str = "kdeconnect.pocketdesk.virtual_screen";

/// The longest side a screen gets, in pixels; a phone's own is scaled down.
pub const MAX_SIDE: u32 = 2560;
/// The shortest side a screen gets.
pub const MIN_SIDE: u32 = 320;

/// The file the outputs made are listed in, so a restart removes them.
const STATE_FILE: &str = "virtual_screens.json";

/// The size a screen for a phone of `width`×`height` gets: its shape, the
/// long side at most [`MAX_SIDE`], both sides even (encoders want it) and
/// at least [`MIN_SIDE`]; `None` for a size that is no screen.
pub fn screen_size(width: u32, height: u32) -> Option<(u32, u32)> {
    if width == 0 || height == 0 {
        return None;
    }
    let long = width.max(height);
    let (w, h) = if long > MAX_SIDE {
        let scale = f64::from(MAX_SIDE) / f64::from(long);
        (
            (f64::from(width) * scale).round() as u32,
            (f64::from(height) * scale).round() as u32,
        )
    } else {
        (width, height)
    };
    let even = |v: u32| (v.max(MIN_SIDE) / 2) * 2;
    Some((even(w), even(h)))
}

/// An output as Sway's `get_outputs` describes it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Output {
    pub name: String,
    #[serde(default)]
    pub rect: Rect,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Whether `name` is an output Sway makes with `create_output` (its
/// headless backend): never a real monitor's connector, so nothing here can
/// unplug one.
pub fn is_virtual(name: &str) -> bool {
    name.strip_prefix("HEADLESS-")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The output `create_output` added: the one virtual output in `after` and
/// not in `before`; `None` when there is not exactly one (a monitor plugged
/// in at the same moment is never taken for it).
pub fn new_output(before: &[Output], after: &[Output]) -> Option<String> {
    let mut new = after
        .iter()
        .filter(|o| !before.iter().any(|b| b.name == o.name));
    match (new.next(), new.next()) {
        (Some(o), None) if is_virtual(&o.name) => Some(o.name.clone()),
        _ => None,
    }
}

/// Where a new screen goes: right of every other output.
pub fn right_edge(outputs: &[Output]) -> i32 {
    outputs
        .iter()
        .map(|o| o.rect.x + o.rect.width)
        .max()
        .unwrap_or(0)
}

/// An i3-IPC message: the magic, the payload's length and the type (both
/// little-endian on every platform Sway runs on), then the payload.
pub fn ipc_frame(kind: u32, payload: &str) -> Vec<u8> {
    let mut frame = Vec::with_capacity(14 + payload.len());
    frame.extend_from_slice(b"i3-ipc");
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&kind.to_le_bytes());
    frame.extend_from_slice(payload.as_bytes());
    frame
}

/// i3-IPC `RUN_COMMAND`.
const RUN_COMMAND: u32 = 0;
/// i3-IPC `GET_OUTPUTS`.
const GET_OUTPUTS: u32 = 3;

/// Send one i3-IPC message to Sway and read its reply.
#[cfg(unix)]
fn ipc(kind: u32, payload: &str) -> std::io::Result<serde_json::Value> {
    use std::io::{Read, Write};
    let socket = std::env::var_os("SWAYSOCK")
        .ok_or_else(|| std::io::Error::other("not running under Sway"))?;
    let mut stream = std::os::unix::net::UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    stream.write_all(&ipc_frame(kind, payload))?;
    let mut head = [0u8; 14];
    stream.read_exact(&mut head)?;
    if &head[..6] != b"i3-ipc" {
        return Err(std::io::Error::other("not an i3-IPC reply"));
    }
    let len = u32::from_le_bytes([head[6], head[7], head[8], head[9]]) as usize;
    if len > 4 * 1024 * 1024 {
        return Err(std::io::Error::other("an i3-IPC reply too large"));
    }
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(std::io::Error::other)
}

#[cfg(not(unix))]
fn ipc(_kind: u32, _payload: &str) -> std::io::Result<serde_json::Value> {
    Err(std::io::Error::other("not running under Sway"))
}

fn outputs() -> std::io::Result<Vec<Output>> {
    serde_json::from_value(ipc(GET_OUTPUTS, "")?).map_err(std::io::Error::other)
}

/// Run a Sway command; an error when Sway says it failed.
fn run(command: &str) -> std::io::Result<()> {
    let reply = ipc(RUN_COMMAND, command)?;
    let ok = reply
        .as_array()
        .is_some_and(|all| all.iter().all(|r| r["success"] == true));
    if ok {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("Sway refused: {reply}")))
    }
}

/// One add at a time: two at once could each take the other's new output.
static CREATING: Mutex<()> = Mutex::new(());

/// Make an output of `width`×`height` right of the others; its name and
/// where it went.
fn create(width: u32, height: u32) -> std::io::Result<(String, i32)> {
    let _one = CREATING.lock().unwrap_or_else(|e| e.into_inner());
    let before = outputs()?;
    run("create_output")?;
    let after = outputs()?;
    let name = new_output(&before, &after)
        .ok_or_else(|| std::io::Error::other("Sway made no single new output"))?;
    let x = right_edge(&before);
    if let Err(e) = run(&format!(
        "output {name} mode --custom {width}x{height} pos {x} 0"
    )) {
        remove(&name);
        return Err(e);
    }
    Ok((name, x))
}

fn remove(name: &str) {
    if !is_virtual(name) {
        tracing::warn!("Not removing {name}: not a screen Pocketdesk makes");
        return;
    }
    if let Err(e) = run(&format!("output {name} unplug")) {
        tracing::warn!("Could not remove the screen {name}: {e}");
    }
}

/// The monitor index a session asks for to capture the output `name`, placed
/// at (`x`, 0): as the capture picks one (`capture_screencopy::pick_output`),
/// the listed monitor at that origin, else the output's place among Sway's
/// when the list is shorter. Waits up to 2 s for the list to show it;
/// `None` when no index would reach it.
fn capture_index(name: &str, x: i32) -> Option<u32> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let listed = crate::plugins::monitors::origins();
        if let Some(i) = listed.iter().position(|&(mx, my)| mx == x && my == 0) {
            return Some(i as u32);
        }
        if std::time::Instant::now() >= deadline {
            let place = outputs().ok()?.iter().position(|o| o.name == name)?;
            return (place >= listed.len()).then_some(place as u32);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// The outputs made, per phone.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Made {
    phones: BTreeMap<String, Vec<String>>,
}

fn made() -> &'static Mutex<Made> {
    static MADE: Mutex<Made> = Mutex::new(Made {
        phones: BTreeMap::new(),
    });
    &MADE
}

fn save(made: &Made) {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    if let Ok(json) = serde_json::to_string(made) {
        let tmp = dir.join(format!("{STATE_FILE}.tmp"));
        if pocketdesk_core::secure_fs::write_private(&tmp, json.as_bytes()).is_ok() {
            let _ = std::fs::rename(&tmp, dir.join(STATE_FILE));
        }
    }
}

/// When each phone's screen was added: a screen is left alone for
/// [`GRACE`] while its session moves onto it.
fn added_at() -> &'static Mutex<BTreeMap<String, std::time::Instant>> {
    static ADDED: Mutex<BTreeMap<String, std::time::Instant>> = Mutex::new(BTreeMap::new());
    &ADDED
}

const GRACE: std::time::Duration = std::time::Duration::from_secs(60);

fn recently_added(phone: &str) -> bool {
    added_at()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(phone)
        .is_some_and(|at| at.elapsed() < GRACE)
}

/// `virtual_screens` from the config, set at start.
static ALLOWED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn configure(allowed: bool) {
    ALLOWED.store(allowed, std::sync::atomic::Ordering::Relaxed);
}

/// Remove the outputs a server that stopped left (at start).
pub fn clear_left_over() {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let path = dir.join(STATE_FILE);
    let Some(left) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Made>(&raw).ok())
    else {
        return;
    };
    for name in left.phones.values().flatten() {
        remove(name);
    }
    let _ = std::fs::remove_file(path);
}

/// Remove the outputs of phones with no session in `live` twice in a row
/// (a session switching screens is gone for a moment). Runs every few
/// seconds for the server's life.
pub async fn run_sweeper() {
    let mut missing: BTreeMap<String, u32> = BTreeMap::new();
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
    loop {
        tick.tick().await;
        // A phone counts while it has a session or a control link: a
        // session switching to the new screen, waiting at the approval gate
        // or reconnecting has its link the whole time.
        let mut live: Vec<String> = pocketdesk_core::streaming::sessions::list()
            .into_iter()
            .filter_map(|s| s.device_id)
            .collect();
        live.extend(
            crate::state::clone_clients()
                .await
                .iter()
                .map(|c| c.device_id().to_string()),
        );
        let gone: Vec<(String, Vec<String>)> = {
            let mut made = made().lock().unwrap_or_else(|e| e.into_inner());
            let phones: Vec<String> = made.phones.keys().cloned().collect();
            let mut gone = Vec::new();
            for phone in phones {
                if live.contains(&phone) || recently_added(&phone) {
                    missing.remove(&phone);
                    continue;
                }
                let n = missing.entry(phone.clone()).or_default();
                *n += 1;
                if *n >= 2 {
                    missing.remove(&phone);
                    if let Some(names) = made.phones.remove(&phone) {
                        gone.push((phone, names));
                    }
                }
            }
            if !gone.is_empty() {
                save(&made);
            }
            gone
        };
        for (phone, names) in gone {
            tracing::info!(
                "Removing the screen(s) of a phone with no session: {}",
                phone.chars().take(8).collect::<String>()
            );
            let _ = tokio::task::spawn_blocking(move || names.iter().for_each(|n| remove(n))).await;
        }
    }
}

pub struct VirtualScreenPlugin;

#[async_trait::async_trait]
impl Plugin for VirtualScreenPlugin {
    fn name(&self) -> &'static str {
        "virtual_screen"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[PACKET]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[PACKET]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type != PACKET {
            return Ok(());
        }
        let phone = sender.device_id().to_string();
        let action = packet.body["action"].as_str().unwrap_or("").to_string();
        let width = packet.body["width"].as_u64().unwrap_or(0) as u32;
        let height = packet.body["height"].as_u64().unwrap_or(0) as u32;
        let reply = tokio::task::spawn_blocking(move || answer(&phone, &action, width, height))
            .await
            .unwrap_or_else(|_| serde_json::json!({ "error": "the computer failed to answer" }));
        sender
            .send_packet(&NetworkPacket::new(PACKET).with_body(reply))
            .await
    }
}

/// What a phone's request comes to: `{name, index}`, `{removed}` or `{error}`.
fn answer(phone: &str, action: &str, width: u32, height: u32) -> serde_json::Value {
    use serde_json::json;
    if !ALLOWED.load(std::sync::atomic::Ordering::Relaxed) {
        return json!({ "error": "This computer does not let phones add a screen (Settings, \"Phones may add a screen\")" });
    }
    match action {
        "remove" => {
            let names = {
                let mut made = made().lock().unwrap_or_else(|e| e.into_inner());
                let names = made.phones.remove(phone).unwrap_or_default();
                save(&made);
                names
            };
            names.iter().for_each(|n| remove(n));
            json!({ "removed": names.len() })
        }
        "add" => {
            let Some((w, h)) = screen_size(width, height) else {
                return json!({ "error": "not a screen size" });
            };
            if std::env::var_os("SWAYSOCK").is_none() {
                return json!({ "error": "This computer cannot add a screen: only Sway can for now" });
            }
            // One screen per phone: asked again, it is the same one (the
            // session may be showing it; replacing it would freeze that).
            let existing = made()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .phones
                .get(phone)
                .and_then(|names| names.first().cloned());
            if let Some(name) = existing {
                let place = outputs()
                    .ok()
                    .and_then(|all| all.into_iter().find(|o| o.name == name));
                if let Some(output) = place
                    && let Some(index) = capture_index(&name, output.rect.x)
                {
                    return json!({ "name": name, "index": index, "width": output.rect.width, "height": output.rect.height });
                }
                // Gone from Sway meanwhile: forget it and make another.
                made()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .phones
                    .remove(phone);
            }
            match create(w, h) {
                Ok((name, x)) => {
                    let Some(index) = capture_index(&name, x) else {
                        remove(&name);
                        return json!({ "error": "The computer made a screen but cannot stream it (its monitor list does not show it)" });
                    };
                    {
                        let mut made = made().lock().unwrap_or_else(|e| e.into_inner());
                        made.phones.insert(phone.to_string(), vec![name.clone()]);
                        save(&made);
                    }
                    added_at()
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(phone.to_string(), std::time::Instant::now());
                    tracing::info!("Added a {w}x{h} screen {name} for a phone");
                    json!({ "name": name, "index": index, "width": w, "height": h })
                }
                Err(e) => json!({ "error": format!("Could not add a screen: {e}") }),
            }
        }
        _ => json!({ "error": "unknown action" }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(name: &str, x: i32, width: i32) -> Output {
        Output {
            name: name.into(),
            rect: Rect {
                x,
                y: 0,
                width,
                height: 720,
            },
        }
    }

    #[test]
    fn a_screen_keeps_the_phone_shape_within_bounds() {
        assert_eq!(screen_size(1080, 2400), Some((1080, 2400)));
        assert_eq!(screen_size(1440, 3200), Some((1152, 2560)), "scaled down");
        assert_eq!(screen_size(1081, 2401), Some((1080, 2400)), "even sides");
        assert_eq!(
            screen_size(100, 100),
            Some((320, 320)),
            "at least the minimum"
        );
        assert_eq!(screen_size(0, 2400), None);
    }

    #[test]
    fn only_screens_sway_makes_are_taken_or_removed() {
        assert!(is_virtual("HEADLESS-2"));
        assert!(!is_virtual("HEADLESS-"));
        assert!(!is_virtual("DP-1"));
        assert!(!is_virtual("eDP-1"));
        let before = vec![output("HEADLESS-1", 0, 1280)];
        let plugged = vec![
            output("HEADLESS-1", 0, 1280),
            output("HEADLESS-2", 1280, 1920),
            output("DP-1", 3200, 1920),
        ];
        assert_eq!(
            new_output(&before, &plugged),
            None,
            "two new: neither is taken"
        );
        let real = vec![output("HEADLESS-1", 0, 1280), output("DP-1", 1280, 1920)];
        assert_eq!(
            new_output(&before, &real),
            None,
            "a real monitor is never taken"
        );
    }

    #[test]
    fn the_new_output_is_found_and_placed_right_of_the_others() {
        let before = vec![output("HEADLESS-1", 0, 1280)];
        let after = vec![
            output("HEADLESS-1", 0, 1280),
            output("HEADLESS-2", 1280, 1920),
        ];
        assert_eq!(new_output(&before, &after).as_deref(), Some("HEADLESS-2"));
        assert_eq!(new_output(&before, &before), None);
        assert_eq!(right_edge(&before), 1280);
        assert_eq!(
            right_edge(&[output("A", 0, 1920), output("B", 1920, 1280)]),
            3200
        );
        assert_eq!(right_edge(&[]), 0);
    }

    #[test]
    fn an_ipc_frame_is_magic_length_type_payload() {
        let frame = ipc_frame(GET_OUTPUTS, "ab");
        assert_eq!(&frame[..6], b"i3-ipc");
        assert_eq!(&frame[6..10], &2u32.to_le_bytes());
        assert_eq!(&frame[10..14], &3u32.to_le_bytes());
        assert_eq!(&frame[14..], b"ab");
    }

    #[test]
    fn sway_outputs_are_read_from_get_outputs() {
        let raw = r#"[{"name":"HEADLESS-1","rect":{"x":0,"y":0,"width":1280,"height":720},"active":true}]"#;
        let outputs: Vec<Output> = serde_json::from_str(raw).unwrap();
        assert_eq!(outputs, vec![output("HEADLESS-1", 0, 1280)]);
    }
}
