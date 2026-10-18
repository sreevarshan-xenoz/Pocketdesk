# A headless test harness for the desktop app (C4)

**Plan:** `docs/research/2026-10-17-next-cycle.md`, C4.

## Why

About twenty of the checklist's owed desktop checks are done by eye, through the app's own window (§24, §88, §94, §99, §103–§105). Nothing in CI clicks a page, so a page that stops drawing, a button that stops doing what it says, or a layout that falls apart is found only when someone looks.

## What

`egui_kittest` (0.36.2, matching egui 0.36) runs the app headless and finds widgets through AccessKit, which the app already turns on:
- **The app as it runs:** the eframe builder drives `PocketdeskApp::new(state_dir, None, page)`, `logic` included, so each test sees what the app reads from its state and config folders.
- **Throwaway folders:** every test runs over a throwaway state folder and config folder, set once for the test binary through the core's test seam (`paths::use_state_dir`, `use_config_dir`), with the tests run one at a time. They never read or write this computer's state or config; a page that looks elsewhere (the file manager's menu folder on Settings, the network on Pair) only reads.
- **Fixtures as the server writes them:** a paired phone in `trusted_devices.json`, its battery in `phone_status.json`, a config file. They are written through `pocketdesk-core`'s own types where it has them, so a format change breaks the test, not the app.

## The tests

- **Navigation:** every page down the left opens, with its heading and its purpose line.
- **The save bar:** a change in Settings shows the bar with Save and Undo; Undo takes the change back and hides the bar; Save writes the config file and says which changes apply now.
- **Unpair:** a paired phone's Unpair… asks by name; Cancel leaves it paired; the confirmation removes it from the trust store.
- **Empty states:** Devices, Files and Sessions with nothing in them each say what to do next and offer the button that does it.
- **The phone panel:** the Overview shows a paired phone's block with its battery and its Ring, Send file… and Send link….

## Snapshots

- The Overview, Devices and Settings at 720×480, in the light and dark themes, as PNGs under `desktop/tests/snapshots/`, compared within `egui_kittest`'s threshold.
- They need a GPU renderer (`wgpu`), which the usual test run should not pay for. They sit behind the desktop crate's `snapshots` feature, which turns on `egui_kittest`'s `snapshot` and `wgpu` features through an optional dependency (Cargo cannot enable a dev-dependency's features from a feature).
- `UPDATE_SNAPSHOTS=1` writes them anew.

## CI

- The AccessKit tests run with the workspace's tests, on Linux and Windows, as every desktop test does now.
- The snapshots get a job of their own (`snapshots`), on Linux with Mesa's software Vulkan (lavapipe), where the images are made.
- A failed comparison uploads the new image and the difference as artifacts.

## Out of scope

- The cast window (`cast_view`), which needs `ffmpeg`.
- The tray.
- Fonts from the system: the tests use egui's own, so text looks the same on every runner.

## As built

- **Tests** (`desktop/src/ui_tests.rs`, six with the workspace's tests):
  - every page opens from the left with its purpose;
  - Devices, Sessions and Files empty, each with its next step;
  - Unpair (Cancel keeps, Unpair removes from the trust store);
  - the Overview's phone with its battery and actions;
  - the save bar (Undo, and Save writing `config.toml`).
- **The harness:** the app is built as `main.rs` builds it, with its theme installed, less the system's fonts. The left panel is drawn first, so a page is opened by the first node with its name (its heading says it too).
- **Snapshots:** one test draws the Overview, Devices and Settings in both themes with a paired phone, and reports every page that changed, not only the first.
- **A dependency mismatch fixed on the way:** `gpu-allocator` takes any `windows` from 0.53 to 0.62. Adding it, Cargo chose 0.61, already in the lock (for `enigo`), while `wgpu-hal` 30 hands it 0.62's types, so the feature did not build on Windows. The lock now gives it 0.62; Cargo cannot pin a dependency's choice in a manifest, so CI's Windows job builds the feature to catch a re-resolve.
- **The first pictures:** drawn here on this box's GPU and looked at, then left to CI: the committed set comes from its software Vulkan.

