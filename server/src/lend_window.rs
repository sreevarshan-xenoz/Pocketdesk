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
//! - **Windows (phase 5):** no second screen (that would need a virtual
//!   display driver), so the window is lent where it stands: it is captured
//!   alone by Windows.Graphics.Capture under an index past every monitor's
//!   (`core`'s `window_target`), the borrower's pointer lands in its frame and
//!   it is brought to the front before a click, scroll or key, and what the
//!   owner sees is that the window is in use. Its rules and flow are in
//!   [`desktop`] (tested everywhere over a fake desktop), its calls to Windows
//!   in `win32` (checked on Windows only). Nothing is written to the state
//!   file there: a window handle is not kept across a restart.
//!
//! **Following the borrower's window** ([`lend_sized`]): a `lend` that names a
//! size (`width` and `height`, the picture area of the window that shows the
//! window, in pixels; a host before this ignores them) gives the screen
//! already made for the window that size where it is
//! ([`crate::virtual_screen::resize_for_lend`], bounded as any screen is), and
//! the window follows: on Sway `resize set` and `move absolute position` once
//! the output's mode is in, on Hyprland the compositor's own full screen
//! fills the new mode (and is set again if it did not). The borrower's view
//! then dials the same screen at its new size, so the picture is the window's,
//! one pixel to a pixel. A window not yet lent is lent at that size.
//!
//! **The list:** `list` answers the windows this computer would lend (id,
//! title, app, size, workspace; [`listed`]) so the other computer's owner can
//! pick one. Titles are personal: they are sent to the asker and never
//! logged.
//!
//! **Who may ask:** only a computer paired both ways ("both are mine"), with
//! the Input permission (held to it before this runs, on a connection and on
//! the link), and only while "Let my computers borrow my windows"
//! (`lend_windows`, off by default) is on, for `lend` and `list` alike;
//! taking a window back is never refused. Window ids are digits (Sway's container id) or `0x` and hex
//! (Hyprland's address), and nothing else reaches a command or the Lua.
//!
//! **Watching what is lent** ([`run_watcher`], awake only while a window is
//! lent, once a second): turning `lend_windows` off puts every lent window
//! back; a borrowing computer with no session, connection or link goes
//! straight away (the virtual screens' sweep, two 5 s checks and a minute's
//! grace, stays as the backstop); a lent window that is gone (two looks in
//! a row) has its empty screen removed and its borrower told
//! (`{lent_gone: <window>, reason}`, [`crate::borrow_window`]), so the view
//! of it ends.
//!
//! What was lent, from where, is kept in `lent_windows.json` (written before
//! the window is moved) so the next server in the same compositor puts them
//! back; in another compositor they are gone with it. The screens are the
//! virtual screens' own records, keyed `<device id>#lend-<window id>`, so a
//! borrowing computer with no session or link for two sweeps loses them, and
//! [`give_back_for_output`], run as any screen is removed, returns the window
//! first.

// Windows lends through its desktop, so the compositor code below is not run
// there; it is still built, and kept whole, so it is not flagged as unused.
#![cfg_attr(windows, allow(dead_code))]

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use pocketdesk_core::local::computers::LendableWindow;
use pocketdesk_core::protocol::kdeconnect::NetworkPacket;

use crate::hyprland::HyprWindow;
use crate::virtual_screen::{Desk, Output};

// What lending a window is on Windows (phase 5): rules and flow over a fake or
// the real desktop. Unused off Windows but for its tests.
#[cfg_attr(not(windows), allow(dead_code))]
mod desktop;
#[cfg(windows)]
mod win32;

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
    /// The monitor index a session captures its screen by, when the host
    /// said it (a record from before has none).
    #[serde(default)]
    pub index: Option<u32>,
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
            index: None,
        }
    }

    /// Whether a record read back from disk names a window and a screen that
    /// may reach a command: the file is the owner's, but is trusted no more
    /// than an ask is.
    fn is_sound(&self) -> bool {
        is_window_id(&self.window) && is_plain(&self.output)
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
/// borrows a window or sees which there are, and only while the owner's
/// switch is on. Taking one back is never refused.
pub fn refusal(action: &str, enabled: bool, owner_computer: bool) -> Option<&'static str> {
    if !matches!(action, "lend" | "list") {
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

// ---- The list ----

/// The most windows a list names.
const MOST_LISTED: usize = 100;

/// `text` without control characters, at most `most` characters.
fn clean(text: &str, most: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(most)
        .collect::<String>()
        .trim()
        .to_string()
}

/// The windows `device` may borrow, from the compositor's window list and
/// what is lent now: those a screen can be made for, not on a special
/// workspace (Hyprland's scratch ones, Sway's hidden one) and not lent to
/// another computer; one lent to `device` is marked.
pub fn listed(windows: Vec<HyprWindow>, lent: &[Lent], device: &str) -> Vec<LendableWindow> {
    windows
        .into_iter()
        .filter(|w| w.mapped && !w.hidden && is_window_id(&w.address))
        .filter(|w| {
            !w.workspace.name.starts_with("special") && !w.workspace.name.starts_with("__i3")
        })
        .filter(|w| lend_size(w.size).is_some())
        .filter(|w| {
            lent.iter()
                .find(|l| l.window == w.address)
                .is_none_or(|l| l.device == device)
        })
        .take(MOST_LISTED)
        .map(|w| LendableWindow {
            lent: lent
                .iter()
                .any(|l| l.window == w.address && l.device == device),
            title: clean(&w.title, 200),
            class: clean(&w.class, 64),
            width: u32::try_from(w.size[0]).unwrap_or(0),
            height: u32::try_from(w.size[1]).unwrap_or(0),
            workspace: clean(&w.workspace.name, 64),
            address: w.address,
        })
        .collect()
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

/// The commands that size a lent window to the screen `to` after the screen
/// took a new mode, and put it at the screen's corner: Sway counts the layout
/// from the outputs' own places, so the corner is `to`'s. `resize set` before
/// `move absolute position` (a window that shrinks with its screen would be
/// put at a place that is then outside it).
pub fn sway_follow(id: i64, to: &Output, width: u32, height: u32) -> Option<Vec<String>> {
    let (w, h) = (i32::try_from(width).ok()?, i32::try_from(height).ok()?);
    (is_plain(&to.name) && w > 0 && h > 0).then(|| sway_place(id, to.rect.x, to.rect.y, w, h))
}

/// The commands that put a lent window back: onto its workspace (by name,
/// else number, else its output), tiled again if it was, with its border,
/// and, if it floated, its size and place; full screen again if it was. `None`
/// when the record names no window or no place that is safe to send.
pub fn sway_back(lent: &Lent) -> Option<Vec<String>> {
    // A record read from disk is checked like an ask is.
    if !is_window_id(&lent.window) {
        return None;
    }
    let id: i64 = lent.window.parse().ok()?;
    let c = format!("[con_id={id}]");
    let target = if is_plain(&lent.workspace) {
        format!("workspace \"{}\"", lent.workspace)
    } else if lent.workspace_id >= 0 {
        format!("workspace number {}", lent.workspace_id)
    } else if is_plain(&lent.from_output) {
        format!("output \"{}\"", lent.from_output)
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

/// Take a window that fills its screen out of full screen and put it in
/// again, so the compositor sizes it to the screen's new mode (the way a
/// screen's mode that changed under a window that did not follow is put
/// right).
pub fn hypr_refill(address: &str) -> Option<Vec<String>> {
    Some(vec![
        hypr_fullscreen(address, 0, 0)?,
        hypr_fullscreen(address, 2, 0)?,
    ])
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
    /// The windows on this computer's screens now.
    fn windows(&self) -> std::io::Result<Vec<HyprWindow>>;
    /// The screen of the key, made at `width`×`height` or the one it has:
    /// `{name, index, width, height}` or `{error}`.
    fn add_screen(&self, key: &str, width: u32, height: u32) -> Value;
    fn drop_screen(&self, key: &str);
    /// The screen of the key, given `width`×`height` where it is:
    /// `{name, index, width, height}` or `{error}`; none is made.
    fn resize_screen(&self, key: &str, width: u32, height: u32) -> Value;
    fn output(&self, name: &str) -> Option<Output>;
    /// Put the window alone on `to`, `width`×`height`.
    fn send(&self, lent: &Lent, to: &Output, width: u32, height: u32) -> std::io::Result<()>;
    /// The window, alone on `to`, takes the size its screen was just given.
    fn follow(&self, lent: &Lent, to: &Output, width: u32, height: u32) -> std::io::Result<()>;
    /// Put the window back where `lent` says. `Ok` also when the window is
    /// gone; an `InvalidInput` error says the record can never be put back
    /// (no place it names is safe to send), any other that it may yet.
    fn back(&self, lent: &Lent) -> std::io::Result<()>;
    /// Write what is lent to the state file (or remove it when nothing is).
    fn save(&self, lent: &[Lent]) -> std::io::Result<()>;
}

/// The windows lent, and the lock one ask at a time holds.
struct Lends {
    lent: Mutex<Vec<Lent>>,
    one: Mutex<()>,
    /// When each lend was made (not kept across a restart: a record read
    /// back is old).
    at: Mutex<BTreeMap<String, Instant>>,
}

impl Lends {
    const fn new() -> Self {
        Self {
            lent: Mutex::new(Vec::new()),
            one: Mutex::new(()),
            at: Mutex::new(BTreeMap::new()),
        }
    }

    fn all(&self) -> std::sync::MutexGuard<'_, Vec<Lent>> {
        self.lent.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Forget (and save without) the windows `matching` says, with no move:
    /// for a record whose screen is already gone.
    fn forget(&self, env: &dyn Env, matching: impl Fn(&Lent) -> bool) {
        let mut all = self.all();
        let before = all.len();
        all.retain(|l| !matching(l));
        if all.len() != before
            && let Err(e) = env.save(&all)
        {
            tracing::warn!("Could not write the list of lent windows: {e}");
        }
    }

    /// Put back the windows `matching` says. A window's record goes (and the
    /// file follows) only once it is back, or is not there to put back; one
    /// that would not go back keeps its record, so it is not lost and a later
    /// try (a sweep, a second `return`, the next start) can put it back. No
    /// lock is held while the compositor is asked.
    fn put_back(&self, env: &dyn Env, matching: impl Fn(&Lent) -> bool) -> Released {
        let candidates: Vec<Lent> = self.all().iter().filter(|l| matching(l)).cloned().collect();
        let mut released = Released::default();
        for lent in candidates {
            match settle(env, &lent) {
                Ok(()) => {
                    self.forget(env, |l| l.window == lent.window);
                    // Back, whichever way: nothing is left to drag back.
                    crate::shared_input::drag_window::forget(&lent.device, &lent.window);
                    released.done.push(lent);
                }
                Err(e) => released.failed.push((lent, e)),
            }
        }
        released
    }
}

/// What putting windows back came to.
#[derive(Default)]
struct Released {
    /// Back (or gone), their records dropped.
    done: Vec<Lent>,
    /// Still lent, and why.
    failed: Vec<(Lent, std::io::Error)>,
}

/// Put `lent` back; `Ok` when it is, or when it cannot ever be (a record
/// that names no safe place, which a retry would only repeat: the compositor
/// places the window when its screen goes).
fn settle(env: &dyn Env, lent: &Lent) -> std::io::Result<()> {
    match env.back(lent) {
        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {
            tracing::warn!("A lent window cannot be put back exactly: {e}");
            Ok(())
        }
        other => other,
    }
}

fn lends() -> &'static Lends {
    static LENDS: Lends = Lends::new();
    &LENDS
}

fn error(why: impl Into<String>) -> Value {
    json!({ "error": why.into() })
}

/// The size a `lend` asks the screen to be, from its `width` and `height`:
/// the screen's size for them ([`lend_size`]), or `None` when it names none
/// (a borrower before this) or none that is a size, so the window's own is
/// kept.
pub fn asked_size(body: &Value) -> Option<(u32, u32)> {
    let side = |key: &str| {
        body.get(key)
            .and_then(Value::as_u64)
            .and_then(|v| i32::try_from(v).ok())
    };
    lend_size([side("width")?, side("height")?])
}

/// [`lend_sized`] with no size asked: the window's own.
#[cfg(test)]
fn lend_with(lends: &Lends, env: &dyn Env, device: &str, window: &str) -> Value {
    lend_sized(lends, env, device, window, None)
}

/// Lend `window` to `device`: `{name, index, width, height, window}` or
/// `{error}`. With a `size` asked ([`asked_size`]) the screen is that size
/// (a window already lent is given its screen's new size, and follows it).
fn lend_sized(
    lends: &Lends,
    env: &dyn Env,
    device: &str,
    window: &str,
    size: Option<(u32, u32)>,
) -> Value {
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
        // Asked again: the same screen (a session may be showing it), at the
        // new size when the borrower's window has one.
        if let Some(screen) = env.output(&held.output) {
            let now = (
                u32::try_from(screen.rect.width).unwrap_or(0),
                u32::try_from(screen.rect.height).unwrap_or(0),
            );
            if let Some((w, h)) = size.filter(|s| *s != now) {
                return follow_borrower(lends, env, &held, &key, now, w, h);
            }
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
        lends.forget(env, |l| l.window == window);
        env.drop_screen(&key);
    }
    let seen = match env.seen(window) {
        Ok(Some(seen)) => seen,
        Ok(None) => return error("No such window on this computer (it may have closed)"),
        Err(e) => return error(format!("Could not read this computer's windows: {e}")),
    };
    let Some((width, height)) = size.or_else(|| lend_size(seen.size)) else {
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
    let mut lent = Lent::new(device, &seen, &name);
    lent.index = reply["index"].as_u64().and_then(|i| u32::try_from(i).ok());
    {
        // Written before the move: a crash between leaves what returns it.
        // Unwritten, the window is not moved: nothing could return it.
        let mut all = lends.all();
        all.push(lent.clone());
        lends
            .at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(window.to_string(), Instant::now());
        WATCH_NOTE.notify_one();
        if let Err(e) = env.save(&all) {
            all.pop();
            drop(all);
            env.drop_screen(&key);
            return error(format!("Could not record the lent window: {e}"));
        }
    }
    match env.send(&lent, &screen, width, height) {
        Ok(()) => {
            tracing::info!("Lent window {window} to a computer on the screen {name}");
            reply["window"] = window.into();
            reply
        }
        Err(e) => {
            // Back where it was, or, if that fails too, still lent (its
            // record and screen kept) for a sweep to try again.
            let released = lends.put_back(env, |l| l.window == window);
            if released.failed.is_empty() {
                env.drop_screen(&key);
            }
            error(format!("Could not move the window: {e}"))
        }
    }
}

/// The borrower's window changed size: the screen `held` is on is given
/// `width`×`height` where it is and the window is sized to it, so the picture
/// is the borrower's window's, a pixel to a pixel. Answers as `lend` does,
/// with the size the screen has.
///
/// A resize is cosmetic: whatever goes wrong with it (the compositor refuses
/// the mode, the screen is slow to show it, the window does not follow) is
/// logged and the answer is the screen as it is then, never an error, so the
/// borrower's view carries on at the size it had. The screen is read again
/// after the attempt, succeeded or not: a mode that took but was not seen in
/// time, or a screen the compositor moved, is what the answer and the record
/// (the index the lease check and a restart capture it by) say.
fn follow_borrower(
    lends: &Lends,
    env: &dyn Env,
    held: &Lent,
    key: &str,
    old: (u32, u32),
    width: u32,
    height: u32,
) -> Value {
    let tried = env.resize_screen(key, width, height);
    let resized = tried.get("error").is_none();
    if let Some(why) = tried.get("error") {
        tracing::warn!("A lent window's screen was not resized, the view keeps its size: {why}");
    }
    let Some(screen) = env.output(&held.output) else {
        return error("The computer resized the screen but does not show it");
    };
    let (w, h) = (
        u32::try_from(screen.rect.width).unwrap_or(old.0),
        u32::try_from(screen.rect.height).unwrap_or(old.1),
    );
    // The screen as it is now: a resize that went through said so; one that
    // did not (or ran out of wait) is the same screen asked for again, its
    // index read again.
    let mut reply = if resized {
        tried
    } else {
        env.add_screen(key, w, h)
    };
    if reply.get("error").is_some() {
        return reply;
    }
    // The window follows the screen it has (one that kept its size has a
    // window that is already there).
    if (resized || (w, h) != old)
        && let Err(e) = env.follow(held, &screen, w, h)
    {
        tracing::warn!("A lent window did not take its screen's new size: {e}");
    }
    // The screen's place can move with its size (Hyprland puts it right of
    // the others again, even when the wait for the mode ran out): the index a
    // session captures it by follows.
    let index = reply["index"].as_u64().and_then(|i| u32::try_from(i).ok());
    {
        let mut all = lends.all();
        if let Some(record) = all.iter_mut().find(|l| l.window == held.window)
            && record.index != index
        {
            record.index = index;
            if let Err(e) = env.save(&all) {
                tracing::warn!("Could not write the list of lent windows: {e}");
            }
        }
    }
    tracing::info!("A lent window follows its borrower's window to {w}x{h}");
    reply["window"] = held.window.as_str().into();
    reply
}

/// The windows `device` may borrow: `{windows: [..]}` or `{error}`. Read
/// only; the titles in it are not logged.
fn list_with(lends: &Lends, env: &dyn Env, device: &str) -> Value {
    if let Some(why) = env.cannot() {
        return error(why);
    }
    match env.windows() {
        Ok(windows) => {
            let lent = lends.all().clone();
            let windows = listed(windows, &lent, device);
            tracing::debug!("Listed {} windows a computer may borrow", windows.len());
            json!({ "windows": windows })
        }
        Err(e) => error(format!("Could not read this computer's windows: {e}")),
    }
}

/// Take back `window` from `device`, or, with none, all it has: `{returned:
/// n}`, or `{error, returned}` when one would not go back: it stays lent
/// (with its screen) for a sweep or another `return` to try again, and
/// `returned` counts the ones that did.
fn return_with(lends: &Lends, env: &dyn Env, device: &str, window: Option<&str>) -> Value {
    let _one = lends.one.lock().unwrap_or_else(|e| e.into_inner());
    let released = lends.put_back(env, |l| {
        l.device == device && window.is_none_or(|w| l.window == w)
    });
    for lent in &released.done {
        env.drop_screen(&lent.key());
    }
    let returned = released.done.len();
    match released.failed.first() {
        Some((_, e)) => json!({
            "error": format!("Could not put a window back: {e} (it stays lent; try again)"),
            "returned": returned,
        }),
        None => json!({ "returned": returned }),
    }
}

/// Where a lent window goes when it is returned, for a window dragged back
/// across the edge: its place is put where the pointer comes in. Only a
/// floating window has a place of its own to change (a tiled one goes back
/// into its layout); whether the record was changed.
fn retarget_with(
    lends: &Lends,
    env: &dyn Env,
    device: &str,
    window: &str,
    origin: [i32; 2],
) -> bool {
    let _one = lends.one.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = lends.all();
    let Some(lent) = all
        .iter_mut()
        .find(|l| l.device == device && l.window == window && l.floating)
    else {
        return false;
    };
    let (dx, dy) = (origin[0] - lent.origin[0], origin[1] - lent.origin[1]);
    lent.origin = origin;
    lent.at = [lent.at[0] + dx, lent.at[1] + dy];
    if let Err(e) = env.save(&all) {
        tracing::warn!("Could not write the list of lent windows: {e}");
    }
    true
}

/// [`return_with`] for one window, put where `origin` says (its container's
/// top left in the layout) when it floats: a window dragged back over the
/// edge lands where the pointer came in.
fn return_dragged_with(
    lends: &Lends,
    env: &dyn Env,
    device: &str,
    window: &str,
    origin: [i32; 2],
) -> Value {
    retarget_with(lends, env, device, window, origin);
    return_with(lends, env, device, Some(window))
}

/// A window of `device`'s dragged back over the edge: returned, to `origin`
/// if it floats (see [`return_dragged_with`]). Blocking.
pub fn return_dragged(device: &str, window: &str, origin: [i32; 2]) -> Value {
    if !is_window_id(window) {
        return error("not a window");
    }
    return_dragged_with(lends(), here(), device, window, origin)
}

/// Why `device` may not borrow a window now, as a `lend` would refuse it:
/// the shared input's drag asks before it offers a window.
pub fn may_lend(device: &str) -> Result<(), &'static str> {
    match refusal("lend", enabled(), is_owner_computer(device)) {
        Some(why) => Err(why),
        None => Ok(()),
    }
}

/// Put back what is lent on `output`, as it is about to be removed; the
/// windows that would not go (still lent, their records kept).
fn give_back_output_with(lends: &Lends, env: &dyn Env, output: &str) -> Vec<Lent> {
    let released = lends.put_back(env, |l| l.output == output);
    for (_, e) in &released.failed {
        tracing::warn!("Could not put a lent window back: {e}");
    }
    released.failed.into_iter().map(|(lent, _)| lent).collect()
}

/// Called as the screen `output` is removed (a borrowing computer gone, a
/// return, a sweep): its window, if one is lent on it, goes back first.
/// `false` when it would not go back: the screen must then stay, so the
/// window is not lost with it (and its record is put back among the virtual
/// screens', for the next sweep to try again).
///
/// This does not take the lock one ask holds (`Lends::one`): the removal is
/// often run inside such an ask (`lend` and `return` make and drop screens),
/// which would then wait for itself. A record leaves the list only when its
/// window is back, so the few moves that could cross are repeats.
pub fn give_back_for_output(output: &str) -> bool {
    if !lends().all().iter().any(|l| l.output == output) {
        return true;
    }
    let failed = give_back_output_with(lends(), here(), output);
    for lent in &failed {
        crate::virtual_screen::keep_for_lend(&lent.key(), &lent.output);
    }
    failed.is_empty()
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
/// take their screens away; the names of the screens it removed. One that
/// would not go back stays lent (its record loaded, its screen kept) for a
/// sweep to try again. Holds nothing: run inside the virtual screens' own
/// sweep.
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
    let mut kept = Vec::new();
    for lent in left_by(&file, crate::virtual_screen::this_instance().as_deref()) {
        if !lent.is_sound() {
            tracing::warn!("Ignoring a lent-window record that names no window");
            continue;
        }
        tracing::info!("Putting back a window a stopped server left lent");
        match settle(here(), &lent) {
            Ok(()) => {
                crate::virtual_screen::remove(&lent.output);
                removed.push(lent.output);
            }
            Err(e) => {
                tracing::warn!("Could not put a left-over lent window back (will retry): {e}");
                kept.push(lent);
            }
        }
    }
    let mut all = lends().all();
    if !kept.is_empty() {
        WATCH_NOTE.notify_one();
    }
    all.extend(kept);
    if let Err(e) = here().save(&all) {
        tracing::warn!("Could not write the list of lent windows: {e}");
    }
    removed
}

/// Put back everything lent, as the server stops. Waits at most 5 s. A window
/// that would not go back is only logged: its record stays in the state file,
/// and the next server's start puts it back.
pub async fn return_all_on_stop() {
    let work = || {
        let lends = lends();
        let _one = lends.one.lock().unwrap_or_else(|e| e.into_inner());
        let released = lends.put_back(here(), |_| true);
        for lent in &released.done {
            here().drop_screen(&lent.key());
        }
        for (_, e) in &released.failed {
            tracing::warn!("Could not put a lent window back at stop: {e}");
        }
    };
    if tokio::time::timeout(Duration::from_secs(5), tokio::task::spawn_blocking(work))
        .await
        .is_err()
    {
        tracing::warn!("Putting the lent windows back is slow; stopping without waiting");
    }
}

// ---- Watching what is lent ----

/// How often what is lent is looked at, while anything is.
const WATCH_EVERY: Duration = Duration::from_secs(1);

/// Wakes [`run_watcher`] when a window is lent: it sleeps while none is.
static WATCH_NOTE: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Why a lent window is lent no more, without its borrower asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gone {
    /// The window was closed here.
    Closed,
    /// "Let my computers borrow my windows" was turned off.
    SwitchOff,
    /// The borrowing computer has no session, connection or link.
    BorrowerAway,
}

impl Gone {
    /// What the borrower is told, if it can hear: a computer that is away
    /// is not told.
    fn said(self) -> Option<&'static str> {
        match self {
            Self::Closed => Some("closed"),
            Self::SwitchOff => Some("off"),
            Self::BorrowerAway => None,
        }
    }
}

/// A window that is lent no more, and why.
#[derive(Debug, PartialEq, Eq)]
struct Ended {
    device: String,
    window: String,
    why: Gone,
}

/// How long a window just lent is left alone by the look for a borrower that
/// is not there: the lend itself proves it was, and what shows who is there
/// is read a moment before the look.
const NEWLY_LENT: Duration = Duration::from_secs(3);

/// How long a lent window may go with no session capturing its screen
/// before it is taken back: longer than a view's tries to dial again take
/// to get going, so a short gap costs nothing, but a `return` that was lost
/// (the packet dropped on a link that stays up) does not strand the window.
pub const LEASE: Duration = Duration::from_secs(60);

/// Who is there, for a look at what is lent.
pub struct Present<'a> {
    /// The devices with a session, a connection or a link, as read just
    /// before the look.
    pub live: &'a [String],
    /// The monitor each device's sessions capture.
    pub captures: &'a [(String, u32)],
    /// Whether the link to a computer is up now: asked inside the look, for
    /// the windows lent as it runs, not those lent when `live` was read.
    pub linked: &'a dyn Fn(&str) -> bool,
}

/// What the looks at what is lent remember between them.
#[derive(Debug, Default)]
pub struct Looks {
    /// How many looks running found a window gone.
    missing: BTreeMap<String, u32>,
    /// Since when no session has captured a window's screen.
    unheld: BTreeMap<String, Instant>,
}

/// One look at what is lent: the windows to give back, and those gone.
///
/// - With `enabled` false, every lent window goes back.
/// - A lent window whose computer is not there (no session, connection or
///   link; not for [`NEWLY_LENT`]) goes back, and so does one that no
///   session has captured the screen of for [`LEASE`].
/// - A lent window the compositor no longer shows, twice running, has its
///   screen removed: there is nothing to put back.
///
/// A window that will not go back stays lent for the next look. Holds the
/// lock one ask holds, so it never crosses a lend or a return.
fn sweep_with(
    lends: &Lends,
    env: &dyn Env,
    enabled: bool,
    present: &Present,
    looks: &mut Looks,
    now: Instant,
) -> Vec<Ended> {
    let _one = lends.one.lock().unwrap_or_else(|e| e.into_inner());
    let mut ended = Vec::new();
    let mut give_back = |why: Gone, matching: &dyn Fn(&Lent) -> bool| {
        let released = lends.put_back(env, matching);
        for (_, e) in &released.failed {
            tracing::warn!("Could not put a lent window back (will retry): {e}");
        }
        for lent in released.done {
            env.drop_screen(&lent.key());
            ended.push(Ended {
                device: lent.device,
                window: lent.window,
                why,
            });
        }
    };
    if !enabled {
        give_back(Gone::SwitchOff, &|_| true);
        *looks = Looks::default();
        return ended;
    }
    // Who is there is judged on the windows lent now.
    let lent_now: Vec<Lent> = lends.all().clone();
    let at = lends.at.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for l in &lent_now {
        let held = present
            .captures
            .iter()
            .any(|(d, m)| *d == l.device && l.index.is_none_or(|i| i == *m));
        if held {
            looks.unheld.remove(&l.window);
        } else {
            looks.unheld.entry(l.window.clone()).or_insert(now);
        }
    }
    let unheld = looks.unheld.clone();
    give_back(Gone::BorrowerAway, &|l| {
        let settled = at
            .get(&l.window)
            .is_none_or(|lent_at| now.saturating_duration_since(*lent_at) >= NEWLY_LENT);
        let there = present.live.contains(&l.device) || (present.linked)(&l.device);
        let lapsed = unheld
            .get(&l.window)
            .is_some_and(|since| now.saturating_duration_since(*since) >= LEASE);
        (settled && !there) || lapsed
    });
    let lent: Vec<Lent> = lends.all().clone();
    for l in &lent {
        match env.seen(&l.window) {
            Ok(None) => {
                let times = looks.missing.entry(l.window.clone()).or_default();
                *times += 1;
                if *times >= 2 {
                    looks.missing.remove(&l.window);
                    // Nothing to put back: its record goes, then its screen
                    // (the record first, so the screen's removal does not
                    // look for a window to give back).
                    lends.forget(env, |m| m.window == l.window);
                    env.drop_screen(&l.key());
                    ended.push(Ended {
                        device: l.device.clone(),
                        window: l.window.clone(),
                        why: Gone::Closed,
                    });
                }
            }
            // Shown, or the compositor could not be read this time: a look
            // that finds nothing wrong forgets an earlier one that did.
            Ok(Some(_)) | Err(_) => {
                looks.missing.remove(&l.window);
            }
        }
    }
    let kept: Vec<String> = lends.all().iter().map(|l| l.window.clone()).collect();
    looks.missing.retain(|window, _| kept.contains(window));
    looks.unheld.retain(|window, _| kept.contains(window));
    lends
        .at
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|window, _| kept.contains(window));
    ended
}

/// One look at what is lent in this computer's own compositor, as the
/// watcher takes it, with the devices that count as there (`live`, each
/// capturing what it was lent) and whether lending is on given: how many
/// windows ended. For the live test, which drives each rule with no timing
/// to wait out.
pub fn sweep_once(enabled: bool, live: &[String], looks: &mut Looks) -> usize {
    let captures: Vec<(String, u32)> = lends()
        .all()
        .iter()
        .filter(|l| live.contains(&l.device))
        .filter_map(|l| Some((l.device.clone(), l.index?)))
        .collect();
    let present = Present {
        live,
        captures: &captures,
        linked: &|_| false,
    };
    // A window lent a moment ago is judged as one lent a while ago.
    let now = Instant::now() + NEWLY_LENT;
    sweep_with(lends(), here(), enabled, &present, looks, now).len()
}

/// Tell the computer a window was lent to that it is gone, if the way to it
/// is up. The window's title is not in it.
async fn tell(ended: &Ended) {
    tracing::info!(
        "A lent window is back ({:?}); its borrower is told if it can hear",
        ended.why
    );
    let Some(reason) = ended.why.said() else {
        return;
    };
    let packet = NetworkPacket::new(crate::virtual_screen::PACKET)
        .with_body(json!({ "lent_gone": ended.window, "reason": reason }));
    if !crate::remote_devices::send_to_computer(&ended.device, packet).await {
        tracing::debug!("A borrower could not be told its window is gone");
    }
}

/// Tell `device`, whose view shows its window `window`, that the window went
/// home by being dragged back (`{lent_gone, reason: returned}`): the view
/// ends there. Whether it could hear.
pub async fn tell_returned(device: &str, window: &str) -> bool {
    let packet = NetworkPacket::new(crate::virtual_screen::PACKET)
        .with_body(json!({ "lent_gone": window, "reason": "returned" }));
    crate::remote_devices::send_to_computer(device, packet).await
}

/// Watch what is lent, for the server's life: asleep while nothing is lent
/// (a lend wakes it), a look a second while something is ([`sweep_with`]).
pub async fn run_watcher() {
    watch_with(|| async {
        (
            crate::virtual_screen::live_devices().await,
            pocketdesk_core::streaming::sessions::captures(),
        )
    })
    .await;
}

/// [`run_watcher`] with who is there (the devices with a session or a
/// connection, and the monitor each captures) read by `who`: the live test
/// says them itself, as no real session runs there.
pub async fn watch_with<F, Fut>(who: F)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = (Vec<String>, Vec<(String, u32)>)>,
{
    let mut looks = Looks::default();
    loop {
        if lends().all().is_empty() {
            looks = Looks::default();
            WATCH_NOTE.notified().await;
        } else {
            tokio::time::sleep(WATCH_EVERY).await;
        }
        if lends().all().is_empty() {
            continue;
        }
        // A computer counts while it has a session or a connection here
        // (read now; the links this computer keeps are asked inside the
        // look, for the windows lent by then).
        let (live, captures) = who().await;
        let looking = enabled();
        let swept = tokio::task::spawn_blocking(move || {
            let present = Present {
                live: &live,
                captures: &captures,
                linked: &crate::peer_computers::is_linked,
            };
            let ended = sweep_with(
                lends(),
                here(),
                looking,
                &present,
                &mut looks,
                Instant::now(),
            );
            (ended, looks)
        })
        .await;
        match swept {
            Ok((ended, kept)) => {
                looks = kept;
                for gone in &ended {
                    tell(gone).await;
                }
            }
            Err(_) => looks = Looks::default(),
        }
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
pub(crate) fn is_owner_computer(device: &str) -> bool {
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

/// The answer to the `lend`, `list` or `return` request `body` from `device`: see
/// [`lend_with`] and [`return_with`].
pub async fn answer(device: &str, body: &Value) -> Value {
    let action = body["action"].as_str().unwrap_or("").to_string();
    let window = body
        .get("window")
        .and_then(Value::as_str)
        .map(str::to_string);
    let size = asked_size(body);
    let device = device.to_string();
    tokio::task::spawn_blocking(move || {
        if let Some(why) = refusal(&action, enabled(), is_owner_computer(&device)) {
            return error(why);
        }
        match (action.as_str(), window.as_deref()) {
            ("lend", Some(window)) => {
                // A window dragged back before this lend was asked for stays
                // where it is: its offer was withdrawn.
                if crate::shared_input::drag_window::offer_withdrawn(&device, window) {
                    return error("That window was dragged back before it was lent");
                }
                let reply = lend_sized(lends(), here(), &device, window, size);
                if reply.get("error").is_none() {
                    crate::shared_input::drag_window::note_lent(&device, window);
                }
                reply
            }
            ("lend", None) => error("no window named"),
            ("list", _) => list_with(lends(), here(), &device),
            ("return", window) => return_with(lends(), here(), &device, window),
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

/// A record that names no place that is safe to send a window back to: no
/// retry would do better (see [`Env::back`]).
fn no_place() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, "no place to put it back")
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

/// Whether Sway may refuse `command` without the window being the worse for
/// it: `border csd` is refused for a window that never asked to draw its own
/// decorations (the tree still says `csd` of it), and it keeps the border it
/// has then. Found by the live test of dragging a window back, which first
/// left such a window lent for good.
fn may_be_refused(command: &str) -> bool {
    command.ends_with(" border csd")
}

/// Sway commands, one by one.
fn sway_all(commands: impl IntoIterator<Item = String>) -> std::io::Result<()> {
    for command in commands {
        match crate::virtual_screen::run(&command) {
            Err(e) if may_be_refused(&command) => {
                tracing::debug!("Sway did not take `border csd` for a lent window: {e}");
            }
            done => done?,
        }
    }
    Ok(())
}

/// Size the Sway window `id` to the screen `to` (`width`×`height`) and put it
/// at its corner. The size takes a moment after a move or a new mode (sent
/// with the move, it was lost): asked again once.
fn sway_sized(id: i64, to: &Output, width: u32, height: u32) -> std::io::Result<()> {
    let (w, h) = (width as i32, height as i32);
    for _ in 0..2 {
        sway_all(
            sway_follow(id, to, width, height)
                .ok_or_else(|| other("not an output this computer makes"))?,
        )?;
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

/// What lending runs against: this computer's compositor (Sway or Hyprland),
/// or, on Windows, its desktop ([`desktop`]).
#[cfg(windows)]
fn here() -> &'static dyn Env {
    static DESKTOP: std::sync::OnceLock<desktop::WinEnv<win32::Win32>> = std::sync::OnceLock::new();
    DESKTOP.get_or_init(|| desktop::WinEnv::new(win32::Win32))
}

/// What lending runs against: this computer's compositor (Sway or Hyprland),
/// or, on Windows, its desktop ([`desktop`]).
#[cfg(not(windows))]
fn here() -> &'static dyn Env {
    &Live
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

    fn windows(&self) -> std::io::Result<Vec<HyprWindow>> {
        match crate::virtual_screen::desk() {
            Some(Desk::Sway) => Ok(crate::sway_tree::list()?.0),
            Some(Desk::Hyprland) => {
                serde_json::from_str(&crate::virtual_screen::hypr("j/clients")?)
                    .map_err(std::io::Error::other)
            }
            None => Err(other("not running under Sway or Hyprland")),
        }
    }

    fn add_screen(&self, key: &str, width: u32, height: u32) -> Value {
        crate::virtual_screen::add_for_lend(key, width, height)
    }

    fn drop_screen(&self, key: &str) {
        crate::virtual_screen::drop_for_lend(key);
    }

    fn resize_screen(&self, key: &str, width: u32, height: u32) -> Value {
        crate::virtual_screen::resize_for_lend(key, width, height)
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
                sway_sized(id, to, width, height)
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

    fn follow(&self, lent: &Lent, to: &Output, width: u32, height: u32) -> std::io::Result<()> {
        match crate::virtual_screen::desk() {
            Some(Desk::Sway) => {
                let id: i64 = lent
                    .window
                    .parse()
                    .map_err(|_| other("not a Sway window id"))?;
                sway_sized(id, to, width, height)
            }
            Some(Desk::Hyprland) => {
                let a = lent.window.as_str();
                let (w, h) = (width as i32, height as i32);
                let fills = || {
                    wait_for(Duration::from_secs(1), || {
                        seen_now(a)
                            .ok()
                            .flatten()
                            .filter(|s| s.output == to.name && s.size == [w, h])
                    })
                };
                // Full screen on the compositor's side follows the screen's
                // mode by itself; one that did not is put in again.
                if fills().is_some() {
                    return Ok(());
                }
                hypr_all(hypr_refill(a).ok_or_else(|| other("not a Hyprland window address"))?)?;
                fills()
                    .map(|_| ())
                    .ok_or_else(|| other("Hyprland did not fill the resized screen"))
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
            Some(Desk::Sway) => sway_all(sway_back(lent).ok_or_else(no_place)?)?,
            Some(Desk::Hyprland) => {
                let a = lent.window.as_str();
                hypr_all(hypr_fullscreen(a, 0, 0))?;
                let place = hypr_workspace(lent.workspace_id, &lent.workspace)
                    .and_then(|ws| hypr_to_workspace(a, &ws))
                    .ok_or_else(no_place)?;
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

    fn save(&self, lent: &[Lent]) -> std::io::Result<()> {
        let dir = crate::state::state_dir().map_err(|e| other(e.to_string()))?;
        let path = dir.join(STATE_FILE);
        if lent.is_empty() {
            return match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        let file = File {
            instance: crate::virtual_screen::this_instance(),
            lent: lent.to_vec(),
        };
        let json = serde_json::to_string(&file).map_err(std::io::Error::other)?;
        let tmp = pocketdesk_core::atomic_file::temp_beside(&path);
        pocketdesk_core::secure_fs::write_private(&tmp, json.as_bytes())?;
        pocketdesk_core::atomic_file::replace(&tmp, path)
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
        /// What the compositor lists (for `list`).
        listing: Mutex<Vec<HyprWindow>>,
        screens: Mutex<Vec<(String, String, u32, u32)>>,
        calls: Mutex<Vec<String>>,
        saved: Mutex<Vec<Lent>>,
        fail_send: bool,
        fail_follow: bool,
        /// A resize the compositor refuses: the screen keeps its mode.
        fail_resize: bool,
        /// A resize whose mode took but whose wait ran out (an error said),
        /// and the screen's index the compositor then gives it.
        resize_late: bool,
        index: Mutex<Option<u32>>,
        /// Whether the window will not go back (set and cleared as a test
        /// likes).
        fail_back: std::sync::atomic::AtomicBool,
        /// What a failed move back says: `InvalidInput` is for good.
        back_is_for_good: bool,
        fail_save: bool,
        /// The records the compositor was asked to put back, as they were.
        backed: Mutex<Vec<Lent>>,
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
        /// The index the screens answer with: 1 until a test moves it.
        fn index_now(&self) -> u32 {
            self.index.lock().unwrap().unwrap_or(1)
        }
        fn back_fails(&self, fails: bool) {
            self.fail_back
                .store(fails, std::sync::atomic::Ordering::Relaxed);
        }
    }

    impl Env for Fake {
        fn cannot(&self) -> Option<&'static str> {
            None
        }
        fn windows(&self) -> std::io::Result<Vec<HyprWindow>> {
            Ok(self.listing.lock().unwrap().clone())
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
                return json!({ "name": name, "index": self.index_now(), "width": w, "height": h });
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
        fn resize_screen(&self, key: &str, width: u32, height: u32) -> Value {
            self.calls
                .lock()
                .unwrap()
                .push(format!("resize {key} {width}x{height}"));
            if self.fail_resize {
                return error("Could not resize the screen: refused");
            }
            let mut screens = self.screens.lock().unwrap();
            match screens.iter_mut().find(|s| s.0 == key) {
                Some(screen) => {
                    screen.2 = width;
                    screen.3 = height;
                    if self.resize_late {
                        return error("Could not resize the screen: the compositor was slow");
                    }
                    json!({ "name": screen.1, "index": self.index_now(), "width": width, "height": height })
                }
                None => error("no screen to resize"),
            }
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
        fn follow(&self, lent: &Lent, to: &Output, width: u32, height: u32) -> std::io::Result<()> {
            self.calls.lock().unwrap().push(format!(
                "follow {} on {} {width}x{height}",
                lent.window, to.name
            ));
            if self.fail_follow {
                return Err(other("stuck"));
            }
            Ok(())
        }
        fn back(&self, lent: &Lent) -> std::io::Result<()> {
            self.backed.lock().unwrap().push(lent.clone());
            self.calls
                .lock()
                .unwrap()
                .push(format!("back {} to {}", lent.window, lent.workspace));
            if self.fail_back.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(if self.back_is_for_good {
                    no_place()
                } else {
                    other("stuck")
                });
            }
            Ok(())
        }
        fn save(&self, lent: &[Lent]) -> std::io::Result<()> {
            if self.fail_save {
                return Err(other("disk full"));
            }
            *self.saved.lock().unwrap() = lent.to_vec();
            Ok(())
        }
    }

    /// A [`Fake`] whose window `.1` will not go back.
    struct StuckWindow<'a>(&'a Fake, String);

    impl Env for StuckWindow<'_> {
        fn cannot(&self) -> Option<&'static str> {
            self.0.cannot()
        }
        fn seen(&self, window: &str) -> std::io::Result<Option<Seen>> {
            self.0.seen(window)
        }
        fn windows(&self) -> std::io::Result<Vec<HyprWindow>> {
            self.0.windows()
        }
        fn add_screen(&self, key: &str, width: u32, height: u32) -> Value {
            self.0.add_screen(key, width, height)
        }
        fn drop_screen(&self, key: &str) {
            self.0.drop_screen(key)
        }
        fn resize_screen(&self, key: &str, width: u32, height: u32) -> Value {
            self.0.resize_screen(key, width, height)
        }
        fn output(&self, name: &str) -> Option<Output> {
            self.0.output(name)
        }
        fn send(&self, lent: &Lent, to: &Output, width: u32, height: u32) -> std::io::Result<()> {
            self.0.send(lent, to, width, height)
        }
        fn follow(&self, lent: &Lent, to: &Output, width: u32, height: u32) -> std::io::Result<()> {
            self.0.follow(lent, to, width, height)
        }
        fn back(&self, lent: &Lent) -> std::io::Result<()> {
            if lent.window == self.1 {
                return Err(other("stuck"));
            }
            self.0.back(lent)
        }
        fn save(&self, lent: &[Lent]) -> std::io::Result<()> {
            self.0.save(lent)
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
            "[con_id=5] move container to output \"OUT-1\""
        );
        lent.from_output = "a;b".into();
        assert!(sway_back(&lent).is_none());
        let mut lent = lent_tiled();
        lent.from_output = "Virtual 1".into();
        lent.workspace = String::new();
        lent.workspace_id = -1;
        assert_eq!(
            sway_back(&lent).unwrap()[0],
            "[con_id=5] move container to output \"Virtual 1\"",
            "an output name with a space is quoted"
        );
        for window in ["0x5", "+5", "5; exec true", "", "-5"] {
            let mut lent = lent_tiled();
            lent.window = window.into();
            assert!(sway_back(&lent).is_none(), "not a Sway id: {window:?}");
            // A Hyprland address is a window id, only not a Sway one.
            assert_eq!(lent.is_sound(), window == "0x5", "{window:?}");
        }
        let mut lent = lent_tiled();
        lent.output = "X; exec true".into();
        assert!(!lent.is_sound(), "a screen name that is not plain");
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
    fn a_size_asked_is_the_screens_size_for_it_or_none() {
        let ask =
            |w: Value, h: Value| asked_size(&json!({ "action": "lend", "width": w, "height": h }));
        assert_eq!(ask(json!(1280), json!(720)), Some((1280, 720)));
        assert_eq!(ask(json!(1281), json!(721)), Some((1280, 720)), "even");
        assert_eq!(ask(json!(100), json!(100)), Some((320, 320)), "raised");
        assert_eq!(ask(json!(5120), json!(2880)), Some((2560, 1440)), "scaled");
        for odd in [json!(0), json!(-4), json!("wide"), json!(null), json!(1.5)] {
            assert_eq!(ask(odd, json!(720)), None);
        }
        assert_eq!(ask(json!(1280), json!(u64::MAX)), None);
        assert_eq!(
            asked_size(&json!({ "action": "lend", "width": 1280 })),
            None,
            "one side is no size"
        );
        assert_eq!(asked_size(&json!({ "action": "lend" })), None);
    }

    #[test]
    fn a_window_lent_with_a_size_gets_a_screen_and_a_place_of_that_size() {
        let fake = Fake::with(vec![seen("5", 1280, 720)]);
        let reply = lend_sized(&Lends::new(), &fake, DEVICE, "5", Some((1000, 600)));
        assert_eq!(
            (reply["width"].as_u64(), reply["height"].as_u64()),
            (Some(1000), Some(600)),
            "{reply}"
        );
        assert_eq!(fake.calls(), ["send 5 to PD-0a1b2c3d-1 1000x600"]);
    }

    #[test]
    fn a_borrowers_new_size_resizes_the_screen_and_the_window_follows() {
        let fake = Fake::with(vec![seen("5", 1280, 720)]);
        let lends = Lends::new();
        let key = format!("{DEVICE}#lend-5");
        lend_sized(&lends, &fake, DEVICE, "5", None);
        // The same size, or none: no resize, no move.
        for same in [None, Some((1280, 720))] {
            let again = lend_sized(&lends, &fake, DEVICE, "5", same);
            assert_eq!(again["width"], 1280, "{again}");
        }
        assert_eq!(fake.calls().len(), 1, "{:?}", fake.calls());
        // A new one: the screen first, then the window onto its new size.
        let reply = lend_sized(&lends, &fake, DEVICE, "5", Some((1000, 600)));
        assert_eq!(reply["name"], "PD-0a1b2c3d-1");
        assert_eq!(reply["window"], "5");
        assert_eq!(
            (reply["width"].as_u64(), reply["height"].as_u64()),
            (Some(1000), Some(600))
        );
        assert_eq!(
            fake.calls()[1..],
            [
                format!("resize {key} 1000x600"),
                "follow 5 on PD-0a1b2c3d-1 1000x600".to_string()
            ]
        );
        // Still one loan, and what puts it back is as it was.
        let held = lends.all().clone();
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].size, [1280, 720]);
        assert_eq!(held[0].from_output, "OUT-1");
        // The same size again is a no-op.
        lend_sized(&lends, &fake, DEVICE, "5", Some((1000, 600)));
        assert_eq!(fake.calls().len(), 3);
    }

    #[test]
    fn a_window_that_does_not_follow_leaves_the_screen_resized_and_answers() {
        let mut fake = Fake::with(vec![seen("5", 1280, 720)]);
        fake.fail_follow = true;
        let lends = Lends::new();
        lend_sized(&lends, &fake, DEVICE, "5", None);
        let reply = lend_sized(&lends, &fake, DEVICE, "5", Some((1000, 600)));
        assert!(reply.get("error").is_none(), "{reply}");
        assert_eq!(reply["width"], 1000);
        assert_eq!(lends.all().len(), 1, "still lent");
    }

    /// A resize is cosmetic: a screen that cannot be resized answers as it
    /// is (its old size), no error, and the loan, the window and the borrower's
    /// view stand.
    #[test]
    fn a_screen_that_cannot_be_resized_answers_as_it_is_and_the_loan_stands() {
        let mut fake = Fake::with(vec![seen("5", 1280, 720)]);
        fake.fail_resize = true;
        let lends = Lends::new();
        lend_sized(&lends, &fake, DEVICE, "5", None);
        let reply = lend_sized(&lends, &fake, DEVICE, "5", Some((1000, 600)));
        assert!(reply.get("error").is_none(), "{reply}");
        assert_eq!(
            (reply["width"].as_u64(), reply["height"].as_u64()),
            (Some(1280), Some(720)),
            "the size it has"
        );
        assert_eq!(reply["window"], "5");
        assert!(!fake.calls().iter().any(|c| c.starts_with("follow")));
        assert_eq!(lends.all().len(), 1);
        assert_eq!(lends.all()[0].index, Some(1));
        // Asked the same size again: tried again (nothing is remembered), and
        // answered the same.
        let again = lend_sized(&lends, &fake, DEVICE, "5", Some((1000, 600)));
        assert_eq!(again["width"], 1280, "{again}");
    }

    /// Hyprland's rule can apply and the wait for the mode run out: the error
    /// is not the answer, the screen is read again, its index (it may have
    /// moved) and its new size are what the answer and the record say, and the
    /// record that the lease check reads is saved, so the window is not
    /// returned for a screen it is not on.
    #[test]
    fn a_resize_that_took_but_ran_out_of_wait_refreshes_the_index_and_the_record() {
        let mut fake = Fake::with(vec![seen("5", 1280, 720)]);
        fake.resize_late = true;
        let lends = Lends::new();
        lend_sized(&lends, &fake, DEVICE, "5", None);
        assert_eq!(lends.all()[0].index, Some(1));
        *fake.index.lock().unwrap() = Some(3);
        let reply = lend_sized(&lends, &fake, DEVICE, "5", Some((1000, 600)));
        assert!(reply.get("error").is_none(), "{reply}");
        assert_eq!(reply["index"], 3);
        assert_eq!(
            (reply["width"].as_u64(), reply["height"].as_u64()),
            (Some(1000), Some(600)),
            "the mode did take"
        );
        assert_eq!(lends.all()[0].index, Some(3));
        assert_eq!(fake.saved.lock().unwrap()[0].index, Some(3), "saved");
        assert!(
            fake.calls()
                .contains(&"follow 5 on PD-0a1b2c3d-1 1000x600".to_string()),
            "{:?}",
            fake.calls()
        );
    }

    #[test]
    fn another_computer_cannot_resize_a_window_it_did_not_borrow() {
        let fake = Fake::with(vec![seen("5", 1280, 720)]);
        let lends = Lends::new();
        lend_sized(&lends, &fake, DEVICE, "5", None);
        let refused = lend_sized(&lends, &fake, "fedcba9876543210", "5", Some((900, 500)));
        assert!(
            refused["error"]
                .as_str()
                .unwrap()
                .contains("Another computer"),
            "{refused}"
        );
        assert_eq!(fake.calls().len(), 1, "{:?}", fake.calls());
    }

    #[test]
    fn a_follow_that_moves_the_screens_index_keeps_the_record_current() {
        let fake = Fake::with(vec![seen("5", 1280, 720)]);
        let lends = Lends::new();
        lend_sized(&lends, &fake, DEVICE, "5", None);
        assert_eq!(lends.all()[0].index, Some(1));
        *fake.index.lock().unwrap() = Some(3);
        let reply = lend_sized(&lends, &fake, DEVICE, "5", Some((1000, 600)));
        assert_eq!(reply["index"], 3);
        assert_eq!(lends.all()[0].index, Some(3));
        assert_eq!(fake.saved.lock().unwrap()[0].index, Some(3), "saved");
    }

    #[test]
    fn the_sway_and_lua_commands_that_follow_a_resize_are_checked() {
        let to = Output {
            name: "HEADLESS-2".into(),
            rect: Rect {
                x: 1920,
                y: 0,
                width: 1000,
                height: 600,
            },
        };
        assert_eq!(
            sway_follow(7, &to, 1000, 600).unwrap(),
            [
                "[con_id=7] resize set width 1000 px height 600 px",
                "[con_id=7] move absolute position 1920 0"
            ]
        );
        assert!(sway_follow(7, &to, 0, 600).is_none());
        let odd = Output {
            name: "X\"; exec true".into(),
            ..to.clone()
        };
        assert!(sway_follow(7, &odd, 1000, 600).is_none(), "no odd name");
        let refill = hypr_refill("0x1a2b").unwrap();
        assert_eq!(refill.len(), 2);
        assert!(refill[0].contains("fullscreen_state({ internal = 0, client = 0"));
        assert!(refill[1].contains("fullscreen_state({ internal = 2, client = 0"));
        for r in &refill {
            assert!(r.starts_with("eval hl.dispatch("));
            assert!(r.contains("address:0x1a2b"));
        }
        for bad in ["", "0x", "5", "0x1\")os.execute(\"true\")--"] {
            assert!(hypr_refill(bad).is_none(), "{bad}");
        }
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
    fn only_a_csd_border_may_be_refused_as_a_window_goes_back() {
        let mut lent = Lent::new(DEVICE, &seen("5", 800, 600), "OUT-2");
        lent.border = "csd".to_string();
        let commands = sway_back(&lent).unwrap();
        let csd = commands
            .iter()
            .filter(|c| may_be_refused(c))
            .collect::<Vec<_>>();
        assert_eq!(csd, ["[con_id=5] border csd"], "{commands:?}");
        for command in commands.iter().filter(|c| !c.ends_with("border csd")) {
            assert!(!may_be_refused(command), "{command}");
        }
        // Not a size, a place or a move to the workspace.
        assert!(!may_be_refused(
            "[con_id=5] move container to workspace \"3\""
        ));
        assert!(!may_be_refused("[con_id=5] border none"));
    }

    #[test]
    fn a_floating_window_dragged_back_goes_where_the_pointer_came_in() {
        let mut floating = seen("5", 800, 600);
        floating.floating = true;
        floating.origin = [100, 73];
        floating.at = [108, 100];
        let fake = Fake::with(vec![floating, seen("6", 800, 600)]);
        let lends = Lends::new();
        lend_with(&lends, &fake, DEVICE, "5");
        let reply = return_dragged_with(&lends, &fake, DEVICE, "5", [1100, 400]);
        assert_eq!(reply, json!({ "returned": 1 }));
        let backed = fake.backed.lock().unwrap();
        assert_eq!(backed.len(), 1);
        // The container's place is the pointer's; the window's own moves by
        // as much (Sway counts the title bar, Hyprland the window).
        assert_eq!(backed[0].origin, [1100, 400]);
        assert_eq!(backed[0].at, [1108, 427]);
        assert_eq!(backed[0].size, [800, 600], "its size is as it was");
        assert!(lends.all().is_empty());
        assert!(fake.saved.lock().unwrap().is_empty());
    }

    #[test]
    fn a_tiled_window_dragged_back_goes_into_its_layout_as_it_always_does() {
        let fake = Fake::with(vec![seen("5", 800, 600)]);
        let lends = Lends::new();
        lend_with(&lends, &fake, DEVICE, "5");
        assert!(!seen("5", 800, 600).floating);
        let before = lends.all()[0].clone();
        assert!(!retarget_with(&lends, &fake, DEVICE, "5", [1100, 400]));
        assert_eq!(lends.all()[0], before, "no place to change");
        let reply = return_dragged_with(&lends, &fake, DEVICE, "5", [1100, 400]);
        assert_eq!(reply, json!({ "returned": 1 }));
        assert_eq!(fake.backed.lock().unwrap()[0].origin, before.origin);
    }

    #[test]
    fn a_window_of_another_computer_or_not_lent_is_not_moved_by_a_drag_back() {
        let mut floating = seen("5", 800, 600);
        floating.floating = true;
        let fake = Fake::with(vec![floating]);
        let lends = Lends::new();
        lend_with(&lends, &fake, DEVICE, "5");
        let before = lends.all()[0].clone();
        let other = return_dragged_with(&lends, &fake, "fedcba9876543210", "5", [1, 2]);
        assert_eq!(other["returned"], 0);
        assert_eq!(
            lends.all()[0],
            before,
            "another computer's drag changed nothing"
        );
        let unlent = return_dragged_with(&lends, &fake, DEVICE, "9", [1, 2]);
        assert_eq!(unlent["returned"], 0);
        assert_eq!(lends.all().len(), 1);
        assert_eq!(
            return_dragged("x", "5; exec true", [0, 0]),
            json!({"error": "not a window"})
        );
    }

    #[test]
    fn a_window_that_will_not_go_back_keeps_its_record_and_screen_and_says_so() {
        let fake = Fake::with(vec![seen("5", 1280, 720)]);
        let lends = Lends::new();
        lend_with(&lends, &fake, DEVICE, "5");
        fake.back_fails(true);
        let reply = return_with(&lends, &fake, DEVICE, Some("5"));
        assert!(
            reply["error"]
                .as_str()
                .unwrap()
                .contains("Could not put a window back")
        );
        assert_eq!(reply["returned"], 0);
        assert_eq!(fake.screens.lock().unwrap().len(), 1, "its screen stays");
        assert_eq!(lends.all().len(), 1, "its record stays");
        assert_eq!(fake.saved.lock().unwrap().len(), 1, "and the file");
        assert!(
            !fake.calls().iter().any(|c| c.starts_with("drop")),
            "{:?}",
            fake.calls()
        );
        // Asked again once the compositor can: it goes.
        fake.back_fails(false);
        let again = return_with(&lends, &fake, DEVICE, Some("5"));
        assert_eq!(again, json!({ "returned": 1 }));
        assert!(lends.all().is_empty());
        assert!(fake.saved.lock().unwrap().is_empty());
        assert!(fake.screens.lock().unwrap().is_empty());
    }

    #[test]
    fn a_return_of_all_counts_those_that_went_and_keeps_the_rest() {
        let fake = Fake::with(vec![seen("5", 1280, 720), seen("6", 800, 600)]);
        let lends = Lends::new();
        lend_with(&lends, &fake, DEVICE, "5");
        lend_with(&lends, &fake, DEVICE, "6");
        // Only 5 will not go back.
        let one_stuck = StuckWindow(&fake, "5".to_string());
        let reply = return_with(&lends, &one_stuck, DEVICE, None);
        assert_eq!(reply["returned"], 1, "{reply}");
        assert!(reply["error"].is_string());
        assert_eq!(lends.all().len(), 1);
        assert_eq!(lends.all()[0].window, "5");
    }

    #[test]
    fn a_later_sweep_puts_a_window_back_that_would_not_go_before() {
        let fake = Fake::with(vec![seen("5", 1280, 720), seen("6", 800, 600)]);
        let lends = Lends::new();
        let name = lend_with(&lends, &fake, DEVICE, "5")["name"]
            .as_str()
            .unwrap()
            .to_string();
        lend_with(&lends, &fake, DEVICE, "6");
        fake.back_fails(true);
        let failed = give_back_output_with(&lends, &fake, &name);
        assert_eq!(failed.len(), 1, "told: the screen must stay");
        assert_eq!(failed[0].window, "5");
        assert_eq!(lends.all().len(), 2, "both still lent");
        assert_eq!(fake.saved.lock().unwrap().len(), 2);
        // The next sweep.
        fake.back_fails(false);
        assert!(give_back_output_with(&lends, &fake, &name).is_empty());
        assert_eq!(lends.all().len(), 1, "the other stays lent");
        assert_eq!(lends.all()[0].window, "6");
        assert_eq!(fake.saved.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_record_that_can_never_be_put_back_is_let_go_not_retried() {
        let mut fake = Fake::with(vec![seen("5", 1280, 720)]);
        fake.back_is_for_good = true;
        let lends = Lends::new();
        let name = lend_with(&lends, &fake, DEVICE, "5")["name"]
            .as_str()
            .unwrap()
            .to_string();
        fake.back_fails(true);
        assert!(give_back_output_with(&lends, &fake, &name).is_empty());
        assert!(lends.all().is_empty());
        assert!(fake.saved.lock().unwrap().is_empty());
    }

    #[test]
    fn a_move_that_fails_and_cannot_be_undone_leaves_the_window_lent() {
        let mut fake = Fake::with(vec![seen("5", 1280, 720)]);
        fake.fail_send = true;
        let lends = Lends::new();
        // The undo fails too.
        fake.back_fails(true);
        let reply = lend_with(&lends, &fake, DEVICE, "5");
        assert!(
            reply["error"].as_str().unwrap().contains("Could not move"),
            "{reply}"
        );
        assert_eq!(lends.all().len(), 1, "kept for a sweep");
        assert_eq!(fake.screens.lock().unwrap().len(), 1, "with its screen");
        fake.back_fails(false);
        let name = lends.all()[0].output.clone();
        assert!(give_back_output_with(&lends, &fake, &name).is_empty());
        assert!(lends.all().is_empty());
    }

    #[test]
    fn a_window_is_not_moved_when_what_returns_it_cannot_be_written() {
        let mut fake = Fake::with(vec![seen("5", 1280, 720)]);
        fake.fail_save = true;
        let lends = Lends::new();
        let reply = lend_with(&lends, &fake, DEVICE, "5");
        assert!(
            reply["error"]
                .as_str()
                .unwrap()
                .contains("Could not record"),
            "{reply}"
        );
        assert!(lends.all().is_empty());
        assert!(fake.screens.lock().unwrap().is_empty(), "its screen goes");
        assert!(
            !fake.calls().iter().any(|c| c.starts_with("send")),
            "never moved: {:?}",
            fake.calls()
        );
    }

    #[test]
    fn a_screen_removed_gives_its_window_back_first() {
        let fake = Fake::with(vec![seen("5", 1280, 720), seen("6", 800, 600)]);
        let lends = Lends::new();
        let reply = lend_with(&lends, &fake, DEVICE, "5");
        lend_with(&lends, &fake, DEVICE, "6");
        let name = reply["name"].as_str().unwrap();
        assert!(give_back_output_with(&lends, &fake, name).is_empty());
        assert!(fake.calls().contains(&"back 5 to 3".to_string()));
        assert!(!fake.calls().iter().any(|c| c.starts_with("back 6")));
        assert_eq!(lends.all().len(), 1, "the other stays lent");
        assert_eq!(lends.all()[0].window, "6");
    }

    fn client(address: &str, title: &str, size: [i32; 2], workspace: &str) -> HyprWindow {
        HyprWindow {
            address: address.into(),
            title: title.into(),
            class: "app.example".into(),
            pid: 1,
            mapped: true,
            hidden: false,
            at: [0, 0],
            size,
            monitor: 0,
            fullscreen: 0,
            workspace: crate::hyprland::HyprWorkspaceRef {
                id: 1,
                name: workspace.into(),
            },
        }
    }

    #[test]
    fn list_is_gated_like_lend_and_returns_are_not() {
        assert!(refusal("list", true, true).is_none());
        assert!(
            refusal("list", true, false)
                .unwrap()
                .contains("paired both ways")
        );
        assert!(
            refusal("list", false, true)
                .unwrap()
                .contains("Let my computers borrow my windows")
        );
        assert!(refusal("list", false, false).is_some());
    }

    #[test]
    fn the_list_names_the_windows_a_screen_can_be_made_for() {
        let mut hidden = client("0x2", "Hidden", [800, 600], "1");
        hidden.hidden = true;
        let mut unmapped = client("0x3", "Unmapped", [800, 600], "1");
        unmapped.mapped = false;
        let windows = vec![
            client("0x1", "Notes\u{7}\n", [1280, 720], "main"),
            hidden,
            unmapped,
            client("0x4", "Scratch", [800, 600], "special:scratch"),
            client("7", "Sway window", [640, 480], "2"),
            client("5; exec true", "Odd id", [640, 480], "2"),
            client("0x6", "No size", [0, 480], "2"),
        ];
        let listed = listed(windows, &[], DEVICE);
        let addresses: Vec<&str> = listed.iter().map(|w| w.address.as_str()).collect();
        assert_eq!(addresses, ["0x1", "7"]);
        assert_eq!(listed[0].title, "Notes", "control characters go");
        assert_eq!(
            (
                listed[0].width,
                listed[0].height,
                listed[0].workspace.as_str()
            ),
            (1280, 720, "main")
        );
        assert_eq!(listed[0].class, "app.example");
        assert!(!listed[0].lent);
    }

    #[test]
    fn a_window_lent_to_another_computer_is_not_listed_and_ones_lent_to_the_asker_are_marked() {
        let windows = vec![
            client("5", "Mine", [800, 600], "1"),
            client("6", "Theirs", [800, 600], "1"),
            client("7", "Free", [800, 600], "1"),
        ];
        let lent = vec![
            Lent::new(DEVICE, &seen("5", 800, 600), "PD-a-1"),
            Lent::new("fedcba9876543210", &seen("6", 800, 600), "PD-a-2"),
        ];
        let listed = listed(windows, &lent, DEVICE);
        let seen: Vec<(&str, bool)> = listed
            .iter()
            .map(|w| (w.address.as_str(), w.lent))
            .collect();
        assert_eq!(seen, [("5", true), ("7", false)]);
    }

    #[test]
    fn a_long_title_and_a_long_list_are_cut() {
        let long = "x".repeat(500);
        let many: Vec<HyprWindow> = (1..=150)
            .map(|n| client(&n.to_string(), &long, [800, 600], "1"))
            .collect();
        let listed = listed(many, &[], DEVICE);
        assert_eq!(listed.len(), MOST_LISTED);
        assert_eq!(listed[0].title.chars().count(), 200);
    }

    #[test]
    fn hyprlands_client_list_reads_into_the_windows_a_computer_may_borrow() {
        // As `j/clients` gives them, with fields this does not read.
        let clients = r#"[
          {"address":"0x560a772079f0","mapped":true,"hidden":false,"at":[10,40],"size":[1280,720],
           "workspace":{"id":2,"name":"2"},"floating":false,"monitor":0,"class":"foot","title":"Terminal",
           "initialClass":"foot","pid":4242,"xwayland":false,"pinned":false,"fullscreen":0,"fullscreenClient":0},
          {"address":"0x560a77207a10","mapped":true,"hidden":false,"at":[0,0],"size":[800,600],
           "workspace":{"id":-99,"name":"special:magic"},"monitor":0,"class":"kitty","title":"Scratch","pid":1},
          {"address":"0x560a77207a20","mapped":false,"hidden":false,"at":[0,0],"size":[800,600],
           "workspace":{"id":1,"name":"1"},"monitor":0,"class":"x","title":"Not shown yet","pid":2}
        ]"#;
        let parsed: Vec<HyprWindow> = serde_json::from_str(clients).unwrap();
        let listed = listed(parsed, &[], DEVICE);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].address, "0x560a772079f0");
        assert_eq!(listed[0].title, "Terminal");
        assert_eq!(listed[0].class, "foot");
        assert_eq!((listed[0].width, listed[0].height), (1280, 720));
    }

    #[test]
    fn list_answers_the_windows_and_changes_nothing() {
        let fake = Fake::with(vec![seen("5", 1280, 720)]);
        *fake.listing.lock().unwrap() = vec![client("5", "Notes", [1280, 720], "1")];
        let lends = Lends::new();
        let reply = list_with(&lends, &fake, DEVICE);
        assert_eq!(reply["windows"][0]["address"], "5");
        assert_eq!(reply["windows"][0]["title"], "Notes");
        assert_eq!(reply["windows"][0]["width"], 1280);
        assert!(fake.calls().is_empty(), "nothing moved or made");
        assert!(lends.all().is_empty());
    }

    const OTHER: &str = "fedcba9876543210";

    /// A look as the watcher takes it, a good while after the lends, with
    /// every device in `live` capturing the window lent to it.
    fn look(
        lends: &Lends,
        env: &dyn Env,
        enabled: bool,
        live: &[String],
        looks: &mut Looks,
    ) -> Vec<Ended> {
        look_at(
            lends,
            env,
            enabled,
            live,
            looks,
            Instant::now() + Duration::from_secs(10),
        )
    }

    fn look_at(
        lends: &Lends,
        env: &dyn Env,
        enabled: bool,
        live: &[String],
        looks: &mut Looks,
        now: Instant,
    ) -> Vec<Ended> {
        let captures: Vec<(String, u32)> = lends
            .all()
            .iter()
            .filter(|l| live.contains(&l.device))
            .filter_map(|l| Some((l.device.clone(), l.index?)))
            .collect();
        let present = Present {
            live,
            captures: &captures,
            linked: &|_| false,
        };
        sweep_with(lends, env, enabled, &present, looks, now)
    }

    /// A fake computer with windows 5 and 6 lent to `DEVICE` and 7 to `OTHER`.
    fn lent_three() -> (Fake, Lends) {
        let fake = Fake::with(vec![
            seen("5", 1280, 720),
            seen("6", 800, 600),
            seen("7", 640, 480),
        ]);
        let lends = Lends::new();
        for (device, window) in [(DEVICE, "5"), (DEVICE, "6"), (OTHER, "7")] {
            let reply = lend_with(&lends, &fake, device, window);
            assert!(reply.get("error").is_none(), "{reply}");
        }
        (fake, lends)
    }

    fn windows_of(ended: &[Ended], why: Gone) -> Vec<(&str, &str)> {
        let mut found: Vec<(&str, &str)> = ended
            .iter()
            .filter(|e| e.why == why)
            .map(|e| (e.device.as_str(), e.window.as_str()))
            .collect();
        found.sort();
        found
    }

    #[test]
    fn turning_the_switch_off_gives_every_lent_window_back() {
        let (fake, lends) = lent_three();
        let both = vec![DEVICE.to_string(), OTHER.to_string()];
        let mut looks = Looks::default();
        // On, with both computers there: nothing changes.
        assert!(look(&lends, &fake, true, &both, &mut looks).is_empty());
        assert_eq!(lends.all().len(), 3);
        // Off: all three go back, their screens go, and their borrowers are named.
        let ended = look(&lends, &fake, false, &both, &mut looks);
        assert_eq!(
            windows_of(&ended, Gone::SwitchOff),
            [(DEVICE, "5"), (DEVICE, "6"), (OTHER, "7")]
        );
        assert!(lends.all().is_empty());
        assert!(fake.screens.lock().unwrap().is_empty());
        let calls = fake.calls();
        for window in ["5", "6", "7"] {
            assert!(calls.contains(&format!("back {window} to 3")), "{calls:?}");
        }
        assert!(fake.saved.lock().unwrap().is_empty(), "the file follows");
        // Off again with none lent: nothing.
        assert!(look(&lends, &fake, false, &both, &mut looks).is_empty());
    }

    #[test]
    fn a_window_that_will_not_go_back_when_the_switch_goes_off_is_tried_again() {
        let (fake, lends) = lent_three();
        let live = vec![DEVICE.to_string(), OTHER.to_string()];
        let mut looks = Looks::default();
        fake.back_fails(true);
        assert!(look(&lends, &fake, false, &live, &mut looks).is_empty());
        assert_eq!(lends.all().len(), 3, "still lent, so not lost");
        assert_eq!(fake.screens.lock().unwrap().len(), 3, "with their screens");
        fake.back_fails(false);
        let ended = look(&lends, &fake, false, &live, &mut looks);
        assert_eq!(ended.len(), 3);
        assert!(lends.all().is_empty());
    }

    #[test]
    fn a_borrower_with_no_session_or_link_gets_its_windows_back_at_once() {
        let (fake, lends) = lent_three();
        let mut looks = Looks::default();
        // Only the other computer is there: DEVICE's two windows go, 7 stays.
        let live = vec![OTHER.to_string()];
        let ended = look(&lends, &fake, true, &live, &mut looks);
        assert_eq!(
            windows_of(&ended, Gone::BorrowerAway),
            [(DEVICE, "5"), (DEVICE, "6")]
        );
        assert_eq!(lends.all().len(), 1);
        assert_eq!(lends.all()[0].window, "7");
        let calls = fake.calls();
        assert!(calls.contains(&"back 5 to 3".to_string()));
        assert!(!calls.iter().any(|c| c.starts_with("back 7")), "{calls:?}");
        assert_eq!(fake.screens.lock().unwrap().len(), 1, "only 7's screen");
        // The one still there is not told, and a computer away cannot be.
        assert_eq!(Gone::BorrowerAway.said(), None);
        assert_eq!(Gone::Closed.said(), Some("closed"));
        assert_eq!(Gone::SwitchOff.said(), Some("off"));
    }

    fn look_with(
        lends: &Lends,
        fake: &Fake,
        live: &[String],
        captures: &[(String, u32)],
        linked: &dyn Fn(&str) -> bool,
        looks: &mut Looks,
        now: Instant,
    ) -> Vec<Ended> {
        let present = Present {
            live,
            captures,
            linked,
        };
        sweep_with(lends, fake, true, &present, looks, now)
    }

    #[test]
    fn a_window_no_session_captures_goes_back_after_the_lease_whatever_the_link_says() {
        let (fake, lends) = lent_three();
        let t0 = Instant::now();
        let both = vec![DEVICE.to_string(), OTHER.to_string()];
        // Both computers are linked the whole time, but only DEVICE has a
        // session capturing its windows' screen (monitor 1 in the fake).
        let captures = vec![(DEVICE.to_string(), 1)];
        let mut looks = Looks::default();
        let at = |secs: u64| t0 + Duration::from_secs(secs);
        for secs in [10, 40, 69] {
            let ended = look_with(
                &lends,
                &fake,
                &both,
                &captures,
                &|_| true,
                &mut looks,
                at(secs),
            );
            assert!(ended.is_empty(), "within the lease, at {secs} s");
        }
        // 60 s after the first look that found 7's screen not captured.
        let ended = look_with(
            &lends,
            &fake,
            &both,
            &captures,
            &|_| true,
            &mut looks,
            at(71),
        );
        assert_eq!(windows_of(&ended, Gone::BorrowerAway), [(OTHER, "7")]);
        let lent: Vec<String> = lends.all().iter().map(|l| l.window.clone()).collect();
        assert_eq!(lent, ["5", "6"], "the captured ones stay lent");
        assert!(fake.calls().contains(&"back 7 to 3".to_string()));
    }

    #[test]
    fn a_session_capturing_another_screen_does_not_hold_a_window() {
        let (fake, lends) = lent_three();
        let t0 = Instant::now();
        let both = vec![DEVICE.to_string(), OTHER.to_string()];
        let mut looks = Looks::default();
        // DEVICE views monitor 2 (the host's own screen, say), not the 1 its
        // windows are on; OTHER's session does capture its window's.
        let captures = vec![(DEVICE.to_string(), 2), (OTHER.to_string(), 1)];
        let at = |secs: u64| t0 + Duration::from_secs(secs);
        let first = look_with(
            &lends,
            &fake,
            &both,
            &captures,
            &|_| true,
            &mut looks,
            at(10),
        );
        assert!(first.is_empty());
        let later = look_with(
            &lends,
            &fake,
            &both,
            &captures,
            &|_| true,
            &mut looks,
            at(80),
        );
        assert_eq!(
            windows_of(&later, Gone::BorrowerAway),
            [(DEVICE, "5"), (DEVICE, "6")]
        );
        assert_eq!(lends.all().len(), 1);
    }

    #[test]
    fn a_capture_that_resumes_within_the_lease_starts_it_afresh() {
        let (fake, lends) = lent_three();
        let t0 = Instant::now();
        let live = vec![DEVICE.to_string(), OTHER.to_string()];
        let all = vec![(DEVICE.to_string(), 1), (OTHER.to_string(), 1)];
        let none: Vec<(String, u32)> = Vec::new();
        let mut looks = Looks::default();
        let at = |secs: u64| t0 + Duration::from_secs(secs);
        assert!(look_with(&lends, &fake, &live, &none, &|_| true, &mut looks, at(10)).is_empty());
        // The view dialled again at 50 s: held, the clock is forgotten.
        assert!(look_with(&lends, &fake, &live, &all, &|_| true, &mut looks, at(50)).is_empty());
        // Gone again at 60 s: the lease runs from there, not from 10 s.
        assert!(look_with(&lends, &fake, &live, &none, &|_| true, &mut looks, at(60)).is_empty());
        assert!(look_with(&lends, &fake, &live, &none, &|_| true, &mut looks, at(100)).is_empty());
        let ended = look_with(&lends, &fake, &live, &none, &|_| true, &mut looks, at(121));
        assert_eq!(ended.len(), 3);
    }

    #[test]
    fn a_window_just_lent_is_not_taken_for_an_absent_borrowers_and_a_link_up_now_counts() {
        let (fake, lends) = lent_three();
        let all = vec![(DEVICE.to_string(), 1), (OTHER.to_string(), 1)];
        let mut looks = Looks::default();
        // `live` was read before these were lent, so no one is in it: a look
        // at once leaves them alone.
        let now = Instant::now();
        assert!(look_with(&lends, &fake, &[], &all, &|_| false, &mut looks, now).is_empty());
        // A look a few seconds on, with the link to DEVICE up now (asked
        // inside the look, not read before it), takes only OTHER's.
        let later = now + Duration::from_secs(4);
        let ended = look_with(
            &lends,
            &fake,
            &[],
            &all,
            &|d| d == DEVICE,
            &mut looks,
            later,
        );
        assert_eq!(windows_of(&ended, Gone::BorrowerAway), [(OTHER, "7")]);
    }

    #[test]
    fn a_closed_window_loses_its_empty_screen_and_its_borrower_is_named() {
        let (fake, lends) = lent_three();
        let live = vec![DEVICE.to_string(), OTHER.to_string()];
        let mut looks = Looks::default();
        // Window 5 closes.
        fake.windows.lock().unwrap().retain(|w| w.id != "5");
        // The first look only notes it (a window mid-move is not gone).
        assert!(look(&lends, &fake, true, &live, &mut looks).is_empty());
        assert_eq!(lends.all().len(), 3);
        // Seen again: forgotten.
        fake.windows.lock().unwrap().push(seen("5", 1280, 720));
        assert!(look(&lends, &fake, true, &live, &mut looks).is_empty());
        assert!(looks.missing.is_empty());
        // Closed for good: the second look in a row.
        fake.windows.lock().unwrap().retain(|w| w.id != "5");
        assert!(look(&lends, &fake, true, &live, &mut looks).is_empty());
        let ended = look(&lends, &fake, true, &live, &mut looks);
        assert_eq!(windows_of(&ended, Gone::Closed), [(DEVICE, "5")]);
        let lent: Vec<String> = lends.all().iter().map(|l| l.window.clone()).collect();
        assert_eq!(lent, ["6", "7"]);
        assert_eq!(fake.screens.lock().unwrap().len(), 2, "its screen is gone");
        let calls = fake.calls();
        assert!(
            !calls.iter().any(|c| c.starts_with("back 5")),
            "nothing to put back: {calls:?}"
        );
        assert!(
            calls
                .iter()
                .any(|c| c.starts_with("drop ") && c.ends_with("#lend-5"))
        );
        let saved: Vec<String> = fake
            .saved
            .lock()
            .unwrap()
            .iter()
            .map(|l| l.window.clone())
            .collect();
        assert_eq!(saved, ["6", "7"], "the file follows");
    }

    #[test]
    fn a_compositor_that_cannot_be_read_closes_nothing() {
        struct Blind<'a>(&'a Fake);
        impl Env for Blind<'_> {
            fn cannot(&self) -> Option<&'static str> {
                self.0.cannot()
            }
            fn seen(&self, _: &str) -> std::io::Result<Option<Seen>> {
                Err(other("the compositor's socket is gone"))
            }
            fn windows(&self) -> std::io::Result<Vec<HyprWindow>> {
                self.0.windows()
            }
            fn add_screen(&self, key: &str, width: u32, height: u32) -> Value {
                self.0.add_screen(key, width, height)
            }
            fn drop_screen(&self, key: &str) {
                self.0.drop_screen(key)
            }
            fn resize_screen(&self, key: &str, width: u32, height: u32) -> Value {
                self.0.resize_screen(key, width, height)
            }
            fn output(&self, name: &str) -> Option<Output> {
                self.0.output(name)
            }
            fn send(&self, lent: &Lent, to: &Output, w: u32, h: u32) -> std::io::Result<()> {
                self.0.send(lent, to, w, h)
            }
            fn follow(&self, lent: &Lent, to: &Output, w: u32, h: u32) -> std::io::Result<()> {
                self.0.follow(lent, to, w, h)
            }
            fn back(&self, lent: &Lent) -> std::io::Result<()> {
                self.0.back(lent)
            }
            fn save(&self, lent: &[Lent]) -> std::io::Result<()> {
                self.0.save(lent)
            }
        }
        let (fake, lends) = lent_three();
        let live = vec![DEVICE.to_string(), OTHER.to_string()];
        let mut looks = Looks::default();
        for _ in 0..4 {
            assert!(look(&lends, &Blind(&fake), true, &live, &mut looks).is_empty());
        }
        assert_eq!(lends.all().len(), 3);
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
