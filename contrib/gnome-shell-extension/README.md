# Pocketdesk windows (GNOME Shell extension)

An optional GNOME Shell extension (GNOME 45 and later). It is **off unless you
install and enable it**; Pocketdesk works without it.

GNOME Shell does not tell ordinary programs which windows are open, which one
is under a point or which has the keyboard (`org.gnome.Shell.Introspect` is
refused to anything that is not one of GNOME's own apps). Without those
answers Pocketdesk can lend a window of a GNOME computer to your other
computers to look at only. With this extension on, it can list the windows to
lend, and a lent window can be clicked and typed into by the other computer,
with every click and key kept to that window.

## What it serves

`dev.pocketdesk.Windows` at `/dev/pocketdesk/Windows` on your session bus:

| Call | Answer |
|---|---|
| `List() -> aa{sv}` | each ordinary top-level window: `id` (t, mutter's id, the one ScreenCast `RecordWindow` takes), `title`, `app`, `pid`, `x`, `y`, `width`, `height` (its frame), `buffer-x`, `buffer-y`, `buffer-width`, `buffer-height` (its buffer: the frame and, for a client-side decorated window, the shadow round it, which is what a window's stream shows), `minimized`, `hidden`, `focus` (it, or a dialog of it of the same program, has the keyboard), `stack` (0 is the bottom) |
| `HitTest(x: i, y: i) -> a(tu)` | the window the stage gives a click at that pixel, then the windows it is a dialog or popup of, as `(id, pid)`; empty when what is there is not a window (top bar, overview, a Shell menu, the lock screen) |
| `Activate(id: t) -> b` | raise and focus that listed window (bringing it back if minimised); the only call that changes anything |
| `Layout() -> (iiuu)` | the box round every monitor |
| `Version` (property, u) | `1` |

It moves, sizes, closes and types into nothing.

**Anyone of your session can ask.** While it is enabled, any program running as
you on your session bus can read the window list (with titles) and raise a
window. It does not check who asks. See SECURITY.md limit 90.

## Install

```sh
cp -r pocketdesk-windows@pocketdesk.dev ~/.local/share/gnome-shell/extensions/
# log out and back in (Wayland cannot restart the Shell), then:
gnome-extensions enable pocketdesk-windows@pocketdesk.dev
```

Turn it off with `gnome-extensions disable pocketdesk-windows@pocketdesk.dev`.

## Check it answers

```sh
gdbus call --session --dest dev.pocketdesk.Windows \
  --object-path /dev/pocketdesk/Windows --method dev.pocketdesk.Windows.List
```

## Licence

MIT or Apache-2.0, as the rest of Pocketdesk (both are GPL-compatible).
