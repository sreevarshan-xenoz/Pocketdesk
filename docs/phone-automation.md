# Automating the phone app

Automation apps on the phone (Tasker, MacroDroid, Automate and the like) can drive Pocketdesk with Android intents. These are the entry points the launcher shortcuts, the Quick Settings tile and the share sheet use, and any app on the phone can start them. Every intent acts only on a computer already saved in the app; a computer that is not saved gets nothing.

The computer's address is the one shown on its card on the app's home screen (for example `192.168.1.20`, or a name such as `mypc.local`), exactly: the same letters and case, with no port. It goes in the extra `dev.pocketdesk.android.extra.ADDRESS`; an empty or unexpanded one (a Tasker variable left as `%ADDR`) is refused.

On the computer, scripts use the local API instead: `docs/local-api.md`.

## Connect to a computer

| | |
|---|---|
| Target | Activity `dev.pocketdesk.android/.MainActivity` |
| Action | `dev.pocketdesk.android.action.CONNECT` |
| Extra | `dev.pocketdesk.android.extra.ADDRESS` = the saved computer's address |

Opens the app and starts a session with that computer, in place of any session already open. Relaunching it from Recents does not connect again.

## Wake a computer

| | |
|---|---|
| Target | Activity `dev.pocketdesk.android/.MainActivity` |
| Action | `dev.pocketdesk.android.action.WAKE` |
| Extra | `dev.pocketdesk.android.extra.ADDRESS` = the saved computer's address |

Sends the Wake-on-LAN packets the computer's Wake settings describe. The computer must have taught the phone how to wake it (a session while it was awake), or have a MAC address set by hand; otherwise the app says it does not know how to wake it yet. Away from the computer's network, only an outside address and port or a relay computer set in its Wake settings can reach it.

## Put text on a computer's clipboard

| | |
|---|---|
| Target | Activity `dev.pocketdesk.android/.ShareActivity` |
| Action | `android.intent.action.SEND` |
| Type | `text/plain` |
| Extras | `android.intent.extra.TEXT` = the text; `dev.pocketdesk.android.extra.ADDRESS` = the computer (optional) |

Without the address, the text goes to the last computer used. Naming a computer needs **Settings → Let other apps choose a computer**, off by default: choosing where a share goes is more than the share sheet lets an app do. A computer chosen in the system's share sheet (its Direct Share row) is the person's own choice and needs no switch: its shortcut id carries a token only the system and the launcher can read, so no app can forge it. With it off, a share that names a computer is refused, never sent to another one.

## Send files to a computer

| | |
|---|---|
| Target | Activity `dev.pocketdesk.android/.ShareActivity` |
| Action | `android.intent.action.SEND` (one file) or `android.intent.action.SEND_MULTIPLE` |
| Type | the file's type, or `*/*` |
| Extras | `android.intent.extra.STREAM` = the file's `content://` URI, with read permission granted; `dev.pocketdesk.android.extra.ADDRESS` = the computer (optional, as for text) |

The files land in the computer's Downloads (or the folder chosen in its Settings), each resumable, as a share from any app does. The automation app must grant read access to the URI (Tasker: tick "Grant read permission"). Only `content://` URIs are taken: a share naming a `file://` path is refused, since the app would open it with its own rights.

## Start a computer's task, or press a deck button

| | |
|---|---|
| Target | Broadcast to `dev.pocketdesk.android/.automation.AutomationReceiver` |
| Action | `dev.pocketdesk.android.action.RUN_TASK` (a task), or `dev.pocketdesk.android.action.PRESS_DECK` (a deck button) |
| Extras | `dev.pocketdesk.android.extra.ADDRESS` = the saved computer's address; `dev.pocketdesk.android.extra.NAME` = the task's name, or the button's label, exactly as the Tasks sheet or the deck lists it |

Starts one of the computer's named tasks, as the Tasks sheet does, or presses one of its deck buttons, as the deck does, with no app on screen. The name is at most 64 characters, matched exactly; of two buttons with one label, the first is pressed.

- **The switch.** It needs **Settings → Let other apps run the computer's tasks**, off by default, since any app on the phone can send it. With it off, nothing is sent, said or read beyond the log line.
- **Limits.** At most 10 a minute, from all apps together.
- **The computer's rules still hold:** the phone's "Start my tasks" and Input permissions, and a task that asks first still asks at the desk.
- **A task rides the phone's link to the computer.** With no link, one is brought up only to the phone's own computer (the one its background features work with); a task for another saved computer needs a session open with it (`not_linked`), and while the phone is linked to one computer, a task for another is not sent (`linked_elsewhere`). A deck button needs no link.
- **Android's ten seconds.** A cold app asked over the internet road may take longer than Android gives the broadcast, and Android may then stop Pocketdesk before the task starts; keeping a background feature on avoids it.
- **The outcome** comes back as `TASK_FINISHED` or `DECK_PRESSED` (below) with **Settings → Tell other apps what happens** on, and in `adb logcat -s Automation`.
- **How long a task is followed.** A task's end is followed while Android keeps Pocketdesk running, which it may not for long without a session or a background feature.

## What to expect

- **No result comes back** to the automation app for the intents above, except tasks and deck buttons (their events). Pocketdesk shows the outcome as a toast or its transfer notification.
- **A computer that is not saved**, named in the address extra, is refused with "No saved computer has that address"; with the setting off, with "Other apps may not choose a computer". The share never goes to another computer instead.
- **Android may block** an activity started from an app in the background. Automation apps usually need "Display over other apps" (Tasker asks for it) to start one from the background.

## Examples

Tasker, task action *Send Intent*: Action `android.intent.action.SEND`, Mime Type `text/plain`, Extra `android.intent.extra.TEXT:%CLIP`, Extra `dev.pocketdesk.android.extra.ADDRESS:192.168.1.20`, Package `dev.pocketdesk.android`, Class `dev.pocketdesk.android.ShareActivity`, Target *Activity*. This puts the phone's clipboard on that computer.

From a computer with adb, the same thing:

```sh
adb shell am start -n dev.pocketdesk.android/.ShareActivity \
  -a android.intent.action.SEND -t text/plain \
  --es android.intent.extra.TEXT "hello" \
  --es dev.pocketdesk.android.extra.ADDRESS 192.168.1.20
```

## What Pocketdesk tells other apps

With **Settings → Tell other apps what happens** on (off by default), Pocketdesk sends an Android broadcast for each of these. An automation app reacts to it with an "Intent Received" event (Tasker) or trigger (MacroDroid, Automate) for the action.

| Action | When | Extras besides the address and kind |
|---|---|---|
| `dev.pocketdesk.android.event.SESSION_STARTED` | A screen session's first picture arrived (after the owner allowed it, on a computer that asks), or a remote or sound session was admitted | `MODE`: `screen`, `remote` or `sound` (N5: the phone as the computer's speaker and mic) |
| `dev.pocketdesk.android.event.SESSION_ENDED` | It ended | `REASON`: `left` (you left it) or `lost` (the link went down, or the computer ended it) |
| `dev.pocketdesk.android.event.FILE_RECEIVED` | A file from the computer is saved (Downloads/Pocketdesk, or the folder picked in Settings); one per file of a folder | `MIME`, `SIZE` (bytes, as the computer gave it) |
| `dev.pocketdesk.android.event.FILES_SENT` | A share or folder to the computer finished | `COUNT` (sent), `FAILED` (meant to be sent and not) |
| `dev.pocketdesk.android.event.LINK_RECEIVED` | A link from the computer was shown during a screen session, with "Links from the computer" on | none |
| `dev.pocketdesk.android.event.TASK_FINISHED` | A task started from this phone ended, even after its sheet closed, or another app's `RUN_TASK` (O1) ended or was refused | `TASK` (its name), `STATE` (`done`, `refused`, `not_sent`, `no_answer`), and `CODE`, `TIMED_OUT` or `REASON` (the computer's `denied`, `busy`, `unknown`, `not_permitted`; the phone's `not_saved`, `bad_name`, `too_many`, `linked_elsewhere`, `not_linked`) |
| `dev.pocketdesk.android.event.DECK_PRESSED` | Another app's `PRESS_DECK` (O1) was pressed, or why not | `LABEL`, `STATE` (`done`, `waiting` for the owner, `refused`, `not_sent`), and `REASON` when refused (the computer's words, `unknown` for no such button, or the phone's `not_saved`, `bad_name`, `too_many`) |

Every extra is named `dev.pocketdesk.android.extra.<NAME>`. Each broadcast carries `dev.pocketdesk.android.extra.ADDRESS`, the computer's saved address, which the intents above take back, and `dev.pocketdesk.android.extra.EVENT`, the kind in lowercase (`session_started`…).

**Any app on the phone can receive these**, as with any broadcast that names no app: an automation app cannot hold a permission of Pocketdesk's. So they say only what happened, with which computer, and the facts in the table: never a file's name, a link, a task's output or a notification. **Any app can also send them**, so an automation must not take one as proof (a forged `TASK_FINISHED` with `CODE` 0 is possible). `adb logcat -s PhoneEvents` shows each one Pocketdesk sends.

Also worth knowing: `no_answer` means the phone stopped waiting (75 s for a task that asks first, else 20 s), not that the task never ran; a session the app is swiped away from, or that the phone kills, may get no `SESSION_ENDED`; and turning the switch off mid-session tells nothing more, its end included.

### Examples

Tasker, task action *Send Intent*: Action `dev.pocketdesk.android.action.RUN_TASK`, Extra `dev.pocketdesk.android.extra.ADDRESS:192.0.2.20`, Extra `dev.pocketdesk.android.extra.NAME:Backup`, Package `dev.pocketdesk.android`, Class `dev.pocketdesk.android.automation.AutomationReceiver`, Target *Broadcast Receiver*. This starts the computer's task "Backup". From a computer with adb:

```
adb shell am broadcast -n dev.pocketdesk.android/.automation.AutomationReceiver \
  -a dev.pocketdesk.android.action.PRESS_DECK \
  --es dev.pocketdesk.android.extra.ADDRESS 192.0.2.20 \
  --es dev.pocketdesk.android.extra.NAME Mute
```

Tasker, profile *Event → System → Intent Received*: Action `dev.pocketdesk.android.event.FILE_RECEIVED`. The task gets each extra as a local variable named after it (Tasker's variable list in the task shows the exact names, which depend on its version), so a test of the `MIME` extra against `image/*` can open the gallery. The same with `SESSION_ENDED` and a test of `REASON` for `lost` can say "the computer went away". Not yet tried with Tasker itself (checklist §83).

## Commands from the computer

With **Settings → Commands from the computer to my automation apps** on (off by default), a script on the computer can hand the phone's automation app a command: `pocketdesk phone-command <phone> <name> [text]` on the computer (the local API, `docs/local-api.md`). The phone broadcasts `dev.pocketdesk.android.event.COMMAND` to that app only, with:

| Extra | What |
|---|---|
| `dev.pocketdesk.android.extra.NAME` | The command's name, 1 to 64 printable characters |
| `dev.pocketdesk.android.extra.TEXT` | Its text, at most 4096 bytes (may be empty) |
| `dev.pocketdesk.android.extra.ADDRESS` | The computer's saved address |

It goes to the app "Only to" names in Settings, among Tasker, Automate and MacroDroid as found on the phone (the first found by default); with none of them found it goes nowhere. Only the phone's own computer is heard, at most 30 a minute. **Another app can send the same broadcast to your automation app**, which cannot tell it from Pocketdesk's: a command must not unlock anything an app on the phone should not. An automation app that was force-stopped, or never opened, gets nothing until it runs.

### Example

Tasker, profile *Event → System → Intent Received*: Action `dev.pocketdesk.android.event.COMMAND`. In the task, test the `NAME` extra (`lights off`) and use `TEXT`. On the computer: `pocketdesk phone-command 5f1c9b "lights off"`. Not yet tried with Tasker itself (checklist §161).

