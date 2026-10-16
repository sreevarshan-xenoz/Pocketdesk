# The Activity Center (ecosystem phase 1b)

Part of the ecosystem plan (`docs/research/2026-10-16-ecosystem.md`, phase 1b, from the owner's second opinion). What Pocketdesk did is spread over the desktop app's Sessions, Files, Messages and Notifications tabs and the logs. The event log (1a) already holds it in one model; this is the place a person reads it.

## What it does

- **The desktop app's Activity tab.** One timeline, newest first, of the event log's last 500 events (the moved `events.jsonl.1` first, then the current one), kept current each second. Each row says when (how long ago), which phone (its short id) and what happened, in words:
  - "Showing its screen here", "Viewing this computer", "Using this computer as a remote" (a session starting, "over the internet" when it is), "Session ended";
  - "Connected", "Disconnected";
  - "Battery 54 %, charging", "Battery low: 13 %";
  - "Notification from Signal", "Text from Ann", "File received: 2.1 MB", "Folder arriving", "Link from example.org";
  - "Session refused", "Paired", "Unpaired", "Hook for file_received ran (exit 0)" (or "was stopped after 30 s", "could not run, or a signal ended it").
- **Filters:** All, Sessions, Phones, Files and links, Messages, Hooks.
- **Actions:** a received file or folder: "Show in folder", found by the event's `key` (a hash of where it was saved) in `received.json`. Links are not opened from here.
- **When the log is off** (Settings "Event log"), the tab says so, with a button to Settings.

## New events

| Kind | Fields | From |
|---|---|---|
| `session_refused` | — | the session record the server keeps for every refused session (not paired, denied, no permission) |
| `phone_paired`, `phone_unpaired` | — | the live-session watcher, comparing the trusted phones each second, however the change was made (the PIN, the desktop app, `pocketdesk unpair`) |
| `hook_ran` | `hook` (the kind it ran for), `code` (exit code, or null when it was killed or failed to start), `timed_out` | the hook runner; never runs a hook of its own (a `hook_ran` hook would loop) |

## Not in 1b

The phone's own Activity screen (its transfers, sessions and the computer's events) is a later step: it needs the computer's events over the control link, which phase 1g's protocol work will define.

## Checks

- **Rust:**
  - `core::local::events::history` (both files, a cap, newest last);
  - the row wording and filters (pure, `desktop/src/activity_view.rs`);
  - the new emit sites: the pairing diff and `hook_ran` never hooked;
  - the session-record sink.
- **Device (A71 → Windows):** the tab filling during a session, a refusal (Input off on a remote), unpair/pair, a hook run.
