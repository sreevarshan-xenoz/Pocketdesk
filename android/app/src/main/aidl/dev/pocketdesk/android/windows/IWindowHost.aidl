// Phone apps tier 2 (separate windows): what Pocketdesk asks of its helper
// process, which Shizuku starts with shell identity. Used for exactly four
// things: making a display for an app window, launching an app on that
// display, sending input to that display, and turning the phone's own hotspot
// on and off (transactions 9 and 10, which take nothing). Nothing else.
package dev.pocketdesk.android.windows;

import android.view.Surface;

interface IWindowHost {
    // The transaction number Shizuku's user service contract fixes.
    void destroy() = 16777114;

    // A display of its own at this size and density that draws into
    // `surface` (the window's video encoder). The display's id, or -1.
    int createDisplay(String name, int width, int height, int dpi, in Surface surface) = 1;

    // Start `component` ("package/activity") on the display. True when it started.
    boolean launch(int displayId, String component) = 2;

    // Give the display back (the app on it ends with it).
    void release(int displayId) = 3;

    // Transaction 4 was a single-finger touch; fingers now go together (below).

    // One wheel step at pixel (x, y); `vscroll` is Android's vertical scroll amount.
    boolean injectScroll(int displayId, float x, float y, float vscroll) = 5;

    // One key press and release.
    boolean injectKey(int displayId, int keyCode) = 6;

    // Typed text (what the keyboard's character map can make of it).
    boolean injectText(int displayId, String text) = 7;

    // Every finger down on the display at once (keys to taps): `action` is
    // Android's MotionEvent action with the changing finger's index in its
    // high bits; finger i has id ids[i] and pixel (xs[i], ys[i]).
    boolean injectPointers(int displayId, int action, in int[] ids, in float[] xs, in float[] ys, long downTime) = 8;

    // Let my computers turn on my hotspot: start the phone's own Wi-Fi hotspot
    // with the settings Android already holds for it. No argument: no name,
    // password or band is ever passed in. 1 when it started, 2 when a Wi-Fi
    // hotspot was already running (nothing was started), 0 when it did not.
    int startHotspot() = 9;

    // Turn the hotspot off again. True when it was asked to stop.
    boolean stopHotspot() = 10;
}
