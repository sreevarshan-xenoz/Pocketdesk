# O7, received images onto the clipboard (design)

Date: 2026-10-22. Plan: `docs/research/2026-10-22-o-cycle.md` §O7.

**The problem.** A photo sent to paste somewhere lands as a file in Downloads, and then has to be found and copied by hand. KDE Connect and LocalSend users ask for a received image to be pasteable at once.

## What

**The computer.**
- **The switch:** desktop Settings "Images sent here onto the clipboard" (`received_images_clipboard`, off, applies at once).
- **What qualifies:** a file a phone, a computer or a KDE device sends on its own, never one of a folder's files.
- **What happens:**
  - A PNG is put on the clipboard as it is, once it decodes within N4's limits: 40 megapixels, 16384 a side.
  - A JPEG is decoded within the same limits, turned as its EXIF says, and put on the clipboard as PNG (`clip_image::png_from_file`).
  - Anything else is left alone. So is a file over 32 MB.
- **Where it runs:** after the file is saved and listed, off the runtime (`clipboard::put_received_image`). The clipboard is not held by a pause (K2a).
- **What it does not do:** the file stays where it landed, and no new permission is involved.

**The phone.**
- **The switch:** Settings "Images from the computer onto my clipboard" (off).
- **What qualifies:** a file the computer sends on its own, never one of a folder's files or one going into a shared folder of N6's. The file is known as a PNG or a JPEG by its first bytes, at most 32 MB (`ReceivedImageRule`, JVM-tested).
- **What happens:** a copy is kept in N4's clipboard folder and put on the clipboard through N4's provider (`ReceivedImageClip`).

**No bounce.**
- *On the phone:* the copy is the app's own, so the clipboard tile, a session's sync and M10 never send it back.
- *On the computer:* M3's image push now skips the device an image came from, by the image's id (`clipboard_push::note_image_from`). This covers O7's images and N4's own images from a phone's clipboard, which had been told back to that phone.

## Not done

- **Other formats:** HEIC, WebP and GIF stay files only. A phone's camera photo is a JPEG on most phones.
- **A folder's images:** never put on the clipboard. The last one would win, and a folder is not one thing to paste.

## Checks

- **Unit tests:**
  - the computer: a PNG as it is, a JPEG as PNG, anything else or a cut PNG refused;
  - the phone: the kind by its first bytes, and the switch, folders and size.
- **Owed:** on the OPPO, a photo each way pasted into an app (checklist §190).

## Review

A fresh review found 0 Critical, 5 Important and several Minor issues.

**Important, fixed:**
- **The clipboard permission was bypassed.** Someone else's phone, a guest (Files and Links only, M2b), a computer with `computer_clipboard` off, or a KDE device with its Clipboard switch off could each replace the clipboard by sending an image. It now takes a device allowed the clipboard (`clipboard_push::may_set_clipboard`).
- **Files asked for went onto the clipboard too.**
  - On the computer: the Phone page's Save, a recent photo, a photo taken (F3). Each fetch or take this computer sends now counts as one file to come from that phone within ten minutes, which is not put on the clipboard (`share::Asked`, tested).
  - On the phone: a file fetched from the computer's folders. Only an offer, a file the computer sent unasked, goes now.
- **Memory unbounded.** Conversions now run one at a time. A PNG is checked by its header, not decoded whole, and the JPEG's limit is counted in RGBA.
- **Size unchecked.** A JPEG made PNG is held to N4's 32 MB.
- **The phone could fetch its own image back.** A phone in a session asks for the clipboard every 4 s and would fetch back the image it had just sent. The computer now answers that phone as empty while the image is its own.

**Minor, fixed:**
- the note forgotten when the write fails;
- the clipboard set only once the file is listed;
- the desktop row's words, which said a phone only.

**Tests added:** the note, a PNG claiming a huge picture, and `Asked`.

**Not done:**
- **An EXIF-rotated JPEG test.** Building EXIF in a test is heavy; checked by hand instead (checklist §190).
- **Two images arriving together.** The one decoded last wins.
