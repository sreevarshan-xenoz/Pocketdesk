# Events and hooks (ecosystem phase 1a)

Part of the ecosystem plan (`docs/research/2026-10-16-ecosystem.md`, phase 1). Other programs — a script, Home Assistant (phase 1c), a status bar — want to know what Pocketdesk does as it happens: a phone connected, its battery fell, a file arrived. Today they can only poll `pocketdesk status --json` and its siblings.

## What it does

- **The event log.** The server appends one JSON object per line to `<state>/events.jsonl`, owner-only, for each event below. At 1 MiB the file moves to `events.jsonl.1` (one kept). Nothing is written with Settings "Event log" off (`events = false`).
- **`pocketdesk events [--json] [--since-start]`** prints each new event as it is written, until stopped. It prints a line per event (`--json`: the object, with `"api": 1`). It reads only the owner's own file, like the rest of the local API, with no listener. `--since-start` prints the events already in the log first.
- **Hooks.** In config.toml, `[hooks]` maps an event kind to a command line, e.g. `file_received = "notify-send 'got a file'"`. The server runs it through the system shell (`sh -c`, `cmd /C`) with the event's JSON on stdin and in `POCKETDESK_EVENT`. It runs it detached: at most 4 at once (more are skipped and logged), each killed after 30 s, and its exit code is logged. No window opens on Windows. The desktop app's Settings lists the hooks and edits them.
- **Detail.** By default an event carries metadata only. Settings "Details in events" (`events_detail = "full"`) adds the content: notification title and text, a text's sender and body, a file's name, a link's address. Hooks see the same detail as the log.

## Events (v1)

Each event is `{"at": <unix s>, "event": <kind>, "device_id": <phone or null>, ...fields}`.

| Kind | Fields (metadata) | Added with full detail |
|---|---|---|
| `session_started` | `session_id`, `transport` (`lan`/`wan`), `mode` (`screen`, `remote`, `cast`) | — |
| `session_ended` | `session_id` | — |
| `phone_linked`, `phone_unlinked` | — (a phone's control link came up or went) | — |
| `phone_battery` | `charge`, `charging` (on a change of either) | — |
| `phone_battery_low` | `charge` (when the low warning fires) | — |
| `notification` | `app` | `title`, `text` |
| `text_received` | — | `from`, `body` |
| `file_received` | `size`, `folder: true` for each file of a folder | `name` |
| `link_received` | `host` | `url` |

A new kind or field is added, never renamed or removed, within `"api": 1`.

## Checks

- **Rust:**
  - the log format, rotation, the off switch, and detail levels (`core::local::events`);
  - `watch` printing appended events and `--since-start`;
  - hook running: stdin, the env var, the cap of 4, the 30 s kill (`server/src/events.rs`), with temp state dirs;
  - each emitting site's event (session start/end from the live-session watcher, battery, notification, text, file, link).
- **Docs:** `docs/local-api.md` (`watch`), protocol.md's files table, SECURITY.md (hooks run as you; details put private content on disk), README.
- **Device (A71 → Windows):** `pocketdesk events` showing a session start and end, a battery change, a file and a link; a hook writing a file.
