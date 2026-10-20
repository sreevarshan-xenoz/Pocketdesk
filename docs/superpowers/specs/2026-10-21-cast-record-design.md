# Screenshot and record the phone's window (L5)

**Date:** 2026-10-21. **Branch:** `feat/cast-record`. The L cycle's fifth item (`docs/research/2026-10-20-l-cycle.md`).

## What is wrong

- **The phone's screen window shows, but keeps nothing.** A moment on the phone's screen cannot be saved from the computer, nor a demo recorded.
- **Asked for:** scrcpy's `--record` and its front ends' buttons.

## What changes

- **A bar at the window's top, whatever control says,** with two buttons:
  - **Screenshot:** the picture shown, saved as a PNG in `Pictures\Pocketdesk` ("Phone screen 2026-10-21 01-02-03.png", the time in UTC, the next free name if taken) and copied to the clipboard.
  - **Record / Stop:** the cast's H.264 is copied, not encoded again, by an `ffmpeg -c copy` sidecar into an MKV in `Videos\Pocketdesk`. The cast's frame rate varies, so the wall clock gives the timestamps (`-use_wallclock_as_timestamps`); it starts at the next keyframe, which it asks the phone for, so the file never opens grey. The bar shows its time and size as it grows. It is MKV, so a file cut short still plays.
- **It stops** with Stop, when the cast ends, or when the phone turns (a new size is a new stream; the recording so far is kept).
- **What the bar says:** where a screenshot or recording went, or why not (no ffmpeg).
- **A private screen:** Android sends a screen it keeps private (FLAG_SECURE) black, so it is saved black.

## Left out

- The phone's sound in the recording (K5's sound plays apart from the picture).
- A folder of the owner's choosing, and a free-space floor: the size shows as it grows.

## Testing

- **Unit:** names' date (a leap day), the next free name.
- **ffmpeg (on this box, skipped without it):** a generated H.264 stream fed at 10 frames a second records as an MKV of 2 s, h264 at its width, the delta frame before the first keyframe left out.
- **On the device (checklist §160):** a cast from the phone showing a still screen; `ffprobe` on a recording (its duration within 10 % of the wall clock, the phone's size); the PNG's size and a pixel against `adb exec-out screencap`.
