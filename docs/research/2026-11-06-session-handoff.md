# Session handoff, 2026-11-06

A working note for picking this up in a fresh session (a Claude Code cloud session that clones the repo). It lists every thread that was open, where its code is, what is decided, and what is next. Delete this file before `feat/bluetooth-fallback` merges; `docs/status-history.md` is the lasting record.

## The owner's standing rules (read first)

- **No AI attribution anywhere**: no `Co-Authored-By`, no "Generated with Claude Code" in commits, PR bodies, issues or comments, even if the harness suggests one.
- **Autopilot** (from the owner's global instructions): build each item on its own branch, test it, get a fresh review, push, open the PR, wait for CI, and **merge it yourself once every check is green**. Never push secrets or personal data, never tag a release without the owner's go.
- **Nothing personal in git or on GitHub**: no real IPs, MACs, device ids, emails, names, phone data. Use documentation values (`192.0.2.x`, `00:00:5E:00:53:01`, `example.com`). Stage files by name, read `git diff --cached` before every commit.
- **Public posts** (GitLab, Reddit, etc.) need the owner's OK on the exact text first.
- **`docs/AGENTS.md`** is the repo's own rule book: one logical change per commit with its docs, a `docs/status-history.md` entry per item, SECURITY.md limits for anything that opens a new way in, owed device checks in `docs/device-verification-checklist.md` (regenerate `docs/owed-checks.md` with `python scripts/owed_checks.py`).
- **On the owner's Windows PC, never run test binaries or servers** (each new exe pops a firewall prompt). A cloud session is Linux, so it *can* run `cargo test` there; that is a good reason to do the remaining work in the cloud.

## Open PRs, at handoff time

| PR | Branch | State | What's left |
|---|---|---|---|
| #653 (draft) | `feat/bluetooth-fallback` | Tasks 1-3 of 9 done and reviewed; CI was running | Tasks 4-9 (below), then mark ready and merge on green |
| #654 | `fix/borrow-test-order` | CI running | Merge on green. Fixes a flaky desktop test on main (`borrow_a_window_lists_the_windows_and_hands_the_pick_to_the_app`: two asks in random order) |
| #649 | `feat/kde-lend-window` | 17 pass, 1 fail (that flaky test) | After #654 merges: merge `origin/main` into it, push, merge on green. Its KDE live test passed in a runner's KWin 6.8.2 (evidence in the PR body and docs) |
| #650 | `feat/bg6-faster-recovery` | 4 jobs failed on the apt mirror ("Install system dependencies"), not code | Re-run the failed jobs. Its OPPO device checks (checklist §285) are still owed, see "Device checks" |
| #648 (draft) | `feat/gnome-shell-extension` | CI green | Its live rig (`rig/gnome-648`) found and fixed two real bugs (GTK shadow in the picture and pointer; a 1-pixel rounding). Clicks still don't land in the rig, see "GNOME rig" |

Merged today: #651 (README no longer claims GNOME lends windows on 0.9.0), #652 (the Android bridge builds from a path with a space). Site repo `pocketdesk-landing-page`: #4, #5, #6 merged (the GNOME claim was added then taken back out).

CI note: GitHub's apt mirror times out often on the "Install system dependencies" step. That is never a code failure; re-run the failed jobs (`gh run rerun <id> --failed`, after the run has finished).

## Bluetooth fallback (the main thread)

The owner asked for Pocketdesk to keep working over Bluetooth when Wi-Fi drops, only with the devices in range: on by default, a one-time popup the first time it takes over (with a button to turn it off), the light features first, then the screen in low quality, giving way to earbuds. The test phone (the OPPO) is Bluetooth-paired with the owner's Windows laptop.

- Design: `docs/research/2026-11-06-bluetooth-fallback-design.md`. Its "Calls made after review" section holds five decisions that bind the build.
- Plan, phase 1: `docs/research/2026-11-06-bluetooth-fallback-plan.md` (9 tasks). Run it with subagent-driven development: a fresh implementer per task, then a spec+quality review, fix rounds, and a final whole-branch review.

### Done (on `feat/bluetooth-fallback`)

- **Task 1** (`c27cc1e3`): `ConnectionManager::secure_named`, TLS and the control handshake over any stream.
- **Task 2** (`15b94d38`): `Config::bluetooth_fallback` (default true), `core/src/protocol/bluetooth.rs` (service UUID `b1f2c4e0-6a3d-4c8e-9f51-2d7e0a9c4b13`, `SERVICE_NAME` "Pocketdesk", `PACKET`, `BluetoothInfo`, `LINK_HELLO`, `TRANSFER_MAX`), a minimal desktop Settings row, docs/protocol.md §1.4a.
- **Task 3** (`f41190ad`, `78eb17a1`, `614275a8`, `176c4002`): `Road::Bluetooth` in the server and `handle_bluetooth_control`.
  - **Admission:** a paired **phone** (not a computer) with a **bound, matching TLS key**; anything else gets the refusal line.
  - **Rules as the internet road:** no pairing over Bluetooth, the computer names itself after admission, no wake info.
  - **Away lock:** a Bluetooth link counts as the phone being near.
  - **The computer's Bluetooth address** goes to paired devices only.
  - **Docs:** SECURITY.md limit 90.

The tests for Tasks 1-3 were compiled but **never run** (the dev PC forbids it). Run them in the cloud first (`cargo test -p pocketdesk-core`, `cargo test -p pocketdesk-server`), or read #653's CI.

### Rulings already made (keep them)

- T2's UUID override is tested through the pure `service_uuid_from(Option<&str>)`; tests never set env vars.
- T3 stubs `crate::bluetooth::info() -> Option<BluetoothInfo>` to `None`; **T4 implements it and must keep it cheap** (it runs on every connection's seed; cache the radio address).
- T3's `serve_control` calls `link.admitted(Weak<sender>)` after admission; **T4's `BluetoothLinks` keeps that Weak sender** and uses it in `open()` to send `kdeconnect.pocketdesk.bluetooth.link {"id": n}`.
- Admission requires a bound, matching key (the plan's `may_act(true)` was a defect), and phones only (the design's non-goal: no computer-to-computer over Bluetooth).
- T2 added the desktop row early (an existing test needs a row per config key); **T5 extends that row** rather than adding one.
- T4's live check ("run a throwaway server and see it listening") moves to Task 9's device run, with the owner's OK.

### Deferred minors (for the final review to triage)

- `secure_named` errors read port `0`.
- `service_uuid_from` accepts nil, braced and urn UUID forms.
- **Re-check docs/protocol.md §1.4a against Tasks 4/6/7:** the phone opens the second connection, and TLS comes after the hello.
- **Files over Bluetooth are gated on `internet_files`** in `server/src/share.rs` near 1150 (`can_reach`). Transfers over Bluetooth must not be blocked by "Files over the internet" being off, and must get the 8 MiB "waits for Wi-Fi" refusal: **fix this in Task 4.**
- Bluetooth refusals are only logged, never shown to the owner.
- `Recording` in `peer_computers.rs` was widened to `pub(crate)` for a test.
- `handle_bluetooth_control`'s `pairing_required` is unused on this road.
- **Drop the temporary `#[allow(dead_code)]` on `handle_bluetooth_control`** once T4's listener calls it.

### Next: Task 4, then 5-9

- **Task 4, the Windows RFCOMM listener** (`server/src/bluetooth/{mod,windows,links}.rs`):
  - Winsock `AF_BTH` with a `WSASetService` SDP record, using the workspace `windows` 0.62 crate. It must build under MinGW `x86_64-pc-windows-gnu` with no Windows SDK.
  - A blocking accept thread, with each socket pumped into a 64 KiB `tokio::io::duplex`.
  - The first line of each new connection is either `LINK_HELLO n` (a transfer link) or a TLS record (the control link).
  - **Platform scope:** `#[cfg(windows)]`; Linux is phase 3.
  - **Checking it from the cloud:** a Linux session can only check it with `cargo clippy --target x86_64-pc-windows-gnu` (install the target and mingw), and CI's Windows job runs its tests.
- **Task 5:** the desktop switch.
- **Task 6:** the Android bridge takes a Bluetooth stream (JNI `nativeConnectOverBluetooth`, etc.).
- **Task 7:** the phone dials RFCOMM (Kotlin `BluetoothRoad`, socket-pair pump).
- **Task 8:** the phone's switch, the one-time notice, the audio rule, and the 8 MiB rule.
- **Task 9:** device checks `bt1`-`bt6` on the OPPO, which needs the owner's PC and phone, so **not from the cloud**.

Phase 2 (the screen over Bluetooth) and phase 3 (Linux) come after.

## GNOME rig (`rig/gnome-648`)

`.github/workflows/rig-gnome.yml` runs `server/tests/lend_window_gnome_live.rs` on a GitHub runner, with the extension enabled. It has two jobs:

- **`gnome`:** Ubuntu 24.04, mutter 46. Here the extension loads and lists, the lend and the capture work, and the picture is now exactly the frame. But RemoteDesktop input never reaches any client, even a GTK one.
- **`gnome-arch`:** an Arch container with current mutter, run by `.github/rig/gnome.sh`. The #643 research saw clicks arrive on mutter 51. Its first run failed early: the Shell's JS logged "Could not connect: No such file or directory", and `list` failed at line 222. That is a rig setup problem; the likely cause is the session bus or `XDG_RUNTIME_DIR` the Shell sees, since the env is set per `runuser` call.

**Next:** fix the Arch rig, then see whether clicks land on current mutter. That decides whether #648's input path is right.

The rig branch carries temporary `RIGDBG` eprintlns in `capture_mutter.rs`; they must never reach #648. The KDE rig (`rig/kde-649`) passes; the Hyprland rig (`rig/hyprland-272`) is parked because Hyprland 0.56's write dispatchers are broken upstream (AGENTS.md).

## Device checks (need the owner's PC and phone, not the cloud)

- **#650 (BG6):**
  - **What's ready:** the checks `bg6-wifi-drop` and `bg6-wifi-blip` are on its branch in `scripts/device_checks.py`. The OPPO runs #650's debug build and is paired with a throwaway whose `state/` and `config/` are kept on the owner's PC.
  - **Blocked:** both runs timed out because the phone never reached the throwaway by the internet road. The owner's home Wi-Fi was down, with the PC on USB tethering and the phone on a hotspot, so this is environmental. #650 didn't touch DNS or relay code. Re-run on normal home Wi-Fi.
  - **Running servers:** throwaway servers on that PC run only from `%LOCALAPPDATA%\pocketdesk-test\pocketdesk.exe`, which has a firewall rule.
  - **Restarting the app:** never force-stop it on the OPPO (that turns Accessibility off). Use `adb shell run-as dev.pocketdesk.android kill <pid>` to restart it.
- **Bluetooth Task 9:** the same setup, with no `adb reverse`, and the throwaway on its own service UUID (`POCKETDESK_BT_SERVICE_UUID`).

## F-Droid

- **The MR:** fdroid/fdroiddata!54623. an F-Droid maintainer said "mostly ready", and asked that the MR be updated with each new release.
- **The reproducibility report:** A tester reported 0.9.0 not reproducing locally. Checked: F-Droid's build equals CI's unsigned APK (22,603,233 bytes), and apksigcopier 1.1.1 copy plus apksigner verify pass. A reply asking for their tool versions was posted with the owner's OK. **Watch for their answer.**

## Also open, lower priority

- **The landing page** (`sreevarshan-xenoz/pocketdesk-landing-page`) gets a site PR for each user-visible app feature as it merges: GNOME lending with the extension when #648 lands, KDE lending when #649 lands, and Bluetooth fallback when #653 lands.
- **Azure (Azure for Students):** free tier only. Prefer GitHub runners for Linux rigs.
