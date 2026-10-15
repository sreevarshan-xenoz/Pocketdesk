# Automating the phone app

Automation apps on the phone (Tasker, MacroDroid, Automate and the like) can drive Pocketdesk with Android intents. These are the same entry points the launcher shortcuts, the Quick Settings tile and the share sheet use, so nothing here opens a door that was not already open. Every intent acts only on a computer already saved in the app; a computer that is not saved gets nothing.

The computer's address is the one shown on its card on the app's home screen (for example `192.168.1.20`, or a name such as `mypc.local`). It goes in the extra `dev.pocketdesk.android.extra.ADDRESS`.

On the computer, scripts use the local API instead: `docs/local-api.md`.

## Connect to a computer

| | |
|---|---|
| Target | Activity `dev.pocketdesk.android/.MainActivity` |
| Action | `dev.pocketdesk.android.action.CONNECT` |
| Extra | `dev.pocketdesk.android.extra.ADDRESS` = the saved computer's address |

Opens the app and starts a session with that computer.

## Wake a computer

| | |
|---|---|
| Target | Activity `dev.pocketdesk.android/.MainActivity` |
| Action | `dev.pocketdesk.android.action.WAKE` |
| Extra | `dev.pocketdesk.android.extra.ADDRESS` = the saved computer's address |

Sends the Wake-on-LAN packets the computer's Wake settings describe. The computer must have taught the phone how to wake it (a session while it was awake), or have a MAC address set by hand.

## Put text on a computer's clipboard

| | |
|---|---|
| Target | Activity `dev.pocketdesk.android/.ShareActivity` |
| Action | `android.intent.action.SEND` |
| Type | `text/plain` |
| Extras | `android.intent.extra.TEXT` = the text; `dev.pocketdesk.android.extra.ADDRESS` = the computer (optional) |

Without the address, the text goes to the last computer used.

## Send files to a computer

| | |
|---|---|
| Target | Activity `dev.pocketdesk.android/.ShareActivity` |
| Action | `android.intent.action.SEND` (one file) or `android.intent.action.SEND_MULTIPLE` |
| Type | the file's type, or `*/*` |
| Extras | `android.intent.extra.STREAM` = the file's `content://` URI, with read permission granted; `dev.pocketdesk.android.extra.ADDRESS` = the computer (optional) |

The files land in the computer's Downloads, each resumable, as a share from any app does. The automation app must grant read access to the URI (Tasker: tick "Grant read permission"); a `file://` path is refused by Android.

## What to expect

- **No result comes back** to the automation app. Pocketdesk shows the outcome as a toast or its transfer notification.
- **A computer that is not saved**, named in the address extra, is refused with "That computer is no longer saved."; the share never goes to another computer instead.
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
