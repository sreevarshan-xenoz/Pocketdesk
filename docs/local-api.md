# The local API

Scripts on this computer can ask Pocketdesk what is going on and tell it what to do, through the `pocketdesk` program with `--json`. There is nothing to turn on and nothing listening: no socket and no HTTP port. It is this program, run as you, reading and writing Pocketdesk's state folder, which is already yours alone. Anything that can run it as you could already read those files.

Source: `server/src/local_api.rs`.

## The rules

These hold for `status`, `devices`, `sent`, `received`, `send`, `link`, `ring`, `quiet`, `clipboard`, `kick`, `pair`, `unpair`, `list` and `relay-ids`. `events` prints many objects, one per line, each following them. Three older commands keep formats of their own: `sessions --json` prints one object per line (one per session), `capabilities --json` the generated report (`docs/capabilities.md`), and `bench --json` the benchmark's record; none of them carries `api`.

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
 "pocketdesk_version": "0.4.0",
 "sessions": [{"session_id": 7, "device_id": "0d7c3a52-…", "peer": "192.0.2.7:51234",
               "transport": "lan", "started_unix": 1792050000, "driving": true,
               "control_only": false, "cast": false, "sound_only": false,
               "phone_sound_only": false}],
 "endpoint": {"…": "what the server announces for the internet path, or null"}}
```

`sessions` is empty and `endpoint` null when no server runs. `pocketdesk_version` is this program's: a server started before an upgrade may still be an older one. `transport` is `lan` or `wan`. `driving` says which phone has the keyboard and pointer when several are connected. `control_only` is a phone used as a remote: input only, no screen sent. `cast` is a phone showing its own screen here. `sound_only` is a phone used as this computer's speaker and mic: sound both ways, no screen or input (N5). `phone_sound_only` is a phone playing its own sound here, with no screen (M7).

### `pocketdesk devices --json`

The paired phones.

```json
{"api": 1, "server_running": true,
 "devices": [{"id": "0d7c3a52-…", "name": "Galaxy A71", "grant_remaining_secs": null, "in_session": true,
              "key_bound": true, "internet_key_bound": true,
              "permissions": {"input": true, "clipboard": true, "files": true, "audio": true,
                              "mic": true, "power": true, "exec": false,
                              "notifications": true, "texts": true, "links": true, "media": true, "cast": true, "tasks": true,
                              "ring": true, "mute": true, "away_lock": false, "phone_sound": false},
              "battery": {"charge": 54, "charging": false, "updated_unix": 1792069000, "storage_free_mb": 12300, "storage_total_mb": 128000, "network": "wifi", "signal": 3, "ringer": "normal"},
              "media": {"title": "Song", "artist": "Artist", "app": "Spotify", "isPlaying": true,
                        "canPlayPause": true, "canNext": true, "canPrevious": true},
              "quiet": {"quiet": true, "until_unix": 1792072600, "by_computer": true, "updated_unix": 1792069000},
              "switches": {"notifications": {"on": true, "android": true}, "links": {"on": true, "android": false}},
              "switches_unix": 1792069000}]}
```

`name` is the owner's rename in the desktop app's Devices page, else the name the phone gives itself (cleaned: one short printable line), or null until it has connected since this version. `grant_remaining_secs` is null for a lasting pairing and counts down for one made with `pair --grant`. `key_bound` says the phone has proved its key on this network; `internet_key_bound`, from outside. Each permission is what the phone may do now, defaults included. `battery` is the phone's last report, sent during sessions and while a background feature keeps it linked (null if it never reported); `updated_unix` says how old it is. Beside the battery, what the phone said of itself (L9), each key left out when it did not say (a KDE Connect device, an older phone): `storage_free_mb` and `storage_total_mb` (decimal MB, the free rounded down to 100), `network` (`wifi`, `mobile`, `ethernet` or `none`), `signal` (0 to 4 bars) and `ringer` (`normal`, `vibrate` or `silent`). `media` is what the phone plays, as it says during a session with "My media on the computer" on, or while linked with "My media on the computer in the background" on (null when nothing plays, or the phone is not connected, has Media off, or the server is stopped); `background` says the second, and `artId` names the cover kept with it (N2). `switches` is which of its features the phone has on, as it last said (L7): for each feature, its own switch (`on`) and whether Android lets it work (`android`, left out where Android needs nothing); null until it has said, with `switches_unix` when it did. `quiet` is the phone's Do Not Disturb as it last said, with "Let the computer mute this phone" on, while it keeps a link to the computer (null otherwise: an old report is not now's); `until_unix` and `by_computer` are set only for a mute a computer asked for (D2).

### `pocketdesk sent --json` and `pocketdesk received --json`

The desktop app's Files tab: the last 20 (`--count N` for another number; the lists keep 50), newest first.

```json
{"api": 1, "sent": [{"id": "8b4b2722-…", "name": "Trip/", "path": "C:\\Users\\me\\Trip", "size": 8030000,
                     "device_id": "0d7c3a52-…", "sent_unix": 1792054206,
                     "state": "sent", "reason": null, "note": "1 left out: names the phone cannot take"}]}
{"api": 1, "received": [{"name": "photo.jpg", "path": "C:\\Users\\me\\Downloads\\photo.jpg", "size": 2100000,
                         "received_unix": 1792054100, "from": "0d7c3a52-…", "still_there": true}]}
```

A sent row's `state` is `offered` (not taken yet), `sent` (saved there: on the phone, the other computer or the KDE Connect device) or `failed` (with `reason`). A folder's name ends in `/`, and its `note` says what it left out.

### `pocketdesk sessions --json`, `pocketdesk capabilities --json`, `pocketdesk list --json`, `pocketdesk relay-ids --json`

- `sessions`: the recorded sessions, one object per line, with their measurements.
- `capabilities`: what this build speaks (`docs/capabilities.md` is made from it).
- `list`: the tailnet's peers, `{"api": 1, "peers": [{"name", "dns_name", "online", "ips"}]}`.
- `relay-ids`: the ids a relay elsewhere should allow, `{"api": 1, "ids": [{"who", "id"}]}`.

`pocketdesk bench --json` prints the encoder benchmark's record, a contract of its own for CI (exit codes in `pocketdesk bench --help` and README).

### `pocketdesk events [--since-start] --json`

What Pocketdesk does as it happens, one object per line, until stopped: the server's event log (`<state>/events.jsonl`, owner-only). `--since-start` prints the events already in the log first.

```json
{"api": 1, "at": 1792050000, "event": "file_received", "device_id": "0d7c3a52-…", "size": 30412}
```

Every event has `at`, `event` and `device_id` (the phone, or null). The kinds and their fields:

| `event` | Fields | With "Details in events" also |
|---|---|---|
| `session_started` | `session_id`, `transport` (`lan`/`wan`), `mode` (`screen`, `remote`, `cast`, `sound`, `phone_sound`) | — |
| `session_ended` | `session_id` | — |
| `phone_linked`, `phone_unlinked` | — (any connection the phone holds to this computer, a brief one for a single send included) | — |
| `phone_battery` | `charge`, `charging` | — |
| `phone_quiet` | `quiet` | — |
| `away_locked` | `failed: true` when the lock failed and will be tried again in a minute (the phone with "Lock this computer when it leaves" on that left last; D9) | — |
| `phone_battery_low` | `charge` | — |
| `notification` | `app`, and `late_secs` (seconds) for one that waited on the phone a minute or more (C3); `true` flags for why it was held or what it is: `alarm` (an alarm, P5), `phone_in_use` (the phone was in hand, M6), `in_use_elsewhere` (another computer was in use, N7), `quiet_hours` or `presenting` (held here, P3), `live` (an ongoing update, N4) | `title`, `text` |
| `text_received` | — | `from`, `body` |
| `file_received` | `size`; or `folder: true`, once, as a folder starts arriving; `key`, the first 16 hex digits of the SHA-256 of where it was saved (its `path` in `received --json`) | `name` (`folder/` for a folder) |
| `link_received` | `host` | `url` |
| `session_refused` | — (not paired, denied by you, or without the permission it needs) | — |
| `phone_paired`, `phone_unpaired` | — (however it was done: the PIN, the desktop app, `pocketdesk unpair`, or a guest grant ending) | — |
| `task_started` | `task` (its name) | — |
| `task_finished` | `task`, `code` (its exit code, or null), `timed_out`; never its output | — |
| `task_refused` | `task`, `reason` (`unknown`, `busy`, `denied`) | — |
| `deck_pressed` | `kind` (`keys`, `task`, `media`, `link`, `text`), `ok` (whether it ran; F1) | `label` |
| `hook_ran` | `hook` (the event it ran for), `code` (its exit code, or null when it could not start or a signal ended it), `timed_out`. It cannot have a hook of its own (that would run at every hook's end) | — |

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

Sends a file or a folder to a connected phone, exactly as the desktop app's Files tab does, so the server's checks apply: the phone is paired, may take files and is connected; a folder leaves out hidden files and stops at its limits. Connected means holding a control link: a screen or sound session keeps one (a remote alone does not), as does the phone's app open or a background feature (T10d): on this network, or by the internet road while `internet_files` is on, where the file rides that link (2c). A phone not connected so is refused at once (exit 1), and one linked only by the internet road while `internet_files` is off is refused with that reason. The target can also be another of your computers paired in Settings → Other computers (2h), by its id, 6 or more of its first characters, or its name: a file goes on the link to it while the link is up (refused at once otherwise), and lands in its Downloads; a folder does not go to a computer yet.

```json
{"api": 1, "id": "8b4b2722-…", "device_id": "0d7c3a52-…", "path": "C:\\Users\\me\\report.pdf",
 "state": "queued", "reason": null, "note": null}
```

Without `--wait` it ends once the request is written for the server to take (`queued`); a server that stops before taking it drops it after a minute. With `--wait`, it waits up to that many seconds. Once the server has taken the request, `state`, `reason` and `note` are the sent list's (`offered` while the phone has not saved it yet), and the exit code is 0 when the phone saved it, 1 when it failed, 4 when time ran out, and 3 when the server stopped while waiting. A send refused before it was queued has `"state": "refused"`, with the same text in `reason` and `error`.

```sh
pocketdesk send 0d7c3a backup.tar --wait 600 --json || notify-send "backup did not reach the phone"
```

Several phones or computers, with commas between (`pocketdesk send 0d7c3a,Desk report.pdf`), each get it as its own send (P6): `{"api": 1, "sends": [{"to": "0d7c3a", "state": "queued", …}, {"to": "Desk", …}]}`, each as one send's object without `api`, with `to` the name given. `--wait` waits for each in turn. The exit code is the worst of them: bad arguments or no server first, then a failure, then a wait that ran out; when one did not go, the object also says so in `error` ("1 of 2 did not go"). A device named twice (`0d7c3a,0d7c3a52-…`) gets it once, and a name that is one whole (a computer called "Home, office") is never split.

### `pocketdesk link <phone> <url> --json`

Hands a web link to a phone in a session; it waits there as a notification that opens it (T8a). Only `http` and `https` links of at most 2048 bytes, with no user name or password, and not to the phone's own `localhost`. `{"api": 1, "id": "…", "device_id": "…", "url": "…", "state": "queued"}`. Exit 2 for a link that may not be opened, 1 for a phone not in a session or with its Links permission off, 3 when no server runs.

### `pocketdesk call <phone> <number> --json`

Has a phone dial a number (L4): its dialer opens with it, and the owner presses Call there (the phone takes it with "Calls from the computer" on). The number may have spaces, dashes, dots and brackets, which are dropped; what is left is digits, a `+` only at the start, 3 to 32 characters (never `*` or `#`, which make codes). `{"api": 1, "id": "…", "device_id": "…", "number": "+15550100100", "state": "queued"}`; it goes to the phone if it is connected within a minute. Exit 2 for a number it is not, or a paired computer; 1 for a phone with "Call numbers from here" off; 3 when no server runs.

### `pocketdesk phone-command <phone> <name> [text] --json`

Sends a command to a phone's automation apps (L6): Tasker, Automate or MacroDroid hear `dev.pocketdesk.android.event.COMMAND` with `name` and `text`, with "Commands from the computer to my automation apps" on there. The name is 1 to 64 printable characters; the text at most 4096 bytes. `{"api": 1, "id": "…", "device_id": "…", "name": "…", "state": "queued"}`; it goes to the phone if it is connected within a minute. Exit 2 for a name or text out of bounds, or a paired computer; 1 for a phone with "Send commands to its automation apps" off; 3 when no server runs.

### `pocketdesk ring <phone> --json`

Asks the server to ring the phone (N1): `{"api": 1, "id": "…", "device_id": "…", "state": "queued"}`. Refused at once (exit 1) when the phone's Ring permission is off or it keeps no link now: it rings in a session, or with "Ring from the computer" on in its own Settings. The server sends at most one ring a minute to a phone, and drops a ring not sent within a minute.

### `pocketdesk quiet <phone> [--minutes N | --off] --json`

Asks the server to turn on the phone's Do Not Disturb for `N` minutes (1 to 1440, 60 by default), or with `--off` to end the mute this computer asked for (D2): `{"api": 1, "id": "…", "device_id": "…", "minutes": 60, "state": "queued"}` (`minutes` 0 for `--off`). Refused at once (exit 1) when the phone's Mute permission is off or it keeps no link now: it is muted with "Let the computer mute this phone" on in its own Settings. The phone keeps the owner's own Do Not Disturb settings; it ends the mute only while it is still the one it set. `devices --json` shows what each phone last said in `quiet`. Exit 2 for an unknown phone or minutes outside 1 to 1440 (`--off` asks for the end; `--minutes 0` is a usage error), 3 when no server runs.

### `pocketdesk pause [--for MINUTES | --until-resumed | --resume] --json`

Pauses what the phones bring to this computer (K2a) for `MINUTES` (1 to 10080), until resumed, or ends the pause; with no flag, says whether it is paused. While paused, phones' notifications are not shown (they stay on the phone, and those that can be answered wait on the Notifications page), texts wait on the Messages page, links are not opened and not kept (Activity names their site), and a phone ringing this computer stays silent; sessions, files and the clipboard go on. `{"api": 1, "paused": true, "since_unix": 1792069000, "until_unix": 1792072600}` (`until_unix` null until resumed), or `{"api": 1, "paused": false}`. It needs no running server: the server reads the pause at each thing it holds. Exit 1 when the pause cannot be read or written (the server then holds nothing), 2 for bad arguments.

### `pocketdesk clipboard [--send N --to <phone>] --json`

The clipboard's history (D3), with Settings "Clipboard history" above 0: the texts that crossed between this computer and its phones, either way, newest first: `{"api": 1, "entries": [{"text": "…", "from": "0d7c3a52-…", "at_unix": 1792069000}]}`. `from` is the phone or computer it came from, or null for this computer; an entry that was longer than 64 KiB is kept cut and says `"cut": true`. With `--send N --to <phone>` the Nth entry (1 the newest) goes to that phone's clipboard: `{"api": 1, "id": "…", "device_id": "…", "state": "queued"}`, refused at once (exit 1) when its Clipboard permission is off or it keeps no link now. Exit 2 for an entry or phone that is not there, 3 when no server runs (for `--send`).

### `pocketdesk kick <target> --json`

Ends live sessions: a device id, an id prefix, a peer address, or `all`. `{"api": 1, "kick_requested": "all"}`. Exit 3 when no server runs.

### `pocketdesk pair [PIN] [--grant 15m] [--someone-else] --json`

Makes a pairing PIN, valid for one phone: `{"api": 1, "pin": "123456", "valid_secs": 300, "grant_secs": null, "someone_else": false}`. The PIN is a secret for 5 minutes; do not log it. With `--grant` (a guest's phone, for that long) or `--someone-else` (for good), the phone pairs with files and links only, and its sessions ask first; `someone_else` is then true.

### `pocketdesk unpair [DEVICE-ID] --json`

Removes a paired phone, or every one when none is named: `{"api": 1, "unpaired": ["0d7c3a52-…"]}`.

## The phone

Automation apps on the phone (Tasker and the like) use Android intents: connect, wake, text to a computer's clipboard, files to it. `docs/phone-automation.md` lists them.
