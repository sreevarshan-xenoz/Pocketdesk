# Notifications with their app's icon, and pictures (K3)

**Date:** 2026-10-20. **Branch:** `feat/notification-icons`. The K cycle's third item (`docs/research/2026-10-20-k-cycle.md`).

## What is wrong

- **A phone's notification on the computer is text only.** `PhoneNotifications.kt` sends its app, title and text. The toast has no icon to tell a chat from a mail at a glance, and no picture where the phone shows one (a photo someone sent, a contact's face).
- **KDE Connect forwards both** (bug 341898, fixed), and Phone Link shows the app's icon.

## What changes

- **The phone sends the app's icon** with each app's notifications to a computer until one of them reached it this run, as `appIcon`.
  - The icon is a PNG of at most 64 × 64, drawn from the app's own icon.
  - The computer keeps it, so later notifications name the app and it shows them with the kept icon.
- **With "Pictures in notifications" on** (phone Settings, off by default), the phone also sends a notification's picture as `picture`: its BigPictureStyle image, or else its large icon (a contact's photo). The picture is a JPEG of at most 720 pixels on its long side.
  - **Never** for a notification sent redacted: the lock screen hides it, or it holds a one-time code. Both follow N5a's rules.
- **The computer:**
  - **Keeping them:** icons are kept owner-only, by phone and app (`notification_icons/`), for a notification the phone's limit let in, at most 50 a phone and 200 in all, the oldest dropped first. A picture is kept for a notification shown here, named by its content, and removed when the notification closes, is held, or comes again without one; at most 30.
  - **Checking them:** each is checked before decoding: dimensions read from the header first, at most 256 × 256 and 256 KiB for an icon, 4096 × 4096 and 2 MiB for a picture.
  - **A switch:** desktop Settings "Apps' icons and pictures" (`notification_pictures`, on); off, none is kept or shown.
  - **Showing them:**
    - Windows: the toast's app logo, and the picture inline.
    - Linux: `image-path` (the icon), with the picture in the body where the server takes `body-images`.
    - The Notifications page: the icon beside each.
- **Under the flood guard,** a notification past 20 a minute from a phone keeps neither its icon nor its picture, as it is not shown.

## Left out

- Icons of KDE Connect devices' notifications: KDE sends them as a payload on a separate transfer.
- Icons on Activity's rows: Activity reads the event log, which keeps no pictures.

## Testing

- **JVM:**
  - what goes when: the icon once per app, and only the first time;
  - a picture only with the setting on and the notification in full;
  - the sizes.
- **Server:** the decode limits (an oversize header refused without decoding); the toast's XML with the logo and the picture; the icon store's cap.
- **On the device (checklist §153, on screen: an XML read back would pass with an image the toast could not read):** the throwaway chat app posts a notification with a large icon and a BigPicture; the toast, read back from the Action Center, carries both.

## As built

- **Phone:** `NotificationPictures` (what goes when, the sizes; JVM-tested); `PhoneNotifications.addPictures` on the link's worker, each extra checked; an icon marked sent by computer once its notification was sent.
- **Core:** `core::local::notification_icons`: the header reads (`png_size`, `jpeg_size`), the store and its caps, `forget_picture`.
- **Server:** `notification.rs` keeps the icon after the flood guard and the picture for a notification shown, off the runtime's workers; `toast_answers::toast_xml_pictured` (the logo and the inline image by `file:` URI) and `Pictures`; on Linux `image-path` and an `<img>` where the server shows body images.
- **Desktop:** the Notifications page's icon (`notes_view.rs`), read again when its file changes; the Settings switch.

## Review

A fresh review (0 Critical, 4 Important, 9 Minor), fixed one commit each:

- **Important:**
  - an app's extras of another type crashed the phone's listener;
  - icons were kept before the flood guard, 2 MiB each, with no cap a phone, so one phone could push the others' out;
  - an icon counted as sent when only put in a notification, and for one computer;
  - pictures stayed until 30 others pushed them out.
- **Minor, fixed:** a picture file reused by an update (now named by its content), a crash's temp files, the page's stale icon, no switch on the computer, and the spec's words.
- **Left:**
  - icons are kept by the app's name, while the phone sends one per package: two apps of one name share an icon;
  - a picture is kept on Linux where the server shows none;
  - a phone unpaired keeps its icons until pushed out;
  - a picture goes with a chat whose messages hold a code, as its text does;
  - whether the toast reads an owner-only file is owed on screen.
