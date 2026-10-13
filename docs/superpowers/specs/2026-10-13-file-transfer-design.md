# File transfer, phone to desktop — design

Status: decided 2026-10-13, branch `feat/file-transfer`. Roadmap F7, first half ([feature roadmap](2026-10-12-feature-roadmap.md)). The second half, desktop to phone and the remote browser, is its own branch.

## Why

Sharing a file from the phone to the desktop works since `fix/share-safety`: the desktop connects back to the phone, presents a one-time key, and saves the file safely in Downloads. But using it is still poor:
- one file at a time;
- the whole file is first copied into the app's cache;
- no progress, and the result flashes for a moment while the activity finishes;
- nothing on the desktop says a file arrived, or where it went.

Every comparable tool (KDE Connect, Phone Link, Quick Share) shows progress on the phone and a notification on the desktop.

## Decisions (made autonomously; each is cheap to revisit)

| # | Decision | Why |
|---|---|---|
| T1 | The share target accepts `SEND_MULTIPLE`. Files go one after another to the saved computer the app used last, as now. | Sharing several photos is the common case. Sequential transfers stay within the desktop's two-transfer limit. |
| T2 | Sending runs in a foreground service (`dataSync`) with one notification. It shows "Sending 2 of 3: name" with a progress bar, then "Sent 3 files to …" or the failure. The share screen closes at once with a toast. | A transfer outlives the share screen, and Android kills background work. The notification is where Android users look for progress. |
| T3 | The bridge reads the file straight from the content provider's file descriptor (`ParcelFileDescriptor.detachFd`) when the provider reports a size. Only a provider with an unknown size is still copied to the cache first. | No double storage and no delay before the first byte. |
| T4 | The bridge exposes the running transfer's bytes sent and total (`nativeTransferProgress`). The service polls it twice a second. | No callback across JNI, and the same pattern as the session's stats. |
| T5 | The desktop raises a notification for each file received ("photo.jpg (2.1 MB) from your phone"), and appends it to `<state>/received.json` (last 50: name, path, size, time, sender). | The owner sees that something arrived, and from whom. |
| T6 | The desktop app gains a **Files** tab listing received files, newest first, each with **Show in folder**. | The list is where "where did it go?" is answered. It shows the folder, never opens the file: the file came from another device, and Windows marks it so. |

## As built (2026-10-13)

- **Queueing.** Shares made back to back queue on one worker thread in the service, not just files within a single share: the desktop accepts two transfers at once, and the progress counters describe one transfer. The final notification sums everything sent since the queue was last empty.
- **A share that arrives while the share screen is closing** comes in through `onNewIntent` and is handled. On the device, three shell intents fired within half a second still lost two to an activity pause timeout. Through the share sheet a person cannot do that; shares 1.5 s apart all went through.
- **Names.** The phone sends the provider's display name as is. The desktop's `safe_file_name` is what makes it safe; the phone no longer turns non-ASCII names into underscores.

## Out of scope (F7b)

- Sending from the desktop to the phone.
- The remote file browser and downloading from it.
- Transfers over WAN (iroh) or over `adb reverse`. The desktop must be able to connect to the phone's address.

## Testing

| Layer | Tests |
|---|---|
| core | `received.json`: append, keep the newest 50, read back; a missing or corrupt file reads as empty. |
| server | A received file is recorded with its final name and size. |
| bridge | Progress counters run from 0 to the total over a loopback transfer, and are reset for the next one. |
| desktop | The Files tab model lists entries newest first. |
| JVM | The notification text for 1 and for N files, and the progress percentage. |
| Device | Share three files from the gallery to the Windows host: notification progress on the phone, three toasts on the desktop, three rows in the Files tab, matching checksums. |
