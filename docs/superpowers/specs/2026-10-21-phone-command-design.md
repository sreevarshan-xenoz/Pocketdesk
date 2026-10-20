# Commands to the phone's automation apps (L6, part 1)

**Date:** 2026-10-21. **Branch:** `feat/phone-command`. The L cycle's sixth item (`docs/research/2026-10-20-l-cycle.md`).

## What is wrong

- **The computer hears the phone's automation apps (1f), but cannot reach them.** A script on the computer cannot tell Tasker to do something.
- **Asked for:** KDE's "Intent Listener" request and bug 377850; Join's and AutoRemote's reason to exist.

## What changes

- **The computer sends:** `pocketdesk phone-command <phone> <name> [text] [--json]` queues a command (`<state>/commands_out/`, a minute to live), and the server sends `kdeconnect.pocketdesk.command {name, text}` to that phone if it is connected.
  - **The name:** 1 to 64 printable characters, none a control or an invisible format character.
  - **The text:** at most 4 KB.
  - **Its permission:** a per-phone "Send commands to its automation apps" (`Capability::Command`, on). It is about what the computer sends, as Ring is.
- **The phone broadcasts** `dev.pocketdesk.android.event.COMMAND` with the extras `dev.pocketdesk.android.extra.NAME`, `.TEXT` and `.ADDRESS` (the computer's saved address), as 1f names them, for Tasker, Automate or MacroDroid, through `CommandRule` (JVM-tested):
  - with Settings "Commands from the computer to my automation apps" on (off by default);
  - from its own computer only, at most 30 a minute;
  - in a session, and outside one while a link to the computer is up for another feature.
- **"Only to":** an automation app found on the phone (Tasker, Automate, MacroDroid), the first found by default. With one set, the broadcast goes to that package only, so no other app reads it. Android 8+ manifest receivers need an explicit package anyway.
  - With no automation app found, the command goes nowhere (a script's text may be a secret), and Settings says so.

## Left out

- Home Assistant's button for a command.
- Any app as "Only to" (it needs the right to list every installed app).

## Testing

- **Unit:** the name and text rule (both ends), the commands' outbox, the CLI's checks, the phone's rule (setting, own computer, budget).
- **On the device (checklist §161; L5 is §160):**
  - a throwaway receiver in a known package logs what it gets, read with `adb logcat`;
  - with "Only to" set to another app, nothing arrives.
- **Owed:** Tasker itself.

## As built

- **Core:** `core::protocol::command` (`COMMAND`, `valid_name`, `valid_text`), `core::local::commands` (the outbox), `Capability::Command`.
- **Server:** `links::run_commands` (watched inbox; a paired phone with Command on), `pocketdesk phone-command` (`local_api::command_in`, refusing a paired computer).
- **Bridge:** `commands.rs`, kept with its computer, dropped when the phone leaves it.
- **Phone:** `automation/CommandRule.kt` (JVM-tested), `automation/Commands.kt`, `ui/CommandsEffect.kt` (a session counter), the listener outside a session, Settings with "Only to" among the apps found, `<queries>` for the three.

## Review

A fresh review (0 Critical, 3 Important, 10 Minor), fixed one commit each:

- **Important:** `commands_out/` unwatched (a command waited up to 5 s; L1's and L4's outboxes had the same gap, fixed with them); with no automation app found a command went to any app, its text and the computer's address with it (now nowhere); the docs promised.
- **Minor, fixed:** a paired phone checked by the permission's default, a session flag the older of two sessions cleared, an unsaved computer's commands refused, the labels naming the command, the extras named unlike 1f's.
- **Left:** any app can send the same action to an automation app, which cannot tell who sent it (SECURITY.md limit 38); a stopped automation app gets nothing until opened; "Only to" rows are switches used as a choice.
