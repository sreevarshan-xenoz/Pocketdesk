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
     r'\b(sleep|shut ?down|hibernat\w*|reboot|log ?off|lock(ed|s)? (it|the (desktop|computer|screen|phone))|power (off|button|actions?)|server[- ]stop|stop(ping)? the (installed )?server|install(ed)? task|start/restart|restart (the )?(installed )?server)\b'),
    ('owner', "Needs the owner's OK on this box: input reaching Windows, or its display settings",
     r'\b(inject\w*|super\+|alt\+tab|ctrl\+|pointer|cursor|taps? (map|land|reach|still map)|tap-to-|typ(ed|ing) (into|in)|hotkeys?|keys? (arrive|reach|land)|keyboard (shortcut|input)|mouse moves?|click(s|ed)? (on|into) the desktop|desktop (moves|receives)|drives? the (desktop|computer)|scal(ed|ing) \(?1[25]\d ?%|1[25]\d ?% scal\w*|dpi)\b'),
    ('sim', 'A phone with a SIM', r'\b(sim|sms|mms|text messages?|texts|cellular|mobile (data|service|network))\b'),
    ('second phone', 'A second phone', r'\b(two phones|second phone|another phone|both phones|second android|phone b)\b'),
    ('away', 'Away from home: another network, or through a router', r'\b(outside|away from home|from away|another network|router|port forward\w*|public (ip|network)|relayed path|from the internet|real distance|hotspot)\b'),
    ('second computer', 'A second computer', r'\b(two (real )?computers|second computer|another computer|computer b|other computer)\b'),
    ('linux', 'A Linux desktop (the Azure VM serves for some)',
     r'\b(linux|sway|gnome|kde|plasma|wayland|x11|xwayland|hyprland|wlroots|d-bus|dbus|pipewire|vaapi|nvenc|orca|fc-list|systemd|wev|the vm|azure|waybar|swaybar|libnotify|notify-send|playerctl|ydotool|uinput)\b'),
    ('other hardware', 'Other hardware: a foldable, tablet, mouse, game pad, older or newer Android',
     r'\b(foldable|tablet|gamepad|game ?pad|mouse|hardware keyboard|bluetooth|sd card|android (8|9|1[0-3]|17)|api 3[0-3]|tv|wear ?os|watch|projector|hdr|4k|third monitor)\b'),
    ('by hand', 'By hand or eye on this box: hearing, looking, the tray, drag and drop, a screen reader, a real app',
     r'\b(by ear|ears?|listen\w*|hear\w*|by eye|look(s|ing)?|tray|drag(ging)?|drop(ped|ping)?|narrator|talkback|screen reader|camera|scan(s|ning)?|real (app|chat app|player|panel)|tasker|browser extension|extension loaded|home assistant itself|stopwatch|feel|smooth\w*|on screen|clicks?|the window by hand)\b'),
]
HERE = ('here', 'Here: this Windows box and the A71, without touching the owner\'s power, lock or input')


def items(text):
    section = ''
    for line in text.splitlines():
        if line.startswith('## '):
            section = line[3:].strip()
        m = re.match(r'^\s*- \[ \] (.*)', line)
        if m:
            yield section, m.group(1).strip()


def place(section, item):
    own = re.search(r'\(where: ([a-z ]+)\)', item)
    if own:
        return own.group(1).strip()
    for key, _, rx in PLACES:
        if re.search(rx, item, re.I):
            return key
    # A section about one place (Linux, a SIM) puts its items there.
    for key in ('linux', 'sim', 'second phone'):
        rx = dict((k, r) for k, _, r in PLACES)[key]
        if re.search(rx, section, re.I):
            return key
    return HERE[0]


def render(text):
    grouped = {}
    for section, item in items(text):
        grouped.setdefault(place(section, item), []).append((section, item))
    order = [HERE] + [(k, d) for k, d, _ in PLACES]
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


def main():
    text = CHECKLIST.read_text(encoding='utf-8')
    rendered = render(text)
    if '--check' in sys.argv:
        current = OUT.read_text(encoding='utf-8') if OUT.exists() else ''
        if current != rendered:
            print('docs/owed-checks.md is stale: run `python3 scripts/owed_checks.py`', file=sys.stderr)
            return 1
        return 0
    OUT.write_text(rendered, encoding='utf-8', newline='\n')
    print(f'wrote {OUT.relative_to(ROOT)}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
