# F3: take a photo with the phone (2026-10-20)

Plan: `docs/research/2026-10-20-next-cycle.md`. The oldest camera request (KDE bug 394547): from the computer, ask the phone for a photo, such as a scan of a page, and have it land on the computer.

## The design

It rides C7's road, recent photos, end to end. A third request beside the list and the fetch:

- **The computer:** "Take a photo with the phone" on the Phone page queues `Ask::Take` in `phone_photos_out/`. The server sends `kdeconnect.pocketdesk.phone_photos.take {}` to the phone if it is connected and its Files permission is on.
- **The phone:**
  - the bridge keeps the request with the computer it came from;
  - the background tick (C7's, run from the notification listener) offers the camera for the phone's own computer, with Settings "Photos the computer asks for" on (on by default);
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
