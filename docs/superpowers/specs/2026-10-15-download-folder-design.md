# Download a folder from the remote browser (T6b, the part left out)

Date: 2026-10-15. Status: design, built on autopilot (the user asked for features to keep going; decisions are mine).

## Why

The phone's remote browser ("Files on this computer") opens folders and downloads files one at a time. T6b made folders travel both ways, but only when the computer's owner sends one, or the phone sends one. A person browsing their computer from the phone still has to tap every file of a folder. The T6b as-built section names this as not built.

## What

A browsed folder has "Download this folder". The phone gets the whole folder, in its own new folder in `Download/Pocketdesk`, the way a folder the computer sends arrives (T6b): each file resumable, hidden entries left out, the same limits.

## How

1. **Phone → desktop:** a new control packet `kdeconnect.pocketdesk.folder_fetch` `{path}`, the folder's path as the browser shows it (the same form `file_fetch` names a file by).
2. **Desktop:** under the **Files** permission (`capability_for`; an unlisted type would be always allowed, so a test holds it there). The path is resolved by the browser's own rules (`file_browse::resolve_in`: inside home, nothing hidden on the way, not the server's state or config, no device names, no links out), and must be a folder.
3. The desktop then writes an ordinary outbox request for **the phone that asked** (`core::local::outbox::request`), exactly what the desktop app writes when its owner picks a folder. The outbox watcher takes it, and T6b's folder offer does the rest: the phone and its Files permission are checked again, the files are walked with hidden entries, links and the server's own folders left out, offered one by one with `folderId`/`folder`/`path`, and the sent list gets one row for the folder, with its note.
4. **Answer:** every request is answered with `kdeconnect.pocketdesk.folder_fetch_reply` `{path, queued, reason}`: `queued: true` once the outbox request is written, else `false` and why. The phone never waits on silence: the bridge sends it as a request and waits for the reply of that type (`control_request`).
5. **Phone:** the browser's folder view gets a "Download this folder" row above its entries (not on home itself: home holds too much to mean). Tapping it sends the request and says "Asked the computer for <folder>". The offers then arrive through the session's existing offer path (`FileOffersEffect` → `TransferService`), so progress, retries and the finished notification are the ones T6b already has. A refusal shows as a snackbar with the desktop's reason.

## Why through the outbox

- One path for every folder the desktop sends: the same checks, walk, limits, offers and sent-list row, already reviewed and device-checked.
- The sent list (the desktop app's Files tab) shows that the phone took a folder, as it does for files the owner sends. The owner can see what left the computer.
- Nothing new is reachable: a phone allowed Files can already download any file of home by path; a folder is the same set of files, walked with stricter rules than browsing.

## Limits

- A phone may have at most two folder sends in progress (counted by the offer module's folder groups, which learn the phone); a third request is refused as busy. Each folder is held to T6b's limits (10 000 files, 32 levels, 32 GiB), so a paired phone cannot fill the desktop's offer table by asking again and again.
- Home itself is not offered as a folder (the phone hides the row there; the desktop refuses it too).

## Tests

- core: `folder_fetch` maps to Files.
- server: a third folder request while two of that phone's are in progress is refused as busy.
- server: a folder request inside home queues an outbox request for the asking phone; a file, home itself, a hidden folder, the state folder, a path outside home and a missing path are refused with a reply naming the path and why; nothing is queued for a refused one, and an accepted one is answered `queued`.
- bridge: `nativeFetchFolder` returns the desktop's answer: queued, or its reason; no answer is an error.
- Kotlin: the row is shown for a folder and not for home.
- Device: download a three-level folder from the browser; MD5s; the desktop's Files tab row.
