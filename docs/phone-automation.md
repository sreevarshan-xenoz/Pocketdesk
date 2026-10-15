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

Without the address, the text goes to the last computer used. Naming a computer needs **Settings → Let other apps choose a computer**, off by default: choosing where a share goes is more than the share sheet lets an app do. With it off, a share that names a computer is refused, never sent to another one.

## Send files to a computer

| | |
|---|---|
| Target | Activity `dev.pocketdesk.android/.ShareActivity` |
| Action | `android.intent.action.SEND` (one file) or `android.intent.action.SEND_MULTIPLE` |
| Type | the file's type, or `*/*` |
| Extras | `android.intent.extra.STREAM` = the file's `content://` URI, with read permission granted; `dev.pocketdesk.android.extra.ADDRESS` = the computer (optional, as for text) |

The files land in the computer's Downloads, each resumable, as a share from any app does. The automation app must grant read access to the URI (Tasker: tick "Grant read permission"). Only `content://` URIs are taken: a share naming a `file://` path is refused, since the app would open it with its own rights.

## What to expect

- **No result comes back** to the automation app. Pocketdesk shows the outcome as a toast or its transfer notification.
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
