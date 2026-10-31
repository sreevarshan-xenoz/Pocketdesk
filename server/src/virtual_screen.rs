//! A second screen at the phone's shape (T10j): a phone asks
//! (`kdeconnect.pocketdesk.virtual_screen`), the server makes an output of
//! the phone's size to the right of the others and answers with its monitor
//! index, and the session switches to it. Sway and Hyprland, through their
//! IPC; elsewhere the phone is told the computer cannot.
//! - Sway: `create_output`, `output … mode --custom`, `output … unplug`.
//! - Hyprland (0.56 on): a monitor rule for the name set with `eval
//!   hl.monitor({…})` (real Lua, so not caught by the broken
//!   `dispatch`/`keyword` of hyprwm/Hyprland#16224), then `output create
//!   headless <name>`, which comes up at the rule's size and place, and
//!   `output remove <name>`. It takes the name it is given, so each is
//!   `PD-<tag>-<n>`, the tag this server's own (from its state folder; with
//!   none, no screen is added). Hyprland 0.56 has no way to drop a rule, so
//!   one stays per name until Hyprland reloads its config; names reuse the
//!   smallest free `n`, so they stay few.
//!
//! Adds run one at a time, from the check of what a phone has to the record
//! of what it got, so the limits hold whatever a phone sends at once.
//!
//! An output lasts while its phone has a session or link, and those a server
//! that stopped left are swept by the next one in the same compositor
//! (`virtual_screens.json`); on Hyprland the next one also removes any of its
//! tagged outputs missing from that list (one made just before a crash).

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

/// Phone screens the computer holds at once, at most.
const MAX_SCREENS: usize = 4;

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

/// The compositor a screen is added in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Desk {
    Sway,
    Hyprland,
}

fn desk() -> Option<Desk> {
    if std::env::var_os("SWAYSOCK").is_some() {
        Some(Desk::Sway)
    } else if crate::hyprland::HyprlandIpc::is_available() {
        Some(Desk::Hyprland)
    } else {
        None
    }
}

/// This server's tag in the names of the screens it makes on Hyprland: the
/// eight hex digits of an FNV-1a hash of its state folder, so a second
/// server (a throwaway beside the real one) never takes or removes the
/// other's.
pub fn hypr_tag(state_dir: &std::path::Path) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for b in state_dir.to_string_lossy().bytes() {
        hash ^= u32::from(b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("{hash:08x}")
}

/// `None` with no state folder: then no tag is safe to share, so no screen
/// is added or swept.
fn this_tag() -> Option<String> {
    crate::state::state_dir().ok().map(|d| hypr_tag(&d))
}

/// Whether `name` is a screen made on Hyprland by the server tagged `tag`:
/// `PD-<tag>-<digits>`, never a real monitor's connector.
pub fn is_hypr_screen(name: &str, tag: &str) -> bool {
    name.strip_prefix("PD-")
        .and_then(|rest| rest.strip_prefix(tag))
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The name for the next screen on Hyprland: `PD-<tag>-<n>`, the smallest
/// `n` from 1 no output has.
pub fn hypr_name(tag: &str, outputs: &[Output]) -> String {
    (1u32..)
        .map(|n| format!("PD-{tag}-{n}"))
        .find(|name| !outputs.iter().any(|o| &o.name == name))
        .unwrap_or_default()
}

/// Whether `name` has the shape of a screen name this server makes:
/// `PD-` and eight lowercase hex digits, `-` and up to nine digits. Only
/// such a name goes into a Lua string.
pub fn is_hypr_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("PD-") else {
        return false;
    };
    let Some((tag, n)) = rest.split_once('-') else {
        return false;
    };
    tag.len() == 8
        && tag.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        && (1..=9).contains(&n.len())
        && n.bytes().all(|b| b.is_ascii_digit())
}

/// The request that sizes the screen `name`: its mode, right of the others
/// (`auto-right` leaves the outputs there where they are; a fixed position
/// moved a monitor whose own rule is `auto`, found in the spike), scale 1.
/// `None` unless `name` is one this server makes and both sides are within
/// [`MIN_SIDE`]..=[`MAX_SIDE`], so nothing else reaches the Lua.
pub fn hypr_monitor_rule(name: &str, width: u32, height: u32) -> Option<String> {
    let side = MIN_SIDE..=MAX_SIDE;
    (is_hypr_name(name) && side.contains(&width) && side.contains(&height)).then(|| {
        format!(
            "eval hl.monitor({{ output = \"{name}\", mode = \"{width}x{height}@60\", position = \"auto-right\", scale = 1 }})"
        )
    })
}

/// Hyprland's answer to a write: `ok`, else its words (`error: …`, `output
/// not found`); `unknown request` (no `eval` before 0.56) says which
/// Hyprland is needed.
pub fn hypr_ok(reply: &str) -> std::io::Result<()> {
    match reply.trim() {
        "ok" => Ok(()),
        "unknown request" => Err(std::io::Error::other(
            "this Hyprland is older than 0.56, which adding a screen needs",
        )),
        other => Err(std::io::Error::other(format!("Hyprland refused: {other}"))),
    }
}

/// Hyprland's `j/monitors` as outputs.
pub fn hypr_outputs(raw: &str) -> std::io::Result<Vec<Output>> {
    let monitors: Vec<crate::hyprland::HyprMonitor> =
        serde_json::from_str(raw).map_err(std::io::Error::other)?;
    Ok(monitors
        .into_iter()
        .map(|m| Output {
            name: m.name,
            rect: Rect {
                x: m.x,
                y: m.y,
                width: m.width,
                height: m.height,
            },
        })
        .collect())
}

fn hypr(req: &str) -> std::io::Result<String> {
    crate::hyprland::HyprlandIpc::from_env()
        .map_err(|e| std::io::Error::other(e.to_string()))?
        .request_blocking(req)
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

/// The commands that pin each output where it is now. Sway arranges outputs
/// with no position of their own, so adding a screen moved the others
/// (found on the VM: the desktop's own output went from x 0 to 2360).
pub fn pin_commands(outputs: &[Output]) -> Vec<String> {
    outputs
        .iter()
        .filter(|o| o.rect.width > 0 && o.rect.height > 0)
        .map(|o| format!("output {} pos {} {}", o.name, o.rect.x, o.rect.y))
        .collect()
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
    match desk() {
        Some(Desk::Hyprland) => hypr_outputs(&hypr("j/monitors")?),
        _ => serde_json::from_value(ipc(GET_OUTPUTS, "")?).map_err(std::io::Error::other),
    }
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

/// Make an output of `width`×`height` right of the others; its name and
/// where it went. The caller holds [`Screens::adding`].
fn create(width: u32, height: u32) -> std::io::Result<(String, i32)> {
    match desk() {
        Some(Desk::Hyprland) => {
            let tag =
                this_tag().ok_or_else(|| std::io::Error::other("no state folder to name it by"))?;
            create_hyprland(
                &hypr,
                &tag,
                width,
                height,
                std::time::Duration::from_secs(2),
            )
        }
        _ => create_sway(width, height),
    }
}

/// Removes the output `name` when dropped, unless kept: once `output create`
/// is sent, every way out but success removes it (a reply that timed out may
/// still have made it).
struct RemoveUnlessKept<'a> {
    request: &'a dyn Fn(&str) -> std::io::Result<String>,
    name: &'a str,
    kept: bool,
}

impl Drop for RemoveUnlessKept<'_> {
    fn drop(&mut self) {
        if self.kept {
            return;
        }
        let removed = (self.request)(&format!("output remove {}", self.name));
        if let Err(e) = removed.and_then(|r| hypr_ok(&r)) {
            tracing::warn!("Could not remove the screen {}: {e}", self.name);
        }
    }
}

/// On Hyprland, through `request` (its socket; a fake in tests): set the
/// rule for a new name, make the output by that name (it comes up at the
/// rule's size), and wait up to `wait` for the monitor list to show it at
/// that size. A list that cannot be read meanwhile is "not yet".
fn create_hyprland(
    request: &dyn Fn(&str) -> std::io::Result<String>,
    tag: &str,
    width: u32,
    height: u32,
    wait: std::time::Duration,
) -> std::io::Result<(String, i32)> {
    let name = hypr_name(tag, &hypr_outputs(&request("j/monitors")?)?);
    let rule = hypr_monitor_rule(&name, width, height)
        .ok_or_else(|| std::io::Error::other("not a screen this computer makes"))?;
    hypr_ok(&request(&rule)?)?;
    let mut made = RemoveUnlessKept {
        request,
        name: &name,
        kept: false,
    };
    hypr_ok(&request(&format!("output create headless {name}"))?)?;
    let deadline = std::time::Instant::now() + wait;
    loop {
        let shown = request("j/monitors")
            .and_then(|raw| hypr_outputs(&raw))
            .ok()
            .and_then(|all| all.into_iter().find(|o| o.name == name));
        if let Some(o) = &shown
            && u32::try_from(o.rect.width) == Ok(width)
            && u32::try_from(o.rect.height) == Ok(height)
        {
            made.kept = true;
            return Ok((name.clone(), o.rect.x));
        }
        if std::time::Instant::now() >= deadline {
            return Err(std::io::Error::other(match shown {
                Some(_) => "Hyprland did not give the screen its size",
                None => "Hyprland made no screen",
            }));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn create_sway(width: u32, height: u32) -> std::io::Result<(String, i32)> {
    let before = outputs()?;
    for pin in pin_commands(&before) {
        run(&pin)?;
    }
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
    let removed = match desk() {
        Some(Desk::Hyprland) if this_tag().is_some_and(|tag| is_hypr_screen(name, &tag)) => {
            hypr(&format!("output remove {name}")).and_then(|r| hypr_ok(&r))
        }
        Some(Desk::Sway) if is_virtual(name) => run(&format!("output {name} unplug")),
        _ => {
            tracing::warn!("Not removing {name}: not a screen Pocketdesk makes");
            return;
        }
    };
    if let Err(e) = removed {
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
    /// The compositor they were made in (Sway's socket path, which holds its
    /// pid; `hyprland:` and Hyprland's instance signature): a restarted one
    /// has none of them, and may reuse their names.
    #[serde(default, alias = "sway")]
    instance: Option<String>,
}

/// The screens made and the lock adds take.
struct Screens {
    made: Mutex<Made>,
    /// Held by an add from the check of what its phone has to the record of
    /// what it got, and by the sweep at start: two adds at once could
    /// otherwise each pass the limits, or take the other's new output.
    adding: Mutex<()>,
}

impl Screens {
    const fn new() -> Self {
        Self {
            made: Mutex::new(Made {
                phones: BTreeMap::new(),
                instance: None,
            }),
            adding: Mutex::new(()),
        }
    }
}

fn screens() -> &'static Screens {
    static SCREENS: Screens = Screens::new();
    &SCREENS
}

fn made() -> &'static Mutex<Made> {
    &screens().made
}

/// What adding and removing need from the computer: [`Live`], or a fake in
/// tests.
trait Host: Sync {
    /// Why no screen can be added here, if none can.
    fn cannot(&self) -> Option<&'static str>;
    fn outputs(&self) -> std::io::Result<Vec<Output>>;
    fn create(&self, width: u32, height: u32) -> std::io::Result<(String, i32)>;
    fn remove(&self, name: &str);
    fn capture_index(&self, name: &str, x: i32) -> Option<u32>;
    fn save(&self, made: &Made);
}

/// The compositor this server runs in.
struct Live;

impl Host for Live {
    fn cannot(&self) -> Option<&'static str> {
        match desk() {
            None => Some("This computer cannot add a screen: only Sway and Hyprland can for now"),
            Some(Desk::Hyprland) if this_tag().is_none() => {
                Some("This computer cannot add a screen: Pocketdesk has no state folder here")
            }
            _ => None,
        }
    }
    fn outputs(&self) -> std::io::Result<Vec<Output>> {
        outputs()
    }
    fn create(&self, width: u32, height: u32) -> std::io::Result<(String, i32)> {
        create(width, height)
    }
    fn remove(&self, name: &str) {
        remove(name)
    }
    fn capture_index(&self, name: &str, x: i32) -> Option<u32> {
        capture_index(name, x)
    }
    fn save(&self, made: &Made) {
        save(made)
    }
}

fn this_instance() -> Option<String> {
    match desk()? {
        Desk::Sway => std::env::var("SWAYSOCK").ok(),
        Desk::Hyprland => std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
            .ok()
            .map(|sig| format!("hyprland:{sig}")),
    }
}

fn save(made: &Made) {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let made = Made {
        phones: made.phones.clone(),
        instance: this_instance(),
    };
    if let Ok(json) = serde_json::to_string(&made) {
        let tmp = dir.join(format!("{STATE_FILE}.tmp"));
        if pocketdesk_core::secure_fs::write_private(&tmp, json.as_bytes()).is_ok() {
            let _ = pocketdesk_core::atomic_file::replace(&tmp, dir.join(STATE_FILE));
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

/// Take over the screens a server that stopped left (at start): in the
/// same compositor they are this server's to sweep like any other (gone once
/// their phone has none of a session or link); a restarted one has none of
/// them and may have reused their names, so then they are only forgotten. On
/// Hyprland, this server's tagged outputs not on that list (made just before
/// a crash, never saved) are removed now.
pub fn clear_left_over() {
    let _one = screens().adding.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let path = dir.join(STATE_FILE);
    let left = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Made>(&raw).ok());
    let mut kept: Vec<String> = Vec::new();
    if let Some(left) = left {
        if left.instance.is_some() && left.instance == this_instance() {
            let mut made = made().lock().unwrap_or_else(|e| e.into_inner());
            for (phone, names) in left.phones {
                kept.extend(names.iter().cloned());
                made.phones.entry(phone).or_default().extend(names);
            }
            save(&made);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
    kept.extend(
        made()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .phones
            .values()
            .flatten()
            .cloned(),
    );
    if desk() == Some(Desk::Hyprland)
        && let Some(tag) = this_tag()
    {
        for o in outputs().unwrap_or_default() {
            if is_hypr_screen(&o.name, &tag) && !kept.contains(&o.name) {
                tracing::info!("Removing the screen {} a stopped server left", o.name);
                remove(&o.name);
            }
        }
    }
}

/// How long start-up waits for [`clear_left_over`] before going on without
/// it: a hung compositor must not keep every phone from the computer.
const SWEEP_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Run [`clear_left_over`] at start and wait for it at most [`SWEEP_WAIT`];
/// slower, it finishes alone. Adds wait for it either way, as it holds
/// their lock.
pub async fn clear_left_over_at_start() {
    if !wait_at_most(SWEEP_WAIT, clear_left_over).await {
        tracing::warn!("The sweep of left-over screens is slow; starting without waiting for it");
    }
}

/// Run `work` on the blocking pool and wait at most `limit`: `true` when it
/// finished in time.
async fn wait_at_most(limit: std::time::Duration, work: impl FnOnce() + Send + 'static) -> bool {
    tokio::time::timeout(limit, tokio::task::spawn_blocking(work))
        .await
        .is_ok()
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
        // A side past u32 is no size, never a wrapped one.
        let side = |key: &str| {
            packet.body[key]
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(0)
        };
        let (width, height) = (side("width"), side("height"));
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
    answer_with(screens(), &Live, phone, action, width, height)
}

fn answer_with(
    screens: &Screens,
    host: &dyn Host,
    phone: &str,
    action: &str,
    width: u32,
    height: u32,
) -> serde_json::Value {
    use serde_json::json;
    // Taking one's own screen back is always allowed.
    if action == "add" && !ALLOWED.load(std::sync::atomic::Ordering::Relaxed) {
        return json!({ "error": "This computer does not let phones add a screen (Settings, \"Phones may add a screen\")" });
    }
    let made = || screens.made.lock().unwrap_or_else(|e| e.into_inner());
    match action {
        "remove" => {
            let names = {
                let mut made = made();
                let names = made.phones.remove(phone).unwrap_or_default();
                host.save(&made);
                names
            };
            names.iter().for_each(|n| host.remove(n));
            json!({ "removed": names.len() })
        }
        "add" => {
            let Some((w, h)) = screen_size(width, height) else {
                return json!({ "error": "not a screen size" });
            };
            if let Some(why) = host.cannot() {
                return json!({ "error": why });
            }
            let _one = screens.adding.lock().unwrap_or_else(|e| e.into_inner());
            // One screen per phone: asked again, it is the same one (the
            // session may be showing it; replacing it would freeze that).
            let existing = made()
                .phones
                .get(phone)
                .and_then(|names| names.first().cloned());
            if let Some(name) = existing {
                let place = host
                    .outputs()
                    .ok()
                    .and_then(|all| all.into_iter().find(|o| o.name == name));
                if let Some(output) = place
                    && let Some(index) = host.capture_index(&name, output.rect.x)
                {
                    return json!({ "name": name, "index": index, "width": output.rect.width, "height": output.rect.height });
                }
                // Gone from the compositor meanwhile: forget it and make another.
                made().phones.remove(phone);
            }
            // A few at most, whatever ids phones announce.
            let held = made().phones.values().map(Vec::len).sum::<usize>();
            if held >= MAX_SCREENS {
                return json!({ "error": "The computer has as many phone screens as it takes" });
            }
            match host.create(w, h) {
                Ok((name, x)) => {
                    let Some(index) = host.capture_index(&name, x) else {
                        host.remove(&name);
                        return json!({ "error": "The computer made a screen but cannot stream it (its monitor list does not show it)" });
                    };
                    {
                        let mut made = made();
                        made.phones.insert(phone.to_string(), vec![name.clone()]);
                        host.save(&made);
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
    fn the_outputs_there_are_pinned_where_they_are() {
        let mut off = output("HEADLESS-9", 0, 0);
        off.rect.height = 0;
        assert_eq!(
            pin_commands(&[
                output("HEADLESS-1", 0, 1280),
                output("DP-1", 1280, 1920),
                off
            ]),
            ["output HEADLESS-1 pos 0 0", "output DP-1 pos 1280 0"]
        );
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
    fn a_hyprland_screen_is_named_with_the_servers_tag() {
        let tag = hypr_tag(std::path::Path::new("/home/user/.local/state/pocketdesk"));
        assert_eq!(tag.len(), 8);
        assert!(tag.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(
            tag,
            hypr_tag(std::path::Path::new("/home/user/.local/state/pocketdesk")),
            "the same folder, the same tag"
        );
        assert_ne!(
            tag,
            hypr_tag(std::path::Path::new("/tmp/throwaway/state")),
            "another server, another tag"
        );
        assert!(is_hypr_name(&format!("PD-{tag}-1")));
        assert_eq!(
            hypr_name("0a1b2c3d", &[output("eDP-1", 0, 1920)]),
            "PD-0a1b2c3d-1"
        );
        let taken = [
            output("eDP-1", 0, 1920),
            output("PD-0a1b2c3d-1", 1920, 1080),
            output("PD-0a1b2c3d-3", 3000, 1080),
        ];
        assert_eq!(hypr_name("0a1b2c3d", &taken), "PD-0a1b2c3d-2");
    }

    #[test]
    fn only_this_servers_hyprland_screens_are_taken_or_removed() {
        let tag = "0a1b2c3d";
        assert!(is_hypr_screen("PD-0a1b2c3d-1", tag));
        assert!(is_hypr_screen("PD-0a1b2c3d-17", tag));
        assert!(!is_hypr_screen("PD-0a1b2c3e-1", tag), "another server's");
        assert!(!is_hypr_screen("PD-0a1b2c3d-", tag));
        assert!(!is_hypr_screen("PD-0a1b2c3d-1x", tag));
        assert!(!is_hypr_screen("PD-0a1b2c3d1", tag));
        assert!(!is_hypr_screen("eDP-1", tag));
        assert!(!is_hypr_screen("HEADLESS-1", tag));
    }

    #[test]
    fn a_hyprland_screen_is_sized_by_a_lua_monitor_rule() {
        assert_eq!(
            hypr_monitor_rule("PD-0a1b2c3d-1", 1080, 2400).as_deref(),
            Some(
                r#"eval hl.monitor({ output = "PD-0a1b2c3d-1", mode = "1080x2400@60", position = "auto-right", scale = 1 })"#
            )
        );
    }

    #[test]
    fn nothing_but_a_screen_name_and_sizes_reach_the_lua() {
        for name in [
            r#"PD-0a1b2c3d-1"})os.execute("true")--"#,
            "PD-0a1b2c3d-",
            "PD-0A1B2C3D-1",
            "PD-0a1b2c3-1",
            "PD-0a1b2c3d-1234567890",
            "eDP-1",
            "",
        ] {
            assert_eq!(hypr_monitor_rule(name, 1080, 2400), None, "{name}");
        }
        for (w, h) in [
            (0, 2400),
            (1080, 0),
            (100_000, 2400),
            (1080, 100_000),
            (319, 2400),
        ] {
            assert_eq!(hypr_monitor_rule("PD-0a1b2c3d-1", w, h), None, "{w}x{h}");
        }
        assert!(hypr_monitor_rule("PD-0a1b2c3d-1", MIN_SIDE, MAX_SIDE).is_some());
    }

    /// A scripted reply to a request, given the request's number from 1.
    type Reply = Box<dyn Fn(&str, usize) -> std::io::Result<String> + Send + Sync>;

    /// Hyprland through a script of replies, with every request kept.
    struct FakeHyprland {
        sent: Mutex<Vec<String>>,
        reply: Reply,
    }

    impl FakeHyprland {
        fn new(
            reply: impl Fn(&str, usize) -> std::io::Result<String> + Send + Sync + 'static,
        ) -> Self {
            Self {
                sent: Mutex::new(Vec::new()),
                reply: Box::new(reply),
            }
        }
        fn request(&self, req: &str) -> std::io::Result<String> {
            let mut sent = self.sent.lock().unwrap();
            sent.push(req.to_string());
            let n = sent.len();
            drop(sent);
            (self.reply)(req, n)
        }
        fn sent(&self) -> Vec<String> {
            self.sent.lock().unwrap().clone()
        }
        fn create(&self) -> std::io::Result<(String, i32)> {
            create_hyprland(
                &|r| self.request(r),
                "0a1b2c3d",
                1080,
                2400,
                std::time::Duration::from_millis(300),
            )
        }
    }

    const SIZED: &str = r#"[{"name":"eDP-1","x":0,"y":0,"width":1920,"height":1080},
        {"name":"PD-0a1b2c3d-1","x":1920,"y":0,"width":1080,"height":2400}]"#;
    const ONLY_EDP: &str = r#"[{"name":"eDP-1","x":0,"y":0,"width":1920,"height":1080}]"#;

    #[test]
    fn a_hyprland_screen_is_ruled_then_made_and_kept() {
        let fake = FakeHyprland::new(|req, n| {
            Ok(match req {
                "j/monitors" if n == 1 => ONLY_EDP,
                "j/monitors" => SIZED,
                _ => "ok",
            }
            .to_string())
        });
        assert_eq!(fake.create().unwrap(), ("PD-0a1b2c3d-1".to_string(), 1920));
        let sent = fake.sent();
        assert!(
            sent[1].starts_with("eval hl.monitor"),
            "the rule first: {sent:?}"
        );
        assert_eq!(sent[2], "output create headless PD-0a1b2c3d-1");
        assert!(!sent.iter().any(|r| r.starts_with("output remove")));
    }

    #[test]
    fn a_made_screen_is_removed_when_its_list_cannot_be_read() {
        let fake = FakeHyprland::new(|req, n| match req {
            "j/monitors" if n == 1 => Ok(ONLY_EDP.into()),
            "j/monitors" => Err(std::io::Error::other("timed out")),
            _ => Ok("ok".into()),
        });
        assert!(fake.create().is_err());
        assert_eq!(
            fake.sent().last().map(String::as_str),
            Some("output remove PD-0a1b2c3d-1")
        );
    }

    #[test]
    fn a_screen_whose_create_reply_timed_out_is_removed() {
        let fake = FakeHyprland::new(|req, _| match req {
            "j/monitors" => Ok(ONLY_EDP.into()),
            r if r.starts_with("output create") => Err(std::io::Error::other("timed out")),
            _ => Ok("ok".into()),
        });
        assert!(fake.create().is_err());
        assert_eq!(
            fake.sent().last().map(String::as_str),
            Some("output remove PD-0a1b2c3d-1")
        );
    }

    #[test]
    fn a_screen_never_sized_is_removed() {
        let fake = FakeHyprland::new(|req, _| {
            Ok(match req {
                "j/monitors" => ONLY_EDP,
                _ => "ok",
            }
            .to_string())
        });
        let err = fake.create().unwrap_err();
        assert!(err.to_string().contains("made no screen"), "{err}");
        assert_eq!(
            fake.sent().last().map(String::as_str),
            Some("output remove PD-0a1b2c3d-1")
        );
    }

    #[test]
    fn an_older_hyprland_is_named_and_nothing_is_made() {
        let fake = FakeHyprland::new(|req, _| {
            Ok(match req {
                "j/monitors" => ONLY_EDP,
                _ => "unknown request",
            }
            .to_string())
        });
        let err = fake.create().unwrap_err();
        assert!(err.to_string().contains("0.56"), "{err}");
        assert!(
            !fake
                .sent()
                .iter()
                .any(|r| r.starts_with("output create") || r.starts_with("output remove")),
            "{:?}",
            fake.sent()
        );
    }

    /// A computer whose outputs are what was made, each made slowly.
    #[derive(Default)]
    struct FakeHost {
        outputs: Mutex<Vec<Output>>,
        made: std::sync::atomic::AtomicUsize,
    }

    impl Host for FakeHost {
        fn cannot(&self) -> Option<&'static str> {
            None
        }
        fn outputs(&self) -> std::io::Result<Vec<Output>> {
            Ok(self.outputs.lock().unwrap().clone())
        }
        fn create(&self, width: u32, _height: u32) -> std::io::Result<(String, i32)> {
            std::thread::sleep(std::time::Duration::from_millis(30));
            let n = self.made.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            let name = format!("PD-0a1b2c3d-{n}");
            let x = 1920 * n as i32;
            self.outputs
                .lock()
                .unwrap()
                .push(output(&name, x, width as i32));
            Ok((name, x))
        }
        fn remove(&self, name: &str) {
            self.outputs.lock().unwrap().retain(|o| o.name != name);
        }
        fn capture_index(&self, _name: &str, _x: i32) -> Option<u32> {
            Some(1)
        }
        fn save(&self, _made: &Made) {}
    }

    #[test]
    fn a_phone_asking_many_times_at_once_gets_one_screen() {
        let screens = Screens::new();
        let host = FakeHost::default();
        let names: Vec<String> = std::thread::scope(|s| {
            let asks: Vec<_> = (0..8)
                .map(|_| {
                    s.spawn(|| {
                        answer_with(&screens, &host, "race-phone", "add", 1080, 2400)["name"]
                            .as_str()
                            .unwrap()
                            .to_string()
                    })
                })
                .collect();
            asks.into_iter().map(|a| a.join().unwrap()).collect()
        });
        assert_eq!(host.made.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(names.iter().all(|n| n == &names[0]), "{names:?}");
        assert_eq!(host.outputs.lock().unwrap().len(), 1);
    }

    #[test]
    fn many_phones_at_once_get_at_most_the_limit() {
        let screens = Screens::new();
        let host = FakeHost::default();
        let added = std::thread::scope(|s| {
            let asks: Vec<_> = (0..10)
                .map(|i| {
                    let (screens, host) = (&screens, &host);
                    s.spawn(move || {
                        answer_with(screens, host, &format!("race-phone-{i}"), "add", 1080, 2400)
                            .get("name")
                            .is_some()
                    })
                })
                .collect();
            asks.into_iter()
                .map(|a| a.join().unwrap())
                .filter(|&ok| ok)
                .count()
        });
        assert_eq!(added, MAX_SCREENS);
        assert_eq!(
            host.made.load(std::sync::atomic::Ordering::SeqCst),
            MAX_SCREENS
        );
        let recorded: usize = screens
            .made
            .lock()
            .unwrap()
            .phones
            .values()
            .map(Vec::len)
            .sum();
        assert_eq!(recorded, MAX_SCREENS, "every screen made is on the record");
    }

    #[test]
    fn hyprland_answers_are_read() {
        assert!(hypr_ok("ok").is_ok());
        assert!(hypr_ok("ok\n").is_ok());
        let err = hypr_ok("output not found").unwrap_err();
        assert!(err.to_string().contains("output not found"));
        assert!(hypr_ok("error: hl.monitor: error applying field 'mode'").is_err());
        assert!(
            hypr_ok("unknown request")
                .unwrap_err()
                .to_string()
                .contains("older than 0.56")
        );
        assert!(hypr_ok("").is_err());
        let raw = r#"[
            {"name":"eDP-1","x":0,"y":0,"width":1920,"height":1080,"scale":1.0,"activeWorkspace":{"id":1}},
            {"name":"PD-0a1b2c3d-1","x":1920,"y":0,"width":1080,"height":2400}
        ]"#;
        let outputs = hypr_outputs(raw).unwrap();
        assert_eq!(outputs[1].name, "PD-0a1b2c3d-1");
        assert_eq!(
            outputs[1].rect,
            Rect {
                x: 1920,
                y: 0,
                width: 1080,
                height: 2400
            }
        );
        assert!(hypr_outputs("not json").is_err());
    }

    /// A sweep stuck on a compositor that takes requests and never answers
    /// holds start-up back no longer than its limit.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_hung_sweep_holds_start_up_back_only_its_limit() {
        let dir = std::env::temp_dir().join(format!("pd-vs-hung-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.sock");
        let _ = std::fs::remove_file(&path);
        let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let at = std::time::Instant::now();
        let sock = path.clone();
        let finished = wait_at_most(std::time::Duration::from_millis(300), move || {
            let _ =
                crate::hyprland::request_at(&sock, "j/monitors", std::time::Duration::from_secs(2));
        })
        .await;
        assert!(!finished, "the sweep could not have finished");
        assert!(
            at.elapsed() < std::time::Duration::from_secs(2),
            "start-up waited"
        );
        assert!(wait_at_most(std::time::Duration::from_secs(1), || {}).await);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_old_record_naming_its_sway_is_still_read() {
        let made: Made =
            serde_json::from_str(r#"{"phones":{"p":["HEADLESS-2"]},"sway":"/run/sway.sock"}"#)
                .unwrap();
        assert_eq!(made.instance.as_deref(), Some("/run/sway.sock"));
    }

    /// On a live Hyprland session (`cargo test -p pocketdesk-server --
    /// --ignored live_hyprland_screen`): a phone's screen is added at its
    /// size, captured by screencopy at that size, and removed again; the
    /// monitor list ends as it began. Adds one output named with the test
    /// folder's tag; moves no window and injects nothing.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "adds and removes an output on the live Hyprland session"]
    fn live_hyprland_screen_is_added_captured_and_removed() {
        use pocketdesk_core::streaming::capture_screencopy::start_screencopy_capture;
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;

        let _state = crate::state::use_test_dirs();
        assert_eq!(desk(), Some(Desk::Hyprland), "run on a Hyprland session");
        let names = |all: Vec<Output>| all.into_iter().map(|o| o.name).collect::<Vec<_>>();
        let before = names(outputs().unwrap());

        let reply = answer("pd-test-phone", "add", 1080, 2400);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let name = reply["name"]
                .as_str()
                .expect("a screen was added")
                .to_string();
            assert!(is_hypr_screen(&name, &this_tag().unwrap()), "{name}");
            assert_eq!(
                (reply["width"].as_u64(), reply["height"].as_u64()),
                (Some(1080), Some(2400))
            );
            let index = reply["index"].as_u64().unwrap() as u32;

            let (tx, mut rx) = pocketdesk_core::streaming::latest_frame::channel::<
                pocketdesk_core::streaming::VideoFrame,
            >();
            let config = pocketdesk_core::streaming::StreamingConfig {
                monitor_index: index,
                ..Default::default()
            };
            let (_window_tx, window_rx) = tokio::sync::watch::channel(0u64);
            let session = start_screencopy_capture(
                config,
                tx,
                tokio_util::sync::CancellationToken::new(),
                window_rx,
                Arc::new(AtomicBool::new(false)),
            )
            .expect("screencopy starts on the new screen");
            let frame = rx.blocking_recv().expect("a frame");
            drop(session);
            assert_eq!(
                (frame.width, frame.height),
                (1080, 2400),
                "captured at the phone's size"
            );

            // Asked again, it is the same screen.
            assert_eq!(
                answer("pd-test-phone", "add", 1080, 2400)["name"].as_str(),
                Some(name.as_str())
            );
        }));
        let removed = answer("pd-test-phone", "remove", 0, 0);
        std::thread::sleep(std::time::Duration::from_millis(500));
        let after = names(outputs().unwrap());
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
        assert_eq!(removed["removed"].as_u64(), Some(1));
        assert_eq!(after, before, "the monitor list is back as it was");
    }

    #[test]
    fn sway_outputs_are_read_from_get_outputs() {
        let raw = r#"[{"name":"HEADLESS-1","rect":{"x":0,"y":0,"width":1280,"height":720},"active":true}]"#;
        let outputs: Vec<Output> = serde_json::from_str(raw).unwrap();
        assert_eq!(outputs, vec![output("HEADLESS-1", 0, 1280)]);
    }
}
