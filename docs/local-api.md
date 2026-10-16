# The local API

Scripts on this computer can ask Pocketdesk what is going on and tell it what to do, through the `pocketdesk` program with `--json`. There is nothing to turn on and nothing listening: no socket and no HTTP port. It is this program, run as you, reading and writing Pocketdesk's state folder, which is already yours alone. Anything that can run it as you could already read those files.

Design: `docs/superpowers/specs/2026-10-15-local-api-design.md`. Source: `server/src/local_api.rs`.

## The rules

These hold for `status`, `devices`, `sent`, `received`, `send`, `link`, `kick`, `pair`, `unpair`, `list` and `relay-ids`. `events` prints many objects, one per line, each following them. Three older commands keep formats of their own: `sessions --json` prints one object per line (one per session), `capabilities --json` the generated report (`docs/capabilities.md`), and `bench --json` the benchmark's record; none of them carries `api`.

- **One JSON object per run** on stdout, on one line. Logs go to stderr.
- **Every object carries `"api": 1`.** Within version 1, fields are only ever added, never renamed, removed or given another meaning. A field a script does not know should be ignored.
- **A failure is JSON too**, for any command run with `--json`: a usage error, a config.toml that does not parse, or the command failing prints `{"api": 1, "error": "…"}` and exits with a code other than 0. A script can test for `error`.
- **Exit codes:**

  | Code | Meaning |
  |---|---|
  | 0 | Done |
  | 1 | Failed (`error` says why), including `unpair` of a phone that is not paired and `pair` with a PIN that is not 6 digits |
  | 2 | Bad arguments: a usage error, no phone by that name for `send`, a path that is not there, nothing named for `kick` |
  | 3 | No Pocketdesk server is running, and the command needs one |
  | 4 | `--wait` ran out while the phone had still not saved the file |

- **A phone** is named by its device id, or by 6 or more of its first characters when no other paired phone's id starts the same way. `pocketdesk devices` lists the ids.
- **Times** are Unix seconds. **Sizes** are bytes.

## Reading

### `pocketdesk status --json`

Whether the server runs, the phones in a session now, and how phones reach this computer from outside.

```json
{"api": 1,
 "server": {"running": true, "pid": 4242},
 "pocketdesk_version": "0.3.0",
 "sessions": [{"session_id": 7, "device_id": "0d7c3a52-…", "peer": "192.0.2.7:51234",
               "transport": "lan", "started_unix": 1792050000, "driving": true,
               "control_only": false, "cast": false}],
 "endpoint": {"…": "what the server announces for the internet path, or null"}}
```

`sessions` is empty and `endpoint` null when no server runs. `pocketdesk_version` is this program's: a server started before an upgrade may still be an older one. `transport` is `lan` or `wan`. `driving` says which phone has the keyboard and pointer when several are connected. `control_only` is a phone used as a remote: input only, no screen sent. `cast` is a phone showing its own screen here.

### `pocketdesk devices --json`

The paired phones.

```json
{"api": 1, "server_running": true,
 "devices": [{"id": "0d7c3a52-…", "grant_remaining_secs": null, "in_session": true,
              "key_bound": true, "internet_key_bound": true,
              "permissions": {"input": true, "clipboard": true, "files": true, "audio": true,
                              "mic": true, "power": true, "exec": false,
                              "notifications": true, "texts": true, "links": true, "media": true, "cast": true},
              "battery": {"charge": 54, "charging": false, "updated_unix": 1792069000},
              "media": {"title": "Song", "artist": "Artist", "app": "Spotify", "isPlaying": true,
                        "canPlayPause": true, "canNext": true, "canPrevious": true}}]}
```

`grant_remaining_secs` is null for a lasting pairing and counts down for one made with `pair --grant`. `key_bound` says the phone has proved its key on this network; `internet_key_bound`, from outside. Each permission is what the phone may do now, defaults included. `battery` is the phone's last report, sent during sessions (null if it never reported); `updated_unix` says how old it is. `media` is what the phone plays, as it says during a session with "My media on the computer" on (null when nothing plays, or the phone is not connected, has Media off, or the server is stopped).

### `pocketdesk sent --json` and `pocketdesk received --json`

The desktop app's Files tab: the last 20 (`--count N` for another number; the lists keep 50), newest first.

```json
{"api": 1, "sent": [{"id": "8b4b2722-…", "name": "Trip/", "path": "C:\\Users\\me\\Trip", "size": 8030000,
                     "device_id": "0d7c3a52-…", "sent_unix": 1792054206,
                     "state": "sent", "reason": null, "note": "1 left out: names the phone cannot take"}]}
{"api": 1, "received": [{"name": "photo.jpg", "path": "C:\\Users\\me\\Downloads\\photo.jpg", "size": 2100000,
                         "received_unix": 1792054100, "from": "0d7c3a52-…", "still_there": true}]}
```

A sent row's `state` is `offered` (the phone has not taken it yet), `sent` (saved on the phone) or `failed` (with `reason`). A folder's name ends in `/`, and its `note` says what it left out.

### `pocketdesk sessions --json`, `pocketdesk capabilities --json`, `pocketdesk list --json`, `pocketdesk relay-ids --json`

- `sessions`: the recorded sessions, one object per line, with their measurements.
- `capabilities`: what this build speaks (`docs/capabilities.md` is made from it).
- `list`: the tailnet's peers, `{"api": 1, "peers": [{"name", "dns_name", "online", "ips"}]}`.
- `relay-ids`: the ids a relay elsewhere should allow, `{"api": 1, "ids": [{"who", "id"}]}`.

`pocketdesk bench --json` prints the encoder benchmark's record, a contract of its own for CI (exit codes in `pocketdesk bench --help` and README).

### `pocketdesk events [--since-start] --json`

What Pocketdesk does as it happens, one object per line, until stopped: the server's event log (`<state>/events.jsonl`, owner-only, design `docs/superpowers/specs/2026-10-16-events-design.md`). `--since-start` prints the events already in the log first.

```json
{"api": 1, "at": 1792050000, "event": "file_received", "device_id": "0d7c3a52-…", "size": 30412}
```

Every event has `at`, `event` and `device_id` (the phone, or null). The kinds and their fields:

| `event` | Fields | With "Details in events" also |
|---|---|---|
| `session_started` | `session_id`, `transport` (`lan`/`wan`), `mode` (`screen`, `remote`, `cast`) | — |
| `session_ended` | `session_id` | — |
| `phone_linked`, `phone_unlinked` | — (any connection the phone holds to this computer, a brief one for a single send included) | — |
| `phone_battery` | `charge`, `charging` | — |
| `phone_battery_low` | `charge` | — |
| `notification` | `app` | `title`, `text` |
| `text_received` | — | `from`, `body` |
| `file_received` | `size`; or `folder: true`, once, as a folder starts arriving | `name` (`folder/` for a folder) |
| `link_received` | `host` | `url` |

By default an event says who, when and how much, never what. Settings "Details in events" (`events_detail = "full"`) adds the content. With "Event log" off (`events = false`) nothing is written, and `events` shows nothing. New kinds and fields may be added within `"api": 1`.

**Hooks.** `[hooks]` in config.toml (or the desktop app's Settings) runs a command for each event of a kind, through the system shell, with the event's JSON on stdin and, when it is under 32 KiB (always, for the events above), in `POCKETDESK_EVENT`:

```toml
[hooks]
file_received = "notify-send 'A file came from the phone'"
```

At most 4 hooks run at once and up to 32 more wait their turn (past that one is skipped, and logged); each is stopped after 30 s (programs it started in the background may outlive it), and none opens a window. A hook runs even with "Event log" off.

**Read the event from stdin, and treat its fields as untrusted text:** a phone chooses a notification's app name and text, a file's name, a link. On Windows never write `%POCKETDESK_EVENT%` into the command line: cmd expands it before it parses `&`, `|` and quotes, so a field could run commands. On Unix, `"$POCKETDESK_EVENT"` in double quotes is safe.

## Acting

### `pocketdesk send <phone> <path> [--wait SECONDS] --json`

Sends a file or a folder to a connected phone, exactly as the desktop app's Files tab does, so the server's checks apply: the phone is paired, may take files and is connected; a folder leaves out hidden files and stops at its limits. Connected means in a session, or holding a control link on this network (the phone's app open, or a background feature keeping one, T10d). A phone not connected so, or connected only over the internet (the computer cannot dial it back), is refused at once (exit 1).

```json
{"api": 1, "id": "8b4b2722-…", "device_id": "0d7c3a52-…", "path": "C:\\Users\\me\\report.pdf",
 "state": "queued", "reason": null, "note": null}
```

Without `--wait` it ends once the request is written for the server to take (`queued`); a server that stops before taking it drops it after a minute. With `--wait`, it waits up to that many seconds. Once the server has taken the request, `state`, `reason` and `note` are the sent list's (`offered` while the phone has not saved it yet), and the exit code is 0 when the phone saved it, 1 when it failed, 4 when time ran out, and 3 when the server stopped while waiting. A send refused before it was queued has `"state": "refused"`, with the same text in `reason` and `error`.

```sh
pocketdesk send 0d7c3a backup.tar --wait 600 --json || notify-send "backup did not reach the phone"
```

### `pocketdesk link <phone> <url> --json`

Hands a web link to a phone in a session; it waits there as a notification that opens it (T8a). Only `http` and `https` links of at most 2048 bytes, with no user name or password, and not to the phone's own `localhost`. `{"api": 1, "id": "…", "device_id": "…", "url": "…", "state": "queued"}`. Exit 2 for a link that may not be opened, 1 for a phone not in a session or with its Links permission off, 3 when no server runs.

### `pocketdesk kick <target> --json`

Ends live sessions: a device id, an id prefix, a peer address, or `all`. `{"api": 1, "kick_requested": "all"}`. Exit 3 when no server runs.

### `pocketdesk pair [PIN] [--grant 15m] --json`

Makes a pairing PIN, valid for one phone: `{"api": 1, "pin": "123456", "valid_secs": 300, "grant_secs": null}`. The PIN is a secret for 5 minutes; do not log it.

### `pocketdesk unpair [DEVICE-ID] --json`

Removes a paired phone, or every one when none is named: `{"api": 1, "unpaired": ["0d7c3a52-…"]}`.

## The phone

Automation apps on the phone (Tasker and the like) use Android intents: connect, wake, text to a computer's clipboard, files to it. `docs/phone-automation.md` lists them.
