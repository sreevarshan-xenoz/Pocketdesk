//! The windows of a Sway desktop, read from i3-IPC's `get_tree`, in the
//! shape Hyprland's `j/clients` gives ([`crate::hyprland::HyprWindow`]), so
//! the window list a phone or a computer asks for
//! ([`crate::plugins::windows`]) and the lending of a window
//! (`crate::lend_window`) read both compositors alike.
//!
//! A window's id is Sway's container id (`con_id`), in decimal; its geometry
//! is the window's own, without the title bar and border Sway draws round it.

use serde::Deserialize;
use serde_json::Value;

use crate::hyprland::{HyprMonitor, HyprWindow, HyprWorkspaceRef};
use crate::virtual_screen::{Output, Rect};

/// i3-IPC `GET_TREE`.
const GET_TREE: u32 = 4;

#[derive(Debug, Default, Deserialize)]
struct Props {
    #[serde(default)]
    class: Option<String>,
}

/// A node of the tree, with what is read of it.
#[derive(Debug, Default, Deserialize)]
struct Node {
    #[serde(default)]
    id: i64,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    num: Option<i32>,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    shell: Option<String>,
    #[serde(default)]
    window: Option<u64>,
    #[serde(default)]
    window_properties: Option<Props>,
    #[serde(default)]
    focused: bool,
    #[serde(default)]
    rect: Rect,
    #[serde(default)]
    window_rect: Rect,
    #[serde(default)]
    deco_rect: Rect,
    #[serde(default)]
    border: Option<String>,
    #[serde(default)]
    current_border_width: i32,
    #[serde(default)]
    fullscreen_mode: i32,
    #[serde(default)]
    nodes: Vec<Node>,
    #[serde(default)]
    floating_nodes: Vec<Node>,
}

/// A window on a Sway desktop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwayWindow {
    /// The container id.
    pub id: i64,
    /// The program's `app_id`, or its X11 class.
    pub class: String,
    pub title: String,
    pub pid: u32,
    pub focused: bool,
    pub floating: bool,
    /// The window's own top left, in the layout's coordinates.
    pub at: [i32; 2],
    /// The window's own size.
    pub size: [i32; 2],
    /// The top left of its container (its title bar and border included),
    /// which is what `move absolute position` places, and the container's
    /// size, which is what `resize set` sets (the title bar counts in both).
    pub origin: [i32; 2],
    pub frame: [i32; 2],
    pub output: String,
    pub workspace: String,
    pub workspace_num: i32,
    /// `normal`, `pixel`, `none` or `csd`, and its width.
    pub border: String,
    pub border_width: i32,
    /// 0 none, 1 its workspace, 2 every output.
    pub fullscreen: i32,
}

/// Whether a leaf of the tree is a program's window, not an empty container.
fn is_window(node: &Node) -> bool {
    matches!(node.kind.as_str(), "con" | "floating_con")
        && (node.app_id.is_some()
            || node.window.is_some()
            || node.shell.is_some()
            || node.pid.is_some_and(|p| p > 0))
}

/// Every window under `node`, on the workspace `workspace` of `output`.
fn collect(all: &mut Vec<SwayWindow>, node: &Node, output: &str, workspace: &Node) {
    for child in node.nodes.iter().chain(&node.floating_nodes) {
        if !(child.nodes.is_empty() && child.floating_nodes.is_empty()) {
            collect(all, child, output, workspace);
        } else if is_window(child) {
            let class = child
                .app_id
                .clone()
                .filter(|a| !a.is_empty())
                .or_else(|| {
                    child
                        .window_properties
                        .as_ref()
                        .and_then(|p| p.class.clone())
                })
                .unwrap_or_default();
            all.push(SwayWindow {
                id: child.id,
                class,
                title: child.name.clone().unwrap_or_default(),
                pid: child.pid.unwrap_or(0),
                focused: child.focused,
                floating: child.kind == "floating_con",
                at: [
                    child.rect.x + child.window_rect.x,
                    child.rect.y + child.window_rect.y,
                ],
                size: [child.window_rect.width, child.window_rect.height],
                // Sway's `rect` is the content and border, under the title
                // bar (`deco_rect`).
                origin: [child.rect.x, child.rect.y - child.deco_rect.height],
                frame: [child.rect.width, child.rect.height + child.deco_rect.height],
                output: output.to_string(),
                workspace: workspace.name.clone().unwrap_or_default(),
                workspace_num: workspace.num.unwrap_or(-1),
                border: child.border.clone().unwrap_or_else(|| "normal".into()),
                border_width: child.current_border_width,
                fullscreen: child.fullscreen_mode,
            });
        }
    }
}

/// The windows in a `get_tree` reply, those on a workspace of a real or
/// virtual output (the scratchpad's are hidden, not on any screen).
pub fn windows(tree: &Value) -> Vec<SwayWindow> {
    let Ok(root) = serde_json::from_value::<Node>(tree.clone()) else {
        return Vec::new();
    };
    let mut all = Vec::new();
    for output in root
        .nodes
        .iter()
        .filter(|n| n.kind == "output" && n.name.as_deref().is_some_and(|name| name != "__i3"))
    {
        let name = output.name.clone().unwrap_or_default();
        for workspace in output.nodes.iter().filter(|n| n.kind == "workspace") {
            collect(&mut all, workspace, &name, workspace);
        }
    }
    all
}

/// The window with container id `id` in a `get_tree` reply.
pub fn find(tree: &Value, id: i64) -> Option<SwayWindow> {
    windows(tree).into_iter().find(|w| w.id == id)
}

/// `window` as Hyprland's `j/clients` would show it: its container id as
/// the address, `monitor` the index of its output in the monitor list.
pub fn as_hypr(window: &SwayWindow, monitor: i32) -> HyprWindow {
    HyprWindow {
        address: window.id.to_string(),
        title: window.title.clone(),
        class: window.class.clone(),
        pid: window.pid,
        mapped: true,
        hidden: false,
        at: window.at,
        size: window.size,
        monitor,
        // Hyprland's own: 2 is full screen.
        fullscreen: if window.fullscreen != 0 { 2 } else { 0 },
        workspace: HyprWorkspaceRef {
            id: window.workspace_num,
            name: window.workspace.clone(),
        },
    }
}

/// `outputs` as Hyprland's `j/monitors` would list them: those on, in
/// Sway's order (a window's `monitor` is its place in it).
pub fn as_monitors(outputs: &[Output]) -> Vec<HyprMonitor> {
    outputs
        .iter()
        .filter(|o| o.rect.width > 0 && o.rect.height > 0)
        .map(|o| HyprMonitor {
            name: o.name.clone(),
            x: o.rect.x,
            y: o.rect.y,
            width: o.rect.width,
            height: o.rect.height,
            scale: 1.0,
            transform: 0,
        })
        .collect()
}

/// The windows of the Sway this server runs in, the focused one, and its
/// outputs, as Hyprland's list gives them. Blocking.
pub fn list() -> std::io::Result<(Vec<HyprWindow>, Option<HyprWindow>, Vec<HyprMonitor>)> {
    let tree = crate::virtual_screen::ipc(GET_TREE, "")?;
    let outputs = crate::virtual_screen::sway_outputs()?;
    let monitors = as_monitors(&outputs);
    let all = windows(&tree);
    let index = |name: &str| {
        monitors
            .iter()
            .position(|m| m.name == name)
            .map_or(-1, |i| i as i32)
    };
    let shown: Vec<HyprWindow> = all.iter().map(|w| as_hypr(w, index(&w.output))).collect();
    let active = all
        .iter()
        .position(|w| w.focused)
        .and_then(|i| shown.get(i).cloned());
    Ok((shown, active, monitors))
}

/// The window `id` now, from the tree. Blocking.
pub fn seen(id: i64) -> std::io::Result<Option<SwayWindow>> {
    Ok(find(&crate::virtual_screen::ipc(GET_TREE, "")?, id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A tree shaped as Sway's: two outputs, a tiled terminal and a split
    /// of two on the first's workspace 1, a floating window on workspace 2
    /// of the second, an empty split, and one in the scratchpad.
    fn tree() -> Value {
        serde_json::from_str(TREE).unwrap()
    }

    const TREE: &str = r#"{
      "id": 1, "type": "root", "name": "root",
      "rect": {"x":0,"y":0,"width":3840,"height":1080}, "floating_nodes": [],
      "nodes": [
        { "id": 2147483647, "type": "output", "name": "__i3", "floating_nodes": [],
          "rect": {"x":0,"y":0,"width":3840,"height":1080},
          "nodes": [ { "id": 2147483646, "type": "workspace", "name": "__i3_scratch", "num": -1,
            "rect": {"x":0,"y":0,"width":3840,"height":1080}, "nodes": [],
            "floating_nodes": [ { "id": 99, "type": "floating_con", "name": "hidden", "app_id": "hid",
              "pid": 9, "rect": {"x":0,"y":0,"width":10,"height":10},
              "window_rect": {"x":0,"y":0,"width":10,"height":10},
              "nodes": [], "floating_nodes": [] } ] } ] },
        { "id": 3, "type": "output", "name": "OUT-1", "floating_nodes": [],
          "rect": {"x":0,"y":0,"width":1920,"height":1080},
          "nodes": [ { "id": 4, "type": "workspace", "name": "1", "num": 1, "floating_nodes": [],
            "rect": {"x":0,"y":0,"width":1920,"height":1080},
            "nodes": [
              { "id": 5, "type": "con", "name": "term", "app_id": "term-app", "pid": 100,
                "shell": "xdg_shell", "focused": true, "border": "normal", "current_border_width": 2,
                "rect": {"x":0,"y":27,"width":960,"height":1053},
                "deco_rect": {"x":0,"y":0,"width":960,"height":27},
                "window_rect": {"x":2,"y":0,"width":956,"height":1051},
                "fullscreen_mode": 0, "nodes": [], "floating_nodes": [] },
              { "id": 6, "type": "con", "name": null, "floating_nodes": [], "nodes": [
                  { "id": 7, "type": "con", "name": "old x11", "app_id": null, "pid": 101,
                    "shell": "xwayland", "window": 4194305, "border": "pixel", "current_border_width": 1,
                    "window_properties": { "class": "XClass" },
                    "rect": {"x":960,"y":0,"width":960,"height":540},
                    "window_rect": {"x":1,"y":1,"width":958,"height":538},
                    "nodes": [], "floating_nodes": [] },
                  { "id": 8, "type": "con", "name": null, "nodes": [], "floating_nodes": [] } ] } ] } ] },
        { "id": 10, "type": "output", "name": "OUT-2", "floating_nodes": [],
          "rect": {"x":1920,"y":0,"width":1920,"height":1080},
          "nodes": [ { "id": 11, "type": "workspace", "name": "work", "num": -1, "nodes": [],
            "rect": {"x":1920,"y":0,"width":1920,"height":1080},
            "floating_nodes": [ { "id": 12, "type": "floating_con", "name": "dialog", "app_id": "dlg",
              "pid": 102, "shell": "xdg_shell", "border": "none",
              "rect": {"x":2000,"y":100,"width":400,"height":300},
              "window_rect": {"x":0,"y":0,"width":400,"height":300},
              "fullscreen_mode": 1, "nodes": [], "floating_nodes": [] } ] } ] }
      ]
    }"#;

    #[test]
    fn the_windows_are_the_programs_not_the_containers_or_the_scratchpad() {
        let all = windows(&tree());
        assert_eq!(
            all.iter().map(|w| w.id).collect::<Vec<_>>(),
            [5, 7, 12],
            "{all:?}"
        );
    }

    #[test]
    fn a_window_is_its_own_content_on_its_output_and_workspace() {
        let all = windows(&tree());
        let term = &all[0];
        assert_eq!(term.class, "term-app");
        assert_eq!(term.title, "term");
        assert_eq!((term.at, term.size), ([2, 27], [956, 1051]));
        assert_eq!(term.origin, [0, 0], "its container's corner, title bar in");
        assert_eq!(term.frame, [960, 1080], "and size");
        assert_eq!(
            (
                term.output.as_str(),
                term.workspace.as_str(),
                term.workspace_num
            ),
            ("OUT-1", "1", 1)
        );
        assert!(term.focused && !term.floating);
        assert_eq!((term.border.as_str(), term.border_width), ("normal", 2));
        let x11 = &all[1];
        assert_eq!(x11.class, "XClass", "an X11 window's class stands in");
        assert_eq!((x11.at, x11.size), ([961, 1], [958, 538]));
        let dialog = &all[2];
        assert!(dialog.floating && !dialog.focused);
        assert_eq!(dialog.output, "OUT-2");
        assert_eq!(dialog.workspace, "work");
        assert_eq!(dialog.fullscreen, 1);
        assert_eq!((dialog.at, dialog.size), ([2000, 100], [400, 300]));
    }

    #[test]
    fn a_window_is_found_by_its_container_id() {
        assert_eq!(find(&tree(), 12).unwrap().class, "dlg");
        assert!(
            find(&tree(), 8).is_none(),
            "an empty container is no window"
        );
        assert!(find(&tree(), 99).is_none(), "nor is one in the scratchpad");
        assert!(find(&json!({}), 5).is_none());
        assert!(windows(&json!("not a tree")).is_empty());
    }

    #[test]
    fn the_list_has_hyprlands_shape() {
        let all = windows(&tree());
        let outputs = [
            Output {
                name: "OUT-1".into(),
                rect: Rect {
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                },
            },
            Output {
                name: "OFF".into(),
                rect: Rect::default(),
            },
            Output {
                name: "OUT-2".into(),
                rect: Rect {
                    x: 1920,
                    y: 0,
                    width: 1920,
                    height: 1080,
                },
            },
        ];
        let monitors = as_monitors(&outputs);
        assert_eq!(
            monitors.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            ["OUT-1", "OUT-2"],
            "an output that is off is no monitor"
        );
        let hypr = as_hypr(&all[2], 1);
        assert_eq!(hypr.address, "12");
        assert_eq!(
            (hypr.class.as_str(), hypr.title.as_str()),
            ("dlg", "dialog")
        );
        assert_eq!((hypr.monitor, hypr.fullscreen), (1, 2));
        assert_eq!(hypr.workspace.name, "work");
        assert!(hypr.mapped && !hypr.hidden);
        // The windows plugin's entry for it: monitor-local origin.
        let entry = crate::plugins::windows::entries(vec![hypr], &monitors);
        let json = serde_json::to_value(&entry[0]).unwrap();
        assert_eq!(json["local_at"], json!([80, 100]));
        assert_eq!(json["monitor_size"], json!([1920, 1080]));
    }
}
