# What happens, told to the phone's automation apps (ecosystem phase 1f)

Part of the ecosystem plan (`docs/research/2026-10-16-ecosystem.md`, phase 1: "the same events as Android broadcasts"). `docs/phone-automation.md` lets Tasker, MacroDroid and Automate drive Pocketdesk (connect, wake, send). Nothing comes back: an automation cannot react to "the session ended" or "a file arrived". 1f closes that loop on the phone, as `pocketdesk events` (1a) does on the computer.

## What it does

- **Off by default.** Settings "Tell other apps what happens" (Automation section, beside "Let other apps choose a computer").
- **Broadcasts:** with it on, the app sends an Android broadcast for each of these:

  | Action (`dev.pocketdesk.android.event.…`) | When |
  |---|---|
  | `SESSION_STARTED` | A session's first frame arrived (or an input-only session was admitted). |
  | `SESSION_ENDED` | A session closed, for any reason (`reason`: `left`, `lost`, `refused`). |
  | `FILE_RECEIVED` | A file from the computer was saved in Downloads. |
  | `FILES_SENT` | A share or folder to the computer finished (`count`, `failed`). |
  | `LINK_RECEIVED` | A link from the computer arrived. |
  | `TASK_FINISHED` | A task started from this phone ended (`task`, `code`, `timed_out`) or was refused (`reason`). |

- **Extras on each:**
  - `dev.pocketdesk.android.extra.ADDRESS`: the computer's saved address, as `docs/phone-automation.md` uses it, so an automation can answer the same computer;
  - `dev.pocketdesk.android.extra.EVENT`: the kind, in lowercase (`session_started`…);
  - the fields above, under `dev.pocketdesk.android.extra.<FIELD>`.
- **Metadata only:**
  - no file names, link addresses or task output;
  - a broadcast can be read by any app on the phone that asks for it;
  - a task's name is sent, since the automation chose it; a file's MIME type is sent, not its name.
- **Delivery:** plain `sendBroadcast` with no package: Tasker's "Intent Received" event and MacroDroid's "Intent Received" trigger register receivers at run time, which receive implicit broadcasts on every Android version. No permission is required to receive, since an automation app cannot declare one of ours.
- **Docs:** `docs/phone-automation.md` gains a "What Pocketdesk tells other apps" section and a Tasker profile example.

## Not in 1f

The computer's events relayed to the phone (they need a session or link, and the computer's own hooks already have them), a file's name or URI (privacy), and results returned to the app that started an intent.

## Checks

- **Kotlin:**
  - `PhoneEvents` (pure, JVM-tested): the action and extras for each event, the switch off sending nothing, no content field in any event;
  - the emit sites call it.
- **Device (A71):** a test receiver app is not at hand, so the check is `adb shell am monitor`… no: `adb logcat` with a debug log of each broadcast, plus `dumpsys activity broadcasts` history for the actions, during a session start and end, an offer saved and a link.
