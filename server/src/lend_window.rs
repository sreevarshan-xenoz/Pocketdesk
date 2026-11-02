//! Lending a window to another computer of the owner's (phase 3a of the
//! window handoff): the host side. A computer paired both ways asks
//! (`kdeconnect.pocketdesk.virtual_screen`'s `lend`, with a window's id from
//! the window list), and this computer makes a headless screen of the
//! window's own size ([`crate::virtual_screen`] makes it, with its limits
//! and its sweep), moves the window onto it alone, and answers with the
//! screen's monitor index, which a session captures like any screen's. The
//! program keeps running here; only its picture crosses, and what the other
//! computer sends to that screen lands on the window. `return` (or the
//! borrowing computer's session ending, the server stopping, or the next
//! server's start after a crash) puts the window back where it was and takes
//! the screen away.
//!
//! - **Sway:** the window is made floating with no border (so it keeps its
//!   size and a program that goes bare when it is told it is full screen does
//!   not), moved to the screen with `move container to output`, sized to it
//!   and placed at its corner. Back: to its workspace (else its output),
//!   tiled again if it was, with its border, size and place.
//! - **Hyprland (0.56 on):** `dispatch` writes are broken upstream
//!   (hyprwm/Hyprland#16224) but `eval` runs real Lua, so the window is moved
//!   with `hl.dispatch(hl.dsp.window.move({ monitor = … }))` and made full
//!   screen on the compositor's side only (`fullscreen_state` with the
//!   program's own state left at 0, so it does not know and its toolbars
//!   stay), which fills the screen exactly, gaps and borders gone. `eval`
//!   says `ok` even to a window that is not there, so each step is checked
//!   in `j/clients`.
//!
//! **Who may ask:** only a computer paired both ways ("both are mine"), with
//! the Input permission (held to it before this runs, on a connection and on
//! the link), and only while "Let my computers borrow my windows"
//! (`lend_windows`, off by default) is on; taking a window back is never
//! refused. Window ids are digits (Sway's container id) or `0x` and hex
//! (Hyprland's address), and nothing else reaches a command or the Lua.
//!
//! What was lent, from where, is kept in `lent_windows.json` (written before
//! the window is moved) so the next server in the same compositor puts them
//! back; in another compositor they are gone with it. The screens are the
//! virtual screens' own records, keyed `<device id>#lend-<window id>`, so a
//! borrowing computer with no session or link for two sweeps loses them, and
//! [`give_back_for_output`], run as any screen is removed, returns the window
//! first.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::virtual_screen::{Desk, Output};

/// The file what is lent is kept in.
pub const STATE_FILE: &str = "lent_windows.json";

/// `lend_windows` from the config at the time of an ask.
fn enabled() -> bool {
    crate::events::current_config().lend_windows
}

/// A window as the compositor shows it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// Sway's container id, or Hyprland's address.
    pub id: String,
    pub floating: bool,
    /// The window's own top left, in the layout's coordinates.
    pub at: [i32; 2],
    pub size: [i32; 2],
    /// The top left of its container: Sway's title bar and border are
    /// outside `at` (Hyprland's is `at`).
    pub origin: [i32; 2],
    /// The size of its container (Sway's, which `resize set` sets, border and
    /// title bar included; Hyprland's is `size`).
    pub frame: [i32; 2],
    pub output: String,
    pub workspace: String,
    pub workspace_id: i32,
    /// Sway's border style and width.
    pub border: String,
    pub border_width: i32,
    /// Full screen as the compositor has it (Sway: 0, 1, 2; Hyprland: its
    /// internal state), and Hyprland's state told to the program.
    pub fullscreen: i32,
    pub fullscreen_client: i32,
}

/// A window lent, with what puts it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lent {
    /// The computer that has it.
    pub device: String,
    pub window: String,
    /// The screen made for it.
    pub output: String,
    pub from_output: String,
    pub workspace: String,
    pub workspace_id: i32,
    pub floating: bool,
    pub at: [i32; 2],
    pub size: [i32; 2],
    #[serde(default)]
    pub origin: [i32; 2],
    #[serde(default)]
    pub frame: [i32; 2],
    #[serde(default)]
    pub border: String,
    #[serde(default)]
    pub border_width: i32,
    #[serde(default)]
    pub fullscreen: i32,
    #[serde(default)]
    pub fullscreen_client: i32,
}

impl Lent {
    fn new(device: &str, seen: &Seen, output: &str) -> Self {
        Self {
            device: device.to_string(),
            window: seen.id.clone(),
            output: output.to_string(),
            from_output: seen.output.clone(),
            workspace: seen.workspace.clone(),
            workspace_id: seen.workspace_id,
            floating: seen.floating,
            at: seen.at,
            size: seen.size,
            origin: seen.origin,
            frame: seen.frame,
            border: seen.border.clone(),
            border_width: seen.border_width,
            fullscreen: seen.fullscreen,
            fullscreen_client: seen.fullscreen_client,
        }
    }

    /// The key of its screen in the virtual screens' record.
    fn key(&self) -> String {
        lend_key(&self.device, &self.window)
    }
}

/// The virtual screens' record key for `device`'s borrowed `window`;
/// `key_owner` there reads the device back from it.
pub fn lend_key(device: &str, window: &str) -> String {
    format!("{device}#lend-{window}")
}

/// Whether `id` has the shape of a window id: a Sway container id (digits)
/// or a Hyprland address (`0x` and hex). Only such an id reaches a command.
pub fn is_window_id(id: &str) -> bool {
    let sway = (1..=12).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit());
    let hypr = id.strip_prefix("0x").is_some_and(|hex| {
        (1..=16).contains(&hex.len()) && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    });
    sway || hypr
}

/// A name that is safe inside a quoted word of a Sway command or a Lua
/// string: plain ASCII, no quote, backslash, semicolon or comma.
fn is_plain(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'_' | b'-' | b'.' | b':'))
}

/// The size of the screen for a window of `size` (its own, without
/// decorations): the virtual screens' bounds, so a window too large or too
/// small is resized to its screen.
pub fn lend_size(size: [i32; 2]) -> Option<(u32, u32)> {
    let side = |v: i32| u32::try_from(v).ok();
    crate::virtual_screen::screen_size(side(size[0])?, side(size[1])?)
}

/// Why `action` is refused: only an owner's computer, paired both ways,
/// borrows a window, and only while the owner's switch is on. Taking one
/// back is never refused.
pub fn refusal(action: &str, enabled: bool, owner_computer: bool) -> Option<&'static str> {
    if action != "lend" {
        return None;
    }
    if !owner_computer {
        return Some("Only a computer of this one's owner, paired both ways, may borrow a window");
    }
    if !enabled {
        return Some(
            "This computer does not lend its windows (Settings, \"Let my computers borrow my windows\")",
        );
    }
    None
}

// ---- Sway's commands ----

/// The commands that move the window with container id `id` onto the
/// screen `output`, alone and with no border; first out of full screen if
/// it was in it. `None` for an output name that is not plain.
pub fn sway_send(id: i64, output: &str, fullscreen: i32) -> Option<Vec<String>> {
    if !is_plain(output) {
        return None;
    }
    let c = format!("[con_id={id}]");
    let mut commands = Vec::new();
    if fullscreen != 0 {
        commands.push(format!("{c} fullscreen disable"));
    }
    commands.push(format!("{c} floating enable"));
    commands.push(format!("{c} border none"));
    commands.push(format!("{c} move container to output {output}"));
    Some(commands)
}

/// Size the floating window to `width`×`height` and put it at (`x`, `y`) in
/// the layout. Sent after it is on its screen (sent with the move, the size
/// did not take, found in the live test).
pub fn sway_place(id: i64, x: i32, y: i32, width: i32, height: i32) -> Vec<String> {
    let c = format!("[con_id={id}]");
    vec![
        format!("{c} resize set width {width} px height {height} px"),
        format!("{c} move absolute position {x} {y}"),
    ]
}

/// The commands that put a lent window back: onto its workspace (by name,
/// else number, else its output), tiled again if it was, with its border,
/// and, if it floated, its size and place; full screen again if it was.
pub fn sway_back(lent: &Lent) -> Option<Vec<String>> {
    let id: i64 = lent.window.parse().ok()?;
    let c = format!("[con_id={id}]");
    let target = if is_plain(&lent.workspace) {
        format!("workspace \"{}\"", lent.workspace)
    } else if lent.workspace_id >= 0 {
        format!("workspace number {}", lent.workspace_id)
    } else if is_plain(&lent.from_output) {
        format!("output {}", lent.from_output)
    } else {
        return None;
    };
    let mut commands = vec![format!("{c} move container to {target}")];
    commands.push(format!(
        "{c} floating {}",
        if lent.floating { "enable" } else { "disable" }
    ));
    // The border before the size: a title bar and border are part of what
    // the container's place is counted from.
    match lent.border.as_str() {
        style @ ("normal" | "pixel") if lent.border_width > 0 => {
            commands.push(format!("{c} border {style} {}", lent.border_width));
        }
        style @ ("normal" | "pixel" | "none" | "csd") => {
            commands.push(format!("{c} border {style}"));
        }
        _ => {}
    }
    if lent.floating {
        let frame = if lent.frame[0] > 0 && lent.frame[1] > 0 {
            lent.frame
        } else {
            lent.size
        };
        commands.extend(sway_place(
            id,
            lent.origin[0],
            lent.origin[1],
            frame[0],
            frame[1],
        ));
    }
    if lent.fullscreen != 0 {
        commands.push(format!("{c} fullscreen enable"));
    }
    Some(commands)
}

// ---- Hyprland's Lua ----

/// Whether `address` is a Hyprland window address.
fn is_hypr_address(address: &str) -> bool {
    address.starts_with("0x") && is_window_id(address)
}

/// `eval` of a Lua dispatch of `dispatcher` (a `hl.dsp…` call).
fn hypr_dispatch(dispatcher: &str) -> String {
    format!("eval hl.dispatch({dispatcher})")
}

/// Move the window to the screen `monitor`, the focus staying where it is.
pub fn hypr_to_monitor(address: &str, monitor: &str) -> Option<String> {
    (is_hypr_address(address) && is_plain(monitor)).then(|| {
        hypr_dispatch(&format!(
            "hl.dsp.window.move({{ monitor = \"{monitor}\", follow = false, window = \"address:{address}\" }})"
        ))
    })
}

/// Move the window to the workspace `selector` (an id, or a name), the
/// focus staying where it is.
pub fn hypr_to_workspace(address: &str, selector: &str) -> Option<String> {
    (is_hypr_address(address) && is_plain(selector)).then(|| {
        hypr_dispatch(&format!(
            "hl.dsp.window.move({{ workspace = \"{selector}\", follow = false, window = \"address:{address}\" }})"
        ))
    })
}

/// The workspace to move a window back to: its id when it is an ordinary
/// workspace's (1 on), else its name (a special workspace's).
pub fn hypr_workspace(id: i32, name: &str) -> Option<String> {
    if id >= 1 {
        Some(id.to_string())
    } else if is_plain(name) {
        Some(name.to_string())
    } else {
        None
    }
}

/// Fill the screen on the compositor's side (`internal` 2) while the program
/// is told `client` (0: nothing), or give the states back.
pub fn hypr_fullscreen(address: &str, internal: i32, client: i32) -> Option<String> {
    let state = 0..=3;
    (is_hypr_address(address) && state.contains(&internal) && state.contains(&client)).then(|| {
        hypr_dispatch(&format!(
            "hl.dsp.window.fullscreen_state({{ internal = {internal}, client = {client}, window = \"address:{address}\" }})"
        ))
    })
}

/// Size a floating window and put it at (`x`, `y`) in the layout.
pub fn hypr_place(address: &str, x: i32, y: i32, width: i32, height: i32) -> Option<Vec<String>> {
    (is_hypr_address(address) && width > 0 && height > 0).then(|| {
        vec![
            hypr_dispatch(&format!(
                "hl.dsp.window.resize({{ x = {width}, y = {height}, exact = true, window = \"address:{address}\" }})"
            )),
            hypr_dispatch(&format!(
                "hl.dsp.window.move({{ x = {x}, y = {y}, exact = true, window = \"address:{address}\" }})"
            )),
        ]
    })
}

/// A window of `j/clients`, with what lending reads.
#[derive(Debug, Deserialize)]
struct Client {
    address: String,
    #[serde(default)]
    mapped: bool,
    #[serde(default)]
    floating: bool,
    #[serde(default)]
    at: [i32; 2],
    #[serde(default)]
    size: [i32; 2],
    #[serde(default)]
    monitor: i32,
    #[serde(default)]
    workspace: crate::hyprland::HyprWorkspaceRef,
    #[serde(default)]
    fullscreen: i32,
    #[serde(default, rename = "fullscreenClient")]
    fullscreen_client: i32,
}

/// The window `address` in Hyprland's `j/clients`, on the screen the
/// `j/monitors` names; `None` when it is not there (or not mapped).
pub fn hypr_seen(clients: &str, monitors: &str, address: &str) -> std::io::Result<Option<Seen>> {
    let clients: Vec<Client> = serde_json::from_str(clients).map_err(std::io::Error::other)?;
    let monitors = crate::virtual_screen::hypr_outputs(monitors)?;
    Ok(clients
        .into_iter()
        .find(|c| c.address == address && c.mapped)
        .map(|c| Seen {
            id: c.address,
            floating: c.floating,
            at: c.at,
            size: c.size,
            origin: c.at,
            frame: c.size,
            output: usize::try_from(c.monitor)
                .ok()
                .and_then(|i| monitors.get(i))
                .map(|m| m.name.clone())
                .unwrap_or_default(),
            workspace: c.workspace.name,
            workspace_id: c.workspace.id,
            border: String::new(),
            border_width: 0,
            fullscreen: c.fullscreen,
            fullscreen_client: c.fullscreen_client,
        }))
}

// ---- The lending ----

/// What lending needs from the computer: [`Live`], or a fake in tests.
trait Env: Sync {
    /// Why no window can be lent here, if none can.
    fn cannot(&self) -> Option<&'static str>;
    fn seen(&self, window: &str) -> std::io::Result<Option<Seen>>;
    /// The screen of the key, made at `width`×`height` or the one it has:
    /// `{name, index, width, height}` or `{error}`.
    fn add_screen(&self, key: &str, width: u32, height: u32) -> Value;
    fn drop_screen(&self, key: &str);
    fn output(&self, name: &str) -> Option<Output>;
    /// Put the window alone on `to`, `width`×`height`.
    fn send(&self, lent: &Lent, to: &Output, width: u32, height: u32) -> std::io::Result<()>;
    /// Put the window back where `lent` says.
    fn back(&self, lent: &Lent) -> std::io::Result<()>;
    fn save(&self, lent: &[Lent]);
}

/// The windows lent, and the lock one ask at a time holds.
struct Lends {
    lent: Mutex<Vec<Lent>>,
    one: Mutex<()>,
}

impl Lends {
    const fn new() -> Self {
        Self {
            lent: Mutex::new(Vec::new()),
            one: Mutex::new(()),
        }
    }

    fn all(&self) -> std::sync::MutexGuard<'_, Vec<Lent>> {
        self.lent.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Take out (and save without) the windows `matching` says.
    fn take(&self, env: &dyn Env, matching: impl Fn(&Lent) -> bool) -> Vec<Lent> {
        let mut all = self.all();
        let (taken, kept): (Vec<Lent>, Vec<Lent>) = all.drain(..).partition(|l| matching(l));
        *all = kept;
        if !taken.is_empty() {
            env.save(&all);
        }
        taken
    }
}

fn lends() -> &'static Lends {
    static LENDS: Lends = Lends::new();
    &LENDS
}

fn error(why: impl Into<String>) -> Value {
    json!({ "error": why.into() })
}

/// Lend `window` to `device`: `{name, index, width, height, window}` or
/// `{error}`.
fn lend_with(lends: &Lends, env: &dyn Env, device: &str, window: &str) -> Value {
    if let Some(why) = env.cannot() {
        return error(why);
    }
    if !is_window_id(window) {
        return error("not a window");
    }
    let _one = lends.one.lock().unwrap_or_else(|e| e.into_inner());
    let key = lend_key(device, window);
    let held = lends.all().iter().find(|l| l.window == window).cloned();
    if let Some(held) = held {
        if held.device != device {
            return error("Another computer has borrowed that window");
        }
        // Asked again: the same screen (a session may be showing it).
        if let Some(screen) = env.output(&held.output) {
            let mut reply = env.add_screen(
                &key,
                u32::try_from(screen.rect.width).unwrap_or(0),
                u32::try_from(screen.rect.height).unwrap_or(0),
            );
            if reply.get("error").is_none() {
                reply["window"] = window.into();
            }
            return reply;
        }
        // Its screen is gone: forget it and lend afresh.
        lends.take(env, |l| l.window == window);
        env.drop_screen(&key);
    }
    let seen = match env.seen(window) {
        Ok(Some(seen)) => seen,
        Ok(None) => return error("No such window on this computer (it may have closed)"),
        Err(e) => return error(format!("Could not read this computer's windows: {e}")),
    };
    let Some((width, height)) = lend_size(seen.size) else {
        return error("not a window size");
    };
    let mut reply = env.add_screen(&key, width, height);
    if reply.get("error").is_some() {
        return reply;
    }
    let Some(name) = reply["name"].as_str().map(str::to_string) else {
        return error("the computer made no screen");
    };
    let Some(screen) = env.output(&name) else {
        env.drop_screen(&key);
        return error("The computer made a screen but does not show it");
    };
    let lent = Lent::new(device, &seen, &name);
    {
        // Written before the move: a crash between leaves what returns it.
        let mut all = lends.all();
        all.push(lent.clone());
        env.save(&all);
    }
    match env.send(&lent, &screen, width, height) {
        Ok(()) => {
            tracing::info!("Lent window {window} to a computer on the screen {name}");
            reply["window"] = window.into();
            reply
        }
        Err(e) => {
            lends.take(env, |l| l.window == window);
            let _ = env.back(&lent);
            env.drop_screen(&key);
            error(format!("Could not move the window: {e}"))
        }
    }
}

/// Take back `window` from `device`, or, with none, all it has: `{returned:
/// n}`, or `{error, returned}` when one could not be put back exactly (its
/// screen is taken away all the same, and the compositor places the
/// window).
fn return_with(lends: &Lends, env: &dyn Env, device: &str, window: Option<&str>) -> Value {
    let _one = lends.one.lock().unwrap_or_else(|e| e.into_inner());
    let taken = lends.take(env, |l| {
        l.device == device && window.is_none_or(|w| l.window == w)
    });
    let (mut returned, mut failed) = (0, None);
    for lent in &taken {
        match env.back(lent) {
            Ok(()) => returned += 1,
            Err(e) => failed = failed.or(Some(format!("Could not put a window back: {e}"))),
        }
        env.drop_screen(&lent.key());
    }
    match failed {
        Some(why) => json!({ "error": why, "returned": returned }),
        None => json!({ "returned": returned }),
    }
}

/// Put back what is lent on `output`, as it is about to be removed.
fn give_back_output_with(lends: &Lends, env: &dyn Env, output: &str) {
    for lent in lends.take(env, |l| l.output == output) {
        if let Err(e) = env.back(&lent) {
            tracing::warn!("Could not put a lent window back: {e}");
        }
    }
}

/// Called as the screen `output` is removed (a borrowing computer gone, a
/// return, a sweep): its window, if one is lent on it, goes back first.
pub fn give_back_for_output(output: &str) {
    if lends().all().iter().any(|l| l.output == output) {
        give_back_output_with(lends(), &Live, output);
    }
}

/// What the file says was lent by a server in `instance`: all of it in the
/// same compositor, none of it in another (it is gone with its windows).
fn left_by(file: &File, instance: Option<&str>) -> Vec<Lent> {
    match (file.instance.as_deref(), instance) {
        (Some(was), Some(now)) if was == now => file.lent.clone(),
        _ => Vec::new(),
    }
}

/// Put back the windows a server that stopped left lent (at start), and
/// take their screens away; the names of the screens it removed. Holds
/// nothing: run inside the virtual screens' own sweep.
pub fn return_left_over() -> Vec<String> {
    let Ok(dir) = crate::state::state_dir() else {
        return Vec::new();
    };
    let path = dir.join(STATE_FILE);
    let Some(file) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<File>(&raw).ok())
    else {
        return Vec::new();
    };
    let mut removed = Vec::new();
    for lent in left_by(&file, crate::virtual_screen::this_instance().as_deref()) {
        tracing::info!("Putting back a window a stopped server left lent");
        if let Err(e) = Live.back(&lent) {
            tracing::warn!("Could not put a left-over lent window back: {e}");
        }
        crate::virtual_screen::remove(&lent.output);
        removed.push(lent.output);
    }
    let _ = std::fs::remove_file(path);
    removed
}

/// Put back everything lent, as the server stops. Waits at most 5 s.
pub async fn return_all_on_stop() {
    let work = || {
        let lends = lends();
        let _one = lends.one.lock().unwrap_or_else(|e| e.into_inner());
        for lent in lends.take(&Live, |_| true) {
            if let Err(e) = Live.back(&lent) {
                tracing::warn!("Could not put a lent window back at stop: {e}");
            }
            Live.drop_screen(&lent.key());
        }
    };
    if tokio::time::timeout(Duration::from_secs(5), tokio::task::spawn_blocking(work))
        .await
        .is_err()
    {
        tracing::warn!("Putting the lent windows back is slow; stopping without waiting");
    }
}

/// What the state file holds.
#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    /// The compositor they were lent in, as the virtual screens' record
    /// names it.
    #[serde(default)]
    instance: Option<String>,
    #[serde(default)]
    lent: Vec<Lent>,
}

/// Whether `device` is one of the owner's computers, paired both ways here.
fn is_owner_computer(device: &str) -> bool {
    use pocketdesk_core::local::computers;
    use pocketdesk_core::protocol::kdeconnect::TrustStore;
    let (Ok(dir), Ok(trust)) = (crate::state::state_dir(), crate::state::trust_store_path()) else {
        return false;
    };
    let Ok(store) = TrustStore::read(trust) else {
        return false;
    };
    store.is_computer(device)
        && crate::remote_devices::is_owner_device(&store, &computers::read(&dir), device)
}

/// The answer to the `lend` or `return` request `body` from `device`: see
/// [`lend_with`] and [`return_with`].
pub async fn answer(device: &str, body: &Value) -> Value {
    let action = body["action"].as_str().unwrap_or("").to_string();
    let window = body
        .get("window")
        .and_then(Value::as_str)
        .map(str::to_string);
    let device = device.to_string();
    tokio::task::spawn_blocking(move || {
        if let Some(why) = refusal(&action, enabled(), is_owner_computer(&device)) {
            return error(why);
        }
        match (action.as_str(), window.as_deref()) {
            ("lend", Some(window)) => lend_with(lends(), &Live, &device, window),
            ("lend", None) => error("no window named"),
            ("return", window) => return_with(lends(), &Live, &device, window),
            _ => error("unknown action"),
        }
    })
    .await
    .unwrap_or_else(|_| error("the computer failed to answer"))
}

// ---- The compositor ----

/// Poll `look` until it gives something, at most `wait`.
fn wait_for<T>(wait: Duration, mut look: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + wait;
    loop {
        if let Some(found) = look() {
            return Some(found);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn other(why: impl Into<String>) -> std::io::Error {
    std::io::Error::other(why.into())
}

/// The window now, whichever compositor this is.
fn seen_now(window: &str) -> std::io::Result<Option<Seen>> {
    match crate::virtual_screen::desk() {
        Some(Desk::Sway) => {
            let id: i64 = window.parse().map_err(|_| other("not a Sway window id"))?;
            Ok(crate::sway_tree::seen(id)?.map(|w| Seen {
                id: w.id.to_string(),
                floating: w.floating,
                at: w.at,
                size: w.size,
                origin: w.origin,
                frame: w.frame,
                output: w.output,
                workspace: w.workspace,
                workspace_id: w.workspace_num,
                border: w.border,
                border_width: w.border_width,
                fullscreen: w.fullscreen,
                fullscreen_client: 0,
            }))
        }
        Some(Desk::Hyprland) => {
            if !is_hypr_address(window) {
                return Err(other("not a Hyprland window address"));
            }
            hypr_seen(
                &crate::virtual_screen::hypr("j/clients")?,
                &crate::virtual_screen::hypr("j/monitors")?,
                window,
            )
        }
        None => Err(other("not running under Sway or Hyprland")),
    }
}

/// Send each of Hyprland's requests, each answered `ok`.
fn hypr_all(requests: impl IntoIterator<Item = String>) -> std::io::Result<()> {
    for request in requests {
        crate::virtual_screen::hypr_ok(&crate::virtual_screen::hypr(&request)?)?;
    }
    Ok(())
}

/// Sway commands, one by one.
fn sway_all(commands: impl IntoIterator<Item = String>) -> std::io::Result<()> {
    for command in commands {
        crate::virtual_screen::run(&command)?;
    }
    Ok(())
}

/// The compositor this server runs in.
struct Live;

impl Env for Live {
    fn cannot(&self) -> Option<&'static str> {
        crate::virtual_screen::desk()
            .is_none()
            .then_some("This computer cannot lend a window: only Sway and Hyprland can for now")
    }

    fn seen(&self, window: &str) -> std::io::Result<Option<Seen>> {
        seen_now(window)
    }

    fn add_screen(&self, key: &str, width: u32, height: u32) -> Value {
        crate::virtual_screen::add_for_lend(key, width, height)
    }

    fn drop_screen(&self, key: &str) {
        crate::virtual_screen::drop_for_lend(key);
    }

    fn output(&self, name: &str) -> Option<Output> {
        crate::virtual_screen::outputs()
            .ok()?
            .into_iter()
            .find(|o| o.name == name)
    }

    fn send(&self, lent: &Lent, to: &Output, width: u32, height: u32) -> std::io::Result<()> {
        let (w, h) = (width as i32, height as i32);
        let wait = Duration::from_secs(2);
        match crate::virtual_screen::desk() {
            Some(Desk::Sway) => {
                let id: i64 = lent
                    .window
                    .parse()
                    .map_err(|_| other("not a Sway window id"))?;
                sway_all(
                    sway_send(id, &to.name, lent.fullscreen)
                        .ok_or_else(|| other("not an output this computer makes"))?,
                )?;
                wait_for(wait, || {
                    crate::sway_tree::seen(id)
                        .ok()
                        .flatten()
                        .filter(|s| s.output == to.name)
                })
                .ok_or_else(|| other("Sway did not move it"))?;
                // The size takes a moment after the move: asked again once.
                for _ in 0..2 {
                    sway_all(sway_place(id, to.rect.x, to.rect.y, w, h))?;
                    let there = wait_for(Duration::from_secs(1), || {
                        crate::sway_tree::seen(id)
                            .ok()
                            .flatten()
                            .filter(|s| s.size == [w, h] && s.at == [to.rect.x, to.rect.y])
                    });
                    if there.is_some() {
                        return Ok(());
                    }
                }
                tracing::warn!("A lent window did not take its screen's size");
                Ok(())
            }
            Some(Desk::Hyprland) => {
                let a = lent.window.as_str();
                if lent.fullscreen != 0 || lent.fullscreen_client != 0 {
                    hypr_all(hypr_fullscreen(a, 0, 0))?;
                }
                hypr_all(hypr_to_monitor(a, &to.name))?;
                hypr_all(hypr_fullscreen(a, 2, 0))?;
                wait_for(wait, || {
                    seen_now(a)
                        .ok()
                        .flatten()
                        .filter(|s| s.output == to.name && s.size == [w, h])
                })
                .map(|_| ())
                .ok_or_else(|| other("Hyprland did not put it alone on the screen"))
            }
            None => Err(other("not running under Sway or Hyprland")),
        }
    }

    fn back(&self, lent: &Lent) -> std::io::Result<()> {
        // Closed meanwhile: nothing to put back.
        if seen_now(&lent.window)?.is_none() {
            return Ok(());
        }
        let wait = Duration::from_secs(2);
        match crate::virtual_screen::desk() {
            Some(Desk::Sway) => {
                sway_all(sway_back(lent).ok_or_else(|| other("no place to put it back"))?)?
            }
            Some(Desk::Hyprland) => {
                let a = lent.window.as_str();
                hypr_all(hypr_fullscreen(a, 0, 0))?;
                let place = hypr_workspace(lent.workspace_id, &lent.workspace)
                    .and_then(|ws| hypr_to_workspace(a, &ws))
                    .ok_or_else(|| other("no place to put it back"))?;
                hypr_all([place])?;
                if lent.floating
                    && let Some(place) =
                        hypr_place(a, lent.at[0], lent.at[1], lent.size[0], lent.size[1])
                {
                    hypr_all(place)?;
                }
                if lent.fullscreen != 0 || lent.fullscreen_client != 0 {
                    hypr_all(hypr_fullscreen(a, lent.fullscreen, lent.fullscreen_client))?;
                }
            }
            None => return Err(other("not running under Sway or Hyprland")),
        }
        wait_for(wait, || {
            seen_now(&lent.window)
                .ok()
                .map(|s| s.is_none_or(|s| s.output != lent.output))
                .filter(|moved| *moved)
        })
        .ok_or_else(|| other("the compositor did not move it back"))?;
        // Sway's tree shows the last command a moment late (the border here),
        // and a window lent again at once would be read as it shows: wait for
        // it to say what was set.
        if !lent.border.is_empty() {
            wait_for(Duration::from_secs(1), || {
                seen_now(&lent.window)
                    .ok()
                    .map(|s| s.is_none_or(|s| s.border == lent.border || s.border.is_empty()))
                    .filter(|settled| *settled)
            });
        }
        Ok(())
    }

    fn save(&self, lent: &[Lent]) {
        let Ok(dir) = crate::state::state_dir() else {
            return;
        };
        let path = dir.join(STATE_FILE);
        if lent.is_empty() {
            let _ = std::fs::remove_file(path);
            return;
        }
        let file = File {
            instance: crate::virtual_screen::this_instance(),
            lent: lent.to_vec(),
        };
        if let Ok(json) = serde_json::to_string(&file) {
            let tmp = pocketdesk_core::atomic_file::temp_beside(&path);
            if pocketdesk_core::secure_fs::write_private(&tmp, json.as_bytes()).is_ok() {
                let _ = pocketdesk_core::atomic_file::replace(&tmp, path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::virtual_screen::Rect;

    const DEVICE: &str = "0123456789abcdef";

    fn seen(id: &str, w: i32, h: i32) -> Seen {
        Seen {
            id: id.to_string(),
            floating: false,
            at: [10, 40],
            size: [w, h],
            origin: [8, 13],
            frame: [w + 4, h + 29],
            output: "OUT-1".into(),
            workspace: "3".into(),
            workspace_id: 3,
            border: "normal".into(),
            border_width: 2,
            fullscreen: 0,
            fullscreen_client: 0,
        }
    }

    /// A computer with windows and screens, every call kept.
    #[derive(Default)]
    struct Fake {
        windows: Mutex<Vec<Seen>>,
        screens: Mutex<Vec<(String, String, u32, u32)>>,
        calls: Mutex<Vec<String>>,
        saved: Mutex<Vec<Lent>>,
        fail_send: bool,
        fail_back: bool,
    }

    impl Fake {
        fn with(windows: Vec<Seen>) -> Self {
            Self {
                windows: Mutex::new(windows),
                ..Self::default()
            }
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Env for Fake {
        fn cannot(&self) -> Option<&'static str> {
            None
        }
        fn seen(&self, window: &str) -> std::io::Result<Option<Seen>> {
            Ok(self
                .windows
                .lock()
                .unwrap()
                .iter()
                .find(|w| w.id == window)
                .cloned())
        }
        fn add_screen(&self, key: &str, width: u32, height: u32) -> Value {
            let mut screens = self.screens.lock().unwrap();
            if let Some((_, name, w, h)) = screens.iter().find(|s| s.0 == key) {
                return json!({ "name": name, "index": 1, "width": w, "height": h });
            }
            if screens.len() >= 4 {
                return error("The computer has as many extra screens as it takes");
            }
            let name = format!("PD-0a1b2c3d-{}", screens.len() + 1);
            screens.push((key.to_string(), name.clone(), width, height));
            json!({ "name": name, "index": 1, "width": width, "height": height })
        }
        fn drop_screen(&self, key: &str) {
            self.calls.lock().unwrap().push(format!("drop {key}"));
            self.screens.lock().unwrap().retain(|s| s.0 != key);
        }
        fn output(&self, name: &str) -> Option<Output> {
            let screens = self.screens.lock().unwrap().clone();
            let i = screens.iter().position(|s| s.1 == name)?;
            let s = &screens[i];
            Some(Output {
                name: s.1.clone(),
                rect: Rect {
                    x: 1920 * (i as i32 + 1),
                    y: 0,
                    width: s.2 as i32,
                    height: s.3 as i32,
                },
            })
        }
        fn send(&self, lent: &Lent, to: &Output, width: u32, height: u32) -> std::io::Result<()> {
            self.calls.lock().unwrap().push(format!(
                "send {} to {} {width}x{height}",
                lent.window, to.name
            ));
            if self.fail_send {
                return Err(other("refused"));
            }
            Ok(())
        }
        fn back(&self, lent: &Lent) -> std::io::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("back {} to {}", lent.window, lent.workspace));
            if self.fail_back {
                return Err(other("stuck"));
            }
            Ok(())
        }
        fn save(&self, lent: &[Lent]) {
            *self.saved.lock().unwrap() = lent.to_vec();
        }
    }

    #[test]
    fn a_window_id_is_digits_or_a_hex_address_and_nothing_else() {
        for id in ["5", "123456789012", "0x560a772079f0", "0x1"] {
            assert!(is_window_id(id), "{id}");
        }
        for id in [
            "",
            "0x",
            "1234567890123",
            "-1",
            "0xZZ",
            "0X1f",
            "0x1F",
            "5; exec true",
            "address:0x1",
            "0x1\")os.execute(\"true\")--",
            "5 ",
        ] {
            assert!(!is_window_id(id), "{id}");
        }
    }

    #[test]
    fn the_screen_is_the_windows_size_within_the_screens_bounds() {
        assert_eq!(lend_size([1280, 720]), Some((1280, 720)));
        assert_eq!(lend_size([1281, 721]), Some((1280, 720)), "even sides");
        assert_eq!(lend_size([100, 100]), Some((320, 320)), "at least 320");
        assert_eq!(lend_size([5120, 1440]), Some((2560, 720)), "scaled down");
        assert_eq!(lend_size([0, 720]), None);
        assert_eq!(lend_size([-5, 720]), None);
    }

    #[test]
    fn only_a_computer_paired_both_ways_borrows_and_only_with_the_switch_on() {
        assert!(refusal("lend", true, true).is_none());
        assert!(
            refusal("lend", true, false)
                .unwrap()
                .contains("paired both ways")
        );
        assert!(
            refusal("lend", false, true)
                .unwrap()
                .contains("Let my computers borrow my windows")
        );
        assert!(refusal("lend", false, false).is_some());
        // Taking a window back is never refused.
        assert!(refusal("return", false, false).is_none());
        assert!(refusal("return", true, true).is_none());
    }

    #[test]
    fn sway_gets_the_window_floating_bare_and_moved_then_sized() {
        assert_eq!(
            sway_send(5, "HEADLESS-2", 0).unwrap(),
            [
                "[con_id=5] floating enable",
                "[con_id=5] border none",
                "[con_id=5] move container to output HEADLESS-2"
            ]
        );
        assert_eq!(
            sway_send(5, "HEADLESS-2", 1).unwrap()[0],
            "[con_id=5] fullscreen disable"
        );
        assert_eq!(
            sway_place(5, 1920, 0, 1280, 720),
            [
                "[con_id=5] resize set width 1280 px height 720 px",
                "[con_id=5] move absolute position 1920 0"
            ]
        );
        assert!(sway_send(5, "X; exec true", 0).is_none());
        assert!(sway_send(5, "", 0).is_none());
    }

    fn lent_tiled() -> Lent {
        Lent::new(DEVICE, &seen("5", 1280, 720), "HEADLESS-2")
    }

    #[test]
    fn sway_puts_a_tiled_window_back_on_its_workspace_with_its_border() {
        assert_eq!(
            sway_back(&lent_tiled()).unwrap(),
            [
                "[con_id=5] move container to workspace \"3\"",
                "[con_id=5] floating disable",
                "[con_id=5] border normal 2"
            ]
        );
    }

    #[test]
    fn sway_puts_a_floating_full_screen_window_back_as_it_was() {
        let mut seen = seen("5", 400, 300);
        seen.floating = true;
        seen.at = [2000, 100];
        seen.origin = [1998, 73];
        seen.frame = [404, 329];
        seen.border = "pixel".into();
        seen.border_width = 2;
        seen.fullscreen = 1;
        let lent = Lent::new(DEVICE, &seen, "HEADLESS-2");
        assert_eq!(
            sway_back(&lent).unwrap(),
            [
                "[con_id=5] move container to workspace \"3\"",
                "[con_id=5] floating enable",
                "[con_id=5] border pixel 2",
                "[con_id=5] resize set width 404 px height 329 px",
                "[con_id=5] move absolute position 1998 73",
                "[con_id=5] fullscreen enable"
            ]
        );
    }

    #[test]
    fn sway_falls_back_to_the_workspace_number_then_the_output() {
        let mut lent = lent_tiled();
        lent.workspace = "w\"; exec true".into();
        assert_eq!(
            sway_back(&lent).unwrap()[0],
            "[con_id=5] move container to workspace number 3"
        );
        lent.workspace_id = -1;
        assert_eq!(
            sway_back(&lent).unwrap()[0],
            "[con_id=5] move container to output OUT-1"
        );
        lent.from_output = "a;b".into();
        assert!(sway_back(&lent).is_none());
        let mut lent = lent_tiled();
        lent.window = "0x5".into();
        assert!(sway_back(&lent).is_none(), "not a Sway id");
    }

    #[test]
    fn hyprland_is_driven_by_lua_dispatchers_on_a_checked_address() {
        let a = "0x560a772079f0";
        assert_eq!(
            hypr_to_monitor(a, "PD-0a1b2c3d-1").unwrap(),
            r#"eval hl.dispatch(hl.dsp.window.move({ monitor = "PD-0a1b2c3d-1", follow = false, window = "address:0x560a772079f0" }))"#
        );
        assert_eq!(
            hypr_fullscreen(a, 2, 0).unwrap(),
            r#"eval hl.dispatch(hl.dsp.window.fullscreen_state({ internal = 2, client = 0, window = "address:0x560a772079f0" }))"#
        );
        assert_eq!(
            hypr_to_workspace(a, "2").unwrap(),
            r#"eval hl.dispatch(hl.dsp.window.move({ workspace = "2", follow = false, window = "address:0x560a772079f0" }))"#
        );
        let place = hypr_place(a, 100, 80, 600, 400).unwrap();
        assert_eq!(
            place[0],
            r#"eval hl.dispatch(hl.dsp.window.resize({ x = 600, y = 400, exact = true, window = "address:0x560a772079f0" }))"#
        );
        assert_eq!(
            place[1],
            r#"eval hl.dispatch(hl.dsp.window.move({ x = 100, y = 80, exact = true, window = "address:0x560a772079f0" }))"#
        );
        assert_eq!(hypr_workspace(2, "2").as_deref(), Some("2"));
        assert_eq!(
            hypr_workspace(-98, "special:scratch").as_deref(),
            Some("special:scratch")
        );
        assert_eq!(hypr_workspace(-98, "a\"b"), None);
    }

    #[test]
    fn nothing_but_a_checked_address_and_names_reach_the_lua() {
        for address in [
            "5",
            "0x",
            "0x1\"})os.execute(\"true\")--",
            "",
            "address:0x1",
        ] {
            assert!(
                hypr_to_monitor(address, "PD-0a1b2c3d-1").is_none(),
                "{address}"
            );
            assert!(hypr_fullscreen(address, 2, 0).is_none(), "{address}");
            assert!(hypr_to_workspace(address, "1").is_none(), "{address}");
            assert!(hypr_place(address, 0, 0, 10, 10).is_none(), "{address}");
        }
        assert!(hypr_to_monitor("0x1", "m\"})os.execute(\"true\")--").is_none());
        assert!(hypr_to_workspace("0x1", "1\"; x").is_none());
        assert!(hypr_fullscreen("0x1", 9, 0).is_none());
        assert!(hypr_fullscreen("0x1", 2, -1).is_none());
        assert!(hypr_place("0x1", 0, 0, 0, 10).is_none());
    }

    const CLIENTS: &str = r#"[
        {"address":"0x1a","mapped":true,"hidden":false,"at":[21,21],"size":[911,477],
         "workspace":{"id":2,"name":"2"},"floating":false,"monitor":1,
         "class":"app","title":"t","pid":7,"fullscreen":0,"fullscreenClient":0},
        {"address":"0x1b","mapped":false,"at":[0,0],"size":[1,1],"workspace":{"id":1,"name":"1"},
         "floating":true,"monitor":0,"fullscreen":2,"fullscreenClient":2}]"#;
    const MONITORS: &str = r#"[{"name":"OUT-1","x":0,"y":0,"width":1906,"height":1038},
        {"name":"PD-0a1b2c3d-1","x":1906,"y":0,"width":911,"height":477}]"#;

    #[test]
    fn a_hyprland_window_is_read_from_its_clients_and_monitors() {
        let seen = hypr_seen(CLIENTS, MONITORS, "0x1a").unwrap().unwrap();
        assert_eq!(seen.output, "PD-0a1b2c3d-1");
        assert_eq!((seen.workspace.as_str(), seen.workspace_id), ("2", 2));
        assert_eq!(
            (seen.at, seen.size, seen.floating),
            ([21, 21], [911, 477], false)
        );
        assert!(
            hypr_seen(CLIENTS, MONITORS, "0x1b").unwrap().is_none(),
            "unmapped"
        );
        assert!(hypr_seen(CLIENTS, MONITORS, "0xff").unwrap().is_none());
        assert!(hypr_seen("nonsense", MONITORS, "0x1a").is_err());
    }

    #[test]
    fn a_window_is_lent_on_a_screen_of_its_size_and_remembered() {
        let fake = Fake::with(vec![seen("5", 1280, 720)]);
        let lends = Lends::new();
        let reply = lend_with(&lends, &fake, DEVICE, "5");
        assert_eq!(reply["name"], "PD-0a1b2c3d-1", "{reply}");
        assert_eq!(
            (reply["width"].as_u64(), reply["height"].as_u64()),
            (Some(1280), Some(720))
        );
        assert_eq!(reply["window"], "5");
        assert_eq!(reply["index"], 1);
        assert_eq!(
            fake.calls(),
            ["send 5 to PD-0a1b2c3d-1 1280x720"],
            "moved onto the screen, sized to it"
        );
        let saved = fake.saved.lock().unwrap().clone();
        assert_eq!(saved.len(), 1, "written to the state file");
        assert_eq!(saved[0].from_output, "OUT-1");
        assert_eq!(saved[0].workspace, "3");
        assert_eq!(saved[0].output, "PD-0a1b2c3d-1");
        assert_eq!(saved[0].key(), format!("{DEVICE}#lend-5"));
    }

    #[test]
    fn a_window_too_big_gets_a_screen_within_the_bounds() {
        let fake = Fake::with(vec![seen("5", 5120, 1440)]);
        let reply = lend_with(&Lends::new(), &fake, DEVICE, "5");
        assert_eq!(
            (reply["width"].as_u64(), reply["height"].as_u64()),
            (Some(2560), Some(720))
        );
        assert_eq!(fake.calls(), ["send 5 to PD-0a1b2c3d-1 2560x720"]);
    }

    #[test]
    fn asked_again_it_is_the_same_screen_and_no_second_move() {
        let fake = Fake::with(vec![seen("5", 1280, 720)]);
        let lends = Lends::new();
        let first = lend_with(&lends, &fake, DEVICE, "5");
        let again = lend_with(&lends, &fake, DEVICE, "5");
        assert_eq!(first["name"], again["name"]);
        assert_eq!(again["window"], "5");
        assert_eq!(fake.calls().len(), 1, "moved once: {:?}", fake.calls());
        assert_eq!(lends.all().len(), 1);
    }

    #[test]
    fn another_computer_cannot_take_a_window_that_is_lent() {
        let fake = Fake::with(vec![seen("5", 1280, 720)]);
        let lends = Lends::new();
        lend_with(&lends, &fake, DEVICE, "5");
        let refused = lend_with(&lends, &fake, "fedcba9876543210", "5");
        assert!(
            refused["error"]
                .as_str()
                .unwrap()
                .contains("Another computer"),
            "{refused}"
        );
        assert_eq!(fake.calls().len(), 1);
    }

    #[test]
    fn a_window_that_is_not_there_or_not_an_id_is_refused_untouched() {
        let fake = Fake::with(vec![seen("5", 1280, 720)]);
        let lends = Lends::new();
        let gone = lend_with(&lends, &fake, DEVICE, "6");
        assert!(gone["error"].as_str().unwrap().contains("No such window"));
        let odd = lend_with(&lends, &fake, DEVICE, "5; exec true");
        assert_eq!(odd["error"], "not a window");
        assert!(fake.calls().is_empty());
        assert!(fake.screens.lock().unwrap().is_empty(), "no screen made");
        assert!(lends.all().is_empty());
    }

    #[test]
    fn a_window_that_cannot_be_moved_is_not_lent_and_its_screen_goes() {
        let mut fake = Fake::with(vec![seen("5", 1280, 720)]);
        fake.fail_send = true;
        let lends = Lends::new();
        let reply = lend_with(&lends, &fake, DEVICE, "5");
        assert!(
            reply["error"].as_str().unwrap().contains("Could not move"),
            "{reply}"
        );
        assert!(lends.all().is_empty(), "no record kept");
        assert!(fake.saved.lock().unwrap().is_empty());
        assert!(
            fake.screens.lock().unwrap().is_empty(),
            "its screen is gone"
        );
        let calls = fake.calls();
        assert!(calls.iter().any(|c| c.starts_with("back 5")), "{calls:?}");
        assert!(calls.last().unwrap().starts_with("drop"), "{calls:?}");
    }

    #[test]
    fn the_screens_limit_refuses_a_fifth_and_leaves_its_window() {
        let fake = Fake::with((1..=5).map(|n| seen(&n.to_string(), 800, 600)).collect());
        let lends = Lends::new();
        for n in 1..=4 {
            let reply = lend_with(&lends, &fake, DEVICE, &n.to_string());
            assert!(reply.get("error").is_none(), "{reply}");
        }
        let reply = lend_with(&lends, &fake, DEVICE, "5");
        assert!(
            reply["error"]
                .as_str()
                .unwrap()
                .contains("as many extra screens")
        );
        assert_eq!(fake.calls().len(), 4, "the fifth was never moved");
        assert_eq!(lends.all().len(), 4);
    }

    #[test]
    fn a_returned_window_goes_back_and_its_screen_goes() {
        let fake = Fake::with(vec![seen("5", 1280, 720), seen("6", 800, 600)]);
        let lends = Lends::new();
        lend_with(&lends, &fake, DEVICE, "5");
        lend_with(&lends, &fake, DEVICE, "6");
        // Not this computer's to return.
        let other_device = return_with(&lends, &fake, "fedcba9876543210", Some("5"));
        assert_eq!(other_device["returned"], 0);
        assert_eq!(lends.all().len(), 2);
        let reply = return_with(&lends, &fake, DEVICE, Some("5"));
        assert_eq!(reply, json!({ "returned": 1 }));
        let calls = fake.calls();
        assert!(calls.contains(&"back 5 to 3".to_string()), "{calls:?}");
        assert!(
            calls.contains(&format!("drop {DEVICE}#lend-5")),
            "{calls:?}"
        );
        assert_eq!(lends.all().len(), 1);
        assert_eq!(fake.saved.lock().unwrap().len(), 1, "the file follows");
        assert_eq!(fake.screens.lock().unwrap().len(), 1);
        // With no window named, all it has.
        let all = return_with(&lends, &fake, DEVICE, None);
        assert_eq!(all, json!({ "returned": 1 }));
        assert!(lends.all().is_empty());
        assert!(fake.saved.lock().unwrap().is_empty());
        assert!(fake.screens.lock().unwrap().is_empty());
    }

    #[test]
    fn a_window_that_will_not_go_back_still_loses_its_screen_and_says_so() {
        let mut fake = Fake::with(vec![seen("5", 1280, 720)]);
        fake.fail_back = true;
        let lends = Lends::new();
        lend_with(&lends, &fake, DEVICE, "5");
        let reply = return_with(&lends, &fake, DEVICE, Some("5"));
        assert!(
            reply["error"]
                .as_str()
                .unwrap()
                .contains("Could not put a window back")
        );
        assert_eq!(reply["returned"], 0);
        assert!(fake.screens.lock().unwrap().is_empty());
        assert!(lends.all().is_empty());
    }

    #[test]
    fn a_screen_removed_gives_its_window_back_first() {
        let fake = Fake::with(vec![seen("5", 1280, 720), seen("6", 800, 600)]);
        let lends = Lends::new();
        let reply = lend_with(&lends, &fake, DEVICE, "5");
        lend_with(&lends, &fake, DEVICE, "6");
        let name = reply["name"].as_str().unwrap();
        give_back_output_with(&lends, &fake, name);
        assert!(fake.calls().contains(&"back 5 to 3".to_string()));
        assert!(!fake.calls().iter().any(|c| c.starts_with("back 6")));
        assert_eq!(lends.all().len(), 1, "the other stays lent");
        assert_eq!(lends.all()[0].window, "6");
    }

    #[test]
    fn a_stopped_servers_windows_are_returned_in_its_compositor_only() {
        let file = File {
            instance: Some("/run/user/1000/sway-ipc.1000.1234.sock".into()),
            lent: vec![lent_tiled()],
        };
        assert_eq!(
            left_by(&file, Some("/run/user/1000/sway-ipc.1000.1234.sock")).len(),
            1
        );
        assert!(
            left_by(&file, Some("/run/user/1000/sway-ipc.1000.9999.sock")).is_empty(),
            "a restarted compositor has none of them"
        );
        assert!(left_by(&file, None).is_empty());
        let none = File {
            instance: None,
            lent: vec![lent_tiled()],
        };
        assert!(left_by(&none, Some("x")).is_empty());
    }

    #[test]
    fn the_state_file_round_trips() {
        let file = File {
            instance: Some("hyprland:sig".into()),
            lent: vec![lent_tiled()],
        };
        let json = serde_json::to_string(&file).unwrap();
        let back: File = serde_json::from_str(&json).unwrap();
        assert_eq!(back.lent, file.lent);
        assert_eq!(back.instance, file.instance);
    }
}
