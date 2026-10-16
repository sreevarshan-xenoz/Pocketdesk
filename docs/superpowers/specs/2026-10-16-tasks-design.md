# Named tasks (ecosystem phase 1c)

Part of the ecosystem plan (`docs/research/2026-10-16-ecosystem.md`, phase 1c, from the owner's second opinion). A phone should be able to start a job on the computer (a backup, a build, a script) and see how it went. Today the only way is the Exec plugin, which runs *any* command a phone with Exec sends. It has no deadline and no output cap, which is why it is off by default.

## What it does

- **On the computer,** the owner names tasks in Settings (`[[tasks]]` in config.toml). Each task has:
  - a name;
  - the command line, run through the system shell as the owner;
  - "ask first", on by default: the desktop app asks Allow / Deny before the task runs, and no answer within 60 s is a deny;
  - a deadline, 60 s by default, at most 1 hour.
- **On the phone,** "Tasks" in a session's Quick Settings and on the remote's screen lists the computer's task names (never their commands). Tapping one runs it and shows the result: exit code, output (the last 64 KiB of stdout and stderr together), and whether it was stopped at its deadline.
- **Permission:** a new per-phone **Tasks** permission. It is on by default, as every permission but Exec is, and the list is empty until the owner adds a task. A guest grant gets it too, and "ask first" is the guard.
- **Events:** `task_started` (`task`) and `task_finished` (`task`, `code`, `timed_out`), with the output never in the event. `task_refused` (`task`, `reason`: denied, unknown, busy) also appears in the Activity Center.
- **At most 2 tasks at once,** and one run of the same task at a time. A start beyond that is refused as `busy`.
- **Exec gets the same limits:** a 60 s deadline, and the last 32 KiB of each of stdout and stderr.

## Wire

| Packet | Direction | Body |
|---|---|---|
| `kdeconnect.pocketdesk.tasks` | phone → computer | `{}`: what are your tasks? |
| `kdeconnect.pocketdesk.tasks` | computer → phone | `{"tasks": [{"name", "ask": bool}]}` |
| `kdeconnect.pocketdesk.task_run` | phone → computer | `{"name", "id"}` (the phone's id for this run, at most 64 printable characters) |
| `kdeconnect.pocketdesk.task_result` | computer → phone | `{"id", "state": "running" \| "done" \| "refused", "code", "output", "truncated", "timed_out", "reason"}` |

All four are under the Tasks capability. `running` is sent once the task starts (after the owner allows it, when it asks first), and `done` when it ends.

## The owner's answer

A task that asks first is listed in `<state>/task_requests.json` (owner-only; the server is its only writer and rewrites it whole, and clears it at start). The desktop app shows it, as a held session is shown, with Allow and Deny, and answers through `task_answers/<id>.json`. The server reads the answer and deletes both files; a request older than 60 s is a deny.

## Checks

- **Rust:**
  - the config (`[[tasks]]`, saved and read back);
  - the runner: deadline, output cap, the last bytes kept, one run per task, two at once;
  - the request and answer files, with expiry;
  - the plugin: refusing an unknown name, a phone without Tasks, a denied ask;
  - Exec's new deadline and cap.
- **Kotlin:** the result's parsing and wording.
- **Device (A71 → Windows):** a task listed and run, its output on the phone; ask-first with Allow and Deny; a task over its deadline; Tasks off refusing; the Activity rows.

## As built

- **Hostile or chatty phones:** a phone may ask for 10 starts a minute; a refusal logs the task's name only when it is one of the owner's; a task hidden from the list (no command, a bad name) cannot be started by name.
- **Output** is read as it comes, keeping the last bytes of each stream (never all of it in memory); a run stopped at its deadline keeps what it wrote. Programs a task starts in the background may outlive it.
- **Results** go to the connection that asked while it is up, else the phone's newest, within 10 s; the phone sends a start only on its live control link, whose reader keeps the results.
- **The phone's sheet** shows a start that could not be sent, and gives up on one nothing answered after 20 s (75 s when it asks first): a phone without Tasks gets no reply, as for every gated packet.
- **Device run (A71 → Windows):** checklist §80.
