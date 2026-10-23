#!/usr/bin/env python3
"""The checklist's open items, by where they can run (C6).

Reads docs/device-verification-checklist.md and writes docs/owed-checks.md:
each unchecked item under one place, from the words of the item and its
section. `--check` exits 1 when the written file is stale (CI).

A line can name its place itself with `(where: <place>)`, which wins.
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CHECKLIST = ROOT / 'docs' / 'device-verification-checklist.md'
OUT = ROOT / 'docs' / 'owed-checks.md'

# The places, in the order a line is tested against them: the first match
# wins, so the more particular come first.
PLACES = [
    ('power', 'Power, lock or server stop on the owner\'s Windows box: never run here (the owner\'s rule)',
     r'\b(sleep|shut ?down|hibernat\w*|reboot|log ?off|lock(ed|s)? (it|the (desktop|computer|screen|phone|pc))'
     r'|lock (pc|desktop)|screen locks|win\+l|uac|power (off|button|actions?)|server[- ]stop'
     r'|stop(ping)? the (installed )?server|pocketdesk (stop|install|uninstall)|logon task|firewall rules?'
     r'|install(ed)? task|start/restart|restart (the )?(installed )?server)\b'),
    ('owner', "Needs the owner's OK on this box: input reaching Windows, or its display settings",
     r'\b(inject\w*|super\b|start menu|alt\+tab|ctrl\+|pointer|cursor|taps? (map|land|reach|still map)|tap-to-'
     r'|typ(ed|ing) (into|in)|letters|hotkeys?|keys? (arrive|reach|land)|keyboard (shortcut|input)|mouse moves?'
     r'|privacy|swipe typing|dictation|composing text|click(s|ed)? (on|into) the desktop|desktop (moves|receives)|drives? the (desktop|computer)|elevated'
     r'|scal(ed|ing)\b[^.]{0,12}\d{3}|\d{3} ?% scal\w*|\d{3}/\d{3} ?%|dpi)'),
    ('sim', 'A phone with a SIM', r'\b(sim|sms|mms|text messages?|texts|cellular|mobile (data|service|network))\b'),
    ('second phone', 'A second phone', r'\b(two phones|second phone|another phone|both phones|second android|phone b)\b'),
    ('away', 'Away from home: another network, or through a router', r'\b(outside|away from home|from away|another network|router|port forward\w*|public (ip|network)|relayed path|from the internet|real distance|hotspot)\b'),
    ('second computer', 'A second computer', r'\b(two (real )?computers|second computer|another computer|computer b|other computer)\b'),
    ('linux', 'A Linux desktop (the Azure VM serves for some)',
     r'\b(linux|sway|gnome|kde plasma|plasma|wayland|x11|xwayland|hyprland|wlroots|d-bus|dbus|pipewire|vaapi|nvenc|orca|fc-list|systemd|wev|the vm|azure|waybar|swaybar|libnotify|notify-send|playerctl|ydotool|uinput|mako|dunst|wpctl|upower|pactl|pavucontrol|pw-play|pw-record|pw-loopback|pw-cat|paplay|wl-paste|wl-copy|xdg-open|screencopy|wl_output|xcap|hyprctl|evdev|eviocgrab|input group|ashpd|portal)\b'),
    ('other hardware', 'Other hardware: a foldable, tablet, mouse, game pad, older or newer Android',
     r'\b(foldable|tablet|gamepad|game ?pad|(usb|bluetooth) mouse|connected mouse|mouse connected|hardware keyboard|bluetooth|sd card|android (8|9|1[0-3]|17)|api 3[0-3]|android tv|wear ?os|smart ?watch|projector|hdr|4k|third monitor)\b'),
    ('by hand', 'By hand or eye on this box: hearing, looking, the tray, drag and drop, a screen reader, a real app',
     r'\b(by ear|ears?|listen\w*|hear\w*|by eye|look(s|ing)?|tray|drag(ging)?|drag and drop|drag-and-drop|drop(ped)? (a|the) (file|folder)|dropped on the window|narrator|talkback|screen reader|camera|scan(s|ning)?|real (app|chat app|player|panel)|tasker|browser extension|extension loaded|home assistant itself|stopwatch|feel|smooth\w*|on screen|clicks?|the window by hand)\b'),
]
HERE = ('here', 'Here: this Windows box and the test phone (the OPPO), without touching the owner\'s power, lock or input')


def items(text):
    """Each open item with its section, read whole: an item wrapped onto
    indented lines is joined (C6 review), and fenced blocks are skipped."""
    section = ''
    current = None
    fenced = False
    for line in text.splitlines():
        if line.lstrip().startswith('```'):
            fenced = not fenced
            continue
        if fenced:
            continue
        m = re.match(r'^\s*- \[ \] (.*)', line)
        if current is not None and line.startswith(' ') and line.strip() and not re.match(r'^\s*- ', line):
            current = (current[0], current[1] + ' ' + line.strip())
            continue
        if current is not None:
            yield current
            current = None
        if line.startswith('## '):
            section = line[3:].strip()
        if m:
            current = (section, m.group(1).strip())
    if current is not None:
        yield current


def place(section, item):
    own = re.search(r'\(where: ([a-z ]+)\)', item)
    if own:
        return own.group(1).strip()
    for key, _, rx in PLACES:
        # A Linux section's items need Linux before anything less
        # particular (hardware, hands) (C6 review).
        if key == 'other hardware' and re.search(r'\blinux\b', section, re.I):
            return 'linux'
        if re.search(rx, item, re.I):
            return key
    # A section about one place puts its items there: Linux by name only
    # (a "KDE Connect devices" section runs on the phone), a SIM, a second
    # phone (C6 review).
    if re.search(r'\blinux\b', section, re.I):
        return 'linux'
    for key in ('sim', 'second phone'):
        rx = dict((k, r) for k, _, r in PLACES)[key]
        if re.search(rx, section, re.I):
            return key
    return HERE[0]


def render(text):
    order = [HERE] + [(k, d) for k, d, _ in PLACES]
    known = {k for k, _ in order}
    grouped = {}
    for section, item in items(text):
        key = place(section, item)
        if key not in known:
            # A mistyped `(where: …)` would drop the item from the index.
            raise SystemExit(f'unknown place {key!r} (one of: {", ".join(sorted(known))}): {item}')
        grouped.setdefault(key, []).append((section, item))
    total = sum(len(v) for v in grouped.values())
    out = [
        '# Owed checks, by where they can run',
        '',
        'Generated from `docs/device-verification-checklist.md` by `scripts/owed_checks.py` (C6); '
        'CI fails when it is stale. Each open item is placed by its own words and its section\'s, '
        'the first place that matches; a line can say `(where: <place>)` itself. Tick items in the '
        'checklist, then run the script again.',
        '',
        f'{total} open items.',
        '',
        '| Where | Items |',
        '|---|---|',
    ]
    for key, desc in order:
        out.append(f'| {desc} | {len(grouped.get(key, []))} |')
    for key, desc in order:
        rows = grouped.get(key, [])
        if not rows:
            continue
        out += ['', f'## {desc}', '']
        last = None
        for section, item in rows:
            if section != last:
                out += ['', f'**{section}**', '']
                last = section
            out.append(f'- {item}')
    return '\n'.join(out).replace('\n\n\n', '\n\n') + '\n'


# Lines whose place must not drift (C6 review): above all, nothing the
# owner's rule forbids may be filed under "here".
CASES = [
    ('A UAC prompt or Win+L mid-session.', '', 'power'),
    ('`pocketdesk stop` from a second terminal stops the server cleanly.', '', 'power'),
    ('`install` adds the logon task and the firewall rules.', '', 'power'),
    ('Lock PC / Lock desktop: the screen locks.', '', 'power'),
    ('Typing letters, arrows and Super (the Start menu) on the phone.', '', 'owner'),
    ('Video on a 100 % and on a scaled (125/150 %) display.', '', 'owner'),
    ('An elevated window: input stops reaching it.', '', 'owner'),
    ('A file transfer cut by a drop of the Wi-Fi continues.', '', 'here'),
    ('Watch only: the window shows the phone.', '', 'here'),
    ('Privacy on: the keyboard and mouse on the desktop stop responding.', '', 'owner'),
    ('Pairing, codes matching both ways.', '101. KDE Connect next steps (N9)', 'here'),
    ('The tray icon in a real panel.', '99. The phone panel and a Linux tray (N6)', 'linux'),
    ('A text message forwarded and answered.', '', 'sim'),
    ('"Privacy: on" → the desktop\'s keyboard stops responding.', '', 'owner'),
    ('Swipe typing a word, and voice dictation.', '', 'owner'),
    ('A file dropped on the window is offered.', '', 'by hand'),
    # S1: the Linux host's own tools name their place.
    ('Audio sheet opens with volume %, mute state, sink list (matches `wpctl status`).', '', 'linux'),
    ('Battery chip shows desktop percentage + charging state; matches `upower`.', '', 'linux'),
    ('Monitor picker lists monitors via xcap; selecting index N streams that monitor.', '', 'linux'),
]


def selftest():
    bad = [(item, want, place(section, item)) for item, section, want in CASES if place(section, item) != want]
    for item, want, got in bad:
        print(f'{item!r}: want {want}, got {got}', file=sys.stderr)
    return 1 if bad else 0


def main():
    if '--selftest' in sys.argv:
        return selftest()
    text = CHECKLIST.read_text(encoding='utf-8')
    rendered = render(text)
    if '--check' in sys.argv:
        current = OUT.read_text(encoding='utf-8') if OUT.exists() else ''
        if current != rendered:
            print('docs/owed-checks.md is stale: run `python3 scripts/owed_checks.py`', file=sys.stderr)
            return 1
        return 0
    with open(OUT, 'w', encoding='utf-8', newline='\n') as out:
        out.write(rendered)
    print(f'wrote {OUT.relative_to(ROOT)}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
