# What happens, told to the phone's automation apps (ecosystem phase 1f)

Part of the ecosystem plan (`docs/research/2026-10-16-ecosystem.md`, phase 1: "the same events as Android broadcasts"). `docs/phone-automation.md` lets Tasker, MacroDroid and Automate drive Pocketdesk (connect, wake, send). Nothing comes back: an automation cannot react to "the session ended" or "a file arrived". 1f closes that loop on the phone, as `pocketdesk events` (1a) does on the computer.

## What it does

- **Off by default.** Settings "Tell other apps what happens" (Automation section, beside "Let other apps choose a computer").
- **Broadcasts:** with it on, the app sends an Android broadcast for each of these:

  | Action (`dev.pocketdesk.android.event.…`) | When |
  |---|---|
  | `SESSION_STARTED` | A screen session's link is up (`mode` `screen`), or a remote was admitted (`remote`). |
  | `SESSION_ENDED` | It ended (`reason`: `left`, the screen was left; `lost`, the link went down). |
  | `FILE_RECEIVED` | A file from the computer was saved in Downloads. |
  | `FILES_SENT` | A share or folder to the computer finished (`count`, `failed`). |
  | `LINK_RECEIVED` | A link from the computer arrived. |
  | `TASK_FINISHED` | A task started from this phone ended (`task`, `state`, and `code` and `timed_out`, or `reason`). |

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
  - `PhoneEvents` (pure, JVM-tested): the action and extras for each event, no content field in any event; `SessionTell` (once each, no end without a start); `TransferSummary.failedOf`; `TaskRun.told`. The switch off sending nothing is in `PhoneEventBroadcast.send`, checked on the device;
  - the emit sites call it.
- **Device (A71):** no automation app is installed, and Android keeps no history of a broadcast no app received, so the debug log (`adb logcat -s PhoneEvents`, the kind and its extras) is the check, during a session start and end, an offer saved and a link, and with the switch off.

## As built

- **A session is started** once its first picture arrives (a computer that asks first holds it), a remote once admitted; a session not told started (the switch was off) is not told ended.
- **A task's end** is told even after its sheet closed: the run is followed from outside the sheet, to its end or until it is given up (`no_answer`).
- **FILES_SENT's `failed`** is the files the send meant to send and did not, a send cut short or one that threw included (`TransferSummary.failedOf`).
- **Review (0 Critical, 2 Important), fixed:** a closed sheet lost the task's end, and a cut-short send reported none failed; plus the start before approval, a lone end, the log line, the Settings wording, and the docs (spoofing, a folder's many `FILE_RECEIVED`, the declared size, where links are told, `no_answer`).

