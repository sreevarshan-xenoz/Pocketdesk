#!/usr/bin/env python3
"""A rig-only probe (not part of the product): does any RemoteDesktop click
reach a client in this headless Shell at all?

It clicks through a *monitor* stream (the long-standing path that
gnome-remote-desktop uses), not a window's, at the screen point given, and
prints what it did. The rig then counts presses in the windows' logs. If no
client sees this click either, the rig delivers no input, whatever the window
path does.

Usage: rd_probe.py X Y
"""
import sys
import time

from gi.repository import Gio, GLib

x, y = float(sys.argv[1]), float(sys.argv[2])
bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)


def call(dest, path, iface, method, args=None, reply=None):
    return bus.call_sync(dest, path, iface, method, args,
                         GLib.VariantType(reply) if reply else None,
                         Gio.DBusCallFlags.NONE, 5000, None)


RD, SC = "org.gnome.Mutter.RemoteDesktop", "org.gnome.Mutter.ScreenCast"
rd_path = call(RD, "/org/gnome/Mutter/RemoteDesktop", RD, "CreateSession", None, "(o)")[0]
rd_id = call(RD, rd_path, "org.freedesktop.DBus.Properties", "Get",
             GLib.Variant("(ss)", (RD + ".Session", "SessionId")), "(v)")[0]
sc_path = call(SC, "/org/gnome/Mutter/ScreenCast", SC, "CreateSession",
               GLib.Variant("(a{sv})", ({"remote-desktop-session-id": GLib.Variant("s", rd_id)},)),
               "(o)")[0]
stream = call(SC, sc_path, SC + ".Session", "RecordMonitor",
              GLib.Variant("(sa{sv})", ("", {})), "(o)")[0]
call(RD, rd_path, RD + ".Session", "Start")
time.sleep(1)
s = RD + ".Session"
call(RD, rd_path, s, "NotifyPointerMotionAbsolute", GLib.Variant("(sdd)", (stream, x, y)))
time.sleep(0.3)
call(RD, rd_path, s, "NotifyPointerButton", GLib.Variant("(ib)", (272, True)))
time.sleep(0.1)
call(RD, rd_path, s, "NotifyPointerButton", GLib.Variant("(ib)", (272, False)))
time.sleep(0.5)
# Relative motion too: another path into the same virtual pointer.
call(RD, rd_path, s, "NotifyPointerMotionRelative", GLib.Variant("(dd)", (5.0, 5.0)))
time.sleep(0.5)
call(RD, rd_path, s, "Stop")
print(f"rd_probe: clicked at ({x}, {y}) through monitor stream {stream}", flush=True)
