# New phone screenshots on the computer (T10h)

Research item 5 (`docs/research/2026-10-15-what-makes-it-worth-using.md`): Windows 11 offers a new phone photo in the Snipping Tool, and Phone Link has a Photos tab. The common case is a screenshot taken on the phone that is wanted on the computer a moment later: pasted into a chat or a document, or kept.

## What it does

- The phone's Settings gain "New screenshots to the computer" (off by default; it asks for photo access), with two choices under it, both on:
  - "On its clipboard": the screenshot goes as the phone's clipboard image does (N4, `clipboard_image`, PNG, at most 32 MiB). It needs the Clipboard permission on the computer.
  - "Saved in its Downloads": the file goes as a share does (F7a/T6a), resumable, noted in the received list, with the computer's "file received" notification. It needs the Files permission.
- Only to this phone's own computer (the last one it used, `PhoneLink.target`), as the other background features do. Both halves need the computer to dial back to the phone, so they work where it answers on its control port (at home, or over a tailnet), not over the internet road. While it does not answer, a screenshot waits, tried every 30 s; one older than 10 minutes is not sent.
- Only screenshots: images MediaStore files under a `Screenshots` folder (`RELATIVE_PATH` on Android 10+, the file path before), added after the switch was turned on (Android 10+ hides one still being written from the query, so it is seen once written). Photos from the camera are not sent.

## How

- **Phone:**
  - `ScreenshotRule` (pure, JVM-tested): whether a MediaStore row is a screenshot to send (folder, time, size, pending), and the order to send them.
  - `Screenshots` runs on a 2 s ticker of its own in the notification listener. A `ContentObserver` on `MediaStore.Images` marks it dirty; the tick queries only rows added after the last one handled, and sends each new screenshot. It keeps no standing link.
  - The clipboard copy uses `ClipboardImages.pngOf` (a JPEG screenshot is converted, as a copied image is) and `RustCore.sendClipboardImage`; the file uses `TransferService.start`.
  - The permission: `READ_MEDIA_IMAGES` on Android 13+, `READ_EXTERNAL_STORAGE` before. Android 14's "selected photos only" access does not see new screenshots, so Settings says full access is needed.
- **Computer:** nothing new. Both paths exist and are checked as they are today.

## Not done

- Camera photos, a gallery of the phone's photos, and a desktop "Open / Copy / Save" prompt (Windows toast buttons need a package identity, research item 13).

## Checks

- **JVM:** `ScreenshotRule`.
- **Device (A71 → Windows):** a screenshot taken with the side keys (or `input keyevent 120`) lands on the computer's clipboard and in Downloads, MD5 of the file matching; a camera photo does not; the switch off sends nothing.

## As built

- **No standing link:** each send uses the link that exists, or a one-off connection as the other background sends do.
- **Android 14's "selected photos only"** is declared (`READ_MEDIA_VISUAL_USER_SELECTED`), so the app is not given a temporary full grant; Settings says all photos are needed.
- **Fresh review (0 Critical, 2 Important), fixed one commit each:** during a session the clipboard sync fetched the screenshot back onto the phone's own clipboard (now it knows the id it sent), and a screenshot taken while the computer was away was dropped (now it waits). Minors fixed: the switch re-checked between images and turned off when photo access goes, one staged file per send, the observer's lifecycle, and a rebuilt media store. Left: a share Android refuses to start in the background is only logged (it started on the A71), and a JPEG is decoded in memory as a copied image is.
- **Device-checked (A71 → Windows):** a screenshot on the clipboard and in Downloads (MD5s matching), a camera image not sent, nothing with the switch off. Checklist §72.
