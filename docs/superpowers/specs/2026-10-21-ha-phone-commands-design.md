# M9: Home Assistant buttons for phone commands (L6 part 2)

Plan: `docs/research/2026-10-21-m-cycle.md`, M9. L6 (`2026-10-21-phone-command-design.md`) sends a command to a phone's automation apps from `pocketdesk phone-command`. 1e (`2026-10-16-home-assistant-design.md`) announces this computer to Home Assistant over MQTT.

## Why

- **Asked for:** command buttons (KDE Discuss 2026-08-05). Join and AutoRemote exist for this.
- **Left over:** L6's spec left Home Assistant out.

## What

- **Desktop Settings "Phone commands"** (Automation; `phone_command` in config.toml, written as a list of tables as the deck's is, at most 32). Each has:
  - a label;
  - a phone, picked from the phones paired for good (not guests);
  - L6's name;
  - an optional text.
- **Home Assistant:** each command becomes a button under its phone's device, through 1e's discovery:
  - its object id is `cmd_<phone>_<label>`, with a hash of the whole command when two would share one, so removing one never hands its id to another;
  - it presses on `pocketdesk/<id>/command/<object>/run`.

  Only a live `PRESS` presses (1e's rule): a retained or empty message presses nothing.
- **A press:** sends L6's `kdeconnect.pocketdesk.command` through `commands_out/`, as `pocketdesk phone-command` does.
  - **On the computer:** held to the phone's Command permission, to the phone still being paired, and to 30 presses a minute for a phone. A refused press is said in the server's log.
  - **On the phone:** held to L6's rules ("Commands from the computer to my automation apps", the three apps).
- **Which commands become buttons:** only those whose phone is paired, and whose name and text L6 takes. A command removed from Settings has its button removed too (1e's empty retained message).

## Privacy and safety

- **Anyone who can press buttons in Home Assistant can trigger the phone's automations,** with the texts set here. SECURITY.md limit 44, beside 19 and 38.
- **What Home Assistant sees:** the labels, never the names or texts. The phone's id is shortened, as 1e's are.

## Tests

- **Config:** saved and read back.
- **Server:** the buttons' configs and topics; unique ids; only paired phones' commands, and only valid ones; a press naming one command and nothing else; a retained or empty message pressing nothing.
- **Desktop:** the editor's problems.

## On the device (checklist §174)

- A local broker as in 1e: a press published with `mosquitto_pub`, and the command seen at the OPPO's throwaway receiver in logcat.
- A retained message: nothing.
- The Command permission off: refused, in the server's log.
- Home Assistant itself, and Tasker (the owner).

## As built

- **Core:** `local::commands::PhoneCommandSpec` (`problem`, `MAX_PHONE_COMMANDS`); the config's `phone_command`.
- **Server** (`home_assistant.rs`):
  - `CommandButton`, `command_buttons`, `command_objects` (stable ids) and `pressed_command`;
  - the `…/command/+/run` subscription;
  - `press_command`, with `within_pace`.
- **Desktop:** Automation's "Phone commands" editor in `settings_view.rs` (`command_problems`), with the phones from `app.rs`.

## Review

A fresh review found 0 Critical, 3 Important and 6 Minor. The fixes are in one commit, with the docs in this one.

- **Important:**
  - ids by place, so removing a command handed its id, and the automations pressing it, to the next;
  - no pace on presses;
  - the docs.
- **Minor, fixed:**
  - guests in the picker;
  - a press for a phone unpaired since, and the log's "sent";
  - the subscription warning naming tasks only;
  - the spec's words on Activity and the config's form;
  - tests for removal and for ids that stay put.
