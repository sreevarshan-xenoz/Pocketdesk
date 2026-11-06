// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Pocketdesk windows: a GNOME Shell extension that answers four questions
// about this desktop's windows on the session bus, and nothing else.
//
//   dev.pocketdesk.Windows at /dev/pocketdesk/Windows
//     List()          every ordinary top-level window
//     HitTest(x, y)   the window the stage would give a click at this pixel
//     Activate(id)    raise and focus one listed window
//     Layout()        the box round every monitor
//
// GNOME Shell keeps its window list from ordinary programs
// (org.gnome.Shell.Introspect.GetWindows answers "Access denied" to anything
// that is not one of GNOME's own apps). Installing this extension is the
// owner's choice to give it to Pocketdesk, and to any other program of the
// same user on the session bus. It moves, sizes, closes and types into
// nothing; the one thing it does is Activate.
//
// The id of a window is Meta.Window.get_id(), the number mutter's ScreenCast
// RecordWindow takes. A "chain" is a window followed by the window it is a
// dialog or popup of, and that one's, and so on (at most eight): a window's own
// dialogs belong to it.

import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const NAME = 'dev.pocketdesk.Windows';
const PATH = '/dev/pocketdesk/Windows';
const VERSION = 1;
const MOST_OWNERS = 8;

const XML = `
<node>
  <interface name="${NAME}">
    <method name="List">
      <arg type="aa{sv}" direction="out" name="windows"/>
    </method>
    <method name="HitTest">
      <arg type="i" direction="in" name="x"/>
      <arg type="i" direction="in" name="y"/>
      <arg type="a(tu)" direction="out" name="chain"/>
    </method>
    <method name="Activate">
      <arg type="t" direction="in" name="id"/>
      <arg type="b" direction="out" name="done"/>
    </method>
    <method name="Layout">
      <arg type="(iiuu)" direction="out" name="box"/>
    </method>
    <property name="Version" type="u" access="read"/>
  </interface>
</node>`;

function actors() {
    return global.get_window_actors();
}

// Whether a window is one to offer: an ordinary window of its own, not a
// dialog or popup of another and not an override-redirect one.
function isListed(win) {
    if (!win || win.get_window_type() !== Meta.WindowType.NORMAL)
        return false;
    if (win.is_override_redirect() || win.get_transient_for() !== null)
        return false;
    return win.get_id() > 0;
}

// The window, then what it is a dialog or popup of, and so on.
function chainOf(win) {
    const chain = [];
    for (let w = win, i = 0; w && i < MOST_OWNERS; w = w.get_transient_for(), i++)
        chain.push(w);
    return chain;
}

// Whether `win` (a top-level window) is the focus window or the one it is a
// dialog or popup of, through windows of one program only.
function holdsFocus(win) {
    const focus = global.display.focus_window;
    if (!focus)
        return false;
    const pid = win.get_pid();
    if (pid <= 0)
        return false;
    for (const w of chainOf(focus)) {
        if (w.get_pid() !== pid)
            return false;
        if (w === win)
            return true;
    }
    return false;
}

function text(value) {
    return GLib.Variant.new_string(value ?? '');
}

function describe(win, stack) {
    const frame = win.get_frame_rect();
    // What mutter's window stream shows: the frame and, for a client-side
    // decorated window, its shadow round it.
    const buffer = win.get_buffer_rect();
    const minimized = !!win.minimized;
    const hidden = minimized || (typeof win.is_hidden === 'function' && win.is_hidden());
    return {
        'id': GLib.Variant.new_uint64(win.get_id()),
        'title': text(win.get_title()),
        'app': text(win.get_wm_class() || win.get_gtk_application_id()),
        'pid': GLib.Variant.new_uint32(Math.max(0, win.get_pid())),
        'x': GLib.Variant.new_int32(frame.x),
        'y': GLib.Variant.new_int32(frame.y),
        'width': GLib.Variant.new_uint32(Math.max(0, frame.width)),
        'height': GLib.Variant.new_uint32(Math.max(0, frame.height)),
        'buffer-x': GLib.Variant.new_int32(buffer.x),
        'buffer-y': GLib.Variant.new_int32(buffer.y),
        'buffer-width': GLib.Variant.new_uint32(Math.max(0, buffer.width)),
        'buffer-height': GLib.Variant.new_uint32(Math.max(0, buffer.height)),
        'minimized': GLib.Variant.new_boolean(minimized),
        'hidden': GLib.Variant.new_boolean(hidden),
        'focus': GLib.Variant.new_boolean(holdsFocus(win)),
        'stack': GLib.Variant.new_uint32(stack),
    };
}

// The window an actor of the stage is part of, if it is one.
function windowOfActor(actor) {
    for (let a = actor; a; a = a.get_parent()) {
        if (a instanceof Meta.WindowActor)
            return a.meta_window;
    }
    return null;
}

export default class PocketdeskWindows extends Extension {
    enable() {
        this._export = Gio.DBusExportedObject.wrapJSObject(XML, this);
        this._export.export(Gio.DBus.session, PATH);
        this._owner = Gio.bus_own_name_on_connection(
            Gio.DBus.session, NAME, Gio.BusNameOwnerFlags.NONE, null, null);
    }

    disable() {
        if (this._owner) {
            Gio.bus_unown_name(this._owner);
            this._owner = null;
        }
        if (this._export) {
            this._export.unexport();
            this._export = null;
        }
    }

    get Version() {
        return VERSION;
    }

    // Every ordinary top-level window, as a list of {name: value}: id (t),
    // title, app (the class, else the application id), pid (u), x, y (i),
    // width, height (u), minimized, hidden (minimized or put away), focus (it,
    // or a dialog of it with the same program, has the keyboard), stack (u,
    // 0 the bottom).
    List() {
        const out = [];
        try {
            actors().forEach((actor, stack) => {
                const win = actor.meta_window;
                if (isListed(win))
                    out.push(describe(win, stack));
            });
        } catch (e) {
            console.error(`pocketdesk-windows: List: ${e}`);
            return new GLib.Variant('(aa{sv})', [[]]);
        }
        return new GLib.Variant('(aa{sv})', [out]);
    }

    // The windows the stage's own pick gives a click at (x, y), as a chain of
    // (id, pid): the window under the point first, then what it is a dialog
    // of. Empty when what is under the point is not a window (the top bar, the
    // overview, a menu of the Shell, the lock screen, nothing), which is
    // what lets a caller tell that something covers a window.
    HitTest(x, y) {
        const chain = [];
        try {
            const actor = global.stage.get_actor_at_pos(Clutter.PickMode.REACTIVE, x, y);
            const win = windowOfActor(actor);
            if (win)
                for (const w of chainOf(win))
                    chain.push([w.get_id(), Math.max(0, w.get_pid())]);
        } catch (e) {
            console.error(`pocketdesk-windows: HitTest: ${e}`);
            return new GLib.Variant('(a(tu))', [[]]);
        }
        return new GLib.Variant('(a(tu))', [chain]);
    }

    // Raise and focus the listed window `id` (bringing it back if it was
    // minimised). False when no listed window has that id. It is the one
    // thing here that changes the desktop.
    Activate(id) {
        try {
            for (const actor of actors()) {
                const win = actor.meta_window;
                if (isListed(win) && win.get_id() === id) {
                    Main.activateWindow(win);
                    return new GLib.Variant('(b)', [true]);
                }
            }
        } catch (e) {
            console.error(`pocketdesk-windows: Activate: ${e}`);
        }
        return new GLib.Variant('(b)', [false]);
    }

    // The box round every monitor, (x, y, width, height).
    Layout() {
        let box = [0, 0, 0, 0];
        try {
            const display = global.display;
            let left = null, top = null, right = null, bottom = null;
            for (let i = 0; i < display.get_n_monitors(); i++) {
                const r = display.get_monitor_geometry(i);
                left = left === null ? r.x : Math.min(left, r.x);
                top = top === null ? r.y : Math.min(top, r.y);
                right = right === null ? r.x + r.width : Math.max(right, r.x + r.width);
                bottom = bottom === null ? r.y + r.height : Math.max(bottom, r.y + r.height);
            }
            if (left !== null)
                box = [left, top, right - left, bottom - top];
        } catch (e) {
            console.error(`pocketdesk-windows: Layout: ${e}`);
        }
        return new GLib.Variant('((iiuu))', [box]);
    }
}
