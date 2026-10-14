# Pocketdesk feature roadmap — making the phone and desktop apps good

Status: decided 2026-10-12, from [the competitor research](../../research/2026-10-12-competitor-features.md) and an audit of the tree at `main` 8c2341f.

## Where the apps stand

The transport, video, audio and pairing are solid and device-verified. The apps on top of them are thin.

**Phone:**
- Missing: any way to type in a session (the bridge has `sendKeyboardEvent` and `sendText`, but nothing captures the phone keyboard), and pinch-zoom outside direct-touch mode.
- Partial: the trackpad has only one-finger move, tap and two-finger scroll.
- Missing: discovery, file transfer UI and image clipboard.

**Desktop app:** status, Pair, Devices, Sessions, Settings and Logs, all read-only views over files. It cannot approve connections, set permissions, transfer files or help diagnose a failure.

## Order of work

Each item is its own branch (`feat/<topic>`), spec, plan and device check on the Galaxy A71, and merges to `main` once CI is green.

| # | Feature | App | Why first | Size |
|---|---|---|---|---|
| F1 | **In-session keyboard.** The stream view is the IME target: committed text goes as Unicode text, Backspace/Enter/arrows as keys, composing text waits. A keyboard button is always in the chrome. A docked accessory row has latching Ctrl/Alt/Shift/Super (tap = once, double-tap = lock) and a scrollable row with Esc, Tab, arrows, Home/End, PgUp/PgDn, Del, F1–F12 and Ctrl+Alt+Del. Zoom is kept when the keyboard opens. | Phone | A remote desktop you cannot type into is the single biggest gap; every competitor has this. | L |
| F2 | **Send text:** a compose box (dictation and long text, with history) and "type the clipboard as keystrokes" for fields that block paste. | Phone | Cheap once F1 exists; loved in Moonlight, scrcpy and KDE Connect. | S |
| F3 | **Trackpad that feels like a laptop's:** two-finger tap = right click, three-finger tap = middle click, two-finger horizontal scroll, double-tap-and-hold drag, sensitivity setting, pinch-zoom in trackpad mode (still open: F3 shipped without it, and zoom remains direct-touch only). | Phone | Precision is the second most common complaint. | M |
| F4 | **Linux text path:** Unicode through `zwp_virtual_keyboard_v1` with an uploaded keymap (the `wtype` technique), and Shift for capitals on uinput (roadmap 2666). | Server | F1 types Unicode correctly on Windows (UTF-16 SendInput) but not on Linux. | M |
| F5 | **Approve connections and set per-device permissions:** an approve/deny prompt in the desktop app for a new session (once / always / deny), and per-device toggles (view-only, input, clipboard, files, audio, mic, power) enforced by the server. | Desktop + server | Every host has this; it is the security baseline a remote desktop owes its owner. | M |
| F6 | **LAN discovery** on the phone home screen (found desktops listed above saved ones, one tap to pair), plus a "visible on the network" switch in the desktop app. | Both | Removes the setup friction users complain about most after input. | M |
| F7 | **File transfer:** share files into Pocketdesk from any app, a remote file browser with download, and a drop zone plus transfer history in the desktop app. | Both | The bridge has file-list and send-file; there is no UI. | L |
| F8 | **Image clipboard** both ways. | Both | Expected next to text clipboard. | M |
| F9 | **Session polish:** in-session monitor quick-switch (shipped 2026-10-13: a chip per screen in the session chrome), optional three-finger swipes for Alt+Tab and workspaces, hardware keyboard and mouse passthrough with pointer capture. | Phone | Power users, on top of F1/F3. | M |
| F10 | **Desktop app diagnostics:** port and firewall check, advertised interfaces, encoder in use, last session's latency tails, "copy debug report" (shipped 2026-10-13). | Desktop | Turns "it doesn't connect" into a fixable message. | M |
| F11 | **Shortcuts outside the app:** home-screen widget, Quick Settings tile, launcher shortcuts for connect and wake (shipped 2026-10-13). | Phone | Small, makes the app feel native. | S |

**Next, agreed 2026-10-13 (in this order; each gets its own spec, review, device test and PR):**

| # | Feature | Side |
|---|---|---|
| N1 | **Cross-network without Tailscale, step 1.** A real cellular test of the built-in iroh path (owed: the test phone has no mobile service). Router port mapping (UPnP / NAT-PMP / PCP) is iroh's default and already on. The phone's HUD already shows direct or relayed. Diagnostics now says whether this computer's NAT allows direct connections, and whether IPv6 is public (shipped 2026-10-13). | Both |
| N2 | **Your own relay.** `pocketdesk relay` runs iroh's relay server: on the desktop itself with one port forwarded, on any always-on box, or on a free cloud VM. It admits only listed endpoints (`--allow-paired` admits this desktop and its phones). The desktop's `relay_urls` setting points at it, and the phone learns it from the desktop, so it has no setting of its own (shipped 2026-10-13). | Both |
| N3 | **Finding the desktop without anyone's server.** The desktop publishes a record signed with its iroh key on the Mainline DHT (pkarr): its relay, and with `dht_publish = "addresses"` its public IPs. A phone whose cached address went stale looks it up by id. IPv6 direct paths were already bound on both ends. Diagnostics' "Found by id" says whether the record reads back (shipped 2026-10-14). | Both |
| N4 | **Image clipboard** (F8): images both ways as PNG through the TLS transfer path, under the Clipboard permission (shipped 2026-10-14). | Both |
| N5 | **Notifications both ways, with actions**, and the phone's notifications and SMS on the desktop. Split in three: N5a, the phone's notifications on the desktop (shipped 2026-10-14: opt-in, per-app, lock-screen privacy kept, a desktop permission and a flood guard); N5b, SMS; N5c, actions and dismissal both ways (part 1 shipped 2026-10-14: a notification closed on the phone closes on the computer). N5b SMS shipped 2026-10-14, built on the user's word; its device checks wait for a phone with a SIM. | Both |
| N6 | **Wake and connect in one tap from anywhere**, without a second always-on device: the computer tells the phone its MAC and broadcast address, the phone sends the magic packet itself at home and to the home's public address (a router forward) from outside, then connects once the computer answers. Each computer's method, relay, outside address and port are the user's to set (shipped 2026-10-14). | Both |
| N7 | **Multiple phones at once**, with a desktop-side picker. | Desktop |
| N8 | **Gamepad mode**: an on-screen controller layout. | Phone |
| N9 | **Session recording and a screenshot button.** | Both |
| N10 | **Drawing-tablet mode**: stylus pressure into the desktop. | Both |
| N11 | **The phone as a second monitor**: a virtual display (Windows indirect display driver, Linux virtual output). | Both |

**Deferred, with reason:**
- A virtual display sized to the phone: Hyprland's write IPC is broken upstream, and Windows needs an indirect display driver.
- Multi-session.
- Session recording.

## Rules for every item

- **Verification on this box:** the phone (Galaxy A71 over USB, unlocked by waking it) against the Windows host. Linux-only behaviour is unit-tested here, with a Linux CI run; on-device Linux checks stay owed in the checklist.
- **Branch, review, merge:** a fresh reviewer reads each branch before merge. Each merge to `main` waits for CI.
- **Docs:** README, the checklist and AGENTS *Current Status* change in the same commits as the code they describe.
