# F6: the phone's camera on the computer (2026-10-20)

Plan: `docs/research/2026-10-20-next-cycle.md`. KDE Connect's oldest open camera request (T4661, 2016) is the phone as a webcam. This is its first half: the phone's camera on the computer. A virtual webcam is the second half, and it waits for the owner's OK.

## Part 1 (built): the camera in the cast window

- **"Show camera" in each computer's menu on Home:**
  - asks for the camera permission;
  - then opens the cast session "Show screen" uses (mode 2, under the same per-phone permission, now "Show its screen or camera here").
- **The picture:**
  - the cast encoder (`CastEncoder`) is fed by Camera2 instead of a screen projection: the back camera, at the largest 16:9 size within 1920 by 1080 it offers an encoder (`CameraFeed.choose`, tested);
  - under a camera foreground service.
- **The frames are the sensor's, landscape:** the phone is held on its side.
- **The computer** shows it in the cast window as it shows the phone's screen. The window takes the stream's shape.
- **Not for the camera:** the computer controlling the phone (C8).

## Part 2 (not built): a webcam on the computer

The camera as a device other apps pick (a video call, OBS):
- **Windows 11:** through Media Foundation's virtual camera (`MFCreateVirtualCamera`), a media source the app registers.
- **Linux:** through v4l2loopback.

Each registers a camera device on the owner's computer. That is the same kind of change as C12's driver, so it waits for the owner's OK.

## Review

A fresh review found 0 Critical and 2 Important issues, fixed one commit each:
- the camera service started before its permission was checked (a crash on Android 14 if it had been taken back);
- the computer calling a camera cast "screen" (now "screen or camera" on the Devices, Sessions and Activity pages).

Minors fixed: a camera still opening when the cast stopped is closed, and a request on a closed camera no longer throws.

Minors left:
- the sideways picture of a phone held upright;
- the frame rate in low light.

Owed (device, with the owner): checklist §131.
