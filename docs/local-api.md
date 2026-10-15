# The local API

Scripts on this computer can ask Pocketdesk what is going on and tell it what to do, through the `pocketdesk` program with `--json`. There is nothing to turn on and nothing listening: no socket and no HTTP port. It is this program, run as you, reading and writing Pocketdesk's state folder, which is already yours alone. Anything that can run it as you could already read those files.

Design: `docs/superpowers/specs/2026-10-15-local-api-design.md`. Source: `server/src/local_api.rs`.

## The rules

- **One JSON object per run** on stdout, on one line. `sessions --json` prints one object per line instead (one per session), as it always has. Logs go to stderr.
- **Every object carries `"api": 1`.** Within version 1, fields are only ever added, never renamed, removed or given another meaning. A field a script does not know should be ignored.
- **A failure is JSON too.** A command run with `--json` that fails prints `{"api": 1, "error": "…"}` and exits with a code other than 0.
- **Exit codes:**

  | Code | Meaning |
  |---|---|
  | 0 | Done |
  | 1 | Failed (the `error` or `reason` field says why) |
  | 2 | Bad arguments: no phone by that name, a path that is not there |
  | 3 | No Pocketdesk server is running, and the command needs one |
  | 4 | `--wait` ran out while the phone had still not saved the file |

- **A phone** is named by its device id, or by 6 or more of its first characters when no other paired phone's id starts the same way. `pocketdesk devices` lists the ids.
- **Times** are Unix seconds. **Sizes** are bytes.

## Reading

### `pocketdesk status --json`

Whether the server runs, the phones in a session now, and how phones reach this computer from outside.

```json
{"api": 1,
 "server": {"running": true, "pid": 4242, "version": "0.2.0"},
 "sessions": [{"session_id": 7, "device_id": "0d7c3a52-…", "peer": "192.0.2.7:51234",
               "transport": "lan", "started_unix": 1792050000, "driving": true}],
 "endpoint": {"…": "what the server announces for the internet path, or null"}}
```

`sessions` is empty and `endpoint` null when no server runs. `transport` is `lan` or `wan`. `driving` says which phone has the keyboard and pointer when several are connected.

### `pocketdesk devices --json`

The paired phones.

```json
{"api": 1, "server_running": true,
 "devices": [{"id": "0d7c3a52-…", "grant_remaining_secs": null, "in_session": true,
              "key_bound": true, "internet_key_bound": true,
              "permissions": {"input": true, "clipboard": true, "files": true, "audio": true,
                              "mic": true, "power": true, "exec": false,
                              "notifications": true, "texts": true}}]}
```

`grant_remaining_secs` is null for a lasting pairing and counts down for one made with `pair --grant`. `key_bound` says the phone has proved its key on this network; `internet_key_bound`, from outside. Each permission is what the phone may do now, defaults included.

### `pocketdesk sent --json` and `pocketdesk received --json`

The desktop app's Files tab: the last 20 (`--count N` for more), newest first.

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

## Acting

### `pocketdesk send <phone> <path> [--wait SECONDS] --json`

Sends a file or a folder to a phone in a session, exactly as the desktop app's Files tab does, so the server's checks apply: the phone is paired, may take files and is connected; a folder leaves out hidden files and stops at its limits.

```json
{"api": 1, "id": "8b4b2722-…", "device_id": "0d7c3a52-…", "path": "C:\\Users\\me\\report.pdf",
 "state": "queued", "reason": null, "note": null}
```

Without `--wait` it ends once the server has the request (`queued`). With `--wait`, it waits up to that many seconds for the outcome; then `state`, `reason` and `note` are the sent list's, and the exit code is 0 when the phone saved it, 1 when it failed, and 4 when time ran out. A send refused before it was queued has `"state": "refused"` and a `reason`.

```sh
pocketdesk send 0d7c3a backup.tar --wait 600 --json || notify-send "backup did not reach the phone"
```

### `pocketdesk kick <target> --json`

Ends live sessions: a device id, an id prefix, a peer address, or `all`. `{"api": 1, "kick_requested": "all"}`. Exit 3 when no server runs.

### `pocketdesk pair [PIN] [--grant 15m] --json`

Makes a pairing PIN, valid for one phone: `{"api": 1, "pin": "123456", "valid_secs": 300, "grant_secs": null}`. The PIN is a secret for 5 minutes; do not log it.

### `pocketdesk unpair [DEVICE-ID] --json`

Removes a paired phone, or every one when none is named: `{"api": 1, "unpaired": ["0d7c3a52-…"]}`.

## The phone

Automation apps on the phone (Tasker and the like) are a later step (T7b): the intents that exist today and a switch to let other apps use Pocketdesk.
