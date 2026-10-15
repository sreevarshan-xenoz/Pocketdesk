# The local API (T7)

Date: 2026-10-15. Status: design, on autopilot (decisions are mine; the user asked for the roadmap to keep going).

## Why

People script their computers: a backup job that sends a file to the phone when it ends, a status bar that shows whether a phone is connected, a script that ends every session at night. Pocketdesk's CLI prints text for people. Only `sessions`, `bench` and `capabilities` print JSON, and nothing says what a script may rely on. The roadmap's T7 asks for `--json` on every verb, a documented owner-only interface, and Android intents for Tasker.

## What (T7a, this change: the desktop)

1. **`--json` on every command that reports something**, printing one JSON object (or JSON lines where a command already does) on stdout, logs on stderr as now:
   - `status --json`: the server (running, pid, version), live sessions, the internet endpoint, the Tailscale state if there is one.
   - `devices [--json]` (new): the paired phones, their permissions, whether a key is bound, and whether each is in a session now.
   - `sent [--json]` and `received [--json]` (new): the Files tab's two lists.
   - `list --json`, `relay-ids --json`; `sessions --json` and `capabilities --json` stay as they are.
2. **Commands that act**:
   - `send <phone> <path>` (new) asks the server to send a file or folder to a phone, exactly as the desktop app's Files tab does (an outbox request). It prints the request's id. `--wait` waits until the sent list says how it ended, and the exit code says it too.
   - `kick`, `unpair` and `pair` get `--json` for their result.
3. **One contract, in `docs/local-api.md`:**
   - each JSON object carries `"api": 1`;
   - within api 1, fields are only ever added, never renamed or removed;
   - exit codes: 0 done, 1 failed, 2 bad arguments, 3 no server running (for the commands that need one), 4 still waiting when `--wait` ran out;
   - `<phone>` may be a device id, a unique prefix of 6 or more characters, or the name the phone gave.

## Owner-only, by construction

There is no new listener: no socket, no HTTP port, nothing another user or another machine can reach. The API is this program, run as the user, reading and writing the state folder, which is already the user's own (0700 on Unix, the user's profile on Windows). `send` goes through the outbox the desktop app uses, so the server's checks apply (the phone is paired, Files is allowed, the size limits). Anything that can run this program as the user could already read those files.

## Later (T7b, the phone)

Tasker and other automation apps start `MainActivity` with the intents that already exist (`ACTION_CONNECT`, `ACTION_WAKE`, for saved computers only). T7b documents them and adds a few actions (send the clipboard, send a file), behind a Settings switch "Let other apps use Pocketdesk", off by default. It is a separate change.

## Tests

- JSON shape tests for each new or changed command, built from a temporary state folder (`POCKETDESK_STATE_DIR`), never the real one.
- `send` writes an outbox request the server's own reader takes; `--wait` ends on each sent state and on its deadline.
- Prefix and name matching: unique, ambiguous, unknown.
- The docs: `docs/local-api.md` lists every command with `--json`, held by a test like `capabilities_doc`.

## As built (2026-10-15)

- **Commands:** `devices`, `sent`, `received` and `send` are new; `status`, `list`, `pair`, `unpair`, `kick` and `relay-ids` gained `--json`. `sessions`, `capabilities` and `bench` keep their own formats, and the contract says so.
- **Not as designed:** a phone is named by its id or a unique prefix only (no phone name is stored anywhere); `status --json` carries no Tailscale state (`list --json` has the tailnet's peers); the version is `pocketdesk_version`, this program's, not the server's.
- **`send`** refuses at once a phone not in a session (exit 1), the server's own files, and anything when no server runs (exit 3); with `--wait` it reports the sent row's state from the moment there is one, and ends with exit 3 if the server stops.
- **Failures** with `--json` are `{"api":1,"error":…}` for usage errors (exit 2), a config.toml that does not parse (exit 1) and the command's own; `--json` on the command line decides it, so a new command cannot miss it.
- **The trust store** is read without being written (`TrustStore::read`), and an unreadable one is an error, never "no paired phones".
- **Fresh review (0 Critical, 4 Important), fixed one commit each:** JSON failures before the command ran, `send --wait` reporting `queued` for an offered file and waiting on a stopped server (and a huge `--wait` panicking), an unreadable store read as empty and created by a read, and a contract that overstated what each command prints. Minors fixed: the version field, `wants_json` by hand, refused sends on stdout, a send to a phone not in a session, a closed pipe panicking, the server's own files sent singly, and the doc test's substring match.
- **Left:** `kick` says it asked, not whether a session matched (the server only logs that).
- **Verified on this box with the A71:** `status --json` showing the LAN session, `devices --json` marking the phone in session, `send <prefix> <file> --wait 120` ending "sent" with exit 0 and the phone's MD5 matching; with no phone connected, `send` refused at once; with no server, exit 3.
