# Notifications with their app's icon, and pictures (K3)

**Date:** 2026-10-20. **Branch:** `feat/notification-icons`. The K cycle's third item (`docs/research/2026-10-20-k-cycle.md`).

## What is wrong

- **A phone's notification on the computer is text only.** `PhoneNotifications.kt` sends its app, title and text. The toast has no icon to tell a chat from a mail at a glance, and no picture where the phone shows one (a photo someone sent, a contact's face).
- **KDE Connect forwards both** (bug 341898, fixed), and Phone Link shows the app's icon.

## What changes

- **The phone sends the app's icon** with the first notification of each app it sends this computer since the app started, as `appIcon`.
  - The icon is a PNG of at most 64 × 64, drawn from the app's own icon.
  - The computer keeps it, so later notifications name the app and it shows them with the kept icon.
- **With "Pictures in notifications" on** (phone Settings, off by default), the phone also sends a notification's picture as `picture`: its BigPictureStyle image, or else its large icon (a contact's photo). The picture is a JPEG of at most 720 pixels on its long side.
  - **Never** for a notification sent redacted: the lock screen hides it, or it holds a one-time code. Both follow N5a's rules.
- **The computer:**
  - **Keeping them:** icons are kept owner-only, by phone and app (`notification_icons/`), at most 200, the oldest dropped first. A picture is kept beside the notification and goes with it.
  - **Checking them:** each is checked before decoding, with C7's limits: dimensions read from the header first, at most 1024 × 1024 for an icon and 4096 × 4096 for a picture, and 2 MiB in all.
  - **Showing them:**
    - Windows: the toast's app logo, and the picture inline.
    - Linux: `image-path` (the icon), with the picture in the body where the server takes `body-images`.
    - The Notifications page: the icon beside each.
- **Under the flood guard,** a picture is the first thing dropped: one past 20 a minute from a phone is not kept.

## Left out

- Icons of KDE Connect devices' notifications: KDE sends them as a payload on a separate transfer.
- Icons on Activity's rows: Activity reads the event log, which keeps no pictures.

## Testing

- **JVM:**
  - what goes when: the icon once per app, and only the first time;
  - a picture only with the setting on and the notification in full;
  - the sizes.
- **Server:** the decode limits (an oversize header refused without decoding); the toast's XML with the logo and the picture; the icon store's cap.
- **On the device (checklist §153):** the throwaway chat app posts a notification with a large icon and a BigPicture; the toast, read back from the Action Center, carries both.
