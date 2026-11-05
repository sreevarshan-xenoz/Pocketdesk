#!/usr/bin/env python3
"""A client-side decorated GTK4 window for the GNOME rig (not part of the product).

`server/tests/lend_window_gnome_live.rs` (docs/development-setup.md, section
3a-2) lends a window of a throwaway headless GNOME Shell and clicks into it.
`wev` has no decorations; this window has GTK's header bar and shadows, so the
test can check that a click lands at the aimed place in a decorated window
too. It writes, to stdout, lines in the words of `wev` that the test reads:

    [ 1: wl_pointer] motion: surface_x: 12.000000, surface_y: 34.000000
    [ 1: wl_pointer] button: state: pressed (1)
    [ 2: wl_keyboard] key: state: pressed (1)

The places are in the window's own coordinates (its visible frame, without the
shadow). Its application id, which is the class the Shell extension reports, is
`dev.pocketdesk.RigClicker`. Run it under a user unit with the output going to
a file (it flushes each line).
"""
import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gtk  # noqa: E402


def out(line):
    print(line, flush=True)


class Clicker(Gtk.Application):
    def __init__(self):
        super().__init__(application_id="dev.pocketdesk.RigClicker")

    def do_activate(self):
        win = Gtk.ApplicationWindow(application=self, title="rig clicker")
        win.set_default_size(640, 480)
        win.set_child(Gtk.DrawingArea(hexpand=True, vexpand=True))

        motion = Gtk.EventControllerMotion()
        motion.set_propagation_phase(Gtk.PropagationPhase.CAPTURE)
        motion.connect(
            "motion",
            lambda _c, x, y: out(
                f"[ 1: wl_pointer] motion: surface_x: {x:.6f}, surface_y: {y:.6f}"
            ),
        )
        win.add_controller(motion)

        click = Gtk.GestureClick()
        click.set_propagation_phase(Gtk.PropagationPhase.CAPTURE)
        click.set_button(0)
        click.connect(
            "pressed",
            lambda _g, _n, _x, _y: out("[ 1: wl_pointer] button: state: pressed (1)"),
        )
        win.add_controller(click)

        keys = Gtk.EventControllerKey()
        keys.set_propagation_phase(Gtk.PropagationPhase.CAPTURE)
        keys.connect(
            "key-pressed",
            lambda _c, _v, _code, _s: out("[ 2: wl_keyboard] key: state: pressed (1)")
            or False,
        )
        win.add_controller(keys)
        win.present()


if __name__ == "__main__":
    Clicker().run()
