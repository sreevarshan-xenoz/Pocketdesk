# F3: take a photo with the phone (2026-10-20)

Plan: `docs/research/2026-10-20-next-cycle.md`. The oldest camera request (KDE bug 394547): from the computer, ask the phone for a photo, such as a scan of a page, and have it land on the computer.

## The design

It rides C7's road, recent photos, end to end. A third request beside the list and the fetch:

- **The computer:** "Take a photo with the phone" on the Phone page queues `Ask::Take` in `phone_photos_out/`. The server sends `kdeconnect.pocketdesk.phone_photos.take {}` to the phone if it is connected and its Files permission is on.
- **The phone:**
  - the bridge keeps the request with the computer it came from;
  - the background tick (C7's, run from the notification listener) offers the camera for the phone's own computer, with Settings "Photos the computer asks for" on (off by default, as it keeps the background link);
  - the offer is a notification, because Android opens no camera from the background. It times out after 5 minutes.
- **Taking it:** a tap opens `CameraActivity`, which shows nothing of its own:
  - it asks for the camera permission when the app does not hold it (the app declares it for the QR scanner, so Android requires it before another app's camera may be used);
  - it then has the camera app take one photo into `cache/camera/` (`ActivityResultContracts.TakePicture` with the FileProvider).
- **Sending it:** the photo goes to that computer as a file (`TransferService`), into its received files. With "Also onto its clipboard" (on) it goes as the computer's clipboard image too (N4).
- **Clean-up:** photos left in the cache over an hour are deleted at the next ask.

## Tests

- **The computer:** the request becomes the `take` packet; the packet needs Files.
- **The bridge:** a take waits with its computer.
- **JVM:** the offer rule; the cache's clean-up.
- **Owed (device, with the owner):**
  - an ask from this box reaching the A71's notification;
  - the camera, the permission prompt;
  - the photo in the received files and on the clipboard;
  - the switch off;
  - another computer's ask.

## As built

As designed, with what the review changed:
- the switch is off by default and keeps the background link while on (with notification access, which runs it), as Recent photos does;
- the Phone page's button is enabled only for a phone files reach now, saying why otherwise;
- old photos in the cache go at each ask as well as each photo.

Not checked on a device (the owner's night of features): checklist §128.

## Review

A fresh review found 0 Critical and 2 Important issues, fixed one commit each:
- the ask never reached a phone with only this switch on (no link was kept for it);
- the computer said "Asked the phone" with the phone out of reach.

Minors fixed: the cache swept at each ask, and an ask with notifications off logged.

Minors left:
- the activity does not check the switch again at the tap (the offer lasts 5 minutes);
- a computer can repeat its asks (the bridge keeps 20).
