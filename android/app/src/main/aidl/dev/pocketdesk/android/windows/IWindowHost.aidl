// Phone apps tier 2 (separate windows): what Pocketdesk asks of its helper
// process, which Shizuku starts with shell identity. Used for exactly three
// things: making a display for an app window, launching an app on that
// display, and sending input to that display. Nothing else.
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

    // One touch at pixel (x, y) of the display; action is Android's MotionEvent action.
    boolean injectTouch(int displayId, int action, float x, float y, long downTime) = 4;

    // One wheel step at pixel (x, y); `vscroll` is Android's vertical scroll amount.
    boolean injectScroll(int displayId, float x, float y, float vscroll) = 5;

    // One key press and release.
    boolean injectKey(int displayId, int keyCode) = 6;

    // Typed text (what the keyboard's character map can make of it).
    boolean injectText(int displayId, String text) = 7;
}
