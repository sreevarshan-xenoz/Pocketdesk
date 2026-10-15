# Mobile input (T9)

Date: 2026-10-15. Status: T9a built (see As built); T9b and T9c designed. On autopilot (the user asked for the roadmap to keep going; the decisions are mine).

## Why

T9 in the roadmap gathers Moonlight's open input requests: pinch-zoom in trackpad mode, portrait sessions, and pointer capture. Each makes the phone a better remote for a desktop drawn at a size made for a monitor.

## T9a: pinch-zoom in trackpad mode

- **Before:** in trackpad mode two fingers always scrolled the desktop, so the only way to magnify small text was to switch to direct touch, pinch, and switch back.
- **Now:** the trackpad engine (`TrackpadGesture`) tells the two apart.
  - A two-finger gesture whose span changes past the touch slop before its centre travels as far is a **pinch**. It zooms the view, and while pinching, moving the fingers pans it, as direct touch's pinch does.
  - Otherwise it is a **scroll**, as before.
  - A gesture stays what it began as until the fingers lift.
  - A pinch is never a click.
  - Only exactly two fingers pinch. Three fingers keep their middle-click tap and their scroll.
- **The view:** it gives the engine the pair's centre motion and span, counted from when the pair formed. A pair that changes (a finger lifts or lands) starts its span afresh instead of jumping. Each step zooms at most 2x either way.
- **Back to 1x:** pinch in, as in direct touch.
- **Units:** the slop, scrolling and the pinch are in view pixels. Only the cursor's travel is divided by zoom, so it matches the on-screen distance the finger covered.

## T9b: portrait sessions

In portrait, the letterboxed desktop fills about a third of the screen and the rest is black. With "Pane: auto", a portrait phone gets the dual-pane layout that unfolded foldables and tablets get: the stream on top, sized to the video's shape, and the trackpad dock and shortcut bar below. The decision and the pane's size are a small pure rule, tested.

## T9c: pointer capture

A mouse plugged into the phone (USB OTG or Bluetooth) should drive the desktop's cursor with relative motion, not the phone's own pointer. Android's pointer capture (`View.requestPointerCapture`) gives raw relative events. The session will capture while it is in front in trackpad mode and a mouse is present, and release on Back or when focus is lost. No mouse is at hand here, so its device check stays owed.

## As built (T9a)

- **Where it lives:** `stream/TrackpadGesture.kt` (`PadOutput.Zoom`, the classification) and the trackpad loop in `stream/RemoteDesktopView.kt`. The foldable dock gives no span, so it never pinches.
- **Fresh review (0 Critical, 4 Important), fixed:**
  - classification counted the first finger's travel from the first touch: a late second finger always scrolled, and a pinch moving only the first-landed finger never zoomed;
  - zoomed travel was set against an unzoomed span;
  - a pair that changed made the zoom jump.
- **Ruled:** nothing pans the view to follow the desktop's cursor, because the phone does not know where the cursor is. Panning comes with a pinch.
- **Device:** adb cannot make multi-finger gestures, so the pinch is JVM-tested (21 trackpad tests) and its device check is owed (checklist §60).
